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

/// SEC-PSA-03: a web script that checks a credential itself (PASS's client secret over REST) notes a wrong one for the
/// web protection, which counts it toward the client address's `auth-failure` ban as a wrong password; outside a
/// request the note is nothing.
#[test]
fn a_script_notes_an_auth_failure_for_the_web_protection() {
    use crate::script::runtime::{clear_context_stack, eval_value};
    crate::web::take_auth_failure();
    clear_context_stack();
    eval_value("ec.web.noteAuthFailure()").unwrap();
    assert!(!crate::web::take_auth_failure(), "no request, nothing noted");
    let _web = crate::web::install(std::collections::BTreeMap::new());
    crate::web::set_request(
        std::sync::Arc::new(crate::web::WebRequest::new("GET", "/rest/pass/site")),
        bytes::Bytes::new(),
    );
    eval_value("ec.web.noteAuthFailure()").unwrap();
    assert!(crate::web::take_auth_failure());
    assert!(!crate::web::take_auth_failure(), "taken once");
}

/// SEC-PSS-25: a script signs a value it hands a client (an article's rating nonce) with the server's own key: the grant
/// checks for its purpose and subject until it expires, and not for another purpose or subject, with its expiry
/// changed, or once the server's key changes; with no key there is no grant.
#[test]
fn a_server_grant_checks_for_its_purpose_and_subject_until_it_expires() {
    use crate::script::runtime::{clear_context_stack, eval_value};
    use base64::Engine as _;
    let _lock = crate::user::session::cookie_property_lock();
    let previous = moqui_lib::env::get_prop_or_env("session_cookie_key");
    let key = |byte: u8| base64::engine::general_purpose::STANDARD.encode([byte; 32]);
    moqui_lib::env::set_property("session_cookie_key", key(5));
    clear_context_stack();
    let checks = eval_value(
        "const t = ec.user.serverGrant('article-rating', 'site1:page1', 60); \
         const [expires, tag] = t.split('.'); \
         [ec.user.checkServerGrant('article-rating', 'site1:page1', t), \
          !ec.user.checkServerGrant('article-rating', 'site1:page2', t), \
          !ec.user.checkServerGrant('other-purpose', 'site1:page1', t), \
          !ec.user.checkServerGrant('article-rating', 'site1:page1', (Number(expires) + 1000) + '.' + tag), \
          !ec.user.checkServerGrant('article-rating', 'site1:page1', '1.' + tag), \
          !ec.user.checkServerGrant('article-rating', 'site1:page1', null), \
          !ec.user.checkServerGrant('article-rating', 'site1:page1', 'x'.repeat(40)), \
          (() => { try { ec.user.serverGrant('', 'x'); return false } catch (e) { return true } })(), \
          Number(expires) - Date.now() > 50000 && Number(expires) - Date.now() <= 60000].map(String).join()",
    );
    let token = eval_value("ec.user.serverGrant('article-rating', 'site1:page1')")
        .map(|v| moqui_lib::object::to_plain_string(&v));
    moqui_lib::env::set_property("session_cookie_key", key(6));
    let after_new_key = token.as_ref().map(|token| {
        eval_value(&format!(
            "ec.user.checkServerGrant('article-rating', 'site1:page1', '{token}')"
        ))
        .map(|v| moqui_lib::object::to_plain_string(&v))
    });
    moqui_lib::env::clear_property("session_cookie_key");
    let no_key = eval_value("(() => { try { ec.user.serverGrant('article-rating', 'x'); return 'made' } catch (e) { return 'refused' } })()")
        .map(|v| moqui_lib::object::to_plain_string(&v));
    if !previous.is_empty() {
        moqui_lib::env::set_property("session_cookie_key", &previous);
    }
    assert_eq!(
        moqui_lib::object::to_plain_string(&checks.unwrap()),
        "true,true,true,true,true,true,true,true,true"
    );
    assert_eq!(
        after_new_key.unwrap().unwrap(),
        "false",
        "a new server key ends every grant"
    );
    assert_eq!(no_key.unwrap(), "refused");
}
