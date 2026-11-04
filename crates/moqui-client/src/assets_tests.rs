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

use super::*;

/// The repository's shell and webroot, as the desktop app's development run reads them.
fn repository() -> Assets {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    Assets::new(
        crates.join("moqui-desktop/shell"),
        crates.join("../runtime/base-component/webroot/screen/webroot"),
    )
}

#[test]
fn the_shell_serves_its_page_scripts_and_libraries_and_nothing_else() {
    let assets = repository();
    let page = assets.read("/").expect("the page");
    assert!(page.page && page.content_type.starts_with("text/html"));
    assert!(String::from_utf8_lossy(&page.body).contains("/shell/js/shell.js"));
    for path in [
        "/shell/js/shell.js",
        "/shell/js/bundled-screens.js",
        "/js/MoquiLib.js",
        "/js/WebrootVue.qvt.render.js",
        "/libs/vue/vue.runtime.global.prod.js",
        "/libs/quasar/quasar.umd.prod.js",
        "/css/WebrootVue.qvt.css",
    ] {
        let asset = assets.read(path).unwrap_or_else(|| panic!("{path}"));
        assert!(!asset.page, "{path}");
    }
    assert_eq!(
        assets.read("/js/MoquiLib.js").unwrap().content_type,
        "text/javascript; charset=utf-8"
    );
    // not the shell's: forwarded to the server
    for path in [
        "/qapps/clientConfig",
        "/shellData",
        "/apps/Comms/Chat/chatData",
        "/js",
        "/notws",
    ] {
        assert!(assets.file_of(path).is_none(), "{path}");
    }
    // nothing outside its directories
    for path in [
        "/js/../../../Cargo.toml",
        "/shell/../src/lib.rs",
        "/libs//x",
        "/js/a\\..\\b",
        "/shell/.",
        "/shell/C:/Windows/win.ini",
    ] {
        assert!(assets.file_of(path).is_none(), "{path}");
    }
    assert!(assets.read("/js/NoSuchFile.js").is_none());
    // a library the bundle does not stage is not served in development either
    assert!(assets.read("/libs/dompurify/purify.min.js").is_some() || assets.file_of("/libs/dompurify/x").is_some());
    assert!(assets.file_of("/libs/mermaid/mermaid.min.js").is_none());
    // the bundled Comms screens' libraries: the rich-text editor, and the crypto device's module as WebAssembly
    for path in [
        "/libs/tiptap/tiptap.min.js",
        "/libs/matrix-sdk-crypto/index.mjs",
        "/js/WebrootCrypto.js",
    ] {
        assert!(assets.read(path).is_some(), "{path}");
    }
    let wasm = assets
        .file_of("/libs/matrix-sdk-crypto/pkg/matrix_sdk_crypto_wasm_bg.wasm")
        .unwrap();
    assert_eq!(content_type(&wasm), "application/wasm");
    assert!(String::from_utf8_lossy(&page.body).contains("/js/WebrootCrypto.js"));
}

#[test]
fn the_shells_own_paths_are_never_forwarded() {
    let assets = repository();
    // a library not staged, a script not there, any path under the shell's directories: its own (404), not the server's
    for path in [
        "/",
        "/index.html",
        "/libs/mermaid/mermaid.min.js",
        "/libs/x.js",
        "/js/NoSuchFile.js",
        "/js/../x",
        "/shell/x",
        "/css/x.css",
        "/libs",
    ] {
        assert!(assets.is_static(path), "{path}");
    }
    assert!(assets.read("/libs/mermaid/mermaid.min.js").is_none());
    for path in [
        "/qapps/clientConfig",
        "/apps/Comms/Chat/chatData",
        "/shellData",
        "/notws",
        "/jsx/a",
        "/rest/s1",
    ] {
        assert!(!assets.is_static(path), "{path}");
    }
}

#[test]
fn the_shell_compiles_webassembly_and_evaluates_no_script_text() {
    assert!(SHELL_CSP.contains("'wasm-unsafe-eval'"));
    assert!(!SHELL_CSP.contains("'unsafe-eval'"));
    assert!(SHELL_CSP.contains("frame-src 'none'"));
    assert!(!WORKER_CSP.contains("unsafe"));
}

/// The directive `name` of a policy, as its sources.
fn directive<'a>(policy: &'a str, name: &str) -> Vec<&'a str> {
    policy
        .split(';')
        .map(str::trim)
        .find_map(|part| part.strip_prefix(name).filter(|rest| rest.starts_with(' ')))
        .map(|rest| rest.split_whitespace().collect())
        .unwrap_or_default()
}

#[test]
fn the_shell_runs_one_worker_an_encrypted_call_s_staged_sframe_transform() {
    // the worker by its exact path: no other script, no blob: or data: worker, no forwarded path of the server's
    assert_eq!(
        directive(SHELL_CSP, "worker-src"),
        vec!["http://moqui-shell.localhost/js/WebrootSFrameWorker.js"]
    );
    assert_eq!(SFRAME_WORKER, "/js/WebrootSFrameWorker.js");
    // the call's media and signaling need no other source: no WebSocket (the host holds /notws and /callws)
    assert_eq!(
        directive(SHELL_CSP, "connect-src"),
        vec!["'self'", "http://ipc.localhost"]
    );
    assert_eq!(directive(SHELL_CSP, "media-src"), vec!["'self'", "blob:"]);
    assert!(!SHELL_CSP.contains("ws:") && !SHELL_CSP.contains("wss:"));
    // a staged file, answered with a policy of its own that lets it import the SFrame code only
    let assets = repository();
    let worker = assets.read(SFRAME_WORKER).expect("the worker is staged");
    assert!(worker.worker && !worker.page);
    assert!(String::from_utf8_lossy(&worker.body).contains("importScripts(\"/js/WebrootSFrame.js\")"));
    assert!(assets.read("/js/WebrootSFrame.js").is_some_and(|code| !code.worker));
    assert!(assets.read("/js/MoquiLib.js").is_some_and(|code| !code.worker));
    assert_eq!(directive(WORKER_CSP, "default-src"), vec!["'none'"]);
    assert_eq!(directive(WORKER_CSP, "worker-src"), vec!["'none'"]);
    assert_eq!(
        directive(WORKER_CSP, "script-src"),
        vec!["http://moqui-shell.localhost/js/WebrootSFrame.js"]
    );
    // the calls' scripts load with the page
    let page = String::from_utf8_lossy(&assets.read("/").unwrap().body).to_string();
    for script in [
        "/js/WebrootCall.js",
        "/js/WebrootCall.render.js",
        "/js/WebrootCallKeys.js",
    ] {
        assert!(page.contains(script), "{script}");
        assert!(assets.read(script).is_some(), "{script}");
    }
}

#[test]
fn a_bundle_reads_its_staged_copy() {
    let assets = Assets::staged(Path::new("/App/Resources"));
    assert_eq!(
        assets.file_of("/js/MoquiLib.js").unwrap(),
        PathBuf::from("/App/Resources/shell-assets/webroot/js/MoquiLib.js")
    );
    assert_eq!(
        assets.file_of("/").unwrap(),
        PathBuf::from("/App/Resources/shell-assets/shell/index.html")
    );
}
