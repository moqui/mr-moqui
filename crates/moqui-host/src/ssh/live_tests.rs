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

use std::collections::HashMap;

use russh::MethodKind;

use super::{offered, takes, Access, Method, SshLimits, SshLive};

/// The settings of a conf that sets `given`, each by selector.
fn live_of(given: &[(&str, &str)]) -> SshLive {
    let conf: HashMap<String, String> = given
        .iter()
        .map(|(selector, value)| ((*selector).to_string(), (*value).to_string()))
        .collect();
    SshLive::from_conf(&|selector: &str| conf.get(selector).cloned())
}

/// A conf that leaves every setting out, or empty, has today's defaults: anyone to the sign-in dialog, the limits of
/// the default conf, every sub-service on, git by key with `SSH_GIT`, SFTP to the drives.
#[test]
fn the_defaults_are_the_default_confs() {
    let defaults = live_of(&[]);
    assert_eq!(defaults, SshLive::default());
    assert_eq!(defaults.access, Access::Open);
    assert_eq!(
        defaults.limits,
        SshLimits {
            max_sessions: 8,
            max_per_user: 4,
            max_per_source: 4,
            max_dialogs: 4,
            max_dialogs_per_source: 2,
            max_file: 268_435_456,
        }
    );
    assert!(defaults.tui && defaults.sftp && defaults.git && defaults.shell && defaults.sftp_drives);
    assert!(!defaults.git_password_auth && !defaults.git_any_account);
    assert!(defaults.problems.is_empty());
    let empty = live_of(&[("ssh-server@access", " "), ("ssh-server@max-sessions", "")]);
    assert_eq!(empty, defaults);
    assert_eq!(defaults.summary(), "access open");
}

/// Each access mode, trimmed and in any case; the settings beside it read as written.
#[test]
fn each_access_mode_and_the_settings_read_as_written() {
    for (given, access) in [("open", Access::Open), (" Login ", Access::Login), ("KEY", Access::Key)] {
        assert_eq!(live_of(&[("ssh-server@access", given)]).access, access, "{given}");
        assert_eq!(Access::parse(access.word()), Some(access));
    }
    let live = live_of(&[
        ("ssh-server@access", "login"),
        ("ssh-server@max-sessions", "20"),
        ("ssh-server@max-sessions-per-user", "0"),
        ("ssh-server@max-dialog-sessions", "0"),
        ("ssh-server@sftp-max-file", "1024"),
        ("ssh-server@git-password-auth", "TRUE"),
        ("ssh-server@sftp-enabled", "false"),
        ("ssh-server@shell-enabled", " false "),
        ("dav-server@sftp-drives", "false"),
    ]);
    assert_eq!(live.limits.max_sessions, 20);
    assert_eq!(live.limits.max_per_user, 0, "0 is no share");
    assert_eq!(live.limits.max_dialogs, 0);
    assert_eq!(live.limits.max_file, 1024);
    assert!(live.git_password_auth && !live.sftp && !live.shell && !live.sftp_drives);
    assert!(live.tui && live.git);
    assert!(live.problems.is_empty(), "{:?}", live.problems);
    assert_eq!(live.summary(), "access login, SFTP, the shell off");
}

/// A setting that does not read fails closed and is named: an access mode that is none is `key`, a flag that is
/// neither true nor false is off, a limit that is no number its default; 0 where a limit must be one is its default.
#[test]
fn a_bad_value_fails_closed_and_is_named() {
    let live = live_of(&[
        ("ssh-server@access", "everyone"),
        ("ssh-server@tui-enabled", "yes"),
        ("dav-server@sftp-drives", "1"),
        ("ssh-server@max-sessions", "many"),
        ("ssh-server@sftp-max-file", "0"),
        ("ssh-server@max-sessions-per-source", "-1"),
    ]);
    assert_eq!(live.access, Access::Key);
    assert!(!live.tui && !live.sftp_drives);
    assert_eq!(live.limits.max_sessions, 8);
    assert_eq!(live.limits.max_file, 256 * 1024 * 1024);
    assert_eq!(live.limits.max_per_source, 4);
    let problems = live.problems.join("\n");
    for named in [
        "ssh-server@access \"everyone\"",
        "ssh-server@tui-enabled \"yes\"",
        "dav-server@sftp-drives \"1\"",
        "ssh-server@max-sessions \"many\"",
        "ssh-server@max-sessions-per-source \"-1\"",
    ] {
        assert!(problems.contains(named), "{named}: {problems}");
    }
    assert!(!problems.contains("sftp-max-file"), "{problems}");
    assert_eq!(live.problems.len(), 5);
    assert!(live
        .summary()
        .starts_with("access key, the terminal client off; ssh-server@access"));
}

/// The methods each access mode offers: the sign-in dialog only under `open` and while it is served.
#[test]
fn the_methods_each_access_mode_offers() {
    let all = [
        MethodKind::PublicKey,
        MethodKind::KeyboardInteractive,
        MethodKind::Password,
    ];
    assert_eq!(offered(Access::Open, true), all);
    assert_eq!(
        offered(Access::Open, false),
        [MethodKind::PublicKey, MethodKind::Password]
    );
    for dialog in [true, false] {
        assert_eq!(
            offered(Access::Login, dialog),
            [MethodKind::PublicKey, MethodKind::Password]
        );
        assert_eq!(offered(Access::Key, dialog), [MethodKind::PublicKey]);
    }
    // a connection takes the methods of the mode it sees, open with no dialog served as login
    let no_dialog = live_of(&[("ssh-server@tui-enabled", "false")]);
    assert_eq!(no_dialog.offered_as(), Access::Login);
    assert_eq!(live_of(&[("ssh-server@access", "key")]).offered_as(), Access::Key);
    let open = SshLive::default();
    assert_eq!(
        open.offered_as(),
        if cfg!(feature = "tui") {
            Access::Open
        } else {
            Access::Login
        }
    );
}

/// The logins each access mode takes, whatever the client was offered: a key always (the cluster's too), a password
/// but under `key`, the sign-in dialog under `open` alone and only while the terminal client is on.
#[test]
fn the_logins_each_access_mode_takes() {
    for access in [Access::Open, Access::Login, Access::Key] {
        assert!(takes(access, Method::PublicKey), "{access:?}");
    }
    assert!(takes(Access::Open, Method::Password) && takes(Access::Login, Method::Password));
    assert!(
        !takes(Access::Key, Method::Password),
        "an app password no more than the account's"
    );
    assert!(takes(Access::Open, Method::Dialog));
    assert!(!takes(Access::Login, Method::Dialog) && !takes(Access::Key, Method::Dialog));
    let open = SshLive::default();
    assert_eq!(open.takes(Method::Dialog), cfg!(feature = "tui"));
    let no_tui = live_of(&[("ssh-server@tui-enabled", "false")]);
    assert!(!no_tui.takes(Method::Dialog) && no_tui.takes(Method::Password));
    let key = live_of(&[("ssh-server@access", "key")]);
    assert!(key.takes(Method::PublicKey) && !key.takes(Method::Password) && !key.takes(Method::Dialog));
}
