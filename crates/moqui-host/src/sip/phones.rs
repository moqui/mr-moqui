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

//! The engine's side of the registrar for desk and soft phones (plans/Comms-Framework.md §5.2 "Registrar for phones
//! (V4)"; docs/interfaces/SIP.md "Registrar"). Off unless `sip_registrar_enabled` is true, even with the SIP listeners
//! on.
//!
//! - **Verifiers.** The user agent asks for a device's digest verifiers by username on each REGISTER or INVITE that
//!   answers a challenge (`org.moqui.impl.SipDeviceServices.get#SipDeviceAuth`, which decrypts them); the password is
//!   never stored.
//! - **Bindings** live in the user agent, with their flows; each change is kept as a `SipRegistration` row
//!   (`store#SipRegistration`), which the services read to know which users' devices ring here.
//! - **Calls to a device**: the services tell the engine to ring a device leg (`ringDevice`); it forks the INVITE to
//!   the device's live bindings, offering Opus, G.722 and G.711 (SRTP over TLS), and reports the answer as the leg's
//!   `answered`, latching its RTP to the flow that answered (behind a NAT, the SDP's private address is not where its
//!   packets come from).
//! - **Calls from a device** go to `receive#DeviceCall`, which places them as the device's user: to another user, or
//!   to a number through `place#PhoneCall`'s policy (caller ID, fraud limits, emergency).
//! - **Rotation and revocation**: `dropDevice` ends the device's bindings and hangs up its legs here.
//! - **On an edge node** (plans/Edge-Node.md §5.4 "Voice (SIP)", decision 4, E8) no verifier is here: each answer is
//!   checked on the local node (`check#SipDeviceDigest`), so phones sign in only while it is reachable.
//! - **On an all-in-one node** (plans/Edge-Node.md §10, E12) by the domain a phone registers to: the server's own realm
//!   (`sip_domain`) is its own phones', checked here; the paired node's domain (its Configure's `sipDomain`, which
//!   the user agent challenges in, `Ua::set_device_realms`) is the paired node's, checked there. A paired node's
//!   device ids are keyed by the node, and its bindings by `username@domain` (`moqui_sip::ua::aor_key`).

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::edge::owner::Owner;
use moqui_lib::sync::lock;
use moqui_sip::digest::Algorithm;
use moqui_sip::registrar::{Binding, RegistrarLimits};
use moqui_sip::sdp::{self, Direction, DEVICE_CODECS};
use moqui_sip::srtp::MasterKey;
use moqui_sip::ua::{
    BindingChange, Decision, DeviceDirectory, DeviceRing, IncomingCall, PhoneDevice, RegistrarConfig, Transport,
};
use serde_json::Value as Json;

use super::leg::{LegCommand, LegMedia};
use super::{call_service, number, owners, text, Engine, Leg, RING_TIMEOUT};

/// The paired node and its SIP device domain (E12), as the engine last read them, for the registrar's directory.
pub(super) type Paired = Arc<Mutex<Option<(String, String)>>>;

const AUTH_SERVICE: &str = "org.moqui.impl.SipDeviceServices.get#SipDeviceAuth";
/// On an edge node, the local node's check of a phone's digest answer (E8).
const CHECK_SERVICE: &str = "org.moqui.impl.SipDeviceServices.check#SipDeviceDigest";
const STORE_SERVICE: &str = "org.moqui.impl.SipDeviceServices.store#SipRegistration";
const RECEIVE_SERVICE: &str = "org.moqui.impl.SipDeviceServices.receive#DeviceCall";

