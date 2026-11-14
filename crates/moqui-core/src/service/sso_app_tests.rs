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

#[test]
fn the_challenge_is_rfc_7636s_s256() {
    // RFC 7636 appendix B
    assert_eq!(
        challenge_of("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
    assert!(verifier_ok("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"));
    assert!(!verifier_ok("short"));
    assert!(!verifier_ok(&"a".repeat(129)));
    assert!(!verifier_ok(&format!("{}+", "a".repeat(50))));
}

#[test]
fn only_an_s256_challenge_a_plain_state_and_a_registered_redirect_start() {
    let allowed = Redirects {
        listed: vec!["org.moqui.desktop:/sso".to_string()],
        loopback: false,
    };
    let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
    assert!(check_start(challenge, "S256", "s1", "org.moqui.desktop:/sso", &allowed).is_ok());
    // an absent method is plain in RFC 7636: not taken
    assert!(check_start(challenge, "", "s1", "org.moqui.desktop:/sso", &allowed).is_err());
    assert!(check_start(challenge, "plain", "s1", "org.moqui.desktop:/sso", &allowed).is_err());
    assert!(check_start("short", "S256", "s1", "org.moqui.desktop:/sso", &allowed).is_err());
    assert!(check_start(challenge, "S256", "", "org.moqui.desktop:/sso", &allowed).is_err());
    assert!(check_start(challenge, "S256", "a b", "org.moqui.desktop:/sso", &allowed).is_err());
    // the redirect exactly as registered: no other scheme, path or host
    for redirect in [
        "https://evil.example.com/sso",
        "org.moqui.desktop:/sso/x",
        "org.moqui.desktop:/SSO",
        "http://127.0.0.1:8080/x",
        "",
    ] {
        assert!(
            check_start(challenge, "S256", "s1", redirect, &allowed).is_err(),
            "{redirect}"
        );
    }
}

/// A dev instance's own address, on any port, signs in to its group server (RFC 8252 §7.3); nothing that only looks
/// like one, and none when loopback redirects are off.
#[test]
fn a_loopback_redirect_on_any_port_starts() {
    let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
    let loopback = Redirects {
        listed: Vec::new(),
        loopback: true,
    };
    for redirect in [
        "http://127.0.0.1:8080/qapps/dev/Settings/serverSignedIn",
        "http://localhost:51234/cb",
        "http://[::1]:9000/",
        "http://localhost/x?y=1",
        "https://127.0.0.1:8443/cb",
    ] {
        assert!(
            check_start(challenge, "S256", "s1", redirect, &loopback).is_ok(),
            "{redirect}"
        );
    }
    for redirect in [
        "http://127.0.0.1.evil.example.com/cb",
        "http://localhost.evil.example.com:80/cb",
        "http://evil@127.0.0.1:8080/cb",
        "http://127.0.0.1:8080/cb#frag",
        "http://127.0.0.1:80a/cb",
        "http://10.0.0.1:8080/cb",
        "ftp://127.0.0.1/cb",
        "org.moqui.desktop:/sso",
    ] {
        assert!(
            check_start(challenge, "S256", "s1", redirect, &loopback).is_err(),
            "{redirect}"
        );
    }
    let off = Redirects {
        listed: Vec::new(),
        loopback: false,
    };
    assert!(check_start(challenge, "S256", "s1", "http://127.0.0.1:8080/cb", &off).is_err());
}

#[test]
fn the_confirmation_page_escapes_what_it_shows() {
    assert_eq!(
        escape("<b>\"Pat's\" & co</b>"),
        "&lt;b&gt;&quot;Pat&#39;s&quot; &amp; co&lt;/b&gt;"
    );
}

#[test]
fn a_redirect_carries_its_values_encoded() {
    assert_eq!(percent("a b&c=d"), "a%20b%26c%3Dd");
    assert_eq!(percent("Az09-._~"), "Az09-._~");
}
