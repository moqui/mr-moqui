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

//! A bare repository on disk, through gitoxide: the `local` kind (written to) and the `remote` kind (a mirror that
//! only fetch changes). Every gix call for a repository on disk is in this file.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use gix::objs::{Find as _, Kind, Write as _};
use gix::refs::transaction::{PreviousValue, RefEdit as GixRefEdit};
use gix::refs::{FullName, Target};
use gix::ObjectId;

use super::store::{is_hex_prefix, ref_changes, GitStore, RefChange, RefEdit, RefOutcome};
use moqui_core::context::Error;

fn git_error(what: &str, path: &Path, err: impl std::fmt::Display) -> Error {
    Error::new(format!("{what} {}: {err}", path.display()))
}

/// A bare repository directory. `gix::ThreadSafeRepository` is shared; each call makes a thread-local handle.
#[derive(Debug)]
pub(crate) struct FsStore {
    path: PathBuf,
    repo: gix::ThreadSafeRepository,
}

impl FsStore {
    /// Open the bare repository at `path`.
    pub(crate) fn open(path: &Path) -> Result<Self, Error> {
        let repo =
            gix::ThreadSafeRepository::open(path).map_err(|err| git_error("cannot open git repository", path, err))?;
        Ok(Self {
            path: path.to_path_buf(),
            repo,
        })
    }

    /// Create a bare repository at `path` whose `HEAD` names `branch`, or open the one already there.
    pub(crate) fn init(path: &Path, branch: &str) -> Result<Self, Error> {
        if path.join("HEAD").is_file() {
            return Self::open(path);
        }
        std::fs::create_dir_all(path).map_err(|err| git_error("cannot create", path, err))?;
        gix::init_bare(path).map_err(|err| git_error("cannot create git repository", path, err))?;
        set_head(path, branch)?;
        Self::open(path)
    }

    fn local(&self) -> gix::Repository {
        self.repo.to_thread_local()
    }

    /// One transaction for `edits`: all are made, or none when a ref moved or another write failed.
    fn edit(&self, edits: &[&RefEdit]) -> Result<(), Error> {
        let mut changes = Vec::with_capacity(edits.len());
        for edit in edits {
            let name = FullName::try_from(edit.name.as_str())
                .map_err(|err| Error::new(format!("bad ref name {}: {err}", edit.name)))?;
            let expected = match edit.expected {
                Some(previous) => PreviousValue::MustExistAndMatch(Target::Object(previous)),
                None => PreviousValue::MustNotExist,
            };
            changes.push(match edit.new {
                Some(new) => GixRefEdit::update(name, new, expected, "moqui"),
                None => GixRefEdit::delete(name, expected),
            });
        }
        self.local()
            .edit_references(changes)
            .map(|_| ())
            .map_err(|err| git_error("cannot update refs of", &self.path, err))
    }
}

/// Point `HEAD` of a bare repository at `branch`, so `git clone` of it checks that branch out.
pub(crate) fn set_head(path: &Path, branch: &str) -> Result<(), Error> {
    std::fs::write(path.join("HEAD"), format!("ref: refs/heads/{branch}\n"))
        .map_err(|err| git_error("cannot write HEAD of", path, err))
}

impl GitStore for FsStore {
    fn find(&self, id: &ObjectId) -> Result<Option<(Kind, Vec<u8>)>, Error> {
        let repo = self.local();
        let mut buf = Vec::new();
        match repo.objects.try_find(id, &mut buf) {
            Ok(Some(data)) => {
                let kind = data.kind;
                Ok(Some((kind, buf)))
            }
            Ok(None) => Ok(None),
            Err(err) => Err(git_error(&format!("cannot read object {id} of"), &self.path, err)),
        }
    }

    fn object_size(&self, id: &ObjectId) -> Result<Option<u64>, Error> {
        self.local()
            .try_find_header(*id)
            .map(|header| header.map(|header| header.size()))
            .map_err(|err| git_error(&format!("cannot read object {id} of"), &self.path, err))
    }

