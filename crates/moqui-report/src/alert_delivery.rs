//! Owner-equivalent business alerts (BOSS impl-insights P8, INS-07; design-insights §8; docs/tools/Monitoring.md
//! "Business alerts").
//!
//! An alert rule is evaluated once, as its owner (alert.rs). That one state and its values may be told to another user
//! only when it is what that user would have read: when their scope fingerprint for the rule equals the owner's, the
//! check of report delivery (delivery.rs [`delivery::judge`], `check#ReportAudience`). A **business rule** is one that
//! names an `audience`, or whose query reads a dataset whose rows depend on the viewer (a business dataset, a
//! `rowFilterService`, `contextFilters`), whatever wrote its row. For such a rule:
//!
//! - it has no contact points: no address, no webhook and no notification to a group as a whole. Its `audience` names
//!   internal users and groups and the channels `notification` (a private in-app notification) and `email` (the
//!   recipient's own account address, through an email template). The server sets its `deliveryMode`, personal when the
//!   owner is the only recipient;
//! - an active rule is saved only when every recipient passes the check and the audience is within the recipient cap
//!   (delivery.rs [`delivery::caps`]); recipients other than the owner need the group delivery artifact;
//! - at each notification every recipient is checked again and told separately; one whose scope changed is skipped
//!   with `scope-differs`, one whose message fails is retried at the next pass, and one already told a state is not told
//!   it again until the repeat interval. Each recipient's last outcome is a row of [`DELIVERY`], which keeps no value;
//! - its state changes write no marker on the shared time series and open no incident;
//! - a source whose index is stale gives no valid reading: Error, never Normal;
//! - a copy of a shipped or shared rule, and an imported one, is paused (`not-validated`) until its owner activates it,
//!   which is the save that runs these checks; a rule whose owner cannot sign in pauses (`owner-disabled`) until
//!   [`transfer_alert_rule_owner`] hands it to a qualified owner, who resumes it.
//!
//! A technical rule (telemetry and other datasets that are not viewer-scoped, with no audience) is evaluated and
//! delivered to its contact points as before.
use crate::delivery::{self, Account, Scope};
use crate::{definitions, fail};
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::transaction::TransactionAttribute;
use moqui_core::{
    context::ExecutionContext,
    native::{NativeRegistry, Parameters, ServiceInfo},
};
use moqui_lib::{error::Error, object::Value};
use serde_json::{json, Value as Json};
use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const DELIVERY: &str = "moqui.report.AlertRecipientDelivery";
const INSTANCE: &str = "moqui.report.AlertInstance";
const RULE_LOCK: &str = "moqui.report.AlertRuleLock";
const SAVE_SERVICE: &str = "org.moqui.report.ReportServices.save#Definition";
const DEFAULT_TOPIC: &str = "ReportAlert";
const CHANNELS: [&str; 2] = ["notification", "email"];
/// What an audience may hold. Anything else (an address, a URL, a header) is refused, not dropped.
const AUDIENCE_KEYS: [&str; 10] = [
    "recipientUserIds",
    "recipientGroupIds",
    "channels",
    "emailTemplateId",
    "topic",
    "recipientCap",
    "groupBy",
    "groupWaitSeconds",
    "repeatSeconds",
    "deliveryMode",
];
/// The most users or groups one audience names; the recipient cap bounds who is told.
const MAX_NAMED: usize = 500;

fn jtext<'a>(value: &'a Json, name: &str, default: &'a str) -> &'a str {
    value.get(name).and_then(Json::as_str).unwrap_or(default)
}

/// A rule held to owner equivalence: one that names an audience, or whose query reads a dataset whose rows depend on
/// the viewer. Read whoever asks: the engine's own question.
pub(crate) fn is_business(ec: &ExecutionContext, definition: &Json) -> bool {
    definition["type"] == "alert" && body_business(ec, &definition["body"])
}
fn body_business(ec: &ExecutionContext, body: &Json) -> bool {
    body["audience"].is_object() || delivery::body_reads_business(ec, body)
}

