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
//! workspace, by their paths in it. Packing reads nothing outside the workspace, and unpacking writes nothing outside
//! it.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;

/// Write `files` (files and directories under `workspace`) to the tar.gz `out`, each by its path in the workspace
/// ([`pack_into`]).
///
/// # Errors
///
/// When a file is outside the workspace or cannot be read, or `out` cannot be written.
pub fn pack(workspace: &Path, files: &[PathBuf], out: &Path) -> Result<(), String> {
    let file = File::create(out).map_err(|err| format!("{}: {err}", out.display()))?;
    pack_into(workspace, files, file)
}

/// Write `files` (files and directories under `workspace`) as a tar.gz to `out`, each by its path in the workspace.
/// A named path is packed as what it resolves to, which must be in the workspace: one that is, or goes through, a link
/// out of it is refused. Under a directory a link is stored as a link and never followed, and a regular file is read
/// only with [`super::open_regular`] (`O_NOFOLLOW`); a FIFO, socket or device is left out (SEC-GIT-81).
///
/// # Errors
///
/// When a file is outside the workspace or cannot be read, or `out` cannot be written.
pub fn pack_into<W: Write>(workspace: &Path, files: &[PathBuf], out: W) -> Result<(), String> {
    let root = workspace
        .canonicalize()
        .map_err(|err| format!("{}: {err}", workspace.display()))?;
    let mut builder = tar::Builder::new(GzEncoder::new(out, Compression::fast()));
    builder.follow_symlinks(false);
    for path in files {
        let relative = path
            .strip_prefix(workspace)
            .map_err(|_| format!("{} is outside the workspace", path.display()))?;
        let real = path
            .canonicalize()
            .map_err(|err| format!("{}: {err}", relative.display()))?;
        if !real.starts_with(&root) {
            return Err(format!("{} is outside the workspace", relative.display()));
        }
        append_tree(&mut builder, &real, relative)?;
    }
    builder
        .into_inner()
        .and_then(GzEncoder::finish)
        .map(|_| ())
        .map_err(|err| format!("the archive was not written: {err}"))
}

/// Append `path` as `name`, and under a directory what it holds, by `symlink_metadata`: a directory is walked, a link
/// is stored as its text, a regular file is read through [`super::open_regular`], anything else is left out.
fn append_tree<W: Write>(builder: &mut tar::Builder<W>, path: &Path, name: &Path) -> Result<(), String> {
    let mut stack = vec![(path.to_path_buf(), name.to_path_buf())];
    while let Some((path, name)) = stack.pop() {
        let failed = |err: &dyn std::fmt::Display| format!("{}: {err}", name.display());
        let meta = path.symlink_metadata().map_err(|err| failed(&err))?;
        let kind = meta.file_type();
        if kind.is_dir() {
            if name != Path::new("") {
                let mut header = tar::Header::new_gnu();
                header.set_metadata_in_mode(&meta, tar::HeaderMode::Complete);
                header.set_entry_type(tar::EntryType::Directory);
                header.set_size(0);
                builder
                    .append_data(&mut header, &name, std::io::empty())
                    .map_err(|err| failed(&err))?;
            }
            let mut entries: Vec<_> = std::fs::read_dir(&path)
                .map_err(|err| failed(&err))?
                .filter_map(Result::ok)
                .map(|entry| entry.file_name())
                .collect();
            entries.sort();
            for entry in entries.into_iter().rev() {
                stack.push((path.join(&entry), name.join(&entry)));
            }
        } else if kind.is_symlink() {
            let target = std::fs::read_link(&path).map_err(|err| failed(&err))?;
            let mut header = tar::Header::new_gnu();
            header.set_metadata_in_mode(&meta, tar::HeaderMode::Complete);
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            builder
                .append_link(&mut header, &name, &target)
                .map_err(|err| failed(&err))?;
        } else if kind.is_file() {
            let mut file = super::open_regular(&path)?;
            builder.append_file(&name, &mut file).map_err(|err| failed(&err))?;
        }
    }
    Ok(())
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