    fn find_prefix(&self, prefix: &str, limit: usize) -> Result<Vec<ObjectId>, Error> {
        if !is_hex_prefix(prefix) {
            return Ok(Vec::new());
        }
        let prefix = gix::hash::Prefix::from_hex(prefix)
            .map_err(|err| Error::new(format!("bad object id prefix {prefix}: {err}")))?;
        let mut candidates = HashSet::new();
        self.local()
            .objects
            .lookup_prefix(prefix, Some(&mut candidates))
            .map_err(|err| git_error("cannot look up objects of", &self.path, err))?;
        let mut found: Vec<ObjectId> = candidates.into_iter().collect();
        found.sort();
        found.truncate(limit);
        Ok(found)
    }

    fn write(&self, kind: Kind, data: &[u8]) -> Result<ObjectId, Error> {
        self.local()
            .objects
            .write_buf(kind, data)
            .map_err(|err| git_error("cannot write an object to", &self.path, err))
    }

    fn read_ref(&self, name: &str) -> Result<Option<ObjectId>, Error> {
        let repo = self.local();
        let found = repo
            .try_find_reference(name)
            .map_err(|err| git_error(&format!("cannot read ref {name} of"), &self.path, err))?;
        match found {
            Some(mut reference) => reference
                .peel_to_id()
                .map(|id| Some(id.detach()))
                .map_err(|err| git_error(&format!("cannot read ref {name} of"), &self.path, err)),
            None => Ok(None),
        }
    }

    fn disk_path(&self) -> Option<&Path> {
        Some(&self.path)
    }

    /// The handle keeps packs mapped while a pack is written from them, and ignores replace refs, as gix's pack
    /// generator requires.
    fn pack_handle(&self) -> Option<gix::odb::store::Handle<std::sync::Arc<gix::odb::Store>>> {
        let mut handle = self.repo.objects.to_handle_arc();
        handle.prevent_pack_unload();
        handle.ignore_replacements = true;
        Some(handle)
    }

    fn list_refs(&self, prefix: &str) -> Result<Vec<(String, ObjectId)>, Error> {
        let repo = self.local();
        let platform = repo
            .references()
            .map_err(|err| git_error("cannot list refs of", &self.path, err))?;
        let mut refs = Vec::new();
        for reference in platform
            .all()
            .map_err(|err| git_error("cannot list refs of", &self.path, err))?
        {
            let reference = match reference {
                Ok(reference) => reference,
                Err(err) => {
                    tracing::error!("an unreadable ref in {}: {err}", self.path.display());
                    continue;
                }
            };
            let name = reference.name().as_bstr().to_string();
            if !name.starts_with(prefix) {
                continue;
            }
            if let Some(id) = reference.target().try_id() {
                refs.push((name, id.to_owned()));
            }
        }
        refs.sort();
        Ok(refs)
    }

    /// An atomic update is one gix transaction (every ref locked, then all written); otherwise each edit is its own.
    /// A failed transaction is stale when a ref no longer points where its edit expects; anything else is an error.
    fn update_refs(&self, edits: &[RefEdit], atomic: bool) -> Result<Vec<RefOutcome>, Error> {
        let groups: Vec<Vec<usize>> = if atomic {
            vec![(0..edits.len()).collect()]
        } else {
            (0..edits.len()).map(|index| vec![index]).collect()
        };
        let mut outcomes = vec![RefOutcome::Aborted; edits.len()];
        for group in groups {
            let members: Vec<&RefEdit> = group.iter().map(|index| &edits[*index]).collect();
            match self.edit(&members) {
                Ok(()) => {
                    for index in &group {
                        outcomes[*index] = RefOutcome::Updated;
                    }
                }
                Err(err) => {
                    let mut any_stale = false;
                    for index in &group {
                        let stale = self.read_ref(&edits[*index].name)? != edits[*index].expected;
                        any_stale |= stale;
                        outcomes[*index] = if stale { RefOutcome::Stale } else { RefOutcome::Aborted };
                    }
                    if !any_stale {
                        return Err(err);
                    }
                }
            }
        }
        Ok(outcomes)
    }
}

/// HTTPS credentials for a fetch.
#[derive(Clone, Debug, Default)]
pub(crate) struct Credentials {
    pub username: Option<String>,
    pub token: Option<String>,
}

