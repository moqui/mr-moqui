//! Dataset aliases need their source's own grant (BOSS impl-insights P3 follow-up, impl-admin F277;
//! docs/tools/Monitoring.md "Dataset aliases").
//!
//! A dataset definition is an alias when it reads a source that has a dataset artifact of its own, under a filter of the
//! definition's choosing: a telemetry index (`org.moqui.report.dataset.otel_logs`), whatever the definition's signal, or,
//! without being a business dataset, a document some business dataset reads (`org.moqui.report.dataset.<document>`). The
//! alias's own artifact says nothing of the source: a name pattern (`insights-.*`) grants it to whoever may save a dataset.
//! So the source's own artifact is asked for, when the alias is saved and each time it is read:
//!
//! - a telemetry index: of the reader, or of the saved definition's owner (asked as the owner at each read, so an alias an
//!   administrator shares stays a narrowed view for readers who do not hold the index, and stops when its owner no longer
//!   holds it). A shipped definition is its component's reviewed code and reads by its own grant, as before;
//! - a business dataset's document: of the reader (update, as the document read by its own id needs: business.rs
//!   `check_governed`), shipped or saved, since a business document is read by others only through an admitted dataset
//!   and its registered row filter.
use super::{business, definitions, fail, metadata, search, text, Error, ExecutionContext, Json, Target};

/// What an alias reads past its own grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Source {
    /// A telemetry index (`otel_logs`, `monitor_annotations`, ...).
    Telemetry,
    /// A document a business dataset reads, read without the business signal.
    Governed,
}

/// The rule: whether an alias of `source` opens, given who holds the source's own artifact. `reader` and `owner` are asked
/// only when the answer needs them (the owner's is a context of its own).
pub(super) fn opens(
    source: Source,
    shipped: bool,
    reader: impl FnOnce() -> bool,
    owner: impl FnOnce() -> bool,
) -> bool {
    match source {
        Source::Telemetry => shipped || reader() || owner(),
        Source::Governed => reader(),
    }
}

/// The artifact of a source read as a dataset by its own id.
pub(super) fn artifact(source: &str) -> String {
    format!("org.moqui.report.dataset.{source}")
}

/// What a dataset body reads: its index, else its document (the dataset's id when it names none).
pub(super) fn target(id: &str, body: &Json) -> Target {
    match body["indexName"].as_str() {
        Some(index) => Target::Index(index.into()),
        None => Target::Documents(vec![text(body, "documentType", id).into()]),
    }
}

fn viewer_holds(source: &str, action: &str) -> bool {
    moqui_core::user::authorize_own("AT_SERVICE", &artifact(source), action).is_ok()
}

/// Whether the owner of a saved definition holds a telemetry index's artifact by their own grants, asked in a context of
/// their own with authorization on. No owner, or one that is not an account, holds nothing.
fn owner_holds(definition: &Json, index: &str) -> bool {
    let owner = text(definition, "ownerId", "");
    !owner.is_empty() && moqui_core::user::as_user(owner, || viewer_holds(index, "view")).unwrap_or(false)
}

/// The engine's check before a dataset that is not the index's own reads a telemetry index (lib.rs `resolve_dataset`).
pub(super) fn check_telemetry(ec: &ExecutionContext, id: &str, definition: &Json, index: &str) -> Result<(), Error> {
    if opens(
        Source::Telemetry,
        definitions::is_shipped(ec, id),
        || viewer_holds(index, "view"),
        || owner_holds(definition, index),
    ) {
        return Ok(());
    }
    Err(fail(format!(
        "dataset {id} reads {index}, which neither its reader nor its owner may read ({})",
        artifact(index)
    )))
}

/// The check when a dataset definition is saved (`save#Definition`, a copy, a restore, an import, a git pull): the saver,
/// who becomes its owner, holds the own artifact of each source the body is an alias of. A body that reads through a
/// service reads no index.
pub(super) fn check_save(ec: &ExecutionContext, id: &str, body: &Json) -> Result<(), Error> {
    if body["serviceName"].is_string() {
        return Ok(());
    }
    let documents = match target(id, body) {
        Target::Documents(documents) => documents,
        Target::Index(index) if search::telemetry::is_telemetry_index(&index) => vec![index],
        target @ Target::Index(_) => {
            let facade = search::current()?;
            metadata(ec, || facade.target_documents(&target))?
        }
    };
    let indexes = documents
        .iter()
        .filter(|document| search::telemetry::is_telemetry_index(document));
    for index in indexes {
        if !opens(Source::Telemetry, false, || viewer_holds(index, "view"), || false) {
            return Err(fail(format!(
                "dataset {id} reads {index}: saving an alias of it needs its own grant ({})",
                artifact(index)
            )));
        }
    }
    business::check_governed(ec, id, body, &documents)
}

/// The viewer holds a business document's own artifact (update), for business.rs `check_governed`.
pub(super) fn governed_opens(document: &str) -> bool {
    opens(Source::Governed, false, || viewer_holds(document, "update"), || false)
}

#[cfg(test)]
#[path = "alias_tests.rs"]
mod tests;
