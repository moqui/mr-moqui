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

//! What a managed instance pinned at pairing (plans/archive/Vault-Instances.md §4.4): its manager's address and key, the id
//! its manager knows it by, and its environment's approvers' signing keys. It is read while the instance is sealed, so
//! it is a file of the runtime (`runtime/db/manage_link.json`, mode 0600), not a row; it holds no secret.

use std::path::Path;

use moqui_vault::approvers::{Approver, ApproverSet};
use serde_json::{json, Value};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PinnedApprover {
    pub user_id: String,
    /// The Ed25519 key of their personal key, hex.
    pub verify_public: String,
}

/// One recovery share the instance sent (M6): to whom, the key it checks their confirmation with, and SHA-256 of the
/// share (`moqui_vault::managed::share_digest`), never the share.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SentShare {
    pub index: u32,
    pub user_id: String,
    /// The holder's Ed25519 key, hex, as an approver checked it.
    pub verify_public: String,
    pub digest: String,
    pub confirmed: bool,
}

/// The recovery share set the instance sent to its holders (§4.5).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShareSet {
    pub set_id: String,
    pub threshold: u32,
    pub shares: Vec<SentShare>,
    /// The manager refused it for good: a new set is made for the current holders.
    pub refused: bool,
}

impl ShareSet {
    /// Every share's holder confirmed it: init is complete.
    #[must_use]
    pub fn complete(&self) -> bool {
        !self.shares.is_empty() && self.shares.iter().all(|share| share.confirmed)
    }
}

/// The instance's side of a pairing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinkState {
    /// The manager: `https://host[:port]`.
    pub url: String,
    /// SHA-256 of the manager's link key, hex.
    pub pin: String,
    pub app_instance_id: String,
    pub environment_id: String,
    pub manager_id: String,
    pub approvers_version: u64,
    pub approvers: Vec<PinnedApprover>,
    /// The instance stored its bootstrap bundle, or its manager had none for it: a first start is over.
    pub bootstrap_done: bool,
    /// The recovery shares sent to holders (M6), none before.
    pub share_set: Option<ShareSet>,
    /// A share set not yet taken by the manager: the encoded `SharesRequest`, hex, sent again on the next link. It holds
    /// only deliveries, each sealed to one holder.
    pub pending_shares: String,
}

impl LinkState {
    /// The state in `file`, or none when there is none; a file that does not read is logged and taken as none, so the
    /// instance pairs again.
    #[must_use]
    pub fn load(file: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(file).ok()?;
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(kept)) => {
                let text = |name: &str| kept.get(name).and_then(Value::as_str).unwrap_or_default().to_string();
                Some(Self {
                    url: text("url"),
                    pin: text("pin"),
                    app_instance_id: text("appInstanceId"),
                    environment_id: text("environmentId"),
                    manager_id: text("managerId"),
                    approvers_version: kept.get("approversVersion").and_then(Value::as_u64).unwrap_or(0),
                    approvers: kept
                        .get("approvers")
                        .and_then(Value::as_array)
                        .map(|list| {
                            list.iter()
                                .map(|approver| PinnedApprover {
                                    user_id: approver["userId"].as_str().unwrap_or_default().to_string(),
                                    verify_public: approver["verifyPublic"].as_str().unwrap_or_default().to_string(),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    bootstrap_done: kept.get("bootstrapDone").and_then(Value::as_bool).unwrap_or(false),
                    share_set: kept.get("shareSet").filter(|set| set.is_object()).map(share_set_from),
                    pending_shares: text("pendingShares"),
                })
            }
            _ => {
                tracing::error!("Manager link: {} does not read; pair again", file.display());
                None
            }
        }
    }

    pub fn save(&self, file: &Path) -> Result<(), String> {
        let kept = json!({
            "url": self.url,
            "pin": self.pin,
            "appInstanceId": self.app_instance_id,
            "environmentId": self.environment_id,
            "managerId": self.manager_id,
            "approversVersion": self.approvers_version,
            "approvers": self.approvers.iter().map(|approver| json!({
                "userId": approver.user_id,
                "verifyPublic": approver.verify_public,
            })).collect::<Vec<_>>(),
            "bootstrapDone": self.bootstrap_done,
            "shareSet": self.share_set.as_ref().map(|set| json!({
                "setId": set.set_id,
                "threshold": set.threshold,
                "refused": set.refused,
                "shares": set.shares.iter().map(|share| json!({
                    "index": share.index, "userId": share.user_id, "verifyPublic": share.verify_public,
                    "digest": share.digest, "confirmed": share.confirmed,
                })).collect::<Vec<_>>(),
            })),
            "pendingShares": self.pending_shares,
        });
        let text = serde_json::to_string_pretty(&kept).map_err(|err| err.to_string())?;
        super::key::write_private(file, text.as_bytes())
    }

    /// The approvers pinned, as the vault crate checks a new set against them.
    #[must_use]
    pub fn approver_set(&self) -> ApproverSet {
        ApproverSet {
            version: self.approvers_version,
            approvers: self
                .approvers
                .iter()
                .filter_map(|approver| {
                    let key = moqui_vault::text::unhex(&approver.verify_public).ok()?;
                    Some(Approver {
                        user_id: approver.user_id.clone(),
                        verify_public: key.as_slice().try_into().ok()?,
                    })
                })
                .collect(),
        }
    }

    /// Pin `set` as the approvers.
    pub fn pin_approvers(&mut self, set: &ApproverSet) {
        self.approvers_version = set.version;
        self.approvers = set
            .approvers
            .iter()
            .map(|approver| PinnedApprover {
                user_id: approver.user_id.clone(),
                verify_public: moqui_vault::text::hex(&approver.verify_public),
            })
            .collect();
    }
}

fn share_set_from(kept: &Value) -> ShareSet {
    let text = |value: &Value, name: &str| value[name].as_str().unwrap_or_default().to_string();
    ShareSet {
        set_id: text(kept, "setId"),
        threshold: kept["threshold"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0),
        refused: kept["refused"].as_bool().unwrap_or(false),
        shares: kept["shares"]
            .as_array()
            .map(|list| {
                list.iter()
                    .map(|share| SentShare {
                        index: share["index"].as_u64().and_then(|n| u32::try_from(n).ok()).unwrap_or(0),
                        user_id: text(share, "userId"),
                        verify_public: text(share, "verifyPublic"),
                        digest: text(share, "digest"),
                        confirmed: share["confirmed"].as_bool().unwrap_or(false),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// An approver set as the link carries it, and back.
pub(crate) fn set_from_proto(set: &super::proto::ApproverSet) -> ApproverSet {
    ApproverSet {
        version: set.version,
        approvers: set
            .approvers
            .iter()
            .filter_map(|approver| {
                Some(Approver {
                    user_id: approver.user_id.clone(),
                    verify_public: approver.verify_public.as_slice().try_into().ok()?,
                })
            })
            .collect(),
    }
}

// the manager's, which sends a set (the `instances` feature)
#[cfg_attr(not(feature = "instances"), allow(dead_code))]
pub(crate) fn set_to_proto(set: &ApproverSet) -> super::proto::ApproverSet {
    super::proto::ApproverSet {
        version: set.version,
        approvers: set
            .approvers
            .iter()
            .map(|approver| super::proto::Approver {
                user_id: approver.user_id.clone(),
                verify_public: approver.verify_public.to_vec(),
            })
            .collect(),
        signer_public: Vec::new(),
        signature: Vec::new(),
    }
}
