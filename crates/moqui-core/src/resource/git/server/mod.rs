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

//! The git server: clone, fetch and push of `GitRepository` repositories over smart HTTP and SSH, with gitoxide's
//! object and pack code under a protocol layer of our own (gix has no server side). The host (moqui-host) owns the
//! sockets and calls [`serve_http`] or [`serve_ssh`] on a job of [`pool`], in a fresh execution context, with
//! blocking streams. Each request logs its user in (HTTP Basic with the password or an access token, `Bearer`, or an
//! API key; the SSH server already checked the key or password), and every repository is checked against
//! `AT_GIT_REPOSITORY`: `view` to fetch, `create`/`update` to push new refs and fast-forwards, `delete` to force a
//! push or delete a ref. With that type's authorization off in the conf the server refuses everything. Guide:
//! `docs/Git-Hosting.md`.

pub mod lfs;
pub(crate) mod pack_in;
pub(crate) mod pack_out;
pub(crate) mod pktline;
pub(crate) mod plan;
pub(crate) mod receive;
#[cfg(test)]
mod hosting_tests;
#[cfg(test)]
mod tests;
pub(crate) mod upload;

use std::collections::HashSet;
use std::io::{BufReader, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use gix::bstr::ByteSlice;
use gix::objs::Kind;
use gix::ObjectId;

use super::store::{GitStore, RefEdit, RefOutcome};
use super::{GitProvider, Repo, RepositoryKind, GIT_ARTIFACT_TYPE};
use crate::context::{Error, ExecutionContext, ExecutionContextFactory};
use crate::resource::git::{events, revwalk};
use crate::service::RequestPool;
use moqui_lib::object::Value;

/// Git's own error for the protocol layer.
pub(crate) fn io_error(err: std::io::Error) -> Error {
    Error::new(format!("git connection: {err}"))
}

/// The `agent` capability: no spaces.
pub(crate) fn agent() -> String {
    format!("moqui/{}", env!("CARGO_PKG_VERSION"))
}

/// A ref name without `refs/heads/` or `refs/tags/`.
pub(crate) fn short_ref(name: &str) -> &str {
    super::browse::short_name(name)
}

/// One ref of an advertisement.
#[derive(Clone, Debug)]
pub(crate) struct AdvertisedRef {
    pub name: String,
    pub id: ObjectId,
    /// What an annotated tag peels to.
    pub peeled: Option<ObjectId>,
}

/// The refs a conversation starts from, read once: what is fetched is what was advertised.
#[derive(Clone, Debug)]
pub(crate) struct Advertised {
    /// The head of the default branch, when it has commits.
    pub head: Option<ObjectId>,
    /// The default branch's full ref name, what `HEAD` points to.
    pub branch_ref: String,
    pub refs: Vec<AdvertisedRef>,
}

fn peel_tag(store: &dyn GitStore, id: &ObjectId) -> Result<Option<ObjectId>, Error> {
    let mut current = *id;
    let mut peeled = None;
    for _ in 0..16 {
        match store.find(&current)? {
            Some((Kind::Tag, data)) => {
                current = gix::objs::TagRef::from_bytes(&data, super::ops::HASH)
                    .map_err(|err| Error::new(format!("git tag {current}: {err}")))?
                    .target();
                peeled = Some(current);
            }
            _ => break,
        }
    }
    Ok(peeled)
}

pub(crate) fn advertised(store: &dyn GitStore, branch: &str) -> Result<Advertised, Error> {
    let branch_ref = super::ops::branch_ref(branch);
    let mut refs = Vec::new();
    for (name, id) in store.list_refs("refs/")? {
        refs.push(AdvertisedRef {
            peeled: peel_tag(store, &id)?,
            name,
            id,
        });
    }
    Ok(Advertised {
        head: refs
            .iter()
            .find(|reference| reference.name == branch_ref)
            .map(|reference| reference.id),
        branch_ref,
        refs,
    })
}

/// The two git services.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitService {
    UploadPack,
    ReceivePack,
}

impl GitService {
    /// `git-upload-pack` or `git-receive-pack`.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "git-upload-pack" => Some(Self::UploadPack),
            "git-receive-pack" => Some(Self::ReceivePack),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::UploadPack => "git-upload-pack",
            Self::ReceivePack => "git-receive-pack",
        }
    }
}

/// Whether a `Git-Protocol` header or `GIT_PROTOCOL` variable asks for protocol v2.
fn wants_v2(git_protocol: Option<&str>) -> bool {
    git_protocol.is_some_and(|text| text.split(':').any(|part| part.trim() == "version=2"))
}

/// The repository id of a path: a leading `/`, a `git/` before it and a `.git` after it are dropped.
pub fn repository_of(path: &str) -> Option<String> {
    let path = path.trim().trim_matches('/');
    let path = path.strip_prefix("git/").unwrap_or(path);
    let id = path.strip_suffix(".git").unwrap_or(path);
    (!id.is_empty() && !id.contains('/') && !id.starts_with('.')).then(|| id.to_string())
}

