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

//! Leases one cluster member holds on a named git resource (`moqui.resource.GitLease`; docs/interfaces/Git-Hosting.md,
//! "Shared storage in a cluster"): the writers of an on-disk repository on shared storage, the pass of mirror fetches,
//! a CI pool's starts, a fork against a delete. A lease is taken by one conditional `UPDATE` (or the row's first
//! `INSERT`), a statement of its own committed at once on a reserve connection, so it holds on every database and
//! whatever transaction the caller is in; it is renewed while held and lapses when its holder stops renewing (a
//! member that died). Its times are the database's clock ([`db_now`]), not each member's.
//!
//! **Fencing.** A holder that cannot renew (a lost database connection, a long pause) may find another member took
//! the lease over. So a lease knows when it was last renewed, and is marked lost when a renewal changes no row or
//! none succeeded for nearly its time-to-live; a writer calls [`Lease::check`] right before what must happen under it
//! (a ref update, a push's quarantine taken in, gc's pack rename or delete), which also renews it on the condition that
//! this holder still has it and it has not lapsed, so a writer that lost it stops instead of writing beside the new
//! holder.
//!
//! A thread that holds a lease takes it again for free (a fork that writes to its source in the same transaction):
//! the inner take shares the outer one's state, and the last of them dropped lets it go, wherever it is dropped. Two
//! "members" of a test are two threads. A re-entry of a take that was marked lost takes the lease anew in the database
//! instead. Renewals are scheduled by one thread for the process ([`renewals`]), each made on a thread of its own and
//! cut off past a sixth of the lease's time ([`renew_limit`]), so one renewal the database holds does not hold the
//! others; a lease let go within its first renewal interval costs no renewal at all.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use moqui_core::context::Error;
use moqui_core::entity::{Condition, EntityFacade};
use moqui_core::transaction::TransactionManager;
use moqui_lib::object::Value;

use crate::conditional::{update_where, Commit};

pub(crate) const LEASE: &str = "moqui.resource.GitLease";

/// How long a lease holds past its last renewal.
pub(crate) const TTL: Duration = Duration::from_secs(60);

/// How long a writer waits for another member's write to the same repository, unless the conf's `git-server`
/// `shared-lock-wait-seconds` says otherwise.
pub(crate) const DEFAULT_WAIT: Duration = Duration::from_secs(120);

/// How often the database's clock is read again for [`db_now`].
const CLOCK_FRESH: Duration = Duration::from_secs(30);

thread_local! {
    /// The leases this thread holds, by name: the state each take shares with its re-entries.
    static HELD: RefCell<HashMap<String, Weak<Held>>> = RefCell::new(HashMap::new());
}

/// What a take and its re-entries on its thread share; the last one dropped lets the lease go.
struct Held {
    entity: EntityFacade,
    name: String,
    holder: String,
    ttl: Duration,
    state: Arc<State>,
    /// Its entry in the process's renewals, while it has one.
    renewal: u64,
}

/// A lease's standing, which the renewer and its writers both read.
struct State {
    /// Cleared when a renewal or a check found another holder, or none succeeded for nearly the time-to-live.
    valid: AtomicBool,
    /// When it was last taken or renewed, by this process's monotonic clock.
    last_ok: Mutex<Instant>,
    /// Set as it is let go, so a renewal racing the release is not taken for a loss.
    released: AtomicBool,
}

impl State {
    fn new() -> Self {
        Self {
            valid: AtomicBool::new(true),
            last_ok: Mutex::new(Instant::now()),
            released: AtomicBool::new(false),
        }
    }

    fn renewed(&self) {
        *moqui_lib::sync::lock(&self.last_ok) = Instant::now();
    }

    /// Whether it has gone too long without a renewal to be trusted: within a sixth of its time-to-live of lapsing.
    fn stale(&self, ttl: Duration) -> bool {
        moqui_lib::sync::lock(&self.last_ok).elapsed() > ttl.saturating_sub(ttl / 6)
    }
}

/// A lease held until dropped (or until the transaction it was handed to ends, [`Lease::until_transaction_end`]).
/// It may be dropped on another thread (a transaction's end hook): the state is shared, not the thread's.
#[must_use = "dropping a lease lets it go"]
pub(crate) struct Lease {
    held: Arc<Held>,
}

impl std::fmt::Debug for Lease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lease")
            .field("name", &self.held.name)
            .field("holder", &self.held.holder)
            .finish()
    }
}

