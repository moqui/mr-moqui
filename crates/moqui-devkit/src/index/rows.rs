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

//! The artifacts that are rows of the database (plans/design-dev-aide.md 7.1): service jobs and the service each runs,
//! data documents and their primary entity, email templates and their body screen, and DbForm forms with the screen
//! form each modifies (configuration a site changes in production, shown beside the site component's extensions).
//! Each source is one pseudo file, `db:<entity>`, read again at each build when its rows changed. The screen mounts
//! are sources too: the `SubscreensItem` rows, and the merged conf's `screen-facade` mounts ([`conf_mounts`], the
//! pseudo file [`CONF_PATH`]), so a screen path through `/apps/...` walks them.

use moqui_lib::error::Error;

use std::collections::BTreeMap;

use super::extract::{ArtifactRow, FileIndex, RefRow, ToKind, CONF_ITEM, CONF_PATH, NO_SUB_PATH_MARK, ROW_ITEM};
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::screen::ConfScreen;

/// One source of row artifacts: its pseudo file path and what its rows define.
pub struct RowSource {
    pub path: String,
    pub hash: String,
    pub index: FileIndex,
}

fn text(row: &EntityValue, field: &str) -> String {
    row.get(field)
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default()
}

fn artifact(path: &str, id: String, kind: &str, name: &str, summary: String) -> ArtifactRow {
    ArtifactRow {
        short_name: name.to_string(),
        id,
        kind: kind.to_string(),
        name: name.to_string(),
        component: String::new(),
        path: path.to_string(),
        line: 0,
        column: 0,
        summary,
        parent: None,
        origin: String::new(),
    }
}

fn reference(from: &str, to_kind: ToKind, to_name: &str, to_scope: &str, ref_type: &str) -> RefRow {
    RefRow {
        from: from.to_string(),
        to_kind,
        to_name: to_name.to_string(),
        to_scope: to_scope.to_string(),
        ref_type: ref_type.to_string(),
        line: 0,
        column: 0,
        to_id: None,
        resolved: false,
        operation: String::new(),
        origin: "db".to_string(),
        origin_hash: String::new(),
    }
}

/// A reference of a row with its operation.
fn doing(mut row: RefRow, operation: &str) -> RefRow {
    row.operation = operation.to_string();
    row
}

/// What reads a source's rows into artifacts and references.
type Reader = fn(&[EntityValue], &str) -> FileIndex;

/// The sources' rows now. A source whose entity cannot be read is logged and left out.
#[must_use]
pub fn read(facade: &EntityFacade) -> Vec<RowSource> {
    let mut sources = Vec::new();
    let readers: [(&str, Reader); 5] = [
        ("moqui.service.job.ServiceJob", jobs),
        ("moqui.entity.document.DataDocument", documents),
        ("moqui.basic.email.EmailTemplate", email_templates),
        ("moqui.screen.form.DbForm", db_forms),
        ("moqui.screen.SubscreensItem", subscreen_items),
    ];
    for (entity, reader) in readers {
        let path = format!("db:{entity}");
        match rows_of(facade, entity) {
            Ok(rows) => {
                let index = reader(&rows, &path);
                sources.push(source(path, index));
            }
            Err(err) => tracing::error!("Artifact index: the rows of {entity} were not read: {err}"),
        }
    }
    sources
}

/// A source of `index`, hashed by what it holds.
fn source(path: String, mut index: FileIndex) -> RowSource {
    let fingerprint: String = index
        .artifacts
        .iter()
        .map(|artifact| format!("{}|{}|", artifact.id, artifact.summary))
        .chain(index.refs.iter().map(|row| format!("{}>{}|", row.from, row.to_name)))
        .collect();
    let hash = super::hash_of(&fingerprint);
    for row in &mut index.refs {
        row.origin_hash.clone_from(&hash);
    }
    RowSource { path, hash, index }
}

/// The conf's screen mounts (`screen-facade/screen` of the merged `MoquiConf.xml`s, by screen location): each
/// `subscreens-item` a subscreen artifact `subscreen:<screen>#<name>@conf` with its reference to the screen it mounts,
/// and each `default-subscreen` a `default-item` edge from the screen.
#[must_use]
pub fn conf_mounts(screens: &BTreeMap<String, ConfScreen>) -> RowSource {
    let mut index = FileIndex::default();
    for (screen, conf) in screens {
        for item in &conf.items {
            let Some(name) = item.attribute("name").filter(|name| !name.is_empty()) else {
                continue;
            };
            let location = item.attribute("location").unwrap_or_default();
            mount(
                &mut index,
                CONF_PATH,
                screen,
                name,
                location,
                item.attribute("no-sub-path") == Some("true"),
                CONF_ITEM,
                "conf",
            );
        }
        if let Some(default) = conf.default_subscreen.as_deref().filter(|name| !name.is_empty()) {
            default_edge(&mut index, screen, default, "conf");
        }
    }
    source(CONF_PATH.to_string(), index)
}

