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

//! The component template (`runtime/dev-component/template`, its README): a component that `./moq component-create`
//! makes from it passes its own specs under `moqui test`, whole (its screens in qvt and tui, its services, its REST API)
//! and REST API only. The template itself, beside them in `dev-component`, never loads.

use std::path::Path;
use std::process::Command;

use super::repo;
use super::test::{moqui_test, test_runtime};

fn component_create(runtime: &Path, args: &[&str]) {
    let output = Command::new("bash")
        .arg(repo().join("moq"))
        .arg("component-create")
        .args(args)
        .arg("--no-git")
        .current_dir(repo())
        .env("MOQUI_RUNTIME", runtime)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn a_component_from_the_template_passes_its_specs() {
    let Some(runtime) = test_runtime("template", &[]) else {
        return;
    };
    std::fs::create_dir_all(runtime.dir.join("dev-component")).unwrap();
    std::os::unix::fs::symlink(
        repo().join("runtime/dev-component/template"),
        runtime.dir.join("dev-component/template"),
    )
    .unwrap();
    component_create(&runtime.dir, &["tpl-shop"]);
    component_create(&runtime.dir, &["tpl-api", "--rest"]);
    let (out, err, ok) = moqui_test(&runtime, &["tpl-shop", "tpl-api"]);
    assert!(ok, "{out}\n{err}");
    assert!(
        out.contains("TplShopScreens > \"EditItem\" renders in \"tui\"  PASSED"),
        "{out}"
    );
    assert!(
        out.contains("TplShopServices > create#Item refuses an item with no name  PASSED"),
        "{out}"
    );
    assert!(
        out.contains("TplShopApi > POST creates an item that GET then returns  PASSED"),
        "{out}"
    );
    assert!(out.contains("TplApiApi > GET lists the demo items  PASSED"), "{out}");
    assert!(!out.contains("TplApiScreens"), "{out}");
}
