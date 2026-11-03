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

//! The user agent's registrar for desk and soft phones (plans/Comms-Framework.md §5.2 "Registrar for phones (V4)").
//!
//! On only when the host gives [`UaConfig::registrar`] (`sip_registrar_enabled`); without it a REGISTER is refused
//! (403) without a challenge. With it:
//!
//! - **REGISTER** is challenged ([`crate::digest::NonceBook`]: SHA-256 first, then MD5, RFC 8760 §2.4), and the
//!   answer is checked against the device's stored verifiers ([`DeviceDirectory`]): an HA1 per algorithm for this
//!   realm, never a password. A device of the standard profile has a SHA-256 verifier only, so an MD5 answer fails for
//!   it; a legacy one has an MD5 verifier too. A failed answer counts against its source address, never the account
//!   ([`crate::guard`]), and a nonce count that does not go up is a replay. The bindings are [`Registrar`]'s; each
//!   change goes to the host as [`UaEvent::Binding`], which keeps them as `SipRegistration` rows.
//! - **NAT.** The binding records the flow the REGISTER came over: its transport, and its source address and port
//!   (`received` and `rport`, RFC 3581). A call to the phone goes back over that flow: the same TCP or TLS
//!   connection, or the UDP source address from the listener's own socket, so the NAT's mapping takes it. A phone
//!   with RFC 5626 Outbound is told to send CRLF keepalives (`Flow-Timer`); one without is given a short expiry
//!   behind a NAT. A stream connection that closes ends the bindings over it.
//! - **Calls to a device** ([`Ua::ring_device`]) fork an INVITE to each live binding. The first to answer wins: the
//!   others are canceled, and an answer that crosses is ACKed and ended with BYE. The host hears of the winner as
//!   [`UaEvent::DeviceAnswered`], with the address it signals from (for RTP latching behind a NAT).
//! - **Calls from a device** are INVITEs with a digest answer for the device (`Proxy-Authorization`), as a PBX trunk's
//!   are; the host decides them as an [`IncomingCall`] with its `device` set.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moqui_lib::sync::lock;
use rsipstack::dialog::dialog::DialogState;
use rsipstack::dialog::invitation::InviteOption;
use rsipstack::sip::prelude::{HeadersExt, ToTypedHeader};
use rsipstack::sip::{Header, StatusCode, Uri};
use rsipstack::transaction::random_text;
use rsipstack::transaction::transaction::Transaction;
use rsipstack::transport::{SipAddr, SipConnection};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::{display_name, header_values, now_secs, sip_err, status_code, Inner, Leg, LegKind, Transport, Ua, UaEvent};
use crate::digest::Algorithm;
use crate::registrar::{self, Binding, Flow, Register, Registrar, RegistrarLimits};

/// How often expired bindings are swept.
const SWEEP_EVERY: Duration = Duration::from_secs(5);

/// A device as the voice services give it (`SipDeviceServices.get#SipDeviceAuth`): only an active one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneDevice {
    /// Its `sipDeviceId`.
    pub id: String,
    pub username: String,
    pub revision: u32,
    /// Its digest verifiers for this registrar's realm: the HA1 of each algorithm it may answer with.
    pub verifiers: Vec<(Algorithm, String)>,
}

/// Where the user agent finds a device's verifiers: the host asks the voice services. `realm` is the registrar's realm
/// the request is in: the configured one, or a device domain of [`Ua::set_device_realms`].
#[async_trait::async_trait]
pub trait DeviceDirectory: Send + Sync {
    /// The active device of the realm with this username, or `None`.
    async fn device(&self, realm: &str, username: &str) -> Option<PhoneDevice>;

    /// The active device with this username and whether `check`'s response is right for its verifier of the check's
    /// algorithm; `None` when there is no such device. By default against [`DeviceDirectory::device`]'s verifiers; a
    /// directory that holds none (an edge node's registrar, plans/Edge-Node.md §5.4 "Voice (SIP)") has the node that
    /// holds them check it, and answers a device with no verifiers, or an error only when that node could not be
    /// asked (it is away), which is no failed attempt of the phone's: answered 503. Any other failure is `Ok(None)`.
    async fn verify(
        &self,
        realm: &str,
        username: &str,
        check: &crate::digest::ResponseCheck,
    ) -> Result<Option<(PhoneDevice, bool)>, String> {
        let Some(device) = self.device(realm, username).await else {
            return Ok(None);
        };
        let verifier = device
            .verifiers
            .iter()
            .find(|(algorithm, _)| *algorithm == check.algorithm)
            .map(|(_, ha1)| ha1.clone());
        let matched = check.matches(verifier.as_deref());
        Ok(Some((device, matched)))
    }
}

