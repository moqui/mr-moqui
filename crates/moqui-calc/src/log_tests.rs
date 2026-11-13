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

fn row(seq: i64, user: &str, kind: &str, target: Option<i64>) -> LogRow {
    LogRow {
        seq,
        user_id: user.to_string(),
        kind: kind.to_string(),
        ops: Vec::new(),
        target,
        undone: None,
    }
}

/// Apply an undo or a redo to the rows as the service does: the new row, and its target marked taken back.
fn take_back(rows: &mut Vec<LogRow>, user: &str, kind: &str, target: i64) {
    let seq = rows.last().map_or(1, |last| last.seq + 1);
    rows.push(row(seq, user, kind, Some(target)));
    if let Some(taken) = rows.iter_mut().find(|row| row.seq == target) {
        taken.undone = Some(seq);
    }
}

#[test]
fn each_person_undoes_and_redoes_their_own_edits_in_order_whatever_others_do_between() {
    let mut rows = vec![
        row(1, "ann", EDIT, None),
        row(2, "bob", EDIT, None),
        row(3, "ann", EDIT, None),
        row(4, "bob", EDIT, None),
    ];
    assert_eq!(undo_state(&rows, "ann", 0), (true, false));
    // Ann's undo takes back her newest edit, not Bob's newer one
    assert_eq!(undo_target(&rows, "ann", 0).map(|row| row.seq), Some(3));
    take_back(&mut rows, "ann", UNDO, 3);
    assert_eq!(undo_target(&rows, "ann", 0).map(|row| row.seq), Some(1));
    take_back(&mut rows, "ann", UNDO, 1);
    assert_eq!(undo_state(&rows, "ann", 0), (false, true));
    // redo takes back the newest undo first
    assert_eq!(redo_target(&rows, "ann", 0).map(|row| row.target), Some(Some(1)));
    take_back(&mut rows, "ann", REDO, 6);
    assert_eq!(redo_target(&rows, "ann", 0).map(|row| row.target), Some(Some(3)));
    // a redo is undone as an edit is
    assert_eq!(undo_target(&rows, "ann", 0).map(|row| row.kind.as_str()), Some(REDO));
    // Bob's edits are his own, and Bob has nothing to redo
    assert_eq!(undo_target(&rows, "bob", 0).map(|row| row.seq), Some(4));
    assert!(redo_target(&rows, "bob", 0).is_none());
    // a new edit of Ann's ends what she could redo
    rows.push(row(8, "ann", EDIT, None));
    assert!(redo_target(&rows, "ann", 0).is_none());
    // nothing at or before the saved edit is undone
    assert!(undo_target(&rows, "bob", 4).is_none());
    assert_eq!(undo_target(&rows, "ann", 4).map(|row| row.seq), Some(8));
}
