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

use std::sync::atomic::Ordering;
use std::sync::Arc;

use std::time::Duration;

use super::{
    conf_listen_target, dialog_closed, dialog_expired_within, for_log, git_allowed, listen_target, Counts, GitAccess,
    GitLogin, SessionRefused, Slot, SlotRefused, Who, MAX_LOGGED_NAME, MAX_PREAUTH, PREAUTH_PER_SOURCE,
};
use crate::ssh::live::SshLimits;

/// SEC-FIL-92: a client's user name goes to the log quoted, its control characters escaped, and cut short.
#[test]
fn a_clients_user_name_is_logged_escaped_and_cut() {
    assert_eq!(for_log("alice", MAX_LOGGED_NAME), "\"alice\"");
    let logged = for_log("x\u{1b}[2J\r\nFAKE line", MAX_LOGGED_NAME);
    assert!(
        !logged.contains('\u{1b}') && !logged.contains('\n') && !logged.contains('\r'),
        "{logged}"
    );
    assert!(logged.contains("\\u{1b}") && logged.contains("\\r\\n"), "{logged}");
    assert_eq!(for_log(&"a".repeat(500), MAX_LOGGED_NAME).len(), MAX_LOGGED_NAME + 2);
}

/// The switch decides (plans/Runtime-Config.md §3.2): off keeps the server off whatever the port; on, the port must be
/// one, an empty one refused naming the switch.
#[test]
fn enabled_false_keeps_the_server_off_with_a_port_set() {
    assert_eq!(conf_listen_target(false, "2222", "").unwrap(), None);
    assert_eq!(conf_listen_target(false, "", "").unwrap(), None);
    assert_eq!(
        conf_listen_target(true, "2222", "").unwrap(),
        Some(("127.0.0.1".to_string(), 2222))
    );
    let empty = conf_listen_target(true, "", "").unwrap_err().to_string();
    assert!(empty.contains("ssh-server@enabled"), "{empty}");
}

