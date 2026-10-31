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

//! The sim preview of a write (plans/LLM-A2A-Assist.md D4, 4.8): on demand from the card's Preview effects (owner
//! only, [`preview_approval`]), or before the card for a tool configured to (`previewMode` always, and DESTRUCTIVE
//! calls unless their exposure turns it off). On a thread of its own, logged in as the user, the write runs in the sim
//! overlay ([`moqui_core::entity::cache::enter_sim`]): a held screen write sent again with its approval's grant, a service
//! called with its parameters. The rows it wrote ([`moqui_core::entity::cache::sim_changes`]) are compared with production
//! after the sim is left, field by field, an encrypted or secret-named field shown as changed but never its value. The
//! write runs under the sim fence ([`fence::enter_sim`]): only entity writes go on, into the overlay; a resource write, a
//! notification, mail, a remote call, a print or a write of the agent's own records (which a sim leaves on production)
//! is refused, so the preview reaches nothing outside the sim. A write that fails, one that would do any of those, or
//! a sim poisoned by a rolled-back transaction that wrote, is "failed in preview". Only a build with the overlay (the embedded Postgres of `crates/moqui-host/src/db/
//! sim.rs`) can preview ([`available`]); elsewhere the card has no Preview effects.

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};
use serde_json::{json, Value as Json};

use super::{load_for_owner, DecideError, TargetKind, APPROVAL};
use crate::run::{self, in_tx, quietly};
use crate::screen::PendingWrite;
use moqui_core::context::ExecutionContextFactory;
use moqui_core::entity::cache::{self, SimChange};
use moqui_core::entity::{fence, EntityFacade};
use moqui_core::screen::{RenderOrigin, TransitionGate};

/// The most changed rows a preview lists.
pub(super) const MAX_ROWS: usize = 50;

/// Whether this process can preview: it has the sim overlay, or can start it.
pub fn available(facade: &EntityFacade) -> bool {
    facade.sim_available()
}

/// What a preview runs.
pub(crate) struct PreviewTarget<'a> {
    pub username: &'a str,
    pub agent_id: &'a str,
    pub run_id: Option<&'a str>,
    pub approval_id: &'a str,
    pub kind: TargetKind,
    pub name: &'a str,
    pub args: &'a Json,
}

/// Run `target` in the sim and answer what it would change: `{ok, rows: [{entity, key, action, fields: [{field,
/// before, after}]}], more}`, or `{ok: false, error}` when it failed in the preview. Never an error: a preview that
/// cannot run says why.
pub(crate) fn run_target(factory: &Arc<ExecutionContextFactory>, target: &PreviewTarget<'_>) -> Json {
    let outcome = std::thread::scope(|scope| {
        scope
            .spawn(|| in_sim(factory, target))
            .join()
            .unwrap_or_else(|_| Err(Error::new("the preview failed unexpectedly")))
    });
    match outcome {
        Ok(preview) => preview,
        Err(error) => json!({"ok": false, "error": format!("failed in preview: {error}")}),
    }
}

fn in_sim(factory: &Arc<ExecutionContextFactory>, target: &PreviewTarget<'_>) -> Result<Json, Error> {
    let label = format!("the preview of approval {}", target.approval_id);
    let run = sim_body(factory, target.username, &label, || run_write(factory, target))?;
    Ok(json!({"ok": true, "rows": run.rows, "more": run.more}))
}

/// What a body run in the sim came to: its value, and the rows it would write against production (at most
/// [`MAX_ROWS`], `more` when there were more).
#[derive(Debug)]
pub struct SimRun<T> {
    pub value: T,
    pub rows: Vec<Json>,
    pub more: bool,
}

/// Run `body` in the sim overlay on a thread of its own, logged in as `username`, as an approval's preview runs a write
/// (the dev-aide's Try in sim, plans/design-dev-aide.md 9.4): entity writes go into the overlay and are answered as rows
/// against production, and an outside effect (a resource write, mail, a notification, a remote call) is refused.
///
/// # Errors
///
/// Returns the body's error, an outside effect it tried, or why the sim could not run.
pub fn run_in_sim<T: Send>(
    factory: &Arc<ExecutionContextFactory>,
    username: &str,
    label: &str,
    body: impl FnOnce() -> Result<T, Error> + Send,
) -> Result<SimRun<T>, Error> {
    std::thread::scope(|scope| {
        scope
            .spawn(|| sim_body(factory, username, label, body))
            .join()
            .unwrap_or_else(|_| Err(Error::new("the sim run failed unexpectedly")))
    })
}

