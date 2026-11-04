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
//! - [`account`]: the servers and users signed in on this device, the current one, and each one's device mode.
//! - [`keys`]: where the device keys are kept, behind a trait the host fills with the OS key store.
//! - [`login`]: password sign-in with `createLoginKey`, the second factor, and sign-out that revokes the key.
//! - [`proxy`]: a request of the page forwarded to the server with the key, its answer made safe for the page.
//! - [`socket`]: `/notws` and `/callws` held by the host and bridged to the page.
//! - [`cache`]: the shell's answers as MD3 kept them in files, moved once into the account's [`store`].
//! - [`store`]: an account's data on the device for offline reading and writing: cached answers, chat, mail, the
//!   outbox, drafts and files, in SQLite under a size cap.
//! - [`conf`]: the client's settings from the `<client-conf>` element of the Moqui XML conf (the device mode, the
//!   mirror and prefetch limits, the store's size cap).
//! - [`sso`]: sign-in through the server's identity providers in the system browser, handed back to the app (RFC 8252).
//! - [`app`]: the client's state while the app runs as a client ([`app::Client`]), and what it asks of its host
//!   ([`app::ClientHost`]): the pages are the host's.
//! - [`bridge`]: the pages' sockets held by the host, and a client's `/notws` frames written into the store.
//! - [`stores`]: each account's store, opened when first used; [`offline`]: the comms transitions' answers kept and
//!   answered when the server cannot be reached; [`sync`]: the pushes, the outbox and its sender, and the mirror.
//! - [`modes`]: what each device mode keeps; [`renewal`]: the keys renewed before they run out; [`assets`]: the
//!   shell's own files.
//!
//! Blocking I/O on threads, as `moqui tui --server` (`crates/moqui-host/src/tui_remote.rs`), whose patterns this
//! follows: no async API. reqwest's blocking client runs a runtime of its own, so a caller must be on a plain thread,
//! not in an async context (a Tauri `async` command), where making or dropping a client panics.
//!
//! TLS is verified against the OS trust store (rustls-platform-verifier) for requests and sockets alike. On Android
//! that verifier needs its JVM init (`rustls_platform_verifier::android`) before the first request.

pub mod account;
pub mod app;
pub mod assets;
pub mod bridge;
pub mod cache;
pub mod conf;
mod files;
mod http;
pub mod keys;
pub mod login;
pub mod modes;
pub mod offline;
pub mod proxy;
pub mod renewal;
pub mod socket;
pub mod sso;
pub mod store;
pub mod stores;
pub mod sync;

/// The shell's origin on Chromium (`http://<scheme>.localhost` of the `moqui-shell` app scheme), the `Origin` the
/// host's own requests (the outbox, the mirror) go to the server with.
pub const SHELL_ORIGIN: &str = "http://moqui-shell.localhost";

pub use moqui_lib::error::Error;
