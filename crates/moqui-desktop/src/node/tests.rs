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

use super::{choose_port, dev_purpose, purpose_properties, save_port, saved_port, set_dev_tools, PURPOSE_FILE};
use std::net::{Ipv4Addr, TcpListener};

#[test]
fn the_saved_port_is_kept_while_free_and_replaced_when_taken() {
    let free = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    assert_eq!(choose_port(Some(free)).unwrap(), free);
    let taken = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = taken.local_addr().unwrap().port();
    assert_ne!(choose_port(Some(port)).unwrap(), port);
    assert_ne!(choose_port(None).unwrap(), 0);
}

#[test]
fn the_port_file_round_trips_and_a_bad_one_is_none() {
    let dir = std::env::temp_dir().join(format!("moqui-desktop-port-{}", std::process::id()));
    let path = dir.join("node-port");
    assert_eq!(saved_port(&path), None);
    save_port(&path, 51234);
    assert_eq!(saved_port(&path), Some(51234));
    std::fs::write(&path, "not a port").unwrap();
    assert_eq!(saved_port(&path), None);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_mode_is_the_variable_then_the_file_then_in_process() {
    use super::Mode;
    assert_eq!(Mode::choose(None, None), Mode::InProcess);
    assert_eq!(Mode::choose(Some("sidecar"), Some("in-process")), Mode::Sidecar);
    assert_eq!(Mode::choose(Some("in-process"), Some("sidecar")), Mode::InProcess);
    assert_eq!(Mode::choose(None, Some("sidecar\n")), Mode::Sidecar);
    assert_eq!(
        Mode::choose(Some(""), Some("sidecar")),
        Mode::Sidecar,
        "an empty variable is unset"
    );
    assert_eq!(
        Mode::choose(Some("both"), Some("sidecar")),
        Mode::Sidecar,
        "a bad variable is passed over"
    );
    assert_eq!(Mode::choose(None, Some("junk")), Mode::InProcess);
    assert_eq!(Mode::choose(Some("client"), Some("sidecar")), Mode::Client);
    assert_eq!(Mode::Client.other(), Mode::InProcess);
    for mode in [Mode::InProcess, Mode::Sidecar, Mode::Client] {
        assert_eq!(Mode::parse(mode.as_str()), Some(mode));
        assert_ne!(mode.other(), mode);
    }
}

#[test]
fn the_mode_file_round_trips() {
    use super::{save_mode, saved_mode, Mode};
    let dir = std::env::temp_dir().join(format!("moqui-desktop-mode-{}", std::process::id()));
    let path = dir.join("node-mode");
    assert_eq!(saved_mode(&path), None);
    save_mode(&path, Mode::Sidecar).unwrap();
    assert_eq!(Mode::choose(None, saved_mode(&path).as_deref()), Mode::Sidecar);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn both_modes_listen_on_loopback_only() {
    let properties = super::listen_properties(51234);
    let get = |name: &str| {
        properties
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.as_str())
    };
    assert_eq!(get("webapp_http_bind"), Some("127.0.0.1"));
    assert_eq!(get("webapp_http_port"), Some("51234"));
    assert_eq!(get("webapp_https_listen_port"), Some("off"));
    assert_eq!(get("cluster_bind_address"), Some("127.0.0.1"));
}

#[test]
fn the_dev_tools_follow_the_variable_then_the_purpose_file_and_are_off_by_default() {
    assert!(!dev_purpose(None, None), "off by default");
    assert!(dev_purpose(None, Some("dev\n")));
    assert!(!dev_purpose(None, Some("production")));
    assert!(dev_purpose(Some("dev"), None));
    assert!(!dev_purpose(Some("production"), Some("dev")), "the variable is taken first");
    assert!(dev_purpose(Some(" "), Some("dev")), "an empty variable is passed over");
    assert_eq!(purpose_properties(true), vec![("instance_purpose", "dev".to_string())]);
    assert!(purpose_properties(false).is_empty(), "off leaves the conf's own purpose");

    let dir = std::env::temp_dir().join(format!("moqui-desktop-purpose-{}", std::process::id()));
    set_dev_tools(&dir, true).unwrap();
    assert_eq!(std::fs::read_to_string(dir.join(PURPOSE_FILE)).unwrap().trim(), "dev");
    set_dev_tools(&dir, false).unwrap();
    assert!(!dir.join(PURPOSE_FILE).exists());
    set_dev_tools(&dir, false).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
