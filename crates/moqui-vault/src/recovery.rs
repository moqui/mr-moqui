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

//! The organization recovery key (plans/Vault-Instances.md §3.10, split k of n by the owner's decision of Nov 19,
//! 2026).
//!
//! - A recovery key is a key pair of the personal key's shape: X25519 to wrap to and Ed25519 to sign. Both halves come
//!   from one 32-byte seed by HKDF, and the seed is split into n Shamir shares, each HPKE-wrapped to one holder's
//!   personal key and signed by the maker, then dropped: no place holds the whole key. Its public halves are signed by
//!   the holder who made it ([`RecoveryKeyRecord`]); a client refuses one whose maker's key it has not checked.
//! - A vault with recovery on lists the recovery key in its member list as a manager
//!   ([`crate::keyversion::recovery_member_id`]): the vault key is wrapped to it as to a member, by a manager who
//!   checked the recovery key's maker, and every rotation wraps to it again.
//! - A recovery takes k holders' clients. The recovering holder's client makes an ephemeral key and signs a
//!   [`RecoverySession`] naming it, the vault, and the member's new personal key; each other holder's client checks
//!   that, opens its share and seals it to the ephemeral key ([`SessionShare`]); the recovering client combines the
//!   shares with its own, checks the result against the signed public halves, opens the vault key through the vault's
//!   chain and, as the recovery member, signs the next member list with the member's new key and wraps the vault key
//!   to it. The shares, the seed and the recovery key never leave the module.

use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

use crate::hpke;
use crate::kdf;
use crate::keys::Key32;
use crate::keyversion::{recovery_member_id, Member, Role};
use crate::names::{refs, Names, Purpose};
use crate::personal::{PersonalKey, PersonalPublic};
use crate::rng::Rng;
use crate::shamir::{self, Share};
use crate::sign::{Statement, VerifyingKey};
use crate::vault::{Recipient, Signer};
use crate::Error;

const ENCRYPT_INFO: &[u8] = b"moqui-vault recovery key encrypt";
const SIGN_INFO: &[u8] = b"moqui-vault recovery key sign";

/// The recovery key from its seed, the secret the shares split.
fn key_from_seed(seed: &[u8]) -> PersonalKey {
    let encrypt = kdf::hkdf_sha256(seed, b"", ENCRYPT_INFO);
    let sign = kdf::hkdf_sha256(seed, b"", SIGN_INFO);
    PersonalKey::from_secrets(&encrypt, &sign)
}

/// A recovery key's public record (`VaultRecoveryKey`), signed by its maker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryKeyRecord {
    pub recovery_key_id: String,
    pub public: PersonalPublic,
    pub k: u8,
    pub n: u8,
    pub maker_user_id: String,
    pub maker_key_id: String,
    pub signature: Vec<u8>,
}

impl RecoveryKeyRecord {
    fn signed_parts(&self) -> Vec<Vec<u8>> {
        vec![
            self.recovery_key_id.as_bytes().to_vec(),
            self.public.to_bytes().to_vec(),
            vec![self.k, self.n],
            self.maker_user_id.as_bytes().to_vec(),
            self.maker_key_id.as_bytes().to_vec(),
        ]
    }

    /// A client refuses a recovery key with no signature it can check, so a server cannot hand out its own.
    pub fn verify(&self, maker: &VerifyingKey) -> Result<(), Error> {
        maker.verify(Statement::RecoveryKey, &refs(&self.signed_parts()), &self.signature)
    }

    /// How the recovery key is listed in a vault's member list: a manager, under `recovery:<id>`.
    pub fn member(&self) -> Member {
        Member {
            user_id: recovery_member_id(&self.recovery_key_id),
            role: Role::Manage,
            key_id: self.recovery_key_id.clone(),
            public: self.public,
        }
    }
}

/// One holder's share (`VaultRecoveryHolder`): wrapped to the holder's personal key, signed by the maker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HolderShare {
    pub recovery_key_id: String,
    pub holder_user_id: String,
    pub holder_key_id: String,
    pub index: u8,
    pub sealed: Vec<u8>,
    pub signature: Vec<u8>,
}

