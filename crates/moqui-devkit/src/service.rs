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

//! `org.moqui.impl.DevToolsServices`: the framework's development tools (this crate) as services, on dev
//! instances only (plans/design-dev-aide.md 17).

use std::collections::BTreeMap;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

use moqui_core::context::ExecutionContext;
use moqui_core::entity::EntityFacade;
use moqui_core::extension::RustServiceFn;
use moqui_core::rust_service;

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    rust_service!(map, restart_server)?;
    rust_service!(map, apply_xml_ops)?;
    rust_service!(map, get_schema_digest)?;
    rust_service!(map, get_xml_outline)?;
    rust_service!(map, find_artifacts)?;
    rust_service!(map, get_artifact)?;
    rust_service!(map, get_artifact_refs)?;
    rust_service!(map, get_artifact_at)?;
    rust_service!(map, get_artifact_extensions)?;
    rust_service!(map, rebuild_artifact_index)?;
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
    let permitted = ["DEV_AIDE_VIEW", "DEV_AIDE_EXTEND", "DEV_AIDE"]
        .iter()
        .any(|permission| moqui_core::user::has_permission(permission).unwrap_or(false))
        || moqui_core::user::is_in_group("ADMIN").unwrap_or(false);
    if !permitted {
        ExecutionContext::with_current(|ec| {
            ec.message()
                .add_error(format!("{what} needs the DEV_AIDE_VIEW permission"));
        });
    }
    permitted
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
    crate::project::cache::requested(&named, &factory, model).map_err(|err| {
        ExecutionContext::with_current(|ec| ec.message().add_error(err.to_string()));
    })
}

/// `find#Artifacts`: the artifacts whose names match a text (plans/design-dev-aide.md 7.2). Out: `artifacts`.
fn find_artifacts(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    if !allowed_read("The artifact index") {
        return Ok(out);
    }
    let limit = text_in(parameters, "limit")
        .parse::<usize>()
        .unwrap_or(50)
        .clamp(1, 500);
    let Ok(project) = project_in(parameters, false) else {
        return Ok(out);
    };
    if let Some(project) = project {
        let found = project.find(
            &text_in(parameters, "text"),
            &list_in(parameters, "kinds"),
            &list_in(parameters, "components"),
            limit,
        );
        out.insert("artifacts".to_string(), json_value(&found));
        return Ok(out);
    }
    let found = with_index(|factory| {
        crate::index::find(
            factory,
            &text_in(parameters, "text"),
            &list_in(parameters, "kinds"),
            &list_in(parameters, "components"),
            limit,
        )
    });
    if let Some(found) = found {
        out.insert("artifacts".to_string(), json_value(&found));
    }
    Ok(out)
}

/// `get#Artifact`: one artifact (including `parameter:<owner>@<direction>.<name>`) by its id,
/// or the best match of a name. Out: `artifact`, with its declared children in `parts`.
fn get_artifact(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    if !allowed_read("The artifact index") {
        return Ok(out);
    }
    let Ok(project) = project_in(parameters, true) else {
        return Ok(out);
    };
    let name = text_in(parameters, "artifactId");
    let (found, parts, parameters_of) = match &project {
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
                return Ok(out);
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
    Ok(out)
}

/// `get#ArtifactRefs`: what uses an artifact (`direction` in) or what it uses (out). Out: `refs`.
/// Parameter IDs include incoming field-map references to a service's declared input parameter.
fn get_artifact_refs(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    if !allowed_read("The artifact index") {
        return Ok(out);
    }
    let direction = crate::index::Direction::parse(&text_in(parameters, "direction"));
    let Ok(project) = project_in(parameters, false) else {
        return Ok(out);
    };
    if let Some(project) = project {
        let found = project.refs(
            &text_in(parameters, "artifactId"),
            direction,
            &list_in(parameters, "kinds"),
        );
        out.insert("refs".to_string(), json_value(&found));
        return Ok(out);
    }
    let found = with_index(|factory| {
        crate::index::refs(
            factory,
            &text_in(parameters, "artifactId"),
            direction,
            &list_in(parameters, "kinds"),
        )
    });
    if let Some(found) = found {
        out.insert("refs".to_string(), json_value(&found));
    }
    Ok(out)
}

/// `get#ArtifactAt`: the reference or definition at a line of a file, for go to definition. Out: `reference`,
/// `artifact`.
fn get_artifact_at(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    if !allowed_read("The artifact index") {
        return Ok(out);
    }
    let line = text_in(parameters, "line").parse::<u32>().unwrap_or(0);
    let Ok(project) = project_in(parameters, false) else {
        return Ok(out);
    };
    if let Some(project) = project {
        let Some(path) = project.file(&text_in(parameters, "path")) else {
            ExecutionContext::with_current(|ec| {
                ec.message().add_error(format!(
                    "{} is in no component of the project",
                    text_in(parameters, "path")
                ));
            });
            return Ok(out);
        };
        if let Some((reference, artifact)) = project.at(&path.display().to_string(), line, &text_in(parameters, "word"))
        {
            out.insert("reference".to_string(), json_value(&reference));
            out.insert("artifact".to_string(), json_value(&artifact));
        }
        return Ok(out);
    }
    let found = with_index(|factory| {
        crate::index::at(
            factory,
            &text_in(parameters, "path"),
            line,
            &text_in(parameters, "word"),
        )
    });
    if let Some(Some((reference, artifact))) = found {
        out.insert("reference".to_string(), json_value(&reference));
        out.insert("artifact".to_string(), json_value(&artifact));
    }
    Ok(out)
}

/// `get#ArtifactExtensions`: the extensions of components (every component when none is named) with whether their
/// targets are there, broken first. Out: `extensions`, `brokenCount`.
fn get_artifact_extensions(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    if !allowed_read("The artifact index") {
        return Ok(out);
    }
    let Ok(project) = project_in(parameters, false) else {
        return Ok(out);
    };
    let found = match project {
        Some(project) => Some(project.extensions(&list_in(parameters, "components"))),
        None => with_index(|factory| crate::index::extensions(factory, &list_in(parameters, "components"))),
    };
    if let Some(found) = found {
        let broken = found.iter().filter(|extension| !extension.resolved).count();
        out.insert("extensions".to_string(), json_value(&found));
        out.insert(
            "brokenCount".to_string(),
            Value::I64(i64::try_from(broken).unwrap_or(0)),
        );
    }
    Ok(out)
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
            if let Ok(found) = crate::project::requested_dir(&named).and_then(|dir| crate::project::detect(&dir)) {
                crate::project::cache::forget(&found.root);
            }
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
    if !allowed("Restart") {
        return Ok(out);
    }
    let user = moqui_core::user::session::current_user_id()
        .ok()
        .flatten()
        .unwrap_or_default();
    tracing::warn!("Restart of the server requested by user {user}");
    // After the response: the shutdown waits for open requests, this one among them.
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(200));
        if !moqui_core::devtools::restart::request_restart() {
            tracing::error!("Restart requested, but this process is not a server that can restart");
        }
    });
    out.insert("restarting".to_string(), Value::Bool(true));
    Ok(out)
}
