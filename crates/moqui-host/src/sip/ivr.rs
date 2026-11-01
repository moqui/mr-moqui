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

//! The IVR, queues and the far end's audio while it waits (plans/Comms-Framework.md §5.3, V5; docs/interfaces/SIP.md
//! "IVR", "Queues", "Transfer and merge"). The voice services decide where a call goes (`IvrServices`,
//! `CallQueueServices`, `CallTransferServices`); the engine answers the call and runs what they answer:
//!
//! - **A menu** ([`Machine`], no I/O): its prompt, then a key, or up to `maxDigits` keys ended by `#` (an extension); a
//!   key during the prompt cuts it short. No key in `timeoutSeconds` plays it again, up to `maxRetries` times, then
//!   `route#IvrCall` with kind `timeout`; a key no option has is kind `invalid` when the menu has an invalid target, else
//!   a retry with the refusal tones first. The services answer the next menu, a queue, a ring step, voicemail or the end.
//! - **A queue**: hold music looped in place of the call's audio, and `tick#QueueCall` every `tickSeconds` (offers to the
//!   agents, the wait limit, the caller's place: a chime and a pip a place, the queue's position prompt, and digits
//!   spoken only when the deployment uploaded `0.wav` to `9.wav` in `call_queue_prompt_location`).
//! - **Ringing** after the IVR or during a blind transfer: a ringback tone looped, until someone answers.
//! - **Hold**: hold music, and the phone's audio kept from the others, until resumed.
//!
//! Prompts and music are the admin's uploaded audio (`dbresource://moqui/call/prompt/`), else the deployment's
//! `call_hold_music_location`, else tone patterns that are plainly synthetic. Moqui ships no recorded speech or music.

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value as Json;

use super::leg::{LegCommand, Prompt};
use super::prompt::tone;
use super::{call_service, number, step_token, text, Command, Engine};

const ROUTE_SERVICE: &str = "org.moqui.impl.IvrServices.route#IvrCall";
const TICK_SERVICE: &str = "org.moqui.impl.CallQueueServices.tick#QueueCall";
const EXPIRE_TRANSFER_SERVICE: &str = "org.moqui.impl.CallTransferServices.expire#CallTransfer";
const REFER_SERVICE: &str = "org.moqui.impl.CallTransferServices.refer#Call";
const RATE: usize = 48_000;
/// The longest prompt or music read; longer is cut.
const MAX_AUDIO_SECONDS: usize = 120;
/// The keys a menu takes at most.
const MAX_DIGITS: usize = 10;

/// A menu as `route#IvrCall` answers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Menu {
    pub(crate) id: String,
    pub(crate) prompt: Option<String>,
    pub(crate) timeout_secs: u64,
    pub(crate) retries: u32,
    pub(crate) max_digits: usize,
    pub(crate) options: Vec<String>,
    pub(crate) has_invalid: bool,
}

impl Menu {
    pub(crate) fn from_json(answer: &Json) -> Menu {
        let options = match answer.get("optionList") {
            Some(Json::Array(list)) => list
                .iter()
                .map(|o| text(o, "digits"))
                .filter(|d| !d.is_empty())
                .collect(),
            _ => Vec::new(),
        };
        Menu {
            id: text(answer, "ivrMenuId"),
            prompt: Some(text(answer, "promptLocation")).filter(|p| !p.is_empty()),
            timeout_secs: number(answer, "timeoutSeconds").unwrap_or(5).clamp(1, 60),
            retries: number(answer, "maxRetries").unwrap_or(2).min(5) as u32,
            max_digits: (number(answer, "maxDigits").unwrap_or(1) as usize).clamp(1, MAX_DIGITS),
            options,
            has_invalid: super::flag(answer, "hasInvalid"),
        }
    }
}

/// Why the caller leaves a menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RouteKind {
    Option,
    Invalid,
    Timeout,
}

impl RouteKind {
    fn code(self) -> &'static str {
        match self {
            RouteKind::Option => "option",
            RouteKind::Invalid => "invalid",
            RouteKind::Timeout => "timeout",
        }
    }
}

