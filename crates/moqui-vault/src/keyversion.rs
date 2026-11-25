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

//! Which key version and member list a client accepts (plans/archive/Vault-Instances.md §3.9 and §9, vault-key rotation).
//!
//! A vault's member list is a chain of signed entries. Each entry is a key version and a revision of its member
//! list: an add is a new revision of the same version, a removal a new version (the key rotates). The first entry is
//! signed by the vault's maker, who is a manager in it. Every later entry is signed by a member who held the manage
//! role in the entry before it, with the key that entry lists for them, so a server or a database writer cannot give a
//! member a member list, or a vault key, of its own choice. A client accepts the chain only when its user has checked
//! (§3.5) the key of at least one signer in it, and trusts it only from the first such entry on ([`Chain`]).
//!
//! A client with memory keeps the highest entry it has accepted and the hash of its member list ([`Remembered`],
//! decision (d)): a chain that does not hold that entry with that member list is a rollback or a fork, and is
//! refused however high it goes.
//!
//! A vault with recovery on lists the organization's recovery key as a member with the manage role
//! ([`recovery_member_id`], §3.10): the vault key is wrapped to it as to any member, and a rotation wraps to it again.
//! An entry it signs (a recovery, made by k holders' clients) may only give one member a new personal key, with
//! nothing else changed ([`Chain::verify`]), so a recovery cannot add a member or change a role.

use sha2::{Digest as _, Sha256};

use crate::names::refs;
use crate::personal::{PersonalKey, PersonalPublic};
use crate::sign::{Statement, VerifyingKey};
use crate::vault::{DepositPublic, MemberWrap};
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

/// The user id a recovery key is listed under in a member list: `recovery:` and the recovery key's id. No signed-in
/// user has it; the server keeps it for recovery keys.
pub fn recovery_member_id(recovery_key_id: &str) -> String {
    format!("{RECOVERY_MEMBER_PREFIX}{recovery_key_id}")
}

const RECOVERY_MEMBER_PREFIX: &str = "recovery:";

/// The recovery key id of a member list's recovery member, or none for a person.
pub fn recovery_key_of(user_id: &str) -> Option<&str> {
    user_id.strip_prefix(RECOVERY_MEMBER_PREFIX)
}

