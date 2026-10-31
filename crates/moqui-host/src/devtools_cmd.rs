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

/// `--project DIR` (a Moqui project, its layout found from marker paths, [`moqui_devkit::project::detect`]) or
/// `--java DIR` (the same, read as a Java Moqui checkout whatever its sources).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectArg {
    pub dir: PathBuf,
    pub java: bool,
}

impl ProjectArg {
    /// The flag `arg` when it is `--project` or `--java` (taking its value from `rest` when not after `=`): `None` for
    /// another argument, `Some(None)` for a flag with no value.
    pub(crate) fn parse<'a>(arg: &str, rest: &mut impl Iterator<Item = &'a String>) -> Option<Option<Self>> {
        let (java, value) = match arg {
            "--project" => (false, rest.next().cloned()),
            "--java" => (true, rest.next().cloned()),
            other if let Some(dir) = other.strip_prefix("--project=") => (false, Some(dir.to_string())),
            other if let Some(dir) = other.strip_prefix("--java=") => (true, Some(dir.to_string())),
            _ => return None,
        };
        Some(value.filter(|dir| !dir.is_empty()).map(|dir| Self {
            dir: PathBuf::from(dir),
            java,
        }))
    }

    /// The project here.
    ///
    /// # Errors
    ///
    /// Returns why the directory is not one.
    pub(crate) fn detect(&self) -> Result<moqui_devkit::project::Project, Error> {
        if self.java {
            moqui_devkit::project::detect_java(&self.dir)
        } else {
            moqui_devkit::project::detect(&self.dir)
        }
    }
}

/// A `--server` connection, naming the project when one is given.
#[cfg(feature = "tui")]
pub(crate) fn remote(url: &str, project: Option<&ProjectArg>) -> Result<crate::devtools_remote::Remote, Error> {
    let remote = crate::devtools_remote::Remote::connect(url)?;
    Ok(match project {
        Some(project) => remote.with_project(&project.dir, project.java),
        None => remote,
    })
}

struct CheckArgs {
    paths: Vec<PathBuf>,
    json: bool,
    extensions: bool,
    server: Option<String>,
    project: Option<ProjectArg>,
}

