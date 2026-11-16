# Attack surface: calls, chat and messaging

Calls and their media, call links and guests, captions, recording, TURN, SIP, XMPP, voicemail, chat bridges and texts. The sockets and webhooks (`/notws`, `/callws`, `/channels`, `/xmpp-websocket`, `/xmpp-upload`) and the chat key transitions are rows in [Attack-Surface-Routes.md](Attack-Surface-Routes.md). This is one file of the attack surface map. The index, how to read a row, the defaults and the lock-down advice are in [Attack-Surface.md](Attack-Surface.md).

## Call links

`/call/<token>` is a call link's guest page. Its token is 32 random characters; only its hash is stored. The token is
refused once the link is revoked or expired. The page's transitions are `joinGuest`, `guestState` and `leaveGuest`,
without the session token. `joinGuest` takes a name of up to 60 characters and gives a 40-character credential that
lasts 4 hours (hash kept). basic-comms rate-limits it per client (`call-guest`: 10 a minute, burst 5).

- A guest's credential reaches only that call's signaling, on `GET /callws`. It is never in a URL.
- `guestState` shows a waiting guest nothing but their admission. Once admitted, a guest sees the names of those in the
  call. They never see the room, its members or its history.
- A guest leg can negotiate media only once admitted (`bind#GuestConnection`).
- Revoking the link, or removing or denying the guest, ends the credential and closes the guest's legs.
- A link made end-to-end encrypted (W8) takes no guest (`encryptedNoGuests`): a guest has no crypto device. The page says
  so from the token's route context (`encrypted`).

## Call captions

