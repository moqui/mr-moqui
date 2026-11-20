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

//! Leases (plans/Vault-Instances.md §3.7): the server asks for an operation's items under an ephemeral X25519 key it
//! holds in memory, and signs the lease with its lease key; a member with the approve role checks that signature
//! against a lease key their user has checked, unwraps each named item key with the type, binding and name the lease
//! gives (so an item that is not the one named does not unwrap), seals them to the lease's public key and signs the
//! approval. Only the lease's private key opens what was sealed.

use sha2::{Digest as _, Sha256};

use crate::hpke;
use crate::keys::Key32;
use crate::names::{refs, ItemLabel, Names, Purpose};
use crate::personal::PersonalKey;
use crate::rng::Rng;
use crate::sign::{SigningKey, Statement, VerifyingKey};
use crate::vault::{self, ItemRef};
use crate::Error;

/// One item a lease names, as signed: its id and its clear columns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaseItem {
    pub item_id: String,
    pub item_type: String,
    pub binding: String,
    pub name: String,
}

impl LeaseItem {
    fn label(&self) -> ItemLabel<'_> {
        ItemLabel {
            item_type: &self.item_type,
            binding: &self.binding,
            name: &self.name,
        }
    }
}

/// A lease as the server signs it and the approver's dialog shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lease {
    pub lease_id: String,
    pub vault_id: String,
    pub key_version: u64,
    pub items: Vec<LeaseItem>,
    pub operation: String,
    pub cluster_member: String,
    /// The time limit, in milliseconds since the epoch.
    pub expires_at: i64,
    /// The ephemeral key the item keys are sealed to; its private half is in one cluster member's memory.
    pub public: hpke::PublicKey,
    pub signature: Vec<u8>,
}

impl Lease {
    fn signed_parts(&self) -> Vec<Vec<u8>> {
        let mut parts = vec![
            self.lease_id.as_bytes().to_vec(),
            self.vault_id.as_bytes().to_vec(),
            self.key_version.to_be_bytes().to_vec(),
            (self.items.len() as u64).to_be_bytes().to_vec(),
        ];
        for item in &self.items {
            parts.push(item.item_id.as_bytes().to_vec());
            parts.push(item.item_type.as_bytes().to_vec());
            parts.push(item.binding.as_bytes().to_vec());
            parts.push(item.name.as_bytes().to_vec());
        }
        parts.push(self.operation.as_bytes().to_vec());
        parts.push(self.cluster_member.as_bytes().to_vec());
        parts.push(self.expires_at.to_be_bytes().to_vec());
        parts.push(self.public.as_bytes().to_vec());
        parts
    }

    /// Signs the lease with the server's lease key (it fills `signature`).
    pub fn sign(&mut self, lease_key: &SigningKey) {
        self.signature = lease_key.sign(Statement::Lease, &refs(&self.signed_parts())).to_vec();
    }

    /// A client's check: signed by `lease_key`, and that key is one its user has checked.
    pub fn verify(&self, lease_key: &VerifyingKey, checked: &[VerifyingKey]) -> Result<(), Error> {
        lease_key.verify(Statement::Lease, &refs(&self.signed_parts()), &self.signature)?;
        if !checked.contains(lease_key) {
            return Err(Error::Refused("the lease key has not been checked"));
        }
        Ok(())
    }

    fn digest(&self) -> [u8; 32] {
        Sha256::digest(crate::aead::associated_data(&refs(&self.signed_parts()))).into()
    }

    fn names<'a>(&'a self, item: &'a LeaseItem) -> Names<'a> {
        Names::new(Purpose::Lease, &self.vault_id, &item.item_id, self.key_version).with_label(Some(item.label()))
    }
}

/// A new lease key pair: the private half stays in the server's memory for the lease's life.
pub fn new_lease_key(rng: &mut dyn Rng) -> Result<(hpke::SecretKey, hpke::PublicKey), Error> {
    let secret = hpke::SecretKey::random(rng);
    let public = secret.public()?;
    Ok((secret, public))
}

/// An item key sealed to a lease.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealedItemKey {
    pub item_id: String,
    pub sealed: Vec<u8>,
}

