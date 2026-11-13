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

//! `org.moqui.devaide.DocServices`: Autodoc in the Dev AIDE (plans/archive/Autodoc.md). `get#ArtifactDoc` makes one doc as
//! the server has its artifacts now, from the dev server's artifact index (the calls observed at run time too), for the
//! Docs screen and the editors' Doc tabs; `export#ArtifactDocs` writes a tree of them into a root, as `moqui autodoc`
//! does, through the root's confinement and the write checks of every save (the extend tier, read-only roots, the
//! upstream lock), in one batch: no reload, check or git status per file, and one audit row. A site component's
//! `template/autodoc/` overrides the framework's templates by name.

use std::path::Path;

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_devkit::autodoc::{self, write, DbSource, DocKind, MemorySource, Options};
use moqui_devkit::index::MemoryIndex;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use serde_json::json;

use crate::audit;
use crate::tier::Tier;
use crate::workspace::{flag, json_value, text, write_atomic, Context};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(get_artifact_doc), get_artifact_doc)?;
    registry.service_function(
        moqui_core::service_location!(export_artifact_docs),
        export_artifact_docs,
    )
}

/// Where the Docs screen is; a doc's path follows it.
const DOC_SCREEN: &str = "/qapps/dev/Artifacts/Doc";

/// The template directories of the site components, which take the place of the framework's templates by name.
fn template_dirs(context: &Context) -> Vec<String> {
    context
        .settings
        .site_components
        .iter()
        .map(|site| format!("component://{site}/template/autodoc"))
        .collect()
}

/// A list parameter given as a list or a comma separated text.
fn list(parameters: &Parameters, key: &str) -> Vec<String> {
    match parameters.get(key) {
        Some(Value::List(items)) => items
            .iter()
            .map(moqui_lib::object::to_plain_string)
            .filter(|item| !item.is_empty())
            .collect(),
        _ => text(parameters, key)
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect(),
    }
}

/// `get#ArtifactDoc`: the doc at a tree path (`entity/mantle/order/OrderHeader.md`) or of an artifact id (a part's
/// opens its owner's), with the way up and the links that open its source.
fn get_artifact_doc(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let started = std::time::Instant::now();
    let source = DbSource::new(std::sync::Arc::clone(&context.factory));
    let plan = autodoc::cached_plan(&context.factory, &source)?;
    let key = [text(parameters, "docPath"), text(parameters, "artifactId")]
        .into_iter()
        .find(|key| !key.is_empty())
        .unwrap_or_else(|| "README.md".to_string());
    let options = Options {
        include_source: flag(parameters, "includeSource"),
        template_dirs: template_dirs(&context),
        ..Options::default()
    };
    // a name (OrderHeader, place#Order) is the index's best match
    let key = if autodoc::find(&plan, &key).is_some() {
        key
    } else {
        moqui_devkit::index::named(&context.factory, &key)?
            .and_then(|row| plan.of_artifact(&row).map(|doc| doc.path.clone()))
            .ok_or_else(|| LibError::new(format!("No doc of {key}")))?
    };
    let doc = autodoc::render_doc(&context.factory, &source, &plan, &options, &key)?;
    tracing::debug!("Autodoc: the doc {} made in {} ms", doc.path, started.elapsed().as_millis());
    let entry = plan
        .at_path(&doc.path)
        .ok_or_else(|| LibError::new(format!("no doc at {}", doc.path)))?;
    let mut crumbs = Vec::new();
    let mut up = plan.parent_of(entry);
    while let Some(parent) = up {
        crumbs.push(
            json!({"title": parent.title, "docPath": parent.path, "url": format!("{DOC_SCREEN}/{}", parent.path)}),
        );
        up = plan.parent_of(parent);
    }
    crumbs.reverse();
    let workspace = (!entry.file.is_empty())
        .then(|| crate::roots::locate(&context.roots, Path::new(&entry.file)))
        .flatten()
        .map(|(root, relative)| crate::artifacts::workspace_url(&root.id, &relative, entry.line));
    let directory = doc.path.rsplit_once('/').map_or("", |(dir, _)| dir);
    let mut out = Parameters::new();
    out.insert("docPath".into(), Value::string(&doc.path));
    out.insert("title".into(), Value::string(&doc.title));
    out.insert("kind".into(), Value::string(doc.kind.as_str()));
    out.insert("page".into(), Value::string(doc.page));
    out.insert("artifactId".into(), Value::string(&doc.artifact_id));
    out.insert("markdown".into(), Value::string(&doc.text));
    out.insert(
        "basePath".into(),
        Value::string(if directory.is_empty() {
            DOC_SCREEN.to_string()
        } else {
            format!("{DOC_SCREEN}/{directory}")
        }),
    );
    out.insert("crumbs".into(), json_value(&crumbs));
    out.insert("workspaceUrl".into(), workspace.map_or(Value::Null, Value::string));
    out.insert(
        "browserUrl".into(),
        if doc.artifact_id.is_empty() {
            Value::Null
        } else {
            Value::string(format!(
                "/qapps/dev/Artifacts/Browse?artifactId={}",
                encode(&doc.artifact_id)
            ))
        },
    );
    Ok(out)
}

