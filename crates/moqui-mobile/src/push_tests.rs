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

// RFC 8291 Appendix A: the user agent's keys and the body the server sends them.
const UA_PRIVATE: &str = "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94";
const UA_PUBLIC: &str = "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
const AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";
const BODY: &str = "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN";

fn rfc_keys() -> PushKeys {
    PushKeys::from_text(&format!("{UA_PRIVATE}.{AUTH}")).unwrap()
}

#[test]
fn the_rfc_8291_example_opens_with_its_keys() {
    let keys = rfc_keys();
    assert_eq!(keys.public_b64(), UA_PUBLIC);
    assert_eq!(keys.auth_b64(), AUTH);
    assert_eq!(
        keys.open(&unb64(BODY).unwrap()).unwrap(),
        b"When I grow up, I want to be a watermelon"
    );
}

#[test]
fn a_push_for_other_keys_or_not_json_is_refused() {
    let other = PushKeys::generate().unwrap();
    assert!(other.open(&unb64(BODY).unwrap()).unwrap_err().contains("does not open"));
    // the example's plaintext is not JSON
    assert!(rfc_keys().open_b64(BODY).unwrap_err().contains("JSON"));
    assert!(rfc_keys().open(b"short").is_err());
}

/// A real account id: a host with dots and a hex digest (`moqui_client::account::account_id`).
const ACCOUNT_A: &str = "chat.example.com-0123456789ab";
const ACCOUNT_B: &str = "chat.example.org-ba9876543210";

/// plans/Comms-Messaging.md §7.4, Device key: each account has its own keys, made once and kept; opening never makes
/// keys, and keys that were registered and cannot be read now are an error, not replaced.
#[test]
fn each_account_has_its_own_keys_made_once_and_never_replaced() {
    let dir = std::env::temp_dir().join(format!("moqui-mobile-push-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let keys = moqui_client::keys::Keys::new(Box::new(moqui_client::keys::FileKeyStore::new(&dir)), "file");
    assert!(kept_keys(&keys, ACCOUNT_A).is_none(), "reading makes none");
    let first = keys_for(&keys, ACCOUNT_A, None).unwrap();
    let again = keys_for(&keys, ACCOUNT_A, Some(&first.public_b64())).unwrap();
    assert_eq!(first.public_b64(), again.public_b64());
    assert_eq!(first.auth_b64(), again.auth_b64());
    assert_eq!(unb64(&first.public_b64()).unwrap().len(), 65);
    let other = keys_for(&keys, ACCOUNT_B, None).unwrap();
    assert_ne!(first.public_b64(), other.public_b64(), "another account, other keys");
    assert_ne!(first.auth_b64(), other.auth_b64());
    assert_eq!(key_entry(ACCOUNT_A), "chat.example.com-0123456789ab.push-keys");
    // registered keys that cannot be read: an error, never new keys
    forget_keys(&keys, ACCOUNT_A);
    assert!(keys_for(&keys, ACCOUNT_A, Some(&first.public_b64())).is_err());
    assert!(kept_keys(&keys, ACCOUNT_A).is_none());
    assert!(PushKeys::from_text("not.keys").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}
