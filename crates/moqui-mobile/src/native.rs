/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! What the app's native code asks of Rust without the WebView (plans/Mobile-Desktop.md MD6, MT2): the FCM token to
//! register, which account a ring is for, a ring declined, the one-shot links of an Answer or a Hang up, a headset's
//! hang-up, whether an account is signed in, and a bounded send of the outbox in the background (`FlushWorker.kt`,
//! WorkManager). A push can start the process with no activity, so no
//! Tauri app: the client ([`core_in`]) is then made here from the client's directory, and the app's setup takes the
//! same one when the activity starts, so there is one client, with one set of stores, per process.
//!
//! The JNI entry points (`Java_org_moqui_mobile_Native_*`, `gen/android/.../Native.kt`) are at the end, for Android;
//! each runs its work under `catch_unwind` (jni's `with_env`), so no panic crosses into the JVM.

// The native code's entry points are Android's; their work is tested on the desktop.
#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moqui_client::account::Account;
use moqui_client::app::{Client, ClientHost};
use moqui_client::conf::ClientConf;
use moqui_client::proxy::Request;
use moqui_client_tauri::SHELL_ORIGIN;
use serde_json::Value;

use crate::calls::{self, Asked, CallLink, CallLinks, Declined, Poster, PushRecord};
use crate::host::MobileHost;

/// The process's client, whoever made it first: the app's setup or a push.
static CORE: Mutex<Option<Arc<Client>>> = Mutex::new(None);

/// One change of `push.json` at a time.
static PUSH: Mutex<()> = Mutex::new(());

/// The app's host, once its setup ran: a background send tells its pages what changed.
static APP_HOST: Mutex<Option<Arc<dyn ClientHost>>> = Mutex::new(None);

/// Set once the app ends its client to end the process: no client is made again, so none recovers the outbox's
/// operations that were sending (and sends them a second time) in the moment before the process ends.
static ENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The process is ending (the app's client stopped before its exit).
pub(crate) fn process_ending() {
    ENDING.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// The longest a background send takes operations ([`flush_background`]); WorkManager stops a run at ten minutes.
const FLUSH_BUDGET_MAX: Duration = Duration::from_secs(5 * 60);
/// The call links the native code made and not yet taken.
static LINKS: Mutex<Option<CallLinks>> = Mutex::new(None);

/// The app's host, once the activity's setup made it: what the native code tells the page goes through it.
static HOST: Mutex<Option<Arc<MobileHost>>> = Mutex::new(None);

/// The host made by the app's setup.
pub(crate) fn set_host(host: &Arc<MobileHost>) {
    *moqui_lib::sync::lock(&HOST) = Some(Arc::clone(host));
}

/// The host, when the app's activity has set it up.
pub(crate) fn host() -> Option<Arc<MobileHost>> {
    moqui_lib::sync::lock(&HOST).clone()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
}

/// A link for the native code's Answer or Hang up of the call `id` of `account`: its nonce, or none when the call id
/// is not one.
pub(crate) fn make_link(id: &str, account: &str, video: bool, hang_up: bool) -> Option<String> {
    let id = calls::call_id(id)?;
    let link = CallLink {
        call_session_id: id.to_string(),
        account: account.to_string(),
        video,
        hang_up,
    };
    let mut links = moqui_lib::sync::lock(&LINKS);
    match links.get_or_insert_with(CallLinks::default).make(link, now_ms()) {
        Ok(nonce) => Some(nonce),
        Err(err) => {
            log::error!("Calls: no link for {id}: {err}");
            None
        }
    }
}

/// The link of `nonce` for the call `id`, taken once ([`CallLinks::take`]).
pub(crate) fn take_link(nonce: &str, id: &str) -> Option<CallLink> {
    moqui_lib::sync::lock(&LINKS)
        .get_or_insert_with(CallLinks::default)
        .take(nonce, id, now_ms())
}

/// The account a push with `push_id` is for ([`calls::ring_account`]); none for a push that is no account's here.
pub(crate) fn ring_account(dir: &Path, push_id: &str) -> Option<String> {
    let core = core_in(dir).ok()?;
    let (keyed, current) = {
        let accounts = moqui_lib::sync::lock(core.accounts());
        let current = accounts.current().map(|account| account.id.clone());
        let ids: Vec<String> = accounts.list().iter().map(|account| account.id.clone()).collect();
        (ids, current)
    };
    let keyed: Vec<String> = keyed.into_iter().filter(|id| core.keys().get(id).is_some()).collect();
    let _one = moqui_lib::sync::lock(&PUSH);
    calls::ring_account(&PushRecord::load(dir), push_id, &keyed, current.as_deref())
}

/// Telecom ended an answered call (a headset's or a car's hang-up): the page leaves it.
pub(crate) fn hang_up(id: &str) {
    match (calls::call_id(id), host()) {
        (Some(id), Some(host)) => host.hang_up(id),
        _ => log::info!("Calls: no page to leave the call {id}"),
    }
}

/// The client of `dir` (the app's data directory's `client`): the one this process has, unless it was stopped, else
/// a new one.
pub(crate) fn core_in(dir: &Path) -> Result<Arc<Client>, String> {
    let mut held = moqui_lib::sync::lock(&CORE);
    if let Some(core) = held.as_ref().filter(|core| !core.is_stopped()) {
        return Ok(Arc::clone(core));
    }
    if ENDING.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("the process is ending".to_string());
    }
    let keys = crate::keystore::open_keys(&dir.join("keys"));
    let conf = ClientConf::parse(crate::client_conf_element());
    let core = Arc::new(Client::new(dir, keys, Box::new(move || conf.clone()))?);
    *held = Some(Arc::clone(&core));
    Ok(core)
}

/// Requests may be made: the TLS verifier is set up (Android), else a request would panic.
fn network_ready() -> bool {
    #[cfg(target_os = "android")]
    return crate::android::verifier_ready();
    #[cfg(not(target_os = "android"))]
    true
}

/// The accounts with a key, the current one first, with their comms apps.
pub(crate) fn asked(core: &Client) -> Vec<Asked> {
    let (list, current) = {
        let accounts = moqui_lib::sync::lock(core.accounts());
        let current = accounts.current().map(|account| account.id.clone());
        (accounts.list().to_vec(), current)
    };
    let mut ordered: Vec<Account> = list.clone();
    ordered.sort_by_key(|account| Some(&account.id) != current.as_ref());
    ordered
        .into_iter()
        .filter_map(|account| {
            let key = core.keys().get(&account.id)?;
            let comms_root = core.comms_root_of(&account.id);
            Some(Asked {
                account,
                key,
                comms_root,
            })
        })
        .collect()
}

/// Posts through the client's proxy, as the pages' requests and the outbox's go.
struct ProxyPoster<'a>(&'a Client);

