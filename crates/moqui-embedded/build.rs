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

//! Packs the files src/lib.rs embeds into `OUT_DIR`: the framework and tools components less their top-level `test/`
//! (the regression and acceptance suites), under `framework/` and `tools/`; the webroot files `webroot-files.txt`
//! names, under `webroot/`; this crate's `skeleton/` tree, under `skeleton/`; and the runtime's `conf/`, under `conf/`.
//!
//! Each file is deflated on its own into `embedded.bin`, so one file is read without unpacking the rest, and
//! `embedded.idx` lists them, one line each: `name<TAB>offset<TAB>packed length<TAB>size`, sorted by name. Both are the
//! same for the same files.
//!
//! Cargo watches each packed top-level entry, not a component directory as a whole, so an edit to a component's tests
//! does not relink the binary; a new top-level directory in a component is packed once anything else changes.

use std::io::Write;
use std::path::{Path, PathBuf};

/// The components packed whole, less [`COMPONENT_SKIP`].
const WHOLE_COMPONENTS: [&str; 2] = ["framework", "tools"];

/// The top-level directories of a whole component that are not packed: what only the tests read.
const COMPONENT_SKIP: [&str; 1] = ["test"];

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let runtime = manifest.join("../../runtime");
    let base = runtime.join("components/base");
    let webroot = base.join("webroot");
    let list = manifest.join("webroot-files.txt");
    let skeleton = manifest.join("skeleton");

    let mut files: Vec<(String, PathBuf)> = Vec::new();

    for component in WHOLE_COMPONENTS {
        pack_top_entries(&base.join(component), component, &COMPONENT_SKIP, &mut files);
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
    // the sets' manifest a skeleton runtime needs, since only the sets it lists load (plans/archive/Components-Dir.md)
    let sets = runtime.join("components/components.xml");
    println!("cargo:rerun-if-changed={}", sets.display());
    files.push(("skeleton/components/components.xml".to_string(), sets));

    // the example and starter confs (runtime/conf/README.md)
    pack_top_entries(&runtime.join("conf"), "conf", &[], &mut files);

    files.sort_by(|a, b| a.0.cmp(&b.0));
    files.dedup_by(|a, b| a.0 == b.0);

    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let mut blob = Vec::new();
    let mut index = String::new();
    for (name, path) in &files {
        assert!(
            !name.contains(['\t', '\n', '\r']),
            "{name}: a packed file name has no tab or line break"
        );
        let bytes = std::fs::read(path).unwrap_or_else(|err| panic!("could not read {}: {err}", path.display()));
        let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(&bytes).expect("deflate");
        let packed = encoder.finish().expect("deflate finish");
        index.push_str(&format!("{name}\t{}\t{}\t{}\n", blob.len(), packed.len(), bytes.len()));
        blob.extend_from_slice(&packed);
    }
    write_out(&out.join("embedded.bin"), &blob);
    write_out(&out.join("embedded.idx"), index.as_bytes());
}

/// Pack each top-level entry of `dir` under `prefix`, less dot files and the `skip` names, watching each one.
fn pack_top_entries(dir: &Path, prefix: &str, skip: &[&str], files: &mut Vec<(String, PathBuf)>) {
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut top: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|err| panic!("could not read {}: {err}", dir.display()))
        .map(|entry| {
            entry
                .expect("directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| !name.starts_with('.') && !skip.contains(&name.as_str()))
        .collect();
    top.sort();
    for name in top {
        let path = dir.join(&name);
        println!("cargo:rerun-if-changed={}", path.display());
        collect(&path, &format!("{prefix}/{name}"), files);
    }
}

fn write_out(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap_or_else(|err| panic!("could not write {}: {err}", path.display()));
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
