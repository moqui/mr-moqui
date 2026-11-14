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

//! Services of `org.moqui.git.GitServices` about a whole repository and what it holds (plans/Dogfood.md DG2):
//! `get#GitRepositoryUsage`, `delete#GitRepository`, `get#GitBlame` and `get#GitArchive`. Each checks the
//! repository's rights as the git server does. Guide: `docs/interfaces/Git-Hosting.md`.

use std::collections::{BTreeMap, HashMap};

use gix::ObjectId;
use moqui_lib::object::Value;

use super::hosting::{allowed, list_param, may, none, Out, Parameters};
use super::{add_error, add_message, count, flag, out, path_segments, provider_and_id, text, viewable};
use moqui_core::entity::EntityFacade;
use moqui_core::rust_service;

use crate::archive::{self, Format};
use crate::blame;
use crate::storage::Directory;
use crate::values::{map, opt_text, timestamp};

/// The most repositories one `get#GitRepositoryUsage` call answers.
const MAX_USAGE_IDS: usize = 1000;

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    rust_service!(map, get_repository_usage)?;
    rust_service!(map, delete_repository)?;
    rust_service!(map, get_blame)?;
    rust_service!(map, get_archive)?;
    Ok(())
}

fn bytes(value: u64) -> Value {
    Value::I64(i64::try_from(value).unwrap_or(i64::MAX))
}

/// `get#GitRepositoryUsage`: what one repository (`repositoryId`) or several (`repositoryIdList`) store now. Out:
/// `usageList` (each `repositoryId`, `repositoryKind`, `storedBytes`, `objectCount`, `lfsBytes`,
/// `lfsObjectCount`), `totalStoredBytes`, `totalLfsBytes`. A repository the user may not view, or that is not
/// there, is left out.
fn get_repository_usage(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let Some((provider, id)) = provider_and_id(parameters, false) else {
        return none();
    };
    let mut ids = list_param(parameters, "repositoryIdList");
    if !id.is_empty() && !ids.contains(&id) {
        ids.insert(0, id);
    }
    if ids.len() > MAX_USAGE_IDS {
        add_error(format!("at most {MAX_USAGE_IDS} repositories in one call"));
        return none();
    }
    ids.retain(|id| may(id, "view"));
    match provider.usage(&ids) {
        Ok(usages) => {
            let stored: u64 = usages.iter().map(|usage| usage.stored_bytes).sum();
            let lfs: u64 = usages.iter().map(|usage| usage.lfs_bytes).sum();
            let list = usages
                .iter()
                .map(|usage| {
                    map([
                        ("repositoryId", Value::string(&usage.repository_id)),
                        ("repositoryKind", Value::string(usage.kind)),
                        ("storedBytes", bytes(usage.stored_bytes)),
                        ("objectCount", usage.object_count.map_or(Value::Null, bytes)),
                        ("lfsBytes", bytes(usage.lfs_bytes)),
                        ("lfsObjectCount", bytes(usage.lfs_object_count)),
                    ])
                })
                .collect();
            Ok(out([
                ("usageList", Value::List(list)),
                ("totalStoredBytes", bytes(stored)),
                ("totalLfsBytes", bytes(lfs)),
            ]))
        }
        Err(err) => {
            add_error(err.message());
            none()
        }
    }
}

/// `delete#GitRepository`: the repository with everything the framework keeps of it. Needs delete. Out:
/// `directoryRemoved`, and `directoryKept` for a directory outside the runtime directory, which is left.
fn delete_repository(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let Some((provider, id)) = allowed(parameters, "delete") else {
        return none();
    };
    // a row that cannot be deleted fails the service, so its transaction rolls the others back
    let directory = provider.delete_repository(&id)?;
    add_message(format!("Deleted git repository {id}"));
    Ok(match directory {
        Directory::None => out([("directoryRemoved", Value::Bool(false))]),
        Directory::Removed(_) => out([("directoryRemoved", Value::Bool(true))]),
        Directory::Kept(path) => {
            let path = path.to_string_lossy().into_owned();
            add_message(format!(
                "Its directory {path} is outside the runtime directory and was left"
            ));
            out([
                ("directoryRemoved", Value::Bool(false)),
                ("directoryKept", Value::string(path)),
            ])
        }
    })
}

