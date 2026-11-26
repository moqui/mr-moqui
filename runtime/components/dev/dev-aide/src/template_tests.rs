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
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

fn component_template() -> PathBuf {
    repo().join("runtime/components/dev/template")
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
fn kinds_by_name() {
    assert_eq!(Kind::of("", ""), Some(Kind::App(Part::All)));
    assert_eq!(Kind::of("app", "rest"), Some(Kind::App(Part::Rest)));
    assert_eq!(Kind::of("screens", ""), Some(Kind::App(Part::Screens)));
    assert_eq!(Kind::of("rest", "screens"), None);
    assert_eq!(Kind::of("data", ""), Some(Kind::Data));
    assert_eq!(Kind::of("native", "all"), Some(Kind::Native));
    assert_eq!(Kind::of("native", "rest"), None);
    assert_eq!(Kind::of("theme", ""), Some(Kind::Theme));
    assert_eq!(Kind::of("skills", ""), Some(Kind::Skills));
    assert_eq!(Kind::of("dev-tool", ""), Some(Kind::DevTool));
    assert_eq!(Kind::of("plugin", ""), None);
    for kind in Kind::OTHERS {
        assert_eq!(Kind::of(kind.name(), ""), Some(kind));
    }
    assert!(Kind::DevTool.dev_only());
    assert!(!Kind::Native.dev_only());
}

#[test]
fn what_a_part_leaves_out_is_in_the_template() {
    for part in [Part::Rest, Part::Screens] {
        for path in part.left_out() {
            assert!(component_template().join(path).exists(), "{path} of {part:?}");
        }
    }
    for kind in Kind::OTHERS {
        let source = kind.source(&component_template());
        assert!(source.join("component.xml").is_file(), "{kind:?}");
        assert!(source.join("README.md").is_file(), "{kind:?}");
    }
}

/// The paths and texts of a component of `name` and `kind` made in `scratch`, each placeholder filled in, without the
/// template's `disabled`, and the template left a template.
fn made(scratch: &Scratch, name: &str, kind: Kind) -> BTreeMap<String, String> {
    let target = scratch.0.join(format!("{name}-{}", kind.name()));
    create(&component_template(), &target, name, kind).unwrap();
    let files = tree(&target);
    for (path, text) in &files {
        assert!(!path.contains('@'), "{path} of {kind:?}");
        for placeholder in ["@component@", "@Title@", "@Class@", "@package@", "@ID@"] {
            assert!(!text.contains(placeholder), "{placeholder} in {path} of {kind:?}");
        }
    }
    assert!(!files["component.xml"].contains("disabled"), "{kind:?}");
    assert!(
        files["component.xml"].contains(&format!(r#"name="{name}" version="1.0.0""#)),
        "{kind:?}"
    );
    assert!(
        std::fs::read_to_string(kind.source(&component_template()).join("component.xml"))
            .unwrap()
            .contains(r#"name="@component@""#)
    );
    files
}

#[test]
fn a_component_from_the_template_has_every_placeholder_filled_in() {
    let scratch = Scratch::new("create");
    let files = made(&scratch, "my-shop", Kind::App(Part::All));
    let template_files = tree(&component_template())
        .into_keys()
        .filter(|path| !path.starts_with(&format!("{KINDS_DIR}/")))
        .count();
    assert_eq!(files.len(), template_files);
    assert!(files.contains_key("service/myshop/MyShopServices.xml"));
    assert!(files.contains_key("screen/MyShop/FindItem.xml"));
    assert!(files["component.xml"].contains(r#"name="my-shop" version="1.0.0"/>"#));
    assert!(files["entity/MyShopEntities.xml"].contains(r#"entity-name="MyShopItem" package="myshop""#));
    assert!(!files.keys().any(|path| path.starts_with("kinds")));
}

#[test]
fn each_kind_has_its_own_files() {
    let scratch = Scratch::new("kinds");
    let data = made(&scratch, "my-data", Kind::Data);
    assert!(data.contains_key("data/MyDataSeedData.xml"));
    assert!(!data
        .keys()
        .any(|path| path.starts_with("screen") || path.starts_with("service")));

    let native = made(&scratch, "my-rates", Kind::Native);
    assert!(native["Cargo.toml"].contains(r#"name = "my-rates""#));
    assert!(native["src/lib.rs"].contains(r#"NativeComponent { name: "my-rates", register }"#));
    assert!(native["src/lib.rs"].contains(r#""myrates::echo""#));
    assert!(native["service/myrates/MyRatesServices.xml"].contains(r#"type="rust" location="myrates::echo""#));
    assert!(native.contains_key(".gitignore"));

    let theme = made(&scratch, "my-look", Kind::Theme);
    let theme_data = &theme["data/MyLookThemeData.xml"];
    assert!(theme_data.contains(r#"screenThemeId="MY_LOOK_QUASAR" screenThemeTypeEnumId="STT_INTERNAL_QUASAR" parentScreenThemeId="DEFAULT_QUASAR""#));
    assert!(theme_data.contains(r#"resourceValue="/my-look/MyLook.css""#));
    assert!(theme.contains_key("screen/my-look/MyLook.css"));
    assert!(theme["MoquiConf.xml"].contains(r#"location="component://my-look/screen/my-look.xml""#));

    let skills = made(&scratch, "my-skills", Kind::Skills);
    assert!(skills["skill/myskills.md"].starts_with("---\nname: myskills\n"));
    assert!(skills["test/skill-retrieval.json"].contains(r#""expect": ["myskills"]"#));
    assert!(!skills
        .keys()
        .any(|path| path.ends_with(".rs") || path.starts_with("service")));

    let tool = made(&scratch, "my-mocks", Kind::DevTool);
    assert!(tool["component.xml"].contains(r#"purpose="dev""#));
    assert!(tool.contains_key("service/mymocks/MyMocksMockServices.xml"));
    assert!(tool["data/MyMocksTestData.xml"].contains(r#"type="mymocks-test""#));
}

#[test]
fn a_part_leaves_out_the_other() {
    let scratch = Scratch::new("parts");
    let rest = scratch.0.join("my-api");
    create(&component_template(), &rest, "my-api", Kind::App(Part::Rest)).unwrap();
    assert!(!rest.join("screen").exists());
    assert!(!rest.join("MoquiConf.xml").exists());
    assert!(rest.join("service/my-api.rest.xml").is_file());
    let screens = scratch.0.join("my-ui");
    create(&component_template(), &screens, "my-ui", Kind::App(Part::Screens)).unwrap();
    assert!(!screens.join("service/my-ui.rest.xml").exists());
    assert!(!screens.join("data/MyUiApiSecurityData.xml").exists());
    assert!(screens.join("screen/MyUi.xml").is_file());
}

#[test]
fn a_remote_is_a_network_url_with_no_credential() {
    use crate::vcs::checked_remote;
    for good in [
        "https://github.com/me/my-shop.git",
        " https://git.example.com:8443/a/b.git ",
        "http://127.0.0.1:3000/me/x.git",
        "ssh://git@github.com/me/x.git",
        "git://git.example.com/x.git",
        "git@github.com:me/my-shop.git",
        "host.example.com:repos/x.git",
    ] {
        assert!(checked_remote(good).is_ok(), "{good}");
    }
    for bad in [
        "",
        "https://ann@github.com/me/x.git",
        "https://ann:token@github.com/me/x.git",
        "http://git.example.com/x.git",
        "ssh://ann:secret@host/x.git",
        "file:///srv/x.git",
        "/srv/repos/x.git",
        "../x.git",
        "C:/repos/x.git",
        "ext::sh -c touch% /tmp/pwned",
        "fd::17",
        "--upload-pack=touch /tmp/x",
        "-oProxyCommand=x:y",
        "host:-x",
        "@host:x",
        "git@host:",
        "https://github.com/me/x.git\nfoo",
        "https://github.com\\me",
        "a b:c",
    ] {
        assert!(checked_remote(bad).is_err(), "{bad}");
    }
}

/// `./moq component-create` and the Dev AIDE make the same component, file for file, for each kind and App part, a dev
/// tool in `runtime/components/dev` either way; and moq's `--git-remote` takes the URLs the Dev AIDE takes.
/// Unix only: it runs `moq` through bash and links the template, which this test does not do on Windows.
#[cfg(unix)]
#[test]
fn moq_makes_the_same_component() {
    if std::process::Command::new("bash").arg("--version").output().is_err() {
        eprintln!("skipped: no bash");
        return;
    }
    let scratch = Scratch::new("moq");
    let runtime = scratch.0.join("runtime");
    std::fs::create_dir_all(runtime.join("components/dev")).unwrap();
    std::os::unix::fs::symlink(component_template(), runtime.join("components/dev/template")).unwrap();
    let moq = |args: &[&str]| {
        std::process::Command::new("bash")
            .arg(repo().join("moq"))
            .arg("component-create")
            .args(args)
            .env("MOQUI_RUNTIME", &runtime)
            .output()
            .unwrap()
    };
    let mut cases: Vec<(String, Kind, Vec<&str>)> = vec![
        ("my-shop".into(), Kind::App(Part::All), vec![]),
        ("my_api".into(), Kind::App(Part::Rest), vec!["--rest"]),
        ("ui".into(), Kind::App(Part::Screens), vec!["--screens"]),
    ];
    for kind in Kind::OTHERS {
        cases.push((format!("my-{}", kind.name()), kind, vec!["--kind", kind.name()]));
    }
    for (name, kind, flags) in &cases {
        let mut args = vec![name.as_str(), "--no-git"];
        args.extend(flags);
        let output = moq(&args);
        assert!(
            output.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let ours = scratch.0.join(format!("ours-{name}"));
        create(&component_template(), &ours, name, *kind).unwrap();
        let set = if kind.dev_only() {
            "components/dev"
        } else {
            "components/addon"
        };
        assert_eq!(tree(&runtime.join(set).join(name)), tree(&ours), "{name}");
    }
    // the parts are an App's, and a dev tool is never a test component
    assert!(!moq(&["bad-kind", "--no-git", "--kind", "native", "--rest"])
        .status
        .success());
    assert!(!moq(&["bad-set", "--no-git", "--kind", "dev-tool", "--test"])
        .status
        .success());
    assert!(!moq(&["bad-kind2", "--no-git", "--kind", "plugin"]).status.success());
    // the same remotes refused both ways, before anything is made (with --no-git a remote is refused on its own)
    for bad in [
        "https://ann:token@github.com/me/x.git",
        "ext::sh -c x",
        "file:///srv/x.git",
        "C:/x.git",
    ] {
        assert!(crate::vcs::checked_remote(bad).is_err(), "{bad}");
        let output = moq(&["bad-remote", "--git-remote", bad]);
        assert!(!output.status.success(), "{bad}");
        assert!(!runtime.join("components/addon/bad-remote").exists(), "{bad}");
    }
}
