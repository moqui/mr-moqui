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
//!
//! Two of the host's first segments are used only under some of their sub-paths ([`PARTLY_RESERVED`]): `.well-known`
//! (a site serves its own `security.txt`, `assetlinks.json` and the like there) and `ci` (only `/ci/runner/` is a
//! route). A request path, a site page's route, is the framework's when [`is_reserved_path`] says so: its first segment
//! is wholly reserved, or it is under one of those sub-paths. A name that owns everything under it (a repository group,
//! a mount, a handle) must stay off the whole segment, since `/ci/runner` would shadow its `ci/runner`: those check
//! [`is_reserved_name`] and read [`reserved_paths`] (`ec.web.reservedPaths`), which lists both kinds whole.

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
    host("dav"),
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

/// A first segment the host uses only under some of its sub-paths; the rest of it is free for a site's pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PartlyReserved {
    pub segment: &'static str,
    /// The host's routes under it, each the second segment of their paths.
    pub sub_paths: &'static [&'static str],
}

/// The first segments the host uses only in part, with the sub-paths it uses.
pub const PARTLY_RESERVED: &[PartlyReserved] = &[
    PartlyReserved {
        segment: ".well-known",
        sub_paths: &["caldav", "carddav", "agent-card.json", "mcp.json", "ai-catalog.json"],
    },
    PartlyReserved {
        segment: "ci",
        sub_paths: &["runner"],
    },
];

/// Whether `first`, the first segment of a request path, is wholly reserved: every path under it is the framework's.
/// Not a [`PARTLY_RESERVED`] one; for a path use [`is_reserved_path`], for a name [`is_reserved_name`].
pub fn is_reserved(first: &str) -> bool {
    RESERVED.iter().any(|reserved| reserved.segment == first)
}

/// Whether `first` is a name no repository group, mount or handle may take: wholly or partly reserved, since a name
/// owns everything under it.
pub fn is_reserved_name(first: &str) -> bool {
    is_reserved(first) || PARTLY_RESERVED.iter().any(|partly| partly.segment == first)
}

/// Whether the request path `path` is the framework's: its first segment is wholly reserved, or it is at or under a
/// sub-path of a [`PARTLY_RESERVED`] one (`/ci/runner/...`, `/.well-known/caldav`). The path as written: a caller that
/// serves a path resolved some other way (`..`) asks of that path too.
pub fn is_reserved_path(path: &str) -> bool {
    let mut segments = path.trim_start_matches('/').split('/');
    let first = segments.next().unwrap_or("");
    if is_reserved(first) {
        return true;
    }
    let second = segments.next().unwrap_or("");
    PARTLY_RESERVED
        .iter()
        .any(|partly| partly.segment == first && partly.sub_paths.contains(&second))
}

/// The reserved sub-paths of the [`PARTLY_RESERVED`] segments, as `segment/sub-path` (`ci/runner`): what
/// `ec.web.reservedSubPaths` gives, for a site page's route.
pub fn reserved_sub_paths() -> Vec<String> {
    PARTLY_RESERVED
        .iter()
        .flat_map(|partly| partly.sub_paths.iter().map(|sub| format!("{}/{sub}", partly.segment)))
        .collect()
}

/// Whether `first` is one of the [`Kind::Static`] segments.
pub fn is_static(first: &str) -> bool {
    RESERVED
        .iter()
        .any(|reserved| reserved.kind == Kind::Static && reserved.segment == first)
}

/// The reserved first segments, whole and partly reserved alike, in the lists' order: what `ec.web.reservedPaths`
/// gives, the names no repository group, mount or handle may take ([`is_reserved_name`]).
pub fn reserved_paths() -> Vec<&'static str> {
    RESERVED
        .iter()
        .map(|reserved| reserved.segment)
        .chain(PARTLY_RESERVED.iter().map(|partly| partly.segment))
        .collect()
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
