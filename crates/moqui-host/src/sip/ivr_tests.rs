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

//! The IVR's menu ([`Machine`]) on simulated time: no SIP, no clock; a wait ends when the test says it did.

use super::*;

fn menu(id: &str, options: &[&str], max_digits: usize, has_invalid: bool) -> Menu {
    Menu {
        id: id.to_string(),
        prompt: Some(format!("dbresource://moqui/call/prompt/menu/{id}/prompt.ogg")),
        timeout_secs: 5,
        retries: 2,
        max_digits,
        options: options.iter().map(|o| o.to_string()).collect(),
        has_invalid,
    }
}

fn play_tag(actions: &[Action]) -> u32 {
    actions
        .iter()
        .find_map(|a| match a {
            Action::Play { tag, .. } => Some(*tag),
            _ => None,
        })
        .expect("a prompt plays")
}

fn wait_id(actions: &[Action]) -> u32 {
    actions
        .iter()
        .find_map(|a| match a {
            Action::Wait { id, .. } => Some(*id),
            _ => None,
        })
        .expect("a wait starts")
}

fn route(actions: &[Action]) -> Option<(RouteKind, String)> {
    actions.iter().find_map(|a| match a {
        Action::Route { kind, digits } => Some((*kind, digits.clone())),
        _ => None,
    })
}

#[test]
fn a_key_after_the_prompt_routes_to_its_option() {
    let (mut machine, actions) = Machine::start(menu("M1", &["1", "2"], 1, false));
    let tag = play_tag(&actions);
    assert!(
        matches!(&actions[0], Action::Play { prompt: Some(p), refused: false, .. } if p.ends_with("M1/prompt.ogg"))
    );
    let waiting = machine.on(Event::Played(tag));
    assert!(matches!(waiting[0], Action::Wait { seconds: 5, .. }));
    let chosen = machine.on(Event::Digit('2'));
    assert_eq!(route(&chosen), Some((RouteKind::Option, "2".to_string())));
    // while it is routed, keys and the old wait mean nothing
    assert!(machine.on(Event::Digit('1')).is_empty());
    assert!(machine.on(Event::Idle(wait_id(&waiting))).is_empty());
}

#[test]
fn a_key_during_the_prompt_cuts_it_short_and_routes() {
    let (mut machine, actions) = Machine::start(menu("M1", &["1"], 1, false));
    let tag = play_tag(&actions);
    let chosen = machine.on(Event::Digit('1'));
    assert_eq!(chosen[0], Action::Stop);
    assert_eq!(route(&chosen), Some((RouteKind::Option, "1".to_string())));
    // the cut prompt's end is stale
    assert!(machine.on(Event::Played(tag)).is_empty());
}

#[test]
fn no_key_plays_the_menu_again_then_goes_to_the_timeout_target() {
    let (mut machine, actions) = Machine::start(menu("M1", &["1"], 1, false));
    let mut tag = play_tag(&actions);
    // the first time, then retries 2: three plays in all, then the timeout target
    for round in 0..3 {
        let waiting = machine.on(Event::Played(tag));
        let next = machine.on(Event::Idle(wait_id(&waiting)));
        if round < 2 {
            tag = play_tag(&next);
            assert!(
                matches!(next[0], Action::Play { refused: false, .. }),
                "played again: {next:?}"
            );
            assert!(route(&next).is_none());
        } else {
            assert_eq!(route(&next), Some((RouteKind::Timeout, String::new())));
        }
    }
}