/// An SSH `exec` command for git: `git-upload-pack '<path>'` (or `git upload-pack`), the same for `git-receive-pack`,
/// with git's single quoting (`'\''` for a quote).
pub fn parse_ssh_command(command: &str) -> Option<(GitService, String)> {
    let command = command.trim();
    let (service, rest) = [
        ("git-upload-pack", GitService::UploadPack),
        ("git-receive-pack", GitService::ReceivePack),
        ("git upload-pack", GitService::UploadPack),
        ("git receive-pack", GitService::ReceivePack),
    ]
    .into_iter()
    .find_map(|(name, service)| command.strip_prefix(name).map(|rest| (service, rest)))?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim();
    let path = match rest.strip_prefix('\'') {
        Some(quoted) => {
            let quoted = quoted.strip_suffix('\'')?;
            quoted.replace("'\\''", "'")
        }
        None => rest.to_string(),
    };
    repository_of(&path).map(|id| (service, id))
}

/// One HTTP request of the smart protocol.
#[derive(Clone, Debug)]
pub struct GitHttpRequest {
    pub repository: String,
    pub service: GitService,
    /// `GET .../info/refs?service=...`: the advertisement; otherwise the `POST` of a service.
    pub advertise: bool,
    /// The `Git-Protocol` header.
    pub git_protocol: Option<String>,
    /// The body is `Content-Encoding: gzip`, as git sends an upload-pack request over 1 KB.
    pub gzip: bool,
    pub authorization: Option<String>,
    /// The `api_key` or `login_key` header.
    pub api_key: Option<String>,
    /// Credentials may be taken: the request came over HTTPS (or a trusted proxy's), from loopback, or
    /// `git-server` `allow-http` is set.
    pub secure: bool,
    pub client_ip: String,
}

/// Where an HTTP response goes: [`HttpResponder::head`] once, then the body.
pub trait HttpResponder {
    fn head(&mut self, status: u16, headers: &[(&str, String)]) -> std::io::Result<()>;
    fn body(&mut self) -> &mut dyn Write;
}

fn property(name: &str) -> String {
    moqui_lib::env::get_prop_or_env(name).trim().to_string()
}

/// An attribute of the conf's `git-server` as a positive number; None when it is empty, 0 or not a number.
fn positive_setting(name: &str) -> Option<usize> {
    crate::resource::git::setting(name)
        .parse()
        .ok()
        .filter(|value| *value > 0)
}

/// The git pool's workers: clones and pushes run long, so they get their own threads, apart from the request pool
/// the screens use (`git-server` `max-concurrent`, 4, and `max-queued`, 16).
pub struct GitPool {
    pool: RequestPool,
}

impl GitPool {
    /// Queue `job` in a fresh execution context. A full pool is an `Unavailable` error.
    ///
    /// # Errors
    ///
    /// Returns an error when the pool is full or stopped.
    pub fn try_submit<T: Send + 'static>(
        &self,
        factory: Arc<ExecutionContextFactory>,
        job: impl FnOnce() -> Result<T, Error> + Send + 'static,
    ) -> Result<std::sync::mpsc::Receiver<Result<T, Error>>, Error> {
        self.pool.try_submit(factory, job)
    }
}

/// The factory's git pool, started on first use.
pub fn pool(factory: &ExecutionContextFactory) -> Arc<GitPool> {
    factory.extension(|factory| {
        let pool = RequestPool::idle();
        let workers = Some(factory.conf_number("git-server", "max-concurrent", 4))
            .filter(|n| *n > 0)
            .unwrap_or(4);
        let queue = Some(factory.conf_number("git-server", "max-queued", 16))
            .filter(|n| *n > 0)
            .unwrap_or(16);
        if let Err(err) = pool.start(workers, queue) {
            tracing::error!("the git pool did not start: {err}");
        }
        GitPool { pool }
    })
}

fn pack_threads() -> Option<usize> {
    positive_setting("pack-threads")
}

fn max_object_size() -> Option<usize> {
    Some(positive_setting("max-object-size").unwrap_or(512 * 1024 * 1024))
}

