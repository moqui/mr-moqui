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

//! The voicemail fallback of an edge node's SIP engine (plans/Edge-Node.md §5.4 "Voice (SIP)", decision 4, E8): a call
//! the local node cannot route now (the link is down, or it did not answer in time) is answered from the number's
//! replicated fallback (`edge.rs`), with no call state anywhere:
//!
//! - **Forward.** The number's outside fallback (a ring group's outside number, or the user's mobile they ring with
//!   their calls) is dialed through the trunk the call came in on, from the number called, in a room of the two legs;
//!   its answer answers the caller, and either hanging up ends both. Unanswered after its seconds, or refused, the
//!   caller goes on to a message.
//! - **A message.** The number's box's greeting (the copy the queue brought, else the default), a beep and the
//!   recording, as any voicemail here (`voicemail.rs`), without phone access; queued to the local node, which finds the
//!   box and tells its owner when it takes it.
//! - **Neither.** 480, so the trunk's own failover can act.
//!
//! A call the local node routed whose `begin#Voicemail` finds it away midway takes a message the same way.

use std::sync::Arc;
use std::time::Duration;

use moqui_sip::ua::{Decision, IncomingCall};
use serde_json::Value as Json;

use super::edge::{self, Fallback};
use super::{Command, Engine};

/// An inbound leg answered from its number's fallback.
pub(crate) struct FallbackCall {
    fallback: Fallback,
    /// The trunk the call came in on, which a forward goes out through.
    trunk: String,
    /// The forward's outbound leg while it rings or talks.
    forward: Option<String>,
    /// The forward answered: the call is the two legs' now.
    answered: bool,
    /// The number called, whose forwards at once its forward counts in while it rings or talks.
    counted: Option<String>,
}

/// The forwards the fallback made of one number: those going on, and when each of the last day began. They are held to
/// the number's limits (from the local node's policy and the user's own, on its fallback row), as the forwards the local
/// node makes count against them.
#[derive(Default)]
pub(crate) struct ForwardCount {
    active: u64,
    started: std::collections::VecDeque<std::time::Instant>,
}

/// A day, for the forwards a day.
const DAY: Duration = Duration::from_secs(86_400);

impl ForwardCount {
    /// Takes one forward at `now` when the limits allow it: fewer than `at_once` going on, fewer than `a_day` begun in
    /// the last day.
    pub(crate) fn take(&mut self, at_once: u64, a_day: u64, now: std::time::Instant) -> bool {
        while self.started.front().is_some_and(|at| now.duration_since(*at) >= DAY) {
            self.started.pop_front();
        }
        if self.active >= at_once || self.started.len() as u64 >= a_day {
            return false;
        }
        self.active += 1;
        self.started.push_back(now);
        true
    }

    /// One forward ended.
    pub(crate) fn release(&mut self) {
        self.active = self.active.saturating_sub(1);
    }
}

/// The longest message, `voicemail_max_seconds` (10 to 600, 120 by default), as `Voicemail.js` reads it.
fn max_seconds() -> u64 {
    moqui_lib::env::get_prop_or_env("voicemail_max_seconds")
        .trim()
        .parse::<u64>()
        .unwrap_or(120)
        .clamp(10, 600)
}

/// What `begin#Voicemail` would answer for a message the fallback takes: a deposit with the number's greeting, marked
/// as the fallback's (no phone access; the local node finds the box).
pub(crate) fn deposit_answer(fallback: &Fallback, caller: &str) -> Json {
    serde_json::json!({"mode": "deposit", "fallback": true, "greetingLocation": fallback.greeting,
        "maxSeconds": max_seconds(), "fromNumber": caller})
}

