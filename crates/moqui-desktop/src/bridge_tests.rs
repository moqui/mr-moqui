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

use std::collections::HashMap;
use std::str::FromStr;

use tauri::utils::acl::RemoteUrlPattern;

use super::*;

fn node() -> Url {
    Url::parse("http://127.0.0.1:18080/qapps").unwrap()
}

#[test]
fn the_capability_pattern_is_the_nodes_origin_alone() {
    let origin = node().origin().ascii_serialization();
    assert_eq!(origin, "http://127.0.0.1:18080");
    let pattern = RemoteUrlPattern::from_str(&origin).unwrap();
    assert!(pattern.test(&Url::parse("http://127.0.0.1:18080/qapps/system?x=1#y").unwrap()));
    assert!(pattern.test(&Url::parse("http://127.0.0.1:18080/").unwrap()));
    assert!(!pattern.test(&Url::parse("http://127.0.0.1:18081/qapps").unwrap()));
    assert!(!pattern.test(&Url::parse("https://127.0.0.1:18080/qapps").unwrap()));
    assert!(!pattern.test(&Url::parse("http://localhost:18080/qapps").unwrap()));
}

#[test]
fn the_permissions_name_the_commands_of_the_app_manifest() {
    for command in ["notify", "set_counts", "set_presence", "open_external"] {
        let permission = format!("allow-{}", command.replace('_', "-"));
        assert!(PERMISSIONS.contains(&permission.as_str()), "{permission}");
    }
}

#[test]
fn clip_drops_control_characters_and_cuts_at_a_character() {
    assert_eq!(clip("  New\tmail\n ", 50), "New mail");
    assert_eq!(clip("héllo wörld", 5), "héllo");
    assert_eq!(clip("ab   cd", 4), "ab");
    assert_eq!(clip("\u{0}\u{7}", 10), "");
}

#[test]
fn a_notification_link_is_on_the_node_or_none() {
    let node = node();
    assert_eq!(
        node_link("/qapps/comms/Inbox", Some(&node)).unwrap().as_str(),
        "http://127.0.0.1:18080/qapps/comms/Inbox"
    );
    assert_eq!(
        node_link("http://127.0.0.1:18080/qapps/x?a=1", Some(&node))
            .unwrap()
            .as_str(),
        "http://127.0.0.1:18080/qapps/x?a=1"
    );
    assert!(node_link("https://example.com/qapps", Some(&node)).is_none());
    assert!(node_link("http://127.0.0.1:18081/qapps", Some(&node)).is_none());
    assert!(node_link("javascript:alert(1)", Some(&node)).is_none());
    assert!(node_link("//example.com/x", Some(&node)).is_none());
    assert!(node_link("/qapps", None).is_none());
    assert!(node_link(&format!("/{}", "a".repeat(URL_MAX)), Some(&node)).is_none());
}

#[test]
fn only_web_and_call_and_mail_links_off_the_node_open_outside() {
    let node = node();
    for ok in [
        "https://example.com/a?b=c",
        "http://example.com",
        "mailto:a@example.com",
        "tel:+15551234",
        "sip:a@example.com",
    ] {
        assert!(external_url(ok, Some(&node)).is_ok(), "{ok}");
    }
    for refused in [
        "file:///etc/passwd",
        "javascript:alert(1)",
        "data:text/html,x",
        "x-apple.systempreferences:com.apple.preference",
        "ftp://example.com/",
        "smb://server/share",
        "http://127.0.0.1:18080/qapps",
        "not a url",
        "",
    ] {
        assert!(external_url(refused, Some(&node)).is_err(), "{refused}");
    }
    assert!(external_url("http://127.0.0.1:18080/qapps", None).is_ok());
    assert!(external_url(&format!("https://example.com/{}", "a".repeat(URL_MAX)), Some(&node)).is_err());
}

#[test]
fn downloads_come_from_the_node_or_the_page() {
    let node = node();
    let allowed = |url: &str| download_allowed(&Url::parse(url).unwrap(), Some(&node));
    assert!(allowed("http://127.0.0.1:18080/fop/apps/x.pdf"));
    assert!(allowed(
        "blob:http://127.0.0.1:18080/0c5f4b9e-1c1d-4d6a-9a43-4c1a8f1b2f00"
    ));
    assert!(allowed("data:text/csv,a,b"));
    assert!(!allowed("blob:http://127.0.0.1:18081/0c5f4b9e"));
    assert!(!allowed("https://example.com/x.zip"));
    assert!(!allowed("file:///tmp/x"));
    assert!(!download_allowed(
        &Url::parse("http://127.0.0.1:18080/x").unwrap(),
        None
    ));
}

#[test]
fn the_total_counts_whole_numbers_above_zero() {
    let counts: HashMap<String, f64> = [
        ("chat", 3.0),
        ("mail", 2.7),
        ("calls", -1.0),
        ("todo", f64::NAN),
        ("x", f64::INFINITY),
        ("y", 0.5),
    ]
    .into_iter()
    .map(|(key, count)| (key.to_string(), count))
    .collect();
    assert_eq!(count_total(&counts), 5);
    assert_eq!(count_total(&HashMap::new()), 0);
    let huge: HashMap<String, f64> = [("a".to_string(), 1e30), ("b".to_string(), 1e30)].into_iter().collect();
    assert_eq!(count_total(&huge), u64::MAX);
}

