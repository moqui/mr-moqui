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

//! Vault keys, item keys and bodies (plans/Vault-Instances.md §3.2): a single-secret vault's key under a password
//! (Argon2id) or a generated key (HKDF); a shared-key vault's key wrapped to each member's personal key and signed by
//! its maker; item keys wrapped by the vault key, or to an operational vault's deposit key; item bodies under their
//! item key. An operational item's type, binding and clear name are in the associated data of its item-key wrap and
//! of its body.

use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

use crate::aead;
use crate::hpke;
use crate::kdf::{self, Argon2Params, PasswordWrap};
use crate::keys::Key32;
use crate::names::{refs, ItemLabel, Names, Purpose};
use crate::personal::{self, PersonalKey, PersonalPublic};
use crate::rng::Rng;
use crate::sign::{Statement, VerifyingKey, SIGNATURE_LEN};
use crate::Error;

const GENERATED_INFO: &[u8] = b"moqui-vault generated key";

/// Which item a key or body belongs to. `label` is an operational item's clear columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemRef<'a> {
    pub vault_id: &'a str,
    pub item_id: &'a str,
    pub key_version: u64,
    pub label: Option<ItemLabel<'a>>,
}

impl<'a> ItemRef<'a> {
    fn names(&self, purpose: Purpose) -> Names<'a> {
        Names::new(purpose, self.vault_id, self.item_id, self.key_version).with_label(self.label)
    }
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// A new random vault key.
pub fn new_vault_key(rng: &mut dyn Rng) -> Key32 {
    Key32::random(rng)
}

// ----- Single-secret vaults -----

/// Wraps a single-secret vault's key under its password.
pub fn wrap_vault_key_with_password(
    vault_key: &Key32,
    vault_id: &str,
    key_version: u64,
    password: &[u8],
    rng: &mut dyn Rng,
) -> Result<PasswordWrap, Error> {
    let parts = Names::new(Purpose::VaultKey, vault_id, "", key_version).parts(&[b"password"]);
    kdf::wrap_with_password(vault_key.as_bytes(), password, &refs(&parts), rng)
}

/// Opens a single-secret vault's key, with the wrap to store again when it was under the Argon2id target.
pub fn open_vault_key_with_password(
    wrap: &PasswordWrap,
    vault_id: &str,
    key_version: u64,
    password: &[u8],
    rng: &mut dyn Rng,
) -> Result<(Key32, Option<PasswordWrap>), Error> {
    let parts = Names::new(Purpose::VaultKey, vault_id, "", key_version).parts(&[b"password"]);
    let opened = kdf::open_with_password(wrap, password, &refs(&parts))?;
    let key = Key32::from_slice(&opened)?;
    let rewrapped = if kdf::needs_rewrap(wrap.params) {
        Some(kdf::wrap_with_password_at(
            key.as_bytes(),
            password,
            &refs(&parts),
            Argon2Params::TARGET,
            rng,
        )?)
    } else {
        None
    };
    Ok((key, rewrapped))
}

/// A new generated key for a single-secret vault: 256 random bits as grouped base32, to download or print.
pub fn new_generated_key(rng: &mut dyn Rng) -> Zeroizing<String> {
    personal::new_recovery_code(rng)
}

/// Wraps a single-secret vault's key under a generated key (HKDF, as it is already random): `salt ‖ AEAD`.
pub fn wrap_vault_key_with_generated(
    vault_key: &Key32,
    vault_id: &str,
    key_version: u64,
    generated: &str,
    rng: &mut dyn Rng,
) -> Result<Vec<u8>, Error> {
    let parts = Names::new(Purpose::VaultKey, vault_id, "", key_version).parts(&[b"generated"]);
    personal::seal_with_code(vault_key.as_bytes(), generated, GENERATED_INFO, &refs(&parts), rng)
}

pub fn open_vault_key_with_generated(
    sealed: &[u8],
    vault_id: &str,
    key_version: u64,
    generated: &str,
) -> Result<Key32, Error> {
    let parts = Names::new(Purpose::VaultKey, vault_id, "", key_version).parts(&[b"generated"]);
    Key32::from_slice(&personal::open_with_code(
        sealed,
        generated,
        GENERATED_INFO,
        &refs(&parts),
    )?)
}

// ----- Members of a shared-key vault -----

