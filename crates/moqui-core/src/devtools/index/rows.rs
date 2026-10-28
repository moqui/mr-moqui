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
//! Each source is one pseudo file, `db:<entity>`, read again at each build when its rows changed.

use moqui_lib::error::Error;

use super::extract::{ArtifactRow, FileIndex, RefRow, ToKind};
use crate::entity::{EntityFacade, EntityValue};

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
    }
}

/// What reads a source's rows into artifacts and references.
type Reader = fn(&[EntityValue], &str) -> FileIndex;

/// The sources' rows now. A source whose entity cannot be read is logged and left out.
#[must_use]
pub fn read(facade: &EntityFacade) -> Vec<RowSource> {
    let mut sources = Vec::new();
    let readers: [(&str, Reader); 4] = [
        ("moqui.service.job.ServiceJob", jobs),
        ("moqui.entity.document.DataDocument", documents),
        ("moqui.basic.email.EmailTemplate", email_templates),
        ("moqui.screen.form.DbForm", db_forms),
    ];
    for (entity, reader) in readers {
        let path = format!("db:{entity}");
        match rows_of(facade, entity) {
            Ok(rows) => {
                let index = reader(&rows, &path);
                let fingerprint: String = index
                    .artifacts
                    .iter()
                    .map(|artifact| format!("{}|{}|", artifact.id, artifact.summary))
                    .chain(index.refs.iter().map(|row| format!("{}>{}|", row.from, row.to_name)))
                    .collect();
                sources.push(RowSource {
                    hash: super::hash_of(&fingerprint),
                    path,
                    index,
                });
            }
            Err(err) => tracing::error!("Artifact index: the rows of {entity} were not read: {err}"),
        }
    }
    sources
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
            index.refs.push(reference(&id, ToKind::Service, &service, "", "job"));
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
            index.refs.push(reference(&id, ToKind::Entity, &entity, "", "document"));
        }
        let service = text(row, "manualDataServiceName");
        if !service.is_empty() {
            index
                .refs
                .push(reference(&id, ToKind::Service, &service, "", "document"));
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
