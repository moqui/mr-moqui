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
use crate::personal::PersonalPublic;
use crate::sign::VerifyingKey;
use crate::text::unhex;

const PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/vectors.json");

fn file() -> Value {
    let text = std::fs::read_to_string(PATH).expect("testdata/vectors.json: MOQUI_REGENERATE_VECTORS=1 writes it");
    serde_json::from_str(&text).expect("vectors.json is JSON")
}

fn bytes(value: &Value) -> Vec<u8> {
    unhex(value.as_str().expect("a hex string")).expect("hex")
}

fn key(value: &Value) -> Key32 {
    Key32::from_slice(&bytes(value)).unwrap()
}

#[test]
fn the_vectors_are_what_the_code_makes() {
    let made = generate().unwrap();
    if std::env::var_os("MOQUI_REGENERATE_VECTORS").is_some() {
        let mut text = serde_json::to_string_pretty(&made).unwrap();
        text.push('\n');
        std::fs::write(PATH, text).unwrap();
        return;
    }
    assert_eq!(
        made,
        file(),
        "the vectors changed: rerun with MOQUI_REGENERATE_VECTORS=1 and read the diff"
    );
}

#[test]
fn the_cfrg_vector_is_computed_not_copied() {
    let made = generate().unwrap();
    let cfrg = &made["hpke_cfrg"];
    assert_eq!(cfrg["computed_pkRm"], cfrg["pkRm"]);
    assert_eq!(cfrg["computed_enc"], cfrg["enc"]);
    assert_eq!(cfrg["computed_ct"], cfrg["ct"]);
    assert_eq!(cfrg["computed_pt"], cfrg["pt"]);
}

#[test]
fn the_file_opens_with_its_own_keys() {
    let v = file();
    let aead_v = &v["aead"];
    let ad = [bytes(&aead_v["ad"][0]), bytes(&aead_v["ad"][1])];
    let opened = aead::open(&key(&aead_v["key"]), &[&ad[0], &ad[1]], &bytes(&aead_v["sealed"])).unwrap();
    assert_eq!(&opened[..], &bytes(&aead_v["plaintext"])[..]);

    let vault_v = &v["vault"];
    let member = PersonalKey::from_secrets(
        &key(&vault_v["member_encrypt_secret"]),
        &key(&vault_v["member_sign_seed"]),
    );
    let manager = PersonalPublic::from_bytes(&bytes(&vault_v["manager_public"])).unwrap();
    let wrap = vault::MemberWrap {
        vault_id: "vault-1".to_string(),
        key_version: 1,
        member_user_id: "member".to_string(),
        member_key_id: "pk-m".to_string(),
        sealed: bytes(&vault_v["member_wrap"]["sealed"]),
        signer_user_id: "manager".to_string(),
        signer_key_id: "pk-a".to_string(),
        signature: bytes(&vault_v["member_wrap"]["signature"]),
    };
    assert_eq!(
        vault::open_member_wrap(&wrap, &member, &manager.verify).unwrap(),
        key(&vault_v["vault_key"])
    );

    let shamir_v = &v["shamir"];
    let shares: Vec<shamir::Share> = (0..3)
        .map(|i| shamir::Share::from_text(shamir_v["shares"][i].as_str().unwrap()).unwrap())
        .collect();
    assert_eq!(&shamir::combine(&shares).unwrap()[..], &bytes(&shamir_v["secret"])[..]);

    let personal_v = &v["personal"];
    let public = PersonalPublic::from_bytes(&bytes(&personal_v["public"])).unwrap();
    let code = personal_v["recovery_code"].as_str().unwrap();
    let opened =
        personal::open_with_recovery_code(&bytes(&personal_v["recovery_wrap"]), "pk-1", &public, code).unwrap();
    assert_eq!(opened.public(), public);
    assert_eq!(
        personal_v["fingerprint"].as_str().unwrap(),
        fingerprint::personal(&public)
    );

    let sign_v = &v["sign"];
    let verify = VerifyingKey::from_bytes(&bytes(&sign_v["public"])).unwrap();
    let parts = [bytes(&sign_v["parts"][0]), bytes(&sign_v["parts"][1])];
    verify
        .verify(
            Statement::MemberWrap,
            &[&parts[0], &parts[1]],
            &bytes(&sign_v["signature"]),
        )
        .unwrap();
}
