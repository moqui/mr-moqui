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

//! Stream management (XEP-0198): counts of the stanzas each side handled, acknowledgement requests every few stanzas,
//! and resumption. A session that enabled resumption and loses its connection stays registered, detached, and keeps
//! taking its pushes as unacknowledged stanzas; a new connection that resumes it within the timeout gets those it
//! missed. Its user is not marked offline meanwhile.

use std::collections::VecDeque;

use minidom::Element;

use super::xml::{attr, E};
use super::{Session, State};
use crate::ns;

/// Unacknowledged stanzas kept at most; past this the oldest are let go and the session can no longer be resumed (a
/// resumption would silently miss them), so a dropped connection ends it.
const MAX_UNACKED: usize = 1000;
/// The bytes of unacknowledged stanzas kept at most (as [`stanza_bytes`] counts them), let go past as [`MAX_UNACKED`]
/// (SEC-COM-93): a thousand large stanzas would otherwise be held for each session.
const MAX_UNACKED_BYTES: usize = 4 * 1024 * 1024;

/// About the bytes `element` takes written: its names, attributes and text, so a large stanza counts as large.
pub(crate) fn stanza_bytes(element: &Element) -> usize {
    let attributes: usize = element
        .attrs()
        .iter()
        .map(|((_, name), value)| name.len() + value.len() + 4)
        .sum();
    let children: usize = element
        .nodes()
        .map(|node| match node {
            minidom::Node::Element(child) => stanza_bytes(child),
            minidom::Node::Text(text) => text.len(),
        })
        .sum();
    2 * element.name().len() + 5 + attributes + children
}
/// An acknowledgement is asked for after this many stanzas.
const REQUEST_EVERY: u32 = 5;

pub(crate) struct Sm {
    pub id: String,
    pub resumable: bool,
    pub h_in: u32,
    h_out: u32,
    unacked: VecDeque<(u32, usize, Element)>,
    /// The bytes of [`Self::unacked`], as [`stanza_bytes`] counts them.
    unacked_bytes: usize,
    since_request: u32,
    /// Unacknowledged stanzas were let go: resumption is off.
    pub lost: bool,
}

impl Sm {
    fn new(resumable: bool) -> Sm {
        Sm {
            id: super::xml::random_id(),
            resumable,
            h_in: 0,
            h_out: 0,
            unacked: VecDeque::new(),
            unacked_bytes: 0,
            since_request: 0,
            lost: false,
        }
    }

    pub fn sent(&mut self, stanza: Element) {
        self.h_out = self.h_out.wrapping_add(1);
        let bytes = stanza_bytes(&stanza);
        self.unacked.push_back((self.h_out, bytes, stanza));
        self.unacked_bytes += bytes;
        self.since_request += 1;
        while self.unacked.len() > MAX_UNACKED || (self.unacked_bytes > MAX_UNACKED_BYTES && self.unacked.len() > 1) {
            if let Some((_, bytes, _)) = self.unacked.pop_front() {
                self.unacked_bytes -= bytes;
            }
            self.lost = true;
        }
    }

    pub fn received(&mut self) {
        self.h_in = self.h_in.wrapping_add(1);
    }

    /// The client handled the first `h` stanzas sent.
    pub fn acked(&mut self, h: u32) {
        while let Some((seq, _, _)) = self.unacked.front() {
            // wrapping comparison: seq is at or before h
            if h.wrapping_sub(*seq) < u32::MAX / 2 {
                if let Some((_, bytes, _)) = self.unacked.pop_front() {
                    self.unacked_bytes -= bytes;
                }
            } else {
                break;
            }
        }
    }

    pub fn request_ack(&mut self) -> Option<Element> {
        if self.since_request < REQUEST_EVERY {
            return None;
        }
        self.since_request = 0;
        Some(E::new("r", ns::SM).build())
    }

    pub fn unacked(&self) -> usize {
        self.unacked.len()
    }
}

impl Session {
    /// `<enable/>`: its `<enabled/>` answer.
    pub(super) fn enable_sm(&mut self, enable: &Element) -> Element {
        let resumable = matches!(attr(enable, "resume").as_str(), "true" | "1");
        let sm = Sm::new(resumable);
        let mut enabled = E::new("enabled", ns::SM).a("id", sm.id.clone());
        if resumable {
            enabled = enabled
                .a("resume", "true")
                .a("max", self.server.config.resume_timeout.as_secs().to_string());
        }
        self.sm = Some(sm);
        enabled.build()
    }

