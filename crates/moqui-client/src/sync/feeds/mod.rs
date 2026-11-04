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
//! What a folded change leaves to read (the room list, a room, a room's latest page, a thread) is kept in the store as
//! a pending read ([`note_pending`]) before the cursor moves past it, and read at the start and the end of each run
//! until it answers (or fails [`PENDING_TRIES`] times): a stop (offline, a refused key) or an error loses none. A
//! change that does not fold has its room or thread read again the same way.
//!
//! They run when a page's `/notws` opens (the mirror's run, or a run of their own when the mirror does not start), on a
//! gap in the pushes' numbers (`push::keep_frame`, chat only: [`Scope::Chat`]), and in the mirror. A `reset` drops the
//! cached reads the feed covers, with the new cursor in one unit, and has the mirror read them again; a first read (no
//! cursor yet) only takes the cursor, as the mirror reads everything then. The mirror and the feeds of one account
//! run one at a time ([`Feeds`]), so an older page of the mirror never lands over a newer change.

mod chat;
mod mail;

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::flush::StoreAccess;
use super::mirror::{self, encode, Ended, Fetched, Transport};
use crate::app::{Client, ClientHost};
use crate::login::ServerSync;
use crate::offline::{self, chat_base, mail_base};
use crate::store::{Store, SyncCursor};

pub(crate) use chat::forget_room;

/// The cursor of the user's chat feed.
pub(crate) const CHAT_CURSOR: &str = "chat";
/// The `headsSince` the chat feed's last page gave.
pub(crate) const HEADS_CURSOR: &str = "chat-heads";
/// The cursor of the mail feed.
pub(crate) const MAIL_CURSOR: &str = "mail";
/// The value of the mail cursor when the server keeps no mail feed (an app's inbox source answers without one).
pub(crate) const UNSUPPORTED: &str = "unsupported";
/// The prefix of a pending read's cursor: `pending:list`, `pending:room:<id>`, `pending:latest:<id>`,
/// `pending:thread:<store>:<id>`; its seq is the tries so far.
pub(crate) const PENDING: &str = "pending:";
/// The tries of a pending read before it is given up (logged).
pub(crate) const PENDING_TRIES: i64 = 5;

/// Changes asked for in one page.
const LIMIT: u32 = 200;
/// Pages of one feed in a run.
const MAX_PAGES: usize = 50;
/// Requests of a run in all.
const MAX_REQUESTS: usize = 1500;
/// The pause between two requests of a run.
const PAUSE: Duration = Duration::from_millis(50);
/// Events of a room's latest page read again.
const PAGE: u32 = 50;

/// Which feeds a run reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    /// Chat only: a gap in the chat pushes.
    Chat,
    /// Chat and mail, and the stores the mail cursor does not cover: a `/notws` opening, the mirror.
    All,
}

