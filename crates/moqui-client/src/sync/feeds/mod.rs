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

//! The server's changes feeds folded into an account's store (plans/Mobile-Desktop.md §6, MD5; the contracts are
//! docs/ui/Desktop-Client.md "The server's sync contracts"), for a server at `clientApi` 4. Each feed is read after the
//! cursor the store keeps (`Store::sync_cursor`), page by page, and the cursor moves on once a page is folded:
//!
//! - [`chat`]: the user's feed (`chatChanges`), then each held room's message changes (`roomChanges`).
//! - [`mail`]: the mail change cursor (`mailChanges`), and the stores it does not cover read again.
//!
//! They run when a page's `/notws` opens (the mirror's run, or a run of their own when the mirror does not start), on a
//! gap in the pushes' numbers (`push::keep_frame`), and in the mirror. A `reset` drops the cached reads the feed
//! covers and has the mirror read them again; a first read (no cursor yet) only takes the cursor, as the mirror reads
//! everything then. A run is bounded, stops when offline or the key is refused, and logs and passes over a change
//! that does not fold.

mod chat;
mod mail;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::flush::StoreAccess;
use super::mirror::{self, Ended, Fetched, Transport};
use crate::app::{Client, ClientHost};
use crate::login::ServerSync;
use crate::offline;
use crate::store::Store;

/// The cursor of the user's chat feed.
pub(crate) const CHAT_CURSOR: &str = "chat";
/// The `headsSince` the chat feed's last page gave.
pub(crate) const HEADS_CURSOR: &str = "chat-heads";
/// The cursor of the mail feed.
pub(crate) const MAIL_CURSOR: &str = "mail";
/// The value of the mail cursor when the server keeps no mail feed (an app's inbox source answers without one).
pub(crate) const UNSUPPORTED: &str = "unsupported";

/// Changes asked for in one page.
const LIMIT: u32 = 200;
/// Pages of one feed in a run.
const MAX_PAGES: usize = 50;
/// Requests of a run in all.
const MAX_REQUESTS: usize = 1500;
/// The pause between two requests of a run.
const PAUSE: Duration = Duration::from_millis(50);

/// What a run reads, and from where.
pub(crate) struct Job<'a> {
    pub(crate) transport: &'a dyn Transport,
    pub(crate) store: &'a dyn StoreAccess,
    pub(crate) comms_root: String,
    pub(crate) server: ServerSync,
    pub(crate) cancelled: &'a dyn Fn() -> bool,
    pub(crate) pause: Duration,
}

/// What a run did.
#[derive(Debug, Default)]
pub(crate) struct Synced {
    /// Chat is to be read again by the mirror: a reset dropped its cached reads, or the feed was read the first time.
    pub(crate) chat_reread: bool,
    /// The same for mail.
    pub(crate) mail_reread: bool,
    /// The server keeps no mail feed: mail is read as before (MD4).
    pub(crate) mail_unsupported: bool,
    pub(crate) requests: usize,
    /// Why the run stopped early, if it did.
    pub(crate) ended: Option<Ended>,
}

/// A run's progress.
pub(super) struct Run<'j, 'a> {
    job: &'j Job<'a>,
    synced: Synced,
}

impl Run<'_, '_> {
    /// `GET path`: the body and JSON of a 200, none for another status (logged), a stop for the rest.
    fn fetch(&mut self, path: &str) -> Result<Option<(Vec<u8>, Value)>, Ended> {
        if (self.job.cancelled)() {
            return Err(Ended::Cancelled);
        }
        if self.synced.requests >= MAX_REQUESTS {
            return Err(Ended::Bounded);
        }
        if self.synced.requests > 0 && !self.job.pause.is_zero() {
            std::thread::sleep(self.job.pause);
        }
        self.synced.requests += 1;
        match self.job.transport.get(path) {
            Fetched::Json(body) => {
                let json = serde_json::from_slice(&body).unwrap_or(Value::Null);
                Ok(Some((body, json)))
            }
            Fetched::Failed(status) => {
                tracing::info!("Feeds: {path} answered {status}; passed over");
                Ok(None)
            }
            Fetched::Unauthorized => Err(Ended::Unauthorized),
            Fetched::Offline => Err(Ended::Offline),
        }
    }

    /// `GET path`, its JSON only (not kept as an answer).
    fn json(&mut self, path: &str) -> Result<Option<Value>, Ended> {
        Ok(self.fetch(path)?.map(|(_, json)| json))
    }

    /// `GET path`, its answer kept as the scheme keeps a page's (`offline::keep`: a timeline page, the room list, a
    /// room, a thread), so it shows offline.
    fn keep(&mut self, path: &str) -> Result<Option<Value>, Ended> {
        let Some((body, json)) = self.fetch(path)? else {
            return Ok(None);
        };
        let (bare, query) = path.split_once('?').unwrap_or((path, ""));
        if let Some(kind) = offline::classify(bare, &self.job.comms_root) {
            self.store(|store| offline::keep(store, kind, bare, query, &body));
        }
        Ok(Some(json))
    }

    /// `work` on the store; none when it is not open.
    fn store<T>(&self, work: impl FnOnce(&Store) -> T) -> Option<T> {
        let mut work = Some(work);
        let mut out = None;
        self.job.store.with_store(&mut |store| {
            if let Some(work) = work.take() {
                out = Some(work(store));
            }
        });
        out
    }

