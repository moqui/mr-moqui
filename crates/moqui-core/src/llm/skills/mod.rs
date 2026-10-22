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

//! Skills (plans/LLM-A2A-Assist.md 4.9 and 7.5): Markdown with front matter that an agent loads on demand by name.
//!
//! **Sources.** The files under `skill/**/*.md` of the framework and of each component, in load order, are synced into
//! `AgentSkill` rows at startup ([`sync`]; a file that does not read is logged by name and left out, the others go on),
//! so the index is one table. Site skills are rows an admin made; personal skills are rows with an owner. For one name
//! ([`resolve`]): a site row replaces a file only when it says `overrides: <name>`, a learned row never takes a name
//! that is taken, and a personal skill shadows the site's unless that one is `locked`. Only published rows count.
//!
//! **Front matter** ([`parse_skill`]): `name` (kebab-case) and `description` ("Use when …", at most 160 characters;
//! `summary` is the same key) are required; `title`, `kind` (procedure, reference, guide), `aliases` (`tags` adds to
//! them), `unlocks`, `requires`, `screens`, `services`, `risk` (reversible, confirm, irreversible), `agents`
//! (`audience` is the same key), `surfaces`, `locked`, `version`, `overrides` and `sections` (a map of section slug to
//! its purpose) are optional. Lists are `[a, b]` or `- item` lines; an unknown key is logged and ignored.
//!
//! **Sections** are the `##` headings outside code fences, addressed by slug: `name#slug` ([`slug`]).
//!
//! The catalog in the frozen prefix ([`catalog`]), the tail's strong matches ([`tail_skills`]), retrieval
//! ([`retrieval`]), the loads a conversation made ([`uses`], with [`loaded`] for a gated tool) and the `@name` mentions
//! of a message ([`mention`]) are built on the index.

pub mod learn;
pub mod mention;
pub mod retrieval;
pub(crate) mod services;
pub mod sync;
pub mod uses;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use moqui_db::sql::Condition;

use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::{EntityFacade, EntityValue};
use crate::llm::prompt::{self, Surface};
use crate::llm::{int_of, text_of};

pub use uses::{loaded, loaded_in};

pub(crate) const SKILL: &str = "moqui.agent.AgentSkill";
pub(crate) const USE: &str = "moqui.agent.AgentSkillUse";
pub(crate) const PIN: &str = "moqui.agent.AgentSkillPin";

/// A body under this size is returned whole by `skill(name)`.
pub const WHOLE_BODY: usize = 6_000;
/// The longest description (the catalog line).
pub const DESCRIPTION_CHARS: usize = 160;
/// The longest name.
pub const NAME_CHARS: usize = 64;
/// The published status; the others are draft and retired.
pub const PUBLISHED: &str = "AgskPublished";
pub const DRAFT: &str = "AgskDraft";
pub const RETIRED: &str = "AgskRetired";

/// What a skill says a write it guides risks; shown on the approval card, it never skips one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Risk {
    Reversible,
    #[default]
    Confirm,
    Irreversible,
}

impl Risk {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_lowercase().as_str() {
            "reversible" => Some(Risk::Reversible),
            "confirm" => Some(Risk::Confirm),
            "irreversible" => Some(Risk::Irreversible),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Risk::Reversible => "reversible",
            Risk::Confirm => "confirm",
            Risk::Irreversible => "irreversible",
        }
    }
}

/// Where a skill came from (`AgentSkill.provenance`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Provenance {
    /// A file of the framework.
    #[default]
    Framework,
    /// A file of a component.
    Component,
    /// A site row an admin wrote.
    Admin,
    /// A personal row its owner wrote.
    User,
    /// Drafted from a run (Save as skill, `save_skill`): model-written text.
    Learned,
}

impl Provenance {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_lowercase().as_str() {
            "framework" => Some(Provenance::Framework),
            "component" => Some(Provenance::Component),
            "admin" => Some(Provenance::Admin),
            "user" => Some(Provenance::User),
            "learned" => Some(Provenance::Learned),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Provenance::Framework => "framework",
            Provenance::Component => "component",
            Provenance::Admin => "admin",
            Provenance::User => "user",
            Provenance::Learned => "learned",
        }
    }

    /// From a file, which the index never lets anyone edit.
    pub fn is_file(self) -> bool {
        matches!(self, Provenance::Framework | Provenance::Component)
    }

    /// A person wrote it (Java's `SkillRiskGate`: only these may lower a risk).
    pub fn is_human(self) -> bool {
        !matches!(self, Provenance::Learned)
    }
}