/// What a run reads, and from where.
pub(crate) struct Job<'a> {
    pub(crate) transport: &'a dyn Transport,
    pub(crate) store: &'a dyn StoreAccess,
    pub(crate) comms_root: String,
    pub(crate) server: ServerSync,
    pub(crate) cancelled: &'a dyn Fn() -> bool,
    pub(crate) pause: Duration,
    pub(crate) scope: Scope,
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
    /// The rooms whose held messages the room feeds changed (a message read again or redacted, a room's events
    /// dropped): a page showing one reads it again (the pages' `rooms` event).
    pub(crate) rooms_changed: std::collections::BTreeSet<String>,
    /// A reset of a chat cursor the store had dropped what the feed covers.
    pub(crate) chat_dropped: bool,
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
    /// the others. Whether it was done.
    fn fold(&self, what: &str, work: impl FnOnce(&Store) -> Result<(), String>) -> bool {
        match self.store(work) {
            Some(Ok(())) => true,
            Some(Err(err)) => {
                tracing::warn!("Feeds: {what} not folded: {err}");
                false
            }
            None => {
                tracing::warn!("Feeds: {what} not folded: the store is not open");
                false
            }
        }
    }

    /// Note a pending read (logged when it cannot be).
    fn pending(&self, what: &str) {
        self.fold(&format!("the pending read {what}"), |store| note_pending(store, what));
    }

    /// Read the pending reads of `kinds` (`list`, `room`, `latest`, `thread`), in that order; each one that answers is
    /// done, one that does not is tried again next time, up to [`PENDING_TRIES`].
    fn drain(&mut self, kinds: &[&str]) -> Result<(), Ended> {
        let chat = chat_base(&self.job.comms_root);
        let mail = mail_base(&self.job.comms_root);
        let mut pending = self.store(|store| store.sync_cursors(PENDING)).unwrap_or_default();
        let order = |name: &str| {
            kinds
                .iter()
                .position(|kind| name.starts_with(&format!("{PENDING}{kind}")))
        };
        pending.retain(|(name, _)| order(name).is_some());
        pending.sort_by_key(|(name, _)| order(name));
        for (name, cursor) in pending {
            let what = name.strip_prefix(PENDING).unwrap_or(&name);
            let path = if what == "list" {
                format!("{chat}/chatData?limit=200")
            } else if let Some(room) = what.strip_prefix("room:") {
                format!("{chat}/room?chatRoomId={}", encode(room))
            } else if let Some(room) = what.strip_prefix("latest:") {
                format!("{chat}/timeline?chatRoomId={}&limit={PAGE}", encode(room))
            } else if let Some((store_name, thread)) =
                what.strip_prefix("thread:").and_then(|rest| rest.split_once(':'))
            {
                format!("{mail}/thread?store={}&threadId={}", encode(store_name), encode(thread))
            } else {
                self.fold("an unknown pending read", |store| {
                    store.remove_sync_cursor(&name).map_err(|err| err.to_string())
                });
                continue;
            };
            let answered = self.keep(&path)?.is_some();
            let tries = cursor.seq.unwrap_or(0) + 1;
            self.fold("a pending read", |store| {
                if answered || tries >= PENDING_TRIES {
                    if !answered {
                        tracing::warn!("Feeds: {path} given up after {tries} tries");
                    }
                    store.remove_sync_cursor(&name).map_err(|err| err.to_string())
                } else {
                    store.set_sync_seq(&name, tries).map_err(|err| err.to_string())
                }
            });
        }
        Ok(())
    }
}

/// Note a read that a folded change leaves (`list`, `room:<id>`, `latest:<id>`, `thread:<store>:<id>`), kept in the
/// store until a run reads it; one noted already keeps its tries.
///
/// # Errors
///
/// Returns an error when the row cannot be written.
pub(crate) fn note_pending(store: &Store, what: &str) -> Result<(), String> {
    let name = format!("{PENDING}{what}");
    if store.sync_cursor(&name).is_some() {
        return Ok(());
    }
    store
        .set_sync_cursor(
            &name,
            &SyncCursor {
                seq: Some(0),
                value: None,
            },
        )
        .map_err(|err| err.to_string())
}

/// Read the feeds of `job`: chat, then (in [`Scope::All`]) mail. A server below `clientApi` 4 has none: nothing is read.
pub(crate) fn run(job: &Job<'_>) -> Synced {
    let mut run = Run {
        job,
        synced: Synced::default(),
    };
    if !job.server.syncs() {
        return run.synced;
    }
    let ended = chat::chat(&mut run).and_then(|()| {
        if job.scope == Scope::All {
            mail::mail(&mut run)
        } else {
            Ok(())
        }
    });
    if let Err(ended) = ended {
        run.synced.ended = Some(ended);
    }
    run.synced
}

/// Whether a query (of an answer's key) names `room` as its `chatRoomId`.
fn names_room(key: &str, room: &str) -> bool {
    let query = key.split_once('?').map_or("", |(_, query)| query);
    let wanted = format!("chatRoomId={}", encode(room));
    query
        .split('&')
        .any(|parameter| parameter == wanted || parameter == format!("chatRoomId={room}"))
}

/// The feeds runs of the client, and the mirror's: one at a time per account, and one more when asked while one runs
/// (of the widest scope asked).
#[derive(Default)]
pub struct Feeds {
    /// The accounts with a run under way, and the scope of another asked for meanwhile.
    running: Mutex<HashMap<String, Option<Scope>>>,
    freed: Condvar,
}

impl Feeds {
    /// Whether a run for `account` may start now; when one is under way it is asked to run once more, of `scope`.
    pub(crate) fn begin(&self, account: &str, scope: Scope) -> bool {
        let mut running = moqui_lib::sync::lock(&self.running);
        if let Some(again) = running.get_mut(account) {
            *again = Some(again.map_or(scope, |kept| kept.max(scope)));
            return false;
        }
        running.insert(account.to_string(), None);
        true
    }