Call captions (W6, [WebRTC](../interfaces/WebRTC.md#captions)) send call audio out to a speech provider, so they are off
unless `call_captions_enabled` is `true`. `start#Captions` and `stop#Captions` (the Calls screen's `startCaptions`,
`stopCaptions`) need a signed-in user with an answered leg in the call; anyone else gets `notFound`. An end-to-end
encrypted call is refused. Every leg, a late joiner's and a guest's too, gets the notice naming the provider before its
audio is captioned. The provider is the `transcribe` model binding, reached through `moqui-llm`'s address guard (a
private address needs `allow-private-network`), counted against the quota of the user who turned captions on, and kept
as an `LlmCall`. `org.moqui.call.SpeechServices.transcribe#Audio` and `get#SpeechProvider` are `allow-remote="false"`:
`transcribe#Audio` reads any `audioLocation` it is given, so its caller decides what may be sent out. Captions are not
stored unless `call_captions_store` is `true`.

## Call media

Off unless `rtc_media_port` is set, and only in a binary with the `rtc` feature (Maximal, Enterprise, Debug). One UDP socket on `rtc_media_host` (127.0.0.1 by default); browsers are told `rtc_public_address` (the bound address by default; a wildcard bind without it does not start). It carries ICE (STUN) and DTLS-SRTP for every browser leg (`crates/moqui-rtc`, on str0m, ICE-lite). A datagram goes to the peer whose ICE session (its ufrag and password from the SDP exchange over the authenticated socket) accepts it; anything else is dropped. A peer exists only after its leg's `join` was bound by the call services and reached the media node; leaving, a newer generation of the leg, the call's end (`send#MediaControl`) and ICE consent loss remove it. At most 200 rooms and 50 legs a room on a node; SDP over 64 KiB is refused. Media is hop by hop: the SFU decrypts and re-encrypts each stream, except the frames of an end-to-end encrypted call (below), which it forwards as they come. The node registers itself (`CallMediaNode`) every 20 seconds; a call whose node stops registering ends.

Cascading media nodes (`rtc_cascade_enabled`, off by default) add relay links between the media nodes of a call on the
same UDP sockets: one str0m peer a room on each side, DTLS-SRTP like a browser leg, negotiated only by `rtc-signal`
messages on the cluster's client-certificate HTTPS transport (kind `relay`), so only a cluster member opens one. The
origin takes at most 8 links a room and 64 in all, and refuses more (`tooManyRelays`); a message of an older link is
dropped, a room that ended takes no new link, and a link not connected within 10 seconds is dropped. A leg node forwards
to the origin only its own legs' tracks; the origin decides nothing a leg node asks beyond the layers it sends it. Any
cluster member may open a link for any room id: the cluster's members are trusted alike.

ICE-TCP (`rtc_media_tcp_port`, off unless set) takes TCP connections on the same address: at most 1,000. Each carries
RFC 4571 frames of up to 2,048 bytes into the same SFU, which accepts only what a peer's ICE session takes.

End-to-end encrypted calls (W8, [WebRTC, Encrypted calls](../interfaces/WebRTC.md#encrypted-calls)) open nothing new.
Each browser SFrame-encrypts its frames (RFC 9605, AES-128-GCM on WebCrypto, in an `RTCRtpScriptTransform` worker) with
a key of its own per call and epoch. It sends that key to the other browsers' crypto devices as Olm to-device messages
over the `/ChatKeys` endpoints (C7 core), so the server holds and relays only Olm ciphertext; no chat Megolm session is
used. What the server and the media nodes still see: the RTP headers and header extensions (the audio level for the
active speaker), each frame's size and timing, the first 10 bytes of a VP8 keyframe and the first 3 of other VP8 frames
(kept in the clear so the SFU finds keyframes and sizes; authenticated as SFrame metadata), the SFrame header (the
sender's key index and epoch, the counter), and who is in the call with which device. The roster the browsers send keys
to is the server's (`get#Call`): a server that adds a device of a user in the call gets that leg's keys. A browser takes a
key only from the device the roster names for its leg (the Olm sender, which the server cannot forge), with that leg's key
index in its KID (refused while another leg has that index), and decrypts a track only with the keys of the leg it comes
from, each counter once (a replay window); a frame that does not decrypt is dropped, never played. A sender sends only
with keys it made, so a server-made roster cannot make two senders share a key and its nonces. A call page shows each
device as unverified (emoji and QR verification are in My devices, C7, but the call does not read their trust yet). The mode is set at the start (`requireE2ee`, a room's or a link's policy) and never lowered, and a
leg of such a call binds only with a crypto device of its user (`noCryptoDevice`); recording, captions, phone legs, desk
phones, forwarded numbers, guests and merges are refused. The browser's crypto store is encrypted at rest with a store
key the server gives only to its signed-in user (see Chat keys).

## Call recording

Off unless `call_recording_enabled` is true ([Call recording](../interfaces/Call-Recording.md)). It opens no port. Only a
host of the call starts, pauses, resumes or stops it (`org.moqui.call.CallRecordingServices`). The recorder runs on the
media node, takes frames from a bounded queue and spools under `runtime/tmp/call-recording/`. `store#RecordingFile`
(internal, `allow-remote="false"`) refuses a spool path outside that directory and a location outside
`dbresource://moqui/call/recording/`. The files are read only through `get#RecordingFile` by segment id, from the
BasicComms Calls `Recordings` screen's `recordingFile` transition: the one who recorded it and the call's hosts get the
file, anyone else "not found" (an empty JSON answer) for the same id. No route serves those `dbresource://` locations
to users, and an app's record links (BOSS) go through the same services. The guest page's `ackRecording` and
`withdrawRecording` take the guest's credential and act on its own leg only. Every leg gets the notice, and each is
logged (`CallRecordingConsent`, with every earlier acknowledgement and withdrawal in `CallRecordingConsentHistory`).
The files count toward the owner's personal drive quota: a full drive refuses a start, and a recording that fills it
stops, so recording cannot fill the database past the quota by more than a segment. Retention deletes the files and
rows (`expire_Recordings_hourly`).

## TURN

The embedded TURN server (`crates/moqui-rtc` `turn/`, host `rtc/turn.rs`) runs on the media node. Each listener is off
unless set: `turn_port` (UDP and TCP), `turn_tls_port` (TLS), and 443 on `turn_tls443_host`. TLS uses the
`runtime/cert` certificates. Relays are UDP sockets in `turn_relay_ports`. Its rules:

- **Credentials** are short-lived TURN REST ones: `<expiry>:<legId>`, HMAC-SHA1 with `turn_auth_secret` (generated
  when unset; is-secret). They go only to an admitted leg (`get#IceServers`, `get#GuestIceServers`). An expired one
  is refused, and so is one of a leg that left (revoked by the media node). On an edge node a local node's are
  `<expiry>:<legId>@<pairing>`, signed with that pairing's secret and taken only while it is active (E16).
- **Relay peers** are only the media nodes' media ports (and `turn_allowed_peers`, by address and port). A permission
  to any other address, or a channel to any other address or port, is dropped before the server sees it, and nothing
  is relayed to or from one, another port of a media node's address included. The exception is an XMPP user's own
  credential (`xmpp.<userId>`, from `XmppServices.get#IceServers`, XEP-0215), for calls between XMPP clients: it may
  relay to public unicast addresses too, never to loopback, private (RFC 1918, unique-local), link-local, shared
  (100.64/10), multicast or documentation ranges, nor an IPv6 address that carries an inside IPv4 one (6to4 of one,
  IPv4-compatible, NAT64, Teredo), nor another port of a media node's address or of the TURN listeners' own address
  (a relay there would come from the server itself, past a firewall that keeps the outside off those ports).
- **Limits:** two allocations per credential, `turn_max_allocations` in all, and `turn_max_kbps` each way per
  allocation; excess data is dropped. A stream buffers at most 64 KiB without a whole message. No TCP relays to
  peers are offered.

coturn can take its place ([TURN](../interfaces/TURN.md)), with the same secret and a deny-all, allow-media-node
peer rule.

## SIP

The SIP engine (`crates/moqui-sip` on rsipstack, host `src/sip/`) runs beside a media node, only in a binary with the
`sip` feature (Maximal, Enterprise, Debug). Each listener is off unless set: `sip_port` (UDP and TCP on `sip_host`),
`sip_tls_port` (TLS on `sip_tls_host`, the `runtime/cert` certificates). Each phone leg has a UDP RTP socket in
`sip_rtp_ports` on `sip_rtp_host`. An exposed SIP listener is scanned within hours, so its rules are release gates
([SIP](../interfaces/SIP.md) "Fraud controls"):

- **Who is answered.** An INVITE from a trunk's host (each address of its SRV targets, for a trunk set up by name
  with no port, looked up again at each registration and ping) or listed signaling addresses is a trunk's call; any other
  INVITE or REGISTER is challenged with SIP Digest (SHA-256 and MD5, nonces that expire after 5 minutes and carry
  random bytes and a MAC under a per-process key, nonce counts that must go up). Only a PBX trunk's credentials
  (profile "PBX on a private network") answer it: a provider trunk's are the provider account's password, so a digest
  with them is refused and counted as a failure, and a provider's calls come only from its addresses. A listed address
  range is at most a /8 (IPv4) or a /32 (IPv6); the services refuse a wider one and the engine ignores it. Other requests from addresses that
  are not a trunk's are not answered. Without the registrar, REGISTER is refused (403) without a
  challenge, so it counts no failures. Over UDP a
  source address can be forged, so a trunk taken by its address is best on TLS (the default) or TCP.
- **The registrar (V4).** Off unless `sip_registrar_enabled` is true, even with the listeners on. With it, a REGISTER
  and a phone's INVITE answer the same challenge with a desk or soft phone's credentials: a generated username and a
  22-character random password that is shown to its user once and never stored. Only its digest verifiers are kept
  (`SipDeviceVerifier.ha1`, encrypted, per algorithm and realm): SHA-256, and MD5 only for a device of the legacy
  profile, so an MD5 answer for a standard device fails. A failed or replayed answer counts against its source as any
  failed digest; no device or account is locked. Each REGISTER or signed INVITE looks the username up once
  (`get#SipDeviceAuth`), within the per-source rate. Rotation deletes the old verifiers and revocation the device's,
  and either ends its bindings and its calls on every engine. A binding is the flow the phone opened (its transport
  and source address and port); calls to the phone go back over it only, never to the address its Contact names, and
  a stream connection that closes ends its bindings. A device has at most 10 bindings, and expiry is held to
  `sip_registrar_min_expires`..`sip_registrar_max_expires`. A phone's own call is placed as its user through the same
  checks as the browser's (`place#PhoneCall`: outbound permission, caller ID, international and premium, limits,
  emergency first), so a phone's credentials give no more than its user has. Plain RTP from a phone is taken from its
  SDP's address or the address it signals from (its NAT's), on any port until the first packet latches.
- **Failed digests block the source.** `authFailureLimit` failures in `authFailureWindowSeconds` from one address
  block it for `authBlockSeconds` (5 in 10 minutes, for an hour, by default): its datagrams and stream messages are
  dropped before a transaction is made, nothing is answered, and the block is kept in `VoiceAuthFailure` across
  restarts (the engine counts failures in memory and writes only the block). An IPv6 source counts as its /64. The
  engine tracks at most 50,000 sources; when full, the ones not blocked go first, so rotating addresses does not lift
  the blocks. No account is locked or counted.
- **What an incoming call reaches.** Only a number in `PhoneNumber` rings anyone, through the call services'
  ringing; the SDP must be audio with a codec in common, and SRTP when the trunk's profile requires it (488
  otherwise).
