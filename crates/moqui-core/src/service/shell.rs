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

//! `org.moqui.impl.ShellServices` (no Java counterpart): the badge counts of the `/qapps` shell. The framework does not
//! count anything itself: `get#ShellCounts` calls the service the user's qapps theme names in the theme value
//! `STRT_SHELL_COUNTS_SERVICE` (IronStone BOSS seeds it), so the framework stays Mantle-free. `publish#ShellCounts`
//! sends a user's counts on the notification topic `shellCounts`, at most once per user every two seconds: a request
//! inside that window is folded into one send at its end. The sends are made on a thread of the factory's after the
//! caller's transaction commits, so the caller never waits for them.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};
use moqui_lib::sync::{lock, wait_timeout};

use crate::context::{Error, ExecutionContext, ExecutionContextFactory};
use crate::entity::EntityFacade;
use crate::screen::sri::{theme_resource_values, user_theme_id, DEFAULT_QUASAR_THEME, QUASAR_THEME_TYPE};

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, get_shell_counts)?;
    rust_service!(map, publish_shell_counts)?;
    Ok(())
}

/// The notification topic the counts go out on.
pub(crate) const COUNTS_TOPIC: &str = "shellCounts";
/// The theme value that names the counts service.
const COUNTS_SERVICE: &str = "STRT_SHELL_COUNTS_SERVICE";
/// At most one send of a user's counts in this time.
const WINDOW: Duration = Duration::from_secs(2);

// ---- the counts ----

