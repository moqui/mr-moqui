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

//! Packs the files src/lib.rs embeds into `OUT_DIR/embedded.tar.gz`: the framework component less its top-level
//! `test/` and `acceptance/`, under `framework/`; the webroot files `webroot-files.txt` names, under `webroot/`; and this
//! crate's `skeleton/` tree, under `skeleton/`. The tar is the same for the same files (sorted, no times or owners).
//!
//! Cargo watches each packed top-level entry, not the framework directory as a whole, so an edit to a framework test
//! does not relink the binary; a new top-level directory in the framework is packed once anything else changes.

use std::io::Write;
use std::path::{Path, PathBuf};

/// The framework's top-level directories that are not packed: what only the tests read.
const FRAMEWORK_SKIP: [&str; 2] = ["test", "acceptance"];

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let base = manifest.join("../../runtime/base-component");
    let framework = base.join("framework");
    let webroot = base.join("webroot");
    let list = manifest.join("webroot-files.txt");
    let skeleton = manifest.join("skeleton");

    let mut files: Vec<(String, PathBuf)> = Vec::new();

    let mut top: Vec<_> = std::fs::read_dir(&framework)
        .unwrap_or_else(|err| panic!("could not read {}: {err}", framework.display()))
        .map(|entry| {
            entry
                .expect("framework entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| !name.starts_with('.') && !FRAMEWORK_SKIP.contains(&name.as_str()))
        .collect();
    top.sort();
    for name in top {
        let path = framework.join(&name);
        println!("cargo:rerun-if-changed={}", path.display());
        collect(&path, &format!("framework/{name}"), &mut files);
    }

    println!("cargo:rerun-if-changed={}", list.display());
    let listed =
        std::fs::read_to_string(&list).unwrap_or_else(|err| panic!("could not read {}: {err}", list.display()));
    for line in listed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let relative = line.trim_end_matches('/');
        let path = webroot.join(relative);
        assert!(
            path.exists(),
            "webroot-files.txt names {relative}, which is not in {}",
            webroot.display()
        );
        println!("cargo:rerun-if-changed={}", path.display());
        collect(&path, &format!("webroot/{relative}"), &mut files);
    }

    println!("cargo:rerun-if-changed={}", skeleton.display());
    collect(&skeleton, "skeleton", &mut files);

    files.sort_by(|a, b| a.0.cmp(&b.0));
    files.dedup_by(|a, b| a.0 == b.0);

    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("embedded.tar.gz");
    let file = std::fs::File::create(&out).unwrap_or_else(|err| panic!("could not create {}: {err}", out.display()));
    let gz = flate2::write::GzEncoder::new(std::io::BufWriter::new(file), flate2::Compression::default());
    let mut tar = tar::Builder::new(gz);
    for (name, path) in &files {
        let bytes = std::fs::read(path).unwrap_or_else(|err| panic!("could not read {}: {err}", path.display()));
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        header.set_entry_type(tar::EntryType::Regular);
        tar.append_data(&mut header, name, bytes.as_slice())
            .unwrap_or_else(|err| panic!("could not pack {name}: {err}"));
    }
    let gz = tar.into_inner().expect("tar finish");
    let mut writer = gz.finish().expect("gzip finish");
    writer.flush().expect("flush embedded.tar.gz");
}

/// Every file under `path` (or `path` itself), named `prefix/<relative path>`, with `/` separators; dot files left out.
fn collect(path: &Path, prefix: &str, files: &mut Vec<(String, PathBuf)>) {
    if path.is_file() {
        files.push((prefix.to_string(), path.to_path_buf()));
        return;
    }
    let mut entries: Vec<_> = std::fs::read_dir(path)
        .unwrap_or_else(|err| panic!("could not read {}: {err}", path.display()))
        .map(|entry| entry.expect("directory entry"))
        .collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        collect(&entry.path(), &format!("{prefix}/{name}"), files);
    }
}
