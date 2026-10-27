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

//! The Postgres `NOTIFY` cluster channel (`cluster_transport=notify`, `plans/archive/Cluster.md`), for members that share the
//! entity database and cannot open a connection to each other.
//!
//! A notify member advertises `pg-notify:`. The core fan-out posts once to that base URL for all of them, and the
//! dispatching sender here routes it to `pg_notify('moqui_cluster', ...)` on a pool connection. Other base URLs go to
//! the HTTPS or the SSH sender, so members on different transports still reach each other. The payload is the topic, the
//! origin, and the same JSON as the HTTPS body, one per line. A message past Postgres's 8000-byte payload limit is kept
//! in a `moqui.server.ClusterMessage` row and the payload carries `@` and its id.
//!
//! A notify member holds one connection outside the pool in `LISTEN`, on a thread of its own, and applies what arrives
//! through the same path as a `POST /cluster/{topic}`. A notification sent while that connection is down is lost, as an
//! HTTPS post that fails every retry is; after it connects again the listener clears the entity caches, so a missed
//! invalidation leaves no stale row.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use moqui_core::cluster::{Sender, NOTIFY_BASE_URL};
use moqui_core::context::{Error as CoreError, ExecutionContextFactory};
use moqui_lib::error::Error;

/// The `LISTEN` channel.
pub const CHANNEL: &str = "moqui_cluster";
/// Payloads up to this many bytes go in the `NOTIFY` itself. Postgres refuses 8000 and more.
const INLINE_LIMIT: usize = 7_900;
/// How long one wait for notifications lasts, which is also how soon a stop is seen.
const WAIT: Duration = Duration::from_millis(500);
const PRUNE_EVERY: Duration = Duration::from_secs(60);
const RECONNECT_MAX: Duration = Duration::from_secs(5);
/// How long one notification waits for room in the request queue.
const BUSY_PATIENCE: Duration = Duration::from_secs(30);

/// Install the sender that routes each peer's base URL to its transport.
///
/// # Errors
///
/// Returns an error when the cluster material is missing, or `cluster_transport` is `notify` and the entity database is
/// not Postgres.
pub(crate) fn install_sender(factory: &Arc<ExecutionContextFactory>, entity_is_postgres: bool) -> Result<(), Error> {
    check_transport(notify_transport(), entity_is_postgres)?;
    let sender = DispatchSender {
        notify: entity_is_postgres.then(|| NotifySender {
            factory: Arc::downgrade(factory),
        }),
        ssh: ssh_sender(factory)?,
        https: crate::cluster_edge::https_sender()?,
    };
    moqui_core::cluster::install_sender(factory, Arc::new(sender));
    Ok(())
}

#[cfg(feature = "ssh")]
fn ssh_sender(factory: &Arc<ExecutionContextFactory>) -> Result<Arc<dyn Sender>, Error> {
    crate::ssh::cluster::sender(factory)
}

#[cfg(not(feature = "ssh"))]
fn ssh_sender(_factory: &Arc<ExecutionContextFactory>) -> Result<Arc<dyn Sender>, Error> {
    Ok(Arc::new(NoSsh))
}

/// A peer that advertises `ssh://` in a build without the `ssh` feature.
#[cfg(not(feature = "ssh"))]
struct NoSsh;

#[cfg(not(feature = "ssh"))]
impl Sender for NoSsh {
    fn post(
        &self,
        base_url: &str,
        _topic: &str,
        _payload: &[u8],
    ) -> Result<moqui_core::cluster::PostOutcome, CoreError> {
        Err(CoreError::new(format!(
            "cluster peer {base_url} is reached over SSH and ssh is not built into this host"
        )))
    }
}

fn check_transport(notify: bool, entity_is_postgres: bool) -> Result<(), Error> {
    if notify && !entity_is_postgres {
        return Err(Error::new(
            "cluster_transport=notify needs a Postgres entity database; this one is MySQL",
        ));
    }
    Ok(())
}

/// True when this member advertises `pg-notify:` and listens.
pub(crate) fn notify_transport() -> bool {
    moqui_lib::env::get_prop_or_env("cluster_transport") == "notify"
}

struct DispatchSender {
    notify: Option<NotifySender>,
    ssh: Arc<dyn Sender>,
    https: Arc<dyn Sender>,
}

