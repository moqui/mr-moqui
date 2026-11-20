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
//! 2026). Its public half is signed by the holder who made it; its private half is split into n Shamir shares, each
//! HPKE-wrapped to one holder's personal key and signed by the maker, and then dropped: no place holds the whole key.
//! A recovery takes k holders' clients: each opens their share and delivers it to the recovering holder
//! ([`crate::delivery`]), whose client combines them, checks the result against the signed public half, and opens
//! the vault's wrap to the recovery key.

use sha2::{Digest as _, Sha256};

use crate::hpke;
use crate::keys::Key32;
use crate::names::{refs, Names, Purpose};
use crate::personal::{PersonalKey, PersonalPublic};
use crate::rng::Rng;
use crate::shamir::{self, Share};
use crate::sign::{Statement, VerifyingKey};
use crate::vault::Signer;
use crate::Error;

/// A recovery key's public record (`VaultRecoveryKey`), signed by its maker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryKeyRecord {
    pub recovery_key_id: String,
    pub public: hpke::PublicKey,
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
            self.public.as_bytes().to_vec(),
            vec![self.k, self.n],
            self.maker_user_id.as_bytes().to_vec(),
            self.maker_key_id.as_bytes().to_vec(),
        ]
    }

    /// A client refuses a recovery key with no signature it can check, so a server cannot hand out its own.
    pub fn verify(&self, maker: &VerifyingKey) -> Result<(), Error> {
        maker.verify(Statement::RecoveryKey, &refs(&self.signed_parts()), &self.signature)
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

/// Makes a recovery key split `k` of the holders' count, one share to each holder. The private key is dropped here.
pub fn make(
    recovery_key_id: &str,
    k: u8,
    holders: &[Holder],
    maker: Signer,
    rng: &mut dyn Rng,
) -> Result<(RecoveryKeyRecord, Vec<HolderShare>), Error> {
    let n = u8::try_from(holders.len()).map_err(|_| Error::Shares("at most 255 holders"))?;
    let secret = hpke::SecretKey::random(rng);
    let shares = shamir::split(secret.as_key().as_bytes(), k, n, rng)?;
    let mut record = RecoveryKeyRecord {
        recovery_key_id: recovery_key_id.to_string(),
        public: secret.public()?,
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

/// The recovery key from k shares, checked against the signed record: too few or wrong shares give another key,
/// which is refused.
pub fn combine(record: &RecoveryKeyRecord, maker: &VerifyingKey, shares: &[Share]) -> Result<hpke::SecretKey, Error> {
    record.verify(maker)?;
    if shares.len() < usize::from(record.k) {
        return Err(Error::Shares("fewer shares than the recovery key needs"));
    }
    let combined = shamir::combine(shares)?;
    let secret = hpke::SecretKey::from_key(Key32::from_slice(&combined)?);
    if secret.public()? != record.public {
        return Err(Error::Refused("the shares do not give this recovery key"));
    }
    Ok(secret)
}

/// A vault's wrap of its key to the recovery key, signed by the member who turned recovery on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryWrap {
    pub vault_id: String,
    pub key_version: u64,
    pub recovery_key_id: String,
    pub sealed: Vec<u8>,
    pub signer_user_id: String,
    pub signer_key_id: String,
    pub signature: Vec<u8>,
}

impl RecoveryWrap {
    fn names(&self) -> Names<'_> {
        Names::new(Purpose::RecoveryKey, &self.vault_id, "", self.key_version)
    }

    fn signed_parts(&self, record: &RecoveryKeyRecord) -> Vec<Vec<u8>> {
        vec![
            self.vault_id.as_bytes().to_vec(),
            self.key_version.to_be_bytes().to_vec(),
            self.recovery_key_id.as_bytes().to_vec(),
            record.public.as_bytes().to_vec(),
            Sha256::digest(&self.sealed).to_vec(),
            self.signer_user_id.as_bytes().to_vec(),
            self.signer_key_id.as_bytes().to_vec(),
        ]
    }

    pub fn verify(&self, record: &RecoveryKeyRecord, signer: &VerifyingKey) -> Result<(), Error> {
        if self.recovery_key_id != record.recovery_key_id {
            return Err(Error::Refused("a wrap to another recovery key"));
        }
        signer.verify(
            Statement::RecoveryWrap,
            &refs(&self.signed_parts(record)),
            &self.signature,
        )
    }
}

/// Turns recovery on for a vault: checks the recovery key's signature by its maker (whose key the member checked),
/// then wraps the vault key to it and signs the wrap.
pub fn wrap_vault_key(
    vault_key: &Key32,
    vault_id: &str,
    key_version: u64,
    record: &RecoveryKeyRecord,
    record_maker: &VerifyingKey,
    signer: Signer,
    rng: &mut dyn Rng,
) -> Result<RecoveryWrap, Error> {
    record.verify(record_maker)?;
    let mut wrap = RecoveryWrap {
        vault_id: vault_id.to_string(),
        key_version,
        recovery_key_id: record.recovery_key_id.clone(),
        sealed: Vec::new(),
        signer_user_id: signer.user_id.to_string(),
        signer_key_id: signer.key_id.to_string(),
        signature: Vec::new(),
    };
    wrap.sealed = hpke::wrap(&record.public, &wrap.names(), &[], vault_key.as_bytes(), rng)?;
    wrap.signature = signer
        .key
        .signing_key()
        .sign(Statement::RecoveryWrap, &refs(&wrap.signed_parts(record)))
        .to_vec();
    Ok(wrap)
}

/// Opens a vault's recovery wrap with the combined recovery key, after checking the signer's signature.
pub fn open_vault_key(
    wrap: &RecoveryWrap,
    record: &RecoveryKeyRecord,
    recovery: &hpke::SecretKey,
    signer: &VerifyingKey,
) -> Result<Key32, Error> {
    wrap.verify(record, signer)?;
    hpke::unwrap_key(recovery, &wrap.names(), &[], &wrap.sealed)
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;