- **Outbound.** Calls go out only from `send#SipControl` (the `rtc-signal` cluster topic, kind `sip`, to that node),
  which only the voice services send, after their checks (caller ID grant, outbound enabled, international and
  premium off by default, concurrency and daily limits, the emergency path). With home country 1, the area codes of
  the plan's other countries (the Caribbean's, Bermuda's) are international. The engine checks the number's class,
  the emergency trunk and the concurrency limit again before an INVITE goes out. A display name or extra header with
  a line break or a quote is cleaned or left out, so it cannot add headers (a `P-Asserted-Identity`) to the INVITE.
- **Forwards to outside numbers (V3).** A call that comes in can make the server dial out: a user's forwarding rules,
  their simultaneous ring to a mobile, and a ring group's fallback number. That is toll fraud's other door (a caller
  rings in to be bridged to a premium or foreign number), so a forward is a phone leg under the same checks as a call
  the user places (`VoiceServices` `forwardCheck` and `dialForward`): the user needs outbound calls and
  `VoiceUserSetting.forwardEnabled` (off by default, set only by a voice admin); international and premium need the
  deployment's and the user's permission; emergency numbers are never a target; the user's limits at once and per day
  count their forwards with the calls they place, and the deployment's calls in a day count every forward; and the
  engine checks the class and its calls at once again before the INVITE. A ring group's fallback number is set by a
  voice admin and checked against the deployment's policy. The caller ID of a forward is the number the call came in
  on, never one the caller supplied. A forward to one of this deployment's numbers is routed inside (no hairpin through
  the trunk), a forward back to the caller's number is skipped, and a call takes at most `call_forward_max_hops`
  forwards, with a user or group already in its plan skipped, so rules cannot loop a call or fan it out without bound.
  Caller lists and rules are the user's own (another user's are "not found"); the rule's target is checked when it is
  saved and again when a call uses it.
