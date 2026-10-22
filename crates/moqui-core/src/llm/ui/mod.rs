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

//! Canvases (plans/LLM-A2A-Assist.md 4.10, D5): generated views in the drawer, the fallback when no screen shows what
//! the user asked for. A canvas dialect is the server half of a client renderer: it parses and checks what the model
//! wrote before anything is shown ([`Dialect::prepare`]). Dialects register through `inventory`
//! ([`submit_dialect!`](crate::submit_dialect)), as tools do: [`openui`] (OpenUI Lang, the default) and [`vue_sfc`]
//! (a Vue 3 component in a sandboxed frame, for an agent whose `canvasDialects` name it).
//!
//! A conversation has one canvas, an `AgentArtifact` of kind `canvas` updated in place ([`Canvas`]): its program (part
//! 1, text), and its data (part 2: the dialect's statement names, the state the user's page posted, the problems the
//! page reported with their hashes and how many revalidations ran). The model's history keeps only what it sent (a
//! whole program once, or a patch), never the merged program again (J5); the tail names the canvas, its version and
//! its open problems. [`proxy`] is the one read proxy of every dialect (J12).

pub mod openui;
pub mod proxy;
pub mod vue_sfc;
pub mod write;

use std::sync::{Arc, OnceLock};

use moqui_lib::error::Error;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use crate::context::ExecutionContextFactory;
use crate::entity::{EntityFacade, EntityValue};
use crate::llm::{int_of, text_of};

pub use inventory::submit;

pub(crate) const ARTIFACT: &str = "moqui.agent.AgentArtifact";
/// The start of the error of a canvas write that another write overtook.
pub const CONFLICT: &str = "canvas conflict";
pub(crate) const ARTIFACT_PART: &str = "moqui.agent.AgentArtifactPart";

/// What the model asked a dialect to show: a whole program, or a patch of the current canvas's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WriteInput {
    pub title: String,
    pub program: Option<String>,
    pub patch: Option<String>,
}

/// A program the dialect checked, ready to show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prepared {
    pub program: String,
    /// The names of its parts (OpenUI statements), for the result the model reads.
    pub statements: Vec<String>,
    /// The writes it can make (OpenUI Mutations), which run only through the canvas's action call.
    pub mutations: Vec<String>,
    /// For a Vue SFC canvas, the `/apps` read paths it declared (its actions' paths and its `<reads>` block): the read
    /// proxy answers only these. Empty for OpenUI, whose Query paths are checked as they run.
    pub reads: Vec<String>,
}

/// Why a program is not shown: the error JSON the model reads (each problem naming its skill section and rule), and
/// the problems themselves for the events.
#[derive(Clone, Debug, PartialEq)]
pub struct Refused {
    pub error: String,
    pub problems: Vec<Json>,
}

/// The server half of a canvas dialect.
pub trait Dialect: Send + Sync {
    /// `openui`, `vue-sfc`.
    fn name(&self) -> &'static str;
    fn label(&self) -> &'static str;
    /// The skill that holds the language, which gates `write_ui` for this dialect.
    fn skill(&self) -> &'static str;
    /// Offered only on a development instance (the planned `screen-xml`).
    fn dev_only(&self) -> bool {
        false
    }
    /// Built in this server: a placeholder dialect is registered but not offered.
    fn available(&self) -> bool {
        true
    }
    /// Parse and check `input` against the current canvas (`previous`, its program) before anything is shown.
    fn prepare(
        &self,
        factory: &ExecutionContextFactory,
        previous: Option<&str>,
        input: &WriteInput,
        paths: &dyn openui::Paths,
    ) -> Result<Prepared, Refused>;
}

/// A registered dialect, submitted with [`submit_dialect!`](crate::submit_dialect).
pub struct DialectRegistration {
    pub make: fn() -> Arc<dyn Dialect>,
}

inventory::collect!(DialectRegistration);

/// The `make` of a dialect that is `Default`.
pub fn make_default<T: Dialect + Default + 'static>() -> Arc<dyn Dialect> {
    Arc::new(T::default())
}

/// Register a canvas dialect (`Dialect + Default`) from any linked crate.
#[macro_export]
macro_rules! submit_dialect {
    ($dialect:ty) => {
        $crate::llm::ui::submit! {
            $crate::llm::ui::DialectRegistration { make: $crate::llm::ui::make_default::<$dialect> }
        }
    };
}

