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

//! Browser host. `plans/archive/Wasm.md`.
//!
//! [`sqlite`] is the local [`moqui_core::driver::DbDriver`]. [`host`] boots a factory on the inline
//! executor over it. [`fetch`] sends outbound HTTP over `fetch` (JSPI). [`api`] is the JSON request
//! surface the emscripten exports call. [`ws`] carries WebSocket frames between the worker's sockets
//! and the module.

// The reports. Naming the crate keeps its moqui_core::extension entry in the module.
use moqui_report as _;

pub mod api;
pub mod fetch;
pub mod host;
pub mod sqlite;
pub mod ws;
