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

//! Files and directories only this user may read: a file `0600` and a directory `0700` on Unix, whatever the process
//! umask, for what holds a secret or a copy of the data (a password file, a data export or snapshot, a database dump,
//! the log) (plans/Security-Review-Low.md SEC-DAT-06). On other systems they are as the system makes them.

use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};

/// Open options that create a file with mode `0600`. The mode applies only to a file the open creates.
#[must_use]
pub fn file_options() -> OpenOptions {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options
}

/// Create `dir` and its missing parents, each it creates with mode `0700`. One that is there already is left as it is.
///
/// # Errors
///
/// The error of the directory that could not be created.
pub fn create_dir_all(dir: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// A new directory of this user alone (`0700`) in `parent` (the shared temp directory, say), named `prefix-` and 16
/// random hex characters, made by this call: a name already there (planted by another local user, or left over) is
/// never taken, so what is read from the directory was written there by this process or its child.
///
/// # Errors
///
/// The error of the directory that could not be made, or no new name after a few tries.
pub fn new_dir_in(parent: &Path, prefix: &str) -> io::Result<PathBuf> {
    for _ in 0..8 {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).map_err(|err| io::Error::other(err.to_string()))?;
        let suffix: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let dir = parent.join(format!("{prefix}-{suffix}"));
        // `mode` is the only mutation, and it is Unix. On Windows the builder is used as created.
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt as _;
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700);
            builder
        };
        #[cfg(not(unix))]
        let builder = std::fs::DirBuilder::new();
        match builder.create(&dir) {
            Ok(()) => return Ok(dir),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "no new directory name"))
}

/// Make the file at `path` readable and writable by this user only (`0600`).
///
/// # Errors
///
/// The error of the change: the file is missing, or another user's.
pub fn restrict(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    {
        std::fs::metadata(path).map(|_| ())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    #[test]
    fn files_and_directories_are_this_user_s_alone() {
        let base = std::env::temp_dir().join(format!("moqui-private-fs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dir = base.join("a/b");
        super::create_dir_all(&dir).unwrap();
        let mode = |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&base.join("a")), 0o700);
        assert_eq!(mode(&dir), 0o700);
        let file = dir.join("secret");
        super::file_options().write(true).create_new(true).open(&file).unwrap();
        assert_eq!(mode(&file), 0o600);
        let open = dir.join("open");
        std::fs::write(&open, "x").unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o664)).unwrap();
        super::restrict(&open).unwrap();
        assert_eq!(mode(&open), 0o600);
        // a new directory is this user's and never one already there (SEC-DEV-84)
        let fresh = super::new_dir_in(&base, "fresh").unwrap();
        assert_eq!(mode(&fresh), 0o700);
        assert_ne!(super::new_dir_in(&base, "fresh").unwrap(), fresh);
        let _ = std::fs::remove_dir_all(&base);
    }
}
