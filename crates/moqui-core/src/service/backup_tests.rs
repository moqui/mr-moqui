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

use std::path::PathBuf;

use super::{backup_dir, paused_for};

#[test]
fn the_call_names_the_directory_before_the_property() {
    assert_eq!(backup_dir("/call", "/property"), Some(PathBuf::from("/call")));
    assert_eq!(backup_dir("  ", " /property "), Some(PathBuf::from("/property")));
    assert_eq!(backup_dir("", ""), None);
}

#[test]
fn the_schedule_property_runs_or_pauses_the_job_and_otherwise_leaves_it() {
    assert_eq!(paused_for("true"), Some("N"));
    assert_eq!(paused_for(" TRUE "), Some("N"));
    assert_eq!(paused_for("false"), Some("Y"));
    assert_eq!(paused_for(""), None);
    assert_eq!(paused_for("maybe"), None);
}