impl HolderShare {
    fn names(&self) -> Names<'_> {
        Names::new(Purpose::RecoveryShare, &self.recovery_key_id, &self.holder_user_id, 0)
    }

    fn signed_parts(&self, holder: &PersonalPublic) -> Vec<Vec<u8>> {
        vec![
            self.recovery_key_id.as_bytes().to_vec(),
            self.holder_user_id.as_bytes().to_vec(),
            self.holder_key_id.as_bytes().to_vec(),
            holder.to_bytes().to_vec(),
            vec![self.index],
            Sha256::digest(&self.sealed).to_vec(),
        ]
    }

    /// Checks the maker's signature over the share and the holder's key: `holder` is a key the maker gave a share to.
    pub fn verify(&self, holder: &PersonalPublic, maker: &VerifyingKey) -> Result<(), Error> {
        maker.verify(
            Statement::RecoveryShare,
            &refs(&self.signed_parts(holder)),
            &self.signature,
        )
    }
}

/// A holder the recovery key is split to.
#[derive(Clone, Copy, Debug)]
pub struct Holder<'a> {
    pub user_id: &'a str,
    pub key_id: &'a str,
    pub public: &'a PersonalPublic,
}

/// Makes a recovery key split `k` of the holders' count, one share to each holder. The seed is dropped here.
pub fn make(
    recovery_key_id: &str,
    k: u8,
    holders: &[Holder],
    maker: Signer,
    rng: &mut dyn Rng,
) -> Result<(RecoveryKeyRecord, Vec<HolderShare>), Error> {
    let n = u8::try_from(holders.len()).map_err(|_| Error::Shares("at most 255 holders"))?;
    for (at, holder) in holders.iter().enumerate() {
        if holders[..at].iter().any(|h| h.user_id == holder.user_id) {
            return Err(Error::Shares("a holder is listed once"));
        }
    }
    let seed = Key32::random(rng);
    let shares = shamir::split(seed.as_bytes(), k, n, rng)?;
    let mut record = RecoveryKeyRecord {
        recovery_key_id: recovery_key_id.to_string(),
        public: key_from_seed(seed.as_bytes()).public(),
        k,
        n,
        maker_user_id: maker.user_id.to_string(),
        maker_key_id: maker.key_id.to_string(),
        signature: Vec::new(),
    };
    record.signature = maker
        .key
        .signing_key()
        .sign(Statement::RecoveryKey, &refs(&record.signed_parts()))
        .to_vec();
    let mut out = Vec::with_capacity(holders.len());
    for (holder, share) in holders.iter().zip(shares.iter()) {
        let mut held = HolderShare {
            recovery_key_id: recovery_key_id.to_string(),
            holder_user_id: holder.user_id.to_string(),
            holder_key_id: holder.key_id.to_string(),
            index: share.index,
            sealed: Vec::new(),
            signature: Vec::new(),
        };
        held.sealed = hpke::wrap(
            &holder.public.encrypt,
            &held.names(),
            &[&[share.index]],
            &share.value,
            rng,
        )?;
        held.signature = maker
            .key
            .signing_key()
            .sign(Statement::RecoveryShare, &refs(&held.signed_parts(holder.public)))
            .to_vec();
        out.push(held);
    }
    Ok((record, out))
}

/// A holder's open of their share, after checking the maker's signature.
pub fn open_share(held: &HolderShare, me: &PersonalKey, maker: &VerifyingKey) -> Result<Share, Error> {
    held.verify(&me.public(), maker)?;
    let value = hpke::unwrap(me.encrypt_key(), &held.names(), &[&[held.index]], &held.sealed)?;
    Ok(Share {
        index: held.index,
        value,
    })
}

/// The recovery key from k shares, checked against the signed record: too few shares are refused, and wrong ones
/// give another key, which is refused.
pub fn combine(record: &RecoveryKeyRecord, maker: &VerifyingKey, shares: &[Share]) -> Result<PersonalKey, Error> {
    record.verify(maker)?;
    if shares.len() < usize::from(record.k) {
        return Err(Error::Shares("fewer shares than the recovery key needs"));
    }
    let seed = shamir::combine(shares)?;
    let key = key_from_seed(&seed);
    if key.public() != record.public {
        return Err(Error::Refused("the shares do not give this recovery key"));
    }
    Ok(key)
}

