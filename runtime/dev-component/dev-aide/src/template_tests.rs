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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

/// A directory under the system temp directory, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "dev-aide-template-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Scratch(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn component_template() -> PathBuf {
    repo().join("runtime/dev-component/template")
}

/// Each file under `dir` (not `.git`) by its path there, with its text.
fn tree(dir: &Path) -> BTreeMap<String, String> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if entry.file_name() == ".git" {
                continue;
            }
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                let relative = path.strip_prefix(base).unwrap().to_string_lossy().into_owned();
                out.insert(relative, std::fs::read_to_string(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

#[test]
fn a_name_has_the_forms_of_the_placeholders() {
    let names = Names::of("my-shop");
    assert_eq!(
        names.fill("@component@ @Title@ @Class@ @package@ @ID@"),
        "my-shop My Shop MyShop myshop MY_SHOP"
    );
    assert_eq!(
        Names::of("shop").fill("@component@ @Title@ @Class@ @package@ @ID@"),
        "shop Shop Shop shop SHOP"
    );
    assert_eq!(
        Names::of("acme_Store2").fill("@component@ @Title@ @Class@ @package@ @ID@"),
        "acme_Store2 Acme Store2 AcmeStore2 acmestore2 ACME_STORE2"
    );
    assert_eq!(
        Names::of("my-shop").fill("an @ in text, @other@"),
        "an @ in text, @other@"
    );
}

#[test]
fn a_name_starts_with_a_letter() {
    assert!(valid_name("my-shop"));
    assert!(valid_name("my_shop2"));
    assert!(!valid_name(""));
    assert!(!valid_name("9lives"));
    assert!(!valid_name("-shop"));
    assert!(!valid_name("my.shop"));
    assert!(!valid_name("my shop"));
    assert!(!valid_name("../shop"));
}

#[test]
fn parts_by_name() {
    assert_eq!(Part::of(""), Some(Part::All));
    assert_eq!(Part::of("all"), Some(Part::All));
    assert_eq!(Part::of("screens"), Some(Part::Screens));
    assert_eq!(Part::of("rest"), Some(Part::Rest));
    assert_eq!(Part::of("both"), None);
}

#[test]
fn what_a_part_leaves_out_is_in_the_template() {
    for part in [Part::Rest, Part::Screens] {
        for path in part.left_out() {
            assert!(component_template().join(path).exists(), "{path} of {part:?}");
        }
    }
}

#[test]
fn a_component_from_the_template_has_every_placeholder_filled_in() {
    let scratch = Scratch::new("create");
    let target = scratch.0.join("my-shop");
    create(&component_template(), &target, "my-shop", Part::All).unwrap();
    let files = tree(&target);
    assert_eq!(files.len(), tree(&component_template()).len());
    for (path, text) in &files {
        assert!(!path.contains('@'), "{path}");
        for placeholder in ["@component@", "@Title@", "@Class@", "@package@", "@ID@"] {
            assert!(!text.contains(placeholder), "{placeholder} in {path}");
        }
    }
    assert!(files.contains_key("service/myshop/MyShopServices.xml"));
    assert!(files.contains_key("screen/MyShop/FindItem.xml"));
    assert!(files["component.xml"].contains(r#"name="my-shop" version="1.0.0"/>"#));
    assert!(!files["component.xml"].contains("disabled"));
    assert!(files["entity/MyShopEntities.xml"].contains(r#"entity-name="MyShopItem" package="myshop""#));
    // the template stays a template
    assert!(std::fs::read_to_string(component_template().join("component.xml"))
        .unwrap()
        .contains(r#"name="@component@""#));
}

#[test]
fn a_part_leaves_out_the_other() {
    let scratch = Scratch::new("parts");
    let rest = scratch.0.join("my-api");
    create(&component_template(), &rest, "my-api", Part::Rest).unwrap();
    assert!(!rest.join("screen").exists());
    assert!(!rest.join("MoquiConf.xml").exists());
    assert!(rest.join("service/my-api.rest.xml").is_file());
    let screens = scratch.0.join("my-ui");
    create(&component_template(), &screens, "my-ui", Part::Screens).unwrap();
    assert!(!screens.join("service/my-ui.rest.xml").exists());
    assert!(!screens.join("data/MyUiApiSecurityData.xml").exists());
    assert!(screens.join("screen/MyUi.xml").is_file());
}

/// `./moq component-create` and the Dev AIDE make the same component, file for file, for each part.
#[test]
fn moq_makes_the_same_component() {
    if std::process::Command::new("bash").arg("--version").output().is_err() {
        eprintln!("skipped: no bash");
        return;
    }
    let scratch = Scratch::new("moq");
    let runtime = scratch.0.join("runtime");
    std::fs::create_dir_all(runtime.join("dev-component")).unwrap();
    std::os::unix::fs::symlink(component_template(), runtime.join("dev-component/template")).unwrap();
    for (name, part, flag) in [
        ("my-shop", Part::All, None),
        ("my_api", Part::Rest, Some("--rest")),
        ("ui", Part::Screens, Some("--screens")),
    ] {
        let output = std::process::Command::new("bash")
            .arg(repo().join("moq"))
            .args(["component-create", name, "--no-git"])
            .args(flag)
            .env("MOQUI_RUNTIME", &runtime)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let ours = scratch.0.join(format!("ours-{name}"));
        create(&component_template(), &ours, name, part).unwrap();
        assert_eq!(tree(&runtime.join("component").join(name)), tree(&ours), "{name}");
    }
}
