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

//! Thread-local transaction stack. Two managers do not share a stack, but linked managers (the entity groups on
//! databases of their own, [`TransactionManager::link`]) begin, suspend and end their frames together.

use std::cell::RefCell;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, Location};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};
use std::thread::ThreadId;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use moqui_lib::error::Error;
use moqui_lib::node::MNode;
use moqui_lib::sync::{lock, read, write};

use crate::driver::{DbCancel, DbConnection, DbDriver};

use super::pool::{Pool, PoolConfig, PoolStats, PooledConn};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
/// How much longer than the transaction's remaining time one read from the database may wait: the deadline's cancel
/// answers first when the database is there, and this bounds the wait when it is not.
const READ_MARGIN: Duration = Duration::from_secs(15);
/// Threads that send deadline cancels, so a cancel that waits on an unreachable host does not hold the timer or the
/// other cancels. Each driver bounds its own cancel's network work.
const CANCEL_WORKERS: usize = 4;
/// Cancels waiting for a worker; past this one is dropped with a warning (the transaction is rollback-only already).
const CANCEL_BACKLOG: usize = 256;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static STACKS: RefCell<HashMap<u64, Vec<Frame>>> = RefCell::new(HashMap::new());
}

type ConnHook = Box<dyn FnOnce(&mut dyn DbConnection) -> Result<(), Error> + Send>;
type AfterHook = Box<dyn FnOnce() + Send>;

struct Frame {
    join_depth: u32,
    deadline: Instant,
    conn: Rc<RefCell<Option<PooledConn>>>,
    suspended: bool,
    /// Set by a `force_new` that suspended the frame below, which this frame's end resumes.
    resumes_parent: bool,
    rollback_only: Option<RollbackInfo>,
    cache_mode: CacheMode,
    fired: Arc<AtomicBool>,
    deadline_token: u64,
    /// When the frame began, in epoch milliseconds, as Java's `transactionBeginStartTime`.
    began_millis: i64,
    cache_flush: Vec<ConnHook>,
    feed_insert: Vec<ConnHook>,
    on_commit: Vec<AfterHook>,
    on_rollback: Vec<AfterHook>,
    /// The transaction's span and duration (`AT_TRANSACTION`), when telemetry is on.
    trace: Option<Box<dyn crate::hooks::TransactionTrace>>,
    /// This thread's waited round trips when the frame began.
    round_trips_at: u64,
}

impl Drop for Frame {
    fn drop(&mut self) {
        if unregister_deadline(self.deadline_token) {
            discard_conn(&self.conn);
        }
    }
}

struct RollbackInfo {
    message: String,
    error: Option<Error>,
    callsite: &'static Location<'static>,
}

struct Phase {
    token: u64,
    conn: Rc<RefCell<Option<PooledConn>>>,
    cache_flush: Vec<ConnHook>,
    feed_insert: Vec<ConnHook>,
    on_commit: Vec<AfterHook>,
    on_rollback: Vec<AfterHook>,
    trace: Option<Box<dyn crate::hooks::TransactionTrace>>,
    round_trips_at: u64,
    cache_mode: CacheMode,
}

enum Top {
    Empty,
    Active,
    Suspended,
}

enum Prepared {
    Joined,
    Ready {
        rollback_only: Option<RollbackInfo>,
        phase: Box<Phase>,
    },
}

/// How [`TransactionManager::run`] enlists the body. Service XML calls this in phase 6.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransactionAttribute {
    /// Begin nothing: the body runs inside the transaction in place, if any, as Java's `ignore`.
    Ignore,
    /// Suspend the transaction in place and run with none: `ignore` plus `force-new`, as Java.
    IgnoreSuspend,
    UseOrBegin,
    ForceNew,
    Cache,
    ForceCache,
}

/// What a transaction frame does with the transaction cache (plans/TX-Cache.md D1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheMode {
    /// No transaction cache: writes go to the database at once and finds read it.
    Off,
    /// Writes are held in memory until the commit, and finds see them (`cache` and `force-cache`).
    Hold,
    /// Writes go to the database at once, and the frame's read cache is kept (Java's read-only transaction cache):
    /// what a frame turns to after an early flush that stops holding (`no-tx-cache`).
    ReadThrough,
}

struct Inner {
    id: u64,
    pool: Pool,
    use_transaction_cache: AtomicBool,
    /// `moqui.tx_cache.strict` (tests): an early flush of a hold frame marks it rollback-only.
    tx_cache_strict: AtomicBool,
    /// `moqui.tx_cache.everywhere` (dev and test census): every frame a `run` begins or joins holds.
    tx_cache_everywhere: AtomicBool,
    use_statement_timeout: AtomicBool,
    use_lock_track: AtomicBool,
    /// The managers one transaction spans, this one among them, in commit order ([`TransactionManager::link`]);
    /// empty when it spans this one alone. Weak, so linked managers do not keep each other's pools alive.
    linked: RwLock<Vec<Weak<Inner>>>,
}

struct LockHit {
    thread: ThreadId,
    frame: u64,
}

/// Transaction manager. `Clone` shares the pool and the thread-local stack key.
#[derive(Clone)]
pub struct TransactionManager {
    inner: Arc<Inner>,
}

/// The error of a frame marked rollback-only, of kind `RollbackOnly` so a caller that marked it can tell it apart.
fn rollback_only_error(message: String) -> Error {
    crate::error::Error::of_kind(crate::error::ErrorKind::RollbackOnly, message).into()
}

impl TransactionManager {
    pub fn new(driver: Arc<dyn DbDriver>, config: PoolConfig) -> Result<Self, Error> {
        Ok(Self {
            inner: Arc::new(Inner {
                id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
                pool: Pool::new(driver, config)?,
                use_transaction_cache: AtomicBool::new(true),
                tx_cache_strict: AtomicBool::new(property_flag("moqui.tx_cache.strict")),
                tx_cache_everywhere: AtomicBool::new(property_flag("moqui.tx_cache.everywhere")),
                use_statement_timeout: AtomicBool::new(false),
                use_lock_track: AtomicBool::new(false),
                linked: RwLock::new(Vec::new()),
            }),
        })
    }

    /// `transaction-facade` flags. An empty expansion keeps the default.
    pub fn configure_from_conf(&self, conf: &MNode) -> Result<(), Error> {
        let node = if conf.has_name("transaction-facade") {
            conf.clone()
        } else {
            match conf.first_named("transaction-facade") {
                Some(node) => node,
                None => return Ok(()),
            }
        };
        self.set_use_transaction_cache(conf_flag(node.attribute("use-transaction-cache"), true));
        self.set_use_lock_track(conf_flag(node.attribute("use-lock-track"), false));
        self.set_use_statement_timeout(conf_flag(node.attribute("use-statement-timeout"), false));
        self.set_tx_cache_strict(property_flag("moqui.tx_cache.strict"));
        self.set_tx_cache_everywhere(property_flag("moqui.tx_cache.everywhere"));
        Ok(())
    }

    /// Strict mode (plans/TX-Cache.md D1, tests only): an early flush of a hold frame marks the frame rollback-only, so
    /// a test sees an operation that does not commit in one batch fail. `moqui.tx_cache.strict` (or
    /// `MOQUI_TX_CACHE_STRICT`) sets it when the manager is made or configured.
    pub fn set_tx_cache_strict(&self, value: bool) {
        self.inner.tx_cache_strict.store(value, Ordering::Relaxed);
    }

    #[must_use]
    pub fn tx_cache_strict(&self) -> bool {
        self.inner.tx_cache_strict.load(Ordering::Relaxed)
    }

    /// Census mode (plans/TX-Cache.md D1, dev and test only): every frame a `run` begins or joins holds its writes, so a
    /// run of the tests lists each operation that flushes early or fails under the cache. `moqui.tx_cache.everywhere`
    /// (or `MOQUI_TX_CACHE_EVERYWHERE`) sets it when the manager is made or configured.
    pub fn set_tx_cache_everywhere(&self, value: bool) {
        self.inner.tx_cache_everywhere.store(value, Ordering::Relaxed);
    }

    #[must_use]
    pub fn tx_cache_everywhere(&self) -> bool {
        self.inner.tx_cache_everywhere.load(Ordering::Relaxed)
    }

    pub fn set_use_transaction_cache(&self, value: bool) {
        self.inner.use_transaction_cache.store(value, Ordering::Relaxed);
    }

    pub fn set_use_statement_timeout(&self, value: bool) {
        self.inner.use_statement_timeout.store(value, Ordering::Relaxed);
    }

