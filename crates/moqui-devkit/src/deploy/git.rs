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

//! Deploy a component from a git URL and branch (plans/Component-Deploy.md, Phase 3). Three storages, chosen per
//! component:
//! - database: a `db` GitRepository imports the branch; its files are written to `runtime/deployed-component/<name>`;
//! - bare: a `remote` GitRepository mirror under `runtime/git`; its files are written there too;
//! - clone: a working clone, by the `git` command, in a component directory, which a developer can edit and commit in.
//!
//! A preview fetches the branch, writes its files to `.staging-<name>` beside where they go, and shows what loading
//! them would do ([`super::load::preview_staged`]); deploy moves them into place and loads the component
//! ([`super::load::load`]). An update fetches again, writes the new commit beside the deployed one, and reloads the
//! paths that changed. Deploying runs the component's code and loads its data, so it needs the sealed
//! `COMPONENT_DEPLOY` permission, and on a production instance also `component_deploy_enabled=true`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use moqui_core::context::conf_reload;
use moqui_core::context::{ComponentInfo, ExecutionContext, ExecutionContextFactory};
use moqui_db::sql::Condition;
use moqui_git::checkout::FetchCredentials;
use moqui_git::GitProvider;
use moqui_lib::object::Value;
use serde::Serialize;

use super::load::{self, LoadPreview, LoadReport, Mode, Shared};
use crate::reload;

const DEPLOYED: &str = "moqui.server.DeployedComponent";
const GIT_REPOSITORY: &str = "moqui.resource.GitRepository";
/// The component directory the database and bare storages write to, under the runtime.
const DEPLOYED_DIR: &str = "deployed-component";

/// Where a deployed component's repository and files are kept.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Storage {
    #[default]
    Database,
    Bare,
    Clone,
}

impl Storage {
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "" | "database" | "db" => Some(Self::Database),
            "bare" => Some(Self::Bare),
            "clone" => Some(Self::Clone),
            _ => None,
        }
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Database => "database",
            Self::Bare => "bare",
            Self::Clone => "clone",
        }
    }
}

/// What to deploy.
#[derive(Clone, Debug, Default)]
pub struct DeployRequest {
    pub url: String,
    pub branch: String,
    pub storage: Storage,
    /// For a clone: the component directory it goes in, relative to the runtime; `component` when empty.
    pub directory: String,
    pub username: Option<String>,
    pub token: Option<String>,
}

/// What deploying would do: the commit fetched and staged, and what loading it would do.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeployPreview {
    pub component: String,
    pub commit: String,
    pub storage: String,
    /// Where its files are staged, and where they go.
    pub staged: String,
    pub location: String,
    pub load: LoadPreview,
    /// Files not written: symbolic links and submodules.
    pub skipped: Vec<String>,
    /// Git LFS pointer files, written as the pointers they are.
    pub lfs_pointers: Vec<String>,
}

/// What an update would do: the commits, the paths that changed, and its conf changes.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePreview {
    pub component: String,
    pub from: String,
    pub to: String,
    pub changed: Vec<String>,
    pub conf: Option<conf_reload::ConfReport>,
    pub refusals: Vec<String>,
}

/// Whether the current user may deploy here: the `COMPONENT_DEPLOY` permission, and on a production instance
/// `component_deploy_enabled=true` too, refused there with no more said.
fn gate() -> Result<(), String> {
    if moqui_core::web::is_production() && moqui_lib::env::get_prop_or_env("component_deploy_enabled") != "true" {
        return Err("Deploying a component is not available on this server".to_string());
    }
    if !moqui_core::user::has_permission("COMPONENT_DEPLOY").unwrap_or(false) {
        return Err("Deploying a component needs the COMPONENT_DEPLOY permission".to_string());
    }
    Ok(())
}

/// A deploy fetches over `https` or `ssh` (also `user@host:path`), never a local path or `file://`. On a production
/// instance the host must not be a loopback, private or link-local address.
fn check_url(url: &str) -> Result<(), String> {
    let url = url.trim();
    let (scheme, rest) = match url.split_once("://") {
        Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest),
        None if url.contains('@') && url.contains(':') && !url.starts_with('/') => ("ssh".to_string(), url),
        None => return Err("the URL must be https:// or ssh://".to_string()),
    };
    if scheme != "https" && scheme != "ssh" {
        return Err(format!("a {scheme} URL is not deployed from; use https:// or ssh://"));
    }
    let authority = rest.split(['/', ':']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("").trim_matches(['[', ']']);
    if host.is_empty() {
        return Err("the URL has no host".to_string());
    }
    if moqui_core::web::is_production() {
        use std::net::ToSocketAddrs;
        let internal = (host, 443)
            .to_socket_addrs()
            .map(|mut addresses| addresses.any(|address| moqui_lib::http::internal_address(address.ip())))
            .unwrap_or(false);
        if internal {
            return Err("the URL's host is a loopback, private or link-local address".to_string());
        }
    }
    Ok(())
}

/// A component name as a directory and a repository id may hold it.
fn check_name(name: &str) -> Result<(), String> {
    let good = !name.is_empty()
        && name.len() <= 100
        && !name.starts_with('.')
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'));
    if good {
        Ok(())
    } else {
        Err(format!("{name:?} is not a component name a deploy takes"))
    }
}

