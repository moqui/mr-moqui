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

//! The summary of a run of the security proofs, `moqui test-client --proofs` (docs/security/proofs/README.md). A proof
//! is a feature of a spec in `test/proofs/` whose name starts with the requirement it is filed under, `v5.0.0-7.2.4: a
//! sign-in issues a new session id`. The run writes `proofs.json` and `proofs.md` into its output directory: for each
//! component, the rows of the responsibility matrix its `test/proofs/suite.js` names (`matrix: "..."`), each with the
//! run's proofs of that requirement and their result, so that a certifier can attach it as evidence. A feature filed
//! under no requirement that passes fails the run: it proves nothing the summary can show.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use moqui_devkit::testkit::{suite_matrix, Outcome};
use moqui_lib::error::Error;
use serde_json::{json, Value};

use crate::test_cmd::report::ComponentSpec;
use crate::test_cmd::{TestComponent, PROOFS_TREE};

/// Where the statuses of a matrix are explained, for the summary's readers.
const STATUSES: &str = "https://github.com/moqui/mr-moqui/blob/master/docs/security/proofs/README.md";

/// What the summary says of the run.
pub(super) struct Run<'a> {
    /// The server's URL, or that `--serve` started the servers.
    pub server: &'a str,
    /// The profile's name (`--profile`).
    pub profile: &'a str,
}

/// What [`write_summary`] did: the lines for the end of the run, and the features filed under no requirement that
/// passed, each a failure of the run.
pub(super) struct Written {
    pub lines: Vec<String>,
    pub unfiled: usize,
}

/// The requirement a feature is filed under: the id its name starts with, before a colon, `v5.0.0-7.2.4` of
/// `v5.0.0-7.2.4: a sign-in issues a new session id` (the standard's version, a hyphen, then the chapter, section and
/// requirement). None for any other name.
pub(super) fn requirement_of(name: &str) -> Option<&str> {
    let (id, _) = name.split_once(':')?;
    let (version, requirement) = id.strip_prefix('v')?.split_once('-')?;
    let numbers = |text: &str| {
        let parts: Vec<&str> = text.split('.').collect();
        parts.len() == 3
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    };
    (numbers(version) && numbers(requirement)).then_some(id)
}

/// The order of requirement ids: by their numbers, so 1.10.1 follows 1.9.1.
fn id_key(id: &str) -> Vec<u32> {
    id.trim_start_matches('v')
        .split(['.', '-'])
        .map(|part| part.parse().unwrap_or(u32::MAX))
        .collect()
}

/// A requirement row of a responsibility matrix, `| Requirement | L | Topic | Status | ... |`; the other columns are for
/// its readers.
#[derive(Clone, Debug, Default, PartialEq)]
struct Row {
    id: String,
    level: String,
    topic: String,
    status: String,
}

/// The cells of a Markdown table line, trimmed; `\|` stays inside its cell.
fn cells(line: &str) -> Vec<String> {
    let line = line.trim();
    let line = line.strip_prefix('|').unwrap_or(line);
    let mut out = Vec::new();
    let mut cell = String::new();
    let mut escaped = false;
    for ch in line.chars() {
        if ch == '|' && !escaped {
            out.push(cell.trim().to_string());
            cell.clear();
        } else {
            cell.push(ch);
        }
        escaped = ch == '\\' && !escaped;
    }
    if !cell.trim().is_empty() {
        out.push(cell.trim().to_string());
    }
    out
}

