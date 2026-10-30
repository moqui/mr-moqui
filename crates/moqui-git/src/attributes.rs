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

//! Which files of a tree git-lfs keeps: the `filter` attribute of `.gitattributes` files, as git works it out (the
//! root's, then each directory's down to the file, a later line over an earlier one), `filter=lfs` meaning LFS. The
//! patterns are gitignore patterns (`*.png`, `media/**`, `/top.bin`), matched by gitoxide's glob.

use std::sync::{Arc, Mutex};

use gix::glob::pattern::Case;
use gix::glob::wildmatch;
use gix::objs::Kind;
use gix::ObjectId;

use super::ops;
use super::store::GitStore;
use moqui_core::context::Error;
use moqui_core::script::bounded::BoundedMap;

/// The attributes file of a directory.
pub(crate) const FILE: &str = ".gitattributes";

/// What a line says of the `filter` attribute: a value, or unset (`-filter`, `!filter`).
type Rule = (gix::glob::Pattern, Option<String>);

/// Parsed attributes files by blob id.
pub(crate) type RulesCache = Mutex<BoundedMap<ObjectId, Arc<Vec<Rule>>>>;

pub(crate) fn rules_cache() -> RulesCache {
    Mutex::new(BoundedMap::new(256))
}

/// The lines of an attributes file that say something of `filter`, in order.
fn parse(text: &str) -> Vec<Rule> {
    let mut rules = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut words = line.split_whitespace();
        let Some(pattern) = words
            .next()
            .and_then(|word| gix::glob::Pattern::from_bytes_without_negation(word.as_bytes()))
        else {
            continue;
        };
        let mut filter = None;
        for word in words {
            if let Some(value) = word.strip_prefix("filter=") {
                filter = Some(Some(value.to_string()));
            } else if word == "-filter" || word == "!filter" {
                filter = Some(None);
            }
        }
        if let Some(filter) = filter {
            rules.push((pattern, filter));
        }
    }
    rules
}

/// Whether the file at `path` (its segments) in `tree` is an LFS file.
pub(crate) fn is_lfs(
    store: &dyn GitStore,
    cache: &RulesCache,
    tree: &ObjectId,
    path: &[String],
) -> Result<bool, Error> {
    let Some((name, dirs)) = path.split_last() else {
        return Ok(false);
    };
    if name == FILE {
        return Ok(false);
    }
    let mut filter: Option<String> = None;
    // the root's attributes, then each directory's on the way to the file
    for depth in 0..=dirs.len() {
        let mut attributes_path: Vec<String> = dirs[..depth].to_vec();
        attributes_path.push(FILE.to_string());
        let Some((mode, blob)) = ops::resolve(store, tree, &attributes_path)? else {
            continue;
        };
        if mode.is_tree() {
            continue;
        }
        // the cache's lock is let go before a miss takes it again
        let cached = moqui_lib::sync::lock(cache).get(&blob);
        let rules = match cached {
            Some(rules) => rules,
            None => {
                let text = String::from_utf8_lossy(&ops::object(store, &blob, Kind::Blob)?).into_owned();
                let rules = Arc::new(parse(&text));
                moqui_lib::sync::lock(cache).insert(blob, Arc::clone(&rules));
                rules
            }
        };
        let relative = path[depth..].join("/");
        let basename = relative.rfind('/').map(|at| at + 1);
        for (pattern, value) in rules.iter() {
            if pattern.matches_repo_relative_path(
                relative.as_str().into(),
                basename,
                Some(false),
                Case::Sensitive,
                wildmatch::Mode::NO_MATCH_SLASH_LITERAL,
            ) {
                filter = value.clone();
            }
        }
    }
    Ok(filter.as_deref() == Some("lfs"))
}

/// The patterns new content repositories keep in LFS: images, video, audio, PDFs and archives.
pub const DEFAULT_LFS_PATTERNS: [&str; 30] = [
    "*.jpg", "*.jpeg", "*.png", "*.gif", "*.webp", "*.avif", "*.heic", "*.tif", "*.tiff", "*.psd", "*.mp4", "*.mov",
    "*.webm", "*.mkv", "*.avi", "*.m4v", "*.mp3", "*.wav", "*.ogg", "*.flac", "*.m4a", "*.aac", "*.pdf", "*.zip",
    "*.gz", "*.tgz", "*.7z", "*.rar", "*.tar", "*.bz2",
];

/// The attributes file `existing` with a `filter=lfs` line for each of `patterns` it does not have, as
/// `git lfs track` writes them.
pub(crate) fn with_lfs_patterns(existing: &str, patterns: &[&str]) -> String {
    let mut out = existing.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    for pattern in patterns {
        let tracked = existing.lines().any(|line| {
            line.split_whitespace().next() == Some(*pattern) && line.split_whitespace().any(|word| word == "filter=lfs")
        });
        if !tracked {
            out.push_str(&format!("{pattern} filter=lfs diff=lfs merge=lfs -text\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{Author, Edit};
    use crate::store::MemoryStore;

    fn commit(store: &MemoryStore, files: &[(&str, &str)]) -> ObjectId {
        let edits: Vec<Edit> = files
            .iter()
            .map(|(path, text)| {
                let blob = store.write(Kind::Blob, text.as_bytes()).unwrap();
                Edit::Put(
                    path.split('/').map(str::to_string).collect(),
                    gix::objs::tree::EntryKind::Blob,
                    blob,
                )
            })
            .collect();
        let author = Author {
            name: "Test".into(),
            email: "test@example.com".into(),
            user_id: None,
            seconds: None,
        };
        let commit = ops::commit(store, "main", &author, |_| Ok(Some((edits.clone(), "files".into()))))
            .unwrap()
            .unwrap();
        ops::read_commit(store, &commit).unwrap().tree
    }

    #[test]
    fn the_filter_attribute_is_worked_out_as_git_does() {
        let store = MemoryStore::default();
        let tree = commit(
            &store,
            &[
                (
                    ".gitattributes",
                    "# media\n*.png filter=lfs diff=lfs merge=lfs -text\n/top.bin filter=lfs\ndocs/** -filter\n",
                ),
                ("docs/.gitattributes", "keep.png filter=lfs\n"),
                ("raw/.gitattributes", "*.png !filter\n"),
            ],
        );
        let cache = rules_cache();
        let lfs = |path: &str| {
            let path: Vec<String> = path.split('/').map(str::to_string).collect();
            is_lfs(&store, &cache, &tree, &path).unwrap()
        };
        assert!(lfs("logo.png"));
        assert!(lfs("images/deep/logo.png"));
        assert!(lfs("top.bin"));
        assert!(!lfs("sub/top.bin"), "an anchored pattern is for the root only");
        assert!(!lfs("readme.md"));
        assert!(!lfs("docs/logo.png"), "a later line unsets it");
        assert!(lfs("docs/keep.png"), "a deeper file sets it again");
        assert!(!lfs("raw/logo.png"));
        assert!(!lfs(".gitattributes"));
    }

    #[test]
    fn patterns_are_added_once() {
        let first = with_lfs_patterns("", &["*.png", "*.pdf"]);
        assert_eq!(
            first,
            "*.png filter=lfs diff=lfs merge=lfs -text\n*.pdf filter=lfs diff=lfs merge=lfs -text\n"
        );
        assert_eq!(with_lfs_patterns(&first, &["*.png", "*.mp4"]).lines().count(), 3);
    }
}
