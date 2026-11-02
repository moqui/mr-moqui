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

fn verifiers(out: &BTreeMap<String, Value>) -> Vec<(String, String)> {
    let Some(Value::List(list)) = out.get("verifierList") else {
        panic!("verifierList")
    };
    list.iter()
        .map(|v| {
            let Value::Map(map) = v else { panic!("a verifier") };
            (to_plain_string(&map["algorithm"]), to_plain_string(&map["ha1"]))
        })
        .collect()
}

#[test]
fn a_credential_keeps_only_verifiers_and_md5_only_for_a_legacy_device() {
    let mut parameters = BTreeMap::from([("realm".to_string(), Value::string("sip.example.com"))]);
    let out = credential(&parameters).unwrap();
    let username = to_plain_string(&out["username"]);
    let password = to_plain_string(&out["password"]);
    assert_eq!(username.len(), 10);
    assert!(username.chars().all(|c| c.is_ascii_digit()) && !username.starts_with('0'));
    assert_eq!(password.len(), 22);
    assert!(password.bytes().all(|b| PASSWORD_CHARS.contains(&b)));
    let secret = format!("{username}:sip.example.com:{password}");
    assert_eq!(
        verifiers(&out),
        vec![("SHA-256".to_string(), hex(&Sha256::digest(secret.as_bytes())))],
        "a standard device has a SHA-256 verifier only"
    );
    let again = credential(&parameters).unwrap();
    assert_ne!(to_plain_string(&again["password"]), password, "each credential is new");
    parameters.insert("username".to_string(), Value::string("4155550100"));
    parameters.insert("legacy".to_string(), Value::Bool(true));
    let legacy = credential(&parameters).unwrap();
    assert_eq!(to_plain_string(&legacy["username"]), "4155550100");
    let algorithms: Vec<String> = verifiers(&legacy).into_iter().map(|(a, _)| a).collect();
    assert_eq!(algorithms, vec!["SHA-256", "MD5"]);
    assert!(credential(&BTreeMap::new()).is_err(), "a realm is needed");
}

/// RFC 2617 §3.5's example answer, computed as the local node checks its edge node's registrar's answers (E8).
#[test]
fn the_digest_response_is_rfc_2617_s_example() {
    let ha1 = hex(&Md5::digest(b"Mufasa:testrealm@host.com:Circle Of Life"));
    let ha2 = hex(&Md5::digest(b"GET:/dir/index.html"));
    let nonce = "dcd98b7102dd2f0e8b11d0f600bfb0c093";
    assert_eq!(
        digest_response("MD5", &ha1, nonce, Some("auth"), "00000001", "0a4f113b", &ha2).as_deref(),
        Some("6629fae49393a05397450978507c4ef1")
    );
    // a qop it does not know, or another algorithm, is no response
    assert_eq!(
        digest_response("MD5", &ha1, nonce, Some("auth-int"), "00000001", "0a4f113b", &ha2),
        None
    );
    assert_eq!(digest_response("SHA-1", &ha1, nonce, None, "", "", &ha2), None);
    assert_eq!(
        digest_response("SHA-256", "a", "n", None, "", "", "b"),
        Some(hex(&Sha256::digest(b"a:n:b")))
    );
}
