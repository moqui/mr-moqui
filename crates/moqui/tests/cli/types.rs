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

//! `moqui types --out dir` writes the declarations and exits 0. The content and `tsc` are the host's `types` test.

use std::process::Command;

use crate::repo;

#[test]
fn types_writes_the_declarations_into_out() {
    let out = std::env::temp_dir().join(format!("moqui-types-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let output = Command::new(env!("CARGO_BIN_EXE_moqui"))
        .args(["types", "--out", &out.display().to_string()])
        .current_dir(repo())
        .env("MOQUI_RUNTIME", repo().join("runtime"))
        .env("MOQUI_FRAMEWORK", repo().join("runtime/components/base/framework"))
        .output()
        .unwrap();
    let names: Vec<String> = std::fs::read_dir(&out)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    let _ = std::fs::remove_dir_all(&out);
    assert!(
        output.status.success(),
        "{:?}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("wrote 8 files"));
    for name in [
        "moqui-host.d.ts",
        "entities.d.ts",
        "services.d.ts",
        "screens.d.ts",
        "shape.ts",
        "tools.d.ts",
        "tsconfig.json",
    ] {
        assert!(names.iter().any(|found| found == name), "{name} in {names:?}");
    }

    // An argument it does not know is the usage error.
    let output = Command::new(env!("CARGO_BIN_EXE_moqui"))
        .args(["types", "--bogus"])
        .current_dir(repo())
        .output()
        .unwrap();
    assert!(!output.status.success());
}
