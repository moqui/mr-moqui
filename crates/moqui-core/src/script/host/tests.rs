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

use super::{all_paths, classes, path_allowed};

/// A path the table installs, and its members, are allowed; another member, another area and `ec` itself are not.
#[test]
fn the_allowed_paths_are_the_table_and_its_members() {
    assert!(path_allowed("ec.message"));
    assert!(path_allowed("ec.message.addError"));
    assert!(path_allowed("ec.message.errorsString"), "a getter");
    assert!(path_allowed("ec.cache.getCache"));
    assert!(!path_allowed("ec.message.getAvailableLocales"));
    assert!(!path_allowed("ec.messages"));
    assert!(!path_allowed("ec.entity.find"));
    assert!(!path_allowed("ec"));
    assert!(path_allowed("ec.user.getPreferences"), "a shim's member");
    assert!(path_allowed("ec.user.context"));
    assert!(path_allowed("ec.user.context.get"), "below a shim's member");
    assert!(!path_allowed("ec.user.contextual"));
    assert!(!path_allowed("ec.user.userId.x"), "below a method");
}

/// The binders take a method's `length` from its required kinds, which must come before the optional ones, and a
/// prototype's cache name must be unique.
#[test]
fn every_method_lists_required_arguments_first() {
    let mut names = std::collections::HashSet::new();
    for class in classes() {
        assert!(names.insert(class.name), "{} is cached twice", class.name);
        for method in class.methods.iter().chain(class.getters).chain(class.hidden) {
            let first_optional = method.args.iter().position(|kind| !kind.required());
            if let Some(first) = first_optional {
                assert!(
                    method.args[first..].iter().all(|kind| !kind.required()),
                    "{}: a required argument after an optional one",
                    method.path
                );
            }
        }
    }
    assert!(!all_paths().is_empty());
}
