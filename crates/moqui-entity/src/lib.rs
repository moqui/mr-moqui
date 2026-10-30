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

//! The entity facade ([`entity`]), the transaction manager and connection pool ([`transaction`]) and the port a
//! database driver implements ([`driver`]), below moqui-core (plans/Crate-Split.md). No ExecutionContext: where the
//! facade needs the context entered on the thread (the user, authorization, scripts, services, entity ECA rules) it
//! calls [`hooks`], which core fills. moqui-core re-exports these modules at its own paths, `moqui_core::entity` and so
//! on, and code above core uses those.

pub mod driver;
pub mod entity;
pub mod error;
pub mod hooks;
pub mod period;
pub mod resource;
// the in-memory entity facade of the tests, here and in the crates above (the test-support feature)
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod testing;
pub mod transaction;
