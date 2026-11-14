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

//! One conditional `UPDATE` for the crate: set fields on the rows a condition matches and say how many changed. A
//! lease's take, renewal and fencing check (`lease.rs`), and a CI row's move from one status (`ci::update_when`), are
//! each one such statement, so of two members only one changes the row.

use moqui_core::context::Error;
use moqui_core::entity::{Condition, EntityFacade};
use moqui_lib::object::Value;

/// Where a conditional update is committed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Commit {
    /// In the caller's transaction, if any.
    Caller,
    /// A statement of its own on a reserve connection, committed at once: a claim other members see before the
    /// caller's transaction ends.
    Own,
    /// [`Commit::Own`] with no more than this long waited for the database's answer (the connection's read timeout:
    /// a server that does not answer, or a row lock held that long, fails the statement and the connection is
    /// closed), so a lease's renewal is never held past its time.
    OwnWithin(std::time::Duration),
}

/// `UPDATE entity_name SET fields, lastUpdatedStamp WHERE condition`, committed as `commit` says: how many rows
/// changed. `lastUpdatedStamp` is the caller's transaction's start time for [`Commit::Caller`], as an entity update in
/// that transaction stamps it, else now.
pub(crate) fn update_where(
    entity: &EntityFacade,
    entity_name: &str,
    fields: &[(&str, Value)],
    condition: &Condition,
    commit: Commit,
) -> Result<u64, Error> {
    let (statement, tx, stamped) = entity.with_group(entity_name, |definition, dialect, _, tx| {
        let stamped = definition.has_field("lastUpdatedStamp");
        let mut names: Vec<&str> = fields.iter().map(|(name, _)| *name).collect();
        if stamped {
            names.push("lastUpdatedStamp");
        }
        Ok((
            moqui_db::sql::update(definition, dialect, &names, Some(condition))?,
            tx.clone(),
            stamped,
        ))
    })?;
    let mut params: Vec<Value> = fields.iter().map(|(_, value)| value.clone()).collect();
    if stamped {
        let now = || jiff::Timestamp::now().as_millisecond();
        let millis = match commit {
            Commit::Caller => tx.start_time_millis().unwrap_or_else(now),
            Commit::Own | Commit::OwnWithin(_) => now(),
        };
        params.push(moqui_core::user::stamp_value(millis));
    }
    params.extend(statement.params);
    Ok(match commit {
        Commit::Caller => {
            // In a transaction-cache frame the caller's entity writes are held until its commit, and this statement
            // reads none of them (a row made just before matches nothing) and changes rows the frame keeps: write the
            // held rows first and forget what the frame read, as raw SQL in a frame does (plans/archive/TX-Cache.md D1)
            moqui_core::entity::cache::flush_before_sql(entity, &tx)?;
            moqui_core::entity::exec::execute_tx(&tx, &statement.sql, &params)?
        }
        Commit::Own => tx.with_reserve(|conn| conn.execute(&statement.sql, &params))?,
        Commit::OwnWithin(limit) => tx.with_reserve(|conn| {
            conn.set_read_timeout(Some(limit))?;
            let changed = conn.execute(&statement.sql, &params);
            // back to the pool with no limit (a connection that timed out is closed and not kept)
            if !conn.is_closed() {
                if let Err(err) = conn.set_read_timeout(None) {
                    tracing::debug!("a reserve connection's read timeout was not cleared: {err}");
                }
            }
            changed
        })?,
    })
}
