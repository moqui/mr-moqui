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
}
