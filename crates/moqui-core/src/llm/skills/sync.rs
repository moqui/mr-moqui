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

//! The skill files into `AgentSkill` rows (plan 4.9): `skill/**/*.md` of the framework, then of each component in
//! load order, each file in a transaction of its own. A file that does not read, or names a skill an earlier file
//! named, is logged by name and left out while the others go on; a row whose file is gone is retired, unless its
//! directory could not be read. A row keeps its id, counts and an admin's lock across syncs; its version goes up
//! when its file changed. Each component may also ship `skill/aliases.json` (`{"term": ["alias", …]}`), the domain
//! words retrieval and `search` add; they are kept in memory, not in rows.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, RwLock};

use moqui_lib::error::Error;
use moqui_lib::object::Value;

use super::{parse_skill, Provenance, Skill, PUBLISHED, RETIRED, SKILL};
use crate::context::ExecutionContextFactory;
use crate::entity::{fence, EntityFacade};
use crate::llm::run::{now_millis, quietly, stamp};
use crate::llm::{int_of, text_of};
use crate::transaction::TransactionAttribute;

/// The factory's skill state: whether the files were synced, the component aliases, and the embeddings of the
/// semantic branch by skill and content hash.
#[derive(Default)]
pub struct SkillState {
    synced: AtomicBool,
    running: Mutex<()>,
    aliases: RwLock<BTreeMap<String, Vec<String>>>,
    pub(crate) embeddings: super::retrieval::EmbeddingCache,
}

impl SkillState {
    pub fn aliases(&self) -> BTreeMap<String, Vec<String>> {
        self.aliases.read().map(|aliases| aliases.clone()).unwrap_or_default()
    }

    fn set_aliases(&self, aliases: BTreeMap<String, Vec<String>>) {
        if let Ok(mut known) = self.aliases.write() {
            *known = aliases;
        }
    }
}

pub fn state(factory: &ExecutionContextFactory) -> std::sync::Arc<SkillState> {
    factory.extension(|factory| {
        let state = SkillState::default();
        // the aliases need no database: read them now, so search has them before a sync
        state.set_aliases(read_dirs(&skill_dirs(factory)).aliases);
        state
    })
}

/// What a sync did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub added: usize,
    pub changed: usize,
    pub unchanged: usize,
    pub retired: usize,
    /// One line a file left out or a row not written, naming it.
    pub problems: Vec<String>,
}

/// The files read.
#[derive(Clone, Debug, Default)]
pub struct ReadSkills {
    /// In order, each with its `location` and provenance.
    pub skills: Vec<Skill>,
    pub aliases: BTreeMap<String, Vec<String>>,
    pub problems: Vec<String>,
    /// The `component://` prefixes of directories that could not be read (their rows are not retired).
    pub unread: Vec<String>,
}

/// The `skill/` directories of the framework and the components, in load order, with each one's component name.
pub fn skill_dirs(factory: &ExecutionContextFactory) -> Vec<(String, PathBuf)> {
    let mut dirs: Vec<(String, PathBuf)> = vec![("framework".to_string(), factory.framework_dir().join("skill"))];
    for component in factory.components() {
        let dir = component.location.join("skill");
        if !dirs.iter().any(|(_, known)| known == &dir) {
            dirs.push((component.name.clone(), dir));
        }
    }
    dirs
}

