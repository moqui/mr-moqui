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

//! `moqui test <component> --skills`: each component's `test/skill-retrieval.json` run over the skill files of the
//! framework and every component (plans/LLM-A2A-Assist.md 7.5), with no model and no database: recall@3 and the
//! strong false matches. A component fails below a recall@3 of 0.9 or with any strong false match.

use std::path::{Path, PathBuf};

use moqui_core::llm::skills::{retrieval, sync, SkillIndex};
use moqui_lib::error::Error;

use super::TestArgs;

/// The recall@3 a component's set needs.
const MIN_RECALL: f64 = 0.9;

/// The component directories under `base-component` and `component`, the framework first, each with its name.
fn component_dirs(runtime: &Path, framework: &Path) -> Vec<(String, PathBuf)> {
    let mut dirs: Vec<(String, PathBuf)> = vec![("framework".to_string(), framework.to_path_buf())];
    for parent in ["base-component", "component"] {
        let Ok(entries) = std::fs::read_dir(runtime.join(parent)) else {
            continue;
        };
        let mut found: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).filter(|dir| dir.is_dir()).collect();
        found.sort();
        for dir in found {
            let same = |known: &PathBuf| std::fs::canonicalize(known).ok() == std::fs::canonicalize(&dir).ok();
            if !dirs.iter().any(|(_, known)| same(known)) {
                dirs.push((super::component_name(&dir), dir));
            }
        }
    }
    dirs
}

/// Run the retrieval sets of the components asked for (all that have one when none is named).
pub(super) fn dispatch(args: &TestArgs, runtime: &Path, framework: &Path) -> Result<(), Error> {
    let dirs = component_dirs(runtime, framework);
    let skill_dirs: Vec<(String, PathBuf)> = dirs.iter().map(|(name, dir)| (name.clone(), dir.join("skill"))).collect();
    let read = sync::read_dirs(&skill_dirs);
    for problem in &read.problems {
        println!("  skill file problem: {problem}");
    }
    let index = SkillIndex {
        skills: read.skills,
        aliases: read.aliases,
    };
    let mut ran = 0;
    let mut failed = Vec::new();
    for (name, dir) in &dirs {
        if !args.components.is_empty() && !args.components.contains(name) {
            continue;
        }
        let file = dir.join("test/skill-retrieval.json");
        let Ok(text) = std::fs::read_to_string(&file) else {
            if args.components.contains(name) {
                return Err(Error::new(format!("component {name} has no test/skill-retrieval.json")));
            }
            continue;
        };
        let cases = retrieval::read_cases(&text).map_err(|error| Error::new(format!("{}: {error}", file.display())))?;
        let report = retrieval::evaluate(&index, &cases);
        println!("{}", report.text(name));
        ran += 1;
        if report.recall() < MIN_RECALL || !report.strong_false.is_empty() {
            failed.push(name.clone());
        }
    }
    if ran == 0 {
        return Err(Error::new("no component has a test/skill-retrieval.json"));
    }
    println!("{} skills in the index", index.skills.len());
    if failed.is_empty() {
        Ok(())
    } else {
        Err(Error::new(format!(
            "skill retrieval fell short (recall@3 under {MIN_RECALL} or a strong false match) for {}",
            failed.join(", ")
        )))
    }
}