/// What a menu asks of the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    /// Play the menu's prompt (its audio, else its tones), the refusal tones first when `refused`; `Played(tag)` at its end.
    Play {
        tag: u32,
        prompt: Option<String>,
        refused: bool,
    },
    /// A key cut the prompt short.
    Stop,
    /// Tell `Idle(id)` after this many seconds.
    Wait { id: u32, seconds: u64 },
    /// Ask the services where the call goes.
    Route { kind: RouteKind, digits: String },
}

/// What happened in the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Event {
    Played(u32),
    Digit(char),
    Idle(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Prompting,
    Waiting,
    /// Asked the services; keys are ignored until a menu comes back.
    Routing,
}

/// One call's menu.
#[derive(Debug)]
pub(crate) struct Machine {
    menu: Menu,
    phase: Phase,
    tag: u32,
    wait: u32,
    tries: u32,
    digits: String,
}

impl Machine {
    /// A call entering `menu`: its prompt plays.
    pub(crate) fn start(menu: Menu) -> (Machine, Vec<Action>) {
        let mut machine = Machine {
            menu,
            phase: Phase::Prompting,
            tag: 0,
            wait: 0,
            tries: 0,
            digits: String::new(),
        };
        let actions = machine.prompt(false);
        (machine, actions)
    }

    /// The services sent the call to another menu (or this one again): it starts over there.
    pub(crate) fn load(&mut self, menu: Menu) -> Vec<Action> {
        self.menu = menu;
        self.tries = 0;
        self.prompt(false)
    }

    pub(crate) fn menu_id(&self) -> &str {
        &self.menu.id
    }

    pub(crate) fn tag(&self) -> u32 {
        self.tag
    }

    fn prompt(&mut self, refused: bool) -> Vec<Action> {
        self.tag += 1;
        self.digits.clear();
        self.phase = Phase::Prompting;
        vec![Action::Play {
            tag: self.tag,
            prompt: self.menu.prompt.clone(),
            refused,
        }]
    }

    fn wait(&mut self) -> Action {
        self.wait += 1;
        Action::Wait {
            id: self.wait,
            seconds: self.menu.timeout_secs,
        }
    }

    fn route(&mut self, kind: RouteKind) -> Vec<Action> {
        self.phase = Phase::Routing;
        self.wait += 1;
        let digits = std::mem::take(&mut self.digits);
        vec![Action::Route { kind, digits }]
    }

    /// Another try after no key or a key no option has; past the retries, the timeout target.
    fn retry(&mut self, refused: bool) -> Vec<Action> {
        self.tries += 1;
        if self.tries > self.menu.retries {
            return self.route(RouteKind::Timeout);
        }
        self.prompt(refused)
    }

    /// The keys so far are a choice: an option's, or one no option has.
    fn choose(&mut self) -> Vec<Action> {
        if self.menu.options.contains(&self.digits) {
            return self.route(RouteKind::Option);
        }
        if self.menu.has_invalid {
            return self.route(RouteKind::Invalid);
        }
        self.retry(true)
    }

    /// The next step for an event; a stale `Played` (a prompt that was replaced), an old wait, and keys while the call is
    /// being routed are ignored.
    pub(crate) fn on(&mut self, event: Event) -> Vec<Action> {
        match (self.phase, event) {
            (Phase::Routing, _) => Vec::new(),
            (Phase::Prompting, Event::Played(tag)) if tag == self.tag => {
                self.phase = Phase::Waiting;
                vec![self.wait()]
            }
            (_, Event::Digit(digit)) => {
                let mut actions = Vec::new();
                if self.phase == Phase::Prompting {
                    // a key cuts the prompt short (the prompt's end is stale from now on)
                    self.tag += 1;
                    self.phase = Phase::Waiting;
                    actions.push(Action::Stop);
                }
                if self.menu.max_digits == 1 {
                    self.digits.push(digit);
                    actions.extend(self.choose());
                    return actions;
                }
                if digit == '#' {
                    if self.digits.is_empty() {
                        actions.push(self.wait());
                        return actions;
                    }
                    actions.extend(self.choose());
                    return actions;
                }
                self.digits.push(digit);
                if self.digits.len() >= self.menu.max_digits {
                    actions.extend(self.choose());
                } else {
                    // the next key is waited for as long as the first
                    actions.push(self.wait());
                }
                actions
            }
            (Phase::Waiting, Event::Idle(id)) if id == self.wait => {
                if self.digits.is_empty() {
                    self.retry(false)
                } else {
                    // keys and then a pause: they are the choice
                    self.choose()
                }
            }
            _ => Vec::new(),
        }
    }
}