/// The requirement rows of a matrix file: the rows of each table whose header starts with `Requirement` and has `L`,
/// `Topic` and `Status` columns, whose first cell is a requirement id.
fn matrix_rows(text: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut columns: Option<(usize, usize, usize)> = None;
    for line in text.lines() {
        if !line.trim_start().starts_with('|') {
            columns = None;
            continue;
        }
        let cells = cells(line);
        if cells.first().map(String::as_str) == Some("Requirement") {
            let at = |name: &str| cells.iter().position(|cell| cell == name);
            columns = match (at("L"), at("Topic"), at("Status")) {
                (Some(level), Some(topic), Some(status)) => Some((level, topic, status)),
                _ => None,
            };
            continue;
        }
        let Some((level, topic, status)) = columns else {
            continue;
        };
        let Some(id) = cells
            .first()
            .filter(|cell| requirement_of(&format!("{cell}:")) == Some(cell.as_str()))
        else {
            continue;
        };
        let cell = |at: usize| cells.get(at).cloned().unwrap_or_default();
        rows.push(Row {
            id: id.clone(),
            level: cell(level),
            topic: cell(topic),
            status: cell(status),
        });
    }
    rows
}

/// A path as the reader of the summary finds it: from the working directory when it is under it.
fn shown(path: &Path) -> String {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    std::env::current_dir()
        .ok()
        .and_then(|cwd| path.strip_prefix(cwd).ok().map(Path::to_path_buf))
        .unwrap_or(path)
        .display()
        .to_string()
}

/// The matrix files a component's `test/proofs/suite.js` names (relative to the component's directory; a directory is
/// its `.md` files), as they are shown, with what could not be found or read.
fn matrix_files(component: &TestComponent) -> (Vec<PathBuf>, Vec<String>, Vec<String>) {
    let suite = component.dir.join(PROOFS_TREE).join("suite.js");
    let Ok(text) = std::fs::read_to_string(&suite) else {
        return (Vec::new(), Vec::new(), Vec::new());
    };
    let named = match suite_matrix(&text) {
        Ok(named) => named,
        Err(err) => {
            return (
                Vec::new(),
                Vec::new(),
                vec![format!("{}/{PROOFS_TREE}/suite.js: {err}", component.name)],
            )
        }
    };
    let mut files = Vec::new();
    let mut names = Vec::new();
    let mut problems = Vec::new();
    for name in named {
        let path = component.dir.join(&name);
        if path.is_dir() {
            match std::fs::read_dir(&path) {
                Ok(entries) => {
                    let mut found: Vec<PathBuf> = entries
                        .filter_map(Result::ok)
                        .map(|entry| entry.path())
                        .filter(|file| file.extension().is_some_and(|ext| ext == "md"))
                        .collect();
                    found.sort();
                    files.extend(found);
                }
                Err(err) => problems.push(format!("{}: the matrix {}: {err}", component.name, path.display())),
            }
        } else if !path.is_file() {
            problems.push(format!(
                "{}: the matrix {name} its suite.js names is not there ({}); the summary has the run's results only",
                component.name,
                path.display()
            ));
            continue;
        } else {
            files.push(path.clone());
        }
        names.push(shown(&path));
    }
    (files, names, problems)
}

/// One proof of a run.
struct Proof<'a> {
    component: &'a str,
    spec: &'a str,
    feature: &'a str,
    requirement: &'a str,
    outcome: &'static str,
    reason: Option<String>,
    seconds: f64,
}

impl Proof<'_> {
    fn to_json(&self) -> Value {
        json!({
            "component": self.component,
            "spec": self.spec,
            "feature": self.feature,
            "outcome": self.outcome,
            "reason": self.reason,
            "seconds": (self.seconds * 1000.0).round() / 1000.0,
        })
    }

    /// The proof in a Markdown cell: its spec, its claim (the name after the id) and its outcome, with the reason of a
    /// skip or the first line of a failure.
    fn cell(&self) -> String {
        let claim = self
            .feature
            .split_once(':')
            .map_or(self.feature, |(_, rest)| rest.trim());
        let mut text = format!("{} › {claim}: {}", self.spec, self.outcome);
        if let Some(reason) = self.reason.as_deref().filter(|reason| !reason.is_empty()) {
            let reason: String = reason.chars().take(160).collect();
            let _ = write!(text, " ({reason})");
        }
        text
    }
}

