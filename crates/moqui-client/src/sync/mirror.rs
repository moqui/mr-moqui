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

//! The current account's chat and mail read ahead into its store (plans/Mobile-Desktop.md §6, MD4), so they open
//! offline. A plain thread, as `renewal.rs`, started when the shell's `/notws` opens for the account: at sign-in and at
//! a start a full run (unless one ended in the last minutes); when the socket comes back after a drop of one of the
//! account's `/notws` ([`Mirror::dropped`]), a run of the heads only, each room's latest page, which brings the
//! timelines the drop marked stale up to date again.
//!
//! - **Personal:** the room list, then each room's latest page and its older pages back to `mirror-days`; the inbox's
//!   folders and the new message's `composeData`, then the unified inbox's and each folder's thread pages back to
//!   `mirror-days`, and those threads' messages.
//! - **Shared:** the latest page of the `prefetch-rooms` most recent rooms, the new message's `composeData`, and the
//!   inbox's first `prefetch-mail` threads with their messages.
//!
//! The requests go through the proxy with the account's key ([`Transport`]), one at a time with a short pause between
//! them, and their answers are written as the scheme writes the pages' (`offline::keep`). A run is bounded (pages per
//! room and folder, requests in all), stops when the server cannot be reached or no longer takes the key, when the
//! account changes or signs out ([`Mirror::cancel`]), and logs and passes over a room or folder that fails.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::account::Account;
use crate::app::{Client, ClientHost};
use crate::conf::{ClientConf, DeviceMode};
use crate::offline::{self, chat_base, compose_base, mail_base, number};
use crate::proxy::{header, Request};
use crate::SHELL_ORIGIN;

/// The pause between two requests of a run.
const PAUSE: Duration = Duration::from_millis(150);
/// The least time between two runs for one account, but after a drop of its socket.
const AGAIN_AFTER: Duration = Duration::from_secs(10 * 60);
/// The most requests of a run.
const MAX_REQUESTS: usize = 4000;
/// The most pages of one room's timeline, or of one folder's threads, in a run.
const MAX_PAGES: usize = 40;
/// The most threads whose messages a run reads.
const MAX_THREADS: usize = 2000;
/// Events of a timeline page, as Chat asks for them.
const TIMELINE_LIMIT: u32 = 50;
/// Threads of a folder's page, as Mail asks for them.
const THREADS_PAGE: u32 = 50;

/// What a request of the mirror got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Fetched {
    /// A 200 JSON answer.
    Json(Vec<u8>),
    /// Another status: this room or folder is passed over.
    Failed(u16),
    /// The key is no longer taken (401): the run stops.
    Unauthorized,
    /// The server could not be reached: the run stops.
    Offline,
}

/// The server, as the mirror asks it: a `GET` of a path with its query, with the account's key.
pub(crate) trait Transport {
    fn get(&self, path: &str) -> Fetched;
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ended {
    Done,
    Cancelled,
    Offline,
    Unauthorized,
    /// It reached [`MAX_REQUESTS`].
    Bounded,
}

/// What a run reads and where its answers go.
pub(crate) struct Job<'a> {
    pub(crate) transport: &'a dyn Transport,
    /// Write an answer of a path (with its query) as the scheme does.
    pub(crate) keep: &'a dyn Fn(&str, &[u8]),
    pub(crate) comms_root: String,
    pub(crate) mode: DeviceMode,
    pub(crate) conf: ClientConf,
    /// Now, ms since the epoch: `mirror-days` reaches back from it.
    pub(crate) now_ms: i64,
    pub(crate) cancelled: &'a dyn Fn() -> bool,
    pub(crate) pause: Duration,
    /// Only each room's latest page (the timelines' heads), after a drop.
    pub(crate) heads_only: bool,
}

/// A run's progress: the requests it made.
struct Run<'j, 'a> {
    job: &'j Job<'a>,
    requests: usize,
}