- **Transfers and adds to outside numbers (V5).** A user in a call can make the server dial out by transferring the
  call's outside party to a number, or adding a number to the call; a desk phone can ask for a transfer with REFER.
  Each is a call the user places, under the same checks (`CallTransfers.js`, `VoiceServices` `forwardCheck` with
  `transfer` and `dialForward`): outbound calls enabled for the user, international and premium only with the
  deployment's and the user's permission, emergency numbers refused (they are dialed directly), the user's limits at
  once and per day (which count every number they added or transferred to, `CallLeg.dialedByUserId`, so adding numbers
  one by one does not get past them), the deployment's calls in a day, and the engine's own check of the class and its calls at once
  before the INVITE. The limits are counted under one lock (`CallLimitLock`), so calls placed at once cannot pass them
  together. A REFER is taken only from a registered phone's answered leg in a call, as its user; its target is
  a username or a number under those checks, checked before the phone is answered (403 when refused, so it keeps the
  call); an attended REFER (Replaces) is refused. A person called, transferred to or added must pass the apps'
  `call-target` providers (IronStone: someone of the user's organizations), answered as a user that does not exist when
  refused. Only someone in the call (an
  answered leg of theirs) may transfer, add or merge; a call that still rings is not transferred; a stale revision or
  a second open transfer is refused. An external user (`EXTERNAL_USER`) neither transfers nor adds. An end-to-end encrypted call adds no phone legs. An IVR option, a menu's or a
  queue's fallback to a number is set by a voice admin and checked against the deployment's policy when it is saved
  and when a call uses it; a menu loop ends after 20 menus, and a queue's fallback chain after 10.
- **Credentials.** Trunk credentials are `VoiceTrunk.authJson`, encrypted; they answer only the trunk's realm and
  allowed algorithms. TLS to a trunk verifies its certificate against `sip_tls_ca_file` (the system bundle by default),
  for the trunk's host as set up, never an SRV target's name, so a changed SRV record cannot point a trunk at a server
  that does not hold the provider's certificate.
- **Media.** SRTP (AES_CM_128_HMAC_SHA1_80, SDES keys over TLS) unless the trunk's profile is a private-network PBX;
  a packet that fails its tag or replay window is dropped. RTP goes back where the leg's authenticated packets come
  from. Plain RTP is taken only from the SDP's address and, once the first packet latched, only from that source;
  anything else is dropped, so a guessed port neither injects audio nor takes the call's audio. RTCP is dropped.
  rsipstack logs whole SIP messages (SDES keys, digest answers) at INFO, so the host keeps only its warnings.
- **Floods.** rsipstack has no caps of its own, so the engine's transport inspector holds new requests (outside a
  dialog) to `sip_request_rate` a second per source (`sip_request_burst` at once; 20 and 100 by default, an IPv6
  source as its /64), drops them while `sip_max_transactions` (2,000) run, and closes TCP and TLS connections past
  `sip_max_connections` (500), or past `sip_max_connections_per_source` (100) from one source, so one source cannot
  hold the connections the registrar's phones keep open. What is over is dropped unanswered; requests in a dialog and responses pass, so calls
  going on are not cut. Answers sent before an incoming call's decision expire after 30 seconds (1,000 at most).
