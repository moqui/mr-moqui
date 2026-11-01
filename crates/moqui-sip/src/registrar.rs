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

//! The registrar's bindings for desk and soft phones, sans-IO (RFC 3261 §10.3, RFC 5626; plans/Comms-Framework.md §5.2
//! "Registrar for phones (V4)").
//!
//! A phone (a `SipDevice`) registers its contacts; each is a [`Binding`]: the contact, the flow its REGISTER came
//! over (transport, source address and port, as `rport` gives it, RFC 3581), its Call-ID and CSeq, its RFC 5626
//! instance and reg-id, its expiry and last refresh, and the phone's user agent. A device may have several bindings
//! (two phones on one account, or a soft phone on two computers), at most [`RegistrarLimits::max_bindings`].
//!
//! - **Expiry.** A contact's `expires`, else the request's `Expires`, else the default; one under the minimum is
//!   refused (423 with `Min-Expires`), one over the maximum is cut to it. A phone behind a NAT (its Via does not name
//!   where its request came from) that does not use Outbound gets the short NAT expiry, so its REGISTER keeps the
//!   NAT's mapping.
//! - **Outbound.** A REGISTER with `Supported: outbound` and a contact with `+sip.instance` and `reg-id` binds that
//!   flow (RFC 5626 §6): the binding is keyed by instance and reg-id instead of the contact, and the answer tells the
//!   phone to keep the flow up with CRLF keepalives (`Flow-Timer`, [`RegistrarLimits::flow_timer_udp`] or
//!   [`RegistrarLimits::flow_timer_stream`] seconds). When a stream flow closes, the bindings over it end
//!   ([`Registrar::drop_flow`]).
//! - **Removal.** `Expires: 0` on a contact removes it; `Contact: *` with `Expires: 0` removes them all.
//! - **Ordering.** A binding updated by a request with its Call-ID takes only a higher CSeq; anything else is out of
//!   order and the whole request is refused (RFC 3261 §10.3 step 6), changing nothing.
//! - **Revision.** A binding keeps the credential revision of the REGISTER that made it. A new revision (rotated
//!   credentials) or a revoked device ends them all ([`Registrar::drop_device`]).
//!
//! Times are seconds since the epoch, given by the caller, so the tests run on simulated time.

use std::collections::HashMap;
use std::net::SocketAddr;

use crate::ua::Transport;

/// Where a binding's requests go back: the flow the phone opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flow {
    pub transport: Transport,
    /// The address and port the REGISTER came from (Via `received` and `rport`).
    pub source: SocketAddr,
}

impl Flow {
    /// A TCP or TLS connection, which ends its bindings when it closes.
    pub fn is_stream(&self) -> bool {
        self.transport != Transport::Udp
    }
}

/// One registered contact of a device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// A random id of this binding (the `SipRegistration` row's).
    pub id: String,
    /// The device (`sipDeviceId`) and its username (the address of record's user).
    pub device: String,
    pub username: String,
    /// The credential revision of the REGISTER that made it.
    pub revision: u32,
    /// The Contact URI, without its header parameters.
    pub contact: String,
    pub flow: Flow,
    pub call_id: String,
    pub cseq: u32,
    /// RFC 5626: `+sip.instance` and `reg-id`, for an Outbound binding.
    pub instance: Option<String>,
    pub reg_id: Option<u32>,
    pub outbound: bool,
    pub expires_at: u64,
    pub refreshed_at: u64,
    pub user_agent: Option<String>,
}

impl Binding {
    /// Its seconds left at `now`.
    pub fn remaining(&self, now: u64) -> u64 {
        self.expires_at.saturating_sub(now)
    }

    fn same_key(&self, contact: &ContactParam, outbound: bool) -> bool {
        match (outbound, &contact.instance, contact.reg_id) {
            (true, Some(instance), Some(reg_id)) => {
                self.outbound && self.instance.as_deref() == Some(instance.as_str()) && self.reg_id == Some(reg_id)
            }
            _ => !self.outbound && same_uri(&self.contact, &contact.uri),
        }
    }
}

