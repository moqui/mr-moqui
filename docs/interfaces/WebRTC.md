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

On a single server reachable at its own address, `rtc_media_port=10000` with `rtc_media_host` set to that address is
enough. Behind 1:1 NAT, bind the private address and set `rtc_public_address` to the public one, then forward UDP 10000.
The advertised address must be reachable from the browsers. A configured address does not prove it: test a call from
outside the network. For browsers that cannot reach the UDP port, add ICE-TCP (`rtc_media_tcp_port`) and the embedded
TURN server, or coturn ([TURN](TURN.md)).

The Calls screen says when calls are unavailable (`get#Capabilities`): supported (the binary has `rtc`), configured
(this server's port is set) and active (a media server is up in the cluster).

## How a call goes

1. `start#Call` (the Calls screen, the page's call store) makes the CallSession, a leg for the caller and an offered leg
   per callee. It rings each callee's tabs on `chatUser:<userId>` and by Web Push, and appends a call event to the room.
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
mixed audio, made on the media node, with a notice to every leg and the `rtc` frame `recording`. See
[Call recording](Call-Recording.md).

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
   that joins later gets it when it joins, a guest's too. Captions go to every leg in the order they were spoken:
   `{rtc: 'caption', leg, participant, name, seq, text}`.
4. A slow provider never holds the call up. When the queue is full the oldest waiting chunk is dropped and its place
   goes out as `{rtc: 'caption', leg, seq, gap: true}`. A provider error or timeout goes out once as
   `{rtc: 'captions', state: 'unavailable', reason, message}`; the call and its media go on, and the next caption that
   works brings them back.
5. Anyone in the call turns them off (`stop#Captions`, `{rtc: 'captions', state: 'off'}`). They also end with the call.

Transcription for other server code is `org.moqui.impl.SpeechServices.transcribe#Audio` (internal): voicemail (V2) and
recordings (W5) use the same seam, and `CallTranscript` holds their text too.

## Cluster

A call's media lives on one node, `CallSession.mediaNodeId`, chosen at start among the live `CallMediaNode` rows. A frame
from a page on another node goes to that node only, on the cluster's `rtc-signal` topic (`moqui_core::cluster::send_to`),
and the replies come back the same way. A media node that stops registering (20-second heartbeat) ends its calls with
the reason "media server lost"; the users start a new call. There is no seamless failover.

## Tests

- `cargo test -p moqui-rtc`: the media gate on an in-memory network with simulated time. It covers two and five parties,
  simulcast layers following the bandwidth estimate, audio only below the floor, renegotiation both ways and glare, ICE
  restart, ICE-TCP, generations, and media taken out and put in.
- `cargo test -p moqui-host --test it rtc::`: a direct call on a real server, with str0m peers on loopback UDP signaling
  through `/notws`, and a guest on `/callws`: a bad credential refused, media both ways, another leg's frames refused,
  and revoking the link ending it.
- `./moq test framework --spec Calls`: the call state machine.
- `cargo test -p moqui-host --lib rtc::captions`: captions with a fake transcriber on simulated time: speaker and chunk
  order with answers out of order, a timeout, backpressure with its gap, silence making nothing, and a provider that
  hangs while the frames keep flowing. `cargo test -p moqui-llm --lib speech`: the transcription call against a fake
  endpoint. `./moq test framework --spec CallCaptions`: the policy switch, someone not in the call, an encrypted call,
  and turning captions on and off.
- `./moq test basic-comms --spec BasicCommsCalls`: the screens' transitions through both mounts.
- `./moq test-client basic-comms --serve`: a call in two Chromium pages with fake media (`CallFlow`), and a guest by
  link let in from the lobby (`CallGuestFlow`). It needs `rtc` in the binary.
