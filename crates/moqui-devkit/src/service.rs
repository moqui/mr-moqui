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

//! `org.moqui.dev.DevToolsServices`: the framework's development tools (this crate) as services, on dev
//! instances only (plans/design-dev-aide.md 17), but for the index reads of an allowed project, which answer on any
//! instance for `AIDE_READ` (7.4).

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

use moqui_core::context::ExecutionContext;
use moqui_core::entity::EntityFacade;
use moqui_core::extension::RustServiceFn;
use moqui_core::rust_service;

use crate::project::cache::ProjectIndex;

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    rust_service!(map, restart_server)?;
    rust_service!(map, apply_xml_ops)?;
    rust_service!(map, get_schema_digest)?;
    rust_service!(map, get_xml_outline)?;
    rust_service!(map, find_artifacts)?;
    rust_service!(map, get_artifact)?;
    rust_service!(map, get_artifact_refs)?;
    rust_service!(map, get_artifact_at)?;
    rust_service!(map, get_artifact_bindings)?;
    rust_service!(map, get_artifact_extensions)?;
    rust_service!(map, rebuild_artifact_index)?;
    crate::deploy::register(map)?;
    Ok(())
}

/// Whether the current user may use the development tools here: a dev instance, and the `DEV_AIDE` permission or the
/// `ADMIN` group. Adds the error and answers false otherwise.
fn allowed(what: &str) -> bool {
    if !moqui_core::devtools::is_dev_instance() {
        ExecutionContext::with_current(|ec| {
            ec.message()
                .add_error(format!("{what} is only available when instance_purpose is dev"));
        });
        return false;
    }
    let permitted = moqui_core::user::has_permission("DEV_AIDE").unwrap_or(false)
        || moqui_core::user::is_in_group("ADMIN").unwrap_or(false);
    if !permitted {
        ExecutionContext::with_current(|ec| {
            ec.message().add_error(format!("{what} needs the DEV_AIDE permission"));
        });
    }
    permitted
}

/// Whether the current user may read the artifact index here: a dev instance, and one of the dev-aide's permissions
/// (`DEV_AIDE_VIEW`, `DEV_AIDE_EXTEND`, `DEV_AIDE`) or the `ADMIN` group. Adds the error and answers false otherwise.
fn allowed_read(what: &str) -> bool {
    if !moqui_core::devtools::is_dev_instance() {
        ExecutionContext::with_current(|ec| {
            ec.message()
                .add_error(format!("{what} is only available when instance_purpose is dev"));
        });
        return false;
    }
    let permitted = dev_permitted();
    if !permitted {
        ExecutionContext::with_current(|ec| {
            ec.message()
                .add_error(format!("{what} needs the DEV_AIDE_VIEW permission"));
        });
    }
    permitted
}

/// One of the dev-aide's permissions or the `ADMIN` group.
fn dev_permitted() -> bool {
    moqui_core::devtools::DEV_AIDE_PERMISSIONS
        .iter()
        .any(|permission| moqui_core::user::has_permission(permission).unwrap_or(false))
        || moqui_core::user::is_in_group("ADMIN").unwrap_or(false)
}

/// The permission that reads the index of an allowed project on any instance (plans/design-dev-aide.md 7.4), apart
/// from the dev-aide's tiers; the framework's seed grants it to `ADMIN` and `AIDE_READERS`.
pub(crate) const AIDE_READ: &str = "AIDE_READ";

/// The projects an admin allowed the index of to be read.
const PROJECT: &str = "moqui.devtools.DevArtifactProject";

/// The one error for a project that is not allowed: it does not say whether the id is there.
pub(crate) const NOT_ALLOWED: &str = "projectId: not an allowed project on this instance";

/// Who reads the index, for [`choose_source`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct Access {
    /// `instance_purpose` dev.
    pub dev: bool,
    /// One of the dev-aide's permissions or `ADMIN` ([`dev_permitted`]).
    pub dev_reader: bool,
    /// [`AIDE_READ`] or `ADMIN`.
    pub reader: bool,
}

