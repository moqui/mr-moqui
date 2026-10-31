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

fn rfc7616_credentials() -> Credentials {
    Credentials {
        username: "Mufasa".to_string(),
        password: "Circle of Life".to_string(),
        realm: Some("http-auth@example.org".to_string()),
        algorithms: vec![Algorithm::Sha256, Algorithm::Md5],
        ..Default::default()
    }
}

const RFC7616_NONCE: &str = "7ypf/xlj9XXwfDPEoM4URrv/xwf94BcCAzFZH4GiTo0v";
const RFC7616_CNONCE: &str = "f2/wE4q74E6zIJEtWaHKaf5wv/H5QzzpXusqGemxURZJ";

fn response_of(value: &str) -> String {
    Authorization::parse(value).unwrap().response
}

#[test]
fn the_rfc_7616_example_in_sha_256_and_md5() {
    let credentials = rfc7616_credentials();
    let sha = Challenge::parse(&format!(
        "Digest realm=\"http-auth@example.org\", qop=\"auth, auth-int\", algorithm=SHA-256, nonce=\"{RFC7616_NONCE}\", opaque=\"FQhe/qaU925kfnzjCev0ciny7QMkPqMAFRtzCUYo5tdS\""
    ))
    .unwrap();
    let md5 = Challenge::parse(&format!(
        "Digest realm=\"http-auth@example.org\", qop=\"auth, auth-int\", algorithm=MD5, nonce=\"{RFC7616_NONCE}\", opaque=\"FQhe/qaU925kfnzjCev0ciny7QMkPqMAFRtzCUYo5tdS\""
    ))
    .unwrap();
    let both = [md5.clone(), sha.clone()];
    assert_eq!(
        choose(&both, &credentials).unwrap().algorithm,
        Some(Algorithm::Sha256),
        "the stronger one"
    );
    let value = answer(&credentials, &sha, "GET", "/dir/index.html", RFC7616_CNONCE).unwrap();
    assert_eq!(
        response_of(&value),
        "753927fa0e85d155564e2e272a28d1802ca10daf4496794697cf8db5856cb6c1"
    );
    assert!(value.contains("opaque=\"FQhe"));
    let value = answer(&credentials, &md5, "GET", "/dir/index.html", RFC7616_CNONCE).unwrap();
    assert_eq!(response_of(&value), "8ca523f5e9506fed4657c9700eebdbec");
}

#[test]
fn a_challenge_for_another_realm_or_algorithm_is_not_answered() {
    let credentials = Credentials {
        algorithms: vec![Algorithm::Sha256],
        ..rfc7616_credentials()
    };
    let other_realm = Challenge::parse("Digest realm=\"evil.example\", nonce=\"abc\", algorithm=SHA-256").unwrap();
    assert_eq!(
        answer(&credentials, &other_realm, "INVITE", "sip:x@y", "c"),
        Err(Refusal::WrongRealm("evil.example".to_string()))
    );
    let md5_only = Challenge::parse("Digest realm=\"http-auth@example.org\", nonce=\"abc\"").unwrap();
    assert_eq!(
        answer(&credentials, &md5_only, "INVITE", "sip:x@y", "c"),
        Err(Refusal::Algorithm("MD5".to_string()))
    );
    let unknown =
        Challenge::parse("Digest realm=\"http-auth@example.org\", nonce=\"abc\", algorithm=AKAv1-MD5").unwrap();
    assert!(matches!(
        answer(&credentials, &unknown, "INVITE", "sip:x@y", "c"),
        Err(Refusal::Algorithm(_))
    ));
    assert!(Challenge::parse("Basic realm=\"x\"").is_none());
    // a trunk with no realm set answers its own server's realm
    let open = Credentials {
        realm: None,
        ..rfc7616_credentials()
    };
    assert!(answer(&open, &other_realm, "INVITE", "sip:x@y", "c").is_ok());
}

