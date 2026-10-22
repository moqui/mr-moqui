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

//! One pool. Another database is another instance.
//!
//! Ordinary checkout never takes a reserve connection. It waits only when a checked-out ordinary
//! connection might be returned. If the only idle connections are reserve and no ordinary
//! connection can be opened or returned, it fails without using [`PoolConfig::acquire_timeout`].
//! A parked client counts against `max`.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use moqui_lib::error::Error;
use moqui_lib::object::Value;

use crate::driver::{DbCancel, DbConnection, DbDriver, DbRow};

/// The idle-connection checkout test when the conf names none.
pub const DEFAULT_TEST_QUERY: &str = "SELECT 1";

/// Datasource pool settings. `reserve` sits inside `max`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PoolConfig {
    pub min: u32,
    pub max: u32,
    pub reserve: u32,
    /// `pool-time-wait`. Zero does not wait.
    pub acquire_timeout: Duration,
    /// `pool-time-idle`: an idle connection past this is closed at the next checkout or return,
    /// while more than `min` are open. `None` keeps idle connections.
    pub idle_timeout: Option<Duration>,
    /// Idle-connection test: `pool-test-query`, else the database's `default-test-query`.
    /// `None` is [`DEFAULT_TEST_QUERY`]. Newly connected clients need no test.
    pub test_query: Option<String>,
    /// An idle connection younger than this is handed out without the test query. Zero tests every reuse.
    /// A client [`DbConnection::is_closed`] already reports is discarded either way.
    pub test_after_idle: Duration,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            min: 5,
            max: 50,
            reserve: 8,
            acquire_timeout: Duration::from_secs(30),
            idle_timeout: None,
            test_query: None,
            test_after_idle: Duration::from_secs(1),
        }
    }
}

impl PoolConfig {
    fn test_sql(&self) -> &str {
        self.test_query
            .as_deref()
            .filter(|sql| !sql.trim().is_empty())
            .unwrap_or(DEFAULT_TEST_QUERY)
    }
}

/// Checked-out, parked, waiting, and idle counts. `checkouts` counts successful checkouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PoolStats {
    pub checked_out: u32,
    pub parked: u32,
    pub waiting: u32,
    pub ordinary_idle: u32,
    pub reserve_idle: u32,
    pub checkouts: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Ordinary,
    Reserve,
}

struct State {
    ordinary_idle: Vec<Tracked>,
    reserve_idle: Vec<Tracked>,
    ordinary_out: u32,
    reserve_out: u32,
    parked: u32,
    waiting: u32,
    creating_ordinary: u32,
    creating_reserve: u32,
    testing_ordinary: u32,
    testing_reserve: u32,
    checkouts: u64,
    /// When a connection was last found dead (its server restarted, or a proxy or NAT dropped it). Every connection
    /// that went idle before then is tested before it is handed out, however recently: the others are likely dead too.
    lost_at: Option<Instant>,
}

impl State {
    fn total(&self) -> u32 {
        self.ordinary_idle.len() as u32
            + self.reserve_idle.len() as u32
            + self.ordinary_out
            + self.reserve_out
            + self.parked
            + self.creating_ordinary
            + self.creating_reserve
            + self.testing_ordinary
            + self.testing_reserve
    }

    fn ordinary_live(&self) -> u32 {
        self.ordinary_idle.len() as u32
            + self.ordinary_out
            + self.parked
            + self.creating_ordinary
            + self.testing_ordinary
    }

    fn reserve_live(&self) -> u32 {
        self.reserve_idle.len() as u32 + self.reserve_out + self.creating_reserve + self.testing_reserve
    }

    fn checked_out(&self) -> u32 {
        self.ordinary_out + self.reserve_out
    }
}

struct Inner {
    driver: Arc<dyn DbDriver>,
    config: PoolConfig,
    state: Mutex<State>,
    available: Condvar,
}

/// Shared pool. `connect` runs on checkout, not on construction.
#[derive(Clone)]
pub struct Pool {
    inner: Arc<Inner>,
}

