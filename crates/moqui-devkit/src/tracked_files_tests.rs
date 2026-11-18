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

//! What git tracks: no build output. A crate built on its own (crates/moqui-mobile is outside the workspace) writes
//! `crates/<crate>/target`, and one `git add` of it once put 5,793 files of cargo output into the repository. The
//! `.gitignore` leaves such directories out; this test fails the run if one is tracked anyway. Skipped when git does
//! not answer (no git, or not a checkout).

use std::path::Path;

/// A path component that only build output uses.
const BUILD_DIRS: &[&str] = &["target", "node_modules", ".gradle"];

#[test]
fn no_build_output_is_tracked() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let Some(output) = std::process::Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(&repo)
        .output()
        .ok()
        .filter(|output| output.status.success())
    else {
        return;
    };
    let listed = String::from_utf8_lossy(&output.stdout);
    let mut found: Vec<&str> = listed
        .split('\0')
        .filter(|name| !name.is_empty())
        .filter(|name| {
            let mut parts: Vec<&str> = name.split('/').collect();
            parts.pop(); // the file's own name is not a directory
            parts.iter().any(|part| BUILD_DIRS.contains(part))
        })
        .collect();
    let count = found.len();
    found.truncate(10);
    assert!(
        count == 0,
        "{count} tracked files are under a build output directory ({}); remove them with `git rm -r --cached <dir>` \
         and check .gitignore: {found:?}",
        BUILD_DIRS.join(", ")
    );
}