impl Engine {
    /// A call routing could not reach the local node for: rung and answered from the fallback of the number `to`, or
    /// refused 480 when it has none.
    pub(super) async fn answer_from_fallback(&mut self, call: IncomingCall, to: String, from: String, srtp: bool) {
        let Some(fallback) = edge::fallback_of(&self.factory, &to).await else {
            tracing::info!("SIP call to {to}: no fallback while the local node is away, refused 480");
            let _ = call.reply.send(Decision::Reject(480));
            return;
        };
        if fallback.trunk.as_deref().is_some_and(|trunk| trunk != call.trunk_id) {
            tracing::info!("SIP call to {to}: not a number of trunk {}, refused 404", call.trunk_id);
            let _ = call.reply.send(Decision::Reject(404));
            return;
        }
        let id = match moqui_core::entity::uuid_v4() {
            Ok(id) => format!("{}{id}", edge::FALLBACK_PREFIX),
            Err(err) => {
                tracing::error!("SIP call to {to}: no id for its fallback, refused 480: {err}");
                let _ = call.reply.send(Decision::Reject(480));
                return;
            }
        };
        let (room, leg) = (id.clone(), format!("{id}-in"));
        let trunk = call.trunk_id.clone();
        if !self.ring_inbound(call, &room, &leg, &from, &to, srtp) {
            return;
        }
        let forward = fallback.forward.clone();
        let seconds = fallback.forward_seconds;
        self.fallbacks.insert(
            leg.clone(),
            FallbackCall {
                fallback,
                trunk,
                forward: None,
                answered: false,
                counted: None,
            },
        );
        if let Some(number) = forward {
            if self.forward_fallback(&leg, &number, seconds) {
                return;
            }
        }
        self.fallback_message(&leg);
    }