/// One skill.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Skill {
    /// Its `AgentSkill` row; empty for a skill read from a file and not stored.
    pub skill_id: String,
    pub name: String,
    pub title: String,
    pub description: String,
    /// procedure, reference or guide; empty when the front matter names none.
    pub kind: String,
    pub aliases: Vec<String>,
    /// Tools gated until this skill is loaded.
    pub unlocks: Vec<String>,
    /// Skills loaded with it.
    pub requires: Vec<String>,
    /// Screen paths: a retrieval boost on these pages, the anchor of a risk.
    pub screens: Vec<String>,
    pub services: Vec<String>,
    /// As the front matter says it; [`Skill::risk`] is what counts.
    pub declared_risk: Option<Risk>,
    /// Agent ids it is for; empty is every agent.
    pub agents: Vec<String>,
    /// Surfaces it is for; empty is every surface.
    pub surfaces: Vec<Surface>,
    pub locked: bool,
    pub version: i64,
    /// A site row that replaces the file of this name.
    pub overrides: Option<String>,
    /// Section slug to what it holds, from the front matter's `sections`.
    pub purposes: BTreeMap<String, String>,
    /// The owner of a personal skill.
    pub owner: Option<String>,
    pub provenance: Provenance,
    pub status_id: String,
    /// The file it came from (`component://<name>/skill/…`), empty for a row an admin or user wrote.
    pub location: String,
    pub content_hash: String,
    /// The text after the front matter.
    pub body: String,
    pub use_count: i64,
}

/// A `##` section of a skill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub slug: String,
    pub title: String,
    /// The section's text, its heading line included.
    pub text: String,
}

impl Skill {
    /// The text before the first `##` heading, and the sections.
    pub fn sections(&self) -> (String, Vec<Section>) {
        let mut intro = String::new();
        let mut sections: Vec<Section> = Vec::new();
        let mut fenced = false;
        for line in self.body.lines() {
            if line.trim_start().starts_with("```") {
                fenced = !fenced;
            }
            if !fenced && line.starts_with("## ") {
                let title = line[3..].trim().to_string();
                sections.push(Section {
                    slug: slug(&title),
                    title,
                    text: String::new(),
                });
            }
            let target = match sections.last_mut() {
                Some(section) => &mut section.text,
                None => &mut intro,
            };
            target.push_str(line);
            target.push('\n');
        }
        (intro.trim().to_string(), sections)
    }

    /// What a section holds: the front matter's purpose for it, else its first sentence.
    pub fn purpose(&self, section: &Section) -> String {
        if let Some(purpose) = self.purposes.get(&section.slug) {
            return purpose.clone();
        }
        let first = section
            .text
            .lines()
            .skip(1)
            .map(str::trim)
            .find(|line| !line.is_empty() && !line.starts_with("```"))
            .unwrap_or_default();
        let sentence = first.split(". ").next().unwrap_or(first).trim_end_matches('.');
        if sentence.chars().count() > 90 {
            format!("{}…", sentence.chars().take(89).collect::<String>())
        } else {
            sentence.to_string()
        }
    }

    /// The risk that counts: what it declares, except that only a skill a person wrote may lower one (a learned
    /// skill's model-written `reversible` is `confirm`).
    pub fn risk(&self) -> Risk {
        match self.declared_risk {
            Some(Risk::Reversible) if !self.provenance.is_human() => Risk::Confirm,
            Some(risk) => risk,
            None => Risk::Confirm,
        }
    }

    /// It is for this agent on this surface.
    pub fn serves(&self, agent_id: Option<&str>, surface: Option<Surface>) -> bool {
        let agent = match agent_id {
            Some(agent) => self.agents.is_empty() || self.agents.iter().any(|known| known == agent),
            None => true,
        };
        let surface = match surface {
            Some(surface) => self.surfaces.is_empty() || self.surfaces.contains(&surface),
            None => true,
        };
        agent && surface
    }

    pub fn is_personal(&self) -> bool {
        self.owner.is_some()
    }

    /// The front matter kept in `frontMatterJson`, beside the fields that have a column.
    pub fn front_matter_json(&self) -> Json {
        json!({
            "unlocks": self.unlocks,
            "requires": self.requires,
            "screens": self.screens,
            "services": self.services,
            "risk": self.declared_risk.map(Risk::as_str),
            "agents": self.agents,
            "surfaces": self.surfaces.iter().map(|surface| surface.as_str()).collect::<Vec<_>>(),
            "version": self.version,
            "overrides": self.overrides,
            "sections": self.purposes,
            "locked": self.locked,
        })
    }