/// Run `name` and return its results. An error it leaves in the message facade is a failure, and is taken back out so
/// the caller's own answer is not one; the caller's earlier messages and errors stay.
///
/// # Errors
///
/// Returns the call's error, or the errors it recorded.
pub(crate) fn call_tolerant(
    name: &str,
    parameters: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, Error> {
    let mark = ExecutionContext::with_current(|ec| ec.message().error_mark());
    let result = super::call::sync(name, parameters);
    let errors: Vec<String> = mark
        .and_then(|mark| ExecutionContext::with_current(|ec| ec.message().take_errors_since(mark)))
        .unwrap_or_default();
    match result {
        Err(err) => Err(err),
        Ok(_) if !errors.is_empty() => Err(Error::new(errors.join("; "))),
        Ok(results) => Ok(results),
    }
}

/// The counts services that failed, kept for the factory.
#[derive(Default)]
struct FailedServices(Mutex<HashSet<String>>);

/// True the first time a counts service fails, so each is logged once while the server runs.
fn first_failure(service: &str) -> bool {
    ExecutionContext::with_current(|ec| {
        let failed = ec.factory().extension(|_| FailedServices::default());
        let first = lock(&failed.0).insert(service.to_string());
        first
    })
    .unwrap_or(true)
}

/// The counts of `user_id`: the `counts` map of the service the user's qapps theme names in `STRT_SHELL_COUNTS_SERVICE`
/// (the theme as a render finds it, else `DEFAULT_QUASAR`), called with `userId`. None when the theme names no service,
/// and when the service is missing, fails or answers no map, which is logged once per service name.
///
/// The service is called with authorization disabled: the theme data that names it is trusted, it reads only that user's
/// rows, and no app's authorization covers it (the webroot transitions are not under an app root). So this is only for
/// the current user's counts, or for internal work; `get#ShellCounts` refuses another user's to a caller with
/// authorization on.
pub(crate) fn shell_counts(user_id: &str) -> Option<IndexMap<String, Value>> {
    let theme = user_theme_id(Some(user_id), QUASAR_THEME_TYPE, DEFAULT_QUASAR_THEME);
    let service = match theme_resource_values(&theme, COUNTS_SERVICE) {
        Ok(values) => values.into_iter().next()?,
        Err(err) => {
            tracing::error!("Could not read {COUNTS_SERVICE} of theme {theme}: {err}");
            return None;
        }
    };
    let failed = |problem: &str| {
        if first_failure(&service) {
            tracing::error!("The shell counts service {service} (theme {theme}) {problem}; the shell shows no counts");
        }
        None
    };
    let parameters = BTreeMap::from([("userId".to_string(), Value::string(user_id))]);
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    match call_tolerant(&service, &parameters) {
        Ok(out) => match out.get("counts") {
            Some(Value::Map(counts)) => Some(counts.clone()),
            _ => failed("answered no counts map"),
        },
        Err(err) => failed(&format!("failed: {err}")),
    }
}

fn text(parameters: &BTreeMap<String, Value>, key: &str) -> Option<String> {
    parameters
        .get(key)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .filter(|text| !text.trim().is_empty())
}

/// `get#ShellCounts`: the counts of `userId`, the current user when it is empty. Out: `counts`, null when there are
/// none. Another user's counts are only for internal work, with authorization disabled.
fn get_shell_counts(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let current = crate::user::current_user_id()
        .ok()
        .flatten()
        .filter(|id| !id.is_empty());
    let Some(user_id) = text(parameters, "userId").or_else(|| current.clone()) else {
        return Ok(BTreeMap::from([("counts".to_string(), Value::Null)]));
    };
    let internal = ExecutionContext::with_current(ExecutionContext::authz_disabled).unwrap_or(true);
    if current.as_deref() != Some(user_id.as_str()) && !internal {
        ExecutionContext::with_current(|ec| {
            ec.message()
                .add_error("get#ShellCounts reads another user's counts only with authorization disabled")
        });
        return Ok(BTreeMap::new());
    }
    let counts = shell_counts(&user_id).map_or(Value::Null, Value::Map);
    Ok(BTreeMap::from([("counts".to_string(), counts)]))
}

/// The user ids of `userIdList`: a list, or a comma separated text; without blanks and repeats.
pub(crate) fn user_ids(value: Option<&Value>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |id: String| {
        let id = id.trim().to_string();
        if !id.is_empty() && !out.contains(&id) {
            out.push(id);
        }
    };
    match value {
        Some(Value::List(items)) => {
            for item in items {
                if !is_empty(item) {
                    add(to_plain_string(item));
                }
            }
        }
        Some(other) if !is_empty(other) => {
            for id in to_plain_string(other).split(',') {
                add(id.to_string());
            }
        }
        _ => {}
    }
    out
}

/// `publish#ShellCounts`: send each user of `userIdList` their counts on the `shellCounts` topic, coalesced per user
/// ([`Coalescer`]). In a transaction the request waits for the commit, so the counts read what the caller wrote, and a
/// rollback sends nothing. It returns at once.
fn publish_shell_counts(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let users = user_ids(parameters.get("userIdList"));
    let Some(current) = ExecutionContext::with_current(|ec| Arc::clone(ec.factory())) else {
        return Ok(BTreeMap::new());
    };
    if users.is_empty() {
        return Ok(BTreeMap::new());
    }
    if !would_deliver(
        current.is_shutting_down(),
        crate::notify::has_listeners(crate::notify::NOTIFICATION_MESSAGE),
        crate::cluster::is_linked(facade),
    ) {
        tracing::debug!(
            users = users.len(),
            "no notification socket or peer to send shell counts to, or shutting down"
        );
        return Ok(BTreeMap::new());
    }
    let factory = Arc::downgrade(&current);
    drop(current);
    let requester = |factory: Weak<ExecutionContextFactory>, users: Vec<String>| {
        move || {
            if let Some(factory) = factory.upgrade() {
                request_sends(&factory, &users);
            }
        }
    };
    match facade.default_transactions() {
        Ok(tx) if tx.in_transaction() => {
            if let Err(err) = tx.on_commit(requester(factory.clone(), users.clone())) {
                tracing::error!("Could not wait for the commit to publish shell counts, publishing now: {err}");
                requester(factory, users)();
            }
        }
        _ => requester(factory, users)(),
    }
    Ok(BTreeMap::new())
}

/// Whether the counts would reach anyone: not while the factory shuts down (its database is closing), and only with a
/// notification socket in this process or cluster peers to post to. A command such as `moqui load` has neither, so the
/// EECAs of the records it loads send nothing (and nothing runs after its database closes).
pub fn would_deliver(shutting_down: bool, listening: bool, linked: bool) -> bool {
    !shutting_down && (listening || linked)
}

// ---- coalescing ----

/// What a request for a user's send comes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    /// No send in the last window: due now.
    Now,
    /// Due at the end of the window of the last send.
    At(Instant),
    /// A send is already due, and covers this request.
    Folded,
}

#[derive(Clone, Copy, Debug, Default)]
struct Slot {
    sent: Option<Instant>,
    due: Option<Instant>,
}

/// The sends due for each user, at most one per user in a window. Times are given, so it is tested without a clock.
#[derive(Debug, Default)]
pub(crate) struct Coalescer {
    users: HashMap<String, Slot>,
}

impl Coalescer {
    /// A request at `now` for `user`'s send.
    pub(crate) fn request(&mut self, user: &str, now: Instant, window: Duration) -> Decision {
        let slot = self.users.entry(user.to_string()).or_default();
        if slot.due.is_some() {
            return Decision::Folded;
        }
        let due = match slot.sent.and_then(|sent| sent.checked_add(window)) {
            Some(end) if now < end => end,
            _ => now,
        };
        slot.due = Some(due);
        if due <= now {
            Decision::Now
        } else {
            Decision::At(due)
        }
    }