    pub fn set_use_lock_track(&self, value: bool) {
        self.inner.use_lock_track.store(value, Ordering::Relaxed);
    }

    /// True when the active frame holds writes in the transaction cache. Only `cache` and
    /// `force-cache` turn it on, and `use-transaction-cache="false"` keeps it off.
    pub fn transaction_cache_active(&self) -> bool {
        self.absorb_deadline();
        self.with_frames(|frames| active(frames).is_some_and(|frame| frame.cache_mode == CacheMode::Hold))
    }

    /// The active frame's token and transaction cache mode, when it has one on ([`CacheMode::Hold`] or
    /// [`CacheMode::ReadThrough`]).
    #[must_use]
    pub fn cache_frame(&self) -> Option<(u64, CacheMode)> {
        self.with_frames(|frames| {
            active(frames)
                .filter(|frame| frame.cache_mode != CacheMode::Off)
                .map(|frame| (frame.deadline_token, frame.cache_mode))
        })
    }

    /// True when `other` is a clone of this manager, so the two share one frame stack.
    pub fn shares_stack(&self, other: &TransactionManager) -> bool {
        self.inner.id == other.inner.id
    }

    /// Link `members` so one transaction spans them all, as Java's JTA transaction spans every datasource, but
    /// without two-phase commit. A begin, join, force-new, suspend or resume on any of them does the same on the
    /// others. A frame checks out no connection until a statement runs, so a database the transaction does not
    /// touch costs a stack entry and nothing else.
    ///
    /// The commit (or rollback) of the frames, on whichever manager began them, ends all of them. The commit goes
    /// in the order of `members`, so the entity facade passes the default group's manager last. Rows of the
    /// default group refer to the others' (a content location), so a failure between two commits leaves rows
    /// nothing refers to, never a reference to a row that is not there. A rollback, or a frame of any of them
    /// marked rollback-only, rolls back all of them.
    ///
    /// Managers that share a stack are one member. Fewer than two members unlinks them.
    pub fn link(members: &[TransactionManager]) {
        let mut distinct: Vec<&TransactionManager> = Vec::new();
        for member in members {
            if !distinct.iter().any(|known| known.shares_stack(member)) {
                distinct.push(member);
            }
        }
        let order: Vec<Weak<Inner>> = if distinct.len() > 1 {
            distinct.iter().map(|member| Arc::downgrade(&member.inner)).collect()
        } else {
            Vec::new()
        };
        for member in distinct {
            *write(&member.inner.linked) = order.clone();
        }
    }

    /// The managers linked with this one ([`Self::link`]), in commit order, this one not among them.
    #[must_use]
    pub fn linked(&self) -> Vec<TransactionManager> {
        self.link_order()
            .map(|order| order.into_iter().filter(|member| !member.shares_stack(self)).collect())
            .unwrap_or_default()
    }

    /// The linked managers in commit order, this one among them; `None` when this one is not linked.
    fn link_order(&self) -> Option<Vec<TransactionManager>> {
        let linked = read(&self.inner.linked);
        if linked.is_empty() {
            return None;
        }
        Some(
            linked
                .iter()
                .filter_map(Weak::upgrade)
                .map(|inner| TransactionManager { inner })
                .collect(),
        )
    }

    pub fn stats(&self) -> PoolStats {
        self.inner.pool.stats()
    }

    /// See [`super::pool::PoolCounters`].
    pub fn pool_counters(&self) -> super::pool::PoolCounters {
        self.inner.pool.counters()
    }

    /// Errors when `(request_cap + service_cap) * 2 > max - reserve`.
    pub fn check_caps(&self, request_cap: u32, service_cap: u32) -> Result<(), Error> {
        self.inner.pool.check_caps(request_cap, service_cap)
    }

    /// See [`super::pool::Pool::spare_connections`].
    pub fn spare_connections(&self, request_cap: u32, service_cap: u32) -> u32 {
        self.inner.pool.spare_connections(request_cap, service_cap)
    }

    /// See [`super::pool::Pool::check_caps_with`].
    pub fn check_caps_with(&self, request_cap: u32, service_cap: u32, single: u32) -> Result<(), Error> {
        self.inner.pool.check_caps_with(request_cap, service_cap, single)
    }

    /// Records the deadline and pushes a frame, or joins the active one. Does not check out a connection.
    ///
    /// Returns `true` when this call began the transaction, and `false` when it joined the frame
    /// already active on this thread. Each `begin`, joined or not, is ended by exactly one
    /// [`Self::commit`] or [`Self::rollback`]: for a joined call these give the join back, and a joined
    /// rollback also marks the frame rollback-only, as Java's `rollback(beganTransaction = false)`.
    /// [`Self::scope`] and [`Self::run`] pair the calls. `timeout` defaults to 60 seconds and is not
    /// changed when this call joins.
    ///
    /// The linked managers ([`Self::link`]) begin or join a frame too.
    pub fn begin(&self, timeout: Option<Duration>) -> Result<bool, Error> {
        let began_millis = epoch_millis();
        let began = self.begin_local(timeout, false, began_millis)?;
        for other in self.linked() {
            other.begin_local(timeout, false, began_millis)?;
        }
        Ok(began)
    }

    /// [`Self::begin`] in a guard: dropping the scope without [`TxScope::commit`] rolls back a frame it
    /// began, or marks a joined frame rollback-only, and gives the join back.
    pub fn scope(&self, timeout: Option<Duration>) -> Result<TxScope, Error> {
        let began = self.begin(timeout)?;
        self.scope_after(began)
    }

    /// Pushes a frame on this manager's stack, or joins the active one. `began_millis` is the transaction's start,
    /// which the linked managers' frames share.
    fn begin_local(&self, timeout: Option<Duration>, resumes_parent: bool, began_millis: i64) -> Result<bool, Error> {
        let timeout = timeout.unwrap_or(DEFAULT_TIMEOUT);
        self.with_frames(|frames| {
            if let Some(frame) = active_mut(frames) {
                frame.join_depth += 1;
                return Ok(false);
            }
            let deadline = Instant::now() + timeout;
            let fired = Arc::new(AtomicBool::new(false));
            let deadline_token = NEXT_DEADLINE.fetch_add(1, Ordering::Relaxed);
            register_deadline(deadline_token, deadline, Arc::clone(&fired));
            frames.push(Frame {
                join_depth: 0,
                deadline,
                conn: Rc::new(RefCell::new(None)),
                suspended: false,
                resumes_parent,
                rollback_only: None,
                cache_mode: CacheMode::Off,
                fired,
                deadline_token,
                began_millis,
                cache_flush: Vec::new(),
                feed_insert: Vec::new(),
                on_commit: Vec::new(),
                on_rollback: Vec::new(),
                trace: crate::hooks::trace_transaction(),
                round_trips_at: crate::driver::thread_round_trips(),
            });
            Ok(true)
        })
    }

    /// Runs `body` under `attr`. A begun frame commits on success and rolls back on error.
    /// A joined frame pops its join and, on error, marks the outer frame rollback-only. The frame
    /// ended is checked to be the one begun or joined, as for [`TxScope`].
    pub fn run<T, F>(&self, attr: TransactionAttribute, timeout: Option<Duration>, body: F) -> Result<T, Error>
    where
        F: FnOnce() -> Result<T, Error>,
    {
        match attr {
            TransactionAttribute::Ignore => self.run_ignore(body),
            TransactionAttribute::IgnoreSuspend => self.run_suspended(body),
            TransactionAttribute::UseOrBegin => self.run_bounded(false, false, timeout, body),
            TransactionAttribute::ForceNew => self.run_bounded(true, false, timeout, body),
            TransactionAttribute::Cache => self.run_bounded(false, true, timeout, body),
            TransactionAttribute::ForceCache => self.run_bounded(true, true, timeout, body),
        }
    }

    /// Ends the innermost `begin` on this thread. A joined call gives its join back and does nothing else.
    ///
    /// For the call that began the frame: runs the cache-flush and feed-insert hooks, commits, returns the
    /// connection, then runs the on-commit hooks. A frame marked rollback-only rolls back instead, runs the
    /// on-rollback hooks, and returns an error.
    ///
    /// The linked managers' frames end with it ([`Self::link`]): they commit in link order, and once one commit
    /// fails, or when any of the frames is marked rollback-only, the rest roll back. The after hooks run when all
    /// have ended, and then each manager resumes the frame its `force_new` suspended.
    pub fn commit(&self) -> Result<(), Error> {
        self.end_frames(None)
    }