/// A `DevArtifactProject` row as [`choose_source`] needs it.
#[derive(Clone, Debug)]
pub(crate) struct ProjectRow {
    pub directory: String,
    /// `projectLayout` java: read as a Java Moqui checkout.
    pub java: bool,
    /// `allowed` Y.
    pub allowed: bool,
}

/// Where a read of the index answers from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    /// The server's own rows.
    Own,
    /// The directory in `project`, any project on this machine ([`project_in`]).
    Directory,
    /// An allowed project's directory.
    Allowed { directory: String, java: bool },
}

/// Where a read answers from, or why it is refused (plans/design-dev-aide.md 7.4):
///
/// - `projectId`: an allowed `DevArtifactProject`, for an [`AIDE_READ`] reader on any instance or a dev-aide reader on
///   a dev instance. Its own index only: a miss is a miss, never answered from this server's rows.
/// - `project` (a directory) or neither: as before, only for a dev-aide reader on a dev instance. Off dev no path of
///   a request is read, and this server's own index is not kept.
///
/// `lookup` finds a project row by its id; it is not called when the access is refused.
pub(crate) fn choose_source(
    access: Access,
    what: &str,
    project_id: &str,
    project: &str,
    lookup: impl FnOnce(&str) -> Option<ProjectRow>,
) -> Result<Source, String> {
    let dev = access.dev && access.dev_reader;
    if !dev && !access.reader {
        return Err(format!(
            "{what} needs the {AIDE_READ} permission (or a Dev AIDE permission on a dev instance)"
        ));
    }
    let (project_id, project) = (project_id.trim(), project.trim());
    if !project_id.is_empty() {
        if !project.is_empty() {
            return Err(format!("{what}: name the project by projectId or by project, not both"));
        }
        return match lookup(project_id) {
            Some(row) if row.allowed && !row.directory.trim().is_empty() => Ok(Source::Allowed {
                directory: row.directory,
                java: row.java,
            }),
            _ => Err(NOT_ALLOWED.to_string()),
        };
    }
    if !dev {
        return Err(format!(
            "{what}: name an allowed projectId; a project directory and this server's own index are read only on a dev \
             instance with a Dev AIDE permission"
        ));
    }
    Ok(if project.is_empty() {
        Source::Own
    } else {
        Source::Directory
    })
}

/// The project row of `project_id`, read with authz off (the service checked the reader). A row that cannot be read
/// is logged and taken as not there, so the read is refused.
fn project_row(facade: &EntityFacade, project_id: &str) -> Option<ProjectRow> {
    let found = facade
        .find(PROJECT)
        .and_then(|find| find.disable_authz().eq("projectId", Value::string(project_id)).one());
    match found {
        Ok(row) => row.map(|row| {
            let field = |name: &str| {
                row.get(name)
                    .map(moqui_lib::object::to_plain_string)
                    .unwrap_or_default()
            };
            ProjectRow {
                directory: field("directory"),
                java: field("projectLayout") == "java",
                allowed: field("allowed") == "Y",
            }
        }),
        Err(err) => {
            tracing::error!("Artifact index: project {project_id} was not read: {err}");
            None
        }
    }
}

fn text_in(parameters: &BTreeMap<String, Value>, name: &str) -> String {
    parameters
        .get(name)
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default()
}

/// A list parameter given as a list or a comma separated text.
fn list_in(parameters: &BTreeMap<String, Value>, name: &str) -> Vec<String> {
    match parameters.get(name) {
        Some(Value::List(items)) => items
            .iter()
            .map(moqui_lib::object::to_plain_string)
            .filter(|item| !item.is_empty())
            .collect(),
        Some(value) => moqui_lib::object::to_plain_string(value)
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect(),
        None => Vec::new(),
    }
}

fn json_value(value: &impl serde::Serialize) -> Value {
    moqui_lib::json::from_json(&serde_json::to_value(value).unwrap_or(serde_json::Value::Null))
}