fn outcome_of(outcome: &Outcome) -> (&'static str, Option<String>) {
    match outcome {
        Outcome::Passed => ("passed", None),
        Outcome::Failed(failure) => ("failed", Some(failure.message.lines().next().unwrap_or("").to_string())),
        Outcome::Ignored(reason) => ("ignored", Some(reason.clone())),
        Outcome::Skipped(reason) => ("skipped", Some(reason.clone())),
    }
}

/// A requirement's result from its proofs: failed when one failed, passed when all passed, partial when some passed
/// and the rest were skipped or ignored, skipped when none ran, none when the run has no proof of it.
fn result_of(proofs: &[&Proof]) -> &'static str {
    if proofs.is_empty() {
        "none"
    } else if proofs.iter().any(|proof| proof.outcome == "failed") {
        "failed"
    } else if proofs.iter().all(|proof| proof.outcome == "passed") {
        "passed"
    } else if proofs.iter().any(|proof| proof.outcome == "passed") {
        "partial"
    } else {
        "skipped"
    }
}

/// A Markdown table cell: no `|` and no line break.
fn md(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

/// The counts of results, in the order the summary gives them.
const RESULTS: [&str; 5] = ["passed", "partial", "failed", "skipped", "none"];

/// Write `proofs.json` and `proofs.md` for a run of `components` with `results` into `out_dir`.
///
/// # Errors
///
/// When the files cannot be written.
pub(super) fn write_summary(
    out_dir: &Path,
    components: &[TestComponent],
    results: &[ComponentSpec],
    run: &Run,
) -> Result<Written, Error> {
    let mut proofs: Vec<Proof> = Vec::new();
    let mut unfiled: Vec<Value> = Vec::new();
    let mut unfiled_lines: Vec<String> = Vec::new();
    let mut spec_failures: Vec<Value> = Vec::new();
    for spec in results {
        if let Some(failure) = &spec.result.setup_failure {
            spec_failures.push(json!({
                "component": spec.component,
                "spec": spec.result.name,
                "message": failure.message.lines().next().unwrap_or(""),
            }));
        }
        for case in &spec.result.cases {
            let (outcome, reason) = outcome_of(&case.outcome);
            match requirement_of(&case.name) {
                Some(requirement) => proofs.push(Proof {
                    component: &spec.component,
                    spec: &spec.result.name,
                    feature: &case.name,
                    requirement,
                    outcome,
                    reason,
                    seconds: case.duration.as_secs_f64(),
                }),
                None => {
                    if outcome == "passed" {
                        unfiled_lines.push(format!("  {}/{} > {}", spec.component, spec.result.name, case.name));
                    }
                    unfiled.push(json!({
                        "component": spec.component,
                        "spec": spec.result.name,
                        "feature": case.name,
                        "outcome": outcome,
                        "reason": reason,
                    }));
                }
            }
        }
    }

    let finished = jiff::Timestamp::now().strftime("%Y-%m-%dT%H:%M:%SZ").to_string();
    let client = format!("moqui test-client {}", env!("CARGO_PKG_VERSION"));
    let mut lines = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    let mut sections = Vec::new();
    let mut markdown = String::new();
    let _ = writeln!(markdown, "# Security proofs\n");
    let _ = writeln!(
        markdown,
        "A run of `moqui test-client --proofs` ({client}) finished {finished}, profile {}, against {}. Each proof is \
         filed under one requirement of the OWASP Application Security Verification Standard, cited by its id; the \
         requirements' own text is the standard's ([OWASP ASVS](https://github.com/OWASP/ASVS), CC BY-SA 4.0). The \
         statuses are explained in [mr-moqui's security proofs]({STATUSES}).\n",
        md(run.profile),
        md(run.server)
    );
    let _ = writeln!(
        markdown,
        "A result is **passed** when every proof of the requirement passed, **partial** when some passed and the rest \
         were skipped, **failed** when one failed, **skipped** when none could run here (each says what it needs), and \
         **none** when the run has no proof of it.\n"
    );
    for component in components {
        let (files, names, mut found) = matrix_files(component);
        problems.append(&mut found);
        let mut rows: Vec<Row> = Vec::new();
        for file in &files {
            match std::fs::read_to_string(file) {
                Ok(text) => {
                    for row in matrix_rows(&text) {
                        if !rows.iter().any(|have| have.id == row.id) {
                            rows.push(row);
                        }
                    }
                }
                Err(err) => problems.push(format!("{}: {}: {err}", component.name, file.display())),
            }
        }
        let in_matrix = rows.len();
        let own: Vec<&Proof> = proofs
            .iter()
            .filter(|proof| proof.component == component.name)
            .collect();
        // a proof of a requirement the matrix has no row for (a level it leaves out, a mistyped id, no matrix) gets a
        // row of its own
        for proof in &own {
            if !rows.iter().any(|row| row.id == proof.requirement) {
                if in_matrix > 0 {
                    problems.push(format!(
                        "{}/{} > {}: {} is not in the component's matrix",
                        proof.component, proof.spec, proof.feature, proof.requirement
                    ));
                }
                rows.push(Row {
                    id: proof.requirement.to_string(),
                    ..Row::default()
                });
            }
        }
        rows.sort_by_key(|row| id_key(&row.id));
        let mut counts = [0_usize; RESULTS.len()];
        let mut requirements = Vec::new();
        let mut table = String::new();
        for row in &rows {
            let mut used: Vec<&Proof> = own
                .iter()
                .copied()
                .filter(|proof| proof.requirement == row.id)
                .collect();
            // an inherited requirement is proved where it is inherited from: the run's proofs of it in other components
            let mut via: Vec<&str> = Vec::new();
            if used.is_empty() && row.status == "Inherited" {
                used = proofs
                    .iter()
                    .filter(|proof| proof.component != component.name && proof.requirement == row.id)
                    .collect();
                for proof in &used {
                    if !via.contains(&proof.component) {
                        via.push(proof.component);
                    }
                }
            }
            let result = result_of(&used);
            if let Some(at) = RESULTS.iter().position(|name| *name == result) {
                counts[at] += 1;
            }
            requirements.push(json!({
                "id": row.id,
                "level": row.level.parse::<u8>().ok(),
                "topic": row.topic,
                "status": if row.status.is_empty() { Value::Null } else { Value::String(row.status.clone()) },
                "result": result,
                "via": via,
                "proofs": used.iter().map(|proof| proof.to_json()).collect::<Vec<_>>(),
            }));
            let proofs_cell = if used.is_empty() {
                "-".to_string()
            } else {
                let mut cell = used
                    .iter()
                    .map(|proof| md(&proof.cell()))
                    .collect::<Vec<_>>()
                    .join("<br>");
                if !via.is_empty() {
                    cell = format!("in {}: {cell}", via.join(", "));
                }
                cell
            };
            let _ = writeln!(
                table,
                "| {} | {} | {} | {} | {result} | {proofs_cell} |",
                row.id,
                md(&row.level),
                md(&row.topic),
                if row.status.is_empty() {
                    "not in the matrix".to_string()
                } else {
                    md(&row.status)
                },
            );
        }
        let with_proofs = rows.len() - counts[RESULTS.len() - 1];
        let tally = format!(
            "{with_proofs} requirements with proofs: {} passed, {} partial, {} failed, {} skipped",
            counts[0], counts[1], counts[2], counts[3]
        );
        lines.push(format!(
            "Proofs of {}: {tally}; {in_matrix} requirements in its matrix",
            component.name
        ));
        let _ = writeln!(markdown, "## {}\n", md(&component.name));
        if names.is_empty() {
            let _ = writeln!(
                markdown,
                "No matrix: its `test/proofs/suite.js` names none that was found, so only the requirements with proofs are listed. {tally}.\n"
            );
        } else {
            let _ = writeln!(
                markdown,
                "Matrix: {} ({in_matrix} requirements). {tally}.\n",
                names
                    .iter()
                    .map(|name| format!("`{}`", md(name)))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        if !rows.is_empty() {
            let _ = writeln!(markdown, "| Requirement | L | Topic | Status | Result | Proofs |");
            let _ = writeln!(markdown, "| --- | --- | --- | --- | --- | --- |");
            markdown.push_str(&table);
            markdown.push('\n');
        }
        sections.push(json!({
            "component": component.name,
            "matrix": names,
            "counts": {
                "requirements": rows.len(),
                "inMatrix": in_matrix,
                "withProofs": with_proofs,
                "passed": counts[0],
                "partial": counts[1],
                "failed": counts[2],
                "skipped": counts[3],
                "none": counts[4],
            },
            "requirements": requirements,
        }));
    }
    if !unfiled.is_empty() {
        let _ = writeln!(markdown, "## Not filed under a requirement\n");
        for case in &unfiled {
            let _ = writeln!(
                markdown,
                "- {}/{} > {}: {}",
                md(case["component"].as_str().unwrap_or("")),
                md(case["spec"].as_str().unwrap_or("")),
                md(case["feature"].as_str().unwrap_or("")),
                case["outcome"].as_str().unwrap_or("")
            );
        }
        markdown.push('\n');
    }
    if !spec_failures.is_empty() {
        let _ = writeln!(markdown, "## Specs that did not start\n");
        for failure in &spec_failures {
            let _ = writeln!(
                markdown,
                "- {}/{}: {}",
                md(failure["component"].as_str().unwrap_or("")),
                md(failure["spec"].as_str().unwrap_or("")),
                md(failure["message"].as_str().unwrap_or(""))
            );
        }
        markdown.push('\n');
    }
    if !problems.is_empty() {
        let _ = writeln!(markdown, "## Notes\n");
        for problem in &problems {
            let _ = writeln!(markdown, "- {}", md(problem));
        }
        markdown.push('\n');
    }
    let summary = json!({
        "client": client,
        "finished": finished,
        "server": run.server,
        "profile": run.profile,
        "standard": {
            "name": "OWASP Application Security Verification Standard",
            "source": "https://github.com/OWASP/ASVS",
            "license": "CC BY-SA 4.0",
        },
        "statuses": STATUSES,
        "components": sections,
        "unfiled": unfiled,
        "specFailures": spec_failures,
        "notes": problems,
    });
    std::fs::create_dir_all(out_dir)
        .map_err(|err| Error::new(format!("could not create {}", out_dir.display())).with_source(err))?;
    let json_path = out_dir.join("proofs.json");
    let text = serde_json::to_string_pretty(&summary)
        .map_err(|err| Error::new("could not write the proofs summary").with_source(err))?;
    std::fs::write(&json_path, text + "\n")
        .map_err(|err| Error::new(format!("could not write {}", json_path.display())).with_source(err))?;
    let md_path = out_dir.join("proofs.md");
    std::fs::write(&md_path, markdown.trim_end().to_string() + "\n")
        .map_err(|err| Error::new(format!("could not write {}", md_path.display())).with_source(err))?;
    for problem in &problems {
        lines.push(format!("Note: {problem}"));
    }
    if !unfiled_lines.is_empty() {
        lines.push(
            "FAILED: features of test/proofs filed under no requirement (start the name with its id, \
             `v5.0.0-<chapter>.<section>.<requirement>: `, or move the feature to test/client):"
                .to_string(),
        );
        lines.extend(unfiled_lines.iter().cloned());
    }
    lines.push(format!("Proof summary: {} and proofs.json", md_path.display()));
    Ok(Written {
        lines,
        unfiled: unfiled_lines.len(),
    })
}

#[cfg(test)]
#[path = "proofs_tests.rs"]
mod tests;
