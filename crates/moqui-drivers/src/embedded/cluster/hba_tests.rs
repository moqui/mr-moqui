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

//! `initdb --auth password` writes method `password` on TCP lines. Those become `scram-sha-256`;
//! a `local` line and the header, which says "password", stay.

use super::scram_host_auth;

const SAMPLE: &str = "\
# Note that \"password\" sends passwords in clear text; \"md5\" or
# \"scram-sha-256\" are preferred since they send encrypted passwords.
local   all             all                                     password\r
# IPv4 local connections:
host    all             all             127.0.0.1/32            password\r
# IPv6 local connections:
host    all             all             ::1/128                 password\r
host    replication     all             127.0.0.1/32            password\r
hostssl all             all             127.0.0.1/32            scram-sha-256 clientcert=verify-full\r
";

#[test]
fn host_password_lines_become_scram_and_a_second_pass_changes_nothing() {
    let (once, changed) = scram_host_auth(SAMPLE);
    assert!(changed);
    assert!(
        once.contains("local   all             all                                     password\r"),
        "{once}"
    );
    assert!(
        once.contains("# Note that \"password\" sends passwords in clear text"),
        "{once}"
    );
    for line in once.lines() {
        let rule = line.trim().trim_end_matches('\r');
        if rule.starts_with('#') || rule.is_empty() || !rule.starts_with("host") {
            continue;
        }
        let method = rule.split_whitespace().nth(4).unwrap_or("");
        assert_ne!(method, "password", "{rule}");
    }
    assert!(once.contains("127.0.0.1/32            scram-sha-256\r"), "{once}");
    assert!(once.contains("::1/128                 scram-sha-256\r"), "{once}");
    assert!(
        once.contains(
            "hostssl all             all             127.0.0.1/32            scram-sha-256 clientcert=verify-full\r"
        ),
        "{once}"
    );
    let (twice, again) = scram_host_auth(&once);
    assert!(!again);
    assert_eq!(twice, once);
}