enum Step {
    /// Idle long enough to run the test query, or the only way this pool checks a reused client.
    Test(Tracked),
    /// Returned recently and still open. No test query.
    Ready(Tracked),
    Connect,
    Wait,
    Fail(Error),
}

pub(super) struct PooledConn {
    tracked: Option<Tracked>,
    pool: Arc<Inner>,
    class: Class,
    parked: bool,
    discard: bool,
    /// The discarded client was dead: the others of the pool are suspect.
    lost: bool,
    timeout_armed: bool,
}

struct Tracked {
    conn: Box<dyn DbConnection>,
    in_tx: bool,
    /// When it last went back on an idle list.
    idle_since: Instant,
}

impl Tracked {
    fn new(conn: Box<dyn DbConnection>) -> Self {
        Self {
            conn,
            in_tx: false,
            idle_since: Instant::now(),
        }
    }
}

impl DbConnection for Tracked {
    fn execute(&mut self, sql: &str, params: &[Value]) -> Result<u64, Error> {
        self.conn.execute(sql, params)
    }

    fn execute_deferred(&mut self, sql: &str, params: Vec<Value>, zero_rows: Option<String>) -> Result<(), Error> {
        self.conn.execute_deferred(sql, params, zero_rows)
    }

    fn execute_deferred_row(&mut self, sql: &str, params: Vec<Value>, key: u64) -> Result<(), Error> {
        self.conn.execute_deferred_row(sql, params, key)
    }

    fn finish_deferred(&mut self) -> Result<(), Error> {
        self.conn.finish_deferred()
    }

    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
        self.conn.query(sql, params)
    }

    fn query_once(&mut self, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
        self.conn.query_once(sql, params)
    }

    fn open_stream(&mut self, sql: &str, params: &[Value]) -> Result<(), Error> {
        self.conn.open_stream(sql, params)
    }

    fn next_row(&mut self) -> Result<Option<DbRow>, Error> {
        self.conn.next_row()
    }

    fn close_stream(&mut self) -> Result<(), Error> {
        self.conn.close_stream()
    }

    fn stream_open(&self) -> bool {
        self.conn.stream_open()
    }

    fn is_closed(&self) -> bool {
        self.conn.is_closed()
    }

    fn begin(&mut self) -> Result<(), Error> {
        self.conn.begin()?;
        self.in_tx = true;
        Ok(())
    }

    fn commit(&mut self) -> Result<(), Error> {
        self.conn.commit()?;
        self.in_tx = false;
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), Error> {
        self.conn.rollback()?;
        self.in_tx = false;
        Ok(())
    }

    fn savepoint(&mut self, name: &str) -> Result<(), Error> {
        self.conn.savepoint(name)
    }

    fn rollback_to_savepoint(&mut self, name: &str) -> Result<(), Error> {
        self.conn.rollback_to_savepoint(name)
    }

    fn release_savepoint(&mut self, name: &str) -> Result<(), Error> {
        self.conn.release_savepoint(name)
    }

    fn canceller(&self) -> Box<dyn DbCancel> {
        self.conn.canceller()
    }

    fn returns_rows(&mut self, sql: &str) -> Result<Option<bool>, Error> {
        self.conn.returns_rows(sql)
    }

    fn set_statement_timeout(&mut self, remaining: Option<std::time::Duration>) -> Result<(), Error> {
        self.conn.set_statement_timeout(remaining)
    }
}

impl Pool {
    pub fn new(driver: Arc<dyn DbDriver>, config: PoolConfig) -> Result<Self, Error> {
        if config.max == 0 {
            return Err(Error::new("pool max must be greater than 0"));
        }
        if config.min > config.max {
            return Err(Error::new(format!(
                "pool min {} is greater than max {}",
                config.min, config.max
            )));
        }
        if config.reserve > config.max {
            return Err(Error::new(format!(
                "pool reserve {} is greater than max {}",
                config.reserve, config.max
            )));
        }
        Ok(Self {
            inner: Arc::new(Inner {
                driver,
                config,
                state: Mutex::new(State {
                    ordinary_idle: Vec::new(),
                    reserve_idle: Vec::new(),
                    ordinary_out: 0,
                    reserve_out: 0,
                    parked: 0,
                    waiting: 0,
                    creating_ordinary: 0,
                    creating_reserve: 0,
                    testing_ordinary: 0,
                    testing_reserve: 0,
                    checkouts: 0,
                    lost_at: None,
                }),
                available: Condvar::new(),
            }),
        })
    }

