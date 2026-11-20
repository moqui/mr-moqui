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

//! Which key version and member list a client accepts (plans/Vault-Instances.md §3.9 and §9, vault-key rotation).
//!
//! A vault's member list is a chain of signed entries. Each entry is a key version and a revision of its member
//! list: an add is a new revision of the same version, a removal a new version (the key rotates). The first entry is
//! signed by the vault's maker, who is a manager in it. Every later entry is signed by a member who held the manage
//! role in the entry before it, with the key that entry lists for them, so a server or a database writer cannot give a
//! member a member list, or a vault key, of its own choice. A client accepts the chain only when its user has checked
//! (§3.5) the key of at least one signer in it.
//!
//! A client with memory keeps the highest entry it has accepted and the hash of its member list ([`Remembered`],
//! decision (d)): a lower entry is a rollback, and the same entry with another member list is a fork; both are
//! refused.

use sha2::{Digest as _, Sha256};

use crate::names::refs;
use crate::personal::{PersonalKey, PersonalPublic};
use crate::sign::{Statement, VerifyingKey};
use crate::vault::MemberWrap;
use crate::Error;

/// A member's role. A personal shared-key vault has manage, write and read; an operational vault manage, approve
/// and reveal. Every member holds the vault key; the roles are rules on what the server serves and accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Manage,
    Write,
    Read,
    Approve,
    Reveal,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Manage => "manage",
            Role::Write => "write",
            Role::Read => "read",
            Role::Approve => "approve",
            Role::Reveal => "reveal",
        }
    }

    pub fn parse(text: &str) -> Result<Role, Error> {
        match text {
            "manage" => Ok(Role::Manage),
            "write" => Ok(Role::Write),
            "read" => Ok(Role::Read),
            "approve" => Ok(Role::Approve),
            "reveal" => Ok(Role::Reveal),
            _ => Err(Error::Format("unknown member role")),
        }
    }
}

/// One member of an entry: the user, their role, and the personal key the vault key is wrapped to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub user_id: String,
    pub role: Role,
    pub key_id: String,
    pub public: PersonalPublic,
}

/// One signed entry of a vault's member-list chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyVersion {
    pub vault_id: String,
    pub version: u64,
    /// The member list's revision within the version, from 1.
    pub revision: u64,
    pub members: Vec<Member>,
    pub signer_user_id: String,
    pub signer_key_id: String,
    pub signature: Vec<u8>,
}

/// The member list in a canonical order (by user, then key id), so its signed bytes and hash do not depend on how
/// the rows came back.
fn member_list_bytes(members: &[Member]) -> Vec<u8> {
    let mut sorted: Vec<&Member> = members.iter().collect();
    sorted.sort_by(|a, b| (&a.user_id, &a.key_id).cmp(&(&b.user_id, &b.key_id)));
    let mut parts: Vec<Vec<u8>> = vec![(sorted.len() as u64).to_be_bytes().to_vec()];
    for member in sorted {
        parts.push(member.user_id.as_bytes().to_vec());
        parts.push(member.role.as_str().as_bytes().to_vec());
        parts.push(member.key_id.as_bytes().to_vec());
        parts.push(member.public.to_bytes().to_vec());
    }
    crate::aead::associated_data(&refs(&parts))
}

impl KeyVersion {
    /// Makes and signs an entry. The caller is a manager of the entry before (or, for the first, of this one).
    pub fn sign(
        vault_id: &str,
        version: u64,
        revision: u64,
        members: Vec<Member>,
        signer_user_id: &str,
        signer_key_id: &str,
        signer: &PersonalKey,
    ) -> KeyVersion {
        let mut entry = KeyVersion {
            vault_id: vault_id.to_string(),
            version,
            revision,
            members,
            signer_user_id: signer_user_id.to_string(),
            signer_key_id: signer_key_id.to_string(),
            signature: Vec::new(),
        };
        entry.signature = signer
            .signing_key()
            .sign(Statement::KeyVersion, &refs(&entry.signed_parts()))
            .to_vec();
        entry
    }

    fn signed_parts(&self) -> Vec<Vec<u8>> {
        vec![
            self.vault_id.as_bytes().to_vec(),
            self.version.to_be_bytes().to_vec(),
            self.revision.to_be_bytes().to_vec(),
            member_list_bytes(&self.members),
            self.signer_user_id.as_bytes().to_vec(),
            self.signer_key_id.as_bytes().to_vec(),
        ]
    }

    /// SHA-256 of the canonical member list.
    pub fn member_list_hash(&self) -> [u8; 32] {
        Sha256::digest(member_list_bytes(&self.members)).into()
    }

