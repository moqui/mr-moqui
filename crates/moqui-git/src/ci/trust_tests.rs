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

//! Masking, which needs no database. Who is trusted and which secrets a job is given are tested with the engine
//! (`server/ci_tests.rs`).

use super::{mask, mask_batch, mask_patterns, MASK};

#[test]
fn secret_values_are_masked_whole_and_by_line() {
    let patterns = mask_patterns(&[
        "s3cr3t-token".to_string(),
        "s3cr3t-token-longer".to_string(),
        "-----BEGIN KEY-----\nabcdef123456\n-----END KEY-----".to_string(),
        "abc".to_string(),
    ]);
    // the longer value first, so it is not left half shown
    assert_eq!(
        mask("token=s3cr3t-token-longer and s3cr3t-token", &patterns),
        format!("token={MASK} and {MASK}")
    );
    // each line of a value of several lines, as a log prints them one by one
    assert_eq!(mask("key line: abcdef123456", &patterns), format!("key line: {MASK}"));
    // a value too short to tell from ordinary text is not masked
    assert_eq!(mask("abc is the alphabet", &patterns), "abc is the alphabet");
    assert_eq!(mask("nothing here", &patterns), "nothing here");
    assert!(mask_patterns(&[]).is_empty());
    assert_eq!(mask("as written", &[]), "as written");
}

/// A secret a runner cut between two log batches is masked on each side of the cut that is long enough to be one
/// (SEC-GIT-08): the end of a batch that starts it, the start of the next that goes on with it.
#[test]
fn a_secret_cut_between_two_batches_is_masked_on_both_sides() {
    let patterns = mask_patterns(&["s3cr3t-token-value".to_string()]);
    let (first, second) = "key=s3cr3t-token-value done\n".split_at(13);
    assert_eq!(first, "key=s3cr3t-to");
    assert_eq!(mask_batch(first, &patterns, false), format!("key={MASK}"));
    assert_eq!(mask_batch(second, &patterns, true), format!("{MASK} done\n"));
    // a batch that starts a line of its own is not taken as the rest of one
    assert_eq!(mask_batch("value done\n", &patterns, false), "value done\n");
    // the whole value in one batch, and text that only looks like a short end of it, as before
    assert_eq!(
        mask_batch("key=s3cr3t-token-value\n", &patterns, true),
        format!("key={MASK}\n")
    );
    assert_eq!(mask_batch("a line\n", &patterns, true), "a line\n");
}
