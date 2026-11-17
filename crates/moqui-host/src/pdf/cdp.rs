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

//! The DevTools Protocol over the browser's pipe (on Unix: its descriptors 3 and 4, each message JSON ending in a NUL;
//! SEC-SCR-10), or over its WebSocket elsewhere: commands with an id and a reply, and events routed by the flat session
//! they belong to. Messages stay `serde_json::Value`, so the workspace's `arbitrary_precision` numbers parse.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moqui_lib::error::Error;
use moqui_lib::sync::lock;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

/// Largest message the browser may send: a PDF comes back as one base64 string.
const MAX_MESSAGE: usize = 512 * 1024 * 1024;

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, Error>>>>>;

/// Events kept for one session before it listens.
const EARLY_EVENTS: usize = 256;
/// Sessions that may have events waiting for a listener at once; a page listens as soon as it is created.
const EARLY_SESSIONS: usize = 64;
/// Forgotten sessions remembered, so their late events (such as `Inspector.detached` after `closeTarget`) are dropped.
const FORGOTTEN: usize = 256;

/// Events of one session, and the events that arrived before anyone listened.
#[derive(Default)]
struct Routes {
    listeners: HashMap<String, mpsc::UnboundedSender<Value>>,
    early: HashMap<String, Vec<Value>>,
    /// The newest last.
    forgotten: VecDeque<String>,
}

impl Routes {
    /// Keep `event` of `session` for its listener, unless the session was forgotten or too many sessions wait.
    fn keep_early(&mut self, session: &str, event: Value) {
        if self.forgotten.iter().any(|gone| gone == session) {
            return;
        }
        if !self.early.contains_key(session) && self.early.len() >= EARLY_SESSIONS {
            tracing::debug!(session, "DevTools event dropped: too many sessions without a listener");
            return;
        }
        let early = self.early.entry(session.to_string()).or_default();
        if early.len() < EARLY_EVENTS {
            early.push(event);
        }
    }

    fn forget(&mut self, session: &str) {
        self.listeners.remove(session);
        self.early.remove(session);
        if self.forgotten.len() >= FORGOTTEN {
            self.forgotten.pop_front();
        }
        self.forgotten.push_back(session.to_string());
    }
}

/// A connected browser. Cloning shares the socket.
#[derive(Clone)]
pub(super) struct Connection {
    outgoing: mpsc::UnboundedSender<String>,
    pending: Pending,
    routes: Arc<Mutex<Routes>>,
    next_id: Arc<AtomicU64>,
    closed: Arc<AtomicBool>,
}

impl Connection {
    /// A connection with no transport yet, and the queue of the messages it sends.
    fn parts() -> (Self, mpsc::UnboundedReceiver<String>) {
        let (outgoing, queue) = mpsc::unbounded_channel::<String>();
        let connection = Self {
            outgoing,
            pending: Arc::default(),
            routes: Arc::default(),
            next_id: Arc::new(AtomicU64::new(1)),
            closed: Arc::new(AtomicBool::new(false)),
        };
        (connection, queue)
    }

    /// Talk to the browser over its DevTools pipe: `commands` is its descriptor 3, `answers` its descriptor 4. A thread
    /// writes each message with its NUL and one reads them; the connection closes when the browser closes its end.
    ///
    /// # Errors
    ///
    /// Returns an error when a thread cannot be started.
    #[cfg(unix)]
    pub(super) fn over_pipe(
        mut commands: std::process::ChildStdin,
        answers: std::process::ChildStdout,
    ) -> Result<Self, Error> {
        use std::io::{BufRead as _, Write as _};
        let (connection, mut queue) = Self::parts();
        std::thread::Builder::new()
            .name("pdf-devtools-out".into())
            .spawn(move || {
                while let Some(text) = queue.blocking_recv() {
                    let sent = commands
                        .write_all(text.as_bytes())
                        .and_then(|()| commands.write_all(b"\0"))
                        .and_then(|()| commands.flush());
                    if sent.is_err() {
                        break;
                    }
                }
            })
            .map_err(|err| Error::new(format!("could not start the browser's writer: {err}")))?;
        let reader = connection.clone();
        std::thread::Builder::new()
            .name("pdf-devtools-in".into())
            .spawn(move || {
                let mut input = std::io::BufReader::new(answers);
                let mut message = Vec::new();
                loop {
                    message.clear();
                    match input.read_until(0, &mut message) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    if message.last() == Some(&0) {
                        message.pop();
                    }
                    if message.len() > MAX_MESSAGE {
                        tracing::warn!("a DevTools message of {} bytes is over the limit", message.len());
                        break;
                    }
                    match serde_json::from_slice::<Value>(&message) {
                        Ok(value) => reader.dispatch(value),
                        Err(err) => tracing::debug!("unreadable DevTools message: {err}"),
                    }
                }
                reader.close();
            })
            .map_err(|err| Error::new(format!("could not start the browser's reader: {err}")))?;
        Ok(connection)
    }