/// Every registered dialect, sorted by name; a second one of a name is logged and left out.
pub fn dialects() -> &'static [Arc<dyn Dialect>] {
    static DIALECTS: OnceLock<Vec<Arc<dyn Dialect>>> = OnceLock::new();
    DIALECTS.get_or_init(|| {
        let mut list: Vec<Arc<dyn Dialect>> = inventory::iter::<DialectRegistration>
            .into_iter()
            .map(|registration| (registration.make)())
            .collect();
        list.sort_by(|left, right| left.name().cmp(right.name()));
        let mut kept: Vec<Arc<dyn Dialect>> = Vec::with_capacity(list.len());
        for dialect in list {
            if kept.last().is_some_and(|last| last.name() == dialect.name()) {
                tracing::error!("a second canvas dialect named {} is registered, so it is left out", dialect.name());
                continue;
            }
            kept.push(dialect);
        }
        kept
    })
}

/// The dialect `name`.
pub fn dialect(name: &str) -> Option<Arc<dyn Dialect>> {
    dialects().iter().find(|dialect| dialect.name() == name.trim()).cloned()
}

/// The dialects a drawer agent offers: those built here, not a development one on another instance, whose
/// capability the surface has (`ui.write:<name>`).
pub fn offered(caps: &crate::llm::tools::ClientCaps, dev: bool) -> Vec<Arc<dyn Dialect>> {
    dialects()
        .iter()
        .filter(|dialect| dialect.available() && (dev || !dialect.dev_only()))
        .filter(|dialect| caps.has(&format!("ui.write:{}", dialect.name())))
        .cloned()
        .collect()
}

/// A conversation's canvas as stored.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Canvas {
    pub artifact_id: String,
    pub conversation_id: String,
    /// The run that last wrote it.
    pub run_id: String,
    pub dialect: String,
    pub title: String,
    pub version: i64,
    pub program: String,
    /// `{statements, mutations, state, problems: [{hash, skill, rule, problem, statement}], revalidations, closed}`.
    pub data: Json,
    pub updated: Option<i64>,
}

impl Canvas {
    /// The canvas of the conversation, if it has one.
    ///
    /// # Errors
    ///
    /// When the rows cannot be read.
    pub fn of_conversation(facade: &EntityFacade, conversation_id: &str) -> Result<Option<Self>, Error> {
        let row = facade
            .find(ARTIFACT)?
            .eq("conversationId", Value::string(conversation_id))
            .eq("artifactKind", Value::string("canvas"))
            .order_by("lastUpdatedDate", true)
            .limit(1)
            .disable_authz()
            .list()?
            .values()
            .first()
            .cloned();
        row.map(|row| Self::read(facade, &row)).transpose()
    }

    /// The canvas `artifact_id`, if there is one.
    ///
    /// # Errors
    ///
    /// When the rows cannot be read.
    pub fn of_id(facade: &EntityFacade, artifact_id: &str) -> Result<Option<Self>, Error> {
        let row = facade
            .find(ARTIFACT)?
            .eq("artifactId", Value::string(artifact_id))
            .disable_authz()
            .one()?;
        match row {
            Some(row) if text_of(&row, "artifactKind").as_deref() == Some("canvas") => Ok(Some(Self::read(facade, &row)?)),
            _ => Ok(None),
        }
    }

    /// The canvas `artifact_id` read for update, in the transaction in place: a change of its data rereads it so, checks
    /// its version and writes it back before the transaction ends, so two writers never overwrite each other.
    ///
    /// # Errors
    ///
    /// When the rows cannot be read.
    pub fn locked(facade: &EntityFacade, artifact_id: &str) -> Result<Option<Self>, Error> {
        let row = facade
            .find(ARTIFACT)?
            .eq("artifactId", Value::string(artifact_id))
            .for_update(true)
            .disable_authz()
            .one()?;
        match row {
            Some(row) if text_of(&row, "artifactKind").as_deref() == Some("canvas") => Ok(Some(Self::read(facade, &row)?)),
            _ => Ok(None),
        }
    }

    fn read(facade: &EntityFacade, row: &EntityValue) -> Result<Self, Error> {
        let artifact_id = text_of(row, "artifactId").unwrap_or_default();
        let parts = facade
            .find(ARTIFACT_PART)?
            .eq("artifactId", Value::string(&artifact_id))
            .order_by("partSeq", false)
            .disable_authz()
            .list()?;
        let mut program = String::new();
        let mut data = json!({});
        for part in parts.values() {
            match int_of(part, "partSeq") {
                Some(1) => program = text_of(part, "textValue").unwrap_or_default(),
                Some(2) => {
                    data = text_of(part, "dataJson")
                        .and_then(|text| serde_json::from_str(&text).ok())
                        .unwrap_or_else(|| json!({}));
                }
                _ => {}
            }
        }
        Ok(Self {
            artifact_id,
            conversation_id: text_of(row, "conversationId").unwrap_or_default(),
            run_id: text_of(row, "runId").unwrap_or_default(),
            dialect: text_of(row, "dialect").unwrap_or_else(|| "openui".to_string()),
            title: text_of(row, "artifactName").unwrap_or_default(),
            version: int_of(row, "versionNum").unwrap_or(0),
            program,
            data,
            updated: row.get("lastUpdatedDate").and_then(crate::llm::millis_of),
        })
    }

