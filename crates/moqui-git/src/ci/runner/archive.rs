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

//! The archives the runner makes and reads: an artifact or a cache is a tar.gz of files and directories of the
//! workspace, by their paths in it. Unpacking writes nothing outside the workspace.

use std::fs::File;
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;

/// Write `files` (files and directories under `workspace`) to the tar.gz `out`, each by its path in the workspace.
/// Symbolic links are stored as links.
///
/// # Errors
///
/// When a file is outside the workspace or cannot be read, or `out` cannot be written.
pub fn pack(workspace: &Path, files: &[PathBuf], out: &Path) -> Result<(), String> {
    let file = File::create(out).map_err(|err| format!("{}: {err}", out.display()))?;
    let mut builder = tar::Builder::new(GzEncoder::new(file, Compression::fast()));
    builder.follow_symlinks(false);
    for path in files {
        let relative = path
            .strip_prefix(workspace)
            .map_err(|_| format!("{} is outside the workspace", path.display()))?;
        let added = if path.is_dir() {
            builder.append_dir_all(relative, path)
        } else {
            builder.append_path_with_name(path, relative)
        };
        added.map_err(|err| format!("{}: {err}", path.display()))?;
    }
    builder
        .into_inner()
        .and_then(GzEncoder::finish)
        .map(|_| ())
        .map_err(|err| format!("{}: {err}", out.display()))
}

/// Unpack the tar.gz `archive` into `workspace`. The count of entries written. An entry whose path would leave the
/// workspace is skipped, as `tar` refuses it.
///
/// # Errors
///
/// When the archive does not read or an entry cannot be written.
pub fn unpack(archive: &Path, workspace: &Path) -> Result<usize, String> {
    let file = File::open(archive).map_err(|err| format!("{}: {err}", archive.display()))?;
    let mut tar = tar::Archive::new(GzDecoder::new(file));
    tar.set_overwrite(true);
    let mut count = 0;
    for entry in tar
        .entries()
        .map_err(|err| format!("the archive does not read: {err}"))?
    {
        let mut entry = entry.map_err(|err| format!("the archive does not read: {err}"))?;
        // unpack_in answers false for a path that would leave the directory
        if entry
            .unpack_in(workspace)
            .map_err(|err| format!("an entry was not written: {err}"))?
        {
            count += 1;
        }
    }
    Ok(count)
}
