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

use super::moq_program;

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A checkout root containing `files` (empty files), removed when dropped.
fn checkout(label: &str, files: &[&str]) -> (Scratch, PathBuf) {
    let dir = std::env::temp_dir().join(format!("dev-aide-moq-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for name in files {
        std::fs::write(dir.join(name), "").unwrap();
    }
    (Scratch(dir.clone()), dir)
}

/// The launcher is `moq` on Unix and `moq.cmd` on Windows, and only when `moq` itself is there.
#[test]
fn moq_program_picks_the_launcher_this_platform_can_run() {
    let (_script, script) = checkout("script", &["moq"]);
    assert_eq!(moq_program(&script, false), Some(script.join("moq")));
    assert_eq!(moq_program(&script, true), None);

    let (_cmd, cmd) = checkout("cmd", &["moq.cmd"]);
    assert_eq!(moq_program(&cmd, false), None);
    assert_eq!(moq_program(&cmd, true), None);

    let (_both, both) = checkout("both", &["moq", "moq.cmd"]);
    assert_eq!(moq_program(&both, false), Some(both.join("moq")));
    assert_eq!(moq_program(&both, true), Some(both.join("moq.cmd")));
}
