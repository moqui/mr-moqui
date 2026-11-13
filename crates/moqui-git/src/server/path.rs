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

//! Repository paths: what a git URL carries for a repository (`moqui/mr-moqui`), and the repository it names
//! (plans/Dogfood.md D4, F2). A path is `GitRepository.repositoryPath`, else the `repositoryId` (one segment), else a
//! path the repository had before a rename (`GitRepositoryPathRedirect`). Paths are compared without case. Guide:
//! `docs/interfaces/Git-Hosting.md`, Clone URLs.

use moqui_core::context::ExecutionContext;
use moqui_lib::object::Value;

use crate::{rows_guard, text, GIT_REPOSITORY};

pub(crate) const GIT_PATH_REDIRECT: &str = "moqui.resource.GitRepositoryPathRedirect";

/// The most segments of a repository path.
pub const MAX_SEGMENTS: usize = 8;
const MAX_LENGTH: usize = 255;

/// Why `path` (no `/` around it, no `.git` after it) is not a repository path; `Ok` when it is one: one to
/// [`MAX_SEGMENTS`] segments of letters, digits, `.`, `_` and `-`, none starting with `.` or `-` (so never `.`, `..`,
/// an option of a command, or the `-` before a page's section) and none ending in `.git` or `.lock`.
///
/// # Errors
///
/// Returns what is wrong with the path.
pub fn check_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("a repository path is not empty".into());
    }
    if path.len() > MAX_LENGTH {
        return Err(format!("a repository path is at most {MAX_LENGTH} characters"));
    }
    let segments: Vec<&str> = path.split('/').collect();
    if segments.len() > MAX_SEGMENTS {
        return Err(format!("a repository path has at most {MAX_SEGMENTS} segments"));
    }
    for segment in segments {
        if segment.is_empty() {
            return Err("a repository path has no empty segment".into());
        }
        if segment.starts_with(['.', '-']) {
            return Err(format!("the path segment {segment} starts with '.' or '-'"));
        }
        if !segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(format!(
                "the path segment {segment} has a character that is not a letter, a digit, '.', '_' or '-'"
            ));
        }
        if segment.ends_with(".git") || segment.ends_with(".lock") {
            return Err(format!("the path segment {segment} ends in .git or .lock"));
        }
    }
    Ok(())
}

/// The repository path of a URL or SSH path: a leading `/`, a `git/` before it and a `.git` after it are dropped.
/// `None` for one that is not a path ([`check_path`]). The repository it names is found by the server, in its job.
pub fn repository_of(path: &str) -> Option<String> {
    let path = path.trim().trim_matches('/');
    let path = path.strip_prefix("git/").unwrap_or(path);
    let path = path.strip_suffix(".git").unwrap_or(path);
    check_path(path).is_ok().then(|| path.to_string())
}

/// The repository part and the rest of a request path under `/git/` (or of a page's path on a site host), neither
/// with a `/` at its start: split at the first `.git/`, else before a smart HTTP or LFS suffix (`info/refs`,
/// `git-upload-pack`, `git-receive-pack`, `info/lfs/...`). `None` when it has neither, so it is not a git request.
/// The repository part is as it was sent: [`repository_of`] checks it.
pub fn split_request(path: &str) -> Option<(&str, &str)> {
    let path = path.trim_start_matches('/');
    if let Some(split) = path.split_once(".git/") {
        return Some(split);
    }
    if let Some(repository) = path.strip_suffix(".git") {
        return Some((repository, ""));
    }
    for suffix in ["/info/refs", "/git-upload-pack", "/git-receive-pack"] {
        if let Some(repository) = path.strip_suffix(suffix) {
            return Some((repository, &suffix[1..]));
        }
    }
    // an LFS path never has `/info/lfs/` in what follows it, so the last one starts it
    let at = path.rfind("/info/lfs/")?;
    Some((&path[..at], &path[at + 1..]))
}

/// What a path is looked up and unique by: the path in lower case, so paths that differ only in case are one path.
pub fn path_key(path: &str) -> String {
    path.to_ascii_lowercase()
}

