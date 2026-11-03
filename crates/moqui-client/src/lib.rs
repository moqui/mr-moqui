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

//! The client of a remote MR Moqui server, for the desktop app's client mode and later the mobile app
//! (plans/Mobile-Desktop.md §4 configuration 3, MD3). Each installed client is a device with its own login key, which
//! the host keeps and adds to every request as the `login_key` header; the page never holds it.
//!
//! - [`account`]: the servers and users signed in on this device, and the current one.
//! - [`keys`]: where the device keys are kept, behind a trait the host fills with the OS key store.
//! - [`login`]: password sign-in with `createLoginKey`, the second factor, and sign-out that revokes the key.
//! - [`proxy`]: a request of the page forwarded to the server with the key, its answer made safe for the page.
//! - [`socket`]: `/notws` and `/callws` held by the host and bridged to the page.
//! - [`cache`]: the last answers the shell opens on when the server cannot be reached.
//! - [`sso`]: sign-in through the server's identity providers in the system browser, handed back to the app (RFC 8252).
//!
//! Blocking I/O on threads, as `moqui tui --server` (`crates/moqui-host/src/tui_remote.rs`), whose patterns this
//! follows: no async API. reqwest's blocking client runs a runtime of its own, so a caller must be on a plain thread,
//! not in an async context (a Tauri `async` command), where making or dropping a client panics.
//!
//! TLS is verified against the OS trust store (rustls-platform-verifier) for requests and sockets alike. On Android
//! that verifier needs its JVM init (`rustls_platform_verifier::android`) before the first request.

pub mod account;
pub mod cache;
mod files;
mod http;
pub mod keys;
pub mod login;
pub mod proxy;
pub mod socket;
pub mod sso;

pub use moqui_lib::error::Error;