/// The registrar's limits, from the `sip_registrar_*` properties.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistrarLimits {
    pub min_expires: u32,
    pub max_expires: u32,
    pub default_expires: u32,
    /// The longest expiry of a binding behind a NAT without Outbound: its REGISTER refreshes the NAT's mapping.
    pub nat_expires: u32,
    /// The keepalive interval asked of an Outbound flow (`Flow-Timer`), for UDP and for a stream.
    pub flow_timer_udp: u32,
    pub flow_timer_stream: u32,
    pub max_bindings: usize,
}

impl Default for RegistrarLimits {
    fn default() -> Self {
        RegistrarLimits {
            min_expires: 60,
            max_expires: 3600,
            default_expires: 600,
            nat_expires: 60,
            flow_timer_udp: 25,
            flow_timer_stream: 120,
            max_bindings: 10,
        }
    }
}

/// A contact of a REGISTER, with its parameters.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContactParam {
    pub uri: String,
    pub expires: Option<u32>,
    pub instance: Option<String>,
    pub reg_id: Option<u32>,
}

/// The Contact of a REGISTER.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Contacts {
    /// No Contact: a query of the current bindings.
    None,
    /// `Contact: *`: remove them all (with `Expires: 0`).
    Star,
    List(Vec<ContactParam>),
}

/// A REGISTER from an authenticated device.
#[derive(Debug, Clone)]
pub struct Register {
    pub device: String,
    pub username: String,
    pub revision: u32,
    pub contacts: Contacts,
    /// The `Expires` header.
    pub expires: Option<u32>,
    pub call_id: String,
    pub cseq: u32,
    pub flow: Flow,
    /// `Supported: outbound` (RFC 5626).
    pub outbound: bool,
    /// The Via's sent-by is not where the request came from.
    pub behind_nat: bool,
    pub user_agent: Option<String>,
}

/// Why a REGISTER changed nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// An expiry under the minimum: 423 with `Min-Expires`.
    TooBrief(u32),
    /// A CSeq not above the binding's for the same Call-ID (RFC 3261 §10.3): 500.
    OutOfOrder,
    /// `*` with other contacts or a non-zero expiry: 400.
    BadRequest,
    /// More bindings than a device may have: 403.
    TooMany,
}

impl Refusal {
    pub fn status(self) -> u16 {
        match self {
            Refusal::TooBrief(_) => 423,
            Refusal::OutOfOrder => 500,
            Refusal::BadRequest => 400,
            Refusal::TooMany => 403,
        }
    }
}

/// What a REGISTER did.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    pub added: Vec<Binding>,
    pub refreshed: Vec<Binding>,
    pub removed: Vec<Binding>,
    /// The device's bindings now, for the 200's Contact list.
    pub current: Vec<Binding>,
    /// The `Flow-Timer` of an Outbound binding made or refreshed, for the 200 (with `Require: outbound`).
    pub flow_timer: Option<u32>,
}

/// The bindings of every device, by username.
#[derive(Debug, Default)]
pub struct Registrar {
    limits: RegistrarLimits,
    bindings: HashMap<String, Vec<Binding>>,
}

