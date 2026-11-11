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

use std::time::{Duration, Instant};

use super::{Status, Why};
use crate::term::track::Tracker;

fn feed(status: &mut Status, tracker: &mut Tracker, bytes: &[u8], at: Instant) -> (bool, Option<super::Attention>) {
    let changes = tracker.feed(bytes);
    status.output(&changes, at)
}

#[test]
fn shell_marks_give_working_then_done_or_error_and_a_long_command() {
    let (mut status, mut tracker, start) = (Status::default(), Tracker::default(), Instant::now());
    feed(&mut status, &mut tracker, b"\x1b]133;A\x07$ ", start);
    feed(&mut status, &mut tracker, b"\x1b]133;C\x07", start);
    assert_eq!(status.status, "working");
    let (changed, attention) = feed(
        &mut status,
        &mut tracker,
        b"\x1b]133;D;1\x07\x1b]133;A\x07$ ",
        start + Duration::from_secs(90),
    );
    assert!(changed);
    assert_eq!((status.status.as_str(), status.text.as_str()), ("error", "exit 1"));
    let attention = attention.unwrap();
    assert_eq!(
        (attention.why, attention.took),
        (Why::LongCommand, Some(Duration::from_secs(90)))
    );
    // output alone says nothing once marks were seen
    feed(&mut status, &mut tracker, b"typing", start + Duration::from_secs(91));
    assert_eq!(status.status, "error");
}

#[test]
fn a_bell_blocks_until_the_user_types_and_the_programs_word_outranks_it() {
    let (mut status, mut tracker, now) = (Status::default(), Tracker::default(), Instant::now());
    let (_, attention) = feed(&mut status, &mut tracker, b"\x1b]777;notify;claude;needs you\x07", now);
    assert_eq!(status.status, "blocked");
    assert_eq!(attention.unwrap().text, "claude: needs you");
    assert!(status.input());
    assert_eq!(status.status, "working");
    let (_, attention) = feed(
        &mut status,
        &mut tracker,
        b"\x1b]7501;state=blocked;kind=permission\x07",
        now,
    );
    assert_eq!(
        (attention.unwrap().why, status.text.as_str()),
        (Why::Blocked, "permission")
    );
    // after the program's word a bell changes nothing
    let (changed, attention) = feed(&mut status, &mut tracker, b"\x07", now);
    assert!(!changed && attention.is_none());
    let (_, attention) = feed(&mut status, &mut tracker, b"\x1b]7501;done\x07", now);
    assert_eq!(attention.unwrap().why, Why::Done);
}

#[test]
fn output_alone_is_working_then_idle_or_an_agents_quiet_done() {
    let start = Instant::now();
    let mut shell = Status::default();
    let mut tracker = Tracker::default();
    feed(&mut shell, &mut tracker, b"compiling", start);
    assert_eq!(shell.status, "working");
    assert_eq!(shell.tick(start + Duration::from_secs(3)), (true, None));
    assert_eq!(shell.status, "idle");
    let mut agent = Status::for_agent(true);
    feed(&mut agent, &mut tracker, b"thinking", start);
    assert!(!agent.tick(start + Duration::from_secs(3)).0, "not quiet long enough");
    let (changed, attention) = agent.tick(start + Duration::from_secs(6));
    assert!(changed);
    assert_eq!((attention.unwrap().why, agent.text.as_str()), (Why::Done, "quiet"));
}