    /// A skill from its `AgentSkill` row.
    pub fn of_row(row: &EntityValue) -> Self {
        let front: Json = text_of(row, "frontMatterJson")
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(Json::Null);
        let list = |key: &str| -> Vec<String> {
            front
                .get(key)
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
                .filter_map(Json::as_str)
                .map(str::to_string)
                .collect()
        };
        Skill {
            skill_id: text_of(row, "skillId").unwrap_or_default(),
            name: text_of(row, "skillName").unwrap_or_default(),
            title: text_of(row, "title").unwrap_or_default(),
            description: text_of(row, "description").unwrap_or_default(),
            kind: text_of(row, "skillKind").unwrap_or_default(),
            aliases: split_list(&text_of(row, "aliases").unwrap_or_default()),
            unlocks: list("unlocks"),
            requires: list("requires"),
            screens: list("screens"),
            services: list("services"),
            declared_risk: front.get("risk").and_then(Json::as_str).and_then(Risk::parse),
            agents: list("agents"),
            surfaces: list("surfaces").iter().filter_map(|name| Surface::parse(name)).collect(),
            locked: crate::llm::flag_of(row, "locked").unwrap_or(false),
            version: int_of(row, "versionNum").unwrap_or(1),
            overrides: front.get("overrides").and_then(Json::as_str).map(str::to_string),
            purposes: front
                .get("sections")
                .and_then(Json::as_object)
                .map(|map| {
                    map.iter()
                        .filter_map(|(slug, purpose)| purpose.as_str().map(|purpose| (slug.clone(), purpose.to_string())))
                        .collect()
                })
                .unwrap_or_default(),
            owner: text_of(row, "ownerUserId"),
            provenance: text_of(row, "provenance")
                .and_then(|text| Provenance::parse(&text))
                .unwrap_or(Provenance::Admin),
            status_id: text_of(row, "statusId").unwrap_or_default(),
            location: text_of(row, "sourceLocation").unwrap_or_default(),
            content_hash: text_of(row, "contentHash").unwrap_or_default(),
            body: text_of(row, "body").unwrap_or_default(),
            use_count: int_of(row, "useCount").unwrap_or(0),
        }
    }

    /// Set the row's fields from this skill (not its id, owner, status, counts or provenance).
    pub fn fill_row(&self, row: &mut EntityValue) {
        row.set("skillName", Value::string(&self.name));
        row.set("title", Value::string(&self.title));
        row.set("description", Value::string(&self.description));
        row.set("skillKind", if self.kind.is_empty() { Value::Null } else { Value::string(&self.kind) });
        row.set("body", Value::string(&self.body));
        row.set("frontMatterJson", Value::string(self.front_matter_json().to_string()));
        row.set(
            "aliases",
            if self.aliases.is_empty() {
                Value::Null
            } else {
                Value::string(self.aliases.join(", "))
            },
        );
        row.set("contentHash", Value::string(&self.content_hash));
    }
}

impl Skill {
    /// The skill as Markdown with its front matter, as an editor shows it (and [`parse_skill`] reads it back).
    pub fn to_markdown(&self) -> String {
        let list = |items: &[String]| format!("[{}]", items.join(", "));
        let mut lines = vec!["---".to_string(), format!("name: {}", self.name)];
        if !self.title.is_empty() {
            lines.push(format!("title: {}", self.title));
        }
        lines.push(format!("description: {}", self.description));
        if !self.kind.is_empty() {
            lines.push(format!("kind: {}", self.kind));
        }
        for (key, items) in [
            ("aliases", &self.aliases),
            ("unlocks", &self.unlocks),
            ("requires", &self.requires),
            ("screens", &self.screens),
            ("services", &self.services),
            ("agents", &self.agents),
        ] {
            if !items.is_empty() {
                lines.push(format!("{key}: {}", list(items)));
            }
        }
        if !self.surfaces.is_empty() {
            let names: Vec<String> = self.surfaces.iter().map(|surface| surface.as_str().to_string()).collect();
            lines.push(format!("surfaces: {}", list(&names)));
        }
        if let Some(risk) = self.declared_risk {
            lines.push(format!("risk: {}", risk.as_str()));
        }
        if let Some(overrides) = &self.overrides {
            lines.push(format!("overrides: {overrides}"));
        }
        if self.locked {
            lines.push("locked: true".to_string());
        }
        if self.version > 1 {
            lines.push(format!("version: {}", self.version));
        }
        if !self.purposes.is_empty() {
            lines.push("sections:".to_string());
            for (slug, purpose) in &self.purposes {
                lines.push(format!("  {slug}: {purpose}"));
            }
        }
        lines.push("---".to_string());
        lines.push(self.body.clone());
        lines.join("\n")
    }
}

