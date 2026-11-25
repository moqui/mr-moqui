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
//! register, which account a ring is for, a ring declined, the one-shot links of an Answer, a Hang up or a number from
//! the phone's dialer, a headset's hang-up, whether an account is signed in, and a bounded send of the outbox in the
//! background (`FlushWorker.kt`, WorkManager). A push can start the process with no activity, so no Tauri app: the client ([`core_in`]) is then made here from the client's directory, and the app's setup takes the
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

pub(crate) fn now_ms() -> i64 {
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

/// The numbers the phone's dialer handed over and not yet taken (plans/Comms-Messaging.md §9.1).
static DIALS: Mutex<Option<crate::dial::DialLinks>> = Mutex::new(None);

/// A number the phone's dialer handed the app (`Native.dialLink`): the nonce of its link, or none for no number.
pub(crate) fn make_dial_link(number: &str) -> Option<String> {
    match moqui_lib::sync::lock(&DIALS)
        .get_or_insert_with(Default::default)
        .make(number, now_ms())
    {
        Ok(nonce) => Some(nonce),
        Err(err) => {
            log::warn!("Calls: a number from the phone's dialer is not taken: {err}");
            None
        }
    }
}

/// The number of a dial link's `nonce`, taken once.
pub(crate) fn take_dial_link(nonce: &str) -> Option<String> {
    moqui_lib::sync::lock(&DIALS)
        .get_or_insert_with(Default::default)
        .take(nonce, now_ms())
}

/// The accounts signing out now (`unregister_push` until they are gone): no push keys are made for them and UnifiedPush
/// registers no instance of theirs, though their key is still there for a moment.
static LEAVING: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The accounts here with a key (signed in), not one signing out.
fn keyed_ids(core: &Client) -> Vec<String> {
    let ids: Vec<String> = {
        let accounts = moqui_lib::sync::lock(core.accounts());
        accounts.list().iter().map(|account| account.id.clone()).collect()
    };
    let leaving = moqui_lib::sync::lock(&LEAVING).clone();
    ids.into_iter()
        .filter(|id| !leaving.contains(id) && core.keys().get(id).is_some())
        .collect()
}

/// The accounts here with a key, not one signing out (the account a number from the phone's dialer is called from).
pub(crate) fn signed_in_ids(core: &Client) -> Vec<String> {
    keyed_ids(core)
}

/// An account signed in (again): it is no longer leaving.
pub(crate) fn account_back(account: &str) {
    moqui_lib::sync::lock(&LEAVING).retain(|leaving| leaving != account);
}

/// Mark `account` as signing out ([`LEAVING`]).
pub(crate) fn account_leaving(account: &str) {
    let mut leaving = moqui_lib::sync::lock(&LEAVING);
    if !leaving.iter().any(|id| id == account) {
        leaving.push(account.to_string());
    }
}

/// Whether a push of `push_id` names an account here (a sealed ring that did not open is then shown as a missed call).
pub(crate) fn push_known(dir: &Path, push_id: &str) -> bool {
    let Ok(core) = core_in(dir) else {
        return false;
    };
    !calls::push_accounts(&load_record(dir), push_id, &keyed_ids(&core)).is_empty()
}

/// The push record, read under the lock.
fn load_record(dir: &Path) -> PushRecord {
    let _one = moqui_lib::sync::lock(&PUSH);
    PushRecord::load(dir)
}

/// The account an unsealed ring is for ([`calls::ring_account`]): the one account its `push_id` can be; none for a
/// push that is no account's here, or that two accounts could be.
pub(crate) fn ring_account(dir: &Path, push_id: &str) -> Option<String> {
    let core = core_in(dir).ok()?;
    let keyed = keyed_ids(&core);
    calls::ring_account(&load_record(dir), push_id, &keyed)
}

/// The native call tells the shell's call screen something (`Native.callTell`: audio, pip, hold).
pub(crate) fn tell_page(what: &str, json: &str) {
    match (calls::tell_script(what, json), host()) {
        (Some(script), Some(host)) => host.tell_call(&script),
        (None, _) => log::warn!("Calls: the page is not told {what}: not one"),
        (Some(_), None) => {}
    }
}

/// Telecom ended an answered call (a headset's or a car's hang-up): the page leaves it.
pub(crate) fn hang_up(id: &str) {
    match (calls::call_id(id), host()) {
        (Some(id), Some(host)) => host.hang_up(id),
        _ => log::info!("Calls: no page to leave the call {id}"),
    }
}

/// The app's screen went (`MainActivity.onStop`: the phone locked, or the app left for another): every vault session
/// locks (plans/Vault-Instances.md §3.6, V5); a key the user had the app remember opens again with the lock or biometrics.
pub(crate) fn vault_lock() {
    if let Some(core) = moqui_lib::sync::lock(&CORE).as_ref() {
        core.vault().lock_all();
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
            if moqui_lib::sync::lock(&LEAVING).contains(&account.id) {
                return None;
            }
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

/// Each account's push keys on `record` (made at its first registration, sealed as the device keys are; `again`, an
/// account that just signed in, may have new ones made if its kept ones cannot be read): new ones register again.
fn with_push_keys(core: &Client, record: &mut PushRecord, again: Option<&str>) -> bool {
    // the one pair of every account an earlier build kept: each account has its own now
    if core.keys().get(crate::push::OLD_KEY_ENTRY).is_some() {
        if let Err(err) = core.keys().delete(crate::push::OLD_KEY_ENTRY) {
            log::warn!("Push: the earlier build's push keys were not deleted: {err}");
        }
    }
    let mut changed = false;
    for id in keyed_ids(core) {
        let registered = if again == Some(id.as_str()) {
            None
        } else {
            record.keys.get(&id).map(|(public, _)| public.as_str())
        };
        match crate::push::keys_for(core.keys(), &id, registered) {
            Ok(keys) => changed |= record.set_keys(&id, &keys.public_b64(), &keys.auth_b64()),
            // tried again at the next registration; the registration made with them stays
            Err(err) => log::error!("Push: no push keys for {id}: {err}"),
        }
    }
    changed
}

/// Change the push record under the lock, keeping it when `change` says it changed.
fn change_record(dir: &Path, change: impl FnOnce(&mut PushRecord) -> bool) {
    let _one = moqui_lib::sync::lock(&PUSH);
    let mut record = PushRecord::load(dir);
    if change(&mut record) {
        if let Err(err) = record.save(dir) {
            log::error!("Push: the push record was not kept: {err}");
        }
    }
}

/// A token from Firebase: kept, and registered with each account signed in.
pub(crate) fn push_token(dir: &Path, token: &str) {
    let token = token.trim();
    if token.is_empty() || token.len() > 4096 {
        log::warn!("Push: a token that is not one was passed over");
        return;
    }
    change_record(dir, |record| record.set_token(token));
    register_pending(dir);
}

/// `account`'s UnifiedPush distributor endpoint (none: it was unregistered), from the connector
/// (`UnifiedPushService.kt`, which finds the account of the connector's instance with [`account_of`]): kept, and
/// registered with the account's server. An endpoint for an account with no instance here (it signed out since) is
/// not taken.
pub(crate) fn unified_endpoint(dir: &Path, account: &str, endpoint: Option<&str>) {
    let Some(account) = calls::account_id(account) else {
        log::warn!("Push: a UnifiedPush endpoint of no account was passed over");
        return;
    };
    let endpoint = endpoint.map(str::trim).filter(|endpoint| !endpoint.is_empty());
    if endpoint.is_some_and(|endpoint| endpoint.len() > 4096 || !endpoint.starts_with("https://")) {
        log::warn!("Push: a UnifiedPush endpoint that is not https was passed over");
        return;
    }
    change_record(dir, |record| {
        if endpoint.is_some() && !record.instances.contains_key(account) {
            log::info!("Push: a UnifiedPush endpoint of {account}, which has no instance here, was passed over");
            return false;
        }
        record.set_endpoint(account, endpoint)
    });
    register_pending(dir);
}

/// `account`'s UnifiedPush instance (`Native.instanceOf`, `UnifiedPushes.registerAll`), made and kept in `push.json`
/// the first time it is asked for: none for an account not signed in here (one signing out gets none again) or when
/// a new one could not be kept.
pub(crate) fn instance_of(dir: &Path, account: &str) -> Option<String> {
    let core = core_in(dir).ok()?;
    // under the lock, as the sign-out's drop of the instance is: an account marked leaving before it gets none
    let _one = moqui_lib::sync::lock(&PUSH);
    if !keyed_ids(&core).iter().any(|id| id == account) {
        return None;
    }
    let mut record = PushRecord::load(dir);
    let (instance, made) = match record.instance_of(account) {
        Ok(found) => found,
        Err(err) => {
            log::error!("Push: no UnifiedPush instance for {account}: {err}");
            return None;
        }
    };
    if made {
        // one not kept would be another at the next ask, and the distributor's endpoint for it no account's
        if let Err(err) = record.save(dir) {
            log::error!("Push: the UnifiedPush instance of {account} was not kept: {err}");
            return None;
        }
    }
    Some(instance)
}

/// The account of the UnifiedPush `instance` (`Native.accountOf`: the connector's events and its key manager); none
/// for an instance not made here, an account id among them (an earlier build's instance, taken back at the next
/// `registerAll`).
pub(crate) fn account_of(dir: &Path, instance: &str) -> Option<String> {
    load_record(dir).account_of(instance).map(str::to_string)
}

/// Register each account signed in that is not registered yet (the app's start, a token, an endpoint), or again with
/// `again` (it signed in, with a new key). The posts go with no lock held, on a copy whose outcome is merged after.
/// Whether every account signed in has a registration now (the native code tries again until then:
/// `RegisterWorker.kt`).
pub(crate) fn register_push(core: &Client, dir: &Path, again: Option<&str>) -> bool {
    if !network_ready() {
        return false;
    }
    let (before, accounts) = {
        let _one = moqui_lib::sync::lock(&PUSH);
        let mut record = PushRecord::load(dir);
        let mut changed = again.is_some_and(|id| record.forget(id));
        changed |= with_push_keys(core, &mut record, again);
        if changed {
            if let Err(err) = record.save(dir) {
                log::error!("Push: the push record was not kept: {err}");
            }
        }
        (record, asked(core))
    };
    if !before.pending(&accounts) {
        return true;
    }
    let mut done = before.clone();
    if !done.register(&accounts, &ProxyPoster(core)) {
        return false;
    }
    let _one = moqui_lib::sync::lock(&PUSH);
    let mut record = PushRecord::load(dir);
    if record.merge(&before, &done) {
        if let Err(err) = record.save(dir) {
            log::error!("Push: the registrations were not kept: {err}");
        }
    }
    !record.pending(&accounts)
}

/// [`register_push`], and when an account is still not registered, the native code's retry worker scheduled
/// (`RegisterWorker.kt`, only then: a worker scheduled at every start would start the process again after it ended).
pub(crate) fn register_or_retry(core: &Client, dir: &Path, again: Option<&str>) {
    if register_push(core, dir, again) {
        return;
    }
    #[cfg(target_os = "android")]
    if let Err(err) = crate::android::register_worker_start() {
        log::warn!("Push: the registration's retry was not scheduled: {err}");
    }
}

/// [`register_push`] from the native code (its retry worker), with no activity: whether it is done.
pub(crate) fn register_pending(dir: &Path) -> bool {
    match core_in(dir) {
        Ok(core) => register_push(&core, dir, None),
        Err(err) => {
            log::warn!("Push: no client to register with: {err}");
            false
        }
    }
}

/// Open an FCM push of the registration `push_id` (`enc`, sealed to an account's push keys; `kind`, its outer kind,
/// which a sealed notification keeps only outside): its JSON with `account`, the account whose keys opened it, and its
/// `kind` and `pushId`, as a text for the native code. The accounts registered with `push_id` are tried, and only the
/// one whose keys open it is the push's (a server cannot make a push open as another's); none for a push that is no
/// account's here or does not open (logged). Keys are only read here, never made.
pub(crate) fn open_push(dir: &Path, push_id: &str, kind: &str, enc: &str) -> Option<String> {
    let core = core_in(dir).ok()?;
    let keyed = keyed_ids(&core);
    let accounts = calls::push_accounts(&load_record(dir), push_id, &keyed);
    if accounts.is_empty() {
        log::warn!("Push: a push of {push_id} is no account's here; dropped");
        return None;
    }
    let keyed: Vec<(String, Option<crate::push::PushKeys>)> = accounts
        .into_iter()
        .map(|account| {
            let keys = crate::push::kept_keys(core.keys(), &account);
            if keys.is_none() {
                log::warn!("Push: the push keys of {account} cannot be read now");
            }
            (account, keys)
        })
        .collect();
    match open_with(&keyed, enc) {
        Some((account, opened)) => Some(opened_for(opened, &account, kind, push_id).to_string()),
        None => {
            log::warn!("Push: a push of {push_id} opened with no account's keys here; dropped");
            None
        }
    }
}

/// The account of `candidates` whose keys open `enc`, and what it opened to: the keys prove which account's server
/// sent it.
pub(crate) fn open_with(candidates: &[(String, Option<crate::push::PushKeys>)], enc: &str) -> Option<(String, Value)> {
    candidates.iter().find_map(|(account, keys)| {
        let opened = keys.as_ref()?.open_b64(enc).ok()?;
        Some((account.clone(), opened))
    })
}

/// An opened push with the account it is for, and its outer `kind` and `pushId` where it has none of its own.
pub(crate) fn opened_for(mut opened: Value, account: &str, kind: &str, push_id: &str) -> Value {
    if opened.get("kind").and_then(Value::as_str).is_none_or(str::is_empty) && !kind.is_empty() {
        opened["kind"] = Value::String(kind.to_string());
    }
    if opened.get("pushId").is_none() && !push_id.is_empty() {
        opened["pushId"] = Value::String(push_id.to_string());
    }
    opened["account"] = Value::String(account.to_string());
    opened
}

/// The UnifiedPush connector's key manager (`RustKeyManager.kt`): open a push body to `account` (the account of the
/// connector's instance, [`account_of`]) with its keys; none for an account not signed in here or a body that does not
/// open. Only reads keys.
pub(crate) fn open_for(dir: &Path, account: &str, body: &[u8]) -> Option<Vec<u8>> {
    let core = core_in(dir).ok()?;
    if !keyed_ids(&core).iter().any(|id| id == account) {
        log::warn!("Push: a UnifiedPush message for {account}, not signed in here; dropped");
        return None;
    }
    let keys = crate::push::kept_keys(core.keys(), account)?;
    match keys.open(body) {
        Ok(plain) => Some(plain),
        Err(err) => {
            log::warn!("Push: a UnifiedPush message for {account} did not open: {err}");
            None
        }
    }
}

/// The connector's key manager asks for `account`'s public key and auth secret (`make`: make them if it has none, as
/// at a registration): `<public>.<auth>`, base64url; none for an account not signed in here.
pub(crate) fn push_public(dir: &Path, account: &str, make: bool) -> Option<String> {
    let core = core_in(dir).ok()?;
    if !keyed_ids(&core).iter().any(|id| id == account) {
        return None;
    }
    let keys = if make {
        let _one = moqui_lib::sync::lock(&PUSH);
        let record = PushRecord::load(dir);
        let registered = record.keys.get(account).map(|(public, _)| public.clone());
        match crate::push::keys_for(core.keys(), account, registered.as_deref()) {
            Ok(keys) => keys,
            Err(err) => {
                log::error!("Push: no push keys for {account}: {err}");
                return None;
            }
        }
    } else {
        crate::push::kept_keys(core.keys(), account)?
    };
    Some(format!("{}.{}", keys.public_b64(), keys.auth_b64()))
}

/// The accounts signed in here, one id a line: the UnifiedPush connector registers each one's instance
/// ([`instance_of`]).
pub(crate) fn account_ids(dir: &Path) -> String {
    core_in(dir).map(|core| keyed_ids(&core).join("\n")).unwrap_or_default()
}

/// The app's key for its own links (`calls::room_link`, and a page notification's tap, `commands::notify`), made and
/// kept in the key store on first use. A key that cannot be read now is made again: the links already shown then open
/// nothing, and nothing else.
pub(crate) fn link_key(core: &Client) -> Option<Vec<u8>> {
    const ENTRY: &str = "app.link-key";
    if let Some(key) = core.keys().get(ENTRY).and_then(|text| crate::push::unb64(&text).ok()) {
        if key.len() == 32 {
            return Some(key);
        }
    }
    let mut key = vec![0_u8; 32];
    getrandom::fill(&mut key).ok()?;
    use base64::Engine;
    let text = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&key);
    if let Err(err) = core.keys().set(ENTRY, &text) {
        log::error!("The app's link key was not kept: {err}");
        return None;
    }
    Some(key)
}

/// A chat notification's tap link (`ChatNotifications.kt`): the room of `account` at `seq`, signed with the app's link
/// key; none for an id that is not one.
pub(crate) fn room_link(dir: &Path, account: &str, room: &str, seq: i64) -> Option<String> {
    let core = core_in(dir).ok()?;
    calls::room_link(&link_key(&core)?, account, room, Some(seq))
}

/// The account, room and message of a room link this app made (its signature checks), for the app's `Opened` event.
pub(crate) fn room_link_taken(core: &Client, url: &tauri::Url) -> Option<(String, String, Option<i64>)> {
    calls::room_link_of(url, &link_key(core)?)
}

/// The chat send transition of the comms app at `root`, as WebrootChat.js posts it: `/qapps/BasicComms` →
/// `/apps/BasicComms/Chat/send`.
pub(crate) fn chat_send_url(root: &str) -> String {
    let root = root.trim_end_matches('/');
    let rest = root.strip_prefix("/qapps").unwrap_or(root);
    format!("/apps{rest}/Chat/send")
}

/// `text` as the safe HTML of a chat message: escaped, a paragraph, its line breaks kept.
pub(crate) fn reply_html(text: &str) -> String {
    let mut out = String::from("<p>");
    for c in text.trim().chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '\n' => out.push_str("<br>"),
            '\r' => {}
            other => out.push(other),
        }
    }
    out.push_str("</p>");
    out
}

