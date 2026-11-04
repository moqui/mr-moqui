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

//! The repository's shell, as a development run serves it (`shell_assets`): the files its page loads are there, and
//! the libraries are those `shell/libs.txt` stages. The serving rules are tested in moqui-client (`assets`).

use std::path::Path;

use moqui_client::assets::SFRAME_WORKER;

use super::shell_assets;

#[test]
fn the_repository_shell_has_its_page_scripts_and_libraries() {
    let assets = shell_assets(true, Path::new("/nowhere"));
    let page = assets.read("/").expect("the page");
    assert!(page.page && page.content_type.starts_with("text/html"));
    let body = String::from_utf8_lossy(&page.body).to_string();
    for script in [
        "/shell/js/shell.js",
        "/js/WebrootCrypto.js",
        "/js/WebrootCall.js",
        "/js/WebrootCall.render.js",
        "/js/WebrootCallKeys.js",
    ] {
        assert!(body.contains(script), "{script}");
    }
    for path in [
        "/shell/js/shell.js",
        "/shell/js/bundled-screens.js",
        "/js/MoquiLib.js",
        "/js/WebrootVue.qvt.render.js",
        "/libs/vue/vue.runtime.global.prod.js",
        "/libs/quasar/quasar.umd.prod.js",
        "/css/WebrootVue.qvt.css",
        "/js/WebrootCallKeys.js",
        "/js/WebrootCall.js",
        "/js/WebrootCall.render.js",
        "/js/WebrootCrypto.js",
        "/js/WebrootSFrame.js",
        // the bundled Comms screens' libraries: the rich-text editor, and the crypto device's module
        "/libs/tiptap/tiptap.min.js",
        "/libs/matrix-sdk-crypto/index.mjs",
    ] {
        let asset = assets.read(path).unwrap_or_else(|| panic!("{path}"));
        assert!(!asset.page, "{path}");
    }
    let wasm = assets
        .read("/libs/matrix-sdk-crypto/pkg/matrix_sdk_crypto_wasm_bg.wasm")
        .expect("the crypto module");
    assert_eq!(wasm.content_type, "application/wasm");
    // a library the bundle does not stage is not served in development either
    assert!(assets.read("/libs/mermaid/mermaid.min.js").is_none());
    let worker = assets.read(SFRAME_WORKER).expect("the worker is staged");
    assert!(worker.worker);
    assert!(String::from_utf8_lossy(&worker.body).contains("importScripts(\"/js/WebrootSFrame.js\")"));
}