/// A heading as a slug: lower case words joined by `-`.
pub fn slug(title: &str) -> String {
    title
        .to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// A comma list as its trimmed items.
fn split_list(text: &str) -> Vec<String> {
    text.split(',')
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

/// The hash of a skill's text, kept to notice that a file changed.
pub fn content_hash(text: &str) -> String {
    moqui_lib::json::canonical_sha256(&Json::String(text.to_string()))
}

/// A value of the front matter.
#[derive(Clone, Debug, PartialEq)]
enum FrontValue {
    Text(String),
    List(Vec<String>),
    Map(Vec<(String, String)>),
}

fn unquote(text: &str) -> String {
    let text = text.trim();
    let quoted = text.len() >= 2
        && ((text.starts_with('"') && text.ends_with('"')) || (text.starts_with('\'') && text.ends_with('\'')));
    if quoted {
        text[1..text.len() - 1].to_string()
    } else {
        text.to_string()
    }
}

/// The front matter's keys, in order: `key: value`, `key: [a, b]`, and a key with no value followed by indented
/// `- item` lines (a list) or `slug: text` lines (a map). A ` #` starts a comment.
fn front_values(front: &str) -> Result<Vec<(String, FrontValue)>, String> {
    let mut out: Vec<(String, FrontValue)> = Vec::new();
    for (number, raw) in front.lines().enumerate() {
        let line = match raw.find(" #") {
            Some(at) => &raw[..at],
            None => raw,
        };
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if indented {
            let item = line.trim();
            let Some((_, last)) = out.last_mut() else {
                return Err(format!("front matter line {} is indented under no key", number + 1));
            };
            if let Some(entry) = item.strip_prefix("- ").or_else(|| (item == "-").then_some("")) {
                match last {
                    FrontValue::List(items) => items.push(unquote(entry)),
                    FrontValue::Text(text) if text.is_empty() => *last = FrontValue::List(vec![unquote(entry)]),
                    _ => return Err(format!("front matter line {} is a list item under a key that has a value", number + 1)),
                }
            } else if let Some((key, value)) = item.split_once(':') {
                let pair = (key.trim().to_string(), unquote(value));
                match last {
                    FrontValue::Map(pairs) => pairs.push(pair),
                    FrontValue::Text(text) if text.is_empty() => *last = FrontValue::Map(vec![pair]),
                    _ => return Err(format!("front matter line {} is a map entry under a key that has a value", number + 1)),
                }
            } else {
                return Err(format!("front matter line {} does not read: {}", number + 1, item));
            }
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            return Err(format!("front matter line {} has no key: {}", number + 1, line.trim()));
        };
        let value = value.trim();
        let parsed = if value.starts_with('[') {
            if !value.ends_with(']') {
                return Err(format!("the list of {} has no closing ]", key.trim()));
            }
            FrontValue::List(
                value[1..value.len() - 1]
                    .split(',')
                    .map(unquote)
                    .filter(|item| !item.is_empty())
                    .collect(),
            )
        } else {
            FrontValue::Text(unquote(value))
        };
        out.push((key.trim().to_string(), parsed));
    }
    Ok(out)
}

/// A skill from its Markdown: the front matter between `---` lines, then the body.
///
/// # Errors
///
/// Returns every reason it is not a skill, joined with `; `: no front matter, a name that is missing or not
/// kebab-case, a description that is missing, too long or does not start "Use when", an unknown kind, risk or surface,
/// a `locked` or `version` that does not read.
pub fn parse_skill(text: &str) -> Result<Skill, String> {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let rest = text
        .strip_prefix("---\n")
        .ok_or("it does not start with front matter (---)")?;
    let (front, body) = match rest.find("\n---") {
        Some(end) => (&rest[..end], &rest[end + 4..]),
        None if rest.starts_with("---") => ("", &rest[3..]),
        None => return Err("its front matter has no closing ---".to_string()),
    };
    let body = body.trim_start_matches('-').trim_start_matches('\n');
    let mut skill = Skill {
        body: body.trim_end().to_string(),
        content_hash: content_hash(&text),
        version: 1,
        status_id: PUBLISHED.to_string(),
        ..Skill::default()
    };
    let mut problems: Vec<String> = Vec::new();
    let values = front_values(front)?;
    let list = |value: &FrontValue| -> Vec<String> {
        match value {
            FrontValue::List(items) => items.clone(),
            FrontValue::Text(text) => split_list(text),
            FrontValue::Map(pairs) => pairs.iter().map(|(key, _)| key.clone()).collect(),
        }
    };
    let text_of = |value: &FrontValue| -> String {
        match value {
            FrontValue::Text(text) => text.clone(),
            FrontValue::List(items) => items.join(", "),
            FrontValue::Map(_) => String::new(),
        }
    };
    let flag = |key: &str, value: &FrontValue, problems: &mut Vec<String>| -> bool {
        match text_of(value).to_lowercase().as_str() {
            "true" | "yes" | "y" => true,
            "false" | "no" | "n" | "" => false,
            other => {
                problems.push(format!("{key} is {other}, not true or false"));
                false
            }
        }
    };
    let mut unknown: Vec<String> = Vec::new();
    for (key, value) in &values {
        match key.as_str() {
            "name" => skill.name = text_of(value),
            "title" => skill.title = text_of(value),
            "description" | "summary" => skill.description = text_of(value),
            "kind" | "type" => skill.kind = text_of(value).to_lowercase(),
            "aliases" | "tags" => {
                for alias in list(value) {
                    let alias = alias.to_lowercase();
                    if !skill.aliases.contains(&alias) {
                        skill.aliases.push(alias);
                    }
                }
            }
            "unlocks" => skill.unlocks = list(value),
            "requires" => skill.requires = list(value),
            "screens" => skill.screens = list(value),
            "services" => skill.services = list(value),
            "risk" => {
                let risk = text_of(value);
                match Risk::parse(&risk) {
                    Some(risk) => skill.declared_risk = Some(risk),
                    None => problems.push(format!("risk is {risk}, not reversible, confirm or irreversible")),
                }
            }
            "agents" | "audience" => skill.agents = list(value),
            "surfaces" => {
                for name in list(value) {
                    match Surface::parse(&name) {
                        Some(surface) => skill.surfaces.push(surface),
                        None => problems.push(format!("{name} is not a surface (drawer, chat, a2a, mcp, job, tui)")),
                    }
                }
            }
            "locked" => skill.locked = flag("locked", value, &mut problems),
            "version" => match text_of(value).parse::<i64>() {
                Ok(version) if version > 0 => skill.version = version,
                _ => problems.push(format!("version is {}, not a whole number above 0", text_of(value))),
            },
            "overrides" => skill.overrides = Some(text_of(value)).filter(|text| !text.is_empty()),
            "sections" => match value {
                FrontValue::Map(pairs) => {
                    for (key, purpose) in pairs {
                        skill.purposes.insert(slug(key), purpose.clone());
                    }
                }
                _ => problems.push("sections is not a map of section to purpose".to_string()),
            },
            other => unknown.push(other.to_string()),
        }
    }
    if skill.name.is_empty() {
        problems.insert(0, "its front matter has no name".to_string());
    } else if skill.name.chars().count() > NAME_CHARS
        || skill.name.starts_with('-')
        || skill.name.ends_with('-')
        || !skill
            .name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        problems.insert(0, format!("its name {} is not kebab-case (a-z, 0-9 and -, at most {NAME_CHARS})", skill.name));
    }
    let description = skill.description.trim();
    if description.is_empty() {
        problems.push("it has no description".to_string());
    } else {
        if description.chars().count() > DESCRIPTION_CHARS {
            problems.push(format!(
                "its description is {} characters, over {DESCRIPTION_CHARS}",
                description.chars().count()
            ));
        }
        if !description.to_lowercase().starts_with("use when") {
            problems.push("its description does not start \"Use when\"".to_string());
        }
    }
    if !skill.kind.is_empty() && !["procedure", "reference", "guide"].contains(&skill.kind.as_str()) {
        problems.push(format!("kind is {}, not procedure, reference or guide", skill.kind));
    }
    if !problems.is_empty() {
        return Err(problems.join("; "));
    }
    if !unknown.is_empty() {
        tracing::warn!("skill {} has front matter keys it does not use: {}", skill.name, unknown.join(", "));
    }
    if skill.title.is_empty() {
        skill.title = skill.name.replace('-', " ");
    }
    Ok(skill)
}

/// For whom an index is: a user's personal skills join the site's, and skills for other agents or surfaces are left
/// out. Empty fields leave nothing out.
#[derive(Clone, Copy, Debug, Default)]
pub struct Audience<'a> {
    pub user_id: Option<&'a str>,
    pub agent_id: Option<&'a str>,
    pub surface: Option<Surface>,
}