/// Run `body` with the factory, answering its error as a message.
fn with_index<T>(
    body: impl FnOnce(&std::sync::Arc<moqui_core::context::ExecutionContextFactory>) -> Result<T, LibError>,
) -> Option<T> {
    let factory = ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory()))?;
    match body(&factory) {
        Ok(value) => Some(value),
        Err(err) => {
            ExecutionContext::with_current(|ec| ec.message().add_error(err.to_string()));
            None
        }
    }
}

/// The project a call names in `project` (another Moqui project on this machine, [`crate::project::cache::requested`]):
/// `Ok(None)` for the server's own runtime. Its error is added as a message and answered `Err`.
fn project_in(
    parameters: &BTreeMap<String, Value>,
    model: bool,
) -> Result<Option<std::sync::Arc<crate::project::cache::ProjectIndex>>, ()> {
    let named = text_in(parameters, "project");
    if named.trim().is_empty() {
        return Ok(None);
    }
    let factory = ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory())).ok_or(())?;
    let java = text_in(parameters, "projectLayout") == "java";
    crate::project::cache::requested(&named, java, &factory, model).map_err(|err| {
        ExecutionContext::with_current(|ec| ec.message().add_error(err.to_string()));
    })
}

/// The index a read names, with the access to it checked ([`choose_source`]): `Ok(None)` for the server's own rows,
/// else a project's index. The error is added as a message and answered `Err`.
fn source_in(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
    what: &str,
    model: bool,
) -> Result<Option<Arc<ProjectIndex>>, ()> {
    let add_error = |message: String| {
        ExecutionContext::with_current(|ec| ec.message().add_error(message));
    };
    let access = Access {
        dev: moqui_core::devtools::is_dev_instance(),
        dev_reader: dev_permitted(),
        reader: moqui_core::user::has_permission(AIDE_READ).unwrap_or(false)
            || moqui_core::user::is_in_group("ADMIN").unwrap_or(false),
    };
    let chosen = choose_source(
        access,
        what,
        &text_in(parameters, "projectId"),
        &text_in(parameters, "project"),
        |id| project_row(facade, id),
    );
    match chosen.map_err(add_error)? {
        Source::Own => Ok(None),
        Source::Directory => project_in(parameters, model),
        Source::Allowed { directory, java } => {
            let factory = ExecutionContext::with_current(|ec| Arc::clone(ec.factory())).ok_or(())?;
            let id = text_in(parameters, "projectId");
            crate::project::cache::allowed(&directory, java, &factory, model)
                .map(Some)
                .map_err(|err| {
                    // the directory and why stay in the log: a reader is not told the server's paths
                    tracing::error!("Dev AIDE: allowed project {} ({directory}) not read: {err}", id.trim());
                    add_error(format!("projectId {}: not readable", id.trim()));
                })
        }
    }
}

/// A read of the index: `body` answers from the index [`source_in`] chose, with the stamp of that index as `indexStamp`
/// (`hash`, `builtAt`, `files`), so a caller can say what it read against. The stamp is taken first, so a change while
/// `body` reads leaves it older than the answer, never newer; a stamp that cannot be made is logged and left out.
fn read_index(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
    model: bool,
    body: impl FnOnce(Option<&ProjectIndex>, &mut BTreeMap<String, Value>),
) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    let Ok(project) = source_in(parameters, facade, "The artifact index", model) else {
        return out;
    };
    let stamp = match project.as_deref() {
        Some(project) => Some(project.stamp()),
        None => ExecutionContext::with_current(|ec| Arc::clone(ec.factory())).and_then(|factory| {
            crate::index::stamp(&factory)
                .map_err(|err| tracing::error!("Dev AIDE: the index stamp was not made: {err}"))
                .ok()
        }),
    };
    body(project.as_deref(), &mut out);
    if let Some(stamp) = stamp {
        out.insert("indexStamp".to_string(), json_value(&stamp));
    }
    out
}

