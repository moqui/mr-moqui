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

const LABEL: ItemLabel = ItemLabel {
    item_type: "database-credential",
    binding: "AppInstance:100:dbPassword",
    name: "db",
};

fn item(label: Option<ItemLabel<'static>>) -> ItemRef<'static> {
    ItemRef {
        vault_id: "vault-1",
        item_id: "item-1",
        key_version: 2,
        label,
    }
}

#[test]
fn a_password_opens_a_single_secret_vault_and_a_wrong_one_fails() {
    let mut rng = TestRng::new("vault-password");
    let vault_key = new_vault_key(&mut rng);
    let wrap = wrap_vault_key_with_password(&vault_key, "vault-1", 1, b"vault password", &mut rng).unwrap();
    let (opened, rewrapped) = open_vault_key_with_password(&wrap, "vault-1", 1, b"vault password", &mut rng).unwrap();
    assert_eq!(opened, vault_key);
    assert!(rewrapped.is_none());
    let wrong = open_vault_key_with_password(&wrap, "vault-1", 1, b"vault passworD", &mut rng);
    assert_eq!(wrong.unwrap_err(), Error::Open);
    let moved = open_vault_key_with_password(&wrap, "vault-2", 1, b"vault password", &mut rng);
    assert_eq!(moved.unwrap_err(), Error::Open);
}

#[test]
fn a_generated_key_opens_a_single_secret_vault_and_another_fails() {
    let mut rng = TestRng::new("vault-generated");
    let vault_key = new_vault_key(&mut rng);
    let generated = new_generated_key(&mut rng);
    let sealed = wrap_vault_key_with_generated(&vault_key, "vault-1", 1, &generated, &mut rng).unwrap();
    assert_eq!(
        open_vault_key_with_generated(&sealed, "vault-1", 1, &generated).unwrap(),
        vault_key
    );
    let other = new_generated_key(&mut rng);
    assert_eq!(
        open_vault_key_with_generated(&sealed, "vault-1", 1, &other).unwrap_err(),
        Error::Open
    );
    assert_eq!(
        open_vault_key_with_generated(&sealed, "vault-1", 2, &generated).unwrap_err(),
        Error::Open
    );
}

#[test]
fn a_member_wrap_opens_only_with_a_good_signature() {
    let mut rng = TestRng::new("vault-member");
    let manager = PersonalKey::generate(&mut rng);
    let member = PersonalKey::generate(&mut rng);
    let member_public = member.public();
    let vault_key = new_vault_key(&mut rng);
    let recipient = Recipient {
        user_id: "member",
        key_id: "pk-m",
        public: &member_public,
    };
    let signer = Signer {
        user_id: "manager",
        key_id: "pk-a",
        key: &manager,
    };
    let wrap = wrap_vault_key_to_member(&vault_key, "vault-1", 1, recipient, signer, &mut rng).unwrap();
    let manager_verify = manager.public().verify;
    assert_eq!(open_member_wrap(&wrap, &member, &manager_verify).unwrap(), vault_key);

    // Signed by someone else, missing, or changed after signing: refused before the unwrap.
    let stranger = PersonalKey::generate(&mut rng).public().verify;
    assert_eq!(
        open_member_wrap(&wrap, &member, &stranger).unwrap_err(),
        Error::Signature
    );
    let unsigned = MemberWrap {
        signature: Vec::new(),
        ..wrap.clone()
    };
    assert_eq!(
        open_member_wrap(&unsigned, &member, &manager_verify).unwrap_err(),
        Error::Signature
    );
    let moved = MemberWrap {
        key_version: 2,
        ..wrap.clone()
    };
    assert_eq!(
        open_member_wrap(&moved, &member, &manager_verify).unwrap_err(),
        Error::Signature
    );
    let mut swapped = wrap.clone();
    swapped.sealed[40] ^= 1;
    assert_eq!(
        open_member_wrap(&swapped, &member, &manager_verify).unwrap_err(),
        Error::Signature
    );
    // A wrap a server made itself to the member's key, signed by nobody it can show.
    let forged = wrap_vault_key_to_member(
        &new_vault_key(&mut rng),
        "vault-1",
        1,
        recipient,
        Signer {
            user_id: "manager",
            key_id: "pk-a",
            key: &PersonalKey::generate(&mut rng),
        },
        &mut rng,
    )
    .unwrap();
    assert_eq!(
        open_member_wrap(&forged, &member, &manager_verify).unwrap_err(),
        Error::Signature
    );
}

#[test]
fn an_item_key_or_body_moved_to_another_item_vault_or_version_fails() {
    let mut rng = TestRng::new("vault-items");
    let vault_key = new_vault_key(&mut rng);
    let item_key = new_item_key(&mut rng);
    let wrapped = wrap_item_key(&vault_key, &item_key, item(None), &mut rng);
    let body = seal_item_body(&item_key, item(None), br#"{"v":1,"type":"login"}"#, &mut rng).unwrap();
    assert_eq!(unwrap_item_key(&vault_key, &wrapped, item(None)).unwrap(), item_key);
    assert_eq!(
        &open_item_body(&item_key, item(None), &body).unwrap()[..],
        br#"{"v":1,"type":"login"}"#
    );
    for moved in [
        ItemRef {
            item_id: "item-2",
            ..item(None)
        },
        ItemRef {
            vault_id: "vault-2",
            ..item(None)
        },
        ItemRef {
            key_version: 3,
            ..item(None)
        },
    ] {
        assert_eq!(
            unwrap_item_key(&vault_key, &wrapped, moved).unwrap_err(),
            Error::Open,
            "{moved:?}"
        );
        assert_eq!(
            open_item_body(&item_key, moved, &body).unwrap_err(),
            Error::Open,
            "{moved:?}"
        );
    }
    assert_eq!(
        unwrap_item_key(&new_vault_key(&mut rng), &wrapped, item(None)).unwrap_err(),
        Error::Open
    );
    // A wrapped item key is not a body, and a body is not a wrapped key.
    assert!(open_item_body(&vault_key, item(None), &wrapped).is_err());
}

#[test]
fn an_operational_item_under_another_type_binding_or_name_fails() {
    let mut rng = TestRng::new("vault-operational");
    let vault_key = new_vault_key(&mut rng);
    let item_key = new_item_key(&mut rng);
    let wrapped = wrap_item_key(&vault_key, &item_key, item(Some(LABEL)), &mut rng);
    let body = seal_item_body(&item_key, item(Some(LABEL)), b"{}", &mut rng).unwrap();
    assert_eq!(
        unwrap_item_key(&vault_key, &wrapped, item(Some(LABEL))).unwrap(),
        item_key
    );
    for label in [
        ItemLabel {
            item_type: "api-key",
            ..LABEL
        },
        ItemLabel {
            binding: "AppInstance:101:dbPassword",
            ..LABEL
        },
        ItemLabel { name: "other", ..LABEL },
    ] {
        assert_eq!(
            unwrap_item_key(&vault_key, &wrapped, item(Some(label))).unwrap_err(),
            Error::Open
        );
        assert_eq!(
            open_item_body(&item_key, item(Some(label)), &body).unwrap_err(),
            Error::Open
        );
    }
    assert_eq!(
        unwrap_item_key(&vault_key, &wrapped, item(None)).unwrap_err(),
        Error::Open
    );
}

#[test]
fn the_server_deposits_only_to_a_signed_deposit_key_and_cannot_read_back() {
    let mut rng = TestRng::new("vault-deposit");
    let manager = PersonalKey::generate(&mut rng);
    let vault_key = new_vault_key(&mut rng);
    let signer = Signer {
        user_id: "manager",
        key_id: "pk-a",
        key: &manager,
    };
    let (public, wrapped_private) = new_deposit_key(&vault_key, "vault-1", 2, signer, &mut rng).unwrap();
    let manager_verify = manager.public().verify;

    let item_key = new_item_key(&mut rng);
    let deposited = wrap_item_key_to_deposit(&public, &manager_verify, &item_key, item(Some(LABEL)), &mut rng).unwrap();
    // A member's client opens the deposit key with the vault key, then the item key.
    let deposit_secret = open_deposit_key(&vault_key, "vault-1", 2, &wrapped_private).unwrap();
    assert_eq!(
        unwrap_deposited_item_key(&deposit_secret, &deposited, item(Some(LABEL))).unwrap(),
        item_key
    );
    let other_binding = item(Some(ItemLabel {
        binding: "AppInstance:9:dbPassword",
        ..LABEL
    }));
    assert_eq!(
        unwrap_deposited_item_key(&deposit_secret, &deposited, other_binding).unwrap_err(),
        Error::Open
    );

    // A deposit key with no manager's signature, or with its public half replaced, is refused.
    let unsigned = DepositPublic {
        signature: Vec::new(),
        ..public.clone()
    };
    let refused = wrap_item_key_to_deposit(&unsigned, &manager_verify, &item_key, item(None), &mut rng);
    assert_eq!(refused.unwrap_err(), Error::Signature);
    let planted = DepositPublic {
        public: hpke::SecretKey::random(&mut rng).public().unwrap(),
        ..public.clone()
    };
    let refused = wrap_item_key_to_deposit(&planted, &manager_verify, &item_key, item(None), &mut rng);
    assert_eq!(refused.unwrap_err(), Error::Signature);
    let elsewhere = ItemRef {
        vault_id: "vault-9",
        ..item(None)
    };
    let refused = wrap_item_key_to_deposit(&public, &manager_verify, &item_key, elsewhere, &mut rng);
    assert!(matches!(refused, Err(Error::Refused(_))));
}