    /// Ends the innermost `begin` on this thread. For the call that began the frame, rolls back, returns the
    /// connection, and runs the on-rollback hooks. A joined call marks the frame rollback-only and gives its
    /// join back, so the owner's commit rolls back. The linked managers' frames end with it.
    #[track_caller]
    pub fn rollback(&self) -> Result<(), Error> {
        self.end_frames(Some(Location::caller()))
    }

    /// A later commit rolls back and logs the message, the cause, and this callsite.
    #[track_caller]
    pub fn set_rollback_only(&self, message: impl Into<String>) -> Result<(), Error> {
        self.store_rollback(message.into(), None)
    }

    /// Same as [`Self::set_rollback_only`], keeping the cause error for the commit log.
    #[track_caller]
    pub fn set_rollback_only_error(&self, message: impl Into<String>, error: Error) -> Result<(), Error> {
        self.store_rollback(message.into(), Some(error))
    }

    /// Checks out once per frame and reuses that connection.
    ///
    /// With no active frame, checks a connection out and returns it before this function returns.
    /// An open stream is that enlisted connection: this reuses it and does not check out another.
    /// An error from the closure marks the frame rollback-only. Later calls fail until rollback.
    #[track_caller]
    pub fn with_connection<T, F>(&self, body: F) -> Result<T, Error>
    where
        F: FnOnce(&mut dyn DbConnection) -> Result<T, Error>,
    {
        self.absorb_deadline();
        if !self.has_active() {
            let mut pooled = self.inner.pool.checkout_ordinary()?;
            let result = body(pooled.conn());
            if result.is_err() && pooled.live() && pooled.conn().is_closed() {
                pooled.discard();
            }
            drop(pooled);
            return result;
        }
        if let Some(message) = self.rollback_message() {
            return Err(rollback_only_error(format!("transaction is rollback-only: {message}")));
        }
        let slot = self.active_conn()?;
        if slot.borrow().is_none() {
            let mut pooled = self.inner.pool.checkout_ordinary()?;
            if let Err(err) = pooled.conn().begin() {
                pooled.discard();
                return Err(err);
            }
            *slot.borrow_mut() = Some(pooled);
            // Once per frame: the enlisted connection stays until the frame ends.
            self.install_canceller();
        }
        let mut borrow = match slot.try_borrow_mut() {
            Ok(borrow) => borrow,
            Err(_) => return Err(Error::new("connection already in use")),
        };
        self.arm_statement_timeout(borrow.as_mut())?;
        self.arm_read_timeout(borrow.as_mut())?;
        let pooled = borrow.as_mut().ok_or_else(|| Error::new("no connection enlisted"))?;
        let result = body(pooled.conn());
        if result.is_err() && pooled.live() && pooled.conn().is_closed() {
            pooled.discard();
        }
        if let Err(err) = &result {
            let message = err.to_string();
            let info = RollbackInfo {
                message: message.clone(),
                error: Some(Error::new(message)),
                callsite: Location::caller(),
            };
            drop(borrow);
            let _ = self.mark_rollback_only(info);
        }
        result
    }

    /// Close the connection enlisted in the active frame when the frame ends, instead of returning it to the pool: a
    /// session a statement may have changed (a setting, a temporary table) goes to no later borrower
    /// (plans/archive/Tools-Database.md 8.2). Call after the frame's first [`Self::with_connection`].
    ///
    /// # Errors
    ///
    /// When there is no active frame or no connection enlisted in it.
    pub fn discard_connection_at_end(&self) -> Result<(), Error> {
        let slot = self.active_conn()?;
        let mut borrow = slot
            .try_borrow_mut()
            .map_err(|_| Error::new("connection already in use"))?;
        let pooled = borrow.as_mut().ok_or_else(|| Error::new("no connection enlisted"))?;
        pooled.discard_on_return();
        Ok(())
    }

    /// [`Self::with_connection`], and the connection is closed instead of going back to the pool: at once when no
    /// frame is active (an autocommit statement), else when the frame ends ([`Self::discard_connection_at_end`]). For a
    /// statement that may leave session state behind (plans/archive/Tools-Database.md 8.2).
    ///
    /// # Errors
    ///
    /// As [`Self::with_connection`].
    pub fn with_connection_closed_after<T, F>(&self, body: F) -> Result<T, Error>
    where
        F: FnOnce(&mut dyn DbConnection) -> Result<T, Error>,
    {
        self.absorb_deadline();
        if !self.has_active() {
            let mut pooled = self.inner.pool.checkout_ordinary()?;
            let result = body(pooled.conn());
            pooled.discard();
            return result;
        }
        let result = self.with_connection(body);
        // the frame has its connection now, unless the checkout itself failed
        let _ = self.discard_connection_at_end();
        result
    }

    /// Reserve checkout. The closure begins and commits its own transaction, or stays autocommit.
    /// Does not use the caller's enlisted connection.
    pub fn with_reserve<T, F>(&self, body: F) -> Result<T, Error>
    where
        F: FnOnce(&mut dyn DbConnection) -> Result<T, Error>,
    {
        let mut pooled = self.inner.pool.checkout_reserve()?;
        let result = body(pooled.conn());
        if result.is_err() && pooled.live() && pooled.conn().is_closed() {
            pooled.discard();
        }
        drop(pooled);
        result
    }

    /// Parks the enlisted client on the frame, off the free list. Returns `false` when nothing is active. The linked
    /// managers suspend their frames too.
    pub fn suspend(&self) -> Result<bool, Error> {
        let others = self.linked();
        if others.iter().any(|other| other.has_active() && other.stream_open()) {
            return Err(Error::new("an open stream is the enlisted connection"));
        }
        let suspended = self.suspend_local()?;
        for other in others {
            other.suspend_local()?;
        }
        Ok(suspended)
    }

    fn suspend_local(&self) -> Result<bool, Error> {
        if !self.has_active() {
            return Ok(false);
        }
        if self.stream_open() {
            return Err(Error::new("an open stream is the enlisted connection"));
        }
        let slot = self.active_conn()?;
        if let Some(conn) = slot.borrow_mut().as_mut() {
            self.inner.pool.park(conn);
        }
        self.with_frames(|frames| {
            let frame = active_mut(frames).ok_or_else(|| Error::new("no transaction in place"))?;
            frame.suspended = true;
            Ok(true)
        })
    }

    /// Checks the parked client out again. The linked managers resume their frames too.
    pub fn resume(&self) -> Result<(), Error> {
        self.resume_local()?;
        for other in self.linked() {
            if let Err(err) = other.resume_local() {
                tracing::warn!(error = %err, "a linked transaction manager had no frame to resume");
            }
        }
        Ok(())
    }

    fn resume_local(&self) -> Result<(), Error> {
        if self.has_active() {
            return Err(Error::new("cannot resume while a transaction is active"));
        }
        let slot = self.with_frames(|frames| {
            let frame = frames.last().ok_or_else(|| Error::new("no suspended transaction"))?;
            if !frame.suspended {
                return Err(Error::new("no suspended transaction"));
            }
            Ok(Rc::clone(&frame.conn))
        })?;
        if let Some(conn) = slot.borrow_mut().as_mut() {
            self.inner.pool.unpark(conn);
        }
        self.with_frames(|frames| {
            if let Some(frame) = frames.last_mut() {
                frame.suspended = false;
            }
            Ok(())
        })
    }

    /// Suspends the active frame, if any, then begins a frame. Ending the new frame resumes the one this
    /// call suspended, and only that one.
    ///
    /// The park budget counts this thread's parked clients on this pool, not other threads'.
    /// See [`Self::park_budget`].
    ///
    /// The linked managers do the same, each resuming its own frame when the new one ends.
    pub fn force_new(&self, timeout: Option<Duration>) -> Result<bool, Error> {
        let members = self.link_order().unwrap_or_else(|| vec![self.clone()]);
        for member in &members {
            member.check_force_new()?;
        }
        let began_millis = epoch_millis();
        let mut began = false;
        for member in &members {
            let suspended = member.has_active() && member.suspend_local()?;
            let member_began = member.begin_local(timeout, suspended, began_millis)?;
            if member.shares_stack(self) {
                began = member_began;
            }
        }
        Ok(began)
    }

    fn check_force_new(&self) -> Result<(), Error> {
        let parking = usize::from(self.active_conn().is_ok_and(|slot| slot.borrow().is_some()));
        if self.parked_in_thread() + parking > self.park_budget() {
            return Err(Error::new("park budget exceeded"));
        }
        if self.stream_open() {
            return Err(Error::new("an open stream is the enlisted connection"));
        }
        Ok(())
    }

