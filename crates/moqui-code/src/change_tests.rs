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

use super::{Change, Cursor, REREAD_WINDOW};

fn change(seq: i64) -> Change {
    Change {
        seq,
        change_type: "thread.upserted".into(),
        conversation_id: None,
        project_id: None,
        user_id: None,
        payload: serde_json::Value::Null,
        date: 0,
    }
}

fn seqs(changes: &[Change]) -> Vec<i64> {
    changes.iter().map(|c| c.seq).collect()
}

#[test]
fn a_cursor_skips_the_snapshot_and_rows_it_has_seen() {
    let mut cursor = Cursor::after(10);
    let first = cursor.take([8, 10, 11, 12].into_iter().map(change));
    assert_eq!(seqs(&first), vec![11, 12]);
    let again = cursor.take([8, 10, 11, 12, 13].into_iter().map(change));
    assert_eq!(seqs(&again), vec![13]);
    assert_eq!(cursor.top(), 13);
}

#[test]
fn a_late_commit_inside_the_window_is_read() {
    let mut cursor = Cursor::after(0);
    assert_eq!(seqs(&cursor.take([1, 2, 4].into_iter().map(change))), vec![1, 2, 4]);
    // 3 commits after 4 was read
    assert_eq!(seqs(&cursor.take([1, 2, 3, 4].into_iter().map(change))), vec![3]);
}

#[test]
fn the_seen_set_keeps_only_the_window() {
    let mut cursor = Cursor::after(0);
    let rows: Vec<i64> = (1..=1000).collect();
    let read = cursor.take(rows.into_iter().map(change));
    assert_eq!(read.len(), 1000);
    assert!(cursor.seen.len() <= usize::try_from(REREAD_WINDOW).unwrap());
    assert!(cursor.take((900..=1000).map(change)).is_empty());
}