    /// Connect to the browser socket. Runs the reader and the writer as tasks on the current runtime.
    #[cfg(not(unix))]
    pub(super) async fn connect(url: &str) -> Result<Self, Error> {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
        use tokio_tungstenite::tungstenite::Message;
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE))
            .max_frame_size(Some(MAX_MESSAGE));
        let (socket, _) = tokio_tungstenite::connect_async_with_config(url, Some(config), true)
            .await
            .map_err(|err| Error::new(format!("could not connect to the browser at {url}: {err}")))?;
        let (mut sink, mut stream) = socket.split();
        let (connection, mut queue) = Self::parts();
        tokio::spawn(async move {
            while let Some(text) = queue.recv().await {
                if sink.send(Message::text(text)).await.is_err() {
                    break;
                }
            }
        });
        let reader = connection.clone();
        tokio::spawn(async move {
            while let Some(message) = stream.next().await {
                let text = match message {
                    Ok(Message::Text(text)) => text.to_string(),
                    Ok(Message::Binary(bytes)) => String::from_utf8_lossy(&bytes).into_owned(),
                    Ok(Message::Close(_)) | Err(_) => break,
                    Ok(_) => continue,
                };
                match serde_json::from_str::<Value>(&text) {
                    Ok(value) => reader.dispatch(value),
                    Err(err) => tracing::debug!("unreadable DevTools message: {err}"),
                }
            }
            reader.close();
        });
        Ok(connection)
    }

    /// True once the socket closed: the browser exited or dropped the connection.
    pub(super) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Send `method` to the browser, or to a page when `session` is set, and wait for its result.
    pub(super) async fn call(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, Error> {
        if self.is_closed() {
            return Err(Error::new("the browser connection is closed"));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut message = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session {
            message["sessionId"] = Value::String(session.to_string());
        }
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(id, tx);
        if self.outgoing.send(message.to_string()).is_err() {
            lock(&self.pending).remove(&id);
            return Err(Error::new("the browser connection is closed"));
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(Error::new(format!("the browser closed before answering {method}"))),
            Err(_) => {
                lock(&self.pending).remove(&id);
                Err(Error::new(format!(
                    "the browser did not answer {method} in {} seconds",
                    timeout.as_secs()
                )))
            }
        }
    }

    /// The events of `session`, including any that arrived before this call.
    pub(super) fn events(&self, session: &str) -> mpsc::UnboundedReceiver<Value> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut routes = lock(&self.routes);
        for event in routes.early.remove(session).unwrap_or_default() {
            let _ = tx.send(event);
        }
        routes.listeners.insert(session.to_string(), tx);
        rx
    }

    /// Stop routing the events of `session`.
    pub(super) fn forget(&self, session: &str) {
        lock(&self.routes).forget(session);
    }

    fn dispatch(&self, message: Value) {
        if let Some(id) = message.get("id").and_then(Value::as_u64) {
            let waiter = lock(&self.pending).remove(&id);
            if let Some(waiter) = waiter {
                let result = match message.get("error") {
                    Some(error) => Err(Error::new(format!(
                        "DevTools error: {}",
                        error.get("message").and_then(Value::as_str).unwrap_or("unknown")
                    ))),
                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = waiter.send(result);
            }
            return;
        }
        let Some(session) = message.get("sessionId").and_then(Value::as_str).map(str::to_string) else {
            return;
        };
        let session = session.as_str();
        let mut routes = lock(&self.routes);
        match routes.listeners.get(session) {
            Some(listener) => {
                let _ = listener.send(message);
            }
            // Events before the page's listener exists, or after it was forgotten.
            None => routes.keep_early(session, message),
        }
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        let waiters: Vec<_> = lock(&self.pending).drain().map(|(_, waiter)| waiter).collect();
        for waiter in waiters {
            let _ = waiter.send(Err(Error::new("the browser connection closed")));
        }
        lock(&self.routes).listeners.clear();
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Routes, EARLY_SESSIONS};

    #[test]
    fn events_of_a_forgotten_session_are_dropped_and_waiting_sessions_are_capped() {
        let mut routes = Routes::default();
        routes.keep_early("page-1", json!({ "method": "Page.loadEventFired" }));
        assert_eq!(routes.early.get("page-1").map(Vec::len), Some(1));
        routes.forget("page-1");
        routes.keep_early("page-1", json!({ "method": "Inspector.detached" }));
        assert!(routes.early.is_empty());

        for index in 0..EARLY_SESSIONS + 10 {
            routes.keep_early(&format!("never-{index}"), json!({}));
        }
        assert_eq!(routes.early.len(), EARLY_SESSIONS);
        routes.keep_early("never-0", json!({}));
        assert_eq!(routes.early.get("never-0").map(Vec::len), Some(2));
    }
}