/// The registrar's switch and setup: `None` in [`UaConfig::registrar`](super::UaConfig::registrar) keeps it off.
#[derive(Clone)]
pub struct RegistrarConfig {
    pub limits: RegistrarLimits,
    pub directory: Arc<dyn DeviceDirectory>,
}

/// What happened to a binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingChange {
    Added,
    Refreshed,
    Removed,
}

/// A call to a user's device, forked to its live bindings.
#[derive(Debug, Clone)]
pub struct DeviceRing {
    /// The device leg's id (its `callLegId`).
    pub leg: String,
    /// The device's username.
    pub username: String,
    /// The user part of From: the caller's number or name, as the phone shows it.
    pub from: String,
    pub from_name: Option<String>,
    /// The offer for a binding over UDP or TCP (plain RTP), and for one over TLS (SRTP with SDES keys; plain RTP when
    /// `None`).
    pub offer_plain: String,
    pub offer_srtp: Option<String>,
    pub ring_timeout: Duration,
}

/// Who signed a request in. A device with the device domain it signed in to, `None` for the configured realm.
pub(super) enum Caller {
    Trunk(super::TrunkConfig),
    Device(PhoneDevice, Option<String>),
}

/// The registrar's key of a device's bindings (its address of record): the username in the configured realm, else
/// `username@realm`, so two device domains' phones with one username are kept apart. [`Ua::ring_device`],
/// [`Ua::drop_device`] and [`Ua::bindings`] take it.
pub fn aor_key(username: &str, realm: Option<&str>) -> String {
    match realm.filter(|realm| !realm.is_empty()) {
        Some(realm) => format!("{username}@{}", realm.to_ascii_lowercase()),
        None => username.to_string(),
    }
}

/// The registrar's state in the user agent.
pub(super) struct Phones {
    pub(super) registrar: Arc<Mutex<Registrar>>,
    pub(super) directory: Arc<dyn DeviceDirectory>,
}

impl Phones {
    pub(super) fn new(config: &RegistrarConfig) -> Phones {
        Phones {
            registrar: Arc::new(Mutex::new(Registrar::new(config.limits))),
            directory: Arc::clone(&config.directory),
        }
    }
}

/// The transport of a signaling address.
pub(super) fn transport_of(transport: rsipstack::sip::Transport) -> Option<Transport> {
    match transport {
        rsipstack::sip::Transport::Udp => Some(Transport::Udp),
        rsipstack::sip::Transport::Tcp => Some(Transport::Tcp),
        rsipstack::sip::Transport::Tls => Some(Transport::Tls),
        _ => None,
    }
}

/// The bindings over a stream connection that closed end (RFC 5626 §6).
pub(super) fn connection_closed(
    registrar: &Mutex<Registrar>,
    events: &tokio::sync::mpsc::UnboundedSender<UaEvent>,
    connection: &SipConnection,
) {
    let Some(remote) = connection.get_remote_addr() else {
        return;
    };
    let (Some(transport), Ok(source)) = (remote.r#type.and_then(transport_of), remote.get_socketaddr()) else {
        return;
    };
    for binding in lock(registrar).drop_flow(transport, source) {
        let _ = events.send(UaEvent::Binding {
            change: BindingChange::Removed,
            binding,
        });
    }
}

/// The sweep of expired bindings, every few seconds until the user agent stops.
pub(super) fn sweep(inner: &Arc<Inner>) {
    let Some(phones) = inner.phones.as_ref() else { return };
    let registrar = Arc::clone(&phones.registrar);
    let events = inner.events.clone();
    let stop = inner.stop.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(SWEEP_EVERY) => {}
                _ = stop.cancelled() => break,
            }
            let gone = lock(&registrar).expire(now_secs());
            for binding in gone {
                let _ = events.send(UaEvent::Binding {
                    change: BindingChange::Removed,
                    binding,
                });
            }
        }
    });
}