#[test]
fn a_key_no_option_has_goes_to_the_invalid_target_or_is_a_retry() {
    // with an invalid target: the services route it
    let (mut machine, actions) = Machine::start(menu("M1", &["1"], 1, true));
    let waiting = machine.on(Event::Played(play_tag(&actions)));
    assert!(!waiting.is_empty());
    assert_eq!(
        route(&machine.on(Event::Digit('9'))),
        Some((RouteKind::Invalid, "9".to_string()))
    );

    // without one: the refusal tones and the menu again, and the third bad key goes to the timeout target
    let (mut machine, actions) = Machine::start(menu("M2", &["1"], 1, false));
    machine.on(Event::Played(play_tag(&actions)));
    let again = machine.on(Event::Digit('7'));
    assert!(matches!(again[0], Action::Play { refused: true, .. }), "{again:?}");
    machine.on(Event::Played(play_tag(&again)));
    let again = machine.on(Event::Digit('8'));
    assert!(matches!(again[0], Action::Play { refused: true, .. }));
    machine.on(Event::Played(play_tag(&again)));
    // the timeout target, with the last bad key for the call's path
    assert_eq!(
        route(&machine.on(Event::Digit('8'))),
        Some((RouteKind::Timeout, "8".to_string()))
    );
    // a good key after a bad one still routes
    let (mut machine, actions) = Machine::start(menu("M3", &["1"], 1, false));
    machine.on(Event::Played(play_tag(&actions)));
    let again = machine.on(Event::Digit('5'));
    machine.on(Event::Played(play_tag(&again)));
    assert_eq!(
        route(&machine.on(Event::Digit('1'))),
        Some((RouteKind::Option, "1".to_string()))
    );
}

#[test]
fn an_extension_takes_keys_until_hash_its_length_or_a_pause() {
    let (mut machine, actions) = Machine::start(menu("Ext", &["101", "2001"], 4, true));
    machine.on(Event::Played(play_tag(&actions)));
    machine.on(Event::Digit('1'));
    machine.on(Event::Digit('0'));
    machine.on(Event::Digit('1'));
    assert_eq!(
        route(&machine.on(Event::Digit('#'))),
        Some((RouteKind::Option, "101".to_string()))
    );

    // four keys are the most: the fourth routes
    let (mut machine, actions) = Machine::start(menu("Ext", &["101", "2001"], 4, true));
    machine.on(Event::Played(play_tag(&actions)));
    for d in ['2', '0', '0'] {
        assert!(route(&machine.on(Event::Digit(d))).is_none());
    }
    assert_eq!(
        route(&machine.on(Event::Digit('1'))),
        Some((RouteKind::Option, "2001".to_string()))
    );

    // keys then a pause: the pause ends them
    let (mut machine, actions) = Machine::start(menu("Ext", &["101"], 4, true));
    machine.on(Event::Played(play_tag(&actions)));
    machine.on(Event::Digit('1'));
    let after = machine.on(Event::Digit('0'));
    let pause = wait_id(&after);
    assert_eq!(
        route(&machine.on(Event::Idle(pause))),
        Some((RouteKind::Invalid, "10".to_string()))
    );
}

#[test]
fn a_nested_menu_starts_over_with_its_own_prompt_and_retries() {
    let (mut machine, actions) = Machine::start(menu("Main", &["1", "2"], 1, false));
    machine.on(Event::Played(play_tag(&actions)));
    // a retry used in the main menu
    let again = machine.on(Event::Idle(1));
    machine.on(Event::Played(play_tag(&again)));
    assert_eq!(
        route(&machine.on(Event::Digit('2'))),
        Some((RouteKind::Option, "2".to_string()))
    );
    // the services answer the sales menu: its prompt plays, with all its retries
    let loaded = machine.load(menu("Sales", &["1", "9"], 1, false));
    assert_eq!(machine.menu_id(), "Sales");
    assert!(matches!(&loaded[0], Action::Play { prompt: Some(p), .. } if p.ends_with("Sales/prompt.ogg")));
    let mut tag = play_tag(&loaded);
    for _ in 0..2 {
        let waiting = machine.on(Event::Played(tag));
        tag = play_tag(&machine.on(Event::Idle(wait_id(&waiting))));
    }
    machine.on(Event::Played(tag));
    assert_eq!(
        route(&machine.on(Event::Digit('9'))),
        Some((RouteKind::Option, "9".to_string()))
    );
}