    /// The users whose send is due at `now`, in id order; each counts as sent now.
    pub(crate) fn take_due(&mut self, now: Instant) -> Vec<String> {
        let mut out = Vec::new();
        for (user, slot) in &mut self.users {
            if slot.due.is_some_and(|due| due <= now) {
                slot.due = None;
                slot.sent = Some(now);
                out.push(user.clone());
            }
        }
        out.sort_unstable();
        out
    }

    /// When the next send is due.
    pub(crate) fn next_due(&self) -> Option<Instant> {
        self.users.values().filter_map(|slot| slot.due).min()
    }

    /// Forget the users with nothing due whose last send is a window old, so only recent ones are kept.
    pub(crate) fn forget_old(&mut self, now: Instant, window: Duration) {
        self.users.retain(|_, slot| {
            slot.due.is_some()
                || slot
                    .sent
                    .and_then(|sent| sent.checked_add(window))
                    .is_some_and(|end| now < end)
        });
    }
}

/// The sends of a factory: the coalescer and the thread that makes them, which runs while one is due.
struct Publisher {
    state: Mutex<PublisherState>,
    wake: Condvar,
}

struct PublisherState {
    coalescer: Coalescer,
    window: Duration,
    running: bool,
    factory: Weak<ExecutionContextFactory>,
}

impl Default for Publisher {
    fn default() -> Self {
        Self {
            state: Mutex::new(PublisherState {
                coalescer: Coalescer::default(),
                window: WINDOW,
                running: false,
                factory: Weak::new(),
            }),
            wake: Condvar::new(),
        }
    }
}

fn publisher(factory: &ExecutionContextFactory) -> Arc<Publisher> {
    factory.extension(|_| Publisher::default())
}

/// A shorter window, for tests of the sends.
#[cfg(test)]
pub(crate) fn set_window(factory: &ExecutionContextFactory, window: Duration) {
    lock(&publisher(factory).state).window = window;
}

/// Ask for a send of each user's counts: now when none was sent in the last window, else once at its end. It does not
/// wait: the sends are made on the factory's sender thread, started when one is due and ended when none is.
pub(crate) fn request_sends(factory: &Arc<ExecutionContextFactory>, users: &[String]) {
    let publisher = publisher(factory);
    let mut state = lock(&publisher.state);
    state.factory = Arc::downgrade(factory);
    let now = Instant::now();
    let window = state.window;
    for user in users {
        state.coalescer.request(user, now, window);
    }
    if !state.running {
        state.running = true;
        let worker = Arc::clone(&publisher);
        let started = std::thread::Builder::new()
            .name("moqui-shell-counts".to_string())
            .spawn(move || run_sends(&worker));
        if let Err(err) = started {
            state.running = false;
            tracing::error!("Could not start the shell counts sender: {err}");
        }
    }
    drop(state);
    publisher.wake.notify_all();
}

/// The sender thread: send what is due, wait for the next, end when nothing is due or the factory is gone.
fn run_sends(publisher: &Publisher) {
    loop {
        let (due, factory) = {
            let mut state = lock(&publisher.state);
            loop {
                let now = Instant::now();
                let due = state.coalescer.take_due(now);
                if !due.is_empty() {
                    break (due, state.factory.clone());
                }
                let window = state.window;
                state.coalescer.forget_old(now, window);
                let Some(next) = state.coalescer.next_due() else {
                    state.running = false;
                    return;
                };
                state = wait_timeout(&publisher.wake, state, next.saturating_duration_since(now)).0;
            }
        };
        let Some(factory) = factory.upgrade().filter(|factory| !factory.is_shutting_down()) else {
            let mut state = lock(&publisher.state);
            state.coalescer = Coalescer::default();
            state.running = false;
            return;
        };
        for user in &due {
            send_counts(&factory, user);
        }
    }
}

/// Send one user's counts, in an execution context of its own with authorization disabled, as internal work. A user
/// with no counts gets nothing; a failure is logged.
fn send_counts(factory: &Arc<ExecutionContextFactory>, user_id: &str) {
    let _cleanup = super::pool::WorkCleanup::new(Some(Arc::clone(factory)));
    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<(), Error> {
        let ctx = factory.new_context();
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        use_user_locale(user_id);
        let Some(counts) = shell_counts(user_id) else {
            return Ok(());
        };
        crate::notify::NotificationMessage::new()
            .topic(COUNTS_TOPIC)
            .user_id(user_id)
            .message_map(counts)
            .persist(false)
            .show_alert(false)
            .send()
    }));
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(err)) => tracing::error!("Could not send the shell counts of {user_id}: {err}"),
        Err(payload) => tracing::error!(
            "Sending the shell counts of {user_id} panicked: {}",
            super::pool::panic_text(payload.as_ref())
        ),
    }
}

