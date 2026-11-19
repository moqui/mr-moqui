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

//! What the app asks of Android through JNI, on the JVM and the application context tao records in `ndk-context`:
//! rustls-platform-verifier's init, the Keystore's sealing of the device keys (`KeyVault.kt` in `gen/android`, loaded
//! through the app's class loader, as a thread of Rust's has only the system's), and a `VIEW` intent for the system
//! browser or a `mailto:`/`tel:` handler. A push can start the process with no activity, so no tao: the native code
//! then hands Rust the context itself (`Native.init`, [`init_from_native`]), which is used before tao's; tao's
//! `ndk-context` is never set here, since tao sets it once and would fail if it were set.
//!
//! The call path's call into the native code: the page's call state to `CallManager` ([`page_call_state`]); the
//! phone's contacts (`PhoneBook`, [`contacts_search`], [`contact_names`]) and the calling account of the phone's
//! dialer (`DialAccount`, [`dial_account`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use jni::errors::Error;
use jni::objects::{JByteArray, JClass, JClassLoader, JObject};
use jni::refs::{Global, LoaderContext};
use jni::strings::JNIStr;
use jni::{jni_sig, jni_str, Env, JValue, JavaVM};

use crate::keystore::Sealer;

/// The JVM and the application context the native code handed over (`Native.init`).
static NATIVE: OnceLock<(JavaVM, Global<JObject<'static>>)> = OnceLock::new();

/// The TLS verifier is set up.
static VERIFIER: AtomicBool = AtomicBool::new(false);

/// The JVM and the application context: the native code's, else tao's.
fn vm_and_context() -> (JavaVM, *mut std::ffi::c_void) {
    if let Some((vm, context)) = NATIVE.get() {
        return (vm.clone(), context.as_obj().as_raw().cast());
    }
    let context = ndk_context::android_context();
    // SAFETY: tao sets both from the live JVM and a global reference to the application context, kept for the life of
    // the process.
    (unsafe { JavaVM::from_raw(context.vm().cast()) }, context.context())
}

/// `Native.init(context)`: keep the JVM and the application context, and set up the TLS verifier; whether requests may
/// be made.
pub(crate) fn init_from_native(env: &mut Env<'_>, context: &JObject<'_>) -> bool {
    if NATIVE.get().is_none() {
        let kept = (|| -> Result<(JavaVM, Global<JObject<'static>>), Error> {
            let app = env
                .call_method(
                    context,
                    jni_str!("getApplicationContext"),
                    jni_sig!("()Landroid/content/Context;"),
                    &[],
                )?
                .l()?;
            Ok((env.get_java_vm()?, env.new_global_ref(&app)?))
        })();
        match kept {
            Ok(kept) => {
                let _ = NATIVE.set(kept);
            }
            Err(err) => {
                if env.exception_check() {
                    env.exception_clear();
                }
                log::error!("The app's context was not kept: {err}");
                return false;
            }
        }
    }
    if !VERIFIER.load(Ordering::SeqCst) {
        if let Err(err) = init_verifier() {
            log::error!("The TLS verifier did not start; the native code makes no requests: {err}");
        }
    }
    VERIFIER.load(Ordering::SeqCst)
}

/// Whether the TLS verifier is set up.
pub(crate) fn verifier_ready() -> bool {
    VERIFIER.load(Ordering::SeqCst)
}

/// `work` with the JVM attached to this thread and the application context; a Java exception is cleared and an error.
fn with_context<T>(work: impl FnOnce(&mut Env<'_>, &JObject<'_>) -> Result<T, Error>) -> Result<T, String> {
    let (vm, context) = vm_and_context();
    vm.attach_current_thread(|env| -> Result<T, Error> {
        // SAFETY: a global reference to the application context, kept for the life of the process, so it outlives this
        // frame.
        let app = unsafe { JObject::from_raw(env, context.cast()) };
        let done = work(env, &app);
        if done.is_err() && env.exception_check() {
            env.exception_clear();
        }
        done
    })
    .map_err(|err| err.to_string())
}

/// rustls-platform-verifier's JVM init, before the first request.
pub(crate) fn init_verifier() -> Result<(), String> {
    if VERIFIER.load(Ordering::SeqCst) {
        return Ok(());
    }
    let (vm, context) = vm_and_context();
    vm.attach_current_thread(|env| -> Result<(), Error> {
        // SAFETY: as in with_context.
        let app = unsafe { JObject::from_raw(env, context.cast()) };
        rustls_platform_verifier::android::init_with_env(env, app)
    })
    .map_err(|err| err.to_string())?;
    VERIFIER.store(true, Ordering::SeqCst);
    log::info!("TLS verifier initialized");
    Ok(())
}

/// Whether the app's activity ended for good (`ActivityEnd.endedForGood`): it finished, and is not made again for a
/// configuration change. When it cannot be asked, it did, as before.
pub(crate) fn activity_ended_for_good() -> bool {
    with_context(|env, app| {
        let class = app_class(env, app, jni_str!("org.moqui.mobile.ActivityEnd"))?;
        env.call_static_method(&class, jni_str!("endedForGood"), jni_sig!("()Z"), &[])?
            .z()
    })
    .unwrap_or_else(|err| {
        log::warn!("Whether the activity finished was not read: {err}");
        true
    })
}

/// Whether the native code has a call or a decline under way (`CallManager.busyNow`); not when it cannot be asked.
pub(crate) fn call_busy() -> bool {
    with_context(|env, app| {
        let class = app_class(env, app, jni_str!("org.moqui.mobile.CallManager"))?;
        env.call_static_method(&class, jni_str!("busyNow"), jni_sig!("()Z"), &[])?
            .z()
    })
    .unwrap_or(false)
}

/// The page's call `id` is in `state` (`crate::commands::CALL_STATES`; an empty id with `offScreen` for the shell
/// leaving for a server page): the native code keeps or ends its call (`CallManager.pageCallState`).
pub(crate) fn page_call_state(id: &str, state: &str, name: &str) -> Result<(), String> {
    with_context(|env, app| {
        let class = app_class(env, app, jni_str!("org.moqui.mobile.CallManager"))?;
        let id = env.new_string(id)?;
        let state = env.new_string(state)?;
        let name = env.new_string(name)?;
        env.call_static_method(
            &class,
            jni_str!("pageCallState"),
            jni_sig!("(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)V"),
            &[JValue::Object(&id), JValue::Object(&state), JValue::Object(&name)],
        )?;
        Ok(())
    })
}

/// The page that had the calls goes (another page, a reload, an account switch): the native calls it joined end
/// (`CallManager.pageGone`); a ring, or a call answered on the phone and not joined yet, stays.
pub(crate) fn calls_page_gone() -> Result<(), String> {
    with_context(|env, app| {
        let class = app_class(env, app, jni_str!("org.moqui.mobile.CallManager"))?;
        env.call_static_method(&class, jni_str!("pageGone"), jni_sig!("()V"), &[])?;
        Ok(())
    })
}

/// The call's audio route and routes (`CallAudio.state()`, JSON `{route, routes}`).
pub(crate) fn call_audio_state() -> Result<String, String> {
    with_context(|env, app| {
        let class = app_class(env, app, jni_str!("org.moqui.mobile.CallAudio"))?;
        let state = env
            .call_static_method(&class, jni_str!("state"), jni_sig!("()Ljava/lang/String;"), &[])?
            .l()?;
        let state = env.cast_local::<jni::objects::JString>(state)?;
        state.try_to_string(env)
    })
}

/// The call's audio to `route` (`CallAudio.request(route)`): whether Telecom was asked.
pub(crate) fn call_audio_route(route: &str) -> Result<bool, String> {
    with_context(|env, app| {
        let class = app_class(env, app, jni_str!("org.moqui.mobile.CallAudio"))?;
        let route = env.new_string(route)?;
        let asked = env
            .call_static_method(
                &class,
                jni_str!("request"),
                jni_sig!("(Ljava/lang/String;)Z"),
                &[JValue::Object(&route)],
            )?
            .z()?;
        Ok(asked)
    })
}

/// Screen security for `reason` on or off (`ScreenSecurity.set(reason, on)`): whether the flag is on now.
pub(crate) fn secure_screen(reason: &str, on: bool) -> Result<bool, String> {
    with_context(|env, app| {
        let class = app_class(env, app, jni_str!("org.moqui.mobile.ScreenSecurity"))?;
        let reason = env.new_string(reason)?;
        let held = env
            .call_static_method(
                &class,
                jni_str!("set"),
                jni_sig!("(Ljava/lang/String;Z)Z"),
                &[JValue::Object(&reason), JValue::Bool(on)],
            )?
            .z()?;
        Ok(held)
    })
}

/// The phone's calendar app on a new event (`CalendarIntent.insert`): whether one took it.
pub(crate) fn calendar_insert(
    title: &str,
    start: i64,
    end: i64,
    all_day: bool,
    zone: &str,
    place: &str,
    description: &str,
) -> Result<bool, String> {
    with_context(|env, app| {
        let class = app_class(env, app, jni_str!("org.moqui.mobile.CalendarIntent"))?;
        let title = env.new_string(title)?;
        let zone = env.new_string(zone)?;
        let place = env.new_string(place)?;
        let description = env.new_string(description)?;
        let opened = env
            .call_static_method(
                &class,
                jni_str!("insert"),
                jni_sig!("(Landroid/content/Context;Ljava/lang/String;JJZLjava/lang/String;Ljava/lang/String;Ljava/lang/String;)Z"),
                &[
                    JValue::Object(app),
                    JValue::Object(&title),
                    JValue::Long(start),
                    JValue::Long(end),
                    JValue::Bool(all_day),
                    JValue::Object(&zone),
                    JValue::Object(&place),
                    JValue::Object(&description),
                ],
            )?
            .z()?;
        Ok(opened)
    })
}

/// The phone's notification state, as the check screen shows it (`PushCheck.state(context)`, JSON: notifications
/// allowed, channels on, battery optimization, the last push received).
pub(crate) fn push_check_state() -> Result<String, String> {
    with_context(|env, app| {
        let class = app_class(env, app, jni_str!("org.moqui.mobile.PushCheck"))?;
        let state = env
            .call_static_method(
                &class,
                jni_str!("state"),
                jni_sig!("(Landroid/content/Context;)Ljava/lang/String;"),
                &[JValue::Object(app)],
            )?
            .l()?;
        let state = env.cast_local::<jni::objects::JString>(state)?;
        state.try_to_string(env)
    })
}

/// Open the system's screen that fixes `what` (`notifications`, `channels`, `battery`, `fullScreen`:
/// `PushCheck.fix(context, what)`).
pub(crate) fn push_check_fix(what: &str) -> Result<(), String> {
    with_context(|env, app| {
        let class = app_class(env, app, jni_str!("org.moqui.mobile.PushCheck"))?;
        let what = env.new_string(what)?;
        env.call_static_method(
            &class,
            jni_str!("fix"),
            jni_sig!("(Landroid/content/Context;Ljava/lang/String;)V"),
            &[JValue::Object(app), JValue::Object(&what)],
        )?;
        Ok(())
    })
}

/// The phone's contacts whose name or number has `term` (`PhoneBook.search`, JSON `[{name, number}]`); a first search
/// asks for the permission and waits for the answer, so never on the main thread.
pub(crate) fn contacts_search(term: &str, limit: i32) -> Result<String, String> {
    with_context(|env, app| {
        let class = app_class(env, app, jni_str!("org.moqui.mobile.PhoneBook"))?;
        let term = env.new_string(term)?;
        let found = env
            .call_static_method(
                &class,
                jni_str!("search"),
                jni_sig!("(Landroid/content/Context;Ljava/lang/String;I)Ljava/lang/String;"),
                &[JValue::Object(app), JValue::Object(&term), JValue::Int(limit)],
            )?
            .l()?;
        let found = env.cast_local::<jni::objects::JString>(found)?;
        found.try_to_string(env)
    })
}

/// The names the phone's contacts have for `numbers` (`PhoneBook.names`, JSON `{number: name}`); never asks.
pub(crate) fn contact_names(numbers: &[String]) -> Result<String, String> {
    with_context(|env, app| {
        let class = app_class(env, app, jni_str!("org.moqui.mobile.PhoneBook"))?;
        let numbers = env.new_string(numbers.join("\n"))?;
        let named = env
            .call_static_method(
                &class,
                jni_str!("names"),
                jni_sig!("(Landroid/content/Context;Ljava/lang/String;)Ljava/lang/String;"),
                &[JValue::Object(app), JValue::Object(&numbers)],
            )?
            .l()?;
        let named = env.cast_local::<jni::objects::JString>(named)?;
        named.try_to_string(env)
    })
}

/// The app's calling account for the phone's dialer registered (`on`) or taken away (`DialAccount.set`): whether
/// Telecom took it.
pub(crate) fn dial_account(on: bool) -> Result<bool, String> {
    with_context(|env, app| {
        let class = app_class(env, app, jni_str!("org.moqui.mobile.DialAccount"))?;
        env.call_static_method(
            &class,
            jni_str!("set"),
            jni_sig!("(Landroid/content/Context;Z)Z"),
            &[JValue::Object(app), JValue::Bool(on)],
        )?
        .z()
    })
}

/// A room of `account` read in the app: its notification from a push goes (`ChatNotifications.clearRoom`).
pub(crate) fn chat_clear(account: &str, room: &str) -> Result<(), String> {
    with_context(|env, app| {
        let class = app_class(env, app, jni_str!("org.moqui.mobile.ChatNotifications"))?;
        let (account, room) = (env.new_string(account)?, env.new_string(room)?);
        env.call_static_method(
            &class,
            jni_str!("clearRoom"),
            jni_sig!("(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)V"),
            &[JValue::Object(app), JValue::Object(&account), JValue::Object(&room)],
        )?;
        Ok(())
    })
}

/// The room of `account` the shell shows now (empty: none): its pushes show nothing while the app is in front
/// (`ChatNotifications.roomShown`).
pub(crate) fn chat_room_shown(account: &str, room: &str) -> Result<(), String> {
    with_context(|env, app| {
        let class = app_class(env, app, jni_str!("org.moqui.mobile.ChatNotifications"))?;
        let (account, room) = (env.new_string(account)?, env.new_string(room)?);
        env.call_static_method(
            &class,
            jni_str!("roomShown"),
            jni_sig!("(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)V"),
            &[JValue::Object(app), JValue::Object(&account), JValue::Object(&room)],
        )?;
        Ok(())
    })
}

/// `account` is signed out here: its chat notifications, their summary, its conversation shortcuts and a room of it
/// shown go (`ChatNotifications.forgetAccount`, SEC-CLI-100).
pub(crate) fn forget_account(account: &str) -> Result<(), String> {
    with_context(|env, app| {
        let class = app_class(env, app, jni_str!("org.moqui.mobile.ChatNotifications"))?;
        let account = env.new_string(account)?;
        env.call_static_method(
            &class,
            jni_str!("forgetAccount"),
            jni_sig!("(Landroid/content/Context;Ljava/lang/String;)V"),
            &[JValue::Object(app), JValue::Object(&account)],
        )?;
        Ok(())
    })
}

/// A push registration failed: the native code's worker tries again (`RegisterWorker.start`).
pub(crate) fn register_worker_start() -> Result<(), String> {
    with_context(|env, app| {
        let class = app_class(env, app, jni_str!("org.moqui.mobile.RegisterWorker"))?;
        env.call_static_method(
            &class,
            jni_str!("start"),
            jni_sig!("(Landroid/content/Context;)V"),
            &[JValue::Object(app)],
        )?;
        Ok(())
    })
}

/// The UnifiedPush instances registered for the accounts signed in here (`UnifiedPushes.registerAll`).
pub(crate) fn unified_register_all() -> Result<(), String> {
    with_context(|env, app| {
        let class = app_class(env, app, jni_str!("org.moqui.mobile.UnifiedPushes"))?;
        env.call_static_method(
            &class,
            jni_str!("registerAll"),
            jni_sig!("(Landroid/content/Context;)V"),
            &[JValue::Object(app)],
        )?;
        Ok(())
    })
}

/// An account signed in: Firebase asked for its token (`Push.startFcm`; nothing in a build without Firebase), which the
/// app's start asks only when an account is signed in already (SEC-CLI-121).
pub(crate) fn fcm_start() -> Result<(), String> {
    with_context(|env, app| {
        let class = app_class(env, app, jni_str!("org.moqui.mobile.Push"))?;
        env.call_static_method(
            &class,
            jni_str!("startFcm"),
            jni_sig!("(Landroid/content/Context;)V"),
            &[JValue::Object(app)],
        )?;
        Ok(())
    })
}

/// The last account here signed out: Firebase's token and installation deleted (`Push.stopFcm`; nothing in a build
/// without Firebase), so the phone stays registered with Google for no account; the next sign-in's [`fcm_start`] asks
/// for a new token (SEC-CLI-121).
pub(crate) fn fcm_stop() -> Result<(), String> {
    with_context(|env, app| {
        let class = app_class(env, app, jni_str!("org.moqui.mobile.Push"))?;
        env.call_static_method(
            &class,
            jni_str!("stopFcm"),
            jni_sig!("(Landroid/content/Context;)V"),
            &[JValue::Object(app)],
        )?;
        Ok(())
    })
}

/// Clear the server pages' origin's storage (`PageStorage.clear`), at a switch of the current account or its sign-out
/// (SEC-CLI-81).
pub(crate) fn clear_page_storage() -> Result<(), String> {
    with_context(|env, app| {
        let class = app_class(env, app, jni_str!("org.moqui.mobile.PageStorage"))?;
        env.call_static_method(&class, jni_str!("clear"), jni_sig!("()V"), &[])?;
        Ok(())
    })
}

/// Whether the phone has a secure lock screen (`DeviceLock.isSecure`: `KeyguardManager.isDeviceSecure()`), which the
/// shell warns of when it has not (SEC-CLI-113).
pub(crate) fn device_secure() -> Result<bool, String> {
    with_context(|env, app| {
        let class = app_class(env, app, jni_str!("org.moqui.mobile.DeviceLock"))?;
        let secure = env
            .call_static_method(
                &class,
                jni_str!("isSecure"),
                jni_sig!("(Landroid/content/Context;)Z"),
                &[JValue::Object(app)],
            )?
            .z()?;
        Ok(secure)
    })
}

/// The host of the server whose pages the app origin shows (`MobileHost::open_remote`), or none as they close: the
/// title of their alert and confirm dialogs, since every account's pages share the one origin
/// (`RustWebChromeClient.pageServer`, wry's patch; SEC-CLI-119).
pub(crate) fn page_server(host: Option<&str>) -> Result<(), String> {
    with_context(|env, app| {
        let class = app_class(env, app, jni_str!("org.moqui.mobile.RustWebChromeClient"))?;
        let host: JObject<'_> = match host {
            Some(host) => env.new_string(host)?.into(),
            None => JObject::null(),
        };
        env.call_static_method(
            &class,
            jni_str!("pageServer"),
            jni_sig!("(Ljava/lang/String;)V"),
            &[JValue::Object(&host)],
        )?;
        Ok(())
    })
}