fn provider(factory: &ExecutionContextFactory) -> Result<GitProvider, String> {
    moqui_git::provider(factory).ok_or_else(|| "the git resources are not installed".to_string())
}

fn repository_id(name: &str) -> String {
    format!("component-{name}")
}

/// Fetch the branch, stage its files and answer what deploying would do.
///
/// # Errors
///
/// When the gate refuses, the URL is not deployed from, the fetch fails, or what was fetched is not a component.
pub fn preview(factory: &Arc<ExecutionContextFactory>, request: &DeployRequest) -> Result<DeployPreview, String> {
    gate()?;
    check_url(&request.url)?;
    let branch = if request.branch.trim().is_empty() {
        "main".to_string()
    } else {
        request.branch.trim().to_string()
    };
    let runtime = factory.runtime_dir().to_path_buf();
    let stamp = moqui_core::service::util::now_millis();
    let (name, commit, staged, written) = match request.storage {
        Storage::Database | Storage::Bare => {
            let git = provider(factory)?;
            let scratch = runtime.join("tmp").join(format!("component-deploy-{stamp}"));
            let credentials = FetchCredentials {
                username: request.username.clone(),
                token: request.token.clone(),
            };
            let fetched = git.fetch_branch(&scratch, &request.url, &branch, &credentials);
            let result = fetched.map_err(|err| err.to_string()).and_then(|head| {
                let manifest = GitProvider::read_head_file(&scratch, &branch, "component.xml")
                    .map_err(|err| err.to_string())?
                    .ok_or_else(|| format!("the branch {branch} has no component.xml at its top"))?;
                let name = component_name(&manifest)?;
                check_name(&name)?;
                Ok((head, name))
            });
            let (head, name) = match result {
                Ok(found) => found,
                Err(err) => {
                    let _ = std::fs::remove_dir_all(&scratch);
                    return Err(err);
                }
            };
            refuse_loaded(factory, &name)?;
            let id = repository_id(&name);
            let staged = runtime.join(DEPLOYED_DIR).join(format!(".staging-{name}"));
            let _ = std::fs::remove_dir_all(&staged);
            let kept = keep_repository(factory, &git, &id, request, &branch, &scratch);
            let _ = std::fs::remove_dir_all(&scratch);
            kept?;
            if let Some(parent) = staged.parent() {
                std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
            }
            let written = git
                .write_tree(&id, Some(&head), &staged)
                .map_err(|err| err.to_string())?;
            (name, written.commit.clone(), staged, Some(written))
        }
        Storage::Clone => {
            let directory = clone_directory(&runtime, &request.directory, &component_dirs(factory))?;
            std::fs::create_dir_all(&directory).map_err(|err| err.to_string())?;
            let scratch = directory.join(format!(".staging-clone-{stamp}"));
            git_command(
                None,
                request,
                &[
                    "clone",
                    "--branch",
                    &branch,
                    "--single-branch",
                    "--",
                    request.url.trim(),
                ],
                Some(&scratch),
            )?;
            let read = std::fs::read(scratch.join("component.xml"))
                .map_err(|_| format!("the branch {branch} has no component.xml at its top"))
                .and_then(|manifest| component_name(&manifest));
            let name = match read.and_then(|name| check_name(&name).map(|()| name)) {
                Ok(name) => name,
                Err(err) => {
                    let _ = std::fs::remove_dir_all(&scratch);
                    return Err(err);
                }
            };
            if let Err(err) = refuse_loaded(factory, &name) {
                let _ = std::fs::remove_dir_all(&scratch);
                return Err(err);
            }
            let staged = directory.join(format!(".staging-{name}"));
            let _ = std::fs::remove_dir_all(&staged);
            std::fs::rename(&scratch, &staged).map_err(|err| err.to_string())?;
            let commit = git_command(Some(&staged), request, &["rev-parse", "HEAD"], None)?;
            (name, commit.trim().to_string(), staged, None)
        }
    };
    let location = final_location(&staged, &name);
    if location.exists() {
        return Err(format!(
            "{} exists already; remove it, or update the deployed component",
            location.display()
        ));
    }
    let component = moqui_core::context::read_component(&staged).map_err(|err| err.to_string())?;
    let mut component = ComponentInfo {
        location: staged.clone(),
        ..component
    };
    component.name.clone_from(&name);
    let load = load::preview_staged_shared(factory, component, request.storage == Storage::Database)?;
    let preview = DeployPreview {
        component: name,
        commit,
        storage: request.storage.name().to_string(),
        staged: staged.display().to_string(),
        location: location.display().to_string(),
        load,
        skipped: written
            .as_ref()
            .map(|written| written.skipped.clone())
            .unwrap_or_default(),
        lfs_pointers: written.map(|written| written.lfs_pointers).unwrap_or_default(),
    };
    record_staged(factory, request, &branch, &preview)?;
    Ok(preview)
}