    pub fn config(&self) -> PoolConfig {
        self.inner.config.clone()
    }

    /// Errors when `(request_cap + service_cap) * 2 > max - reserve`.
    pub fn check_caps(&self, request_cap: u32, service_cap: u32) -> Result<(), Error> {
        self.check_caps_with(request_cap, service_cap, 0)
    }

    /// The connections left of `max - reserve` once `(request_cap + service_cap) * 2` are counted.
    pub fn spare_connections(&self, request_cap: u32, service_cap: u32) -> u32 {
        let demand = (u64::from(request_cap) + u64::from(service_cap)) * 2;
        let room = u64::from(self.inner.config.max - self.inner.config.reserve);
        u32::try_from(room.saturating_sub(demand)).unwrap_or(u32::MAX)
    }

    /// [`Self::check_caps`] with `single` more workers that hold one connection each: the agent pool's step jobs, which
    /// open only short transactions of their own and none across a model call.
    pub fn check_caps_with(&self, request_cap: u32, service_cap: u32, single: u32) -> Result<(), Error> {
        let demand = (u64::from(request_cap) + u64::from(service_cap)) * 2 + u64::from(single);
        let room = u64::from(self.inner.config.max - self.inner.config.reserve);
        if demand > room {
            let plus = if single > 0 { format!(" + {single}") } else { String::new() };
            return Err(Error::new(format!(
                "transaction caps exceed pool: ({request_cap} + {service_cap}) * 2{plus} = {demand} > max - reserve = {room}"
            )));
        }
        Ok(())
    }

    pub fn stats(&self) -> PoolStats {
        let state = self.lock();
        PoolStats {
            checked_out: state.checked_out(),
            parked: state.parked,
            waiting: state.waiting,
            ordinary_idle: state.ordinary_idle.len() as u32,
            reserve_idle: state.reserve_idle.len() as u32,
            checkouts: state.checkouts,
        }
    }

    pub(super) fn checkout_ordinary(&self) -> Result<PooledConn, Error> {
        self.acquire(Class::Ordinary)
    }

    pub(super) fn checkout_reserve(&self) -> Result<PooledConn, Error> {
        if self.inner.config.reserve == 0 {
            return Err(Error::new("pool has no reserve connections"));
        }
        self.acquire(Class::Reserve)
    }

    /// A failed test drops its connection. Once as many have failed as were idle when the checkout began (a server
    /// restart leaves every idle connection dead), the idle list is passed over for a new connection.
    fn acquire(&self, class: Class) -> Result<PooledConn, Error> {
        let mut failures = 0usize;
        let mut budget: Option<usize> = None;
        let mut failed_test: Option<Error> = None;
        let started = Instant::now();
        loop {
            let (step, expired) = {
                let mut state = self.lock();
                let mut expired = self.take_expired(&mut state);
                if budget.is_none() {
                    budget = Some(idle_len(&state, class));
                }
                let allowed = budget.unwrap_or(0);
                let skip_idle = allowed > 0 && failures >= allowed;
                let step = decide(&mut state, class, &self.inner.config, skip_idle, &mut expired);
                (step, expired)
            };
            drop(expired);
            match step {
                Step::Ready(tracked) => return Ok(self.wrap(tracked, class)),
                Step::Test(tracked) => match test_connection(tracked, self.inner.config.test_sql()) {
                    Ok(tracked) => {
                        self.finish_test(class, true);
                        return Ok(self.wrap(tracked, class));
                    }
                    Err((tracked, err)) => {
                        self.finish_test(class, false);
                        drop(tracked);
                        failures += 1;
                        failed_test = Some(err);
                    }
                },
                Step::Connect => {
                    let connected = self.inner.driver.connect();
                    let tracked = match connected {
                        Ok(conn) => Tracked::new(conn),
                        Err(err) => {
                            self.mark_creating(class, false);
                            self.inner.available.notify_all();
                            return Err(unavailable(err));
                        }
                    };
                    // A successful connect already established a live session. Let the caller's
                    // metadata query or application work be its first SQL, as on Java startup.
                    self.finish_create(class);
                    return Ok(self.wrap(tracked, class));
                }
                Step::Wait => {
                    if !self.wait_for_return(started)? {
                        return Err(unavailable(Error::new("timed out waiting for a connection")));
                    }
                }
                Step::Fail(err) => return Err(unavailable(failed_test.take().unwrap_or(err))),
            }
        }
    }