    /// Parked clients one thread may hold on this pool: every ordinary connection but the one the
    /// innermost frame needs.
    ///
    /// The startup caps check, `(request cap + service cap) * 2 <= max - reserve`, covers one parked
    /// client per thread. A deeper `force-new` draws on what is left. Checkout never waits on a
    /// parked client, so when the pool is out, that level fails after `pool-time-wait` at most,
    /// and does not deadlock.
    pub fn park_budget(&self) -> usize {
        let config = self.inner.pool.config();
        config.max.saturating_sub(config.reserve).saturating_sub(1).max(1) as usize
    }

    /// Clients this thread has parked on this pool. A suspended frame that never checked out
    /// parks nothing.
    fn parked_in_thread(&self) -> usize {
        self.with_frames(|frames| {
            frames
                .iter()
                .filter(|frame| frame.suspended && frame.conn.borrow().is_some())
                .count()
        })
    }

    pub fn savepoint(&self, name: &str) -> Result<(), Error> {
        self.with_enlisted(|conn| conn.savepoint(name))
    }

    pub fn rollback_to_savepoint(&self, name: &str) -> Result<(), Error> {
        self.with_enlisted(|conn| conn.rollback_to_savepoint(name))
    }

    pub fn release_savepoint(&self, name: &str) -> Result<(), Error> {
        self.with_enlisted(|conn| conn.release_savepoint(name))
    }

    pub fn on_cache_flush<F>(&self, hook: F) -> Result<(), Error>
    where
        F: FnOnce(&mut dyn DbConnection) -> Result<(), Error> + Send + 'static,
    {
        self.push_conn_hook(HookKind::Cache, Box::new(hook))
    }

    pub fn on_feed_insert<F>(&self, hook: F) -> Result<(), Error>
    where
        F: FnOnce(&mut dyn DbConnection) -> Result<(), Error> + Send + 'static,
    {
        self.push_conn_hook(HookKind::Feed, Box::new(hook))
    }

    /// True when this frame already has a cache-flush hook.
    pub fn has_cache_flush(&self) -> bool {
        self.with_frames(|frames| active_mut(frames).is_some_and(|frame| !frame.cache_flush.is_empty()))
    }

    /// True when this frame already has a feed-insert hook.
    pub fn has_feed_insert(&self) -> bool {
        self.with_frames(|frames| active_mut(frames).is_some_and(|frame| !frame.feed_insert.is_empty()))
    }

    /// True when this frame already has an on-commit hook.
    pub fn has_commit_hook(&self) -> bool {
        self.with_frames(|frames| active_mut(frames).is_some_and(|frame| !frame.on_commit.is_empty()))
    }

    pub fn on_commit<F>(&self, hook: F) -> Result<(), Error>
    where
        F: FnOnce() + Send + 'static,
    {
        self.push_after_hook(HookKind::Commit, Box::new(hook))
    }

    pub fn on_rollback<F>(&self, hook: F) -> Result<(), Error>
    where
        F: FnOnce() + Send + 'static,
    {
        self.push_after_hook(HookKind::Rollback, Box::new(hook))
    }

    /// Commits a frame left open, resumes and commits parked frames, and returns parked connections.
    ///
    /// Closes an open stream first. An open stream is the enlisted connection. This is Java's
    /// `destroyAllInThread`; the request and service pools use [`Self::rollback_all_in_thread`]. Does
    /// nothing when this thread has no frame for this manager.
    pub fn destroy_all_in_thread(&self) -> Result<(), Error> {
        if self.depth() > 0 {
            tracing::warn!("thread ending with a transaction in place; committing");
        }
        let result = self.destroy_frames(true);
        self.clear_thread_locks();
        result
    }

    /// [`Self::destroy_all_in_thread`], rolling back in place of committing. The request and service pools call this
    /// after each unit of work: a frame still open then was never ended by the code that began it, which `run` and
    /// [`TxScope`] always do, so it holds writes nobody decided to keep.
    pub fn rollback_all_in_thread(&self) -> Result<(), Error> {
        if self.depth() > 0 {
            tracing::error!("thread ending its work with a transaction in place; rolling back");
        }
        let result = self.destroy_frames(false);
        self.clear_thread_locks();
        result
    }

    /// When a frame's deadline has passed, marks it rollback-only and cancels the active statement.
    ///
    /// The background timer does the same without this call. This remains for the same thread.
    #[track_caller]
    pub fn poll_deadline(&self) -> Result<(), Error> {
        let now = Instant::now();
        let canceller = self.with_frames(|frames| {
            let mut canceller = None;
            for frame in frames.iter_mut() {
                if now < frame.deadline {
                    continue;
                }
                if frame.rollback_only.is_none() {
                    frame.fired.store(true, Ordering::SeqCst);
                    frame.rollback_only = Some(RollbackInfo {
                        message: "transaction deadline exceeded".to_string(),
                        error: None,
                        callsite: Location::caller(),
                    });
                    tracing::warn!("transaction deadline exceeded");
                }
            }
            if let Some(frame) = frames.last() {
                if !frame.suspended && now >= frame.deadline {
                    if let Some(conn) = frame.conn.borrow().as_ref() {
                        canceller = conn.canceller();
                    }
                }
            }
            canceller
        });
        if let Some(canceller) = canceller {
            canceller.cancel()?;
        }
        Ok(())
    }

    /// True when the active frame is marked rollback-only, as Java's `STATUS_MARKED_ROLLBACK`, or the active frame
    /// of a linked manager is: the commit then rolls back all of them.
    pub fn is_rollback_only(&self) -> bool {
        self.is_rollback_only_local() || self.linked().iter().any(TransactionManager::is_rollback_only_local)
    }

    fn is_rollback_only_local(&self) -> bool {
        self.with_frames(|frames| active(frames).is_some_and(|frame| frame.rollback_only.is_some()))
    }

    /// Wait for what the enlisted connection sent without waiting ([`DbConnection::execute_deferred`]) and report the
    /// first that failed, which marks the frame rollback-only as a failed statement does. Nothing when no connection
    /// is enlisted. The linked managers' enlisted connections are waited for too.
    pub fn finish_deferred(&self) -> Result<(), Error> {
        self.finish_deferred_local()?;
        for other in self.linked() {
            other.finish_deferred_local()?;
        }
        Ok(())
    }

    fn finish_deferred_local(&self) -> Result<(), Error> {
        let enlisted = self.has_active() && self.active_conn().is_ok_and(|slot| slot.borrow().is_some());
        if !enlisted {
            return Ok(());
        }
        self.with_connection(|conn| conn.finish_deferred())
    }

    /// True when this thread has an active frame, as Java `isTransactionInPlace`. A thread whose
    /// frames are all suspended, as in an `ignore` plus `force-new` body, has no transaction in place.
    pub fn in_transaction(&self) -> bool {
        self.has_active()
    }

    /// When the active transaction began, in epoch milliseconds, as Java's `getCurrentTransactionStartTime`: every
    /// write in one transaction stamps `lastUpdatedStamp` with it. `None` with no transaction in place.
    pub fn start_time_millis(&self) -> Option<i64> {
        self.with_frames(|frames| active(frames).map(|frame| frame.began_millis))
    }

    /// A token for the active frame, unique per begun transaction and shared by its joins.
    /// `None` when no frame is active, including when the top frame is suspended.
    pub fn frame_token(&self) -> Option<u64> {
        self.with_frames(|frames| active(frames).map(|frame| frame.deadline_token))
    }

