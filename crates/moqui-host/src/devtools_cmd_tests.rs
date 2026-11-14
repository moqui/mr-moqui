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

//! `moqui check --agreement`'s server and token rule (F19).

use super::{agreement_server, AgreementServer};

#[test]
fn the_token_goes_only_to_a_configured_server_the_remote_is_on() {
    let team = Some("https://team.example.com");
    let on =
        |configured: Option<&str>, remote: &str| agreement_server(configured, remote).map(|target| target.send_token);
    assert_eq!(on(team, "https://team.example.com/git/acme.git"), Ok(true));
    assert_eq!(on(team, "ssh://ann@team.example.com:2222/acme.git"), Ok(true));
    assert_eq!(on(team, "ann@team.example.com:acme.git"), Ok(true));
    // a remote elsewhere: the configured server is asked with no token
    assert_eq!(on(team, "https://evil.example.com/git/acme.git"), Ok(false));
    assert_eq!(on(team, "https://team.example.com:8443/git/acme.git"), Ok(false));
    // a server read from the remote alone: no token
    assert_eq!(
        agreement_server(None, "https://u:pw@team.example.com/git/acme.git"),
        Ok(AgreementServer {
            base: "https://team.example.com".into(),
            send_token: false
        })
    );
    assert!(agreement_server(None, "git@team.example.com:acme.git").is_err());
    // plain http only to a loopback IP literal, by the parsed host
    assert_eq!(
        on(Some("http://127.0.0.1:8080"), "http://127.0.0.1:8080/git/a.git"),
        Ok(true)
    );
    assert_eq!(on(Some("http://[::1]:8080"), "http://[::1]:8080/git/a.git"), Ok(true));
    for bad in [
        "http://127.0.0.1.evil.example",
        "http://127.0.0.1@evil.example",
        "http://localhost:8080",
        "http://team.example.com",
    ] {
        assert!(
            agreement_server(Some(bad), "https://team.example.com/git/a.git").is_err(),
            "{bad}"
        );
    }
    assert!(agreement_server(None, "http://127.0.0.1.evil.example/git/a.git").is_err());
}