fn random_id() -> String {
    let mut bytes = [0u8; 12];
    let _ = getrandom::fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Two contact URIs are the same binding when they are equal but for the case of their scheme and host (RFC 3261
/// §10.3 step 5 compares URIs by §19.1.4; this keeps it to what phones send).
fn same_uri(a: &str, b: &str) -> bool {
    fn key(uri: &str) -> String {
        let uri = uri.trim();
        match uri.split_once('@') {
            Some((user, host)) => format!("{}@{}", user.to_ascii_lowercase(), host.to_ascii_lowercase()),
            None => uri.to_ascii_lowercase(),
        }
    }
    key(a) == key(b)
}

impl Registrar {
    pub fn new(limits: RegistrarLimits) -> Self {
        Registrar {
            limits,
            bindings: HashMap::new(),
        }
    }

    pub fn limits(&self) -> RegistrarLimits {
        self.limits
    }

    pub fn set_limits(&mut self, limits: RegistrarLimits) {
        self.limits = limits;
    }

    /// The expiry a contact gets, or the refusal of one too brief.
    fn expiry(&self, asked: Option<u32>, header: Option<u32>, nat_cap: bool) -> Result<u32, Refusal> {
        let asked = asked.or(header).unwrap_or(self.limits.default_expires);
        if asked == 0 {
            return Ok(0);
        }
        if asked < self.limits.min_expires {
            return Err(Refusal::TooBrief(self.limits.min_expires));
        }
        let mut granted = asked.min(self.limits.max_expires);
        if nat_cap {
            granted = granted.min(self.limits.nat_expires.max(self.limits.min_expires));
        }
        Ok(granted)
    }

    /// Applies a REGISTER of a device that authenticated. On a refusal nothing changes.
    pub fn register(&mut self, request: Register, now: u64) -> Result<Outcome, Refusal> {
        let existing = self.bindings.get(&request.username).cloned().unwrap_or_default();
        // a binding of an older revision is gone already (drop_device), but a lookup that raced a rotation could
        // leave one: it never survives a REGISTER of the new revision
        let mut list: Vec<Binding> = Vec::with_capacity(existing.len() + 1);
        let mut outcome = Outcome::default();
        for binding in existing {
            if binding.revision != request.revision || binding.expires_at <= now {
                outcome.removed.push(binding);
            } else {
                list.push(binding);
            }
        }
        let ordered = |binding: &Binding| binding.call_id != request.call_id || request.cseq > binding.cseq;
        match &request.contacts {
            Contacts::None => {}
            Contacts::Star => {
                if request.expires != Some(0) {
                    return Err(Refusal::BadRequest);
                }
                if !list.iter().all(ordered) {
                    return Err(Refusal::OutOfOrder);
                }
                outcome.removed.append(&mut list);
            }
            Contacts::List(contacts) => {
                let flow_timer = if request.flow.is_stream() {
                    self.limits.flow_timer_stream
                } else {
                    self.limits.flow_timer_udp
                };
                for contact in contacts {
                    let outbound = request.outbound && contact.instance.is_some() && contact.reg_id.is_some();
                    let expires = self.expiry(contact.expires, request.expires, request.behind_nat && !outbound)?;
                    let at = list.iter().position(|b| b.same_key(contact, outbound));
                    if let Some(at) = at {
                        if !ordered(&list[at]) {
                            return Err(Refusal::OutOfOrder);
                        }
                    }
                    match (at, expires) {
                        (Some(at), 0) => outcome.removed.push(list.remove(at)),
                        (None, 0) => {}
                        (Some(at), expires) => {
                            let binding = &mut list[at];
                            binding.contact = contact.uri.clone();
                            binding.flow = request.flow;
                            binding.call_id = request.call_id.clone();
                            binding.cseq = request.cseq;
                            binding.expires_at = now + u64::from(expires);
                            binding.refreshed_at = now;
                            binding.user_agent = request.user_agent.clone();
                            outcome.refreshed.push(binding.clone());
                            if outbound {
                                outcome.flow_timer = Some(flow_timer);
                            }
                        }
                        (None, expires) => {
                            let binding = Binding {
                                id: random_id(),
                                device: request.device.clone(),
                                username: request.username.clone(),
                                revision: request.revision,
                                contact: contact.uri.clone(),
                                flow: request.flow,
                                call_id: request.call_id.clone(),
                                cseq: request.cseq,
                                instance: contact.instance.clone().filter(|_| outbound),
                                reg_id: contact.reg_id.filter(|_| outbound),
                                outbound,
                                expires_at: now + u64::from(expires),
                                refreshed_at: now,
                                user_agent: request.user_agent.clone(),
                            };
                            outcome.added.push(binding.clone());
                            list.push(binding);
                            if outbound {
                                outcome.flow_timer = Some(flow_timer);
                            }
                        }
                    }
                }
                if list.len() > self.limits.max_bindings {
                    return Err(Refusal::TooMany);
                }
            }
        }
        outcome.current = list.clone();
        if list.is_empty() {
            self.bindings.remove(&request.username);
        } else {
            self.bindings.insert(request.username, list);
        }
        Ok(outcome)
    }

    /// The bindings a call to the device rings: the live ones of its current revision.
    pub fn live(&self, username: &str, now: u64) -> Vec<Binding> {
        self.bindings
            .get(username)
            .map(|list| list.iter().filter(|b| b.expires_at > now).cloned().collect())
            .unwrap_or_default()
    }

    /// Removes the bindings past their expiry, which are returned.
    pub fn expire(&mut self, now: u64) -> Vec<Binding> {
        let mut gone = Vec::new();
        self.bindings.retain(|_, list| {
            let (dead, live): (Vec<Binding>, Vec<Binding>) = list.drain(..).partition(|b| b.expires_at <= now);
            gone.extend(dead);
            *list = live;
            !list.is_empty()
        });
        gone
    }

    /// Ends every binding of a device: its credentials were rotated, or it was revoked.
    pub fn drop_device(&mut self, username: &str) -> Vec<Binding> {
        self.bindings.remove(username).unwrap_or_default()
    }

    /// Ends the bindings over a stream connection that closed (RFC 5626 §6: the flow failed). A UDP flow has no close.
    pub fn drop_flow(&mut self, transport: Transport, source: SocketAddr) -> Vec<Binding> {
        if transport == Transport::Udp {
            return Vec::new();
        }
        let mut gone = Vec::new();
        self.bindings.retain(|_, list| {
            let (dead, live): (Vec<Binding>, Vec<Binding>) = list
                .drain(..)
                .partition(|b| b.flow.transport == transport && b.flow.source == source);
            gone.extend(dead);
            *list = live;
            !list.is_empty()
        });
        gone
    }

    /// How many bindings there are, over every device.
    pub fn len(&self) -> usize {
        self.bindings.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }
}

/// The contacts of a REGISTER's Contact header values (each may hold several, comma separated).
pub fn parse_contacts(values: &[String]) -> Contacts {
    let mut list = Vec::new();
    for value in values {
        for item in split_list(value) {
            let item = item.trim();
            if item == "*" {
                return Contacts::Star;
            }
            if let Some(contact) = parse_contact(item) {
                list.push(contact);
            }
        }
    }
    if list.is_empty() {
        Contacts::None
    } else {
        Contacts::List(list)
    }
}

/// Splits on the commas outside quotes and angle brackets.
fn split_list(value: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut quoted, mut angle, mut start) = (false, false, 0);
    for (i, c) in value.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '<' if !quoted => angle = true,
            '>' if !quoted => angle = false,
            ',' if !quoted && !angle => {
                out.push(&value[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&value[start..]);
    out
}

fn parse_contact(item: &str) -> Option<ContactParam> {
    // `"name" <uri;uri-params>;header-params` or `uri;header-params`
    let (uri, params) = match item.find('<') {
        Some(open) => {
            let rest = item.get(open + 1..)?;
            let (uri, after) = rest.split_once('>')?;
            (uri.trim().to_string(), after)
        }
        None => match item.split_once(';') {
            Some((uri, params)) => (uri.trim().to_string(), params),
            None => (item.trim().to_string(), ""),
        },
    };
    if uri.is_empty() {
        return None;
    }
    let mut contact = ContactParam {
        uri,
        ..Default::default()
    };
    for param in params.split(';') {
        let (name, value) = param.split_once('=').unwrap_or((param, ""));
        let value = value.trim().trim_matches('"');
        match name.trim().to_ascii_lowercase().as_str() {
            "expires" => contact.expires = value.parse().ok(),
            "+sip.instance" => contact.instance = Some(value.to_string()).filter(|v| !v.is_empty()),
            "reg-id" => contact.reg_id = value.parse().ok(),
            _ => {}
        }
    }
    Some(contact)
}

/// A binding as the 200 to a REGISTER lists it (RFC 3261 §10.3 step 8).
pub fn contact_header(binding: &Binding, now: u64) -> String {
    let mut out = format!("<{}>;expires={}", binding.contact, binding.remaining(now));
    if let (Some(instance), Some(reg_id)) = (&binding.instance, binding.reg_id) {
        out.push_str(&format!(";+sip.instance=\"{instance}\";reg-id={reg_id}"));
    }
    out
}

#[cfg(test)]
#[path = "registrar_tests.rs"]
mod tests;