    /// Dials the fallback's outside number for the inbound `leg`, in its room; false when it could not be dialed.
    fn forward_fallback(&mut self, leg: &str, number: &str, seconds: u64) -> bool {
        let (Some(entry), Some(call)) = (self.legs.get(leg), self.fallbacks.get(leg)) else {
            return false;
        };
        let room = entry.room.clone();
        let dialed = entry.dialed.clone();
        let (at_once, a_day) = (call.fallback.forward_max_concurrent, call.fallback.forward_max_per_day);
        if !self
            .forward_counts
            .entry(dialed.clone())
            .or_default()
            .take(at_once, a_day, std::time::Instant::now())
        {
            tracing::warn!(
                "SIP fallback {room}: {dialed}'s forwards are at their limit ({at_once} at once, {a_day} a day)"
            );
            return false;
        }
        let Some(entry) = self.legs.get(leg) else { return false };
        let Some(call) = self.fallbacks.get(leg) else {
            return false;
        };
        let out = format!("{room}-out");
        let body = serde_json::json!({"callSessionId": room, "callLegId": out, "voiceTrunkId": call.trunk,
            "toNumber": number, "fromNumber": entry.dialed});
        self.dial(&body);
        if !self.legs.contains_key(&out) {
            tracing::warn!("SIP fallback {room}: the forward to {number} was not dialed");
            if let Some(count) = self.forward_counts.get_mut(&dialed) {
                count.release();
            }
            return false;
        }
        tracing::info!("SIP fallback {room}: forwarded to {number} for {seconds} s");
        self.forwards.insert(out.clone(), leg.to_string());
        if let Some(call) = self.fallbacks.get_mut(leg) {
            call.forward = Some(out);
            call.counted = Some(dialed);
        }
        let (commands, leg) = (self.commands.clone(), leg.to_string());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(seconds)).await;
            let _ = commands.send(Command::FallbackTimeout { leg });
        });
        true
    }

    /// The fallback's message for the inbound `leg`, or the end of the call when its number takes none.
    fn fallback_message(&mut self, leg: &str) {
        let Some(call) = self.fallbacks.get(leg) else { return };
        if !call.fallback.voicemail {
            if let Some(room) = self.legs.get(leg).map(|entry| entry.room.clone()) {
                self.end_leg(leg, &room);
            }
            return;
        }
        let caller = self.legs.get(leg).map(|entry| entry.caller.clone()).unwrap_or_default();
        let answer = deposit_answer(&call.fallback, &caller);
        self.voicemail_begun(leg.to_string(), "fallback", Ok(answer));
    }

    /// A routed call whose `begin#Voicemail` found the local node away: its number's fallback message, looked up off
    /// the engine task; a number with none ends the call.
    pub(super) fn message_from_fallback(&mut self, leg: &str) {
        let Some((to, caller)) = self
            .legs
            .get(leg)
            .map(|entry| (entry.dialed.clone(), entry.caller.clone()))
        else {
            return;
        };
        let (factory, commands, leg) = (Arc::clone(&self.factory), self.commands.clone(), leg.to_string());
        tokio::spawn(async move {
            let result = match edge::fallback_of(&factory, &to).await.filter(|f| f.voicemail) {
                Some(fallback) => Ok(deposit_answer(&fallback, &caller)),
                None => Err(format!(
                    "noRoute: the local node is away and {to} has no fallback message"
                )),
            };
            let _ = commands.send(Command::VmBegun {
                leg,
                reason: "fallback".to_string(),
                result,
            });
        });
    }

    /// An outbound leg answered: when it is a fallback's forward, its caller is answered too.
    pub(super) fn forward_answered(&mut self, out: &str) {
        let Some(leg) = self.forwards.get(out).cloned() else {
            return;
        };
        if let Some(call) = self.fallbacks.get_mut(&leg) {
            call.answered = true;
        }
        if let Some(entry) = self.legs.get_mut(&leg) {
            entry.answered = true;
        }
        if let Err(err) = self.answer_leg(&leg) {
            tracing::warn!("SIP fallback {leg}: the caller was not answered: {err}");
            self.forwards.remove(out);
            if self.legs.remove(out).is_some() {
                self.ua.hangup(out);
            }
            if let Some(room) = self.legs.get(&leg).map(|entry| entry.room.clone()) {
                self.end_leg(&leg, &room);
            }
        }
    }

    /// A leg ended (already out of `legs`): a fallback's forward that ends before its answer leaves the caller to a
    /// message; after it, or when the caller hangs up, the other leg hangs up too.
    pub(super) fn fallback_leg_ended(&mut self, leg: &str) {
        if let Some(inbound) = self.forwards.remove(leg) {
            let answered = self.fallbacks.get(&inbound).is_some_and(|call| call.answered);
            if let Some(call) = self.fallbacks.get_mut(&inbound) {
                call.forward = None;
            }
            self.release_forward(&inbound);
            if answered {
                if let Some(room) = self.legs.get(&inbound).map(|entry| entry.room.clone()) {
                    self.end_leg(&inbound, &room);
                }
            } else {
                self.fallback_message(&inbound);
            }
            return;
        }
        self.release_forward(leg);
        if let Some(call) = self.fallbacks.remove(leg) {
            if let Some(out) = call.forward {
                self.forwards.remove(&out);
                if self.legs.remove(&out).is_some() {
                    self.ua.hangup(&out);
                }
            }
        }
    }

    /// The forward of the inbound `leg` no longer rings or talks: it no longer counts in its number's forwards at once.
    fn release_forward(&mut self, leg: &str) {
        let Some(number) = self.fallbacks.get_mut(leg).and_then(|call| call.counted.take()) else {
            return;
        };
        if let Some(count) = self.forward_counts.get_mut(&number) {
            count.release();
        }
    }

    /// A fallback's forward rang its time: when nobody answered it, it is canceled and the caller goes on to a message.
    pub(super) fn forward_timed_out(&mut self, leg: &str) {
        let Some(call) = self.fallbacks.get_mut(leg) else {
            return;
        };
        if call.answered {
            return;
        }
        let Some(out) = call.forward.take() else { return };
        self.forwards.remove(&out);
        if self.legs.remove(&out).is_some() {
            self.ua.hangup(&out);
        }
        self.release_forward(leg);
        tracing::info!("SIP fallback {leg}: the forward was not answered, the caller leaves a message");
        self.fallback_message(leg);
    }
}

#[cfg(test)]
#[path = "fallback_tests.rs"]
mod tests;