    pub fn member(&self, user_id: &str, key_id: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.user_id == user_id && m.key_id == key_id)
    }

    fn manager(&self, user_id: &str, key_id: &str) -> Option<&Member> {
        self.member(user_id, key_id).filter(|m| m.role == Role::Manage)
    }

    /// Whether `next` may follow this entry: the next revision of this version, or revision 1 of the next version.
    fn followed_by(&self, next: &KeyVersion) -> bool {
        (next.version == self.version && next.revision == self.revision + 1)
            || (next.version == self.version + 1 && next.revision == 1)
    }
}

/// A chain that has been checked: every entry signed by a manager of the entry before.
#[derive(Debug)]
pub struct Chain<'a> {
    entries: &'a [KeyVersion],
}

impl<'a> Chain<'a> {
    /// Checks `entries`, oldest first. `checked` holds the signing keys the user has checked (their own included);
    /// at least one signer of the chain must be among them.
    pub fn verify(entries: &'a [KeyVersion], checked: &[VerifyingKey]) -> Result<Chain<'a>, Error> {
        let [first, ..] = entries else {
            return Err(Error::Refused("no key version"));
        };
        if first.version != 1 || first.revision != 1 {
            return Err(Error::Refused(
                "the chain does not start at the vault's first key version",
            ));
        }
        let mut anchored = false;
        let mut previous: Option<&KeyVersion> = None;
        for entry in entries {
            if entry.vault_id != first.vault_id {
                return Err(Error::Refused("a key version of another vault"));
            }
            // The maker signs the first entry as a manager in it; each later one is signed by a manager of the one
            // before, with the key that entry lists for them.
            let signers_list = previous.unwrap_or(entry);
            if let Some(before) = previous {
                if !before.followed_by(entry) {
                    return Err(Error::Refused("key versions out of order"));
                }
            }
            let Some(signer) = signers_list.manager(&entry.signer_user_id, &entry.signer_key_id) else {
                return Err(Error::Refused("signed by someone who did not hold the manage role"));
            };
            signer
                .public
                .verify
                .verify(Statement::KeyVersion, &refs(&entry.signed_parts()), &entry.signature)?;
            anchored |= checked.contains(&signer.public.verify);
            previous = Some(entry);
        }
        if !anchored {
            return Err(Error::Refused("no signer's key in this chain has been checked"));
        }
        Ok(Chain { entries })
    }

    pub fn latest(&self) -> &'a KeyVersion {
        self.entries.last().expect("a verified chain has at least one entry")
    }

    /// The last entry of `version`.
    fn last_of(&self, version: u64) -> Option<&'a KeyVersion> {
        self.entries.iter().rev().find(|entry| entry.version == version)
    }

    /// Accepts a member's wrap of `wrap.key_version`: its signer must be a manager in that version's last entry,
    /// and its member must be in it, with the key it is wrapped to. Returns the signer's key to open it with.
    pub fn accept_wrap(&self, wrap: &MemberWrap) -> Result<(&'a Member, VerifyingKey), Error> {
        if wrap.vault_id != self.latest().vault_id {
            return Err(Error::Refused("a wrap of another vault"));
        }
        let Some(entry) = self.last_of(wrap.key_version) else {
            return Err(Error::Refused("a wrap of a key version not in the chain"));
        };
        let Some(signer) = entry.manager(&wrap.signer_user_id, &wrap.signer_key_id) else {
            return Err(Error::Refused("the wrap's signer did not hold the manage role"));
        };
        let Some(member) = entry.member(&wrap.member_user_id, &wrap.member_key_id) else {
            return Err(Error::Refused("the wrap is for someone not in the member list"));
        };
        wrap.verify(&member.public, &signer.public.verify)?;
        Ok((member, signer.public.verify))
    }
}

/// What a client with memory keeps of a vault: the highest entry it accepted and the hash of its member list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Remembered {
    pub version: u64,
    pub revision: u64,
    pub member_list_hash: [u8; 32],
}

/// Checks the latest entry against what the client remembers, and gives what to remember now.
pub fn check_remembered(latest: &KeyVersion, remembered: Option<&Remembered>) -> Result<Remembered, Error> {
    let now = Remembered {
        version: latest.version,
        revision: latest.revision,
        member_list_hash: latest.member_list_hash(),
    };
    if let Some(before) = remembered {
        if (now.version, now.revision) < (before.version, before.revision) {
            return Err(Error::Refused("an older key version than one already seen"));
        }
        if (now.version, now.revision) == (before.version, before.revision)
            && now.member_list_hash != before.member_list_hash
        {
            return Err(Error::Refused("the member list changed without a new revision"));
        }
    }
    Ok(now)
}

#[cfg(test)]
#[path = "keyversion_tests.rs"]
mod tests;