/// Queue an operation of `kind` for `account` in its outbox, as a page would (`moqui_client::sync::outbox`), and wake
/// the app's sender: the notification's Reply and Mark read (plans/Comms-Messaging.md §7.4, Actions), sent with no
/// WebView, offline too (the native code also asks `FlushWorker.kt` to send).
fn queue_op(dir: &Path, account: &str, kind: &str, payload: &Value) -> Result<(), String> {
    let core = core_in(dir)?;
    if core.keys().get(account).is_none() {
        return Err(format!("the account {account} is not signed in here"));
    }
    let server = core.server_sync_of(account);
    let now = moqui_client::sync::outbox::now_ms();
    let op = moqui_client::sync::outbox::check_with(kind, payload, &[], now, &server)?;
    core.with_store(account, |store| moqui_client::sync::outbox::add_kept(store, op))
        .ok_or_else(|| format!("no store for {account}"))??;
    core.flusher().wake();
    if let Some(host) = moqui_lib::sync::lock(&APP_HOST).clone() {
        host.notify_pages(account, &serde_json::json!({ "kind": "changed" }));
    }
    Ok(())
}

/// The notification's Reply: `text` to the room, queued in `account`'s outbox.
pub(crate) fn chat_reply(dir: &Path, account: &str, room: &str, text: &str) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("an empty reply".to_string());
    }
    let core = core_in(dir)?;
    let client_ref = moqui_client::store::new_client_ref().map_err(|err| err.to_string())?;
    let url = chat_send_url(&core.comms_root_of(account));
    let payload = serde_json::json!({
        "clientRef": client_ref, "chatRoomId": room, "url": url,
        "params": { "body": reply_html(text) }, "text": text,
        "createdDate": moqui_client::sync::outbox::now_ms(),
    });
    queue_op(dir, account, "chat-send", &payload)
}

