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

//! Files of the client that hold an account's data (its keys, the account list, the cached settings): readable by
//! their owner only on Unix, in directories only the owner can enter, and written whole, so a crash leaves the old file
//! rather than half of a new one.

use std::io::Write;
use std::path::Path;

/// Write `bytes` to `path`: to a temporary file beside it, mode 0600, then renamed over it. The parent directories are
/// made, mode 0700.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        create_private_dir(dir)?;
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = options.open(&temporary).and_then(|mut file| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(bytes)?;
        file.sync_all()
    });
    let renamed = written.and_then(|()| std::fs::rename(&temporary, path));
    if renamed.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    renamed
}

/// Make `dir` and its missing parents, mode 0700 on Unix. An existing directory keeps its mode.
pub(crate) fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// A name that stays in its directory as a file name: letters, digits, `.`, `-` and `_`, not empty and not starting
/// with `.` (so not `..` or a hidden file). Account ids and the store's file names are such names. A name Windows
/// keeps for a device is one too, so what an earlier version wrote under it is always found and forgotten; only a new
/// file is refused such a name, where Windows would take it for the device ([`writable_name`]).
pub(crate) fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
}

/// A [`plain_name`] a new file may take on this system: on Windows not a device name ([`windows_reserved`]: an account
/// of `aux.corp.net` keeps no store there), anywhere else any plain name. Never asked of a forget.
pub(crate) fn writable_name(name: &str) -> bool {
    plain_name(name) && !(cfg!(windows) && windows_reserved(name))
}

/// Whether Windows keeps `name` for a device whatever its extension: CON, PRN, AUX, NUL, COM0 to COM9 and LPT0 to LPT9
/// before its first `.`, in any case (`nul.txt`, `Com1.tar.gz`).
pub(crate) fn windows_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    match stem.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" => true,
        _ => {
            let (head, digit) = stem.split_at(stem.len().min(3));
            (head == "COM" || head == "LPT") && digit.len() == 1 && digit.bytes().all(|b| b.is_ascii_digit())
        }
    }
}