/// Move the staged files of `name` into place and load it ([`load::load`]). A load that fails puts them back.
pub fn deploy(factory: &Arc<ExecutionContextFactory>, name: &str, mode: Mode, data_types: &[String]) -> LoadReport {
    let failed = |problem: String| LoadReport {
        component: name.to_string(),
        status: "failed".to_string(),
        problems: vec![problem],
        ..LoadReport::default()
    };
    if let Err(err) = gate() {
        return failed(err);
    }
    let row = match deployed_row(factory, name) {
        Ok(Some(row)) if text(&row, "status") == "staged" => row,
        Ok(_) => return failed(format!("{name} is not staged; preview the deploy first")),
        Err(err) => return failed(err),
    };
    let location = PathBuf::from(text(&row, "location"));
    let staged = location
        .parent()
        .map(|parent| parent.join(format!(".staging-{name}")))
        .unwrap_or_default();
    if !staged.is_dir() {
        return failed(format!("the staged files of {name} are gone; preview the deploy again"));
    }
    if location.exists() {
        return failed(format!("{} exists already", location.display()));
    }
    if let Err(err) = std::fs::rename(&staged, &location) {
        return failed(format!("could not move {} into place: {err}", staged.display()));
    }
    // a database repository's files reach the other cluster members, which load it from there
    let database = Storage::parse(&text(&row, "storage")) == Some(Storage::Database);
    let shared = Shared {
        data: true,
        cluster_ok: database,
    };
    let report = load::load_shared(factory, name, mode, data_types, shared);
    if report.status == "failed" {
        if let Err(err) = std::fs::rename(&location, &staged) {
            tracing::error!("Could not put the staged files of {name} back after a failed deploy: {err}");
        }
    } else if database && mode == Mode::Now {
        publish(factory, name, "deploy", "", &text(&row, "commitId"));
    }
    report
}

/// Drop what a preview staged for `name`: its files, and the repository it kept when nothing deployed it.
///
/// # Errors
///
/// When the gate refuses or `name` is not staged.
pub fn discard(factory: &Arc<ExecutionContextFactory>, name: &str) -> Result<(), String> {
    gate()?;
    let row = deployed_row(factory, name)?.filter(|row| text(row, "status") == "staged");
    let Some(row) = row else {
        return Err(format!("{name} is not staged"));
    };
    let location = PathBuf::from(text(&row, "location"));
    if let Some(parent) = location.parent() {
        let _ = std::fs::remove_dir_all(parent.join(format!(".staging-{name}")));
    }
    in_context(factory, || -> Result<(), moqui_lib::error::Error> {
        let facade = factory
            .entity()
            .ok_or_else(|| moqui_lib::error::Error::new("no entity facade"))?;
        row.delete(&facade)
    })
    .map_err(|err| err.to_string())
}

/// Fetch the branch of a deployed component again and answer what updating it would do; the new commit is staged.
///
/// # Errors
///
/// When the gate refuses, `name` was not deployed from git, or the fetch fails.
pub fn preview_update(factory: &Arc<ExecutionContextFactory>, name: &str) -> Result<UpdatePreview, String> {
    gate()?;
    let row = deployed_row(factory, name)?
        .filter(|row| text(row, "source") == "git" && text(row, "status") == "deployed")
        .ok_or_else(|| format!("{name} was not deployed from git"))?;
    let storage = Storage::parse(&text(&row, "storage")).unwrap_or_default();
    let location = PathBuf::from(text(&row, "location"));
    let from = text(&row, "commitId");
    let mut preview = UpdatePreview {
        component: name.to_string(),
        from: from.clone(),
        ..UpdatePreview::default()
    };
    if storage != Storage::Database && clustered(factory) {
        preview.refusals.push(
            "this server is in a cluster, whose other members would not have the new files; only a component kept in \
             the database updates on every member"
                .to_string(),
        );
    }
    match storage {
        Storage::Database | Storage::Bare => {
            let git = provider(factory)?;
            let id = text(&row, "gitRepositoryId");
            match storage {
                Storage::Database => git.import(&id, None).map(|_| ()),
                _ => git.fetch(&id).map(|_| ()),
            }
            .map_err(|err| err.to_string())?;
            let to = git
                .branch_head(&id)
                .map_err(|err| err.to_string())?
                .ok_or_else(|| "the branch has no commits".to_string())?;
            preview.to.clone_from(&to);
            if to == from {
                return Ok(preview);
            }
            preview.changed = git.changed_paths(&id, &from, &to).map_err(|err| err.to_string())?;
            let staged = location.with_file_name(format!(".staging-{name}"));
            let _ = std::fs::remove_dir_all(&staged);
            git.write_tree(&id, Some(&to), &staged).map_err(|err| err.to_string())?;
            preview.conf = conf_with(factory, name, &staged);
        }
        Storage::Clone => {
            let request = request_of(&row);
            git_command(Some(&location), &request, &["fetch", "origin"], None)?;
            let branch = text(&row, "branch");
            let to = git_command(
                Some(&location),
                &request,
                &["rev-parse", &format!("origin/{branch}")],
                None,
            )?;
            preview.to = to.trim().to_string();
            if preview.to == from {
                return Ok(preview);
            }
            let dirty = git_command(Some(&location), &request, &["status", "--porcelain"], None)?;
            if !dirty.trim().is_empty() {
                preview
                    .refusals
                    .push("the clone has changes of its own; commit or discard them first".to_string());
            }
            let ahead = git_command(
                Some(&location),
                &request,
                &["merge-base", "--is-ancestor", "HEAD", &preview.to],
                None,
            );
            if ahead.is_err() {
                preview
                    .refusals
                    .push("the clone and the branch have diverged; it is updated only by a fast-forward".to_string());
            }
            let changed = git_command(
                Some(&location),
                &request,
                &["diff", "--name-only", &from, &preview.to],
                None,
            )?;
            preview.changed = changed.lines().map(str::to_string).collect();
        }
    }
    Ok(preview)
}