/// The notification's Mark read: the room read up to `seq`, queued in `account`'s outbox.
pub(crate) fn chat_read(dir: &Path, account: &str, room: &str, seq: i64) -> Result<(), String> {
    queue_op(
        dir,
        account,
        "chat-read",
        &serde_json::json!({ "chatRoomId": room, "lastReadEventSeq": seq }),
    )
}

/// Tell `account`'s server the app showed the push `ack_id` (`/qapps/ackClientPush`, System, Push Health); a failure is
/// logged, and the push is shown all the same.
pub(crate) fn ack_push(dir: &Path, account: &str, ack_id: &str) {
    if ack_id.is_empty() || !network_ready() {
        return;
    }
    let Ok(core) = core_in(dir) else {
        return;
    };
    let Some(asked) = asked(&core).into_iter().find(|asked| asked.account.id == account) else {
        return;
    };
    match ProxyPoster(&core).post(&asked.account, &asked.key, "/qapps/ackClientPush", &[("ackId", ack_id)]) {
        Ok((status, _)) if status < 400 => {}
        Ok((status, _)) => log::info!("Push: {account} did not take the acknowledgement: HTTP {status}"),
        Err(err) => log::info!("Push: the acknowledgement did not reach {account}: {err}"),
    }
}

/// The check screen's test (plans/Comms-Messaging.md §7.6): `account`'s server pushes a test to this device through
/// the full path (`/qapps/testClientPush`). Its answer: `{ackIdList, queuedDate, reasonCode}`.
pub(crate) fn test_push(core: &Client, account: &str) -> Result<Value, String> {
    let asked = asked(core)
        .into_iter()
        .find(|asked| asked.account.id == account)
        .ok_or_else(|| format!("the account {account} is not signed in here"))?;
    match ProxyPoster(core).post(&asked.account, &asked.key, "/qapps/testClientPush", &[])? {
        (status, json) if status < 400 => Ok(json),
        (status, _) => Err(format!("HTTP {status}")),
    }
}