- **Limits.** A leg's jitter buffer holds 10 packets; a leg's consumer queue a second of frames; datagrams over 2 KiB
  are cut. The engine registers its node (`SipNode`) every 20 seconds.

## XMPP

The XMPP server (`crates/moqui-xmpp`, host `src/xmpp/`, [XMPP](../interfaces/XMPP.md)) runs only in a binary with the
`xmpp` feature (Maximal, Enterprise, Debug) and only when `xmpp_domain` is set. Each listener is off unless set:
`xmpp_port` (STARTTLS, required before anything else) on `xmpp_host`, `xmpp_tls_port` (direct TLS) on
`xmpp_tls_host`, and `/xmpp-websocket` with `xmpp_websocket_enabled`. Local users only: no server-to-server.

- **Sign-in** takes app passwords only, never the account password (which would count toward the account's lockout
  and cannot carry a second factor): SCRAM-SHA-256 and SCRAM-SHA-256-PLUS against the verifiers stored when the
  app password was made (salted, 10,000 iterations), with tls-exporter channel binding where the connection has its
  own TLS session and a downgrade (`y` where the server binds) refused; PLAIN over TLS only; FAST tokens (XEP-0484)
  bound to the client's user-agent id, kept encrypted, expiring (`xmpp_fast_token_days`), and without channel binding
  refused unless their count grows; a token goes with the app password it was issued under. An unknown user gets a salt
  derived from the name and `xmpp_scram_decoy_key` (is-secret, generated when unset), so the exchange does not tell it
  apart, and its salt stays the same across restarts and nodes. A disabled account cannot sign in, and each service
  call logs the user in again by name. A session checks every minute that its app password is not revoked and the
  account may sign in, and ends when not.
