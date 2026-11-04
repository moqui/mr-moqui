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

//! Writing a tree of docs into a directory (plans/archive/Autodoc.md, "CLI" and "Dev AIDE"): only the files whose text changed,
//! so a repository's diff is the docs that changed, and a manifest (`.autodoc.json`) of every file the generator wrote
//! there. A file of the manifest whose doc is gone (its artifact removed or renamed) is stale and is removed when asked;
//! a file the manifest does not list (written by hand) is never touched. A partial export adds to the manifest.
//!
//! [`plan_writes`] decides; [`write_plan`] does it on the file system for `moqui autodoc`, and the Dev AIDE does it
//! through its confinement and write checks.

use std::collections::BTreeMap;
use std::path::Path;

use moqui_lib::error::Error;
use serde::{Deserialize, Serialize};

use super::plan::DocPlan;
use super::Generated;

/// The manifest's file name in the tree's directory.
pub const MANIFEST: &str = ".autodoc.json";

/// The files the generator wrote in a directory.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Each file by its path in the tree.
    pub files: BTreeMap<String, ManifestFile>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestFile {
    pub title: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub artifact_id: String,
}

impl Manifest {
    /// The manifest of `dir`; an empty one when there is none, or when it does not read (logged).
    #[must_use]
    pub fn read(dir: &Path) -> Self {
        let path = dir.join(MANIFEST);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        Self::parse(&text).unwrap_or_else(|err| {
            tracing::warn!(
                "Autodoc: the manifest {} does not read, taken as empty: {err}",
                path.display()
            );
            Self::default()
        })
    }

    /// # Errors
    ///
    /// Returns the JSON error when the text is not a manifest.
    pub fn parse(text: &str) -> Result<Self, Error> {
        serde_json::from_str(text).map_err(|err| Error::new(err.to_string()))
    }

    #[must_use]
    pub fn to_text(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string());
        text.push('\n');
        text
    }

    /// The doc paths, for [`super::Options::previous`].
    #[must_use]
    pub fn paths(&self) -> Vec<String> {
        self.files.keys().cloned().collect()
    }
}

/// What to do to a directory.
#[derive(Clone, Debug, Default)]
pub struct WritePlan {
    /// Each file to write, by its path in the tree, and its text.
    pub write: Vec<(String, String)>,
    /// How many were the same as on disk.
    pub unchanged: usize,
    /// The stale files to remove (only with `remove_stale`).
    pub remove: Vec<String>,
    /// The manifest after.
    pub manifest: Manifest,
    pub manifest_text: String,
}

/// What writing `generated` into `dir` does: write the files whose text differs from the disk's, and, with
/// `remove_stale`, remove the files of `previous` that `plan` has no doc for any more.
#[must_use]
pub fn plan_writes(
    dir: &Path,
    generated: &Generated,
    previous: &Manifest,
    plan: &DocPlan,
    remove_stale: bool,
) -> WritePlan {
    let mut out = WritePlan {
        manifest: previous.clone(),
        ..WritePlan::default()
    };
    for file in &generated.files {
        let existing = std::fs::read_to_string(dir.join(&file.path)).ok();
        if existing.as_deref() == Some(file.text.as_str()) {
            out.unchanged += 1;
        } else {
            out.write.push((file.path.clone(), file.text.clone()));
        }
        out.manifest.files.insert(
            file.path.clone(),
            ManifestFile {
                title: file.title.clone(),
                kind: file.kind.as_str().to_string(),
                artifact_id: file.artifact_id.clone(),
            },
        );
    }
    if remove_stale {
        for path in previous.files.keys() {
            if plan.at_path(path).is_none() {
                out.remove.push(path.clone());
                out.manifest.files.remove(path);
            }
        }
    }
    out.manifest_text = out.manifest.to_text();
    out
}

/// What a write did.
#[derive(Clone, Debug, Default, Serialize)]
pub struct WriteReport {
    pub written: usize,
    pub unchanged: usize,
    pub removed: usize,
    /// Each file that was not written or removed, and why.
    pub failures: Vec<(String, String)>,
}

/// Do `plan` in `dir` on the file system: each file on its own (one that fails is reported and the rest go on), then the
/// manifest.
#[must_use]
pub fn write_plan(dir: &Path, plan: &WritePlan) -> WriteReport {
    let mut report = WriteReport {
        unchanged: plan.unchanged,
        ..WriteReport::default()
    };
    for (path, text) in &plan.write {
        let target = dir.join(path);
        let done = target
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&target, text));
        match done {
            Ok(()) => report.written += 1,
            Err(err) => report.failures.push((path.clone(), err.to_string())),
        }
    }
    for path in &plan.remove {
        match std::fs::remove_file(dir.join(path)) {
            Ok(()) => report.removed += 1,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => report.failures.push((path.clone(), err.to_string())),
        }
    }
    if let Err(err) =
        std::fs::create_dir_all(dir).and_then(|()| std::fs::write(dir.join(MANIFEST), &plan.manifest_text))
    {
        report.failures.push((MANIFEST.to_string(), err.to_string()));
    }
    report
}