#[test]
fn a_menu_reads_from_the_services_answer() {
    let answer = serde_json::json!({"mode": "menu", "ivrMenuId": "M9", "promptLocation": null, "timeoutSeconds": 99,
        "maxRetries": 9, "maxDigits": 3, "optionList": [{"digits": "1"}, {"digits": "22"}], "hasInvalid": true});
    let menu = Menu::from_json(&answer);
    assert_eq!(menu.id, "M9");
    assert_eq!(menu.prompt, None);
    assert_eq!(menu.timeout_secs, 60, "held to a minute");
    assert_eq!(menu.retries, 5);
    assert_eq!(menu.max_digits, 3);
    assert_eq!(menu.options, vec!["1".to_string(), "22".to_string()]);
    assert!(menu.has_invalid);
}

#[test]
fn the_tone_patterns_are_synthetic_and_a_place_is_pips_or_spoken_digits() {
    let rate = 48;
    // the menu's tones without audio, its audio with; the refusal first when asked
    let tones = menu_prompt(None, false);
    assert_eq!(tones.len(), (150 + 80 + 150 + 80 + 150) * rate);
    let own = vec![7i16; 480];
    assert_eq!(menu_prompt(Some(&own), false), own);
    assert_eq!(menu_prompt(Some(&own), true).len(), (250 + 80 + 350 + 200) * rate + 480);
    // ringback 2 s on and 4 s off; hold tones a second and a half of rest after four notes
    assert_eq!(ringback().len(), 6000 * rate);
    assert!(ringback()[2000 * rate..].iter().all(|s| *s == 0));
    assert_eq!(hold_tones().len(), 2500 * rate);
    // place 3 with no digit prompts: the chime and three pips
    let mut none = |_: char| None;
    let chime = (150 + 100 + 150 + 250) * rate;
    assert_eq!(
        position_prompt(3, None, &mut none).len(),
        chime + 3 * (120 + 150) * rate
    );
    // place 12: a long tone with no digits; spoken when every digit has its prompt
    assert_eq!(position_prompt(12, None, &mut none).len(), chime + 900 * rate);
    let mut spoken = |_: char| Some(vec![1i16; 100]);
    assert_eq!(
        position_prompt(12, None, &mut spoken).len(),
        chime + 2 * (100 + 80 * rate)
    );
}

#[test]
fn a_transfer_that_is_over_leaves_the_leg_where_it_is_now() {
    // done: the target answered, or a later transfer put the leg in a queue, whose ticks a routed done would stop
    let done = serde_json::json!({"answer": {"mode": "done"}});
    assert!(expired_route("L1", Ok(done)).is_none());
    // a failed expiry is logged; the leg is not ended
    assert!(expired_route("L1", Err("db".to_string())).is_none());
    // nobody answered: the target's voicemail
    let voicemail = serde_json::json!({"answer": {"mode": "voicemail", "stepToken": "3"}});
    let routed = expired_route("L1", Ok(voicemail.clone())).expect("routed");
    assert_eq!(routed, Ok(voicemail));
}

#[test]
fn a_queue_calls_failed_tick_is_tried_again_until_the_failures_in_a_row_reach_the_limit() {
    let waiting = |failures| QueueCall {
        entry: "E1".to_string(),
        generation: 1,
        seconds: 5,
        failures,
    };
    // a call in no queue is not tried again: its failed route ends it
    assert_eq!(tick_retry(None), None);
    // a first and a second failure in a row are tried again, counted
    assert_eq!(tick_retry(Some(&waiting(0))), Some(1));
    assert_eq!(tick_retry(Some(&waiting(1))), Some(2));
    // the third in a row ends the call
    assert_eq!(tick_retry(Some(&waiting(MAX_TICK_FAILURES - 1))), None);
}
