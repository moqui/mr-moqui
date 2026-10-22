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

//! What a conversation has loaded (`AgentSkillUse`): one row a load, its `sectionName` empty for the head (the text
//! before the first section and the contents), [`WHOLE`] for the whole body, else a section's slug. A gated tool asks
//! [`loaded`] whether its guide, or which sections of it, are in the conversation; the skill's `useCount` and
//! `lastUsedDate` count the loads.

use moqui_lib::error::Error;
use moqui_lib::object::Value;

use moqui_db::sql::Condition;

use super::{Skill, SKILL, USE};
use crate::entity::{fence, EntityFacade};
use crate::llm::run::context::TurnContext;
use crate::llm::run::{now_millis, quietly, stamp};
use crate::transaction::TransactionAttribute;
use crate::llm::{int_of, text_of};

/// The `sectionName` of a load that sent the whole body.
pub const WHOLE: &str = "*";

/// The sections of skill `name` loaded in the run's conversation: `None` when it was not loaded there; else the slugs
/// whose text the model got, all of them when the whole body came, none when only the head did (the text before the
/// first section and the contents). A call with no run (a direct MCP call) has loaded nothing. A failed read is logged
/// and counts as not loaded.
pub fn loaded(ctx: &TurnContext<'_>, name: &str) -> Option<Vec<String>> {
    let run = ctx.run?;
    let facade = super::facade()?;
    match quietly(|| loaded_in(&facade, &run.conversation_id, name)) {
        Ok(found) => found,
        Err(error) => {
            tracing::error!(
                "the skills loaded in agent conversation {} could not be read: {error}",
                run.conversation_id
            );
            None
        }
    }
}

/// [`loaded`] for a conversation by its id.
///
/// # Errors
///
/// Returns an error when the rows cannot be read.
pub fn loaded_in(facade: &EntityFacade, conversation_id: &str, name: &str) -> Result<Option<Vec<String>>, Error> {
    let rows = facade
        .find(SKILL)?
        .eq("skillName", Value::string(name.trim()))
        .disable_authz()
        .list()?;
    let ids: Vec<Value> = rows
        .values()
        .iter()
        .filter_map(|row| text_of(row, "skillId"))
        .map(Value::string)
        .collect();
    if ids.is_empty() {
        return Ok(None);
    }
    let uses = facade
        .find(USE)?
        .eq("conversationId", Value::string(conversation_id))
        .condition(Condition::is_in("skillId", ids))
        .disable_authz()
        .list()?;
    if uses.values().is_empty() {
        return Ok(None);
    }
    let mut slugs: Vec<String> = Vec::new();
    let mut add = |slug: String| {
        if !slugs.contains(&slug) {
            slugs.push(slug);
        }
    };
    for row in uses.values() {
        match text_of(row, "sectionName") {
            None => {}
            Some(section) if section == WHOLE => {
                let skill_id = text_of(row, "skillId").unwrap_or_default();
                if let Some(skill) = rows
                    .values()
                    .iter()
                    .find(|skill| text_of(skill, "skillId").as_deref() == Some(skill_id.as_str()))
                {
                    let body = Skill {
                        body: text_of(skill, "body").unwrap_or_default(),
                        ..Skill::default()
                    };
                    for section in body.sections().1 {
                        add(section.slug);
                    }
                }
            }
            Some(section) => add(section),
        }
    }
    Ok(Some(slugs))
}

/// Record that the run loaded `skill` (`section` empty for the head, [`WHOLE`], or a slug) and count it on the skill, in
/// a transaction of its own. The run's own bookkeeping, so a read tool's fence lifts for it; a failure is logged and the
/// load goes on.
pub fn record(facade: &EntityFacade, run_id: Option<&str>, conversation_id: Option<&str>, skill: &Skill, section: &str) {
    if skill.skill_id.is_empty() {
        return;
    }
    let _lifted = fence::lift();
    // a transaction of its own: a failed count must not roll back the caller's (a run's start, a mention)
    let stored = quietly(|| {
        facade.transactions_for(USE)?.run(TransactionAttribute::ForceNew, None, || {
            let now = now_millis();
            let mut row = facade.make_value(USE)?;
            row.set_sequenced_id_primary(facade)?;
            row.set("skillId", Value::string(&skill.skill_id));
            if let Some(run_id) = run_id {
                row.set("runId", Value::string(run_id));
            }
            if let Some(conversation_id) = conversation_id {
                row.set("conversationId", Value::string(conversation_id));
            }
            if !section.is_empty() {
                row.set("sectionName", Value::string(section));
            }
            row.set("usedDate", stamp(now));
            row.create(facade)?;
            if let Some(mut skill_row) = facade
                .find(SKILL)?
                .eq("skillId", Value::string(&skill.skill_id))
                .for_update(true)
                .disable_authz()
                .one()?
            {
                skill_row.set("useCount", Value::I64(int_of(&skill_row, "useCount").unwrap_or(0) + 1));
                skill_row.set("lastUsedDate", stamp(now));
                skill_row.update(facade)?;
            }
            Ok(())
        })
    });
    if let Err(error) = stored {
        tracing::error!("the use of skill {} was not recorded: {error}", skill.name);
    }
}