/// A recovery under way (`VaultRecoverySession`): the ephemeral key the recovering holder's client made, the vault, and
/// the member's new personal key the vault key will be wrapped to, signed by the recovering holder. A holder who
/// seals their share to it sees what they are agreeing to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoverySession {
    pub session_id: String,
    pub recovery_key_id: String,
    pub vault_id: String,
    pub subject_user_id: String,
    pub subject_key_id: String,
    pub subject_public: PersonalPublic,
    pub ephemeral: hpke::PublicKey,
    pub holder_user_id: String,
    pub holder_key_id: String,
    pub signature: Vec<u8>,
}

impl RecoverySession {
    fn signed_parts(&self) -> Vec<Vec<u8>> {
        vec![
            self.session_id.as_bytes().to_vec(),
            self.recovery_key_id.as_bytes().to_vec(),
            self.vault_id.as_bytes().to_vec(),
            self.subject_user_id.as_bytes().to_vec(),
            self.subject_key_id.as_bytes().to_vec(),
            self.subject_public.to_bytes().to_vec(),
            self.ephemeral.as_bytes().to_vec(),
            self.holder_user_id.as_bytes().to_vec(),
            self.holder_key_id.as_bytes().to_vec(),
        ]
    }

    /// Checks the recovering holder's signature.
    pub fn verify(&self, holder: &VerifyingKey) -> Result<(), Error> {
        holder.verify(Statement::RecoverySession, &refs(&self.signed_parts()), &self.signature)
    }
}

/// Starts a recovery in the recovering holder's client: a new ephemeral key, which the caller keeps in memory only,
/// and the signed session naming it.
pub fn start_session(
    session_id: &str,
    recovery_key_id: &str,
    vault_id: &str,
    subject: Recipient,
    holder: Signer,
    rng: &mut dyn Rng,
) -> Result<(RecoverySession, hpke::SecretKey), Error> {
    let ephemeral = hpke::SecretKey::random(rng);
    let mut session = RecoverySession {
        session_id: session_id.to_string(),
        recovery_key_id: recovery_key_id.to_string(),
        vault_id: vault_id.to_string(),
        subject_user_id: subject.user_id.to_string(),
        subject_key_id: subject.key_id.to_string(),
        subject_public: *subject.public,
        ephemeral: ephemeral.public()?,
        holder_user_id: holder.user_id.to_string(),
        holder_key_id: holder.key_id.to_string(),
        signature: Vec::new(),
    };
    session.signature = holder
        .key
        .signing_key()
        .sign(Statement::RecoverySession, &refs(&session.signed_parts()))
        .to_vec();
    Ok((session, ephemeral))
}

/// One holder's share sealed to a session's ephemeral key, signed by the holder (`VaultRecoveryDelivery`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionShare {
    pub session_id: String,
    pub recovery_key_id: String,
    pub holder_user_id: String,
    pub holder_key_id: String,
    pub sealed: Vec<u8>,
    pub signature: Vec<u8>,
}

impl SessionShare {
    fn names(&self) -> Names<'_> {
        Names::new(Purpose::RecoverySessionShare, &self.session_id, &self.holder_user_id, 0)
    }

    fn extra<'a>(&'a self, session: &'a RecoverySession) -> [&'a [u8]; 3] {
        [
            self.recovery_key_id.as_bytes(),
            self.holder_key_id.as_bytes(),
            session.ephemeral.as_bytes(),
        ]
    }

    fn signed_parts(&self, session: &RecoverySession) -> Vec<Vec<u8>> {
        vec![
            self.session_id.as_bytes().to_vec(),
            self.recovery_key_id.as_bytes().to_vec(),
            self.holder_user_id.as_bytes().to_vec(),
            self.holder_key_id.as_bytes().to_vec(),
            Sha256::digest(&session.signature).to_vec(),
            Sha256::digest(&self.sealed).to_vec(),
        ]
    }

    /// Checks the delivering holder's signature, which covers the session it was sealed for.
    pub fn verify(&self, session: &RecoverySession, holder: &VerifyingKey) -> Result<(), Error> {
        if self.session_id != session.session_id || self.recovery_key_id != session.recovery_key_id {
            return Err(Error::Refused("a share for another recovery"));
        }
        holder.verify(
            Statement::RecoverySessionShare,
            &refs(&self.signed_parts(session)),
            &self.signature,
        )
    }
}