/// The skills of one name, by precedence (plan 4.9 and answered question 6): the base is the first file of the name,
/// or a site row that `overrides` it, or a site row when no file has it (a learned row never takes a name a file or
/// a person's row has); a personal skill of `user_id` shadows the base unless the base is locked. Rows of other
/// owners and rows not published are left out. Out: the skills that count, by name.
pub fn resolve(candidates: Vec<Skill>, user_id: Option<&str>) -> Vec<Skill> {
    let mut by_name: BTreeMap<String, Vec<Skill>> = BTreeMap::new();
    for skill in candidates {
        if !skill.status_id.is_empty() && skill.status_id != PUBLISHED {
            continue;
        }
        if skill.owner.is_some() && skill.owner.as_deref() != user_id {
            continue;
        }
        by_name.entry(skill.name.clone()).or_default().push(skill);
    }
    let mut out = Vec::new();
    for (name, skills) in by_name {
        let (personal, site): (Vec<Skill>, Vec<Skill>) = skills.into_iter().partition(Skill::is_personal);
        let file = site.iter().find(|skill| skill.provenance.is_file()).cloned();
        let rows: Vec<&Skill> = site.iter().filter(|skill| !skill.provenance.is_file()).collect();
        let human_row = rows.iter().find(|skill| skill.provenance.is_human()).copied();
        let base = match (&file, human_row) {
            (Some(file), Some(row)) if row.overrides.as_deref() == Some(name.as_str()) => Some(row.clone()),
            (Some(file), _) => Some(file.clone()),
            (None, Some(row)) => Some(row.clone()),
            (None, None) => rows.first().map(|row| (*row).clone()),
        };
        let mine = personal
            .iter()
            .find(|skill| skill.provenance.is_human())
            .or_else(|| personal.first())
            .cloned();
        let chosen = match (base, mine) {
            (Some(base), Some(mine)) if base.locked => {
                tracing::debug!("skill {name} is locked, so the personal one of {:?} is not used", mine.owner);
                Some(base)
            }
            // a learned personal row does not take a name a file or a person's site row has
            (Some(base), Some(mine)) if mine.provenance == Provenance::Learned => Some(base),
            (_, Some(mine)) => Some(mine),
            (base, None) => base,
        };
        out.extend(chosen);
    }
    out
}

