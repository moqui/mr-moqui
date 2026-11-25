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

//! A managed instance's recovery shares, on the instance (plans/archive/Vault-Instances.md §4.5, M6). Nobody is present on the
//! instance to compare a holder's fingerprint, so an approver's check stands in for it:
//!
//! 1. The instance takes each holder's public keys over the link with the key checks of them, and takes a key only
//!    with a check signed by an approver it pinned at pairing ([`check_holder`]); a key without one is refused.
//! 2. It seals each share to one holder, a different holder for each, and sends it as a delivery signed with its link
//!    key ([`make_set`]); it keeps only each share's digest and the holder's signing key.
//! 3. Each holder's client checks the delivery against the instance's pinned link key, files the share in their
//!    personal vault and signs a confirmation, which the instance checks itself ([`take_confirmations`]). Init is
//!    complete when every share is confirmed; an unconfirmed share is reported. A holder no longer among the checked
//!    holders (replaced) makes the instance make a new set.
//!
//! When fewer holders are checked than there are shares, nothing is sent: the shares wait for `moqui operator init
//! --take-shares` on the operator socket, printed, as Sealed-Secrets has it.

use moqui_vault::delivery::{self, Sender, To};
use moqui_vault::fingerprint::{require_checked, KeyCheck};
use moqui_vault::managed::{self, ShareValue};
use moqui_vault::personal::PersonalPublic;
use moqui_vault::rng::{OsRng, Rng as _};
use moqui_vault::sign::{SigningKey, VerifyingKey};
use moqui_vault::text::hex;
use zeroize::Zeroizing;

use super::proto;
use super::state::{SentShare, ShareSet};

/// A holder whose key an approver the instance pinned checked.
#[derive(Clone, Debug)]
pub(crate) struct Checked {
    pub user_id: String,
    pub key_id: String,
    pub public: PersonalPublic,
}

fn key_check(check: &proto::KeyCheck) -> KeyCheck {
    KeyCheck {
        checker_user_id: check.checker_user_id.clone(),
        checker_key_id: check.checker_key_id.clone(),
        checked_key_id: check.checked_key_id.clone(),
        checked_user_id: check.checked_user_id.clone(),
        checked_encrypt: check.checked_encrypt.clone(),
        checked_verify: check.checked_verify.clone(),
        checked_at: check.checked_at,
        signature: check.signature.clone(),
    }
}

/// `holder`'s keys, when a key check of them by an approver in `approvers` names them as that holder's.
pub(crate) fn check_holder(
    holder: &proto::Holder,
    approvers: &moqui_vault::approvers::ApproverSet,
) -> Result<Checked, String> {
    let mut both = holder.encrypt_public.clone();
    both.extend_from_slice(&holder.verify_public);
    let public =
        PersonalPublic::from_bytes(&both).map_err(|err| format!("{}'s key does not read: {err}", holder.user_id))?;
    let checks: Vec<KeyCheck> = holder.checks.iter().map(key_check).collect();
    let checked = approvers.approvers.iter().any(|approver| {
        VerifyingKey::from_bytes(&approver.verify_public)
            .is_ok_and(|key| require_checked(&checks, &key, &holder.user_id, &public).is_ok())
    });
    if !checked {
        return Err(format!(
            "{}'s key has no key check by an approver this instance pinned: refused",
            holder.user_id
        ));
    }
    Ok(Checked {
        user_id: holder.user_id.clone(),
        key_id: holder.personal_key_id.clone(),
        public,
    })
}

/// The holders of `reply` whose keys are checked, in user order, one each; the refused ones are logged.
pub(crate) fn checked_holders(
    reply: &proto::HoldersReply,
    approvers: &moqui_vault::approvers::ApproverSet,
) -> Vec<Checked> {
    let mut checked = Vec::new();
    for holder in &reply.holders {
        match check_holder(holder, approvers) {
            // one share to a holder, and to a key: two holders given one key hold no two shares between them
            Ok(holder)
                if !checked.iter().any(|other: &Checked| {
                    other.user_id == holder.user_id
                        || other.public.encrypt.as_bytes() == holder.public.encrypt.as_bytes()
                        || other.public.verify.as_bytes() == holder.public.verify.as_bytes()
                }) =>
            {
                checked.push(holder)
            }
            Ok(holder) => tracing::error!(
                "Manager link: recovery holder {} is given twice, or with another holder's key: left out",
                holder.user_id
            ),
            Err(err) => tracing::error!("Manager link: recovery holder {err}"),
        }
    }
    checked.sort_by(|a, b| a.user_id.cmp(&b.user_id));
    checked
}

