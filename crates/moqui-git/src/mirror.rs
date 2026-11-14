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

//! Push mirrors of a repository (`moqui.resource.GitRepositoryMirror`; plans/Dogfood.md F6, DG8;
//! docs/interfaces/Git-Hosting.md "Push mirrors"): each a URL (HTTPS with a username and a password or token, or SSH
//! with a pinned host key and a key or password), which refs (patterns of branches and tags), and whether refs gone
//! here are deleted there. A push forces each ref its patterns cover to the local value. Pushes are asked for after
//! each change of the repository (the framework's post-receive rule) and run from a queue: `nextAttemptDate`, set to
//! now when a push is wanted and pushed back after each failure (1 minute doubling to 6 hours), which
//! `run#GitRepositoryMirrors` works through each minute. One push of a mirror at a time across a cluster (a GitLease of
//! its own), and a push asked for while one runs is pushed by that one when it ends. The application's outbound check
//! is asked before each push. A failure is kept on the row (`lastError`, the credential taken out) and the first of a
//! run of failures is a notification (topic `GitMirrorFailure`) to the user who stored the mirror. A mirror is kept
//! by who administers the repository (the delete right, [`RIGHT`]), and its credential rides on the right of who gave
//! it: a push of a mirror whose `createdByUserId` no longer has it pauses the mirror instead of using the credential.
//! The push holds the mirror's lease and checks it again right before it records its outcome.

use moqui_core::context::Error;
use moqui_core::entity::{Condition, EntityValue};
use moqui_lib::object::Value;

use crate::server::push::{self, RefMap, SshUrl};
use crate::server::smart_http;
use crate::{lease, long, millis, outbound_allowed, rows_guard, setting, text, GitProvider, Outbound};

pub(crate) const GIT_REPOSITORY_MIRROR: &str = "moqui.resource.GitRepositoryMirror";

/// The code a push of a mirror answers with while another push of it runs.
pub(crate) const BUSY: &str = "GitMirrorBusy";

/// The right on a repository that keeps its push mirrors and fetches refs into it, as for its webhooks and branch
/// rules: a mirror's credential and a fetch reach out of the server.
pub(crate) const RIGHT: &str = "delete";

/// The notification topic of a mirror's first failure after a success.
pub(crate) const FAILURE_TOPIC: &str = "GitMirrorFailure";

/// The service the queue runs a mirror's push on.
pub(crate) const RUN_SERVICE: &str = "org.moqui.git.GitServices.run#GitRepositoryMirrors";

const BACKOFF_FIRST_MILLIS: i64 = 60_000;
const BACKOFF_CAP_MILLIS: i64 = 6 * 3_600_000;

/// The longest `lastError` kept.
const MAX_ERROR_CHARS: usize = 4000;

/// How long after its `attempts`-th failure in a row a mirror is pushed again: 1 minute, doubling, at most 6 hours.
pub(crate) fn backoff_millis(attempts: i64) -> i64 {
    let shift = u32::try_from((attempts - 1).clamp(0, 20)).unwrap_or(20);
    BACKOFF_FIRST_MILLIS
        .saturating_mul(1_i64 << shift)
        .min(BACKOFF_CAP_MILLIS)
}

/// Where a mirror is.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MirrorUrl {
    /// `https://host/owner/repo.git` (or `http://` with private networks allowed).
    Http(String),
    Ssh(SshUrl),
}

/// Whether internal addresses (and plain http) may be a mirror's: `git-server` `webhook-private-network`, as for
/// webhooks.
pub(crate) fn private_network() -> bool {
    setting("webhook-private-network") == "true"
}

/// A mirror's URL: HTTPS with no user or password in it, or SSH (`ssh://git@host/path` or `git@host:path`) with a user
/// name and no password in it.
pub(crate) fn parse_mirror_url(url: &str, private_network: bool) -> Result<MirrorUrl, String> {
    let url = url.trim();
    if url.starts_with("https://") || url.starts_with("http://") {
        smart_http::check_url(url, private_network)?;
        return Ok(MirrorUrl::Http(url.to_string()));
    }
    if url.contains("://") && !url.starts_with("ssh://") {
        return Err("a push mirror is an https or ssh URL".into());
    }
    let parsed = push::parse_ssh_url(url).map_err(|err| err.message().to_string())?;
    if parsed.user.as_deref().is_some_and(|user| user.contains(':')) {
        return Err("the URL has a password in it; give the credential apart".into());
    }
    Ok(MirrorUrl::Ssh(parsed))
}