/// `find#Artifacts`: the artifacts whose names match a text (plans/design-dev-aide.md 7.2). Out: `artifacts`,
/// `indexStamp`.
fn find_artifacts(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let limit = text_in(parameters, "limit")
        .parse::<usize>()
        .unwrap_or(50)
        .clamp(1, 500);
    let (text, kinds, components) = (
        text_in(parameters, "text"),
        list_in(parameters, "kinds"),
        list_in(parameters, "components"),
    );
    Ok(read_index(parameters, facade, false, |project, out| {
        let found = match project {
            Some(project) => Some(project.find(&text, &kinds, &components, limit)),
            None => with_index(|factory| crate::index::find(factory, &text, &kinds, &components, limit)),
        };
        if let Some(found) = found {
            out.insert("artifacts".to_string(), json_value(&found));
        }
    }))
}

/// `get#Artifact`: one artifact (including `parameter:<owner>@<direction>.<name>`) by its id,
/// or the best match of a name. Out: `artifact`, with its declared children in `parts`, `indexStamp`.
fn get_artifact(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let name = text_in(parameters, "artifactId");
    Ok(read_index(parameters, facade, true, |project, out| {
        let (found, parts, parameters_of) = match project {
            Some(project) => {
                let found = project.named(&name);
                let parts = found
                    .as_ref()
                    .map(|artifact| project.children(&artifact.id))
                    .unwrap_or_default();
                let params = found
                    .as_ref()
                    .filter(|artifact| artifact.kind == "service")
                    .and_then(|artifact| project.service_parameters(&artifact.name));
                (found, parts, params)
            }
            None => {
                let Some(found) = with_index(|factory| crate::index::named(factory, &name)) else {
                    return;
                };
                let parts = found
                    .as_ref()
                    .and_then(|artifact| with_index(|factory| crate::index::children(factory, &artifact.id)))
                    .unwrap_or_default();
                let params = found
                    .as_ref()
                    .filter(|artifact| artifact.kind == "service")
                    .and_then(|artifact| moqui_core::service::define::find(&artifact.name))
                    .map(|def| crate::project::cache::parameters_of(&def));
                (found, parts, params)
            }
        };
        if let Some(found) = found {
            out.insert("artifact".to_string(), json_value(&found));
            // an entity's fields, a screen's transitions and forms, or an owner's declared parameters
            out.insert("parts".to_string(), json_value(&parts));
            if let Some(params) = parameters_of {
                out.insert("parameters".to_string(), json_value(&params));
            }
        }
    }))
}

/// `get#ArtifactRefs`: what uses an artifact (`direction` in) or what it uses (out). Out: `refs`, `indexStamp`.
/// Parameter IDs include incoming field-map references to a service's declared input parameter.
fn get_artifact_refs(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let direction = crate::index::Direction::parse(&text_in(parameters, "direction"));
    let (id, kinds) = (text_in(parameters, "artifactId"), list_in(parameters, "kinds"));
    Ok(read_index(parameters, facade, false, |project, out| {
        let found = match project {
            Some(project) => Some(project.refs(&id, direction, &kinds)),
            None => with_index(|factory| crate::index::refs(factory, &id, direction, &kinds)),
        };
        if let Some(found) = found {
            out.insert("refs".to_string(), json_value(&found));
        }
    }))
}

/// `get#ArtifactBindings`: the values flowing into calls' parameters, by the artifact making the call
/// (`fromArtifactId`), the parameter (`toId`) or both. Out: `bindings`, `indexStamp`.
fn get_artifact_bindings(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let (from, to) = (
        text_in(parameters, "fromArtifactId").trim().to_string(),
        text_in(parameters, "toId").trim().to_string(),
    );
    if from.is_empty() && to.is_empty() {
        ExecutionContext::with_current(|ec| ec.message().add_error("fromArtifactId or toId is required".to_string()));
        return Ok(BTreeMap::new());
    }
    Ok(read_index(parameters, facade, false, |project, out| {
        let found = match project {
            Some(project) => Some(project.bindings(&from, &to)),
            None => {
                let some = |value: &str| (!value.is_empty()).then(|| value.to_string());
                let (from, to) = (some(&from), some(&to));
                with_index(|factory| crate::index::bindings(factory, from.as_deref(), to.as_deref()))
            }
        };
        if let Some(found) = found {
            out.insert("bindings".to_string(), json_value(&found));
        }
    }))
}

