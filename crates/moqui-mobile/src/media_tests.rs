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

use super::{action, action_script, artwork, state, MediaInfo, TEXT_MAX};

#[test]
fn actions_are_start_update_stop_only() {
    assert_eq!(action("start"), Ok("start"));
    assert_eq!(action("update"), Ok("update"));
    assert_eq!(action("stop"), Ok("stop"));
    assert!(action("play").is_err());
    assert!(action("").is_err());
}

#[test]
fn artwork_is_https_with_a_host_only() {
    assert_eq!(artwork(Some("https://img.example.com/a.jpg")).as_deref(), Some("https://img.example.com/a.jpg"));
    assert_eq!(artwork(Some("http://img.example.com/a.jpg")), None);
    assert_eq!(artwork(Some("https://user:pw@img.example.com/a.jpg")), None);
    assert_eq!(artwork(Some("file:///data/a.jpg")), None);
    assert_eq!(artwork(Some("javascript:alert(1)")), None);
    assert_eq!(artwork(Some("")), None);
    assert_eq!(artwork(None), None);
    assert_eq!(artwork(Some(&format!("https://example.com/{}", "a".repeat(3000)))), None);
}

#[test]
fn state_clips_texts_and_drops_bad_numbers() {
    let info = MediaInfo {
        title: Some(format!("A\ntitle {}", "x".repeat(400))),
        artist: Some("   ".to_string()),
        artwork_url: Some("http://example.com/a.jpg".to_string()),
        playing: Some(true),
        position: Some(f64::NAN),
        duration: Some(-3.0),
        rate: Some(2.5),
    };
    let checked = state(&info);
    let title = checked.title.unwrap();
    assert!(title.starts_with("A title"));
    assert_eq!(title.chars().count(), TEXT_MAX);
    assert_eq!(checked.artist, None);
    assert_eq!(checked.artwork_url, None);
    assert_eq!(checked.playing, Some(true));
    assert_eq!(checked.position, None);
    assert_eq!(checked.duration, None);
    assert_eq!(checked.rate, Some(2.5));
    assert_eq!(state(&MediaInfo { rate: Some(0.0), ..Default::default() }).rate, None);
    assert_eq!(state(&MediaInfo { rate: Some(99.0), ..Default::default() }).rate, Some(16.0));
}

#[test]
fn state_serializes_only_what_is_given() {
    let json = serde_json::to_string(&state(&MediaInfo { playing: Some(false), position: Some(12.5), ..Default::default() })).unwrap();
    assert_eq!(json, r#"{"playing":false,"position":12.5}"#);
}

#[test]
fn page_actions_are_known_ones_with_json_arguments() {
    assert_eq!(
        action_script("pause", None).unwrap(),
        r#"window.__moquiMediaAction && window.__moquiMediaAction({"action":"pause","position":null})"#
    );
    assert!(action_script("seekto", Some(42.0)).unwrap().contains(r#""position":42.0"#));
    assert!(action_script("seekto", Some(f64::INFINITY)).unwrap().contains(r#""position":null"#));
    assert_eq!(action_script("alert(1)", None), None);
}