/// Update a deployed component to the commit [`preview_update`] staged: the files swapped (or the clone
/// fast-forwarded), its entities relinked, the conf applied, the changed files reloaded, and the changed data files of
/// `data_types` loaded (`seed` when empty).
pub fn update(factory: &Arc<ExecutionContextFactory>, name: &str, data_types: &[String]) -> LoadReport {
    let started = std::time::Instant::now();
    let mut report = LoadReport {
        component: name.to_string(),
        status: "failed".to_string(),
        ..LoadReport::default()
    };
    if let Err(err) = gate() {
        report.problems.push(err);
        return report;
    }
    let preview = match preview_update(factory, name) {
        Ok(preview) if preview.from == preview.to => {
            report.status = "deployed".to_string();
            report.problems.push("up to date".to_string());
            return report;
        }
        Ok(preview) if !preview.refusals.is_empty() => {
            report.problems.extend(preview.refusals);
            return report;
        }
        Ok(preview) => preview,
        Err(err) => {
            report.problems.push(err);
            return report;
        }
    };
    let data_types: Vec<String> = if data_types.is_empty() {
        vec!["seed".to_string()]
    } else {
        data_types.to_vec()
    };
    let row = match deployed_row(factory, name) {
        Ok(Some(row)) => row,
        _ => {
            report.problems.push(format!("{name} is not deployed"));
            return report;
        }
    };
    let storage = Storage::parse(&text(&row, "storage")).unwrap_or_default();
    let location = PathBuf::from(text(&row, "location"));
    let applied = reload::with_reload_lock(factory, || {
        update_held(
            factory,
            name,
            storage,
            &row,
            &location,
            &preview,
            Some(&data_types),
            &mut report,
        )
    });
    if let Err(err) = applied {
        report.problems.push(err);
    } else {
        report.status = "deployed".to_string();
        if storage == Storage::Database {
            publish(factory, name, "update", &preview.from, &preview.to);
        }
    }
    report.millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    record_update(factory, name, &preview.to, &report);
    report
}

/// Whether this server has live cluster members besides itself.
fn clustered(factory: &ExecutionContextFactory) -> bool {
    factory
        .entity()
        .is_some_and(|facade| moqui_core::cluster::live_member_count(factory, &facade).unwrap_or(1) > 1)
}

/// Tell the other cluster members that a component kept in the database was deployed or updated, so each writes its
/// files from the shared repository and loads them (plans/Component-Deploy.md, Phase 4).
fn publish(factory: &ExecutionContextFactory, name: &str, action: &str, from: &str, to: &str) {
    let Some(facade) = factory.entity() else {
        return;
    };
    let body = serde_json::json!({"componentName": name, "action": action, "from": from, "to": to});
    let sent = moqui_core::cluster::prepare_message(body)
        .and_then(|message| moqui_core::cluster::publish(&facade, moqui_core::notify::COMPONENT_DEPLOY, &message));
    if let Err(err) = sent {
        tracing::error!("Component {name}: the other cluster members were not told of the {action}: {err}");
    }
}

/// The subscription to other cluster members' component deploys, kept for the life of the factory.
struct Subscribed(#[allow(dead_code)] moqui_core::notify::Subscription);

/// Apply on this server the deploys and updates other cluster members publish: the files written from the shared
/// repository and the component loaded or reloaded, without its data, which is in the shared database. Called once
/// at start.
pub fn install(factory: &Arc<ExecutionContextFactory>) {
    let weak = Arc::downgrade(factory);
    let listener: moqui_core::notify::Listener = Arc::new(move |message| {
        if message.origin_id == moqui_core::notify::origin_id() {
            return;
        }
        let Some(factory) = weak.upgrade() else {
            return;
        };
        let body = message.body.clone();
        // the work reads a repository and reloads artifacts: not on the thread that received the message
        let spawned = std::thread::Builder::new()
            .name("component-deploy".into())
            .spawn(move || {
                let applied = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| apply_remote(&factory, &body)));
                if applied.is_err() {
                    tracing::error!("Applying another cluster member's component deploy failed: {body}");
                }
            });
        if let Err(err) = spawned {
            tracing::error!("Could not start applying another cluster member's component deploy: {err}");
        }
    });
    match moqui_core::notify::subscribe(moqui_core::notify::COMPONENT_DEPLOY, listener) {
        Ok(subscription) => {
            factory.extension(move |_| Subscribed(subscription));
        }
        Err(err) => tracing::error!("Component deploys of other cluster members will not apply here: {err}"),
    }
}