/// The users and groups an audience names; nobody named is the owner alone.
fn named(body: &Json) -> (Vec<String>, Vec<String>) {
    let list = |key: &str| -> Vec<String> {
        let mut ids: Vec<String> = body["audience"][key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
            .map(|id| id.trim().to_owned())
            .filter(|id| !id.is_empty())
            .collect();
        ids.sort();
        ids.dedup();
        ids
    };
    (list("recipientUserIds"), list("recipientGroupIds"))
}
fn channels(body: &Json) -> Vec<&str> {
    let given: Vec<&str> = body["audience"]["channels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .filter(|channel| CHANNELS.contains(channel))
        .collect();
    if given.is_empty() {
        vec!["notification"]
    } else {
        CHANNELS.into_iter().filter(|channel| given.contains(channel)).collect()
    }
}

/// A path on this server a notification may link to: the mounting app's rule screen.
pub(super) fn server_path(link: &str) -> bool {
    link.starts_with('/')
        && !link.starts_with("//")
        && link.len() <= 255
        && link.bytes().all(|c| c.is_ascii_alphanumeric() || b"/_-.".contains(&c))
        && !link.contains("..")
}

fn exists(ec: &ExecutionContext, entity_name: &str, key: &str, id: &str) -> Result<bool, Error> {
    let _authz = ec.disable_authz();
    Ok(delivery::entity(ec)?
        .find(entity_name)?
        .eq(key, Value::string(id))
        .one()?
        .is_some())
}

/// Before a rule is saved: its link checked, and for a business rule its contact points refused and its audience made
/// the engine's own (known fields only, the delivery mode set here). Returns whether it is a business rule.
pub(super) fn prepare(ec: &ExecutionContext, body: &mut Json) -> Result<bool, Error> {
    if !body.is_object() {
        return Ok(false);
    }
    if !body["detailLink"].is_null() && !body["detailLink"].as_str().is_some_and(server_path) {
        return Err(fail("alert detailLink must be a plain path on this server"));
    }
    if !body["audience"].is_null() && !body["audience"].is_object() {
        return Err(fail("an alert audience must be an object"));
    }
    // a rule saved active is paused for no reason any more (one resumed by a new owner, or after its owner returned)
    if body["paused"] != true {
        if let Some(map) = body.as_object_mut() {
            map.remove("pausedReason");
        }
    }
    if !body_business(ec, body) {
        return Ok(false);
    }
    let has = |key: &str| body[key].as_array().is_some_and(|items| !items.is_empty());
    if has("contactPoints") || has("contactPointRefs") {
        return Err(fail(
            "BUSINESS_CONTACT_REFUSED: an alert over business data notifies its audience in the app or at their account's \
             address; it takes no contact point, address or webhook",
        ));
    }
    let given = body["audience"].as_object().cloned().unwrap_or_default();
    if let Some(unknown) = given.keys().find(|key| !AUDIENCE_KEYS.contains(&key.as_str())) {
        return Err(fail(format!(
            "BUSINESS_AUDIENCE_REFUSED: an alert audience names users and groups, never {unknown}"
        )));
    }
    let ids = |key: &str| -> Result<Vec<String>, Error> {
        let Some(value) = given.get(key).filter(|value| !value.is_null()) else {
            return Ok(Vec::new());
        };
        let items = value
            .as_array()
            .filter(|items| items.iter().all(Json::is_string) && items.len() <= MAX_NAMED)
            .ok_or_else(|| {
                fail(format!(
                    "alert audience {key} must be a list of at most {MAX_NAMED} ids"
                ))
            })?;
        let mut ids: Vec<String> = items
            .iter()
            .filter_map(Json::as_str)
            .map(|id| id.trim().to_owned())
            .filter(|id| !id.is_empty())
            .collect();
        ids.sort();
        ids.dedup();
        Ok(ids)
    };
    let (users, groups) = (ids("recipientUserIds")?, ids("recipientGroupIds")?);
    let wanted = ids("channels")?;
    if let Some(other) = wanted.iter().find(|channel| !CHANNELS.contains(&channel.as_str())) {
        return Err(fail(format!(
            "BUSINESS_CONTACT_REFUSED: an alert over business data notifies in the app or by account email, not by {other}"
        )));
    }
    let chosen: Vec<&str> = if wanted.is_empty() {
        vec!["notification"]
    } else {
        CHANNELS
            .into_iter()
            .filter(|channel| wanted.iter().any(|wanted| wanted.as_str() == *channel))
            .collect()
    };
    let name = |key: &str| -> Result<Option<String>, Error> {
        match given.get(key).filter(|value| !value.is_null()) {
            None => Ok(None),
            Some(value) => value
                .as_str()
                .filter(|name| {
                    !name.is_empty() && name.len() <= 64 && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                })
                .map(|name| Some(name.to_owned()))
                .ok_or_else(|| fail(format!("alert audience {key} is not a name"))),
        }
    };
    let template = name("emailTemplateId")?;
    if chosen.contains(&"email") {
        let Some(template) = template.as_deref() else {
            return Err(fail("an alert audience told by email needs an emailTemplateId"));
        };
        if !exists(ec, "moqui.basic.email.EmailTemplate", "emailTemplateId", template)? {
            return Err(fail(format!("email template {template} does not exist")));
        }
    }
    let topic = name("topic")?.unwrap_or_else(|| DEFAULT_TOPIC.to_owned());
    if !exists(ec, "moqui.security.user.NotificationTopic", "topic", &topic)? {
        return Err(fail(format!("notification topic {topic} does not exist")));
    }
    let seconds = |key: &str, low: i64, high: i64| -> Result<Json, Error> {
        match given.get(key).filter(|value| !value.is_null()) {
            None => Ok(Json::Null),
            Some(value) => value
                .as_i64()
                .filter(|seconds| (low..=high).contains(seconds))
                .map(Json::from)
                .ok_or_else(|| fail(format!("alert audience {key} must be {low} to {high} seconds"))),
        }
    };
    let group_by = ids("groupBy")?;
    let cap = match given.get("recipientCap").filter(|value| !value.is_null()) {
        None => Json::Null,
        Some(value) => value
            .as_i64()
            .filter(|cap| *cap > 0)
            .map(Json::from)
            .ok_or_else(|| fail("alert audience recipientCap must be a positive whole number"))?,
    };
    let viewer = delivery::viewer()?;
    let personal = groups.is_empty() && (users.is_empty() || users == [viewer]);
    let mut audience = json!({
        "recipientUserIds": users, "recipientGroupIds": groups, "channels": chosen, "topic": topic,
        "deliveryMode": if personal { "personal" } else { "group" },
    });
    for (key, value) in [
        ("emailTemplateId", template.map_or(Json::Null, Json::from)),
        ("recipientCap", cap),
        ("groupWaitSeconds", seconds("groupWaitSeconds", 0, 86_400)?),
        ("repeatSeconds", seconds("repeatSeconds", 60, 2_592_000)?),
        (
            "groupBy",
            if given.get("groupBy").is_some_and(Json::is_array) {
                json!(group_by)
            } else {
                Json::Null
            },
        ),
    ] {
        if !value.is_null() {
            audience[key] = value;
        }
    }
    body["audience"] = audience;
    Ok(true)
}

/// A copy of a rule, or an imported one, over business data: it keeps no contact point and notifies nobody until its
/// owner activates it (a save with `paused` false, which checks the owner's sources and every recipient).
pub(super) fn prepare_copy(ec: &ExecutionContext, body: &mut Json) {
    if !body.is_object() || !body_business(ec, body) {
        return;
    }
    if let Some(map) = body.as_object_mut() {
        map.remove("contactPoints");
        map.remove("contactPointRefs");
    }
    body["paused"] = json!(true);
    body["pausedReason"] = json!("not-validated");
}

/// After a business rule's row is written, in the save's transaction: the audience checked as `check#ReportAudience`
/// does, against the rule as it now stands. An active rule with a recipient who may not be told the owner's state, or
/// more recipients than the cap, is an error, which rolls the save back; a paused one may be saved to be mended.
pub(super) fn check_saved(ec: &ExecutionContext, saved: &Json) -> Result<(), Error> {
    let facade = delivery::entity(ec)?;
    let id = jtext(saved, "id", "");
    let body = &saved["body"];
    let now = moqui_core::user::now_millis();
    let owner =
        delivery::account(ec, &facade, &delivery::viewer()?, now)?.ok_or_else(|| fail("the viewer has no account"))?;
    let (users, groups) = named(body);
    let audience = delivery::audience(
        ec,
        &owner,
        id,
        &users,
        &groups,
        body["audience"]["recipientCap"].as_i64(),
    )?;
    if body["paused"] != true {
        // an address is needed only where email is a channel
        delivery::refuse(&audience, "alert", channels(body).contains(&"email"))?;
    }
    // one who left the audience keeps no delivery row
    let kept: BTreeSet<&str> = audience
        .verdicts
        .iter()
        .map(|verdict| verdict.recipient.account.user_id.as_str())
        .collect();
    let _authz = ec.disable_authz();
    for row in facade
        .find(DELIVERY)?
        .use_cache(false)
        .eq("definitionId", Value::string(id))
        .list()?
        .iter()
        .filter(|row| !kept.contains(delivery::field(row, "userId")))
    {
        row.delete(&facade)?;
    }
    tracing::info!(target: "moqui.report.audit", user = %owner.user_id, rule = %id, mode = audience.mode,
        recipients = audience.verdicts.len(), paused = body["paused"] == true, "business alert rule stored");
    Ok(())
}

/// A business rule's body as the evaluator delivers it: one contact a channel of its audience, in place of any contact
/// point or shared contact reference its row holds (a row written past `save#Definition` gains no address by it).
pub(super) fn contact_body(body: &Json) -> Json {
    let mut effective = body.clone();
    if let Some(map) = effective.as_object_mut() {
        map.remove("contactPointRefs");
    }
    let audience = &body["audience"];
    let contacts: Vec<Json> = channels(body)
        .into_iter()
        .map(|channel| {
            let mut contact = json!({"id": format!("audience:{channel}"), "type": channel});
            for key in ["groupBy", "groupWaitSeconds", "repeatSeconds"] {
                if !audience[key].is_null() {
                    contact[key] = audience[key].clone();
                }
            }
            contact
        })
        .collect();
    effective["contactPoints"] = json!(contacts);
    effective
}

/// Whether a business dataset the rule's query names has an index older than its fields.
pub(super) fn stale_source(ec: &ExecutionContext, body: &Json) -> bool {
    fn datasets(value: &Json, out: &mut BTreeSet<String>) {
        match value {
            Json::Object(map) => {
                for (key, value) in map {
                    match value.as_str() {
                        Some(id) if key == "datasetId" => {
                            out.insert(id.to_owned());
                        }
                        _ => datasets(value, out),
                    }
                }
            }
            Json::Array(items) => {
                for item in items {
                    datasets(item, out);
                }
            }
            _ => {}
        }
    }
    let mut ids = BTreeSet::new();
    datasets(&body["query"], &mut ids);
    ids.iter().any(|id| crate::business::index_stale(ec, id))
}

/// Pause a business rule whose owner cannot sign in, until a transfer hands it on.
pub(super) fn pause_for_owner(ec: &ExecutionContext, id: &str) {
    let paused = delivery::entity(ec).and_then(|facade| {
        facade
            .transactions_for(delivery::DEFINITION)?
            .run(TransactionAttribute::ForceNew, None, || {
                definitions::engine_revise(ec, id, "Paused: its owner is not active", "system", |body| {
                    body["paused"] = json!(true);
                    body["pausedReason"] = json!("owner-disabled");
                    None
                })
            })
    });
    match paused {
        Ok(_) => {
            tracing::info!(target: "moqui.report.audit", rule = %id, "business alert rule paused: its owner is not active")
        }
        Err(error) => {
            tracing::error!(rule = %id, %error, "could not pause a business alert rule whose owner is not active")
        }
    }
}

// ===== a notification to each recipient =====

struct Resolved {
    owner: Account,
    owner_scope: Scope,
    /// The owner first, so a cap never leaves out the one whose reading it is; then in user id order.
    recipients: Vec<Account>,
    may_distribute: bool,
    cap: usize,
}

/// One evaluation's audience: resolved at the first notification that is due and used for every group of the pass, so
/// each recipient's scope is computed once a pass.
pub(super) struct Pass<'a> {
    ec: &'a ExecutionContext,
    definition: &'a Json,
    resolved: OnceCell<Result<Resolved, String>>,
    verdicts: RefCell<BTreeMap<String, (&'static str, String)>>,
}

impl<'a> Pass<'a> {
    pub(super) fn new(ec: &'a ExecutionContext, definition: &'a Json) -> Self {
        Pass {
            ec,
            definition,
            resolved: OnceCell::new(),
            verdicts: RefCell::new(BTreeMap::new()),
        }
    }

    fn resolve(&self) -> Result<Resolved, Error> {
        let facade = delivery::entity(self.ec)?;
        let id = jtext(self.definition, "id", "");
        let wall = moqui_core::user::now_millis();
        let owner = delivery::account(self.ec, &facade, jtext(self.definition, "ownerId", ""), wall)?
            .ok_or_else(|| fail("alert owner account no longer exists"))?;
        let (mut users, groups) = named(&self.definition["body"]);
        if users.is_empty() && groups.is_empty() {
            users.push(owner.user_id.clone());
        }
        let mut recipients: Vec<Account> = delivery::expand(self.ec, &facade, &users, &groups, wall)?
            .into_iter()
            .map(|recipient| recipient.account)
            .collect();
        recipients.sort_by_key(|recipient| (recipient.user_id != owner.user_id, recipient.user_id.clone()));
        let owner_scope = delivery::scope_as(self.ec, &owner.username, id)?;
        let others = recipients.iter().any(|recipient| recipient.user_id != owner.user_id);
        let may_distribute =
            !others || delivery::holds_as(self.ec, &owner.username, delivery::GROUP_ARTIFACT, "create");
        let (cap, _) = delivery::caps(self.definition["body"]["audience"]["recipientCap"].as_i64());
        Ok(Resolved {
            owner,
            owner_scope,
            recipients,
            may_distribute,
            cap: usize::try_from(cap).unwrap_or(usize::MAX),
        })
    }

    fn resolved(&self) -> Result<&Resolved, Error> {
        self.resolved
            .get_or_init(|| self.resolve().map_err(|error| error.to_string()))
            .as_ref()
            .map_err(fail)
    }

    /// Why this recipient may not be told the owner's state ("" when they may), and their fingerprint.
    fn verdict(&self, resolved: &Resolved, recipient: &Account) -> (&'static str, String) {
        if let Some(known) = self.verdicts.borrow().get(&recipient.user_id) {
            return known.clone();
        }
        let verdict = delivery::judge(
            self.ec,
            jtext(self.definition, "id", ""),
            &resolved.owner,
            &resolved.owner_scope,
            recipient,
        );
        self.verdicts
            .borrow_mut()
            .insert(recipient.user_id.clone(), verdict.clone());
        verdict
    }

    /// Tell one notification group's state to each recipient on one channel, each checked first and told separately.
    /// One already told this state within the repeat interval is passed over; one who may not be told is skipped with
    /// the reason; one whose message fails is recorded and the others go on. An error when a recipient failed, so the
    /// group is tried again at the next pass.
    pub(super) fn send(
        &self,
        contact: &Json,
        payload: &Json,
        group: &str,
        hash: &str,
        now: i64,
        repeat: i64,
    ) -> Result<(), Error> {
        let resolved = self.resolved()?;
        let facade = delivery::entity(self.ec)?;
        let id = jtext(self.definition, "id", "");
        let audience = &self.definition["body"]["audience"];
        let channel = jtext(contact, "type", "notification");
        let states: BTreeSet<&str> = payload["events"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|event| event["state"].as_str())
            .collect();
        let state = states.into_iter().collect::<Vec<_>>().join(", ");
        let key = Key {
            id,
            channel,
            group,
            hash,
            state: &state,
        };
        let (mut told, mut failed) = (0_usize, 0_usize);
        for (position, recipient) in resolved.recipients.iter().enumerate() {
            let standing = standing(self.ec, &facade, &key, &recipient.user_id)?;
            if standing.as_ref().is_some_and(|row| {
                delivery::field(row, "statusId") == delivery::SENT
                    && delivery::field(row, "stateHash") == hash
                    && now.saturating_sub(delivery::millis(row, "attemptDate").as_i64().unwrap_or(0)) < repeat
            }) {
                continue;
            }
            let (status, reason, scope_hash) = if position >= resolved.cap {
                (delivery::SKIPPED, "over-cap", String::new())
            } else if recipient.user_id != resolved.owner.user_id && !resolved.may_distribute {
                (delivery::SKIPPED, "owner-unauthorized", String::new())
            } else {
                let (reason, scope_hash) = self.verdict(resolved, recipient);
                // the in-app notification needs no address
                let reason = if reason == "no-address" && channel != "email" {
                    ""
                } else {
                    reason
                };
                if !reason.is_empty() {
                    (delivery::SKIPPED, reason, String::new())
                } else {
                    let target = if channel == "email" {
                        json!({"type": "email", "emailTemplateId": audience["emailTemplateId"],
                            "toAddresses": recipient.address})
                    } else {
                        // to this one user: nobody receives it by a subscription to the whole topic
                        json!({"type": "notification", "topic": jtext(audience, "topic", DEFAULT_TOPIC),
                            "userId": recipient.user_id, "direct": true})
                    };
                    match crate::alert::send_contact(&target, payload) {
                        Ok(()) => {
                            told += 1;
                            (delivery::SENT, "", scope_hash)
                        }
                        Err(error) => {
                            tracing::error!(rule = id, recipient = %recipient.user_id, channel, %error,
                                "alert notification to one recipient failed; the others go on");
                            failed += 1;
                            (delivery::FAILED, "transport-failure", scope_hash)
                        }
                    }
                }
            };
            if let Err(error) = record(
                self.ec,
                &facade,
                &key,
                standing,
                recipient,
                status,
                reason,
                &scope_hash,
                now,
            ) {
                tracing::error!(rule = id, recipient = %recipient.user_id, %error, "alert delivery could not be recorded");
            }
        }
        if told > 0 {
            tracing::info!(target: "moqui.report.audit", rule = %id, channel, told,
                considered = resolved.recipients.len(), "business alert told to each recipient separately");
        }
        if failed > 0 {
            return Err(fail(format!(
                "{failed} alert recipients were not notified; each delivery says why"
            )));
        }
        Ok(())
    }
}

/// One notification group's state on one channel of one rule.
struct Key<'a> {
    id: &'a str,
    channel: &'a str,
    group: &'a str,
    hash: &'a str,
    state: &'a str,
}

fn standing(
    ec: &ExecutionContext,
    facade: &EntityFacade,
    key: &Key<'_>,
    user_id: &str,
) -> Result<Option<EntityValue>, Error> {
    let _authz = ec.disable_authz();
    facade
        .find(DELIVERY)?
        .use_cache(false)
        .eq("definitionId", Value::string(key.id))
        .eq("channel", Value::string(key.channel))
        .eq("groupId", Value::string(key.group))
        .eq("userId", Value::string(user_id))
        .one()
}

/// A recipient's last outcome for a group on a channel, in a transaction of its own: evidence without a value.
#[allow(clippy::too_many_arguments)]
fn record(
    ec: &ExecutionContext,
    facade: &EntityFacade,
    key: &Key<'_>,
    standing: Option<EntityValue>,
    recipient: &Account,
    status: &str,
    reason: &str,
    scope_hash: &str,
    now: i64,
) -> Result<(), Error> {
    delivery::write(ec, facade, || {
        let creating = standing.is_none();
        let mut row = match standing {
            Some(row) => row,
            None => {
                let mut row = facade.make_value(DELIVERY)?;
                row.set_sequenced_id_primary(facade)?;
                row.set("definitionId", Value::string(key.id));
                row.set("channel", Value::string(key.channel));
                row.set("groupId", Value::string(key.group));
                row.set("userId", Value::string(&recipient.user_id));
                row
            }
        };
        row.set("stateHash", Value::string(key.hash));
        // the state's name is part of the owner's reading: kept for one it was sent to (or would have been, but for the
        // transport), never for one skipped, who reads their own row (find#AlertDeliveries received)
        row.set(
            "alertState",
            Value::string(if status == delivery::SKIPPED { "" } else { key.state }),
        );
        row.set("statusId", Value::string(status));
        row.set("reasonCode", Value::string(reason));
        row.set("scopeHash", Value::string(scope_hash));
        row.set("attemptDate", Value::I64(now));
        row.set(
            "attemptCount",
            Value::I64(delivery::number(row.get("attemptCount")).unwrap_or(0) + 1),
        );
        if creating {
            row.create(facade)
        } else {
            row.update(facade)
        }
    })
}

// ===== reads =====

fn delivery_json(row: &EntityValue) -> Json {
    let reason = delivery::field(row, "reasonCode");
    json!({
        "deliveryId": delivery::field(row, "deliveryId"), "definitionId": delivery::field(row, "definitionId"),
        "channel": delivery::field(row, "channel"), "seriesGroupId": delivery::field(row, "groupId"),
        "userId": delivery::field(row, "userId"), "statusId": delivery::field(row, "statusId"),
        "alertState": delivery::field(row, "alertState"), "reasonCode": reason,
        "reason": delivery::reason_text(reason), "attemptDate": delivery::millis(row, "attemptDate"),
        "attemptCount": delivery::number(row.get("attemptCount")).unwrap_or(0),
    })
}

/// The saved alert rules, whoever owns them, as id, owner and body: the engine's own list.
fn rules(ec: &ExecutionContext, facade: &EntityFacade, owner: Option<&str>) -> Result<Vec<Json>, Error> {
    let _authz = ec.disable_authz();
    let mut find = facade
        .find(delivery::DEFINITION)?
        .use_cache(false)
        .eq("definitionType", Value::string("alert"));
    if let Some(owner) = owner {
        find = find.eq("ownerId", Value::string(owner));
    }
    let mut out = Vec::new();
    for row in find.order_by("definitionId", false).list()?.iter() {
        match definitions::row_json(row) {
            Ok(definition) => out.push(definition),
            Err(error) => {
                tracing::error!(rule = ?row.get("definitionId"), %error, "alert rule could not be read, continuing")
            }
        }
    }
    Ok(out)
}

/// `find#AlertDeliveries`.
fn find_alert_deliveries(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let user = delivery::viewer()?;
    let facade = delivery::entity(ec)?;
    let size = delivery::number(p.get("pageSize")).unwrap_or(50).clamp(1, 200);
    let index = delivery::number(p.get("pageIndex"))
        .unwrap_or(0)
        .clamp(0, 10_000 / size);
    let wanted = delivery::text(p, "definitionId");
    let page = |total: usize, deliveries: Vec<Json>| {
        delivery::out(json!({"deliveries": deliveries, "total": total, "pageIndex": index, "pageSize": size}))
    };
    let (skip, take) = (
        usize::try_from(index * size).unwrap_or(0),
        usize::try_from(size).unwrap_or(50),
    );
    if delivery::flag(p, "administrative") == Some(true) {
        if !moqui_core::user::has_permission(delivery::ADMIN_PERMISSION)? {
            return Err(fail(format!(
                "{} is required to list every owner's alert deliveries",
                delivery::ADMIN_PERMISSION
            )));
        }
        // every owner's business rules as states and counters: no title, no series, no recipient and no value
        let listed: Vec<Json> = rules(ec, &facade, None)?
            .into_iter()
            .filter(|rule| is_business(ec, rule) && (wanted.is_empty() || jtext(rule, "id", "") == wanted))
            .collect();
        let _authz = ec.disable_authz();
        let mut rows = Vec::new();
        for rule in listed.iter().skip(skip).take(take) {
            let id = jtext(rule, "id", "");
            let count = |status: &str| -> Result<i64, Error> {
                facade
                    .find(DELIVERY)?
                    .use_cache(false)
                    .eq("definitionId", Value::string(id))
                    .eq("statusId", Value::string(status))
                    .count()
            };
            let lock = facade
                .find(RULE_LOCK)?
                .use_cache(false)
                .eq("definitionId", Value::string(id))
                .one()?;
            rows.push(json!({
                "definitionId": id, "ownerUserId": rule["ownerId"], "paused": rule["body"]["paused"] == true,
                "pausedReason": jtext(&rule["body"], "pausedReason", ""),
                "deliveryMode": jtext(&rule["body"]["audience"], "deliveryMode", "personal"),
                "sentCount": count(delivery::SENT)?, "skippedCount": count(delivery::SKIPPED)?,
                "failedCount": count(delivery::FAILED)?,
                "lastEvaluated": lock.as_ref().map_or(Json::Null, |lock| delivery::millis(lock, "lastEvaluated")),
            }));
        }
        return Ok(page(listed.len(), rows));
    }
    let _authz = ec.disable_authz();
    if delivery::flag(p, "received") == Some(true) {
        // the deliveries to the viewer: their own outcome, and the rule's title where they read it
        let mut find = facade
            .find(DELIVERY)?
            .use_cache(false)
            .eq("userId", Value::string(&user));
        if !wanted.is_empty() {
            find = find.eq("definitionId", Value::string(wanted));
        }
        let rows: Vec<EntityValue> = find
            .order_by("attemptDate", true)
            .order_by("deliveryId", true)
            .list()?
            .iter()
            .cloned()
            .collect();
        let deliveries = rows
            .iter()
            .skip(skip)
            .take(take)
            .map(|row| {
                let mut json = delivery_json(row);
                json["title"] = definitions::read(ec, delivery::field(row, "definitionId"))
                    .ok()
                    .map_or(Json::Null, |definition| definition["title"].clone());
                json
            })
            .collect();
        return Ok(page(rows.len(), deliveries));
    }
    // the rules the viewer owns: each recipient's last outcome, by name
    let now = moqui_core::user::now_millis();
    let mut deliveries = Vec::new();
    for rule in rules(ec, &facade, Some(&user))? {
        let id = jtext(&rule, "id", "");
        if !wanted.is_empty() && id != wanted {
            continue;
        }
        for row in facade
            .find(DELIVERY)?
            .use_cache(false)
            .eq("definitionId", Value::string(id))
            .order_by("userId", false)
            .order_by("deliveryId", false)
            .list()?
            .iter()
        {
            let mut json = delivery_json(row);
            json["title"] = rule["title"].clone();
            if let Some(recipient) = delivery::account(ec, &facade, delivery::field(row, "userId"), now)? {
                json["username"] = json!(recipient.username);
                json["userFullName"] = json!(recipient.name);
            }
            deliveries.push(json);
        }
    }
    let total = deliveries.len();
    Ok(page(total, deliveries.into_iter().skip(skip).take(take).collect()))
}

/// `get#AlertState`.
fn get_alert_state(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let user = delivery::viewer()?;
    let facade = delivery::entity(ec)?;
    let id = delivery::text(p, "definitionId");
    let definition = definitions::read(ec, id)?;
    if definition["type"] != "alert" {
        return Err(fail("definition is not an alert rule"));
    }
    let body = &definition["body"];
    let business = is_business(ec, &definition);
    let own = jtext(&definition, "ownerId", "") == user && definition["readOnly"] != true;
    // a business rule's state is its owner's reading: the owner's, and a recipient's whose scope is the owner's now. Any
    // other rule's state is for one who reads the rule and every source it names.
    let reason = if own || definition["readOnly"] == true {
        ""
    } else if !business {
        let username = moqui_core::user::current_username()?.unwrap_or_default();
        match delivery::scope_as(ec, &username, id) {
            Ok(scope) if !scope.denied => "",
            _ => "denied-source",
        }
    } else {
        let now = moqui_core::user::now_millis();
        let (users, groups) = named(body);
        let recipient = delivery::expand(ec, &facade, &users, &groups, now)?
            .into_iter()
            .find(|recipient| recipient.account.user_id == user);
        let owner = delivery::account(ec, &facade, jtext(&definition, "ownerId", ""), now)?;
        match (recipient, owner) {
            (None, _) => "not-recipient",
            (_, None) => "inactive",
            (Some(recipient), Some(owner)) => match delivery::scope_as(ec, &owner.username, id) {
                Err(_) => "owner-inactive",
                Ok(owner_scope) => match delivery::judge(ec, id, &owner, &owner_scope, &recipient.account).0 {
                    "no-address" => "",
                    reason => reason,
                },
            },
        }
    };
    let _authz = ec.disable_authz();
    let mut instances = Vec::new();
    if reason.is_empty() && definition["readOnly"] != true {
        for row in facade
            .find(INSTANCE)?
            .use_cache(false)
            .eq("definitionId", Value::string(id))
            .order_by("seriesId", false)
            .list()?
            .iter()
        {
            instances.push(json!({
                "seriesId": delivery::field(row, "seriesId"), "state": delivery::field(row, "state"),
                "labels": serde_json::from_str::<Json>(delivery::field(row, "labelsJson")).unwrap_or_else(|_| json!({})),
                "stateSince": delivery::millis(row, "stateSince"), "lastEvaluated": delivery::millis(row, "lastEvaluated"),
                "value": row.get("lastValue").map_or(Json::Null, moqui_lib::json::to_json),
            }));
        }
    }
    let lock = facade
        .find(RULE_LOCK)?
        .use_cache(false)
        .eq("definitionId", Value::string(id))
        .one()?;
    let words = match reason {
        "not-recipient" => "You are not in this alert's audience",
        "denied-source" => "You may not read one of this alert's sources",
        reason => delivery::reason_text(reason),
    };
    Ok(delivery::out(json!({
        "definitionId": id, "title": definition["title"], "own": own, "business": business,
        "template": definition["readOnly"] == true, "paused": body["paused"] == true,
        "pausedReason": jtext(body, "pausedReason", ""),
        "deliveryMode": if business { jtext(&body["audience"], "deliveryMode", "personal") } else { "" },
        "available": reason.is_empty(), "reasonCode": reason,
        "reason": words,
        "instances": instances,
        "lastEvaluated": lock.as_ref().map_or(Json::Null, |lock| delivery::millis(lock, "lastEvaluated")),
    })))
}

// ===== ownership: the lifecycle participant =====

/// `find#OwnedAlertRules`: names only.
fn find_owned_alert_rules(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let user_id = delivery::text(p, "userId");
    delivery::owner_authority(user_id)?;
    let facade = delivery::entity(ec)?;
    let resources: Vec<Json> = rules(ec, &facade, Some(user_id))?
        .iter()
        .map(|rule| {
            let body = &rule["body"];
            let note = match (body["paused"] == true, jtext(body, "pausedReason", "")) {
                (true, "owner-disabled") => "Paused until a new owner takes it",
                (true, "transfer-pending") => "Paused until its new owner resumes it",
                (true, _) => "Paused",
                _ => match jtext(&body["audience"], "deliveryMode", "") {
                    "group" => "Notifies a group",
                    _ => "Notifies its owner",
                },
            };
            json!({"resourceId": rule["id"], "resourceKind": "alert", "transferable": true, "note": note,
                "resourceName": format!("Alert {}", jtext(rule, "title", jtext(rule, "id", "")))})
        })
        .collect();
    Ok(delivery::out(json!({"resourceList": resources})))
}

/// `pause#OwnedAlertRules`.
fn pause_owned_alert_rules(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let user_id = delivery::text(p, "userId");
    let actor = delivery::owner_authority(user_id)?;
    let facade = delivery::entity(ec)?;
    let mut paused = 0;
    for rule in rules(ec, &facade, Some(user_id))? {
        let body = &rule["body"];
        if body["paused"] == true && !jtext(body, "pausedReason", "").is_empty() {
            continue;
        }
        let id = jtext(&rule, "id", "");
        // one rule that cannot be paused is named and the others are still paused
        let revised = definitions::engine_revise(ec, id, "Paused: its owner's access ended", &actor, |body| {
            body["paused"] = json!(true);
            body["pausedReason"] = json!("owner-disabled");
            None
        });
        match revised {
            Ok(_) => paused += 1,
            Err(error) => tracing::error!(rule = %id, %error, "could not pause an owned alert rule"),
        }
    }
    tracing::info!(target: "moqui.report.audit", user = %actor, owner = %user_id, paused,
        reason = %delivery::text(p, "reason"), "owned alert rules paused");
    Ok(delivery::out(json!({"pausedCount": paused})))
}

/// `transfer#AlertRuleOwner`.
fn transfer_alert_rule_owner(ec: &ExecutionContext, _: &ServiceInfo<'_>, p: &Parameters) -> Result<Parameters, Error> {
    let (from, to, id) = (
        delivery::text(p, "fromUserId"),
        delivery::text(p, "toUserId"),
        delivery::text(p, "definitionId"),
    );
    let actor = delivery::owner_authority(from)?;
    let facade = delivery::entity(ec)?;
    let now = moqui_core::user::now_millis();
    let rule = rules(ec, &facade, Some(from))?
        .into_iter()
        .find(|rule| jtext(rule, "id", "") == id)
        .ok_or_else(|| fail("alert rule not found for this owner"))?;
    let new_owner = delivery::account(ec, &facade, to, now)?
        .filter(|account| account.active && !moqui_core::user::is_external_user(&account.user_id))
        .ok_or_else(|| fail("NEW_OWNER_REFUSED: the new owner is not an active internal user"))?;
    if from == to {
        return Err(fail("NEW_OWNER_REFUSED: the new owner already owns the alert rule"));
    }
    if !delivery::holds_as(ec, &new_owner.username, SAVE_SERVICE, "all") {
        return Err(fail("NEW_OWNER_REFUSED: the new owner may not save alert rules"));
    }
    if jtext(&rule["body"]["audience"], "deliveryMode", "") == "group"
        && !delivery::holds_as(ec, &new_owner.username, delivery::GROUP_ARTIFACT, "create")
    {
        return Err(fail(
            "NEW_OWNER_REFUSED: the new owner may not notify other users of an alert",
        ));
    }
    // the shared silences and mute timings it names that the leaving owner owns go with it, so the new owner reads them
    let mut moved = Vec::new();
    for (_, key, _) in crate::alert::SHARED {
        let references = rule["body"][key].as_array().into_iter().flatten();
        for reference in references.filter_map(Json::as_str) {
            let owned = definitions::read_unchecked(ec, reference)?
                .is_some_and(|shared| jtext(&shared, "ownerId", "") == from && shared["readOnly"] != true);
            if owned {
                definitions::engine_revise(ec, reference, "Handed to a new owner", &actor, |_| Some(to.to_owned()))?;
                moved.push(reference.to_owned());
            }
        }
    }
    // paused until the new owner resumes it, which checks the audience against the new owner's scope
    definitions::engine_revise(ec, id, "Handed to a new owner", &actor, |body| {
        body["paused"] = json!(true);
        body["pausedReason"] = json!("transfer-pending");
        Some(to.to_owned())
    })?;
    moved.push(id.to_owned());
    // The state its series hold (labels and last values) and what its notification groups were sent are the leaving
    // owner's readings, not the new owner's to read (get#AlertState) or to have told on as No data: the rule starts
    // again from nothing. An evaluation still running as the leaving owner loses its claim, so it saves nothing.
    {
        let _authz = ec.disable_authz();
        for dependent in [INSTANCE, "moqui.report.AlertNotification"] {
            facade
                .find(dependent)?
                .eq("definitionId", Value::string(id))
                .delete_all()?;
        }
        if let Some(mut lock) = facade
            .find(RULE_LOCK)?
            .eq("definitionId", Value::string(id))
            .for_update(true)
            .one()?
        {
            lock.set("claimToken", Value::Null);
            lock.set("claimedUntil", Value::Null);
            lock.update(&facade)?;
        }
    }
    // qualified: the new owner reads the rule and every source it names (an error rolls the moves back)
    if delivery::scope_as(ec, &new_owner.username, id)?.denied {
        return Err(fail(
            "NEW_OWNER_REFUSED: the new owner may not read this alert rule or one of its sources",
        ));
    }
    tracing::info!(target: "moqui.report.audit", user = %actor, rule = %id, from = %from, to = %to,
        definitions = moved.len(), reason = %delivery::text(p, "reason"), "alert rule transferred to a new owner");
    Ok(delivery::out(json!({"transferred": true, "definitionIds": moved})))
}

pub fn register(registry: &mut NativeRegistry) -> Result<(), moqui_core::context::Error> {
    for (name, function) in [
        (
            "find_alert_deliveries",
            find_alert_deliveries as moqui_core::native::NativeService,
        ),
        ("get_alert_state", get_alert_state),
        ("find_owned_alert_rules", find_owned_alert_rules),
        ("pause_owned_alert_rules", pause_owned_alert_rules),
        ("transfer_alert_rule_owner", transfer_alert_rule_owner),
    ] {
        registry.service_function(&format!("moqui_core::report_alert::{name}"), function)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "alert_delivery_tests.rs"]
mod tests;