/// A member's approval, written to the lease row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Approval {
    pub lease_id: String,
    pub sealed: Vec<SealedItemKey>,
    pub approver_user_id: String,
    pub signature: Vec<u8>,
}

impl Approval {
    fn signed_parts(&self, lease: &Lease) -> Vec<Vec<u8>> {
        let mut parts = vec![
            self.lease_id.as_bytes().to_vec(),
            lease.digest().to_vec(),
            self.approver_user_id.as_bytes().to_vec(),
        ];
        for sealed in &self.sealed {
            parts.push(sealed.item_id.as_bytes().to_vec());
            parts.push(Sha256::digest(&sealed.sealed).to_vec());
        }
        parts
    }

    /// The server's check of the approver's signature, with the key of the approver's personal key.
    pub fn verify(&self, lease: &Lease, approver: &VerifyingKey) -> Result<(), Error> {
        if self.lease_id != lease.lease_id {
            return Err(Error::Refused("an approval of another lease"));
        }
        approver.verify(
            Statement::LeaseApproval,
            &refs(&self.signed_parts(lease)),
            &self.signature,
        )
    }
}

/// What an approver's client holds for the approval: the vault key and each item's wrapped key by item id.
pub struct ApproveInput<'a> {
    pub lease: &'a Lease,
    pub lease_key: &'a VerifyingKey,
    pub checked: &'a [VerifyingKey],
    pub vault_key: &'a Key32,
    pub wrapped_item_keys: &'a [(&'a str, &'a [u8])],
    pub approver_user_id: &'a str,
    pub approver: &'a PersonalKey,
}

/// Approves a lease in the member's client: checks it, unwraps each named item key under the lease's own type,
/// binding and name, seals them to the lease and signs the approval.
pub fn approve(input: ApproveInput, rng: &mut dyn Rng) -> Result<Approval, Error> {
    let lease = input.lease;
    lease.verify(input.lease_key, input.checked)?;
    let mut sealed = Vec::with_capacity(lease.items.len());
    for item in &lease.items {
        let Some((_, wrapped)) = input.wrapped_item_keys.iter().find(|(id, _)| *id == item.item_id) else {
            return Err(Error::Refused("a lease item has no wrapped key"));
        };
        let item_ref = ItemRef {
            vault_id: &lease.vault_id,
            item_id: &item.item_id,
            key_version: lease.key_version,
            label: Some(item.label()),
        };
        let item_key = vault::unwrap_item_key(input.vault_key, wrapped, item_ref)?;
        let to_lease = hpke::wrap(
            &lease.public,
            &lease.names(item),
            &[lease.lease_id.as_bytes()],
            item_key.as_bytes(),
            rng,
        )?;
        sealed.push(SealedItemKey {
            item_id: item.item_id.clone(),
            sealed: to_lease,
        });
    }
    let mut approval = Approval {
        lease_id: lease.lease_id.clone(),
        sealed,
        approver_user_id: input.approver_user_id.to_string(),
        signature: Vec::new(),
    };
    approval.signature = input
        .approver
        .signing_key()
        .sign(Statement::LeaseApproval, &refs(&approval.signed_parts(lease)))
        .to_vec();
    Ok(approval)
}

/// The server's open of an approval with the lease's private key: the approver's signature first, then each item
/// key, by item id.
pub fn open_approval(
    lease: &Lease,
    lease_secret: &hpke::SecretKey,
    approval: &Approval,
    approver: &VerifyingKey,
) -> Result<Vec<(String, Key32)>, Error> {
    approval.verify(lease, approver)?;
    let mut keys = Vec::with_capacity(lease.items.len());
    for item in &lease.items {
        let Some(sealed) = approval.sealed.iter().find(|s| s.item_id == item.item_id) else {
            return Err(Error::Refused("the approval lacks a lease item"));
        };
        let key = hpke::unwrap_key(
            lease_secret,
            &lease.names(item),
            &[lease.lease_id.as_bytes()],
            &sealed.sealed,
        )?;
        keys.push((item.item_id.clone(), key));
    }
    Ok(keys)
}

#[cfg(test)]
#[path = "lease_tests.rs"]
mod tests;
