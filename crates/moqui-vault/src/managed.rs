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

//! A managed instance's sealed start (plans/Vault-Instances.md §4.4, §4.5, M6): what the instance, its manager and its
//! recovery holders' clients each check, with no I/O, so the three use one implementation.
//!
//! - **The link key** is an Ed25519 key; the manager pins it by the SHA-256 of its SubjectPublicKeyInfo
//!   ([`link_fingerprint`]), the fingerprint a TLS handshake shows, so a holder's client checks a key it is handed
//!   against that pin before it trusts a signature by it.
//! - **Transit** ([`wrap_root`], [`open_root`], [`seal_reply`], [`open_reply`]): the instance's root key wrapped to its
//!   transit key (HPKE, the instance id as associated data), and the manager's answer sealed to the ephemeral key the
//!   instance made for that one unseal, with the instance id and that key as associated data.
//! - **The unsealed proof** ([`proof_parts`]): the instance signs one link's instance id, link key, generation and the
//!   manager's challenge with a key derived from its root key, which a holder of the link key alone does not have.
//! - **Recovery shares**: each share goes to one holder as a delivery signed with the link key ([`ShareValue`] is what
//!   it carries); the holder confirms it with their signing key ([`confirm_share`]). A sealed instance makes a key for
//!   one unseal and signs it with its link key ([`UnsealKey`]); a holder seals their share to it ([`seal_share`]), and
//!   the manager relays only that ciphertext.

use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

use crate::hpke::{self, PublicKey, SecretKey};
use crate::rng::Rng;
use crate::sign::{SigningKey, Statement, VerifyingKey, SIGNATURE_LEN};
use crate::text::hex;
use crate::Error;

/// The DER head of an Ed25519 SubjectPublicKeyInfo (RFC 8410): the 32-byte key follows.
const ED25519_SPKI_HEAD: [u8; 12] = [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];

const TRANSIT_INFO: &[u8] = b"moqui-manage transit/1";
const TRANSIT_REPLY_INFO: &[u8] = b"moqui-manage transit-reply/1";
const UNSEAL_SHARE_INFO: &[u8] = b"moqui-manage unseal-share/1";

/// The purpose of a recovery share's delivery.
pub const SHARE_PURPOSE: &str = "recovery-share";

/// SHA-256 of an Ed25519 key's SubjectPublicKeyInfo, hex: what a manager pins of an instance's link key.
#[must_use]
pub fn link_fingerprint(public: &[u8; 32]) -> String {
    let mut digest = Sha256::new();
    digest.update(ED25519_SPKI_HEAD);
    digest.update(public);
    hex(&digest.finalize())
}

/// The link key `public` when it is the one pinned as `fingerprint`.
///
/// # Errors
///
/// Not an Ed25519 key, or not the pinned one.
pub fn pinned_link_key(public: &[u8], fingerprint: &str) -> Result<VerifyingKey, Error> {
    let key = VerifyingKey::from_bytes(public)?;
    if fingerprint.is_empty() || !link_fingerprint(key.as_bytes()).eq_ignore_ascii_case(fingerprint.trim()) {
        return Err(Error::Refused("the link key is not the one pinned for the instance"));
    }
    Ok(key)
}

/// The instance's root key wrapped to its transit key, for `instance_id`.
///
/// # Errors
///
/// Not a public key.
pub fn wrap_root(transit: &PublicKey, instance_id: &str, root: &[u8; 32], rng: &mut dyn Rng) -> Result<Vec<u8>, Error> {
    hpke::seal_raw(transit, TRANSIT_INFO, instance_id.as_bytes(), root, rng)
}

/// The root key [`wrap_root`] wrapped, on the manager with that instance's transit key.
///
/// # Errors
///
/// Another instance's wrap, another transit key's, changed bytes, or not a 32-byte key.
pub fn open_root(transit: &SecretKey, instance_id: &str, wrapped: &[u8]) -> Result<Zeroizing<[u8; 32]>, Error> {
    let opened = hpke::open_raw(transit, TRANSIT_INFO, instance_id.as_bytes(), wrapped)?;
    let mut root = Zeroizing::new([0_u8; 32]);
    if opened.len() != 32 {
        return Err(Error::Format("a root key is 32 bytes"));
    }
    root.copy_from_slice(&opened);
    Ok(root)
}

fn reply_aad(instance_id: &str, ephemeral: &[u8; 32]) -> Vec<u8> {
    crate::aead::associated_data(&[instance_id.as_bytes(), &ephemeral[..]])
}