#[test]
fn the_server_side_checks_nonces_counts_and_staleness() {
    let mut book = NonceBook::new("moqui.example", 300);
    let challenge = Challenge::parse(&book.challenges(1_000, &[Algorithm::Sha256], false)[0]).unwrap();
    assert_eq!(challenge.realm, "moqui.example");
    let credentials = Credentials {
        username: "trunk1".to_string(),
        password: "s3cret".to_string(),
        ..Default::default()
    };
    let value = answer(
        &credentials,
        &challenge,
        "INVITE",
        "sip:+15551230000@moqui.example",
        "cn1",
    )
    .unwrap();
    let auth = Authorization::parse(&value).unwrap();
    assert_eq!(book.verify(&auth, "INVITE", Some("s3cret"), 1_010), Verdict::Valid);
    // the same nonce count again is a replay, even with the right password
    assert_eq!(book.verify(&auth, "INVITE", Some("s3cret"), 1_011), Verdict::Replay);
    // a higher count on the same nonce is fine
    let next = value.replace("nc=00000001", "nc=00000002");
    let mut auth2 = Authorization::parse(&next).unwrap();
    let ha1 = Algorithm::Sha256.hash("trunk1:moqui.example:s3cret");
    let ha2 = Algorithm::Sha256.hash("INVITE:sip:+15551230000@moqui.example");
    auth2.response = Algorithm::Sha256.hash(&format!("{ha1}:{}:00000002:cn1:auth:{ha2}", auth2.nonce));
    assert_eq!(book.verify(&auth2, "INVITE", Some("s3cret"), 1_012), Verdict::Valid);
    // a wrong password, an unknown user, a nonce from elsewhere and another realm are failures
    let fresh = Challenge::parse(&book.challenges(1_020, &[Algorithm::Sha256], false)[0]).unwrap();
    let wrong = answer(&credentials, &fresh, "INVITE", "sip:a@b", "cn2").unwrap();
    assert_eq!(
        book.verify(&Authorization::parse(&wrong).unwrap(), "INVITE", Some("other"), 1_021),
        Verdict::Invalid
    );
    assert_eq!(
        book.verify(&Authorization::parse(&wrong).unwrap(), "INVITE", None, 1_021),
        Verdict::Invalid
    );
    let forged = wrong.replace(&fresh.nonce, "AAAAAAAAA-forged-nonce");
    assert_eq!(
        book.verify(&Authorization::parse(&forged).unwrap(), "INVITE", Some("s3cret"), 1_021),
        Verdict::Invalid
    );
    let other_book = NonceBook::new("moqui.example", 300);
    let theirs = Challenge::parse(&other_book.challenges(1_020, &[Algorithm::Sha256], false)[0]).unwrap();
    let theirs = answer(&credentials, &theirs, "INVITE", "sip:a@b", "cn3").unwrap();
    assert_eq!(
        book.verify(&Authorization::parse(&theirs).unwrap(), "INVITE", Some("s3cret"), 1_021),
        Verdict::Invalid
    );
    // the right answer on an expired nonce is stale, not a failure
    let old = Challenge::parse(&book.challenges(1_000, &[Algorithm::Md5], false)[0]).unwrap();
    let late = answer(&credentials, &old, "INVITE", "sip:a@b", "cn4").unwrap();
    assert_eq!(
        book.verify(&Authorization::parse(&late).unwrap(), "INVITE", Some("s3cret"), 1_400),
        Verdict::Stale
    );
}

#[test]
fn two_calls_challenged_in_the_same_second_are_not_replays() {
    // a PBX placing two calls at once: each INVITE is challenged, each retry starts its count at 1
    let mut book = NonceBook::new("moqui.example", 300);
    let credentials = Credentials {
        username: "pbx".to_string(),
        password: "s3cret".to_string(),
        ..Default::default()
    };
    let first = Challenge::parse(&book.challenges(2_000, &[Algorithm::Sha256], false)[0]).unwrap();
    let second = Challenge::parse(&book.challenges(2_000, &[Algorithm::Sha256], false)[0]).unwrap();
    assert_ne!(first.nonce, second.nonce);
    for (challenge, cnonce) in [(&first, "a"), (&second, "b")] {
        let value = answer(
            &credentials,
            challenge,
            "INVITE",
            "sip:+15551230000@moqui.example",
            cnonce,
        )
        .unwrap();
        assert_eq!(
            book.verify(&Authorization::parse(&value).unwrap(), "INVITE", Some("s3cret"), 2_001),
            Verdict::Valid
        );
    }
}