    /// Wait until no run of `account` is under way, then mark one (the mirror's); false when `cancelled` first.
    pub(crate) fn acquire(&self, account: &str, cancelled: &dyn Fn() -> bool) -> bool {
        let mut running = moqui_lib::sync::lock(&self.running);
        while running.contains_key(account) {
            if cancelled() {
                return false;
            }
            running = moqui_lib::sync::wait_timeout(&self.freed, running, Duration::from_millis(200)).0;
        }
        running.insert(account.to_string(), None);
        true
    }

    /// A run for `account` ended: the scope of another asked for meanwhile (it stays under way), else none and it is
    /// done.
    pub(crate) fn finish(&self, account: &str) -> Option<Scope> {
        let mut running = moqui_lib::sync::lock(&self.running);
        match running.get_mut(account) {
            Some(again @ Some(_)) => again.take(),
            _ => {
                running.remove(account);
                self.freed.notify_all();
                None
            }
        }
    }

    /// The run for `account` stopped early (offline, a refused key, a shutdown): no longer under way, whatever was
    /// asked meanwhile (the next `/notws` opening runs again).
    pub(crate) fn end(&self, account: &str) {
        moqui_lib::sync::lock(&self.running).remove(account);
        self.freed.notify_all();
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

/// Read the feeds of the account `id` (all of them) on a thread of their own: a `/notws` opening the mirror did not
/// take. One under way runs again instead. After a reset the mirror reads what it dropped.
pub fn start(client: &Arc<Client>, host: &Arc<dyn ClientHost>, id: &str) {
    start_scope(client, host, id, Scope::All);
}

/// [`start`] of `scope`: [`Scope::Chat`] for a gap in the chat pushes.
pub fn start_scope(client: &Arc<Client>, host: &Arc<dyn ClientHost>, id: &str, scope: Scope) {
    if client.is_stopped() || !server_of(client, id).syncs() || !client.feeds.begin(id, scope) {
        return;
    }
    let thread_client = Arc::clone(client);
    let thread_host = Arc::clone(host);
    let thread_id = id.to_string();
    let started = std::thread::Builder::new()
        .name("client-feeds".to_string())
        .spawn(move || {
            let mut scope = scope;
            loop {
                let synced = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_for(thread_host.as_ref(), &thread_client, &thread_id, scope)
                }))
                .unwrap_or_else(|_| {
                    tracing::error!("The feeds of {thread_id} panicked");
                    Synced {
                        ended: Some(Ended::Cancelled),
                        ..Synced::default()
                    }
                });
                if synced.chat_reread || synced.mail_reread {
                    // read again by a full run: now, or after the one under way (Mirror::want_full)
                    thread_client.mirror.want_full(&thread_id);
                }
                if synced.ended.is_some() || thread_client.is_stopped() {
                    thread_client.feeds.end(&thread_id);
                    break;
                }
                match thread_client.feeds.finish(&thread_id) {
                    Some(again) => scope = again,
                    None => break,
                }
            }
            if thread_client.mirror.is_wanted(&thread_id) {
                mirror::start(&thread_client, &thread_host, &thread_id);
            }
        });
    if let Err(err) = started {
        tracing::error!("The feeds of {id} did not start: {err}");
        client.feeds.end(id);
    }
}

/// One run of the feeds of the account `id`, with its key and comms app; cancelled by the mirror's cancel (an account
/// switch, a sign-out). The caller holds the account's [`Feeds`] mark.
pub(crate) fn run_for(host: &dyn ClientHost, client: &Client, id: &str, scope: Scope) -> Synced {
    let Some((account, Some(key))) = client.account(id) else {
        return Synced {
            ended: Some(Ended::Cancelled),
            ..Synced::default()
        };
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
        scope,
    };
    let started = Instant::now();
    let synced = run(&job);
    tracing::info!(
        "Feeds of {id} ({scope:?}): {} requests in {}ms{}{}",
        synced.requests,
        started.elapsed().as_millis(),
        if synced.chat_reread { ", chat to read again" } else { "" },
        if synced.mail_reread { ", mail to read again" } else { "" }
    );
    if let Some(event) = rooms_event(&synced) {
        host.notify_pages(id, &event);
    }
    synced
}

/// The pages' `rooms` event of a run that changed held messages: `{kind: "rooms", rooms, reset}`, `reset` when the
/// chat feed was reset (every room's held messages may be stale). None when nothing a page shows changed.
pub(crate) fn rooms_event(synced: &Synced) -> Option<Value> {
    let reset = synced.chat_dropped;
    if synced.rooms_changed.is_empty() && !reset {
        return None;
    }
    Some(serde_json::json!({ "kind": "rooms", "rooms": synced.rooms_changed, "reset": reset }))
}

#[cfg(test)]
mod tests;