/// The skills of these directories, as [`sync`] reads them: failure tolerant, a second skill of a name left out.
pub fn read_dirs(dirs: &[(String, PathBuf)]) -> ReadSkills {
    let mut read = ReadSkills::default();
    for (component, dir) in dirs {
        if !dir.is_dir() {
            continue;
        }
        let prefix = format!("component://{component}/skill/");
        let mut files = Vec::new();
        if !markdown_files(dir, &mut files) {
            read.unread.push(prefix.clone());
        }
        files.sort();
        let provenance = if component == "framework" {
            Provenance::Framework
        } else {
            Provenance::Component
        };
        for file in files {
            let relative = file
                .strip_prefix(dir)
                .map(|path| path.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| file.display().to_string());
            let location = format!("{prefix}{relative}");
            let parsed = std::fs::read_to_string(&file)
                .map_err(|error| error.to_string())
                .and_then(|text| parse_skill(&text));
            match parsed {
                Ok(skill) if read.skills.iter().any(|known| known.name == skill.name) => {
                    let problem = format!("skill {} in {location} is already defined, so it is left out", skill.name);
                    tracing::error!("{problem}");
                    read.problems.push(problem);
                }
                Ok(mut skill) => {
                    skill.location = location;
                    skill.provenance = provenance;
                    read.skills.push(skill);
                }
                Err(problem) => {
                    let problem = format!("skill file {location} is left out: {problem}");
                    tracing::error!("{problem}");
                    read.problems.push(problem);
                }
            }
        }
        let aliases = dir.join("aliases.json");
        if aliases.is_file() {
            match std::fs::read_to_string(&aliases)
                .map_err(|error| error.to_string())
                .and_then(|text| {
                    serde_json::from_str::<BTreeMap<String, Vec<String>>>(&text).map_err(|error| error.to_string())
                }) {
                Ok(found) => {
                    for (term, words) in found {
                        let known = read.aliases.entry(term.to_lowercase()).or_default();
                        for word in words {
                            let word = word.to_lowercase();
                            if !known.contains(&word) {
                                known.push(word);
                            }
                        }
                    }
                }
                Err(problem) => {
                    let problem = format!("skill aliases {prefix}aliases.json are left out: {problem}");
                    tracing::error!("{problem}");
                    read.problems.push(problem);
                }
            }
        }
    }
    read
}

/// The Markdown files under `dir`; false when a directory of it could not be read (logged).
fn markdown_files(dir: &Path, out: &mut Vec<PathBuf>) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        tracing::error!("skill directory {} could not be read", dir.display());
        return false;
    };
    let mut whole = true;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            whole &= markdown_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "md") {
            out.push(path);
        }
    }
    whole
}

/// Sync the files of the factory's components into `AgentSkill` rows, once per factory (a later call does nothing);
/// [`sync`] does it again on demand. A failure is logged.
pub fn ensure_synced(factory: &ExecutionContextFactory, facade: &EntityFacade) {
    let state = state(factory);
    if state.synced.load(Ordering::SeqCst) {
        return;
    }
    let Ok(_running) = state.running.lock() else {
        return;
    };
    if state.synced.load(Ordering::SeqCst) {
        return;
    }
    match sync(factory, facade) {
        Ok(report) => {
            tracing::info!(
                "skills synced: {} added, {} changed, {} unchanged, {} retired, {} problems",
                report.added,
                report.changed,
                report.unchanged,
                report.retired,
                report.problems.len()
            );
            state.synced.store(true, Ordering::SeqCst);
        }
        // not tried again on every use: System › LLM › Skills › Sync Files tries again
        Err(error) => {
            tracing::error!("the skill files could not be synced (Sync Files on System › LLM › Skills tries again): {error}");
            state.synced.store(true, Ordering::SeqCst);
        }
    }
}

/// Sync the files now. Every file is its own transaction (a new one: a sync inside a request's transaction does not
/// join it), and the fence of a read tool lifts for it: the rows are the framework's bookkeeping.
///
/// # Errors
///
/// Returns an error when the file rows cannot be read at all; a file or row that fails is a problem of the report.
pub fn sync(factory: &ExecutionContextFactory, facade: &EntityFacade) -> Result<SyncReport, Error> {
    let read = read_dirs(&skill_dirs(factory));
    state(factory).set_aliases(read.aliases.clone());
    sync_skills(facade, &read)
}

