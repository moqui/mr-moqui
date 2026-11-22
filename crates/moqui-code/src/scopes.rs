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

//! The method scopes of `/agentws` (plans/agent-code/14-Security.md section 2, a port of T3's
//! `clientRpcPermissions.ts`). Every method names one scope and one permission; a request passes only when the caller
//! holds both. A browser session and a full-account key hold every scope their permissions allow; a scoped key holds
//! only its own (pairing, phase P6).

use crate::{ADMIN_PERMISSION, HOST_PERMISSION, USE_PERMISSION};

/// A scope a socket's caller can hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
    Read,
    Operate,
    Settings,
    Providers,
    Maintain,
    Preview,
    Diagnostics,
    TerminalRead,
    Terminal,
    FilesRead,
    FilesWrite,
    Review,
    ScmWrite,
    AccessRead,
    AccessWrite,
    EdgeRead,
    EdgeWrite,
    McpSession,
}

impl Scope {
    /// Every scope, in the order of the table in 14 section 2.
    pub const ALL: [Self; 18] = [
        Self::Read,
        Self::Operate,
        Self::Settings,
        Self::Providers,
        Self::Maintain,
        Self::Preview,
        Self::Diagnostics,
        Self::TerminalRead,
        Self::Terminal,
        Self::FilesRead,
        Self::FilesWrite,
        Self::Review,
        Self::ScmWrite,
        Self::AccessRead,
        Self::AccessWrite,
        Self::EdgeRead,
        Self::EdgeWrite,
        Self::McpSession,
    ];

    /// The scope's name, as a key and an error carry it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Read => "code:read",
            Self::Operate => "code:operate",
            Self::Settings => "code:settings",
            Self::Providers => "code:providers",
            Self::Maintain => "code:maintain",
            Self::Preview => "code:preview",
            Self::Diagnostics => "code:diagnostics",
            Self::TerminalRead => "code:terminal-read",
            Self::Terminal => "code:terminal",
            Self::FilesRead => "code:files-read",
            Self::FilesWrite => "code:files-write",
            Self::Review => "code:review",
            Self::ScmWrite => "code:scm-write",
            Self::AccessRead => "code:access-read",
            Self::AccessWrite => "code:access-write",
            Self::EdgeRead => "code:edge-read",
            Self::EdgeWrite => "code:edge-write",
            Self::McpSession => "code:mcp-session",
        }
    }

    /// The scope of a name; `None` for a name that is not one (exchange drops it).
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|scope| scope.name() == name)
    }

    /// The permission a holder of the scope needs at least.
    #[must_use]
    pub const fn permission(self) -> &'static str {
        match self {
            Self::Read
            | Self::Operate
            | Self::Settings
            | Self::Preview
            | Self::FilesRead
            | Self::FilesWrite
            | Self::Review
            | Self::ScmWrite
            | Self::AccessRead
            | Self::AccessWrite
            | Self::Providers
            | Self::McpSession => USE_PERMISSION,
            Self::TerminalRead | Self::Terminal => HOST_PERMISSION,
            Self::Maintain | Self::Diagnostics | Self::EdgeRead | Self::EdgeWrite => ADMIN_PERMISSION,
        }
    }

    /// A key can carry the scope (`code:review` and `code:mcp-session` it cannot: a browser session and a run's own
    /// token only).
    #[must_use]
    pub const fn grantable(self) -> bool {
        !matches!(self, Self::Review | Self::McpSession)
    }

    /// The scopes of a key's `scope` text (space or comma list); unknown names are dropped.
    #[must_use]
    pub fn parse_list(text: &str) -> Vec<Self> {
        let mut scopes: Vec<Self> = text
            .split([' ', ','])
            .filter_map(Self::parse)
            .filter(|scope| scope.grantable())
            .collect();
        scopes.sort();
        scopes.dedup();
        scopes
    }
}

/// What a method needs, beyond its scope's permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodSpec {
    pub name: &'static str,
    pub scope: Scope,
    /// The permission the method needs; it is never less than the scope's.
    pub permission: &'static str,
    /// The method opens a stream (`"s":true`).
    pub stream: bool,
}

const fn method(name: &'static str, scope: Scope) -> MethodSpec {
    MethodSpec {
        name,
        scope,
        permission: scope.permission(),
        stream: false,
    }
}

const fn stream(name: &'static str, scope: Scope) -> MethodSpec {
    MethodSpec {
        name,
        scope,
        permission: scope.permission(),
        stream: true,
    }
}

const fn admin(name: &'static str, scope: Scope) -> MethodSpec {
    MethodSpec {
        name,
        scope,
        permission: ADMIN_PERMISSION,
        stream: false,
    }
}

/// Every `/agentws` method this server answers. A method not here is refused with `unknown_method`.
pub const METHODS: &[MethodSpec] = &[
    method("server.getConfig", Scope::Read),
    method("server.getDescriptor", Scope::Read),
    method("orchestration.dispatchCommand", Scope::Operate),
    method("orchestration.getShellSnapshot", Scope::Read),
    stream("orchestration.subscribeShell", Scope::Read),
    method("settings.getClient", Scope::Read),
    method("settings.updateClient", Scope::Settings),
    method("settings.getServer", Scope::Read),
    admin("settings.updateServer", Scope::Settings),
    method("settings.getSchema", Scope::Read),
];

/// The spec of a method.
#[must_use]
pub fn method_spec(name: &str) -> Option<&'static MethodSpec> {
    METHODS.iter().find(|spec| spec.name == name)
}

/// What a caller holds: its permissions decide, and a scoped key narrows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    /// `None` = every scope the permissions allow (a browser session or a full-account key).
    pub key_scopes: Option<Vec<Scope>>,
    pub is_browser_session: bool,
}

/// Why a request is refused, with what it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub required_scope: Scope,
    pub required_permission: &'static str,
}

impl Grant {
    /// Check `spec` for this grant, with `has_permission` naming the caller's permissions.
    ///
    /// # Errors
    ///
    /// The scope and permission the method needs, when the caller lacks one.
    pub fn check(&self, spec: &MethodSpec, has_permission: impl Fn(&str) -> bool) -> Result<(), Refusal> {
        let refusal = Refusal {
            required_scope: spec.scope,
            required_permission: spec.permission,
        };
        let scope_ok = match &self.key_scopes {
            None => spec.scope != Scope::Review || self.is_browser_session,
            Some(scopes) => scopes.contains(&spec.scope),
        };
        if !scope_ok || !has_permission(spec.scope.permission()) || !has_permission(spec.permission) {
            return Err(refusal);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "scopes_tests.rs"]
mod tests;