// ---------- tone patterns: plainly synthetic, never speech or music ----------

/// A menu's prompt with no uploaded audio: three rising tones.
const MENU_TONES: &[(f64, u32)] = &[(600.0, 150), (0.0, 80), (800.0, 150), (0.0, 80), (1000.0, 150)];
/// A key no option has: two falling tones.
const REFUSED_TONES: &[(f64, u32)] = &[(480.0, 250), (0.0, 80), (380.0, 350)];
/// Before the caller's place in a queue.
const CHIME: &[(f64, u32)] = &[(880.0, 150), (0.0, 100), (1320.0, 150), (0.0, 250)];

fn tones(pattern: &[(f64, u32)], out: &mut Vec<i16>) {
    for (hz, millis) in pattern {
        tone(*hz, *millis, out);
    }
}

/// A menu's prompt: the refusal tones first when asked, then its audio, else its tones.
pub(crate) fn menu_prompt(audio: Option<&[i16]>, refused: bool) -> Vec<i16> {
    let mut out = Vec::new();
    if refused {
        tones(REFUSED_TONES, &mut out);
        tone(0.0, 200, &mut out);
    }
    match audio.filter(|a| !a.is_empty()) {
        Some(audio) => out.extend_from_slice(audio),
        None => tones(MENU_TONES, &mut out),
    }
    out
}

/// Two tones at once, quieter than a prompt's.
fn dual(a: f64, b: f64, millis: u32, out: &mut Vec<i16>) {
    let n = millis as usize * (RATE / 1000);
    out.extend((0..n).map(|i| {
        let t = i as f64 / RATE as f64;
        (2500.0 * ((2.0 * std::f64::consts::PI * a * t).sin() + (2.0 * std::f64::consts::PI * b * t).sin())) as i16
    }));
}

/// Ringback while a call rings someone after the IVR or a transfer: 440 and 480 Hz, 2 s on and 4 s off.
pub(crate) fn ringback() -> Vec<i16> {
    let mut out = Vec::new();
    dual(440.0, 480.0, 2000, &mut out);
    tone(0.0, 4000, &mut out);
    out
}

/// Hold music with no audio set: a soft rising and falling four-tone figure, then a rest, looped.
pub(crate) fn hold_tones() -> Vec<i16> {
    let mut out = Vec::new();
    for hz in [523.25, 659.25, 783.99, 659.25] {
        let start = out.len();
        tone(hz, 250, &mut out);
        // quieter than a prompt
        for sample in &mut out[start..] {
            *sample /= 3;
        }
    }
    tone(0.0, 1500, &mut out);
    out
}

/// The caller's place in a queue: the chime, the queue's own prompt (`intro`), then the place as spoken digits when
/// the deployment has them all (`digit` gives each), else a pip a place (at most 9; a long tone for more).
pub(crate) fn position_prompt(
    position: u64,
    intro: Option<&[i16]>,
    digit: &mut dyn FnMut(char) -> Option<Vec<i16>>,
) -> Vec<i16> {
    let mut out = Vec::new();
    tones(CHIME, &mut out);
    if let Some(intro) = intro.filter(|a| !a.is_empty()) {
        out.extend_from_slice(intro);
        tone(0.0, 200, &mut out);
    }
    let text = position.to_string();
    let spoken: Option<Vec<Vec<i16>>> = text.chars().map(&mut *digit).collect();
    match spoken {
        Some(parts) => {
            for part in parts {
                out.extend_from_slice(&part);
                tone(0.0, 80, &mut out);
            }
        }
        None if position <= 9 => {
            for _ in 0..position {
                tone(1000.0, 120, &mut out);
                tone(0.0, 150, &mut out);
            }
        }
        None => tone(1000.0, 900, &mut out),
    }
    out
}