/// A member's wrap of the vault key, as `VaultMember` stores it: HPKE to the member's personal key, signed by the
/// member with the manage role who made it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberWrap {
    pub vault_id: String,
    pub key_version: u64,
    pub member_user_id: String,
    pub member_key_id: String,
    pub sealed: Vec<u8>,
    pub signer_user_id: String,
    pub signer_key_id: String,
    pub signature: Vec<u8>,
}

impl MemberWrap {
    fn signed_parts(&self, member: &PersonalPublic) -> Vec<Vec<u8>> {
        vec![
            self.vault_id.as_bytes().to_vec(),
            self.key_version.to_be_bytes().to_vec(),
            self.member_user_id.as_bytes().to_vec(),
            self.member_key_id.as_bytes().to_vec(),
            member.to_bytes().to_vec(),
            digest(&self.sealed).to_vec(),
            self.signer_user_id.as_bytes().to_vec(),
            self.signer_key_id.as_bytes().to_vec(),
        ]
    }

    /// Checks the signature with `signer`'s key. Whether that signer may make this wrap is the key-version chain's
    /// question ([`crate::keyversion::Chain::accept_wrap`]).
    pub fn verify(&self, member: &PersonalPublic, signer: &VerifyingKey) -> Result<(), Error> {
        signer.verify(
            Statement::MemberWrap,
            &refs(&self.signed_parts(member)),
            &self.signature,
        )
    }
}

/// The member a wrap is for.
#[derive(Clone, Copy, Debug)]
pub struct Recipient<'a> {
    pub user_id: &'a str,
    pub key_id: &'a str,
    pub public: &'a PersonalPublic,
}

/// The member who signs.
#[derive(Clone, Copy, Debug)]
pub struct Signer<'a> {
    pub user_id: &'a str,
    pub key_id: &'a str,
    pub key: &'a PersonalKey,
}

/// Wraps the vault key to a member and signs the wrap (§3.9, Add; and each remaining member at a rotation).
pub fn wrap_vault_key_to_member(
    vault_key: &Key32,
    vault_id: &str,
    key_version: u64,
    member: Recipient,
    signer: Signer,
    rng: &mut dyn Rng,
) -> Result<MemberWrap, Error> {
    let names = Names::new(Purpose::VaultKey, vault_id, "", key_version);
    let sealed = hpke::wrap(
        &member.public.encrypt,
        &names,
        &[member.user_id.as_bytes()],
        vault_key.as_bytes(),
        rng,
    )?;
    let mut wrap = MemberWrap {
        vault_id: vault_id.to_string(),
        key_version,
        member_user_id: member.user_id.to_string(),
        member_key_id: member.key_id.to_string(),
        sealed,
        signer_user_id: signer.user_id.to_string(),
        signer_key_id: signer.key_id.to_string(),
        signature: Vec::new(),
    };
    let signature: [u8; SIGNATURE_LEN] = signer
        .key
        .signing_key()
        .sign(Statement::MemberWrap, &refs(&wrap.signed_parts(member.public)));
    wrap.signature = signature.to_vec();
    Ok(wrap)
}

/// Opens this member's wrap after checking its signature with `signer`'s key. A missing or wrong signature is
/// refused before anything is decrypted.
pub fn open_member_wrap(wrap: &MemberWrap, me: &PersonalKey, signer: &VerifyingKey) -> Result<Key32, Error> {
    wrap.verify(&me.public(), signer)?;
    let names = Names::new(Purpose::VaultKey, &wrap.vault_id, "", wrap.key_version);
    hpke::unwrap_key(
        me.encrypt_key(),
        &names,
        &[wrap.member_user_id.as_bytes()],
        &wrap.sealed,
    )
}

// ----- Items -----

/// A new random item key.
pub fn new_item_key(rng: &mut dyn Rng) -> Key32 {
    Key32::random(rng)
}

/// Wraps an item key by the vault key.
pub fn wrap_item_key(vault_key: &Key32, item_key: &Key32, item: ItemRef, rng: &mut dyn Rng) -> Vec<u8> {
    let parts = item.names(Purpose::ItemKey).parts(&[]);
    aead::seal(vault_key, &refs(&parts), item_key.as_bytes(), rng)
}

/// Unwraps an item key; one moved to another item, vault or version, or an operational item under another type,
/// binding or name, does not open.
pub fn unwrap_item_key(vault_key: &Key32, wrapped: &[u8], item: ItemRef) -> Result<Key32, Error> {
    let parts = item.names(Purpose::ItemKey).parts(&[]);
    Key32::from_slice(&aead::open(vault_key, &refs(&parts), wrapped)?)
}