/// The registrar's limits from the `sip_registrar_*` properties, or `None` while `sip_registrar_enabled` is not true.
/// A value that is not a number is logged and keeps its default.
pub(super) fn registrar_from(property: &dyn Fn(&str) -> String) -> Option<RegistrarLimits> {
    let enabled = property("sip_registrar_enabled");
    if !matches!(enabled.trim().to_ascii_lowercase().as_str(), "true" | "y" | "yes" | "1") {
        return None;
    }
    let read = |name: &str, default: u32| {
        let value = property(name);
        let value = value.trim();
        if value.is_empty() {
            return default;
        }
        value.parse().unwrap_or_else(|_| {
            tracing::error!("SIP: {name} {value} is not a number, its default is used");
            default
        })
    };
    let defaults = RegistrarLimits::default();
    let min_expires = read("sip_registrar_min_expires", defaults.min_expires).max(1);
    Some(RegistrarLimits {
        min_expires,
        max_expires: read("sip_registrar_max_expires", defaults.max_expires).max(min_expires),
        default_expires: read("sip_registrar_default_expires", defaults.default_expires).max(min_expires),
        nat_expires: read("sip_registrar_nat_expires", defaults.nat_expires).max(min_expires),
        ..defaults
    })
}

/// The user agent's registrar, with the voice services as its directory of devices. On a dedicated edge node every
/// phone is the paired node's; elsewhere those of the realm `realm` are the server's own.
pub(super) fn registrar_config(
    factory: &Arc<ExecutionContextFactory>,
    limits: RegistrarLimits,
    realm: &str,
    dedicated: bool,
    paired: Paired,
) -> RegistrarConfig {
    tracing::info!("SIP: the registrar for desk and soft phones is on, realm {realm}");
    RegistrarConfig {
        limits,
        directory: Arc::new(ServiceDirectory {
            factory: Arc::downgrade(factory),
            realm: realm.to_string(),
            dedicated,
            paired,
        }),
    }
}

/// A device's verifiers from `get#SipDeviceAuth`, which answers only an active device.
struct ServiceDirectory {
    factory: Weak<ExecutionContextFactory>,
    /// The registrar's own realm.
    realm: String,
    /// A dedicated edge node: every phone is the paired node's.
    dedicated: bool,
    paired: Paired,
}

impl ServiceDirectory {
    /// Whose phones register to `realm` (E12): the server's own in its realm, but on a dedicated edge node; the paired
    /// node's in its domain (and on a dedicated edge node in any); `None` for a realm of no one (no node paired).
    fn owner_of_realm(&self, realm: &str) -> Option<Owner> {
        if !self.dedicated && realm.eq_ignore_ascii_case(&self.realm) {
            return Some(Owner::Own);
        }
        let paired = lock(&self.paired).clone()?;
        (self.dedicated || paired.1.eq_ignore_ascii_case(realm)).then(|| Owner::paired(&paired.0))
    }
}

#[moqui_sip::async_trait]
impl DeviceDirectory for ServiceDirectory {
    async fn device(&self, realm: &str, username: &str) -> Option<PhoneDevice> {
        let factory = self.factory.upgrade()?;
        // the paired node's verifiers never leave it (E8): its phones are checked there, in verify
        if self.owner_of_realm(realm) != Some(Owner::Own) {
            return None;
        }
        let parameters = serde_json::json!({"username": username, "realm": realm});
        let out = match call_service(&factory, Owner::Own, AUTH_SERVICE, parameters).await {
            Ok(out) => out,
            Err(err) => {
                tracing::warn!("SIP: the device {username:?} was not looked up: {err}");
                return None;
            }
        };
        let id = text(&out, "sipDeviceId");
        if id.is_empty() {
            return None;
        }
        let verifiers = match out.get("verifierList") {
            Some(Json::Array(list)) => list
                .iter()
                .filter_map(|v| Some((Algorithm::parse(&text(v, "algorithm"))?, text(v, "ha1"))))
                .filter(|(_, ha1)| !ha1.is_empty())
                .collect(),
            _ => Vec::new(),
        };
        Some(PhoneDevice {
            id,
            username: text(&out, "username"),
            revision: number(&out, "credentialRevision")
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(1),
            verifiers,
        })
    }