/// A deploy or update another cluster member published.
fn apply_remote(factory: &Arc<ExecutionContextFactory>, body: &serde_json::Value) {
    let field = |name: &str| {
        body.get(name)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let (name, action, from, to) = (field("componentName"), field("action"), field("from"), field("to"));
    let Ok(git) = provider(factory) else {
        return;
    };
    let row = match deployed_row(factory, &name) {
        Ok(Some(row)) if text(&row, "storage") == "database" => row,
        _ => {
            tracing::warn!("Component {name}: another member's {action} names no component kept in the database");
            return;
        }
    };
    let id = text(&row, "gitRepositoryId");
    let location = factory.runtime_dir().join(DEPLOYED_DIR).join(&name);
    let loaded = factory.components().iter().any(|info| info.name == name);
    let outcome = if action == "update" && loaded {
        let changed = match git.changed_paths(&id, &from, &to) {
            Ok(changed) => changed,
            Err(err) => {
                tracing::error!("Component {name}: another member's update could not be read: {err}");
                return;
            }
        };
        let staged = location.with_file_name(format!(".staging-{name}"));
        let _ = std::fs::remove_dir_all(&staged);
        if let Err(err) = git.write_tree(&id, Some(&to), &staged) {
            tracing::error!("Component {name}: another member's update could not be written here: {err}");
            return;
        }
        let preview = UpdatePreview {
            component: name.clone(),
            from,
            to,
            changed,
            ..UpdatePreview::default()
        };
        let mut report = LoadReport::default();
        reload::with_reload_lock(factory, || {
            update_held(
                factory,
                &name,
                Storage::Database,
                &row,
                &location,
                &preview,
                None,
                &mut report,
            )
        })
        .map(|()| report.problems)
    } else if !loaded {
        if !location.exists() {
            if let Err(err) = git.write_tree(&id, Some(&to), &location) {
                tracing::error!("Component {name}: another member's deploy could not be written here: {err}");
                return;
            }
        }
        let shared = Shared {
            data: false,
            cluster_ok: true,
        };
        let report = load::load_shared(factory, &name, Mode::Now, &[], shared);
        if report.status == "failed" {
            Err(report.problems.join("; "))
        } else {
            Ok(report.problems)
        }
    } else {
        return;
    };
    match outcome {
        Ok(problems) => tracing::info!(
            "Component {name}: another cluster member's {action} applied here, {} problems",
            problems.len()
        ),
        Err(err) => tracing::error!("Component {name}: another cluster member's {action} failed here: {err}"),
    }
}

/// At start, write out the files of the components kept in the database whose directory is missing (a new container,
/// or a cluster member started after the deploy), from the shared repository, and move aside those of one removed on
/// another member. Answers how many changed; the host then starts the factory again so the components load (or not)
/// as at every start.
pub fn restore(factory: &Arc<ExecutionContextFactory>) -> usize {
    let (Some(facade), Ok(git)) = (factory.entity(), provider(factory)) else {
        return 0;
    };
    let rows = in_context(factory, || {
        facade
            .find(DEPLOYED)
            .and_then(|find| {
                find.condition(Condition::equals("storage", Value::string("database")))
                    .disable_authz()
                    .list()
            })
            .map(|list| list.into_values())
    });
    let rows = match rows {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!("Could not read the components kept in the database: {err}");
            return 0;
        }
    };
    let mut written = 0;
    for row in rows {
        let name = text(&row, "componentName");
        if check_name(&name).is_err() {
            continue;
        }
        let location = factory.runtime_dir().join(DEPLOYED_DIR).join(&name);
        let status = text(&row, "status");
        // removed on another member: its files here go aside too, and the factory starts again without it
        if status == "removed" && location.is_dir() {
            let aside = location
                .with_file_name(".removed")
                .join(format!("{name}-{}", moqui_core::service::util::now_millis()));
            let moved = aside
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::rename(&location, &aside));
            match moved {
                Ok(()) => {
                    tracing::warn!("Component {name}: removed, its files moved to {}", aside.display());
                    written += 1;
                }
                Err(err) => tracing::error!("Component {name}: removed, but its files could not be moved aside: {err}"),
            }
            continue;
        }
        if !matches!(status.as_str(), "deployed" | "pending-restart") || location.exists() {
            continue;
        }
        let commit = text(&row, "commitId");
        let revision = Some(commit.as_str()).filter(|commit| !commit.is_empty());
        match git.write_tree(&text(&row, "gitRepositoryId"), revision, &location) {
            Ok(tree) => {
                tracing::warn!(
                    "Component {name}: its files written from the database at {}",
                    tree.commit
                );
                written += 1;
            }
            Err(err) => tracing::error!("Component {name}: its files could not be written from the database: {err}"),
        }
    }
    written
}