impl Poster for ProxyPoster<'_> {
    fn post(&self, account: &Account, key: &str, path: &str, form: &[(&str, &str)]) -> Result<(u16, Value), String> {
        let mut body = tauri::Url::parse("http://form.invalid/").map_err(|err| err.to_string())?;
        body.query_pairs_mut().extend_pairs(form.iter().copied());
        let request = Request {
            method: "POST".to_string(),
            path: path.to_string(),
            headers: vec![
                ("Accept".to_string(), "application/json".to_string()),
                (
                    "Content-Type".to_string(),
                    "application/x-www-form-urlencoded".to_string(),
                ),
                ("X-Requested-With".to_string(), "XMLHttpRequest".to_string()),
            ],
            body: body.query().unwrap_or("").as_bytes().to_vec(),
        };
        let upstream = self
            .0
            .proxy()
            .send(&account.server, Some(key), SHELL_ORIGIN, request)
            .map_err(|err| err.to_string())?;
        let status = upstream.status;
        let bytes = upstream.body().map_err(|err| err.to_string())?;
        Ok((status, serde_json::from_slice(&bytes).unwrap_or(Value::Null)))
    }
}

/// Decline the ringing call `id` on the server of `account`, the ring's, with no WebView: what happened, for the log.
pub(crate) fn decline_call(dir: &Path, id: &str, account: &str) -> String {
    let Some(id) = calls::call_id(id) else {
        return "not a call id".to_string();
    };
    if !network_ready() {
        return "no requests can be made (the TLS verifier did not start)".to_string();
    }
    let core = match core_in(dir) {
        Ok(core) => core,
        Err(err) => return format!("no client: {err}"),
    };
    let Some(asked) = asked(&core).into_iter().find(|asked| asked.account.id == account) else {
        return format!("the account {account} is not signed in here");
    };
    match calls::decline(&asked, id, &ProxyPoster(&core)) {
        Declined::Told => format!("declined on the server of {account}"),
        Declined::Refused(reason) => format!("the server of {account} did not decline it: {reason}"),
        Declined::Failed(why) => format!("the server of {account} was not told: {why}"),
    }
}

/// A token from Firebase: kept, and registered with each account signed in.
pub(crate) fn push_token(dir: &Path, token: &str) {
    let token = token.trim();
    if token.is_empty() || token.len() > 4096 {
        log::warn!("Push: a token that is not one was passed over");
        return;
    }
    let _one = moqui_lib::sync::lock(&PUSH);
    let mut record = PushRecord::load(dir);
    let mut changed = record.set_token(token);
    if network_ready() {
        match core_in(dir) {
            Ok(core) => changed |= record.register(&asked(&core), &ProxyPoster(&core)),
            Err(err) => log::warn!("Push: no client to register with: {err}"),
        }
    }
    if changed {
        if let Err(err) = record.save(dir) {
            log::error!("Push: the token was not kept: {err}");
        }
    }
}