fn parse_check(args: &[String]) -> Result<CheckArgs, Error> {
    let mut parsed = CheckArgs {
        paths: Vec::new(),
        json: false,
        extensions: false,
        server: None,
        project: None,
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
            other if let Some(project) = ProjectArg::parse(other, &mut rest) => {
                parsed.project = Some(project.ok_or_else(|| crate::usage::command_usage("check"))?);
            }
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
                let remote = remote(url, args.project.as_ref())?;
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
            let factory = model_only(args.project.as_ref())?;
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

pub(crate) use moqui_devkit::project::ModelDirs;

/// The directories of a model-only boot. `project` names a Moqui project (`--project`, or `--java` for a Java Moqui
/// checkout); with none, a Java checkout is found from `MOQUI_RUNTIME` (its `runtime`) or the working directory (the
/// checkout or a directory in it) when there is no MR Moqui runtime there; else the MR Moqui runtime as every command
/// finds it.
pub(crate) fn model_dirs(project: Option<&ProjectArg>) -> Result<ModelDirs, Error> {
    let found = match project {
        Some(project) => Some(project.detect()?),
        None => detected_java_checkout(),
    };
    let Some(found) = found else {
        let (runtime, framework) = crate::runtime_framework_dirs();
        return Ok(ModelDirs {
            runtime,
            framework,
            framework_component: None,
        });
    };
    let exe = std::env::current_exe().and_then(std::fs::canonicalize).ok();
    let dirs = found
        .model_dirs(|| mr_framework_for(std::env::var_os("MOQUI_FRAMEWORK").map(PathBuf::from), exe.as_deref()))?;
    tracing::info!(
        "Reading the {} Moqui project {} with the framework conf of {}",
        if found.layout == moqui_devkit::project::Layout::Java {
            "Java"
        } else {
            "MR"
        },
        found.root.display(),
        dirs.framework.display()
    );
    Ok(dirs)
}

/// `dir` when it is a Java Moqui checkout (or its runtime), by [`moqui_devkit::project::detect`]'s markers.
fn java_checkout(dir: &Path) -> Option<moqui_devkit::project::Project> {
    moqui_devkit::project::detect(dir)
        .ok()
        .filter(|project| project.layout == moqui_devkit::project::Layout::Java)
}

/// The Java Moqui checkout `MOQUI_RUNTIME` is the runtime of, or else the one the working directory is in, when the
/// working directory has no MR Moqui runtime.
fn detected_java_checkout() -> Option<moqui_devkit::project::Project> {
    if let Some(runtime) = std::env::var_os("MOQUI_RUNTIME").filter(|dir| !dir.is_empty()) {
        return java_checkout(Path::new(&runtime));
    }
    let cwd = std::env::current_dir().ok()?;
    if cwd.join("runtime/base-component/framework/component.xml").is_file() {
        return None;
    }
    cwd.ancestors().find_map(java_checkout)
}

/// MR Moqui's framework directory for reading a Java checkout: `MOQUI_FRAMEWORK`, else the one of the runtime beside
/// the executable (an installed runtime, `runtime/` beside it, or a source checkout above a `target` build).
fn mr_framework_for(configured: Option<PathBuf>, exe: Option<&Path>) -> Option<PathBuf> {
    let is_framework =
        |dir: &Path| dir.join("component.xml").is_file() && dir.join("conf/MoquiDefaultConf.xml").is_file();
    if let Some(dir) = configured.filter(|dir| !dir.as_os_str().is_empty()) {
        // One that is not a framework directory is passed over, so the error says what was not found.
        if is_framework(&dir) {
            return Some(dir);
        }
        tracing::warn!(
            "MOQUI_FRAMEWORK {} has no component.xml and conf/MoquiDefaultConf.xml, so it is not an MR Moqui framework",
            dir.display()
        );
        return None;
    }
    let exe_dir = exe?.parent()?;
    exe_dir
        .ancestors()
        .flat_map(|dir| {
            [
                dir.join("base-component/framework"),
                dir.join("runtime/base-component/framework"),
            ]
        })
        .find(|dir| is_framework(dir))
}

/// A factory booted on the runtime with an entity facade of the framework's and the components' entities that opens no
/// database, as `moqui types` boots. `project` names another Moqui project to read instead ([`model_dirs`]).
pub(crate) fn model_only(project: Option<&ProjectArg>) -> Result<Arc<ExecutionContextFactory>, Error> {
    model_only_in(model_dirs(project)?)
}

/// [`model_only`] on `dirs`.
fn model_only_in(dirs: ModelDirs) -> Result<Arc<ExecutionContextFactory>, Error> {
    let options = moqui_core::context::BootOptions {
        framework_component: dirs.framework_component,
        ..moqui_core::context::BootOptions::default()
    };
    let factory = ExecutionContextFactory::boot_with_options(&dirs.runtime, &dirs.framework, options)
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
    let mut project = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--in" => outgoing = false,
            "--out" => outgoing = true,
            "--json" => json = true,
            "--server" => server = Some(rest.next().ok_or_else(|| crate::usage::command_usage("refs"))?.clone()),
            other if let Some(url) = other.strip_prefix("--server=") => server = Some(url.to_string()),
            other if let Some(found) = ProjectArg::parse(other, &mut rest) => {
                project = Some(found.ok_or_else(|| crate::usage::command_usage("refs"))?);
            }
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
        return remote_refs(&url, project.as_ref(), &name, outgoing, json, &kinds);
    }
    let factory = model_only(project.as_ref())?;
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
fn remote_refs(
    url: &str,
    project: Option<&ProjectArg>,
    name: &str,
    outgoing: bool,
    json: bool,
    kinds: &[String],
) -> Result<(), Error> {
    #[cfg(feature = "tui")]
    {
        let remote = remote(url, project)?;
        let direction = if outgoing { "out" } else { "in" };
        // a file: every artifact it defines
        let answer = if Path::new(name).is_file() {
            remote.file_refs(Path::new(name), direction)?
        } else {
            remote.refs(name, direction)?
        };
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
        let _ = (project, name, outgoing, json, kinds);
        Err(Error::new(format!(
            "moqui refs --server {url} needs the tui feature's client"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_with_a_flag_without_its_value_is_the_usage_error() {
        for flag in ["--server", "--java", "--project"] {
            let err = dispatch_refs(&["a#B".to_string(), flag.to_string()])
                .unwrap_err()
                .to_string();
            assert!(err.contains("moqui help refs"), "{flag}: {err}");
        }
    }

    #[test]
    fn check_takes_the_server_either_way() {
        let args = |list: &[&str]| list.iter().map(|arg| (*arg).to_string()).collect::<Vec<_>>();
        let parsed = parse_check(&args(&["--server=https://a.test", "a.xml"])).unwrap();
        assert_eq!(parsed.server.as_deref(), Some("https://a.test"));
        assert_eq!(parsed.paths, [PathBuf::from("a.xml")]);
        let parsed = parse_check(&args(&["--server", "https://b.test"])).unwrap();
        assert_eq!(parsed.server.as_deref(), Some("https://b.test"));
        assert!(parse_check(&args(&["--serverx"])).is_err());
        let parsed = parse_check(&args(&["--java", "/src/moqui", "a.xml"])).unwrap();
        assert_eq!(
            parsed.project,
            Some(ProjectArg {
                dir: PathBuf::from("/src/moqui"),
                java: true
            })
        );
        assert!(parse_check(&args(&["--java"])).is_err());
        let parsed = parse_check(&args(&["--project=/src/mr", "a.xml"])).unwrap();
        assert_eq!(
            parsed.project,
            Some(ProjectArg {
                dir: PathBuf::from("/src/mr"),
                java: false
            })
        );
        assert!(parse_check(&args(&["--project"])).is_err());
    }

    /// A Java Moqui checkout in `dir`: the framework (no component.xml, no conf) with an entity of its own, a view
    /// whose member is no entity and a relationship to no entity, and a component whose screen and service use them.
    fn java_checkout_at(dir: &Path) {
        let write = |path: &str, text: &str| {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write("framework/build.gradle", "");
        write("framework/src/main/groovy/org/moqui/Moqui.groovy", "");
        write(
            "framework/entity/OlapEntities.xml",
            r#"<entities>
    <entity entity-name="JavaOnlyFact" package="moqui.olap">
        <field name="factId" type="id" is-pk="true"/>
        <relationship type="one" related="moqui.olap.NoSuchDimension"><key-map field-name="factId"/></relationship>
    </entity>
    <view-entity entity-name="BrokenView" package="moqui.olap">
        <member-entity entity-alias="F" entity-name="moqui.olap.NoSuchFact"/>
        <alias-all entity-alias="F"/>
    </view-entity>
</entities>"#,
        );
        write(
            "runtime/base-component/tools/component.xml",
            r#"<component name="tools"/>"#,
        );
        write(
            "runtime/component/fp-core/component.xml",
            r#"<component name="fp-core"/>"#,
        );
        write(
            "runtime/component/fp-core/entity/PicklistEntities.xml",
            r#"<entities>
    <entity entity-name="PicklistBatch" package="fp.picklist">
        <field name="batchId" type="id" is-pk="true"/><field name="statusId" type="id"/>
    </entity>
    <view-entity entity-name="BatchFact" package="fp.picklist">
        <member-entity entity-alias="B" entity-name="fp.picklist.PicklistBatch"/>
        <member-entity entity-alias="F" entity-name="moqui.olap.JavaOnlyFact" join-from-alias="B">
            <key-map field-name="batchId" related="factId"/></member-entity>
        <alias-all entity-alias="B"/>
    </view-entity>
</entities>"#,
        );
        write(
            "runtime/component/fp-core/screen/Batch/BatchDetails.xml",
            r#"<screen>
    <transition name="updateBatch">
        <actions><service-call name="update#fp.picklist.PicklistBatch" in-map="[batchId:batchId]"/></actions>
        <default-response url="."/>
    </transition>
    <widgets/>
</screen>"#,
        );
    }

    /// `moqui refs` on a Java Moqui checkout (no runtime/base-component/framework): found as one, booted with this
    /// repository's framework conf, its framework's own entities indexed, and a view or relationship that does not
    /// resolve left out of the model with the rest loaded.
    #[test]
    fn a_java_checkout_is_read_directly() {
        let dir = std::env::temp_dir().join(format!("moqui-java-checkout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        java_checkout_at(&dir);
        let checkout = java_checkout(&dir).expect("a Java checkout").root;
        // its runtime names the same checkout
        assert_eq!(
            java_checkout(&dir.join("runtime")).map(|found| found.root),
            Some(checkout.clone())
        );
        // an MR Moqui runtime is not one
        let repo_runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime");
        assert!(java_checkout(&repo_runtime).is_none());
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let framework = repo.join("runtime/base-component/framework");
        assert_eq!(
            mr_framework_for(None, Some(&repo.join("target/develop/moqui")))
                .map(|dir| std::fs::canonicalize(dir).unwrap()),
            Some(std::fs::canonicalize(&framework).unwrap())
        );
        assert_eq!(mr_framework_for(Some(framework.clone()), None), Some(framework.clone()));
        let factory = model_only_in(ModelDirs {
            runtime: checkout.join("runtime"),
            framework,
            framework_component: Some(checkout.join("framework")),
        });
        let factory = factory.expect("the model-only boot of a Java checkout");
        assert_eq!(
            factory.framework_component_dir(),
            std::fs::canonicalize(checkout.join("framework")).unwrap()
        );
        let names: Vec<&str> = factory
            .components()
            .iter()
            .map(|component| component.name.as_str())
            .collect();
        assert!(
            names.contains(&"framework") && names.contains(&"fp-core") && names.contains(&"tools"),
            "{names:?}"
        );
        let facade = factory.entity().expect("an entity facade");
        facade.with_model(|model| {
            assert!(model.entity("moqui.olap.JavaOnlyFact").is_ok());
            assert!(model.entity("fp.picklist.BatchFact").is_ok());
            assert!(model.entity("moqui.olap.BrokenView").is_err());
            // this repository's framework entities are not loaded for a Java checkout
            assert!(model.entity("moqui.basic.Enumeration").is_err());
            let problems = model.problems().join("\n");
            assert!(problems.contains("moqui.olap.BrokenView"), "{problems}");
            assert!(problems.contains("moqui.olap.NoSuchDimension"), "{problems}");
        });
        let index = MemoryIndex::build(&factory);
        let screen = "transition:component://fp-core/screen/Batch/BatchDetails.xml#updateBatch:any";
        let used: Vec<Option<String>> = index
            .refs(screen, Direction::Out)
            .into_iter()
            .map(|found| found.to_id)
            .collect();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(used, [Some("entity:fp.picklist.PicklistBatch".to_string())]);
        assert!(index.describe("entity:moqui.olap.JavaOnlyFact").is_some());
        assert!(index
            .refs("entity:moqui.olap.JavaOnlyFact", Direction::In)
            .iter()
            .any(|found| found.from == "entity:fp.picklist.BatchFact"),);
    }
}
