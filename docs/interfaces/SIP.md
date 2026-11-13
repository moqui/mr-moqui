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
- The voice services are `org.moqui.call.VoiceServices` (`framework/service/org/moqui/call/Voice.js`), with
  `framework/entity/VoiceEntities.xml`; the phones' are `org.moqui.call.SipDeviceServices` (`SipDevices.js`); call
  handling, ring groups and schedules are `org.moqui.call.CallHandlingServices` (`CallHandling.js`, V3); the IVR, queues and
  transfers are `org.moqui.call.IvrServices`, `CallQueueServices` and `CallTransferServices` (`Ivr.js`, `CallQueues.js`,
  `CallTransfers.js`, V5), with the engine's side in `src/sip/ivr.rs`.
- Administration is System, Voice (in basic-comms).
- The design is [plans/archive/Comms-Framework.md](../../plans/archive/Comms-Framework.md) §5.2, and "The V1 gate" in §6.2.

The engine needs:

- the `sip` feature, in the Maximal, Enterprise and Debug packages;
- call media on the same server (`rtc_media_port`, [WebRTC](WebRTC.md)).

Every listener is off until its setting is set.

## Settings

| Property | Default | |
| --- | --- | --- |
| `sip_port` | empty (off) | SIP over UDP and TCP; 5060 by convention |
| `sip_host` | `127.0.0.1` | The address `sip_port` binds |
| `sip_tls_port` | empty (off) | SIP over TLS; 5061 by convention. The certificate comes from `runtime/cert` (for `sip_public_address`, else `default`), as the HTTPS listener's and TURN's do. A client must finish its TLS handshake within 10 seconds; at most 128 handshakes run at once, 8 from one source |
| `sip_tls_host` | `sip_host` | The address `sip_tls_port` binds |
| `sip_rtp_ports` | `20000-20999` | The UDP range of the phone legs' RTP, one port a leg |
| `sip_rtp_host` | `sip_host` | The address the RTP sockets bind |
| `sip_public_address` | the bound address | The address in Contact headers and SDP, for a server behind 1:1 NAT. A host bound to every address does not start without it |
| `sip_tls_ca_file` | the system bundle (`/etc/ssl/certs/ca-certificates.crt`) | The CA certificates that verify a trunk's TLS certificate |
| `sip_request_rate`, `sip_request_burst` | `20`, `100` | New requests (outside a dialog: INVITE, REGISTER, OPTIONS and the rest) a second from one source, and how many it may send at once; past them its requests are dropped unanswered. `0` turns it off |
| `sip_max_transactions` | `2000` | New requests are dropped while the engine runs this many transactions; calls going on are not cut. `0` turns it off |
| `sip_max_connections` | `500` | TCP and TLS connections open at once; one past it is closed. `0` turns it off |
| `sip_max_connections_per_source` | `100` | TCP and TLS connections open at once from one source (an IPv6 /64 is one), so one source cannot hold every connection phones keep open; one past it is closed. `0` turns it off |
| `sip_registrar_enabled` | `false` | The registrar for desk and soft phones ([Registrar](#registrar)); off even with the listeners on. Read when the engine starts, as the listeners' settings are: a change takes a restart |
| `sip_domain` | `sip_public_address`, else `moqui` | The realm of the digest challenges and the domain phones register to. The phones' verifiers are for it: after a change, each phone needs a new password |
| `sip_registrar_min_expires`, `sip_registrar_max_expires` | `60`, `3600` | A phone binding's expiry is held between these, in seconds; under the minimum is refused (423) |
| `sip_registrar_default_expires` | `600` | The expiry of a binding whose phone asks none |
| `sip_registrar_nat_expires` | `60` | The longest expiry of a phone behind a NAT that does not use Outbound, so its REGISTER keeps the NAT's mapping open |
| `call_forward_max_hops` | `5` | The most forwards one call takes ([Call handling](#call-handling)): a user's to another user or group, a group's fallback group; past it the forward is skipped. 1 to 10 |
| `call_forward_ring_seconds` | `30` | How long a forward to an outside number rings before the call goes on (to voicemail, or the next step). 10 to 120 |
| `call_queue_tick_seconds` | `5` | How often the engine asks about each call waiting in a queue ([Queues](#queues)): offers that rang out, the next offer, the wait limit, the caller's place. 1 to 30 |
| `call_transfer_ring_seconds` | `30` | How long a blind transfer's target rings before the far end goes to the target's voicemail ([Transfer and merge](#transfer-and-merge)). 10 to 120 |
| `call_hold_music_location` | empty | Hold music for calls on hold and in queues without their own (any resource location; WAV or Ogg Opus, looped). Empty: a tone pattern. Use only audio you may use |
| `call_queue_prompt_location` | empty | A folder with `0.wav` to `9.wav`: a queue caller's place spoken as digits. Without all ten, the place is pips |

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

**Host and port.** A trunk with a port, or whose host is an address, goes to that host and port. One with a name and no
port is looked up by its SRV records (RFC 3263): `_sips._tcp.<host>` for TLS, `_sip._tcp.<host>` for TCP,
`_sip._udp.<host>` for UDP.

- The targets are tried by priority, the lowest first, then by weight, the highest first. Each target's addresses are
  tried in turn, at the record's port.
- With no SRV records, or when the lookup fails, the host's own addresses are used, at 5060 (5061 for TLS).
- Records whose target is `.` say the provider offers no SIP there: the trunk has no address.
- A registration goes to the next target when one takes no connection (5 seconds for TCP and TLS), has no answer in
  32 seconds, or answers 503. A call goes to the next target the same way, before anything rang. An OPTIONS ping
  passes a target only when it takes no connection.
- The registration and the pings look the host up again each time, so the addresses follow its DNS.
- The Request-URI, From and To still name the host (or the From domain). TLS names the host too, for SNI and for the
  certificate check, never an SRV target: a provider's certificate must name the host it is set up with.
- A trunk whose host does not resolve is logged with its id and still loaded; its registration or pings fail, and the
  other trunks go on.

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

**OPTIONS pings.** A trunk that does not register (an IP-authenticated trunk, a PBX) is sent OPTIONS every minute, so
System, Voice, Trunks shows it Reachable or Failed before a call fails. Any final answer counts as up (a 200, a 405, a
challenge); no answer within 8 seconds is no answer, and three pings in a row with none are down (one lost ping is
not: a failed trunk is left out of the emergency readiness and the calls' capabilities), with the reason as the trunk's
last error, and the admins hear of it as of a failed registration. The status is sent when it changes, and every ten pings.

**Incoming calls** are taken:

- from a trunk's host addresses (every SRV target's, [Host and port](#trunks)) and the signaling addresses listed on
  it (a registering trunk's calls come from its host too), or
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
- SMS is through Telnyx's messaging API, and does not need the SIP trunk ([SMS](SMS.md)). A number with a messaging
  bridge puts its calls and voicemail in the same room as its texts with each outside number (below).

### Other providers

The generic profile, TLS and SRTP, fits providers that take digest-registered or IP-authenticated trunks:

- Twilio Elastic SIP, Bandwidth, VoIP.ms, Flowroute;
- an Asterisk, FreeSWITCH or 3CX server set up as a trunk (a PBX on a private network with UDP or plain RTP takes
  the profile "PBX on a private network").

Each provider is unverified until its own live check.

## Calls

- **Calling a number.** `place#PhoneCall` starts a phone call (`CskPhone`): the user's browser leg, and a phone leg
  (`CltPstn`) that the engine dials through the caller ID's trunk. BasicComms has the Dialer (Calls, Dialer, and the
  tray's Dialer tab, V3): a keypad, the search for a person or a number through the apps' `recipient-source` providers
  (else the framework's users that a number rings), the number in E.164 by the deployment's rules, and a caller-ID
  choice from the user's grants and their ring groups' numbers. **Hide my number** (the user's setting, or the call's)
  sends `Privacy: id` (RFC 3323, 3325); the trunk still gets the number it authorizes in From.
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
    ([Notices](#notices)); in an IVR menu the engine takes them itself ([IVR](#ivr)).
- **Hold** is a re-INVITE either way.
  - The provider's `sendonly` or `inactive` is answered `recvonly` or `inactive`, and Moqui stops sending.
  - `hold#PhoneCall` and `resume#PhoneCall` (m-call's Hold) send Moqui's own re-INVITE (`sendonly`); the phone hears hold
    music (`call_hold_music_location`, else a tone pattern) and is not heard by the others (`CallLeg.heldDate`, V5).
- **Cancel and forks.**
  - A call the user gives up while it rings is canceled (CANCEL, 487).
  - An answer that crosses the CANCEL is ACKed and ended with a BYE, and the call counts as canceled.
  - When a provider forks and several phones answer, the first answer wins; the others are ACKed and ended.
  - An outbound call that rings 60 seconds is canceled.

## Call handling

A call to a number routed to a user (`PnrUser`) or a ring group (`PnrRingGroup`) gets a plan when it arrives
(`receive#PhoneCall` hands it to `CallHandlingServices` `inbound#PhoneCall`, kept in `CallRoute`): a list of steps, each
"ring these users and numbers for so long", a voicemail box, a refusal or the end. The engine rings the first step for
its seconds (`voicemailSeconds`); when its timer ends, when everyone rung declines, or when a forward to a number fails,
the engine asks `begin#Voicemail` as before, and the plan answers with its next step: `mode: ring` with `ringSeconds`
(the engine rings on, with a new timer that replaces the last), the plan's box, or nothing (the call ends). Each step has a
token (`CallRoute.stepSeq`, `stepToken` in the answers): the engine's timer and the services' `voicemail` control carry
the token of the step they came from, and one that arrives after the plan moved on is `stale` and changes nothing, so a
late decline or timer of an earlier step cannot skip the step that rings now.

**A user's rules** (Settings, Call handling; `CallHandlingRule`, read in `sequenceNum` order, each while its schedule
is open or closed):

1. Anonymous rejection: a caller with no number is refused, 433 Anonymity Disallowed.
2. Selective rejection: a caller on the rule's list is refused, 603 Decline (or goes to its target, such as voicemail).
3. Selective acceptance: only callers on the lists ring; the others are refused, or go to the rule's target.
4. Selective forward: a caller on the list goes to the rule's target.
5. Do not disturb (`UserStatus`, the account menu or the settings): straight to voicemail.
6. Always forward: every call goes to the target.
7. Busy: in a call with call waiting off (`CallHandlingSetting.callWaiting`, off by default): the busy rule's target, else
   voicemail.
8. The ring: the user's tabs, phones and, with **simultaneous ring**, their mobile (`ringMobile`, `mobileNumber`, under
   `mobileScheduleId` when set), for their box's rings; then the no-answer rule's target, else voicemail.

A target is an outside number, another user (their rules apply), a ring group, voicemail, or a refusal. Forwards are
bounded: a user or group already in the plan is skipped (a loop), as is a forward past `call_forward_max_hops`, a
forward back to the caller's own number, and an emergency number (never a target). A forward to one of this deployment's
numbers routes inside, to its user or group, never through the trunk.

**Forwards to outside numbers.** A forwarded number is a phone leg of the user's (`CltPstn` under their participant),
placed through the trunk of the number the call came in on, which is its caller ID. It is under the full fraud policy:
the user needs outbound calls and the forward permission (`VoiceUserSetting.forwardEnabled`, set by a voice admin);
international and premium destinations need the deployment's and the user's permission; the user's limits at once and
per day count their forwards with the calls they place, the deployment's calls in a day count every forward (a ring
group's fallback too), and the engine checks the destination and its calls at once again. A forward that is refused is
left out of the plan, or closed as failed when its turn comes; a step left with nothing ringing goes on at once.

**Ring groups** (System, Voice, Ring groups; `RingGroup`, `RingGroupMember`). A group rings its members all at once
(`RgsSimultaneous`), in order (`RgsInOrder`, `ringSeconds` each) or in round robin (`RgsRoundRobin`: in order, starting
after the member rung first last time, `lastMemberUserId`). Members in do not disturb, or busy with call waiting off, are
skipped, when the plan is made and again when their turn comes; each member's mobile rings with them. Every member rung
gets a `CallUserLog` row (`CurRingGroupMember`): answered, or missed (seen at once when another member answered). With
nobody answering, or outside the group's schedule, the fallback runs: the group's own voicemail box (its members share
it, and its heard state), another group, an outside number (set by a voice admin, so the deployment's policy alone
applies), or the end.

**The first answer wins** across every fork of an inbound call, in one transaction on the call's row: a tab, a desk or
soft phone, a mobile or a forwarded number. Every other offer ends (the tabs' close, the engine cancels the phones and
the forwarded numbers) and a late answer is hung up. The caller's leg is answered toward the trunk when the winner
answers.

**Schedules** (System, Voice, Schedules for the deployment's; a user may keep their own; `CommsSchedule`,
`CommsScheduleRule`): weekly hours (a span past midnight runs into the next day), hours on dates, and holidays, once or
each year, read in the schedule's time zone (a time zone that is not known is logged and read as the server's). A date
with a dated rule is read from its dated rules alone.

**Presence** comes from the user's answered legs (`UserStatus.inCall`): leaving one of two calls keeps it, whichever
leg (a tab, a phone, a forwarded number) is left.

## IVR

A number routed to a menu (`PnrIvr`, System, Voice, IVR menus; `IvrMenu`, `IvrOption`) is answered by the engine at once:
`receive#PhoneCall` answers `ivr`, the menu, and the engine runs its keys (`src/sip/ivr.rs`, a state machine with no
I/O, like voicemail's):

- the menu's prompt (the admin's uploaded audio, made Ogg Opus as a greeting is; without one, three rising tones), then
  a key; a key during the prompt cuts it short;
- `maxDigits` above 1 is an extension: keys until `#`, the most keys, or a pause of `timeoutSeconds`;
- no key in `timeoutSeconds` plays the menu again, `maxRetries` times, then the timeout target;
- a key no option has goes to the invalid target; a menu without one plays two falling tones and the menu again, and
  counts it as a retry;
- outside the menu's schedule (`commsScheduleId`, a `CommsSchedule`) its closed target runs instead of the menu.

Where a choice goes is `IvrServices.route#IvrCall`'s answer (`CallTarget`):

| Target | The call |
| --- | --- |
| `CtgUser`, `CtgGroup` | A user (their call handling rules) or a ring group (its strategy): V3's plan (`CallRoute`); the caller hears ringback, and the engine's timer moves the plan on as for any inbound call |
| `CtgVoicemail` | A voicemail box: the engine starts voicemail with the plan's step token |
| `CtgNumber` | An outside number, set by a voice admin: the deployment's fraud policy (international and premium off until enabled, emergency numbers never) |
| `CtgQueue` | A queue ([Queues](#queues)) |
| `CtgMenu`, `CtgRepeat` | Another menu, or this one again; a call goes through at most 20 menus (`CallIvrState`), so a loop of menus ends |
| `CtgEnd` | The engine hangs up |

An option may have its own prompt ("connecting you to sales"), played as the call goes on. The call's path through
the menus is kept in `CallIvrState.pathText`. Moqui ships no recorded speech and calls no text to speech: prompts are
the admin's recordings or tones.

## Queues

A call goes into a queue (`CallQueue`; System, Voice, Queues) from a number routed to it (`PnrQueue`), an IVR option or
a transfer: a `CallQueueEntry`, waiting.

- **What the caller hears.** Hold music looped (the queue's uploaded audio, else `call_hold_music_location`, else a soft
  four-tone figure), and every `announceSeconds` their place: a chime, the queue's position prompt when uploaded, then
  the place as digits when `call_queue_prompt_location` has `0.wav` to `9.wav`, else a pip a place (a long tone past 9).
- **Offers.** Every `call_queue_tick_seconds` the engine asks `tick#QueueCall`. The waiting calls are offered oldest
  first to the queue's free agents (`CallQueueAgent`: logged in, not paused, not in a call, out of wrap-up, not ringing
  for another queue call) by its strategy: the agent idle longest (since their last queue call ended, else since they
  logged in), round robin (after the last one offered) or all at once. An offer rings the agents' tabs and phones as
  any call does, with a `CallUserLog` row (`CurQueueAgent`); the first answer wins. An offer that rings `ringSeconds`
  ends (the agents rung missed it) and the next agent is offered; one who missed the call is offered it again only when
  nobody else is free. An agent who declines ends their offer at once: the call waits on (it does not go to voicemail)
  and the next tick offers it to another agent, the one who declined only when nobody else is free.
- **Locks.** A tick, a call entering and an agent's status change lock the queue's row first, then a call, then its
  entry, as an answer locks the call, then the entry; so two ticks of one queue never wait on each other. A tick that
  fails (the services unavailable for a moment) is tried again on the next one, up to three in a row, before the call
  ends.
- **After the call** the agent who answered is in wrap-up for `wrapUpSeconds` and skipped.
- **Limits.** A call that waited `maxWaitSeconds`, one that finds `maxLength` calls waiting, and one outside the queue's
  schedule go to its fallback (a `CallTarget`, as an IVR option's). A caller who hangs up abandons the entry.
- **Agents** log in, pause, resume and log out in Calls, Queues (`set#QueueAgentStatus`), which shows each queue's calls
  waiting, the longest wait and the agents free; System, Voice, Queues shows the same for every queue and each agent's
  state.

## Transfer and merge

Every call is one `CallSession`; these move legs and people, not media (§5.1). `CallTransferServices`, from m-call's
Hold and its More menu (Transfer, Add a participant, Merge another call, Make it a meeting):

- **Blind transfer** (`transfer#Call` mode `blind`): the call's outside party (its answered phone leg) goes to a user, an
  outside number or a queue, and the user's leg leaves at once (`CerTransferred`). The far end hears ringback until the
  target answers; a target that does not answer in `call_transfer_ring_seconds` (the engine's timer, then
  `expire#CallTransfer`), or declines, missed it, and the far end goes to the target user's voicemail (a number's: the
  call ends). A transfer to a queue puts the far end in it at once. The timer of a transfer that is over (answered, or
  the call moved on to a queue or another transfer) does nothing.
- **Attended transfer** (mode `attended`): the far end is put on hold (hold music, not heard by the others), the target
  rings in the same call, the user talks to them, then `complete#CallTransfer` (the target answered: the far end comes
  off hold with them and the user leaves) or `cancel#CallTransfer` (the target leaves, the far end comes back).
- **Races.** A call that still rings is not transferred. A request names the call's `revision`: a stale one is refused,
  and so is a second transfer while one is open (`CallTransfer`), so two cannot both win. The target answering and the
  far end hanging up are each one transaction on the call's row: if the far end hangs up first, the transfer is
  canceled, the call ends and the target's answer finds it ended; if the target answers first, the transfer completes
  and the call goes on until either hangs up. Two outside parties left alone by a transfer to a number stay bridged
  until one of them hangs up.
- **Who.** A user transfers, adds to and merges only calls they are in (an answered leg). An external user (the
  framework permission `EXTERNAL_USER`) neither transfers nor adds, as they start no call to people of their choosing,
  and is offered no queues.
- **Add a participant** (`add#CallParticipant`): a user rings in the call (a direct call becomes a group call first), or
  a number joins it as one more phone leg. An end-to-end encrypted call adds no phone legs.
- **Numbers** a transfer or an add calls are calls the user places: their outbound permission, the deployment's and
  their international and premium permissions, emergency numbers refused, and their limits at once and per day, which
  count the numbers they added or transferred to (`CallLeg.dialedByUserId`) with the calls they placed and forwards; the
  number presented is the one the call came in on, else the user's first caller ID. One of our own numbers goes to the
  user or queue it routes to.
- **Merge** (`merge#Calls`): another call the user is in moves into this one, which becomes a meeting; its people and
  legs move (a browser joins the new room with the same leg, a phone leg is moved by its engine: the control `move`),
  its own legs on hold come back, and the other call ends (`CerMerged`). Calls on two media nodes merge only with
  cascading media nodes on (`rtc_cascade_enabled`, [WebRTC](WebRTC.md#cascading-media-nodes)): the other call's phones
  stay on their node, which relays to this call's node; without it they are refused (`differentNodes`). **Make it a meeting** (`convert#CallToMeeting`): the call goes on as long as
  anyone is in it.
- **A desk phone's transfer.** A registered phone's blind transfer (REFER, RFC 3515) is checked before it is
  answered: its user transfers the call to the `Refer-To` user (a username, or a number) as in the browser
  (`CallTransferServices.refer#Call`), and only when that went is it answered 202, then a NOTIFY of `200 OK`. One the
  services refuse (a number the fraud policy keeps out, a user the `call-target` hook refuses), or that has no answer in
  10 seconds, is answered 403 and the phone keeps the call. A phone's attended transfer (REFER with Replaces, RFC 3891)
  is refused (501): transfer from the browser instead.
- **New legs hear the notices.** A leg that joins by a transfer, an add or a merge gets the recording and captions
  notices as a late joiner does: a browser leg when it binds, a phone leg when it answers (`notify#PhoneLeg`, and a 1 to
  acknowledge under `call_recording_ack_required`).

**Screen pop.** When a phone call rings a user or an agent, their ring bar (and m-call) asks `get#CallContext`, which
calls the apps' `call-context` providers as that user (`ShellServices.call#Providers`: each read in its own transaction,
one that fails left out) with the caller's number, and shows who it is with links to their records. IronStone's provider
matches its parties' phone numbers, under the Comms app's organization filters, and lists their open requests.

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
native-calls path of §7.2) register with the SIP engine, which is their registrar (plans/archive/Comms-Framework.md §5.2
"Registrar for phones", V4). It is off unless `sip_registrar_enabled` is true, even with the listeners on; until then a
REGISTER is refused (403) without a challenge, so a phone pointed here is not blocked for failed digests, and phones may be added but cannot register or ring.

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
  keepalives, which the engine answers. Such a binding keeps its expiry. Over UDP a phone may send a STUN Binding
  request on the SIP socket instead (RFC 5626 §4.4.2): it is answered there with the address it came from
  (XOR-MAPPED-ADDRESS, no authentication, `moqui-sip` `stun.rs`), before the SIP stack sees it.
- **Short expiry.** A phone behind a NAT (its Via does not name where it sends from) without Outbound gets at most
  `sip_registrar_nat_expires` (60 seconds), so its own REGISTER keeps the mapping open.
- **RTP latching.** A phone's RTP is answered where it comes from. With SRTP, only an authenticated packet moves it.
  With plain RTP, packets are taken from the SDP's address or from the address the phone signals from (its NAT's,
  when its SDP names its private address), on any port until the first one latches; then only from that source.

**Calls to a phone.** When a call rings a user (a call from a colleague or a browser, or a call to their number
through a trunk), each of their phones registered with the call's node gets a leg (`CltSipDevice`, offered), and the
engine forks an INVITE to every live binding of it, beside the ring in their tabs. With cascading media nodes on
(`rtc_cascade_enabled`), a phone registered with another node rings there too: its leg's media is on that node
(`CallLeg.mediaNodeId`), whose room relays to the call's node.

- The offer is Opus, G.722 (wideband desk phones; 16 kHz audio on its 8000 RTP clock) and G.711, with
  telephone-event; SRTP to a binding over TLS, plain RTP otherwise.
- The first answer wins, through the call services' state machine: the answering phone's leg is the user's in the call,
  and the other offers end. Their tabs stop ringing, and the other bindings and phones are canceled; a phone that
  answers after is ACKed and ended with BYE. A tab that answers first cancels the phones the same way.
- A decline on any of them declines the user's ring on all of them: in a tab, or a phone's 603 Decline or 600 Busy
  Everywhere (`EndReason::Declined`, `note#PhoneLeg` `declined`). A phone's 486 Busy Here (on another call) ends only
  that phone's leg (`busy`): the user's tabs and other phones ring on. A phone whose reject key sends 486 stops only
  itself; set it to send 603 (most desk phones call it "reject with decline") for a decline everywhere.
- A phone rings as long as the call may ring (`ringSeconds`, from the call's ring expiry; at most 5 minutes). With a
  voicemail box, tabs and phones ring together for the box's rings; then the call goes to voicemail and every phone
  still ringing is canceled. A decline that leaves nobody rung sends the call to voicemail at once ([Voicemail, When a
  call goes to voicemail](Voicemail.md#when-a-call-goes-to-voicemail)).
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
  dropped while the engine runs `sip_max_transactions`; TCP and TLS connections past `sip_max_connections`, or past
  `sip_max_connections_per_source` from one source, are closed
  ([Settings](#settings); moqui-lib `source_guard.rs` `RequestGate`). Requests in a dialog and responses always pass.
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
  - A user's calls count the numbers they forward to, add to a call or transfer a call to.
  - Calls placed at once cannot pass a limit together: placing a call, a forward, a transfer to a number and an added
    number each lock one row (`CallLimitLock`) until their transaction ends, then count the call rows, which that
    transaction makes. A call refused or never placed rolls back; a forward or added number the limits or policy refuse
    is closed `CerRefused`, which no limit counts.
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

## Behind an edge node

A local node behind NAT takes its trunks and outside phones through its edge node (plans/Edge-Node.md §5.4 "Voice
(SIP)", E8; [Run-Deploy](../Run-Deploy.md#a-local-server-with-an-edge-node)). The edge node runs the SIP listeners,
the RTP and the call media; the local node keeps the numbers, the users, the call state and the boxes.

- **Setup.** The SIP properties above go in the edge node's `.env` (`sip_port`, `sip_tls_port`, `sip_host`,
  `sip_public_address`, `sip_registrar_enabled`); the local node needs none. Trunks, numbers and phones are set up on
  the local node. The edge node gets the trunks and the policy by replication (set `voiceTrunks`, `authJson` encrypted
  at rest there), registers them and keeps them registered while the local node is away; it loads them again when a
  change arrives.
- **The wizard** (System › Edge Node, plans/Edge-Node.md §5.7 "As built (E10, phones)"). Discover shows the edge
  node's SIP engine, the local trunks and their numbers that go through it, its registrar and how many numbers have a
  fallback. Apply writes `edge_sip_enabled` in the local node's `runtime/.env` (on unless `false`; unpairing removes
  it). Off, the local node sends `voiceTrunks` and `voiceFallbacks` empty, so the edge node's engine registers no trunk
  and answers no number from a fallback; the edge node's `SipNode` row stays but takes no new call, no phone rings
  through a binding there (Apply removes them), and its registrar
  signs no phone in. Test checks the row, that the edge node's engine loaded every enabled trunk (its offer lists them
  with their state) and that its copy holds every fallback. The DNS step checks the SRV records of the edge node's
  `sip_domain` (`_sips._tcp` for its TLS port, `_sip._udp` and `_sip._tcp` for the other) against its names and
  ports.
- **Its node.** The edge node registers no `SipNode`: its link's Hello names its SIP addresses, its registrar's realm
  and switch, and the local node keeps the row (`edgeLinked`, `sipDomain`, `registrarEnabled`), beside its media
  node's, so its calls go there and `send#SipControl` crosses the link. Phone calls go to it also while its media row
  drains (the wizard's calls off), since its trunks and phones reach only it.
- **Routing.** The engine's services run on the local node (delegated): `receive#PhoneCall` (which waits 7 s, under the
  user agent's 10 s), the leg and trunk notes, the IVR, queues and transfers, voicemail's begin, PIN and keys. Run as
  the edge link's user `_edge_`, they act only for calls whose media is on that edge node. The local node is told how
  long the edge node waits, starts nothing past it and rolls back what ran past it, so a call the edge node gave up on
  leaves no ringing session or dial; the engine hangs up, and dials nothing into, a call whose phone it refused as
  decided too late.
- **Phones outside.** The registrar is the edge node's, without the verifiers: phones are made on the local node with
  verifiers for the edge node's realm (its `sip_domain`, which the local node reads from its `SipNode` row, as it does
  its registrar switch), and each digest answer is checked on the local node (`check#SipDeviceDigest`), which holds the
  HA1, while the edge node keeps the nonces and their counts. The local node takes an answer from the edge node only
  for that realm, with a nonce at most 330 seconds old, and each nonce count once, so an answer the edge node kept
  cannot be used again. Phones sign in only while the local node is reachable (503 while it is away, not counted as a
  failed attempt), and register again when it is back.
- **Audio.** A greeting or prompt the edge node does not have is fetched from the local node and kept; a saved message
  played by phone is fetched for the call that gave its PIN. Each box's greeting is copied to the edge node ahead, for
  the fallback.
- **While the local node is away** (decision 4): a call is answered from its number's replicated fallback
  (`PhoneNumberFallback`, set `voiceFallbacks`): ring the number's outside fallback (the user's mobile when they ring it
  with their calls and may forward to it, or a ring group's outside number) through the trunk the call came on, held
  per number to the user's limits at once and a day (a ring group's: the policy's), then
  take a message ([Voicemail](Voicemail.md#while-the-local-node-is-away)); a number with neither, and an IVR, queue or
  access number, gets 480, so the trunk's own failover can act. Call handling rules and schedules are not followed then.
- **On an all-in-one node** ([Run-Deploy](../Run-Deploy.md#an-all-in-one-node), plans/Edge-Node.md §10): one engine
  for both sides. Trunks come from the server's own config and the local node's copy. An inbound call goes by the
  number dialled: the server's own `PhoneNumber`, else the local node's fallback copy, else the trunk's owner. Phones
  go by realm: the server's `sip_domain` is checked on the server, the local node's domain is delegated to it. The
  local node's ids are keyed by node, and the fallback applies only to its numbers.

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

- **Call handling.**
  - A forwarded mobile that its carrier's voicemail answers wins the call: there is no "press 1 to accept" yet.
  - A forward presents the number the call came in on, not the original caller's.
  - A calendar-based schedule from a provider (BOSS's calendar) is not built; schedules are hours and dates.
  - Not checked with a live provider: forwards and simultaneous ring are tested against the services and the fake trunk.
- **IVR, queues and transfer** (V5).
  - No text to speech: prompts are recordings the admin uploads, or tones; queue places are spoken only with uploaded
    digit prompts.
  - A desk phone's attended transfer (REFER with Replaces) is refused; transfer attended from the browser.
  - A transfer moves a phone call's outside party; between users in the browser, add the person instead.
  - Merging calls on two media nodes waits for cascading SFUs (W7).
  - Queue offers are made on the engine's tick (`call_queue_tick_seconds`), so a waiting call may ring a newly free agent
    a few seconds late; there are no skills, priorities or callbacks.
  - Not checked with a live provider or a real desk phone's REFER (403 and 202 with its NOTIFY are checked against the
    tests' fake far end).
- **Registrar.**
  - Provisioning files for the common desk phones come later; a phone is set up by hand.
  - Without `rtc_cascade_enabled` a phone rings only for calls whose media is on the node it registered with.
  - An extension is a colleague's username; there are no numeric extensions yet.
  - Not checked with a real desk phone or app yet: the in-memory phones of the tests play their part.
- **Offers.**
  - An INVITE without an SDP offer (late offer) is refused (488).
  - Video in an offer is refused with port 0 (D-V3: SIP legs are audio only).
- **RTCP.** No RTCP is sent, and the RTCP that comes is dropped; SRTCP is not supported. Call quality from RTCP
  reports comes later.
- **Trunk DNS.** NAPTR records are not looked up, so a trunk's transport is the one set on it. SRV weights are an
  order, not a random share. Over UDP a dead first target costs each registration its 32-second wait before the next
  is tried. Not checked against a real provider's SRV records.
- **IPv6** is not tested.
- **Behind an edge node.** Not checked with a real trunk or phone through a deployed edge node; the fallback's forward
  is checked by unit tests only, not end to end.

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
  - trunk hosts by SRV (`resolve_tests.rs` on fixed records): the order, the fallback to the host's addresses, an
    explicit port or an address with no lookup; a registration and a call that fail over on 503; calls in taken from
    every target; TLS to an SRV target naming the trunk's host, past a target that refuses connections;
  - two phone legs mixed;
  - the registrar against in-memory phones (`ua_phones_tests.rs`): a SHA-256 registration and MD5 refused for a
    standard phone, a replayed REGISTER refused, several bindings, unregistering one or all, expiry, rotation and
    revocation, a call forked to two bindings with the first answer winning, `rport` and an Outbound TCP flow (its
    keepalive, a call over the same connection, its close), and a phone's own signed call;
  - the bindings on simulated time (`registrar_tests.rs`), G.722 negotiation and audio, and plain RTP latching behind a
    NAT (`media_tests.rs`).
- `cargo test -p moqui-host --test it sip::`: an outbound and an inbound call on a real server, through a fake trunk,
  heard by a browser stand-in; a desk phone that registers, rings for a colleague's call, answers in G.722 and is
  heard by the colleague's browser; and (V5) a call to an IVR number answered by the engine, whose caller presses 1 into
  a queue, hears its music, and is answered by the queue's agent.
- `cargo test -p moqui-host --test it edge_sip::` (about 11 s): the edge node in a child process with its SIP engine,
  registering the fake trunk set up on the local node, a call routed by the local node to voicemail and stored there,
  a desk phone made for the edge node's realm registering with its registrar and rung by a colleague's call, and with
  the link down a call answered from the number's fallback whose message arrives once the link is back.
  `./moq test framework --spec EdgeVoice`: the link-kept `SipNode`, `_edge_`'s limits, the fallbacks, a queued message
  stored in the number's box, the digest checked on the local node. `cargo test -p moqui-host --lib sip::edge`,
  `sip::fallback`: the engine config from the copies, a fallback's fields, its message without phone access.
- `cargo test -p moqui-host --features sip --lib sip::ivr`: the IVR menu on simulated time: an option, a key during the
  prompt, no key and the retries to the timeout target, a key no option has (the invalid target, or a retry), an
  extension's keys until `#`, its length or a pause, nested menus, and the tone patterns (ringback, hold, a queue place).
- `./moq test framework --spec Ivr`: admin only; DTMF routing to a user's ring, a nested menu, a voicemail box and a
  queue; the timeout and invalid targets; a menu's closed hours; a loop of menus ending.
- `./moq test framework --spec CallQueue`: admin only and agents' own status; the agent idle longest first, an offer
  ringing out (missed) to the next agent; wrap-up skipping an agent; all at once with the first answer winning; the
  wait limit and the length to the fallback; an abandoned call; the stats.
- `./moq test framework --spec Transfer`: a ringing call, a stale revision and a second transfer refused; attended
  cancel and complete; blind with the far end hanging up first and with the target answering first, and a target that
  does not answer to voicemail; international, premium and emergency refused and a domestic number dialed; the
  recording notice on a transferred number and its acknowledgement by pressing 1; add a person, an encrypted call
  refusing a number; a merge on one node and a refusal across two; a call made a meeting; a desk phone's REFER refused
  (the engine's 403) and the same REFER to a domestic number taken; a `call-target` provider (the example gallery's)
  refusing a transfer, an add and a call to a user it keeps out, with the same requests reaching another.
- `cargo test -p moqui-sip refer`: a REFER's target, and Replaces not taken; a REFER the host refuses answered 403
  with no NOTIFY, and one it takes answered 202 with a NOTIFY of `SIP/2.0 200`.
- `cargo test -p moqui-sip stun`, `…stun_keepalive`: the Binding answer (RFC 5769's mapped address, IPv6, what is not
  a Binding request left alone), and a UDP phone's keepalive answered on the SIP socket beside its CRLF keepalive.
- `cargo test -p moqui-sip pinged_with_options`: a trunk that does not register shows Reachable when it answers OPTIONS
  and Failed when three pings in a row had no answer.
- `cargo test -p moqui-host --lib sip`: the notices' tone patterns, prompt audio from WAV, prompts queued one after
  another, the registrar's settings and a phone's ring time; voicemail's ([Voicemail, Tests](Voicemail.md#tests)).
- `./moq test framework --spec Voice`: the services' authorization, fraud controls (inbound digest for a PBX trunk
  only, inbound ranges no wider than a /8 or a /32), six calls placed at once on their own threads against the limits at
  once and per day (exactly the limit placed), emergency routing, a phone leg's
  notices and its acknowledgement by pressing 1, and capabilities.
- `./moq test framework --spec CallHandling`: anonymous, selective reject, accept and forward by caller list, business
  hours and holidays across a time zone (the plan read at a given time), forward loops and the hop bound, a forward to
  the caller skipped and one to our own number routed inside, do not disturb and busy with and without call waiting, the
  forward permission and international refused, simultaneous ring to a mobile whose answer wins over the tab, and
  presence with a phone leg and a tab leg.
- `./moq test framework --spec RingGroup`: admin only; all at once with a member in do not disturb skipped and the
  first answer canceling the rest; a member's mobile winning; in order step by step to the group's box; the box shared
  by its members and refused to others; round robin across calls; a schedule and the other fallbacks.
- `cargo test -p moqui-sip ua::phones::tests::a_busy_phone_ends_only_its_ring_and_a_decline_declines` and
  `cargo test -p moqui-host --features sip --lib sip::tests`: 486 against 603 and 600, the refusal statuses, and that
  only a leg's last ring timer moves its call on.
- `./moq test framework --spec SipDevice`: the registrar off by default, credentials shown once with only verifiers
  kept, another user refused with the same parameters, rotation and revocation, a phone ringing beside the tabs with
  the first answer winning (a phone, a tab, one of two phones), a phone call to a user with phones and a voicemail box
  (all ring until voicemail, which cancels the phones; a decline on a phone goes to voicemail, a busy phone stops only
  itself), and a phone's call to a
  colleague.
