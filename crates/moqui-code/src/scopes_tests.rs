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

use super::{method_spec, Grant, Scope, METHODS};
use crate::{ADMIN_PERMISSION, USE_PERMISSION};

#[test]
fn every_method_names_a_scope_and_a_permission_once() {
    let mut names: Vec<&str> = METHODS.iter().map(|spec| spec.name).collect();
    names.sort_unstable();
    let count = names.len();
    names.dedup();
    assert_eq!(names.len(), count, "a method is in the table twice");
    for spec in METHODS {
        assert!(Scope::parse(spec.scope.name()).is_some(), "{}", spec.name);
        assert!(spec.permission.starts_with("AgentCode"), "{}", spec.name);
    }
    assert!(method_spec("orchestration.subscribeShell").is_some_and(|spec| spec.stream));
    assert!(method_spec("nope").is_none());
}

#[test]
fn scope_names_round_trip_and_unknown_or_ungrantable_names_drop() {
    for scope in Scope::ALL {
        assert_eq!(Scope::parse(scope.name()), Some(scope));
    }
    assert_eq!(
        Scope::parse_list("code:read, code:operate code:review code:bogus code:read"),
        vec![Scope::Read, Scope::Operate]
    );
}

#[test]
fn a_key_holds_only_its_scopes_and_a_session_all_its_permissions_allow() {
    let user = |permission: &str| permission == USE_PERMISSION;
    let read = method_spec("orchestration.getShellSnapshot").unwrap();
    let operate = method_spec("orchestration.dispatchCommand").unwrap();
    let server = method_spec("settings.updateServer").unwrap();
    let session = Grant { key_scopes: None, is_browser_session: true };
    assert!(session.check(read, user).is_ok());
    assert!(session.check(operate, user).is_ok());
    let refused = session.check(server, user).unwrap_err();
    assert_eq!(refused.required_permission, ADMIN_PERMISSION);
    assert!(session.check(server, |_| true).is_ok());
    let key = Grant { key_scopes: Some(vec![Scope::Read]), is_browser_session: false };
    assert!(key.check(read, user).is_ok());
    let refused = key.check(operate, user).unwrap_err();
    assert_eq!(refused.required_scope, Scope::Operate);
    assert!(session.check(read, |_| false).is_err());
}
