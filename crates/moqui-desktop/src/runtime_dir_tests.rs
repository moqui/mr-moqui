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

use super::{Location, READ_ONLY};
use std::path::{Path, PathBuf};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("moqui-desktop-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn moqui_runtime_wins_then_the_repository_runtime_in_a_dev_run_then_one_found() {
    let found = || Some(PathBuf::from("/work/runtime"));
    let env = Some(PathBuf::from("/srv/runtime"));
    assert_eq!(
        Location::choose(env, true, found(), Path::new("/r"), Path::new("/d")),
        Location::Existing(PathBuf::from("/srv/runtime"))
    );
    let Location::Existing(dev) =
        Location::choose(Some(PathBuf::new()), true, found(), Path::new("/r"), Path::new("/d"))
    else {
        panic!("a dev run uses the repository's runtime");
    };
    assert!(dev.ends_with("../../runtime"));
    assert_eq!(
        Location::choose(None, false, found(), Path::new("/r"), Path::new("/d")),
        Location::Existing(PathBuf::from("/work/runtime"))
    );
    assert_eq!(
        Location::choose(None, false, None, Path::new("/r"), Path::new("/d")),
        Location::Bundled {
            resources: PathBuf::from("/r/runtime"),
            runtime: PathBuf::from("/d/runtime")
        }
    );
}

#[test]
fn a_bundled_runtime_links_the_read_only_parts_and_keeps_the_rest() {
    let root = temp("bundled");
    let resources = root.join("resources");
    // no conf: a part the bundle lacks
    for name in ["components/base", "components/dev"] {
        std::fs::create_dir_all(resources.join(name)).unwrap();
    }
    std::fs::write(resources.join("MoquiConf.xml"), "<moqui-conf/>").unwrap();
    let runtime = root.join("data/runtime");
    std::fs::create_dir_all(runtime.join("db")).unwrap();
    std::fs::write(runtime.join(".env"), "secret=1\n").unwrap();

    let location = Location::Bundled {
        resources: resources.clone(),
        runtime: runtime.clone(),
    };
    assert_eq!(location.prepare().unwrap(), runtime);
    for name in READ_ONLY {
        let link = runtime.join(name);
        if name == "conf" {
            assert!(
                link.symlink_metadata().is_err(),
                "a part the bundle lacks is not linked"
            );
        } else {
            assert_eq!(std::fs::read_link(&link).unwrap(), resources.join(name));
        }
    }
    assert_eq!(std::fs::read_to_string(runtime.join(".env")).unwrap(), "secret=1\n");
    assert!(runtime.join("db").is_dir());

    // An upgrade that moved the app relinks; a second start with the same app changes nothing.
    let moved = root.join("moved");
    std::fs::rename(&resources, &moved).unwrap();
    let location = Location::Bundled {
        resources: moved.clone(),
        runtime: runtime.clone(),
    };
    location.prepare().unwrap();
    assert_eq!(
        std::fs::read_link(runtime.join("components")).unwrap(),
        moved.join("components")
    );
    let location = Location::Bundled {
        resources: moved.clone(),
        runtime: runtime.clone(),
    };
    location.prepare().unwrap();
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_directory_in_the_way_of_a_link_is_an_error_naming_it() {
    let root = temp("in-the-way");
    let resources = root.join("resources");
    std::fs::create_dir_all(resources.join("conf")).unwrap();
    let runtime = root.join("runtime");
    std::fs::create_dir_all(runtime.join("conf")).unwrap();
    let err = Location::Bundled {
        resources,
        runtime: runtime.clone(),
    }
    .prepare()
    .unwrap_err();
    assert!(err.contains(&runtime.join("conf").display().to_string()), "{err}");
    std::fs::remove_dir_all(&root).unwrap();
}
