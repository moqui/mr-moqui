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

use std::time::{Duration, Instant};

use super::{valid_id, Capture, Source, GRANT_FOR};

fn offered(capture: &Capture, ids: &[&str]) {
    *moqui_lib::sync::lock(&capture.offered) = ids
        .iter()
        .map(|id| Source {
            id: id.to_string(),
            screen: id.starts_with("screen:"),
            name: id.to_string(),
            bounds: None,
        })
        .collect();
}

#[test]
fn a_desktop_media_id_is_a_screen_or_window_number() {
    for id in ["screen:1:0", "window:4242:0", "screen:69732928:0"] {
        assert!(valid_id(id), "{id}");
    }
    for id in [
        "",
        "screen",
        "screen:1",
        "screen:1:1",
        "tab:1:0",
        "window:-1:0",
        "window:1a:0",
        "screen::0",
        "window:1:0:0",
    ] {
        assert!(!valid_id(id), "{id}");
    }
}

/// A choice grants one request, of the window that made it, within GRANT_FOR, and only of what was offered.
#[test]
fn a_choice_grants_one_request_of_its_window_while_it_is_fresh() {
    let capture = Capture::default();
    let now = Instant::now();
    offered(&capture, &["screen:1:0", "window:7:0"]);
    assert!(capture.choose("main", "window:8:0", now).is_err(), "not offered");
    assert!(capture.choose("main", "tab:1:0", now).is_err(), "not a source");
    assert_eq!(capture.choose("main", "window:7:0", now).unwrap().id, "window:7:0");
    assert!(capture.take("app-2", now).is_none(), "another window");
    assert_eq!(
        capture.take("main", now + Duration::from_secs(1)).unwrap().id,
        "window:7:0"
    );
    assert!(capture.take("main", now).is_none(), "one request");

    capture.choose("main", "screen:1:0", now).unwrap();
    assert!(
        capture.take("main", now + GRANT_FOR + Duration::from_secs(1)).is_none(),
        "lapsed"
    );
}