/// `get#ArtifactAt`: the reference or definition at a line of a file, for go to definition. Out: `reference`,
/// `artifact`, `indexStamp`.
fn get_artifact_at(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let line = text_in(parameters, "line").parse::<u32>().unwrap_or(0);
    let (path, word) = (text_in(parameters, "path"), text_in(parameters, "word"));
    Ok(read_index(parameters, facade, false, |project, out| {
        let found = match project {
            Some(project) => {
                let Some(file) = project.file(&path) else {
                    ExecutionContext::with_current(|ec| {
                        ec.message()
                            .add_error(format!("{path} is in no component of the project"));
                    });
                    return;
                };
                project.at(&file.display().to_string(), line, &word)
            }
            None => with_index(|factory| crate::index::at(factory, &path, line, &word)).flatten(),
        };
        if let Some((reference, artifact)) = found {
            out.insert("reference".to_string(), json_value(&reference));
            out.insert("artifact".to_string(), json_value(&artifact));
        }
    }))
}

/// `get#ArtifactExtensions`: the extensions of components (every component when none is named) with whether their
/// targets are there, broken first. Out: `extensions`, `brokenCount`, `indexStamp`.
fn get_artifact_extensions(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let components = list_in(parameters, "components");
    Ok(read_index(parameters, facade, false, |project, out| {
        let found = match project {
            Some(project) => Some(project.extensions(&components)),
            None => with_index(|factory| crate::index::extensions(factory, &components)),
        };
        if let Some(found) = found {
            let broken = found.iter().filter(|extension| !extension.resolved).count();
            out.insert("extensions".to_string(), json_value(&found));
            out.insert(
                "brokenCount".to_string(),
                Value::I64(i64::try_from(broken).unwrap_or(0)),
            );
        }
    }))
}

/// `rebuild#ArtifactIndex`: read every file whose hash changed (all of them with `full`), now. Out: `report`.
fn rebuild_artifact_index(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    if !allowed_read("The artifact index") {
        return Ok(out);
    }
    let full = text_in(parameters, "full") == "true";
    let named = text_in(parameters, "project");
    if !named.trim().is_empty() {
        // another project: a full rebuild boots it again; else its files are looked at now
        if full {
            crate::project::cache::forget_requested(&named, text_in(parameters, "projectLayout") == "java");
        }
        let Ok(project) = project_in(parameters, true) else {
            return Ok(out);
        };
        if let Some(project) = project {
            out.insert("report".to_string(), json_value(&project.summary()));
            return Ok(out);
        }
    }
    let found = with_index(|factory| crate::index::rebuild(factory, full));
    if let Some(report) = found {
        out.insert("report".to_string(), json_value(&report));
    }
    Ok(out)
}

