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

//! The example confs under `runtime/conf/`, the topology directories included (plans/Edge-Node2.md §4.9): each one
//! parses, merges over the default conf as boot merges a runtime conf, and passes the schema. Schema problems are
//! warnings in `moqui check`, by design; here, for the confs shipped as examples, they fail. Only the files git tracks
//! are checked when git answers, so a developer's own untracked conf there does not fail the run.

use std::path::{Path, PathBuf};

use moqui_core::context::merge_conf;
use moqui_lib::node::MNode;

use crate::check::schema_problems;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The XML files under `dir` and its subdirectories, and the subdirectories, each sorted; a symbolic link to a
/// directory is not followed.
fn walk(dir: &Path, files: &mut Vec<PathBuf>, dirs: &mut Vec<PathBuf>) {
    let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(dir).unwrap().flatten().collect();
    entries.sort_by_key(std::fs::DirEntry::path);
    for entry in entries {
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            dirs.push(path.clone());
            walk(&path, files, dirs);
        } else if path.extension().is_some_and(|ext| ext == "xml") {
            files.push(path);
        }
    }
}

/// The XML files git tracks under `runtime/conf`, and the subdirectories that hold any tracked file; `None` when git
/// does not answer (no git, or not a checkout).
fn tracked(repo: &Path) -> Option<(Vec<PathBuf>, Vec<PathBuf>)> {
    let output = std::process::Command::new("git")
        .args(["ls-files", "-z", "runtime/conf"])
        .current_dir(repo)
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let conf = repo.join("runtime/conf");
    let (mut files, mut dirs) = (Vec::new(), Vec::new());
    for name in String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|name| !name.is_empty())
    {
        let path = repo.join(name);
        if let Some(parent) = path.parent().filter(|parent| *parent != conf) {
            dirs.push(parent.to_path_buf());
        }
        if path.extension().is_some_and(|ext| ext == "xml") {
            files.push(path);
        }
    }
    if files.is_empty() {
        return None;
    }
    files.sort();
    dirs.sort();
    dirs.dedup();
    Some((files, dirs))
}

fn line(problem: &crate::Problem) -> String {
    format!(
        "{}:{}:{} {}",
        problem.path, problem.line, problem.column, problem.message
    )
}

#[test]
fn every_runtime_conf_parses_merges_over_the_default_and_passes_the_schema() {
    let repo = repo();
    let framework = repo.join("runtime/components/base/framework");
    let xsd = framework.join("xsd");
    let default_conf = framework.join("conf/MoquiDefaultConf.xml");
    let (files, dirs) = tracked(&repo).unwrap_or_else(|| {
        let (mut files, mut dirs) = (Vec::new(), Vec::new());
        walk(&repo.join("runtime/conf"), &mut files, &mut dirs);
        (files, dirs)
    });
    for moved in ["edge/MoquiEdgeConf.xml", "all-in-one/MoquiAllInOneConf.xml"] {
        assert!(
            files.iter().any(|file| file.ends_with(moved)),
            "runtime/conf/{moved} is not there"
        );
    }

    let mut problems: Vec<String> = Vec::new();
    // The default conf alone, so a problem of its own is named once and not as each example's
    let base_text = std::fs::read_to_string(&default_conf).unwrap();
    let base = MNode::parse_text("MoquiDefaultConf.xml", &base_text).unwrap();
    problems.extend(
        schema_problems(&xsd, &base, "MoquiDefaultConf.xml", &base_text)
            .iter()
            .map(line),
    );
    let base_count = problems.len();

    for file in &files {
        let display = file.strip_prefix(&repo).unwrap_or(file).display().to_string();
        let text = std::fs::read_to_string(file).unwrap();
        let conf = match MNode::parse_text(&display, &text) {
            Ok(conf) => conf,
            Err(err) => {
                problems.push(format!("{display}: does not parse: {err}"));
                continue;
            }
        };
        // merge_conf takes any root, so a conf with another one would merge to nothing without a word
        if conf.name() != "moqui-conf" {
            problems.push(format!("{display}: its root is <{}>, not <moqui-conf>", conf.name()));
            continue;
        }
        // The file alone: an element, attribute or value the schema does not take. A required attribute may come from
        // the default conf's element it merges into, so that rule is the merged conf's.
        problems.extend(
            schema_problems(&xsd, &conf, &display, &text)
                .iter()
                .filter(|problem| problem.rule != "schema.missing-attribute")
                .map(line),
        );
        let merged = MNode::parse_text("MoquiDefaultConf.xml", &base_text).unwrap();
        merge_conf(&merged, &conf);
        let merged_problems = schema_problems(&xsd, &merged, &format!("{display} merged"), "");
        // The merged conf holds the default's own problems, already named above
        if merged_problems.len() > base_count {
            problems.extend(merged_problems.iter().map(line));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));

    for dir in &dirs {
        assert!(
            dir.join("README.md").is_file(),
            "{} has no README.md (plans/Edge-Node2.md §4.9: the diagram, which file each node runs, the .env names, the commands)",
            dir.display()
        );
    }
}