impl Sender for DispatchSender {
    fn post(&self, base_url: &str, topic: &str, payload: &[u8]) -> Result<moqui_core::cluster::PostOutcome, CoreError> {
        if base_url.starts_with(NOTIFY_BASE_URL) {
            return match &self.notify {
                Some(notify) => notify.send(topic, payload),
                None => Err(CoreError::new(
                    "a cluster peer listens on Postgres NOTIFY and this entity database is not Postgres",
                )),
            };
        }
        if base_url.starts_with("ssh://") {
            return self.ssh.post(base_url, topic, payload);
        }
        self.https.post(base_url, topic, payload)
    }
}

struct NotifySender {
    factory: Weak<ExecutionContextFactory>,
}

impl NotifySender {
    fn send(&self, topic: &str, payload: &[u8]) -> Result<moqui_core::cluster::PostOutcome, CoreError> {
        if payload.len() > moqui_core::cluster::MAX_CLUSTER_BODY {
            return Ok(moqui_core::cluster::PostOutcome::Rejected(413));
        }
        let factory = self
            .factory
            .upgrade()
            .ok_or_else(|| CoreError::new("the server is stopping"))?;
        let facade = factory
            .entity()
            .ok_or_else(|| CoreError::new("the entity facade is not installed"))?;
        let json = std::str::from_utf8(payload).map_err(|_| CoreError::new("cluster payload is not UTF-8"))?;
        let origin = moqui_core::notify::origin_id();
        let mut message = format!("{topic}\n{origin}\n{json}");
        if message.len() > INLINE_LIMIT {
            let id = moqui_core::cluster::store_message(&facade, topic, json)?;
            message = format!("{topic}\n{origin}\n@{id}");
        }
        let tx = facade
            .transactions_for("moqui.server.ClusterMessage")
            .map_err(CoreError::from)?;
        let params = [
            moqui_lib::object::Value::string(CHANNEL),
            moqui_lib::object::Value::string(message),
        ];
        tx.with_connection(|conn| {
            conn.query_once("SELECT pg_notify(?, ?)", &params)
                .map(|_| moqui_core::cluster::PostOutcome::Accepted)
        })
        .map_err(CoreError::from)
    }
}

