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
mod skeleton;
mod template;
mod test;
mod test_client;
mod test_load;
mod types;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn moqui(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_moqui"))
        .env("MOQUI_SHARED_RUNTIME", "off")
        .args(args)
        .current_dir(repo())
        .env("MOQUI_RUNTIME", repo().join("runtime"))
        .env("MOQUI_FRAMEWORK", repo().join("runtime/components/base/framework"))
        .output()
        .unwrap()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn installed_dispatch_resolves_the_runtime_before_a_command_from_an_empty_directory() {
    let empty = std::env::temp_dir().join(format!("moqui-installed-cli-{}", std::process::id()));
    std::fs::create_dir_all(&empty).unwrap();
    let shared = empty.join("installed");
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_moqui"))
            .current_dir(&empty)
            .env("MOQUI_RUNTIME", empty.join("other"))
            .env("MOQUI_SHARED_RUNTIME", &shared)
            .args(["--installed", "embedded", "diff", "framework/component.xml"])
            .output()
            .unwrap()
    };
    let missing = run();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("run moqui install"));
    assert!(!shared.exists());
    let framework = shared.join("components/base/framework");
    std::fs::create_dir_all(&framework).unwrap();
    std::fs::write(framework.join("component.xml"), "<component name='installed-only'/>").unwrap();
    let found = run();
    let output = String::from_utf8_lossy(&found.stdout);
    assert!(output.contains("installed-only"), "{output}");
    assert!(String::from_utf8_lossy(&found.stderr).contains("overrides MOQUI_RUNTIME"));
    assert!(!empty.join("runtime").exists());
    std::fs::remove_dir_all(empty).unwrap();
}
