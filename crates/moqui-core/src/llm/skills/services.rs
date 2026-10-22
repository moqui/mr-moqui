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

//! The skill services of `org.moqui.agent.AgentAdminServices` (System › LLM › Skills): `sync#AgentSkills`,
//! `store#AgentSkill` (a site or personal skill from its Markdown; a file's row takes only a lock; publishing a site copy
//! of a learned skill, shared for review, makes it the site's own), `review#AgentLesson` (approve or reject a lesson) and
//! `get#SkillRetrieval` (RetrievalTest: a query's ranked skills with their scores, and the text the model would get).

use std::collections::BTreeMap;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use super::{parse_skill, retrieval, sync, Audience, Provenance, Skill, SkillIndex, DRAFT, PUBLISHED, RETIRED, SKILL};
use crate::context::ExecutionContext;
use crate::entity::EntityFacade;
use crate::llm::prompt::{self, Surface};
use crate::llm::{int_of, text_of};
use crate::service::rust_services::{rust_service, RustServiceFn};

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    rust_service!(map, sync_agent_skills)?;
    rust_service!(map, store_agent_skill)?;
    rust_service!(map, get_skill_retrieval)?;
    rust_service!(map, get_agent_skill)?;
    rust_service!(map, review_agent_lesson)?;
    Ok(())
}

type Parameters = BTreeMap<String, Value>;
type Out = Result<BTreeMap<String, Value>, LibError>;