    /// Ends the innermost `begin` on this manager and, when it began the frames, ends them on the linked managers:
    /// commits (`rollback_at` is `None`) or rolls back. See [`Self::commit`].
    fn end_frames(&self, rollback_at: Option<&'static Location<'static>>) -> Result<(), Error> {
        // This manager first: no frame here is the caller's error, and changes nothing on the others.
        let mut own = Some(self.prepare(rollback_at)?);
        let mut ready: Vec<(TransactionManager, Option<RollbackInfo>, Box<Phase>)> = Vec::new();
        for member in self.link_order().unwrap_or_default() {
            let prepared = if member.shares_stack(self) {
                own.take()
            } else {
                // a linked manager with no active frame has nothing to end
                member.prepare(rollback_at).ok()
            };
            if let Some(Prepared::Ready { rollback_only, phase }) = prepared {
                ready.push((member, rollback_only, phase));
            }
        }
        if let Some(Prepared::Ready { rollback_only, phase }) = own {
            ready.push((self.clone(), rollback_only, phase));
        }
        if ready.is_empty() {
            return Ok(());
        }
        // Committing: a frame marked rollback-only, this manager's before the others', rolls back all of them.
        let mut rollback_only = None;
        if rollback_at.is_none() {
            let at = ready
                .iter()
                .position(|(member, info, _)| info.is_some() && member.shares_stack(self))
                .or_else(|| ready.iter().position(|(_, info, _)| info.is_some()));
            if let Some(info) = at.and_then(|at| ready[at].1.take()) {
                let error_text = info.error.as_ref().map(ToString::to_string).unwrap_or_default();
                let callsite = format!("{}:{}", info.callsite.file(), info.callsite.line());
                tracing::warn!(
                    message = %info.message,
                    error = %error_text,
                    callsite = %callsite,
                    "commit marked rollback-only; rolling back"
                );
                rollback_only = Some(rollback_only_error(format!(
                    "transaction marked rollback-only: {}",
                    info.message
                )));
            }
        }
        let commit = rollback_at.is_none() && rollback_only.is_none();
        let mut failure: Option<Error> = None;
        let mut committed = 0usize;
        let mut ended = Vec::with_capacity(ready.len());
        for (member, _, mut phase) in ready {
            let commit = commit && failure.is_none();
            let enlisted = phase.conn.borrow().is_some();
            let result = if commit {
                try_commit(&mut phase)
            } else {
                rollback_enlisted(&phase.conn)
            };
            // The deadline slot holds this connection's canceller: clear it before the connection goes back. One whose
            // canceller the timer took may still get that cancel, so it is closed, never handed to another borrower.
            if unregister_deadline(phase.token) {
                discard_conn(&phase.conn);
            } else {
                return_conn(&phase.conn);
            }
            match result {
                Ok(()) => {
                    if commit && enlisted {
                        committed += 1;
                    }
                    ended.push((member, phase, commit));
                }
                Err(err) => {
                    if commit && committed > 0 {
                        tracing::error!(
                            error = %err,
                            committed,
                            "a commit failed after linked databases committed; their writes stay, with nothing referring to them"
                        );
                    }
                    failure.get_or_insert(err);
                    ended.push((member, phase, false));
                }
            }
        }
        let mut resumes = Vec::new();
        for (member, _, _) in &ended {
            match member.pop_active() {
                Ok(true) => resumes.push(member.clone()),
                Ok(false) => {}
                Err(err) => {
                    failure.get_or_insert(err);
                }
            }
        }
        // The after hooks run with every frame ended, then each manager resumes the frame its `force_new` suspended.
        for (_, mut phase, committed) in ended {
            end_trace(&mut phase, committed);
            if committed {
                run_after_hooks("commit", phase.on_commit.drain(..));
            } else {
                run_after_hooks("rollback", phase.on_rollback.drain(..));
            }
        }
        for member in resumes {
            member.resume_parked_parent();
        }
        match (failure, rollback_only) {
            (Some(err), _) | (None, Some(err)) => Err(err),
            (None, None) => Ok(()),
        }
    }

    /// Takes the active frame's end apart. A joined frame only gives its join back; `rollback_at`, the
    /// callsite of a rollback, also marks it rollback-only.
    fn prepare(&self, rollback_at: Option<&'static Location<'static>>) -> Result<Prepared, Error> {
        self.absorb_deadline();
        self.with_frames(|frames| {
            let frame = active_mut(frames).ok_or_else(|| Error::new("no transaction in place"))?;
            if frame.join_depth > 0 {
                frame.join_depth -= 1;
                if let Some(callsite) = rollback_at {
                    if frame.rollback_only.is_none() {
                        frame.rollback_only = Some(RollbackInfo {
                            message: "rollback in a joined transaction".to_string(),
                            error: None,
                            callsite,
                        });
                        // the owner's commit logs the rollback with this message and callsite
                        tracing::debug!("transaction set rollback-only");
                    }
                }
                return Ok(Prepared::Joined);
            }
            let rollback_only = frame.rollback_only.take();
            let phase = Phase {
                token: frame.deadline_token,
                conn: Rc::clone(&frame.conn),
                cache_flush: std::mem::take(&mut frame.cache_flush),
                feed_insert: std::mem::take(&mut frame.feed_insert),
                on_commit: std::mem::take(&mut frame.on_commit),
                on_rollback: std::mem::take(&mut frame.on_rollback),
                trace: frame.trace.take(),
                round_trips_at: frame.round_trips_at,
                cache_mode: frame.cache_mode,
            };
            Ok(Prepared::Ready {
                rollback_only,
                phase: Box::new(phase),
            })
        })
    }

    /// Pops the active frame, which unregisters its deadline. Returns whether ending it resumes the frame below.
    fn pop_active(&self) -> Result<bool, Error> {
        let (token, resumes_parent) = self.with_frames(|frames| match frames.last() {
            Some(frame) if !frame.suspended => {
                let ended = (frame.deadline_token, frame.resumes_parent);
                frames.pop();
                Ok(ended)
            }
            _ => Err(Error::new("no transaction in place")),
        })?;
        self.clear_frame_locks(token);
        Ok(resumes_parent)
    }

    fn push_conn_hook(&self, kind: HookKind, hook: ConnHook) -> Result<(), Error> {
        self.with_frames(|frames| {
            let frame = active_mut(frames).ok_or_else(|| Error::new("no transaction in place"))?;
            match kind {
                HookKind::Cache => frame.cache_flush.push(hook),
                HookKind::Feed => frame.feed_insert.push(hook),
                HookKind::Commit | HookKind::Rollback => {}
            }
            Ok(())
        })
    }

    fn push_after_hook(&self, kind: HookKind, hook: AfterHook) -> Result<(), Error> {
        self.with_frames(|frames| {
            let frame = active_mut(frames).ok_or_else(|| Error::new("no transaction in place"))?;
            match kind {
                HookKind::Commit => frame.on_commit.push(hook),
                HookKind::Rollback => frame.on_rollback.push(hook),
                HookKind::Cache | HookKind::Feed => {}
            }
            Ok(())
        })
    }

    fn mark_rollback_only(&self, info: RollbackInfo) -> Result<bool, Error> {
        self.with_frames(|frames| {
            let frame = active_mut(frames).ok_or_else(|| Error::new("no transaction in place"))?;
            if frame.rollback_only.is_none() {
                frame.rollback_only = Some(info);
                Ok(true)
            } else {
                Ok(false)
            }
        })
    }

    /// The message of the active frame's rollback-only mark, or of a linked manager's active frame: a statement then
    /// fails, as it does on one database.
    fn rollback_message(&self) -> Option<String> {
        self.rollback_message_local().or_else(|| {
            self.linked()
                .iter()
                .find_map(TransactionManager::rollback_message_local)
        })
    }

    fn rollback_message_local(&self) -> Option<String> {
        self.with_frames(|frames| {
            active(frames).and_then(|frame| frame.rollback_only.as_ref().map(|info| info.message.clone()))
        })
    }

    fn active_conn(&self) -> Result<Rc<RefCell<Option<PooledConn>>>, Error> {
        self.with_frames(|frames| {
            let frame = active_mut(frames).ok_or_else(|| Error::new("no transaction in place"))?;
            Ok(Rc::clone(&frame.conn))
        })
    }

    fn with_enlisted<T>(&self, body: impl FnOnce(&mut dyn DbConnection) -> Result<T, Error>) -> Result<T, Error> {
        let slot = self.active_conn().map_err(|_| Error::new("no connection enlisted"))?;
        let mut borrow = slot.borrow_mut();
        let pooled = borrow.as_mut().ok_or_else(|| Error::new("no connection enlisted"))?;
        body(pooled.conn())
    }

    fn stream_open(&self) -> bool {
        let Ok(slot) = self.active_conn() else {
            return false;
        };
        let borrow = slot.borrow();
        borrow.as_ref().is_some_and(|conn| conn.stream_open())
    }

    fn has_active(&self) -> bool {
        self.with_frames(|frames| active(frames).is_some())
    }

    fn depth(&self) -> usize {
        self.with_frames(|frames| frames.len())
    }

    fn top(&self) -> Top {
        self.with_frames(|frames| match frames.last() {
            None => Top::Empty,
            Some(frame) if frame.suspended => Top::Suspended,
            Some(_) => Top::Active,
        })
    }

    fn resume_parked_parent(&self) {
        let suspended = self.with_frames(|frames| matches!(frames.last(), Some(frame) if frame.suspended));
        if suspended {
            if let Err(err) = self.resume_local() {
                tracing::warn!(error = %err, "resume after completion failed");
            }
        }
    }

    fn with_frames<T>(&self, body: impl FnOnce(&mut Vec<Frame>) -> T) -> T {
        STACKS.with(|cell| {
            let mut map = cell.borrow_mut();
            let result = {
                let frames = map.entry(self.inner.id).or_default();
                body(frames)
            };
            if map.get(&self.inner.id).is_some_and(|frames| frames.is_empty()) {
                map.remove(&self.inner.id);
            }
            result
        })
    }

