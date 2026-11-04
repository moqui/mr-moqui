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
//! browser or a `mailto:`/`tel:` handler.

use jni::errors::Error;
use jni::objects::{JByteArray, JClassLoader, JObject};
use jni::refs::LoaderContext;
use jni::{jni_sig, jni_str, Env, JValue, JavaVM};

use crate::keystore::Sealer;

/// `work` with the JVM attached to this thread and the application context; a Java exception is cleared and an error.
fn with_context<T>(work: impl FnOnce(&mut Env<'_>, &JObject<'_>) -> Result<T, Error>) -> Result<T, String> {
    let context = ndk_context::android_context();
    // SAFETY: tao sets both from the live JVM and a global reference to the application context, kept for the life of
    // the process.
    let vm = unsafe { JavaVM::from_raw(context.vm().cast()) };
    vm.attach_current_thread(|env| -> Result<T, Error> {
        // SAFETY: as above; the reference is global, so it outlives this frame.
        let app = unsafe { JObject::from_raw(env, context.context().cast()) };
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
    let context = ndk_context::android_context();
    // SAFETY: as in with_context.
    let vm = unsafe { JavaVM::from_raw(context.vm().cast()) };
    vm.attach_current_thread(|env| -> Result<(), Error> {
        // SAFETY: as in with_context.
        let app = unsafe { JObject::from_raw(env, context.context().cast()) };
        rustls_platform_verifier::android::init_with_env(env, app)
    })
    .map_err(|err| err.to_string())?;
    log::info!("TLS verifier initialized");
    Ok(())
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
            let class = vault_class(env, app)?;
            env.call_static_method(&class, jni_str!("ready"), jni_sig!("()V"), &[])?;
            Ok(())
        })?;
        Ok(Self)
    }

    /// `bytes` through `KeyVault.<method>(byte[]): byte[]`.
    fn call(method: &'static str, bytes: &[u8]) -> Result<Vec<u8>, String> {
        with_context(|env, app| {
            let class = vault_class(env, app)?;
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

/// The app's `KeyVault` class, through the application context's class loader (`getClassLoader`, the app's; the
/// context's own class is the framework's).
fn vault_class<'local>(env: &mut Env<'local>, app: &JObject<'_>) -> Result<jni::objects::JClass<'local>, Error> {
    let loader = env
        .call_method(
            app,
            jni_str!("getClassLoader"),
            jni_sig!("()Ljava/lang/ClassLoader;"),
            &[],
        )?
        .l()?;
    let loader = env.cast_local::<JClassLoader>(loader)?;
    LoaderContext::Loader(&loader).load_class(env, jni_str!("org.moqui.mobile.KeyVault"), true)
}
