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

//! What a sealed value is, written into its associated data or its HPKE `info` (plans/Vault-Instances.md §3.2): the
//! purpose, the vault id, the item id and the key version, and for an operational item its type, binding and clear
//! name. A value copied to another row, vault, version or purpose, or an operational item whose clear columns were
//! changed, does not open.

/// The format's name, the first part of every associated data and `info`.
pub const DOMAIN: &[u8] = b"moqui-vault/1";

/// What a sealed value is for. Each has its own name, so a value sealed for one purpose never opens as another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// A vault key, wrapped by a password, a generated key or a member's personal key.
    VaultKey,
    /// An item key wrapped by the vault key.
    ItemKey,
    /// An item body under its item key.
    Body,
    /// The deposit key's private half wrapped by the vault key.
    DepositKey,
    /// An item key wrapped to the deposit key's public half.
    DepositItemKey,
    /// A personal key's private halves wrapped by the key passphrase.
    PersonalPassphrase,
    /// A personal key's private halves wrapped by the recovery code.
    PersonalRecoveryCode,
    /// A vault key wrapped to the organization recovery key.
    RecoveryKey,
    /// A recovery-key share wrapped to a holder's personal key.
    RecoveryShare,
    /// A delivered value wrapped to its recipient's personal key.
    Delivery,
    /// An item key sealed to a lease's public key.
    Lease,
    /// An item key sealed under the machine key for an environment.
    Machine,
}

impl Purpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Purpose::VaultKey => "vault-key",
            Purpose::ItemKey => "item-key",
            Purpose::Body => "body",
            Purpose::DepositKey => "deposit-key",
            Purpose::DepositItemKey => "deposit-item-key",
            Purpose::PersonalPassphrase => "personal-passphrase",
            Purpose::PersonalRecoveryCode => "personal-recovery-code",
            Purpose::RecoveryKey => "recovery-key",
            Purpose::RecoveryShare => "recovery-share",
            Purpose::Delivery => "delivery",
            Purpose::Lease => "lease",
            Purpose::Machine => "machine",
        }
    }
}

/// An operational item's clear columns: its type, its binding (`AppInstance:<id>:dbPassword`) and its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemLabel<'a> {
    pub item_type: &'a str,
    pub binding: &'a str,
    pub name: &'a str,
}

/// The names of one sealed value. `item_id` is empty for a value that is not an item's (a vault key).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Names<'a> {
    pub purpose: Purpose,
    pub vault_id: &'a str,
    pub item_id: &'a str,
    pub key_version: u64,
    /// An operational item's clear columns; `None` for a personal item, whose type and name are inside the body.
    pub label: Option<ItemLabel<'a>>,
}

impl<'a> Names<'a> {
    pub fn new(purpose: Purpose, vault_id: &'a str, item_id: &'a str, key_version: u64) -> Self {
        Names {
            purpose,
            vault_id,
            item_id,
            key_version,
            label: None,
        }
    }

    pub fn with_label(self, label: Option<ItemLabel<'a>>) -> Self {
        Names { label, ..self }
    }

    /// The parts, in order, with `extra` after them: the AEAD associated data or, encoded, the HPKE `info`.
    pub fn parts(&self, extra: &[&[u8]]) -> Vec<Vec<u8>> {
        let mut parts = vec![
            DOMAIN.to_vec(),
            self.purpose.as_str().as_bytes().to_vec(),
            self.vault_id.as_bytes().to_vec(),
            self.item_id.as_bytes().to_vec(),
            self.key_version.to_be_bytes().to_vec(),
        ];
        if let Some(label) = self.label {
            parts.push(b"operational".to_vec());
            parts.push(label.item_type.as_bytes().to_vec());
            parts.push(label.binding.as_bytes().to_vec());
            parts.push(label.name.as_bytes().to_vec());
        }
        parts.extend(extra.iter().map(|part| part.to_vec()));
        parts
    }

    /// The parts as one length-prefixed byte string.
    pub fn encode(&self, extra: &[&[u8]]) -> Vec<u8> {
        let parts = self.parts(extra);
        crate::aead::associated_data(&refs(&parts))
    }
}

/// Borrowed views of owned parts, for [`crate::aead::seal`] and friends.
pub(crate) fn refs(parts: &[Vec<u8>]) -> Vec<&[u8]> {
    parts.iter().map(Vec::as_slice).collect()
}