/// The skills a run may load and the domain aliases of the components.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkillIndex {
    pub skills: Vec<Skill>,
    /// A term and the words that mean it.
    pub aliases: BTreeMap<String, Vec<String>>,
}

impl SkillIndex {
    /// The site's skills (no personal ones) of the factory's components, synced first if they were not.
    pub fn load(factory: &ExecutionContextFactory) -> Self {
        match factory.entity() {
            Some(facade) => Self::for_audience(factory, &facade, &Audience::default()),
            None => Self::with_aliases(factory),
        }
    }

    /// No skills, only the component aliases (what `search` uses).
    pub fn with_aliases(factory: &ExecutionContextFactory) -> Self {
        Self {
            skills: Vec::new(),
            aliases: sync::state(factory).aliases(),
        }
    }

    /// The skills that count for `audience` ([`resolve`]), synced from the files first if they were not. A read that
    /// fails is logged and the index is empty.
    pub fn for_audience(factory: &ExecutionContextFactory, facade: &EntityFacade, audience: &Audience<'_>) -> Self {
        sync::ensure_synced(factory, facade);
        let aliases = sync::state(factory).aliases();
        let rows = crate::llm::run::quietly(|| {
            let mut find = facade
                .find(SKILL)?
                .eq("statusId", Value::string(PUBLISHED))
                .order_by("skillName", false)
                .disable_authz();
            find = match audience.user_id {
                Some(user_id) => find.condition(Condition::or(vec![
                    Condition::is_null("ownerUserId"),
                    Condition::equals("ownerUserId", Value::string(user_id)),
                ])),
                None => find.condition(Condition::is_null("ownerUserId")),
            };
            find.list()
        });
        let candidates: Vec<Skill> = match rows {
            Ok(rows) => rows.values().iter().map(Skill::of_row).collect(),
            Err(error) => {
                tracing::error!("the agent skills could not be read: {error}");
                Vec::new()
            }
        };
        let hidden = hidden_dialect_skills(facade, audience.agent_id);
        let skills = resolve(candidates, audience.user_id)
            .into_iter()
            .filter(|skill| skill.serves(audience.agent_id, audience.surface))
            .filter(|skill| !hidden.contains(&skill.name.as_str()))
            .collect();
        Self { skills, aliases }
    }

