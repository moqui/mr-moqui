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
use crate::rng::TestRng;
use crate::vault;

struct Org {
    rng: TestRng,
    maker: PersonalKey,
    holders: Vec<PersonalKey>,
    record: RecoveryKeyRecord,
    shares: Vec<HolderShare>,
}

fn org() -> Org {
    let mut rng = TestRng::new("recovery");
    let maker = PersonalKey::generate(&mut rng);
    let holders: Vec<PersonalKey> = (0..5).map(|_| PersonalKey::generate(&mut rng)).collect();
    let publics: Vec<PersonalPublic> = holders.iter().map(PersonalKey::public).collect();
    let ids: Vec<String> = (0..5).map(|i| format!("holder-{i}")).collect();
    let list: Vec<Holder> = ids
        .iter()
        .zip(publics.iter())
        .map(|(id, public)| Holder {
            user_id: id,
            key_id: id,
            public,
        })
        .collect();
    let signer = vault::Signer {
        user_id: "maker",
        key_id: "pk-maker",
        key: &maker,
    };
    let (record, shares) = make("rk-1", 3, &list, signer, &mut rng).unwrap();
    Org {
        rng,
        maker,
        holders,
        record,
        shares,
    }
}

#[test]
fn k_holders_recover_a_vault_key_and_fewer_cannot() {
    let mut o = org();
    let maker = o.maker.public().verify;
    assert_eq!((o.record.k, o.record.n), (3, 5));
    let vault_key = vault::new_vault_key(&mut o.rng);
    let member = PersonalKey::generate(&mut o.rng);
    let signer = vault::Signer {
        user_id: "member",
        key_id: "pk-member",
        key: &member,
    };
    let wrap = wrap_vault_key(&vault_key, "vault-1", 4, &o.record, &maker, signer, &mut o.rng).unwrap();

    // Each holder opens only their own share.
    let opened: Vec<Share> = o
        .shares
        .iter()
        .zip(o.holders.iter())
        .map(|(held, holder)| open_share(held, holder, &maker).unwrap())
        .collect();
    assert_eq!(
        open_share(&o.shares[0], &o.holders[1], &maker).unwrap_err(),
        Error::Signature
    );

    let recovery = combine(&o.record, &maker, &opened[1..4]).unwrap();
    let member_verify = member.public().verify;
    assert_eq!(
        open_vault_key(&wrap, &o.record, &recovery, &member_verify).unwrap(),
        vault_key
    );
    assert!(matches!(
        combine(&o.record, &maker, &opened[..2]),
        Err(Error::Shares(_))
    ));
    // A share of another split gives another key, which the signed public half refuses.
    let mut wrong = opened[..3].to_vec();
    wrong[2].value[10] ^= 1;
    assert!(matches!(combine(&o.record, &maker, &wrong), Err(Error::Refused(_))));
}

#[test]
fn an_unsigned_or_replaced_recovery_key_is_refused() {
    let mut o = org();
    let maker = o.maker.public().verify;
    let vault_key = vault::new_vault_key(&mut o.rng);
    let member = PersonalKey::generate(&mut o.rng);
    let signer = vault::Signer {
        user_id: "member",
        key_id: "pk-member",
        key: &member,
    };

    let server_key = hpke::SecretKey::random(&mut o.rng);
    let replaced = RecoveryKeyRecord {
        public: server_key.public().unwrap(),
        ..o.record.clone()
    };
    let refused = wrap_vault_key(&vault_key, "vault-1", 1, &replaced, &maker, signer, &mut o.rng);
    assert_eq!(refused.unwrap_err(), Error::Signature);
    let unsigned = RecoveryKeyRecord {
        signature: Vec::new(),
        ..o.record.clone()
    };
    let refused = wrap_vault_key(&vault_key, "vault-1", 1, &unsigned, &maker, signer, &mut o.rng);
    assert_eq!(refused.unwrap_err(), Error::Signature);
    let stranger = PersonalKey::generate(&mut o.rng).public().verify;
    let refused = wrap_vault_key(&vault_key, "vault-1", 1, &o.record, &stranger, signer, &mut o.rng);
    assert_eq!(refused.unwrap_err(), Error::Signature);
}

#[test]
fn a_recovery_wrap_needs_its_signer() {
    let mut o = org();
    let maker = o.maker.public().verify;
    let vault_key = vault::new_vault_key(&mut o.rng);
    let member = PersonalKey::generate(&mut o.rng);
    let signer = vault::Signer {
        user_id: "member",
        key_id: "pk-member",
        key: &member,
    };
    let wrap = wrap_vault_key(&vault_key, "vault-1", 1, &o.record, &maker, signer, &mut o.rng).unwrap();
    let opened: Vec<Share> = o
        .shares
        .iter()
        .zip(o.holders.iter())
        .map(|(held, holder)| open_share(held, holder, &maker).unwrap())
        .collect();
    let recovery = combine(&o.record, &maker, &opened[..3]).unwrap();
    let stranger = PersonalKey::generate(&mut o.rng).public().verify;
    assert_eq!(
        open_vault_key(&wrap, &o.record, &recovery, &stranger).unwrap_err(),
        Error::Signature
    );
    let moved = RecoveryWrap {
        vault_id: "vault-2".to_string(),
        ..wrap
    };
    assert_eq!(
        open_vault_key(&moved, &o.record, &recovery, &member.public().verify).unwrap_err(),
        Error::Signature
    );
}