- **Failures** are counted per source address (moqui-lib `source_guard.rs`, as the SIP engine counts failed digests):
  5 in 10 minutes block the address for an hour, a /64 for IPv6; the account is never locked. Over WebSocket the
  address is the client a trusted proxy names. At most 20 connections from one address at once and 10,000 in all
  (TCP and WebSocket); a connection not signed in within a minute is closed; a stanza or stream header over 256 KiB,
  or nested over 32 deep, ends the stream; a session whose pushes back up (512) is closed. Stream management keeps at
  most 1,000 unacknowledged stanzas and 10 detached sessions a user; PEP 200 nodes a user and 256 items a node;
  uploads 60 slots a user an hour.
- **Authorization** is the user's: each stanza becomes a service call as the signed-in user, through
  `XmppClientServices` (granted to `ALL_USERS`), and every service checks the room (`get#RoomAccess`) or works on the
  user's own rows. An external user (`EXTERNAL_USER`) learns only the usernames of the people of their own rooms.
- **XML** is parsed by rxml, which refuses DTDs, entity declarations and processing instructions.
- **End-to-end encrypted Moqui rooms** (C7) are not served; OMEMO messages are kept as they came, unreadable to Moqui,
  never in a bridged room, with no text in snippets, search, notifications or agents. PEP items (OMEMO bundles hold
  public keys) are read by access model: `open` any local user, `presence` the people the owner shares a room with,
  `whitelist` the owner.