/// The time zone and locale of `user_id`'s `UserAccount` for the current context, as a login sets them, so the day
/// boundaries of the counts (`ec.user.nowTimestamp`, "today") and their formatting are the user's, not the server's.
/// The sender's context has no session of its own; a user without them keeps the server's, as after a login.
fn use_user_locale(user_id: &str) {
    let account = ExecutionContext::with_current(|ec| ec.factory().entity())
        .flatten()
        .map(|facade| {
            facade
                .find("moqui.security.UserAccount")?
                .eq("userId", Value::string(user_id))
                .one()
        });
    let account = match account {
        Some(Ok(Some(account))) => account,
        Some(Err(err)) => {
            tracing::warn!("Could not read the time zone of user {user_id} for the shell counts: {err}");
            return;
        }
        _ => return,
    };
    let field = |name: &str| {
        account
            .get(name)
            .map(to_plain_string)
            .filter(|text| !text.trim().is_empty())
    };
    crate::user::session::set_locale(field("locale").as_deref());
    crate::user::session::set_time_zone(field("timeZone").as_deref());
}

#[cfg(test)]
mod tests {
    use super::{would_deliver, Coalescer, Decision};
    use std::time::{Duration, Instant};

    const WINDOW: Duration = Duration::from_secs(2);

    /// The counts go to a socket of this process or to cluster peers, and never while the factory shuts down: a command
    /// such as `moqui load` (no socket, no peers) sends nothing for the records it loads.
    #[test]
    fn counts_are_sent_only_where_a_socket_or_a_peer_can_take_them() {
        assert!(would_deliver(false, true, false));
        assert!(would_deliver(false, false, true));
        assert!(!would_deliver(false, false, false), "a command: no socket, no peers");
        assert!(!would_deliver(true, true, true), "shutting down");
    }

    #[test]
    fn a_user_gets_one_send_per_window_and_a_later_request_is_folded_into_one_at_its_end() {
        let start = Instant::now();
        let at = |millis: u64| start + Duration::from_millis(millis);
        let mut coalescer = Coalescer::default();
        assert_eq!(coalescer.request("alice", at(0), WINDOW), Decision::Now);
        // a request before the send is made is the same send
        assert_eq!(coalescer.request("alice", at(10), WINDOW), Decision::Folded);
        assert_eq!(coalescer.take_due(at(20)), ["alice"]);
        assert_eq!(coalescer.next_due(), None);
        // inside the window of that send: due at its end, and the next requests fold into it
        assert_eq!(coalescer.request("alice", at(500), WINDOW), Decision::At(at(2020)));
        assert_eq!(coalescer.request("alice", at(900), WINDOW), Decision::Folded);
        assert_eq!(coalescer.request("alice", at(1900), WINDOW), Decision::Folded);
        assert!(coalescer.take_due(at(1500)).is_empty());
        assert_eq!(coalescer.next_due(), Some(at(2020)));
        assert_eq!(coalescer.take_due(at(2020)), ["alice"]);
        // after a quiet window the next request is due at once
        assert_eq!(coalescer.request("alice", at(5000), WINDOW), Decision::Now);
    }

    #[test]
    fn users_are_coalesced_apart_and_old_ones_are_forgotten() {
        let start = Instant::now();
        let at = |millis: u64| start + Duration::from_millis(millis);
        let mut coalescer = Coalescer::default();
        assert_eq!(coalescer.request("bob", at(0), WINDOW), Decision::Now);
        assert_eq!(coalescer.request("alice", at(0), WINDOW), Decision::Now);
        assert_eq!(coalescer.take_due(at(0)), ["alice", "bob"]);
        assert_eq!(coalescer.request("bob", at(100), WINDOW), Decision::At(at(2000)));
        // alice's window is not bob's
        assert_eq!(coalescer.take_due(at(100)), Vec::<String>::new());
        coalescer.forget_old(at(3000), WINDOW);
        // bob still has a send due, alice is forgotten
        assert_eq!(coalescer.users.len(), 1);
        assert_eq!(coalescer.take_due(at(3000)), ["bob"]);
        coalescer.forget_old(at(6000), WINDOW);
        assert!(coalescer.users.is_empty());
    }
}
