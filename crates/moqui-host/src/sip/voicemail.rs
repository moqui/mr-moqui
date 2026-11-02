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

//! Voicemail on the SIP engine (plans/Comms-Framework.md §5.3, V2; docs/interfaces/Voicemail.md): a phone leg that
//! nobody here answers. The voice services decide when (`VoicemailServices.begin#Voicemail`): an inbound call that rang
//! the box's rings with no answer (the engine's timer), whose user is in another call (`receive#PhoneCall` answers busy),
//! that its user declined (the control `voicemail`), or to the access number. Then the engine answers the call with no one
//! else in it and this module runs it:
//!
//! - **Leaving a message.** The greeting (the user's own, else the default: a tone pattern and the deployment's
//!   `greeting.wav`), a beep, then the caller's audio, decoded by the leg (M0) and written as Ogg Opus (W5's writer, through
//!   [`super::audio::OpusFile`]) to a spool under `runtime/tmp/voicemail`, until the caller hangs up, presses `#`, or
//!   `voicemail_max_seconds` passes. The file goes to `store#Voicemail`; one shorter than a second is dropped.
//! - **Phone access.** `*` during the greeting, or the access number after a box number and `#`, asks for the PIN and
//!   `#` (`check#VoicemailPin`, which limits failures per box and per source). Then the new messages one by one: `7`
//!   deletes, `9` saves (heard), `#` skips to the next; after the last, goodbye. Three refusals in a call, or a block, end
//!   it. A caller who keys nothing for [`IDLE_SECONDS`] after a box-number or PIN prompt hears it again, and the third
//!   time the call ends, so a silent caller does not hold a call.
//!
//! The prompts are tone patterns; the deployment's recorded speech plays after each when `voicemail_prompt_location`
//! names a folder with `greeting.wav`, `enter-box.wav`, `enter-pin.wav`, `pin-refused.wav`, `menu.wav`,
//! `no-messages.wav` and `goodbye.wav` (any resource location; 16-bit PCM WAV). Moqui ships no recorded speech.
//!
//! [`Machine`] is the call's state, with no I/O, so its tests need no SIP; the engine runs its actions (`impl Engine`
//! below) and the leg's thread plays and records (`leg.rs`).

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value as Json;

use super::audio::{self, OpusFile};
use super::leg::{LegCommand, Prompt};
use super::{call_service, text, Command, Engine};

const BEGIN_SERVICE: &str = "org.moqui.impl.VoicemailServices.begin#Voicemail";
const STORE_SERVICE: &str = "org.moqui.impl.VoicemailServices.store#Voicemail";
const PIN_SERVICE: &str = "org.moqui.impl.VoicemailServices.check#VoicemailPin";
const ACT_SERVICE: &str = "org.moqui.impl.VoicemailServices.act#PhoneVoicemail";
/// Where messages are spooled, under the runtime directory (`VOICEMAIL_SPOOL` in moqui-core `service/call_media.rs`).
pub(crate) const SPOOL: &str = "tmp/voicemail";
/// A message shorter than this (a caller who hung up at the beep) is not kept.
pub(crate) const MIN_MESSAGE_MS: u64 = 1000;
/// The PIN refusals one call takes before it ends.
pub(crate) const MAX_TRIES: u32 = 3;
/// How often the menu after a message, or a box-number or PIN prompt, is played before the call ends with no key.
const MENU_REPEATS: u32 = 3;
/// How long a box-number or PIN prompt waits for a key before it plays again.
pub(crate) const IDLE_SECONDS: u64 = 10;
/// The longest message played over the phone.
const MAX_PLAY_SECONDS: usize = 600;
/// The digits a box number or a PIN may have.
const MAX_DIGITS: usize = 20;
const RATE: usize = 48_000;

/// What the caller hears, each a tone pattern and, for the spoken ones, the deployment's recorded audio after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Say {
    /// The box's own greeting, else the default.
    Greeting,
    Beep,
    EnterBox,
    EnterPin,
    Refused,
    /// Before each message.
    MessageStart,
    /// A message's audio, at its location.
    Message(String),
    /// After a message: 7 delete, 9 save, # next.
    Menu,
    NoMessages,
    Goodbye,
}

