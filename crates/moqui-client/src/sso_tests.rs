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
fn a_start_has_a_fresh_verifier_its_challenge_and_the_redirect() {
    let (pending, url) = start(
        "https://erp.example.com",
        "GOOGLE",
        DESKTOP_REDIRECT,
        "MR Moqui desktop",
    )
    .unwrap();
    let (again, _) = start(
        "https://erp.example.com",
        "GOOGLE",
        DESKTOP_REDIRECT,
        "MR Moqui desktop",
    )
    .unwrap();
    assert_ne!(pending.verifier, again.verifier);
    assert_ne!(pending.state, again.state);
    assert_eq!(pending.verifier.len(), 64);
    assert!(
        url.starts_with("https://erp.example.com/sso/appLogin?authFlowId=GOOGLE&"),
        "{url}"
    );
    assert!(
        url.contains(&format!("codeChallenge={}", challenge_of(&pending.verifier))),
        "{url}"
    );
    assert!(url.contains("codeChallengeMethod=S256"), "{url}");
    assert!(url.contains("redirectUri=org.moqui.desktop%3A%2Fsso"), "{url}");
    assert!(url.contains("deviceName=MR+Moqui+desktop"), "{url}");
    assert!(!url.contains(&pending.verifier), "the verifier stays in the app");
    assert!(!pending.stale());
}

#[test]
fn the_challenge_is_rfc_7636s() {
    assert_eq!(
        challenge_of("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
}

#[test]
fn only_the_pending_sign_ins_hand_back_gives_its_code() {
    let (pending, _) = start(
        "https://erp.example.com",
        "GOOGLE",
        DESKTOP_REDIRECT,
        "MR Moqui desktop",
    )
    .unwrap();
    let link = |query: &str| format!("org.moqui.desktop:/sso?{query}");
    let state = &pending.state;
    assert_eq!(
        callback(&link(&format!("code=c1&state={state}")), &pending).unwrap(),
        "c1"
    );
    assert!(callback(&link("code=c1&state=other"), &pending).is_err());
    assert!(callback(&link("code=c1"), &pending).is_err());
    let denied = callback(&link(&format!("error=access_denied&state={state}")), &pending).unwrap_err();
    assert!(denied.to_string().contains("access_denied"), "{denied}");
    assert!(callback(&link(&format!("state={state}")), &pending).is_err());
    assert_eq!(link_state(&link("code=c1&state=s%20x")).as_deref(), Some("s x"));
    assert_eq!(link_state(&link("code=c1")), None);
}