/// The phone's model (`android.os.Build.MODEL`), for its name in the user's device list.
pub(crate) fn device_model() -> Result<String, String> {
    with_context(|env, _| {
        let build = env.find_class(jni_str!("android/os/Build"))?;
        let model = env
            .get_static_field(&build, jni_str!("MODEL"), jni_sig!("Ljava/lang/String;"))?
            .l()?;
        let model = env.cast_local::<jni::objects::JString>(model)?;
        let text = model.try_to_string(env)?;
        Ok(text)
    })
}

/// Open `url` with Android's handler of its scheme (the browser for http and https), as a new task.
pub(crate) fn open_url(url: &str) -> Result<(), String> {
    with_context(|env, app| {
        let text = env.new_string(url)?;
        let uri_class = env.find_class(jni_str!("android/net/Uri"))?;
        let uri = env
            .call_static_method(
                &uri_class,
                jni_str!("parse"),
                jni_sig!("(Ljava/lang/String;)Landroid/net/Uri;"),
                &[JValue::Object(&text)],
            )?
            .l()?;
        let action = env.new_string("android.intent.action.VIEW")?;
        let intent_class = env.find_class(jni_str!("android/content/Intent"))?;
        let intent = env.new_object(
            &intent_class,
            jni_sig!("(Ljava/lang/String;Landroid/net/Uri;)V"),
            &[JValue::Object(&action), JValue::Object(&uri)],
        )?;
        // FLAG_ACTIVITY_NEW_TASK: started from the application context
        env.call_method(
            &intent,
            jni_str!("addFlags"),
            jni_sig!("(I)Landroid/content/Intent;"),
            &[JValue::Int(0x1000_0000)],
        )?;
        env.call_method(
            app,
            jni_str!("startActivity"),
            jni_sig!("(Landroid/content/Intent;)V"),
            &[JValue::Object(&intent)],
        )?;
        Ok(())
    })
}