/// The `LISTEN` thread. Dropping it stops the thread and closes its connection.
pub(crate) struct NotifyListener {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for NotifyListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// With `cluster_transport=notify`, connect, `LISTEN`, and start the thread that applies what arrives. `None`
/// otherwise.
///
/// # Errors
///
/// Returns an error when the transport is notify and the entity database is not Postgres, or the first connection or
/// `LISTEN` fails.
#[cfg(feature = "postgres")]
pub(crate) fn listen_if_configured(
    factory: &Arc<ExecutionContextFactory>,
    db: &crate::db::entity_db::EntityDb,
    posts: Arc<Mutex<Vec<String>>>,
) -> Result<Option<NotifyListener>, Error> {
    if !notify_transport() {
        return Ok(None);
    }
    let Some(driver) = db.postgres.clone() else {
        return Err(Error::new("cluster_transport=notify needs a Postgres entity database"));
    };
    let first = connect_listen(&driver)?;
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let weak = Arc::downgrade(factory);
    let thread = std::thread::Builder::new()
        .name("moqui-cluster-listen".to_string())
        .spawn(move || listen_loop(&driver, first, &weak, &posts, &thread_stop))
        .map_err(|err| Error::new("could not start the cluster listen thread").with_source(err))?;
    Ok(Some(NotifyListener {
        stop,
        thread: Some(thread),
    }))
}

#[cfg(not(feature = "postgres"))]
pub(crate) fn listen_if_configured(
    _factory: &Arc<ExecutionContextFactory>,
    _db: &crate::db::entity_db::EntityDb,
    _posts: Arc<Mutex<Vec<String>>>,
) -> Result<Option<NotifyListener>, Error> {
    if notify_transport() {
        return Err(Error::new("the postgres driver is not in this build"));
    }
    Ok(None)
}

#[cfg(feature = "postgres")]
fn connect_listen(driver: &crate::db::postgres::PostgresDriver) -> Result<crate::db::postgres::PostgresConn, Error> {
    let mut conn = driver.connect_postgres()?;
    conn.listen(CHANNEL)?;
    Ok(conn)
}

#[cfg(feature = "postgres")]
fn listen_loop(
    driver: &crate::db::postgres::PostgresDriver,
    first: crate::db::postgres::PostgresConn,
    factory: &Weak<ExecutionContextFactory>,
    posts: &Mutex<Vec<String>>,
    stop: &AtomicBool,
) {
    let mut conn = Some(first);
    let mut backoff = Duration::from_millis(100);
    let mut last_prune = Instant::now();
    while !stop.load(Ordering::Acquire) {
        let Some(live) = conn.as_mut() else {
            std::thread::sleep(backoff.min(WAIT));
            match connect_listen(driver) {
                Ok(fresh) => {
                    conn = Some(fresh);
                    backoff = Duration::from_millis(100);
                    if let Some(factory) = factory.upgrade() {
                        clear_entity_caches(&factory);
                    }
                }
                Err(err) => {
                    backoff = (backoff * 2).min(RECONNECT_MAX);
                    tracing::warn!("cluster LISTEN could not reconnect: {err}");
                }
            }
            continue;
        };
        match live.notifications(WAIT) {
            Ok(payloads) => {
                let Some(factory) = factory.upgrade() else {
                    return;
                };
                for payload in payloads {
                    apply(&factory, posts, &payload, stop);
                }
            }
            Err(err) => {
                tracing::warn!("cluster LISTEN connection was lost: {err}");
                conn = None;
            }
        }
        if last_prune.elapsed() >= PRUNE_EVERY {
            last_prune = Instant::now();
            if let Some(factory) = factory.upgrade() {
                if let Some(facade) = factory.entity() {
                    if let Err(err) = moqui_core::cluster::prune_messages(&factory, &facade) {
                        tracing::warn!("cluster message cleanup failed: {err}");
                    }
                }
            }
        }
    }
}

/// One `NOTIFY` payload: skipped when this member sent it, else applied as a `POST /cluster/{topic}` would be.
///
/// No sender is told to try again, so a full request queue is waited out on this thread, which is the listener's own;
/// an invalidation still refused after [`BUSY_PATIENCE`] clears every entity list cache instead.
fn apply(factory: &Arc<ExecutionContextFactory>, posts: &Mutex<Vec<String>>, payload: &str, stop: &AtomicBool) {
    let mut lines = payload.splitn(3, '\n');
    let (Some(topic), Some(origin), Some(body)) = (lines.next(), lines.next(), lines.next()) else {
        tracing::warn!("cluster NOTIFY payload is not topic, origin and body");
        return;
    };
    if origin == moqui_core::notify::origin_id() {
        return;
    }
    let kept;
    let body = if let Some(id) = body.strip_prefix('@') {
        let Some(facade) = factory.entity() else {
            return;
        };
        match moqui_core::cluster::load_message(&facade, id) {
            Ok(Some((_, payload))) => {
                kept = payload;
                kept.as_str()
            }
            Ok(None) => {
                tracing::warn!("cluster message {id} was gone before this member read it");
                return;
            }
            Err(err) => {
                tracing::warn!("cluster message {id} could not be read: {err}");
                return;
            }
        }
    } else {
        body
    };
    let status =
        crate::cluster_edge::apply_topic_patiently(factory, posts, topic, body.as_bytes(), stop, BUSY_PATIENCE);
    if status != 204 {
        tracing::warn!("cluster NOTIFY for {topic} was not applied: status {status}");
    }
    if status == 503 && topic == moqui_core::notify::ENTITY_CACHE_INVALIDATE {
        clear_entity_caches(factory);
    }
}

/// Clear every entity list cache, for an invalidation that may have been missed.
fn clear_entity_caches(factory: &ExecutionContextFactory) {
    if let Some(facade) = factory.entity() {
        if let Err(err) = moqui_core::entity::cache::clear_all_list_caches(&facade) {
            tracing::warn!("could not clear the entity caches after a missed cluster invalidation: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::check_transport;

    #[test]
    fn notify_refuses_oversized_payloads_before_trying_to_store_them() {
        let sender = super::NotifySender {
            factory: std::sync::Weak::new(),
        };
        assert_eq!(
            sender
                .send("collab-update", &vec![b'x'; moqui_core::cluster::MAX_CLUSTER_BODY + 1])
                .unwrap(),
            moqui_core::cluster::PostOutcome::Rejected(413)
        );
    }

    #[test]
    fn notify_needs_a_postgres_entity_database() {
        let err = check_transport(true, false).unwrap_err();
        assert!(err.message().contains("MySQL"), "{err}");
        assert!(check_transport(true, true).is_ok());
        assert!(check_transport(false, false).is_ok());
    }
}