impl Say {
    /// Its tones: (Hz, milliseconds), 0 Hz a silence.
    pub(crate) fn tones(&self) -> &'static [(f64, u32)] {
        match self {
            Say::Greeting => &[(660.0, 200), (0.0, 100), (880.0, 200)],
            Say::Beep => &[(0.0, 200), (1000.0, 500)],
            Say::EnterBox => &[(880.0, 150), (0.0, 100), (880.0, 150), (0.0, 100), (880.0, 150)],
            Say::EnterPin => &[(880.0, 150), (0.0, 100), (880.0, 150)],
            Say::Refused => &[(480.0, 300), (0.0, 100), (380.0, 400)],
            Say::MessageStart => &[(1200.0, 80), (0.0, 300)],
            Say::Message(_) => &[],
            Say::Menu => &[(700.0, 100), (0.0, 80), (900.0, 100)],
            Say::NoMessages => &[(600.0, 300)],
            Say::Goodbye => &[(880.0, 150), (660.0, 150), (440.0, 300)],
        }
    }

    /// The file of its recorded speech in `voicemail_prompt_location`, if it has one.
    pub(crate) fn file(&self) -> Option<&'static str> {
        Some(match self {
            Say::Greeting => "greeting.wav",
            Say::EnterBox => "enter-box.wav",
            Say::EnterPin => "enter-pin.wav",
            Say::Refused => "pin-refused.wav",
            Say::Menu => "menu.wav",
            Say::NoMessages => "no-messages.wav",
            Say::Goodbye => "goodbye.wav",
            Say::Beep | Say::MessageStart | Say::Message(_) => return None,
        })
    }
}

/// What the call's state asks of the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    /// Play these, one after another, in place of anything playing; `Played(tag)` when they end.
    Play {
        tag: u32,
        says: Vec<Say>,
    },
    /// Record the caller from now.
    Record,
    StopRecording,
    /// Check a PIN, for the call's box or the box a number names.
    CheckPin {
        pin: String,
        box_number: Option<String>,
    },
    /// A key on a message: delete or save.
    Act {
        voicemail_id: String,
        op: &'static str,
    },
    /// Tell `Idle(id)` after [`IDLE_SECONDS`].
    Wait(u32),
    Hangup,
}

/// A new message the PIN opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Message {
    pub(crate) id: String,
    pub(crate) location: String,
}

/// The PIN's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PinAnswer {
    Accepted(Vec<Message>),
    Refused { blocked: bool },
}

/// What happened on the call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Event {
    Played(u32),
    Digit(char),
    /// The recording ended (the `#` key, its limit, or a failure).
    Recorded,
    Pin(PinAnswer),
    /// The time of a [`Action::Wait`] passed.
    Idle(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Greeting,
    Recording,
    Stopping,
    BoxNumber,
    Pin,
    Checking,
    /// Playing message i.
    Message(usize),
    /// The menu after message i, played n times.
    Menu(usize, u32),
    Ending,
    Done,
}

/// One voicemail call's state: a deposit (the greeting, then a recording) or phone access (a box number, a PIN, then
/// the new messages).
#[derive(Debug)]
pub(crate) struct Machine {
    phase: Phase,
    tag: u32,
    digits: String,
    box_number: Option<String>,
    tries: u32,
    access: bool,
    messages: Vec<Message>,
    /// The wait for a box number or a PIN ([`Action::Wait`]): its id, whether a key came since it began, and how many
    /// passed with none.
    wait: u32,
    keyed: bool,
    idles: u32,
    /// `*` in the greeting opens nothing (an edge node's fallback, E8).
    no_access: bool,
}

impl Machine {
    /// A caller about to leave a message: the greeting and the beep play first.
    pub(crate) fn deposit() -> (Machine, Vec<Action>) {
        let mut machine = Machine::new(Phase::Greeting, false);
        let play = machine.play(vec![Say::Greeting, Say::Beep]);
        (machine, vec![play])
    }

    /// A message an edge node's fallback takes while its local node is away (E8): as [`Machine::deposit`], but `*`
    /// opens no phone access, which needs the local node.
    pub(crate) fn deposit_only() -> (Machine, Vec<Action>) {
        let (mut machine, actions) = Machine::deposit();
        machine.no_access = true;
        (machine, actions)
    }

    /// A caller on the access number: the box number first.
    pub(crate) fn access() -> (Machine, Vec<Action>) {
        let mut machine = Machine::new(Phase::BoxNumber, true);
        let play = machine.play(vec![Say::EnterBox]);
        (machine, vec![play])
    }