/// Leaves the sim, its schema dropped, when dropped: at the end of a sim run, and also when the run panics.
pub(super) struct LeaveSim<'a> {
    pub(super) facade: &'a EntityFacade,
    pub(super) label: &'a str,
}

impl Drop for LeaveSim<'_> {
    fn drop(&mut self) {
        if let Err(error) = cache::exit_sim(self.facade) {
            tracing::error!("the sim of {} was not dropped: {error}", self.label);
        }
    }
}

fn sim_body<T>(
    factory: &Arc<ExecutionContextFactory>,
    username: &str,
    label: &str,
    body: impl FnOnce() -> Result<T, Error>,
) -> Result<SimRun<T>, Error> {
    let context = factory.new_context();
    let _entered = context.enter();
    {
        let _authz = context.disable_authz();
        moqui_core::user::internal_login_user(username).map_err(|error| Error::new(error.to_string()))?;
    }
    let facade = run::facade_of(factory)?;
    cache::enter_sim(&facade)?;
    let left = LeaveSim { facade: &facade, label };
    // entity writes go into the overlay; a resource write, a notification, mail, a remote call or a print would be
    // real, so they are refused, and so is a lift of them by the approved transition's render
    let (ran, effects) = {
        let fence = fence::enter_sim(label.to_string());
        let ran = body();
        let effects: Vec<String> = fence.violations().iter().map(fence::Violation::describe).collect();
        (ran, effects)
    };
    let changes = cache::sim_changes(&facade);
    // A privileged service can write a row the caller cannot read. Its simulated after-values
    // need the same view check as production values, before leaving the overlay.
    let visible_after: Vec<bool> = match &changes {
        Ok(changes) => changes.iter().take(MAX_ROWS).map(|change| {
            change.values.is_none() || match production_row(&facade, change) {
                Ok(row) => row.is_some(),
                Err(error) => {
                    tracing::warn!(entity = %change.entity, error = %error, "preview after-values could not be authorized");
                    false
                }
            }
        }).collect(),
        Err(_) => Vec::new(),
    };
    let poisoned = cache::sim_poisoned();
    drop(left);
    if !effects.is_empty() {
        return Err(Error::new(format!(
            "it would do what a preview cannot simulate: {}",
            effects.join(", ")
        )));
    }
    let value = ran?;
    if let Some(reason) = poisoned {
        return Err(Error::new(reason));
    }
    let changes = changes?;
    let more = changes.len() > MAX_ROWS;
    let rows: Vec<Json> = changes
        .iter()
        .take(MAX_ROWS)
        .zip(visible_after)
        .map(|(change, visible)| {
            if visible {
                compared(&facade, change)
            } else {
                json!({"entity": change.entity, "action": "unavailable", "error": "values are not visible to the user"})
            }
        })
        .collect();
    Ok(SimRun { value, rows, more })
}

fn run_write(factory: &Arc<ExecutionContextFactory>, target: &PreviewTarget<'_>) -> Result<(), Error> {
    match target.kind {
        TargetKind::Screen => {
            let write: PendingWrite = serde_json::from_value(target.args.clone())
                .map_err(|error| Error::new(format!("the held write does not read: {error}")))?;
            let (path, query) = write.url.split_once('?').unwrap_or((&write.url, ""));
            let mut parameters: BTreeMap<String, Value> = query
                .split('&')
                .filter(|pair| !pair.is_empty())
                .map(|pair| {
                    let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
                    (
                        moqui_core::web::percent_decode(name),
                        Value::string(moqui_core::web::percent_decode(value)),
                    )
                })
                .collect();
            for (name, value) in &write.fields {
                parameters.insert(name.clone(), Value::string(value));
            }
            // as the agent's session sends it, so the gate's hash is the approved one
            let output = moqui_core::screen::make_render(factory)
                .screen_path(path)
                .method(&write.method)
                .parameters(parameters)
                .origin(RenderOrigin::Agent {
                    agent_id: target.agent_id.to_string(),
                    run_id: target.run_id.map(str::to_string),
                })
                .transition_gate(TransitionGate::Approved {
                    approval_id: target.approval_id.to_string(),
                    args_hash: write.args_hash.clone(),
                })
                .write_fence(true)
                .header("Accept", "application/json")
                .render()?;
            if output.status == 409 {
                return Err(Error::new("the write was held again, so it is not the one proposed"));
            }
            if output.status >= 400 {
                let body = String::from_utf8_lossy(&output.body)
                    .chars()
                    .take(300)
                    .collect::<String>();
                return Err(Error::new(format!("the screen answered {}: {body}", output.status)));
            }
            Ok(())
        }
        TargetKind::Service => {
            let parameters: BTreeMap<String, Value> = match moqui_lib::json::from_json(target.args) {
                Value::Map(map) => map.into_iter().collect(),
                _ => BTreeMap::new(),
            };
            moqui_core::service::call::sync_checked(
                target.name,
                &parameters,
                &moqui_core::service::call::CallOptions::default(),
            )
            .map(|_| ())
            .map_err(|error| Error::new(error.to_string()))
        }
        TargetKind::Tool | TargetKind::File => Err(Error::new("this kind of write has no preview")),
    }
}

