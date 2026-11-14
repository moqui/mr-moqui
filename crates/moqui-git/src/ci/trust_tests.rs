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

use super::{mask, mask_patterns, MASK};

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
