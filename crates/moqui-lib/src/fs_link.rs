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

//! A link to a file or a directory that the platform lets this user make: what a scratch runtime, a test fixture or a
//! tool that builds one runtime from another uses to reach the real files without copying them.

use std::io;
use std::path::{Path, PathBuf};

/// Link `to` to `from`. On Unix a symbolic link. On Windows a directory becomes a junction and a file a hard link,
/// which need no extra privilege (a symbolic link needs Developer Mode or one); a hard link cannot cross volumes, so a
/// file on another one is copied. A copy does not follow a later write to `from`.
///
/// # Errors
///
/// Returns the error of reading `from` (on Windows) or of making the link or the copy.
pub fn link(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(from, to)
    }
    #[cfg(windows)]
    {
        let from = plain_path(from);
        let to = plain_path(to);
        if std::fs::metadata(&from)?.is_dir() {
            junction::create(&from, &to)
        } else if std::fs::hard_link(&from, &to).is_ok() {
            Ok(())
        } else {
            std::fs::copy(&from, &to).map(|_| ())
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (from, to);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "a link needs a symbolic link, a junction or a hard link",
        ))
    }
}

/// `path` without the verbatim prefix (`\\?\`) that `canonicalize` gives it on Windows; elsewhere `path` itself. A
/// junction stores an NT path, and a second prefix is a link that does not open; Git for Windows refuses a verbatim
/// working directory or `GIT_DIR` ("Invalid argument").
#[must_use]
pub fn plain_path(path: &Path) -> PathBuf {
    #[cfg(not(windows))]
    {
        path.to_path_buf()
    }
    #[cfg(windows)]
    {
        plain_windows_path(path)
    }
}

#[cfg(windows)]
fn plain_windows_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        PathBuf::from(rest.to_string())
    } else {
        path.to_path_buf()
    }
}

#[cfg(test)]
#[path = "fs_link_tests.rs"]
mod tests;
