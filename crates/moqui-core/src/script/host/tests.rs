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
    assert!(path_allowed("ec.entity.find"), "a function path");
    assert!(path_allowed("ec.entity.conditionFactory.makeConditionWhere"), "a method of an object path");
    assert!(!path_allowed("ec.entity.getEntityDefinition"), "what the tools shim adds on QuickJS only");
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
            // setFields(map, setIfEmpty, prefix, include) has an optional flag between required arguments, as its
            // typed QuickJS method had: three are required, and the flag is converted when passed
            if let (Some(first), false) = (first_optional, method.path == "EntityValue.setFields") {
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

/// SEC-PSF-08: a download forwarded from the PASS authority over gRPC is answered by its type only when inert, else as
/// bytes, with `default-src 'none'; sandbox` (an inline PDF aside), as the authority's local answer is.
#[test]
fn a_forwarded_pass_download_has_an_inert_type_and_the_sandbox() {
    use crate::pass_authority::DownloadMeta;
    let headers = |content_type: &str, disposition: &str| {
        let meta = DownloadMeta {
            content_type: content_type.to_string(),
            filename: "x.js".to_string(),
            disposition: disposition.to_string(),
            ..DownloadMeta::default()
        };
        let headers = super::pass::download_headers(&meta, "getReleaseAsset");
        let header = |wanted: &str| {
            headers
                .iter()
                .find(|(name, _)| name == wanted)
                .map(|(_, value)| value.clone())
                .unwrap_or_default()
        };
        (
            header("Content-Type"),
            header("Content-Security-Policy"),
            header("Content-Disposition"),
        )
    };
    let sandbox = crate::web::USER_FILE_SANDBOX.to_string();
    let (kind, policy, disposition) = headers("application/javascript", "attachment");
    assert_eq!((kind.as_str(), policy), ("application/octet-stream", sandbox.clone()));
    assert!(disposition.starts_with("attachment"));
    assert_eq!(
        headers("image/png", "inline"),
        ("image/png".into(), sandbox.clone(), "inline; filename=\"x.js\"".into())
    );
    assert_eq!(headers("application/pdf", "inline").1, "");
    assert_eq!(headers("text/html", "inline").0, "application/octet-stream");
    assert_eq!(headers("text/html", "inline").1, sandbox);
}
