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

use moqui_rtc::audio::FRAME;

use super::*;

fn tone_frame(tick: usize) -> Vec<i16> {
    (0..FRAME)
        .map(|i| (8000.0 * (2.0 * std::f64::consts::PI * 440.0 * (tick * FRAME + i) as f64 / 48_000.0).sin()) as i16)
        .collect()
}

fn plan(dir: &std::path::Path, max_ms: u64) -> Box<RecordPlan> {
    Box::new(RecordPlan {
        path: dir.join(SPOOL).join("leg-1.ogg"),
        max_ms,
        target: serde_json::json!({"voicemailBoxId": "B1"}),
    })
}

fn played(actions: &[Action]) -> (u32, Vec<Say>) {
    match actions {
        [Action::Play { tag, says }] => (*tag, says.clone()),
        other => panic!("one prompt, got {other:?}"),
    }
}

fn keys(machine: &mut Machine, digits: &str) -> Vec<Action> {
    digits.chars().flat_map(|d| machine.on(Event::Digit(d))).collect()
}

fn message(id: &str) -> Message {
    Message {
        id: id.to_string(),
        location: format!("dbresource://moqui/call/voicemail/B1/{id}.ogg"),
    }
}

/// No answer: the greeting, the beep, then the caller's audio as Ogg Opus until `#`, then goodbye and the hang-up.
#[test]
fn no_answer_plays_the_greeting_and_beep_then_records_an_ogg_until_the_hash_key() {
    let dir = std::env::temp_dir().join(format!("moqui-vm-{}", std::process::id()));
    let (mut machine, start) = Machine::deposit();
    let (tag, says) = played(&start);
    assert_eq!(says, vec![Say::Greeting, Say::Beep]);
    // a key in the greeting other than * does nothing; the greeting's end starts the recording
    assert!(machine.on(Event::Digit('5')).is_empty());
    assert!(
        machine.on(Event::Played(tag + 7)).is_empty(),
        "a stale prompt's end is ignored"
    );
    assert_eq!(machine.on(Event::Played(tag)), vec![Action::Record]);

    let mut recording = Recording::start(plan(&dir, 120_000)).expect("the spool opens");
    for tick in 0..75 {
        assert!(recording.push(&tone_frame(tick)), "not full yet");
    }
    assert_eq!(machine.on(Event::Digit('#')), vec![Action::StopRecording]);
    let recorded = recording.finish();
    assert_eq!(recorded.error, None);
    assert_eq!(recorded.duration_ms, 1500);
    let bytes = std::fs::read(&recorded.plan.path).unwrap();
    let packets = audio::ogg_packets(&bytes).unwrap();
    assert!(packets[0].starts_with(b"OpusHead") && packets[1].starts_with(b"OpusTags"));
    let pcm = audio::decode_ogg_opus(&bytes, 60).unwrap();
    assert!(
        (71_000..=76_000).contains(&pcm.len()),
        "1.5 s of audio: {} samples",
        pcm.len()
    );
    let loud = pcm.iter().filter(|s| s.unsigned_abs() > 2000).count();
    assert!(loud > pcm.len() / 4, "the tone is in it: {loud} loud samples");

    let (goodbye, says) = played(&machine.on(Event::Recorded));
    assert_eq!(says, vec![Say::Goodbye]);
    assert_eq!(machine.on(Event::Played(goodbye)), vec![Action::Hangup]);
    assert!(machine.is_done());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_recording_stops_at_its_limit_and_a_spool_that_cannot_open_is_an_ended_recording() {
    let dir = std::env::temp_dir().join(format!("moqui-vm-max-{}", std::process::id()));
    let mut recording = Recording::start(plan(&dir, 200)).unwrap();
    let mut frames = 0;
    while recording.push(&tone_frame(frames)) {
        frames += 1;
        assert!(frames < 100, "it stops");
    }
    let recorded = recording.finish();
    assert_eq!((recorded.duration_ms, recorded.error), (200, None));
    let _ = std::fs::remove_dir_all(&dir);

    let blocked = std::env::temp_dir().join(format!("moqui-vm-file-{}", std::process::id()));
    std::fs::write(&blocked, b"a file where the spool folder would be").unwrap();
    let failed = Recording::start(plan(&blocked, 1000))
        .err()
        .expect("the spool cannot open");
    assert!(failed.error.is_some());
    let _ = std::fs::remove_file(&blocked);
}

/// A caller who knows the PIN presses * in the greeting; a wrong PIN is refused and asked again, the right one plays the
/// new messages: 9 saves, 7 deletes, then goodbye.
#[test]
fn the_pin_wrong_then_right_and_the_menu_keys() {
    let (mut machine, _) = Machine::deposit();
    let (_, says) = played(&machine.on(Event::Digit('*')));
    assert_eq!(says, vec![Say::EnterPin]);
    assert!(keys(&mut machine, "12").is_empty());
    assert_eq!(
        keys(&mut machine, "#"),
        vec![Action::CheckPin {
            pin: "12".to_string(),
            box_number: None
        }]
    );
    let (_, says) = played(&machine.on(Event::Pin(PinAnswer::Refused { blocked: false })));
    assert_eq!(says, vec![Say::Refused, Say::EnterPin]);
    assert_eq!(
        keys(&mut machine, "4719#"),
        vec![Action::CheckPin {
            pin: "4719".to_string(),
            box_number: None
        }]
    );
    let (first, says) = played(&machine.on(Event::Pin(PinAnswer::Accepted(vec![message("V1"), message("V2")]))));
    assert_eq!(says[0], Say::MessageStart);
    assert_eq!(says[1], Say::Message(message("V1").location));
    // the menu after the message, then 9 saves it and the next one plays
    let (_, says) = played(&machine.on(Event::Played(first)));
    assert_eq!(says, vec![Say::Menu]);
    let actions = machine.on(Event::Digit('9'));
    assert_eq!(
        actions[0],
        Action::Act {
            voicemail_id: "V1".to_string(),
            op: "save"
        }
    );
    assert!(matches!(&actions[1], Action::Play { says, .. } if says[1] == Say::Message(message("V2").location)));
    // 7 during the message deletes it; no more: goodbye
    let actions = machine.on(Event::Digit('7'));
    assert_eq!(
        actions[0],
        Action::Act {
            voicemail_id: "V2".to_string(),
            op: "delete"
        }
    );
    let (bye, says) = played(&actions[1..]);
    assert_eq!(says, vec![Say::NoMessages, Say::Goodbye]);
    assert_eq!(machine.on(Event::Played(bye)), vec![Action::Hangup]);
}

/// The rate limit, as the call sees it: a blocked answer ends the call at once, and so do three refusals in one call;
/// # skips a message without a mark.
#[test]
fn a_block_or_three_refusals_end_the_call_and_hash_skips() {
    let (mut machine, _) = Machine::deposit();
    machine.on(Event::Digit('*'));
    keys(&mut machine, "1111#");
    let (bye, says) = played(&machine.on(Event::Pin(PinAnswer::Refused { blocked: true })));
    assert_eq!(says, vec![Say::Refused, Say::Goodbye]);
    assert_eq!(machine.on(Event::Played(bye)), vec![Action::Hangup]);

    let (mut machine, _) = Machine::deposit();
    machine.on(Event::Digit('*'));
    for attempt in 1..=MAX_TRIES {
        keys(&mut machine, "2222#");
        let (_, says) = played(&machine.on(Event::Pin(PinAnswer::Refused { blocked: false })));
        let last = if attempt == MAX_TRIES {
            Say::Goodbye
        } else {
            Say::EnterPin
        };
        assert_eq!(says, vec![Say::Refused, last]);
    }

    let (mut machine, _) = Machine::deposit();
    machine.on(Event::Digit('*'));
    keys(&mut machine, "4719#");
    machine.on(Event::Pin(PinAnswer::Accepted(vec![message("V1")])));
    let actions = machine.on(Event::Digit('#'));
    assert!(
        !actions.iter().any(|a| matches!(a, Action::Act { .. })),
        "# marks nothing"
    );
    let (_, says) = played(&actions);
    assert_eq!(says, vec![Say::NoMessages, Say::Goodbye]);
}

/// The access number asks for the box number, then the PIN; a refusal asks for both again.
#[test]
fn the_access_number_asks_for_the_box_then_the_pin() {
    let (mut machine, start) = Machine::access();
    assert_eq!(played(&start).1, vec![Say::EnterBox]);
    keys(&mut machine, "5551230000");
    assert_eq!(played(&keys(&mut machine, "#")).1, vec![Say::EnterPin]);
    assert_eq!(
        keys(&mut machine, "4719#"),
        vec![Action::CheckPin {
            pin: "4719".to_string(),
            box_number: Some("5551230000".to_string())
        }]
    );
    let (_, says) = played(&machine.on(Event::Pin(PinAnswer::Refused { blocked: false })));
    assert_eq!(says, vec![Say::Refused, Say::EnterBox]);
    assert!(
        machine.on(Event::Pin(PinAnswer::Accepted(Vec::new()))).is_empty(),
        "an answer out of turn is ignored"
    );
}

/// Prompts are tones with the deployment's audio after them; the user's own greeting plays alone, with no tones.
#[test]
fn prompts_are_tones_then_the_deployment_s_audio() {
    let none = render(&[Say::EnterPin], &mut |_| None);
    assert_eq!(none.len(), 400 * 48, "the tones alone");
    let spoken = render(&[Say::EnterPin], &mut |_| Some(vec![100; 4800]));
    assert_eq!(spoken.len(), (400 + 150) * 48 + 4800);
    let own = render(&[Say::Greeting, Say::Beep], &mut |say| {
        matches!(say, Say::Greeting).then(|| vec![100; 9600])
    });
    assert_eq!(own.len(), 9600 + 200 * 48 + 700 * 48, "the greeting, a gap, the beep");
}

/// A WAV greeting becomes Ogg Opus that decodes back to about as long.
#[test]
fn a_wav_greeting_converts_to_ogg_opus() {
    let pcm: Vec<i16> = (0..50).flat_map(tone_frame).collect();
    let mut wav = Vec::new();
    let data_len = (pcm.len() * 2) as u32;
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&48_000u32.to_le_bytes());
    wav.extend_from_slice(&96_000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for sample in &pcm {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    let decoded = audio::decode_audio(&wav, 60).unwrap();
    assert_eq!(decoded.len(), pcm.len());
    let ogg = audio::encode_ogg_opus(&decoded, 7).unwrap();
    let back = audio::decode_audio(&ogg, 60).unwrap();
    assert!(
        back.len() >= pcm.len() && back.len() <= pcm.len() + 2 * FRAME,
        "{}",
        back.len()
    );
    assert!(audio::decode_audio(b"not audio at all", 60).is_err());
}
