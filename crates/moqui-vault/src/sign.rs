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

//! Ed25519 signatures over canonical statements (plans/archive/Vault-Instances.md §3.2, Signatures). A statement is its kind
//! and its parts, each written with its length, so no statement of one kind or with other parts has the same bytes.
//! Verification is strict (RFC 8032 with the small-order and malleability checks).

use std::fmt;

use ed25519_dalek::{Signature, Signer as _};

use crate::aead::associated_data;
use crate::keys::Key32;
use crate::rng::Rng;
use crate::text::hex;
use crate::Error;

pub const SIGNATURE_LEN: usize = 64;
const DOMAIN: &[u8] = b"moqui-vault-signature/1";

/// What a signature is over. Each kind has its own name in the signed bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Statement {
    /// A member's wrap of the vault key (§3.9, Add).
    MemberWrap,
    /// A key version with its member list (§3.9).
    KeyVersion,
    /// An operational vault's deposit public key (§3.2).
    DepositKey,
    /// An organization recovery key's public half (§3.10).
    RecoveryKey,
    /// A recovery-key share wrapped to a holder (§3.10, split k of n).
    RecoveryShare,
    /// A recovery session: the ephemeral key a recovering holder's client made, and what it recovers (§3.10).
    RecoverySession,
    /// A holder's share sealed to a recovery session's ephemeral key (§3.10).
    RecoverySessionShare,
    /// A delivered value (§3.3, `VaultDelivery`).
    Delivery,
    /// A lease, by the server's lease key (§3.7).
    Lease,
    /// A member's approval of a lease, with the item keys sealed to it (§3.7).
    LeaseApproval,
    /// A checked key (§3.5, `VaultKeyCheck`).
    KeyCheck,
    /// A managed instance's changed approver set, by an approver it pinned (§4.4, [`crate::approvers`]).
    ApproverSet,
    /// A managed instance's proof, on one link to its manager, that it holds its root key: by a key derived from it
    /// (§4.4, "Unsealed" after M6).
    UnsealedProof,
    /// The key a sealed managed instance made for one unseal, by its link key: a holder seals a share to it (§4.5).
    UnsealKey,
    /// A recovery holder's confirmation that they filed a managed instance's share, by their signing key (§4.5).
    ShareConfirm,
    /// A managed instance's new proof key after a rotate-root, by its old one (§4.4).
    ProofRotated,
}

impl Statement {
    pub fn as_str(self) -> &'static str {
        match self {
            Statement::MemberWrap => "member-wrap",
            Statement::KeyVersion => "key-version",
            Statement::DepositKey => "deposit-key",
            Statement::RecoveryKey => "recovery-key",
            Statement::RecoveryShare => "recovery-share",
            Statement::RecoverySession => "recovery-session",
            Statement::RecoverySessionShare => "recovery-session-share",
            Statement::Delivery => "delivery",
            Statement::Lease => "lease",
            Statement::LeaseApproval => "lease-approval",
            Statement::KeyCheck => "key-check",
            Statement::ApproverSet => "approver-set",
            Statement::UnsealedProof => "unsealed-proof",
            Statement::UnsealKey => "unseal-key",
            Statement::ShareConfirm => "share-confirm",
            Statement::ProofRotated => "proof-rotated",
        }
    }
}

/// The signed bytes of a statement.
pub fn statement_bytes(kind: Statement, parts: &[&[u8]]) -> Vec<u8> {
    let mut all: Vec<&[u8]> = Vec::with_capacity(parts.len() + 2);
    all.push(DOMAIN);
    all.push(kind.as_str().as_bytes());
    all.extend_from_slice(parts);
    associated_data(&all)
}

/// An Ed25519 signing key. Cleared on drop.
#[derive(Clone)]
pub struct SigningKey(ed25519_dalek::SigningKey);

impl SigningKey {
    pub fn random(rng: &mut dyn Rng) -> Self {
        Self::from_seed(&Key32::random(rng))
    }

    /// The key from its 32-byte seed (RFC 8032's private key).
    pub fn from_seed(seed: &Key32) -> Self {
        SigningKey(ed25519_dalek::SigningKey::from_bytes(seed.as_bytes()))
    }

    pub fn seed(&self) -> Key32 {
        Key32::from_slice(self.0.as_bytes()).expect("an Ed25519 seed is 32 bytes")
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        VerifyingKey(self.0.verifying_key())
    }

    pub fn sign(&self, kind: Statement, parts: &[&[u8]]) -> [u8; SIGNATURE_LEN] {
        self.0.sign(&statement_bytes(kind, parts)).to_bytes()
    }
}

impl fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SigningKey(redacted)")
    }
}

/// An Ed25519 public key.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct VerifyingKey(ed25519_dalek::VerifyingKey);

impl VerifyingKey {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let array: &[u8; 32] = bytes
            .try_into()
            .map_err(|_| Error::Format("an Ed25519 public key is 32 bytes"))?;
        ed25519_dalek::VerifyingKey::from_bytes(array)
            .map(VerifyingKey)
            .map_err(|_| Error::Format("not an Ed25519 public key"))
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }

    /// Checks `signature` over the statement; a missing (empty), malformed or wrong signature is [`Error::Signature`].
    pub fn verify(&self, kind: Statement, parts: &[&[u8]], signature: &[u8]) -> Result<(), Error> {
        let Ok(signature) = Signature::from_slice(signature) else {
            return Err(Error::Signature);
        };
        self.0
            .verify_strict(&statement_bytes(kind, parts), &signature)
            .map_err(|_| Error::Signature)
    }
}

impl fmt::Debug for VerifyingKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "VerifyingKey({})", hex(self.as_bytes()))
    }
}

#[cfg(test)]
#[path = "sign_tests.rs"]
mod tests;
