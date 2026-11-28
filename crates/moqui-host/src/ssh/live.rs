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

//! The SSH server's settings that apply while it runs (plans/Runtime-Config.md §3.4, §8.4): who may connect
//! (`ssh-server@access`), the session limits, git's two settings, SFTP to the drives and the four sub-service flags.
//! The listener registry reads them from the running conf at each pass and the server keeps them where each new
//! connection, login and channel reads them ([`SshLive`]); a change needs no new bind. What each access mode offers and
//! takes is here too ([`offered`], [`takes`]), so the server's configs and its auth handlers agree.

use russh::MethodKind;

/// `ssh-server@access`: who may connect (plans/Runtime-Config.md §3.4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Access {
    /// Anyone: a client with no key or password is let in for the terminal client's sign-in dialog, within
    /// `max-dialog-sessions`; an account's password or key logs in at once.
    #[default]
    Open,
    /// An account's password or one of its registered keys.
    Login,
    /// A registered key (`UserSshKey`) only, the cluster's own included.
    Key,
}

impl Access {
    /// The access mode `text` names, trimmed and in any case; none for another word.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "open" => Some(Self::Open),
            "login" => Some(Self::Login),
            "key" => Some(Self::Key),
            _ => None,
        }
    }

    /// `open`, `login` or `key`, as the conf writes it.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Login => "login",
            Self::Key => "key",
        }
    }
}

/// How a client asks to log in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// A key: an account's `UserSshKey`, or the cluster's key.
    PublicKey,
    /// An account's password or app password.
    Password,
    /// Keyboard-interactive with no questions: the terminal client's sign-in dialog.
    Dialog,
}

/// Whether a login by `method` is taken under `access`: a key in every mode, the cluster's included; a password under
/// `open` and `login`; the sign-in dialog under `open` alone. The auth handlers ask it whatever the client was offered,
/// as russh hands them a method it did not offer.
#[must_use]
pub fn takes(access: Access, method: Method) -> bool {
    match method {
        Method::PublicKey => true,
        Method::Password => access != Access::Key,
        Method::Dialog => access == Access::Open,
    }
}

/// The methods a connection accepted under `access` is offered, keyboard-interactive (the sign-in dialog) only while
/// `dialog` (a build with the terminal client, and `tui-enabled`). "none" is never offered, so a client tries its keys
/// first; OpenSSH tries keyboard-interactive after them, and a password last.
#[must_use]
pub fn offered(access: Access, dialog: bool) -> &'static [MethodKind] {
    match access {
        Access::Open if dialog => &[
            MethodKind::PublicKey,
            MethodKind::KeyboardInteractive,
            MethodKind::Password,
        ],
        Access::Open | Access::Login => &[MethodKind::PublicKey, MethodKind::Password],
        Access::Key => &[MethodKind::PublicKey],
    }
}

/// The SSH session limits (`ssh-server` `max-sessions`, `max-sessions-per-user`, `max-sessions-per-source`,
/// `max-dialog-sessions`, `max-dialog-sessions-per-source`) and the largest file SFTP takes (`sftp-max-file`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SshLimits {
    /// The most sessions at once; never 0.
    pub max_sessions: usize,
    /// Of them, one user logged in by key or password; 0 is no share.
    pub max_per_user: usize,
    /// Of them, one address (an IPv6 /64); 0 is no share.
    pub max_per_source: usize,
    /// Of them, those let in with no user for the sign-in dialog; 0 is no share but `max_sessions`.
    pub max_dialogs: usize,
    /// Of those, one address; 0 is no share.
    pub max_dialogs_per_source: usize,
    /// The largest file SFTP takes, in bytes; never 0.
    pub max_file: usize,
}

impl Default for SshLimits {
    fn default() -> Self {
        Self {
            max_sessions: 8,
            max_per_user: 4,
            max_per_source: 4,
            max_dialogs: 4,
            max_dialogs_per_source: 2,
            max_file: 256 * 1024 * 1024,
        }
    }
}

/// The SSH server's settings applied in place (plans/Runtime-Config.md §8.4): each new connection takes the access
/// mode it sees at accept, each login and channel reads the rest when it asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshLive {
    pub access: Access,
    pub limits: SshLimits,
    /// `git-password-auth`: git over SSH on a connection that logged in by password.
    pub git_password_auth: bool,
    /// `git-any-account`: git over SSH for any account that logged in by key, without `SSH_GIT`.
    pub git_any_account: bool,
    /// `dav-server@sftp-drives`: SFTP to the drives for a user without `SSH_SFTP`.
    pub sftp_drives: bool,
    /// `tui-enabled`: the terminal client (`apps`, `agent-apps` and the sign-in dialog).
    pub tui: bool,
    /// `sftp-enabled`: the `sftp` subsystem.
    pub sftp: bool,
    /// `git-enabled`: git over SSH.
    pub git: bool,
    /// `shell-enabled`: the admin shell, and the commands and host sessions at its prompt.
    pub shell: bool,
    /// The settings that did not read, each with what was taken in its place, for the listener's row and the log.
    pub problems: Vec<String>,
}