/// Where a mirror's credential goes: the scheme, host and port of an https URL, or the user, host and port of an ssh
/// one, the host in lower case. A change of it needs the credential again (`store#GitRepositoryMirror`).
pub(crate) fn credential_target(url: &str) -> String {
    let host_port = |authority: &str, default: u16| match moqui_core::sftp::split_host_port(authority, default) {
        Ok((host, port)) => format!("{}:{port}", host.to_ascii_lowercase()),
        Err(_) => authority.to_ascii_lowercase(),
    };
    match parse_mirror_url(url, true) {
        Ok(MirrorUrl::Http(url)) => {
            let (scheme, rest) = url.split_once("://").unwrap_or(("", url.as_str()));
            let scheme = scheme.to_ascii_lowercase();
            let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
            let default = if scheme == "https" { 443 } else { 80 };
            format!("{scheme}://{}", host_port(authority, default))
        }
        Ok(MirrorUrl::Ssh(ssh)) => format!("ssh://{}@{}", ssh.user.unwrap_or_default(), host_port(&ssh.host, 22)),
        Err(_) => url.trim().to_string(),
    }
}

/// Where an ssh mirror's push connects, for its URL's `host` (`host`, `host:port`, `[v6]:port`, one parser for the
/// check and the connection): one of the addresses the name resolves to, each of them public, so a name that resolves
/// again to an internal address between the check and the connection (DNS rebinding) is not followed; the host key
/// is pinned, so a connection by address reaches the same server. By name with private networks allowed.
pub(crate) fn ssh_address(host: &str, private: bool) -> Result<(String, u16), Error> {
    let (name, port) = moqui_core::sftp::split_host_port(host, 22)?;
    if private {
        return Ok((name, port));
    }
    let addresses = moqui_lib::net::public_addresses(&name, port)
        .map_err(|err| Error::new(format!("push mirror {name}: {err}")))?;
    let address = addresses
        .first()
        .ok_or_else(|| Error::new(format!("push mirror {name}: the name has no address")))?;
    Ok((address.ip().to_string(), address.port()))
}

/// A mirror's `refPattern`: comma-separated patterns of branches and tags (`refs/heads/*`, `main`, `refs/tags/v*`,
/// `main:mirror`), every branch and tag when empty. A local ref outside `refs/heads/` and `refs/tags/` (a merge
/// request's head, the framework's own refs) is never a mirror's.
pub(crate) fn mirror_maps(pattern: &str) -> Result<Vec<RefMap>, String> {
    let maps = push::parse_ref_maps(pattern).map_err(|err| err.message().to_string())?;
    for map in &maps {
        let branch_or_tag = |name: &str| name.starts_with("refs/heads/") || name.starts_with("refs/tags/");
        if !branch_or_tag(&map.from) || !branch_or_tag(&map.to) {
            return Err(format!(
                "ref pattern {}:{} is not of branches or tags (refs/heads/, refs/tags/)",
                map.from, map.to
            ));
        }
    }
    Ok(maps)
}

fn now_millis() -> i64 {
    moqui_core::user::now_millis()
}

fn stamp(millis: i64) -> Value {
    moqui_core::user::stamp_value(millis)
}

/// `text` cut to [`MAX_ERROR_CHARS`].
fn kept_text(text: &str) -> String {
    match text.char_indices().nth(MAX_ERROR_CHARS) {
        Some((at, _)) => format!("{}...", &text[..at]),
        None => text.to_string(),
    }
}

impl GitProvider {
    fn mirror_row(&self, mirror_id: &str) -> Result<Option<EntityValue>, Error> {
        let _rows = rows_guard();
        Ok(self
            .entity()?
            .find(GIT_REPOSITORY_MIRROR)?
            .eq("mirrorId", Value::string(mirror_id))
            .use_cache(false)
            .one()?)
    }