fn stamp(millis: i64) -> Value {
    moqui_core::user::stamp_value(millis)
}

fn ttl_millis(ttl: Duration) -> i64 {
    i64::try_from(ttl.as_millis()).unwrap_or(i64::MAX)
}

fn local_millis() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

/// The database server's time in epoch milliseconds, for the lease times: every member compares them by the one
/// clock, whatever its own says. Read again every [`CLOCK_FRESH`] (by the statement the database's dialect takes,
/// `Dialect::current_timestamp_sql`) and kept as an offset from this host's clock; this host's own when the database
/// answers with no time it tells (a test's in-memory one). A read that fails keeps the offset last read, is not kept
/// (the next call reads again) and is warned of, so a failure does not put the member on its own clock.
pub(crate) fn db_now(entity: &EntityFacade) -> i64 {
    db_now_within(entity, None)
}

/// [`db_now`], a read of the database's clock waiting at most `limit` (a renewal's [`renew_limit`], where the
/// connection's driver can bound a read: `DbConnection::set_read_timeout`).
fn db_now_within(entity: &EntityFacade, limit: Option<Duration>) -> i64 {
    static OFFSET: Mutex<Option<(Instant, i64)>> = Mutex::new(None);
    static WARNED: Mutex<Option<Instant>> = Mutex::new(None);
    let cached = *moqui_lib::sync::lock(&OFFSET);
    if let Some((at, offset)) = cached {
        if at.elapsed() < CLOCK_FRESH {
            return local_millis() + offset;
        }
    }
    let read = read_db_millis(entity, limit).map(|db| db.map(|db| db - local_millis()));
    let (offset, keep) = next_offset(
        read.as_ref().map(|offset| *offset).map_err(Error::message),
        cached.map(|(_, offset)| offset),
    );
    if keep {
        *moqui_lib::sync::lock(&OFFSET) = Some((Instant::now(), offset));
    } else if let Err(err) = &read {
        // once each CLOCK_FRESH at most: every lease call reads again while it fails
        let mut warned = moqui_lib::sync::lock(&WARNED);
        if warned.is_none_or(|at| at.elapsed() >= CLOCK_FRESH) {
            *warned = Some(Instant::now());
            tracing::warn!(
                "the database's clock was not read for the git lease times; the offset last read ({offset} ms) is \
                 kept: {err}"
            );
        }
    }
    local_millis() + offset
}

/// The database server's time, by the statement its dialect takes: `None` when it answers with no time this can read.
/// With `limit`, the read waits at most that long where the driver bounds a read (the connection is closed past it).
fn read_db_millis(entity: &EntityFacade, limit: Option<Duration>) -> Result<Option<i64>, Error> {
    let (sql, tx) = entity
        .with_group(LEASE, |_, dialect, _, tx| {
            Ok((dialect.current_timestamp_sql(), tx.clone()))
        })
        .map_err(Error::from)?;
    let rows = tx.with_reserve(|conn| {
        let Some(limit) = limit else {
            return conn.query(sql, &[]);
        };
        conn.set_read_timeout(Some(limit))?;
        let rows = conn.query(sql, &[]);
        // back to the pool with no limit (a connection that timed out is closed and not kept)
        if !conn.is_closed() {
            if let Err(err) = conn.set_read_timeout(None) {
                tracing::debug!("a reserve connection's read timeout was not cleared: {err}");
            }
        }
        rows
    })?;
    Ok(rows
        .first()
        .and_then(|row| row.columns.first())
        .and_then(|(_, value)| moqui_core::context::millis_of(value)))
}

/// The offset of the database's clock to use after a read (`Ok(None)`: it answered with no time), given the one last
/// read, and whether to keep it for [`CLOCK_FRESH`]: a failed read keeps the previous offset (none: this host's clock)
/// and is not kept.
fn next_offset(read: Result<Option<i64>, &str>, previous: Option<i64>) -> (i64, bool) {
    match read {
        Ok(Some(offset)) => (offset, true),
        Ok(None) => (0, true),
        Err(_) => (previous.unwrap_or(0), false),
    }
}

/// A holder id no other take shares, on any member: this process and 128 random bits.
fn new_holder() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // no random bytes: the time and a counter, which still differ between this process's takes
        static TAKES: AtomicU64 = AtomicU64::new(0);
        bytes[..8].copy_from_slice(&local_millis().to_le_bytes());
        bytes[8..].copy_from_slice(&TAKES.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    }
    format!("{}-{}", std::process::id(), moqui_lib::codec::hex_encode(&bytes))
}

