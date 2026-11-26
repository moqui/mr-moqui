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

//! The index's sources that are rows and conf rather than files (`rows.rs`): read again when they change (a conf
//! reload, a `SubscreensItem` row written), and the artifact groups' members with their patterns matched against the
//! artifacts the index knows (`resolve/groups.rs`).

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::entity::{fence, EntityFacade};
use moqui_lib::error::Error;
use moqui_lib::object::Value;
use moqui_lib::sync::{lock, try_lock};

use super::{apply, extract, now_millis, rows, state, text, BuildReport, Change, IndexState, Located, FILE};
use crate::ArtifactKind;

/// Read again the sources that changed when no build or update holds the index's lock (a conf reload's hook, a query):
/// one that holds it reads them before it lets go ([`refresh_locked`]), so nothing waits behind it.
pub(super) fn refresh(factory: &Arc<ExecutionContextFactory>) {
    let state = state(factory);
    let Some(_running) = try_lock(&state.running) else {
        return;
    };
    let entered = factory.new_context();
    let _enter = entered.enter();
    let _authz = entered.disable_authz();
    let _lifted = fence::lift();
    let Some(facade) = factory.entity() else {
        return;
    };
    refresh_locked(factory, &facade, &state);
}

/// With the index's lock held: read again the conf's mounts when a conf reload is pending, and the `SubscreensItem`
/// rows when one was written; what fails is left to be tried again at the next use, its failure logged at most once a
/// minute.
pub(super) fn refresh_locked(factory: &ExecutionContextFactory, facade: &EntityFacade, state: &IndexState) {
    let conf = state.conf_pending.swap(false, Ordering::SeqCst);
    if let Err(err) = update_sources(factory, facade, state, conf) {
        if conf {
            state.conf_pending.store(true, Ordering::SeqCst);
        }
        log_throttled(
            state,
            &format!("Artifact index: the update of the conf mounts and SubscreensItem rows failed: {err}"),
        );
    }
}

/// With the index's lock held: replace the rows of the sources that changed ([`source_changes`]; with `conf`, the
/// conf's screen mounts too); a mount added, moved or removed walks the screen paths again (`apply`). Nothing before
/// the first build, which reads them all.
fn update_sources(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    state: &IndexState,
    conf: bool,
) -> Result<BuildReport, Error> {
    if !state.built.load(Ordering::SeqCst) || factory.is_shutting_down() {
        return Ok(BuildReport::default());
    }
    let started = std::time::Instant::now();
    let (changes, taken) = source_changes(factory, facade, state, conf);
    let mut report = BuildReport {
        files: changes.len(),
        ..BuildReport::default()
    };
    if let Err(err) = apply(factory, facade, state, changes, &mut report) {
        restore_screen_rows(state, taken);
        return Err(err);
    }
    report.millis = started.elapsed().as_millis();
    Ok(report)
}

/// Log `message` as an error unless one was logged in the last minute.
pub(super) fn log_throttled(state: &IndexState, message: &str) {
    let now = now_millis();
    let last = state.logged_at.load(Ordering::SeqCst);
    if now - last >= 60_000
        && state
            .logged_at
            .compare_exchange(last, now, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    {
        tracing::error!("{message}");
    }
}

/// Whether a `SubscreensItem` row was written since the index read them (a check of a counter, no lock).
pub(super) fn screen_rows_moved(factory: &ExecutionContextFactory) -> bool {
    factory.entity().is_some_and(|facade| {
        moqui_core::entity::screen_config_generation(&facade) != state(factory).screen_rows.load(Ordering::SeqCst)
    })
}

/// The generation a refresh took in place of the one before it (old, new): put back when the refresh failed, so the
/// next use tries again (not when another write moved it meanwhile).
type Taken = Option<(u64, u64)>;

fn restore_screen_rows(state: &IndexState, taken: Taken) {
    if let Some((old, new)) = taken {
        let _ = state
            .screen_rows
            .compare_exchange(new, old, Ordering::SeqCst, Ordering::SeqCst);
    }
}

/// The sources whose rows are no longer what the index holds: with `conf`, the conf's screen mounts now
/// ([`rows::conf_mounts`], read after a conf reload), and the `SubscreensItem` rows when one was written since they
/// were read, with the generation taken for them. Each is compared with the hash of its pseudo file's row, so one that
/// did not change writes nothing.
fn source_changes(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    state: &IndexState,
    conf: bool,
) -> (Vec<Change>, Taken) {
    let mut sources = Vec::new();
    if conf {
        sources.push(rows::conf_mounts(
            &moqui_core::screen::ScreenFacade::of(factory).conf().screens,
        ));
    }
    // taken before the rows are read: a write while they are read moves it again
    let generation = moqui_core::entity::screen_config_generation(facade);
    let old = state.screen_rows.swap(generation, Ordering::SeqCst);
    let mut taken = None;
    if old != generation {
        match rows::read_entity(facade, rows::SUBSCREENS_ITEM) {
            Ok(source) => {
                sources.push(source);
                taken = Some((old, generation));
            }
            Err(err) => {
                restore_screen_rows(state, Some((old, generation)));
                log_throttled(
                    state,
                    &format!("Artifact index: the SubscreensItem rows were not read: {err}"),
                );
            }
        }
    }
    let mut changes = Vec::new();
    for source in sources {
        let stored = match stored_hash(facade, &source.path) {
            Ok(hash) => hash,
            Err(err) => {
                // written again: its rows are replaced whole, so a write of the same rows changes nothing
                tracing::error!("Artifact index: the hash of {} was not read: {err}", source.path);
                None
            }
        };
        if stored.as_deref() != Some(source.hash.as_str()) {
            changes.push(source_change(source));
        }
    }
    (changes, taken)
}

/// The hash the rows hold for the file or pseudo file `path`; none when it has no row.
pub(super) fn stored_hash(facade: &EntityFacade, path: &str) -> Result<Option<String>, Error> {
    Ok(facade
        .find(FILE)?
        .eq("filePath", Value::string(path))
        .select_field("fileHash")
        .list()?
        .values()
        .first()
        .map(|row| text(row, "fileHash")))
}

/// The change that writes the rows of `source`, a pseudo file of no component.
pub(super) fn source_change(source: rows::RowSource) -> Change {
    Change::Read {
        file: Located {
            path: PathBuf::from(&source.path),
            component: String::new(),
            location: source.path,
            kind: ArtifactKind::Other,
        },
        hash: source.hash,
        index: source.index,
    }
}

/// `source` with its artifact groups' patterns matched against the artifacts the index knows, hashed by what they
/// matched ([`Resolver::expand_patterns`]): a source of other rows as it is.
pub(super) fn expand_source(state: &IndexState, mut source: rows::RowSource) -> rows::RowSource {
    if !source
        .index
        .refs
        .iter()
        .any(|row| row.to_scope == extract::PATTERN_SCOPE)
    {
        return source;
    }
    let refs = std::mem::take(&mut source.index.refs);
    source.index.refs = lock(&state.resolver).expand_patterns(refs);
    rows::rehash(source)
}
