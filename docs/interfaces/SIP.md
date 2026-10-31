# SIP trunks and phone calls

A user calls a phone number from BasicComms, and a number's calls ring its user, through a SIP trunk provider. The
browser never speaks SIP: it talks WebRTC to Moqui ([WebRTC](WebRTC.md)), and Moqui places the SIP leg toward the
trunk. The phone is one more leg of the call's `CallSession`. Its audio joins the call's room on the media node: the
browsers hear it as a participant, and the phone hears everyone else mixed (mix-minus). Desk and soft phones register
with Moqui's own registrar ([Registrar](#registrar), V4): a call to a user rings them beside the user's tabs, and they
call out as their user.

- The SIP user agent and B2BUA are in `crates/moqui-sip`, on rsipstack; the registrar's bindings are its
  `registrar.rs`, and its side of the user agent `ua_phones.rs`.
- The engine, the listeners and the RTP sockets are in `crates/moqui-host/src/sip/`.
- The voice services are `org.moqui.impl.VoiceServices` (`framework/service/org/moqui/impl/Voice.js`), with
  `framework/entity/VoiceEntities.xml`; the phones' are `org.moqui.impl.SipDeviceServices` (`SipDevices.js`).
- Administration is System, Voice (in basic-comms).
- The design is [plans/Comms-Framework.md](../../plans/Comms-Framework.md) §5.2, and "The V1 gate" in §6.2.

The engine needs:

- the `sip` feature, in the Maximal, Enterprise and Debug packages (for a Dev build,
  `./moq build -- --features sip`);
- call media on the same server (`rtc_media_port`, [WebRTC](WebRTC.md)).

Every listener is off until its setting is set.

## Settings

