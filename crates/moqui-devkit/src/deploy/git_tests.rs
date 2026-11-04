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

use std::path::Path;

use super::{check_name, check_url, clone_directory, component_name, final_location, Storage};

#[test]
fn a_deploy_fetches_over_https_or_ssh_only() {
    assert!(check_url("https://github.com/moqui/mr-pass.git").is_ok());
    assert!(check_url("ssh://git@github.com/moqui/mr-pass.git").is_ok());
    assert!(check_url("git@github.com:moqui/mr-pass.git").is_ok());
    for refused in [
        "file:///home/me/repo",
        "/home/me/repo",
        "http://example.com/repo.git",
        "git://example.com/repo.git",
        "https:///repo.git",
    ] {
        assert!(check_url(refused).is_err(), "{refused}");
    }
}

#[test]
fn a_component_name_is_one_directory_name() {
    for good in ["mr-pass", "my_site", "site.v2"] {
        assert!(check_name(good).is_ok(), "{good}");
    }
    for bad in ["", ".hidden", "a/b", "../up", "a b"] {
        assert!(check_name(bad).is_err(), "{bad:?}");
    }
    assert_eq!(
        component_name(br#"<component name="mr-pass" version="1"/>"#).unwrap(),
        "mr-pass"
    );
    assert!(component_name(b"<component/>").is_err());
}

#[test]
fn a_clone_goes_in_a_component_directory_under_the_runtime() {
    let runtime = Path::new("/srv/moqui/runtime");
    let dirs = [
        "base-component".to_string(),
        "component".to_string(),
        "dev-component".to_string(),
    ];
    assert_eq!(clone_directory(runtime, "", &dirs).unwrap(), runtime.join("component"));
    assert_eq!(
        clone_directory(runtime, "dev-component/", &dirs).unwrap(),
        runtime.join("dev-component")
    );
    assert!(clone_directory(runtime, "../elsewhere", &dirs).is_err());
    assert!(clone_directory(runtime, "/etc", &dirs).is_err());
    assert!(clone_directory(runtime, "tmp", &dirs).is_err());
    assert_eq!(
        final_location(&runtime.join("component/.staging-site"), "site"),
        runtime.join("component/site")
    );
    assert_eq!(Storage::parse(""), Some(Storage::Database));
    assert_eq!(Storage::parse("clone"), Some(Storage::Clone));
    assert_eq!(Storage::parse("zip"), None);
}