/// Seals an item's JSON body under its item key, padded, at most 64 KiB.
pub fn seal_item_body(item_key: &Key32, item: ItemRef, body: &[u8], rng: &mut dyn Rng) -> Result<Vec<u8>, Error> {
    let parts = item.names(Purpose::Body).parts(&[]);
    aead::seal_body(item_key, &refs(&parts), body, rng)
}

pub fn open_item_body(item_key: &Key32, item: ItemRef, sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
    let parts = item.names(Purpose::Body).parts(&[]);
    aead::open_body(item_key, &refs(&parts), sealed)
}

// ----- The deposit key of an operational vault -----

/// The deposit key's public half as an operational vault stores it in the clear, signed by the member with the
/// manage role who made it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DepositPublic {
    pub vault_id: String,
    pub key_version: u64,
    pub public: hpke::PublicKey,
    pub signer_user_id: String,
    pub signer_key_id: String,
    pub signature: Vec<u8>,
}

impl DepositPublic {
    fn signed_parts(&self) -> Vec<Vec<u8>> {
        vec![
            self.vault_id.as_bytes().to_vec(),
            self.key_version.to_be_bytes().to_vec(),
            self.public.as_bytes().to_vec(),
            self.signer_user_id.as_bytes().to_vec(),
            self.signer_key_id.as_bytes().to_vec(),
        ]
    }

    /// The server checks this against the vault's signed member list before it deposits; a member's client on each
    /// open.
    pub fn verify(&self, signer: &VerifyingKey) -> Result<(), Error> {
        signer.verify(Statement::DepositKey, &refs(&self.signed_parts()), &self.signature)
    }
}

/// A new deposit key: its private half wrapped by the vault key, and its public half signed.
pub fn new_deposit_key(
    vault_key: &Key32,
    vault_id: &str,
    key_version: u64,
    signer: Signer,
    rng: &mut dyn Rng,
) -> Result<(DepositPublic, Vec<u8>), Error> {
    let secret = hpke::SecretKey::random(rng);
    let mut public = DepositPublic {
        vault_id: vault_id.to_string(),
        key_version,
        public: secret.public()?,
        signer_user_id: signer.user_id.to_string(),
        signer_key_id: signer.key_id.to_string(),
        signature: Vec::new(),
    };
    public.signature = signer
        .key
        .signing_key()
        .sign(Statement::DepositKey, &refs(&public.signed_parts()))
        .to_vec();
    let parts = Names::new(Purpose::DepositKey, vault_id, "", key_version).parts(&[]);
    let wrapped = aead::seal(vault_key, &refs(&parts), secret.as_key().as_bytes(), rng);
    Ok((public, wrapped))
}

/// The deposit key's private half, unwrapped by the vault key.
pub fn open_deposit_key(
    vault_key: &Key32,
    vault_id: &str,
    key_version: u64,
    wrapped: &[u8],
) -> Result<hpke::SecretKey, Error> {
    let parts = Names::new(Purpose::DepositKey, vault_id, "", key_version).parts(&[]);
    let opened = aead::open(vault_key, &refs(&parts), wrapped)?;
    Ok(hpke::SecretKey::from_key(Key32::from_slice(&opened)?))
}

/// The server's deposit of a new item's key: checked against the deposit key's signature by `signer` (whom the
/// caller found with the manage role in the vault's signed member list), then wrapped to it. The server can write
/// the item but not read it back.
pub fn wrap_item_key_to_deposit(
    deposit: &DepositPublic,
    signer: &VerifyingKey,
    item_key: &Key32,
    item: ItemRef,
    rng: &mut dyn Rng,
) -> Result<Vec<u8>, Error> {
    deposit.verify(signer)?;
    if deposit.vault_id != item.vault_id {
        return Err(Error::Refused("the deposit key is another vault's"));
    }
    hpke::wrap(
        &deposit.public,
        &item.names(Purpose::DepositItemKey),
        &[],
        item_key.as_bytes(),
        rng,
    )
}

/// A member's unwrap of a deposited item key, with the deposit key's private half; the client then wraps it by the
/// vault key ([`wrap_item_key`]).
pub fn unwrap_deposited_item_key(deposit: &hpke::SecretKey, wrapped: &[u8], item: ItemRef) -> Result<Key32, Error> {
    hpke::unwrap_key(deposit, &item.names(Purpose::DepositItemKey), &[], wrapped)
}

#[cfg(test)]
#[path = "vault_tests.rs"]
mod tests;