    fn wait_for_return(&self, started: Instant) -> Result<bool, Error> {
        let timeout = self.inner.config.acquire_timeout;
        if timeout.is_zero() {
            return Ok(false);
        }
        if super::manager::single_threaded() {
            return Err(Error::new(
                "no connection is free, and no other thread can return one (inline executor)",
            ));
        }
        let deadline = started + timeout;
        let now = Instant::now();
        if now >= deadline {
            return Ok(false);
        }
        let wait_started = Instant::now();
        {
            let mut state = self.lock();
            state.waiting += 1;
            publish(&state, Duration::ZERO);
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                state.waiting = state.waiting.saturating_sub(1);
                publish(&state, wait_started.elapsed());
                return Ok(false);
            }
            let waited = self.inner.available.wait_timeout(state, remaining);
            let (mut state, result) = match waited {
                Ok(pair) => pair,
                Err(poisoned) => poisoned.into_inner(),
            };
            state.waiting = state.waiting.saturating_sub(1);
            publish(&state, wait_started.elapsed());
            if result.timed_out() {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn wrap(&self, tracked: Tracked, class: Class) -> PooledConn {
        PooledConn {
            tracked: Some(tracked),
            pool: Arc::clone(&self.inner),
            class,
            parked: false,
            discard: false,
            lost: false,
            timeout_armed: false,
        }
    }

    pub(super) fn park(&self, conn: &mut PooledConn) {
        if conn.parked || conn.class != Class::Ordinary {
            return;
        }
        let mut state = self.lock();
        state.ordinary_out = state.ordinary_out.saturating_sub(1);
        state.parked += 1;
        conn.parked = true;
        publish(&state, Duration::ZERO);
    }

    pub(super) fn unpark(&self, conn: &mut PooledConn) {
        if !conn.parked {
            return;
        }
        let mut state = self.lock();
        state.parked = state.parked.saturating_sub(1);
        state.ordinary_out += 1;
        conn.parked = false;
        publish(&state, Duration::ZERO);
    }

    /// A test that failed lost its connection.
    fn finish_test(&self, class: Class, handed_out: bool) {
        {
            let mut state = self.lock();
            adjust_testing(&mut state, class, false);
            if handed_out {
                hand_out(&mut state, class);
            } else {
                state.lost_at = Some(Instant::now());
            }
            publish(&state, Duration::ZERO);
        }
        if !handed_out {
            self.inner.available.notify_all();
        }
    }

    fn mark_creating(&self, class: Class, starting: bool) {
        let mut state = self.lock();
        adjust_creating(&mut state, class, starting);
        publish(&state, Duration::ZERO);
    }

    fn finish_create(&self, class: Class) {
        let mut state = self.lock();
        adjust_creating(&mut state, class, false);
        hand_out(&mut state, class);
        publish(&state, Duration::ZERO);
    }

    fn recycle(&self, mut tracked: Tracked, class: Class, parked: bool) {
        if tracked.in_tx {
            if let Err(err) = tracked.rollback() {
                tracing::debug!(error = %err, "rollback before returning a connection failed");
                let mut state = self.lock();
                release_hold(&mut state, class, parked);
                publish(&state, Duration::ZERO);
                self.inner.available.notify_all();
                return;
            }
        }
        tracked.idle_since = Instant::now();
        let mut state = self.lock();
        release_hold(&mut state, class, parked);
        match class {
            Class::Ordinary => state.ordinary_idle.push(tracked),
            Class::Reserve => state.reserve_idle.push(tracked),
        }
        let expired = self.take_expired(&mut state);
        publish(&state, Duration::ZERO);
        drop(state);
        self.inner.available.notify_all();
        // Closing a client can block on its runtime, so it happens off the lock.
        drop(expired);
    }

    /// Idle connections past `idle_timeout`, while more than `min` stay open. The idle lists hand
    /// out from the end, so the oldest sit at the front.
    fn take_expired(&self, state: &mut State) -> Vec<Tracked> {
        let Some(timeout) = self.inner.config.idle_timeout else {
            return Vec::new();
        };
        let now = Instant::now();
        let mut closable = state.total().saturating_sub(self.inner.config.min) as usize;
        let mut expired = Vec::new();
        for list in [&mut state.ordinary_idle, &mut state.reserve_idle] {
            let stale = list
                .iter()
                .take_while(|tracked| now.duration_since(tracked.idle_since) >= timeout)
                .count()
                .min(closable);
            closable -= stale;
            expired.extend(list.drain(..stale));
        }
        expired
    }

    fn discard_hold(&self, class: Class, parked: bool, lost: bool) {
        let mut state = self.lock();
        release_hold(&mut state, class, parked);
        if lost {
            state.lost_at = Some(Instant::now());
        }
        publish(&state, Duration::ZERO);
        drop(state);
        self.inner.available.notify_all();
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|err| err.into_inner())
    }
}

impl PooledConn {
    pub(super) fn conn(&mut self) -> &mut dyn DbConnection {
        self.tracked.as_mut().expect("pooled connection is empty")
    }