/// `get#GitBlame`: for each line of the file at `path` in `revision`, the commit that last changed it. Out:
/// `commitId`, `lineCount`, `hunkList` (runs of lines of one commit, in line order); nothing when the revision
/// names nothing or the path is not a file there. Refused past the conf's limits.
fn get_blame(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let Some((provider, id)) = viewable(parameters) else {
        return none();
    };
    let revision = text(parameters, "revision").unwrap_or_default();
    let path = path_segments(parameters, "path");
    let limits = blame::Limits {
        max_bytes: crate::number_setting("blame-max-bytes", blame::DEFAULT_MAX_BYTES),
        max_lines: crate::number_setting("blame-max-lines", blame::DEFAULT_MAX_LINES),
        max_commits: crate::number_setting("blame-max-commits", blame::DEFAULT_MAX_COMMITS),
        max_diffs: crate::number_setting("blame-max-diffs", blame::DEFAULT_MAX_DIFFS),
        max_time: std::time::Duration::from_secs(crate::number_setting(
            "blame-max-seconds",
            blame::DEFAULT_MAX_SECONDS,
        )),
    };
    let found = match provider.blame(&id, &revision, &path, limits) {
        Ok(Some(found)) => found,
        Ok(None) => return none(),
        Err(err) => {
            add_error(err.message());
            return none();
        }
    };
    let lines: Vec<&str> = if flag(parameters, "withLines") {
        moqui_core::resource::textdiff::split_lines(&found.blame.text)
    } else {
        Vec::new()
    };
    let commits: HashMap<ObjectId, &crate::ops::CommitDetails> =
        found.commits.iter().map(|commit| (commit.id, commit)).collect();
    let hunks = found
        .blame
        .hunks
        .iter()
        .map(|hunk| {
            let mut entries = vec![
                ("startLine", count(hunk.start)),
                ("lineCount", count(hunk.lines)),
                ("commitId", Value::string(hunk.commit.to_string())),
                ("shortId", Value::string(hunk.commit.to_hex_with_len(10).to_string())),
            ];
            if let Some(commit) = commits.get(&hunk.commit) {
                entries.push(("summary", Value::string(commit.summary())));
                entries.push(("authorName", opt_text(&commit.author.name)));
                entries.push(("authorEmail", opt_text(&commit.author.email)));
                entries.push(("authorDate", timestamp(commit.author.seconds)));
            }
            if !lines.is_empty() {
                let text = lines
                    .iter()
                    .skip(hunk.start - 1)
                    .take(hunk.lines)
                    .map(|line| Value::string(line.trim_end_matches(['\n', '\r'])))
                    .collect();
                entries.push(("lines", Value::List(text)));
            }
            map(entries)
        })
        .collect();
    Ok(out([
        ("commitId", Value::string(found.commit.to_string())),
        (
            "lineCount",
            count(found.blame.hunks.iter().map(|hunk| hunk.lines).sum()),
        ),
        ("hunkList", Value::List(hunks)),
    ]))
}

/// The top directory of an archive: the last segment of the repository's path (its id with none) and the commit's
/// short id, as `mr-moqui-0123456789`.
fn archive_top(repository_id: &str, commit: &ObjectId) -> String {
    let path = crate::server::path::path_of(repository_id);
    let name: String = path
        .rsplit('/')
        .next()
        .unwrap_or(repository_id)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let name = name.trim_start_matches(['.', '-']);
    let name = if name.is_empty() { "repository" } else { name };
    format!("{name}-{}", commit.to_hex_with_len(10))
}

/// `get#GitArchive`: the tree at `revision` (under `path`, when given) as a zip or tar.gz file. Out:
/// `archiveLocation` (a file under `runtime/tmp/git-archive` to serve), `fileName`, `contentType`,
/// `archiveSize`, `contentBytes`, `fileCount`, `commitId`; nothing when the revision names nothing or the path is
/// not a directory there. Refused past the conf's limits.
fn get_archive(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let Some((provider, id)) = viewable(parameters) else {
        return none();
    };
    let Some(format) = Format::parse(&text(parameters, "format").unwrap_or_default()) else {
        add_error("format is zip or tar.gz");
        return none();
    };
    let revision = text(parameters, "revision").unwrap_or_default();
    let commit = match provider.resolve_commit(&id, &revision) {
        Ok(Some(commit)) => commit,
        Ok(None) => return none(),
        Err(err) => {
            add_error(err.message());
            return none();
        }
    };
    let limits = archive::Limits {
        max_bytes: crate::number_setting("archive-max-bytes", archive::DEFAULT_MAX_BYTES),
        max_entries: crate::number_setting("archive-max-files", archive::DEFAULT_MAX_ENTRIES),
        max_disk_bytes: crate::number_setting("archive-max-disk-bytes", archive::DEFAULT_MAX_DISK_BYTES),
        max_concurrent: crate::number_setting("archive-max-concurrent", archive::DEFAULT_MAX_CONCURRENT),
    };
    let top = archive_top(&id, &commit);
    let path = path_segments(parameters, "path");
    match provider.archive(&id, &commit.to_string(), &path, &top, format, limits) {
        Ok(Some(archived)) => Ok(out([
            (
                "archiveLocation",
                Value::string(archived.path.to_string_lossy().into_owned()),
            ),
            ("fileName", Value::string(format!("{top}.{}", format.extension()))),
            ("contentType", Value::string(format.content_type())),
            ("archiveSize", bytes(archived.size)),
            ("contentBytes", bytes(archived.content_bytes)),
            ("fileCount", count(archived.files)),
            ("commitId", Value::string(archived.commit.to_string())),
            (
                "skippedList",
                Value::List(archived.skipped.iter().map(Value::string).collect()),
            ),
        ])),
        Ok(None) => none(),
        Err(err) => {
            add_error(err.message());
            none()
        }
    }
}
