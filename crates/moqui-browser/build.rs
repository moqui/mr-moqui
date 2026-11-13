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

//! Link arguments only this crate's emscripten module takes: the exports and the preload package.
//! Target-wide flags (memory, stack, module shape) are in `.cargo/config.toml`.

use std::path::PathBuf;

/// `src/exports.rs`, plus `main`. emcc names C symbols with a leading underscore.
const EXPORTS: &[&str] = &[
    "main",
    "moqui_alloc",
    "moqui_dealloc",
    "moqui_request",
    "moqui_init",
    "moqui_call_service",
    "moqui_render_screen",
    "moqui_entity_find",
    "moqui_result_ptr",
    "moqui_result_len",
    "moqui_free_handle",
    "moqui_mount_opfs",
    "moqui_ws_binary",
    "moqui_ws_outbox_len",
    "moqui_ws_drain",
];

/// Exports that can run framework code, so can reach a suspending import (`fetch`, or any file
/// operation on the OPFS mount). Each is a JSPI export and returns a Promise in JS. An export that
/// can reach a suspending import but is missing here traps at the suspension.
const SUSPENDING: &[&str] = &[
    "main",
    "moqui_request",
    "moqui_init",
    "moqui_call_service",
    "moqui_render_screen",
    "moqui_entity_find",
    "moqui_mount_opfs",
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("emscripten") {
        return;
    }
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let framework = manifest.join("../../runtime/components/base/framework");
    let runtime = manifest.join("runtime");
    println!("cargo:rerun-if-changed={}", runtime.display());
    let exported: Vec<String> = EXPORTS.iter().map(|name| format!("_{name}")).collect();
    println!("cargo:rustc-link-arg-bins=-sEXPORTED_FUNCTIONS={}", exported.join(","));
    println!("cargo:rustc-link-arg-bins=-sJSPI_EXPORTS={}", SUSPENDING.join(","));
    // The fetch import (src/fetch.rs).
    let library = manifest.join("emlib/moqui_fetch.js");
    println!("cargo:rerun-if-changed={}", library.display());
    println!("cargo:rustc-link-arg-bins=--js-library={}", library.display());
    // Read-only framework and the browser runtime conf, in MEMFS at /moqui. The factory writes
    // runtime/.env there; MEMFS is writable, and nothing in it outlives the page.
    println!(
        "cargo:rustc-link-arg-bins=--preload-file={}@/moqui/framework",
        framework.display()
    );
    println!(
        "cargo:rustc-link-arg-bins=--preload-file={}@/moqui/runtime",
        runtime.display()
    );
}
