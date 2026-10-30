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

//! `moqui check` and `moqui refs` (plans/design-dev-aide.md 13.6, F13): the checks of `moqui_devkit::check` and
//! the references of `moqui_devkit::refs` from the command line, on a model-only boot as `moqui types` boots,
//! for a developer, a git hook or a coding agent outside the server.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;
use moqui_devkit::index::{Direction, MemoryIndex};
use moqui_devkit::{check, Problem, Severity};
use moqui_lib::error::Error;

struct CheckArgs {
    paths: Vec<PathBuf>,
    json: bool,
    extensions: bool,
    server: Option<String>,
}

fn parse_check(args: &[String]) -> Result<CheckArgs, Error> {
    let mut parsed = CheckArgs {
        paths: Vec::new(),
        json: false,
        extensions: false,
        server: None,
    };
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--json" => parsed.json = true,
            "--extensions" => parsed.extensions = true,
            "--server" => {
                parsed.server = Some(
                    rest.next()
                        .cloned()
                        .ok_or_else(|| crate::usage::command_usage("check"))?,
                );
            }
            other if let Some(url) = other.strip_prefix("--server=") => parsed.server = Some(url.to_string()),
            other if other.starts_with('-') => return Err(crate::usage::command_usage("check")),
            other => parsed.paths.push(PathBuf::from(other)),
        }
    }
    Ok(parsed)
}

/// `moqui check [paths] [--extensions] [--json]`: the problems of each file, as `path:line:col: severity: message [rule,
/// skill#section]` or one JSON object per line; with no paths, the XML files git shows as changed under the current
/// directory. Exit status 1 when there is an error, 0 otherwise (warnings do not fail it).
pub(crate) fn dispatch_check(args: &[String]) -> Result<(), Error> {
    let args = parse_check(args)?;
    crate::logging::install_with_console(Some(tracing::Level::ERROR));
    let paths = if args.paths.is_empty() && !args.extensions {
        changed_files()?
    } else {
        args.paths.clone()
    };
    let mut files = Vec::new();
    for path in &paths {
        if path.is_dir() {
            collect_xml(path, &mut files);
        } else {
            files.push(path.clone());
        }
    }
    let mut problems: Vec<Problem> = Vec::new();
    match &args.server {
        // a running dev instance's checks, file by file; one that fails is reported and the others go on
        Some(url) => {
            #[cfg(feature = "tui")]
            {
                if args.extensions {
                    return Err(Error::new("moqui check --extensions runs here, not with --server"));
                }
                let remote = crate::devtools_remote::Remote::connect(url)?;
                for file in &files {
                    let text = std::fs::read_to_string(file)
                        .map_err(|err| Error::new(format!("{}: {err}", file.display())))?;
                    match remote.check(file, &text) {
                        Ok(found) => problems.extend(found),
                        Err(err) => eprintln!("{}: not checked: {err}", file.display()),
                    }
                }
            }
            #[cfg(not(feature = "tui"))]
            return Err(Error::new(format!(
                "moqui check --server {url} needs the tui feature's client"
            )));
        }
        None => {
            let factory = model_only()?;
            for file in &files {
                problems.extend(check::check_file(&factory, file));
            }
            if args.extensions {
                problems.extend(check::check_extensions(&factory));
            }
        }
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    for problem in &problems {
        if args.json {
            println!("{}", serde_json::to_string(problem).unwrap_or_default());
        } else {
            println!("{}", line_of(problem, &cwd));
        }
    }
    let errors = problems
        .iter()
        .filter(|problem| problem.severity == Severity::Error)
        .count();
    if !args.json {
        let files = if args.paths.is_empty() && !args.extensions {
            format!(
                "{} changed file{}",
                paths.len(),
                if paths.len() == 1 { "" } else { "s" }
            )
        } else {
            format!("{} path{}", paths.len(), if paths.len() == 1 { "" } else { "s" })
        };
        eprintln!(
            "moqui check: {files}, {errors} error{}, {} warning{}",
            if errors == 1 { "" } else { "s" },
            problems.len() - errors,
            if problems.len() - errors == 1 { "" } else { "s" }
        );
    }
    if errors > 0 {
        return Err(Error::new(format!(
            "moqui check found {errors} error{}",
            if errors == 1 { "" } else { "s" }
        )));
    }
    Ok(())
}

fn line_of(problem: &Problem, cwd: &Path) -> String {
    let path = Path::new(&problem.path);
    let shown = path.strip_prefix(cwd).unwrap_or(path).display();
    let severity = match problem.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
    };
    let skill = problem
        .skill
        .as_deref()
        .map(|skill| format!(", {skill}"))
        .unwrap_or_default();
    format!(
        "{shown}:{}:{}: {severity}: {} [{}{skill}]",
        problem.line, problem.column, problem.message, problem.rule
    )
}

