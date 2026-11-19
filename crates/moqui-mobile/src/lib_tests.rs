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

use moqui_client::assets::Assets;
use moqui_client::conf::{ClientConf, DeviceMode};
use tauri::Url;

#[test]
fn the_builds_client_conf_is_its_element_and_parses() {
    let element = super::client_conf_element();
    assert!(
        element.starts_with("<client-conf ") && element.ends_with("/>"),
        "{element}"
    );
    let conf = ClientConf::parse(element);
    assert_eq!(conf.device_mode, DeviceMode::Choose);
    // the element as parsed, not the defaults of text that does not parse: a changed value comes through
    let shared = ClientConf::parse(&element.replace("device-mode=\"choose\"", "device-mode=\"shared\""));
    assert_eq!(shared.device_mode, DeviceMode::Shared);
}

#[test]
fn the_embedded_shell_has_the_libraries_of_the_desktops_list() {
    let layout = Assets::embedded(super::SHELL_LIBS);
    assert_eq!(
        layout.staged_path("/libs/vue/vue.runtime.global.prod.js").as_deref(),
        Some("webroot/libs/vue/vue.runtime.global.prod.js")
    );
    assert_eq!(layout.staged_path("/").as_deref(), Some("shell/index.html"));
}

#[test]
fn the_bridge_defines_the_adapter_on_the_apps_two_origins_only() {
    assert!(super::BRIDGE.contains("location.origin === \"http://moqui-shell.localhost\""));
    assert!(super::BRIDGE.contains("location.origin === \"http://moqui-app.localhost\""));
    // the server's pages have no shell command in their adapter
    let app_part = super::BRIDGE
        .split("if (appPage) {")
        .nth(1)
        .and_then(|rest| rest.split("return;\n    }").next())
        .unwrap();
    assert!(!app_part.contains("client_sign_in") && !app_part.contains("client_state"));
    // the main frame only, and the shell's commands carry the load's nonce
    assert!(super::BRIDGE.contains("window.top !== window.self) return;"));
    assert!(!super::BRIDGE.contains(" ask(\"client_state\""));
    assert!(super::BRIDGE.contains("shellAsk(\"client_state\")"));
}

#[test]
fn the_device_is_named_for_the_phones_model() {
    assert_eq!(super::named_device(Some("Pixel 7")), "MR Moqui on Pixel 7");
    assert_eq!(super::named_device(Some(" \n")), "MR Moqui on Android");
    assert_eq!(super::named_device(None), "MR Moqui on Android");
    assert_eq!(
        super::named_device(Some(&"x".repeat(100))).len(),
        "MR Moqui on ".len() + 60
    );
}

/// Each of the app's own commands in the handler is in build.rs' `COMMANDS` (the app manifest) and allowed in the
/// capability, so none is refused on the phone (secure_screen, plans/Comms-Messaging.md §18.3 phase 16, was not at
/// first).
#[test]
fn every_command_of_the_handler_is_in_the_manifest_and_allowed() {
    let lib = include_str!("lib.rs");
    let build = include_str!("../build.rs");
    let capability = include_str!("../capabilities/main.json");
    let handled: Vec<&str> = lib
        .split("            commands::")
        .skip(1)
        .filter_map(|rest| rest.split([',', ']', '\n']).next())
        .map(str::trim)
        .filter(|name| !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
        .collect();
    assert!(handled.contains(&"secure_screen"), "{handled:?}");
    for name in handled {
        assert!(
            build.contains(&format!("\"{name}\"")),
            "{name} is not in build.rs COMMANDS"
        );
        let allowed = format!("\"allow-{}\"", name.replace('_', "-"));
        assert!(
            capability.contains(&allowed),
            "{allowed} is not in capabilities/main.json"
        );
    }
}

/// SEC-CLI-112: a refused navigation is logged by its URL's scheme and host only, and a path that did not open by its
/// path less the query (and so is its error, which may name it again): logcat is read by adb and bug reports, and a
/// share link's token or a query value may be in the rest.
#[test]
fn a_logged_url_has_only_its_scheme_and_host_and_a_logged_path_no_query() {
    let url = Url::parse("https://ann:pw@chat.example.com:8443/share/abc123?token=s3cret#frag").unwrap();
    assert_eq!(super::url_for_log(&url), "https://chat.example.com");
    assert_eq!(
        super::url_for_log(&Url::parse("javascript:alert(1)").unwrap()),
        "javascript:"
    );
    assert_eq!(
        super::url_for_log(&Url::parse("data:text/html,<p>s3cret</p>").unwrap()),
        "data:"
    );
    assert_eq!(
        super::without_query("/qapps/chat/room?token=s3cret&x=1"),
        "/qapps/chat/room"
    );
    assert_eq!(super::without_query("/qapps/chat"), "/qapps/chat");
    assert_eq!(
        super::without_query("Not a path on the server: //x?token=s3cret"),
        "Not a path on the server: //x"
    );
}
