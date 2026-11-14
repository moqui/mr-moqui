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

//! The first path segments the framework reserves on every host: the host's own routes (git, the CI runner REST,
//! DAV, the agent endpoints, the sockets, the probes), the framework's screens and its sign-in, token and asset paths.
//! The one list of them: a public site's routes never take one (`screen::public_site`), git at a page's URL never
//! starts with one (the host's `git_http::page_request`), the host's route registration is checked against it (a
//! test of `moqui-host` fails for a route whose first segment is not here), and scripts read it as
//! `ec.web.reservedPaths` (docs/dev/JS-Core-API.md), so an application keeps its mounts, pages and names off them.
//!
//! A segment is matched as written, case and all (`Login`); an application that compares names without case lowers
//! both.

/// What a reserved segment is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A route of the host, answered before any screen (`moqui-host`'s `http.rs` and the modules it registers).
    Host,
    /// A screen or token route of the framework, or a file of its PWA.
    Screen,
    /// Static files a page loads, which a host whose `host-check-service` restricts it still serves.
    Static,
}

/// One reserved first path segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reserved {
    pub segment: &'static str,
    pub kind: Kind,
}

const fn host(segment: &'static str) -> Reserved {
    Reserved {
        segment,
        kind: Kind::Host,
    }
}

const fn screen(segment: &'static str) -> Reserved {
    Reserved {
        segment,
        kind: Kind::Screen,
    }
}

const fn asset(segment: &'static str) -> Reserved {
    Reserved {
        segment,
        kind: Kind::Static,
    }
}

/// Every reserved first path segment, whether or not this binary's features or conf serve it now: a name reserved
/// only while its feature is on would be taken by an application and then shadowed when the feature is turned on.
pub const RESERVED: &[Reserved] = &[
    // the host's routes
    host("health"),
    host("metrics"),
    host("notws"),
    host("termws"),
    host("callws"),
    host("git"),
    host("ci"),
    host("dav"),
    host(".well-known"),
    host("assist"),
    host("a2a"),
    host("mcp"),
    host("llm"),
    host("v1"),
    host("channels"),
    host("xmpp-websocket"),
    host("xmpp-upload"),
    // the framework's screens and token routes
    screen("apps"),
    screen("qapps"),
    screen("rest"),
    screen("rpc"),
    screen("status"),
    screen("error"),
    screen("fop"),
    screen("Login"),
    screen("SecondFactor"),
    screen("PasswordReset"),
    screen("ChangePassword"),
    screen("ReAuth"),
    screen("Device"),
    screen("ChatKeys"),
    screen("emailOAuth"),
    screen("sso"),
    screen("rum"),
    screen("PublicShell"),
    screen("s"),
    screen("u"),
    screen("cal"),
    screen("r"),
    // the PWA: its service worker, manifest and the push transitions
    screen("sw.js"),
    screen("manifest.webmanifest"),
    screen("WebPush"),
    // the static files
    asset("js"),
    asset("css"),
    asset("libs"),
    asset("images"),
    asset("static"),
    asset("assets"),
];

/// Whether `first`, the first segment of a request path, is reserved.
pub fn is_reserved(first: &str) -> bool {
    RESERVED.iter().any(|reserved| reserved.segment == first)
}

/// Whether `first` is one of the [`Kind::Static`] segments.
pub fn is_static(first: &str) -> bool {
    RESERVED
        .iter()
        .any(|reserved| reserved.kind == Kind::Static && reserved.segment == first)
}

/// The reserved segments, in the list's order: what `ec.web.reservedPaths` gives.
pub fn reserved_paths() -> Vec<&'static str> {
    RESERVED.iter().map(|reserved| reserved.segment).collect()
}

/// The host routes a host whose root screen refuses the framework's routes (`framework-routes="refuse"`, a site of
/// its own) still serves, each behind its own authentication and web protection: git over HTTP and Git LFS under
/// `/git/`, and the CI runner REST under `/ci/runner/` (plans/Edge-Node2.md §4.2). Everything else of the host's is
/// Not Found there.
pub const SERVED_ON_REFUSING_HOSTS: [&str; 2] = ["/git/", "/ci/runner/"];

/// Whether a request at `path` is served by the host on a host that refuses the framework's routes
/// ([`SERVED_ON_REFUSING_HOSTS`]).
pub fn served_on_refusing_host(path: &str) -> bool {
    SERVED_ON_REFUSING_HOSTS.iter().any(|prefix| path.starts_with(prefix))
}

#[cfg(test)]
#[path = "reserved_tests.rs"]
mod tests;