#[test]
fn the_port_check_reads_the_conf_as_the_server_does() {
    let conf =
        |ssh: &str| moqui_lib::node::MNode::parse_text("conf", &format!("<moqui-conf>{ssh}</moqui-conf>")).unwrap();
    assert_eq!(
        listen_target(&conf(r#"<ssh-server enabled=" True " port="2223" host="127.0.0.2"/>"#)).unwrap(),
        Some(("127.0.0.2".to_string(), 2223))
    );
    assert_eq!(
        listen_target(&conf(r#"<ssh-server enabled="false" port="2223"/>"#)).unwrap(),
        None
    );
    // a switch not written is off: the default conf writes it
    assert_eq!(listen_target(&conf(r#"<ssh-server port="2223"/>"#)).unwrap(), None);
}

#[test]
fn a_login_trades_the_preauth_slot_for_a_session() {
    let counts = Arc::new(Counts::new(1, 0, 0));
    let source = |n: usize| std::net::IpAddr::from([198, 51, 100, u8::try_from(n / PREAUTH_PER_SOURCE).unwrap()]);
    let waiting: Vec<Slot> = (0..MAX_PREAUTH)
        .map(|n| Slot::preauth(&counts, source(n)).unwrap())
        .collect();
    assert_eq!(
        Slot::preauth(&counts, source(MAX_PREAUTH + PREAUTH_PER_SOURCE)).err(),
        Some(SlotRefused::Full),
        "past MAX_PREAUTH"
    );
    assert!(waiting[0].log_in(Who::Dialog).is_ok());
    // The login gave its pre-auth slot back, and took the one session.
    let next = Slot::preauth(&counts, source(0)).unwrap();
    assert_eq!(
        next.log_in(Who::Dialog),
        Err(SessionRefused::Total),
        "past ssh-server max-sessions"
    );
    drop(waiting);
    assert!(next.log_in(Who::Dialog).is_ok());
    drop(next);
    assert_eq!(counts.preauth.held(), 0);
    assert_eq!(counts.sessions.load(Ordering::Acquire), 0);
}

/// One source holds at most its share of the pre-auth slots; another source still gets one (HL-032).
#[test]
fn one_source_holds_only_its_share_of_the_preauth_slots() {
    let counts = Arc::new(Counts::new(8, 0, 0));
    let busy: std::net::IpAddr = "2001:db8:9:9::1".parse().unwrap();
    let held: Vec<Slot> = (0..PREAUTH_PER_SOURCE)
        .map(|_| Slot::preauth(&counts, busy).unwrap())
        .collect();
    let same_64: std::net::IpAddr = "2001:db8:9:9::2".parse().unwrap();
    assert_eq!(Slot::preauth(&counts, same_64).err(), Some(SlotRefused::Source));
    let other: std::net::IpAddr = "203.0.113.20".parse().unwrap();
    assert!(Slot::preauth(&counts, other).unwrap().log_in(Who::Dialog).is_ok());
    drop(held);
}

/// One user, and one source, holds at most its share of the sessions; ending one gives it back.
#[test]
fn a_user_and_a_source_hold_only_their_share_of_the_sessions() {
    let counts = Arc::new(Counts::new(8, 2, 3));
    let at = |last: u8| std::net::IpAddr::from([198, 51, 100, last]);
    let slot = |last: u8| Slot::preauth(&counts, at(last)).unwrap();
    // two sessions of ann, from two addresses; a third is refused wherever it comes from
    let (first, second) = (slot(1), slot(2));
    assert!(first.log_in(Who::User("ann")).is_ok() && second.log_in(Who::User("ann")).is_ok());
    assert!(first.log_in(Who::User("ann")).is_ok(), "a session logged in stays one");
    let third = slot(3);
    assert_eq!(third.log_in(Who::User("ann")), Err(SessionRefused::User));
    assert_eq!(SessionRefused::User.limit(), "max-sessions-per-user");
    // another user is not held back by her, and the refused login took nothing
    assert!(third.log_in(Who::User("bob")).is_ok());
    assert_eq!(counts.sessions.load(Ordering::Acquire), 3);
    drop(second);
    let again = slot(2);
    assert!(again.log_in(Who::User("ann")).is_ok(), "her share came back");

    // three sessions from one address, of users or of the login dialog; a cluster peer is not counted by it
    let busy: Vec<Slot> = (0..2).map(|_| slot(9)).collect();
    assert!(busy[0].log_in(Who::User("cy")).is_ok() && busy[1].log_in(Who::Dialog).is_ok());
    let one_more = slot(9);
    assert!(one_more.log_in(Who::User("dee")).is_ok());
    let past = slot(9);
    assert_eq!(past.log_in(Who::User("eve")), Err(SessionRefused::Source));
    assert_eq!(past.log_in(Who::Dialog), Err(SessionRefused::Source));
    assert!(past.log_in(Who::Cluster).is_ok());
    assert_eq!(counts.sessions.load(Ordering::Acquire), 7);
    // the total still bounds everyone
    let last = slot(20);
    assert!(last.log_in(Who::User("fay")).is_ok());
    assert_eq!(slot(21).log_in(Who::User("gus")), Err(SessionRefused::Total));
    assert!(
        !moqui_lib::sync::lock(&counts.by_user).contains_key("gus"),
        "a refused login holds no share"
    );
    drop((first, third, again, busy, one_more, past, last));
    assert_eq!(counts.sessions.load(Ordering::Acquire), 0);
    assert!(moqui_lib::sync::lock(&counts.by_user).is_empty());
    // 0 is no share
    let open = Arc::new(Counts::new(8, 0, 0).with_dialogs(0, 0));
    let mut many = Vec::new();
    for _ in 0..8 {
        let slot = Slot::preauth(&open, at(1)).unwrap();
        assert!(slot.log_in(Who::User("ann")).is_ok());
        many.push(slot);
    }
}

/// Connections with no user hold only their share of the sessions, and of it one source only its own, so they
/// never keep a key or password login out (SEC-FIL-87).
#[test]
fn dialogs_hold_only_their_share_of_the_sessions() {
    let counts = Arc::new(Counts::new(8, 4, 8).with_dialogs(4, 2));
    let at = |last: u8| std::net::IpAddr::from([198, 51, 100, last]);
    let slot = |last: u8| Slot::preauth(&counts, at(last)).unwrap();
    let one = [slot(1), slot(1)];
    assert!(one.iter().all(|slot| slot.log_in(Who::Dialog).is_ok()));
    let third = slot(1);
    assert_eq!(
        third.log_in(Who::Dialog),
        Err(SessionRefused::Dialog),
        "the source's share"
    );
    assert_eq!(SessionRefused::Dialog.limit(), "max-dialog-sessions");
    // the same address still logs in by key or password
    assert!(third.log_in(Who::User("ann")).is_ok());
    let two = [slot(2), slot(2)];
    assert!(two.iter().all(|slot| slot.log_in(Who::Dialog).is_ok()));
    let past = slot(3);
    assert_eq!(
        past.log_in(Who::Dialog),
        Err(SessionRefused::Dialog),
        "the dialogs' share"
    );
    // the sessions a key or password login needs are still there
    assert!(past.log_in(Who::User("bob")).is_ok());
    drop(one);
    let again = slot(3);
    assert!(again.log_in(Who::Dialog).is_ok(), "a dialog's place came back");
    drop((third, two, past, again));
    assert_eq!(counts.dialogs.held(), 0);
    assert_eq!(counts.sessions.load(Ordering::Acquire), 0);
}

/// A dialog that logs in gives back its place among the dialogs and takes its user's share, so logged-in dialog users
/// are bounded by max-sessions-per-user and not by max-dialog-sessions; a failed login gives the user's share back.
#[test]
fn a_dialog_that_logs_in_trades_its_dialog_place_for_its_user_s_share() {
    let counts = Arc::new(Counts::new(8, 1, 0).with_dialogs(1, 0));
    let at = |last: u8| std::net::IpAddr::from([198, 51, 100, last]);
    let first = Slot::preauth(&counts, at(1)).unwrap();
    first.log_in(Who::Dialog).unwrap();
    assert_eq!(
        Slot::preauth(&counts, at(2)).unwrap().log_in(Who::Dialog),
        Err(SessionRefused::Dialog),
        "the one dialog place is held"
    );
    // a wrong password takes and gives back the user's share
    first.take_dialog_user("ann").unwrap();
    first.give_back_dialog_user();
    assert!(moqui_lib::sync::lock(&counts.by_user).is_empty());
    first.take_dialog_user("ann").unwrap();
    first.dialog_logged_in();
    assert_eq!(counts.dialogs.held(), 0, "the dialog place came back at login");
    let second = Slot::preauth(&counts, at(2)).unwrap();
    second.log_in(Who::Dialog).unwrap();
    // ann has her one session: her second dialog login is refused, and bob's goes
    assert!(second
        .take_dialog_user("ann")
        .unwrap_err()
        .contains("max-sessions-per-user"));
    second.take_dialog_user("bob").unwrap();
    drop((first, second));
    assert!(moqui_lib::sync::lock(&counts.by_user).is_empty());
    assert_eq!(counts.sessions.load(Ordering::Acquire), 0);
}

/// `git-any-account` serves git to an account that proved a key, and gives a password login nothing.
#[test]
fn git_any_account_is_for_a_key_login_only() {
    let login = |has_permission, by_password, any_account, password_auth| {
        git_allowed(GitLogin {
            has_permission,
            by_password,
            any_account,
            password_auth,
        })
    };
    // no SSH_GIT: by key only with the attribute; by password never, whatever git-password-auth says
    assert_eq!(login(false, false, false, false), GitAccess::Refused);
    assert_eq!(login(false, false, true, false), GitAccess::Served);
    assert_eq!(login(false, true, true, false), GitAccess::Refused);
    assert_eq!(login(false, true, true, true), GitAccess::Refused);
    // SSH_GIT: as before the attribute, with it and without
    for any_account in [false, true] {
        assert_eq!(login(true, false, any_account, false), GitAccess::Served);
        assert_eq!(login(true, true, any_account, false), GitAccess::NeedsKey);
        assert_eq!(login(true, true, any_account, true), GitAccess::Served);
    }
}

/// A connection let in with no user is closed when it opens no `apps` within the start wait (the pre-auth time, 30
/// s), or its dialog does not log in within the login wait; one that logs in is not (SEC-FIL-87).
#[tokio::test]
async fn a_dialog_with_no_apps_or_no_login_is_closed_in_time() {
    let (start, login) = (Duration::from_millis(50), Duration::from_millis(150));
    let counts = Arc::new(Counts::new(8, 0, 0));
    let dialog = || {
        let slot = Slot::preauth(&counts, std::net::IpAddr::from([198, 51, 100, 7])).unwrap();
        slot.log_in(Who::Dialog).unwrap();
        slot.dialog.store(true, Ordering::Release);
        slot
    };
    async fn ends(slot: &Slot, within: Duration, (start, login): (Duration, Duration)) -> bool {
        let since = tokio::time::Instant::now();
        tokio::time::timeout(within, dialog_expired_within(slot, since, start, login))
            .await
            .is_ok()
    }
    let waits = (start, login);
    // no apps: closed at the start wait
    let idle = dialog();
    assert!(ends(&idle, Duration::from_millis(120), waits).await);
    // apps but no login: closed at the login wait, not before
    let waiting = dialog();
    waiting.dialog_started.store(true, Ordering::Release);
    assert!(!ends(&waiting, Duration::from_millis(100), waits).await);
    assert!(ends(&waiting, Duration::from_millis(300), waits).await);
    // logged in: never
    let logged_in = dialog();
    logged_in.dialog_started.store(true, Ordering::Release);
    logged_in.dialog_logged_in();
    assert!(!ends(&logged_in, Duration::from_millis(300), waits).await);
}

/// The limits apply while the server runs (plans/Runtime-Config.md §3.4): a lowered one refuses the next login and
/// keeps the sessions open past it; a raised one lets the next in.
#[test]
fn new_limits_apply_to_the_next_login_and_keep_the_sessions_open() {
    let counts = Arc::new(Counts::of(&SshLimits {
        max_sessions: 4,
        max_per_user: 0,
        max_per_source: 0,
        max_dialogs: 2,
        max_dialogs_per_source: 0,
        ..SshLimits::default()
    }));
    let at = |last: u8| std::net::IpAddr::from([198, 51, 100, last]);
    let slot = |last: u8| Slot::preauth(&counts, at(last)).unwrap();
    let open = [slot(1), slot(1), slot(2)];
    assert!(open[0].log_in(Who::User("ann")).is_ok() && open[1].log_in(Who::User("ann")).is_ok());
    assert!(open[2].log_in(Who::Dialog).is_ok());
    counts.set(&SshLimits {
        max_sessions: 2,
        max_per_user: 1,
        max_per_source: 1,
        max_dialogs: 1,
        max_dialogs_per_source: 1,
        ..SshLimits::default()
    });
    assert_eq!(
        counts.sessions.load(Ordering::Acquire),
        3,
        "the sessions past the new total stay"
    );
    assert_eq!(slot(3).log_in(Who::User("bob")), Err(SessionRefused::Total));
    assert_eq!(slot(4).log_in(Who::Dialog), Err(SessionRefused::Dialog));
    drop(open);
    let first = slot(1);
    assert!(first.log_in(Who::User("ann")).is_ok());
    assert_eq!(
        slot(1).log_in(Who::User("cy")),
        Err(SessionRefused::Source),
        "the new share of a source"
    );
    assert_eq!(
        slot(5).log_in(Who::User("ann")),
        Err(SessionRefused::User),
        "the new share of a user"
    );
    counts.set(&SshLimits::default());
    assert!(
        slot(5).log_in(Who::User("ann")).is_ok(),
        "a raised share lets the next in"
    );
}

/// A connection at the sign-in dialog ends when the access mode leaves `open`, also when it got there after the change
/// was first seen; one signed in, or never at the dialog, does not.
#[tokio::test]
async fn a_dialog_ends_when_the_access_mode_leaves_open() {
    let counts = Arc::new(Counts::new(8, 0, 0));
    let slot = |dialog: bool| {
        let slot = Slot::preauth(&counts, std::net::IpAddr::from([198, 51, 100, 9])).unwrap();
        slot.dialog.store(dialog, Ordering::SeqCst);
        slot
    };
    async fn ends(slot: &Slot, off: tokio::sync::watch::Receiver<bool>) -> bool {
        tokio::time::timeout(Duration::from_millis(100), dialog_closed(slot, off))
            .await
            .is_ok()
    }
    let (off, seen) = tokio::sync::watch::channel(false);
    let at_dialog = slot(true);
    let signed_in = slot(true);
    signed_in.dialog_logged_in();
    let by_key = slot(false);
    assert!(!ends(&at_dialog, seen.clone()).await, "open");
    off.send_replace(true);
    assert!(ends(&at_dialog, seen.clone()).await);
    assert!(!ends(&signed_in, seen.clone()).await);
    assert!(!ends(&by_key, seen.clone()).await);
}