impl Run<'_, '_> {
    /// `GET path`, its answer kept; the JSON of a 200, none for another status (logged), a stop for the rest.
    fn get(&mut self, path: &str) -> Result<Option<Value>, Ended> {
        if (self.job.cancelled)() {
            return Err(Ended::Cancelled);
        }
        if self.requests >= MAX_REQUESTS {
            return Err(Ended::Bounded);
        }
        if self.requests > 0 && !self.job.pause.is_zero() {
            std::thread::sleep(self.job.pause);
            if (self.job.cancelled)() {
                return Err(Ended::Cancelled);
            }
        }
        self.requests += 1;
        match self.job.transport.get(path) {
            Fetched::Json(body) => {
                (self.job.keep)(path, &body);
                Ok(serde_json::from_slice(&body).ok())
            }
            Fetched::Failed(status) => {
                tracing::info!("Mirror: {path} answered {status}; passed over");
                Ok(None)
            }
            Fetched::Unauthorized => Err(Ended::Unauthorized),
            Fetched::Offline => Err(Ended::Offline),
        }
    }
}

/// Run the mirror of `job`: chat, then mail.
pub(crate) fn run(job: &Job<'_>) -> Ended {
    let mut run = Run { job, requests: 0 };
    let ended = if job.heads_only {
        chat(&mut run)
    } else {
        chat(&mut run).and_then(|()| mail(&mut run))
    };
    match ended {
        Ok(()) => Ended::Done,
        Err(ended) => ended,
    }
}

/// The oldest date a personal mirror reaches back to.
fn cutoff(job: &Job<'_>) -> i64 {
    job.now_ms - i64::from(job.conf.mirror_days) * 24 * 3600 * 1000
}

fn encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn chat(run: &mut Run<'_, '_>) -> Result<(), Ended> {
    let job = run.job;
    let base = chat_base(&job.comms_root);
    let Some(data) = run.get(&format!("{base}/chatData?limit=200"))? else {
        return Ok(());
    };
    // the user's rooms, the latest first
    let mut rooms: Vec<(String, i64)> = data["roomList"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|room| room["isMember"].as_bool() != Some(false))
        .filter_map(|room| {
            let id = room["chatRoomId"].as_str()?.to_string();
            let sort = number(&room["lastMessage"]["sentDate"])
                .or_else(|| number(&room["lastEventDate"]))
                .unwrap_or(0);
            Some((id, sort))
        })
        .collect();
    rooms.sort_by_key(|room| std::cmp::Reverse(room.1));
    if job.mode == DeviceMode::Shared {
        rooms.truncate(usize::try_from(job.conf.prefetch_rooms).unwrap_or(usize::MAX));
    }
    let cutoff = cutoff(job);
    for (room, _) in rooms {
        let mut path = format!("{base}/timeline?chatRoomId={}&limit={TIMELINE_LIMIT}", encode(&room));
        for _ in 0..MAX_PAGES {
            let Some(page) = run.get(&path)? else { break };
            if job.mode == DeviceMode::Shared || job.heads_only || page["hasMoreBefore"].as_bool() != Some(true) {
                break;
            }
            let events = page["eventList"].as_array().cloned().unwrap_or_default();
            let Some(oldest) = events.first() else { break };
            let (Some(seq), sent) = (number(&oldest["eventSeq"]), number(&oldest["sentDate"]).unwrap_or(0)) else {
                break;
            };
            if sent < cutoff {
                break;
            }
            path = format!(
                "{base}/timeline?chatRoomId={}&beforeSeq={seq}&limit={TIMELINE_LIMIT}",
                encode(&room)
            );
        }
    }
    Ok(())
}

/// The query of a folder's thread page, as Mail asks for it (`listParams`), so the kept answer is the page's.
fn threads_query(view: &str, account: &str, folder: &str, cursor: Option<&str>) -> String {
    let mut query = format!(
        "view={}&account={}&folder={}&label=&mailbox=&q=&unreadOnly=false&pageSize={THREADS_PAGE}",
        encode(view),
        encode(account),
        encode(folder)
    );
    if let Some(cursor) = cursor {
        query.push_str("&cursor=");
        query.push_str(&encode(cursor));
    }
    query
}