/// The XML files git shows as changed (modified, added, renamed, untracked) under the current directory.
fn changed_files() -> Result<Vec<PathBuf>, Error> {
    let top = git(&["rev-parse", "--show-toplevel"])?;
    let top = PathBuf::from(top.trim());
    let status = git(&["status", "--porcelain=v1", "-z", "--untracked-files=all", "--", "."])?;
    let mut paths = Vec::new();
    let mut entries = status.split('\0');
    while let Some(entry) = entries.next() {
        if entry.len() < 4 {
            continue;
        }
        let (code, path) = entry.split_at(3);
        // A rename lists the old path next; it is gone.
        if code.starts_with('R') || code.starts_with('C') {
            entries.next();
        }
        if code.contains('D') {
            continue;
        }
        let path = top.join(path);
        if path.extension().is_some_and(|ext| ext == "xml") && path.is_file() {
            paths.push(path);
        }
    }
    Ok(paths)
}

/// Run `git` with `args` for its output, as the dev tools run it: argv, no prompt, no optional locks.
fn git(args: &[&str]) -> Result<String, Error> {
    let output = std::process::Command::new("git")
        .args(["-c", "core.quotepath=off", "-c", "color.ui=false"])
        .args(args)
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|err| Error::new("moqui check with no paths runs git, which could not start").with_source(err))?;
    if !output.status.success() {
        return Err(Error::new(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn collect_xml(dir: &Path, into: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        // The XSDs' own catalog is not a Moqui file.
        let skipped = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with('.') || matches!(name, "target" | "node_modules" | "xsd"));
        if skipped {
            continue;
        }
        if path.is_dir() {
            collect_xml(&path, into);
        } else if path.extension().is_some_and(|ext| ext == "xml") {
            into.push(path);
        }
    }
}

/// A factory booted on the runtime with an entity facade of the framework's and the components' entities that opens no
/// database, as `moqui types` boots.
pub(crate) fn model_only() -> Result<Arc<ExecutionContextFactory>, Error> {
    let (runtime, framework) = crate::runtime_framework_dirs();
    let factory = ExecutionContextFactory::boot_with_framework(&runtime, &framework)
        .map_err(|err| Error::new(err.to_string()))?;
    let pool = moqui_core::transaction::PoolConfig {
        min: 0,
        max: 1,
        reserve: 0,
        acquire_timeout: std::time::Duration::from_millis(10),
        ..Default::default()
    };
    let facade = crate::db::entity_facade::open_entity_facade_for_factory(
        &factory,
        Arc::new(crate::qvt_report::ModelOnly),
        pool,
        "postgres",
    )?;
    facade.set_model_only();
    crate::db::entity_facade::attach(&factory, facade);
    Ok(factory)
}

/// `moqui refs <name> [--in | --out] [--kind k] [--json]`: what uses an artifact (`--in`, the default) or what it uses
/// (`--out`), from the artifact index read into memory (plans/design-dev-aide.md 7.1, 13.6). The name is an index id
/// (`entity:moqui.example.Example`), a full or short name (`OrderHeader`, `place#Order`), or a file; `--kind` keeps the
/// references whose other side is of that kind (entity, field, service, screen, transition, form, seca, ...).
pub(crate) fn dispatch_refs(args: &[String]) -> Result<(), Error> {
    let mut name = None;
    let mut outgoing = false;
    let mut json = false;
    let mut kinds = Vec::new();
    let mut server = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--in" => outgoing = false,
            "--out" => outgoing = true,
            "--json" => json = true,
            "--server" => server = rest.next().cloned(),
            other if let Some(url) = other.strip_prefix("--server=") => server = Some(url.to_string()),
            "--kind" => kinds.extend(rest.next().cloned()),
            other if let Some(kind) = other.strip_prefix("--kind=") => kinds.push(kind.to_string()),
            other if other.starts_with('-') => return Err(crate::usage::command_usage("refs")),
            other if name.is_none() => name = Some(other.to_string()),
            _ => return Err(crate::usage::command_usage("refs")),
        }
    }
    let name = name.ok_or_else(|| crate::usage::command_usage("refs"))?;
    crate::logging::install_with_console(Some(tracing::Level::ERROR));
    if let Some(url) = server {
        return remote_refs(&url, &name, outgoing, json, &kinds);
    }
    let factory = model_only()?;
    let index = MemoryIndex::build(&factory);
    // a file: every artifact it defines
    let targets: Vec<String> = match Path::new(&name)
        .exists()
        .then(|| std::fs::canonicalize(&name).ok())
        .flatten()
    {
        Some(path) => {
            let path = path.display().to_string();
            index
                .artifacts
                .iter()
                .filter(|artifact| artifact.path == path && artifact.parent.is_none())
                .map(|artifact| artifact.id.clone())
                .collect()
        }
        None => {
            let location = name.strip_prefix("component://").map(|_| format!("screen:{name}"));
            location
                .and_then(|id| index.describe(&id))
                .or_else(|| index.named(&name))
                .map(|artifact| vec![artifact.id])
                .unwrap_or_default()
        }
    };
    if targets.is_empty() {
        return Err(Error::new(format!("moqui refs: no artifact is named {name}")));
    }
    let direction = if outgoing { Direction::Out } else { Direction::In };
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut count = 0;
    for target in &targets {
        for found in index.refs(target, direction) {
            if !kinds.is_empty() {
                let kind = found
                    .other
                    .as_ref()
                    .map_or_else(|| found.to_kind.clone(), |other| other.kind.clone());
                if !kinds.contains(&kind) {
                    continue;
                }
            }
            count += 1;
            if json {
                println!("{}", serde_json::to_string(&found).unwrap_or_default());
                continue;
            }
            let path = Path::new(&found.path);
            let other = match direction {
                Direction::In => found.from.clone(),
                Direction::Out => found
                    .to_id
                    .clone()
                    .unwrap_or_else(|| format!("{}:{} (unresolved)", found.to_kind, found.to_name)),
            };
            println!(
                "{}:{}: {} {other}",
                path.strip_prefix(&cwd).unwrap_or(path).display(),
                found.line,
                found.ref_type,
            );
        }
    }
    eprintln!(
        "moqui refs: {count} reference{} {} {}",
        if count == 1 { "" } else { "s" },
        if outgoing { "from" } else { "to" },
        targets.join(", ")
    );
    Ok(())
}