    /// A paired node's phone (on an edge node, plans/Edge-Node.md §5.4 "Voice (SIP)", decision 4, E8; on an all-in-one
    /// node the phones of its domain, E12): the verifiers stay on the local node, the answer is checked there
    /// (`check#SipDeviceDigest`, delegated), and this registrar keeps the nonces. While the local node is away no
    /// phone signs in (503, not a failed attempt). The server's own as the default, against `get#SipDeviceAuth`'s
    /// verifiers.
    async fn verify(
        &self,
        realm: &str,
        username: &str,
        check: &moqui_sip::digest::ResponseCheck,
    ) -> Result<Option<(PhoneDevice, bool)>, String> {
        let Some(factory) = self.factory.upgrade() else {
            return Err("the server is stopping".to_string());
        };
        let mut owner = self.owner_of_realm(realm);
        if owner.is_none() && lock(&self.paired).is_none() {
            // paired since the engine last read it (it reads again every few seconds): read now, so a phone of a node
            // that has just paired is not refused until then
            let now = owners::paired_domain(&factory).await;
            if now.is_some() {
                *lock(&self.paired) = now;
                owner = self.owner_of_realm(realm);
            }
        }
        let owner = match owner {
            Some(owner) => owner,
            // a dedicated edge node's phones wait for its local node, as while it is away
            None if self.dedicated => return Err("no local node is paired".to_string()),
            None => return Ok(None),
        };
        if owner == Owner::Own {
            let Some(device) = self.device(realm, username).await else {
                return Ok(None);
            };
            let verifier = device
                .verifiers
                .iter()
                .find(|(algorithm, _)| *algorithm == check.algorithm)
                .map(|(_, ha1)| ha1.clone());
            let matched = check.matches(verifier.as_deref());
            return Ok(Some((device, matched)));
        }
        let (nc, cnonce) = check.qop_auth.clone().unzip();
        let parameters = serde_json::json!({"username": username, "realm": realm,
            "algorithm": check.algorithm.name(), "nonce": check.nonce, "nc": nc, "cnonce": cnonce,
            "qop": check.qop_auth.as_ref().map(|_| "auth"), "ha2": check.ha2, "response": check.response});
        // only the local node away is "cannot check now" (503, not counted); any other failure is a refusal
        let out = match call_service(&factory, owner, CHECK_SERVICE, parameters).await {
            Ok(out) => out,
            Err(err) if super::edge::is_offline(&err) => return Err(format!("the local node did not check it: {err}")),
            Err(err) => {
                tracing::warn!("SIP: the device {username:?}'s answer was refused by the local node: {err}");
                return Ok(None);
            }
        };
        let id = text(&out, "sipDeviceId");
        if !super::flag(&out, "matched") || id.is_empty() {
            return Ok(None);
        }
        let device = PhoneDevice {
            id,
            username: text(&out, "username"),
            revision: number(&out, "credentialRevision")
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(1),
            verifiers: Vec::new(),
        };
        Ok(Some((device, true)))
    }
}

/// The longest a phone rings for a call.
const MAX_DEVICE_RING: Duration = Duration::from_secs(300);

/// How long a `ringDevice` rings its phone: `ringSeconds`, the call's time left to ring (a voicemail box's rings can make
/// it longer than `call_ring_seconds`), at most [`MAX_DEVICE_RING`]; [`RING_TIMEOUT`] when it is missing.
pub(super) fn device_ring_timeout(body: &Json) -> Duration {
    number(body, "ringSeconds")
        .filter(|s| *s > 0)
        .map_or(RING_TIMEOUT, |s| Duration::from_secs(s).min(MAX_DEVICE_RING))
}

fn transport_code(transport: Transport) -> &'static str {
    match transport {
        Transport::Udp => "udp",
        Transport::Tcp => "tcp",
        Transport::Tls => "tls",
    }
}

impl Engine {
    /// The registrar's key of the bindings of the phone a control names (its `username`, `sipDeviceId`): on an
    /// all-in-one node a paired node's phone's are in that node's domain (E12, `moqui_sip::ua::aor_key`).
    fn device_aor(&self, body: &Json) -> String {
        let username = text(body, "username");
        let owner = owners::owner_in(body);
        if self.dedicated || owner == Owner::Own {
            return username;
        }
        let domain = lock(&self.paired).as_ref().map(|(_, domain)| domain.clone());
        moqui_sip::ua::aor_key(&username, domain.as_deref())
    }