    /// `work` on the store, its error (or a closed store) logged as `what`'s: one change that does not fold never stops
    /// the others.
    fn fold(&self, what: &str, work: impl FnOnce(&Store) -> Result<(), String>) {
        match self.store(work) {
            Some(Ok(())) => {}
            Some(Err(err)) => tracing::warn!("Feeds: {what} not folded: {err}"),
            None => tracing::warn!("Feeds: {what} not folded: the store is not open"),
        }
    }
}

/// Read the feeds of `job`: chat, then mail. A server below `clientApi` 4 has none: nothing is read.
pub(crate) fn run(job: &Job<'_>) -> Synced {
    let mut run = Run {
        job,
        synced: Synced::default(),
    };
    if !job.server.syncs() {
        return run.synced;
    }
    let ended = chat::chat(&mut run).and_then(|()| mail::mail(&mut run));
    if let Err(ended) = ended {
        run.synced.ended = Some(ended);
    }
    run.synced
}

/// Whether a query (of an answer's key) names `room` as its `chatRoomId`.
fn names_room(key: &str, room: &str) -> bool {
    let query = key.split_once('?').map_or("", |(_, query)| query);
    let wanted = format!("chatRoomId={}", mirror::encode(room));
    query
        .split('&')
        .any(|parameter| parameter == wanted || parameter == format!("chatRoomId={room}"))
}

/// The feeds runs of the client: one at a time per account, and one more when asked while one runs.
#[derive(Default)]
pub struct Feeds {
    /// The accounts with a run under way, and whether another was asked for meanwhile.
    running: Mutex<HashMap<String, bool>>,
}

impl Feeds {
    /// Whether a run for `account` may start now; when one is under way it is asked to run once more.
    pub(crate) fn begin(&self, account: &str) -> bool {
        let mut running = moqui_lib::sync::lock(&self.running);
        if let Some(again) = running.get_mut(account) {
            *again = true;
            return false;
        }
        running.insert(account.to_string(), false);
        true
    }

    /// A run for `account` ended: true when another was asked for meanwhile (it stays under way), else it is done.
    pub(crate) fn finish(&self, account: &str) -> bool {
        let mut running = moqui_lib::sync::lock(&self.running);
        match running.get_mut(account) {
            Some(again) if *again => {
                *again = false;
                true
            }
            _ => {
                running.remove(account);
                false
            }
        }
    }
}

/// The server sync an account's server supports, from its kept client config.
pub fn server_of(client: &Client, id: &str) -> ServerSync {
    client
        .with_store(id, |store| store.answer("/qapps/clientConfig", ""))
        .flatten()
        .and_then(|config| serde_json::from_slice::<Value>(&config.body).ok())
        .map(|config| ServerSync::of(&config))
        .unwrap_or_default()
}

/// Read the feeds of the account `id` on a thread of their own (a push's gap, a `/notws` opening the mirror did not
/// take); one under way runs again instead. After a reset the mirror reads what it dropped.
pub fn start(client: &Arc<Client>, host: &Arc<dyn ClientHost>, id: &str) {
    if client.is_stopped() || !server_of(client, id).syncs() || !client.feeds.begin(id) {
        return;
    }
    let thread_client = Arc::clone(client);
    let thread_host = Arc::clone(host);
    let thread_id = id.to_string();
    let started = std::thread::Builder::new()
        .name("client-feeds".to_string())
        .spawn(move || loop {
            let synced = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_for(thread_host.as_ref(), &thread_client, &thread_id)
            }))
            .unwrap_or_else(|_| {
                tracing::error!("The feeds of {thread_id} panicked");
                Synced::default()
            });
            if synced.chat_reread || synced.mail_reread {
                thread_client.mirror.forget(&thread_id);
                mirror::start(&thread_client, &thread_host, &thread_id);
            }
            if thread_client.is_stopped() || !thread_client.feeds.finish(&thread_id) {
                return;
            }
        });
    if let Err(err) = started {
        tracing::error!("The feeds of {id} did not start: {err}");
        client.feeds.finish(id);
    }
}

/// One run of the feeds of the account `id`, with its key and comms app; cancelled by the mirror's cancel (an account
/// switch, a sign-out).
pub(crate) fn run_for(host: &dyn ClientHost, client: &Client, id: &str) -> Synced {
    let Some((account, Some(key))) = client.account(id) else {
        return Synced::default();
    };
    let transport = mirror::ProxyTransport {
        host,
        client,
        account: &account,
        key: &key,
    };
    let store = super::flush::AccountStore { client, id };
    let generation = client.mirror.generation();
    let cancelled = || client.mirror.cancelled(generation) || client.is_stopped();
    let job = Job {
        transport: &transport,
        store: &store,
        comms_root: client.comms_root_of(id),
        server: server_of(client, id),
        cancelled: &cancelled,
        pause: PAUSE,
    };
    let started = Instant::now();
    let synced = run(&job);
    tracing::info!(
        "Feeds of {id}: {} requests in {}ms{}{}",
        synced.requests,
        started.elapsed().as_millis(),
        if synced.chat_reread { ", chat to read again" } else { "" },
        if synced.mail_reread { ", mail to read again" } else { "" }
    );
    synced
}

#[cfg(test)]
mod tests;
