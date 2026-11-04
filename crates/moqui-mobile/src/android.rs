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
//! The call path's call into the native code: the page's call state to `CallManager` ([`page_call_state`]).

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
pub(crate) fn page_call_state(id: &str, state: &str) -> Result<(), String> {
    with_context(|env, app| {
        let class = app_class(env, app, jni_str!("org.moqui.mobile.CallManager"))?;
        let id = env.new_string(id)?;
        let state = env.new_string(state)?;
        env.call_static_method(
            &class,
            jni_str!("pageCallState"),
            jni_sig!("(Ljava/lang/String;Ljava/lang/String;)V"),
            &[JValue::Object(&id), JValue::Object(&state)],
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

/// The Android Keystore's AES-GCM key of the app's device keys (`org.moqui.mobile.KeyVault`).
pub(crate) struct KeyVault;

impl KeyVault {
    /// The vault, once its Keystore key is made or found.
    pub(crate) fn ready() -> Result<Self, String> {
        with_context(|env, app| {
            let class = app_class(env, app, jni_str!("org.moqui.mobile.KeyVault"))?;
            env.call_static_method(&class, jni_str!("ready"), jni_sig!("()V"), &[])?;
            Ok(())
        })?;
        Ok(Self)
    }

    /// `bytes` through `KeyVault.<method>(byte[]): byte[]`.
    fn call(method: &'static str, bytes: &[u8]) -> Result<Vec<u8>, String> {
        with_context(|env, app| {
            let class = app_class(env, app, jni_str!("org.moqui.mobile.KeyVault"))?;
            let input = env.byte_array_from_slice(bytes)?;
            let name = match method {
                "seal" => jni_str!("seal"),
                _ => jni_str!("open"),
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
        Self::call("seal", plain)
    }

    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, String> {
        Self::call("open", sealed)
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