/// Set `fields` on the lease row `name` when `condition` holds as well, a statement committed at once: how many rows
/// changed.
fn update(entity: &EntityFacade, name: &str, fields: Vec<(&str, Value)>, condition: Condition) -> Result<u64, Error> {
    let condition = Condition::and(vec![Condition::equals("leaseName", Value::string(name)), condition]);
    update_where(entity, LEASE, &fields, &condition, Commit::Own)
}

/// The longest a renewal of a lease of `ttl` waits for the database: a sixth of its time, from 1 to 10 seconds, so a
/// renewal the database holds fails well before the lease goes stale and the next one is tried. It holds where the
/// driver bounds a read (`DbConnection::set_read_timeout`: Postgres, the embedded one too); MySQL's client cannot change
/// a connection's read timeout once open, so there a renewal waits up to the datasource's `read-timeout` (600 s by
/// default), and the renewer marks the lease lost when it goes stale meanwhile (docs/interfaces/Git-Hosting.md).
fn renew_limit(ttl: Duration) -> Duration {
    (ttl / 6).clamp(Duration::from_secs(1), Duration::from_secs(10))
}

/// Renew the lease `name` of `holder` for `ttl` from the database's now, on the condition that it is still this
/// holder's and has not lapsed, the clock's read and the update each within [`renew_limit`]: whether it was.
fn renew(entity: &EntityFacade, name: &str, holder: &str, ttl: Duration) -> Result<bool, Error> {
    #[cfg(test)]
    tests::hold_renewal(name);
    let now = db_now_within(entity, Some(renew_limit(ttl)));
    let condition = Condition::and(vec![
        Condition::equals("leaseName", Value::string(name)),
        Condition::equals("holderId", Value::string(holder)),
        Condition::greater("leaseUntil", stamp(now)),
    ]);
    let changed = update_where(
        entity,
        LEASE,
        &[("leaseUntil", stamp(now + ttl_millis(ttl)))],
        &condition,
        Commit::OwnWithin(renew_limit(ttl)),
    )?;
    Ok(changed > 0)
}

/// One try at the lease `name` for `holder` for `ttl`: free, lapsed or its own. Whether it was taken.
fn attempt(entity: &EntityFacade, name: &str, holder: &str, ttl: Duration) -> Result<bool, Error> {
    let now = db_now(entity);
    let free = Condition::or(vec![
        Condition::is_null("holderId"),
        Condition::less("leaseUntil", stamp(now)),
        Condition::equals("holderId", Value::string(holder)),
    ]);
    let fields = vec![
        ("holderId", Value::string(holder)),
        ("leaseUntil", stamp(now + ttl_millis(ttl))),
        ("takenDate", stamp(now)),
    ];
    if update(entity, name, fields.clone(), free.clone())? > 0 {
        return Ok(true);
    }
    // no row yet: make it, free, then take it as any other; of two members making it at the same moment one insert
    // fails on the key and the other's row is taken by one update
    let there = entity
        .find(LEASE)?
        .eq("leaseName", Value::string(name))
        .use_cache(false)
        .disable_authz()
        .count()?
        > 0;
    if there {
        return Ok(false);
    }
    let columns = ["leaseName", "lastUpdatedStamp"];
    let (sql, tx) = entity.with_group(LEASE, |definition, dialect, _, tx| {
        Ok((moqui_db::sql::insert(definition, dialect, &columns)?, tx.clone()))
    })?;
    if let Err(err) = tx.with_reserve(|conn| conn.execute(&sql, &[Value::string(name), stamp(now)])) {
        tracing::debug!(lease = name, "a git lease row was made by another: {err}");
    }
    Ok(update(entity, name, fields, free)? > 0)
}

/// Take the lease `name` now when no other holder has it; `None` when another does. Renewed every third of `ttl`
/// while held.
///
/// # Errors
///
/// When the lease row cannot be read or written.
pub(crate) fn try_take(entity: &EntityFacade, name: &str, ttl: Duration) -> Result<Option<Lease>, Error> {
    if let Some(lease) = reentered(name) {
        return Ok(Some(lease));
    }
    let holder = new_holder();
    if !attempt(entity, name, &holder, ttl)? {
        return Ok(None);
    }
    Ok(Some(held(entity, name, holder, ttl)))
}