    /// Whether the user closed it.
    pub fn closed(&self) -> bool {
        self.data["closed"].as_bool() == Some(true)
    }

    /// Its open problems (reported by the page, not fixed by a newer version).
    pub fn problems(&self) -> Vec<Json> {
        self.data["problems"].as_array().cloned().unwrap_or_default()
    }

    /// Write a new version (`program` replacing the old one) for `run_id`: the artifact's row and both parts, in the
    /// transaction in place. The existing canvas is read again for update first: when its version moved past
    /// `existing`'s (another write came first) this fails with a conflict and writes nothing. The state the page posted is
    /// kept; the problems and the revalidation count start over.
    ///
    /// # Errors
    ///
    /// When the rows cannot be written.
    pub fn write_version(
        facade: &EntityFacade,
        existing: Option<&Canvas>,
        conversation_id: &str,
        run_id: &str,
        dialect: &str,
        title: &str,
        prepared: &Prepared,
    ) -> Result<Canvas, Error> {
        let now = crate::llm::run::now_millis();
        let current = match existing {
            Some(canvas) => {
                let locked = Self::locked(facade, &canvas.artifact_id)?
                    .ok_or_else(|| Error::new(format!("canvas {} is gone", canvas.artifact_id)))?;
                if locked.version != canvas.version {
                    return Err(Error::new(format!(
                        "{CONFLICT}: canvas {} is at version {}, not {}",
                        canvas.artifact_id, locked.version, canvas.version
                    )));
                }
                Some(locked)
            }
            None => None,
        };
        let existing = current.as_ref();
        let version = existing.map_or(1, |canvas| canvas.version + 1);
        let state = existing.map(|canvas| canvas.data["state"].clone()).unwrap_or(Json::Null);
        let total = existing.and_then(|canvas| canvas.data["revalidationsTotal"].as_i64()).unwrap_or(0);
        let data = json!({
            "statements": prepared.statements,
            "mutations": prepared.mutations,
            "reads": prepared.reads,
            "state": state,
            "problems": [],
            "revalidations": 0,
            "revalidationsTotal": total,
            "closed": false,
        });
        let artifact_id = match existing {
            Some(canvas) => {
                let mut row = facade
                    .find(ARTIFACT)?
                    .eq("artifactId", Value::string(&canvas.artifact_id))
                    .for_update(true)
                    .disable_authz()
                    .one()?
                    .ok_or_else(|| Error::new(format!("canvas {} is gone", canvas.artifact_id)))?;
                row.set("runId", Value::string(run_id));
                row.set("artifactName", Value::string(title));
                row.set("dialect", Value::string(dialect));
                row.set("versionNum", Value::I64(version));
                row.set("lastUpdatedDate", crate::llm::run::stamp(now));
                row.update(facade)?;
                canvas.artifact_id.clone()
            }
            None => {
                let mut row = facade.make_value(ARTIFACT)?;
                row.set_sequenced_id_primary(facade)?;
                row.set("runId", Value::string(run_id));
                row.set("conversationId", Value::string(conversation_id));
                row.set("artifactKind", Value::string("canvas"));
                row.set("artifactName", Value::string(title));
                row.set("dialect", Value::string(dialect));
                row.set("versionNum", Value::I64(version));
                row.set("createdDate", crate::llm::run::stamp(now));
                row.set("lastUpdatedDate", crate::llm::run::stamp(now));
                row.create(facade)?;
                text_of(&row, "artifactId").unwrap_or_default()
            }
        };
        set_part(facade, &artifact_id, 1, "AgpkText", Some(&prepared.program), None)?;
        set_part(facade, &artifact_id, 2, "AgpkData", None, Some(&data))?;
        Ok(Canvas {
            artifact_id,
            conversation_id: conversation_id.to_string(),
            run_id: run_id.to_string(),
            dialect: dialect.to_string(),
            title: title.to_string(),
            version,
            program: prepared.program.clone(),
            data,
            updated: Some(now),
        })
    }

    /// Write its data part again (state, problems, closed), in the transaction in place.
    ///
    /// # Errors
    ///
    /// When the row cannot be written.
    pub fn write_data(&self, facade: &EntityFacade) -> Result<(), Error> {
        set_part(facade, &self.artifact_id, 2, "AgpkData", None, Some(&self.data))
    }

