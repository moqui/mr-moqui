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

//! Closing one web listener's connections (plans/Runtime-Config.md §3.3, §8.3, RC2-a). Each connection a listener
//! accepts carries the listener's name and its closing signal, which every request it carries has in its extensions
//! as an [`OnListener`]. A handler that holds a connection open (a WebSocket, an event stream, a long wait) ends it on
//! [`closing_of`]: its listener's signal, so a listener that stops ends its own sockets and leaves the others', or the
//! process-wide stop for a request that came over no listener (in process, down the edge link). Once a stopped
//! listener's [`Grace`] has passed, `http::serve` drops whatever of its connections is left.

use std::sync::Arc;
use std::time::Duration;

use axum::http::request::Parts;
use tokio::sync::watch;

use super::HttpState;

/// The listener a request came over: its name in the startup report (`HTTP`, `HTTPS`) and its closing signal, which
/// turns true as it stops taking connections. It stays in the request's extensions, where the request's record of
/// its listener is read from (`WebRequest`, RC4).
#[derive(Clone, Debug)]
pub(crate) struct OnListener {
    // what RC4 copies into the request's record; the tests read it until then
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) name: &'static str,
    pub(crate) closing: watch::Receiver<bool>,
}

/// The signal a handler ends a long-lived connection on: the closing of the listener the request came over, or
/// `process`, the process-wide stop, for a request that came over none.
pub(crate) fn closing_of(
    extensions: &axum::http::Extensions,
    process: &watch::Receiver<bool>,
) -> watch::Receiver<bool> {
    extensions
        .get::<OnListener>()
        .map_or_else(|| process.clone(), |on| on.closing.clone())
}

/// [`closing_of`] a handler's request, with the server's process-wide stop.
pub(crate) struct Closing(pub(crate) watch::Receiver<bool>);

impl axum::extract::FromRequestParts<Arc<HttpState>> for Closing {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &Arc<HttpState>) -> Result<Self, Self::Rejection> {
        Ok(Self(closing_of(&parts.extensions, &state.shutdown)))
    }
}

/// How long the open connections of a listener that stopped may take to finish before they are dropped: the
/// process stop's `web-protection/connections@shutdown-timeout`, a live stop's `LIVE_STOP_GRACE`, or none for "end
/// connections now".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Grace {
    /// Up to this long; zero drops them at once.
    Upto(Duration),
    /// As long as they take: a `shutdown-timeout` of 0.
    Unbounded,
}

impl Grace {
    /// The grace of `limit`, the `shutdown-timeout` of [`super::ListenLimits::shutdown`].
    pub(crate) fn of(limit: Option<Duration>) -> Self {
        limit.map_or(Self::Unbounded, Self::Upto)
    }
}

/// A web listener's stop as its loop waits for it: when it comes and the grace it gives, and a later "end connections
/// now" that cuts a grace already running short (plans/Runtime-Config.md §3.1: an operator who turned a listener off
/// with its grace may still end a download that does not finish).
pub(crate) struct StopSignal {
    pub(crate) grace: std::pin::Pin<Box<dyn std::future::Future<Output = Grace> + Send>>,
    pub(crate) now: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
}

impl StopSignal {
    pub(crate) fn new(
        grace: impl std::future::Future<Output = Grace> + Send + 'static,
        now: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> Self {
        Self {
            grace: Box::pin(grace),
            now: Box::pin(now),
        }
    }

    /// A stop with no "end connections now" after it.
    #[cfg(test)]
    pub(crate) fn once(grace: impl std::future::Future<Output = Grace> + Send + 'static) -> Self {
        Self::new(grace, std::future::pending())
    }
}

#[cfg(test)]
#[path = "closing_tests.rs"]
mod tests;