/// Register the kept token with the accounts it is not registered with (the app's start), or again with `again`
/// (it signed in, with a new key). Nothing without a token.
pub(crate) fn register_push(core: &Client, dir: &Path, again: Option<&str>) {
    if !network_ready() {
        return;
    }
    let _one = moqui_lib::sync::lock(&PUSH);
    let mut record = PushRecord::load(dir);
    if record.token.is_none() {
        return;
    }
    let mut changed = again.is_some_and(|id| record.forget(id));
    changed |= record.register(&asked(core), &ProxyPoster(core));
    if changed {
        if let Err(err) = record.save(dir) {
            log::error!("Push: the registrations were not kept: {err}");
        }
    }
}

/// Take the token back from `account`'s server before its key is revoked.
pub(crate) fn unregister_push(core: &Client, dir: &Path, account: &Account, key: &str) {
    if !network_ready() {
        return;
    }
    let _one = moqui_lib::sync::lock(&PUSH);
    let mut record = PushRecord::load(dir);
    if record.token.is_none() && record.registered.is_empty() {
        return;
    }
    let asked = Asked {
        account: account.clone(),
        key: key.to_string(),
        comms_root: String::new(),
    };
    if record.unregister(&asked, &ProxyPoster(core)) {
        if let Err(err) = record.save(dir) {
            log::error!("Push: the registrations were not kept: {err}");
        }
    }
}

/// The app's host, for the pages of a background send ([`flush_background`]).
pub(crate) fn set_app_host(host: Arc<dyn ClientHost>) {
    *moqui_lib::sync::lock(&APP_HOST) = Some(host);
}

/// The host of a background send with no app (WorkManager started the process): no page to tell, and a 401 holds
/// the operation, as for the app, until a sign-in or a renewed key releases it.
struct NoPages;

impl ClientHost for NoPages {
    fn page_account(&self, _page: &str) -> Option<String> {
        None
    }

    fn is_shell(&self, _page: &str) -> bool {
        false
    }

    fn notify_pages(&self, _account: &str, _event: &serde_json::Value) {}

    fn socket_event(&self, _page: &str, _id: u64, _event: &serde_json::Value) {}

    fn check_key(&self, account: &Account) {
        log::info!(
            "Outbox: the server of {} refused its key; the app checks it when it starts",
            account.id
        );
    }
}

/// Send what the outbox has due, taking nothing after `budget_ms` (at most [`FLUSH_BUDGET_MAX`]): the background run
/// of `FlushWorker.kt`, with the app asleep or its process started for it. Gives the milliseconds until the next
/// operation waiting is due, or -1 when none waits for a time (nothing left, or only held or refused ones). In a
/// process that is ending it sends nothing and gives 0, so the job runs again, in a process of its own.
pub(crate) fn flush_background(dir: &Path, budget_ms: i64) -> i64 {
    if ENDING.load(std::sync::atomic::Ordering::SeqCst) {
        return 0;
    }
    if !network_ready() {
        return -1;
    }
    let core = match core_in(dir) {
        Ok(core) => core,
        Err(err) => {
            log::warn!("Outbox: no client for the background send: {err}");
            return -1;
        }
    };
    let host = moqui_lib::sync::lock(&APP_HOST)
        .clone()
        .unwrap_or_else(|| Arc::new(NoPages));
    let budget = Duration::from_millis(u64::try_from(budget_ms).unwrap_or(0)).min(FLUSH_BUDGET_MAX);
    let next = moqui_client::sync::flush::flush_bounded(&core, host.as_ref(), budget);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX));
    next.map_or(-1, |due| due.saturating_sub(now).max(0))
}

/// Whether an account here has a key: the native code asks for the notification permission only then.
pub(crate) fn has_account(dir: &Path) -> bool {
    match core_in(dir) {
        Ok(core) => core.current().is_some_and(|(_, key)| key.is_some()),
        Err(_) => false,
    }
}

#[cfg(target_os = "android")]
mod entries {
    //! `org.moqui.mobile.Native`'s methods. Each is called on a thread of the native code's own (never the main
    //! thread for one that makes requests); errors and panics are logged and the default answered.

    use std::path::PathBuf;

    use jni::errors::LogErrorAndDefault;
    use jni::objects::{JClass, JObject, JString};
    use jni::sys::{jboolean, jlong};
    use jni::EnvUnowned;