/// One mounted subscreens item of the screen at `screen`, with its reference to the screen it names (an empty
/// `location` names `<the screen's directory>/<name>.xml`).
#[allow(clippy::too_many_arguments)]
fn mount(
    index: &mut FileIndex,
    path: &str,
    screen: &str,
    name: &str,
    location: &str,
    no_sub_path: bool,
    mark: &str,
    origin: &str,
) {
    let id = format!("subscreen:{screen}#{name}{mark}");
    if index.artifacts.iter().any(|known| known.id == id) {
        return;
    }
    let mut summary = location.to_string();
    if no_sub_path {
        summary.push_str(NO_SUB_PATH_MARK);
    }
    let mut row = artifact(path, id.clone(), "subscreen", name, summary);
    row.parent = Some(format!("screen:{screen}"));
    index.artifacts.push(row);
    let target = if location.is_empty() {
        format!("{}/{name}.xml", screen.strip_suffix(".xml").unwrap_or(screen))
    } else {
        location.to_string()
    };
    if target.contains("://") {
        let mut reference = reference(&id, ToKind::Screen, &target, "", "subscreens-item");
        reference.origin = origin.to_string();
        index.refs.push(reference);
    }
}

/// The `default-item` edge from the screen at `screen` to its default subscreen `name`.
fn default_edge(index: &mut FileIndex, screen: &str, name: &str, origin: &str) {
    let mut reference = reference(
        &format!("screen:{screen}"),
        ToKind::ScreenPath,
        name,
        screen,
        "default-item",
    );
    reference.origin = origin.to_string();
    index.refs.push(reference);
}

/// The `SubscreensItem` rows: a subscreen artifact `subscreen:<screen>#<name>@row` each, `@row:<userGroupId>` for a
/// row of a user group, and a `default-item` edge for each `makeDefault`.
fn subscreen_items(rows: &[EntityValue], path: &str) -> FileIndex {
    let mut index = FileIndex::default();
    for row in rows {
        let screen = text(row, "screenLocation");
        let name = text(row, "subscreenName");
        if screen.is_empty() || name.is_empty() {
            continue;
        }
        // a row of a user group is an item of its own, which a row of every user wins over
        let group = text(row, "userGroupId");
        let mark = match group.as_str() {
            "" | "ALL_USERS" => ROW_ITEM.to_string(),
            group => format!("{ROW_ITEM}:{group}"),
        };
        mount(
            &mut index,
            path,
            &screen,
            &name,
            &text(row, "subscreenLocation"),
            text(row, "noSubPath") == "Y",
            &mark,
            "db",
        );
        if text(row, "makeDefault") == "Y" {
            default_edge(&mut index, &screen, &name, "db");
        }
    }
    index
}

fn rows_of(facade: &EntityFacade, entity: &str) -> Result<Vec<EntityValue>, Error> {
    Ok(facade.find(entity)?.disable_authz().list()?.into_values())
}

fn jobs(rows: &[EntityValue], path: &str) -> FileIndex {
    let mut index = FileIndex::default();
    for row in rows {
        let name = text(row, "jobName");
        let id = format!("job:{name}");
        let service = text(row, "serviceName");
        index
            .artifacts
            .push(artifact(path, id.clone(), "job", &name, text(row, "description")));
        if !service.is_empty() {
            index
                .refs
                .push(doing(reference(&id, ToKind::Service, &service, "", "job"), "call"));
        }
    }
    index
}

fn documents(rows: &[EntityValue], path: &str) -> FileIndex {
    let mut index = FileIndex::default();
    for row in rows {
        let name = text(row, "dataDocumentId");
        let id = format!("document:{name}");
        index
            .artifacts
            .push(artifact(path, id.clone(), "document", &name, text(row, "documentName")));
        let entity = text(row, "primaryEntityName");
        if !entity.is_empty() {
            index
                .refs
                .push(doing(reference(&id, ToKind::Entity, &entity, "", "document"), "find"));
        }
        let service = text(row, "manualDataServiceName");
        if !service.is_empty() {
            index
                .refs
                .push(doing(reference(&id, ToKind::Service, &service, "", "document"), "call"));
        }
    }
    index
}

fn email_templates(rows: &[EntityValue], path: &str) -> FileIndex {
    let mut index = FileIndex::default();
    for row in rows {
        let name = text(row, "emailTemplateId");
        let id = format!("email:{name}");
        index.artifacts.push(artifact(
            path,
            id.clone(),
            "email-template",
            &name,
            text(row, "description"),
        ));
        let screen = text(row, "bodyScreenLocation");
        if screen.contains("://") {
            index
                .refs
                .push(reference(&id, ToKind::Screen, &screen, "", "email-template"));
        }
    }
    index
}

fn db_forms(rows: &[EntityValue], path: &str) -> FileIndex {
    let mut index = FileIndex::default();
    for row in rows {
        let name = text(row, "formId");
        let id = format!("dbform:{name}");
        index
            .artifacts
            .push(artifact(path, id.clone(), "dbform", &name, text(row, "description")));
        // the screen location and form name, separated by a hash, of the form it modifies
        if let Some((screen, form)) = text(row, "modifyXmlScreenForm").split_once('#') {
            index.refs.push(reference(&id, ToKind::Anchor, form, screen, "extend"));
        }
    }
    index
}