    #[track_caller]
    fn store_rollback(&self, message: String, error: Option<Error>) -> Result<(), Error> {
        let info = RollbackInfo {
            message,
            error,
            callsite: Location::caller(),
        };
        let stored = self.mark_rollback_only(info)?;
        if stored {
            // the commit logs the rollback with its message, cause and callsite: one line, as Java's rollback
            tracing::debug!("transaction set rollback-only");
        }
        Ok(())
    }

    /// The body joins the transaction in place without a frame of its own. An error marks that
    /// transaction rollback-only, as Java's `rollback(false, ...)` after a service that began nothing.
    fn run_ignore<T, F>(&self, body: F) -> Result<T, Error>
    where
        F: FnOnce() -> Result<T, Error>,
    {
        let result = body();
        if let Err(err) = &result {
            if self.has_active() {
                let _ = self.store_rollback(err.to_string(), None);
            }
        }
        result
    }

    fn run_suspended<T, F>(&self, body: F) -> Result<T, Error>
    where
        F: FnOnce() -> Result<T, Error>,
    {
        let mut guard = ResumeGuard {
            mgr: self,
            armed: false,
        };
        if self.has_active() {
            self.suspend()?;
            guard.armed = true;
        }
        body()
    }

    fn run_bounded<T, F>(&self, force: bool, cache: bool, timeout: Option<Duration>, body: F) -> Result<T, Error>
    where
        F: FnOnce() -> Result<T, Error>,
    {
        let began = if force {
            self.force_new(timeout)?
        } else {
            self.begin(timeout)?
        };
        // The scope rolls back, or poisons a joined frame, when the body unwinds.
        let scope = self.scope_after(began)?;
        // As Java `initTransactionCache`: the frame keeps the cache once this call returns, even when
        // this call joined. Held rows would otherwise flush after later direct writes to the same rows. A frame that
        // turned to read-through stays so, as Java's read-only cache does.
        let cache = cache || self.inner.tx_cache_everywhere.load(Ordering::Relaxed);
        if cache && self.inner.use_transaction_cache.load(Ordering::Relaxed) {
            self.with_frames(|frames| {
                if let Some(frame) = active_mut(frames).filter(|frame| frame.cache_mode == CacheMode::Off) {
                    frame.cache_mode = CacheMode::Hold;
                }
            });
        }
        match body() {
            Ok(value) => {
                scope.commit()?;
                Ok(value)
            }
            Err(err) => {
                if let Err(ended) = scope.end(Some(err.to_string())) {
                    tracing::warn!(error = %ended, cause = %err, "rollback after an error failed");
                }
                Err(err)
            }
        }
    }

    fn scope_after(&self, began: bool) -> Result<TxScope, Error> {
        let mark = self.with_frames(|frames| {
            active(frames).map(|frame| FrameMark {
                token: frame.deadline_token,
                join_depth: frame.join_depth,
                began,
            })
        });
        let mark = mark.ok_or_else(|| Error::new("no transaction in place"))?;
        Ok(TxScope {
            mgr: self.clone(),
            mark,
            ended: false,
            _thread: PhantomData,
        })
    }

    /// Ends the `begin` that `mark` records: commits when `rollback` is `None`, and otherwise rolls back a
    /// begun frame, or marks a joined one rollback-only with that message and gives the join back.
    fn end_frame(&self, mark: FrameMark, rollback: Option<String>) -> Result<(), Error> {
        self.restore_frame(mark)?;
        match rollback {
            None => self.commit(),
            Some(_) if mark.began => self.rollback(),
            Some(message) => {
                self.store_rollback(message, None)?;
                self.rollback()
            }
        }
    }

    /// Puts the frame `mark` records back on top, as it was, when the body left the stack otherwise: rolls
    /// back frames begun above it and not ended, resumes frames left suspended, and resets its join depth.
    /// Anything out of place is logged as an error and marks the frame rollback-only. Errors, and changes
    /// nothing, when the frame itself was ended.
    fn restore_frame(&self, mark: FrameMark) -> Result<(), Error> {
        let mut misplaced = false;
        loop {
            // Whether the frame is on top, and whether the top frame is suspended.
            let place = self.with_frames(|frames| {
                let at = frames.iter().rposition(|frame| frame.deadline_token == mark.token)?;
                Some((
                    at + 1 == frames.len(),
                    frames.last().is_some_and(|frame| frame.suspended),
                ))
            });
            match place {
                None => {
                    tracing::error!("transaction frame was ended by code inside its scope");
                    return Err(Error::new("transaction frame was ended by code inside its scope"));
                }
                Some((true, false)) => break,
                Some((_, true)) => {
                    misplaced = true;
                    self.resume()?;
                }
                Some((false, false)) => {
                    misplaced = true;
                    let before = self.depth();
                    for member in self.link_order().unwrap_or_else(|| vec![self.clone()]) {
                        member.with_frames(|frames| {
                            if let Some(frame) = active_mut(frames) {
                                frame.join_depth = 0;
                            }
                        });
                    }
                    self.store_rollback("transaction begun and not ended inside a scope".to_string(), None)?;
                    if let Err(err) = self.rollback() {
                        tracing::warn!(error = %err, "rollback of a transaction left open failed");
                    }
                    if self.depth() >= before {
                        return Err(Error::new("could not end a transaction left open inside a scope"));
                    }
                }
            }
        }
        let joins_moved = self.with_frames(|frames| {
            let frame = active_mut(frames)?;
            let moved = frame.join_depth != mark.join_depth;
            frame.join_depth = mark.join_depth;
            Some(moved)
        });
        // the linked managers joined and gave back with this one
        for other in self.linked() {
            other.with_frames(|frames| {
                if let Some(frame) = active_mut(frames) {
                    frame.join_depth = mark.join_depth;
                }
            });
        }
        if misplaced || joins_moved == Some(true) {
            tracing::error!(
                began = mark.began,
                "transaction stack left out of order inside a scope; marking it rollback-only"
            );
            self.store_rollback("transaction stack left out of order inside a scope".to_string(), None)?;
        }
        Ok(())
    }

    pub(crate) fn set_cache_mode(&self, mode: CacheMode) -> Result<(), Error> {
        self.with_frames(|frames| {
            let frame = active_mut(frames).ok_or_else(|| Error::new("no transaction in place"))?;
            frame.cache_mode = mode;
            Ok(())
        })
    }

    /// Ends every frame on this thread, innermost first: a joined frame gives back each join, so its owner ends it.
    fn destroy_frames(&self, commit: bool) -> Result<(), Error> {
        loop {
            match self.top() {
                Top::Empty => return Ok(()),
                Top::Suspended => self.resume()?,
                Top::Active => {
                    if let Err(err) = self.close_open_stream() {
                        let _ = self.set_rollback_only(err.to_string());
                    }
                    let before = self.depth();
                    let ended = if commit { self.commit() } else { self.rollback() };
                    if let Err(err) = ended {
                        if self.depth() >= before {
                            return Err(err);
                        }
                        if commit {
                            tracing::warn!(error = %err, "destroy_all_in_thread rolled back a transaction");
                        }
                    }
                }
            }
        }
    }

    fn close_open_stream(&self) -> Result<(), Error> {
        if !self.stream_open() {
            return Ok(());
        }
        self.with_enlisted(|conn| conn.close_stream())
    }

    fn install_canceller(&self) {
        let found = self.with_frames(|frames| {
            let frame = active(frames)?;
            let token = frame.deadline_token;
            let cancel = frame.conn.borrow().as_ref().and_then(|conn| conn.canceller());
            Some((token, cancel?))
        });
        if let Some((token, cancel)) = found {
            deadline_set_cancel(token, cancel);
        }
    }

    /// Time left before the active frame's deadline. `None` with no frame.
    pub fn time_remaining(&self) -> Option<Duration> {
        self.with_frames(|frames| active(frames).map(|frame| frame.deadline.saturating_duration_since(Instant::now())))
    }

    fn arm_statement_timeout(&self, pooled: Option<&mut PooledConn>) -> Result<(), Error> {
        if !self.inner.use_statement_timeout.load(Ordering::Relaxed) {
            return Ok(());
        }
        let Some(pooled) = pooled else {
            return Ok(());
        };
        let remaining = self
            .with_frames(|frames| active(frames).map(|frame| frame.deadline.saturating_duration_since(Instant::now())));
        let remaining = remaining.unwrap_or(DEFAULT_TIMEOUT).max(Duration::from_millis(1));
        pooled.arm_statement_timeout(remaining)
    }