    /// The canvas as the client draws it (`GET conversation`'s `canvas`, the `canvas.open` payload).
    pub fn to_json(&self) -> Json {
        json!({
            "artifactId": self.artifact_id,
            "conversationId": self.conversation_id,
            "runId": self.run_id,
            "kind": self.dialect,
            "title": self.title,
            "version": self.version,
            "program": self.program,
            "statements": self.data["statements"],
            "mutations": self.data["mutations"],
            "reads": self.data["reads"],
            "state": self.data["state"],
            "problems": self.problems(),
            "closed": self.closed(),
            "updatedDate": self.updated,
        })
    }

    /// The line of the tail (plan 7.1): `canvas: "Logins by day" v3 (openui), open problems: …`.
    pub fn tail_line(&self) -> String {
        let mut line = format!("canvas: \"{}\" v{} ({})", self.title, self.version, self.dialect);
        if self.closed() {
            line.push_str(", closed by the user");
        }
        let problems = self.problems();
        if !problems.is_empty() {
            let shown: Vec<String> = problems
                .iter()
                .take(3)
                .map(|problem| {
                    format!(
                        "{} ({})",
                        problem["problem"].as_str().unwrap_or_default(),
                        problem["skill"].as_str().unwrap_or_default()
                    )
                })
                .collect();
            line.push_str(&format!(", problems the page reported: {}", shown.join("; ")));
        }
        line
    }
}

fn set_part(
    facade: &EntityFacade,
    artifact_id: &str,
    seq: i64,
    kind: &str,
    text: Option<&str>,
    data: Option<&Json>,
) -> Result<(), Error> {
    let found = facade
        .find(ARTIFACT_PART)?
        .eq("artifactId", Value::string(artifact_id))
        .eq("partSeq", Value::I64(seq))
        .for_update(true)
        .disable_authz()
        .one()?;
    let mut row = match found {
        Some(row) => row,
        None => {
            let mut row = facade.make_value(ARTIFACT_PART)?;
            row.set("artifactId", Value::string(artifact_id));
            row.set("partSeq", Value::I64(seq));
            row
        }
    };
    let create = row.get("partKindEnumId").is_none();
    row.set("partKindEnumId", Value::string(kind));
    if let Some(text) = text {
        row.set("textValue", Value::string(text));
    }
    if let Some(data) = data {
        row.set("dataJson", Value::string(data.to_string()));
    }
    if create {
        row.create(facade)
    } else {
        row.update(facade)
    }
}

/// The programs of the `write_ui` calls a model is streaming, by tool call index: each delta of the arguments gives
/// the new text of the program (or patch) as a live `canvas.delta` `{index, patch, offset, text}`, which the drawer
/// appends and parses as it streams. The program is read from the partial JSON of the arguments.
#[derive(Debug, Default)]
pub struct StreamingPrograms {
    calls: std::collections::HashMap<u32, (String, usize)>,
}

impl StreamingPrograms {
    pub fn start(&mut self, index: u32, name: &str) {
        if name == "write_ui" {
            self.calls.insert(index, (String::new(), 0));
        }
    }

    /// The canvas delta of `delta` for call `index`, when it is a `write_ui` call and its program grew.
    pub fn args(&mut self, index: u32, delta: &str) -> Option<Json> {
        let (args, sent) = self.calls.get_mut(&index)?;
        args.push_str(delta);
        let (patch, text) = partial_program(args)?;
        let count = text.chars().count();
        if count <= *sent {
            return None;
        }
        let new: String = text.chars().skip(*sent).collect();
        let offset = *sent;
        *sent = count;
        Some(json!({"index": index, "patch": patch, "offset": offset, "text": new}))
    }
}

/// The program (or patch) of the partial JSON arguments of a `write_ui` call: the string value of `program` or
/// `patch` as far as it has come, its escapes decoded; `true` for a patch.
pub fn partial_program(args: &str) -> Option<(bool, String)> {
    let (patch, at) = ["\"program\"", "\"patch\""]
        .iter()
        .enumerate()
        .filter_map(|(which, key)| args.find(key).map(|at| (which == 1, at + key.len())))
        .min_by_key(|(_, at)| *at)?;
    let rest = args[at..].trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => break,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => {}
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if hex.len() < 4 {
                        break;
                    }
                    if let Some(decoded) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(decoded);
                    }
                }
                Some(other) => out.push(other),
                None => break,
            },
            other => out.push(other),
        }
    }
    Some((patch, out))
}

#[cfg(test)]
mod tests;
