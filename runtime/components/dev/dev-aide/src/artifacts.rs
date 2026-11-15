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

//! `org.moqui.devaide.ArtifactServices`: the framework's artifact index (`moqui_devkit::index`,
//! plans/design-dev-aide.md 7) as the dev-aide shows it: each artifact with the root and path that open it in the
//! Workspace and whether it is the site's or upstream (D17), search for the screens and the shell's palette, the
//! reference lists, go to definition, and the site's customizations with their state.

use std::path::Path;

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_devkit::index::{self, ArtifactRow, Direction, RefEntry};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use crate::roots::{self, Root};
use crate::tier::Tier;
use crate::workspace::{json_value, text, Context};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(search_artifacts), search_artifacts)?;
    registry.service_function(moqui_core::service_location!(get_artifact_detail), get_artifact_detail)?;
    registry.service_function(moqui_core::service_location!(get_refs), get_refs)?;
    registry.service_function(moqui_core::service_location!(get_definition), get_definition)?;
    registry.service_function(
        moqui_core::service_location!(get_artifacts_in_file),
        get_artifacts_in_file,
    )?;
    registry.service_function(moqui_core::service_location!(get_customizations), get_customizations)?;
    registry.service_function(moqui_core::service_location!(search_palette), search_palette)
}

/// A list parameter given as a list or a comma separated text.
pub(crate) fn list(parameters: &Parameters, key: &str) -> Vec<String> {
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

/// `text` percent-encoded for a URL's query: every byte but the unreserved ones.
pub(crate) fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// The Workspace link of a file and line.
pub(crate) fn workspace_url(root: &str, path: &str, line: u32) -> String {
    format!(
        "/qapps/dev/Workspace?rootId={}&path={}&line={line}",
        encode(root),
        encode(path)
    )
}

/// An artifact as the dev-aide shows it: with the root and path that open it, whether it is the site's, and the
/// Workspace link.
pub(crate) fn shown(context: &Context, artifact: &ArtifactRow) -> Json {
    shown_in(&context.roots, artifact)
}

/// [`shown`] against `roots`.
fn shown_in(roots: &[Root], artifact: &ArtifactRow) -> Json {
    let mut out = serde_json::to_value(artifact).unwrap_or(Json::Null);
    if let Some((root, relative)) = roots::locate(roots, Path::new(&artifact.path)) {
        let site = root.is_site_path(&relative);
        out["rootId"] = json!(root.id);
        out["relativePath"] = json!(relative);
        out["site"] = json!(site);
        out["upstream"] = json!(!site);
        out["url"] = json!(workspace_url(&root.id, &relative, artifact.line));
    } else {
        out["upstream"] = json!(true);
    }
    out
}

pub(crate) fn shown_ref(context: &Context, entry: &RefEntry) -> Json {
    let mut out = serde_json::to_value(entry).unwrap_or(Json::Null);
    if let Some(other) = &entry.other {
        out["other"] = shown(context, other);
    }
    if let Some((root, relative)) = roots::locate(&context.roots, Path::new(&entry.path)) {
        out["rootId"] = json!(root.id);
        out["relativePath"] = json!(relative);
        out["url"] = json!(workspace_url(&root.id, &relative, entry.line));
    }
    out
}

/// `search#Artifacts`: the artifacts whose names match a text, best first.
fn search_artifacts(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let limit = text(parameters, "limit").parse::<usize>().unwrap_or(50).clamp(1, 500);
    let found = index::find(
        &context.factory,
        &text(parameters, "text"),
        &list(parameters, "kinds"),
        &list(parameters, "components"),
        limit,
    )?;
    let shown: Vec<Json> = found.iter().map(|artifact| shown(&context, artifact)).collect();
    let mut out = Parameters::new();
    out.insert("artifacts".into(), json_value(&shown));
    Ok(out)
}

/// `get#ArtifactDetail`: an artifact with its origin (the defining component and each extension of it, the site's
/// first) and how many artifacts use it and it uses.
fn get_artifact_detail(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let mut out = Parameters::new();
    let Some(artifact) = index::named(&context.factory, &text(parameters, "artifactId"))? else {
        return Ok(out);
    };
    let users = index::refs(&context.factory, &artifact.id, Direction::In, &[])?;
    let uses = index::refs(&context.factory, &artifact.id, Direction::Out, &[])?;
    let mut extensions: Vec<Json> = users
        .iter()
        .filter(|entry| entry.ref_type == "extend" || entry.ref_type == "anchor")
        .map(|entry| shown_ref(&context, entry))
        .collect();
    extensions.sort_by_key(|entry| !entry["other"]["site"].as_bool().unwrap_or(false));
    out.insert("artifact".into(), json_value(&shown(&context, &artifact)));
    out.insert("extensions".into(), json_value(&extensions));
    out.insert(
        "usedByCount".into(),
        Value::I64(i64::try_from(users.len()).unwrap_or(0)),
    );
    out.insert("usesCount".into(), Value::I64(i64::try_from(uses.len()).unwrap_or(0)));
    Ok(out)
}

/// `get#Refs`: what uses an artifact (`direction` in) or what it uses (out), grouped by the other side's kind.
fn get_refs(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let direction = Direction::parse(&text(parameters, "direction"));
    let mut out = Parameters::new();
    let Some(artifact) = index::named(&context.factory, &text(parameters, "artifactId"))? else {
        return Ok(out);
    };
    let found = index::refs(&context.factory, &artifact.id, direction, &list(parameters, "kinds"))?;
    let mut groups: std::collections::BTreeMap<String, Vec<Json>> = std::collections::BTreeMap::new();
    for entry in &found {
        let kind = entry
            .other
            .as_ref()
            .map_or_else(|| entry.to_kind.clone(), |other| other.kind.clone());
        groups.entry(kind).or_default().push(shown_ref(&context, entry));
    }
    let groups: Vec<Json> = groups
        .into_iter()
        .map(|(kind, refs)| json!({"kind": kind, "count": refs.len(), "refs": refs}))
        .collect();
    let rows: Vec<Json> = found.iter().map(|entry| shown_ref(&context, entry)).collect();
    out.insert("artifact".into(), json_value(&shown(&context, &artifact)));
    out.insert("groups".into(), json_value(&groups));
    out.insert("refs".into(), json_value(&rows));
    Ok(out)
}

/// `get#Definition`: go to definition from a line of a file in a root: the artifact the reference there names (the one
/// naming `word` when given), with where it opens.
fn get_definition(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let root = context.root(&text(parameters, "rootId"))?;
    let resolved = context.resolve(root, &text(parameters, "path"))?;
    let line = text(parameters, "line").parse::<u32>().unwrap_or(0);
    let mut out = Parameters::new();
    let found = index::at(
        &context.factory,
        &resolved.path.display().to_string(),
        line,
        &text(parameters, "word"),
    )?;
    if let Some((reference, artifact)) = found {
        if let Some(reference) = reference {
            out.insert("reference".into(), json_value(&shown_ref(&context, &reference)));
        }
        if let Some(artifact) = artifact {
            out.insert("artifact".into(), json_value(&shown(&context, &artifact)));
        }
    }
    Ok(out)
}

/// `get#ArtifactsInFile`: the artifacts a file of a root defines at its top (a screen, an entity file's entities, a
/// service file's services), in file order, each as [`shown`]; none for a file the index does not have.
fn get_artifacts_in_file(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let root = context.root(&text(parameters, "rootId"))?;
    let resolved = context.resolve(root, &text(parameters, "path"))?;
    // a file the index never reads (not in a loaded component, or of no kind it reads) asks nothing of the index, which
    // would otherwise be built at its first use
    let found = if index::locate(&context.factory, &resolved.path).is_some() {
        index::in_file(&context.factory, &resolved.path.display().to_string())?
    } else {
        Vec::new()
    };
    let mut out = Parameters::new();
    out.insert("artifacts".into(), json_value(&in_file_shown(&context.roots, &found)));
    Ok(out)
}

/// The artifacts of a file as [`get_artifacts_in_file`] answers them.
fn in_file_shown(roots: &[Root], found: &[ArtifactRow]) -> Vec<Json> {
    found.iter().map(|artifact| shown_in(roots, artifact)).collect()
}

/// The site components' extensions with their state, broken first.
pub(crate) fn customizations(context: &Context) -> Result<(Vec<Json>, usize), LibError> {
    let sites = context.settings.site_components.clone();
    if sites.is_empty() {
        return Ok((Vec::new(), 0));
    }
    let found = index::extensions(&context.factory, &sites)?;
    let broken = found.iter().filter(|extension| !extension.resolved).count();
    let shown = found
        .iter()
        .map(|extension| {
            let mut out = serde_json::to_value(extension).unwrap_or(Json::Null);
            if let Some((root, relative)) = roots::locate(&context.roots, Path::new(&extension.path)) {
                out["rootId"] = json!(root.id);
                out["relativePath"] = json!(relative);
                out["url"] = json!(workspace_url(&root.id, &relative, extension.line));
            }
            out["state"] = json!(if extension.resolved { "ok" } else { "broken" });
            out
        })
        .collect();
    Ok((shown, broken))
}

/// `get#Customizations`: every extension the site components make, with whether what it extends is there (the
/// upgrade checklist after a pull).
fn get_customizations(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    _parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let (found, broken) = customizations(&context)?;
    let mut out = Parameters::new();
    out.insert(
        "extensionCount".into(),
        Value::I64(i64::try_from(found.len()).unwrap_or(0)),
    );
    out.insert("brokenCount".into(), Value::I64(i64::try_from(broken).unwrap_or(0)));
    out.insert("extensions".into(), json_value(&found));
    out.insert("siteComponents".into(), json_value(&context.settings.site_components));
    Ok(out)
}

/// `search#Palette`: the shell palette's Artifacts group (`palette-source`): entities, services, screens and rules
/// matching the text, each opening in the Workspace at its line.
fn search_palette(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let mut out = Parameters::new();
    // the shell asks every source; one the user may not use answers nothing
    let Ok(context) = Context::of(ec, Tier::View) else {
        out.insert("rows".into(), json_value(&Vec::<Json>::new()));
        return Ok(out);
    };
    let query = text(parameters, "text");
    let limit = text(parameters, "limit").parse::<usize>().unwrap_or(8).clamp(1, 50);
    let kinds: Vec<String> = [
        "entity",
        "view-entity",
        "service",
        "screen",
        "seca",
        "eeca",
        "rest",
        "transition",
        "form",
    ]
    .iter()
    .map(|kind| (*kind).to_string())
    .collect();
    let found = if query.trim().len() < 2 {
        Vec::new()
    } else {
        index::find(&context.factory, &query, &kinds, &[], limit)?
    };
    let rows: Vec<Json> = found
        .iter()
        .map(|artifact| {
            let shown = shown(&context, artifact);
            json!({
                "title": artifact.short_name,
                "subtitle": format!("{} · {} · {}", artifact.kind, artifact.component, artifact.name),
                "icon": icon_of(&artifact.kind),
                "url": shown["url"].as_str().map_or_else(
                    || format!("/qapps/dev/Artifacts/Refs?artifactId={}", encode(&artifact.id)),
                    str::to_string,
                ),
            })
        })
        .collect();
    out.insert("rows".into(), json_value(&rows));
    out.insert(
        "moreUrl".into(),
        Value::string(format!("/qapps/dev/Artifacts/Search?text={}", encode(&query))),
    );
    Ok(out)
}

fn icon_of(kind: &str) -> &'static str {
    match kind {
        "entity" | "view-entity" | "extend-entity" => "sym_o_table",
        "field" => "sym_o_view_column",
        "service" => "sym_o_settings_suggest",
        "screen" | "screen-extend" => "sym_o_web",
        "transition" => "sym_o_arrow_forward",
        "form" => "sym_o_dynamic_form",
        "seca" | "eeca" | "emeca" | "giteca" => "sym_o_bolt",
        "rest" => "sym_o_api",
        _ => "sym_o_code",
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::roots::{RootComponent, RootKind};

    fn root() -> Root {
        Root {
            id: "base".into(),
            label: "mr-moqui".into(),
            path: PathBuf::from("/w/base"),
            kind: RootKind::Git,
            components: vec![
                RootComponent {
                    name: "shop".into(),
                    path: "runtime/components/addon/shop".into(),
                    site: false,
                },
                RootComponent {
                    name: "my-site".into(),
                    path: "runtime/components/addon/my-site".into(),
                    site: true,
                },
            ],
            upstream: false,
            read_only: false,
            publish_branch: None,
            publish_branch_required: false,
            hosted: None,
            worktree_of: None,
            conversation_id: None,
            worktree_user_id: None,
        }
    }

    fn artifact(id: &str, kind: &str, short: &str, path: &str, line: u32) -> ArtifactRow {
        ArtifactRow {
            id: id.into(),
            kind: kind.into(),
            name: id.split_once(':').map_or(id, |(_, name)| name).into(),
            short_name: short.into(),
            component: "shop".into(),
            path: path.into(),
            line,
            column: 5,
            summary: String::new(),
            parent: None,
            origin: String::new(),
            marks: String::new(),
        }
    }

    /// The artifacts of a file, as the Workspace's References links take them: in the index's order, each with its id
    /// and short name, the root and path that open it at its line, and whether it is the site's.
    #[test]
    fn a_files_artifacts_carry_where_they_open_and_whose_they_are() {
        let services = "/w/base/runtime/components/addon/shop/service/shop/OrderServices.xml";
        let found = [
            artifact(
                "service:shop.OrderServices.place#Order",
                "service",
                "place#Order",
                services,
                18,
            ),
            artifact(
                "service:shop.OrderServices.cancel#Order",
                "service",
                "cancel#Order",
                services,
                40,
            ),
        ];
        let shown = in_file_shown(&[root()], &found);
        assert_eq!(shown.len(), 2);
        assert_eq!(shown[0]["id"], "service:shop.OrderServices.place#Order");
        assert_eq!(shown[0]["shortName"], "place#Order");
        assert_eq!(shown[1]["shortName"], "cancel#Order");
        assert_eq!(shown[0]["rootId"], "base");
        assert_eq!(
            shown[0]["relativePath"],
            "runtime/components/addon/shop/service/shop/OrderServices.xml"
        );
        assert_eq!(shown[0]["upstream"], true);
        assert_eq!(
            shown[1]["url"],
            "/qapps/dev/Workspace?rootId=base&path=runtime/components/addon/shop/service/shop/OrderServices.xml&line=40"
        );
        // a site component's screen is the site's
        let screen = artifact(
            "screen:component://my-site/screen/MySite.xml",
            "screen",
            "MySite",
            "/w/base/runtime/components/addon/my-site/screen/MySite.xml",
            1,
        );
        let site = in_file_shown(&[root()], std::slice::from_ref(&screen));
        assert_eq!(site[0]["site"], true);
        assert_eq!(site[0]["upstream"], false);
        // a file in no root still names its artifacts, upstream and without a Workspace link
        let elsewhere = artifact("entity:x.Y", "entity", "Y", "/elsewhere/YEntities.xml", 3);
        let outside = in_file_shown(&[root()], &[elsewhere]);
        assert_eq!(outside[0]["id"], "entity:x.Y");
        assert_eq!(outside[0]["upstream"], true);
        assert!(outside[0].get("url").is_none());
        assert!(in_file_shown(&[root()], &[]).is_empty());
    }
}