/// Whether `entry`, signed by a recovery member, only gives one person in `before` a new personal key: the same set of
/// users on both sides, each once, with the same roles, and exactly one of them (not a recovery member) under another
/// key, so their old key is gone.
fn recovery_change_only(before: &KeyVersion, entry: &KeyVersion) -> bool {
    if entry.version != before.version || entry.members.len() != before.members.len() {
        return false;
    }
    let users = |list: &KeyVersion| -> std::collections::BTreeSet<String> {
        list.members.iter().map(|m| m.user_id.clone()).collect()
    };
    let (was_users, now_users) = (users(before), users(entry));
    if was_users.len() != before.members.len() || now_users != was_users {
        return false;
    }
    let mut changed = 0;
    for member in &entry.members {
        let Some(was) = before.members.iter().find(|m| m.user_id == member.user_id) else {
            return false;
        };
        if was.role != member.role {
            return false;
        }
        if was.key_id != member.key_id || was.public != member.public {
            if recovery_key_of(&member.user_id).is_some() {
                return false;
            }
            changed += 1;
        }
    }
    changed == 1
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
    /// Makes and signs an entry. The caller is a manager of the entry before (or, for the first, of this one);
    /// [`Chain::next`] checks that for a client. A member list that names one personal key of a user twice is refused.
    pub fn sign(
        vault_id: &str,
        version: u64,
        revision: u64,
        members: Vec<Member>,
        signer_user_id: &str,
        signer_key_id: &str,
        signer: &PersonalKey,
    ) -> Result<KeyVersion, Error> {
        check_members(&members)?;
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
        Ok(entry)
    }

    /// The vault's first entry (1.1), signed by its maker, who must be a manager in it with this key.
    pub fn first(
        vault_id: &str,
        members: Vec<Member>,
        signer_user_id: &str,
        signer_key_id: &str,
        signer: &PersonalKey,
    ) -> Result<KeyVersion, Error> {
        let entry = KeyVersion::sign(vault_id, 1, 1, members, signer_user_id, signer_key_id, signer)?;
        match entry.manager(signer_user_id, signer_key_id) {
            Some(maker) if maker.public.verify == signer.public().verify => Ok(entry),
            _ => Err(Error::Refused(
                "the maker must be a manager in the first member list, with this key",
            )),
        }
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

/// Refuses a member list that names a user twice (with one personal key or two: a member has one key in a list).
fn check_members(members: &[Member]) -> Result<(), Error> {
    for (at, member) in members.iter().enumerate() {
        if members[..at].iter().any(|m| m.user_id == member.user_id) {
            return Err(Error::Refused("a member list names one user twice"));
        }
    }
    Ok(())
}

/// A chain that has been checked: every entry signed by a manager of the entry before.
///
/// The entries before the first one whose signer the user checked (the anchor) cannot be told from a forged history:
/// their signers were placed only by the entries before them. Entries from the anchor on are signed by a checked key
/// or by a manager its entries name, so only they are trusted: a wrap or a deposit key is accepted only for a version
/// whose last entry is at or after the anchor.
#[derive(Debug)]
pub struct Chain<'a> {
    entries: &'a [KeyVersion],
    anchor: usize,
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
        let mut anchor = None;
        let mut previous: Option<&KeyVersion> = None;
        for (at, entry) in entries.iter().enumerate() {
            if entry.vault_id != first.vault_id {
                return Err(Error::Refused("a key version of another vault"));
            }
            check_members(&entry.members)?;
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
            if recovery_key_of(&entry.signer_user_id).is_some()
                && !previous.is_some_and(|before| recovery_change_only(before, entry))
            {
                return Err(Error::Refused(
                    "a recovery only gives one member a new personal key, with nothing else changed",
                ));
            }
            signer
                .public
                .verify
                .verify(Statement::KeyVersion, &refs(&entry.signed_parts()), &entry.signature)?;
            if anchor.is_none() && checked.contains(&signer.public.verify) {
                anchor = Some(at);
            }
            previous = Some(entry);
        }
        let Some(anchor) = anchor else {
            return Err(Error::Refused("no signer's key in this chain has been checked"));
        };
        Ok(Chain { entries, anchor })
    }

    pub fn latest(&self) -> &'a KeyVersion {
        self.entries.last().expect("a verified chain has at least one entry")
    }

    /// The last entry of `version`, when it is at or after the anchor.
    fn trusted_last_of(&self, version: u64) -> Result<&'a KeyVersion, Error> {
        let Some(at) = self.entries.iter().rposition(|entry| entry.version == version) else {
            return Err(Error::Refused("a key version not in the chain"));
        };
        if at < self.anchor {
            return Err(Error::Refused("a key version from before the first checked signer"));
        }
        Ok(&self.entries[at])
    }

    /// Accepts a member's wrap of `wrap.key_version`: its signer must be a manager in that version's last entry,
    /// and its member must be in it, with the key it is wrapped to. Returns the signer's key to open it with.
    pub fn accept_wrap(&self, wrap: &MemberWrap) -> Result<(&'a Member, VerifyingKey), Error> {
        if wrap.vault_id != self.latest().vault_id {
            return Err(Error::Refused("a wrap of another vault"));
        }
        let entry = self.trusted_last_of(wrap.key_version)?;
        let Some(signer) = entry.manager(&wrap.signer_user_id, &wrap.signer_key_id) else {
            return Err(Error::Refused("the wrap's signer did not hold the manage role"));
        };
        let Some(member) = entry.member(&wrap.member_user_id, &wrap.member_key_id) else {
            return Err(Error::Refused("the wrap is for someone not in the member list"));
        };
        wrap.verify(&member.public, &signer.public.verify)?;
        Ok((member, signer.public.verify))
    }

    /// Accepts an operational vault's deposit key: of the current key version, signed by a manager of it (a person,
    /// never a recovery key). Returns the signer's key, which [`crate::vault::wrap_item_key_to_deposit`] checks again.
    ///
    /// A manager whom a recovery gave a new key (§3.10) still vouches for the deposit key they signed with the key the
    /// recovery replaced: the signer, with that key, was a manager in a trusted entry of this version, is still a
    /// manager in its last entry, and every change of their key since was a recovery's.
    pub fn accept_deposit(&self, deposit: &DepositPublic) -> Result<VerifyingKey, Error> {
        let latest = self.latest();
        if deposit.vault_id != latest.vault_id {
            return Err(Error::Refused("a deposit key of another vault"));
        }
        if deposit.key_version != latest.version {
            return Err(Error::Refused("a deposit key of an earlier key version"));
        }
        if recovery_key_of(&deposit.signer_user_id).is_some() {
            return Err(Error::Refused("a deposit key is signed by a person"));
        }
        let entry = self.trusted_last_of(deposit.key_version)?;
        if let Some(signer) = entry.manager(&deposit.signer_user_id, &deposit.signer_key_id) {
            deposit.verify(&signer.public.verify)?;
            return Ok(signer.public.verify);
        }
        let still_manages = entry
            .members
            .iter()
            .any(|m| m.user_id == deposit.signer_user_id && m.role == Role::Manage);
        let last = self.entries.len() - 1;
        let signed = (self.anchor..=last).rev().find_map(|at| {
            let listed = &self.entries[at];
            (listed.version == deposit.key_version)
                .then(|| listed.manager(&deposit.signer_user_id, &deposit.signer_key_id))
                .flatten()
                .map(|m| (at, m.public.verify))
        });
        let (true, Some((signed_at, verify))) = (still_manages, signed) else {
            return Err(Error::Refused("the deposit key's signer did not hold the manage role"));
        };
        let key_of = |listed: &KeyVersion| {
            listed
                .members
                .iter()
                .find(|m| m.user_id == deposit.signer_user_id)
                .map(|m| (m.key_id.clone(), m.public))
        };
        for pair in self.entries[signed_at..=last].windows(2) {
            let [before, after] = pair else {
                continue;
            };
            if key_of(before) != key_of(after) && recovery_key_of(&after.signer_user_id).is_none() {
                return Err(Error::Refused(
                    "the deposit key's signer has another key now: a manager signs it again",
                ));
            }
        }
        deposit.verify(&verify)?;
        Ok(verify)
    }

    /// Signs the entry after the latest: `signer` must be a manager in the latest entry, with this key.
    pub fn next(
        &self,
        version: u64,
        revision: u64,
        members: Vec<Member>,
        signer_user_id: &str,
        signer_key_id: &str,
        signer: &PersonalKey,
    ) -> Result<KeyVersion, Error> {
        let latest = self.latest();
        match latest.manager(signer_user_id, signer_key_id) {
            Some(listed) if listed.public.verify == signer.public().verify => {}
            _ => {
                return Err(Error::Refused(
                    "only a manager of the latest member list signs the next",
                ))
            }
        }
        let entry = KeyVersion::sign(
            &latest.vault_id,
            version,
            revision,
            members,
            signer_user_id,
            signer_key_id,
            signer,
        )?;
        if !latest.followed_by(&entry) {
            return Err(Error::Refused("the next entry must follow the latest"));
        }
        Ok(entry)
    }
}