    /// The skills of these `skill/` directories, in order, as files of the framework (not stored).
    pub fn from_dirs(dirs: &[std::path::PathBuf]) -> Self {
        let named: Vec<(String, std::path::PathBuf)> = dirs
            .iter()
            .map(|dir| ("framework".to_string(), dir.clone()))
            .collect();
        let read = sync::read_dirs(&named);
        Self {
            skills: read.skills,
            aliases: read.aliases,
        }
    }

    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().find(|skill| skill.name == name.trim())
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.skills.iter().map(|skill| skill.name.as_str())
    }

    /// The words a query word also stands for: its aliases, and the terms it is an alias of.
    pub fn alias_words(&self, word: &str) -> Vec<String> {
        let word = word.to_lowercase();
        let mut out: Vec<String> = self.aliases.get(&word).cloned().unwrap_or_default();
        for (term, words) in &self.aliases {
            if words.contains(&word) && !out.contains(term) {
                out.push(term.clone());
            }
        }
        out
    }

    /// The skills that match `text`, best first ([`retrieval::rank`]).
    pub fn find(&self, text: &str) -> Vec<&Skill> {
        retrieval::rank(self, text, None)
            .into_iter()
            .filter_map(|found| self.get(&found.name))
            .collect()
    }
}

/// The entity facade of the current execution context.
pub(crate) fn facade() -> Option<EntityFacade> {
    ExecutionContext::with_current(|ec| ec.factory().entity()).flatten()
}

/// The factory of the current execution context.
pub(crate) fn factory() -> Option<Arc<ExecutionContextFactory>> {
    ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))
}

/// The pins of an agent (`AgentSkillPin`), in order. A failed read is logged and there are none.
pub fn pins(facade: &EntityFacade, agent_id: &str) -> Vec<String> {
    let rows = crate::llm::run::quietly(|| {
        facade
            .find(PIN)?
            .eq("agentId", Value::string(agent_id))
            .order_by("sequenceNum", false)
            .order_by("skillName", false)
            .disable_authz()
            .list()
    });
    match rows {
        Ok(rows) => rows.values().iter().filter_map(|row| text_of(row, "skillName")).collect(),
        Err(error) => {
            tracing::error!("the skill pins of agent {agent_id} could not be read: {error}");
            Vec::new()
        }
    }
}

/// The skills of the canvas dialects `agent_id` does not write (`AgentDefinition.canvasDialects`; with no agent, the
/// default, OpenUI Lang only): such a skill is neither listed nor loaded, since its `write_ui` kind is not offered.
fn hidden_dialect_skills(facade: &EntityFacade, agent_id: Option<&str>) -> Vec<&'static str> {
    let agent = match agent_id {
        Some(agent_id) => match crate::llm::run::quietly(|| crate::llm::run::AgentDef::load(facade, agent_id)) {
            Ok(agent) => agent.unwrap_or_default(),
            Err(error) => {
                tracing::error!("the canvas dialects of agent {agent_id} could not be read: {error}");
                crate::llm::run::AgentDef::default()
            }
        },
        None => crate::llm::run::AgentDef::default(),
    };
    crate::llm::ui::dialects()
        .iter()
        .filter(|dialect| !agent.allows_dialect(dialect.name()))
        .map(|dialect| dialect.skill())
        .collect()
}

/// What the catalog of a new conversation is built from.
#[derive(Clone, Debug, Default)]
pub struct CatalogInput<'a> {
    /// The agent's pinned skills, in order.
    pub pins: &'a [String],
    /// The conversation's first message.
    pub first_text: &'a str,
    /// The page it was sent from.
    pub page: Option<&'a str>,
    /// The names of the tools the conversation offers.
    pub tools: &'a [String],
}

/// The catalog of a new conversation (plan 7.5): the admin's pins first, then the guides of tools it offers that a
/// skill unlocks, then the skills that match the first message and its page, then the most used, then by name; the
/// prompt lists the first [`prompt::caps::CATALOG_LINES`] and counts the rest. Learned skills are never in it.
pub fn catalog(index: &SkillIndex, input: &CatalogInput<'_>) -> prompt::Catalog {
    let mut order: Vec<&str> = Vec::new();
    for name in input.pins {
        if index.get(name).is_none() {
            tracing::debug!("pinned skill {name} is not in the index");
        }
        push_name(index, name, &mut order);
    }
    for skill in &index.skills {
        if skill.unlocks.iter().any(|tool| input.tools.iter().any(|offered| offered == tool)) {
            push_name(index, &skill.name, &mut order);
        }
    }
    for found in retrieval::rank(index, input.first_text, input.page) {
        push_name(index, &found.name, &mut order);
    }
    let mut rest: Vec<&Skill> = index.skills.iter().collect();
    rest.sort_by(|left, right| right.use_count.cmp(&left.use_count).then_with(|| left.name.cmp(&right.name)));
    for skill in rest {
        push_name(index, &skill.name, &mut order);
    }
    let shown = order.len().min(prompt::caps::CATALOG_LINES);
    prompt::Catalog {
        entries: order
            .iter()
            .take(shown)
            .filter_map(|name| index.get(name))
            .map(|skill| prompt::CatalogEntry {
                name: skill.name.clone(),
                description: skill.description.clone(),
                kind: skill.kind.clone(),
            })
            .collect(),
        more: order.len() - shown,
    }
}

