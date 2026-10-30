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

//! Transaction manager and connection pool. They call [`crate::driver`] and do not import a database client.
//!
//! An open stream is the enlisted connection. [`TransactionManager::with_connection`] returns that
//! connection while it is open and does not check out a second one. Beginning another frame is not
//! how a stream gets a connection.
//!
//! No JTA types, no XA, and no `PREPARE TRANSACTION`. Savepoints forward to the enlisted connection.
//! [`TransactionManager::force_new`] is a suspend of the current frame plus a new frame, not a savepoint.

mod manager;
mod pool;

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub use manager::single_threaded;
pub use manager::{thread_in_transaction, TransactionAttribute, TransactionManager, TxScope};
pub use pool::{Pool, PoolConfig, PoolStats};

#[cfg(test)]
mod tests;