    /// A binding added, refreshed or removed: kept as its `SipRegistration` row.
    pub(super) fn binding_changed(&self, change: BindingChange, binding: &Binding) {
        let change = match change {
            BindingChange::Added => "added",
            BindingChange::Refreshed => "refreshed",
            BindingChange::Removed => "removed",
        };
        self.note(
            STORE_SERVICE,
            serde_json::json!({
                "change": change, "sipRegistrationId": binding.id, "sipDeviceId": binding.device,
                "instanceId": self.own, "contactUri": binding.contact,
                "transport": transport_code(binding.flow.transport),
                "sourceAddress": binding.flow.source.ip().to_string(), "sourcePort": binding.flow.source.port(),
                "sipCallId": binding.call_id, "cseq": binding.cseq, "instanceKey": binding.instance,
                "regId": binding.reg_id, "outbound": binding.outbound, "credentialRevision": binding.revision,
                "expiresDate": binding.expires_at.saturating_mul(1000),
                "lastRefreshDate": binding.refreshed_at.saturating_mul(1000), "userAgent": binding.user_agent,
            }),
        );
    }

    /// `ringDevice`: a device leg of a call, forked to the device's live bindings.
    pub(super) fn ring_device(&mut self, body: &Json) {
        let room = text(body, "callSessionId");
        let leg = text(body, "callLegId");
        let started = self.local_media(false).and_then(|(socket, mut plain)| {
            plain.codecs = DEVICE_CODECS.to_vec();
            let mut srtp = plain.clone();
            srtp.key = Some(MasterKey::random());
            let media = LegMedia::start(&room, &leg, socket, None, self.notes_sender())?;
            Ok((media, plain, srtp))
        });
        let (media, plain, srtp) = match started {
            Ok(started) => started,
            Err(err) => {
                tracing::warn!("SIP device leg {leg}: its media did not start: {err}");
                self.note_leg(&room, &leg, "failed", Some(503), &err);
                return;
            }
        };
        let ring = DeviceRing {
            leg: leg.clone(),
            username: self.device_aor(body),
            from: text(body, "fromUser"),
            from_name: Some(text(body, "fromName")).filter(|n| !n.is_empty()),
            offer_plain: sdp::offer(&plain),
            offer_srtp: Some(sdp::offer(&srtp)),
            ring_timeout: device_ring_timeout(body),
        };
        self.legs.insert(
            leg.clone(),
            Leg {
                room: room.clone(),
                media,
                local: plain,
                caller: String::new(),
                dialed: String::new(),
                answered: false,
                device: Some(text(body, "sipDeviceId")),
                srtp_local: Some(srtp),
                sip_answered: false,
                held: false,
            },
        );
        if let Err(err) = self.ua.ring_device(ring) {
            // no live binding: it cannot ring
            self.legs.remove(&leg);
            self.note_leg(&room, &leg, "failed", Some(480), &err);
        }
    }

    /// A binding answered a device leg: the media settles with its answer, and plain RTP is taken from where it
    /// signals.
    pub(super) fn device_answered(&mut self, leg: &str, answer: &str, source: SocketAddr) {
        let Some(entry) = self.legs.get_mut(leg) else {
            // hung up here as the answer came
            self.ua.hangup(leg);
            return;
        };
        let parsed = sdp::parse(answer);
        if parsed.as_ref().is_ok_and(|a| a.secure) {
            if let Some(srtp) = entry.srtp_local.take() {
                entry.local = srtp;
            }
        }
        entry.srtp_local = None;
        let room = entry.room.clone();
        match parsed.and_then(|a| sdp::accept(&entry.local, &a)) {
            Ok(settled) => {
                entry.media.send(LegCommand::Settled(Box::new(settled)));
                entry.media.send(LegCommand::AllowSource(source.ip()));
                self.note_leg(&room, leg, "answered", Some(200), "");
            }
            Err(err) => {
                tracing::warn!("SIP device leg {leg}: the answer did not settle: {err}");
                self.ua.hangup(leg);
                self.legs.remove(leg);
                self.note_leg(&room, leg, "failed", Some(488), &err.to_string());
            }
        }
    }