/// Take the lease `name`, waiting up to `wait` for another holder to let it go.
///
/// # Errors
///
/// `GitLeaseBusy` when another holder keeps it past `wait`, and a lease row that cannot be read or written.
pub(crate) fn take(entity: &EntityFacade, name: &str, ttl: Duration, wait: Duration) -> Result<Lease, Error> {
    if let Some(lease) = reentered(name) {
        return Ok(lease);
    }
    let holder = new_holder();
    let deadline = Instant::now() + wait;
    let mut pause = Duration::from_millis(5);
    loop {
        if attempt(entity, name, &holder, ttl)? {
            return Ok(held(entity, name, holder, ttl));
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(Error::new(format!(
                "GitLeaseBusy: {name} is held by another cluster member or writer past {} seconds",
                wait.as_secs()
            )));
        }
        std::thread::sleep(pause.min(deadline - now));
        pause = (pause * 2).min(Duration::from_millis(200));
    }
}

/// A lease this thread holds already: taken again, sharing the outer take's state. One marked lost is not shared: the
/// caller takes the lease anew in the database (it may have lapsed and be free, or another member holds it now), and
/// the new take's state is what later re-entries share; the outer take still sees its own as lost.
fn reentered(name: &str) -> Option<Lease> {
    HELD.with(|held| {
        let mut held = held.borrow_mut();
        match held.get(name).map(Weak::upgrade) {
            Some(Some(shared)) if shared.state.valid.load(Ordering::Acquire) => Some(Lease { held: shared }),
            Some(_) => {
                // let go elsewhere (its transaction's end hook on another thread), or lost: not held here any more
                held.remove(name);
                None
            }
            None => None,
        }
    })
}

fn held(entity: &EntityFacade, name: &str, holder: String, ttl: Duration) -> Lease {
    let state = Arc::new(State::new());
    let renewal = renewals().add(Renewal {
        entity: entity.clone(),
        name: name.to_string(),
        holder: holder.clone(),
        ttl,
        due: Instant::now() + every(ttl),
        state: Arc::clone(&state),
        in_flight: false,
    });
    let shared = Arc::new(Held {
        entity: entity.clone(),
        name: name.to_string(),
        holder,
        ttl,
        state,
        renewal,
    });
    HELD.with(|held| held.borrow_mut().insert(name.to_string(), Arc::downgrade(&shared)));
    Lease { held: shared }
}

/// How often a lease of `ttl` is renewed.
fn every(ttl: Duration) -> Duration {
    (ttl / 3).max(Duration::from_millis(100))
}

impl Lease {
    /// Whether this lease is still this holder's, right before a write that must happen under it: not marked lost,
    /// renewed recently enough, and renewed now on the condition that the row still names this holder and has not
    /// lapsed by the database's clock (a fencing check: a holder that lost it to another member stops here).
    ///
    /// # Errors
    ///
    /// `GitLeaseLost` when it is not, and a lease row that cannot be written (the write is not made unverified).
    pub(crate) fn check(&self) -> Result<(), Error> {
        let held = &self.held;
        let lost = || {
            held.state.valid.store(false, Ordering::Release);
            Error::new(format!(
                "GitLeaseLost: the lease {} lapsed or was taken over by another cluster member before this write; \
                 nothing was written, try again",
                held.name
            ))
        };
        if !held.state.valid.load(Ordering::Acquire) || held.state.stale(held.ttl) {
            return Err(lost());
        }
        if !renew(&held.entity, &held.name, &held.holder, held.ttl)? {
            return Err(lost());
        }
        held.state.renewed();
        Ok(())
    }

    /// Keep the lease until the transaction in place ends (committed or rolled back), so another member that takes it
    /// next reads what this transaction wrote; with no transaction it is let go when dropped, as usual.
    ///
    /// # Errors
    ///
    /// When the transaction's end hooks cannot be added (the lease is then let go at once).
    pub(crate) fn until_transaction_end(self, transactions: &TransactionManager) -> Result<(), Error> {
        if !transactions.in_transaction() {
            return Ok(());
        }
        let kept = Arc::new(Mutex::new(Some(self)));
        let on_rollback = Arc::clone(&kept);
        transactions.on_commit(move || drop(moqui_lib::sync::lock(&kept).take()))?;
        transactions.on_rollback(move || drop(moqui_lib::sync::lock(&on_rollback).take()))?;
        Ok(())
    }
}

