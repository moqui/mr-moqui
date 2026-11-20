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

struct Org {
    rng: TestRng,
    maker: PersonalKey,
    holders: Vec<PersonalKey>,
    ids: Vec<String>,
    record: RecoveryKeyRecord,
    shares: Vec<HolderShare>,
}

/// A recovery key split 3 of 5, made by `maker`.
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
    let signer = Signer {
        user_id: "maker",
        key_id: "pk-maker",
        key: &maker,
    };
    let (record, shares) = make("rk-1", 3, &list, signer, &mut rng).unwrap();
    Org {
        rng,
        maker,
        holders,
        ids,
        record,
        shares,
    }
}

impl Org {
    fn signer(&self, at: usize) -> Signer<'_> {
        Signer {
            user_id: &self.ids[at],
            key_id: &self.ids[at],
            key: &self.holders[at],
        }
    }
}

#[test]
fn k_shares_give_the_recovery_key_and_fewer_or_wrong_ones_do_not() {
    let o = org();
    let maker = o.maker.public().verify;
    assert_eq!((o.record.k, o.record.n), (3, 5));
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
    assert_eq!(recovery.public(), o.record.public);
    assert_eq!(o.record.member().user_id, "recovery:rk-1");
    assert!(matches!(
        combine(&o.record, &maker, &opened[..2]),
        Err(Error::Shares(_))
    ));
    // A changed share gives another key, which the signed public halves refuse.
    let mut wrong = opened[..3].to_vec();
    wrong[2].value[10] ^= 1;
    assert!(matches!(combine(&o.record, &maker, &wrong), Err(Error::Refused(_))));
}

#[test]
fn an_unsigned_or_replaced_recovery_key_is_refused() {
    let mut o = org();
    let maker = o.maker.public().verify;
    o.record.verify(&maker).unwrap();
    let server_key = PersonalKey::generate(&mut o.rng);
    let replaced = RecoveryKeyRecord {
        public: server_key.public(),
        ..o.record.clone()
    };
    assert_eq!(replaced.verify(&maker).unwrap_err(), Error::Signature);
    let unsigned = RecoveryKeyRecord {
        signature: Vec::new(),
        ..o.record.clone()
    };
    assert_eq!(unsigned.verify(&maker).unwrap_err(), Error::Signature);
    let stranger = PersonalKey::generate(&mut o.rng).public().verify;
    assert_eq!(o.record.verify(&stranger).unwrap_err(), Error::Signature);
}

#[test]
fn a_session_takes_k_holders_shares_sealed_to_its_ephemeral_key() {
    let mut o = org();
    let maker = o.maker.public().verify;
    let subject = PersonalKey::generate(&mut o.rng).public();
    let to = Recipient {
        user_id: "alice",
        key_id: "pk-alice-2",
        public: &subject,
    };
    let mut rng = TestRng::new("recovery-session");
    let (session, ephemeral) = start_session("rs-1", "rk-1", "vault-1", to, o.signer(0), &mut rng).unwrap();
    let recovering = o.holders[0].public().verify;
    session.verify(&recovering).unwrap();

    // Holders 1 and 2 seal their shares to the session; a session its holder did not sign is refused.
    let sent: Vec<SessionShare> = (1..3)
        .map(|at| deliver_share(&o.shares[at], o.signer(at), &maker, &session, &recovering, &mut rng).unwrap())
        .collect();
    let stranger = PersonalKey::generate(&mut rng).public().verify;
    assert_eq!(
        deliver_share(&o.shares[1], o.signer(1), &maker, &session, &stranger, &mut rng).unwrap_err(),
        Error::Signature
    );
    let elsewhere = RecoverySession {
        vault_id: "vault-2".to_string(),
        ..session.clone()
    };
    assert_eq!(
        deliver_share(&o.shares[1], o.signer(1), &maker, &elsewhere, &recovering, &mut rng).unwrap_err(),
        Error::Signature
    );
    // a holder delivers only their own share
    assert!(deliver_share(&o.shares[2], o.signer(1), &maker, &session, &recovering, &mut rng).is_err());

    let own = || open_share(&o.shares[0], &o.holders[0], &maker).unwrap();
    let publics: Vec<PersonalPublic> = o.holders.iter().map(PersonalKey::public).collect();
    let delivered = |at: usize, holder: usize| Delivered {
        share: &sent[at],
        holder: &o.shares[holder],
        public: &publics[holder],
    };
    let key = combine_session(
        &o.record,
        &maker,
        &session,
        &ephemeral,
        own(),
        &[delivered(0, 1), delivered(1, 2)],
    )
    .unwrap();
    assert_eq!(key.public(), o.record.public);
    // k - 1 shares, or the same holder's share twice, do not combine
    let short = combine_session(&o.record, &maker, &session, &ephemeral, own(), &[delivered(0, 1)]);
    assert!(matches!(short, Err(Error::Shares(_))));
    let twice = combine_session(
        &o.record,
        &maker,
        &session,
        &ephemeral,
        own(),
        &[delivered(0, 1), delivered(0, 1)],
    );
    assert!(matches!(twice, Err(Error::Shares(_))));
    // a share passed off under another holder's row is refused
    let swapped = combine_session(
        &o.record,
        &maker,
        &session,
        &ephemeral,
        own(),
        &[delivered(0, 2), delivered(1, 2)],
    );
    assert!(swapped.is_err());
    // another session's ephemeral key does not open them
    let (_, other_ephemeral) = start_session("rs-2", "rk-1", "vault-1", to, o.signer(0), &mut rng).unwrap();
    let foreign = combine_session(
        &o.record,
        &maker,
        &session,
        &other_ephemeral,
        own(),
        &[delivered(0, 1), delivered(1, 2)],
    );
    assert_eq!(foreign.unwrap_err(), Error::Open);
}