/// A learned skill (model-written, plan 11) never enters the frozen prefix: it is found through `skill find=` and the
/// tail's `skills that may fit`, and its text comes wrapped as untrusted.
fn push_name<'a>(index: &'a SkillIndex, name: &str, order: &mut Vec<&'a str>) {
    if let Some(skill) = index.get(name).filter(|skill| skill.provenance != Provenance::Learned) {
        if !order.contains(&skill.name.as_str()) {
            order.push(skill.name.as_str());
        }
    }
}

/// The page path of a tail (`page /apps/… "Title"`), if it has one.
pub fn tail_page(tail: &str) -> Option<String> {
    tail.lines()
        .find_map(|line| line.strip_prefix("page "))
        .and_then(|rest| rest.split_whitespace().next())
        .map(|path| path.split('?').next().unwrap_or(path).to_string())
}

/// The skills that may fit a later message (answered question 7): strong matches of its text that the conversation's
/// frozen catalog does not list and that it has not loaded, at most three. A conversation with no catalog yet (its
/// first message) gets only the learned skills that strongly match: the catalog is built from that message, and a
/// learned skill never enters it.
pub fn tail_skills(facade: &EntityFacade, index: &SkillIndex, conversation_id: Option<&str>, text: &str) -> Vec<String> {
    let first_message = || {
        retrieval::rank(index, text, None)
            .into_iter()
            .filter(|found| found.strong)
            .filter(|found| index.get(&found.name).is_some_and(|skill| skill.provenance == Provenance::Learned))
            .map(|found| found.name)
            .take(3)
            .collect()
    };
    let Some(conversation_id) = conversation_id else {
        return first_message();
    };
    let row = crate::llm::run::quietly(|| {
        facade
            .find(crate::llm::run::CONVERSATION)?
            .eq("conversationId", Value::string(conversation_id))
            .disable_authz()
            .one()
    });
    let listed: Vec<String> = match row {
        Ok(Some(row)) if text_of(&row, "systemText").is_some() => {
            split_list(&text_of(&row, "skillCatalog").unwrap_or_default())
        }
        Ok(_) => return first_message(),
        Err(error) => {
            tracing::error!("agent conversation {conversation_id} could not be read for its skills: {error}");
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for found in retrieval::rank(index, text, None).into_iter().filter(|found| found.strong) {
        if listed.contains(&found.name) {
            continue;
        }
        if crate::llm::run::quietly(|| loaded_in(facade, conversation_id, &found.name))
            .ok()
            .flatten()
            .is_some()
        {
            continue;
        }
        out.push(found.name);
        if out.len() == 3 {
            break;
        }
    }
    out
}

/// The risk a write of `target` (a screen path or a service name) carries by the skills loaded in the conversation
/// that name it in `screens` or `services`: the highest, with the skill that says it. None when no loaded skill names
/// it.
pub fn risk_for(facade: &EntityFacade, index: &SkillIndex, conversation_id: &str, target: &str) -> Option<(String, Risk)> {
    let target = target.trim();
    let mut best: Option<(String, Risk)> = None;
    for skill in &index.skills {
        let names = skill.services.iter().any(|service| service == target)
            || skill
                .screens
                .iter()
                .any(|screen| !screen.is_empty() && (target == screen || target.starts_with(&format!("{screen}/"))));
        if !names {
            continue;
        }
        let was_loaded = crate::llm::run::quietly(|| loaded_in(facade, conversation_id, &skill.name))
            .ok()
            .flatten()
            .is_some();
        if was_loaded && best.as_ref().is_none_or(|(_, risk)| skill.risk() > *risk) {
            best = Some((skill.name.clone(), skill.risk()));
        }
    }
    best
}

/// [`risk_for`] in a run: the skills of its user, agent and surface loaded in its conversation. None with no run.
pub fn risk_of_write(ctx: &crate::llm::run::context::TurnContext<'_>, target: &str) -> Option<(String, Risk)> {
    let run = ctx.run?;
    let (factory, facade) = (factory()?, facade()?);
    let index = SkillIndex::for_audience(
        &factory,
        &facade,
        &Audience {
            user_id: Some(&ctx.user.user_id),
            agent_id: Some(&ctx.agent.agent_id),
            surface: Some(ctx.surface),
        },
    );
    risk_for(&facade, &index, &run.conversation_id, target)
}