fn mail(run: &mut Run<'_, '_>) -> Result<(), Ended> {
    let job = run.job;
    let base = mail_base(&job.comms_root);
    let data = run.get(&format!("{base}/inboxData"))?;
    // what a new message starts with (the identities, their signatures, the preferences), so one is written offline and
    // a reply or forward is built from it and the kept message (offline.rs compose_json)
    run.get(&format!("{}/composeData", compose_base(&job.comms_root)))?;
    let Some(data) = data else {
        return Ok(());
    };
    // the unified inbox first, then (personal) each account's folders
    let mut lists: Vec<(String, String, String)> = vec![("inbox".to_string(), String::new(), String::new())];
    if job.mode == DeviceMode::Personal {
        for account in data["accountList"].as_array().into_iter().flatten() {
            let Some(account_id) = account["emailAccountId"].as_str() else {
                continue;
            };
            for folder in account["folderList"].as_array().into_iter().flatten() {
                if let Some(folder_id) = folder["emailFolderId"].as_str() {
                    lists.push((String::new(), account_id.to_string(), folder_id.to_string()));
                }
            }
        }
    }
    let cutoff = cutoff(job);
    let shared_max = usize::try_from(job.conf.prefetch_mail).unwrap_or(usize::MAX);
    let mut threads: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for (view, account, folder) in lists {
        let mut cursor: Option<String> = None;
        let mut listed = 0;
        for _ in 0..MAX_PAGES {
            if job.mode == DeviceMode::Shared && listed >= shared_max {
                break;
            }
            let query = threads_query(&view, &account, &folder, cursor.as_deref());
            let Some(page) = run.get(&format!("{base}/threads?{query}"))? else {
                break;
            };
            let rows = page["threadList"].as_array().cloned().unwrap_or_default();
            let mut past = false;
            for row in &rows {
                if job.mode == DeviceMode::Shared && listed >= shared_max {
                    break;
                }
                if job.mode == DeviceMode::Personal && number(&row["sortDate"]).unwrap_or(0) < cutoff {
                    past = true;
                    continue;
                }
                listed += 1;
                let thread = row["threadId"]
                    .as_str()
                    .map(|id| (row["store"].as_str().unwrap_or("email"), id));
                if let Some((store, id)) = thread {
                    if seen.insert(format!("{store}:{id}")) {
                        threads.push(format!("store={}&threadId={}", encode(store), encode(id)));
                    }
                }
            }
            cursor = page["nextCursor"].as_str().map(str::to_string);
            if past || page["hasMore"].as_bool() != Some(true) || cursor.is_none() {
                break;
            }
        }
    }
    for thread in threads.into_iter().take(MAX_THREADS) {
        run.get(&format!("{base}/thread?{thread}"))?;
    }
    Ok(())
}

/// The mirror runs of the client: one at a time, for the current account.
#[derive(Default)]
pub struct Mirror {
    /// Bumped to cancel the run under way.
    generation: AtomicU64,
    /// The account of the run under way, with its generation.
    running: Mutex<Option<(String, u64)>>,
    /// When each account's last run ended.
    last: Mutex<Vec<(String, Instant)>>,
    /// The accounts one of whose `/notws` dropped since their last run.
    dropped: Mutex<HashSet<String>>,
}

impl Mirror {
    /// Whether a run for `account` should start now, and of what: none when one is under way for it; a full run when
    /// none ended in the last minutes; the heads only when one did but a socket of the account dropped since. Marks it
    /// under way, with the generation it runs in, and gives that and whether it is of the heads only.
    pub(crate) fn begin(&self, account: &str) -> Option<(u64, bool)> {
        let generation = self.generation.load(Ordering::SeqCst);
        let mut running = moqui_lib::sync::lock(&self.running);
        if running
            .as_ref()
            .is_some_and(|(id, at)| id == account && *at == generation)
        {
            return None;
        }
        let recent = moqui_lib::sync::lock(&self.last)
            .iter()
            .any(|(id, ended)| id == account && ended.elapsed() < AGAIN_AFTER);
        let dropped = moqui_lib::sync::lock(&self.dropped).remove(account);
        if recent && !dropped {
            return None;
        }
        *running = Some((account.to_string(), generation));
        Some((generation, recent))
    }

    /// A `/notws` of `account` dropped: pushes may have been missed, so its next start runs the heads again.
    pub(crate) fn dropped(&self, account: &str) {
        moqui_lib::sync::lock(&self.dropped).insert(account.to_string());
    }

    /// A run of `generation` for `account` ended.
    pub(crate) fn end(&self, account: &str, generation: u64, ended: Ended) {
        let mut running = moqui_lib::sync::lock(&self.running);
        if running
            .as_ref()
            .is_some_and(|(id, at)| id == account && *at == generation)
        {
            *running = None;
        }
        drop(running);
        if ended == Ended::Done || ended == Ended::Bounded {
            let mut last = moqui_lib::sync::lock(&self.last);
            last.retain(|(id, _)| id != account);
            last.push((account.to_string(), Instant::now()));
        }
    }