/// Sync `read` into the rows (the part of [`sync`] past reading the files).
///
/// # Errors
///
/// Returns an error when the file rows cannot be read at all.
pub fn sync_skills(facade: &EntityFacade, read: &ReadSkills) -> Result<SyncReport, Error> {
    let _lifted = fence::lift();
    let mut report = SyncReport {
        problems: read.problems.clone(),
        ..SyncReport::default()
    };
    let rows = quietly(|| {
        facade
            .find(SKILL)?
            .condition(moqui_db::sql::Condition::is_not_null("sourceLocation"))
            .condition(moqui_db::sql::Condition::is_null("ownerUserId"))
            .disable_authz()
            .list()
    })?;
    let mut by_location: HashMap<String, crate::entity::EntityValue> = HashMap::new();
    for row in rows.into_values() {
        if let Some(location) = text_of(&row, "sourceLocation") {
            by_location.insert(location, row);
        }
    }
    let now = now_millis();
    let tx = facade.transactions_for(SKILL)?;
    for skill in &read.skills {
        let existing = by_location.remove(&skill.location);
        let outcome = quietly(|| {
            tx.run(TransactionAttribute::ForceNew, None, || {
                match existing.clone() {
                    Some(mut row) => {
                        let same = text_of(&row, "contentHash").as_deref() == Some(skill.content_hash.as_str())
                            && text_of(&row, "statusId").as_deref() == Some(PUBLISHED)
                            && text_of(&row, "skillName").as_deref() == Some(skill.name.as_str());
                        if same {
                            return Ok(Outcome::Unchanged);
                        }
                        skill.fill_row(&mut row);
                        // the file's lock, or an admin's lock kept
                        let locked = skill.locked || crate::llm::flag_of(&row, "locked").unwrap_or(false);
                        row.set("locked", Value::string(if locked { "Y" } else { "N" }));
                        row.set("statusId", Value::string(PUBLISHED));
                        row.set("provenance", Value::string(skill.provenance.as_str()));
                        let version = int_of(&row, "versionNum").unwrap_or(0) + 1;
                        row.set("versionNum", Value::I64(version.max(skill.version)));
                        row.set("lastUpdatedDate", stamp(now));
                        row.update(facade)?;
                        Ok(Outcome::Changed)
                    }
                    None => {
                        let mut row = facade.make_value(SKILL)?;
                        row.set_sequenced_id_primary(facade)?;
                        skill.fill_row(&mut row);
                        row.set("sourceLocation", Value::string(&skill.location));
                        row.set("locked", Value::string(if skill.locked { "Y" } else { "N" }));
                        row.set("statusId", Value::string(PUBLISHED));
                        row.set("provenance", Value::string(skill.provenance.as_str()));
                        row.set("versionNum", Value::I64(skill.version));
                        row.set("useCount", Value::I64(0));
                        row.set("successCount", Value::I64(0));
                        row.set("lastUpdatedDate", stamp(now));
                        row.create(facade)?;
                        Ok(Outcome::Added)
                    }
                }
            })
        });
        match outcome {
            Ok(Outcome::Added) => report.added += 1,
            Ok(Outcome::Changed) => report.changed += 1,
            Ok(Outcome::Unchanged) => report.unchanged += 1,
            Err(error) => {
                let problem = format!("skill {} of {} was not stored: {error}", skill.name, skill.location);
                tracing::error!("{problem}");
                report.problems.push(problem);
            }
        }
    }
    // rows whose file is gone: retired, unless their directory could not be read
    for (location, row) in by_location {
        if text_of(&row, "statusId").as_deref() == Some(RETIRED)
            || read.unread.iter().any(|prefix| location.starts_with(prefix))
        {
            continue;
        }
        let retired = quietly(|| {
            tx.run(TransactionAttribute::ForceNew, None, || {
                let mut row = row.clone();
                row.set("statusId", Value::string(RETIRED));
                row.set("lastUpdatedDate", stamp(now));
                row.update(facade)
            })
        });
        match retired {
            Ok(()) => report.retired += 1,
            Err(error) => {
                let problem = format!("the skill row of {location}, whose file is gone, was not retired: {error}");
                tracing::error!("{problem}");
                report.problems.push(problem);
            }
        }
    }
    Ok(report)
}

enum Outcome {
    Added,
    Changed,
    Unchanged,
}