/// Copy the objects `head` reaches and the commits `haves` do not from `from` into `to`, as a push does: a pack of
/// them (deltas from `from`'s packs or rows kept) written to `scratch` (a directory, removed after), indexed into a
/// quarantine, checked to be whole, and accepted. How many objects.
pub(crate) fn copy_objects(
    from: &dyn GitStore,
    to: &dyn GitStore,
    head: ObjectId,
    haves: &[ObjectId],
    scratch: &std::path::Path,
) -> Result<usize, Error> {
    let planned = plan::plan(from, &[head], haves, &[], &HashSet::new(), &[])?;
    if planned.objects.is_empty() {
        return Ok(0);
    }
    std::fs::create_dir_all(scratch)
        .map_err(|err| Error::new(format!("cannot create {}: {err}", scratch.display())))?;
    let result = (|| {
        let pack_path = scratch.join("copy.pack");
        let mut file = std::io::BufWriter::new(
            std::fs::File::create(&pack_path)
                .map_err(|err| Error::new(format!("cannot create {}: {err}", pack_path.display())))?,
        );
        let deltas = pack_out::Deltas { ofs: true, thin: None };
        pack_out::write_pack(from, &planned.objects, deltas, pack_threads(), &mut file)?;
        file.flush()
            .map_err(|err| Error::new(format!("writing {}: {err}", pack_path.display())))?;
        drop(file);
        let mut input = std::io::BufReader::new(
            std::fs::File::open(&pack_path)
                .map_err(|err| Error::new(format!("cannot open {}: {err}", pack_path.display())))?,
        );
        let quarantine = match to.disk_path() {
            // on the repository's own filesystem, so accepting is a rename
            Some(path) => path.join("objects").join(format!(
                "incoming-{}-{}",
                std::process::id(),
                QUARANTINES.fetch_add(1, Ordering::Relaxed)
            )),
            None => scratch.join("quarantine"),
        };
        let mut received = pack_in::receive(to, &mut input, &quarantine, max_object_size(), pack_threads())?;
        received.check_closure(&HashSet::new())?;
        received.accept()
    })();
    if let Err(err) = std::fs::remove_dir_all(scratch) {
        tracing::warn!("cannot remove {}: {err}", scratch.display());
    }
    result.map(|_| planned.objects.len())
}

/// The base URL git clients reach the web server at: the conf's `git-server` `public-url` (`git_public_url`), else
/// from the webapp host and port properties.
pub(crate) fn public_base_url() -> String {
    let mut configured = crate::resource::git::setting("public-url");
    if configured.is_empty() {
        configured = property("git_public_url");
    }
    if !configured.is_empty() {
        return configured.trim_end_matches('/').to_string();
    }
    let host = Some(property("webapp_http_host"))
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| "localhost".to_string());
    let (scheme, port, default) = if property("webapp_https_enabled") == "true" {
        ("https", property("webapp_https_port"), "443")
    } else {
        ("http", property("webapp_http_port"), "80")
    };
    if port.is_empty() || port == default {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}:{port}")
    }
}

/// Why a request is refused, before any protocol is spoken.
#[derive(Debug)]
enum Refusal {
    /// No credentials, or wrong ones: ask for them.
    Unauthenticated(String),
    /// Logged in, not allowed: 403.
    Forbidden(String),
    /// No such repository, or not one the user may see: 404.
    NotFound(String),
    /// The server cannot serve git now: 503.
    Unavailable(String),
}

impl Refusal {
    fn message(&self) -> &str {
        match self {
            Self::Unauthenticated(text) | Self::Forbidden(text) | Self::NotFound(text) | Self::Unavailable(text) => {
                text
            }
        }
    }
}

fn factory() -> Result<Arc<ExecutionContextFactory>, Refusal> {
    ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))
        .ok_or_else(|| Refusal::Unavailable("no execution context".into()))
}

/// The git provider, when the server may run: `AT_GIT_REPOSITORY` authorization must be on, or every check would
/// pass.
fn provider() -> Result<GitProvider, Refusal> {
    let factory = factory()?;
    if !factory.artifact_flag(GIT_ARTIFACT_TYPE, "authz-enabled") {
        tracing::error!(
            "the git server is off: <artifact-execution type=\"{GIT_ARTIFACT_TYPE}\" authz-enabled=\"true\"/> is not in the conf"
        );
        return Err(Refusal::Unavailable(
            "the git server is off: AT_GIT_REPOSITORY authorization is not enabled".into(),
        ));
    }
    factory
        .resource()
        .git()
        .cloned()
        .ok_or_else(|| Refusal::Unavailable("git repositories are not installed".into()))
}

fn current_user() -> Option<String> {
    crate::user::current_user_id()
        .ok()
        .flatten()
        .filter(|id| !id.is_empty())
}

/// Log the user of an HTTP request in. False when it carries no credentials.
fn authenticate(request: &GitHttpRequest) -> Result<bool, Refusal> {
    authenticate_with(request.authorization.as_deref(), request.api_key.as_deref())
}

/// Log a user in by an `Authorization` header (Basic, with a password or an access token, or Bearer) or an `api_key`.
/// False when there are no credentials.
fn authenticate_with(authorization: Option<&str>, api_key: Option<&str>) -> Result<bool, Refusal> {
    let refused = |err: Error| Refusal::Unauthenticated(err.message().to_string());
    if let Some(header) = authorization.map(str::trim) {
        if let Some(token) = header
            .strip_prefix("Bearer ")
            .or_else(|| header.strip_prefix("bearer "))
        {
            crate::user::login_user_key(token.trim()).map_err(refused)?;
            return Ok(true);
        }
        if let Some((username, password)) = crate::web::basic_credentials(header) {
            // an app password, as the DAV servers take it
            if crate::user::looks_like_app_password(&password) {
                if crate::user::login_app_password(&username, &password).is_ok() {
                    return Ok(true);
                }
                // not one, so an account password of that form: only the password login below counts as a failure
                crate::web::take_auth_failure();
            }
            // an access token (a UserLoginKey) as the password, as git hosts take personal access tokens
            if password.len() == 40 {
                if crate::user::login_user_key(&password).is_ok() {
                    let user = crate::user::current_username().ok().flatten().unwrap_or_default();
                    if username.is_empty() || user.eq_ignore_ascii_case(&username) {
                        return Ok(true);
                    }
                    let _ = crate::user::logout();
                    return Err(Refusal::Unauthenticated("the access token is not this user's".into()));
                }
                // not a token, so a password of 40 characters: only the password login below counts as a failure
                crate::web::take_auth_failure();
            }
            crate::user::login_user(&username, &password).map_err(refused)?;
            if current_user().is_none() {
                return Err(Refusal::Unauthenticated(
                    "this account needs a second factor: use an access token as the password".into(),
                ));
            }
            return Ok(true);
        }
    }
    if let Some(key) = api_key.filter(|key| !key.is_empty()) {
        crate::user::login_user_key(key).map_err(refused)?;
        return Ok(true);
    }
    Ok(false)
}