    /// Whether a push of the mirror runs now, on any member.
    pub(crate) fn mirror_busy(&self, mirror_id: &str) -> Result<bool, Error> {
        Ok(lease::try_take(&self.entity()?, &lease_name(mirror_id), lease::TTL)?.is_none())
    }

    /// Ask for a push of each enabled mirror of a repository now (its `nextAttemptDate`, so the queue has it whatever
    /// happens to this process) and start them on the service pool. How many.
    pub fn request_mirror_pushes(&self, repository_id: &str) -> Result<usize, Error> {
        let entity = self.entity()?;
        let ids: Vec<String> = {
            let _rows = rows_guard();
            entity
                .find(GIT_REPOSITORY_MIRROR)?
                .eq("repositoryId", Value::string(repository_id))
                .condition(enabled())
                .use_cache(false)
                .list()?
                .iter()
                .filter_map(|row| text(row.get("mirrorId")))
                .collect()
        };
        for id in &ids {
            self.request_mirror_push(id)?;
        }
        Ok(ids.len())
    }

    /// Ask for a push of one mirror now: its `nextAttemptDate` is now, and the push is started on the service pool
    /// (when the pool cannot take it, the queue's next run pushes it).
    pub(crate) fn request_mirror_push(&self, mirror_id: &str) -> Result<(), Error> {
        {
            let _rows = rows_guard();
            self.entity()?
                .find(GIT_REPOSITORY_MIRROR)?
                .eq("mirrorId", Value::string(mirror_id))
                .update_all([("nextAttemptDate".to_string(), stamp(now_millis()))])?;
        }
        let parameters = [("mirrorId".to_string(), Value::string(mirror_id))]
            .into_iter()
            .collect();
        if let Err(err) = moqui_core::service::call::async_call(RUN_SERVICE, &parameters) {
            tracing::info!(
                mirror = mirror_id,
                "a push mirror is left to the queue's next run: {err}"
            );
        }
        Ok(())
    }

    /// Push the mirrors that are due (enabled, `nextAttemptDate` now or before), or only `only` when given (as soon as
    /// it is asked for). A mirror another push holds is passed over: that push pushes again when it ends. (pushed,
    /// failed) ids; each failure is on its row.
    pub fn push_due_mirrors(&self, only: Option<&str>) -> Result<(Vec<String>, Vec<String>), Error> {
        let rows: Vec<EntityValue> = {
            let _rows = rows_guard();
            let mut find = self
                .entity()?
                .find(GIT_REPOSITORY_MIRROR)?
                .condition(enabled())
                .use_cache(false);
            find = match only {
                Some(id) => find.eq("mirrorId", Value::string(id)),
                None => find.condition(Condition::less_equal("nextAttemptDate", stamp(now_millis()))),
            };
            find.order_by("nextAttemptDate", false)
                .limit(500)
                .list()?
                .iter()
                .cloned()
                .collect()
        };
        let (mut pushed, mut failed) = (Vec::new(), Vec::new());
        for row in rows {
            let Some(id) = text(row.get("mirrorId")) else {
                continue;
            };
            match self.push_repository_mirror(&id) {
                Ok(_) => pushed.push(id),
                Err(err) if err.message().starts_with(BUSY) => {}
                Err(err) => {
                    tracing::warn!(mirror = %id, "push mirror failed: {}", err.message());
                    failed.push(id);
                }
            }
        }
        Ok((pushed, failed))
    }

