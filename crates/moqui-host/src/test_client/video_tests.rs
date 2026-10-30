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

use std::time::Duration;

use moqui_core::testkit::Failure;

use super::*;

fn case(name: &str, outcome: Outcome, seconds: f64) -> CaseResult {
    CaseResult {
        name: name.to_string(),
        outcome,
        duration: Duration::from_secs_f64(seconds),
        output: String::new(),
    }
}

fn frame(session: &str, timestamp: Option<f64>, received: f64) -> ScreencastFrame {
    // a 1x1 JPEG header is enough for the size; the bytes are not decoded here
    let mut data = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x02, 0x58, 0x03, 0x20, 0x03];
    data.extend([0u8; 16]);
    ScreencastFrame {
        session: session.to_string(),
        data,
        timestamp,
        received,
    }
}

#[test]
fn a_timeline_entry_has_its_status_reason_and_times() {
    let passed = timeline_entry(&case("a", Outcome::Passed, 1.5), 4.0);
    assert_eq!(
        passed,
        TimelineEntry {
            feature: "a".to_string(),
            start: 2.5,
            end: 4.0,
            status: "passed",
            reason: None
        }
    );
    let skipped = timeline_entry(&case("b", Outcome::Skipped("phase is before".to_string()), 0.0), 4.25);
    assert_eq!(
        (skipped.status, skipped.reason.as_deref()),
        ("skipped", Some("phase is before"))
    );
    assert_eq!((skipped.start, skipped.end), (4.25, 4.25));
    let ignored = timeline_entry(&case("c", Outcome::Ignored("later".to_string()), 0.0), 1.0);
    assert_eq!(ignored.status, "skipped");
    let failure = Failure {
        kind: "assertion".to_string(),
        block: "expect".to_string(),
        message: "1 == 2".to_string(),
        location: None,
        stack: String::new(),
        details: Vec::new(),
        attachments: Vec::new(),
    };
    let failed = timeline_entry(&case("d", Outcome::Failed(failure), 9.0), 2.0);
    assert_eq!((failed.status, failed.reason.as_deref()), ("failed", Some("1 == 2")));
    // a case longer than the video so far starts at 0
    assert_eq!(failed.start, 0.0);
    assert_eq!(
        passed.to_json(),
        serde_json::json!({"feature": "a", "start": 2.5, "end": 4.0, "status": "passed", "reason": null})
    );
}

#[test]
fn a_frame_takes_its_own_timestamp_when_it_is_near_when_it_was_read() {
    assert_eq!(frame_time(Some(101.25), 101.5, 100.0), 1.25);
    assert_eq!(frame_time(None, 101.5, 100.0), 1.5);
    // a timestamp on another clock is not used
    assert_eq!(frame_time(Some(12.0), 101.5, 100.0), 1.5);
    assert_eq!(frame_time(Some(99.0), 99.5, 100.0), 0.0);
}

#[test]
fn the_concat_list_gives_each_frame_its_time_until_the_next() {
    let frames = vec![
        (1.0, "a.jpg".to_string()),
        (1.5, "b.jpg".to_string()),
        (1.5, "c.jpg".to_string()),
        (4.0, "it's.jpg".to_string()),
    ];
    let list = concat_list(&frames, 10.0, Some("blank.jpg"));
    assert_eq!(
        list,
        "ffconcat version 1.0\n\
         file 'blank.jpg'\nduration 1.000\n\
         file 'a.jpg'\nduration 0.500\n\
         file 'c.jpg'\nduration 2.500\n\
         file 'it'\\''s.jpg'\nduration 6.000\n\
         file 'it'\\''s.jpg'\n"
    );
    // a static page: one frame lasts the whole spec
    let one = concat_list(&[(0.0, "a.jpg".to_string())], 7.5, Some("blank.jpg"));
    assert_eq!(
        one,
        "ffconcat version 1.0\nfile 'a.jpg'\nduration 7.500\nfile 'a.jpg'\n"
    );
    // a frame at the very end still shows
    assert!(concat_list(&[(3.0, "a.jpg".to_string())], 3.0, None).contains("duration 0.040"));
    assert_eq!(concat_list(&[], 3.0, None), "ffconcat version 1.0\n");
}

#[test]
fn a_jpeg_tells_its_size() {
    // SOI, an APP0 segment, then SOF0 of 800x600
    let mut data = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00];
    data.extend([
        0xFF, 0xC0, 0x00, 0x11, 0x08, 0x02, 0x58, 0x03, 0x20, 0x03, 0, 0, 0, 0, 0, 0,
    ]);
    assert_eq!(jpeg_size(&data), Some((800, 600)));
    assert_eq!(jpeg_size(b"\x89PNG...."), None);
    assert_eq!(jpeg_size(&[0xFF, 0xD8]), None);
}

#[test]
fn the_video_shows_the_page_opened_last_and_the_one_before_when_it_closes() {
    let out = std::env::temp_dir().join(format!("moqui-video-recorder-{}", std::process::id()));
    let mut recorder = Recorder::new("Spec", &out, true);
    let t0 = recorder.t0;
    recorder.page_opened("one");
    recorder.frame(frame("one", Some(t0 + 0.5), t0 + 0.5));
    recorder.page_opened("two");
    recorder.frame(frame("two", Some(t0 + 1.0), t0 + 1.0));
    // the page under it changes: kept for later, not shown
    recorder.frame(frame("one", Some(t0 + 1.2), t0 + 1.2));
    recorder.page_closed("two");
    // a late frame of the closed page is not shown
    recorder.frame(frame("two", Some(t0 + 1.3), t0 + 1.3));
    let shown: Vec<&str> = recorder
        .shown
        .iter()
        .map(|(_, path)| path.rsplit('/').next().unwrap())
        .collect();
    assert_eq!(shown, vec!["frame-000001.jpg", "frame-000002.jpg", "frame-000003.jpg"]);
    assert_eq!(recorder.shown[0].0, 0.5);
    assert!(recorder.shown.windows(2).all(|pair| pair[0].0 <= pair[1].0));
    assert_eq!(recorder.size, (800, 600));
    recorder.case(&case("a", Outcome::Passed, 0.1));
    // with nothing shown there is no video; the timeline is written and the frames removed
    recorder.shown.clear();
    assert_eq!(recorder.finish(), None);
    let timeline: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("video/Spec.timeline.json")).unwrap()).unwrap();
    assert_eq!(timeline.as_array().unwrap().len(), 1);
    assert_eq!(timeline[0]["status"], "passed");
    assert!(!out.join(format!("video/.frames-Spec-{}", std::process::id())).exists());
    let _ = std::fs::remove_dir_all(&out);
}