/// Whether the push token is registered with `account`'s server (the check screen).
pub(crate) fn push_registered(dir: &Path, account: &str) -> bool {
    let _one = moqui_lib::sync::lock(&PUSH);
    PushRecord::load(dir).registered.contains_key(account)
}

/// Take the registration back from `account`'s server before its key is revoked (the post with no lock held), and
/// forget the account's registration, push keys and endpoint here.
pub(crate) fn unregister_push(core: &Client, dir: &Path, account: &Account, key: &str) {
    // no keys are made for it again, and UnifiedPush registers no instance of it, while it goes
    account_leaving(&account.id);
    let mut copy = load_record(dir);
    if network_ready() && copy.registered.contains_key(&account.id) {
        let asked = Asked {
            account: account.clone(),
            key: key.to_string(),
            comms_root: String::new(),
        };
        copy.unregister(&asked, &ProxyPoster(core));
    }
    change_record(dir, |record| record.drop_account(&account.id));
    crate::push::forget_keys(core.keys(), &account.id);
}

/// `account` is signed out here, its key gone, whether its server heard or not (`MobileHost::account_signed_out`): its
/// registration, push keys, endpoint and UnifiedPush instance are forgotten here with no request, as
/// [`unregister_push`] does after its post (none when the app could not make requests). With no account left signed
/// in, the FCM token goes too ([`PushRecord::forget_token`], SEC-CLI-121), which Firebase then deletes. Whether no
/// account is left signed in.
pub(crate) fn push_signed_out(core: &Client, dir: &Path, account: &str) -> bool {
    let last = keyed_ids(core).is_empty();
    change_record(dir, |record| {
        let dropped = record.drop_account(account);
        let forgot = last && record.forget_token();
        dropped || forgot
    });
    crate::push::forget_keys(core.keys(), account);
    last
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
    use jni::objects::{JByteArray, JClass, JObject, JString};
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

    /// `Native.dialLink(number)`: the nonce of the link of a number the phone's dialer handed over, or null.
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_dialLink<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        number: JString<'caller>,
    ) -> JString<'caller> {
        env.with_env(|env| -> Result<JString<'caller>, jni::errors::Error> {
            match super::make_dial_link(&number.try_to_string(env)?) {
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

    /// `Native.vaultLock()`: the app's screen went; the vault sessions lock.
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_vaultLock<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
    ) {
        env.with_env(|_env| -> Result<(), jni::errors::Error> {
            super::vault_lock();
            Ok(())
        })
        .resolve::<LogErrorAndDefault>();
    }

    /// `Native.callTell(what, json)`: the native call tells the shell's call screen something.
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_callTell<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        what: JString<'caller>,
        json: JString<'caller>,
    ) {
        env.with_env(|env| -> Result<(), jni::errors::Error> {
            super::tell_page(&what.try_to_string(env)?, &json.try_to_string(env)?);
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

    /// `Native.unifiedEndpoint(dataDir, account, endpoint)`: an account's UnifiedPush endpoint (empty: unregistered).
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_unifiedEndpoint<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        data_dir: JString<'caller>,
        account: JString<'caller>,
        endpoint: JString<'caller>,
    ) {
        env.with_env(|env| -> Result<(), jni::errors::Error> {
            let dir = client_dir(&data_dir.try_to_string(env)?);
            let account = account.try_to_string(env)?;
            let endpoint = endpoint.try_to_string(env)?;
            super::unified_endpoint(&dir, &account, Some(endpoint.as_str()).filter(|e| !e.is_empty()));
            Ok(())
        })
        .resolve::<LogErrorAndDefault>();
    }

    /// `Native.openFor(dataDir, account, body)`: a UnifiedPush body opened with the account's keys, or null.
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_openFor<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        data_dir: JString<'caller>,
        account: JString<'caller>,
        body: JByteArray<'caller>,
    ) -> JByteArray<'caller> {
        env.with_env(|env| -> Result<JByteArray<'caller>, jni::errors::Error> {
            let dir = client_dir(&data_dir.try_to_string(env)?);
            let account = account.try_to_string(env)?;
            let body = env.convert_byte_array(&body)?;
            match super::open_for(&dir, &account, &body) {
                Some(plain) => env.byte_array_from_slice(&plain),
                None => Ok(JByteArray::default()),
            }
        })
        .resolve::<LogErrorAndDefault>()
    }

    /// `Native.pushPublic(dataDir, account, make)`: the account's `<public>.<auth>`, or null.
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_pushPublic<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        data_dir: JString<'caller>,
        account: JString<'caller>,
        make: jboolean,
    ) -> JString<'caller> {
        env.with_env(|env| -> Result<JString<'caller>, jni::errors::Error> {
            let dir = client_dir(&data_dir.try_to_string(env)?);
            let account = account.try_to_string(env)?;
            match super::push_public(&dir, &account, make) {
                Some(public) => env.new_string(public),
                None => Ok(JString::default()),
            }
        })
        .resolve::<LogErrorAndDefault>()
    }

    /// `Native.instanceOf(dataDir, account)`: the account's UnifiedPush instance (made the first time), or null.
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_instanceOf<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        data_dir: JString<'caller>,
        account: JString<'caller>,
    ) -> JString<'caller> {
        env.with_env(|env| -> Result<JString<'caller>, jni::errors::Error> {
            let dir = client_dir(&data_dir.try_to_string(env)?);
            let account = account.try_to_string(env)?;
            match super::instance_of(&dir, &account) {
                Some(instance) => env.new_string(instance),
                None => Ok(JString::default()),
            }
        })
        .resolve::<LogErrorAndDefault>()
    }

    /// `Native.accountOf(dataDir, instance)`: the account of a UnifiedPush instance made here, or null.
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_accountOf<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        data_dir: JString<'caller>,
        instance: JString<'caller>,
    ) -> JString<'caller> {
        env.with_env(|env| -> Result<JString<'caller>, jni::errors::Error> {
            let dir = client_dir(&data_dir.try_to_string(env)?);
            let instance = instance.try_to_string(env)?;
            match super::account_of(&dir, &instance) {
                Some(account) => env.new_string(account),
                None => Ok(JString::default()),
            }
        })
        .resolve::<LogErrorAndDefault>()
    }

    /// `Native.pushKnown(dataDir, pushId)`: whether a push of that registration names an account here.
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_pushKnown<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        data_dir: JString<'caller>,
        push_id: JString<'caller>,
    ) -> jboolean {
        env.with_env(|env| -> Result<bool, jni::errors::Error> {
            let dir = client_dir(&data_dir.try_to_string(env)?);
            Ok(super::push_known(&dir, &push_id.try_to_string(env)?))
        })
        .resolve::<LogErrorAndDefault>()
    }

    /// `Native.accountIds(dataDir)`: the accounts signed in here, one a line.
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_accountIds<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        data_dir: JString<'caller>,
    ) -> JString<'caller> {
        env.with_env(|env| -> Result<JString<'caller>, jni::errors::Error> {
            let dir = client_dir(&data_dir.try_to_string(env)?);
            env.new_string(super::account_ids(&dir))
        })
        .resolve::<LogErrorAndDefault>()
    }

    /// `Native.roomLink(dataDir, account, chatRoomId, eventSeq)`: a notification's signed tap link, or null.
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_roomLink<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        data_dir: JString<'caller>,
        account: JString<'caller>,
        room: JString<'caller>,
        seq: jlong,
    ) -> JString<'caller> {
        env.with_env(|env| -> Result<JString<'caller>, jni::errors::Error> {
            let dir = client_dir(&data_dir.try_to_string(env)?);
            let (account, room) = (account.try_to_string(env)?, room.try_to_string(env)?);
            match super::room_link(&dir, &account, &room, seq) {
                Some(link) => env.new_string(link),
                None => Ok(JString::default()),
            }
        })
        .resolve::<LogErrorAndDefault>()
    }

    /// `Native.registerPending(dataDir)`: the token registered with every account signed in; whether that is done.
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_registerPending<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        data_dir: JString<'caller>,
    ) -> jboolean {
        env.with_env(|env| -> Result<bool, jni::errors::Error> {
            Ok(super::register_pending(&client_dir(&data_dir.try_to_string(env)?)))
        })
        .resolve::<LogErrorAndDefault>()
    }

    /// `Native.openPush(dataDir, pushId, kind, enc)`: the push opened (JSON with its `account`), or null.
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_openPush<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        data_dir: JString<'caller>,
        push_id: JString<'caller>,
        kind: JString<'caller>,
        enc: JString<'caller>,
    ) -> JString<'caller> {
        env.with_env(|env| -> Result<JString<'caller>, jni::errors::Error> {
            let dir = client_dir(&data_dir.try_to_string(env)?);
            let push_id = push_id.try_to_string(env)?;
            let enc = enc.try_to_string(env)?;
            let kind = kind.try_to_string(env)?;
            match super::open_push(&dir, &push_id, &kind, &enc) {
                Some(opened) => env.new_string(opened),
                None => Ok(JString::default()),
            }
        })
        .resolve::<LogErrorAndDefault>()
    }

    /// `Native.chatReply(dataDir, account, chatRoomId, text)`: queued; null, or why not.
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_chatReply<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        data_dir: JString<'caller>,
        account: JString<'caller>,
        room: JString<'caller>,
        text: JString<'caller>,
    ) -> JString<'caller> {
        env.with_env(|env| -> Result<JString<'caller>, jni::errors::Error> {
            let dir = client_dir(&data_dir.try_to_string(env)?);
            let (account, room, text) = (
                account.try_to_string(env)?,
                room.try_to_string(env)?,
                text.try_to_string(env)?,
            );
            match super::chat_reply(&dir, &account, &room, &text) {
                Ok(()) => Ok(JString::default()),
                Err(err) => env.new_string(err),
            }
        })
        .resolve::<LogErrorAndDefault>()
    }

    /// `Native.chatRead(dataDir, account, chatRoomId, eventSeq)`: queued; null, or why not.
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_chatRead<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        data_dir: JString<'caller>,
        account: JString<'caller>,
        room: JString<'caller>,
        seq: jlong,
    ) -> JString<'caller> {
        env.with_env(|env| -> Result<JString<'caller>, jni::errors::Error> {
            let dir = client_dir(&data_dir.try_to_string(env)?);
            let (account, room) = (account.try_to_string(env)?, room.try_to_string(env)?);
            match super::chat_read(&dir, &account, &room, seq) {
                Ok(()) => Ok(JString::default()),
                Err(err) => env.new_string(err),
            }
        })
        .resolve::<LogErrorAndDefault>()
    }

    /// `Native.ackPush(dataDir, account, ackId)`.
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_ackPush<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        data_dir: JString<'caller>,
        account: JString<'caller>,
        ack_id: JString<'caller>,
    ) {
        env.with_env(|env| -> Result<(), jni::errors::Error> {
            let dir = client_dir(&data_dir.try_to_string(env)?);
            let (account, ack_id) = (account.try_to_string(env)?, ack_id.try_to_string(env)?);
            super::ack_push(&dir, &account, &ack_id);
            Ok(())
        })
        .resolve::<LogErrorAndDefault>();
    }

    /// `Native.screenFrame(data, ptsUs)`: an encoded VP8 frame of the screen share, for its leg (`crate::screen`).
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_screenFrame<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        data: JByteArray<'caller>,
        pts_us: jlong,
    ) {
        env.with_env(|env| -> Result<(), jni::errors::Error> {
            crate::screen::frame(env.convert_byte_array(&data)?, pts_us);
            Ok(())
        })
        .resolve::<LogErrorAndDefault>();
    }

    /// `Native.screenEnded()`: the projection stopped (the user, the system, another app's share): the leg goes.
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_screenEnded<'caller>(
        _env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
    ) {
        crate::screen::stop();
    }

    /// The client's directory under the app's data directory, as the app's setup has it (`app_data_dir()/client`).
    fn client_dir(data_dir: &str) -> PathBuf {
        PathBuf::from(data_dir).join("client")
    }
}

#[cfg(test)]
#[path = "native_tests.rs"]
mod tests;