    /// Push one mirror now, under its lease (another push of it running: an error starting `GitMirrorBusy: `), and
    /// record the outcome on its row; a push asked for while this one ran is pushed before the lease is let go. What
    /// was done.
    pub fn push_repository_mirror(&self, mirror_id: &str) -> Result<String, Error> {
        let Some(lease) = lease::try_take(&self.entity()?, &lease_name(mirror_id), lease::TTL)? else {
            return Err(Error::new(format!("{BUSY}: a push of mirror {mirror_id} is running")));
        };
        let mut done = String::new();
        for _ in 0..3 {
            let started = now_millis();
            let Some(row) = self.mirror_row(mirror_id)? else {
                return Err(Error::new(format!("no push mirror {mirror_id}")));
            };
            if text(row.get("enabled")).as_deref() == Some("N") {
                return Ok(format!("push mirror {mirror_id} is not enabled"));
            }
            let repository_id = text(row.get("repositoryId")).unwrap_or_default();
            if let Some(why) = creator_without_right(&row, &repository_id) {
                // the credential is not used for a user who no longer administers the repository
                lease.check()?;
                self.pause_mirror(mirror_id, &why);
                return Err(Error::new(why));
            }
            let credential = text(row.get("credential"));
            let outcome = match outbound_allowed(&repository_id, Outbound::PushMirror, "") {
                Ok(()) => self.push_mirror_once(&repository_id, &row),
                Err(why) => Err(Error::new(format!("not pushed: {why}"))),
            };
            // a push may take up to 30 minutes: the lease is checked right before its outcome is written, so a
            // holder that lost it to another member does not write over that member's
            lease.check()?;
            match outcome {
                Ok(text) => {
                    done = text;
                    if !self.record_mirror(mirror_id, Ok(&done), started) {
                        break;
                    }
                }
                Err(err) => {
                    let message = kept_text(&smart_http::redact(err.message(), credential.as_deref()));
                    let first = self.record_mirror(mirror_id, Err(&message), started);
                    if first {
                        notify_failure(&row, &message);
                    }
                    return Err(Error::new(message));
                }
            }
        }
        Ok(done)
    }

