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
//! register, a ring declined, whether an account is signed in. A push can start the process with no activity, so no
//! Tauri app: the client ([`core_in`]) is then made here from the client's directory, and the app's setup takes the
//! same one when the activity starts, so there is one client, with one set of stores, per process.
//!
//! The JNI entry points (`Java_org_moqui_mobile_Native_*`, `gen/android/.../Native.kt`) are at the end, for Android;
//! each runs its work under `catch_unwind` (jni's `with_env`), so no panic crosses into the JVM.

// The native code's entry points are Android's; their work is tested on the desktop.
#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use std::path::Path;
use std::sync::{Arc, Mutex};

use moqui_client::account::Account;
use moqui_client::app::Client;
use moqui_client::conf::ClientConf;
use moqui_client::proxy::Request;
use moqui_client_tauri::SHELL_ORIGIN;
use serde_json::Value;

use crate::calls::{self, Asked, Declined, Poster, PushRecord};

/// The process's client, whoever made it first: the app's setup or a push.
static CORE: Mutex<Option<Arc<Client>>> = Mutex::new(None);

/// One change of `push.json` at a time.
static PUSH: Mutex<()> = Mutex::new(());

/// The client of `dir` (the app's data directory's `client`): the one this process has, unless it was stopped, else
/// a new one.
pub(crate) fn core_in(dir: &Path) -> Result<Arc<Client>, String> {
    let mut held = moqui_lib::sync::lock(&CORE);
    if let Some(core) = held.as_ref().filter(|core| !core.is_stopped()) {
        return Ok(Arc::clone(core));
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

/// Decline the ringing call `id` on its server, with no WebView: what happened, for the log.
pub(crate) fn decline_call(dir: &Path, id: &str) -> String {
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
    let poster = ProxyPoster(&core);
    match calls::decline(&asked(&core), id, &poster) {
        Declined::Told(account) => format!("declined on the server of {account}"),
        Declined::NotFound => "no account's server knows the call".to_string(),
        Declined::Failed(why) => format!("no server was told: {}", why.join("; ")),
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
    use jni::sys::jboolean;
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

    /// `Native.decline(dataDir, callSessionId)`: what happened.
    #[no_mangle]
    pub extern "system" fn Java_org_moqui_mobile_Native_decline<'caller>(
        mut env: EnvUnowned<'caller>,
        _class: JClass<'caller>,
        data_dir: JString<'caller>,
        call_session_id: JString<'caller>,
    ) -> JString<'caller> {
        env.with_env(|env| -> Result<JString<'caller>, jni::errors::Error> {
            let dir = client_dir(&data_dir.try_to_string(env)?);
            let id = call_session_id.try_to_string(env)?;
            let done = super::decline_call(&dir, &id);
            log::info!("Calls: the ring of {id}: {done}");
            env.new_string(done)
        })
        .resolve::<LogErrorAndDefault>()
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

    /// The client's directory under the app's data directory, as the app's setup has it (`app_data_dir()/client`).
    fn client_dir(data_dir: &str) -> PathBuf {
        PathBuf::from(data_dir).join("client")
    }
}

#[cfg(test)]
#[path = "native_tests.rs"]
mod tests;