    /// `dropDevice`: a device rotated or revoked; its bindings end and its legs here hang up.
    pub(super) fn drop_device(&mut self, body: &Json) {
        let username = text(body, "username");
        let device = text(body, "sipDeviceId");
        let ended = self.ua.drop_device(&self.device_aor(body));
        let legs: Vec<String> = self
            .legs
            .iter()
            .filter(|(_, l)| !device.is_empty() && l.device.as_deref() == Some(device.as_str()))
            .map(|(id, _)| id.clone())
            .collect();
        for leg in &legs {
            self.legs.remove(leg);
            self.ua.hangup(leg);
        }
        tracing::info!("SIP: device {username} dropped: {ended} bindings, {} legs", legs.len());
    }

    /// A registered phone's own call: the services place it as the device's user; it is answered when the far end is.
    pub(super) async fn incoming_device(&mut self, call: IncomingCall) {
        let device = call.device.clone().unwrap_or_default();
        let parameters = serde_json::json!({"sipDeviceId": device, "toUser": call.to, "instanceId": self.own,
            "sipCallId": call.call_id, "sourceAddress": call.source.ip().to_string()});
        // E12: the phone's owner, by its id (the directory keyed a paired node's)
        let owner = owners::owner_of(&device);
        let decided = match call_service(&self.factory, owner, RECEIVE_SERVICE, parameters).await {
            Ok(out) => {
                let (room, leg) = (text(&out, "callSessionId"), text(&out, "callLegId"));
                if room.is_empty() || leg.is_empty() {
                    Err(404)
                } else {
                    Ok((room, leg))
                }
            }
            Err(err) => {
                tracing::info!("SIP call from device {device} to {} not placed: {err}", call.to);
                Err(match err.split(':').next().unwrap_or_default() {
                    "notFound" | "badNumber" | "noCallee" => 404,
                    "busy" | "tooManyCalls" | "dailyLimit" => 486,
                    "voiceUnavailable" | "mediaUnavailable" => 480,
                    _ => 403,
                })
            }
        };
        let (room, leg) = match decided {
            Ok(ids) => ids,
            Err(code) => {
                let _ = call.reply.send(Decision::Reject(code));
                return;
            }
        };
        let offer = match sdp::parse(&call.offer) {
            Ok(offer) => offer,
            Err(err) => {
                let _ = call.reply.send(Decision::Reject(488));
                self.note_leg(&room, &leg, "failed", Some(488), &err.to_string());
                return;
            }
        };
        let started = self.local_media(offer.secure).and_then(|(socket, mut local)| {
            local.codecs = DEVICE_CODECS.to_vec();
            local.direction = Direction::SendRecv;
            let (answer, settled) = sdp::answer(&offer, &local).map_err(|e| e.to_string())?;
            let media = LegMedia::start(&room, &leg, socket, Some(settled), self.notes_sender())?;
            media.send(LegCommand::AllowSource(call.source.ip()));
            Ok((answer, media, local))
        });
        match started {
            Ok((answer, media, local)) => {
                let ring = Decision::Ring {
                    leg: leg.clone(),
                    answer,
                };
                if call.reply.send(ring).is_err() {
                    tracing::warn!("SIP device call {leg}: decided too late, the call was refused");
                    drop(media);
                    self.note_leg(&room, &leg, "failed", Some(480), "decided too late");
                    // E8: nothing more goes out for it, a number it dialed included
                    self.abandon_room(&room);
                    return;
                }
                self.legs.insert(
                    leg,
                    Leg {
                        room,
                        media,
                        local,
                        caller: String::new(),
                        dialed: String::new(),
                        answered: true,
                        device: Some(device),
                        srtp_local: None,
                        sip_answered: false,
                        held: false,
                    },
                );
            }
            Err(err) => {
                tracing::warn!("SIP device call {leg}: its media did not start: {err}");
                let _ = call.reply.send(Decision::Reject(503));
                self.note_leg(&room, &leg, "failed", Some(503), &err);
            }
        }
    }
}