    /// A read waits at most the remaining time plus [`READ_MARGIN`], set once when the connection joins the frame.
    fn arm_read_timeout(&self, pooled: Option<&mut PooledConn>) -> Result<(), Error> {
        let Some(pooled) = pooled else {
            return Ok(());
        };
        let remaining = self.time_remaining().unwrap_or(DEFAULT_TIMEOUT);
        pooled.arm_read_timeout(remaining + READ_MARGIN)
    }

    #[track_caller]
    fn absorb_deadline(&self) {
        let callsite = Location::caller();
        self.with_frames(|frames| {
            for frame in frames.iter_mut() {
                if !frame.fired.load(Ordering::SeqCst) || frame.rollback_only.is_some() {
                    continue;
                }
                frame.rollback_only = Some(RollbackInfo {
                    message: "transaction deadline exceeded".to_string(),
                    error: None,
                    callsite,
                });
                tracing::warn!("transaction deadline exceeded");
            }
        });
    }

    /// Warns when another transaction already touched this primary key. Off unless lock track is on.
    pub fn note_entity_lock(&self, entity: &str, pk: &str) {
        if !self.inner.use_lock_track.load(Ordering::Relaxed) || entity.is_empty() || pk.is_empty() {
            return;
        }
        let thread = std::thread::current().id();
        let frame = self.with_frames(|frames| frames.last().map(|frame| frame.deadline_token).unwrap_or(0));
        if frame == 0 {
            return;
        }
        let mut map = lock(lock_map());
        let key = (entity.to_string(), pk.to_string());
        if let Some(existing) = map.get(&key) {
            if existing.thread != thread || existing.frame != frame {
                tracing::warn!(entity, pk, "two transactions touched the same entity primary key");
            }
        }
        map.insert(key, LockHit { thread, frame });
    }

    /// True when `note_entity_lock` records. Callers check this before building the key.
    pub fn lock_track_enabled(&self) -> bool {
        self.inner.use_lock_track.load(Ordering::Relaxed)
    }

    fn clear_frame_locks(&self, frame: u64) {
        if !self.lock_track_enabled() {
            return;
        }
        let thread = std::thread::current().id();
        let mut map = lock(lock_map());
        map.retain(|_, hit| !(hit.thread == thread && hit.frame == frame));
    }

    fn clear_thread_locks(&self) {
        let thread = std::thread::current().id();
        let mut map = lock(lock_map());
        map.retain(|_, hit| hit.thread != thread);
    }
}

enum HookKind {
    Cache,
    Feed,
    Commit,
    Rollback,
}

/// The frame a scope began or joined, and its join depth just after.
#[derive(Clone, Copy)]
struct FrameMark {
    token: u64,
    join_depth: u32,
    began: bool,
}

/// One [`TransactionManager::begin`], ended by [`Self::commit`] or [`Self::rollback`]. Dropped without
/// either, it rolls back a frame it began, or marks a joined frame rollback-only, and gives the join back.
///
/// Ending checks that the frame is the one the scope began or joined, on top and at its join depth. When
/// code inside left frames or joins open, they are rolled back, the frame is marked rollback-only, and an
/// error is logged. Not `Send`: the frame is on this thread's stack.
#[must_use = "dropping the scope rolls it back"]
pub struct TxScope {
    mgr: TransactionManager,
    mark: FrameMark,
    ended: bool,
    _thread: PhantomData<*const ()>,
}

impl TxScope {
    /// True when this scope began the transaction, false when it joined the one in place.
    pub fn began(&self) -> bool {
        self.mark.began
    }

    /// Commits the frame this scope began. A joined scope gives its join back.
    pub fn commit(self) -> Result<(), Error> {
        self.end(None)
    }

    /// Rolls back the frame this scope began. A joined scope marks the frame rollback-only and gives its
    /// join back.
    pub fn rollback(self) -> Result<(), Error> {
        self.end(Some("rollback in a joined transaction".to_string()))
    }

    fn end(mut self, rollback: Option<String>) -> Result<(), Error> {
        self.ended = true;
        self.mgr.end_frame(self.mark, rollback)
    }
}

impl Drop for TxScope {
    fn drop(&mut self) {
        if self.ended {
            return;
        }
        let message = if std::thread::panicking() {
            "body panicked"
        } else {
            "transaction scope dropped without commit"
        };
        if let Err(err) = self.mgr.end_frame(self.mark, Some(message.to_string())) {
            tracing::warn!(error = %err, "rollback of a dropped transaction scope failed");
        }
    }
}

struct ResumeGuard<'a> {
    mgr: &'a TransactionManager,
    armed: bool,
}

impl Drop for ResumeGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            if let Err(err) = self.mgr.resume() {
                tracing::warn!(error = %err, "resume after ignore failed");
            }
        }
    }
}

fn epoch_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}

fn active(frames: &[Frame]) -> Option<&Frame> {
    match frames.last() {
        Some(frame) if !frame.suspended => Some(frame),
        _ => None,
    }
}

fn active_mut(frames: &mut [Frame]) -> Option<&mut Frame> {
    match frames.last_mut() {
        Some(frame) if !frame.suspended => Some(frame),
        _ => None,
    }
}

fn try_commit(phase: &mut Phase) -> Result<(), Error> {
    let enlisted = phase.conn.borrow().is_some();
    if !enlisted {
        if phase.cache_flush.is_empty() && phase.feed_insert.is_empty() {
            return Ok(());
        }
        return Err(Error::new("no connection enlisted"));
    }
    if let Err(err) = run_conn_hooks(&phase.conn, std::mem::take(&mut phase.cache_flush)) {
        rollback_after_failure(&phase.conn);
        return Err(err);
    }
    if let Err(err) = run_conn_hooks(&phase.conn, std::mem::take(&mut phase.feed_insert)) {
        rollback_after_failure(&phase.conn);
        return Err(err);
    }
    let committed = {
        let mut slot = phase.conn.borrow_mut();
        let pooled = slot.as_mut().ok_or_else(|| Error::new("no connection enlisted"))?;
        pooled.conn().commit()
    };
    if let Err(err) = committed {
        rollback_after_failure(&phase.conn);
        return Err(Error::new("commit failed").with_source(err));
    }
    Ok(())
}

/// The end of a frame for its transaction cache counts (plans/TX-Cache.md D1) and its span: what it did, as attributes.
fn end_trace(phase: &mut Phase, committed: bool) {
    let stats = crate::entity::cache::end_frame_stats(phase.token);
    let Some(trace) = phase.trace.take() else {
        return;
    };
    let mode = match phase.cache_mode {
        CacheMode::Off => "off",
        CacheMode::Hold => "hold",
        CacheMode::ReadThrough => "read-through",
    };
    let count = |value: u64| moqui_lib::object::Value::I64(i64::try_from(value).unwrap_or(i64::MAX));
    let mut attributes = vec![
        ("moqui.transaction.cache", moqui_lib::object::Value::string(mode)),
        (
            "moqui.transaction.round_trips",
            count(crate::driver::thread_round_trips().saturating_sub(phase.round_trips_at)),
        ),
    ];
    if let Some(stats) = stats {
        attributes.extend([
            ("moqui.tx_cache.held", count(stats.held())),
            ("moqui.tx_cache.memory_reads", count(stats.memory_reads)),
            ("moqui.tx_cache.db_reads", count(stats.db_reads)),
            ("moqui.tx_cache.early_flushes", count(stats.early_flush_count())),
            ("moqui.tx_cache.batch_statements", count(stats.batch_statements)),
            ("moqui.tx_cache.batch_round_trips", count(stats.batch_round_trips)),
        ]);
    }
    trace.finish(committed, attributes);
}

/// Runs the after-commit or after-rollback hooks in order. The transaction has already ended, so a panicking hook is
/// logged and the rest still run: it must not skip them or unwind out of the commit that succeeded.
fn run_after_hooks(kind: &str, hooks: impl Iterator<Item = AfterHook>) {
    for hook in hooks {
        if let Err(payload) = std::panic::catch_unwind(AssertUnwindSafe(hook)) {
            let text = payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            tracing::error!(panic = %text, "after-{kind} hook panicked");
        }
    }
}

