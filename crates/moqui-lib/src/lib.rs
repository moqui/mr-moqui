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

//! Standalone utilities. No `ExecutionContext`.
//!
//! Feature `http` links blocking `reqwest`. It is off unless a dependent enables it.

pub mod barcode;
pub mod cache;
pub mod codec;
pub mod collection;
pub mod context_stack;
pub mod cron;
pub mod datefmt;
pub mod env;
pub mod error;
pub mod etl;
pub mod format;
pub mod histogram;
#[cfg(feature = "http")]
pub mod http;
pub mod json;
pub mod markdown;
pub mod net;
pub mod node;
pub mod object;
pub mod reconnect;
pub mod remote_login;
pub mod resource;
pub mod safe_html;
pub mod signer;
pub mod string;
pub mod source_guard;
pub mod sync;
pub mod xml_edit;

/// Marker so a downstream crate links this one.
pub fn candidate() -> &'static str {
    "moqui-lib"
}