- **TURN for XMPP calls** (XEP-0215): off unless `xmpp_turn_enabled` is true; while off, external services are not
  advertised and a request gets an empty list. On, a credential keyed to the user (`xmpp.<userId>`) may relay to public
  unicast addresses only, never loopback, private, link-local, shared or documentation ranges ([TURN](#turn)); it is a
  relay to the internet within the TURN quotas, as a public TURN service's is. An external user (`EXTERNAL_USER`) never
  gets one, and a user at most `xmpp_turn_per_hour` (30) an hour (counted on the distributed cache
  `moqui.xmpp.turn.issued`); a refusal is the same empty list.
- **Push** (XEP-0357) registrations are stored, with their app server's secret encrypted; a notification has the message
  count only, and goes nowhere until federation (C3b).

## Voicemail

Voicemail ([Voicemail](../interfaces/Voicemail.md)) opens no port: it runs on the SIP engine above, for calls to a
user's number that nobody answers, and on the access number (`PnrVoicemail`) when an administrator sets one.

- **The PIN over the phone.** A caller who presses `*` in a greeting, or calls the access number, can enter a box
  number and a PIN. `check#VoicemailPin` (internal) checks it against the box's argon2 hash and limits failures per
  box (5 in 15 minutes, then 15 minutes refused from any caller) and per source (the caller's number, 10 in an hour,
  then an hour). The source is a caller ID the caller can fake, so the box's limit is the one that holds: about 240
  guesses a day, which tries every four-digit PIN in six weeks and every six-digit one in eleven years, so a new PIN
  has at least `voicemail_pin_min_digits` (6 by default) digits. A box that does not exist, has no PIN, or is blocked is refused in
  the same words after the same hash work and is counted the same way, so the answers say nothing of which boxes
  exist. Three refusals end the call. The PIN is never logged, and weak ones (one digit repeated, a run) are refused
  when set. Accepted, the caller hears only that box's new messages, and its keys act only on them (`act#PhoneVoicemail`
  checks the box).
- **The audio route.** Messages and greetings are under `dbresource://moqui/call/voicemail/`, which no route serves.
  The audio is read only through `get#VoicemailAudio` and `get#VoicemailGreeting`, from the Calls `Voicemail` and
  Settings `Phone` screens' transitions: the box's owner gets the file, anyone else "not found" (an empty JSON answer),
  the same as for an id that does not exist. The engine writes only through `store#VoicemailFile` (internal), which
  takes a spool path only under `runtime/tmp/voicemail` and a location only under the voicemail root.
- **Uploads.** A greeting upload is at most 10 MB, checked as WAV or Ogg Opus by its first bytes, then decoded and
  re-encoded by the server (`convert#GreetingAudio`, internal, which reads and writes only under the greetings' root);
  a minute is kept.
- **Mail out.** The email notice carries only a link unless the owner turns on the audio or the transcript in it; the
  screen warns that mail cannot be recalled. Transcription sends the audio to the speech provider only when
  `voicemail_transcribe` is on.
- **Limits.** A message is at most `voicemail_max_seconds` (120; at most 600), spooled to disk as it is recorded; one
  under a second is dropped.

## Chat bridges

The bridge layer ([Chat bridges](../interfaces/Chat-Bridges.md), C1) opens no port and routes no endpoint: a network
adapter (C4 on) brings its own transport, and C1 ships none.

- **Input.** `ChatBridgeServices.ingest#Event` is `allow-remote="false"`: only code in the server (an adapter, after its
  own checks of a webhook's signature or a gateway's session) calls it. It authenticates the bridge by its inbound token
  (`check#BridgeToken`: the SHA-256 against `ChatBridge.inboundTokenHash`, in constant time; a bridge with no hash takes
  none) and refuses the whole batch, storing nothing, for an unknown or disabled bridge, a wrong token, a malformed
  transaction id, more than 200 items or 1 MB, or an item that names a local user. An item posts only as an external
  actor (no login, no `UserAccount`) into a room an administrator mapped to that bridge (`noMapping` otherwise), as a
  member a moderator has not taken out, edits and deletes only its own messages, and carries plain text only (escaped to
  HTML; files are refused). A puppet link to a local user is provenance, never authority. Its messages fetch no link
  previews, reach a room's agents only under the room's `agentsReadExternal`, and never start the framework's agents.
- **Output.** A room's messages go only to the networks an administrator mapped it to; a link to an external room grants
  no rights. Delivery rows keep no message text.
- **Administration.** `find#Bridges`, `store#Bridge`, `store#RoomBridge` and `end#RoomBridge` need `ChatBridgeAdmin`.
  `ChatBridge.configJson` is an encrypted field and never returned; the inbound token is kept only as its hash.
  `retry#Delivery`: the message's sender, a moderator of the room, or `ChatBridgeAdmin`; anyone else is told "not found"
  or "not allowed" with the same parameters.
- **Internal services.** `send#Message`, `edit#Message`, `react#Message` and `redact#Message` take `senderActorId` only
  with authorization disabled. `get#LocalActors`, `get#ActorNames`, `get#RoomBridges`, `check#BridgeSupport`,
  `get#BridgeCapabilities`, `wake#BridgeDelivery`, `run#BridgeDeliveries` and the
  `ChatBridgeServices` internals are `allow-remote="false"`.

## Conf-level shell providers

A component's MoquiConf.xml may register a provider of the hooks the framework asks for whoever acts (`screen-facade/shell-providers`; only `sms-consent`, `CONF_PROVIDER_HOOKS`; any other element is logged and left out). call#Providers asks it after the apps' providers for every caller, the system with no user included, with authorization disabled, since the deployment's conf names it: such a provider answers only what its hook asks; conf providers are always asked beside an app's provider of the same name, a "no" from any provider wins, and a provider that fails refuses the text (Mantle's check#SmsConsent reads consent and suppression rows and answers yes or no, nothing else). A promotional text counts against its number's day of promotional texts (VoicePolicy smsMaxPromotionalPerNumber, 1000), and one with no user not against the system's own budget, so a campaign cannot use up sign-in codes. Marketing's texts (the ledger's SMS channel, MdcSms: each attempt is handed to the framework's `org.moqui.SmsServices.send#SmsMessage`; the consent answer and STOP handling are `mantle.party.MessageSmsServices` check#SmsConsent and note#SmsKeyword, both `allow-remote="false"`) are sent as the system from an SMS sending profile's number, which must be one of the organization's telecom numbers that is one of ours with a messaging bridge (MESSAGE_SENDING_CONFIGURE sets it); each text is checked for `PcpSms` consent on the send's topic and the SMS suppression before the call, and again by the framework's sms-consent hook (yes only for a party holding the number with PcpSms granted with an organization that owns the sending number and no suppression there); a STOP the framework keeps becomes a PcpSms withdrawal and a suppression (an EECA on SmsOptOut, in its own transaction), and a START ends neither; a lost answer is uncertain and never resent; no link, token or tracking in a text. PASS Email preferences may leave a text message topic, never join one.