/// Whether the user may make any change to the repository.
fn may_push(id: &str) -> bool {
    ["create", "update", "delete"]
        .iter()
        .any(|action| permitted(id, action))
}

/// Whether the logged-in user may do `action` (view, create, update, delete) on repository `id`: by `AT_GIT_REPOSITORY`
/// authorization, or as a member of the drive the repository holds (`ResourceDrive.repositoryId`), by the drive's own
/// rule (`FileServices.get#DriveAccess`): view to fetch, edit to push new refs and fast-forwards on a drive that is not
/// read only, manage to force a push or delete a ref. Asked once per request and action.
pub(crate) fn permitted(id: &str, action: &str) -> bool {
    if crate::user::authorize_own(GIT_ARTIFACT_TYPE, id, action).is_ok() {
        return true;
    }
    let key = format!("git.driveAccess.{id}");
    let cached = ExecutionContext::with_current(|ec| ec.shared_map().get(&key).cloned()).flatten();
    let (level, can_write) = match cached.and_then(|value| {
        moqui_lib::object::to_plain_string(&value)
            .split_once(':')
            .map(|(a, b)| (a.to_string(), b.to_string()))
    }) {
        Some((level, write)) => (level.parse::<i64>().unwrap_or(0), write == "true"),
        None => {
            let found = drive_access(id);
            ExecutionContext::with_current(|ec| {
                ec.shared_map()
                    .insert(key.clone(), Value::string(format!("{}:{}", found.0, found.1)))
            });
            found
        }
    };
    match action {
        "view" => level >= 1,
        "create" | "update" => level >= 2 && can_write,
        "delete" => level >= 3 && can_write,
        _ => false,
    }
}

/// The user's level on the drive repository `id` holds and whether the drive may be written; (0, false) for none.
fn drive_access(id: &str) -> (i64, bool) {
    let Some(user) = crate::user::current_user_id()
        .ok()
        .flatten()
        .filter(|user| !user.is_empty())
    else {
        return (0, false);
    };
    let drive = ExecutionContext::with_current(|ec| {
        let _authz = ec.disable_authz();
        ec.factory()
            .entity()?
            .find("moqui.resource.ResourceDrive")
            .ok()?
            .eq("repositoryId", Value::string(id))
            .one()
            .ok()
            .flatten()
    })
    .flatten();
    let Some(drive_id) = drive.and_then(|row| row.get("driveId").map(moqui_lib::object::to_plain_string)) else {
        return (0, false);
    };
    let parameters: std::collections::BTreeMap<String, Value> = [("driveId".to_string(), Value::string(&drive_id))]
        .into_iter()
        .collect();
    // the service reads the drive's members for the current user; its own rows are moqui.* entities the user may not read
    let options = crate::service::call::CallOptions {
        disable_authz: true,
        ..Default::default()
    };
    match crate::service::call::sync_checked("org.moqui.impl.FileServices.get#DriveAccess", &parameters, &options) {
        Ok(out) => {
            let level = out
                .get("level")
                .map(moqui_lib::object::to_plain_string)
                .and_then(|text| text.parse().ok())
                .unwrap_or(0);
            let can_write = out
                .get("canWrite")
                .is_some_and(|value| moqui_lib::object::to_plain_string(value) == "true");
            tracing::debug!(
                repository = id,
                drive = drive_id,
                user,
                level,
                "git access through the drive"
            );
            (level, can_write)
        }
        Err(err) => {
            ExecutionContext::with_current(|ec| ec.message().clear_errors());
            tracing::debug!(repository = id, drive = drive_id, "no drive access: {err}");
            (0, false)
        }
    }
}