/// One changed row against production: created, updated (the fields that differ) or deleted.
fn compared(facade: &EntityFacade, change: &SimChange) -> Json {
    match production_row(facade, change) {
        Ok(production) => compared_to(facade, change, production.as_ref()),
        Err(error) => {
            tracing::warn!(entity = %change.entity, error = %error, "preview production values could not be authorized");
            json!({"entity": change.entity, "action": "unavailable", "error": "production values could not be read"})
        }
    }
}

/// The key of a changed row as a JSON object of its fields.
pub(super) fn key_json(change: &SimChange) -> serde_json::Map<String, Json> {
    change
        .key
        .iter()
        .map(|(name, value)| (name.clone(), Json::String(value.clone())))
        .collect()
}

/// One changed row against its values `before` (none when it was not there): created, updated (the fields that differ)
/// or deleted, in the preview row shape `{entity, key, action, fields: [{field, before, after}]}`.
pub(super) fn compared_to(
    facade: &EntityFacade,
    change: &SimChange,
    production: Option<&BTreeMap<String, Value>>,
) -> Json {
    let key = key_json(change);
    if change.values.is_none() && production.is_none() {
        return json!({"entity": change.entity, "action": "unavailable", "error": "deleted row is not visible to the user"});
    }
    let text = |value: Option<&Value>| value.filter(|value| !value.is_null()).map(to_plain_string);
    // an encrypted or secret-named field is shown changed, never its value
    let hidden: Vec<String> = facade
        .with_entity(&change.entity, |definition, _, _| {
            Ok(definition
                .fields
                .iter()
                .filter(|field| field.encrypt || crate::tools::data::secret_name(&field.name))
                .map(|field| field.name.clone())
                .collect())
        })
        .unwrap_or_default();
    let is_hidden = |field: &str| hidden.iter().any(|name| name == field);
    let shown = |field: &str, value: Option<String>| {
        if is_hidden(field) {
            value.map(|_| "(hidden)".to_string())
        } else {
            value
        }
    };
    let skip = |field: &str| field == "lastUpdatedStamp";
    match (&change.values, &production) {
        (None, _) => json!({"entity": change.entity, "key": key, "action": "delete"}),
        (Some(values), None) => json!({"entity": change.entity, "key": key, "action": "create",
            "fields": values.iter().filter(|(field, value)| !skip(field) && !value.is_null())
                .map(|(field, value)| json!({"field": field, "after": shown(field, Some(to_plain_string(value)))}))
                .collect::<Vec<_>>()}),
        (Some(values), Some(before)) => {
            let fields: Vec<Json> = values
                .iter()
                .filter(|(field, _)| !skip(field))
                .filter_map(|(field, value)| {
                    let (old, new) = (text(before.get(field)), text(Some(value)));
                    (old != new)
                        .then(|| json!({"field": field, "before": shown(field, old), "after": shown(field, new)}))
                })
                .collect();
            json!({"entity": change.entity, "key": key, "action": "update", "fields": fields})
        }
    }
}

pub(super) fn production_row(
    facade: &EntityFacade,
    change: &SimChange,
) -> Result<Option<BTreeMap<String, Value>>, Error> {
    let mut find = facade.find(&change.entity)?;
    for (name, value) in &change.key {
        find = find.eq(name, Value::string(value));
    }
    Ok(find.one()?.map(|row| moqui_core::entity::string_map(row.fields())))
}