/// Runs the hooks in order on the enlisted connection. A panicking hook is an error, so the frame still ends.
fn run_conn_hooks(conn: &Rc<RefCell<Option<PooledConn>>>, hooks: Vec<ConnHook>) -> Result<(), Error> {
    for hook in hooks {
        let mut slot = conn.borrow_mut();
        let pooled = slot.as_mut().ok_or_else(|| Error::new("no connection enlisted"))?;
        match std::panic::catch_unwind(AssertUnwindSafe(|| hook(pooled.conn()))) {
            Ok(result) => result?,
            Err(payload) => {
                let text = payload
                    .downcast_ref::<&str>()
                    .map(|text| (*text).to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_default();
                return Err(Error::new(format!("commit hook panicked: {text}")));
            }
        }
    }
    Ok(())
}

fn rollback_after_failure(conn: &Rc<RefCell<Option<PooledConn>>>) {
    if let Err(err) = rollback_enlisted(conn) {
        tracing::warn!(error = %err, "rollback after a failed commit failed");
    }
}

fn rollback_enlisted(conn: &Rc<RefCell<Option<PooledConn>>>) -> Result<(), Error> {
    let mut slot = conn.borrow_mut();
    if let Some(pooled) = slot.as_mut() {
        if !pooled.live() || pooled.conn().is_closed() {
            pooled.discard();
            return Ok(());
        }
        pooled.conn().rollback()?;
    }
    Ok(())
}

fn return_conn(conn: &Rc<RefCell<Option<PooledConn>>>) {
    drop(conn.borrow_mut().take());
}

/// Close the frame's connection instead of returning it: a cancel sent for its statement may still arrive, and on a
/// returned connection it would stop the next borrower's statement (the idle list is last-in first-out). Closed, the
/// session the cancel names is gone.
fn discard_conn(conn: &Rc<RefCell<Option<PooledConn>>>) {
    let Ok(mut slot) = conn.try_borrow_mut() else {
        return;
    };
    if let Some(mut pooled) = slot.take() {
        tracing::debug!("closing a connection whose deadline cancel was sent");
        pooled.discard();
    }
}

/// A process property (or its environment variable: `moqui.tx_cache.strict` is `MOQUI_TX_CACHE_STRICT`) that is
/// `true`.
fn property_flag(name: &str) -> bool {
    let env_name = name.to_ascii_uppercase().replace('.', "_");
    [name, env_name.as_str()].iter().any(|name| {
        moqui_lib::env::get_prop_or_env(name)
            .trim()
            .eq_ignore_ascii_case("true")
    })
}

fn conf_flag(raw: Option<String>, default: bool) -> bool {
    let Some(raw) = raw else {
        return default;
    };
    let expanded = moqui_lib::env::expand(&raw).unwrap_or(raw);
    match expanded.as_str() {
        "true" => true,
        "false" => false,
        _ => default,
    }
}

static NEXT_DEADLINE: AtomicU64 = AtomicU64::new(1);

struct DeadlineSlot {
    when: Instant,
    fired: Arc<AtomicBool>,
    cancel: Option<Box<dyn DbCancel>>,
    /// The timer took `cancel` to send: the frame's connection is closed when the frame ends.
    taken: bool,
}

/// Deadline slots by frame token.
fn deadlines() -> &'static Mutex<HashMap<u64, DeadlineSlot>> {
    static SLOTS: OnceLock<Mutex<HashMap<u64, DeadlineSlot>>> = OnceLock::new();
    SLOTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// True when any manager has a frame on this thread, active or suspended.
pub fn thread_in_transaction() -> bool {
    STACKS.with(|stacks| stacks.borrow().values().any(|frames| !frames.is_empty()))
}

/// True on a target without threads, or when the entered context's factory uses the inline
/// executor. No other thread will return a connection or fire a deadline.
pub fn single_threaded() -> bool {
    cfg!(target_os = "emscripten") || (crate::hooks::get().inline_executor)()
}

/// Single-threaded, the timer thread does not start. A deadline is then only noticed by
/// [`TransactionManager::poll_deadline`] and a running statement is not cancelled: statement
/// timeouts are not enforced (`plans/archive/Wasm.md`).
fn ensure_deadline_timer() {
    if single_threaded() {
        return;
    }
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        let spawned = std::thread::Builder::new()
            .name("moqui-tx-deadline".to_string())
            .spawn(|| loop {
                std::thread::sleep(Duration::from_millis(50));
                // the one timer of the process: a pass that panics is logged, and the next pass runs
                if std::panic::catch_unwind(fire_due_deadlines).is_err() {
                    tracing::error!("a transaction deadline timer pass panicked; the timer goes on");
                }
            });
        if let Err(err) = spawned {
            tracing::error!(error = %err, "could not start the transaction deadline timer");
        }
    });
}

fn register_deadline(token: u64, when: Instant, fired: Arc<AtomicBool>) {
    ensure_deadline_timer();
    let mut slots = lock(deadlines());
    slots.insert(
        token,
        DeadlineSlot {
            when,
            fired,
            cancel: None,
            taken: false,
        },
    );
}

/// True while the deadline timer has a slot for the frame `token` names.
#[cfg(test)]
pub(super) fn deadline_registered(token: u64) -> bool {
    let slots = lock(deadlines());
    slots.contains_key(&token)
}

/// Drop the frame's slot. True when the timer took its canceller, so the frame's connection must not be reused.
fn unregister_deadline(token: u64) -> bool {
    let mut slots = lock(deadlines());
    slots.remove(&token).is_some_and(|slot| slot.taken)
}

fn deadline_set_cancel(token: u64, cancel: Box<dyn DbCancel>) {
    let mut slots = lock(deadlines());
    if let Some(slot) = slots.get_mut(&token) {
        slot.cancel = Some(cancel);
    }
}

fn fire_due_deadlines() {
    let now = Instant::now();
    let mut slots = lock(deadlines());
    let mut due = Vec::new();
    for slot in slots.values_mut() {
        if now >= slot.when && !slot.fired.swap(true, Ordering::SeqCst) {
            if let Some(cancel) = slot.cancel.take() {
                slot.taken = true;
                due.push(cancel);
            }
        }
    }
    drop(slots);
    // Sent by the cancel workers, never here: a cancel waits on the network, and the timer must keep its passes.
    for cancel in due {
        dispatch_cancel(cancel);
    }
}

/// The queue of the cancel workers, started with the first cancel. `None` when no worker could start.
fn cancel_queue() -> Option<&'static std::sync::mpsc::SyncSender<Box<dyn DbCancel>>> {
    static QUEUE: OnceLock<Option<std::sync::mpsc::SyncSender<Box<dyn DbCancel>>>> = OnceLock::new();
    QUEUE
        .get_or_init(|| {
            let (sender, receiver) = std::sync::mpsc::sync_channel::<Box<dyn DbCancel>>(CANCEL_BACKLOG);
            let receiver = Arc::new(Mutex::new(receiver));
            let mut started = 0;
            for index in 0..CANCEL_WORKERS {
                let receiver = Arc::clone(&receiver);
                let spawned = std::thread::Builder::new()
                    .name(format!("moqui-tx-cancel-{index}"))
                    .spawn(move || loop {
                        let next = lock(&receiver).recv();
                        let Ok(cancel) = next else {
                            return;
                        };
                        run_cancel(cancel.as_ref());
                    });
                match spawned {
                    Ok(_) => started += 1,
                    Err(err) => tracing::error!(error = %err, "could not start a transaction cancel worker"),
                }
            }
            (started > 0).then_some(sender)
        })
        .as_ref()
}

/// Hand a due cancel to a worker. With every worker busy and the backlog full it is dropped: its transaction is
/// rollback-only already, and its connection's read timeout bounds the statement's wait.
fn dispatch_cancel(cancel: Box<dyn DbCancel>) {
    let Some(queue) = cancel_queue() else {
        run_cancel(cancel.as_ref());
        return;
    };
    match queue.try_send(cancel) {
        Ok(()) => {}
        Err(std::sync::mpsc::TrySendError::Full(_)) => {
            tracing::warn!("every transaction cancel worker is busy; a deadline cancel is not sent");
        }
        Err(std::sync::mpsc::TrySendError::Disconnected(cancel)) => run_cancel(cancel.as_ref()),
    }
}

/// One cancel, its failure or panic logged.
fn run_cancel(cancel: &dyn DbCancel) {
    match std::panic::catch_unwind(AssertUnwindSafe(|| cancel.cancel())) {
        Ok(Ok(())) => {}
        Ok(Err(err)) => tracing::warn!(error = %err, "deadline cancel failed"),
        Err(_) => tracing::error!("a deadline cancel panicked"),
    }
}

fn lock_map() -> &'static Mutex<HashMap<(String, String), LockHit>> {
    static LOCKS: OnceLock<Mutex<HashMap<(String, String), LockHit>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}
