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

//! A managed instance's approvers (plans/Vault-Instances.md §4.4): the signing keys of its environment's approvers,
//! which the instance pins at pairing and uses to check its recovery holders' keys (§4.5). A changed set reaches the
//! instance only signed by an approver of the set it has pinned, over the instance (its id and the fingerprint of its
//! link key, so a set signed for one manager's instance does not replay to another manager's instance of the same id),
//! the new version and every key of the new set, so a manager cannot put a key of its own in the set after pairing
//! (§4.9). Any one pinned approver can replace the whole set.

use crate::sign::{Statement, VerifyingKey};
use crate::Error;

/// One approver: their user id and the Ed25519 key of their personal key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Approver {
    pub user_id: String,
    pub verify_public: [u8; 32],
}

/// An instance's approver set at a version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApproverSet {
    pub version: u64,
    pub approvers: Vec<Approver>,
}

impl ApproverSet {
    /// The set with its approvers in key order, so the signed bytes do not depend on the order they were listed in.
    #[must_use]
    pub fn sorted(mut self) -> Self {
        self.approvers
            .sort_by(|a, b| a.verify_public.cmp(&b.verify_public).then(a.user_id.cmp(&b.user_id)));
        self.approvers.dedup_by(|a, b| a.verify_public == b.verify_public);
        self
    }

    /// Whether `key` is the key of an approver of this set.
    #[must_use]
    pub fn has(&self, key: &[u8]) -> bool {
        self.approvers
            .iter()
            .any(|approver| approver.verify_public.as_slice() == key)
    }

    /// The parts of the statement an approver signs for this set as the set of `instance_id`, whose link key is
    /// `link_key` (its SHA-256 fingerprint, hex).
    fn parts(&self, instance_id: &str, link_key: &str) -> Vec<Vec<u8>> {
        let mut parts = vec![
            instance_id.as_bytes().to_vec(),
            link_key.as_bytes().to_vec(),
            self.version.to_be_bytes().to_vec(),
        ];
        for approver in &self.approvers {
            parts.push(approver.user_id.as_bytes().to_vec());
            parts.push(approver.verify_public.to_vec());
        }
        parts
    }

    /// Checks `self`, a new set for `instance_id` with link key `link_key`, signed by `signer` with `signature`, against the `pinned` set: the
    /// signer is an approver of the pinned set, the version is higher than the pinned one, the set is not empty and its
    /// keys are keys, and the signature is over this set.
    ///
    /// # Errors
    ///
    /// [`Error::Refused`] for a signer not pinned, a version not higher, an empty set; [`Error::Format`] for a key that
    /// is not one; [`Error::Signature`] for a signature that does not verify.
    pub fn verify_update(
        &self,
        instance_id: &str,
        link_key: &str,
        pinned: &ApproverSet,
        signer: &[u8],
        signature: &[u8],
    ) -> Result<(), Error> {
        if !pinned.has(signer) {
            return Err(Error::Refused("the signer is not an approver this instance pinned"));
        }
        if self.version <= pinned.version {
            return Err(Error::Refused(
                "an approver set's version must be higher than the pinned one",
            ));
        }
        if self.approvers.is_empty() {
            return Err(Error::Refused("an approver set has at least one approver"));
        }
        for approver in &self.approvers {
            VerifyingKey::from_bytes(&approver.verify_public)?;
        }
        let set = self.clone().sorted();
        let parts = set.parts(instance_id, link_key);
        let parts: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
        VerifyingKey::from_bytes(signer)?.verify(Statement::ApproverSet, &parts, signature)
    }

    /// The signature of `key` on this set as the set of `instance_id` with link key `link_key`: what an approver's
    /// client makes.
    #[must_use]
    pub fn sign(
        &self,
        instance_id: &str,
        link_key: &str,
        key: &crate::sign::SigningKey,
    ) -> [u8; crate::sign::SIGNATURE_LEN] {
        let set = self.clone().sorted();
        let parts = set.parts(instance_id, link_key);
        let parts: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
        key.sign(Statement::ApproverSet, &parts)
    }
}

#[cfg(test)]
#[path = "approvers_tests.rs"]
mod tests;
