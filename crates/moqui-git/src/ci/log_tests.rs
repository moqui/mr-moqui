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

//! What a member keeps in memory of a job's log, when the job ends on another member.

use std::sync::Arc;

use super::{LogState, Watchers, MASK_MILLIS, WATCH_MILLIS};

#[test]
fn mask_patterns_of_a_job_ended_elsewhere_are_let_go() {
    let state = LogState::default();
    state.keep("J1".into(), Arc::new(vec!["s3cret".into()]), 1_000);
    assert_eq!(
        state.known("J1", 1_000 + MASK_MILLIS - 1).as_deref(),
        Some(&vec!["s3cret".to_string()])
    );
    // the job ended on another member: this one never calls forget, and the patterns go once old
    state.keep("J2".into(), Arc::new(Vec::new()), 1_000 + MASK_MILLIS);
    assert_eq!(state.known("J1", 1_000 + MASK_MILLIS + 1), None);
    assert_eq!(moqui_lib::sync::lock(&state.masks).len(), 1, "J1 swept, J2 kept");
    assert!(state.known("J2", 1_000 + 3 * MASK_MILLIS).is_none());
    assert!(moqui_lib::sync::lock(&state.masks).is_empty());
}

#[test]
fn watchers_nobody_reads_are_let_go() {
    let state = LogState::default();
    let mut watchers = Watchers::default();
    watchers.read.insert("u1".into(), 0);
    moqui_lib::sync::lock(&state.watchers).insert("J1".into(), watchers);
    state.sweep_watchers(WATCH_MILLIS - 1);
    assert_eq!(moqui_lib::sync::lock(&state.watchers).len(), 1);
    state.sweep_watchers(WATCH_MILLIS + 1);
    assert!(moqui_lib::sync::lock(&state.watchers).is_empty());
}