#[test]
fn the_tray_shows_the_total_and_presence() {
    assert_eq!(badge(0), None);
    assert_eq!(badge(7).as_deref(), Some("7"));
    assert_eq!(badge(100).as_deref(), Some("99+"));
    assert_eq!(tooltip(0, None), "MR Moqui");
    assert_eq!(tooltip(3, None), "MR Moqui · 3 new");
    assert_eq!(tooltip(3, presence_label("dnd")), "MR Moqui · Do not disturb · 3 new");
    assert_eq!(tooltip(0, presence_label("away")), "MR Moqui · Away");
    assert_eq!(presence_label("invisible"), None);
    assert_eq!(presence_label("<b>"), None);
}

#[test]
fn the_capability_resolves_against_the_built_acl_to_the_bridge_commands_only() {
    use std::collections::BTreeMap;
    use tauri::ipc::RuntimeCapability;
    use tauri::utils::acl::capability::CapabilityFile;
    use tauri::utils::acl::manifest::Manifest;
    use tauri::utils::acl::resolved::Resolved;
    use tauri::utils::platform::Target;

    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/gen/schemas/acl-manifests.json");
    let acl: BTreeMap<String, Manifest> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let CapabilityFile::Capability(capability) = capability("http://127.0.0.1:18080").build() else {
        panic!("one capability");
    };
    let resolved = Resolved::resolve(
        &acl,
        [(capability.identifier.clone(), capability)].into(),
        Target::current(),
    )
    .unwrap();
    assert!(resolved.has_app_acl);
    let mut commands: Vec<&str> = resolved.allowed_commands.keys().map(String::as_str).collect();
    commands.sort_unstable();
    assert_eq!(
        commands,
        [
            "notify",
            "open_external",
            "plugin:notification|is_permission_granted",
            "set_comms_root",
            "set_counts",
            "set_presence"
        ]
    );
    assert!(resolved.denied_commands.is_empty());
}

#[test]
fn the_camera_and_microphone_are_only_for_the_app_s_own_pages() {
    let node = Url::parse("http://127.0.0.1:43210/qapps").unwrap();
    let shell = Url::parse("http://moqui-shell.localhost/").unwrap();
    let at = |text: &str| Url::parse(text).unwrap();
    assert!(media_allowed(
        &at("http://127.0.0.1:43210/qapps/BasicComms/Call"),
        Some(&node),
        None
    ));
    assert!(!media_allowed(&at("http://127.0.0.1:43211/"), Some(&node), None));
    assert!(!media_allowed(&at("https://example.com/"), Some(&node), Some(&shell)));
    assert!(media_allowed(
        &at("http://moqui-shell.localhost/chat"),
        Some(&node),
        Some(&shell)
    ));
    assert!(
        !media_allowed(&at("http://moqui-shell.localhost/chat"), Some(&node), None),
        "not a client"
    );
    assert!(!media_allowed(&at("data:text/html,x"), Some(&node), Some(&shell)));
    assert!(
        !media_allowed(&at("http://127.0.0.1:43210/"), None, None),
        "no node yet"
    );
    // a client of a server: its node is the app windows' origin, and the shell's calls (Calls/Call, bundled) have them too
    let app = crate::client::app_origin();
    let client_shell = crate::client::shell_origin();
    for page in [
        "http://moqui-shell.localhost/",
        "http://moqui-app.localhost/qapps/BasicComms/Calls/Call?callSessionId=C1",
    ] {
        assert!(media_allowed(&at(page), Some(&app), Some(&client_shell)), "{page}");
    }
    assert!(!media_allowed(
        &at("http://moqui-other.localhost/"),
        Some(&app),
        Some(&client_shell)
    ));
    assert!(!media_allowed(
        &at("https://moqui-shell.localhost/"),
        Some(&app),
        Some(&client_shell)
    ));
}

#[test]
fn a_comms_root_is_an_app_path_only() {
    assert!(comms_root_valid("/qapps/Comms"));
    assert!(comms_root_valid("/qapps/BasicComms"));
    assert!(comms_root_valid("/qapps/my-apps/Comms_2"));
    for bad in [
        "",
        "/qapps/",
        "/apps/Comms",
        "https://example.com/qapps/x",
        "/qapps/../x",
        "/qapps/a//b",
        "/qapps/a?b",
    ] {
        assert!(!comms_root_valid(bad), "{bad}");
    }
}

#[test]
fn the_main_window_is_told_whether_the_user_sees_it() {
    assert_eq!(focus_script(true), "window.__moquiWindowFocused = true;");
    assert_eq!(focus_script(false), "window.__moquiWindowFocused = false;");
    assert!(SCRIPT.contains("windowSeen: () =>") && SCRIPT.contains("window.__moquiWindowFocused"));
}