/// The update itself, holding the reload lock. `data_types` `None` loads no data: a cluster member applying another
/// member's update, whose data is in the shared database.
#[allow(clippy::too_many_arguments)]
fn update_held(
    factory: &Arc<ExecutionContextFactory>,
    name: &str,
    storage: Storage,
    row: &moqui_core::entity::EntityValue,
    location: &Path,
    preview: &UpdatePreview,
    data_types: Option<&[String]>,
    report: &mut LoadReport,
) -> Result<(), String> {
    // 1. the files: the new commit's in place of the deployed one's (kept in .previous/), or the clone fast-forwarded
    let previous = location
        .with_file_name(".previous")
        .join(format!("{name}-{}", short(&preview.from)));
    match storage {
        Storage::Database | Storage::Bare => {
            let staged = location.with_file_name(format!(".staging-{name}"));
            if let Some(parent) = previous.parent() {
                std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
            }
            let _ = std::fs::remove_dir_all(&previous);
            std::fs::rename(location, &previous).map_err(|err| err.to_string())?;
            if let Err(err) = std::fs::rename(&staged, location) {
                let _ = std::fs::rename(&previous, location);
                return Err(format!("could not move the new files into place: {err}"));
            }
        }
        Storage::Clone => {
            git_command(
                Some(location),
                &request_of(row),
                &["merge", "--ff-only", &preview.to],
                None,
            )?;
        }
    }
    let changed: Vec<PathBuf> = preview.changed.iter().map(|path| location.join(path)).collect();
    // 2. its entities, when an entity file changed
    let entity_changed = preview
        .changed
        .iter()
        .any(|path| path.starts_with("entity/") && path.ends_with(".xml") && !path.ends_with(".eecas.xml"));
    if entity_changed {
        if let Some(facade) = factory.entity() {
            let dirs = moqui_core::context::component_entity_dirs(factory);
            let files: Vec<PathBuf> = changed.iter().filter(|path| path.is_file()).cloned().collect();
            match facade.relink_components(Some(dirs), &files) {
                Ok(relinked) => report.entities = Some(relinked),
                Err(err) => {
                    if matches!(storage, Storage::Database | Storage::Bare) {
                        let _ = std::fs::rename(location, location.with_file_name(format!(".staging-{name}")));
                        let _ = std::fs::rename(&previous, location);
                    }
                    return Err(format!("its entities: {err}; the deployed files are back in place"));
                }
            }
        }
    }
    // 3. the conf, when its MoquiConf.xml changed
    if preview.changed.iter().any(|path| path == "MoquiConf.xml") {
        match conf_reload::apply(factory) {
            Ok(applied) => {
                report.problems.extend(applied.problems.iter().cloned());
                report.conf = Some(applied);
            }
            Err(err) => report.problems.push(format!("the conf: {err}")),
        }
    }
    // 4. the changed files through the reload pipeline (entity definitions were relinked; data loads below)
    let paths: Vec<PathBuf> = changed
        .iter()
        .filter(|path| {
            let relative = path.strip_prefix(location).map(Path::to_path_buf).unwrap_or_default();
            let text = relative.to_string_lossy().replace('\\', "/");
            !(text == "MoquiConf.xml"
                || text == "component.xml"
                || text.starts_with("data/")
                || (text.starts_with("entity/") && !text.ends_with(".eecas.xml")))
        })
        .cloned()
        .collect();
    let reloaded = reload::reload_paths_held(factory, &paths);
    for file in &reloaded.files {
        for problem in &file.problems {
            report.problems.push(format!("{}: {}", problem.path, problem.message));
        }
    }
    report.reload = Some(reloaded);
    moqui_core::extension::artifacts_changed(factory);
    // 5. the changed data files of the types asked for
    let (Some(facade), Some(data_types)) = (factory.entity(), data_types) else {
        return Ok(());
    };
    let options = moqui_core::entity::load_feed::LoadOptions {
        data_types: data_types.to_vec(),
        ..moqui_core::entity::load_feed::LoadOptions::default()
    };
    let data: Vec<String> = preview
        .changed
        .iter()
        .filter(|path| path.starts_with("data/") && location.join(path).is_file())
        .map(|path| format!("component://{name}/{path}"))
        .collect();
    in_context(factory, || {
        for location in data {
            match moqui_core::entity::load_feed::load_entity_data(&facade, &location, &options) {
                Ok(result) => {
                    report.records += result.records;
                    report.data.push(load::DataFile {
                        location,
                        records: result.records,
                        error: None,
                    });
                }
                Err(err) => {
                    let text = moqui_core::entity::load_feed::error_chain(&err);
                    report.problems.push(format!("{location}: {text}"));
                    report.data.push(load::DataFile {
                        location,
                        records: 0,
                        error: Some(text),
                    });
                }
            }
        }
    });
    Ok(())
}

/// The conf the files make with the component `name` read from `dir` in place of its directory, compared with the
/// running conf.
fn conf_with(factory: &ExecutionContextFactory, name: &str, dir: &Path) -> Option<conf_reload::ConfReport> {
    let components: Vec<ComponentInfo> = factory
        .components()
        .iter()
        .map(|info| {
            if info.name == name {
                ComponentInfo {
                    location: dir.to_path_buf(),
                    ..info.clone()
                }
            } else {
                info.clone()
            }
        })
        .collect();
    let merged = conf_reload::merged_from_files(factory, &components).ok()?;
    conf_reload::preview_conf(factory, &merged).ok()
}

fn refuse_loaded(factory: &ExecutionContextFactory, name: &str) -> Result<(), String> {
    if factory.components().iter().any(|info| info.name == name) {
        return Err(format!(
            "{name} is loaded already; to change it, update it (a deployed one) or edit its directory"
        ));
    }
    Ok(())
}

