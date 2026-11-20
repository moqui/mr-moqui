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

//! Deliveries (plans/Vault-Instances.md §3.3, `VaultDelivery`): a value sealed to one user's personal key, from a user
//! (their signing key) or an instance (its link key, also Ed25519), with its purpose and the sender's signature. The
//! recipient's client checks the signature before it files the value into a vault. A client files each `delivery_id`
//! once and drops a repeat ([`Filed`]). A recovery holder's share goes to a recovery session's ephemeral key instead
//! ([`crate::recovery::SessionShare`]).

use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

use crate::hpke;
use crate::names::{refs, Names, Purpose};
use crate::personal::{PersonalKey, PersonalPublic};
use crate::rng::Rng;
use crate::sign::{SigningKey, Statement, VerifyingKey};
use crate::Error;

/// A delivery as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delivery {
    pub delivery_id: String,
    pub recipient_user_id: String,
    pub recipient_key_id: String,
    pub purpose: String,
    /// `user` or `instance`.
    pub sender_kind: String,
    pub sender_id: String,
    pub sealed: Vec<u8>,
    pub signature: Vec<u8>,
}

impl Delivery {
    fn names(&self) -> Names<'_> {
        Names::new(Purpose::Delivery, "", &self.delivery_id, 0)
    }

    fn extra(&self) -> [&[u8]; 4] {
        [
            self.recipient_user_id.as_bytes(),
            self.recipient_key_id.as_bytes(),
            self.purpose.as_bytes(),
            self.sender_id.as_bytes(),
        ]
    }

    fn signed_parts(&self) -> Vec<Vec<u8>> {
        vec![
            self.delivery_id.as_bytes().to_vec(),
            self.recipient_user_id.as_bytes().to_vec(),
            self.recipient_key_id.as_bytes().to_vec(),
            self.purpose.as_bytes().to_vec(),
            self.sender_kind.as_bytes().to_vec(),
            self.sender_id.as_bytes().to_vec(),
            Sha256::digest(&self.sealed).to_vec(),
        ]
    }

    pub fn verify(&self, sender: &VerifyingKey) -> Result<(), Error> {
        sender.verify(Statement::Delivery, &refs(&self.signed_parts()), &self.signature)
    }
}

/// Who sends.
#[derive(Clone, Copy, Debug)]
pub struct Sender<'a> {
    /// `user` or `instance`.
    pub kind: &'a str,
    pub id: &'a str,
    pub key: &'a SigningKey,
}

/// To whom.
#[derive(Clone, Copy, Debug)]
pub struct To<'a> {
    pub user_id: &'a str,
    pub key_id: &'a str,
    pub public: &'a PersonalPublic,
}

/// Seals `value` to the recipient and signs it.
pub fn send(
    delivery_id: &str,
    to: To,
    purpose: &str,
    value: &[u8],
    sender: Sender,
    rng: &mut dyn Rng,
) -> Result<Delivery, Error> {
    let mut delivery = Delivery {
        delivery_id: delivery_id.to_string(),
        recipient_user_id: to.user_id.to_string(),
        recipient_key_id: to.key_id.to_string(),
        purpose: purpose.to_string(),
        sender_kind: sender.kind.to_string(),
        sender_id: sender.id.to_string(),
        sealed: Vec::new(),
        signature: Vec::new(),
    };
    delivery.sealed = hpke::wrap(&to.public.encrypt, &delivery.names(), &delivery.extra(), value, rng)?;
    delivery.signature = sender
        .key
        .sign(Statement::Delivery, &refs(&delivery.signed_parts()))
        .to_vec();
    Ok(delivery)
}

/// The recipient's open: the sender's signature first, then the value.
pub fn open(delivery: &Delivery, me: &PersonalKey, sender: &VerifyingKey) -> Result<Zeroizing<Vec<u8>>, Error> {
    delivery.verify(sender)?;
    hpke::unwrap(me.encrypt_key(), &delivery.names(), &delivery.extra(), &delivery.sealed)
}

/// The delivery ids a client has filed. A delivery is signed but carries no freshness, so a server could hand the
/// same one over again (a second copy of an item, a recovery share counted twice); a client files each id once and
/// drops a repeat.
#[derive(Clone, Debug, Default)]
pub struct Filed {
    ids: std::collections::BTreeSet<String>,
}

impl Filed {
    /// True the first time `delivery`'s id is seen, false for a repeat.
    pub fn first_time(&mut self, delivery: &Delivery) -> bool {
        self.ids.insert(delivery.delivery_id.clone())
    }
}

#[cfg(test)]
#[path = "delivery_tests.rs"]
mod tests;