/// What a fetch changed.
#[derive(Clone, Debug, Default)]
pub(crate) struct FetchOutcome {
    /// The branch's head after the fetch, or `None` when the remote branch has no commits.
    pub head: Option<ObjectId>,
    /// The local branches and tags before the fetch.
    pub before: Vec<(String, ObjectId)>,
    /// The local branches and tags that were created, moved or deleted.
    pub changes: Vec<RefChange>,
}

/// The refs a mirror keeps: every branch and tag.
const MIRRORED: [&str; 2] = ["refs/heads/", "refs/tags/"];

fn mirrored_refs(store: &FsStore) -> Result<Vec<(String, ObjectId)>, Error> {
    let mut refs = Vec::new();
    for prefix in MIRRORED {
        refs.extend(store.list_refs(prefix)?);
    }
    Ok(refs)
}

/// Fetch `url` into the bare repository at `path` (created when it is not there), replacing local refs with the
/// remote's: a remote repository is a mirror. With `mirror` every branch and tag is fetched and a local one the remote
/// no longer has is deleted (gix does not prune); without it only `branch`, for an import.
pub(crate) fn fetch(
    path: &Path,
    url: &str,
    branch: &str,
    credentials: &Credentials,
    mirror: bool,
) -> Result<FetchOutcome, Error> {
    let store = FsStore::init(path, branch)?;
    let before = mirrored_refs(&store)?;
    let repo = store.local();
    let specs: Vec<String> = if mirror {
        MIRRORED.iter().map(|prefix| format!("+{prefix}*:{prefix}*")).collect()
    } else {
        vec![format!("+refs/heads/{branch}:refs/heads/{branch}")]
    };
    let remote = repo
        .remote_at(url)
        .map(|remote| remote.with_fetch_tags(gix::remote::fetch::Tags::None))
        .and_then(|remote| remote.with_refspecs(specs.iter().map(String::as_str), gix::remote::Direction::Fetch))
        .map_err(|err| Error::new(format!("bad git remote {}: {err}", moqui_core::outbound::redacted(url))))?;
    let connection = remote.connect(gix::remote::Direction::Fetch).map_err(|err| {
        Error::new(format!(
            "cannot connect to {}: {err}",
            moqui_core::outbound::redacted(url)
        ))
    })?;
    let username = credentials.username.clone().unwrap_or_default();
    let token = credentials.token.clone();
    let connection = connection.with_credentials(move |action| match action {
        gix::credentials::helper::Action::Get(context) => {
            Ok(token.clone().map(|password| gix::credentials::protocol::Outcome {
                identity: gix::sec::identity::Account {
                    username: if username.is_empty() {
                        "git".to_string()
                    } else {
                        username.clone()
                    },
                    password,
                    oauth_refresh_token: None,
                },
                next: context.into(),
            }))
        }
        _ => Ok(None),
    });
    let interrupt = AtomicBool::new(false);
    let outcome = connection
        .prepare_fetch(gix::progress::Discard, gix::remote::ref_map::Options::default())
        .and_then(|prepare| prepare.receive(gix::progress::Discard, &interrupt))
        .map_err(|err| Error::new(format!("fetching {}: {err}", moqui_core::outbound::redacted(url))))?;
    if mirror {
        let remote_names: HashSet<String> = outcome
            .ref_map
            .remote_refs
            .iter()
            .map(|reference| reference.unpack().0.to_string())
            .collect();
        let gone: Vec<RefEdit> = mirrored_refs(&store)?
            .into_iter()
            .filter(|(name, _)| !remote_names.contains(name))
            .map(|(name, id)| RefEdit {
                name,
                expected: Some(id),
                new: None,
            })
            .collect();
        for (edit, outcome) in gone.iter().zip(store.update_refs(&gone, false)?) {
            if outcome != RefOutcome::Updated {
                tracing::warn!("{} of {} was not pruned: it moved", edit.name, path.display());
            }
        }
    }
    let after = mirrored_refs(&store)?;
    Ok(FetchOutcome {
        head: store.read_ref(&super::ops::branch_ref(branch))?,
        changes: ref_changes(&before, &after),
        before,
    })
}