fn text(parameters: &Parameters, key: &str) -> Option<String> {
    parameters
        .get(key)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn add_error(text: impl Into<String>) {
    let text = text.into();
    ExecutionContext::with_current(|ctx| ctx.message().add_error(text));
}

fn permitted(permission: &str) -> bool {
    match crate::user::has_permission(permission) {
        Ok(true) => true,
        _ => {
            add_error(format!("this needs the {permission} permission"));
            false
        }
    }
}

fn out(entries: impl IntoIterator<Item = (&'static str, Value)>) -> BTreeMap<String, Value> {
    entries.into_iter().map(|(key, value)| (key.to_string(), value)).collect()
}

fn map(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    let mut out = indexmap::IndexMap::new();
    for (key, value) in entries {
        out.insert(key.to_string(), value);
    }
    Value::Map(out)
}

fn count(value: usize) -> Value {
    Value::I64(i64::try_from(value).unwrap_or(i64::MAX))
}

/// `sync#AgentSkills`: the skill files into rows again now. Out: the counts and each problem, naming its file.
fn sync_agent_skills(_parameters: &Parameters, facade: &EntityFacade) -> Out {
    if !permitted("AgentAdmin") {
        return Ok(BTreeMap::new());
    }
    let Some(factory) = super::factory() else {
        return Ok(BTreeMap::new());
    };
    let report = sync::sync(&factory, facade)?;
    Ok(out([
        ("added", count(report.added)),
        ("changed", count(report.changed)),
        ("unchanged", count(report.unchanged)),
        ("retired", count(report.retired)),
        ("problemList", Value::List(report.problems.into_iter().map(Value::string).collect())),
    ]))
}

/// `store#AgentSkill`: create or change a site skill (AgentAdmin) or the caller's personal skill (`personal=Y`,
/// AgentUse) from its Markdown, or lock or unlock a skill. A skill from a file takes only `locked` (its text is the
/// file's); a site skill of a file's name needs `overrides: <name>`; one site skill a name and one personal skill a
/// name for each owner. Out: the skill's id and version.
fn store_agent_skill(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let user_id = crate::user::current_user_id().ok().flatten().filter(|id| !id.is_empty());
    let Some(user_id) = user_id else {
        add_error("storing a skill needs a logged-in user");
        return Ok(BTreeMap::new());
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let existing = match text(parameters, "skillId") {
        Some(skill_id) => match facade
            .find(SKILL)?
            .eq("skillId", Value::string(&skill_id))
            .for_update(true)
            .one()?
        {
            Some(row) => Some(row),
            None => {
                add_error(format!("there is no skill {skill_id}"));
                return Ok(BTreeMap::new());
            }
        },
        None => None,
    };
    let personal = match &existing {
        Some(row) => text_of(row, "ownerUserId").is_some(),
        None => text(parameters, "personal").is_some_and(|flag| flag == "Y" || flag == "true"),
    };
    if personal {
        if let Some(owner) = existing.as_ref().and_then(|row| text_of(row, "ownerUserId")) {
            if owner != user_id {
                add_error("a personal skill is changed only by its owner");
                return Ok(BTreeMap::new());
            }
        }
        if !permitted("AgentUse") {
            return Ok(BTreeMap::new());
        }
    } else if !permitted("AgentAdmin") {
        return Ok(BTreeMap::new());
    }
    let locked = text(parameters, "locked").map(|flag| flag == "Y" || flag == "true");
    let status = text(parameters, "statusId");
    if let Some(status) = &status {
        if ![DRAFT, PUBLISHED, RETIRED].contains(&status.as_str()) {
            add_error(format!("{status} is not a skill status (AgskDraft, AgskPublished, AgskRetired)"));
            return Ok(BTreeMap::new());
        }
    }
    // a file's row: only the lock is the admin's
    if let Some(mut row) = existing.clone().filter(|row| text_of(row, "sourceLocation").is_some()) {
        if text(parameters, "skillText").is_some() || status.is_some() {
            add_error(format!(
                "skill {} comes from {}; change the file (or store a site skill with overrides: {})",
                text_of(&row, "skillName").unwrap_or_default(),
                text_of(&row, "sourceLocation").unwrap_or_default(),
                text_of(&row, "skillName").unwrap_or_default()
            ));
            return Ok(BTreeMap::new());
        }
        if let Some(locked) = locked {
            row.set("locked", Value::string(if locked { "Y" } else { "N" }));
            row.set("lastUpdatedDate", crate::llm::run::stamp(crate::llm::run::now_millis()));
            row.update(facade)?;
        }
        return Ok(out([
            ("skillId", Value::string(text_of(&row, "skillId").unwrap_or_default())),
            ("versionNum", Value::I64(int_of(&row, "versionNum").unwrap_or(1))),
        ]));
    }
    let skill_text = text(parameters, "skillText");
    let parsed: Option<Skill> = match &skill_text {
        Some(markdown) => match parse_skill(markdown) {
            Ok(skill) => Some(skill),
            Err(problem) => {
                add_error(format!("the skill does not read: {problem}"));
                return Ok(BTreeMap::new());
            }
        },
        None if existing.is_none() => {
            add_error("skillText (the skill's Markdown with its front matter) is required");
            return Ok(BTreeMap::new());
        }
        None => None,
    };
    if let Some(skill) = &parsed {
        let others = facade
            .find(SKILL)?
            .eq("skillName", Value::string(&skill.name))
            .list()?;
        let this_id = existing.as_ref().and_then(|row| text_of(row, "skillId"));
        for other in others.values() {
            if text_of(other, "skillId") == this_id {
                continue;
            }
            let other_owner = text_of(other, "ownerUserId");
            let is_file = text_of(other, "sourceLocation").is_some();
            if personal {
                if other_owner.as_deref() == Some(user_id.as_str()) {
                    add_error(format!("you have a personal skill {} already; open it to change it", skill.name));
                    return Ok(BTreeMap::new());
                }
            } else if other_owner.is_none() {
                if is_file && skill.overrides.as_deref() != Some(skill.name.as_str()) {
                    add_error(format!(
                        "skill {} comes from {}; a site skill of that name needs overrides: {} in its front matter",
                        skill.name,
                        text_of(other, "sourceLocation").unwrap_or_default(),
                        skill.name
                    ));
                    return Ok(BTreeMap::new());
                }
                if !is_file && text_of(other, "statusId").as_deref() != Some(RETIRED) {
                    add_error(format!("there is a site skill {} already; open it to change it", skill.name));
                    return Ok(BTreeMap::new());
                }
            }
        }
    }
    let now = crate::llm::run::now_millis();
    let (mut row, version, is_new) = match existing {
        Some(row) => {
            let version = int_of(&row, "versionNum").unwrap_or(0) + i64::from(parsed.is_some());
            (row, version, false)
        }
        None => {
            let mut row = facade.make_value(SKILL)?;
            row.set_sequenced_id_primary(facade)?;
            if personal {
                row.set("ownerUserId", Value::string(&user_id));
            }
            row.set(
                "provenance",
                Value::string(if personal { Provenance::User } else { Provenance::Admin }.as_str()),
            );
            row.set("useCount", Value::I64(0));
            row.set("successCount", Value::I64(0));
            (row, 1, true)
        }
    };
    if let Some(skill) = &parsed {
        skill.fill_row(&mut row);
        row.set("locked", Value::string(if skill.locked { "Y" } else { "N" }));
    }
    if let Some(locked) = locked {
        row.set("locked", Value::string(if locked { "Y" } else { "N" }));
    }
    // a site copy of a learned skill an admin publishes was reviewed: it is the site's own now, and may lower a risk
    let reviewed = !personal
        && status.as_deref() == Some(PUBLISHED)
        && text_of(&row, "provenance").as_deref() == Some(Provenance::Learned.as_str());
    match status {
        Some(status) => row.set("statusId", Value::string(status)),
        None if text_of(&row, "statusId").is_none() => row.set("statusId", Value::string(PUBLISHED)),
        None => {}
    }
    if reviewed {
        row.set("provenance", Value::string(Provenance::Admin.as_str()));
    }
    row.set("versionNum", Value::I64(version.max(1)));
    row.set("lastUpdatedDate", crate::llm::run::stamp(now));
    let skill_id = text_of(&row, "skillId").unwrap_or_default();
    if is_new {
        row.create(facade)?;
    } else {
        row.update(facade)?;
    }
    Ok(out([("skillId", Value::string(skill_id)), ("versionNum", Value::I64(version.max(1)))]))
}

/// `get#SkillRetrieval`: what retrieval makes of a query for a user, agent and surface (default the caller, assist,
/// drawer): the ranked skills with their scores, strength and what matched; the answer of `skill find=` with it; the
/// tail's `skills that may fit` line it would give a later message; and the catalog a conversation that starts with it
/// on `page` would list.
fn get_skill_retrieval(parameters: &Parameters, facade: &EntityFacade) -> Out {
    if !permitted("AgentAdmin") {
        return Ok(BTreeMap::new());
    }
    let Some(factory) = super::factory() else {
        return Ok(BTreeMap::new());
    };
    let query = text(parameters, "query").unwrap_or_default();
    let page = text(parameters, "page");
    let user_id = text(parameters, "userId").or_else(|| crate::user::current_user_id().ok().flatten());
    let agent_id = text(parameters, "agentId").unwrap_or_else(|| "assist".to_string());
    let surface = text(parameters, "surface").and_then(|name| Surface::parse(&name)).unwrap_or_default();
    let index = SkillIndex::for_audience(
        &factory,
        facade,
        &Audience {
            user_id: user_id.as_deref(),
            agent_id: Some(&agent_id),
            surface: Some(surface),
        },
    );
    let ranked = retrieval::rank_for(&factory, &index, &query, page.as_deref());
    let result_list: Vec<Value> = ranked
        .iter()
        .enumerate()
        .map(|(at, found)| {
            let skill = index.get(&found.name);
            map([
                ("rank", count(at + 1)),
                ("skillName", Value::string(&found.name)),
                ("skillId", Value::string(skill.map(|skill| skill.skill_id.clone()).unwrap_or_default())),
                ("title", Value::string(skill.map(|skill| skill.title.clone()).unwrap_or_default())),
                ("score", Value::F64((found.score * 10_000.0).round() / 10_000.0)),
                ("lexical", Value::I64(found.lexical)),
                ("semantic", found.semantic.map_or(Value::Null, |cosine| Value::F64(f64::from(cosine)))),
                ("strong", Value::string(if found.strong { "Y" } else { "N" })),
                ("why", Value::string(found.why.join(", "))),
                ("personal", Value::string(if skill.is_some_and(Skill::is_personal) { "Y" } else { "N" })),
                ("provenance", Value::string(skill.map(|skill| skill.provenance.as_str()).unwrap_or_default())),
            ])
        })
        .collect();
    let strong: Vec<String> = ranked.iter().filter(|found| found.strong).take(3).map(|found| found.name.clone()).collect();
    let tail_line = if strong.is_empty() {
        String::new()
    } else {
        format!("skills that may fit: {}", strong.join(", "))
    };
    let pins = super::pins(facade, &agent_id);
    let tools: Vec<String> = prompt::offered_tools(surface, crate::llm::conf::PromptVariant::default())
        .iter()
        .map(|tool| tool.name.clone())
        .collect();
    let catalog = super::catalog(
        &index,
        &super::CatalogInput {
            pins: &pins,
            first_text: &query,
            page: page.as_deref(),
            tools: &tools,
        },
    );
    Ok(out([
        ("resultList", Value::List(result_list)),
        ("findText", Value::string(crate::llm::tools::skill::find(&index, &query))),
        ("tailLine", Value::string(tail_line)),
        ("catalogText", Value::string(prompt::catalog_text(&catalog))),
        ("skillCount", count(index.skills.len())),
    ]))
}

/// `get#AgentSkill`: one skill for System › LLM › Skills: its Markdown as an editor shows it, its sections with their
/// purposes and sizes, the risk that counts, whether it is a file's, and the skill that counts for its name for the
/// site (another when this one is shadowed or overridden).
fn get_agent_skill(parameters: &Parameters, facade: &EntityFacade) -> Out {
    if !permitted("AgentAdmin") {
        return Ok(BTreeMap::new());
    }
    let Some(skill_id) = text(parameters, "skillId") else {
        add_error("skillId is required");
        return Ok(BTreeMap::new());
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let Some(row) = facade.find(SKILL)?.eq("skillId", Value::string(&skill_id)).one()? else {
        add_error(format!("there is no skill {skill_id}"));
        return Ok(BTreeMap::new());
    };
    let skill = Skill::of_row(&row);
    let (intro, sections) = skill.sections();
    let section_list: Vec<Value> = sections
        .iter()
        .map(|section| {
            map([
                ("slug", Value::string(format!("{}#{}", skill.name, section.slug))),
                ("title", Value::string(&section.title)),
                ("purpose", Value::string(skill.purpose(section))),
                ("chars", count(section.text.chars().count())),
            ])
        })
        .collect();
    let in_effect = super::factory().and_then(|factory| {
        let index = SkillIndex::for_audience(
            &factory,
            facade,
            &Audience {
                user_id: skill.owner.as_deref(),
                ..Audience::default()
            },
        );
        index.get(&skill.name).map(|found| found.skill_id.clone())
    });
    Ok(out([
        ("skillText", Value::string(skill.to_markdown())),
        ("introChars", count(intro.chars().count())),
        ("bodyChars", count(skill.body.chars().count())),
        ("sectionList", Value::List(section_list)),
        ("risk", Value::string(skill.risk().as_str())),
        ("fromFile", Value::string(if skill.provenance.is_file() { "Y" } else { "N" })),
        ("inEffectSkillId", in_effect.map_or(Value::Null, Value::string)),
    ]))
}

/// `review#AgentLesson` (AgentAdmin): approve a lesson for every user (`statusId` AglApproved), reject it or withdraw an
/// approved one (AglRejected). Out: the status now.
fn review_agent_lesson(parameters: &Parameters, facade: &EntityFacade) -> Out {
    if !permitted("AgentAdmin") {
        return Ok(BTreeMap::new());
    }
    let (Some(lesson_id), Some(status_id)) = (text(parameters, "lessonId"), text(parameters, "statusId")) else {
        add_error("lessonId and statusId are required");
        return Ok(BTreeMap::new());
    };
    let reviewer = crate::user::current_user_id().ok().flatten().unwrap_or_default();
    match crate::llm::memory::review_lesson(facade, &reviewer, &lesson_id, &status_id) {
        Ok(status) => Ok(out([("statusId", Value::string(status))])),
        Err(error) => {
            add_error(error.message().to_string());
            Ok(BTreeMap::new())
        }
    }
}