impl Default for SshLive {
    fn default() -> Self {
        Self {
            access: Access::Open,
            limits: SshLimits::default(),
            git_password_auth: false,
            git_any_account: false,
            sftp_drives: true,
            tui: true,
            sftp: true,
            git: true,
            shell: true,
            problems: Vec::new(),
        }
    }
}

impl SshLive {
    /// The settings from the conf, by selector (`ssh-server@access`, `dav-server@sftp-drives`); a setting the conf
    /// leaves out or empty is its default. One that does not read fails closed and is named in [`SshLive::problems`]:
    /// an access mode that is no mode is `key`, a flag that is neither true nor false is off, and a limit that is no
    /// number is its default (a limit of 0 where none is meant, as before).
    #[must_use]
    pub fn from_conf(setting: &dyn Fn(&str) -> Option<String>) -> Self {
        let mut problems = Vec::new();
        let text = |selector: &str| {
            setting(selector)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        let access = match text("ssh-server@access") {
            None => Access::Open,
            Some(given) => Access::parse(&given).unwrap_or_else(|| {
                problems.push(format!(
                    "ssh-server@access \"{given}\" is not open, login or key, so a key alone logs in"
                ));
                Access::Key
            }),
        };
        let mut flag = |selector: &str, default: bool| match text(selector) {
            None => default,
            Some(given) if given.eq_ignore_ascii_case("true") => true,
            Some(given) if given.eq_ignore_ascii_case("false") => false,
            Some(given) => {
                problems.push(format!("{selector} \"{given}\" is not true or false, so it is off"));
                false
            }
        };
        let git_password_auth = flag("ssh-server@git-password-auth", false);
        let git_any_account = flag("ssh-server@git-any-account", false);
        let sftp_drives = flag("dav-server@sftp-drives", true);
        let tui = flag("ssh-server@tui-enabled", true);
        let sftp = flag("ssh-server@sftp-enabled", true);
        let git = flag("ssh-server@git-enabled", true);
        let shell = flag("ssh-server@shell-enabled", true);
        let defaults = SshLimits::default();
        let mut number = |name: &str, default: usize, positive: bool| {
            let selector = format!("ssh-server@{name}");
            let Some(given) = text(&selector) else {
                return default;
            };
            let parsed: Result<usize, _> = given.parse();
            match parsed {
                Ok(0) if positive => default,
                Ok(number) => number,
                Err(_) => {
                    problems.push(format!("{selector} \"{given}\" is not a number, so it is {default}"));
                    default
                }
            }
        };
        let limits = SshLimits {
            max_sessions: number("max-sessions", defaults.max_sessions, true),
            max_per_user: number("max-sessions-per-user", defaults.max_per_user, false),
            max_per_source: number("max-sessions-per-source", defaults.max_per_source, false),
            max_dialogs: number("max-dialog-sessions", defaults.max_dialogs, false),
            max_dialogs_per_source: number("max-dialog-sessions-per-source", defaults.max_dialogs_per_source, false),
            max_file: number("sftp-max-file", defaults.max_file, true),
        };
        Self {
            access,
            limits,
            git_password_auth,
            git_any_account,
            sftp_drives,
            tui,
            sftp,
            git,
            shell,
            problems,
        }
    }

    /// Whether the sign-in dialog is served: a build with the terminal client, and `tui-enabled`.
    #[must_use]
    pub fn dialog(&self) -> bool {
        cfg!(feature = "tui") && self.tui
    }

    /// Whether a login by `method` is taken now ([`takes`]), the sign-in dialog only while it is served.
    #[must_use]
    pub fn takes(&self, method: Method) -> bool {
        takes(self.access, method) && (method != Method::Dialog || self.dialog())
    }

    /// The access mode whose methods a connection accepted now is offered: `open` offers no sign-in dialog while it is
    /// not served, which is what `login` offers.
    #[must_use]
    pub fn offered_as(&self) -> Access {
        match self.access {
            Access::Open if !self.dialog() => Access::Login,
            access => access,
        }
    }

    /// What the listener's row adds to its address: the access mode, the sub-services turned off, and what did not
    /// read.
    #[must_use]
    pub fn summary(&self) -> String {
        let off: Vec<&str> = [
            (self.tui, "the terminal client"),
            (self.sftp, "SFTP"),
            (self.git, "git"),
            (self.shell, "the shell"),
        ]
        .into_iter()
        .filter(|(on, _)| !on)
        .map(|(_, name)| name)
        .collect();
        let mut text = format!("access {}", self.access.word());
        if !off.is_empty() {
            text.push_str(&format!(", {} off", off.join(", ")));
        }
        for problem in &self.problems {
            text.push_str("; ");
            text.push_str(problem);
        }
        text
    }
}

#[cfg(test)]
#[path = "live_tests.rs"]
mod tests;
