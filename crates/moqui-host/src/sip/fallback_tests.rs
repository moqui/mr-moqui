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

use super::*;
use crate::sip::voicemail::{Action, Event, Machine, Say};

#[test]
fn the_fallback_s_message_is_a_deposit_without_phone_access() {
    let fallback = Fallback {
        voicemail: true,
        greeting: Some("dbresource://moqui/call/voicemail/greeting/B1/greeting-1.ogg".to_string()),
        ..Fallback::default()
    };
    let answer = deposit_answer(&fallback, "+15557770000");
    assert_eq!(answer["mode"], "deposit");
    assert_eq!(answer["fallback"], true);
    assert_eq!(answer["greetingLocation"], fallback.greeting.clone().unwrap());
    assert_eq!(answer["fromNumber"], "+15557770000");
    assert!(answer["maxSeconds"].as_u64().is_some_and(|s| (10..=600).contains(&s)));

    // * in the greeting opens nothing, which needs the local node; the recording starts after it as usual
    let (mut machine, actions) = Machine::deposit_only();
    let Some(Action::Play { tag, says }) = actions.first().cloned() else {
        panic!("{actions:?}")
    };
    assert_eq!(says, vec![Say::Greeting, Say::Beep]);
    assert!(machine.on(Event::Digit('*')).is_empty());
    assert_eq!(machine.on(Event::Played(tag)), vec![Action::Record]);
    // a deposit with phone access asks for the PIN on *
    let (mut machine, _) = Machine::deposit();
    assert!(
        matches!(machine.on(Event::Digit('*')).as_slice(), [Action::Play { says, .. }] if says == &vec![Say::EnterPin])
    );
}

#[test]
fn a_number_s_forwards_are_held_to_its_limits_at_once_and_a_day() {
    let mut count = ForwardCount::default();
    let start = std::time::Instant::now();
    assert!(count.take(2, 3, start));
    assert!(count.take(2, 3, start));
    assert!(!count.take(2, 3, start), "two at once");
    count.release();
    assert!(count.take(2, 3, start));
    count.release();
    assert!(!count.take(2, 3, start), "three a day");
    assert!(count.take(2, 3, start + DAY), "a day later");
    assert!(!ForwardCount::default().take(0, 10, start), "no limit forwards none");
}