/// The transport a request came over: its connection's (a TCP or TLS stream, else the UDP listener), never what its Via
/// claims, so a phone cannot have its calls sent over another transport than the flow it opened.
pub(super) fn transport_in(tx: &Transaction) -> Transport {
    match tx.connection.as_ref() {
        Some(connection) if connection.is_reliable() => connection
            .get_remote_addr()
            .and_then(|remote| remote.r#type)
            .and_then(transport_of)
            .unwrap_or(Transport::Tcp),
        _ => Transport::Udp,
    }
}

/// A request's flow and whether its Via's sent-by is not where it came from (a NAT between).
fn flow_of(tx: &Transaction, source: SocketAddr) -> Option<(Flow, bool)> {
    let via = tx.original.via_header().ok()?;
    let typed = via.first_value().ok()?.typed().ok()?;
    let transport = transport_in(tx);
    let sent_by = typed.uri.host_with_port;
    let sent_ip: Option<std::net::IpAddr> = sent_by.host.to_string().trim_matches(['[', ']']).parse().ok();
    let sent_port: Option<u16> = sent_by.port.map(|p| p.value());
    let behind_nat = match transport {
        // a stream's source port is the phone's ephemeral one: only its address tells
        Transport::Tcp | Transport::Tls => sent_ip != Some(source.ip()),
        Transport::Udp => sent_ip != Some(source.ip()) || sent_port.unwrap_or(5060) != source.port(),
    };
    Some((Flow { transport, source }, behind_nat))
}

fn cseq_of(tx: &Transaction) -> u32 {
    tx.original
        .cseq_header()
        .ok()
        .and_then(|h| h.value().split_whitespace().next().and_then(|n| n.parse().ok()))
        .unwrap_or(0)
}

/// A user part safe in a SIP URI: what a phone shows as the caller.
fn uri_user(text: &str) -> String {
    let user: String = text
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || "+-_.!~*'()".contains(*c))
        .take(64)
        .collect();
    if user.is_empty() {
        "anonymous".to_string()
    } else {
        user
    }
}

impl Ua {
    /// Rings a device: an INVITE to each of its live bindings, over the flow each came in on. Its outcome comes as
    /// [`UaEvent::DeviceAnswered`] or [`UaEvent::Ended`] for `ring.leg`; how many bindings ring. An error when the
    /// registrar is off or the device has no live binding (it cannot ring).
    pub fn ring_device(&self, ring: DeviceRing) -> Result<usize, String> {
        let phones = self.inner.phones.as_ref().ok_or("the registrar is off")?;
        let bindings = lock(&phones.registrar).live(&ring.username, now_secs());
        if bindings.is_empty() {
            return Err("noBinding".to_string());
        }
        let (give_up, gave_up) = oneshot::channel();
        lock(&self.inner.legs).insert(
            ring.leg.clone(),
            Leg {
                kind: LegKind::Outbound,
                dialog: None,
                give_up: Some(give_up),
                answer: None,
                answered: false,
            },
        );
        let count = bindings.len();
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move { inner.fork(ring, bindings, gave_up).await });
        Ok(count)
    }

    /// Ends every binding of a device (its credentials were rotated, or it was revoked): it cannot ring until it
    /// registers again with what it now has. How many ended.
    pub fn drop_device(&self, username: &str) -> usize {
        let Some(phones) = self.inner.phones.as_ref() else {
            return 0;
        };
        let gone = lock(&phones.registrar).drop_device(username);
        let count = gone.len();
        for binding in gone {
            self.inner.emit(UaEvent::Binding {
                change: BindingChange::Removed,
                binding,
            });
        }
        count
    }

    /// The live bindings of a device.
    pub fn bindings(&self, username: &str) -> Vec<Binding> {
        match self.inner.phones.as_ref() {
            Some(phones) => lock(&phones.registrar).live(username, now_secs()),
            None => Vec::new(),
        }
    }

    pub fn set_registrar_limits(&self, limits: RegistrarLimits) {
        if let Some(phones) = self.inner.phones.as_ref() {
            lock(&phones.registrar).set_limits(limits);
        }
    }
}

/// How one forked INVITE ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForkEnd {
    Answered,
    Refused(u16),
    Canceled,
    Failed,
}