    pub(super) fn sm_nonza(&mut self, element: &Element) {
        match element.name() {
            "enable" => {
                if self.sm.is_some() {
                    self.send(
                        E::new("failed", ns::SM)
                            .c(E::new("unexpected-request", ns::STANZAS))
                            .build(),
                    );
                    return;
                }
                let enabled = self.enable_sm(element);
                self.send(enabled);
            }
            "r" => {
                let h = self.sm.as_ref().map(|sm| sm.h_in);
                match h {
                    Some(h) => self.send(E::new("a", ns::SM).a("h", h.to_string()).build()),
                    None => self.fail("unsupported-stanza-type", Some("stream management is not on")),
                }
            }
            "a" => {
                let h = attr(element, "h").parse::<u32>().ok();
                if let (Some(sm), Some(h)) = (&mut self.sm, h) {
                    sm.acked(h);
                }
            }
            "resume" => self.fail("unsupported-stanza-type", Some("resume before binding a resource")),
            _ => {}
        }
    }

    /// The `<resumed/>` of this detached session, after the client's count of what it got.
    pub(super) fn resumed_element(&mut self, h: u32) -> Element {
        let (h_in, id) = match &mut self.sm {
            Some(sm) => {
                sm.acked(h);
                (sm.h_in, sm.id.clone())
            }
            None => (0, String::new()),
        };
        E::new("resumed", ns::SM)
            .a("h", h_in.to_string())
            .a("previd", id)
            .build()
    }

    /// This detached session goes on over the connection of `new`, which resumed it.
    pub(super) fn adopt_connection(&mut self, new: &Session) {
        self.transport = new.transport;
        self.binding.clone_from(&new.binding);
        self.peer = new.peer;
        self.stream_id.clone_from(&new.stream_id);
        // the resuming connection signed in again, maybe with another app password
        self.credential.clone_from(&new.credential);
        self.state = State::Bound;
        self.detached = false;
        self.last_push = None;
        self.pushed_count = 0;
        self.out.clear();
    }

    /// Send again what the client did not acknowledge.
    pub(super) fn replay_unacked(&mut self) {
        let Some(sm) = &mut self.sm else {
            return;
        };
        let pending: Vec<Element> = sm.unacked.iter().map(|(_, _, stanza)| stanza.clone()).collect();
        for stanza in pending {
            self.out.push(super::Out::Element(stanza));
        }
    }

    /// RFC 6120 SASL, then `<resume/>` (XEP-0198 §5).
    pub(super) async fn resume(&mut self, element: &Element) {
        let previd = attr(element, "previd");
        let h = attr(element, "h").parse::<u32>().unwrap_or(0);
        let user_id = self.user_id();
        match self.server.registry.take_detached(&previd, &user_id).await {
            Some(mut old) => {
                let resumed = old.resumed_element(h);
                old.adopt_connection(self);
                old.send(resumed);
                old.replay_unacked();
                self.auth.swap = Some(old);
                self.state = State::Closed;
            }
            None => {
                self.send(
                    E::new("failed", ns::SM)
                        .c(E::new("item-not-found", ns::STANZAS))
                        .build(),
                );
            }
        }
    }

    /// The connection dropped: whether the session is kept detached.
    pub fn detach(&mut self) -> bool {
        if self.state != State::Bound || self.resumable().is_none() {
            return false;
        }
        self.detached = true;
        true
    }

    pub fn unacked(&self) -> usize {
        self.sm.as_ref().map_or(0, Sm::unacked)
    }
}

#[cfg(test)]
mod tests {
    use super::{stanza_bytes, Sm, MAX_UNACKED_BYTES};
    use crate::ns;
    use minidom::Element;

    /// SEC-COM-93: the unacknowledged stanzas are held to a byte bound as well as a count; past it the oldest go and
    /// resumption is off, as past the count.
    #[test]
    fn unacknowledged_stanzas_are_held_to_a_byte_bound() {
        let body = "x".repeat(200 * 1024);
        let large = Element::builder("message", ns::CLIENT)
            .append(Element::builder("body", ns::CLIENT).append(body.as_str()).build())
            .build();
        assert!(stanza_bytes(&large) > 200 * 1024);
        let mut sm = Sm::new(true);
        for _ in 0..10 {
            sm.sent(large.clone());
        }
        assert!(!sm.lost, "2 MB is within the bound");
        for _ in 0..20 {
            sm.sent(large.clone());
        }
        assert!(sm.lost);
        assert!(
            sm.unacked_bytes <= MAX_UNACKED_BYTES && sm.unacked() < 30,
            "{}",
            sm.unacked()
        );
        sm.acked(30);
        assert_eq!((sm.unacked(), sm.unacked_bytes), (0, 0));
    }
}
