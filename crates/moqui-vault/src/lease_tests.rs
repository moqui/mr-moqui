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

struct Setup {
    rng: TestRng,
    vault_key: Key32,
    item_keys: Vec<Key32>,
    wrapped: Vec<(String, Vec<u8>)>,
    server_key: SigningKey,
    approver: PersonalKey,
}

fn item(id: &str, binding: &str) -> LeaseItem {
    LeaseItem {
        item_id: id.to_string(),
        item_type: "database-credential".to_string(),
        binding: binding.to_string(),
        name: format!("{id} name"),
    }
}

fn setup() -> Setup {
    let mut rng = TestRng::new("lease");
    let vault_key = vault::new_vault_key(&mut rng);
    let mut item_keys = Vec::new();
    let mut wrapped = Vec::new();
    for leased in [
        item("item-1", "AppInstance:1:dbPassword"),
        item("item-2", "AppInstance:2:dbPassword"),
    ] {
        let key = vault::new_item_key(&mut rng);
        let item_ref = ItemRef {
            vault_id: "vault-1",
            item_id: &leased.item_id,
            key_version: 1,
            label: Some(leased.label()),
        };
        wrapped.push((
            leased.item_id.clone(),
            vault::wrap_item_key(&vault_key, &key, item_ref, &mut rng),
        ));
        item_keys.push(key);
    }
    let server_key = SigningKey::random(&mut rng);
    let approver = PersonalKey::generate(&mut rng);
    Setup {
        rng,
        vault_key,
        item_keys,
        wrapped,
        server_key,
        approver,
    }
}

fn make_lease(s: &mut Setup, items: Vec<LeaseItem>) -> (Lease, hpke::SecretKey) {
    let (secret, public) = new_lease_key(&mut s.rng).unwrap();
    let mut lease = Lease {
        lease_id: "lease-1".to_string(),
        vault_id: "vault-1".to_string(),
        key_version: 1,
        items,
        operation: "provision#AppInstance".to_string(),
        cluster_member: "member-a".to_string(),
        expires_at: 1_800_000_000_000,
        public,
        signature: Vec::new(),
    };
    lease.sign(&s.server_key);
    (lease, secret)
}

fn approve_as(s: &mut Setup, lease: &Lease, checked: &[VerifyingKey]) -> Result<Approval, Error> {
    let wrapped: Vec<(&str, &[u8])> = s.wrapped.iter().map(|(id, w)| (id.as_str(), w.as_slice())).collect();
    let lease_key = s.server_key.verifying_key();
    let input = ApproveInput {
        lease,
        lease_key: &lease_key,
        checked,
        vault_key: &s.vault_key,
        wrapped_item_keys: &wrapped,
        approver_user_id: "approver",
        approver: &s.approver,
    };
    approve(input, &mut s.rng)
}

#[test]
fn an_approved_lease_opens_its_items_on_the_server() {
    let mut s = setup();
    let (lease, secret) = make_lease(&mut s, vec![item("item-1", "AppInstance:1:dbPassword")]);
    let checked = [s.server_key.verifying_key()];
    let approval = approve_as(&mut s, &lease, &checked).unwrap();
    let keys = open_approval(&lease, &secret, &approval, &s.approver.public().verify).unwrap();
    assert_eq!(keys.len(), 1, "a lease opens only the items it names");
    assert_eq!(keys[0].0, "item-1");
    assert_eq!(keys[0].1, s.item_keys[0]);

    // Another lease's private key, or the approval checked with another member's key, opens nothing.
    let (other_secret, _) = new_lease_key(&mut s.rng).unwrap();
    assert_eq!(
        open_approval(&lease, &other_secret, &approval, &s.approver.public().verify).unwrap_err(),
        Error::Open
    );
    let stranger = PersonalKey::generate(&mut s.rng).public().verify;
    assert_eq!(
        open_approval(&lease, &secret, &approval, &stranger).unwrap_err(),
        Error::Signature
    );
    let mut tampered = approval.clone();
    tampered.sealed[0].sealed[5] ^= 1;
    assert_eq!(
        open_approval(&lease, &secret, &tampered, &s.approver.public().verify).unwrap_err(),
        Error::Signature
    );
}

#[test]
fn a_lease_changed_after_signing_or_unsigned_is_refused() {
    let mut s = setup();
    let (lease, _) = make_lease(&mut s, vec![item("item-1", "AppInstance:1:dbPassword")]);
    let checked = [s.server_key.verifying_key()];

    // A database writer's own public key on the lease.
    let (_, planted) = new_lease_key(&mut s.rng).unwrap();
    let swapped = Lease {
        public: planted,
        ..lease.clone()
    };
    assert_eq!(approve_as(&mut s, &swapped, &checked).unwrap_err(), Error::Signature);
    // An item added to the list.
    let mut more = lease.clone();
    more.items.push(item("item-2", "AppInstance:2:dbPassword"));
    assert_eq!(approve_as(&mut s, &more, &checked).unwrap_err(), Error::Signature);
    // A longer time limit, another cluster member.
    let later = Lease {
        expires_at: lease.expires_at + 1,
        ..lease.clone()
    };
    assert_eq!(approve_as(&mut s, &later, &checked).unwrap_err(), Error::Signature);
    let unsigned = Lease {
        signature: Vec::new(),
        ..lease.clone()
    };
    assert_eq!(approve_as(&mut s, &unsigned, &checked).unwrap_err(), Error::Signature);
}

#[test]
fn a_lease_signed_by_an_unchecked_lease_key_is_refused() {
    let mut s = setup();
    let (lease, _) = make_lease(&mut s, vec![item("item-1", "AppInstance:1:dbPassword")]);
    assert!(matches!(approve_as(&mut s, &lease, &[]), Err(Error::Refused(_))));
    let other = SigningKey::random(&mut s.rng).verifying_key();
    assert!(matches!(approve_as(&mut s, &lease, &[other]), Err(Error::Refused(_))));
}

#[test]
fn a_binding_pointed_at_another_item_fails_at_the_unwrap() {
    let mut s = setup();
    // The lease asks for item-2 under item-1's harmless binding: signed, but item-2's key does not unwrap under it.
    let mut pointed = item("item-2", "AppInstance:1:dbPassword");
    pointed.name = "item-1 name".to_string();
    let (lease, _) = make_lease(&mut s, vec![pointed]);
    let checked = [s.server_key.verifying_key()];
    assert_eq!(approve_as(&mut s, &lease, &checked).unwrap_err(), Error::Open);
    // An item the client has no wrapped key for.
    let (lease, _) = make_lease(&mut s, vec![item("item-9", "AppInstance:9:dbPassword")]);
    assert!(matches!(approve_as(&mut s, &lease, &checked), Err(Error::Refused(_))));
}
