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

use super::*;

/// SEC-TLS-96: a config that may not reach the private network (a remote whose allowPrivateNetwork is N) is
/// refused at 127.0.0.1 before any connection, even with a server listening there; a link-local address always.
#[test]
fn a_guarded_login_reaches_no_internal_address() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let config = |host: &str, deny_internal: bool| SftpConfig {
        host: host.into(),
        port: listener.local_addr().unwrap().port(),
        username: "joe".into(),
        password: Some("pw".into()),
        deny_internal,
        ..SftpConfig::default()
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let refused = runtime
        .block_on(login(&config("127.0.0.1", true), Trust::Any))
        .err()
        .expect("loopback refused");
    assert!(refused.message().contains("internal address"), "{refused}");
    let metadata = runtime
        .block_on(login(&config("169.254.169.254", false), Trust::Any))
        .err()
        .expect("link-local refused");
    assert!(metadata.message().contains("link-local"), "{metadata}");
}