    pub(super) fn canceller(&self) -> Option<Box<dyn DbCancel>> {
        self.tracked.as_ref().map(|tracked| tracked.canceller())
    }

    pub(super) fn stream_open(&self) -> bool {
        self.tracked.as_ref().is_some_and(|tracked| tracked.stream_open())
    }

    pub(super) fn discard(&mut self) {
        self.discard = true;
        if let Some(tracked) = self.tracked.take() {
            self.lost = tracked.is_closed();
        }
    }

    /// False after [`Self::discard`] took the client.
    pub(super) fn live(&self) -> bool {
        self.tracked.is_some()
    }

    /// Sets the statement timeout once. Returning the connection to the pool clears it, so a
    /// later borrower never inherits it.
    pub(super) fn arm_statement_timeout(&mut self, remaining: std::time::Duration) -> Result<(), Error> {
        if self.timeout_armed {
            return Ok(());
        }
        self.conn().set_statement_timeout(Some(remaining))?;
        self.timeout_armed = true;
        Ok(())
    }
}

impl Drop for PooledConn {
    fn drop(&mut self) {
        if self.timeout_armed && !self.discard {
            if let Some(tracked) = self.tracked.as_mut() {
                if let Err(err) = tracked.set_statement_timeout(None) {
                    tracing::warn!(error = %err, "could not clear statement timeout; discarding connection");
                    self.discard = true;
                }
            }
        }
        let Some(tracked) = self.tracked.take() else {
            if self.discard {
                self.pool_discard();
            }
            return;
        };
        if self.discard {
            drop(tracked);
            self.pool_discard();
            return;
        }
        Pool {
            inner: Arc::clone(&self.pool),
        }
        .recycle(tracked, self.class, self.parked);
    }
}

impl PooledConn {
    fn pool_discard(&self) {
        Pool {
            inner: Arc::clone(&self.pool),
        }
        .discard_hold(self.class, self.parked, self.lost);
    }
}

fn idle_len(state: &State, class: Class) -> usize {
    match class {
        Class::Ordinary => state.ordinary_idle.len(),
        Class::Reserve => state.reserve_idle.len(),
    }
}

/// A checkout that found no connection it could use: the database cannot take the work now, a 503 to a web client.
/// The message stays the cause's.
fn unavailable(cause: Error) -> Error {
    let message = cause.message().to_string();
    Error::from(crate::context::Error::of_kind(crate::context::ErrorKind::Unavailable, message).with_source(cause))
}

/// A client the driver already reports closed goes to `closed`, for the caller to drop off the lock. With `skip_idle`,
/// after failed tests, only a new connection will do.
fn decide(state: &mut State, class: Class, config: &PoolConfig, skip_idle: bool, closed: &mut Vec<Tracked>) -> Step {
    if !skip_idle {
        while let Some(tracked) = pop_idle(state, class) {
            // A client the driver already knows is dead is not worth a test query. Off the idle list, it no longer
            // counts toward the pool, so another can be opened in its place.
            if tracked.is_closed() {
                closed.push(tracked);
                state.lost_at = Some(Instant::now());
                continue;
            }
            let suspect = state.lost_at.is_some_and(|lost_at| tracked.idle_since <= lost_at);
            if suspect || tracked.idle_since.elapsed() >= config.test_after_idle {
                adjust_testing(state, class, true);
                return Step::Test(tracked);
            }
            // Handed out now, as `finish_test` does after a test.
            hand_out(state, class);
            return Step::Ready(tracked);
        }
    }
    if can_create(state, class, config) {
        adjust_creating(state, class, true);
        return Step::Connect;
    }
    if skip_idle {
        return Step::Fail(Error::new("connection test failed"));
    }
    if might_return(state, class) {
        return Step::Wait;
    }
    if class == Class::Ordinary && !state.reserve_idle.is_empty() {
        return Step::Fail(Error::new("only reserve connections are idle"));
    }
    Step::Fail(Error::new("no connection available"))
}

fn pop_idle(state: &mut State, class: Class) -> Option<Tracked> {
    match class {
        Class::Ordinary => state.ordinary_idle.pop(),
        Class::Reserve => state.reserve_idle.pop(),
    }
}

fn can_create(state: &State, class: Class, config: &PoolConfig) -> bool {
    if state.total() >= config.max {
        return false;
    }
    match class {
        Class::Ordinary => state.ordinary_live() < config.max.saturating_sub(config.reserve),
        Class::Reserve => state.reserve_live() < config.reserve,
    }
}

/// A checked-out client of this class can come back to the idle list. A parked client does not:
/// the thread that parked it cannot return it while blocked in checkout.
fn might_return(state: &State, class: Class) -> bool {
    match class {
        Class::Ordinary => state.ordinary_out > 0,
        Class::Reserve => state.reserve_out > 0,
    }
}

fn test_connection(mut tracked: Tracked, sql: &str) -> Result<Tracked, (Tracked, Error)> {
    match tracked.query_once(sql, &[]) {
        Ok(_) => Ok(tracked),
        Err(err) => {
            let err = Error::new("connection test failed").with_source(err);
            Err((tracked, err))
        }
    }
}

fn adjust_testing(state: &mut State, class: Class, starting: bool) {
    let slot = match class {
        Class::Ordinary => &mut state.testing_ordinary,
        Class::Reserve => &mut state.testing_reserve,
    };
    if starting {
        *slot += 1;
    } else {
        *slot = slot.saturating_sub(1);
    }
}

fn adjust_creating(state: &mut State, class: Class, starting: bool) {
    let slot = match class {
        Class::Ordinary => &mut state.creating_ordinary,
        Class::Reserve => &mut state.creating_reserve,
    };
    if starting {
        *slot += 1;
    } else {
        *slot = slot.saturating_sub(1);
    }
}

fn hand_out(state: &mut State, class: Class) {
    match class {
        Class::Ordinary => state.ordinary_out += 1,
        Class::Reserve => state.reserve_out += 1,
    }
    state.checkouts += 1;
}

fn release_hold(state: &mut State, class: Class, parked: bool) {
    if parked {
        state.parked = state.parked.saturating_sub(1);
        return;
    }
    match class {
        Class::Ordinary => state.ordinary_out = state.ordinary_out.saturating_sub(1),
        Class::Reserve => state.reserve_out = state.reserve_out.saturating_sub(1),
    }
}

fn publish(state: &State, wait: Duration) {
    tracing::debug!(
        target: "moqui.pool",
        checked_out = state.checked_out(),
        parked = state.parked,
        waiting = state.waiting,
        wait_ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX),
        "pool gauges"
    );
}
