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

use super::{cast_output, history_output, strip_queries, Keep, Recorder, Wrote, HISTORY_MAX};

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("moqui-record-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn the_terminals_queries_are_left_out_and_a_split_one_waits_for_its_end() {
    let (kept, rest) = strip_queries(
        b"a\x1b[c b\x1b[>c c\x1b[6n d\x1b]11;?\x07 e\x1b[31mred\x1b]0;title\x07",
        false,
    );
    assert_eq!(kept, b"a b c d e\x1b[31mred\x1b]0;title\x07");
    assert!(rest.is_empty());
    let (kept, rest) = strip_queries(b"ok\x1b[3", false);
    assert_eq!((kept.as_slice(), rest), (&b"ok"[..], &b"\x1b[3"[..]));
    let (kept, rest) = strip_queries(b"ok\x1b[3", true);
    assert_eq!((kept.as_slice(), rest), (&b"ok\x1b[3"[..], &b""[..]));
}

#[test]
fn a_recording_is_asciicast_with_its_resizes_and_stops_at_its_limit() {
    let dir = scratch("file");
    let path = dir.join("bob").join("s1-1.cast");
    let mut recorder = Recorder::start(
        Keep::File {
            path: path.clone(),
            limit: 40,
        },
        80,
        24,
    )
    .unwrap();
    assert_eq!(recorder.output(b"hello \xc3").unwrap(), Wrote::Kept);
    assert_eq!(recorder.output(b"\xa9 \x1b[6nworld\r\n").unwrap(), Wrote::Kept);
    recorder.resize(100, 30).unwrap();
    assert_eq!(recorder.output(&[b'x'; 40]).unwrap(), Wrote::Full);
    assert_eq!(recorder.output(b"more").unwrap(), Wrote::Full);
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let header: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(
        (header["version"].as_u64(), header["width"].as_u64()),
        (Some(2), Some(80))
    );
    assert!(lines.iter().any(|line| line.contains(r#""r","100x30""#)), "{text}");
    assert!(!text.contains("more"));
    assert_eq!(&cast_output(&path)[..16], "hello é world\r\n".as_bytes());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(
        std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_history_rolls_its_segments_and_reads_back_in_order() {
    let dir = scratch("history");
    let keep = || Keep::History {
        dir: dir.clone(),
        stem: "s1-history".to_string(),
    };
    let mut recorder = Recorder::start(keep(), 80, 24).unwrap();
    let chunk = vec![b'y'; 256 * 1024];
    for _ in 0..48 {
        recorder.output(&chunk).unwrap();
    }
    recorder.output(b"the end").unwrap();
    recorder.finish().unwrap();
    let kept = history_output(&dir, "s1-history");
    assert!(kept.len() as u64 <= HISTORY_MAX, "{}", kept.len());
    assert!(kept.len() as u64 > HISTORY_MAX / 2);
    assert!(kept.ends_with(b"the end"));
    // started again, it goes on after what it kept
    let mut again = Recorder::start(keep(), 80, 24).unwrap();
    again.output(b" again").unwrap();
    again.finish().unwrap();
    assert!(history_output(&dir, "s1-history").ends_with(b"the end again"));
    super::remove_history(&dir, "s1-history");
    assert!(history_output(&dir, "s1-history").is_empty());
    let _ = std::fs::remove_dir_all(dir);
}
