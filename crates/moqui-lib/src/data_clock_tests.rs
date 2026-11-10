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

use super::*;

#[test]
fn supplied_assignments_advance_and_invalid_updates_fail_closed() {
    let path = std::env::temp_dir().join(format!(
        "moqui-data-clock-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
    ));
    let result = std::panic::catch_unwind(|| {
        std::fs::write(&path, "1700000000000\n").unwrap();
        let outer_frame = read_assignment(&path).unwrap();
        assert_eq!(outer_frame, 1_700_000_000_000);
        std::fs::write(&path, "1700000000001\n").unwrap();
        assert_eq!(read_assignment(&path).unwrap(), outer_frame + 1);
        assert_eq!(outer_frame, 1_700_000_000_000);
        for invalid in [
            "",
            "0",
            "-1",
            "1.5",
            " 12",
            "12\r\n",
            "253402300800000",
            "111111111111111111111111111111111111",
        ] {
            std::fs::write(&path, invalid).unwrap();
            assert!(read_assignment(&path).is_err(), "accepted {invalid:?}");
        }
        std::fs::remove_file(&path).unwrap();
        assert!(read_assignment(&path).is_err());
        assert!(read_assignment(Path::new("relative-clock")).is_err());
        assert!(read_assignment(&std::env::temp_dir()).is_err());
    });
    let _ = std::fs::remove_file(&path);
    if let Err(failure) = result {
        std::panic::resume_unwind(failure);
    }
}

#[test]
fn disabled_clock_uses_real_epoch_without_environment_mutation() {
    let before = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;
    let actual = from_setting(None).unwrap();
    let after = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;
    assert!((before..=after).contains(&actual));
}
