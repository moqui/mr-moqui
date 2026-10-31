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

use super::media_config_from;

fn config(pairs: &[(&str, &str)]) -> Result<Option<super::MediaConfig>, String> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    media_config_from(&|name: &str| map.get(name).cloned().unwrap_or_default())
}

#[test]
fn media_is_off_without_a_port() {
    assert_eq!(config(&[]), Ok(None));
    assert_eq!(config(&[("rtc_media_port", "  ")]), Ok(None));
}

#[test]
fn media_binds_loopback_by_default_and_advertises_its_bind() {
    let c = config(&[("rtc_media_port", "10000")]).expect("valid").expect("on");
    assert_eq!(c.bind.to_string(), "127.0.0.1:10000");
    assert_eq!(c.public, c.bind);
}

#[test]
fn a_public_address_is_an_ip_or_an_ip_and_port() {
    let c = config(&[
        ("rtc_media_port", "10000"),
        ("rtc_media_host", "0.0.0.0"),
        ("rtc_public_address", "203.0.113.5"),
    ])
    .expect("valid")
    .expect("on");
    assert_eq!(c.bind.to_string(), "0.0.0.0:10000");
    assert_eq!(c.public.to_string(), "203.0.113.5:10000");
    let c = config(&[("rtc_media_port", "10000"), ("rtc_public_address", "203.0.113.5:443")])
        .expect("valid")
        .expect("on");
    assert_eq!(c.public.to_string(), "203.0.113.5:443");
}

#[test]
fn a_bad_port_or_address_is_an_error() {
    assert!(config(&[("rtc_media_port", "media")]).is_err());
    assert!(config(&[("rtc_media_port", "10000"), ("rtc_public_address", "not an address")]).is_err());
}

#[test]
fn a_room_keeps_one_notice_of_each_kind_for_its_joiners() {
    use super::driver::RoomNotices;
    use super::NoticeKind;
    use serde_json::json;

    let mut notices = RoomNotices::default();
    assert!(notices.of("call-1").is_empty());
    // captions on first, then a recording: a joiner gets both, the recording's first
    let captions = json!({"rtc": "captions", "state": "on", "provider": "whisper"});
    notices.set("call-1", NoticeKind::Captions, Some(captions.clone()));
    let recording = json!({"rtc": "recording", "state": "recording", "callRecordingId": "R1"});
    notices.set("call-1", NoticeKind::Recording, Some(recording));
    // a newer notice of a kind takes the place of the older
    let paused = json!({"rtc": "recording", "state": "paused", "callRecordingId": "R1"});
    notices.set("call-1", NoticeKind::Recording, Some(paused.clone()));
    assert_eq!(notices.of("call-1"), vec![paused.clone(), captions]);
    assert!(notices.of("call-2").is_empty(), "another room has none");
    // captions off: the recording's stays
    notices.set("call-1", NoticeKind::Captions, None);
    assert_eq!(notices.of("call-1"), vec![paused]);
    notices.set(
        "call-1",
        NoticeKind::Captions,
        Some(json!({"rtc": "captions", "state": "on"})),
    );
    notices.room_closed("call-1");
    assert!(notices.of("call-1").is_empty());
}