/// Keep the fetched branch in the component's repository: imported into a `db` one, or the fetched bare repository
/// moved into place as a `remote` one.
fn keep_repository(
    factory: &Arc<ExecutionContextFactory>,
    git: &GitProvider,
    id: &str,
    request: &DeployRequest,
    branch: &str,
    scratch: &Path,
) -> Result<(), String> {
    let facade = factory.entity().ok_or_else(|| "no entity facade".to_string())?;
    let kind = match request.storage {
        Storage::Bare => "remote",
        _ => "db",
    };
    let local_path = format!("git/{id}");
    in_context(factory, || -> Result<(), moqui_lib::error::Error> {
        let existing = facade
            .find(GIT_REPOSITORY)?
            .condition(Condition::equals("repositoryId", Value::string(id)))
            .disable_authz()
            .one()?;
        let mut row = match existing {
            Some(row) => row,
            None => {
                let mut row = facade.make_value(GIT_REPOSITORY)?;
                row.set("repositoryId", Value::string(id));
                row
            }
        };
        row.set("repositoryKind", Value::string(kind));
        row.set("remoteUrl", Value::string(request.url.trim()));
        row.set("branch", Value::string(branch));
        row.set("localPath", Value::string(local_path.as_str()));
        row.set("username", request.username.clone().map_or(Value::Null, Value::string));
        row.set("accessToken", request.token.clone().map_or(Value::Null, Value::string));
        row.set(
            "description",
            Value::string(format!(
                "Deployed component from {}",
                moqui_core::outbound::redacted(request.url.trim())
            )),
        );
        row.create_or_update(&facade)
    })
    .map_err(|err| err.to_string())?;
    git.refresh(id);
    match request.storage {
        Storage::Bare => {
            let path = factory.runtime_dir().join(&local_path);
            let _ = std::fs::remove_dir_all(&path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
            }
            std::fs::rename(scratch, &path).map_err(|err| err.to_string())?;
            git.refresh(id);
            Ok(())
        }
        _ => git
            .import(id, Some(&scratch.display().to_string()))
            .map(|_| ())
            .map_err(|err| err.to_string()),
    }
}

/// The component directory a clone goes in: `component`, or the one named, which must be a `component-dir` of the
/// conf's `component-list` (`dirs`), so the boot finds the clone there.
fn clone_directory(runtime: &Path, named: &str, dirs: &[String]) -> Result<PathBuf, String> {
    let named = named.trim().trim_end_matches('/');
    let named = if named.is_empty() { "component" } else { named };
    if !dirs.iter().any(|dir| dir.trim_end_matches('/') == named) {
        return Err(format!(
            "{named} is not a component directory of the conf's component-list ({})",
            dirs.join(", ")
        ));
    }
    Ok(runtime.join(named))
}

/// The `component-dir` locations of the conf's `component-list`.
fn component_dirs(factory: &ExecutionContextFactory) -> Vec<String> {
    let Ok(conf) = moqui_lib::node::MNode::parse_shared("moqui-conf", &factory.conf_text()) else {
        return Vec::new();
    };
    conf.first_named("component-list")
        .map(|list| {
            list.children_named("component-dir")
                .iter()
                .filter_map(|dir| dir.attribute("location"))
                .collect()
        })
        .unwrap_or_default()
}

/// Where the files staged at `staged` (`.staging-<name>`) go: beside it, as `<name>`.
fn final_location(staged: &Path, name: &str) -> PathBuf {
    staged.with_file_name(name)
}

