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

//! Gate 4: type generation emits declarations for a sample of framework and tools services and screens, and the output
//! passes `tsc --noEmit`. `tsc` runs when it is on PATH; otherwise the check says so and the declarations are still
//! checked by content. Node is not a build dependency.

use std::path::{Path, PathBuf};
use std::process::Command;

use moqui_host::types::generate_types;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_dir(name: &str) -> DirGuard {
    let dir = std::env::temp_dir().join(format!("moqui-types-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    DirGuard(dir)
}

/// `tsc --noEmit -p dir`, or `None` when `tsc` is not on PATH.
fn tsc(dir: &Path) -> Option<std::process::Output> {
    match Command::new("tsc").arg("--noEmit").arg("-p").arg(dir).output() {
        Ok(output) => Some(output),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => panic!("tsc: {err}"),
    }
}

fn assert_tsc(dir: &Path, what: &str) {
    match tsc(dir) {
        Some(output) => assert!(
            output.status.success(),
            "tsc --noEmit over {what}:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
        None => eprintln!("tsc is not on PATH; skipped tsc --noEmit over {what}"),
    }
}

#[test]
fn declarations_for_framework_and_tools_pass_tsc() {
    let files = generate_types(
        &repo().join("runtime"),
        &repo().join("runtime/base-component/framework"),
    )
    .unwrap();
    let get = |name: &str| files.get(name).unwrap_or_else(|| panic!("no {name}"));

    // Framework entities, with the dictionary types.
    let entities = get("entities.d.ts");
    assert!(entities.contains("interface moqui_basic_Enumeration {"), "{entities}");
    assert!(entities.contains("\"moqui.basic.Enumeration\": MoquiEntity.moqui_basic_Enumeration;"));
    assert!(entities.contains("\"Enumeration\": MoquiEntity.moqui_basic_Enumeration;"));

    // Framework services: a required key, a defaulted key, auto-parameters expanded against the model.
    let services = get("services.d.ts");
    let find = services
        .split("\"org.moqui.impl.BasicServices.find#Enumeration\": {")
        .nth(1)
        .and_then(|rest| rest.split("\n    }").next())
        .unwrap();
    assert!(find.contains("\"enumTypeId\": string;"), "{find}");
    let update = services
        .split("\"org.moqui.impl.UserServices.update#UserAccount\": {")
        .nth(1)
        .and_then(|rest| rest.split("\n    }").next())
        .unwrap();
    assert!(update.contains("\"emailAddress\"?: string | null;"), "{update}");

    // Tools screens: the keys the definition proves, and the package of a server-static shell.
    let screens = get("screens.d.ts");
    let detail = screens
        .split("interface tools_screen_System_Security_ArtifactGroup_ArtifactGroupDetail {")
        .nth(1)
        .and_then(|rest| rest.split("\n    }").next())
        .unwrap();
    assert!(
        detail.contains("\"artifactGroup\"?: EntityValue<\"moqui.security.ArtifactGroup\"> | null;"),
        "{detail}"
    );
    assert!(
        detail.contains("\"memberList\"?: EntityList<\"moqui.security.ArtifactGroupMember\">;"),
        "{detail}"
    );
    assert!(
        screens
            .contains("\"component://tools/screen/System.xml\": MoquiScreenPackage<MoquiScreen.tools_screen_System,"),
        "{screens}"
    );
    // Framework screens load from the framework component.
    assert!(
        screens.contains("\"component://framework/screen/NotificationEmail.xml\":"),
        "{screens}"
    );

    let out = temp_dir("framework");
    files.write(&out.0).unwrap();
    std::fs::copy(
        repo().join("crates/moqui-host/tests/typegen/usage-framework.ts"),
        out.0.join("usage-framework.ts"),
    )
    .unwrap();
    assert_tsc(&out.0, "the framework and tools declarations");
}

/// The golden output of the core emitter tests, with the authored file that exercises it.
#[test]
fn golden_fixture_declarations_pass_tsc() {
    let expected = repo().join("crates/moqui-devkit/src/typegen/testdata/expected");
    let out = temp_dir("fixture");
    for entry in std::fs::read_dir(&expected).unwrap() {
        let path = entry.unwrap().path();
        std::fs::copy(&path, out.0.join(path.file_name().unwrap())).unwrap();
    }
    std::fs::copy(
        repo().join("crates/moqui-devkit/src/typegen/testdata/usage-fixture.ts"),
        out.0.join("usage-fixture.ts"),
    )
    .unwrap();
    assert_tsc(&out.0, "the fixture declarations");
}
