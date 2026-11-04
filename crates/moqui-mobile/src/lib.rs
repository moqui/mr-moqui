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

//! The mobile app (plans/Mobile-Desktop.md, MD6). For now the stage 5 spike: Tauri on the wry runtime (Android's
//! WebView) showing a static page, which shows the outcome of one HTTPS request made by moqui-client on a thread of
//! its own, verified by Android's trust manager through rustls-platform-verifier.

use std::sync::{Arc, Mutex};

/// The URL the spike asks for: `MOQUI_MOBILE_PROBE_URL` at build time, else example.com. The client's config path is
/// added, so any answer but a transport error shows that the TLS handshake passed.
const PROBE_URL: &str = match option_env!("MOQUI_MOBILE_PROBE_URL") {
    Some(url) => url,
    None => "https://example.com",
};

/// The probe's outcome, for the page: empty while it runs.
#[derive(Default)]
struct Probe(Arc<Mutex<String>>);

/// The probe's outcome so far, polled by the page.
#[tauri::command]
fn probe_result(probe: tauri::State<'_, Probe>) -> String {
    probe.0.lock().map(|text| text.clone()).unwrap_or_default()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "android")]
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("moqui-mobile"),
    );
    let probe = Probe::default();
    let outcome = Arc::clone(&probe.0);
    let result = tauri::Builder::default()
        .runtime(tauri_runtime_wry::Wry::default())
        .manage(probe)
        .invoke_handler(tauri::generate_handler![probe_result])
        .setup(move |_app| {
            #[cfg(target_os = "android")]
            if let Err(err) = android::init_verifier() {
                log::error!("TLS verifier init failed: {err}");
            }
            // reqwest's blocking client must not run on Tauri's async runtime: a plain thread, as the desktop's.
            std::thread::spawn(move || {
                let text = run_probe(PROBE_URL);
                log::info!("probe: {text}");
                if let Ok(mut slot) = outcome.lock() {
                    *slot = text;
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!());
    if let Err(err) = result {
        log::error!("the app did not start: {err}");
    }
}

/// One HTTPS request through moqui-client (its `/qapps/clientConfig`, with no key), described for the page and the log.
fn run_probe(base: &str) -> String {
    let conf = moqui_client::conf::ClientConf::parse(r#"<client-conf device-mode="personal"/>"#);
    let answer = match moqui_client::login::client_config(base, "") {
        Ok(Some(_)) => "a client config".to_string(),
        Ok(None) => "401 (TLS verified)".to_string(),
        // An HTTP status is an answer over a verified connection; a transport error names the TLS failure.
        Err(err) if err.to_string().contains(": HTTP ") => format!("{err} (TLS verified)"),
        Err(err) => format!("failed: {err}"),
    };
    format!(
        "moqui-client {} (device mode {}): {base}: {answer}",
        moqui_client::login::CLIENT_API,
        conf.device_mode.as_str()
    )
}

#[cfg(target_os = "android")]
mod android {
    use jni::objects::JObject;
    use jni::JavaVM;

    /// rustls-platform-verifier's JVM init, with the VM and the application context tao's activity recorded in
    /// ndk-context, before the first request.
    pub(crate) fn init_verifier() -> Result<(), String> {
        let context = ndk_context::android_context();
        // SAFETY: tao sets both from the live JVM and a global reference to the application context, kept for the
        // life of the process.
        let vm = unsafe { JavaVM::from_raw(context.vm().cast()) };
        vm.attach_current_thread(|env| -> Result<(), jni::errors::Error> {
            // SAFETY: as above; the reference is global, so it outlives this frame.
            let app = unsafe { JObject::from_raw(env, context.context().cast()) };
            rustls_platform_verifier::android::init_with_env(env, app)
        })
        .map_err(|err| err.to_string())?;
        log::info!("TLS verifier initialized");
        Ok(())
    }
}