    fn new(phase: Phase, access: bool) -> Machine {
        Machine {
            phase,
            tag: 0,
            digits: String::new(),
            box_number: None,
            tries: 0,
            access,
            messages: Vec::new(),
            wait: 0,
            keyed: false,
            idles: 0,
            no_access: false,
        }
    }

    /// The tag of the prompt that should be playing; an older one that is ready late is not played.
    pub(crate) fn tag(&self) -> u32 {
        self.tag
    }

    #[cfg(test)]
    pub(crate) fn is_done(&self) -> bool {
        self.phase == Phase::Done
    }

    fn play(&mut self, says: Vec<Say>) -> Action {
        self.tag += 1;
        Action::Play { tag: self.tag, says }
    }

    fn end(&mut self, mut says: Vec<Say>) -> Vec<Action> {
        self.phase = Phase::Ending;
        says.push(Say::Goodbye);
        vec![self.play(says)]
    }

    fn ask_again(&mut self, first: Say) -> Vec<Action> {
        self.digits.clear();
        let ask = if self.access {
            self.box_number = None;
            self.phase = Phase::BoxNumber;
            Say::EnterBox
        } else {
            self.phase = Phase::Pin;
            Say::EnterPin
        };
        vec![self.play(vec![first, ask])]
    }

    fn message(&mut self, index: usize) -> Vec<Action> {
        match self.messages.get(index) {
            Some(message) => {
                let location = message.location.clone();
                self.phase = Phase::Message(index);
                vec![self.play(vec![Say::MessageStart, Say::Message(location)])]
            }
            None => self.end(vec![Say::NoMessages]),
        }
    }

    /// The next step for an event; stale `Played` tags (a prompt that was replaced) and keys that mean nothing now are
    /// ignored.
    pub(crate) fn on(&mut self, event: Event) -> Vec<Action> {
        let current = |tag: u32, machine: &Machine| tag == machine.tag;
        if matches!(self.phase, Phase::BoxNumber | Phase::Pin) && matches!(event, Event::Digit(_)) {
            self.keyed = true;
        }
        match (self.phase, event) {
            (Phase::Greeting, Event::Played(tag)) if current(tag, self) => {
                self.phase = Phase::Recording;
                vec![Action::Record]
            }
            (Phase::Greeting, Event::Digit('*')) if !self.no_access => {
                self.phase = Phase::Pin;
                self.digits.clear();
                vec![self.play(vec![Say::EnterPin])]
            }
            (Phase::Recording, Event::Digit('#')) => {
                self.phase = Phase::Stopping;
                vec![Action::StopRecording]
            }
            (Phase::Recording | Phase::Stopping, Event::Recorded) => self.end(Vec::new()),
            (Phase::BoxNumber | Phase::Pin, Event::Digit('*')) => {
                self.digits.clear();
                let ask = if self.phase == Phase::BoxNumber {
                    Say::EnterBox
                } else {
                    Say::EnterPin
                };
                vec![self.play(vec![ask])]
            }
            (Phase::BoxNumber, Event::Digit('#')) => {
                self.box_number = Some(std::mem::take(&mut self.digits));
                self.phase = Phase::Pin;
                vec![self.play(vec![Say::EnterPin])]
            }
            (Phase::Pin, Event::Digit('#')) => {
                self.phase = Phase::Checking;
                vec![Action::CheckPin {
                    pin: std::mem::take(&mut self.digits),
                    box_number: self.box_number.clone(),
                }]
            }
            (Phase::BoxNumber | Phase::Pin, Event::Digit(digit)) if digit.is_ascii_digit() => {
                if self.digits.len() < MAX_DIGITS {
                    self.digits.push(digit);
                }
                Vec::new()
            }
            // a box-number or PIN prompt ended: wait for keys
            (Phase::BoxNumber | Phase::Pin, Event::Played(tag)) if current(tag, self) => {
                self.keyed = false;
                self.wait += 1;
                vec![Action::Wait(self.wait)]
            }
            (Phase::BoxNumber | Phase::Pin, Event::Idle(wait)) if wait == self.wait => {
                if std::mem::take(&mut self.keyed) {
                    self.idles = 0;
                    self.wait += 1;
                    return vec![Action::Wait(self.wait)];
                }
                self.idles += 1;
                if self.idles >= MENU_REPEATS {
                    return self.end(Vec::new());
                }
                self.digits.clear();
                let ask = if self.phase == Phase::BoxNumber {
                    Say::EnterBox
                } else {
                    Say::EnterPin
                };
                vec![self.play(vec![ask])]
            }
            (Phase::Checking, Event::Pin(PinAnswer::Accepted(messages))) => {
                self.messages = messages;
                self.message(0)
            }
            (Phase::Checking, Event::Pin(PinAnswer::Refused { blocked })) => {
                self.tries += 1;
                if blocked || self.tries >= MAX_TRIES {
                    self.end(vec![Say::Refused])
                } else {
                    self.ask_again(Say::Refused)
                }
            }
            (Phase::Message(index), Event::Played(tag)) if current(tag, self) => {
                self.phase = Phase::Menu(index, 1);
                vec![self.play(vec![Say::Menu])]
            }
            (Phase::Menu(index, times), Event::Played(tag)) if current(tag, self) => {
                if times >= MENU_REPEATS {
                    self.end(Vec::new())
                } else {
                    self.phase = Phase::Menu(index, times + 1);
                    vec![self.play(vec![Say::Menu])]
                }
            }
            (Phase::Message(index) | Phase::Menu(index, _), Event::Digit(key @ ('7' | '9' | '#'))) => {
                let id = self.messages.get(index).map(|m| m.id.clone()).unwrap_or_default();
                let mut actions = match key {
                    '7' => vec![Action::Act {
                        voicemail_id: id,
                        op: "delete",
                    }],
                    '9' => vec![Action::Act {
                        voicemail_id: id,
                        op: "save",
                    }],
                    _ => Vec::new(),
                };
                actions.extend(self.message(index + 1));
                actions
            }
            (Phase::Ending, Event::Played(tag)) if current(tag, self) => {
                self.phase = Phase::Done;
                vec![Action::Hangup]
            }
            _ => Vec::new(),
        }
    }
}