    /// One push of a mirror's refs as they are now.
    fn push_mirror_once(&self, repository_id: &str, row: &EntityValue) -> Result<String, Error> {
        let private = private_network();
        let url = text(row.get("url")).unwrap_or_default();
        let target = parse_mirror_url(&url, private).map_err(|why| Error::new(format!("push mirror URL: {why}")))?;
        let maps = mirror_maps(&text(row.get("refPattern")).unwrap_or_default()).map_err(Error::new)?;
        let delete = text(row.get("deleteRefs")).as_deref() == Some("Y");
        let username = text(row.get("username"));
        let credential = text(row.get("credential"));
        if self.entity()?.sim_active() {
            return Err(Error::new("push mirror denied because sim mode is active"));
        }
        let repo = self.repo(repository_id)?;
        let local: Vec<_> = repo
            .store
            .list_refs("refs/")?
            .into_iter()
            .filter(|(name, _)| name.starts_with("refs/heads/") || name.starts_with("refs/tags/"))
            .collect();
        let refs = push::mapped(&maps, &local);
        let covered = delete.then_some(maps.as_slice());
        let (shown, pushed) = match target {
            MirrorUrl::Http(url) => {
                let remote = smart_http::Remote {
                    url: &url,
                    username: username.as_deref(),
                    password: credential.as_deref(),
                    private_network: private,
                };
                moqui_core::entity::fence::check_effect(
                    moqui_core::entity::fence::Effect::RemoteCall,
                    &format!("push mirror to {}", remote.shown()),
                )?;
                let pushed = smart_http::push(
                    repo.store.as_ref(),
                    &remote,
                    &refs,
                    covered,
                    &self.scratch("mirror", repository_id),
                )?;
                (remote.shown(), pushed)
            }
            MirrorUrl::Ssh(url) => {
                let shown = format!("{}:{}", url.host, url.path);
                moqui_core::entity::fence::check_effect(
                    moqui_core::entity::fence::Effect::RemoteCall,
                    &format!("push mirror to {shown}"),
                )?;
                let pushed = self.push_over_ssh(&repo, &url, row, username, credential, &refs, covered, private)?;
                (shown, pushed)
            }
        };
        if !pushed.refused.is_empty() {
            return Err(Error::new(format!(
                "push to {shown} refused {}",
                pushed
                    .refused
                    .iter()
                    .map(|(name, reason)| format!("{name} ({reason})"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        Ok(format!(
            "pushed {} refs ({} objects) to {shown}; {} already current",
            pushed.pushed.len(),
            pushed.objects,
            pushed.current
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn push_over_ssh(
        &self,
        repo: &crate::Repo,
        url: &SshUrl,
        row: &EntityValue,
        username: Option<String>,
        credential: Option<String>,
        refs: &[(String, gix::ObjectId)],
        covered: Option<&[RefMap]>,
        private: bool,
    ) -> Result<push::PushOutcome, Error> {
        let fingerprint = text(row.get("hostKeyFingerprint"))
            .ok_or_else(|| Error::new("an ssh push mirror needs its hostKeyFingerprint"))?;
        let (address, port) = ssh_address(&url.host, private)?;
        let user = url.user.clone().or(username).unwrap_or_else(|| "git".into());
        let mut client = moqui_core::sftp::SftpClient::new(&address, &user, Some(port))?;
        match credential {
            Some(key) if key.contains("PRIVATE KEY") => {
                client.public_key_pkcs8(&key, None);
            }
            Some(password) => {
                client.password(&password);
            }
            None => {}
        }
        client.host_key_fingerprint(&fingerprint);
        let mut exec = client.exec(&push::receive_pack_command(&url.path))?;
        let pushed = {
            let io = push::ExecIo(std::cell::RefCell::new(exec.as_mut()));
            let mut output = std::io::BufWriter::with_capacity(64 * 1024, &io);
            let pushed = push::push(repo.store.as_ref(), refs, covered, &mut &io, &mut output);
            drop(output);
            pushed
        };
        let pushed = pushed.map_err(|err| {
            let stderr = exec.stderr();
            match stderr.trim() {
                "" => err,
                stderr => Error::new(format!("{} ({stderr})", err.message())),
            }
        })?;
        if let Err(err) = push::finish(exec.as_mut()) {
            tracing::warn!("ending the push to {}: {err}", url.host);
        }
        Ok(pushed)
    }

    /// Pause a mirror (`enabled` N, nothing due) with why on its row, in a transaction of its own.
    fn pause_mirror(&self, mirror_id: &str, why: &str) {
        let paused = (|| -> Result<(), Error> {
            let entity = self.entity()?;
            let transactions = entity.transactions_for(GIT_REPOSITORY_MIRROR)?;
            transactions.run(moqui_core::transaction::TransactionAttribute::ForceNew, None, || {
                let _rows = rows_guard();
                entity
                    .find(GIT_REPOSITORY_MIRROR)?
                    .eq("mirrorId", Value::string(mirror_id))
                    .update_all([
                        ("enabled".to_string(), Value::string("N")),
                        ("lastResult".to_string(), Value::string("paused")),
                        ("lastError".to_string(), Value::string(kept_text(why))),
                        ("nextAttemptDate".to_string(), Value::Null),
                    ])?;
                Ok(())
            })?;
            Ok(())
        })();
        if let Err(err) = paused {
            tracing::error!(mirror = mirror_id, "cannot pause the push mirror: {err}");
        }
    }

    /// Record a push's outcome on the mirror's row, in a transaction of its own. A success clears the error and the
    /// count; it keeps `nextAttemptDate` when a push was asked for after `started` (true: push again), else clears it.
    /// A failure counts and puts the next try off ([`backoff_millis`]); true when it is the first of a run.
    fn record_mirror(&self, mirror_id: &str, outcome: Result<&str, &str>, started: i64) -> bool {
        let recorded = (|| -> Result<bool, Error> {
            let entity = self.entity()?;
            let transactions = entity.transactions_for(GIT_REPOSITORY_MIRROR)?;
            Ok(
                transactions.run(moqui_core::transaction::TransactionAttribute::ForceNew, None, || {
                    let _rows = rows_guard();
                    let Some(row) = entity
                        .find(GIT_REPOSITORY_MIRROR)?
                        .eq("mirrorId", Value::string(mirror_id))
                        .use_cache(false)
                        .one()?
                    else {
                        return Ok(false);
                    };
                    let attempts = long(row.get("attemptCount")).unwrap_or(0);
                    let now = now_millis();
                    let (fields, answer) = match outcome {
                        Ok(done) => {
                            let again = millis(row.get("nextAttemptDate")).is_some_and(|next| next >= started);
                            let mut fields = vec![
                                ("lastPushDate".to_string(), stamp(now)),
                                ("lastResult".to_string(), Value::string(kept_text(done))),
                                ("lastError".to_string(), Value::Null),
                                ("attemptCount".to_string(), Value::I64(0)),
                            ];
                            if !again {
                                fields.push(("nextAttemptDate".to_string(), Value::Null));
                            }
                            (fields, again)
                        }
                        Err(error) => (
                            vec![
                                ("lastResult".to_string(), Value::string("failed")),
                                ("lastError".to_string(), Value::string(error)),
                                ("attemptCount".to_string(), Value::I64(attempts + 1)),
                                ("nextAttemptDate".to_string(), stamp(now + backoff_millis(attempts + 1))),
                            ],
                            attempts == 0,
                        ),
                    };
                    entity
                        .find(GIT_REPOSITORY_MIRROR)?
                        .eq("mirrorId", Value::string(mirror_id))
                        .update_all(fields)?;
                    Ok(answer)
                })?,
            )
        })();
        recorded.unwrap_or_else(|err| {
            tracing::error!(mirror = mirror_id, "cannot record the push mirror outcome: {err}");
            false
        })
    }
}

/// A mirror that is enabled: `enabled` not N (empty is enabled).
fn enabled() -> Condition {
    Condition::or(vec![
        Condition::is_null("enabled"),
        Condition::not_equals("enabled", Value::string("N")),
    ])
}

/// Why a mirror's credential is not to be used now: the user who gave it (`createdByUserId`) no longer has [`RIGHT`]
/// on the repository, decided as that user. Not for a mirror stored with authorization disabled (the application's,
/// `storedAuthzDisabled`) or with no user.
fn creator_without_right(row: &EntityValue, repository_id: &str) -> Option<String> {
    if text(row.get("storedAuthzDisabled")).as_deref() == Some("Y") {
        return None;
    }
    let creator = text(row.get("createdByUserId"))?;
    let may = moqui_core::user::as_user(&creator, || crate::server::permitted(repository_id, RIGHT)).unwrap_or(false);
    (!may).then(|| {
        format!(
            "paused: {creator}, who gave the mirror's credential, no longer administers repository {repository_id}; \
             an administrator gives the credential again and enables it"
        )
    })
}

fn lease_name(mirror_id: &str) -> String {
    format!("git-mirror:{mirror_id}")
}

/// Tell the user who stored the mirror that its pushes fail (topic [`FAILURE_TOPIC`], kept and shown as an alert).
fn notify_failure(row: &EntityValue, error: &str) {
    let Some(user_id) = text(row.get("createdByUserId")) else {
        return;
    };
    let repository_id = text(row.get("repositoryId")).unwrap_or_default();
    let mirror_id = text(row.get("mirrorId")).unwrap_or_default();
    let url = text(row.get("url")).unwrap_or_default();
    let shown = if url.contains("://") && !url.starts_with("ssh://") {
        moqui_lib::http::redacted(&url)
    } else {
        url
    };
    let mut message = moqui_lib::object::ValueMap::default();
    message.insert("repositoryId".to_string(), Value::string(&repository_id));
    message.insert("mirrorId".to_string(), Value::string(&mirror_id));
    message.insert("url".to_string(), Value::string(&shown));
    message.insert("error".to_string(), Value::string(error));
    let sent = moqui_core::notify::NotificationMessage::new()
        .topic(FAILURE_TOPIC)
        .sub_topic(&repository_id)
        .user_id(&user_id)
        .title(format!("The push mirror of {repository_id} to {shown} fails"))
        .message_map(message)
        .persist(true)
        .show_alert(true)
        .send();
    if let Err(err) = sent {
        tracing::warn!(mirror = %mirror_id, "the push mirror failure was not notified: {err}");
    }
}

#[cfg(test)]
#[path = "mirror_tests.rs"]
mod tests;
