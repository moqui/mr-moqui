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

//! Packs the runtime files src/lib.rs embeds into `OUT_DIR` (`embedded.bin` and `embedded.idx`), at the level the
//! features `production` and `all` select; src/select.rs has what each level takes and the format.
//!
//! Cargo watches each packed top-level entry of a component, not the component as a whole, so an edit to a component's
//! tests (or its `.git`) does not pack again; a new top-level directory in a component, or a new component, is packed
//! once anything else changes, or when `MOQUI_EMBED_COMPONENTS` (which `./moq build` sets to the runtime's component
//! directories) changes.

#[allow(dead_code)]
#[path = "src/select.rs"]
mod select;

use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let runtime = manifest.join("../../runtime");
    let level = select::Level::from_features(
        std::env::var_os("CARGO_FEATURE_PRODUCTION").is_some(),
        std::env::var_os("CARGO_FEATURE_ALL").is_some(),
    );
    println!("cargo:rerun-if-env-changed=MOQUI_EMBED_COMPONENTS");
    println!(
        "cargo:rerun-if-changed={}",
        manifest.join("archive-excludes.txt").display()
    );

    let mut watch = |path: &Path| println!("cargo:rerun-if-changed={}", path.display());
    let files = select::gather(&runtime, &manifest, level, &mut watch).unwrap_or_else(|err| panic!("{err}"));
    let (blob, index) = select::pack(&files).unwrap_or_else(|err| panic!("{err}"));

    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    write_out(&out.join("embedded.bin"), &blob);
    write_out(&out.join("embedded.idx"), index.as_bytes());
}

fn write_out(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap_or_else(|err| panic!("could not write {}: {err}", path.display()));
}