/// The manager's answer: the root key sealed to the ephemeral key the instance made for this unseal.
///
/// # Errors
///
/// Not a public key.
pub fn seal_reply(
    ephemeral: &PublicKey,
    instance_id: &str,
    root: &[u8; 32],
    rng: &mut dyn Rng,
) -> Result<Vec<u8>, Error> {
    hpke::seal_raw(
        ephemeral,
        TRANSIT_REPLY_INFO,
        &reply_aad(instance_id, ephemeral.as_bytes()),
        root,
        rng,
    )
}

/// The root key in the manager's answer, with the ephemeral key's secret half.
///
/// # Errors
///
/// An answer for another unseal or instance, or changed bytes.
pub fn open_reply(ephemeral: &SecretKey, instance_id: &str, sealed: &[u8]) -> Result<Zeroizing<[u8; 32]>, Error> {
    let public = ephemeral.public()?;
    let opened = hpke::open_raw(
        ephemeral,
        TRANSIT_REPLY_INFO,
        &reply_aad(instance_id, public.as_bytes()),
        sealed,
    )?;
    let mut root = Zeroizing::new([0_u8; 32]);
    if opened.len() != 32 {
        return Err(Error::Format("a root key is 32 bytes"));
    }
    root.copy_from_slice(&opened);
    Ok(root)
}

/// What an unsealed proof signs: one link of an instance.
#[derive(Clone, Copy, Debug)]
pub struct ProofLink<'a> {
    pub instance_id: &'a str,
    pub link_fingerprint: &'a str,
    pub generation: u64,
    pub challenge: &'a [u8],
}

fn proof_parts(link: &ProofLink<'_>) -> [Vec<u8>; 4] {
    [
        link.instance_id.as_bytes().to_vec(),
        link.link_fingerprint.as_bytes().to_vec(),
        link.generation.to_be_bytes().to_vec(),
        link.challenge.to_vec(),
    ]
}

/// The instance's proof on `link`, by the key derived from its root key.
#[must_use]
pub fn sign_proof(key: &SigningKey, link: &ProofLink<'_>) -> [u8; SIGNATURE_LEN] {
    let parts = proof_parts(link);
    key.sign(Statement::UnsealedProof, &crate::names::refs(&parts))
}

/// The manager's check of a proof on `link` against the key it pinned.
///
/// # Errors
///
/// A wrong or missing signature, or one for another link.
pub fn verify_proof(key: &VerifyingKey, link: &ProofLink<'_>, signature: &[u8]) -> Result<(), Error> {
    let parts = proof_parts(link);
    key.verify(Statement::UnsealedProof, &crate::names::refs(&parts), signature)
}

/// A key a sealed instance made for one unseal by recovery shares, signed with its link key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsealKey {
    pub instance_id: String,
    pub unseal_public: [u8; 32],
    pub signature: Vec<u8>,
}

impl UnsealKey {
    fn parts(instance_id: &str, unseal_public: &[u8; 32]) -> [Vec<u8>; 2] {
        [instance_id.as_bytes().to_vec(), unseal_public.to_vec()]
    }

    /// Signed with the instance's link key.
    #[must_use]
    pub fn sign(link_key: &SigningKey, instance_id: &str, unseal_public: &[u8; 32]) -> Self {
        let signature = link_key
            .sign(
                Statement::UnsealKey,
                &crate::names::refs(&Self::parts(instance_id, unseal_public)),
            )
            .to_vec();
        Self {
            instance_id: instance_id.to_string(),
            unseal_public: *unseal_public,
            signature,
        }
    }

    /// A holder's check: signed by the link key pinned as `link_fingerprint`.
    ///
    /// # Errors
    ///
    /// The link key is not the pinned one, or the signature is wrong.
    pub fn verify(&self, link_public: &[u8], link_fingerprint: &str) -> Result<(), Error> {
        let link = pinned_link_key(link_public, link_fingerprint)?;
        link.verify(
            Statement::UnsealKey,
            &crate::names::refs(&Self::parts(&self.instance_id, &self.unseal_public)),
            &self.signature,
        )
    }
}

fn share_aad(instance_id: &str, unseal_public: &[u8; 32]) -> Vec<u8> {
    crate::aead::associated_data(&[instance_id.as_bytes(), &unseal_public[..]])
}

/// A holder's share sealed to one unseal's key: only that unseal's secret half opens it.
///
/// # Errors
///
/// Not a public key.
pub fn seal_share(key: &UnsealKey, share: &str, rng: &mut dyn Rng) -> Result<Vec<u8>, Error> {
    let public = PublicKey::from_bytes(&key.unseal_public)?;
    hpke::seal_raw(
        &public,
        UNSEAL_SHARE_INFO,
        &share_aad(&key.instance_id, &key.unseal_public),
        share.as_bytes(),
        rng,
    )
}