/// `preview#AgentApproval`: the effects of one of the user's Pending approvals, run in the sim and kept on the approval
/// (`previewJson`); anyone else's is not found.
///
/// # Errors
///
/// [`DecideError::NotFound`] for another user's approval or none, [`DecideError::Refused`] when it is decided already
/// or the server cannot preview.
pub fn preview_approval(
    factory: &Arc<ExecutionContextFactory>,
    approval_id: &str,
    user_id: &str,
) -> Result<Json, DecideError> {
    let facade = run::facade_of(factory)?;
    let view = quietly(|| load_for_owner(&facade, approval_id, user_id))?.ok_or(DecideError::NotFound)?;
    if view.status != super::ApprovalStatus::Pending {
        return Err(DecideError::Refused(format!(
            "this approval is {}, so there is nothing to preview",
            view.status.as_str()
        )));
    }
    if !available(&facade) {
        return Err(DecideError::Refused("this server cannot preview writes".to_string()));
    }
    let username = quietly(|| -> Result<Option<String>, Error> {
        Ok(facade
            .find("moqui.security.UserAccount")?
            .eq("userId", Value::string(user_id))
            .disable_authz()
            .one()?
            .and_then(|row| crate::text_of(&row, "username")))
    })?
    .ok_or(DecideError::NotFound)?;
    let preview = run_target(
        factory,
        &PreviewTarget {
            username: &username,
            agent_id: &view.agent_id,
            run_id: Some(&view.run_id),
            approval_id,
            kind: view.target_kind,
            name: &view.target_name,
            args: &view.args,
        },
    );
    quietly(|| {
        in_tx(&facade, || {
            facade
                .find(APPROVAL)?
                .eq("approvalId", Value::string(approval_id))
                .disable_authz()
                .update_all([("previewJson".to_string(), Value::string(preview.to_string()))])
        })
    })?;
    Ok(preview)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deleted_row_without_visible_production_does_not_disclose_its_key() {
        let facade = moqui_core::testing::open();
        let shown = compared(
            &facade,
            &SimChange {
                entity: "moqui.security.UserAccount".into(),
                key: vec![("userId".into(), "private-key".into())],
                values: None,
            },
        );
        assert_eq!(shown["action"], "unavailable");
        assert!(!shown.to_string().contains("private-key"));
    }

    #[test]
    fn a_denied_preview_read_does_not_fall_back_to_showing_simulated_values() {
        let context = ExecutionContextFactory::test_context_with_conf(
            r#"<moqui-conf><artifact-execution-facade><artifact-execution type="AT_ENTITY" authz-enabled="true"/></artifact-execution-facade></moqui-conf>"#,
        );
        let facade = moqui_core::testing::open();
        context.factory().set_entity(facade.clone());
        let _entered = context.enter();
        moqui_core::user::session::set_user("unprivileged", "unprivileged");
        let change = SimChange {
            entity: "moqui.security.UserAccount".into(),
            key: vec![("userId".into(), "private-user".into())],
            values: Some(BTreeMap::from([("userFullName".into(), Value::string("private-name"))])),
        };
        let shown = compared(&facade, &change);
        assert_eq!(shown["action"], "unavailable", "{shown}");
        assert!(!shown.to_string().contains("private-name"));
        assert!(shown.get("fields").is_none());
        moqui_core::user::session::reset_session();
    }

    /// An encrypted or secret-named field of a previewed row is shown changed, never its value.
    #[test]
    fn a_previewed_secret_is_shown_changed_but_never_its_value() {
        let facade = moqui_core::testing::open();
        let mut row = facade.make_value("moqui.security.UserAccount").unwrap();
        for (field, value) in [
            ("userId", "PV1"),
            ("username", "pv.one"),
            ("currentPassword", "old-secret-hash"),
        ] {
            row.set(field, Value::string(value));
        }
        row.create(&facade).unwrap();
        let change = SimChange {
            entity: "moqui.security.UserAccount".to_string(),
            key: vec![("userId".to_string(), "PV1".to_string())],
            values: Some(BTreeMap::from([
                ("userId".to_string(), Value::string("PV1")),
                ("username".to_string(), Value::string("pv.one")),
                ("currentPassword".to_string(), Value::string("new-secret-hash")),
                ("userFullName".to_string(), Value::string("Preview One")),
            ])),
        };
        let shown = compared(&facade, &change);
        let text = shown.to_string();
        assert!(!text.contains("secret-hash"), "{text}");
        let fields = shown["fields"].as_array().unwrap();
        let password = fields.iter().find(|field| field["field"] == "currentPassword").unwrap();
        assert_eq!(
            (password["before"].as_str(), password["after"].as_str()),
            (Some("(hidden)"), Some("(hidden)"))
        );
        let name = fields.iter().find(|field| field["field"] == "userFullName").unwrap();
        assert_eq!(name["after"], "Preview One");
        let created = compared(
            &facade,
            &SimChange {
                key: vec![("userId".to_string(), "PV2".to_string())],
                ..change
            },
        );
        assert!(!created.to_string().contains("secret-hash"), "{created}");
    }
}
