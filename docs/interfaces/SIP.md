# SIP trunks and phone calls

A user calls a phone number from BasicComms, and a number's calls ring its user, through a SIP trunk provider. The
browser never speaks SIP: it talks WebRTC to Moqui ([WebRTC](WebRTC.md)), and Moqui places the SIP leg toward the
trunk. The phone is one more leg of the call's `CallSession`. Its audio joins the call's room on the media node: the
browsers hear it as a participant, and the phone hears everyone else mixed (mix-minus).

- The SIP user agent and B2BUA are in `crates/moqui-sip`, on rsipstack.
- The engine, the listeners and the RTP sockets are in `crates/moqui-host/src/sip/`.
- The voice services are `org.moqui.impl.VoiceServices` (`framework/service/org/moqui/impl/Voice.js`), with
  `framework/entity/VoiceEntities.xml`.
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

Trunks, numbers, caller-ID grants, fraud limits and emergency locations are records, set up in System, Voice:
`VoiceTrunk`, `PhoneNumber`, `PhoneNumberUser`, `VoicePolicy`, `VoiceUserSetting` and `EmergencyLocation`.

- A trunk's credentials (`VoiceTrunk.authJson`) are an encrypted field. The services that list trunks never return
  them; only the engine's `get#EngineConfig` decrypts them.
- After a trunk or the policy changes, each running engine reloads them.

Open these ports on the firewall, to the provider's signaling and media addresses only where it publishes them:

- `sip_port`, UDP and TCP;
- `sip_tls_port`, TCP;
- the `sip_rtp_ports` range, UDP.

**DNS.** Desk and soft phones (the registrar comes in V4) find a server through its SRV record:

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
- the provider's signaling addresses calls come from;
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

- from a trunk's host addresses and the signaling addresses listed on it, or
- from another address, with a digest answer for a trunk's username (a PBX that authenticates its calls).

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
  - The digits a phone sends are noted (`note#PhoneLeg` `dtmf`), for the IVR and queues later.
- **Hold** is a re-INVITE either way.
  - The provider's `sendonly` or `inactive` is answered `recvonly` or `inactive`, and Moqui stops sending.
  - `hold#PhoneCall` and `resume#PhoneCall` send Moqui's own re-INVITE.
- **Cancel and forks.**
  - A call the user gives up while it rings is canceled (CANCEL, 487).
  - An answer that crosses the CANCEL is ACKed and ended with a BYE, and the call counts as canceled.
  - When a provider forks and several phones answer, the first answer wins; the others are ACKed and ended.
  - An outbound call that rings 60 seconds is canceled.
- **Recording notice.** Recording (W5) plays its notice to a phone leg through `play_prompt` in `src/sip/mod.rs`, as
  a tone or recorded speech mixed over what the phone hears.

## Fraud controls

Scanners probe SIP listeners for weak credentials, then place international calls (§5.2). Each of these is a release
gate.

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

- **Registrar.** A desk or soft phone cannot register yet (V4). A REGISTER from anyone is refused (403), after the
  same digest checks.
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
  - a call over TLS with SRTP;
  - two phone legs mixed.
- `cargo test -p moqui-host --test it sip::`: an outbound and an inbound call on a real server, through a fake trunk,
  heard by a browser stand-in.
- `./moq test framework --spec Voice`: the services' authorization, fraud controls, emergency routing and
  capabilities.
