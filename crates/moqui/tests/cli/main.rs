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

//! The binary's commands as one test binary: each runs `moqui` as a subprocess over the tree.

use std::path::PathBuf;
use std::process::Command;

mod check;
mod lint;
mod serve;
mod template;
mod test;
mod test_client;
mod types;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn moqui(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_moqui"))
        .args(args)
        .current_dir(repo())
        .env("MOQUI_RUNTIME", repo().join("runtime"))
        .env("MOQUI_FRAMEWORK", repo().join("runtime/base-component/framework"))
        .output()
        .unwrap()
}
