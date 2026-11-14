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
//! member that died). The lease times are this host's clock: members keep their clocks within seconds, far less than
//! [`TTL`].
//!
//! A thread that holds a lease takes it again for free (a fork that writes to its source in the same transaction), so
//! two "members" of a test are two threads.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_core::context::Error;
use moqui_core::entity::{Condition, EntityFacade};
use moqui_core::transaction::TransactionManager;
use moqui_lib::object::Value;

pub(crate) const LEASE: &str = "moqui.resource.GitLease";

/// How long a lease holds past its last renewal.
pub(crate) const TTL: Duration = Duration::from_secs(60);

/// How long a writer waits for another member's write to the same repository, unless the conf's `git-server`
/// `shared-lock-wait-seconds` says otherwise.
pub(crate) const DEFAULT_WAIT: Duration = Duration::from_secs(120);

thread_local! {
    /// The leases this thread holds, by name, with how many times it took each.
    static HELD: RefCell<HashMap<String, usize>> = RefCell::new(HashMap::new());
}

/// A lease held until dropped (or until the transaction it was handed to ends, [`Lease::until_transaction_end`]).
#[must_use = "dropping a lease lets it go"]
pub(crate) struct Lease {
    entity: EntityFacade,
    name: String,
    holder: String,
    renewer: Option<mpsc::Sender<()>>,
}

impl std::fmt::Debug for Lease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lease")
            .field("name", &self.name)
            .field("holder", &self.holder)
            .finish()
    }
}

fn now_millis() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

fn stamp(millis: i64) -> Value {
    moqui_core::user::stamp_value(millis)
}

fn ttl_millis(ttl: Duration) -> i64 {
    i64::try_from(ttl.as_millis()).unwrap_or(i64::MAX)
}

/// A holder id no other take shares: this process and a counter.
fn new_holder() -> String {
    static TAKES: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}-{}",
        std::process::id(),
        now_millis(),
        TAKES.fetch_add(1, Ordering::Relaxed)
    )
}

/// Run one statement on a reserve connection, committed at once.
fn execute(tx: &TransactionManager, sql: &str, params: &[Value]) -> Result<u64, Error> {
    Ok(tx.with_reserve(|conn| conn.execute(sql, params))?)
}

/// Set `fields` on the lease row `name` when `condition` holds as well: how many rows changed.
fn update(entity: &EntityFacade, name: &str, fields: Vec<(&str, Value)>, condition: Condition) -> Result<u64, Error> {
    let condition = Condition::and(vec![Condition::equals("leaseName", Value::string(name)), condition]);
    let mut names: Vec<&str> = fields.iter().map(|(name, _)| *name).collect();
    names.push("lastUpdatedStamp");
    let (statement, tx) = entity.with_group(LEASE, |definition, dialect, _, tx| {
        Ok((
            moqui_db::sql::update(definition, dialect, &names, Some(&condition))?,
            tx.clone(),
        ))
    })?;
    let mut params: Vec<Value> = fields.into_iter().map(|(_, value)| value).collect();
    params.push(stamp(now_millis()));
    params.extend(statement.params);
    execute(&tx, &statement.sql, &params)
}

/// One try at the lease `name` for `holder` until `until`: free, lapsed or its own. Whether it was taken.
fn attempt(entity: &EntityFacade, name: &str, holder: &str, until: i64) -> Result<bool, Error> {
    let now = now_millis();
    let free = Condition::or(vec![
        Condition::is_null("holderId"),
        Condition::less("leaseUntil", stamp(now)),
        Condition::equals("holderId", Value::string(holder)),
    ]);
    let fields = vec![
        ("holderId", Value::string(holder)),
        ("leaseUntil", stamp(until)),
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
    if let Err(err) = execute(&tx, &sql, &[Value::string(name), stamp(now)]) {
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
    if let Some(lease) = reentered(entity, name) {
        return Ok(Some(lease));
    }
    let holder = new_holder();
    if !attempt(entity, name, &holder, now_millis() + ttl_millis(ttl))? {
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
    if let Some(lease) = reentered(entity, name) {
        return Ok(lease);
    }
    let holder = new_holder();
    let deadline = Instant::now() + wait;
    let mut pause = Duration::from_millis(5);
    loop {
        if attempt(entity, name, &holder, now_millis() + ttl_millis(ttl))? {
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

/// A lease this thread holds already: taken again, let go when the outer one is.
fn reentered(entity: &EntityFacade, name: &str) -> Option<Lease> {
    HELD.with(|held| {
        let mut held = held.borrow_mut();
        let count = held.get_mut(name)?;
        *count += 1;
        Some(Lease {
            entity: entity.clone(),
            name: name.to_string(),
            holder: String::new(),
            renewer: None,
        })
    })
}

fn held(entity: &EntityFacade, name: &str, holder: String, ttl: Duration) -> Lease {
    HELD.with(|held| held.borrow_mut().insert(name.to_string(), 1));
    let (stop, stopped) = mpsc::channel::<()>();
    let (renew_entity, renew_name, renew_holder) = (entity.clone(), name.to_string(), holder.clone());
    let every = (ttl / 3).max(Duration::from_millis(100));
    let spawned = std::thread::Builder::new()
        .name("git-lease".into())
        .spawn(move || loop {
            match stopped.recv_timeout(every) {
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let until = now_millis() + ttl_millis(ttl);
                    let renewed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        update(
                            &renew_entity,
                            &renew_name,
                            vec![("leaseUntil", stamp(until))],
                            Condition::equals("holderId", Value::string(&renew_holder)),
                        )
                    }));
                    match renewed {
                        Ok(Ok(1)) => {}
                        Ok(Ok(_)) => {
                            tracing::error!(lease = %renew_name, "a git lease was lost before it was let go");
                            return;
                        }
                        Ok(Err(err)) => tracing::warn!(lease = %renew_name, "a git lease was not renewed: {err}"),
                        Err(_) => tracing::error!(lease = %renew_name, "renewing a git lease panicked"),
                    }
                }
                _ => return,
            }
        });
    let renewer = match spawned {
        Ok(_) => Some(stop),
        Err(err) => {
            tracing::warn!(lease = name, "a git lease is not renewed (no thread): {err}");
            None
        }
    };
    Lease {
        entity: entity.clone(),
        name: name.to_string(),
        holder,
        renewer,
    }
}

impl Lease {
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

impl Drop for Lease {
    fn drop(&mut self) {
        let outer = HELD.with(|held| {
            let mut held = held.borrow_mut();
            match held.get_mut(&self.name) {
                Some(count) if *count > 1 => {
                    *count -= 1;
                    false
                }
                Some(_) => {
                    held.remove(&self.name);
                    true
                }
                None => !self.holder.is_empty(),
            }
        });
        if !outer || self.holder.is_empty() {
            return;
        }
        if let Some(stop) = self.renewer.take() {
            let _ = stop.send(());
        }
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

/// The lease of the writers of repository `repository_id`.
pub(crate) fn repository_name(repository_id: &str) -> String {
    format!("repository:{repository_id}")
}

#[cfg(test)]
#[path = "lease_tests.rs"]
mod tests;