    /// `Native.init(context)`: the app's context for Rust's JNI calls and the TLS verifier; whether requests may be made.
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_init<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        context: JObject<'caller>,
    ) -> jboolean {
        crate::init_logging();
        env.with_env(|env| -> Result<bool, jni::errors::Error> { Ok(crate::android::init_from_native(env, &context)) })
            .resolve::<LogErrorAndDefault>()
    }

    /// `Native.pushToken(dataDir, token)`.
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_pushToken<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        data_dir: JString<'caller>,
        token: JString<'caller>,
    ) {
        env.with_env(|env| -> Result<(), jni::errors::Error> {
            let dir = client_dir(&data_dir.try_to_string(env)?);
            let token = token.try_to_string(env)?;
            super::push_token(&dir, &token);
            Ok(())
        })
        .resolve::<LogErrorAndDefault>();
    }

    /// `Native.decline(dataDir, callSessionId, account)`: what happened.
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_decline<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        data_dir: JString<'caller>,
        call_session_id: JString<'caller>,
        account: JString<'caller>,
    ) -> JString<'caller> {
        env.with_env(|env| -> Result<JString<'caller>, jni::errors::Error> {
            let dir = client_dir(&data_dir.try_to_string(env)?);
            let id = call_session_id.try_to_string(env)?;
            let account = account.try_to_string(env)?;
            let done = super::decline_call(&dir, &id, &account);
            log::info!("Calls: the ring of {id}: {done}");
            env.new_string(done)
        })
        .resolve::<LogErrorAndDefault>()
    }

    /// `Native.ringAccount(dataDir, pushId)`: the account a push is for, or null.
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_ringAccount<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        data_dir: JString<'caller>,
        push_id: JString<'caller>,
    ) -> JString<'caller> {
        env.with_env(|env| -> Result<JString<'caller>, jni::errors::Error> {
            let dir = client_dir(&data_dir.try_to_string(env)?);
            let push_id = push_id.try_to_string(env)?;
            match super::ring_account(&dir, &push_id) {
                Some(account) => env.new_string(account),
                None => Ok(JString::default()),
            }
        })
        .resolve::<LogErrorAndDefault>()
    }

    /// `Native.callLink(callSessionId, account, video, hangUp)`: the nonce of a link the native code makes, or null.
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_callLink<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        call_session_id: JString<'caller>,
        account: JString<'caller>,
        video: jboolean,
        hang_up: jboolean,
    ) -> JString<'caller> {
        env.with_env(|env| -> Result<JString<'caller>, jni::errors::Error> {
            let id = call_session_id.try_to_string(env)?;
            let account = account.try_to_string(env)?;
            match super::make_link(&id, &account, video, hang_up) {
                Some(nonce) => env.new_string(nonce),
                None => Ok(JString::default()),
            }
        })
        .resolve::<LogErrorAndDefault>()
    }

    /// `Native.hangUp(callSessionId)`: Telecom ended an answered call.
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_hangUp<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        call_session_id: JString<'caller>,
    ) {
        env.with_env(|env| -> Result<(), jni::errors::Error> {
            super::hang_up(&call_session_id.try_to_string(env)?);
            Ok(())
        })
        .resolve::<LogErrorAndDefault>();
    }

    /// `Native.hasAccount(dataDir)`.
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_hasAccount<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        data_dir: JString<'caller>,
    ) -> jboolean {
        env.with_env(|env| -> Result<bool, jni::errors::Error> {
            Ok(super::has_account(&client_dir(&data_dir.try_to_string(env)?)))
        })
        .resolve::<LogErrorAndDefault>()
    }

    /// `Native.flush(dataDir, budgetMs)`: the background send; the milliseconds until the next operation is due, or -1.
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_flush<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        data_dir: JString<'caller>,
        budget_ms: jlong,
    ) -> jlong {
        // -1 (nothing to run again for) on an error or a panic, not the policy's default 0, which is "due now"
        env.with_env(|env| -> Result<i64, jni::errors::Error> {
            let dir = match data_dir.try_to_string(env) {
                Ok(dir) => client_dir(&dir),
                Err(err) => {
                    log::error!("Outbox: the background send's directory was not read: {err}");
                    return Ok(-1);
                }
            };
            Ok(
                std::panic::catch_unwind(|| super::flush_background(&dir, budget_ms)).unwrap_or_else(|_| {
                    log::error!("Outbox: the background send panicked");
                    -1
                }),
            )
        })
        .resolve::<LogErrorAndDefault>()
    }

    /// The client's directory under the app's data directory, as the app's setup has it (`app_data_dir()/client`).
    fn client_dir(data_dir: &str) -> PathBuf {
        PathBuf::from(data_dir).join("client")
    }
}

#[cfg(test)]
#[path = "native_tests.rs"]
mod tests;