/// Where the waiting far end's loop comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LoopKind {
    Ringback,
    /// Hold music: this audio (a queue's, uploaded), else the deployment's, else [`hold_tones`].
    Music(Option<String>),
}

/// What a blind transfer's timer does with `expire#CallTransfer`'s answer: where the far end goes now, or nothing. A
/// transfer that is over (mode `done`: its target answered, the call ended) leaves the leg as it is now, which may be
/// waiting in a queue or ringing a later transfer's target; a `done` routed would stop that queue's ticks. One that failed
/// is logged and leaves the leg too, rather than ending a call its target may have answered.
fn expired_route(leg: &str, result: Result<Json, String>) -> Option<Result<Json, String>> {
    match result {
        Ok(out) => {
            let mode = out.get("answer").map(|a| text(a, "mode")).unwrap_or_default();
            (mode != "done").then_some(Ok(out))
        }
        Err(err) => {
            tracing::error!("SIP leg {leg}: the transfer's expiry failed: {err}");
            None
        }
    }
}

/// One call in a menu on the engine.
pub(super) struct IvrCall {
    machine: Machine,
}

/// One call waiting in a queue: its entry, the generation of its next tick (a newer one replaces it), the seconds
/// between ticks, and the ticks that failed in a row.
pub(super) struct QueueCall {
    entry: String,
    generation: u64,
    seconds: u64,
    failures: u32,
}

/// The ticks of a queue call that may fail in a row (the services unavailable for a moment) before the call ends.
const MAX_TICK_FAILURES: u32 = 3;

/// The audio at a location, 48 kHz mono; none when it is not set or does not read (logged).
fn location_audio(factory: &Arc<moqui_core::context::ExecutionContextFactory>, location: &str) -> Option<Vec<i16>> {
    if location.trim().is_empty() {
        return None;
    }
    match super::voicemail::location_bytes_job(factory, location)
        .and_then(|bytes| super::audio::decode_audio(&bytes, MAX_AUDIO_SECONDS))
    {
        Ok(pcm) if !pcm.is_empty() => Some(pcm),
        Ok(_) => None,
        Err(err) => {
            tracing::error!("SIP: the audio {location} did not play: {err}");
            None
        }
    }
}

impl Engine {
    /// Answers an inbound call for the IVR or a queue (receive#PhoneCall's `ivr`), then does what the services said.
    pub(super) fn start_ivr(&mut self, leg: &str, answer: Json) {
        if let Err(err) = self.answer_leg(leg) {
            tracing::warn!("SIP leg {leg}: not answered for the IVR: {err}");
            self.end_leg_here(leg);
            return;
        }
        self.route_answer(leg, &answer);
    }

    /// Where the services sent a call: a menu, a queue, a ring step, voicemail, done (it is answered, or over) or the end.
    pub(super) fn route_answer(&mut self, leg: &str, answer: &Json) {
        if !self.legs.contains_key(leg) {
            return;
        }
        let say = text(answer, "sayLocation");
        if !say.is_empty() {
            self.say(leg, say);
        }
        let mode = text(answer, "mode");
        if mode != "queue" && mode != "done" {
            self.queued.remove(leg);
        }
        if mode != "menu" {
            self.ivr.remove(leg);
        }
        match mode.as_str() {
            "menu" => {
                let menu = Menu::from_json(answer);
                self.set_loop(leg, None);
                let actions = match self.ivr.get_mut(leg) {
                    Some(call) => call.machine.load(menu),
                    None => {
                        let (machine, actions) = Machine::start(menu);
                        self.ivr.insert(leg.to_string(), IvrCall { machine });
                        actions
                    }
                };
                self.ivr_run(leg, actions);
            }
            "queue" => {
                let entry = text(answer, "callQueueEntryId");
                let new_entry = self.queued.get(leg).is_none_or(|q| q.entry != entry);
                if new_entry {
                    let music = Some(text(answer, "holdMusicLocation")).filter(|m| !m.is_empty());
                    self.set_loop(leg, Some(LoopKind::Music(music)));
                }
                if super::flag(answer, "announce") {
                    self.announce(leg, answer);
                }
                let seconds = number(answer, "tickSeconds").unwrap_or(5).clamp(1, 30);
                self.arm_tick(leg, entry, seconds, 0);
            }
            "ring" => {
                self.set_loop(leg, Some(LoopKind::Ringback));
                match number(answer, "ringSeconds").filter(|s| *s > 0) {
                    Some(seconds) => self.ring_timer(leg, seconds, step_token(answer)),
                    None => self.ring_timers.clear(leg),
                }
            }
            "voicemail" => {
                self.set_loop(leg, None);
                self.start_voicemail(leg, "busy", step_token(answer));
            }
            // the call no longer waits (an agent answered, or it ended); a tick answered as the agent answered may have
            // put the music back, which goes now unless the leg is on hold
            "done" => {
                self.queued.remove(leg);
                if self.legs.get(leg).is_some_and(|e| e.answered && !e.held) {
                    self.set_loop(leg, None);
                }
            }
            _ => {
                tracing::info!("SIP leg {leg}: the call ends ({})", text(answer, "reasonCode"));
                self.end_leg_here(leg);
            }
        }
    }