/// An Android Keystore AES-GCM key of the app's (`org.moqui.mobile.KeyVault`): the device keys' (`unlocked` false), or
/// the one that opens only while the phone is unlocked, for what no background work reads (SEC-CLI-113,
/// `crate::keystore`).
pub(crate) struct KeyVault {
    unlocked: bool,
}

impl KeyVault {
    /// The device keys' vault, once its Keystore key is made or found.
    pub(crate) fn ready() -> Result<Self, String> {
        with_context(|env, app| {
            let class = app_class(env, app, jni_str!("org.moqui.mobile.KeyVault"))?;
            env.call_static_method(&class, jni_str!("ready"), jni_sig!("()V"), &[])?;
            Ok(())
        })?;
        Ok(Self { unlocked: false })
    }

    /// The vault of an unlocked phone, once its Keystore key is made or found; an error before Android 9, which has no
    /// such key.
    pub(crate) fn ready_unlocked() -> Result<Self, String> {
        with_context(|env, app| {
            let class = app_class(env, app, jni_str!("org.moqui.mobile.KeyVault"))?;
            env.call_static_method(&class, jni_str!("readyUnlocked"), jni_sig!("()V"), &[])?;
            Ok(())
        })?;
        Ok(Self { unlocked: true })
    }

    /// `bytes` through `KeyVault.<method>(byte[]): byte[]` (`seal`, `open`, or for the unlocked phone's key
    /// `sealUnlocked`, `openUnlocked`).
    fn call(&self, method: &'static str, bytes: &[u8]) -> Result<Vec<u8>, String> {
        let unlocked = self.unlocked;
        with_context(|env, app| {
            let class = app_class(env, app, jni_str!("org.moqui.mobile.KeyVault"))?;
            let input = env.byte_array_from_slice(bytes)?;
            let name = match (method, unlocked) {
                ("seal", false) => jni_str!("seal"),
                ("seal", true) => jni_str!("sealUnlocked"),
                (_, false) => jni_str!("open"),
                (_, true) => jni_str!("openUnlocked"),
            };
            let output = env
                .call_static_method(&class, name, jni_sig!("([B)[B"), &[JValue::Object(&input)])?
                .l()?;
            let output = env.cast_local::<JByteArray>(output)?;
            env.convert_byte_array(&output)
        })
    }
}

impl Sealer for KeyVault {
    fn seal(&self, plain: &[u8]) -> Result<Vec<u8>, String> {
        self.call("seal", plain)
    }

    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, String> {
        self.call("open", sealed)
    }
}

/// A class of the app's (`KeyVault`, `CallManager`), through the application context's class loader (`getClassLoader`,
/// the app's; the context's own class is the framework's, and a thread of Rust's has only the system's).
fn app_class<'local>(env: &mut Env<'local>, app: &JObject<'_>, name: &JNIStr) -> Result<JClass<'local>, Error> {
    let loader = env
        .call_method(
            app,
            jni_str!("getClassLoader"),
            jni_sig!("()Ljava/lang/ClassLoader;"),
            &[],
        )?
        .l()?;
    let loader = env.cast_local::<JClassLoader>(loader)?;
    LoaderContext::Loader(&loader).load_class(env, name, true)
}
