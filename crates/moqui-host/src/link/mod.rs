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

//! What the links a node dials out on share (plans/archive/Vault-Instances.md §4.4): the edge link
//! (plans/archive/Edge-Node.md §5.1, `crate::edge_link`) and the manager link (`crate::manage_link`). Each dials an
//! HTTPS port with an ALPN of its own, pins the other end's key by SHA-256 of its public key (`crate::tls_pin`), sends a
//! one-byte role and reads an [`Answer`], then speaks gRPC on HTTP/2 over the connection ([`H2Channel`]); a lost link is
//! dialed again with a [`Backoff`].
//!
//! - [`answer`]: the answer to the role byte, a code and a short message.
//! - [`backoff`]: the waits between dials, with jitter.
//! - [`hello`]: the ClientHello read before the HTTPS listener picks a link's TLS settings or its own.
//! - [`identity`]: a link key, the self-signed certificate made from it, and its fingerprint.

// writing an answer and reading a ClientHello are the accepting end's (an edge node, a manager), and new_key the edge
// link's: a build that only dials a manager (Lite) uses neither
#![cfg_attr(not(any(feature = "edge", feature = "instances")), allow(dead_code, unused_imports))]

pub(crate) mod answer;
pub(crate) mod backoff;
pub(crate) mod hello;
pub(crate) mod identity;

pub(crate) use crate::grpc_client::H2Channel;
pub(crate) use answer::{read_answer, write_answer, Answer};
pub(crate) use backoff::jitter;
pub use backoff::Backoff;
pub(crate) use hello::read_hello;
pub use identity::new_key;
pub(crate) use identity::Identity;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