/// Where and how long a recording goes, and what `store#Voicemail` is told with it.
#[derive(Debug)]
pub(crate) struct RecordPlan {
    pub(crate) path: PathBuf,
    pub(crate) max_ms: u64,
    /// `voicemailBoxId`, `callSessionId`, `fromNumber`, `fromName`.
    pub(crate) target: Json,
}

/// A recording that ended: its plan, its length and what went wrong, if anything.
#[derive(Debug)]
pub(crate) struct Recorded {
    pub(crate) plan: Box<RecordPlan>,
    pub(crate) duration_ms: u64,
    pub(crate) error: Option<String>,
}

/// The caller's audio being written, on the leg's thread.
pub(crate) struct Recording {
    file: Option<OpusFile<BufWriter<File>>>,
    plan: Box<RecordPlan>,
    error: Option<String>,
}

impl Recording {
    /// Opens the spool file; one that cannot be opened is a recording that ended at once, with why.
    pub(crate) fn start(plan: Box<RecordPlan>) -> Result<Recording, Recorded> {
        let opened = plan
            .path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| File::create(&plan.path))
            .map_err(|e| e.to_string())
            .and_then(|file| OpusFile::new(BufWriter::new(file), serial_of(&plan.path)));
        match opened {
            Ok(file) => Ok(Recording {
                file: Some(file),
                plan,
                error: None,
            }),
            Err(error) => Err(Recorded {
                plan,
                duration_ms: 0,
                error: Some(error),
            }),
        }
    }

    /// One frame of the caller's audio; false once the recording is full or failed (then finish it).
    pub(crate) fn push(&mut self, pcm: &[i16]) -> bool {
        let Some(file) = self.file.as_mut() else { return false };
        if let Err(err) = file.push(pcm) {
            self.error = Some(err);
            return false;
        }
        file.duration_ms() < self.plan.max_ms
    }

    /// Writes the last page and closes the file.
    pub(crate) fn finish(mut self) -> Recorded {
        let Some(file) = self.file.take() else {
            return Recorded {
                plan: self.plan,
                duration_ms: 0,
                error: self.error.or_else(|| Some("the recording did not start".to_string())),
            };
        };
        let duration_ms = file.duration_ms();
        let closed = file.finish().and_then(|mut out| {
            use std::io::Write;
            out.flush().map_err(|e| e.to_string())
        });
        Recorded {
            plan: self.plan,
            duration_ms,
            error: self.error.or(closed.err()),
        }
    }
}