/// Who started a session: their holder row (the maker's signature over their key and share) and their public key.
#[derive(Clone, Copy, Debug)]
pub struct Starter<'a> {
    pub held: &'a HolderShare,
    pub public: &'a PersonalPublic,
}

/// A holder's part in a recovery: checks that the session's starter is a holder of this recovery key (their holder row,
/// signed by the maker over their key) and that they signed the session, opens this holder's own share (checking the
/// maker's signature) and seals it to the session's ephemeral key, signed. The share is never shown to the holder or
/// their page.
pub fn deliver_share(
    held: &HolderShare,
    me: Signer,
    maker: &VerifyingKey,
    session: &RecoverySession,
    starter: Starter,
    rng: &mut dyn Rng,
) -> Result<SessionShare, Error> {
    let first = starter.held;
    if first.recovery_key_id != session.recovery_key_id
        || first.holder_user_id != session.holder_user_id
        || first.holder_key_id != session.holder_key_id
    {
        return Err(Error::Refused(
            "the session was not started by a holder of this recovery key",
        ));
    }
    first.verify(starter.public, maker)?;
    session.verify(&starter.public.verify)?;
    if held.recovery_key_id != session.recovery_key_id {
        return Err(Error::Refused("a share of another recovery key"));
    }
    if held.holder_user_id != me.user_id || held.holder_key_id != me.key_id {
        return Err(Error::Refused("the share is someone else's"));
    }
    if session.holder_user_id == me.user_id {
        return Err(Error::Refused("the recovering holder combines their own share"));
    }
    let share = open_share(held, me.key, maker)?;
    let mut value = Zeroizing::new(Vec::with_capacity(share.value.len() + 1));
    value.push(share.index);
    value.extend_from_slice(&share.value);
    let mut sent = SessionShare {
        session_id: session.session_id.clone(),
        recovery_key_id: session.recovery_key_id.clone(),
        holder_user_id: me.user_id.to_string(),
        holder_key_id: me.key_id.to_string(),
        sealed: Vec::new(),
        signature: Vec::new(),
    };
    sent.sealed = hpke::wrap(&session.ephemeral, &sent.names(), &sent.extra(session), &value, rng)?;
    sent.signature = me
        .key
        .signing_key()
        .sign(Statement::RecoverySessionShare, &refs(&sent.signed_parts(session)))
        .to_vec();
    Ok(sent)
}

/// A delivered share with what checks it: the sender's holder row (signed by the maker over the sender's key) and
/// the sender's public halves.
#[derive(Clone, Copy, Debug)]
pub struct Delivered<'a> {
    pub share: &'a SessionShare,
    pub holder: &'a HolderShare,
    pub public: &'a PersonalPublic,
}

/// The recovering client's combine: its own share and the delivered ones, each from a holder the maker gave a share
/// to (so a delivered share carries that holder's index, and one index counts once), checked against the signed
/// record. The ephemeral key opens each delivered share.
pub fn combine_session(
    record: &RecoveryKeyRecord,
    maker: &VerifyingKey,
    session: &RecoverySession,
    ephemeral: &hpke::SecretKey,
    own: Share,
    delivered: &[Delivered],
) -> Result<PersonalKey, Error> {
    record.verify(maker)?;
    if session.recovery_key_id != record.recovery_key_id {
        return Err(Error::Refused("a session of another recovery key"));
    }
    let mut shares = vec![own];
    for one in delivered {
        let holder = one.holder;
        if holder.recovery_key_id != record.recovery_key_id
            || holder.holder_user_id != one.share.holder_user_id
            || holder.holder_key_id != one.share.holder_key_id
        {
            return Err(Error::Refused("a share from someone who is not its holder"));
        }
        holder.verify(one.public, maker)?;
        one.share.verify(session, &one.public.verify)?;
        if shares.iter().any(|share| share.index == holder.index) {
            continue;
        }
        let value = hpke::unwrap(
            ephemeral,
            &one.share.names(),
            &one.share.extra(session),
            &one.share.sealed,
        )?;
        let Some((&index, rest)) = value.split_first() else {
            return Err(Error::Format("an empty share"));
        };
        if index != holder.index {
            return Err(Error::Refused("a share under another holder's index"));
        }
        shares.push(Share {
            index,
            value: Zeroizing::new(rest.to_vec()),
        });
    }
    combine(record, maker, &shares)
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;