/// The share sealed to this unseal's key.
///
/// # Errors
///
/// A share sealed for another unseal or instance, or changed bytes.
pub fn open_share(unseal: &SecretKey, instance_id: &str, sealed: &[u8]) -> Result<Zeroizing<String>, Error> {
    let public = unseal.public()?;
    let opened = hpke::open_raw(
        unseal,
        UNSEAL_SHARE_INFO,
        &share_aad(instance_id, public.as_bytes()),
        sealed,
    )?;
    let text = std::str::from_utf8(&opened).map_err(|_| Error::Format("a share is text"))?;
    Ok(Zeroizing::new(text.to_string()))
}

/// What a recovery share's delivery carries: the instance, its share set, the share's index of how many, the
/// threshold, and the share.
#[derive(Clone, PartialEq, Eq)]
pub struct ShareValue {
    pub instance_id: String,
    pub instance_name: String,
    pub set_id: String,
    pub index: u32,
    pub of: u32,
    pub threshold: u32,
    pub share: Zeroizing<String>,
}

impl std::fmt::Debug for ShareValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShareValue")
            .field("instance_id", &self.instance_id)
            .field("set_id", &self.set_id)
            .field("index", &self.index)
            .field("of", &self.of)
            .finish_non_exhaustive()
    }
}

impl ShareValue {
    /// The delivery's value.
    #[must_use]
    pub fn to_text(&self) -> Zeroizing<String> {
        Zeroizing::new(
            json!({
                "instance": self.instance_id, "instanceName": self.instance_name, "setId": self.set_id,
                "index": self.index, "of": self.of, "threshold": self.threshold, "share": self.share.as_str(),
            })
            .to_string(),
        )
    }

    /// A delivery's value read back.
    ///
    /// # Errors
    ///
    /// Not a share's value.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let mut value: Value = serde_json::from_str(text).map_err(|_| Error::Format("a share's value is JSON"))?;
        // the share out of the parsed tree into a zeroized string, so no other copy of it is left behind
        let share = match value.get_mut("share").map(Value::take) {
            Some(Value::String(share)) => Some(Zeroizing::new(share)),
            _ => None,
        };
        let field = |name: &str| value[name].as_str().map(str::to_string);
        let number = |name: &str| value[name].as_u64().and_then(|n| u32::try_from(n).ok());
        match (
            field("instance"),
            field("setId"),
            number("index"),
            number("of"),
            number("threshold"),
            share,
        ) {
            (Some(instance_id), Some(set_id), Some(index), Some(of), Some(threshold), Some(share)) => Ok(Self {
                instance_id,
                instance_name: field("instanceName").unwrap_or_default(),
                set_id,
                index,
                of,
                threshold,
                share,
            }),
            _ => Err(Error::Format("not a recovery share's value")),
        }
    }

    /// The name a holder files it under: "Recovery share 2 of 5, prod-1".
    #[must_use]
    pub fn item_name(&self) -> String {
        let name = if self.instance_name.is_empty() {
            &self.instance_id
        } else {
            &self.instance_name
        };
        format!("Recovery share {} of {}, {name}", self.index, self.of)
    }
}

fn confirm_parts(instance_id: &str, set_id: &str, index: u32, share: &str) -> [Vec<u8>; 4] {
    [
        instance_id.as_bytes().to_vec(),
        set_id.as_bytes().to_vec(),
        index.to_be_bytes().to_vec(),
        share_digest(share).to_vec(),
    ]
}

/// SHA-256 of a share's text: what the instance keeps of a share it sent, to check its confirmation.
#[must_use]
pub fn share_digest(share: &str) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"moqui-manage share/1\0");
    digest.update(share.trim().as_bytes());
    let mut out = [0_u8; 32];
    out.copy_from_slice(&digest.finalize());
    out
}

/// A holder's confirmation that they filed `value`, by their signing key.
#[must_use]
pub fn confirm_share(key: &SigningKey, value: &ShareValue) -> [u8; SIGNATURE_LEN] {
    let parts = confirm_parts(&value.instance_id, &value.set_id, value.index, &value.share);
    key.sign(Statement::ShareConfirm, &crate::names::refs(&parts))
}

/// The instance's check of a confirmation: by the holder's key, for the share whose digest it kept.
///
/// # Errors
///
/// A wrong signature, or one for another share.
pub fn verify_confirmation(
    holder: &VerifyingKey,
    instance_id: &str,
    set_id: &str,
    index: u32,
    digest: &[u8; 32],
    signature: &[u8],
) -> Result<(), Error> {
    let parts = [
        instance_id.as_bytes().to_vec(),
        set_id.as_bytes().to_vec(),
        index.to_be_bytes().to_vec(),
        digest.to_vec(),
    ];
    holder.verify(Statement::ShareConfirm, &crate::names::refs(&parts), signature)
}

#[cfg(test)]
#[path = "managed_tests.rs"]
mod tests;