/// [`Lease::check`] of a lease a writer may hold (`None`: a `db` repository, or no shared storage).
pub(crate) fn check(lease: Option<&Lease>) -> Result<(), Error> {
    lease.map_or(Ok(()), Lease::check)
}

impl Drop for Held {
    fn drop(&mut self) {
        self.state.released.store(true, Ordering::Release);
        renewals().remove(self.renewal);
        let _ = HELD.try_with(|held| {
            let mut held = held.borrow_mut();
            if held.get(&self.name).is_some_and(|weak| weak.strong_count() == 0) {
                held.remove(&self.name);
            }
        });
        let released = update(
            &self.entity,
            &self.name,
            vec![("holderId", Value::Null), ("leaseUntil", Value::Null)],
            Condition::equals("holderId", Value::string(&self.holder)),
        );
        if let Err(err) = released {
            tracing::warn!(lease = %self.name, "a git lease was not let go (it lapses): {err}");
        }
    }
}

/// A held lease's renewal, kept by the process's renewer.
struct Renewal {
    entity: EntityFacade,
    name: String,
    holder: String,
    ttl: Duration,
    due: Instant,
    state: Arc<State>,
    /// Its renewal is being made, on a thread of its own: no other is started until it ends.
    in_flight: bool,
}

/// The renewals of the process's held leases, scheduled by one thread ([`renewals`]).
struct Renewals {
    entries: Mutex<HashMap<u64, Renewal>>,
    changed: Condvar,
    next_id: AtomicU64,
}

/// After a renewal that failed (the database did not answer), the next try comes this soon.
const RETRY: Duration = Duration::from_secs(1);