/// The id of the repository `path` (as [`repository_of`] gives it) names, in an entered context: for a path of one
/// segment the repository with exactly that id; else the one whose path has the same key ([`path_key`]:
/// `moqui/PopCommerce` finds `moqui/popcommerce`); else the one that had such a path before a rename. Read each time:
/// a rename on another node is seen at once, and each is one indexed read.
pub(crate) fn resolve(path: &str) -> Option<String> {
    if check_path(path).is_err() {
        return None;
    }
    let entity = ExecutionContext::with_current(|ec| ec.factory().entity()).flatten()?;
    let _rows = rows_guard();
    let found = (|| -> Result<Option<String>, moqui_core::context::Error> {
        if !path.contains('/') {
            let by_id = entity
                .find(GIT_REPOSITORY)?
                .eq("repositoryId", Value::string(path))
                .one()?;
            if by_id.is_some() {
                return Ok(Some(path.to_string()));
            }
        }
        let key = path_key(path);
        let by_key = entity
            .find(GIT_REPOSITORY)?
            .eq("repositoryPathKey", Value::string(&key))
            .one()?;
        if let Some(row) = by_key {
            return Ok(text(row.get("repositoryId")));
        }
        let redirect = entity
            .find(GIT_PATH_REDIRECT)?
            .eq("oldPathKey", Value::string(&key))
            .one()?;
        Ok(redirect.and_then(|row| text(row.get("repositoryId"))))
    })();
    match found {
        Ok(found) => found,
        Err(err) => {
            tracing::error!(path, "git repository path not looked up: {err}");
            None
        }
    }
}

/// What URLs carry for repository `id`, in an entered context: its `repositoryPath`, else the id.
pub(crate) fn path_of(id: &str) -> String {
    let entity = ExecutionContext::with_current(|ec| ec.factory().entity()).flatten();
    let _rows = rows_guard();
    entity
        .and_then(|entity| {
            entity
                .find(GIT_REPOSITORY)
                .ok()?
                .eq("repositoryId", Value::string(id))
                .one()
                .ok()
                .flatten()
        })
        .and_then(|row| text(row.get("repositoryPath")))
        .unwrap_or_else(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_segments_of_safe_names() {
        for good in ["code", "moqui/mr-moqui", "a/b/c/d/e/f/g/h", "DRIVE_100", "v1.2/x_y-z"] {
            assert_eq!(check_path(good), Ok(()), "{good}");
        }
        for bad in [
            "",
            "a//b",
            "a/./b",
            "../etc",
            "a/..",
            ".hidden",
            "a/-/issues",
            "-option",
            "a b",
            "a/b?c",
            "caf\u{e9}",
            "a.git/b",
            "a/b.lock",
            "a/b/c/d/e/f/g/h/i",
        ] {
            assert!(check_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn url_paths_name_a_repository_path() {
        assert_eq!(repository_of("/git/code.git"), Some("code".to_string()));
        assert_eq!(repository_of("code"), Some("code".to_string()));
        assert_eq!(
            repository_of("/moqui/mr-moqui.git/"),
            Some("moqui/mr-moqui".to_string())
        );
        assert_eq!(repository_of("git/moqui/mr-moqui"), Some("moqui/mr-moqui".to_string()));
        assert_eq!(repository_of("../etc/passwd"), None);
        assert_eq!(repository_of("/.git"), None);
        assert_eq!(repository_of("a/.git"), None);
    }

    #[test]
    fn request_paths_split_at_the_repository() {
        assert_eq!(split_request("code.git/info/refs"), Some(("code", "info/refs")));
        assert_eq!(split_request("code/info/refs"), Some(("code", "info/refs")));
        assert_eq!(
            split_request("moqui/mr-moqui.git/git-upload-pack"),
            Some(("moqui/mr-moqui", "git-upload-pack"))
        );
        assert_eq!(
            split_request("/moqui/mr-moqui/git-receive-pack"),
            Some(("moqui/mr-moqui", "git-receive-pack"))
        );
        assert_eq!(
            split_request("moqui/mr-moqui/info/lfs/objects/batch"),
            Some(("moqui/mr-moqui", "info/lfs/objects/batch"))
        );
        assert_eq!(
            split_request("moqui/mr-moqui.git/info/lfs/locks/7/unlock"),
            Some(("moqui/mr-moqui", "info/lfs/locks/7/unlock"))
        );
        assert_eq!(split_request("moqui/mr-moqui.git"), Some(("moqui/mr-moqui", "")));
        assert_eq!(split_request("moqui/mr-moqui/-/issues/12"), None);
        assert_eq!(split_request("moqui/mr-moqui"), None);
    }
}