/// A new share set: share `i` sealed to holder `i` alone, each a delivery signed with the link key; the request for
/// the manager, and what the instance keeps of it.
pub(crate) fn make_set(
    instance_id: &str,
    link_key: &SigningKey,
    shares: &[Zeroizing<String>],
    holders: &[Checked],
    threshold: u32,
) -> Result<(proto::SharesRequest, ShareSet), String> {
    if holders.len() < shares.len() {
        return Err(format!(
            "{} recovery shares and {} checked holders: each holder holds one",
            shares.len(),
            holders.len()
        ));
    }
    let mut id = [0_u8; 8];
    OsRng.fill(&mut id);
    let set_id = format!("rs-{}", hex(&id));
    let of = u32::try_from(shares.len()).map_err(|_| "too many shares".to_string())?;
    let mut request = proto::SharesRequest {
        set_id: set_id.clone(),
        link_public: link_key.verifying_key().as_bytes().to_vec(),
        threshold,
        deliveries: Vec::new(),
    };
    let mut kept = ShareSet {
        set_id: set_id.clone(),
        threshold,
        shares: Vec::new(),
        refused: false,
    };
    for ((position, share), holder) in shares.iter().enumerate().zip(holders) {
        let index = u32::try_from(position + 1).map_err(|_| "too many shares".to_string())?;
        let value = ShareValue {
            instance_id: instance_id.to_string(),
            instance_name: String::new(),
            set_id: set_id.clone(),
            index,
            of,
            threshold,
            share: share.clone(),
        };
        let sent = delivery::send(
            &format!("{set_id}-{index}"),
            To {
                user_id: &holder.user_id,
                key_id: &holder.key_id,
                public: &holder.public,
            },
            managed::SHARE_PURPOSE,
            value.to_text().as_bytes(),
            Sender {
                kind: "instance",
                id: instance_id,
                key: link_key,
            },
            &mut OsRng,
        )
        .map_err(|err| format!("the share for {} was not sealed: {err}", holder.user_id))?;
        request.deliveries.push(proto::ShareDelivery {
            delivery_id: sent.delivery_id,
            recipient_user_id: sent.recipient_user_id,
            recipient_key_id: sent.recipient_key_id,
            purpose: sent.purpose,
            sealed: sent.sealed,
            signature: sent.signature,
            index,
        });
        kept.shares.push(SentShare {
            index,
            user_id: holder.user_id.clone(),
            verify_public: hex(holder.public.verify.as_bytes()),
            digest: hex(&managed::share_digest(share)),
            confirmed: false,
        });
    }
    Ok((request, kept))
}

/// Take the holders' confirmations into `set`, each checked with the holder's key and the digest kept of their share;
/// the number newly confirmed. One that does not check is logged and left unconfirmed.
pub(crate) fn take_confirmations(
    set: &mut ShareSet,
    instance_id: &str,
    reply: &proto::ShareConfirmationsReply,
) -> usize {
    let mut taken = 0;
    let of = set.shares.len();
    for confirmation in &reply.confirmations {
        let Some(share) = set
            .shares
            .iter_mut()
            .find(|share| share.index == confirmation.index && share.user_id == confirmation.holder_user_id)
        else {
            continue;
        };
        if share.confirmed {
            continue;
        }
        let key = moqui_vault::text::unhex(&share.verify_public)
            .ok()
            .and_then(|bytes| VerifyingKey::from_bytes(&bytes).ok());
        let digest = moqui_vault::text::unhex(&share.digest)
            .ok()
            .and_then(|bytes| <[u8; 32]>::try_from(bytes.as_slice()).ok());
        let (Some(key), Some(digest)) = (key, digest) else {
            continue;
        };
        match managed::verify_confirmation(
            &key,
            instance_id,
            &set.set_id,
            share.index,
            &digest,
            &confirmation.signature,
        ) {
            Ok(()) => {
                share.confirmed = true;
                taken += 1;
                tracing::info!(
                    "Manager link: {} confirmed recovery share {} of {}",
                    share.user_id,
                    share.index,
                    of
                );
            }
            Err(_) => tracing::error!(
                "Manager link: the confirmation of recovery share {} by {} does not check: left unconfirmed",
                share.index,
                share.user_id
            ),
        }
    }
    taken
}

/// Whether a holder of `set` is no longer among `holders` with the key it was sent to: replaced, so a new set is due.
pub(crate) fn holder_replaced(set: &ShareSet, holders: &[Checked]) -> bool {
    set.shares.iter().any(|share| {
        !holders.iter().any(|holder| {
            holder.user_id == share.user_id && hex(holder.public.verify.as_bytes()) == share.verify_public
        })
    })
}

#[cfg(test)]
#[path = "shares_tests.rs"]
mod tests;