/// `text` percent-encoded for a URL's query.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/' | b':') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// `export#ArtifactDocs`: the docs the options choose, written under a directory of a root (`autodoc` by default):
/// only the files whose text changed, the manifest, and with `removeStale` the files the manifest lists whose
/// artifacts are gone. Each file on its own: one that may not or does not write is reported and the rest go on.
fn export_artifact_docs(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let started = std::time::Instant::now();
    let root = context.root(&text(parameters, "rootId"))?;
    let base = text(parameters, "path");
    let base = base.trim().trim_matches('/');
    let base = if base.is_empty() { "autodoc" } else { base };
    let dir = context.resolve(root, base)?;
    context.may_write(root, &format!("{base}/{}", write::MANIFEST))?;
    let mut kinds = Vec::new();
    for kind in list(parameters, "kinds") {
        kinds.push(DocKind::parse(&kind).ok_or_else(|| LibError::new(format!("no doc kind {kind}")))?);
    }
    let previous = write::Manifest::read(&dir.path);
    let options = Options {
        kinds,
        components: list(parameters, "components"),
        packages: list(parameters, "packages"),
        include_source: flag(parameters, "includeSource"),
        template_dirs: template_dirs(&context),
        previous: previous.paths(),
    };
    // a snapshot of the files as they are, the references read from them
    let index = MemoryIndex::build(&context.factory);
    let source = MemorySource::new(&index);
    let plan = autodoc::build_plan(&context.factory, &source)?;
    let generated = autodoc::generate(&context.factory, &source, &plan, &options);
    let writes = write::plan_writes(&dir.path, &generated, &previous, &plan, flag(parameters, "removeStale"));

    let mut failures: Vec<serde_json::Value> = generated
        .failures
        .iter()
        .map(|failure| json!({"path": failure.path, "message": failure.message}))
        .collect();
    let mut written = 0;
    let mut removed = 0;
    let checked = |relative: &str| -> Result<std::path::PathBuf, LibError> {
        let target = format!("{base}/{relative}");
        context.may_write(root, &target)?;
        Ok(context.resolve(root, &target)?.path)
    };
    for (relative, body) in &writes.write {
        match checked(relative).and_then(|path| write_atomic(&path, body.as_bytes())) {
            Ok(()) => written += 1,
            Err(err) => failures.push(json!({"path": relative, "message": err.to_string()})),
        }
    }
    for relative in &writes.remove {
        let done = checked(relative).and_then(|path| match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(LibError::new(format!("{}: {err}", path.display()))),
        });
        match done {
            Ok(true) => removed += 1,
            Ok(false) => {}
            Err(err) => failures.push(json!({"path": relative, "message": err.to_string()})),
        }
    }
    if let Err(err) = checked(write::MANIFEST).and_then(|path| write_atomic(&path, writes.manifest_text.as_bytes())) {
        failures.push(json!({"path": write::MANIFEST, "message": err.to_string()}));
    }
    let detail = json!({
        "docs": generated.files.len(),
        "written": written,
        "unchanged": writes.unchanged,
        "removed": removed,
        "failures": failures.len(),
        "kinds": options.kinds.iter().map(|kind| kind.as_str()).collect::<Vec<_>>(),
        "components": options.components,
        "packages": options.packages,
    });
    audit::record(
        &context.facade,
        &audit::Write {
            action: "DAA_EXPORT",
            root: &root.id,
            path: base,
            detail: Some(detail),
            ..audit::Write::default()
        },
    );
    tracing::info!(
        "Autodoc: {} docs exported to {}/{base} ({written} written, {} unchanged, {removed} removed, {} failures) in {} ms",
        generated.files.len(),
        root.id,
        writes.unchanged,
        failures.len(),
        started.elapsed().as_millis()
    );
    let mut out = Parameters::new();
    out.insert(
        "docCount".into(),
        Value::I64(i64::try_from(generated.files.len()).unwrap_or(i64::MAX)),
    );
    out.insert("written".into(), Value::I64(written));
    out.insert(
        "unchanged".into(),
        Value::I64(i64::try_from(writes.unchanged).unwrap_or(i64::MAX)),
    );
    out.insert("removed".into(), Value::I64(removed));
    out.insert("failures".into(), json_value(&failures));
    out.insert("path".into(), Value::string(base));
    out.insert(
        "url".into(),
        Value::string(crate::artifacts::workspace_url(
            &root.id,
            &format!("{base}/README.md"),
            0,
        )),
    );
    Ok(out)
}