impl Inner {
    /// A REGISTER, with the registrar on: signed in by a device, then applied to its bindings.
    pub(super) async fn register_phone(self: &Arc<Self>, mut tx: Transaction, source: Option<SocketAddr>) {
        let Some(phones) = self.phones.as_ref() else { return };
        let Some(Caller::Device(device, realm)) = self.authenticate(&mut tx, source, false).await else {
            return;
        };
        let Some(source) = source else {
            let _ = tx.reply(StatusCode::BadRequest).await;
            return;
        };
        let Some((flow, behind_nat)) = flow_of(&tx, source) else {
            let _ = tx.reply(StatusCode::BadRequest).await;
            return;
        };
        let headers = &tx.original.headers;
        let outbound = header_values(headers, "Supported")
            .iter()
            .chain(header_values(headers, "Require").iter())
            .any(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case("outbound")));
        let request = Register {
            device: device.id.clone(),
            username: aor_key(&device.username, realm.as_deref()),
            revision: device.revision,
            contacts: registrar::parse_contacts(&header_values(headers, "Contact")),
            expires: header_values(headers, "Expires")
                .first()
                .and_then(|v| v.trim().parse().ok()),
            call_id: tx
                .original
                .call_id_header()
                .map(|h| h.value().to_string())
                .unwrap_or_default(),
            cseq: cseq_of(&tx),
            flow,
            outbound,
            behind_nat,
            user_agent: header_values(headers, "User-Agent")
                .first()
                .map(|v| v.chars().filter(|c| !c.is_control()).take(200).collect()),
        };
        let now = now_secs();
        let outcome = lock(&phones.registrar).register(request, now);
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(refusal) => {
                let mut extra = Vec::new();
                if let registrar::Refusal::TooBrief(min) = refusal {
                    extra.push(Header::Other("Min-Expires".to_string(), min.to_string()));
                }
                tracing::debug!("SIP: a REGISTER of {} refused: {refusal:?}", device.username);
                let _ = tx.reply_with(status_code(refusal.status()), extra, None).await;
                return;
            }
        };
        let mut reply: Vec<Header> = outcome
            .current
            .iter()
            .map(|b| Header::Contact(registrar::contact_header(b, now).into()))
            .collect();
        if let Some(timer) = outcome.flow_timer {
            reply.push(Header::Other("Require".to_string(), "outbound".to_string()));
            reply.push(Header::Other("Flow-Timer".to_string(), timer.to_string()));
        }
        reply.push(Header::Other("Supported".to_string(), "outbound".to_string()));
        let _ = tx.reply_with(StatusCode::OK, reply, None).await;
        let changes = [
            (BindingChange::Removed, outcome.removed),
            (BindingChange::Added, outcome.added),
            (BindingChange::Refreshed, outcome.refreshed),
        ];
        for (change, list) in changes {
            for binding in list {
                self.emit(UaEvent::Binding { change, binding });
            }
        }
    }

    /// The device domain a request is in, besides the configured realm: its answer's realm, else the domain it is for
    /// (a REGISTER's To, an INVITE's From); `None` for the configured realm, or with the registrar off.
    pub(super) fn realm_of(&self, tx: &Transaction, given: Option<&crate::digest::Authorization>) -> Option<String> {
        self.phones.as_ref()?;
        let books = lock(&self.device_realms);
        if books.is_empty() {
            return None;
        }
        let host = |uri: &Uri| uri.host_with_port.host.to_string();
        let name = match given {
            Some(auth) => auth.realm.clone(),
            None if tx.original.method == rsipstack::sip::Method::Register => {
                host(&tx.original.to_header().ok()?.typed().ok()?.uri)
            }
            None => host(&tx.original.from_header().ok()?.typed().ok()?.uri),
        };
        let name = name.trim().trim_end_matches('.').to_ascii_lowercase();
        books.contains_key(&name).then_some(name)
    }

    /// The realm of a binding's key ([`aor_key`]): its device domain, else the configured realm.
    fn realm_of_key(&self, key: &str) -> String {
        if let Some((_, realm)) = key.rsplit_once('@') {
            if lock(&self.device_realms).contains_key(realm) {
                return realm.to_string();
            }
        }
        lock(&self.nonces).realm().to_string()
    }

    /// A device's verifier check: the answer's username and algorithm against the device's verifiers, in `realm` (a
    /// device domain) or the configured realm.
    pub(super) async fn check_device(
        &self,
        auth: &crate::digest::Authorization,
        method: &str,
        realm: Option<&str>,
    ) -> (crate::digest::Verdict, Option<PhoneDevice>) {
        // a forged or foreign nonce fails whoever it names: the directory (a service call) is not asked for it
        let (ours, realm_name) = match realm {
            Some(realm) => (
                lock(&self.device_realms)
                    .get(realm)
                    .is_some_and(|book| book.answers_ours(auth)),
                realm.to_string(),
            ),
            None => {
                let book = lock(&self.nonces);
                (book.answers_ours(auth), book.realm().to_string())
            }
        };
        let check = crate::digest::ResponseCheck::of(auth, method);
        let found = match (self.phones.as_ref(), &check) {
            (Some(phones), Some(check)) if ours => {
                match phones.directory.verify(&realm_name, &auth.username, check).await {
                    Ok(found) => found.filter(|(d, _)| d.username == auth.username),
                    Err(err) => {
                        tracing::warn!("SIP: {:?}'s answer was not checked: {err}", auth.username);
                        return (crate::digest::Verdict::Unavailable, None);
                    }
                }
            }
            _ => None,
        };
        if let (None, Some(check)) = (&found, &check) {
            // checked against a stand-in, so the time does not tell whether the username exists
            let _ = check.matches(None);
        }
        let matched = found.as_ref().is_some_and(|(_, matched)| *matched);
        let verdict = match realm {
            Some(realm) => match lock(&self.device_realms).get_mut(realm) {
                Some(book) => book.verify_checked(auth, matched, now_secs()),
                None => crate::digest::Verdict::Invalid,
            },
            None => lock(&self.nonces).verify_checked(auth, matched, now_secs()),
        };
        (verdict, found.map(|(device, _)| device))
    }

    /// The INVITEs to a device's bindings, the first answer winning.
    async fn fork(self: Arc<Self>, ring: DeviceRing, bindings: Vec<Binding>, mut gave_up: oneshot::Receiver<()>) {
        let leg = ring.leg.clone();
        let cancel = CancellationToken::new();
        let won = Arc::new(Mutex::new(false));
        let ringing = Arc::new(AtomicBool::new(false));
        let mut forks = tokio::task::JoinSet::new();
        for binding in bindings {
            let inner = Arc::clone(&self);
            let (ring, cancel, won, ringing) = (ring.clone(), cancel.clone(), Arc::clone(&won), Arc::clone(&ringing));
            forks.spawn(async move { inner.fork_one(&ring, &binding, &cancel, &won, &ringing).await });
        }
        let deadline = tokio::time::sleep(ring.ring_timeout);
        tokio::pin!(deadline);
        let (mut given_up, mut timed_out) = (false, false);
        let mut ends = Vec::new();
        loop {
            tokio::select! {
                joined = forks.join_next() => match joined {
                    Some(Ok(end)) => ends.push(end),
                    Some(Err(_)) => ends.push(ForkEnd::Failed),
                    None => break,
                },
                _ = &mut gave_up, if !given_up => {
                    given_up = true;
                    cancel.cancel();
                }
                _ = &mut deadline, if !timed_out && !given_up => {
                    timed_out = true;
                    cancel.cancel();
                }
            }
        }
        if ends.contains(&ForkEnd::Answered) {
            // the winner's dialog watcher reports its end
            return;
        }
        let reason = if given_up {
            super::EndReason::Canceled
        } else if timed_out {
            super::EndReason::TimedOut
        } else {
            let refused: Vec<u16> = ends
                .iter()
                .filter_map(|e| match e {
                    ForkEnd::Refused(code) => Some(*code),
                    _ => None,
                })
                .collect();
            // a decline (603, or 600 busy everywhere) from any binding is the person's; 486 from every binding is only this
            // phone being busy, which ends this leg and leaves the person's other devices ringing (V3)
            if refused.iter().any(|c| matches!(c, 600 | 603)) {
                super::EndReason::Declined
            } else if !refused.is_empty() && refused.iter().all(|c| *c == 486) {
                super::EndReason::Busy
            } else {
                super::EndReason::Failed(refused.first().copied())
            }
        };
        lock(&self.legs).remove(&leg);
        self.emit(UaEvent::Ended { leg, reason });
    }

    /// One INVITE of a fork, to one binding over its flow.
    async fn fork_one(
        self: &Arc<Self>,
        ring: &DeviceRing,
        binding: &Binding,
        cancel: &CancellationToken,
        won: &Mutex<bool>,
        ringing: &AtomicBool,
    ) -> ForkEnd {
        let transport = binding.flow.transport;
        let offer = match (transport, &ring.offer_srtp) {
            (Transport::Tls, Some(srtp)) => srtp.clone(),
            _ => ring.offer_plain.clone(),
        };
        let realm = self.realm_of_key(&binding.username);
        let from = uri_user(&ring.from);
        let built = (|| -> Result<InviteOption, String> {
            Ok(InviteOption {
                caller_display_name: ring.from_name.as_deref().map(display_name).filter(|n| !n.is_empty()),
                caller: Uri::try_from(format!("sip:{from}@{realm}").as_str()).map_err(sip_err)?,
                callee: Uri::try_from(binding.contact.as_str()).map_err(sip_err)?,
                destination: Some(SipAddr {
                    r#type: Some(transport.sip()),
                    addr: binding.flow.source.into(),
                }),
                content_type: Some("application/sdp".to_string()),
                offer: Some(offer.into_bytes()),
                contact: Uri::try_from(self.contact(transport, &from).as_str()).map_err(sip_err)?,
                call_id: Some(format!("{}@moqui", random_text(20))),
                ..Default::default()
            })
        })();
        let option = match built {
            Ok(option) => option,
            Err(err) => {
                tracing::warn!("SIP: device {} binding {}: {err}", binding.username, binding.contact);
                return ForkEnd::Failed;
            }
        };
        let (states_tx, mut states) = self.dialogs.new_dialog_state_channel();
        let (dialog, mut handle) = match self.dialogs.do_invite_async(option, states_tx) {
            Ok(started) => started,
            Err(err) => {
                tracing::debug!("SIP: device {} binding {}: {err}", binding.username, binding.contact);
                return ForkEnd::Failed;
            }
        };
        let mut canceled = false;
        let result = loop {
            tokio::select! {
                result = &mut handle => break result,
                _ = cancel.cancelled(), if !canceled => {
                    canceled = true;
                    let _ = dialog.cancel().await;
                }
                state = states.recv() => {
                    if let Some(DialogState::Early(_, resp)) = state {
                        if u16::from(resp.status_code.clone()) >= 180 && !ringing.swap(true, Ordering::AcqRel) {
                            self.emit(UaEvent::Ringing { leg: ring.leg.clone() });
                        }
                    }
                }
            }
        };
        let response = match result {
            Ok(Ok((_, response))) => response,
            _ => return ForkEnd::Failed,
        };
        let Some(resp) = response else {
            return ForkEnd::Canceled;
        };
        let code = u16::from(resp.status_code.clone());
        if !(200..300).contains(&code) {
            return if canceled {
                ForkEnd::Canceled
            } else {
                ForkEnd::Refused(code)
            };
        }
        // the first answer wins; one that crosses the others' CANCEL is ended
        let first = {
            let mut won = lock(won);
            let mut legs = lock(&self.legs);
            match legs.get_mut(&ring.leg) {
                Some(entry) if !*won && !canceled => {
                    *won = true;
                    entry.dialog = Some(dialog.clone());
                    entry.answered = true;
                    entry.give_up = None;
                    true
                }
                _ => false,
            }
        };
        if !first {
            if let Err(err) = dialog.bye().await {
                tracing::debug!(
                    "SIP: device {} binding {}: BYE failed: {err}",
                    binding.username,
                    binding.contact
                );
            }
            return ForkEnd::Canceled;
        }
        cancel.cancel();
        self.emit(UaEvent::DeviceAnswered {
            leg: ring.leg.clone(),
            answer: String::from_utf8_lossy(&resp.body).into_owned(),
            source: binding.flow.source,
        });
        let inner = Arc::clone(self);
        let leg = ring.leg.clone();
        tokio::spawn(async move { inner.watch(leg, states, None).await });
        ForkEnd::Answered
    }
}

#[cfg(test)]
#[path = "ua_phones_tests.rs"]
mod tests;