/// The repository a logged-in user may see, and may push to when `service` is receive-pack.
fn open(provider: &GitProvider, id: &str, service: GitService) -> Result<Arc<Repo>, Refusal> {
    let not_found = || Refusal::NotFound(format!("repository {id} not found"));
    if !permitted(id, "view") {
        return Err(not_found());
    }
    // the user may see it: say why it cannot be opened, unless there is no such repository
    let repo = provider.repo(id).map_err(|err| {
        if err.message().contains("is not defined") {
            not_found()
        } else {
            tracing::error!(repository = id, "git repository not opened: {err}");
            Refusal::Unavailable(format!("repository {id} cannot be opened: {}", err.message()))
        }
    })?;
    if service == GitService::ReceivePack {
        if repo.definition.kind == RepositoryKind::Remote {
            return Err(Refusal::Forbidden(format!(
                "repository {id} is a read-only mirror of another repository"
            )));
        }
        if repo.definition.archived {
            return Err(Refusal::Forbidden(format!("repository {id} is archived and read only")));
        }
        if !may_push(id) {
            return Err(Refusal::Forbidden(format!("you may not push to repository {id}")));
        }
    }
    Ok(repo)
}

fn content_type(service: GitService, advertise: bool) -> String {
    let kind = if advertise { "advertisement" } else { "result" };
    format!("application/x-{}-{kind}", service.name())
}

fn no_cache() -> [(&'static str, String); 3] {
    [
        ("Cache-Control", "no-cache, max-age=0, must-revalidate".to_string()),
        ("Pragma", "no-cache".to_string()),
        ("Expires", "Fri, 01 Jan 1980 00:00:00 GMT".to_string()),
    ]
}

fn refuse_http(out: &mut dyn HttpResponder, refusal: &Refusal) {
    let status = match refusal {
        Refusal::Unauthenticated(_) => 401,
        Refusal::Forbidden(_) => 403,
        Refusal::NotFound(_) => 404,
        Refusal::Unavailable(_) => 503,
    };
    let mut headers = vec![("Content-Type", "text/plain; charset=utf-8".to_string())];
    if status == 401 {
        headers.push(("WWW-Authenticate", "Basic realm=\"Moqui Git\"".to_string()));
    }
    if out.head(status, &headers).is_ok() {
        let body = out.body();
        let _ = writeln!(body, "{}", refusal.message());
        let _ = body.flush();
    }
}

/// One smart-HTTP request, in an entered context (a [`pool`] job). Refusals are HTTP statuses; a failure once the
/// protocol started is an `ERR` line, as git reports it.
pub fn serve_http(request: &GitHttpRequest, body: &mut dyn Read, out: &mut dyn HttpResponder) {
    let mut gunzip;
    let body: &mut dyn Read = if request.gzip {
        gunzip = flate2::read::GzDecoder::new(body);
        &mut gunzip
    } else {
        body
    };
    let started = (|| -> Result<(GitProvider, Arc<Repo>), Refusal> {
        let provider = provider()?;
        let has_credentials = request.authorization.is_some() || request.api_key.is_some();
        if has_credentials && !request.secure {
            return Err(Refusal::Forbidden(
                "credentials over plain HTTP are refused; use HTTPS".into(),
            ));
        }
        if !authenticate(request)? {
            return Err(Refusal::Unauthenticated(format!(
                "log in to reach repository {}",
                request.repository
            )));
        }
        let repo = open(&provider, &request.repository, request.service)?;
        Ok((provider, repo))
    })();
    let (provider, repo) = match started {
        Ok(started) => started,
        Err(refusal) => {
            if !matches!(refusal, Refusal::Unauthenticated(_) | Refusal::NotFound(_)) {
                tracing::info!(repository = %request.repository, "git request refused: {}", refusal.message());
            }
            refuse_http(out, &refusal);
            return;
        }
    };
    let mut headers = vec![("Content-Type", content_type(request.service, request.advertise))];
    headers.extend(no_cache());
    if out.head(200, &headers).is_err() {
        return;
    }
    let v2 = wants_v2(request.git_protocol.as_deref()) && request.service == GitService::UploadPack;
    let session = Session {
        provider: &provider,
        repo: &repo,
        transport: "http",
        client: request.client_ip.clone(),
    };
    let outcome = (|| -> Result<(), Error> {
        let out = out.body();
        if request.advertise {
            if v2 {
                return upload::advertise_v2(out);
            }
            pktline::text(out, &format!("# service={}", request.service.name())).map_err(io_error)?;
            pktline::flush(out).map_err(io_error)?;
            let advertised = session.advertised()?;
            return match request.service {
                GitService::UploadPack => upload::advertise_v0(out, &advertised),
                GitService::ReceivePack => receive::advertise(out, &advertised),
            };
        }
        match request.service {
            GitService::UploadPack if v2 => {
                let advertised = session.advertised()?;
                upload::serve_v2(repo.store.as_ref(), &advertised, body, out, true, pack_threads())
            }
            GitService::UploadPack => {
                let advertised = session.advertised()?;
                upload::upload_v0(repo.store.as_ref(), &advertised, body, out, true, pack_threads())
            }
            GitService::ReceivePack => session.push(body, out),
        }
    })();
    if let Err(err) = outcome {
        tracing::warn!(repository = %request.repository, "git {} failed: {err}", request.service.name());
        let out = out.body();
        let _ = pktline::text(out, &format!("ERR {}", err.message()));
        let _ = out.flush();
    }
}

/// One SSH `exec` of git, as `username` (whom the SSH server logged in), in an entered context (a [`pool`] job): the
/// conversation on `input` and `output`, messages on `stderr`. The exit status.
pub fn serve_ssh(
    username: &str,
    command: &str,
    git_protocol: Option<&str>,
    input: &mut dyn Read,
    output: &mut dyn Write,
    stderr: &mut dyn Write,
) -> u32 {
    let fail = |stderr: &mut dyn Write, text: &str| {
        let _ = writeln!(stderr, "fatal: {text}");
        let _ = stderr.flush();
        128
    };
    if lfs::parse_ssh_command(command).is_some() {
        return match lfs::ssh_authenticate(username, command) {
            Ok(answer) => {
                let _ = writeln!(output, "{answer}");
                let _ = output.flush();
                0
            }
            Err(err) => {
                let _ = writeln!(stderr, "{}", err.message());
                let _ = stderr.flush();
                1
            }
        };
    }
    let Some((service, id)) = parse_ssh_command(command) else {
        return fail(stderr, &format!("not a git command: {command}"));
    };
    let started = (|| -> Result<(GitProvider, Arc<Repo>), Refusal> {
        let provider = provider()?;
        crate::user::internal_login_user(username)
            .map_err(|err| Refusal::Unauthenticated(err.message().to_string()))?;
        let repo = open(&provider, &id, service)?;
        Ok((provider, repo))
    })();
    let (provider, repo) = match started {
        Ok(started) => started,
        Err(refusal) => return fail(stderr, refusal.message()),
    };
    let session = Session {
        provider: &provider,
        repo: &repo,
        transport: "ssh",
        client: String::new(),
    };
    let v2 = wants_v2(git_protocol) && service == GitService::UploadPack;
    let mut input = BufReader::new(input);
    let outcome = (|| -> Result<(), Error> {
        match service {
            GitService::UploadPack if v2 => {
                upload::advertise_v2(output)?;
                output.flush().map_err(io_error)?;
                let advertised = session.advertised()?;
                upload::serve_v2(
                    repo.store.as_ref(),
                    &advertised,
                    &mut input,
                    output,
                    false,
                    pack_threads(),
                )
            }
            GitService::UploadPack => {
                let advertised = session.advertised()?;
                upload::advertise_v0(output, &advertised)?;
                output.flush().map_err(io_error)?;
                upload::upload_v0(
                    repo.store.as_ref(),
                    &advertised,
                    &mut input,
                    output,
                    false,
                    pack_threads(),
                )
            }
            GitService::ReceivePack => {
                receive::advertise(output, &session.advertised()?)?;
                output.flush().map_err(io_error)?;
                session.push(&mut input, output)
            }
        }
    })();
    match outcome {
        Ok(()) => 0,
        Err(err) => {
            tracing::warn!(repository = %id, "git {} over SSH failed: {err}", service.name());
            let _ = pktline::text(output, &format!("ERR {}", err.message()));
            let _ = output.flush();
            fail(stderr, err.message())
        }
    }
}

/// Unique names for quarantine directories.
static QUARANTINES: AtomicU64 = AtomicU64::new(0);

/// One conversation with a client about one repository.
struct Session<'a> {
    provider: &'a GitProvider,
    repo: &'a Repo,
    transport: &'static str,
    client: String,
}

