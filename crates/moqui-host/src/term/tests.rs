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

use std::sync::Arc;

use super::{Sessions, Terminal};

#[test]
fn sessions_are_limited_in_all_and_per_user() {
    let sessions = Arc::new(Sessions::new(3, 2));
    let first = sessions.take("a").unwrap();
    let _second = sessions.take("a").unwrap();
    assert!(sessions.take("a").err().unwrap().contains("max-sessions-per-user"));
    let _third = sessions.take("b").unwrap();
    assert!(sessions.take("c").err().unwrap().contains("max-sessions"));
    drop(first);
    assert!(sessions.take("c").is_ok());
}

#[test]
fn a_session_with_no_user_takes_the_users_slot_at_login() {
    let sessions = Arc::new(Sessions::new(4, 1));
    let _a = sessions.take("a").unwrap();
    let mut login = sessions.take("").unwrap();
    let _other = sessions.take("").unwrap();
    assert!(login.assign("a").unwrap_err().contains("max-sessions-per-user"));
    login.assign("b").unwrap();
    assert!(sessions.take("b").is_err());
    login.assign("").unwrap();
    let b = sessions.take("b").unwrap();
    drop(login);
    drop(b);
    assert!(sessions.take("b").is_ok());
}

#[test]
fn the_terminal_sets_colors_and_ascii() {
    let mut terminal = Terminal {
        term: "xterm-256color".to_string(),
        ..Terminal::default()
    };
    assert_eq!(terminal.theme().depth, moqui_tui::Depth::Ansi256);
    assert!(!terminal.theme().ascii);
    terminal.env.insert("COLORTERM".to_string(), "truecolor".to_string());
    terminal.env.insert("LANG".to_string(), "C".to_string());
    assert_eq!(terminal.theme().depth, moqui_tui::Depth::TrueColor);
    assert!(terminal.theme().ascii);
    terminal.env.insert("LC_ALL".to_string(), "en_US.UTF-8".to_string());
    assert!(!terminal.theme().ascii);
}
