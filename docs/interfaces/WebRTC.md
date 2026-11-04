# WebRTC calls

Calls in the Comms app go through an SFU (selective forwarding unit) inside the moqui binary, a two-person call too.
There is no peer-to-peer mode. The design is [plans/Comms-Framework.md](../../plans/Comms-Framework.md) §5.4 and §6. The
user guide is [Comms](../tools/Comms.md#calls).

- **Media:** `crates/moqui-rtc`, on [str0m](https://github.com/algesten/str0m) (sans-IO: ICE-lite, DTLS-SRTP, simulcast,
  TWCC). The host's driver thread (`crates/moqui-host/src/rtc/`) owns the UDP socket.
- **Signaling:** `rtc` frames on the notification socket (`crates/moqui-host/src/notws/rtc.rs`).
- **Call state:** `org.moqui.impl.CallServices` and `Calls.js` (framework), with the model in
  `framework/entity/CallEntities.xml`.
- **Browser:** `webroot/screen/webroot/js/WebrootCall.js`.

## Turning it on

Calls need a binary with the `rtc` feature (Maximal, Enterprise, Debug, or `./moq build -- --features rtc`) and a media
port:

| Property | Default | |
| --- | --- | --- |
| `rtc_media_port` | empty (off) | The UDP port browsers send media to; `0` is a free port |
| `rtc_media_host` | `127.0.0.1` | The address it binds |
| `rtc_public_address` | empty: the bound address | What browsers are told: an IP, or IP:port, for a server behind 1:1 NAT. Required when `rtc_media_host` binds every address |
| `call_ring_seconds` | `45` | How long a call rings before it is missed |
| `rtc_cascade_enabled` | `false` | Cascading media nodes ([Cluster](#cluster)): a leg's media goes to the node of its own socket, which relays to the call's node |

On a single server reachable at its own address, `rtc_media_port=10000` with `rtc_media_host` set to that address is
enough. Behind 1:1 NAT, bind the private address and set `rtc_public_address` to the public one, then forward UDP 10000.
The advertised address must be reachable from the browsers. A configured address does not prove it: test a call from
outside the network. For browsers that cannot reach the UDP port, add ICE-TCP (`rtc_media_tcp_port`) and the embedded
TURN server, or coturn ([TURN](TURN.md)).

The Calls screen says when calls are unavailable (`get#Capabilities`): supported (the binary has `rtc`), configured
(this server's port is set) and active (a media server is up in the cluster).

## How a call goes

1. `start#Call` (the Calls screen, the page's call store) makes the CallSession, a leg for the caller and an offered leg
   per callee. It rings each callee's tabs on `chatUser:<userId>`, by Web Push, and by FCM to a native app as a
   high-priority data message ([Qvt-Client](../ui/Qvt-Client.md#install-and-web-push)), and appends a call event to the room.
   The callees are enabled users whom no `call-target` provider of the caller's apps refuses: an app with
   organizations keeps them to the caller's own (IronStone BOSS: the users of the parties the caller may see), and a
   refused one is answered as one that does not exist (`noCallee`). Transfer and Add (`transfer#Call`,
   `add#CallParticipant`) ask the same hook (`notFound`); without a provider anyone enabled may be reached; a provider
   that fails refuses.
2. The callee answers (`answer#Call`). The first answer wins, and the other tabs hear it.
3. Each page sends `{rtc: 'join', callSessionId, callLegId, sdp}` on its notification socket. The socket's node runs
   `bind#Connection` as the socket's user. That checks the leg and claims its next connection generation, and the frame
   goes to the call's media node with that generation.
4. The media node answers. As other people's tracks arrive, it offers them to the page, and the page answers. Video
   goes in three simulcast layers (rids `l`, `m`, `h`), and each subscriber gets the one that fits its bandwidth
   estimate.
5. Leaving (`leave#Call`, or the socket's `leave`) closes the leg. A direct call ends with either side, and a group call
   with its last leg. A socket that closes keeps its leg for 30 seconds. A reconnect joins again with the next
   generation, and the media node refuses the old connection's frames.

## Guests by link

A call link (Calls, Call links; `create#CallLink`) is `/call/<token>`, a page in the public shell for a person with no
account (`basic-comms` `CallGuest.xml`, the `call` token route). The token is shown once, and only its hash is kept.

1. The guest gives a name. `join#AsGuest` opens the link's meeting, starting one when none is going. The guest is a
   participant with no login and a guest leg. With a lobby, they wait until someone in the call lets them in (Let in,
   Turn away); a link set to "straight in" admits them at once.
2. The guest's page gets a credential that lasts 4 hours. It sends the credential in the first frame of its own socket,
   `/callws`, never in a URL. That socket takes only the rtc frames of the guest's own call and leg: no topics, no
   notifications, nothing of the room or its history.
3. Revoking the link, or removing or denying the guest, ends the credential and closes the guest's legs. The socket
   checks the credential every 30 seconds and closes when it no longer works.

The link's owner opens its meeting from Calls (Open) as its host. A meeting nobody is in or waiting for ends after two
minutes.

## Recording

A call's host can record it when `call_recording_enabled` is on: each track as sent (Opus in Ogg, VP8 in WebM) and the
mixed audio, made on the media node, with a notice to every leg and the `rtc` frame `recording`; a phone leg hears it
as a prompt ([SIP, Notices](SIP.md#notices)). See [Call recording](Call-Recording.md).

## Captions

Live captions are off unless `call_captions_enabled` is `true`, and they need a speech to text provider: a model bound
to the `transcribe` feature on an OpenAI-compatible endpoint (`POST /v1/audio/transcriptions`, such as a local Whisper
server or a hosted one), set up as for the Assist drawer's dictation ([Assist](../ai/Assist.md)). Local transcription in
the binary (`speech-local`, Whisper on candle) is not built yet.

| Property | Default | |
| --- | --- | --- |
| `call_captions_enabled` | `false` | The switch: the CC button shows only when it is `true` |
| `call_captions_provider` | `llm` | `llm` (the `transcribe` binding) or `local` (speech-local, which answers "not built in") |
| `call_captions_chunk_seconds` | `5` | The longest chunk of one speaker's audio sent at once; a pause of 0.6 s ends one sooner |
| `call_captions_queue` | `6` | Chunks waiting for the provider; past this the oldest is dropped and shown as a gap |
| `call_captions_workers` | `2` | Chunks of one call transcribed at once, on the service pool; all calls on a node together use at most half its workers |
| `call_captions_timeout_seconds` | `20` | How long one chunk may take |
| `call_captions_language` | empty | An ISO 639-1 code; empty lets the model tell |
| `call_captions_store` | `false` | `true` keeps each caption as a `CallTranscript` row; by default nothing is kept |
| `call_captions_prompt_audio` | empty | A WAV file a phone leg hears after the captions tones ([SIP, Notices](SIP.md#notices)) |

How they work:

1. Someone in the call presses CC: `start#Captions` (`org.moqui.impl.CallCaptionServices`, `CallCaptions.js`). It
   refuses with a reason code when the switch is off (`captionsOff`), the user is not in the call (`notFound`, as for a
   call that does not exist), the call ended (`callEnded`), its media is end-to-end encrypted (`encrypted`: the server
   cannot hear it, and captions never downgrade it), or no provider is set up (`noProvider`). It keeps the state in
   `CallCaptions` and tells the media node (`send#MediaControl`, `startCaptions`).
2. The media node (`crates/moqui-host/src/rtc/captions.rs`) takes the call's audio through a tap and decodes it on a
   thread of its own, never the media thread. Each speaker's audio is gated by its level, cut at a pause or the longest
   chunk, resampled to 16 kHz and sent as WAV to the provider (`moqui_core::llm_hooks::transcribe`, through
   `moqui-llm`). The call counts against the quota of the user who turned captions on, and each is an `LlmCall`.
3. Every leg gets the notice, `{rtc: 'captions', state: 'on', provider, local}`, which names who hears the audio; a leg
   that joins later gets it when it joins, a guest's too. A phone leg hears it as a prompt, when captions turn on or when
   it answers while they are on ([SIP, Notices](SIP.md#notices)). Each notice is logged for its leg, as the recording's
   is: a `CallCaptionsNotice` row each time captions turn on for every leg then in the call, and for a leg that binds or
   answers while they are on (`note#CaptionsLeg`), with the method (`CnmWeb` or `CnmVoice`), the time and the provider
   it named. Captions go to every leg in the order they were spoken:
   `{rtc: 'caption', leg, participant, name, seq, text}`.
4. A slow provider never holds the call up. When the queue is full the oldest waiting chunk is dropped and its place
   goes out as `{rtc: 'caption', leg, seq, gap: true}`. A provider error or timeout goes out once as
   `{rtc: 'captions', state: 'unavailable', reason, message}`; the call and its media go on, and the next caption that
   works brings them back.
5. Anyone in the call turns them off (`stop#Captions`, `{rtc: 'captions', state: 'off'}`). They also end with the call.

Transcription for other server code is `org.moqui.impl.SpeechServices.transcribe#Audio` (internal): voicemail (V2) and
a recording's transcript will use the same seam, and `CallTranscript` holds their text too; a recording's transcripts
are deleted with it.

## Encrypted calls

A call can be end-to-end encrypted (W8; plans/Comms-Framework.md §6.4, D-W3): each browser encrypts the frames it sends
with SFrame (RFC 9605), with keys the server never has, and the SFU forwards them as they are. The user guide is
[Comms, End-to-end encrypted calls](../tools/Comms.md#encrypted-calls).

- **Browser:** `js/WebrootSFrame.js` (SFrame on WebCrypto, page and worker), `js/WebrootSFrameWorker.js` (the
  `RTCRtpScriptTransform` worker), `js/WebrootCallKeys.js` (`moqui.callKeys`, the call-key protocol), used by
  `WebrootCall.js`. No WebAssembly: HKDF, AES-GCM, AES-CTR and HMAC are WebCrypto's.
- **Services:** `Calls.js` (the policy and the roster), `register#CallDevice`, `set#RoomCallEncryption`; the keys travel
  over [Chat keys](Chat-Keys.md) (C7 core).

**In the desktop client** ([Desktop-Client](../ui/Desktop-Client.md#calls)), the shell owns the account's calls, as it
owns its crypto device, so each call has one owner of its media: it rings (a native notification when its window is not
seen) and shows the call, and an encrypted call's keys go over its device. Its CSP allows one worker, the staged SFrame
transform (`worker-src`, answered with its own policy), and no new connect or media sources. An app window rings
silently and opens a call in the shell; `moqui.callKeys.prepare` there refuses with `elsewhere`, so an encrypted call
still fails closed rather than joining without keys.

### The policy

`CallSession.encryptionMode` is one of the enumeration `CallEncryptionMode` (by enumCode): `dtls-srtp`, hop by hop
through the SFU (every call before W8, and any call not asked to be more), or `e2ee-sframe`. Any other value is treated
as end to end, never as plain.

- **Asking for it.** `start#Call` with `requireE2ee` (New call, End-to-end encrypted); a room whose owner or moderator
  set `ChatRoom.callE2eeRequired` (`set#RoomCallEncryption`; Room settings in Chat), for every call of the room and the
  links made for it; a call link with `requireE2ee` (`CallLink.e2eeRequired`), for its meetings. Call back from History
  of an encrypted call (`find#Calls` `e2ee`) asks for it again.
- **No downgrade.** The mode is set at the start and nothing lowers it. A room's call going on that is not encrypted is
  refused to a start that asks for encryption, or in a room whose policy asks for it (`callNotEncrypted`), never
  answered in its place. A browser drops a frame that does not decrypt instead of playing it, sends nothing before it
  has a key, and opens no media for an encrypted call it has no keys for (a call it was moved to).
- **Who can take part.** A browser leg answers or joins (`answer#Call`, `join#Call`) only when it says it has
  `RTCRtpScriptTransform` (`scriptTransform`) and names a crypto device of its user (`cryptoDeviceId`: a `ChatDevice`
  with its keys uploaded, not revoked); else `unsupportedClient` or `noCryptoDevice`, and the call rings on for the
  user's other tabs. A call started from a form (Calls, New call) or a link's meeting opened from Calls names the
  device before its media (`register#CallDevice`, which the page calls). `bind#Connection` refuses a leg of an encrypted
  call that has no device (`noCryptoDevice`), and any guest or phone leg. The page itself checks first and shows why it
  cannot join (current Chrome, Edge, Firefox and Safari have `RTCRtpScriptTransform`).
- **Refused in an encrypted call:** recording (`start#Recording`: `encrypted`, said before the recording policy; Record
  is not shown), captions (`encrypted`), a phone number (`add#CallParticipant`: `encrypted`, before the number is looked
  at; `addPhoneLeg`), desk and soft phones (`ring#SipDevices`: `encrypted`, and `start#Call` does not ring them), the
  numbers of a handling plan or a user's mobile (`ringStep` drops them, logged), guests (`join#AsGuest`:
  `encryptedNoGuests`; the guest page says so from the token's route context), and merging with any call
  (`merge#Calls`: `encrypted`, two encrypted calls too, since their keys and rosters are each their own). The terminal
  twin shows the mode and no Answer.

### The roster and the keys

The call's view (`get#Call` and every push) carries `encryptionMode` and `e2ee`, and for each leg of an encrypted call
its `deviceId`, `keyIndex` (the leg's number in the call, given when it names its device) and `keyGeneration` (its
connection generation). A device named, a leg bound again (a reconnect or a reload) or a leg gone pushes the new view
(`{kind: 'call', action: 'rekey'}`, or the action of the change). The roster is the answered browser legs with a device.

1. **A key per sender.** Each page makes a random 16-byte SFrame base key for its leg, per call and epoch. Its KID is
   `keyIndex * 65536 + epoch`; the counter is one per sender across its tracks and layers. The cipher suite is
   AES_128_GCM_SHA256_128 (0x0004): HKDF-SHA-256 for the key and salt, AES-128-GCM with a 16-byte tag.
2. **Sent over Olm.** The page sends the key to every other device on the roster as the to-device event
   `io.moqui.call.key` (`{callSessionId, callLegId, kid, epoch, suite, key}`) through `moqui.crypto.sendToDevices`: Olm
   between devices, never a chat room's Megolm session. The server queues and passes on only the ciphertext
   ([Chat keys](Chat-Keys.md); reach: users who share a live call).
3. **Taken only from the right device.** A page installs a key only when it comes from the device the roster names for
   its leg (`senderUserId` and `senderDeviceId` from Olm, which the server cannot forge) and its KID carries that leg's
   key index, and no other leg of the roster has that key index; one that comes before the roster shows the leg waits up
   to 15 seconds. A track is decrypted only with the keys of the leg it comes from (its stream id), each counter once (a
   replay window of 1024 per track and key), and each leg's last four epochs are kept; a leg given another device or key
   index loses the keys it had. A sender sends only with keys it made itself, kept apart from the keys that came in, so
   a roster that gives two legs one key index cannot make two senders share a key and its nonces.
4. **Rotation.** Any change of the roster (a join, a leave or removal, a revoked device, whose legs leave, or a
   reconnect, a new `keyGeneration`) makes each sender a new key and epoch, sent to the new roster. A sender moves to
   the new key a second after sending it, so the others have it first; its first key it uses at once. Who left does not
   get the new key, so cannot read what is said after; rotation cannot take back a key a leg already had.

**The frame.** Opus is encrypted whole. A VP8 frame keeps its payload header in the clear, 10 bytes of a keyframe (the
frame tag, the start code and the size) and 3 of others, as other SFrame-over-SFU deployments do, so the SFU's keyframe
detection, simulcast layers and the W7 relay work unchanged; those bytes are SFrame metadata, authenticated with the
frame. A frame is `clear bytes || SFrame header || ciphertext || tag`, the layout of the `sframe` crate's frames with
metadata.

**One tab.** The keys travel over the user's crypto device, whose store one tab of a browser holds at a time
([Chat keys](Chat-Keys.md#devices), `busy`). The tab that answers or joins takes it; while another tab of the same browser
and user holds it (My devices, or a call in that tab), this one says so and does not join. Close the other or take the
call there.

**What the server still sees.** RTP headers and their extensions (the audio level, for the active speaker), frame sizes
and timing, the clear VP8 bytes, the SFrame header, and who is in the call with which device. The roster is the server's:
a server that put a device of a user in the call on it would get that leg's keys. Devices are not verified yet (emoji and
QR verification and cross-signing are a later C7 pass), so the participants list shows each device as unverified.

## Cluster

A call's media lives on one node, `CallSession.mediaNodeId`, chosen at start among the live `CallMediaNode` rows. A frame
from a page on another node goes to that node only, on the cluster's `rtc-signal` topic (`moqui_core::cluster::send_to`),
and the replies come back the same way. A media node that stops registering (20-second heartbeat) ends its calls with
the reason "media server lost"; the users start a new call. There is no seamless failover.

### Cascading media nodes

With `rtc_cascade_enabled` (off by default; set it on every node) a call spans media nodes (W7):

- **Where a leg's media goes.** As a leg binds (`bind#Connection`), its media goes to the node of its own signaling
  socket when that node is a live media node that is not draining, else to the call's node. `CallLeg.mediaNodeId` keeps
  it, and the answer names both nodes. A phone (SIP) leg's media is on its SIP engine's node: a desk phone registered
  with another node rings there, and a merged call's phones stay on their node.
- **The relay link.** A leg node, the node of a leg's socket holding that leg's media, links the call's room to the
  call's node (the origin): one str0m peer on each side per room, on the two nodes' media sockets. The origin is
  ICE-lite, as it is to browsers; the leg node is full ICE, the controlling side, and makes the first offer. The offers
  and answers go on `rtc-signal` (kind `relay`), with a link id that a new link replaces.
- **What crosses.** Each side sends the other every track of the room it has that did not come from that side, so the
  origin sees every leg (recording, captions and the SIP mix are there) and each leg node every leg. A video track
  crosses a link once, in its simulcast layers up to the highest layer any subscriber behind the link wants; each
  subscriber then gets its own layer as on one node. With no one wanting it, no video crosses. A room recorded at the
  origin wants the high layer of the leg nodes' tracks; the mixer and captions take only audio and bring no video
  across.
- **Room frames.** The notices (recording, captions), `participant`, the relayed `mute`, `hand` and `reaction`, and the
  captions' lines go to the room's other nodes too (kind `roomFrame`); the origin passes a leg node's on to the others.
  Each node works out the active speaker itself, since it hears every leg.
- **Limits.** At most 8 links a room and 64 a node (`SfuConfig`). Past them the origin refuses the link explicitly
  (`tooManyRelays`): the leg node's legs get `mediaLost` and, as they reconnect, bind to the call's node
  (`CallLeg.mediaNodePinned`).
- **Losses.** The origin lost ends the call ("media server lost"), as before. A leg node lost ends only its legs, with
  the same reason (`expire#Calls`). A link lost (its ICE or DTLS failed) is fenced at both ends: the leg node's legs get
  `mediaLost`, reconnect, and the leg node links again with a new link; a room with phone legs on the leg node links
  again at once. A link that does not connect within 10 seconds (`unreachable`: its offer or answer was lost, or the
  nodes do not reach each other's media ports) is handled as a refused one: the leg node's browsers go back to the
  call's node. Phone legs on such a leg node cannot move and are logged as cut off.
- **Late messages.** A room that ended on a node takes no new link there: a leg node's late offer gets a `Close`
  (`ended`), and a late `link` control or leg does not open it. A leg node never takes a new link of a room from that
  room's origin. Each node keeps these for 10 minutes; a call's id is never used again.
- **Calls across nodes merge** (`merge#Calls`): without cascading they are refused (`differentNodes`).

The relay link uses the media ports of both nodes (`rtc_media_port`): the leg node sends from its port to the origin's
public address, so the nodes must reach each other's media port. It carries no TCP or TURN fallback and does not share
out a bandwidth estimate: the link between servers is not a browser's last mile.

The choice of one peer per room and leg node, beside one peer for every pair of legs or a single link per node pair:
it keeps a track to one copy per node, uses the same str0m code and ICE as the browser legs (so no second transport),
and a room ends its own link. A link per node pair carrying many rooms would save ICE and DTLS handshakes but needs its
own multiplexing of rooms; that waits for a measured need.

### The edge node as media node

A local node behind NAT with an edge node ([Edge-Node](../../plans/Edge-Node.md) §5.4) has its calls' media on the edge
node, which browsers and phones outside and in the office reach at its fixed address. The two share no database, so
the edge node is not a member of the local node's cluster:

- **Its row.** The edge node registers no `CallMediaNode` row (it has no call state). Its link Hello names its media
  address and TURN URLs, and the local node keeps its row from the link: written as the link comes up and every 15
  seconds while it lasts (`edgeLinked` Y), so a link down past the media nodes' grace (60 seconds) ends its calls with
  "media server lost", and unpairing removes it. New calls prefer that row to the local node's own media node, if it has
  one (office calls go through the edge node too; plans decision 6).
- **Turning it off.** The Edge Node wizard's calls area (System › Edge Node, Discover) sets `edge_calls_enabled` in the
  local node's `runtime/.env`. While it is `false` the link keeps the row as draining: new calls go to the local node's
  own media nodes (none, and calls are unavailable), and calls already on the edge node go on, as the row stays live.
  Unpairing removes the setting. The wizard's Test checks the row is live and first, and allocates on the edge node's
  TURN with a credential `make#IceServers` gave ([TURN](TURN.md#on-an-edge-node)).
- **Signaling.** The browsers' sockets stay on the local node (through web access from outside). A frame, reply or
  control for the edge node's instance id goes over the link (`Link.Signal` up, `LocalLink.Signal` down, one ordered
  stream each way per link) instead of the cluster's sender: `moqui_core::edge::signal`, the seam in
  `cluster::send_to`. As the cluster's direct send it is one try from a bounded queue (1,024), `overloaded` when full.
- **The edge node's services.** Its media events (`note#MediaEvent`) and its captions' reads, writes and transcription
  (`CallCaptionServices.get#CaptionSpeaker`, `store#CallTranscript`, `transcribe#CaptionAudio`, the audio chunk as
  base64) are delegated to the local node, which has the call state and the agent runtime's quota; its recordings are
  queued ([Call-Recording](Call-Recording.md)). With the link down they fail at once as "local node offline", and
  the media goes on.
- **TURN** runs on the edge node; the local node signs its credentials with the edge node's secret ([TURN](TURN.md)).
- **An all-in-one node** ([Run-Deploy](../Run-Deploy.md#an-all-in-one-node)) registers its own `CallMediaNode` for its
  own calls and is also the local node's media node. Rooms of the local node's calls are keyed by node and call, and
  the link stamps the sending node (`linkNode`) on each message, so the two sides' calls never meet. Recordings and
  captions go to the room's owner.

- **Only the edge node's own messages.** The local node delivers from the link only the media driver's replies, and
  only to a socket that bound that leg of that call with its media on the edge node; the edge node takes from the
  local node only frames whose replies go back to it, and media controls. Run as the link's delegate user, each
  delegated or queued service refuses a call whose media is not on the edge node.
- **Pinned, alone, not cascaded.** Pairing pins the edge node's instance id (`edge_node_id`); a link naming another,
  this node's own or a cluster member's is refused. A call on the edge node does not cascade (`rtc_cascade_enabled`
  leaves its legs there), and the edge node takes call messages only while one local node is linked.
- **A clustered local node.** Only the member that holds the link has the route to the edge node. A frame or control
  for the edge node on another member is refused ("the edge link to .. is down"; a browser's frame gets `unavailable`),
  so a call with media on the edge node works only for sockets on the link's member. Put the local node's web access
  (and its load balancer) on that member, or run one local member; relaying through the link's member is later work.
- **A restart.** The edge node's Hello names when its media socket started, which the local node keeps as the row's
  `startedDate`: a recording that began before an edge node restart is failed by `expire#Recordings`, as on any media
  node.

Settings: `rtc_media_port`, `rtc_media_host` 0.0.0.0, `rtc_public_address` and `turn_port` on the edge node
(`MoquiEdgeConf.xml` sets all but the address); nothing on the local node, which needs no media port. W7 cascading
between the edge node and a local media node is later work.

## Tests

- `cargo test -p moqui-host --test it edge_call`: an outside browser stand-in relayed by the edge node's TURN and an
  office one on UDP in one call, both signaling on the local node's `/notws`, the edge node in a child process: the edge
  node's row kept from the link, the answer from its media socket, media both ways, the media events noted on the local
  node, and a recording made on the edge node whose segments reach the local node. About 6 seconds.
- `cargo test -p moqui-rtc`: the media gate on an in-memory network with simulated time. It covers two and five parties,
  simulcast layers following the bandwidth estimate, audio only below the floor, renegotiation both ways and glare, ICE
  restart, ICE-TCP, generations, and media taken out and put in.
- `cargo test -p moqui-host --test it rtc::`: a direct call on a real server, with str0m peers on loopback UDP signaling
  through `/notws`, and a guest on `/callws`: a bad credential refused, media both ways, another leg's frames refused,
  and revoking the link ending it.
- `./moq test framework --spec Calls`: the call state machine, and the node of a leg with cascading on: its signaling
  node, the call's node when that is no media node or its relay was refused, and a leg node lost ending only its legs.
  `--spec Transfer` merges calls on two nodes, and refuses a transfer, an add and a call to a user a `call-target`
  provider keeps out (the example gallery's); `--spec SipDevice` rings a phone registered with another node.
  `./moq test mr-boss --spec CallTarget`: IronStone's provider, another organization's user refused with the same calls
  that reach a user of a shared party. `--spec CallCaptions` logs the captions notice per leg, a late joiner's too.
- `cargo test -p moqui-rtc sfu::relay_tests`: two SFUs on the in-memory network: media both ways over one link, a track
  crossing once in the layers up to the highest wanted (a layer change at the leg node, nothing wanted), a leg node's
  tracks reaching the origin's taps in every layer, the caps refusing a link, a lost link reported and replaced (an old
  link's message dropped), the link closed when the leg node's last leg leaves or the room ends, an audio-only tap
  bringing no video across, late links of an ended room fenced at both ends, and a link that never connects dropped as
  unreachable.
- `cargo test -p moqui-host --test it rtc::cascade`: two servers of one cluster on one test database, the second in a
  child process of the test binary (the media driver and the socket bindings are process globals, and its loss is a real
  kill): media both ways between a leg on each, a layer change at the leg node, and the leg node lost ending only its
  leg.
- `cargo test -p moqui-host --lib rtc::captions`: captions with a fake transcriber on simulated time: speaker and chunk
  order with answers out of order, a timeout, backpressure with its gap, silence making nothing, and a provider that
  hangs while the frames keep flowing. `cargo test -p moqui-llm --lib speech`: the transcription call against a fake
  endpoint. `./moq test framework --spec CallCaptions`: the policy switch, someone not in the call, an encrypted call,
  and turning captions on and off.
- `./moq test basic-comms --spec BasicCommsCalls`: the screens' transitions through both mounts, an encrypted call's
  answer and its terminal twin.
- `./moq test framework --spec CallEncryption`: encrypted calls' policy: a start, an answer and a join only from a
  browser that can take part (the same parameters with `scriptTransform` false, another user's or a revoked device), a
  leg that binds only with its device and a reconnect's new key generation, a room's and a link's policy, a plain call
  never offered for an encrypted one, a revoked device's leg leaving, and every refusal (recording, captions, a number,
  desk phones, forwarded numbers, merging, guests) beside a plain call that the same parameters pass.
- `./moq test-client webroot --offline --spec WebrootSFrame`: the RFC 9605 Appendix C vectors (all 289 headers, the
  three AES-CTR-HMAC AEADs, each suite's key, salt and frame) and the call's frame layout in the browser's SFrame.
  `cargo test -p moqui-rtc --test sframe`: the same vectors (`crates/moqui-rtc/testdata/`) against the `sframe` crate, and
  the call frames the browser makes, made again by the crate.
- `./moq test-client basic-comms --serve --spec EncryptedCallFlow`: an encrypted video call in three Chromium pages: media
  both ways, decrypted with the keys each got from the other's device; the third person answering late gets both keys;
  Record refused; the third leaving makes the others rotate, the new keys going only to the two left. About 6 seconds.
- **Not checked live:** the key protocol against an independent implementation (there is none for this protocol yet: it
  is Moqui's own over Olm), and Firefox and Safari (the client spec runs Chromium).
- `./moq test-client basic-comms --serve`: a call in two Chromium pages with fake media (`CallFlow`), and a guest by
  link let in from the lobby (`CallGuestFlow`). It needs `rtc` in the binary.
