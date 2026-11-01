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

/// Unacknowledged stanzas kept at most; past this the stream ends, as the client does not acknowledge.
const MAX_UNACKED: usize = 1000;
/// An acknowledgement is asked for after this many stanzas.
const REQUEST_EVERY: u32 = 5;

pub(crate) struct Sm {
    pub id: String,
    pub resumable: bool,
    pub h_in: u32,
    h_out: u32,
    unacked: VecDeque<(u32, Element)>,
    since_request: u32,
}

impl Sm {
    fn new(resumable: bool) -> Sm {
        Sm {
            id: super::xml::random_id(),
            resumable,
            h_in: 0,
            h_out: 0,
            unacked: VecDeque::new(),
            since_request: 0,
        }
    }

    pub fn sent(&mut self, stanza: Element) {
        self.h_out = self.h_out.wrapping_add(1);
        self.unacked.push_back((self.h_out, stanza));
        self.since_request += 1;
        if self.unacked.len() > MAX_UNACKED {
            self.unacked.pop_front();
        }
    }

    pub fn received(&mut self) {
        self.h_in = self.h_in.wrapping_add(1);
    }

    /// The client handled the first `h` stanzas sent.
    pub fn acked(&mut self, h: u32) {
        while let Some((seq, _)) = self.unacked.front() {
            // wrapping comparison: seq is at or before h
            if h.wrapping_sub(*seq) < u32::MAX / 2 {
                self.unacked.pop_front();
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
        let pending: Vec<Element> = sm.unacked.iter().map(|(_, stanza)| stanza.clone()).collect();
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