    /// An event of a call in a menu; false when the leg is in none.
    pub(super) fn ivr_event(&mut self, leg: &str, event: Event) -> bool {
        let Some(call) = self.ivr.get_mut(leg) else {
            return false;
        };
        let actions = call.machine.on(event);
        self.ivr_run(leg, actions);
        true
    }

    fn ivr_run(&mut self, leg: &str, actions: Vec<Action>) {
        for action in actions {
            let Some(call) = self.ivr.get(leg) else { return };
            match action {
                Action::Play { tag, prompt, refused } => {
                    let (factory, commands, leg) = (Arc::clone(&self.factory), self.commands.clone(), leg.to_string());
                    tokio::spawn(async move {
                        let built = tokio::task::spawn_blocking(move || {
                            let audio = prompt.as_deref().and_then(|p| location_audio(&factory, p));
                            menu_prompt(audio.as_deref(), refused)
                        })
                        .await;
                        if let Ok(pcm) = built {
                            let _ = commands.send(Command::IvrPlay {
                                leg,
                                tag,
                                prompt: Prompt(Arc::from(pcm)),
                            });
                        }
                    });
                }
                Action::Stop => {
                    if let Some(entry) = self.legs.get(leg) {
                        entry.media.send(LegCommand::Play {
                            tag: 0,
                            prompt: Prompt(Arc::from(Vec::new())),
                        });
                    }
                }
                Action::Wait { id, seconds } => {
                    let (commands, leg) = (self.commands.clone(), leg.to_string());
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(seconds)).await;
                        let _ = commands.send(Command::IvrIdle { leg, wait: id });
                    });
                }
                Action::Route { kind, digits } => {
                    let Some(entry) = self.legs.get(leg) else { return };
                    let parameters = serde_json::json!({"callSessionId": entry.room, "callLegId": leg,
                        "ivrMenuId": call.machine.menu_id(), "kind": kind.code(), "digits": digits});
                    self.routed_by(leg, ROUTE_SERVICE, parameters);
                }
            }
        }
    }

    /// Asks a service where the call goes; its `answer` comes back as [`Command::Routed`].
    fn routed_by(&self, leg: &str, service: &'static str, parameters: Json) {
        let (factory, commands, leg) = (Arc::clone(&self.factory), self.commands.clone(), leg.to_string());
        tokio::spawn(async move {
            let result = call_service(&factory, service, parameters).await;
            let _ = commands.send(Command::Routed { leg, result });
        });
    }

    /// The next tick of a call waiting in a queue, replacing any it had.
    fn arm_tick(&mut self, leg: &str, entry: String, seconds: u64, failures: u32) {
        self.next_tick += 1;
        let generation = self.next_tick;
        self.queued.insert(
            leg.to_string(),
            QueueCall {
                entry,
                generation,
                seconds,
                failures,
            },
        );
        let (commands, leg) = (self.commands.clone(), leg.to_string());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(seconds)).await;
            let _ = commands.send(Command::QueueTick { leg, generation });
        });
    }

    /// A service's answer of where the call goes. One that failed ends the call, but for a call waiting in a queue,
    /// whose tick is tried again (the caller keeps their place and music) up to [`MAX_TICK_FAILURES`] times in a row.
    pub(super) fn routed(&mut self, leg: &str, result: Result<Json, String>) {
        match result {
            Ok(out) => {
                let answer = out.get("answer").cloned().unwrap_or(Json::Null);
                self.route_answer(leg, &answer);
            }
            Err(err) if self.queued.get(leg).is_some_and(|q| q.failures + 1 < MAX_TICK_FAILURES) => {
                let Some(call) = self.queued.get(leg) else { return };
                let (entry, seconds, failures) = (call.entry.clone(), call.seconds, call.failures + 1);
                tracing::error!("SIP leg {leg}: its queue tick failed ({failures} in a row), tried again: {err}");
                self.arm_tick(leg, entry, seconds, failures);
            }
            Err(err) => {
                tracing::warn!("SIP leg {leg}: not routed, the call ends: {err}");
                self.end_leg_here(leg);
            }
        }
    }

    /// A menu's prompt is ready: it plays when it is still the menu's current one.
    pub(super) fn ivr_play(&mut self, leg: &str, tag: u32, prompt: Prompt) {
        if self.ivr.get(leg).is_none_or(|call| call.machine.tag() != tag) {
            return;
        }
        if let Some(entry) = self.legs.get(leg) {
            entry.media.send(LegCommand::Play { tag, prompt });
        }
    }

    /// A queue call's tick: the services offer it on, and say what the caller hears next.
    pub(super) fn queue_tick(&mut self, leg: &str, generation: u64) {
        let Some(call) = self.queued.get(leg).filter(|q| q.generation == generation) else {
            return;
        };
        let parameters = serde_json::json!({"callQueueEntryId": call.entry});
        self.routed_by(leg, TICK_SERVICE, parameters);
    }

    /// The caller's place in the queue, mixed over the music.
    fn announce(&self, leg: &str, answer: &Json) {
        let position = number(answer, "position").unwrap_or(0);
        if position == 0 {
            return;
        }
        let intro = text(answer, "positionPromptLocation");
        let (factory, commands, leg) = (Arc::clone(&self.factory), self.commands.clone(), leg.to_string());
        tokio::spawn(async move {
            let built = tokio::task::spawn_blocking(move || {
                let intro = location_audio(&factory, &intro);
                let folder = moqui_lib::env::get_prop_or_env("call_queue_prompt_location")
                    .trim()
                    .trim_end_matches('/')
                    .to_string();
                let mut digit = |d: char| {
                    if folder.is_empty() {
                        return None;
                    }
                    location_audio(&factory, &format!("{folder}/{d}.wav"))
                };
                position_prompt(position, intro.as_deref(), &mut digit)
            })
            .await;
            if let Ok(pcm) = built {
                let _ = commands.send(Command::Prompt {
                    leg,
                    prompt: Prompt(Arc::from(pcm)),
                });
            }
        });
    }

    /// An option's prompt ("connecting you to sales"), mixed over what plays next.
    fn say(&self, leg: &str, location: String) {
        let (factory, commands, leg) = (Arc::clone(&self.factory), self.commands.clone(), leg.to_string());
        tokio::spawn(async move {
            let built = tokio::task::spawn_blocking(move || location_audio(&factory, &location)).await;
            if let Ok(Some(pcm)) = built {
                let _ = commands.send(Command::Prompt {
                    leg,
                    prompt: Prompt(Arc::from(pcm)),
                });
            }
        });
    }

    /// The far end hears this in place of the call (ringback, hold music), or the call again (`None`). Music is read on
    /// a blocking thread; one that is ready after the loop changed again is dropped.
    pub(super) fn set_loop(&mut self, leg: &str, kind: Option<LoopKind>) {
        self.next_loop += 1;
        let token = self.next_loop;
        self.loops.insert(leg.to_string(), token);
        let Some(kind) = kind else {
            if let Some(entry) = self.legs.get(leg) {
                entry.media.send(LegCommand::Loop(None));
            }
            return;
        };
        let (factory, commands, leg) = (Arc::clone(&self.factory), self.commands.clone(), leg.to_string());
        tokio::spawn(async move {
            let built = tokio::task::spawn_blocking(move || match kind {
                LoopKind::Ringback => ringback(),
                LoopKind::Music(own) => {
                    let deployment = moqui_lib::env::get_prop_or_env("call_hold_music_location");
                    own.as_deref()
                        .and_then(|location| location_audio(&factory, location))
                        .or_else(|| location_audio(&factory, deployment.trim()))
                        .unwrap_or_else(hold_tones)
                }
            })
            .await;
            if let Ok(pcm) = built {
                let _ = commands.send(Command::LoopReady {
                    leg,
                    token,
                    audio: Prompt(Arc::from(pcm)),
                });
            }
        });
    }

    pub(super) fn loop_ready(&mut self, leg: &str, token: u64, audio: Prompt) {
        if self.loops.get(leg) != Some(&token) {
            return;
        }
        if let Some(entry) = self.legs.get(leg) {
            entry.media.send(LegCommand::Loop(Some(audio)));
        }
    }

    /// A blind transfer's target has `seconds` to answer; then `expire#CallTransfer` says where the far end goes.
    pub(super) fn transfer_timer(&self, leg: &str, seconds: u64, transfer: String) {
        let (factory, commands, leg) = (Arc::clone(&self.factory), self.commands.clone(), leg.to_string());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(seconds)).await;
            let result = call_service(
                &factory,
                EXPIRE_TRANSFER_SERVICE,
                serde_json::json!({"callTransferId": transfer}),
            )
            .await;
            if let Some(result) = expired_route(&leg, result) {
                let _ = commands.send(Command::Routed { leg, result });
            }
        });
    }

    /// A desk phone's REFER: its user's blind transfer of the call (`refer#Call`); a refusal is logged.
    pub(super) fn refer(&self, leg: &str, target: &str) {
        if !self.legs.contains_key(leg) {
            return;
        }
        let parameters = serde_json::json!({"callLegId": leg, "target": target});
        let (factory, leg) = (Arc::clone(&self.factory), leg.to_string());
        tokio::spawn(async move {
            if let Err(err) = call_service(&factory, REFER_SERVICE, parameters).await {
                tracing::info!("SIP leg {leg}: its REFER was refused: {err}");
            }
        });
    }

    /// The leg's media moves to another call's room (a merge); the services already moved its records.
    pub(super) fn move_leg(&mut self, leg: &str, room: &str) {
        let Some(entry) = self.legs.get_mut(leg) else {
            tracing::debug!("SIP: no leg {leg} here to move");
            return;
        };
        if room.is_empty() || entry.room == room {
            return;
        }
        entry.room = room.to_string();
        entry.media.send(LegCommand::Move(room.to_string()));
    }

    /// Answers the leg toward its trunk, once.
    pub(super) fn answer_leg(&mut self, leg: &str) -> Result<(), String> {
        let Some(entry) = self.legs.get_mut(leg) else {
            return Err("no such leg".to_string());
        };
        if entry.sip_answered {
            return Ok(());
        }
        self.ua.answer(leg)?;
        entry.sip_answered = true;
        Ok(())
    }

    /// The engine hangs up a leg it answered (the IVR or a queue ended it): the services hear it ended.
    pub(super) fn end_leg_here(&mut self, leg: &str) {
        self.ivr.remove(leg);
        self.queued.remove(leg);
        self.loops.remove(leg);
        if let Some(room) = self.legs.get(leg).map(|e| e.room.clone()) {
            self.end_leg(leg, &room);
        }
    }

    /// A leg is gone: what this module keeps of it goes too.
    pub(super) fn forget_ivr(&mut self, leg: &str) {
        self.ivr.remove(leg);
        self.queued.remove(leg);
        self.loops.remove(leg);
    }
}

#[cfg(test)]
#[path = "ivr_tests.rs"]
mod tests;
