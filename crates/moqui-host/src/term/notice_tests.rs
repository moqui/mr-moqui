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

use std::time::Duration;

use super::{title_of, Settings};
use crate::term::attention::{Attention, Why};
use crate::term::session::Notice;

fn notice(why: Why, took: Option<u64>, notify: &str) -> Notice {
    Notice {
        id: "s1".to_string(),
        username: "dev".to_string(),
        title: "claude".to_string(),
        cwd: "/home/dev/mr-mantle".to_string(),
        notify: notify.to_string(),
        attention: Attention {
            why,
            text: String::new(),
            took: took.map(Duration::from_secs),
        },
    }
}

#[test]
fn by_default_only_a_blocked_session_notifies() {
    let settings = Settings::of(None);
    assert!(settings.wants(&notice(Why::Blocked, None, "default")));
    assert!(!settings.wants(&notice(Why::Done, None, "default")));
    assert!(!settings.wants(&notice(Why::Bell, None, "default")));
    assert!(!settings.wants(&notice(Why::LongCommand, Some(600), "default")));
    assert!(
        !settings.wants(&notice(Why::Blocked, None, "off")),
        "the session's own setting first"
    );
    assert!(settings.wants(&notice(Why::Done, None, "all")));
}

#[test]
fn a_long_command_notifies_past_the_users_time() {
    let settings = Settings::of(Some(r#"{"notifyLongCommandSeconds": 60, "notifyDone": true}"#));
    assert!(settings.wants(&notice(Why::LongCommand, Some(61), "default")));
    assert!(!settings.wants(&notice(Why::LongCommand, Some(5), "default")));
    assert!(settings.wants(&notice(Why::Done, None, "default")));
}

#[test]
fn the_title_says_what_where_and_why() {
    assert_eq!(
        title_of(&notice(Why::Blocked, None, "default")),
        "claude in mr-mantle is waiting"
    );
    assert_eq!(
        title_of(&notice(Why::LongCommand, Some(125), "default")),
        "claude in mr-mantle: the command finished after 2m 5s"
    );
}