fn serial_of(path: &std::path::Path) -> u32 {
    path.to_string_lossy()
        .bytes()
        .fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

/// A silence of `millis`.
fn silence(millis: u32, out: &mut Vec<i16>) {
    out.resize(out.len() + millis as usize * (RATE / 1000), 0);
}

/// The audio of `says`: each one's tones and then its audio (`audio_of`, which gives the greeting, a message or the
/// deployment's recorded speech when there is one), with a short silence between.
pub(crate) fn render(says: &[Say], audio_of: &mut dyn FnMut(&Say) -> Option<Vec<i16>>) -> Vec<i16> {
    let mut out = Vec::new();
    for say in says {
        if !out.is_empty() {
            silence(200, &mut out);
        }
        let own = audio_of(say);
        // the user's own greeting stands alone; the default's tones come before the deployment's audio
        let tones = !(matches!(say, Say::Greeting) && own.is_some());
        if tones {
            for (hz, millis) in say.tones() {
                super::prompt::tone(*hz, *millis, &mut out);
            }
        }
        if let Some(audio) = own.filter(|a| !a.is_empty()) {
            if tones && !say.tones().is_empty() {
                silence(150, &mut out);
            }
            out.extend_from_slice(&audio);
        }
    }
    out
}

/// One voicemail call on the engine: its state and what it needs to run.
pub(super) struct VmCall {
    machine: Machine,
    room: String,
    /// The box: the call's (a deposit), or the one the PIN opened.
    box_id: Option<String>,
    greeting: Option<String>,
    max_ms: u64,
    target: Json,
}

pub(super) fn location_bytes_job(
    factory: &Arc<moqui_core::context::ExecutionContextFactory>,
    location: &str,
) -> Result<Vec<u8>, String> {
    call_audio_job(factory, location, None)
}

/// [`location_bytes_job`], for the call `call` when it plays a saved message: on an edge node a file it does not have
/// comes from the local node (E8, `edge.rs` `fetch_audio`), a greeting or prompt kept here for the next time.
pub(super) fn call_audio_job(
    factory: &Arc<moqui_core::context::ExecutionContextFactory>,
    location: &str,
    call: Option<&str>,
) -> Result<Vec<u8>, String> {
    let job = {
        let (factory, location, call) = (Arc::clone(factory), location.to_string(), call.map(str::to_string));
        move || {
            // the engine's own read of a greeting, a message or a prompt, as internal work
            let _authz = moqui_core::context::ExecutionContext::with_current(
                moqui_core::context::ExecutionContext::disable_authz,
            );
            match factory.resource().location_bytes(&location) {
                Ok(None) if super::edge::is_edge(&factory) => {
                    super::edge::fetch_audio(&factory, &location, call.as_deref())
                        .map(Some)
                        .map_err(moqui_core::context::Error::new)
                }
                read => read,
            }
        }
    };
    let rx = factory
        .request_pool()
        .try_submit(Arc::clone(factory), job)
        .map_err(|e| e.message().to_string())?;
    match rx.recv() {
        Ok(Ok(Some(bytes))) => Ok(bytes),
        Ok(Ok(None)) => Err(format!("{location}: no file there")),
        Ok(Err(err)) => Err(format!("{location}: {}", err.message())),
        Err(_) => Err("request worker stopped".to_string()),
    }
}

/// The audio a `Say` plays besides its tones, read and decoded on a blocking thread; none is tones only.
fn audio_for(
    factory: &Arc<moqui_core::context::ExecutionContextFactory>,
    say: &Say,
    greeting: Option<&str>,
    call: &str,
) -> Option<Vec<i16>> {
    let (location, max_seconds) = match say {
        Say::Message(location) => (location.clone(), MAX_PLAY_SECONDS),
        Say::Greeting if greeting.is_some() => (greeting.unwrap_or_default().to_string(), audio::MAX_GREETING_SECONDS),
        _ => {
            let folder = moqui_lib::env::get_prop_or_env("voicemail_prompt_location")
                .trim()
                .to_string();
            let file = say.file()?;
            if folder.is_empty() {
                return None;
            }
            (
                format!("{}/{file}", folder.trim_end_matches('/')),
                audio::MAX_GREETING_SECONDS,
            )
        }
    };
    let message_of = matches!(say, Say::Message(_)).then_some(call);
    match call_audio_job(factory, &location, message_of).and_then(|bytes| audio::decode_audio(&bytes, max_seconds)) {
        Ok(pcm) => Some(pcm),
        Err(err) => {
            // the default prompt's file is optional; a user's greeting or a message that does not play is logged
            if matches!(say, Say::Message(_)) || (matches!(say, Say::Greeting) && greeting.is_some()) {
                tracing::error!("SIP voicemail: {location} did not play: {err}");
            } else {
                tracing::debug!("SIP voicemail: no prompt audio {location}: {err}");
            }
            None
        }
    }
}

impl Engine {
    /// Sends the leg to voicemail (reason noAnswer, busy, declined or access): the services move the call there first.
    /// `step` is the token of the plan's step this comes from (V3), so the services ignore a stale one.
    pub(super) fn start_voicemail(&mut self, leg: &str, reason: &str, step: Option<String>) {
        if !self.legs.contains_key(leg)
            || self.voicemail.contains_key(leg)
            || !self.voicemail_starting.insert(leg.to_string())
        {
            return;
        }
        let Some(entry) = self.legs.get(leg) else { return };
        let parameters =
            serde_json::json!({"callSessionId": entry.room, "callLegId": leg, "reason": reason, "stepToken": step});
        let (factory, commands, leg, reason) = (
            Arc::clone(&self.factory),
            self.commands.clone(),
            leg.to_string(),
            reason.to_string(),
        );
        tokio::spawn(async move {
            let result = call_service(&factory, BEGIN_SERVICE, parameters).await;
            let _ = commands.send(Command::VmBegun { leg, reason, result });
        });
    }

    /// The services' answer to [`Engine::start_voicemail`]: the call is answered and its greeting starts, or nothing
    /// changes (someone answered it, it ended), or it ends (any other refusal).
    pub(super) fn voicemail_begun(&mut self, leg: String, reason: &str, result: Result<Json, String>) {
        self.voicemail_starting.remove(&leg);
        let Some(entry) = self.legs.get(&leg) else { return };
        let room = entry.room.clone();
        let caller = entry.caller.clone();
        let dialed = entry.dialed.clone();
        let out = match result {
            Ok(out) => out,
            Err(err) => {
                let code = err.split(':').next().unwrap_or_default();
                // E8: the local node went away while the call rang: its number's fallback takes a message (once)
                if self.edge && super::edge::is_offline(&err) && reason != "fallback" {
                    tracing::warn!("SIP leg {leg}: the local node is away ({err}); the number's fallback answers");
                    self.message_from_fallback(&leg);
                    return;
                }
                // stale: a timer or control of a step the plan has already left (V3), which changes nothing
                if matches!(code, "answered" | "callEnded" | "stale") {
                    tracing::debug!("SIP leg {leg}: not to voicemail ({reason}): {err}");
                } else {
                    tracing::warn!("SIP leg {leg}: voicemail did not start ({reason}), the call ends: {err}");
                    self.end_leg(&leg, &room);
                }
                return;
            }
        };
        // V3: the call's handling plan rang its next step instead (a ring group's next member, a forward): it rings on,
        // with a new timer when the step has one
        if text(&out, "mode") == "ring" {
            tracing::debug!("SIP leg {leg}: rings its next step ({reason})");
            match super::number(&out, "ringSeconds").filter(|s| *s > 0) {
                Some(seconds) => self.ring_timer(&leg, seconds, super::step_token(&out)),
                None => self.ring_timers.clear(&leg),
            }
            return;
        }
        if let Err(err) = self.answer_leg(&leg) {
            tracing::warn!("SIP leg {leg}: not answered for voicemail: {err}");
            self.end_leg(&leg, &room);
            return;
        }
        // V5: no ringback or music under the greeting
        self.set_loop(&leg, None);
        let max_ms = super::number(&out, "maxSeconds").unwrap_or(120) * 1000;
        // E8: the fallback's message, while the local node is away, opens no phone access
        let fallback = super::flag(&out, "fallback");
        let (machine, actions) = if text(&out, "mode") == "access" {
            Machine::access()
        } else if fallback {
            Machine::deposit_only()
        } else {
            Machine::deposit()
        };
        let from_number = Some(text(&out, "fromNumber"))
            .filter(|n| !n.is_empty())
            .unwrap_or(caller);
        // the call, unless the fallback made it (a call the local node never heard of); the number called, which the
        // local node finds the box of for a message an edge node queued without one
        let call_id = (!super::edge::is_fallback(&room)).then(|| room.clone());
        let call = VmCall {
            machine,
            room: room.clone(),
            box_id: Some(text(&out, "voicemailBoxId")).filter(|b| !b.is_empty()),
            greeting: Some(text(&out, "greetingLocation")).filter(|g| !g.is_empty()),
            max_ms,
            target: serde_json::json!({"voicemailBoxId": text(&out, "voicemailBoxId"), "callSessionId": call_id,
                "fromNumber": from_number, "fromName": text(&out, "fromName"), "toNumber": dialed}),
        };
        tracing::info!("SIP leg {leg}: to voicemail ({reason})");
        self.voicemail.insert(leg.clone(), call);
        self.run_actions(&leg, actions);
    }

    /// An event of a voicemail call; false when the leg is not in voicemail.
    pub(super) fn voicemail_event(&mut self, leg: &str, event: Event) -> bool {
        let Some(call) = self.voicemail.get_mut(leg) else {
            return false;
        };
        let actions = call.machine.on(event);
        self.run_actions(leg, actions);
        true
    }

    /// The PIN's answer from the services.
    pub(super) fn voicemail_pin(&mut self, leg: &str, result: Result<Json, String>) {
        let answer = match result {
            Ok(out) if super::flag(&out, "accepted") => {
                if let Some(call) = self.voicemail.get_mut(leg) {
                    call.box_id = Some(text(&out, "voicemailBoxId")).filter(|b| !b.is_empty());
                }
                let messages = match out.get("messageList") {
                    Some(Json::Array(list)) => list
                        .iter()
                        .filter_map(|m| {
                            let (id, location) = (text(m, "voicemailId"), text(m, "audioLocation"));
                            (!id.is_empty() && !location.is_empty()).then_some(Message { id, location })
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                PinAnswer::Accepted(messages)
            }
            Ok(out) => PinAnswer::Refused {
                blocked: super::flag(&out, "blocked"),
            },
            Err(err) => {
                tracing::warn!("SIP leg {leg}: the voicemail PIN was not checked: {err}");
                PinAnswer::Refused { blocked: false }
            }
        };
        self.voicemail_event(leg, Event::Pin(answer));
    }

    /// A recording that ended on the leg's thread: stored when long enough, else removed; then the call goes on.
    pub(super) fn voicemail_recorded(&mut self, leg: &str, recorded: Recorded) {
        let path = recorded.plan.path.to_string_lossy().to_string();
        match &recorded.error {
            // E8: an edge node's message goes to the local node on the queue, routed or not, with its audio
            None if recorded.duration_ms >= MIN_MESSAGE_MS && self.edge => {
                let target = &recorded.plan.target;
                let received_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                let payload = serde_json::json!({"callSessionId": target.get("callSessionId"),
                    "toNumber": target.get("toNumber"), "fromNumber": target.get("fromNumber"),
                    "fromName": target.get("fromName"), "durationMs": recorded.duration_ms, "receivedMs": received_ms});
                let (factory, path) = (Arc::clone(&self.factory), recorded.plan.path.clone());
                tokio::spawn(async move { super::edge::queue_voicemail(&factory, &path, payload).await });
            }
            None if recorded.duration_ms >= MIN_MESSAGE_MS => {
                let mut parameters = recorded.plan.target.clone();
                if let Json::Object(map) = &mut parameters {
                    map.insert("tempPath".to_string(), Json::String(path));
                    map.insert("durationMs".to_string(), Json::from(recorded.duration_ms));
                }
                let factory = Arc::clone(&self.factory);
                tokio::spawn(async move {
                    if let Err(err) = call_service(&factory, STORE_SERVICE, parameters).await {
                        tracing::error!("SIP voicemail: the message was not stored: {err}");
                    }
                });
            }
            error => {
                if let Some(error) = error {
                    tracing::error!("SIP leg {leg}: the voicemail recording failed: {error}");
                }
                if let Err(err) = std::fs::remove_file(&recorded.plan.path) {
                    tracing::debug!("SIP voicemail: {path} not removed: {err}");
                }
            }
        }
        self.voicemail_event(leg, Event::Recorded);
    }

    fn run_actions(&mut self, leg: &str, actions: Vec<Action>) {
        for action in actions {
            let Some(call) = self.voicemail.get(leg) else { return };
            match action {
                Action::Play { tag, says } => {
                    let factory = Arc::clone(&self.factory);
                    let commands = self.commands.clone();
                    let greeting = call.greeting.clone();
                    let room = call.room.clone();
                    let leg = leg.to_string();
                    tokio::spawn(async move {
                        let built = tokio::task::spawn_blocking(move || {
                            render(&says, &mut |say| audio_for(&factory, say, greeting.as_deref(), &room))
                        })
                        .await;
                        if let Ok(pcm) = built {
                            let _ = commands.send(Command::VmPlay {
                                leg,
                                tag,
                                prompt: Prompt(Arc::from(pcm)),
                            });
                        }
                    });
                }
                Action::Record => {
                    let stamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_nanos())
                        .unwrap_or(0);
                    let name: String = leg
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                        .collect();
                    let plan = RecordPlan {
                        path: self
                            .factory
                            .runtime_dir()
                            .join(SPOOL)
                            .join(format!("{name}-{stamp}.ogg")),
                        max_ms: call.max_ms,
                        target: call.target.clone(),
                    };
                    if let Some(entry) = self.legs.get(leg) {
                        entry.media.send(LegCommand::Record(Box::new(plan)));
                    }
                }
                Action::StopRecording => {
                    if let Some(entry) = self.legs.get(leg) {
                        entry.media.send(LegCommand::StopRecording);
                    }
                }
                Action::CheckPin { pin, box_number } => {
                    let caller = self.legs.get(leg).map(|e| e.caller.clone()).unwrap_or_default();
                    let mut parameters = serde_json::json!({"pin": pin, "fromNumber": caller,
                        "callSessionId": call.room, "callLegId": leg});
                    if let Json::Object(map) = &mut parameters {
                        match (&box_number, &call.box_id) {
                            (Some(number), _) => map.insert("boxNumber".to_string(), Json::String(number.clone())),
                            (None, Some(id)) => map.insert("voicemailBoxId".to_string(), Json::String(id.clone())),
                            (None, None) => None,
                        };
                    }
                    let (factory, commands, leg) = (Arc::clone(&self.factory), self.commands.clone(), leg.to_string());
                    tokio::spawn(async move {
                        let result = call_service(&factory, PIN_SERVICE, parameters).await;
                        let _ = commands.send(Command::VmPin { leg, result });
                    });
                }
                Action::Act { voicemail_id, op } => {
                    let Some(box_id) = call.box_id.clone() else { continue };
                    // the call too, which an edge node's must have opened the box on (E8)
                    self.note(
                        ACT_SERVICE,
                        serde_json::json!({"voicemailBoxId": box_id, "voicemailId": voicemail_id, "op": op,
                            "callSessionId": call.room}),
                    );
                }
                Action::Wait(wait) => {
                    let (commands, leg) = (self.commands.clone(), leg.to_string());
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(IDLE_SECONDS)).await;
                        let _ = commands.send(Command::VmIdle { leg, wait });
                    });
                }
                Action::Hangup => {
                    let room = call.room.clone();
                    self.voicemail.remove(leg);
                    self.end_leg(leg, &room);
                    return;
                }
            }
        }
    }

    /// A prompt of a voicemail call is ready: it plays in place of what was playing. Prompts are made on their own
    /// threads, so one can be ready after a newer one (a long message after the goodbye a key asked for); only the
    /// machine's current one plays, or the newer would be replaced and its end, which the machine waits for, never told.
    pub(super) fn voicemail_play(&mut self, leg: &str, tag: u32, prompt: Prompt) {
        if self.voicemail.get(leg).is_none_or(|call| call.machine.tag() != tag) {
            return;
        }
        if let Some(entry) = self.legs.get(leg) {
            entry.media.send(LegCommand::Play { tag, prompt });
        }
    }

    /// The engine hangs up a leg it answered: the services hear it ended.
    pub(super) fn end_leg(&mut self, leg: &str, room: &str) {
        self.voicemail.remove(leg);
        if self.legs.remove(leg).is_some() {
            self.ua.hangup(leg);
            self.note_leg(room, leg, "ended", None, "");
        }
    }

    /// The timer of an inbound call's rings: unanswered by now, it goes to voicemail.
    /// Starts the leg's ring timer, replacing the one it had: when it ends the call goes on to its next step (V3) or to
    /// voicemail, through `begin#Voicemail`.
    pub(super) fn ring_timer(&mut self, leg: &str, seconds: u64, step: Option<String>) {
        let generation = self.ring_timers.arm(leg, step);
        let (commands, leg) = (self.commands.clone(), leg.to_string());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(seconds)).await;
            let _ = commands.send(Command::RingTimeout { leg, generation });
        });
    }
}

#[cfg(test)]
#[path = "voicemail_tests.rs"]
mod tests;