/// `moqui refs --server URL`: the references from a running dev instance's artifact index.
fn remote_refs(url: &str, name: &str, outgoing: bool, json: bool, kinds: &[String]) -> Result<(), Error> {
    #[cfg(feature = "tui")]
    {
        let remote = crate::devtools_remote::Remote::connect(url)?;
        let answer = remote.refs(name, if outgoing { "out" } else { "in" })?;
        let mut count = 0;
        for row in answer["refs"].as_array().into_iter().flatten() {
            let kind = row["artifact"]["kind"].as_str().unwrap_or_default();
            if !kinds.is_empty() && !kinds.iter().any(|wanted| wanted == kind) {
                continue;
            }
            count += 1;
            if json {
                println!("{row}");
                continue;
            }
            let other = row["artifact"]["id"]
                .as_str()
                .or_else(|| row["name"].as_str())
                .unwrap_or_default();
            println!(
                "{}:{}: {} {other}",
                row["file"].as_str().unwrap_or_default(),
                row["line"],
                row["how"].as_str().unwrap_or_default()
            );
        }
        eprintln!(
            "moqui refs: {count} reference{} {} {}",
            if count == 1 { "" } else { "s" },
            if outgoing { "from" } else { "to" },
            answer["artifact"].as_str().unwrap_or(name)
        );
        Ok(())
    }
    #[cfg(not(feature = "tui"))]
    {
        let _ = (name, outgoing, json, kinds);
        Err(Error::new(format!(
            "moqui refs --server {url} needs the tui feature's client"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_takes_the_server_either_way() {
        let args = |list: &[&str]| list.iter().map(|arg| (*arg).to_string()).collect::<Vec<_>>();
        let parsed = parse_check(&args(&["--server=https://a.test", "a.xml"])).unwrap();
        assert_eq!(parsed.server.as_deref(), Some("https://a.test"));
        assert_eq!(parsed.paths, [PathBuf::from("a.xml")]);
        let parsed = parse_check(&args(&["--server", "https://b.test"])).unwrap();
        assert_eq!(parsed.server.as_deref(), Some("https://b.test"));
        assert!(parse_check(&args(&["--serverx"])).is_err());
    }
}