/// What a client with memory keeps of a vault: the highest entry it accepted and the hash of its member list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Remembered {
    pub version: u64,
    pub revision: u64,
    pub member_list_hash: [u8; 32],
}

/// Checks a verified chain against what the client remembers, and gives what to remember now. The chain must hold
/// the remembered entry, with the remembered member list: a lower latest entry is a rollback, and a chain that went
/// another way from an earlier entry (a fork, however high it goes) does not hold it.
pub fn check_remembered(chain: &Chain, remembered: Option<&Remembered>) -> Result<Remembered, Error> {
    let latest = chain.latest();
    let now = Remembered {
        version: latest.version,
        revision: latest.revision,
        member_list_hash: latest.member_list_hash(),
    };
    if let Some(before) = remembered {
        if (now.version, now.revision) < (before.version, before.revision) {
            return Err(Error::Refused("an older key version than one already seen"));
        }
        let held = chain
            .entries
            .iter()
            .find(|entry| (entry.version, entry.revision) == (before.version, before.revision));
        match held {
            Some(entry) if entry.member_list_hash() == before.member_list_hash => {}
            _ => return Err(Error::Refused("the member list does not continue the one already seen")),
        }
    }
    Ok(now)
}

#[cfg(test)]
#[path = "keyversion_tests.rs"]
mod tests;