| Property | Default | |
| --- | --- | --- |
| `sip_port` | empty (off) | SIP over UDP and TCP; 5060 by convention |
| `sip_host` | `127.0.0.1` | The address `sip_port` binds |
| `sip_tls_port` | empty (off) | SIP over TLS; 5061 by convention. The certificate comes from `runtime/cert` (for `sip_public_address`, else `default`), as the HTTPS listener's and TURN's do |
| `sip_tls_host` | `sip_host` | The address `sip_tls_port` binds |
| `sip_rtp_ports` | `20000-20999` | The UDP range of the phone legs' RTP, one port a leg |
| `sip_rtp_host` | `sip_host` | The address the RTP sockets bind |
| `sip_public_address` | the bound address | The address in Contact headers and SDP, for a server behind 1:1 NAT. A host bound to every address does not start without it |
| `sip_tls_ca_file` | the system bundle (`/etc/ssl/certs/ca-certificates.crt`) | The CA certificates that verify a trunk's TLS certificate |
| `sip_request_rate`, `sip_request_burst` | `20`, `100` | New requests (outside a dialog: INVITE, REGISTER, OPTIONS and the rest) a second from one source, and how many it may send at once; past them its requests are dropped unanswered. `0` turns it off |
| `sip_max_transactions` | `2000` | New requests are dropped while the engine runs this many transactions; calls going on are not cut. `0` turns it off |
| `sip_max_connections` | `500` | TCP and TLS connections open at once; one past it is closed. `0` turns it off |
| `sip_registrar_enabled` | `false` | The registrar for desk and soft phones ([Registrar](#registrar)); off even with the listeners on |
| `sip_domain` | `sip_public_address`, else `moqui` | The realm of the digest challenges and the domain phones register to. The phones' verifiers are for it: after a change, each phone needs a new password |
| `sip_registrar_min_expires`, `sip_registrar_max_expires` | `60`, `3600` | A phone binding's expiry is held between these, in seconds; under the minimum is refused (423) |
| `sip_registrar_default_expires` | `600` | The expiry of a binding whose phone asks none |
| `sip_registrar_nat_expires` | `60` | The longest expiry of a phone behind a NAT that does not use Outbound, so its REGISTER keeps the NAT's mapping open |

Trunks, numbers, caller-ID grants, fraud limits and emergency locations are records, set up in System, Voice:
`VoiceTrunk`, `PhoneNumber`, `PhoneNumberUser`, `VoicePolicy`, `VoiceUserSetting` and `EmergencyLocation`.

- A trunk's credentials (`VoiceTrunk.authJson`) are an encrypted field. The services that list trunks never return
  them; only the engine's `get#EngineConfig` decrypts them.
- After a trunk or the policy changes, each running engine reloads them.

Open these ports on the firewall, to the provider's signaling and media addresses only where it publishes them:

- `sip_port`, UDP and TCP;
- `sip_tls_port`, TCP;
- the `sip_rtp_ports` range, UDP.

**DNS.** Desk and soft phones ([Registrar](#registrar)) find a server through its SRV record:

```
_sips._tcp.example.com. 3600 IN SRV 10 10 5061 sip.example.com.
```

The certificate in `runtime/cert` must name `sip.example.com`. Trunks are set up with the provider's host, so they
need no record of Moqui's.

## Trunks

A trunk is set up with these:

- the provider's host and port, and its transport;
- the media encryption;
- the digest credentials: username, an authentication username when the provider's differs, the password, the
  realm, and the algorithms allowed;
- whether to register;
- the provider's signaling addresses calls come from (addresses or ranges, none wider than a /8, or a /32 for IPv6:
  `0.0.0.0/0` and `::/0` are refused);
- a From domain;
- whether it takes emergency calls;
- a provider profile.

**Transports and media.**

- TLS with SRTP (SDES keys in the SDP, RFC 4568) is the default.
- UDP, TCP or plain RTP need the profile "PBX on a private network". It is for a PBX on the same network, set up as
  a trunk. The services refuse them with any other profile; nothing is ever downgraded silently (D-V4).
- An incoming call to an SRTP trunk that offers plain RTP is refused (488).

**Credentials.** A trunk's credentials answer only:

- its realm (when the realm is set);
- the algorithms it allows: SHA-512-256, SHA-256 or MD5; SHA-256 and MD5 when none are set.

The strongest challenge the credentials allow is answered. A challenge for another realm, or only with another
algorithm, fails the registration or the call. The trunk's status then shows the reason, and no response is
computed.

**Registration.** A registering trunk sends REGISTER every 300 seconds, or as the registrar grants. A failure is
retried after 10 seconds, doubling up to 5 minutes. Each trunk registers on its own: one that fails is logged, and
its status and last error are on the trunk. The admins get a `VoiceAlert` notification. The other trunks go on.

**Incoming calls** are taken:

- from a trunk's host addresses and the signaling addresses listed on it (a registering trunk's calls come from its
  host too), or
- for a trunk with the profile "PBX on a private network" only, from another address with a digest answer for that
  trunk's username: a PBX that authenticates its calls.

A provider trunk's credentials are its account's at the provider, so they never sign a call in: anyone could guess
them online, and a guess would be the provider's password. A call that answers with them is refused, and counts as a
failed digest of its source.

The number called (the Request-URI's user, in E.164) is looked up in `PhoneNumber`. A number routed to a user rings
that user's tabs and devices like a call from a colleague. An unknown number gets 404; with no media server, 480.

## Provider profiles

Only a profile checked by an authorized live call is marked verified. None is yet (§7.3: a fake trunk, a release
label or a refusal notice is no evidence for a live provider).

### Telnyx (not yet verified)

Telnyx is the first provider for the documentation and the live checks (D-V4). Check each value against Telnyx's
current documentation; the live check will settle them.

- In the Telnyx portal:
  - make a SIP Connection with credential authentication, and its username and password;
  - assign the numbers to it;
  - enable TLS and SRTP on it.
- In Moqui, the trunk:
  - host `sip.telnyx.com`, port 5061, transport TLS, media SRTP, profile Telnyx;
  - the connection's username and password;
  - realm `sip.telnyx.com`;
  - algorithms SHA-256 and MD5;
  - register on;
  - the signaling addresses Telnyx publishes, as its inbound addresses.
- Caller ID: a number presented must be one of the connection's numbers. Telnyx does the STIR/SHAKEN attestation.
- Emergency: Telnyx's emergency calling is set up per number, with an address it validates. Enable it on the
  callback number in the portal, and put Telnyx's address id in the `EmergencyLocation`'s provider reference. Then
  mark the trunk emergency-capable.
- SMS is through Telnyx's messaging API (C4), and does not need the SIP trunk.

### Other providers

The generic profile, TLS and SRTP, fits providers that take digest-registered or IP-authenticated trunks:

- Twilio Elastic SIP, Bandwidth, VoIP.ms, Flowroute;
- an Asterisk, FreeSWITCH or 3CX server set up as a trunk (a PBX on a private network with UDP or plain RTP takes
  the profile "PBX on a private network").

Each provider is unverified until its own live check.

## Calls

- **Calling a number.** `place#PhoneCall` starts a phone call (`CskPhone`): the user's browser leg, and a phone leg
  (`CltPstn`) that the engine dials through the caller ID's trunk. BasicComms has "Call a number", with a caller-ID
  choice from the user's grants. The full dialer is V3.
- **Answering.** When the number answers, the call is active. When the user leaves, or the far end hangs up, it
  ends.
- **Codecs:** G.711 μ-law and A-law, and Opus where the provider offers it. The offer lists PCMU, PCMA and Opus, and
  the answer takes the offerer's order. Phone audio is resampled between 8 and 48 kHz, and transcoded to and from the
  browsers' Opus (moqui-rtc's audio core).
- **Symmetric RTP.** Media goes back where the phone's packets come from (RFC 4961).
  - With SRTP, only an authenticated packet moves it.
  - With plain RTP, only packets from the SDP's address are taken (on any port, for a NAT), the first one latches,
    and from then on only that source is taken. A NAT that changes the address, not only the port, needs SRTP. Keep
    plain RTP to private networks.
- **DTMF** is RFC 4733 telephone-event, both ways.
  - The in-call keypad of `m-call` sends digits (`send#PhoneDtmf`).
  - The digits a phone sends are noted (`note#PhoneLeg` `dtmf`): a 1 acknowledges a recording that asks for it
    ([Notices](#notices)); the IVR and queues come later.
- **Hold** is a re-INVITE either way.
  - The provider's `sendonly` or `inactive` is answered `recvonly` or `inactive`, and Moqui stops sending.
  - `hold#PhoneCall` and `resume#PhoneCall` send Moqui's own re-INVITE.
- **Cancel and forks.**
  - A call the user gives up while it rings is canceled (CANCEL, 487).
  - An answer that crosses the CANCEL is ACKed and ended with a BYE, and the call counts as canceled.
  - When a provider forks and several phones answer, the first answer wins; the others are ACKed and ended.
  - An outbound call that rings 60 seconds is canceled.

## Voicemail

An inbound call to a user goes to their voicemail box (V2) when it rings the box's rings with no answer (the engine's
timer, from `receive#PhoneCall`'s `voicemailSeconds`), when the user is in another call (`voicemail: busy`: not rung),
when the user declines it (the control `voicemail` from `decline#Call`), or on the access number (`PnrVoicemail`). The
engine asks `VoicemailServices.begin#Voicemail`, answers the call itself, plays the greeting and a beep, records the
caller as Ogg Opus, and hands the file to `store#Voicemail`; `*` in the greeting, or the access number, opens phone
access with a PIN. All of it is in [Voicemail](Voicemail.md).

## Notices

What a browser in the call sees as an `rtc` frame, a phone leg hears as a prompt mixed over the call's audio. The
services send it with `VoiceServices.notify#PhoneLeg` (`send#SipControl`, action `prompt`) to the engine of the call's
node, which plays a tone pattern for each notice (`src/sip/prompt.rs`) and then the deployment's audio, when a
property names a WAV file (16-bit PCM, mono or stereo, 8 to 48 kHz, at most a minute; any resource location). A
notice that comes while another plays waits for it.

| Notice | When | Tones | Audio |
| --- | --- | --- | --- |
| `recording` | A recording starts, or the phone answers while one goes ([Call recording](Call-Recording.md#the-notice-and-consent)) | 1400 Hz, 400 ms | `call_recording_prompt_audio` |
| `recordingAck` | The same, under `call_recording_ack_required` | 1400 Hz twice | `call_recording_prompt_audio` |
| `captions` | Captions turn on, or the phone answers while they are on ([WebRTC, Captions](WebRTC.md#captions)) | 900 then 1200 Hz | `call_captions_prompt_audio` |

- **Acknowledging** a recording is pressing 1. The phone's DTMF comes back through `note#PhoneLeg`, and
  `CallRecordingServices.ack#LegRecordingNotice` logs it with the method `dtmf`. Other keys, and a 1 when nothing asks
  for it, are the call's own.
- **Withdrawing** is hanging up.
- Audio that does not read is logged, and the notice plays its tones only.

## Registrar

Desk phones (Yealink, Polycom, Grandstream) and soft phones (Linphone, Zoiper, Groundwire, on a computer or a mobile, the
native-calls path of §7.2) register with the SIP engine, which is their registrar (plans/Comms-Framework.md §5.2
"Registrar for phones", V4). It is off unless `sip_registrar_enabled` is true, even with the listeners on; until then a
REGISTER is refused as before, and phones may be added but cannot register or ring.

**Phones and their credentials.** A user adds a phone in Comms, Settings, Desk and soft phones
(`SipDeviceServices.create#SipDevice`). Its credentials are generated: a ten-digit username and a 22-character random
password. They show once, with the domain, the port and the transport, each with a copy button; the password is never
stored. What is kept (`SipDeviceVerifier`, encrypted) is a digest verifier per algorithm for `sip_domain`: HA1 =
H(username:realm:password), as RFC 7616 and RFC 8760 define it.

- **Profiles.** A standard phone has a SHA-256 verifier only. The registrar challenges with SHA-256 first and MD5 second
  (RFC 8760), and an MD5 answer from a standard phone fails: it counts against its source like any failed digest. The
  legacy profile keeps an MD5 verifier too, for a phone that has no SHA-256; choose it only for such a phone.
- **New password** (`rotate#SipDeviceCredential`) makes a new password for the same username, deletes the old
  verifiers, and ends the phone's registrations on every engine; the phone stops until it has the new one.
- **Remove** (`revoke#SipDevice`) deletes its verifiers, ends its registrations and hangs up its calls. It cannot
  register or ring again.
- An admin sees everyone's phones and removes one in System, Voice, Phones.

**Setting up a phone.** In the phone or the app, add a SIP account:

- username and password as shown; the domain (also the registrar, or server, or proxy) is `sip_domain`;
- transport TLS on `sip_tls_port` (5061) with SRTP media (SDES) for a phone outside the office network. UDP or TCP on
  `sip_port` is for a phone on the same network, with plain RTP;
- with an SRV record (`_sips._tcp`, [Settings](#settings)) a phone given only the domain finds the server;
- behind a home router, turn on the phone's "outbound" (RFC 5626) or keepalive option where it has one;
- leave the registration expiry at its default; the registrar holds it to its limits.

**Bindings.** Each registered contact is a binding (`SipRegistration`, one row each): the contact, the flow the REGISTER
came over (transport, and the source address and port as `rport` gives them, RFC 3581), its Call-ID and CSeq, its
Outbound instance and reg-id, when it expires and was last refreshed, and the phone's user agent. Settings shows them.

- A phone may have several bindings (at most 10), say a soft phone on two computers.
- Expiry is the contact's, else the request's, else `sip_registrar_default_expires`, held between
  `sip_registrar_min_expires` and `sip_registrar_max_expires` (one under the minimum is refused with 423 and
  `Min-Expires`).
- `Expires: 0` on a contact removes it; `Contact: *` with `Expires: 0` removes them all.
- A REGISTER with the same Call-ID must have a higher CSeq; one out of order is refused (500) and changes nothing.
- An expired binding, one of an older credential revision, and those of a removed phone cannot ring. The engine sweeps
  expired bindings every 5 seconds, and the job `expire_SipRegistrations` removes the rows of an engine that stopped.

**NAT.** Phones behind NAT mostly do not do ICE, so the registrar keeps the path the phone opened:

- **The flow.** A call to the phone goes back over the flow its REGISTER came on: the same TCP or TLS connection, or,
  over UDP, its source address and port from the listener's own socket, which the NAT's mapping lets in. The address in
  its Contact is never dialed. A TCP or TLS connection that closes ends the bindings over it.
- **Outbound (RFC 5626).** A phone that sends `Supported: outbound` with `+sip.instance` and `reg-id` binds its flow; the
  200 says `Require: outbound` and `Flow-Timer` (25 seconds over UDP, 120 over a stream), and the phone sends CRLF
  keepalives, which the engine answers. Such a binding keeps its expiry.
- **Short expiry.** A phone behind a NAT (its Via does not name where it sends from) without Outbound gets at most
  `sip_registrar_nat_expires` (60 seconds), so its own REGISTER keeps the mapping open.
- **RTP latching.** A phone's RTP is answered where it comes from. With SRTP, only an authenticated packet moves it.
  With plain RTP, packets are taken from the SDP's address or from the address the phone signals from (its NAT's,
  when its SDP names its private address), on any port until the first one latches; then only from that source.

**Calls to a phone.** When a call rings a user (a call from a colleague or a browser, or a call to their number
through a trunk), each of their phones registered with the call's node gets a leg (`CltSipDevice`, offered), and the
engine forks an INVITE to every live binding of it, beside the ring in their tabs.

- The offer is Opus, G.722 (wideband desk phones; 16 kHz audio on its 8000 RTP clock) and G.711, with
  telephone-event; SRTP to a binding over TLS, plain RTP otherwise.
- The first answer wins, through the call services' state machine: the answering phone's leg is the user's in the call,
  and the other offers end. Their tabs stop ringing, and the other bindings and phones are canceled; a phone that
  answers after is ACKed and ended with BYE. A tab that answers first cancels the phones the same way; declining in a
  tab stops them too.
- An inbound phone call is answered toward its trunk when the user's phone answers. The phone hears the call's notices
  (recording, captions) as a phone leg does.

**Calls from a phone.** A registered phone's INVITE answers a challenge (`Proxy-Authorization`) with its credentials,
and the call is placed as its user (`SipDeviceServices.receive#DeviceCall`), with the phone as their leg:

- a colleague's username is an internal call, which rings their tabs and phones;
- anything else is a number, through `place#PhoneCall`: the user's outbound permission and caller ID, international
  and premium, the limits, and the emergency path first. A phone gives no more than its user has.
- The phone hears ringing (180) until the far end answers, and the call ends when either hangs up.

**Mobile.** On a mobile, a SIP soft phone registered this way is how calls ring natively (§7.2). Push to wake a
sleeping app is the app's own service; the registrar does not send pushes.

## Fraud controls

Scanners probe SIP listeners for weak credentials, then place international calls (§5.2). Each of these is a release
gate.

- **Floods are held.** New requests from one source are held to `sip_request_rate` (`sip_request_burst` at once), and
  dropped while the engine runs `sip_max_transactions`; TCP and TLS connections past `sip_max_connections` are closed
  ([Settings](#settings); `moqui-sip` `guard.rs` `RequestGate`). Requests in a dialog and responses always pass.
- **Failed digests block the source address, not the account.**
  - A request from an address that is not a trunk's is challenged: expiring nonces, and nonce counts that cannot be
    replayed (`moqui-sip` `digest.rs`).
  - Each failed answer counts against its source.
  - At `authFailureLimit` (5) failures in `authFailureWindowSeconds` (600), the address is blocked for
    `authBlockSeconds` (3600). Everything it sends is dropped unanswered, before a transaction is made.
  - The engine counts failures in memory; the block is kept in `VoiceAuthFailure` (so it holds after a restart), and
    the admins get one alert for it.
  - An IPv6 source counts as its /64. At most 50,000 sources are tracked; past that the ones not blocked go first.
  - No user account is ever locked or counted. An admin clears a block in System, Voice, Fraud.
- **International and premium destinations are off.**
  - They need the deployment's `VoicePolicy` and the user's `VoiceUserSetting` both.
  - Premium is the policy's E.164 prefixes (`+1900`, `+1976` by default).
  - Numbers are classed by the policy's home country code (1 by default).
  - With home country 1, the area codes of the plan's other countries (Bahamas, Barbados, Jamaica, the Dominican
    Republic and the rest of the Caribbean, Bermuda) are international: they dial like domestic numbers and are the
    usual toll-fraud destinations. US territories (Puerto Rico, the US Virgin Islands, Guam) stay domestic.
- **Outbound calling is per user.** A user needs outbound enabled in their setting.
- **Caller ID is authorized per user.** A user presents only a number granted to them (`PhoneNumberUser`).
- **Limits.**
  - Concurrent phone calls: 20 in all, 2 per user.
  - Calls a day: 100 per user, 1,000 in all.
  - Each is a `VoicePolicy` value, and a user's own setting can lower its own, never raise it.
  - Provider spending limits are set at the provider.
- **The engine checks again.** Before an INVITE goes to a trunk, the engine classes the number itself. It refuses
  international or premium numbers the deployment does not allow, an emergency number outside the emergency path, a
  non-emergency trunk for one, and calls past the concurrency limit, whatever asked for the call.
- **No header injection.** The caller's display name is cleaned of line breaks and quotes, and an extra header with
  a line break is not sent, so a user's name cannot add a `P-Asserted-Identity` the provider trusts.
- **IP-authenticated trunks over UDP.** A UDP source address can be forged. Prefer TLS (the default) or TCP for a
  trunk taken by its address.
- **Logs.** rsipstack logs whole SIP messages at INFO, SDES keys and digest answers included; the host keeps only
  its warnings and errors.

## Emergency readiness

Emergency numbers (the policy's `emergencyNumbers`: 911, 112 and 999 by default) are routed before ordinary prefixes
and limits (D-V2).

- An emergency call goes only through an emergency-capable trunk.
- It needs the user's registered location: a validated `EmergencyLocation`, their own or that of a number they
  present.
- It needs the location's validated callback number. The call presents it as caller ID, with `P-Asserted-Identity`.
- It needs a notification destination: the location's `notifyEmailAddress`. It and the admins are told of each
  emergency call.
- Without any of these the call is refused with a message naming what is missing (`emergencyNotReady`). System,
  Voice, Emergency shows the same list, and `get#VoiceCapabilities` returns it (`emergencyMissing`).

Before emergency calling is enabled for a deployment, check off:

1. The trunk's provider takes emergency calls, and the trunk is marked emergency-capable.
2. Each location's address is validated with the provider, and its provider reference recorded.
3. Each callback number is provisioned with the provider for its location (Telnyx: emergency calling enabled on the
   number with its address).
4. Each user who may call has a registered location. Mobile and remote users update theirs when they move.
5. The notification destination reaches someone who can act: a front desk or a security desk (Kari's Law).
6. The operator has reviewed the deployment against its jurisdiction's rules. The [US guidance on Kari's Law and RAY
   BAUM'S Act](https://www.911.gov/issues/legislation-and-policy/kari-s-law-and-ray-baum-s-act/) covers direct dialing, notification and dispatchable location.
   Other jurisdictions need their own review.
7. A live check uses the provider's approved test procedure (in the US, a test call to 933 where the provider offers
   it). Never place an unarranged call to an emergency number.

Moqui supplies the controls and the notices; meeting the rules is the operator's job.

## Status

- `get#VoiceCapabilities` reports separately (§3.1):
  - supported: the binary has `sip`;
  - configured: a SIP port is set and a trunk exists;
  - active: a SIP engine runs beside a media node, and a trunk has not failed;
  - the reason when one is not;
  - emergency readiness.
- System, Voice, Status shows them, with the SIP nodes and media nodes.
- Each engine registers its node (`SipNode`) every 20 seconds.

## Not yet

- **Registrar.**
  - Provisioning files for the common desk phones come later; a phone is set up by hand.
  - A phone rings only for calls whose media is on the node it registered with (one node a call, until W7).
  - A UDP phone's STUN keepalive (RFC 5626 over UDP) is not answered; its CRLF keepalive is, and without Outbound
    the short expiry keeps its NAT open.
  - An extension is a colleague's username; there are no numeric extensions yet.
  - Not checked with a real desk phone or app yet: the in-memory phones of the tests play their part.
- **Offers.**
  - An INVITE without an SDP offer (late offer) is refused (488).
  - Video in an offer is refused with port 0 (D-V3: SIP legs are audio only).
- **RTCP.** No RTCP is sent, and the RTCP that comes is dropped; SRTCP is not supported. Call quality from RTCP
  reports comes later.
- **SRV for trunks.** A trunk's host is resolved by its address records; a provider that needs SRV lookups for its
  trunk host takes its address in the host field.
- **IPv6** is not tested.

## Tests

- `cargo test -p moqui-sip`: the V1 cases against an in-process fake trunk, in about 3 seconds:
  - digest and SRTP vectors;
  - registration;
  - calls each way with media and DTMF;
  - hold;
  - cancel and a crossing answer;
  - forked answers;
  - failed digests blocking a source;
  - a provider trunk's credentials refused for a call in, a PBX trunk's taken;
  - a flood from one source held to its rate while another source calls in;
  - inbound ranges wider than a /8 or a /32 ignored;
  - a call over TLS with SRTP;
  - two phone legs mixed;
  - the registrar against in-memory phones (`ua_phones_tests.rs`): a SHA-256 registration and MD5 refused for a
    standard phone, a replayed REGISTER refused, several bindings, unregistering one or all, expiry, rotation and
    revocation, a call forked to two bindings with the first answer winning, `rport` and an Outbound TCP flow (its
    keepalive, a call over the same connection, its close), and a phone's own signed call;
  - the bindings on simulated time (`registrar_tests.rs`), G.722 negotiation and audio, and plain RTP latching behind a
    NAT (`media_tests.rs`).
- `cargo test -p moqui-host --test it sip::`: an outbound and an inbound call on a real server, through a fake trunk,
  heard by a browser stand-in; and a desk phone that registers, rings for a colleague's call, answers in G.722 and is
  heard by the colleague's browser.
- `cargo test -p moqui-host --lib sip`: the notices' tone patterns, prompt audio from WAV, prompts queued one after
  another, and the registrar's settings; voicemail's ([Voicemail, Tests](Voicemail.md#tests)).
- `./moq test framework --spec Voice`: the services' authorization, fraud controls (inbound digest for a PBX trunk
  only, inbound ranges no wider than a /8 or a /32), emergency routing, a phone leg's
  notices and its acknowledgement by pressing 1, and capabilities.
- `./moq test framework --spec SipDevice`: the registrar off by default, credentials shown once with only verifiers
  kept, another user refused with the same parameters, rotation and revocation, a phone ringing beside the tabs with
  the first answer winning (a phone, a tab, one of two phones), and a phone's call to a colleague.
