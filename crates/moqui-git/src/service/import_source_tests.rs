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

//! What `import#GitRepository` takes as a source from its caller (plans/archive/Security-Review.md SEC-GIT-01): another
//! repository only with the view right on it, a server directory or another scheme only with `TERMINAL_HOST` or with
//! authorization disabled, and a URL only over https to a public address, whoever calls.

use super::import_source_allowed;
use crate::server::tests::{fixture, user, READER};

#[test]
fn an_import_source_is_checked_for_the_caller() {
    let fixture = fixture("import-source");
    let _entered = fixture.context.enter();
    {
        let _authz = fixture.context.disable_authz();
        user(&fixture.entity, "outsider");
    }
    moqui_core::user::internal_login_user("outsider").unwrap();
    // another repository, with no view right on it
    let other = import_source_allowed("mine", Some("git://theirs")).unwrap_err();
    assert!(other.contains("not found"), "{other}");
    // a directory of the server, and other schemes
    let directory = import_source_allowed("mine", Some("/etc")).unwrap_err();
    assert!(directory.contains("TERMINAL_HOST"), "{directory}");
    assert!(import_source_allowed("mine", Some("file:///etc")).is_err());
    assert!(import_source_allowed("mine", Some("ssh://git@192.0.2.1/x.git")).is_err());
    // URLs: an internal address, plain http, a credential in it
    let internal = import_source_allowed("mine", Some("https://127.0.0.1/x.git")).unwrap_err();
    assert!(internal.contains("internal"), "{internal}");
    assert!(import_source_allowed("mine", Some("https://[::1]:8443/x.git")).is_err());
    assert!(import_source_allowed("mine", Some("http://192.0.2.1/x.git")).is_err());
    assert!(import_source_allowed("mine", Some("https://user:pw@192.0.2.1/x.git")).is_err());
    // no source is the repository's own remoteUrl
    assert!(import_source_allowed("mine", None).is_ok());

    // a reader of every repository may copy another
    moqui_core::user::internal_login_user(READER).unwrap();
    assert!(import_source_allowed("mine", Some("git://theirs")).is_ok());

    // authorization disabled (an operator's own call): a directory is taken, the address rule still holds
    let _authz = fixture.context.disable_authz();
    assert!(import_source_allowed("mine", Some("/tmp")).is_ok());
    assert!(import_source_allowed("mine", Some("https://127.0.0.1/x.git")).is_err());
}