/// `apply#XmlOps`: the operations of the lossless XML layer (`moqui_lib::xml_edit`, plans/design-dev-aide.md 5.2) on a
/// text, answered as the new text and each operation's edits. Pure text: no file is read or written. Out: `text`,
/// `steps` (a list of lists of `{from, to, from16, to16, text}`).
fn apply_xml_ops(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    if !allowed_read("The XML operations") {
        return Ok(out);
    }
    let text = parameters
        .get("text")
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default();
    let ops = parameters
        .get("ops")
        .map(moqui_lib::json::to_json)
        .unwrap_or(serde_json::Value::Null);
    // ops may come as a JSON string (a form post) or a list (a JSON body)
    let ops = match ops {
        serde_json::Value::String(raw) => serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null),
        other => other,
    };
    let ops: Vec<moqui_lib::xml_edit::Op> = match serde_json::from_value(ops) {
        Ok(ops) => ops,
        Err(err) => {
            ExecutionContext::with_current(|ec| ec.message().add_error(format!("ops: {err}")));
            return Ok(out);
        }
    };
    match moqui_lib::xml_edit::apply(&text, &ops) {
        Ok(applied) => {
            let steps = serde_json::to_value(&applied.steps).unwrap_or(serde_json::Value::Null);
            if let Ok(outline) = moqui_lib::xml_edit::outline(&applied.text) {
                out.insert("outline".to_string(), json_value(&outline));
            }
            out.insert("text".to_string(), Value::string(applied.text));
            out.insert("steps".to_string(), moqui_lib::json::from_json(&steps));
        }
        Err(err) => {
            ExecutionContext::with_current(|ec| ec.message().add_error(err.to_string()));
        }
    }
    Ok(out)
}

/// `get#XmlOutline`: the outline of an XML text (plans/design-dev-aide.md 5.2), for the visual views. Out: `outline`;
/// a text that does not parse answers its error as a message.
fn get_xml_outline(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    if !allowed_read("The XML outline") {
        return Ok(out);
    }
    match moqui_lib::xml_edit::outline(&text_in(parameters, "text")) {
        Ok(outline) => {
            out.insert("outline".to_string(), json_value(&outline));
        }
        Err(err) => {
            ExecutionContext::with_current(|ec| ec.message().add_error(err.to_string()));
        }
    }
    Ok(out)
}

/// `get#SchemaDigest`: the digest of the XSDs for a root element (plans/design-dev-aide.md 5.3), for the editor's XML
/// completion and checks. Out: `digest`, `null` for a root no XSD defines.
fn get_schema_digest(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    if !allowed_read("The schema digest") {
        return Ok(out);
    }
    let root = parameters
        .get("rootElement")
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default();
    let digest = ExecutionContext::with_current(|ec| crate::check::xsd_dir(ec.factory()))
        .and_then(|dir| crate::schema::digest(&dir, &root));
    let json = digest.map_or(serde_json::Value::Null, |digest| {
        serde_json::to_value(&*digest).unwrap_or(serde_json::Value::Null)
    });
    out.insert("digest".to_string(), moqui_lib::json::from_json(&json));
    Ok(out)
}

/// `restart#Server`: shut the server down gracefully and run it again (plans/design-dev-aide.md 6.5). Out: `restarting`.
fn restart_server(
    _parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    if restart_soon("Restart") {
        out.insert("restarting".to_string(), Value::Bool(true));
    }
    Ok(out)
}

/// Whether the current user may restart this server (`restart#Server`, a deploy's "and restart"): a dev instance,
/// `DEV_AIDE` or the `ADMIN` group, and a server that can restart itself. Adds no message.
pub(crate) fn restart_allowed() -> bool {
    moqui_core::devtools::is_dev_instance()
        && moqui_core::devtools::restart::can_restart()
        && (moqui_core::user::has_permission("DEV_AIDE").unwrap_or(false)
            || moqui_core::user::is_in_group("ADMIN").unwrap_or(false))
}

/// Restart the server just after this request answers, when [`allowed`] for `what`; false (with the error added)
/// when not.
pub(crate) fn restart_soon(what: &str) -> bool {
    if !allowed(what) {
        return false;
    }
    if !moqui_core::devtools::restart::can_restart() {
        ExecutionContext::with_current(|ec| {
            ec.message()
                .add_error(format!("{what}: this process is not a server that can restart"));
        });
        return false;
    }
    let user = moqui_core::user::session::current_user_id()
        .ok()
        .flatten()
        .unwrap_or_default();
    tracing::warn!("Restart of the server requested by user {user} ({what})");
    // After the response: the shutdown waits for open requests, this one among them.
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(200));
        if !moqui_core::devtools::restart::request_restart() {
            tracing::error!("Restart requested, but this process is not a server that can restart");
        }
    });
    true
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