impl Renewals {
    fn add(&self, renewal: Renewal) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        moqui_lib::sync::lock(&self.entries).insert(id, renewal);
        self.changed.notify_all();
        id
    }

    fn remove(&self, id: u64) {
        moqui_lib::sync::lock(&self.entries).remove(&id);
    }

    /// The renewer's loop: start each lease's renewal when it is due, outside the lock, each on a thread of its own
    /// ([`Self::renew_one`]), so a renewal the database holds (up to [`renew_limit`]) holds no other lease's. A lease
    /// whose renewal has not ended for nearly its time is marked lost here.
    fn run(&'static self) {
        loop {
            let due: Vec<(u64, EntityFacade, String, String, Duration, Arc<State>)> = {
                let mut entries = moqui_lib::sync::lock(&self.entries);
                let now = Instant::now();
                let mut waiting = false;
                for entry in entries.values().filter(|entry| entry.in_flight) {
                    waiting = true;
                    let state = &entry.state;
                    if state.stale(entry.ttl)
                        && !state.released.load(Ordering::Acquire)
                        && state.valid.swap(false, Ordering::AcqRel)
                    {
                        tracing::error!(
                            lease = %entry.name,
                            "a git lease's renewal has not ended for nearly its time: its writes stop"
                        );
                    }
                }
                // a lease marked lost (by a check, or above) is not renewed again: its row is let go, so the other
                // members can take it and a re-entry here takes it anew
                let lost: Vec<u64> = entries
                    .iter()
                    .filter(|(_, entry)| {
                        !entry.in_flight
                            && !entry.state.valid.load(Ordering::Acquire)
                            && !entry.state.released.load(Ordering::Acquire)
                    })
                    .map(|(id, _)| *id)
                    .collect();
                if !lost.is_empty() {
                    let gone: Vec<Renewal> = lost.iter().filter_map(|id| entries.remove(id)).collect();
                    drop(entries);
                    for entry in gone {
                        release_lost(&entry.entity, &entry.name, &entry.holder);
                    }
                    continue;
                }
                let next = entries
                    .values()
                    .filter(|entry| !entry.in_flight)
                    .map(|entry| entry.due)
                    .min();
                // a renewal in flight is looked at again within RETRY, to mark it lost in time
                let cap = if waiting { RETRY } else { Duration::from_secs(60) };
                match next {
                    Some(next) if next <= now => {}
                    Some(next) => {
                        let _ = moqui_lib::sync::wait_timeout(&self.changed, entries, (next - now).min(cap));
                        continue;
                    }
                    None => {
                        let _ = moqui_lib::sync::wait_timeout(&self.changed, entries, cap);
                        continue;
                    }
                }
                entries
                    .iter_mut()
                    .filter(|(_, entry)| entry.due <= now && !entry.in_flight)
                    .map(|(id, entry)| {
                        entry.in_flight = true;
                        // pushed back now, so one that fails is not tried again at once
                        entry.due = now + RETRY;
                        (
                            *id,
                            entry.entity.clone(),
                            entry.name.clone(),
                            entry.holder.clone(),
                            entry.ttl,
                            Arc::clone(&entry.state),
                        )
                    })
                    .collect()
            };
            for (id, entity, name, holder, ttl, state) in due {
                let spawned = std::thread::Builder::new()
                    .name("git-lease-renew".into())
                    .spawn(move || self.renew_one(id, &entity, &name, &holder, ttl, &state));
                if let Err(err) = spawned {
                    tracing::warn!("a git lease's renewal thread did not start, trying again: {err}");
                    if let Some(entry) = moqui_lib::sync::lock(&self.entries).get_mut(&id) {
                        entry.in_flight = false;
                    }
                }
            }
        }
    }

    /// One lease's renewal, and what it leaves: due again in a third of its time, tried again soon after a failure,
    /// or gone when it was lost or let go.
    fn renew_one(&self, id: u64, entity: &EntityFacade, name: &str, holder: &str, ttl: Duration, state: &State) {
        let renewed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| renew(entity, name, holder, ttl)));
        let ok = matches!(renewed, Ok(Ok(true)));
        let keep = match renewed {
            Ok(Ok(true)) => {
                state.renewed();
                true
            }
            Ok(Ok(false)) => {
                // let go as this ran, or lost: lapsed, and maybe taken over by another member
                if !state.released.load(Ordering::Acquire) {
                    state.valid.store(false, Ordering::Release);
                    tracing::error!(lease = %name, "a git lease was lost before it was let go: its writes stop");
                }
                false
            }
            Ok(Err(err)) => {
                tracing::warn!(lease = %name, "a git lease was not renewed, trying again: {err}");
                true
            }
            Err(_) => {
                tracing::error!(lease = %name, "renewing a git lease panicked");
                true
            }
        };
        if keep && state.stale(ttl) && !state.released.load(Ordering::Acquire) {
            // no renewal for nearly its time: another member may take it over, so it is not written under
            state.valid.store(false, Ordering::Release);
            tracing::error!(lease = %name, "a git lease went unrenewed for nearly its time: its writes stop");
        }
        // marked lost while this renewal ran (by the loop, by a check) or just now: a renewal that went through
        // anyway does not keep the row held for a holder whose writes have stopped
        let lost = !state.valid.load(Ordering::Acquire) && !state.released.load(Ordering::Acquire);
        let keep = keep && !lost;
        if lost {
            release_lost(entity, name, holder);
        }
        {
            let mut entries = moqui_lib::sync::lock(&self.entries);
            if keep {
                if let Some(entry) = entries.get_mut(&id) {
                    entry.in_flight = false;
                    if ok {
                        entry.due = Instant::now() + every(ttl);
                    }
                }
            } else {
                entries.remove(&id);
            }
        }
        self.changed.notify_all();
    }
}

/// Let go of the row of a lease marked lost, on the condition that this holder still has it (another member may have
/// taken it over already). A failure is logged: the row lapses at its time.
fn release_lost(entity: &EntityFacade, name: &str, holder: &str) {
    let released = update(
        entity,
        name,
        vec![("holderId", Value::Null), ("leaseUntil", Value::Null)],
        Condition::equals("holderId", Value::string(holder)),
    );
    if let Err(err) = released {
        tracing::warn!(lease = %name, "a lost git lease was not let go (it lapses): {err}");
    }
}

/// The process's renewer, started with the first lease held.
fn renewals() -> &'static Renewals {
    static RENEWALS: OnceLock<&'static Renewals> = OnceLock::new();
    RENEWALS.get_or_init(|| {
        let renewals: &'static Renewals = Box::leak(Box::new(Renewals {
            entries: Mutex::new(HashMap::new()),
            changed: Condvar::new(),
            next_id: AtomicU64::new(1),
        }));
        if let Err(err) = std::thread::Builder::new()
            .name("git-lease".into())
            .spawn(move || renewals.run())
        {
            tracing::error!("git leases are not renewed (no thread): {err}");
        }
        renewals
    })
}

/// The lease of the writers of repository `repository_id`.
pub(crate) fn repository_name(repository_id: &str) -> String {
    format!("repository:{repository_id}")
}

#[cfg(test)]
#[path = "lease_tests.rs"]
mod tests;