impl Session<'_> {
    fn advertised(&self) -> Result<Advertised, Error> {
        advertised(self.repo.store.as_ref(), &self.repo.definition.branch)
    }

    fn quarantine_dir(&self) -> std::path::PathBuf {
        let name = format!(
            "incoming-{}-{}",
            std::process::id(),
            QUARANTINES.fetch_add(1, Ordering::Relaxed)
        );
        match self.repo.store.disk_path() {
            // on the repository's own filesystem, so accepting is a rename
            Some(path) => path.join("objects").join(name),
            None => self
                .provider
                .inner
                .runtime_dir
                .join("tmp")
                .join(format!("git-{}-{name}", self.repo.definition.id)),
        }
    }

    /// Whether a command may be made, against the repository with the pushed objects over it: its name, its old
    /// value, what it points to, and the user's authorization for what it does. How it changes the ref: create, update
    /// (a fast-forward), force or delete.
    fn check_command(&self, view: &dyn GitStore, command: &receive::Command) -> Result<&'static str, String> {
        let id = &self.repo.definition.id;
        let name = command.name.as_str();
        if gix::validate::reference::name(name.as_bytes().as_bstr()).is_err()
            || !(name.starts_with("refs/heads/") || name.starts_with("refs/tags/"))
        {
            return Err("funny refname: only branches (refs/heads/) and tags (refs/tags/) can be pushed".into());
        }
        let current = view.read_ref(name).map_err(|err| err.message().to_string())?;
        if current != command.old {
            return Err("stale info: the ref moved since it was read; fetch first".into());
        }
        let action = match (command.old, command.new) {
            (_, None) => {
                if name == super::ops::branch_ref(&self.repo.definition.branch) {
                    return Err("deletion of the current branch prohibited".into());
                }
                "delete"
            }
            (old, Some(new)) => {
                let kind = view
                    .find(&new)
                    .map_err(|err| err.message().to_string())?
                    .map(|(kind, _)| kind);
                match kind {
                    None => return Err(format!("missing necessary objects: {new}")),
                    Some(kind) if name.starts_with("refs/heads/") && kind != Kind::Commit => {
                        return Err(format!("a branch must point to a commit, and {new} is a {kind}"));
                    }
                    Some(_) => {}
                }
                match old {
                    None => "create",
                    Some(old) if name.starts_with("refs/heads/") => match revwalk::is_ancestor(view, &old, &new) {
                        Ok(true) => "update",
                        Ok(false) => "delete",
                        Err(err) => return Err(err.message().to_string()),
                    },
                    // moving a tag is a forced change
                    Some(_) => "delete",
                }
            }
        };
        permitted(id, action).then_some(()).ok_or(()).map_err(|()| {
            if action == "delete" && command.new.is_some() {
                format!("non-fast-forward: forcing a ref needs the delete permission on {id}")
            } else {
                format!("permission denied: {action} on repository {id}")
            }
        })?;
        let change = match (action, command.new) {
            ("delete", Some(_)) => "force",
            (action, _) => action,
        };
        // the branch rules, built in: no component's rule can let a push past them
        let found = self
            .provider
            .entity()
            .and_then(|entity| super::rules::rules_for(&entity, id, name))
            .map_err(|err| format!("branch rules not read: {err}"))?;
        super::rules::check_direct(
            &found,
            name,
            super::rules::RefChangeKind::parse(change),
            &super::rules::current_user_in,
        )?;
        Ok(change)
    }

    /// A push: the commands, the pack into quarantine, the checks, then under the repository's lock the objects into
    /// the repository and the ref updates, and the report.
    fn push(&self, input: &mut dyn Read, out: &mut dyn Write) -> Result<(), Error> {
        let mut input = BufReader::new(input);
        let Some(request) = receive::read_request(&mut input)? else {
            return Ok(());
        };
        let store = self.repo.store.as_ref();
        let definition = &self.repo.definition;
        let refs_before = store.list_refs("refs/")?;
        // each command's change type while it may still be made, else why not
        let mut results: Vec<Result<&'static str, String>> = vec![Ok("update"); request.commands.len()];
        let needs_pack = request.commands.iter().any(|command| command.new.is_some());
        let mut quarantine = None;
        if needs_pack {
            let dir = self.quarantine_dir();
            match pack_in::receive(store, &mut input, &dir, max_object_size(), pack_threads()) {
                Ok(received) => quarantine = Some(received),
                Err(err) => {
                    tracing::warn!(repository = %definition.id, "a pushed pack was refused: {err}");
                    let results = vec![Err("unpacker error".to_string()); request.commands.len()];
                    return receive::report(out, &request, Err(err.message().to_string()), &results);
                }
            }
        }
        let shallow: HashSet<ObjectId> = request.shallow.iter().copied().collect();
        if let Some(received) = &quarantine {
            if let Err(err) = received.check_closure(&shallow) {
                tracing::warn!(repository = %definition.id, "a pushed pack is incomplete: {err}");
                receive::message(out, &request, err.message())?;
                for result in &mut results {
                    *result = Err("missing necessary objects".into());
                }
            }
        }
        let origin = events::UpdateOrigin {
            source: "push",
            transport: self.transport,
            user_id: current_user(),
            remote_address: Some(self.client.clone()).filter(|client| !client.is_empty()),
            push_options: request.options.clone(),
        };
        let refs: Vec<events::RefUpdate> = {
            let view: &dyn GitStore = match &quarantine {
                Some(received) => received,
                None => store,
            };
            for (command, result) in request.commands.iter().zip(results.iter_mut()) {
                if result.is_ok() {
                    *result = self.check_command(view, command);
                }
            }
            let refs: Vec<events::RefUpdate> = request
                .commands
                .iter()
                .zip(&results)
                .map(|(command, result)| events::RefUpdate {
                    name: command.name.clone(),
                    old: command.old,
                    new: command.new,
                    change_type: match (result, command.old, command.new) {
                        (Ok(change_type), _, _) => (*change_type).to_string(),
                        (_, None, _) => "create".into(),
                        (_, _, None) => "delete".into(),
                        _ => "update".into(),
                    },
                })
                .collect();
            // gitECA pre-receive and pre-ref-update rules, on the refs the push may still make
            let candidates: Vec<usize> = (0..results.len()).filter(|index| results[*index].is_ok()).collect();
            if !candidates.is_empty() {
                let decided =
                    events::pre_receive(view, &definition.id, &definition.branch, &origin, &refs, &candidates);
                for message in &decided.messages {
                    receive::message(out, &request, message)?;
                }
                if let Some(reason) = &decided.refused {
                    receive::message(out, &request, &format!("push declined: {reason}"))?;
                    for index in &candidates {
                        results[*index] = Err(format!("pre-receive hook declined: {reason}"));
                    }
                }
                for (index, reason) in &decided.refused_refs {
                    receive::message(out, &request, &format!("{} declined: {reason}", refs[*index].name))?;
                    results[*index] = Err(format!("hook declined: {reason}"));
                }
            }
            refs
        };
        if request.has("atomic") && results.iter().any(Result::is_err) {
            for result in &mut results {
                if result.is_ok() {
                    *result = Err("atomic push failed".into());
                }
            }
        }
        let accepted: Vec<usize> = (0..results.len()).filter(|index| results[*index].is_ok()).collect();
        let recorded: Vec<(events::RefUpdate, String)> = refs
            .iter()
            .zip(&results)
            .map(|(reference, result)| {
                (
                    reference.clone(),
                    match result {
                        Ok(_) => "pending".to_string(),
                        Err(reason) => reason.clone(),
                    },
                )
            })
            .collect();
        let update_id = self
            .provider
            .begin_update(&definition.id, &origin, &refs_before, &recorded);
        if !accepted.is_empty() {
            let lock = self.provider.lock_of(&definition.id);
            let _turn = moqui_lib::sync::lock(&lock);
            let stored = match quarantine.as_mut() {
                Some(received) => received.accept().map(|_| ()),
                None => Ok(()),
            };
            match stored {
                Err(err) => {
                    tracing::error!(repository = %definition.id, "pushed objects not stored: {err}");
                    for index in &accepted {
                        results[*index] = Err(format!("cannot store the objects: {}", err.message()));
                    }
                }
                Ok(()) => {
                    let edits: Vec<RefEdit> = accepted
                        .iter()
                        .map(|index| {
                            let command = &request.commands[*index];
                            RefEdit {
                                name: command.name.clone(),
                                expected: command.old,
                                new: command.new,
                            }
                        })
                        .collect();
                    match store.update_refs(&edits, request.has("atomic")) {
                        Ok(outcomes) => {
                            for (index, outcome) in accepted.iter().zip(outcomes) {
                                match outcome {
                                    RefOutcome::Updated => {}
                                    RefOutcome::Stale => {
                                        results[*index] = Err("stale info: the ref moved; fetch first".into());
                                    }
                                    RefOutcome::Aborted => results[*index] = Err("atomic push failed".into()),
                                }
                            }
                        }
                        Err(err) => {
                            tracing::error!(repository = %definition.id, "refs not updated: {err}");
                            for index in &accepted {
                                results[*index] = Err(format!("failed to update ref: {}", err.message()));
                            }
                        }
                    }
                }
            }
        }
        drop(quarantine);
        let results: Vec<Result<(), String>> = results.into_iter().map(|result| result.map(|_| ())).collect();
        if results.iter().any(Result::is_ok) {
            self.updated(&request, &results, &refs_before);
        }
        receive::report(out, &request, Ok(()), &results)?;
        if let Some(update_id) = update_id {
            let outcomes: Vec<(String, String)> = request
                .commands
                .iter()
                .zip(&results)
                .map(|(command, result)| {
                    (
                        command.name.clone(),
                        match result {
                            Ok(()) => "ok".to_string(),
                            Err(reason) => reason.clone(),
                        },
                    )
                })
                .collect();
            self.provider.finish_update(&definition.id, &update_id, &outcomes);
        }
        Ok(())
    }

    /// After a push changed refs: the head on the repository's row, the default branch of an empty repository, and
    /// a fresh open of a repository on disk (gix sizes its pack slots when it opens).
    fn updated(
        &self,
        request: &receive::PushRequest,
        results: &[Result<(), String>],
        refs_before: &[(String, ObjectId)],
    ) {
        let definition = &self.repo.definition;
        let made: Vec<&receive::Command> = request
            .commands
            .iter()
            .zip(results)
            .filter(|(_, result)| result.is_ok())
            .map(|(command, _)| command)
            .collect();
        let branch_ref = super::ops::branch_ref(&definition.branch);
        let mut head = made
            .iter()
            .find(|command| command.name == branch_ref)
            .map(|command| command.new);
        if refs_before.is_empty() && !definition.branch_explicit && head.is_none() {
            // the first push to an empty repository whose row names no branch: its main branch is the default
            let created: Vec<&str> = made
                .iter()
                .filter(|command| command.new.is_some())
                .filter_map(|command| command.name.strip_prefix("refs/heads/"))
                .collect();
            let chosen = ["main", "master"]
                .into_iter()
                .find(|name| created.contains(name))
                .or_else(|| created.first().copied());
            if let Some(chosen) = chosen {
                self.provider.set_default_branch(&definition.id, chosen);
                head = made
                    .iter()
                    .find(|command| command.name == format!("refs/heads/{chosen}"))
                    .map(|command| command.new);
            }
        }
        match head {
            Some(head) => self.provider.remember_head(&definition.id, head, None),
            None => self.provider.remember_update(&definition.id),
        }
        if definition.kind != RepositoryKind::Db {
            self.provider.refresh(&definition.id);
        }
        tracing::info!(
            repository = %definition.id,
            transport = self.transport,
            "push updated {} of {} refs",
            made.len(),
            request.commands.len()
        );
    }
}