    /// Whether the run of `generation` is to stop.
    pub(crate) fn cancelled(&self, generation: u64) -> bool {
        self.generation.load(Ordering::SeqCst) != generation
    }

    /// Stop the run under way: the account changed, signed out or was removed.
    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Forget when an account last ran, so its next sign-in runs at once.
    pub fn forget(&self, account: &str) {
        moqui_lib::sync::lock(&self.last).retain(|(id, _)| id != account);
    }
}

/// Start a run for the account `id` on a thread of its own, as [`Mirror::begin`] decides.
pub fn start(client: &Arc<Client>, host: &Arc<dyn ClientHost>, id: &str) {
    let Some((generation, heads_only)) = client.mirror.begin(id) else {
        return;
    };
    let thread_client = Arc::clone(client);
    let thread_host = Arc::clone(host);
    let thread_id = id.to_string();
    let started = std::thread::Builder::new()
        .name("client-mirror".to_string())
        .spawn(move || {
            let ended = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_for(thread_host.as_ref(), &thread_client, &thread_id, generation, heads_only)
            }))
            .unwrap_or_else(|_| {
                tracing::error!("The mirror of {thread_id} panicked");
                Ended::Cancelled
            });
            thread_client.mirror.end(&thread_id, generation, ended);
        });
    if let Err(err) = started {
        tracing::error!("The mirror of {id} did not start: {err}");
        client.mirror.end(id, generation, Ended::Cancelled);
    }
}

/// One run for the account `id`, with its key, mode and comms app.
fn run_for(host: &dyn ClientHost, client: &Client, id: &str, generation: u64, heads_only: bool) -> Ended {
    let Some((account, Some(key))) = client.account(id) else {
        return Ended::Cancelled;
    };
    let comms_root = client.comms_root_of(id);
    let transport = ProxyTransport {
        host,
        client,
        account: &account,
        key: &key,
    };
    let keep = |path: &str, body: &[u8]| {
        let (bare, query) = path.split_once('?').unwrap_or((path, ""));
        if let Some(kind) = offline::classify(bare, &comms_root) {
            client.with_store(id, |store| offline::keep(store, kind, bare, query, body));
        }
    };
    let cancelled = || client.mirror.cancelled(generation);
    let job = Job {
        transport: &transport,
        keep: &keep,
        comms_root: comms_root.clone(),
        mode: client.mode_of(id),
        conf: client.conf().clone(),
        now_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX)),
        cancelled: &cancelled,
        pause: PAUSE,
        heads_only,
    };
    let started = Instant::now();
    let ended = run(&job);
    tracing::info!(
        "Mirror of {id} ({}{}): {ended:?} in {}s",
        job.mode.as_str(),
        if heads_only { ", heads" } else { "" },
        started.elapsed().as_secs()
    );
    ended
}

/// The account's server through the proxy, with its key, as the shell's requests go.
struct ProxyTransport<'a> {
    host: &'a dyn ClientHost,
    client: &'a Client,
    account: &'a Account,
    key: &'a str,
}

impl Transport for ProxyTransport<'_> {
    fn get(&self, path: &str) -> Fetched {
        let request = Request {
            method: "GET".to_string(),
            path: path.to_string(),
            headers: vec![("Accept".to_string(), "application/json".to_string())],
            body: Vec::new(),
        };
        let upstream = match self
            .client
            .proxy
            .send(&self.account.server, Some(self.key), SHELL_ORIGIN, request)
        {
            Ok(upstream) => upstream,
            Err(err) => {
                tracing::info!("Mirror: {} {path}: {err}", self.account.server);
                return Fetched::Offline;
            }
        };
        let status = upstream.status;
        if status == 401 {
            self.host.check_key(self.account);
            return Fetched::Unauthorized;
        }
        let json = header(&upstream.headers, "content-type").is_some_and(|kind| kind.contains("json"));
        if status != 200 || !json {
            return Fetched::Failed(status);
        }
        match upstream.body() {
            Ok(body) => Fetched::Json(body),
            Err(err) => {
                tracing::info!("Mirror: {path} was cut off: {err}");
                Fetched::Offline
            }
        }
    }
}

#[cfg(test)]
#[path = "mirror_tests.rs"]
mod tests;