/// Run the `git` command with `args` (in `dir` when given, with `target` last when given), the request's token sent as
/// an HTTP header, never kept in the clone's conf, and no prompt. Answers its output.
fn git_command(
    dir: Option<&Path>,
    request: &DeployRequest,
    args: &[&str],
    target: Option<&Path>,
) -> Result<String, String> {
    let mut command = Command::new("git");
    command.env("GIT_TERMINAL_PROMPT", "0");
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    if let Some(token) = request.token.as_deref().filter(|token| !token.is_empty()) {
        use base64::Engine;
        let user = request
            .username
            .as_deref()
            .filter(|user| !user.is_empty())
            .unwrap_or("git");
        let basic = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{token}"));
        command.args(["-c", &format!("http.extraHeader=Authorization: Basic {basic}")]);
    }
    command.args(args);
    if let Some(target) = target {
        command.arg(target);
    }
    let output = command
        .output()
        .map_err(|err| format!("the clone storage needs the git command: {err}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!(
            "git {}: {}",
            args.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// The deploy request a row was made with: its URL, branch and credentials.
fn request_of(row: &moqui_core::entity::EntityValue) -> DeployRequest {
    let optional = |field: &str| Some(text(row, field)).filter(|value| !value.is_empty());
    DeployRequest {
        url: text(row, "remoteUrl"),
        branch: text(row, "branch"),
        storage: Storage::parse(&text(row, "storage")).unwrap_or_default(),
        directory: String::new(),
        username: optional("username"),
        token: optional("accessToken"),
    }
}

/// Keep a preview's state: the component's row, `staged`, with where its files go, where they came from, and the
/// preview, which System › Components shows until the deploy.
fn record_staged(
    factory: &Arc<ExecutionContextFactory>,
    request: &DeployRequest,
    branch: &str,
    preview: &DeployPreview,
) -> Result<(), String> {
    let (name, commit) = (preview.component.as_str(), preview.commit.as_str());
    let location = Path::new(&preview.location);
    let report = serde_json::to_string(preview).unwrap_or_default();
    let facade = factory.entity().ok_or_else(|| "no entity facade".to_string())?;
    let user = moqui_core::user::session::current_user_id()
        .ok()
        .flatten()
        .unwrap_or_default();
    in_context(factory, || -> Result<(), moqui_lib::error::Error> {
        let existing = facade
            .find(DEPLOYED)?
            .condition(Condition::equals("componentName", Value::string(name)))
            .disable_authz()
            .one()?;
        if existing
            .as_ref()
            .is_some_and(|row| !matches!(text(row, "status").as_str(), "staged" | "removed" | "failed"))
        {
            return Err(moqui_lib::error::Error::new(format!(
                "{name} is deployed already; update it instead"
            )));
        }
        let mut row = match existing {
            Some(row) => row,
            None => {
                let mut row = facade.make_value(DEPLOYED)?;
                row.set("componentName", Value::string(name));
                row
            }
        };
        row.set("source", Value::string("git"));
        row.set("storage", Value::string(request.storage.name()));
        row.set("location", Value::string(location.display().to_string()));
        row.set(
            "gitRepositoryId",
            match request.storage {
                Storage::Clone => Value::Null,
                _ => Value::string(repository_id(name)),
            },
        );
        row.set("remoteUrl", Value::string(request.url.trim()));
        row.set("branch", Value::string(branch));
        row.set("commitId", Value::string(commit));
        row.set("username", request.username.clone().map_or(Value::Null, Value::string));
        row.set("accessToken", request.token.clone().map_or(Value::Null, Value::string));
        row.set("status", Value::string("staged"));
        row.set("lastReport", Value::string(report.as_str()));
        row.set("changedDate", moqui_core::service::util::now_stamp());
        row.set("changedByUserId", Value::string(user.as_str()));
        row.create_or_update(&facade)
    })
    .map_err(|err| err.to_string())
}

/// Keep what an update did: the new commit, and a history row.
fn record_update(factory: &Arc<ExecutionContextFactory>, name: &str, commit: &str, report: &LoadReport) {
    let Some(facade) = factory.entity() else {
        return;
    };
    let user = moqui_core::user::session::current_user_id()
        .ok()
        .flatten()
        .unwrap_or_default();
    let text_report = serde_json::to_string(report).unwrap_or_default();
    let now = moqui_core::service::util::now_stamp();
    let written = in_context(factory, || -> Result<(), moqui_lib::error::Error> {
        if let Some(mut row) = facade
            .find(DEPLOYED)?
            .condition(Condition::equals("componentName", Value::string(name)))
            .disable_authz()
            .one()?
        {
            if report.status == "deployed" {
                row.set("commitId", Value::string(commit));
            }
            row.set("lastReport", Value::string(text_report.as_str()));
            row.set("changedDate", now.clone());
            row.set("changedByUserId", Value::string(user.as_str()));
            row.update(&facade)?;
        }
        let mut history = facade.make_value("moqui.server.DeployedComponentHistory")?;
        history.set("componentName", Value::string(name));
        history.set("historyDate", now.clone());
        history.set("action", Value::string("update"));
        history.set("status", Value::string(report.status.as_str()));
        history.set("commitId", Value::string(commit));
        history.set("userId", Value::string(user.as_str()));
        history.set("report", Value::string(text_report.as_str()));
        history.create(&facade)
    });
    if let Err(err) = written {
        tracing::error!("Could not record the update of component {name}: {err}");
    }
    tracing::warn!(
        target: moqui_core::live_settings::AUDIT,
        "Component {name} updated to {commit}: {} problems",
        report.problems.len()
    );
}

fn deployed_row(
    factory: &Arc<ExecutionContextFactory>,
    name: &str,
) -> Result<Option<moqui_core::entity::EntityValue>, String> {
    let facade = factory.entity().ok_or_else(|| "no entity facade".to_string())?;
    in_context(factory, || {
        facade.find(DEPLOYED).and_then(|find| {
            find.condition(Condition::equals("componentName", Value::string(name)))
                .disable_authz()
                .one()
        })
    })
    .map_err(|err| err.to_string())
}

fn in_context<R>(factory: &Arc<ExecutionContextFactory>, body: impl FnOnce() -> R) -> R {
    let entered_here = ExecutionContext::with_current(|_| ())
        .is_none()
        .then(|| factory.new_context());
    let _entered = entered_here.as_ref().map(ExecutionContext::enter);
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    body()
}

fn text(row: &moqui_core::entity::EntityValue, field: &str) -> String {
    row.get(field)
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default()
}

fn short(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}

/// The `name` of a `component.xml`.
fn component_name(manifest: &[u8]) -> Result<String, String> {
    let text = String::from_utf8_lossy(manifest);
    let root = moqui_lib::node::MNode::parse_text("component.xml", &text).map_err(|err| err.to_string())?;
    root.attribute("name")
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "its component.xml has no name".to_string())
}

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;
