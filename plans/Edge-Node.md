# Edge Node

Status 2026-11-02: E0–E4, E5a, E5b, E6, E7, E9 and E10 built; E8 (SIP) and E11 not yet. The E4 two-runtime check (RemoteHttp) passed over REST and over gRPC
direct; the run on the link, which needs a publicly trusted certificate on the edge node, is pending. mr-pass reaches
its authority over gRPC, direct or on the link, with REST kept; after E3's delegate, queue and replicate and E2's web
access. Mail crosses the link, with the MTA in moqui on the edge node by default (E5b) or Postfix and Rspamd beside it
(E5a); the manual check at a large provider is pending. Chat bridges' webhooks and MMS files go through the edge node
(E6); a real Telnyx round trip is pending. Calls have their media on the edge node, with its TURN, and recordings are
queued to the local node (E7); real outside browsers over the internet are pending. Every question in [§8](#8-decisions) is
decided. E0 done: the entity access audit is [§4a](#4a-entity-access-audit), and the build cost is in §5.6. What E1
built, where it differs from the design, is in [§5.1](#51-the-edge-link) ("As built"),
[§5.6](#56-moquiedgeconfxml-and-packaging) and [§5.7](#57-attaching-a-local-node-system-app); what E2 built is in
[§5.3](#53-web-access) ("As built"); what E3 built is in [§5.2](#52-how-work-crosses-the-link) ("As built"); what E4
built is in [§5.5](#55-mr-pass-on-the-edge-node-grpc-by-default) ("As built"); what E5a built is in
[§5.4](#54-comms-on-the-edge-node) ("As built (E5a)"), with the queue's content in §5.2, E5b's in §5.4 ("As built
(E5b)"), E6's in §5.4 ("As built (E6)"), E7's in §5.4 ("As built (E7)"), E9's, the XMPP tunnel with the design of C3b's and C6b's split, in §5.4
("As built (E9)"), and E10's, the wizard's Discover, Apply and Test, in
[§5.7](#57-attaching-a-local-node-system-app) ("As built (E10)", its XMPP area in "As built (E10, XMPP)" and its calls area in "As built (E10, calls)"). A real local node attached through the wizard on a
served Maximal build, with real DNS, is pending.

An edge node is a small MR Moqui instance on a cloud host with a fixed address. It gives a **local node** a public
presence. The local node is the system of record, behind a firewall on a dynamic IP. The edge node is what the world
reaches: web access to the local node's apps, the public sites (mr-pass), mail, chat providers' webhooks, SIP trunks and
phones, and the media of calls. The local node dials out to the edge node and keeps one link open. Everything the edge
node needs from the local node goes over that link, so the local node needs no open port, port forward, fixed address or
dynamic DNS.

The edge node holds no business data. Its own small database has the framework tables, its configuration, the link's
queues, the sessions of public-site visitors, and a replicated subset of the local node's configuration: the part it
needs to answer while the local node is away, such as which mail addresses exist. For async comms (mail, chat) it
stores and forwards. What arrives while the local node is offline or unreachable waits in the edge node's queue. It is
delivered in order, and once, when the link is back.

Sections: [1 shape](#1-shape) · [2 topologies](#2-topologies) · [3 what is built](#3-what-is-built) ·
[4 gap analysis](#4-gap-analysis) · [4a entity access](#4a-entity-access-audit) · [5 design](#5-design) ·
[6 phases](#6-phases) · [7 documents to change](#7-documents-to-change) · [8 decisions](#8-decisions) ·
[9 references](#9-references)

---

## 1. Shape

```
 the internet: browsers, site visitors, mail servers, SMS and chat providers, SIP trunks, phones, call legs
    |  443: web access, public sites, webhooks, the link    25: MX    5060/5061: SIP    10000/udp: media    3478: TURN
    v
 edge node   edge.example.com, fixed address, Maximal package (--release), MoquiEdgeConf.xml, production
    base components and mr-pass; its own embedded Postgres: conf, queues, replicated subset, visitor sessions
    SFU and TURN, SIP edge, mail edge, chat webhooks, public sites, an offline page
    ^
    |  the edge link: TLS on 443, dialed by the local node, kept open
    |  gRPC both ways on HTTP/2 (the local node's calls up; the edge node's calls down a reversed connection); byte tunnels
    |
 local node   behind NAT, dynamic address, no inbound port; Maximal or Enterprise, production
    mr-mantle, mr-boss, mr-marble and the rest; all business data; comms logic (routing, inboxes, rooms); PASS authority
    office users reach it on the LAN under the same names (split-horizon DNS)
```

| | Edge node | Local node |
|---|---|---|
| Data | Framework tables, edge node conf, queues, replicated subset, PASS visitor sessions | Everything |
| Web | The public endpoint of the local node's apps; mr-pass sites; webhooks; an offline page | Serves the apps |
| Mail | MX and the outbound edge, with a queue each way | Inboxes, rules, composing and sending |
| Chat | Provider webhooks, MMS media; XMPP's public endpoint | Rooms, bridges, history |
| Voice | SIP trunks, the registrar for phones outside, RTP; a voicemail fallback while the local node is away | Routing, users, boxes, voicemail storage |
| Calls | The SFU (media node) and TURN | Call state, signaling, recordings, captions |
| PASS | mr-pass in remote mode | The authority (mr-mantle) |

Anything the local node can do by dialing out stays on the local node. That covers IMAP and POP sync, OAuth token
refresh, Web Push, LLM calls, and providers' send APIs. The edge node is for what must be **reached**: MX, webhooks,
inbound SIP and RTP, the SFU, TURN, public sites, and outside users of the apps.

## 2. Topologies

How an edge node changes the topologies in [Run-Deploy](../docs/Run-Deploy.md#choose-a-topology), and what it adds:

| Topology | With an edge node |
|---|---|
| One production server | **A local server with an edge node** (new): the server stays in the office or at home, and the edge node gives it public reach and its comms. This is the case for a small business or a household running BOSS, Comms Inbox included, on its own machine |
| PASS servers with a BOSS server or cluster | The edge node is the PASS server, and the authority needs no public address. Other PASS servers can still dial an authority that is reachable |
| Cluster | A local cluster behind an edge node. One member holds the link at a time, through a lock row as scheduled jobs use, and another member takes over when it drops |
| Dev instances with a dev group server | The group server can sit in the office. Laptops reach git, the LLM gateway and tasks through the edge node (web access, and the SSH tunnel) |
| New: several local nodes on one edge node | Branch offices, or a provider hosting edge nodes for many small businesses. One local node per edge node comes first, but the edge node's rows are keyed by the paired node, so several can come later without a redesign ([decision 1](#8-decisions)) |
| New: an edge node with more media nodes | W7 cascading from the edge node to regional media nodes. Later |
| New: a desktop or laptop local node | A personal instance ([Usability](proposals/Usability.md) hosts) attached to an edge node. It follows from the rest |

## 3. What is built

These pieces exist today. Each is a seam or a model for the edge node; none does the job yet.

- **Cluster transports.** HTTPS POST with one shared client certificate on 8444
  (`crates/moqui-host/src/cluster_edge.rs`), Postgres `LISTEN`/`NOTIFY` (`cluster_notify.rs`), and SSH
  (`ssh/cluster.rs`).
  - They carry 5 topics, cache invalidation and `rtc-signal` among them, sent through `cluster::send_to`
    (`crates/moqui-core/src/cluster/direct.rs`).
  - All three need one shared database and a peer that can be dialed. Delivery is best effort, and a message that fails
    3 tries is lost.
- **gRPC.** tonic 0.14 runs the server only, for the OTLP receiver on its own port 4317
  (`crates/moqui-host/src/monitor/grpc.rs`). It is the `grpc` feature, in Enterprise and Maximal.
  - There is no client and no `.proto` of our own: the OTLP messages come pre-generated.
  - The main listener is HTTP/1.1 only (`http.rs:378`, ALPN `http/1.1` in `tls.rs:143`).
- **An outbound reconnecting socket.** `moqui tui --server` (`crates/moqui-host/src/tui_remote.rs`) reconnects to
  `/notws` with a backoff from 2 s to 60 s. It is the only reconnecting outbound socket, and it is client only.
- **Instance-to-instance data.**
  - EntitySync over REST, push or pull (`crates/moqui-core/src/service/system_sync/entity_sync.rs`). Pull lets a node
    behind NAT fetch.
  - SystemMessage with retry (`SystemMessageServices.xml`; its jobs are paused in the seed data).
  - SFTP send and poll.
- **Leased outboxes.**
  - `ChatBridgeDelivery` (`crates/moqui-core/src/service/chat_bridge.rs`): an idempotency key, per-mapping order, and
    backoff for 8 tries, about 2.7 hours.
  - `ChatBridgeInbound`: a batch is stored before it is acknowledged.
  - The A2A push outbox (`crates/moqui-llm/src/a2a/push.rs`).
  - These are the model for the edge node queue.
- **The PASS remote mode.** A public node with its own small database calls an authority for every read and write.
  - One facade carries nearly all calls: `mr-pass/service/pass/Facade.js` (`authorityClient`, `call`), plus
    `Account.js` for sign-in and SSO.
  - There are 154 routes onto 149 operations. Each takes one `input` map and returns one `result` map.
  - The authority's servlet is `mr-mantle/script/PassApi.js`.
  - The PASS server dials the authority.
- **A remote mail gateway.** [Mail-Inbound](../docs/interfaces/Mail-Inbound.md#dns-and-the-gateway) already describes a
  small VPS gateway (Postfix with Rspamd) that delivers to mr-moqui over WireGuard. The `inbound-smtp` listener speaks
  SMTP or LMTP, takes PROXY headers, and drops duplicates by Message-ID.
- **A media node apart from the signaling node.** A call's media lives on one node (`CallSession.mediaNodeId`).
  Signaling from a page on another node goes there over `rtc-signal`, and W7 cascades media between nodes. Today all of
  this is inside one cluster.
- **TURN apart from the data.** coturn needs only `turn_auth_secret` and no database. The embedded TURN server
  restricts relays to the media nodes (`turn_allowed_peers`).
- **The setup wizard pattern.** `<subscreens-panel type="wizard">` (`m-subscreens-wizard` in the qvt client). The
  MailAccounts Add wizard in basic-comms (`screen/BasicComms/Settings/MailAccounts/Add/`: Address, Connect, Test,
  Options, Finish) is the template for attaching to an edge node.
- **Web Push** goes out from the node that holds the notification, so it works from behind NAT.

## 4. Gap analysis

| Area | What it assumes today | Gap for edge mode |
|---|---|---|
| **Node link** | A shared database; every peer reachable at `cluster_base_url`; the same secrets on every member | A link the local node dials and both sides use; no shared database; a key per local node; durable delivery for what must not be lost |
| **gRPC** | Server only, on a port of its own | The tonic client (its `channel` feature is already compiled in, §5.6) and code generation (`tonic-prost-build` with `protox`) for our own `.proto`; HTTP/2 on the HTTPS listener, or ALPN routing; a script binding to make gRPC calls |
| **Tunnels and proxying** | None. SSH has no port forwarding; "proxy" in the host only means trusting `X-Forwarded-*` | A reversed HTTP/2 connection, HTTP and WebSocket forwarding, byte tunnels, and PROXY headers on the HTTPS listener (only the SMTP listener takes them) |
| **Replicated data** | EntitySync to a URL that can be dialed | One-way replication of named entity subsets, from the local node to the edge node over the link, driven by commits, with a cursor and a full resync |
| **Durable queues** | Outboxes inside one database. Mail out has no automatic retry: a failed send stays `ES_READY` with `sendError` until someone retries it, and a lost connection is treated as ambiguous and never resent (`moqui-mail/src/implementation.rs:1429`) | An edge node queue in each direction, kept for days, with receipts on the local node. Retry mail on its own when the edge node could not be reached |
| **Web access** | Clients reach the node; certificates in `runtime/cert`; no ACME, no DDNS | The edge node forwards by host name, either passing TLS through by SNI or ending TLS itself. WebSockets (`/notws`, `/termws`, `/xmpp-websocket`), the client's address passed to the local node, and an offline page |
| **mr-pass remote** | The PASS server dials the authority's public HTTPS. REST paths from `Routes.js`; the client secret in plain headers; the authority's HTTP session cookie carries the second factor (`passAuthorityCookie`); downloads streamed with `response.forward`; a 30 s timeout; no cache | A gRPC transport, with REST kept as an option. The authority reached down the link. The second factor's session as an explicit handle. Streamed downloads and uploads. A read cache for when the local node is away |
| **Mail in** | An edge gateway does port 25, SPF, DKIM and DMARC, spam filtering, and the queue. `reject_unverified_recipient` asks mr-moqui live at `RCPT TO`. The listener stores into the account's `EmailMessage` and runs the ECAs on the same node | The edge node as the mail edge: first with the gateway beside it, then with an MTA in moqui, and later with filtering in moqui ([decision 3](#8-decisions)). The recipient check answered from a replicated list while the local node is away. Delivery to the local node's existing delivery code over the link |
| **Mail out** | SMTP through an `EmailServer` that can be reached. No DKIM signing or MX delivery in moqui | Submission to the edge node, a delivery queue with retry and DKIM signing, and delivery reports back to the `EmailMessage` |
| **Mail standards, filtering** | Nothing in moqui checks SPF, DKIM, DMARC or ARC, or scores spam. The listener trusts the gateway's `Authentication-Results` and spam fields | Verification and signing in moqui, applied to mail inside the organization as well as mail from the internet. Filtering in moqui for mail and for the other inbound channels (public forms, chat) |
| **Chat bridges, SMS** | The webhook handler runs `receive#Sms` on the node that has the `PhoneNumber`, room and opt-out rows. MMS media is served from that node at `channels_public_url` | Webhooks verified on the edge node with a replicated secret, then queued to the local node. MMS media kept on the edge node for the token's life. The `ChatBridgeAdapter` seam (`Extension::chat_bridge_adapters`) is where an edge node adapter would plug in |
| **XMPP** | C3a turns each stanza into a service call by name, as the user; the SCRAM verifiers, rooms and MAM are on that node | Tunnel 5222 and 5223 to the local node first. C3b (s2s) on the edge node, queued while the local node is away |
| **Matrix, federation** | Not built (C6a to C6c, C2). The plan keeps the room DAG and the signing keys in Moqui's Postgres | Decide the split before C6: other servers ask for keys and backfill whether the local node is up or not |
| **SIP, voicemail** | The engine runs beside the media port, on a node that is both a `SipNode` and a `CallMediaNode`. Routing reads `PhoneNumber`, `CallHandlingRule`, `CommsSchedule` and `UserStatus`. The registrar checks `SipDeviceVerifier`. Boxes, greetings and audio are on that node | SIP and RTP on the edge node, with routing decided on the local node over the link. A voicemail fallback while it is away ([decision 4](#8-decisions)). Audio sent back through the queue |
| **WebRTC SFU** | Media nodes are cluster members on the shared database (`CallMediaNode` heartbeats). `rtc-signal` goes through `cluster::send_to`. Tested as "two servers of one cluster on one database" | The edge node as a media node of the local node's calls, without a shared database. The local node keeps the edge node's `CallMediaNode` row from the link. `rtc-signal` goes over the link. The edge node's driver calls its services on the local node |
| **TURN** | Already separable | Run it on the edge node, with `turn_allowed_peers` set to the edge node's media port |
| **Recording, captions** | The recorder runs on the call's media node and writes `dbresource://moqui/call/recording/` | Segments queued to the local node. Captions through the local node's `transcribe` binding (quota and `LlmCall` stay there) |
| **Push** | Outbound HTTPS | None. The local node sends. Subscriptions are bound to the origin, so the apps keep one host name inside and outside |
| **Conf, packaging** | MoquiProductionConf.xml and MoquiDemoConf.xml. A conf cannot remove a component | MoquiEdgeConf.xml, a `node_role` property, and a way to make an edge node runtime with only the base components and mr-pass |
| **Setup screens** | Wizard support, and the MailAccounts wizard | System › Edge Node on the local node, and System › Edge Clients on the edge node |
| **Dynamic address** | `rtc_public_address`, `sip_public_address` and `cluster_base_url` are static | Fine on the edge node, whose address is fixed. The local node needs none of them |

## 4a. Entity access audit

What each comms engine reads and writes, and where each access goes on an edge node, which has none of the business
data. The classes:

- **edge node:** runs there, on its own rows or none;
- **delegate:** a synchronous call to the local node, which fails while it is away;
- **queue:** stored on the edge node and run on the local node when delivered;
- **replicate:** answered on the edge node from a copy;
- **link:** done by the link itself, not by a service call;
- **stays local:** runs on the local node only, and the edge node never calls it.

Most of the engines' work already goes through services by name, which the delegate and queue ways can carry as they
are. The direct entity access in Rust is in the mail listener, the chat webhook and media routes, the call captions,
the cluster sender and the XMPP upload. The SIP engine also reads `dbresource://` audio itself.

**Mail: the SMTP listener** (`crates/moqui-host/src/smtp.rs`, `crates/moqui-mail/src/delivery.rs`)

| Access | Entities and files | R/W | Class |
|---|---|---|---|
| `FactoryBackend::recipient` → `delivery::find_recipient`, `sync::account_info` | `EmailAccount` (by address, hosted and shared), `EmailIdentity`, `EmailServer` | R | replicate (`mailRecipients`) |
| `FactoryBackend::deliver` → `delivery::deliver` → `deliver_one` | `EmailAccount`, `EmailFolder`, `EmailMessage` (the Message-ID check) | R | queue |
| `deliver_one` → `store::store_message` | `EmailMessage`, `EmailMessageAttachment`, files under `dbresource://email/` | W | queue |
| `deliver_one`: the email ECAs, `EmailClientServices.run#EmailRules`, `wake#SnoozedThreads`, `send#OutOfOfficeReplies`, `publish#AccountCounts` | the account's rules, threads and counts | R/W | queue |
| `send_delivered_notice` (the delivered DSN) | the account's outgoing `EmailServer` | R | stays local |

The queued item is the raw message with its envelope: the sender, the recipients with their DSN parameters, and the
client's address. The local node runs `deliver` on it unchanged.

**Chat bridges and SMS** (`crates/moqui-host/src/channels.rs`, `crates/moqui-core/src/service/chat_bridge_web.rs` and
`chat_bridge.rs`, `crates/moqui-channels`)

| Access | Entities and files | R/W | Class |
|---|---|---|---|
| `chat_bridge_web::webhook`, `chat_bridge::adapter_of` | `ChatBridge`, `Enumeration` (the adapter) | R | replicate (the bridge row and its secret) |
| the adapter's `verify_webhook`: the signature, `replay_take`, `replay_finish` | the `moqui.security.hmac.replay` cache | R/W | edge node |
| the adapter's `webhook`: `SmsServices.receive#Sms` (then `ChatBridgeServices.ingest#Event`), `ChatBridgeServices.note#DeliveryReceipt` | rooms, events, opt-outs, `ChatBridgeInbound`, deliveries | R/W | queue (the raw event) |

The adapter's webhook splits in two. The edge node verifies the signature: Telnyx signs `timestamp|body` with a 300 s
tolerance, so the local node cannot verify it again later. The local node applies the queued event without verifying it
again, with the transport-verified mark (today `WEBHOOK_MARK`) set from the queue item.
| `chat_bridge_web::media` (`GET /channels/{id}/media/{token}`) | `ChatAttachment` and its file | R | edge node (a copy for the token's life) |
| `store#BridgeMedia` (inbound MMS, fetched from the provider) | `ChatAttachment` and its file | W | stays local |
| the outbound delivery loop (`chat_bridge::start`, `round`) | `ChatBridgeDelivery` | R/W | stays local |
| Telnyx sends: `SmsServices.check#SmsSend`, `release#SmsSend` | limits, opt-outs | R/W | stays local |

**Calls: WebRTC** (`crates/moqui-host/src/rtc/`, `crates/moqui-rtc`, `crates/moqui-host/src/notws/rtc.rs`)

| Access | Entities and files | R/W | Class |
|---|---|---|---|
| the heartbeat, `CallServices.register#MediaNode` | `CallMediaNode` | W | link (the local node keeps the row from link liveness and Hello; E7: `edgeLinked`) |
| the driver, `CallServices.note#MediaEvent` | `CallLeg`, `CallConnection`, `CallSession` | R/W | delegate |
| the driver, `cluster::send_to` for `rtc-signal` | `ClusterMember` | R | link |
| the recorder's spool (`runtime/tmp/call-recording`), then `CallRecordingServices.store#RecordingSegment` (through `store#RecordingFile`) and `note#RecorderEnd` | recording, track and segment rows, files under `dbresource://moqui/call/recording/` | W | queue (the segment file with its service call) |
| captions: `speaker_of` | `CallLeg`, `CallParticipant`, `UserAccount` | R | delegate (E7: `CallCaptionServices.get#CaptionSpeaker`) |
| captions: `store`, `create#moqui.call.CallTranscript` (entity-auto) | `CallTranscript` | W | delegate (E7: `store#CallTranscript`, not a generic create granted to `_EDGE_`) |
| captions: `llm_hooks::transcribe` (a Rust hook) | `LlmCall`, the user's quota | R/W | delegate (E7: `transcribe#CaptionAudio`); the audio chunks go over the link as base64 |
| TURN (`turn.rs`) | none: `turn_auth_secret` only | | edge node |
| browser signaling on `/notws` (`notws/rtc.rs`), binding a leg, `get#IceServers` | call state | R/W | stays local (through web access) |

**Voice: SIP** (`crates/moqui-host/src/sip/`, `crates/moqui-sip`). The Rust code reads no entity directly; every entity
access is a service call. It does read audio files itself, through `factory.resource().location_bytes`.

| Access | Entities and files | R/W | Class |
|---|---|---|---|
| `VoiceServices.get#EngineConfig` | trunks, with their `authJson` | R | replicate |
| `VoiceServices.register#SipNode` | `SipNode` | W | link, as the media node's heartbeat |
| `VoiceServices.receive#PhoneCall`, `note#PhoneLeg`, `note#TrunkStatus`, `record#AuthFailure` | routing, call state, trunk status | R/W | delegate |
| `SipDeviceServices.get#SipDeviceAuth`, `store#SipRegistration`, `receive#DeviceCall` | `SipDeviceVerifier` (not replicated), registrations | R/W | delegate |
| IVR and transfers: `IvrServices.route#IvrCall`, `CallQueueServices.tick#QueueCall`, `CallTransferServices.refer#Call`, `expire#CallTransfer` | menus, queues, transfers | R/W | delegate |
| `send#SipControl` to the engine, on `rtc-signal` | | | link |
| voicemail: `VoicemailServices.begin#Voicemail` | the box | R/W | delegate; the replicated fallback while the local node is away |
| voicemail: the spool (`runtime/tmp/voicemail`), then `store#Voicemail` | voicemail rows, files under `dbresource://moqui/call/voicemail/` | W | queue |
| voicemail by phone: `check#VoicemailPin`, `act#PhoneVoicemail` | the box, PIN failures | R/W | delegate |
| every box's greeting (`greetingLocation`, `dbresource://moqui/call/voicemail/greeting/`, in `sip/voicemail.rs`) | files | R | replicate, as files |
| IVR and queue prompts and hold music (`sip/ivr.rs` `location_audio`, `PROMPT_ROOT` in `sip/audio.rs`) | files under `dbresource://moqui/call/prompt/` | R | replicate, as files |
| saved voicemail played by phone (`Say::Message(audioLocation)`, `voicemail::location_bytes_job`) | files under `dbresource://moqui/call/voicemail/` | R | delegate, fetched on demand |
| the deployment's recorded prompts (`voicemail_prompt_location`) | files | R | edge node (deployment files) |
| recording a phone leg | as for WebRTC | W | queue |

**XMPP** (`crates/moqui-xmpp`, `crates/moqui-host/src/xmpp/`)

| Access | Entities and files | R/W | Class |
|---|---|---|---|
| stanzas through `Backend::call`: 42 services of `XmppServices` and `ChatServices`, `get#ScramCredentials` among them | users, rooms, MAM, PEP, push | R/W | stays local while 5222 and 5223 are tunneled; delegate (and queue for s2s) under C3b |
| HTTP upload (`xmpp/upload.rs`) | `XmppUpload` and its files | R/W | stays local (through web access) |

## 5. Design

### 5.1 The edge link

- **Port.** The local node dials the edge node's 443, the one port nearly every network lets out. TLS uses ALPN
  `moqui-edge/1`, so the HTTPS listener hands the connection to the link handler. Browsers' `h2` and `http/1.1` are not
  touched, and the listener otherwise stays HTTP/1.1. A port of its own, as the OTLP receiver has, is the fallback.
- **Identity.**
  - The edge node's certificate is checked by WebPKI, and the fingerprint seen at pairing is pinned.
  - The local node has a key of its own, which the edge node pins at pairing: mutual TLS with one key per local node,
    not the cluster's one shared pair.
  - The local node keeps `edge_node_url`, `edge_node_key` and `edge_node_pin` in `runtime/.env`, since keys are
    configuration, not database rows (README redesign 7).
- **Two connections, one in each direction of control.**
  - **Up.** The local node is the HTTP/2 client. It carries Hello, outbound submissions, acknowledgements, replication
    changes and telemetry.
  - **Down.** The local node dials this one too, and the roles flip after the TLS handshake: the edge node is the HTTP/2
    client and the local node serves it with hyper's HTTP/2 server and its own router. Everything the edge node starts
    goes down:
    - delegated service calls, queue delivery, and PASS authority calls, all as gRPC;
    - forwarded HTTP, and WebSockets by HTTP/2 extended CONNECT (RFC 8441);
    - byte tunnels.
  - On the edge node, a tonic channel's connector hands out the down connection. An edge-to-local call is then an
    ordinary gRPC call, with deadlines, cancellation, flow control and streaming. Forwarded web requests reuse the local
    node's router unchanged.
  - One bidirectional stream with our own framing inside it was considered. The reversed connection gets all of that
    for free.
- **Hello.** Each side sends:
  - its binary and link protocol versions;
  - the features compiled in (rtc, sip, xmpp, inbound-smtp, channels);
  - the edge node sends its capabilities and public names; the local node sends its instance id.

  Protobuf changes are additive only. The edge node accepts a local node one version behind, and says plainly when it
  cannot.
- **Liveness.**
  - HTTP/2 PINGs every 15 s keep NAT mappings open.
  - A dropped connection reconnects with a backoff, doubling from 1 s to 30 s (see "As built").
  - A link generation number fences a stale connection: it cannot acknowledge or answer once a new one is up.
- **Not on the link.** Call media. Browsers and phones send it to the edge node's media sockets, and the local node is
  not in the media path. One exception: caption audio chunks go to the local node over the link, since the quota and
  `LlmCall` stay there ([§4a](#4a-entity-access-audit)).
- **QUIC.** Its connection migration would carry a link across an address change. It is a later option; quinn is in
  the tree only under reqwest.

**As built (E1).**
- **Code.** The link is `crates/moqui-host/src/edge_link/`, the `edge` feature, in Maximal and Enterprise. It is not
  named `edge` because `src/edge.rs` is the host's async runtime. `moqui-core/src/edge.rs` is the interface the services
  call. The `.proto` is `crates/moqui-host/proto/edge.proto`.
- **ALPN per ClientHello.** The HTTPS listener reads each ClientHello first (tokio-rustls `LazyConfigAcceptor`) and
  picks the TLS settings by its ALPN. Only an edge connection asks for a client certificate, so a browser is never
  asked for one.
- **Two ALPNs and a link key.** The edge node has a long-lived link key, `edge_link_key`. It is generated as a secret
  at the first start and becomes an Ed25519 key.
  - **Up and down** use ALPN `moqui-edge/1`, with a certificate made from the link key. The local node pins the link
    key's fingerprint as `edge_node_pin`.
  - **Checking and pairing** use ALPN `moqui-edge-pair/1`, on the web certificate, which the local node checks by
    WebPKI. The edge node tells the link key's fingerprint over it.
  - So a renewed web certificate does not touch the link. Changing `edge_link_key` means every local node must pair
    again. The local node finds the change: the edge node's key no longer matches its pin. It stops in state Refused,
    and its Edge Node screen shows the reason, such as "its link key changed, so pair again". A local node the edge
    node revoked is refused the same way.
- **Pins are of public keys,** the SHA-256 of each side's public key, not of a certificate. A certificate made again
  from the same key keeps its pin.
- **Backoff.** A lost link is dialed again after a delay that doubles from 1 s to 30 s, with jitter. After a refusal
  the delay starts at 30 s and grows up to 10 minutes.
- **Limits.** A checking or pairing connection stays open at most 30 s, with at most 4 calls and 2 streams. Pair is
  limited to 10 tries per IP address per 10 minutes.
- **Services.** `org.moqui.impl.EdgeServices` (`framework/service/org/moqui/impl/EdgeServices.xml`, bodies in Rust):
  - on a local node: `get#LinkStatus`, `check#EdgeNode`, `pair#EdgeNode` and `unpair#EdgeNode`;
  - on an edge node: `create#PairingCode`, `get#EdgeClients`, `revoke#EdgeClient` and `ping#EdgeClient`.

  `check#EdgeNode` (connect and show the edge node before pairing) and `ping#EdgeClient` (a Ping down a local node's
  down connection) were not in the design. In a binary without the link, each fails with a plain message, and
  `get#LinkStatus` answers state `notBuilt`.
- **Entities.** `moqui.edge.EdgeClient` (a paired local node, keyed by its instance id) and `EdgePairingCode` (only the
  code's hash is kept), in `framework/entity/EdgeEntities.xml`.

### 5.2 How work crosses the link

There are four ways, and each comms area uses some of them:

| Way | Direction | While the local node is away | Used for |
|---|---|---|---|
| **Delegate** | Edge node to local: a synchronous gRPC call of a named service | Fails at once with "local node offline" | Call routing (`VoiceServices.receive#PhoneCall`), registrar checks, call media events and captions, PASS authority calls; XMPP stanzas under C3b |
| **Queue** | Either way: durable and ordered, delivered at least once and applied once by receipt | Waits on the edge node (a setting, 14 days by default, with a size cap) | Inbound mail, SMS, MMS and other bridge events, voicemail and recordings, delivery reports; XMPP stanzas both ways and Matrix transactions in, under C3b and C6b (§5.4) |
| **Replicate** | Local to edge node; read-only on the edge node; driven by commits, with a cursor and a full resync | The edge node answers from its copy | Recipient addresses; trunk config (`get#EngineConfig`, with `authJson`); each phone number's fallback (voicemail box, greeting, outside number); bridge webhook secrets; PASS site info and, later, cached public pages |
| **Tunnel** | Raw bytes, edge node to local (TLS passthrough, SSH, XMPP) or local to edge node (SMTP submission) | Refused, or the offline page | Web access in passthrough, SSH, XMPP until C3b |

Rows kept from the link (`CallMediaNode`, `SipNode`) and `cluster::send_to` over the link are the link's own work, not
one of the four ways. The MMS copy the local node uploads to the edge node is part of the outbound send.

- **Delegate.**
  - On the edge node, with `node_role=edge`, the service facade sends the services a conf lists over the link, such as
    `<edge-delegate service="org.moqui.impl.VoiceServices.receive#PhoneCall"/>`.
  - The call runs on the local node as a dedicated user, `_EDGE_` in the group `EDGE_DELEGATE`, which is granted only
    those services. The original caller's identity travels as data, never as a login.
  - The comms engines also read and write entities directly, or through services the edge node cannot run: heartbeats,
    `CallMediaNode`, `SipRegistration`, the recorder's spool. [§4a](#4a-entity-access-audit) lists each access and its
    class.
- **Queue.**
  - On the edge node, `moqui.edge.EdgeQueueItem` holds:
    - the paired node's id, as on every edge node row that belongs to one local node (decision 1);
    - the item id, direction, kind and idempotency key;
    - the payload, or a content location for a large body;
    - the size, the created date, attempts, the lease and the status.

    It is modeled on `ChatBridgeDelivery`.
  - The local node keeps `moqui.edge.EdgeReceipt` by item id, so a redelivery does nothing. The receipt goes up only
    after the local transaction commits.
  - Order is kept per conversation key (a room, a mailbox, a phone number).
  - The depth and the oldest item of each kind are metrics, with alerts.
- **Replicate.**
  - The conf names entity sets, with only the fields needed: `<edge-replicate set="mailRecipients" ...>`.
  - On the local node, a commit to an entity in a set registers the change in the transaction, as data feeds register
    search documents. It is sent after the commit.
  - A cursor is kept per set. On reconnect the local node sends from the cursor, or the whole set when the edge node has
    lost it.
  - EntitySync's code is the model.

**As built (E3).**
- **Conf.** One `<edge>` element in moqui-conf (`moqui-conf-3.xsd`, merged by `context/conf.rs` `merge_edge`), read by
  both ends: `delegate` (by service, with `allow-anonymous`), `queue-kind` (name → handler service), `replicate set`
  with `entity name fields` children, and the queue's `queue-retention-days` (14), `queue-max-megabytes` (1024),
  `queue-max-attempts` (10) and `queue-retry-seconds` (5, doubling to 10 minutes). Service names are canonicalized as
  the service facade does (`create-moqui.X` is `create#moqui.X`). `moqui_core::edge::settings` reads it once per
  factory with the node's role, so two servers in one test process keep their own. An edge node reads it at boot
  (`edge::conf::init`, from the factory's boot), so a delegated service goes to the link from the first call, and with
  no link yet fails as offline rather than running on the edge node; any other node reads it when its listeners start.
- **Code.** What needs no network is in core (`moqui_core::edge::{conf, delegate, queue, replica}`): rows, receipts,
  handlers, change notes, batches. The host's link moves it: `edge_link/queue.rs` (the delivery loops, metrics,
  retention), `edge_link/replicate.rs` (the local node's sender), new RPCs `LocalLink.Deliver`, `LocalLink.Delegate`,
  `Link.Deliver` and `Link.Replicate` (additive). The down connection takes gRPC only under
  `/moqui.edge.v1.LocalLink/` and, since E4, `/moqui.pass.v1.PassAuthority/` (§5.5), and only with no client named,
  so a forwarded request reaches none of them. Every message is at
  most 16 MiB both ways. The loops catch a panic per round and go on.
- **Delegate.** The service facade (`service::call` `sync_with`) hands a listed synchronous call to the link; an async
  call or a scheduled job runs where it was queued. The edge node keeps the has-error and rollback-only checks, and with
  the service's definition checks its authorization and `authenticate` first (as an async call is checked before it is
  queued); the design said after the lookup, but the edge node may not have the definition, and then a caller with no
  user is refused unless the entry has `allow-anonymous`. Parameters go as JSON without credentials or `_` names, and
  the caller (`username`, `userId`) as data; a service that declares `edgeCaller` gets it. The local node drops every
  credential, `auth` + capital and `_` name again, refuses a service its own conf does not list, logs in `_edge_` (no
  password; group `EDGE_DELEGATE`, `UgtRemoteSystems`, granted the `EDGE_DELEGATE` artifact group, which starts with
  the diagnostic `echo#Delegate`) and runs it with authorization. Its errors come back as service errors; with no link
  the call fails at once with "local node offline". A call inside the caller's transaction holds it while it waits, up
  to the link's answer timeout (60 s). With several nodes linked a call is refused (decision 1).
- **Queue.** `moqui.edge.EdgeQueueItem` on both ends (direction `EqdToLocal` on the edge node, `EqdToEdge` for a local
  node's outbox; `pairedNodeId` is the local node of the pair), `sequenceNum` for order. The item id is a UUID, or for an
  idempotency key a hash of node, direction and key, so a racing enqueue of one key makes one row and a key queued again
  after the sender's database was lost is the same item to the receiver. A node enqueues only the kinds its conf
  lists, and a payload is at most 1 MiB (larger is refused; a larger body is the item's content, below, since E5a). Delivery runs while the link is up, one task per direction and node, woken
  by a commit; a round reads the head of each key with one grouped query (`EdgeQueueHead`), without payloads, and reads
  a payload only for the item it claims. The receiver inserts the `moqui.edge.EdgeReceipt` first (insert-if-missing, so
  an overlapping redelivery waits on its key and then finds it), then runs the kind's handler as `_edge_` in the same
  transaction, and answers after the commit. **Head of line:** the items of a key go one at a time; a refused head holds
  the key, retried after the backoff, until it is delivered or fails for good (`queue-max-attempts`). Not counted: no
  answer, a paused, busy or stopping receiver (retried soon), and a receiver with no handler or grant for the kind
  (retried every 10 minutes, so a version skew fails nothing). A receiver's refusal of the message itself (too large,
  malformed) is counted. Retention: delivered items by `deliveredDate`, failed by `failedDate`, and receipts go after
  the retention period (keep it the same on both nodes); a waiting item past it, or the oldest while the waiting
  payloads pass the cap, fails with an error naming it. The counts are grouped queries (`EdgeQueueCount`). On a
  clustered local node only the member that holds the link sends its outbox and replication; a lock row is later
  work (§2).
- **Replicate: no change log.** The entity write hook (beside the cluster's, `entity_hooks.rs`) notes a replicated
  record's key at commit (none on a rollback; a note that cannot wait for the commit sends the set whole); the sender
  drains the keys of a set into a batch and reads each record as it is then (gone is a delete). The keys wait in memory
  while the link is down and across dialers; a batch not acknowledged waits again. Each full send starts an epoch (a
  UUID), and the edge node's `moqui.edge.EdgeReplicaCursor` keeps the last epoch and sequence per node and set, written
  with the batch; a batch that does not follow it (the edge node lost its database, or the local node restarted and so
  lost its notes) gets "resync", and the set goes whole, in key order with keyset paging (after the last key, not by
  offset), after which records of older epochs are removed. A batch is at most 500 records and 4 MiB; a larger one
  goes in parts, each acknowledged. Each new link sends an empty batch first to check the cursor. A resync asked for
  during a full send (by the operator, or for more than 10,000 waiting keys) sends the set whole again after it; a set
  whose sends keep failing is tried again after a backoff from 1 s to 60 s. The copies are
  `moqui.edge.EdgeReplicaValue` (entity, key hash, `pkJson` and `fieldsJson` encrypted at rest); `get#ReplicaValues`
  and `moqui_core::edge::replica::{get_value, list_values}` read them.
- **Content (E5a).** A body too large for the payload (a mail message; later MMS media) is the item's content, up to
  64 MiB, counted with the payload in `queue-max-megabytes`. `enqueue` takes the bytes, or a location it reads, and
  keeps them at `dbresource://moqui/edge/content/out/<itemId>` in the transaction of the row. Before each `Deliver`
  the sender streams them in 1 MiB chunks over a new client-streaming RPC (`Link.PutContent` and
  `LocalLink.PutContent`, additive), and the receiver keeps them at `dbresource://moqui/edge/content/in/<node>/<itemId>`;
  each upload with a suffix of its own, so a delivery that ended late cannot remove a newer upload; the `Deliver` then
  names that location, which the receiver checks is its own for the item before the handler gets it as
  `contentLocation`. The receiver removes its copy once the delivery is answered, applied or not, since each attempt
  sends the content again; the sender's copy goes with the item at retention, and a received copy never answered goes
  after the retention period. The answer timeout grows with the content (60 s plus its time at 32 KiB/s). A receiver
  without `PutContent` (a version skew) answers unimplemented, which is "not handled", tried again every 10 minutes.
  `enqueue#Item` takes only a `dbresource://` `contentLocation`.
- **Handlers (E5a).** A handler also gets `pairedNodeId`. A result `retryLater` (the reason) rolls the receipt back and
  answers unavailable: the item waits, not counted, to the end of the retention period, for a cause that is not the
  item's (a relay or database away); an error is a counted refusal. A `queue-kind` may name a `failed-service`, which
  the sender calls for an item of the kind that failed for good (its last refusal, the retention period, the size cap,
  unpairing), so the failure is not silent.
- **Unpair and revoke.** A local node that unpairs stops noting writes for replication (`replica::disable`, so its
  `.replication.waiting` and `.lag_seconds` stop growing) and fails its pending outbox (`EqdToEdge`) with
  lastErrorText "unpaired", kept for inspection the retention period rather than delivered to whichever edge node it
  pairs with next. An edge node that revokes a node keeps that node's `EdgeHost` names, queue items and replica rows
  on purpose: a re-pair of the same local node (its instance id), even with a new key, is the same node, so it gets
  the items queued for it, and replication resyncs on the new link anyway.
- **Status and metrics.** System › Edge Node shows the outbox by kind and each set's epoch, last batch and waiting
  changes, with Pause (`pause#QueueDelivery`: deliveries to this node are answered "paused" and its outbox waits; in
  memory) and Resync (`resync#Replication`; on an edge node it forgets the cursors). Edge Clients shows each node's
  queue and replicated sets. Metrics: `moqui.edgelink.queue.depth` and `.oldest_seconds` by direction and kind
  (gauges, `Telemetry::observe_gauge_series`), `.queue.delivered`, `.retried`, `.failed`, `.delegate.duration` and
  `.delegate.errors`, `.replication.waiting` and `.lag_seconds` by set.

### 5.3 Web access

- **One name everywhere.** `erp.example.com` points at the edge node in public DNS, and at the local node's LAN address
  in the office's DNS. One name and one certificate then work inside and outside, and things bound to the origin keep
  working: session cookies, Web Push subscriptions, and the PWA.
- **Passthrough** (the default for the local node's apps, [decision 2](#8-decisions)):
  - The edge node reads the SNI, opens a tunnel down, writes a PROXY v2 header with the client's address, and pipes the
    bytes. The local node ends TLS with its own certificate, so the edge node sees no content.
  - The HTTPS listener needs PROXY headers on tunnel connections, which is new.
  - The local node needs a certificate. ACME TLS-ALPN-01 through the tunnel would do it, but there is no ACME client
    yet. Until then: DNS-01 with an outside tool, or a copied certificate.
  - Web protection runs on the local node, by the PROXY address.
- **Terminate:**
  - The edge node ends TLS with its own certificate and sends each request down as HTTP/2, adding the client's address.
    The local node trusts the link as a proxy.
  - The edge node can rate-limit and ban at the edge, and show an offline page while the link is down.
  - Used for webhooks and the mr-pass sites, which the edge node serves itself, and for the apps on a host name the
    operator sets to terminate.
- **While the link is down.** In terminate mode the edge node shows the offline page. In passthrough mode it refuses the
  connection; it holds no certificate for the name.
- **What rides on web access:** the apps and `/notws`, DAV (`/dav`), git over HTTPS, the LLM gateway, and MCP and A2A
  when they are on. SSH (SFTP, git, the TUI) goes through a TCP tunnel on an edge node port.

**As built (E2).**
- **Hosts.** `moqui.edge.EdgeHost` (in `EdgeEntities.xml`): a host name, keyed by the name, so one name belongs to one
  node; its node; its mode, `EdgeHostPassthrough` or `EdgeHostTerminate`. Names are saved in lower case with no port.
  `get#EdgeHosts`, `add#EdgeHost` and `remove#EdgeHost` manage them, and so does a Host names box on Edge Clients. The
  edge node keeps them in memory: loaded at start, tried again with a backoff until a load works, and loaded again
  after each change commits (the change runs in a transaction of its own). Only an active paired node gets a name. A
  pushed `Configure` from the wizard is E10.
- **The down connection serves the app.** It is one hyper HTTP/2 server with extended CONNECT
  (`edge_link/down.rs`). Tunnels go by `:protocol` `moqui-tunnel`. A request that names a client in
  `x-moqui-edge-client` is forwarded and goes to the local node's router, whatever its path or content type. The
  link's own gRPC is taken only with no client named, never for a forwarded request, and only on
  `/moqui.edge.v1.LocalLink/` and (E4, §5.5) `/moqui.pass.v1.PassAuthority/`; anything else is 404.
  `http/serve.rs` has the per-request part (`serve_request`) that the listeners and the down connection share.
  `http::listen` keeps what the down connection serves per factory (`edge_link::local_web`), and the dialer takes it.
  The listener now reports ready only after the edge link has started.
- **Terminate** (`edge_link/web.rs`). A request whose `Host` is a terminate host goes down after the edge node's own
  web protection, with the bodies streamed both ways. The edge node removes the hop-by-hop headers and every forwarding
  header a client sent (`Forwarded`, `X-Forwarded-*`, `X-Real-IP`, its own client-IP header and its own link headers),
  then names the client in `x-moqui-edge-client` and the scheme in `x-moqui-edge-proto`. The local node believes those
  only on the down connection, removes them and the same forwarding headers, and serves the request as from that
  client: `ClientAddr`, `ConnectInfo`, the visit and web protection all see it. On the edge node a forwarded request
  goes through the router's request log and telemetry (a layer inside `log_http` sends it down). A client's own
  `CONNECT` is answered 405. On an edge node a client-IP header with no trusted proxies is ignored, with an error
  (`ClientAddressing::of_factory`). A WebSocket upgrade becomes an extended
  CONNECT with `:protocol` `websocket`; axum's `WebSocketUpgrade` takes it (the `axum/http2` feature, and the socket
  routes also take `CONNECT`), and web protection checks it as a GET. The edge node answers the browser 101 itself and
  pipes the two. While the link is down: 503 with `Retry-After: 30`, the built-in page or `edge_offline_page`. A
  passthrough host on plain HTTP is sent to its https URL; one asked for under another TLS name is answered 421.
- **Passthrough** (`edge_link/tunnel.rs`). The edge node reads the ClientHello itself (rustls `Acceptor`), keeps the
  bytes, and picks the link's settings, a passthrough host's tunnel or the web settings by it. A tunnel writes a PROXY
  v2 header and the ClientHello's bytes, then pipes both ways. The local node reads the PROXY header (the parser moved
  from `smtp.rs` to `proxy_header.rs`), has web protection admit the PROXY address, then ends TLS with its HTTPS
  listener's own settings and serves the connection as that listener does. Nothing reads a PROXY header on a socket. With
  the link down the connection is closed after the ClientHello.
- **SSH tunnel.** `edge_ssh_tunnel_port` (on `edge_ssh_tunnel_host`) pipes each connection down a tunnel of kind ssh,
  after the edge node's web protection admits it. The local node hands the stream to its own SSH server in process, as
  a connection from the client's address, or refuses when it has none. The node is `edge_ssh_tunnel_node`, or the only
  one linked. At most 64 at once.
- **Limits.** A forwarded request's head is under the edge node's request deadline (sockets, git, DAV and uploads
  exempt, as on the local node), and opening a tunnel or sending a request head down takes at most 10 s. A ClientHello
  is at most 64 KiB (rustls's own buffer limit comes first). The down connection takes 1024 streams, of which tunnels
  and forwarded WebSockets take at most 768 per node, so the link's calls and plain requests keep room; past that a new
  one is refused. Its windows adapt to the link, and the local node's own body limits apply.
- **Metrics.** `moqui.edgelink.forwards`, `moqui.edgelink.tunnels` (by kind: https, ssh, websocket; E9 adds xmpp,
  xmpps),
  `moqui.edgelink.offline`, `moqui.edgelink.refused` and the gauge `moqui.edgelink.tunnels.active`.
- **Tests.** `tests/it/edge_web.rs` runs both ends as real servers in one process: the local node starts first, then
  the edge node with `node_role=edge`. In both modes it checks an app page, a `/notws` socket, a DAV PROPFIND and git's
  `info/refs`, and in terminate mode the local node's web protection and visit by the client's address. It also checks
  the offline page, passthrough refused, PROXY refused on the local node's sockets, a forged client header, and the
  SSH tunnel. `edge_link/down_tests.rs` checks the down connection on an in-memory connection: the client address in
  both modes, and that a forwarded request never reaches the link's gRPC. `tunnel_tests.rs` checks that a passthrough
  tunnel starts with the client's address, and `accept_tests.rs` the ClientHello reader.

### 5.4 Comms on the edge node

**Mail.** Decided 2026-11-01 ([decision 3](#8-decisions)): both layouts are options. Once it is built, the MTA in
moqui is the default, and Postfix with Rspamd is the supported external alternative, as coturn is for TURN. Spam
filtering stays with an external Rspamd for some time. Filtering in moqui is a late phase, possibly after the first
release.

- **Three stages.**
  1. **The gateway beside the edge node (E5a).** Postfix and Rspamd run on the edge node's host, as Mail-Inbound
     describes. Postfix does port 25, the SPF, DKIM and DMARC checks, DKIM signing and delivery to MX, with Rspamd's
     spam verdicts. The edge node's moqui does the rest of this section. It gets mail through the edge node with what
     exists.
  2. **The MTA in moqui (E5b), the default once built.** The edge node's moqui takes port 25 itself.
     - **Inbound:** SMTP from the internet with STARTTLS; SPF, DKIM, DMARC and ARC checks, writing its own
       `Authentication-Results`; the recipient check; connection and rate limits.
     - **Outbound:** a delivery queue to each recipient's MX with retries; DKIM signing; DSNs for bounces; TLS policy
       (MTA-STS, DANE); the edge node's reverse DNS and SPF record.
     - **Spam verdicts** still come from an external Rspamd, over its HTTP API, so the spam fields the listener reads
       today keep their meaning.
     - **A library to check:** `mail-auth` (SPF, DKIM, DMARC and ARC, verifying and signing), against the license gate
       in `deny.toml`.
  3. **Filtering in moqui (E11, late).** Content and reputation scoring in moqui, which replaces the external Rspamd as
     the default. It serves more than mail from the internet:
     - mail inside a large organization;
     - messages from public-facing forms (PASS, contact and request forms);
     - chat from outside (bridges, and later federation).

     It is a high cost, but the business need is high too, which justifies building it in.
- **The same standards inside.** The checks and signing apply to mail between the organization's own accounts, not
  only to mail from the internet. Internal mail is DKIM-signed and verified and carries `Authentication-Results`, so
  rules, filters and users see one consistent picture. The verifying and signing code therefore goes in `moqui-mail`,
  for the local node as well as the edge node, not in edge-only code.
- **Inbound path, in every stage.**
  - The edge node accepts mail only for the replicated recipients. `RCPT TO` is answered from that list, so unknown
    addresses are refused while the local node is away too.
  - In E5a, Postfix hands each accepted message by LMTP to the edge node's moqui. An edge mode of the SMTP listener
    queues the raw message, instead of storing it into an account. In E5b the MTA queues it itself.
  - The queue delivers it to the local node's delivery code (`moqui-mail/src/delivery.rs`), with the same routing,
    rules, ECAs and Message-ID check as today.
- **Outbound path.**
  - The local node's `SYSTEM` server and the accounts' outgoing servers point at the edge node. Submission is SMTP
    through a local tunnel port, which needs no change to the SMTP client, or a gRPC submit that returns an acceptance
    id.
  - The edge node queues the message, signs it with DKIM (Rspamd in E5a, moqui in E5b), delivers it to the MX with
    retries, and queues its status (sent, deferred, bounced) back to the local node's `EmailMessage`.
  - Mail the local node cannot hand to the edge node stays in its Outbox. Add automatic retry for "edge node not
    reached"; it is safe, since nothing was sent. The ambiguous lost-connection case stays as it is.
- **IMAP and POP sync, and OAuth refresh,** stay on the local node. The OAuth sign-in redirect comes back to the local
  node's own host name through web access.

**As built (E5a).**
- **Kinds and set.** The `<edge>` entries are in MoquiDefaultConf.xml, since both ends need them: queue kinds `mail`,
  `mailOut` and `mailStatus`, and the replicated set `mailRecipients` (`EmailAccount` address, kind, status and server;
  `EmailIdentity` address and account; `EmailServer` `storeHost`, so an account the sync feeds is not a recipient). The
  handlers are `org.moqui.impl.EmailServices` services with Rust bodies in `moqui-mail/src/edge_mail.rs`, granted to
  `_EDGE_`; each refuses to run on the other role.
- **Recipients.** `EdgeReplicaValue` has no index on field values, so the edge node's listener keeps the copies in
  memory as a `RecipientMap` per local node (`moqui-mail` `delivery.rs`, the same rules as `find_recipient`, plus tags
  included). It builds them again when a batch of the set is applied (`replica::applied_count`). Until the set has been
  sent whole once, an address not in it gets 451, not 550 (per node). Only nodes paired now are answered for: an
  address only a revoked node's kept copies have gets 451, so the gateway holds the mail while the node may pair
  again.
- **In.** On `node_role=edge` the listener's backend is `QueueBackend` (`moqui-host/src/smtp_queue.rs`). After the
  data it queues one `mail` item per local node: the envelope and the recipient addresses with their DSN parameters as
  the payload, never account ids; the message as content; the idempotency key from the Message-ID (or the message's
  hash) and the sorted addresses, so Postfix's retry is the same item; the conversation key the first recipient's
  account. Each recipient is answered 250 once the item commits, 451 if it fails. The local node's `deliver#EdgeMail`
  resolves the addresses again with `find_recipient` and calls `delivery::deliver` unchanged; a store that fails fails
  the item, and the retry stores nothing twice (the Message-ID check).
- **Out.** An `EmailServer` with `smtpHost` `edge:` (as `pass_backend_url=edge:`) makes a send queue a `mailOut` item
  in the caller's transaction, with the built message as content, so the send never waits on the link. The message is
  Sent once it is queued; a queue failure is `deliveryOutcome` retryable. The edge node's `deliver#EdgeOutboundMail`
  relays it with lettre to the MTA beside it (`edge_mail_relay_host`, `edge_mail_relay_port`, default
  127.0.0.1:10025), from the VERP sender `bounce+<item id>@<edge_mail_bounce_domain>` when that is set; Postfix signs
  it with DKIM through Rspamd. Not reached, a 4xx or a lost connection answers `retryLater`: the item waits, not
  counted. A 5xx, or an address or envelope the relay could never take, is a bounce. The answer goes back as
  `mailStatus` for the `mailOut` item id, queued in a transaction of its own right after the relay; an apply that finds
  it queued does not relay again, so a later rollback never sends twice. No "deferred" status is sent.
- **Bounces later.** A remote MX's delivery status notification comes back by mail to the VERP address. Postfix routes
  the bounce domain to the LMTP listener, which takes `bounce+<id>` only when it holds the receipt of that `mailOut`
  item from a paired node, reads the `message/delivery-status` part (failed outranks delayed; each recipient's status
  and diagnostic) and queues a `mailStatus` for that node instead of mail.
- **Status.** The local node's `note#EdgeMailStatus` finds its own `mailOut` item by id (nothing the edge node names is
  trusted for the message) and, for a bounce, puts a mail client message back in the Outbox (`ES_READY` with
  `sendError`, no folder) or marks a template's message `ES_BOUNCED`, only while the message is still `ES_SENT` with
  the Message-ID it went out with, so one sent again or cancelled is left. `mailOut` has a failed-service,
  `fail#EdgeOutboundMail`, which does the same with the reason for an item that failed for good on the local node
  (unpairing, the retention period).
- **Not reached.** A direct SMTP send whose server was not reached (refused, not resolved, connect timeout)
  now has `deliveryOutcome` `retryable`, so callers that retry (the forum's `DeliveryWorker.js`) do. `send#EmailMessage`
  returns `deliveryOutcome` too. A connection lost after it was made, or a TLS handshake that failed (lettre's network
  error), stays `uncertain`.
- **Conf.** MoquiEdgeConf.xml turns the listener to LMTP from Postfix on this host (`smtp_allowed_addresses`
  127.0.0.1); it starts when `.env` sets `smtp_listen_port`. Run-Deploy has the Postfix and Rspamd snippet. A store
  that fails on the local node answers `retryLater` too; a message that is not mail is a counted error, logged.
- **Test.** `tests/it/edge_mail.rs`: RCPT for known and unknown addresses; with the link down unknown still refused
  and known queued; stored once when it is back; a message over 1 MiB crosses as content; outbound with the relay not
  reached (the item waits, not counted), then through a fake relay from the VERP sender; a 5xx back as a bounce in the
  Outbox; a later notification to the VERP address bounces the accepted message, and a bounce address of an item the
  edge node never relayed is refused.
- **Manual, not automated.** Outbound mail DKIM-signed and passing DMARC at a large provider, with real DNS and
  Postfix with Rspamd: to be checked once on a deployed edge node, as E4's two-runtime check was.

**As built (E5b).**
- **Auth** (`moqui-mail/src/auth.rs`, on `mail-auth` with hickory's resolver in `dns.rs`). SPF (HELO and MAIL FROM),
  DKIM (the first ten signatures of a message), DMARC and ARC verified into an `Authentication-Results` under the
  node's authserv-id, within 45 seconds (else a temporary error); DKIM signing (RSA-SHA256, relaxed; From, Subject, To
  and Date oversigned). DMARC is RFC 9989's: `t=y` lowers the policy and there is no `pct`; a passing ARC chain last
  sealed by one of `mail_arc_trusted_sealers` overrides reject and quarantine, and with none named there is no ARC
  exception. The resolver runs on a Tokio runtime of `moqui-mail`'s own (four workers) and keeps answers to their TTL;
  a task that panics is "not checked", not a crash. `dns.rs` also answers TXT, MX and A/AAAA for E10; an address lookup
  that failed with no address back is an error, never "no address". DNS records can be fixed in its cache (tests, and
  the node's own key), and `mail_dns_offline` (testing only) makes no query.
- **The DKIM key** is configuration: `mail_dkim_key` (base64 PKCS#8 or PEM), made RSA-2048 into `runtime/.env` the
  first time a server needs it (not at every start, so tests and nodes without mail make none), `mail_dkim_selector`
  (`moqui`), `mail_dkim_domains` (else the hosted and shared accounts' domains; on an edge node from the replicated
  accounts). `EmailServices.get#DkimRecord` returns the TXT value (`v=DKIM1; k=rsa; p=...`) for E10.
- **In.** `<smtp-server internet="true">` or `smtp_internet` (on in MoquiEdgeConf.xml): no allowed-addresses needed,
  STARTTLS on by default, per-address limits (`max-connections-per-ip` 5, `connections-per-minute` 30; on the address a
  PROXY header names when that is on; IPv6 by its /64). After the data and before the answer: the checks, with its own
  `Authentication-Results` (authserv-id `smtp_authserv_id`, else `smtp_hostname`) on top and any that claim that id
  removed; Rspamd's verdict over its HTTP API (`mail_rspamd_url`, `/checkv2`) written as the listener's spam fields
  (`spam-headers`, default `X-Spam-Flag` and `X-Spam-Status`), any that came removed; reject is 550, soft reject and
  greylist 451. Fail open: Rspamd not reached is logged and the mail taken; checks that time out answer 451. A DMARC
  failure under `p=reject` is refused with 550, under `p=quarantine` marked spam (Junk). Then the edge
  `QueueBackend`, or local delivery on a normal node. The queued item carries the edge node's authserv-id and spam
  fields, which the local node trusts for it.
- **Out** (`moqui-mail/src/mx.rs`), with `edge_mail_mta=moqui` (set in MoquiEdgeConf.xml; empty or `postfix` keeps
  E5a's relay, so an upgraded E5a edge node is not switched). `deliver#EdgeOutboundMail` signs the message, queues it
  back as `mail` for the recipients that are the local node's own replicated accounts (internal mail does not go round
  through port 25), and records one `moqui.edge.EdgeMailDelivery` per other recipient domain, with the message kept
  once under `dbresource://moqui/edge/mail/`, in the receipt's transaction: the `mailOut` item is delivered once these
  commit. A thread of its own starts each row due on a thread of its own, at most eight at once, never two rows of one
  message together, without waiting for them; a try ends within 10 minutes (a minute per command), inside the row's
  15-minute lease, so a tarpit domain holds only its own thread. MX by preference (the domain's address with none, a
  null MX refused), each address in turn; STARTTLS when offered, any certificate; MTA-STS fetched over HTTPS (64 KiB at
  most) and kept to its `max_age`, and with `mode: enforce` only the policy's hosts with a valid certificate; MAIL
  FROM the VERP sender, RCPT per recipient. 2xx is sent, 5xx refused, 4xx or not reached waits: 5 minutes
  (`mail_delivery_retry_seconds`) doubling to 4 hours, until `mail_delivery_days` (5), when the rest are refused with
  the last reply. Rows of a local node no longer paired fail, logged. `mail_mx_override` (testing and staging only)
  maps domains to `host:port`.
- **Reports, no DSN.** A try's answers are committed on its row first; its report (`mailStatus` for the `mailOut`
  item) is queued after, in a transaction of its own, and one that fails is queued at the next round
  (`reportPending`), never by delivering again. While other rows of the message wait, refused recipients go back as
  "partial". The last report: "accepted" when every recipient got it; "partial" when some did and some were refused,
  which leaves the message Sent with the refused named in its `sendError` (a resend from the Outbox would reach the
  others twice); "bounced" when none did, which puts it back in the Outbox (E5a's path). So no RFC 3464 DSN is
  generated: the local node's status is the report, for a refusal and a give-up alike. A remote MX's later DSN to the
  VERP address still comes back as in E5a.
- **Internal mail.** There is no separate hosted-to-hosted path on a local node: a hosted account's send goes out
  through its `EmailServer` and comes back through the listener. A send by SMTP (`implementation.rs`) is DKIM-signed
  with the node's key when its From domain is a hosted one (kept a minute) and either `mail_dkim_key` is set or the
  server is this node's own listener, so a node sending through another provider sends no signature of an unpublished
  key, and a send never makes a key. `delivery::deliver` verifies a signature under the node's selector against the
  node's own key, with no DNS query, and adds an `Authentication-Results` when it passes and no trusted field reports
  DKIM already; another node's key under the same selector adds nothing. Through an edge node, the edge node signs and
  queues it back with its own `Authentication-Results`.
- **Queue.** An `Unavailable` answer (a handler's `retryLater`) now waits twice as long each time in a row, to the
  10-minute cap, still uncounted (`EdgeQueueItem.waitCount`, back to 0 on any other answer). An attempt in flight holds
  the attempt count up by one under its lease (the claim's fencing), given back when the answer is not counted; a
  count is read between attempts.
- **Test.** `tests/it/edge_mail.rs` `the_moqui_mta_takes_port_25_and_delivers_to_the_mx`: no Postfix; port 25 in
  internet mode with fixed DNS records, a forged result dropped, a DMARC reject refused, the rest stored once; out to a
  fake MX through `mail_mx_override`, DKIM-signed and verified against `get#DkimRecord`, a 4xx tried again, a 5xx back
  in the Outbox, a partial delivery left Sent with the refused named, internal mail queued back with `dkim=pass`.
  `tests/it/smtp.rs`: a hosted account's send to the node's own listener carries `dkim=pass` from the node's key, made
  into `runtime/.env`. Unit tests: `auth_tests.rs` (sign and verify, DMARC policies, another node's key, oversigning),
  `mx_tests.rs` (MTA-STS and enforce, a null MX, failed address lookups, retries, a session, a report that cannot be
  queued, partial and bounced reports), `smtp_tests.rs` (internet mode with a fake Rspamd and with Rspamd not
  reached, IPv6 limits by /64), `queue_tests.rs` (the uncounted backoff).
- **Acceptance.** E5a's cases pass with no Postfix, and internal mail carries verified DKIM and an
  `Authentication-Results`: met in the tests above. Not automated: delivery to and DMARC pass at a large provider,
  with real DNS, MTA-STS and reverse DNS, to be checked once on a deployed edge node.

**Chat bridges and SMS.**
- The provider's webhook URL is the edge node's, `/channels/{chatBridgeId}/webhook`.
  - The edge node checks the signature with the replicated bridge secret, drops a repeated provider transaction id,
    answers 200 once the event is queued, and queues the raw event.
  - The local node runs `SmsServices.receive#Sms` and the rest as today, without verifying again: the signature's
    300 s tolerance has passed by then. The transport-verified mark (today `WEBHOOK_MARK`) is set from the queue item.
  - While the edge node is down, providers retry, but their windows are short, hours rather than days.
- Outbound sends are the local node's own calls to the provider's API, so they need no edge node. For MMS media, the
  local node uploads the file to the edge node with the send, and the edge node serves `/channels/{id}/media/{token}`
  for `channels_media_minutes`.
- Channels that only connect outward need no edge node: Telegram's `getUpdates`, Slack Socket Mode, the Discord Gateway.

**As built (E6).**
- **Kinds and set.** In MoquiDefaultConf.xml beside mail's: queue kinds `chatWebhook` (edge to local,
  `ChatBridgeServices.apply#EdgeWebhook`) and `chatMedia` (local to edge, `store#EdgeMedia`), and the replicated set
  `chatBridges`: `ChatBridge` `typeEnumId`, `adapterName`, `statusId` and two new fields, `webhookPublicKey` and
  `webhookToleranceSeconds`. The edge node never gets `configJson`, so the provider's API key stays on the local node.
  Telnyx signs with Ed25519, so what the edge node needs is a public key, not a secret. `store#Bridge` copies
  `configJson`'s `publicKey` and `webhookToleranceSeconds` to the fields. For a bridge saved before E6,
  `chat_bridge::fill_webhook_keys` does it when the delivery worker starts, and logs an error for an SMS bridge with no
  key. The status is replicated as it is: the replica copies entity fields only, so it cannot carry a derived disabled
  flag, and a note is taken on any write to the row anyway, so leaving `statusId` out would not save the
  connected/not-connected writes. The handlers are Rust in moqui-core (`service/chat_bridge_edge.rs`), granted to
  `_EDGE_`, and each refuses to run on the other role.
- **The seam.** `ChatBridgeAdapter` has two more methods: `webhook_event_id`, the event id and their number of a
  verified request, and `apply_webhook`, the work after the checks. Telnyx's `webhook` is now verify, replay, then
  `apply_webhook`. A node that is not an edge node works as before, except that Telnyx answers a server failure
  (`receive#Sms` or `note#DeliveryReceipt` erroring, or a failed outcome other than a refusal) 503 instead of 500, and a
  text `ingest#Event` refuses for what it is (unknown bridge, bad token or transaction id, batch too large, a local
  identity) 422. The provider retries either way.
- **In.** On `node_role=edge`, `/channels/{id}/webhook` reads the bridge from the replica. An unknown bridge, or one
  that more than one paired node has, gets 401 as before; so does one whose copy has no key, logged as an error naming
  it. The edge node checks the signature with the adapter's `verify_webhook`, with the bridge's own tolerance, and a
  disabled bridge then gets 503. It queues the raw request: method, headers without cookies or credentials, the body
  in base64, the client address and the time it came. The idempotency key is the provider's event id (Telnyx
  `/data/id`, else the SHA-256 of the body, which a retry sends unchanged), so a repeat is answered 200 `duplicate` and
  queued once. The conversation key is the bridge and their number. The answer is 200 once the item commits, and 503
  when it cannot be queued. The local node's `apply#EdgeWebhook` runs only for the queue: the user must be `_edge_` and
  the item's receipt must be there. It rebuilds the request, with the time it came as its clock, sets `WEBHOOK_MARK`
  and calls `apply_webhook`, with no second signature check. `ingest#Event`'s transaction id (the event id) still makes
  a redelivery do nothing. A 2xx answer applies the item. A bridge disabled here, a database that cannot be read, or
  409, 429 or 503 (a server failure) answers `retryLater`, and the item waits, not counted, to the end of the
  retention period. A refusal of the data (422, 400), a panic, a bridge not on the local node and a malformed item are
  counted errors, retried with the queue's backoff up to `queue-max-attempts`.
- **MMS out.** On a paired local node (`edge_node_url` set, the edge link built in and installed) whose conf lists
  `chatMedia`, `chat_bridge::media_urls` (which Telnyx's send now calls) puts each file of a send on the edge node. A
  Lite or Dev build has no link, so it sends by its own address. It queues the `ChatAttachment`'s file as a `chatMedia`
  item's content, in a transaction of its own. While the link is up it waits up to 2 s for every item to be delivered.
  Otherwise the send is a new `DeliveryOutcome::Wait`: the attempt is given back and the send is tried again after
  5 s, so a waiting MMS never fails for attempts and the worker is not held while the link is down. The provider
  never fetches an address the edge node does not have yet. A failed item makes the send fail (`mediaUnreachable`).
  The token is an HMAC under the local node's `channels_media_key` of the delivery's idempotency key, the file and a
  window of half `channels_media_minutes`. So an attempt within that window names the same item, and a later one
  sends the file again and gets an address with at least half its life left. The address is under the local node's
  `channels_public_url`, which on a paired node names the edge node, the same host as the webhook URL. The edge node
  keeps the file as `moqui.edge.EdgeMedia` (the token's SHA-256, node, bridge, file under
  `dbresource://moqui/edge/media/`, expiry after the local node's minutes, at most its own `channels_media_minutes`).
  A file over `channels_edge_media_max_bytes` (5 MiB), or past `channels_edge_media_node_megabytes` (256) of the
  node's files, is refused, logged and counted. It serves `/channels/{id}/media/{token}` only for that bridge, of an
  active paired node, until the expiry. The files that ran out are removed by the link's retention pass, by each new
  file, and when asked for, each in a transaction of its own. Inbound MMS stays local (`store#BridgeMedia` fetches
  from the provider).
- **The queue's order.** Items go one at a time per conversation key, and `chatMedia` has a key per file, so it never
  waits behind a mail message's key. A delivery round does send its due heads one after another, so a file can wait
  for a large mail upload in the same round: seconds, and the send then waits, not counted.
- **Conf.** MoquiEdgeConf.xml sets `channels_webhooks_enabled=true`. On the local node, `channels_public_url` is the
  edge node's address.
- **Test.** `tests/it/edge_chat.rs` (about 5 s). With the link down, a Telnyx-signed text to the edge node gets 200
  and is queued, its repeat is dropped, and a bad signature is refused there. When the link is back, the text is taken
  once on the local node (`receive#Sms` and `ingest#Event`, one `ChatEventExternal`). A disabled bridge gets 503 at the
  edge node; a bridge with no key in its copy 401; one saved with only `configJson` gets its key from
  `fill_webhook_keys`. An MMS sent through the Telnyx adapter to a fake Telnyx waits for its file, then carries the
  edge node's address. The edge node serves that address for its bridge only, and refuses it, and removes it, once it
  ran out. A file over the edge node's limit is refused there while the send waits. An MMS while the link is down
  waits, not failed, and goes when it is back. The retention sweep removes files that ran out. The replicated copy has
  no API key.
- **Not done.** A real Telnyx round trip is checked by hand, as E5a's DMARC check is.

**XMPP.**
- First, tunnel 5222 and 5223 to the local node, with `/xmpp-websocket` over web access.
  - The local node holds the certificates for the domain, `rooms.` and `upload.`.
  - Clients cannot connect while the local node is away. They retry, and their messages wait in MAM as today.
- C3b's s2s belongs on the edge node, with stanzas to local users queued while the local node is away (the split
  below).

**As built (E9).**
- **Tunnel ports.** `edge_xmpp_port` (STARTTLS, 5222) and `edge_xmpp_tls_port` (direct TLS, 5223) on
  `edge_xmpp_host`, each off while empty, pipe each connection down a tunnel of kind `xmpp` or `xmpps`, after the
  edge node's web protection admits it, as the SSH tunnel does (`edge_link/tunnel.rs`, one listener loop for all
  three). The node is `edge_xmpp_node`, or the only one linked. At most 256 XMPP tunnels at once, both ports
  together; they count toward the link's 768 tunnels and WebSockets too.
- **The local node** (`edge_link/down.rs`, `xmpp::serve_tunneled`) serves the stream in process as a client
  connection from the address the edge node names: the source's block for failed sign-ins and its connection count
  (20 a source, 10,000 in all) apply as on its own ports, and it runs on the XMPP server's runtime. The TLS ends
  there with its own certificates, so SCRAM-SHA-256-PLUS has the session's exporter and FAST's `HT-SHA-256-EXPR`
  works. The local node needs no XMPP listener of its own: each kind's TLS is made when a listener or the first
  tunnel needs it. A node with no XMPP server, or no TLS for that kind, answers the tunnel 404, which the edge node
  counts as refused. The address is taken canonical (an IPv4-mapped one as IPv4). The limits are per source, so
  the edge node must see clients' own addresses (no TCP load balancer in front of it without PROXY).
- **moqui-xmpp.** `conn::serve_stream` serves any byte stream (`AsyncRead + AsyncWrite + Unpin + Send`);
  `serve_tcp` sets `TCP_NODELAY` and calls it. Each write is flushed, as a tunnel may buffer. C3b's s2s streams will
  run on the same.
- **No XMPP server on an edge node.** `xmpp::start` checks the factory's role (`edge::settings(factory).edge_node`)
  and logs a warning when `xmpp_domain` is set there.
- **Settings the operator keeps in step.** Chat apps (`get#ClientSettings`) shows the local node's `xmpp_port` and
  `xmpp_tls_port`; give the edge node's tunnel ports the same numbers (5222, 5223), so what it shows works inside
  and outside. The SRV records (`_xmpp-client._tcp`, `_xmpps-client._tcp`) name the edge node in public DNS and the
  local node in the office's. Uploads (`xmpp_upload_url`, `https://<domain>` by default) and `/xmpp-websocket` ride
  on web access, so the XMPP domain must be an `EdgeHost` of the node; in terminate mode the WebSocket has no
  exporter, so it signs in with SCRAM-SHA-256 without `-PLUS`.
- **Tests.** `tests/it/edge_xmpp.rs` (two servers in one process, about 4 s): the edge node inherits the process's
  `xmpp_domain` and starts no XMPP server; before the link the STARTTLS port answers nothing; a tokio-xmpp client
  over the direct TLS port (SCRAM-SHA-256-PLUS) and a raw client over the STARTTLS port (STARTTLS, then SASL2 with
  SCRAM-PLUS and Bind 2) sign in through the edge node and trade direct messages, which are in the local node's
  direct room; with the link down both ports are refused. `down_tests.rs`: both kinds refused on a node with no XMPP
  server.
- **Not done.** A real client (Conversations) through a deployed edge node is checked by hand. The wizard's XMPP
  area is built (§5.7, "As built (E10, XMPP)").

**XMPP federation (C3b) with an edge node.** The design C3b builds to ([Comms-Framework](Comms-Framework.md) §4.3).
- **On the edge node:** the 5269 listener (`xmpp_s2s_port`, documented and off; nothing listens yet) and the
  outbound s2s streams, with its own certificate for the XMPP domain (and `rooms.`); SASL EXTERNAL with the remote's
  certificate checked by WebPKI for its domain, and dialback (XEP-0220) as the fallback, with its dialback secret and
  the remote's `_xmpp-server._tcp` lookups; the RFC 6120 domain checks (a stanza's `from` is the authenticated domain,
  its `to` is ours), the stanza size and depth limits, and per-domain limits. All of it answers whether the local node
  is up or not. The s2s code is moqui-xmpp's, over `conn::serve_stream`'s byte streams.
- **Replicated** (local to edge node): which local JIDs exist (users, and rooms with federation on, §4.4), so a
  stanza to anyone else is refused at the edge node (`service-unavailable`), the local node away or not.
- **Inbound, queued:** messages and subscription presence to a local JID go on the queue (kind `xmppIn`: the
  stanza and the domain it was authenticated as), with the conversation key the local bare JID, so each user's and
  each room's stanzas keep their order and one busy room does not hold back the others. The local node's handler
  (`XmppServices.receive#RemoteStanza`, granted to `_EDGE_`) routes it as from a remote sender.
- **Inbound, delegated:** an iq to a local JID (disco, MUC, PEP) is delegated and answered over the s2s stream;
  while the local node is away it is answered `recipient-unavailable` (type wait). Ephemeral presence (available,
  unavailable, a MUC join) is not kept: a join is refused the same way.
- **Outbound:** the local node's stanzas to remote JIDs (moqui-xmpp's `Outbound`, so XEP-0357 push too) go up on the
  queue (kind `xmppOut`, conversation key the remote bare JID), and the edge node sends them on its s2s stream to
  that domain, trying again while the remote is unreachable; one that cannot be sent in time comes back as an error
  stanza on `xmppIn` to its sender. Through the edge node rather than dialed from the local node: one s2s identity
  (one certificate, one dialback secret), and an office network that lets out only 443 still federates.
- **While the local node is away:** remote servers' messages to local users wait on the edge node and arrive in
  order, once; iqs and joins are refused as above; dialback checks are still answered; nothing is sent out, as the
  local node is the source. Clients are tunneled as today and cannot connect.
- **Without an edge node** the same code runs in one process: inbound to the session registry, outbound straight to
  the s2s streams.

**Matrix federation (C6b) with an edge node.** The federation endpoint (`/_matrix/federation/`, `/_matrix/key/`, on
443 with `.well-known` delegation, or 8448) is the edge node's own, served there, never forwarded. It checks each
request's X-Matrix signature against the origin's keys (fetched and cached on the edge node), and each transaction's
PDUs (hashes, signatures) and EDUs, then queues them (kind `matrixTxn`, conversation key the room id) and answers 200.
The local node keeps the room DAG, the auth rules, state resolution and the private signing key. Key queries
(`/_matrix/key/v2/server`) are answered from a copy: the local node signs its key response with a validity of days
and replicates it, so the edge node answers while it is away. Backfill, state, `make_join` and `send_join` need the
local node and are delegated, failing while it is away. Outbound federation requests are signed on the local node,
which needs the private key, and dialed from there (HTTPS out), or sent through the edge node when C6b wants one
public address. The client-server API (C6a) goes to the local node through web access.

**Voice (SIP).**
- **On the edge node.** It runs the SIP listeners and RTP, with `sip_public_address` set to its fixed address. Trunks
  register from the edge node. Phones outside the office register to the edge node's registrar, which delegates the
  digest check to the local node. The HA1 verifiers (`SipDeviceVerifier`) are not copied to the cloud, so outside phones
  fail to register while the local node is away and register again when it is back ([decision 4](#8-decisions)).
- **The edge node's `SipNode` row.** The local node keeps it from the link, as it keeps the `CallMediaNode` row.
- **An incoming call.** The edge node delegates `receive#PhoneCall` to the local node for the routing decision. All the
  media stays on the edge node (phone RTP and browser legs alike), so the local node is in signaling only.
- **While the local node is away** (decision 4). The edge node answers from the replicated fallback:
  - the number's voicemail box and greeting, or a forward to the number's outside fallback;
  - a voicemail is recorded on the edge node and queued, and its notices go out when the local node takes it;
  - without a fallback, it answers unavailable, so the trunk's own failover can act.
- Recordings and voicemail audio are queued to the local node. The edge node deletes its copy after the receipt.

**Calls (WebRTC).**
- The edge node is a media node of the local node's calls.
  - The local node keeps the call state (`CallSession`, `CallLeg` and the rest) and the signaling. The browser's
    `/notws` reaches it through web access.
  - The local node keeps the edge node's `CallMediaNode` row from the link: the heartbeat from link liveness, and
    `mediaAddress` and `turnUrls` from Hello.
  - `cluster::send_to` for the edge node's instance id goes over the link instead of the cluster sender.
- On the edge node, the rtc driver's service calls are delegated or queued: media events (`note#MediaEvent`) and
  captions are delegated, and recording segments are queued. Binding a leg and TURN credentials (`get#IceServers`) are
  already the local node's, from the browser through web access ([§4a](#4a-entity-access-audit)).
- TURN runs on the edge node with `turn_allowed_peers` set to the edge node's media port. coturn also works.
- **Office-to-office calls.** Office users in a call with each other send media to the cloud and back. That is
  accepted ([decision 6](#8-decisions)). A local media node, cascading to the edge node for outside legs, stays a
  later option. W7 needs the shared database today, so edge-mode cascading is new work.

**As built (E7).**
- **Hello.** New fields (additive): `Hello.media_address` (8), `turn_urls` (9) and `media_started_ms` (10), the edge
  node's while its media socket is up (`moqui_core::edge::media::set_advert` from `rtc::start`), and
  `HelloReply.turn_secret` (3). `EdgeInfo` keeps the edge node's `instance_id`, `media_address`, `turn_urls` and
  `media_started_ms`; `get#LinkStatus` shows the address and URLs.
- **No cascading.** `bindNode` keeps every leg of a call whose media node is the edge node there, also with
  `rtc_cascade_enabled`: the link carries no relay links between media nodes (each end drops `relay` and `roomFrame`).
- **One local node** (decision 1). The media driver keys rooms by call alone, so an edge node takes call messages
  (frames, controls) from a local node only while exactly one is linked; with two, each is refused, logged. Rooms keyed
  by node come with several local nodes per edge node. The start time is the row's `startedDate`
  (`register#MediaNode` takes it), so `expire#Recordings` fails a recording that began before an edge node restart.
- **The edge node's id is pinned.** Everything below trusts the edge node's instance id, so pairing keeps it beside
  its key, as `edge_node_id` in `runtime/.env` (`moqui_core::edge::NODE_ID`; unpairing removes it). A link whose Hello
  names another id is refused (state Refused, "its instance id changed ..., so pair again"), and so is an id that is
  this node's own or a live member of its cluster (`cluster::is_live_member`), checked before the link carries
  anything (`dial.rs` `check_peer`). A node paired before E7 has no pinned id: its first link pins the id it names and
  writes it to `.env` (trust on first use, on a link whose key was already pinned), rather than asking for a re-pair.
  `register#MediaNode` never turns a row a media node registered into an `edgeLinked` one, nor the reverse (refused,
  logged), so an edge node cannot take a media node's row or calls.
- **The row.** The edge node registers no `CallMediaNode` (`rtc::start` skips the register loop on
  `edge::settings(factory).edge_node`). The local node's dialer writes it with `register#MediaNode` (new `edgeLinked`,
  a new `CallMediaNode.edgeLinked` field) as each link comes up and every `ping_every` (15 s) while it lasts, a task
  of the link's `Work`; a link down leaves it to go stale (60 s), so a short loss ends no call, a longer one ends them
  as "media server lost" (`expire#Calls`). An edge node without media has its row removed at the next link, and
  unpairing removes it: after the dialer has stopped and its link's work ended, every `edgeLinked` row, whether or not a
  link came up since the process started. `Calls.js` `liveMediaNodes` puts `edgeLinked` rows first, so a paired node's
  new calls have their media on the edge node even with a media node of its own.
- **Signaling seam.** `moqui_core::edge::signal`: a route per node at the other end of a link (the dialer adds its edge
  node's by instance id when the link is up; the edge node each local node's as its down connection attaches; a guard
  removes it with the link). `cluster::send_to` asks it first, so the browsers' frames (`notws/rtc.rs` forward), the
  driver's replies and relay messages, `send#MediaControl` and E8's `send#SipControl` go over the link. A route hands
  the message to a bounded queue (1,024, `overloaded` when full) that one task sends on a client-streaming call, new
  RPCs `Link.Signal` and `LocalLink.Signal` (`edge_link/signal.rs`), so messages keep their order without a round trip
  each; a call the link lost is opened again for the next message, and what it carried is not sent again (one try, as
  the cluster's direct send); one the call cannot take is dropped with a warning and counted. A node that has had a
  route in the process and has none now is refused ("the edge link to .. is down"), never published here or posted to
  the cluster. The receiver takes only what the other end may send, in order: a local node only `reply` messages,
  each delivered only to a socket that bound that leg of that call with its media on that edge node (topic
  `rtc:<call>`, `to` or `callLegId`; `notws/rtc.rs` `deliver_edge_reply`), never a frame, control or relay; an edge
  node only `frame` (whose `origin` must be the sending node), `control` and `sip`, published on its bus
  (`signal::receive`, `rtc-signal` only). The design named a hook on `EdgeLink`; a route registry instead works for any
  dialer (a test's too) and needs no lookup of the link per message. It is per process, as the bus is, keyed by
  instance id. **A clustered local node:** only the member holding the link has the route, so a call's socket on
  another member is refused; relaying through that member is later work (WebRTC.md).
- **Delegated:** `CallServices.note#MediaEvent`, and new `CallCaptionServices.get#CaptionSpeaker` (Rust in
  `call_media.rs`, replacing the host's entity reads in `speaker_of`, on every node), `store#CallTranscript` (replacing
  the entity-auto create; it checks the leg is the call's) and `transcribe#CaptionAudio` (the WAV chunk as base64, at
  most 4 MiB; the user must be a participant of the call; a provider's refusal comes back as `errorCode`, so captions
  keep its code; `userId` is required). An edge node's captions use `DelegatedTranscriber`; other nodes call
  `llm_hooks::transcribe` as before. In MoquiDefaultConf.xml's `<edge>`, granted to `_EDGE_`. Run as `_edge_`, these,
  `note#MediaEvent` and `receive#EdgeRecording` refuse a call (or recording) whose media node (the leg's, else the
  call's) is not an `edgeLinked` row. The host's service calls for captions and the media row keep their errors apart
  from the caller's (`push_errors`, `pop_errors`).
- **Queued:** recordings, kind `callRecording` (edge to local), handler `CallRecordingServices.receive#EdgeRecording`
  (runs only as `_edge_`). The edge node's recorder queues each closed segment with its parameters as the payload and
  the file as content, keyed `<callRecordingId>/<file>`, and its end (`note#RecorderEnd`) keyed `<id>/end`, all under
  the conversation `callRecording:<id>`, for the only paired node; the spool file goes once the item is queued (a
  failure fails the recording, which removes the spool). `store#RecordingSegment` takes `fromLocation`
  instead of `tempPath`, and `store#RecordingFile` copies a `fromLocation` only under `CONTENT_IN`. An edge node's
  segments are at most 32 MiB (`Limits::for_edge`), and MoquiEdgeConf.xml sets `call_recording_segment_mb` 16.
- **TURN.** The edge node's `turn_auth_secret` goes to the local node in the Hello answer only (a Hello also answers
  Probe before pairing), kept in memory (`edge::media::set_peer_turn`), never logged or stored, forgotten at unpairing.
  `make#IceServers` (new `mediaNodeId`, which `Calls.js` passes) signs the edge node's TURN URLs with it and
  `turn_external_urls` with the local node's own, as two entries. Chosen over sending the local node's secret to the
  edge node, which would put a secret that also opens the local node's other TURN servers on the cloud host.
  `turn_allowed_peers` needs nothing: the edge node's own media port is the default peer. XMPP's `get#IceServers`
  passes `mediaNodeId` too. Revoking a local node does not revoke its TURN access: rotate the edge node's
  `turn_auth_secret` after a revoke (TURN.md); a secret per pair needs the TURN username to name the pair, later work.
- **Conf.** MoquiEdgeConf.xml sets `rtc_media_port` 10000, `rtc_media_host` 0.0.0.0, `turn_port` 3478 and
  `call_recording_segment_mb` 16; `rtc_public_address` goes in `.env` (the media socket does not start without it when
  binding every address).
- **Test.** `tests/it/edge_call.rs` (about 6 s): the edge node in a child process of the test binary, as W7's leg node
  (the media driver, the socket bindings and the bus are process globals), on a database of its own, paired by the
  local node's real instance id. An outside browser stand-in relayed by the edge node's TURN, with the credential the
  local node signed (each node with its own `turn_auth_secret`), and an office one on UDP: the row from the link, the
  call's media node the edge node, the answer from its media socket, media both ways, both legs' `joinedDate` set on the
  local node by the delegated `note#MediaEvent`, and a recording on the edge node whose two tracks and mix are stored
  on the local node as Ogg files. Both browsers signal on the local node's `/notws` directly; through web access is
  E2's test. Unit tests: `edge/signal_tests.rs`, `call_media_tests.rs` (the content location check, the ICE server's
  credential), `recorder_tests.rs` (an edge node's segment cap). Specs: `Calls.spec.js` (the edge node's row preferred,
  removed at unpairing), `CallRecording.spec.js` (`receive#EdgeRecording`: refused for another user and for a location
  outside the queue's copies, a segment stored, the end failing the recording).
- **Not done.** Captions through the edge node are not run end to end (no transcription provider in the tests); a
  link down fails a delegated media event or caption at once, and the call goes on. Several local nodes on one edge
  node (decision 1): routes are per node, but recordings go to the only paired node and delegated calls need one link.
  Real browsers over the internet, with the edge node's real address, are to be checked by hand.

**Push.** No change. The local node sends Web Push itself.

### 5.5 mr-pass on the edge node, gRPC by default

- **Settings.**
  - `pass_backend_transport` is `grpc`, `rest`, or empty. Empty means automatic: `grpc` where the build has it, and
    `rest` on Lite ([decision 8](#8-decisions)).
  - `pass_backend_url` keeps naming an authority to dial in direct mode.
  - A new value, `edge:`, means the local node on the link. Local mode (in-process calls) stays the default when it is
    empty.
- **PASS seam.**
  - Split into transport adapters behind one function:
    - `Facade.js` `authorityClient` and the remote branch of `call`;
    - `Account.js` `remoteAccount` and `callAuthority`;
    - `download`, `downloadExport` and multipart uploads.
  - The REST adapter is today's code.
- **Authority seam.** Factor the logic in `PassApi.js` that runs after authentication into one function that the
  servlet and the gRPC handler both call: the client check, the delegation, the operation to schema to service step,
  and the error envelope.
- **The service, first cut.**
  - `PassAuthority.Call(operation, siteId, input as JSON bytes, delegation, visitor address, operation id, session
    handle)` returns the result as JSON, or the error envelope.
  - `Download` streams from the server, and `Upload` streams from the client.
  - The account and SSO actions are operations like the rest.
  - This matches today's one-input-map, one-result-map shape ([decision 7](#8-decisions)). Typed messages would
    need typed results, and only 3 of the 149 have them (`Contracts.d.ts`). They may come later, for efficiency or for
    what they make possible, perhaps generated by `moqui types`.
- **What it gains.** One long-lived, multiplexed HTTP/2 connection, with no TLS handshake or header parsing per call.
  On the link it also means no public authority at all.
- **Script side.** A host binding beside `ec.service.rest()` makes gRPC calls to a configured endpoint: a direct
  channel, or the link.
- **Authentication.**
  - On the link, the link's identity replaces the client secret header. The edge node becomes a PASS client at pairing,
    and the wizard writes `pass_client_<id>_*` into the local node's `.env`.
  - Direct gRPC keeps the client secret, in metadata. Mutual TLS or request signing would be better, later.
  - The second-factor cookie becomes an explicit session handle in metadata.
  - The SSO front channel goes to the authority's host name, which in edge mode is the local node's, through web
    access.
- **Packages.** Lite, the package Run-Deploy suggests for PASS servers, has no `grpc` and keeps REST
  ([decision 8](#8-decisions)). REST on Lite is a supported mode for the long term, not a stopgap, and it is the
  default there. The edge node itself is Maximal.
- **While the local node is away.** Every PASS page needs the authority today. A read cache on the edge node, scoped by
  publication generation as `plan/design-forum.md` describes, lets public pages read while the local node is away.
  Writes fail closed with a notice, and the Shop takes no offline orders (`design-shop.md`). Later.
- **Plan text to revise:** `design-shop.md:1333-1337` ("no new REST or GraphQL server") and
  `implementation-forum.md:16` ("one configured HTTP authority").

**As built (E4).**
- **Proto.** `crates/moqui-host/proto/pass.proto`, package `moqui.pass.v1`, a file of its own because the direct
  listener serves it too; generated and checked in as `edge.proto` is (`src/pass_authority/proto/`,
  `MOQUI_REGENERATE_PROTO=1`). `Call`, `Download` (metadata or the error first, then 64 KiB chunks) and `Upload` (the
  call, the file's metadata, its bytes; over 10 MiB the REST API's 422). JSON input and result, the REST API's error
  envelope with its status; typed messages may come later (decision 7). A request message is at most 2 MiB, an answer
  16 MiB.
- **Authority seam.** Mantle's `script/PassAuthority.js` `handle(p)` runs after the transport's checks: the client's
  sites (`clientRegistered(..., verified)`), the delegation, the write rule for a call with no browser session (a
  write needs a delegation unless it is an account action, an opt-out or a bearer link's token write), routed
  operations only, the schema's fields, `account.<action>` and the SSO back channel, the error envelope. `PassApi.js`
  is the REST adapter (headers, CSRF, the JSON body, the bearer as `/^Bearer (\S+)$/`, the `sso`/`sso-complete`
  redirects); gRPC runs `mantle.pass.PassAuthorityServices.handle#Call` (`transaction="ignore"`). The framework added
  `check#Client`, and `requesterKey` reads `ec.user.context.passRequester`, set by `handle` for a registered client.
  The client the transport authenticated is `ec.user.context.passVerifiedClient`, which `handle` sets from its
  `clientVerified` (only `handle#Call`, `allow-remote="false"`, passes it) and which the account services
  (`registeredClient`) read instead of a flag in their input; `handle` clears both before and after each call.
- **REST answers that changed** in the refactor, since a credential is now checked in `handle`, after the route: with
  a credential that is not good (from a registered client), an unknown route is 404 (was 401), a non-POST or unknown
  account action 404 (was 401), and a body that is not a JSON object 422 (was 401); the SSO front channel ignores the
  bearer (it was checked); an empty or malformed `Bearer` is no credential, so it no longer skips the CSRF check or
  the write rule. A bearer from an unregistered client is still 401 first.
- **Session handle.** The second factor's state stays in the authority's framework session (`moqui.server.HttpSession`
  and its expiry). The handle is `p1.<session id>.<MAC>`, the MAC under a key derived from `session_cookie_key` for
  this use only (as the MCP requestState's), so it is never a `moqui.session` cookie (`session::bind_id` binds the id
  it names). A session made for an account action is marked with its client and site (`passClientId`, `passSiteId`);
  a handle of another client or site, or of no PASS session, gets a new session. Once a delegation is issued the
  session's user is cleared (not `logoutUser`, which would sign the user out everywhere), so a handle carries only a
  sign-in in progress. The SSO back channel binds no session. REST still carries the session as its cookie and keeps
  it signed in in the frontend's server-side session, as before E4.
- **Host.** `src/grpc_listen.rs` (the `grpc` feature) is the host's gRPC listener, the OTLP routes and the PASS
  authority's (the `edge` feature; the direct listener is off unless `pass_authority_grpc_enabled=true`) on one port:
  the receiver's address when that is on, else
  `pass_authority_grpc_host`:`pass_authority_grpc_port` (4317); TLS with the HTTPS certificates, else h2c, and then the
  PASS routes only on a loopback address or with `pass_authority_grpc_insecure=true` (an error logged otherwise). A
  direct call names its client in `x-pass-client` (or `client_id`) with `x-pass-client-secret`; `check#Client` runs on
  the request pool first, for an upload on its first part before the file is read; wrong secrets are counted per
  address, refused for the rest of a 10-minute window after 10, the first and tenth logged. On a local node's down
  connection the service shares `LocalLink`'s `Routes` and rule (no client named, or it is not reached); its client is
  `pass_edge_client` (default `edge`), with `pass_client_<id>_sites` and `_callback` and no secret; pairing writes
  nothing for it. Calls run `pass_authority_service` (default Mantle's) on the request pool; they are not fenced by
  the link generation.
- **Core and scripts.** `moqui_core::pass_authority`: `serve`, `serve_download` (the file the service names, its
  location never sent), `check_client`, the handle, and the `PassSender` slot, which the host fills: `grpc://` and
  `grpcs://` on one kept HTTP/2 connection each (WebPKI roots only), `edge:` down the only link
  (`EdgeServer::only_link`, shared with delegated calls). The host installs the sender at server start, with or without
  listeners, and clears it (closing the kept connections) when the server stops; before this node's edge link runs,
  `edge:` is "local node offline". Scripts use `ec.service.passAuthority(endpoint)`; with no
  sender (Lite, Dev) it throws "gRPC not built into this host".
- **mr-pass.** `service/pass/Transport.js` has the REST adapter (the old code) and the gRPC one behind the same
  functions and error kinds. The transport rule: `pass_backend_transport` `rest` or `grpc` forces one; empty is gRPC
  when the host has the binding and a gRPC endpoint is set (`pass_backend_url=edge:`, or `pass_backend_grpc_url`),
  else REST, so a frontend upgraded to Maximal does not move to an authority port nobody opened. `grpc` with no URL
  dials the `pass_backend_url` host on 4317; `edge:` takes gRPC only. A gRPC GET sends text values and no nulls, as
  REST's query does. `pass_backend_web_url` is the authority's web address for the SSO front channel.
- **Tests.** `tests/it/edge_pass.rs`, with a stub authority service: Call, Download and Upload direct and on the link,
  the session handle across a rotation, refused for another site and another client, never a cookie, without a user
  once a delegation is issued; a wrong secret refused, an upload before its file; then "local node offline".
  Mantle's `PassAuthority.spec.js` runs `handle` and the servlet; mr-pass's `Transport.spec.js` the transport rule.
  Mantle's own `handle#Call` through the host is not tested there: loading mr-mantle in a host test takes longer than
  a fast test allows. The two-runtime compatibility check over each transport is in mr-pass's Deployment.md.

### 5.6 MoquiEdgeConf.xml and packaging

Built in E1 as `runtime/conf/MoquiEdgeConf.xml`: `instance_purpose` production, `node_role=edge`, web protection, and
the entity facade's crypt-pass. The properties of what is not built yet are comments in it, each with its phase:
`channels_webhooks_enabled` (E6, now set) and the rtc and TURN ports (E7, now set). E4 set `pass_backend_url=edge:`, so mr-pass calls
its local node over the link. E3 filled in its `<edge>` element: the queue's limits, with the
delegated services and replicated sets as commented examples for the comms phases; E5a moved mail's entries to
MoquiDefaultConf.xml and set the listener to LMTP. The draft below is the target. The listeners that need the
operator's own choices stay off until `.env` sets them: SIP ports, the XMPP domain, LMTP from the gateway
(`smtp_listen_port`).

```xml
<?xml version="1.0" encoding="UTF-8" ?>
<!-- An edge node (plans/Edge-Node.md): the production conf for a small cloud instance in front of a local node.
    Build with --release (the Maximal package: rtc, sip, xmpp, inbound-smtp, channels, grpc and embedded Postgres).
    The runtime has the base components and mr-pass only. Per-host settings (names, addresses, ports) go in .env.
        ./moq run-production --release -c conf/MoquiEdgeConf.xml -->
<moqui-conf xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:noNamespaceSchemaLocation="http://moqui.org/xsd/moqui-conf-3.xsd">
    <default-property name="instance_purpose" value="production"/>
    <default-property name="node_role" value="edge"/>
    <default-property name="webapp_web_protection" value="true"/>
    <default-property name="channels_webhooks_enabled" value="true"/>
    <default-property name="rtc_media_port" value="10000"/>
    <default-property name="rtc_media_host" value="0.0.0.0"/>
    <default-property name="turn_port" value="3478"/>
    <default-property name="pass_backend_url" value="edge:"/>

    <entity-facade crypt-pass="${entity_ds_crypt_pass}" query-stats="true"/>
    <!-- queue retention and size cap, delegated services and replicated sets: the elements are settled in E1 and E3 -->
</moqui-conf>
```

- **"Release build profile."** `--release` is the Maximal package, built with the `release` Cargo profile. Enterprise
  has no embedded Postgres, and the edge node uses the embedded one.
- **Components.** A conf adds components and never removes one, so an edge node runtime is a checkout or archive with
  only the base components (framework, tools, webroot, basic-comms) and mr-pass. An `edge` component set in `addons.xml`
  makes that `./moq component-get edge`. Then `./moq archive --release --no-data --conf conf/MoquiEdgeConf.xml`. A set
  names only components from `addons.xml`, so `edge` is mr-pass alone; the base components come with mr-moqui.
- **`node_role`** is empty on a normal node, or `edge`. A local node needs no role: it links when `edge_node_url` is
  set.
- **Backups.** The queue is the only data on the edge node that matters, so the conf should run the embedded backup job
  instead of leaving it paused. A conf cannot unpause a job, which is seed data, so MoquiEdgeConf.xml says to unpause
  `backup_EmbeddedDatabase_daily` in System › Service Jobs.
- **Build cost** (measured in E0, in the dev profile: the Debug package, `cargo build -p moqui`):
  - **Nothing new to compile in Maximal or Enterprise.** Their graph already has all the link needs:
    - tonic 0.14.9 with `channel`, turned on by moqui-core's `grpc` through opentelemetry-proto's `gen-tonic`;
    - tonic-prost 0.14.9, prost 0.14.4 and h2 0.4.20;
    - hyper 1.12 with `client` and `http2` (through reqwest's `http2`), and hyper-util 0.1.21 with `client`,
      `client-legacy`, `http2` and `server-auto` (through tonic's server).

    So an `edge` feature in Enterprise adds no crate and no dependency feature. Turning it on recompiled only
    moqui-host and the binary.
  - **Linking the link's code** (hyper's HTTP/2 client and server, a tonic channel and a generated client) adds 2.1 MB
    to the binary, 0.7% (313,281,368 to 315,394,216 bytes). A one-line rebuild: 4.3 s before, 4.3 to 5 s after.
  - **Lite** has 423 crates, and already has prost, h2 and hyper's `http2`. `grpc` would add 5 crates (tonic,
    tonic-prost, hyper-timeout, pin-project, pin-project-internal) and turn on hyper-util's `server-auto` and more of
    tower (balance, buffer, discover, limit, load). Its size cost was not measured. Lite keeps REST (decision 8).
  - **Code generation.** protoc is not installed, and builds must not need it.
    - The `.proto` files go in `crates/moqui-host/proto/`, package `moqui.edge.v1`.
    - tonic-prost-build 0.14.9 generates the Rust, with protox 0.10.0, a protobuf compiler in Rust, in place of
      protoc. Both build on prost 0.14. They and their crates (tonic-build, prost-build, prost-types, prost-reflect,
      protox-parse, miette) are all MIT or Apache-2.0, in `deny.toml`'s allow list.
    - They are dev-dependencies only. The generated code is checked in, and a test regenerates it with
      `MOQUI_REGENERATE_PROTO=1`, as goldens are. A normal build has no build script for it.
- **Effect on a node that is not an edge node** (nor paired), as built through E4:
  - `edge_link_key` is a secret default property (`MoquiDefaultConf.xml`), so it is generated into `runtime/.env` on
    every node at its first start, used or not.
  - The seed data (`EdgeSetupData.xml`) loads the `_edge_` user, the `EDGE_DELEGATE` group and its artifact group on
    every node; `_edge_` has no password and only that group's grants.
  - In a build with `edge` (Maximal, Enterprise): when the listeners start the edge link hook is installed and the
    `moqui.edgelink.up` gauge reads 0; the PASS authority sender is installed at server start; the HTTP app has the
    `edge_forward` layer, one request extension check per request; a service call costs one atomic load (no `<edge>`
    lookup unless some factory delegates) and a committed entity write one more (no replication unless the link
    enabled it). The `<edge>` conf is read only when the listeners start (at boot on an edge node).
  - The gRPC listener's log line names what it serves (`gRPC listener (OTLP/gRPC receiver) on <address>`, with `and
    PASS authority` only when that is on); with the OTLP receiver off and `pass_authority_grpc_enabled` not true there
    is no gRPC listener.

### 5.7 Attaching a local node (System app)

**On the edge node.** System › Edge Clients, for the operator:
- make a pairing code (one use, good for an hour);
- see each linked node: its link state, queues, replicated sets and last errors;
- revoke a node.

The edge node's own capabilities (listeners, names, trunks, the mail edge) are set in its `.env` and conf. They are what
discovery reports.

**On the local node.** System › Edge Node, a new screen in `tools/screen/System/`. It is a wizard panel, as the
MailAccounts Add wizard is:

1. **Edge node.** The URL and the pairing code.
2. **Connect.**
   - Check TLS, and show the edge node's name and fingerprint to confirm.
   - Check the versions, then pair: the local node's key is made and written to `.env`.
   - Bring the link up and show the round-trip time.
3. **Discover.** The edge node reports what it offers, and the wizard turns it into a filled-out form. Each area can be
   turned on or off, with the local choices filled in:
   - **Web access:** the host names, passthrough or terminate.
   - **Mail:** the domains whose MX is the edge node; which hosted and shared accounts receive through it; outgoing mail
     through the edge node for `SYSTEM` and the accounts.
   - **Phone:** the edge node's trunks and numbers, each mapped to local routing; the registrar for outside phones; the
     voicemail fallback.
   - **Chat:** which bridges' webhooks go to the edge node; MMS media.
   - **Calls:** the edge node as media node; TURN.
   - **XMPP:** the domain to tunnel.
   - **PASS:** the sites the edge node serves. The PASS client registration is made on both sides.
   - **Monitoring:** the edge node's telemetry sent to this node's Monitor.
4. **Apply.**
   - Write the local rows and settings: `EmailServer` records pointing at the edge node, the bridges' webhook base, the
     edge node's `CallMediaNode` and `SipNode` (kept from the link after that), and the PASS client properties in
     `.env`.
   - Send the edge node its part (`Configure`) and start replication.
   - List the DNS records to set, each checked live: MX, SPF, the DKIM key, DMARC, A and AAAA for each host,
     `_sips._tcp`, and the XMPP SRV records.
5. **Test.** Mail in and out, a webhook round trip, a test call, and the PASS readiness check. Then show the status
   screen.

**The status screen** shows:
- the link's state and generation, the round-trip time, and the last connect;
- the queue of each kind: depth and oldest item;
- the replication cursors, errors, and certificate expiry.

Its actions are pause, resync and unpair.

**As built (E1).** Screens in `runtime/base-component/tools/screen/System/`:
- **Edge Node** (`EdgeNode.xml`, menu index 17): Status (the state, generation, round trip, last connect, links lost,
  last error, and the edge node's name, fingerprint, versions and public names; Refresh, and Unpair with a
  confirmation), and Pair, a two-step wizard:
  1. **Edge node.** The URL and the pairing code. Next calls `check#EdgeNode`. The code is kept in the session for the
     next step, as Chat apps keeps a new app password, and never goes in a URL.
  2. **Connect.** The edge node's name, link key fingerprint, versions and offer, and whether the versions fit. They
     are step 1's answer, kept in the session, so there is no second round trip. Pair calls `pair#EdgeNode` with that
     fingerprint as `expectFingerprint`, then shows Status.

  Steps 3 to 5 come with the comms. In E1 the status screen had no queues or replication, so no pause or resync; E3
  added both (§5.2 "Status and metrics"). Unpair also stops this node's replication notes and fails its pending
  outbox as "unpaired"; Revoke on Edge Clients keeps the node's host names, queue and replica rows for a re-pair of the
  same node (§5.2 "Unpair and revoke").
- **Edge Clients** (`EdgeClients.xml`, menu index 18): the paired local nodes (state, generation, last connect,
  address, versions, the start of each node's key fingerprint), Ping (shows the round trip) and Revoke (with a
  confirmation). "Make a pairing code" lets the operator pick the code's lifetime, from 15 minutes to a day, and shows
  the code once with its expiry. The screen also shows this edge node's link key fingerprint, for the operator to
  compare with what the local node's Connect step shows. An edge node whose HTTPS listener is off says it takes no
  links.
- **Menus.** The System menu cannot leave a screen out by a condition, so both are listed on every node. Edge Node
  says so on an edge node, and Edge Clients on a node that is not one. In a binary without the link both say this build
  has no edge link (state `notBuilt`). Where the build has it but no HTTPS listener started it, as under `moqui test`,
  both say it is not running in this process (`notStarted`). A menu condition is a framework item for later.
- **Spec.** `tools/test/EdgeScreens.spec.js` renders both screens and the wizard's steps in qvt and tui, one case per
  `where` row: no link in this process, a local node, an edge node. On an edge node it also makes a pairing code and
  checks that it shows once. It checks that Connect uses the answer step 1 kept. A case that is not this node's skips.
  `moqui test` starts no listener, and `--serve` starts it only after the specs, so a run is always the no-link case.
  The local and edge cases were checked over HTTP on a served Maximal build.

**As built (E10).**
- **Offer.** The edge node's Hello now names its areas in `capabilities` (`web`, and `mail`, `chat`, `pass` when set
  up), and a new RPC `Link.GetOffer` (additive) answers a structured `Offer`: web access (its public names and the
  modes), mail (moqui or postfix, the listener on, the MX host `smtp_hostname`, the bounce domain, and with the MTA in
  moqui its DKIM selector and record), chat (webhooks on, the public URL), PASS (mr-pass loaded, the client id `edge`),
  SIP as not available (XMPP and calls since their areas, below), its public addresses (a new property, `edge_public_addresses`) and the asking
  node's host names. The offer is asked on the link, never on the pairing connection, so it is always fresh: the wizard
  and Status read it with `get#EdgeOffer`. Code: `edge_link/offer.rs`; `moqui_core::edge::setup` holds the types.
- **Configure.** A new RPC `Link.Configure` (additive): the local node's host names with their modes (`set_hosts`: the
  node's whole list; a name of the node not listed is removed), its mail domains, chat bridges and PASS sites. The edge
  node checks it as input from any client first (`setup::check_setup`: at most 64 of each, domains and ids by their
  syntax, the modes; a bad one refuses the whole request, before any change). Host names go through E2's
  `hosts::add`, for the node the link's key names, never one in the request; a name another node holds is answered
  "another local node has this name", without naming it. A local node cannot claim the edge node's own names, which
  the host table would route before the edge node's own handling: its public names and `webapp_http_host`,
  `smtp_hostname`, `edge_mail_bounce_domain`, the host of `channels_public_url`, its `public-site` hosts, and their
  subdomains, and IP addresses, `localhost` and one-label names (`setup::check_host_name`, `is_reserved`); a name the
  operator adds on Edge Clients is not checked. Other errors go back as one generic message, the detail logged. The
  rest is kept as `EdgeClient.setupJson` (new fields `setupJson`, `setupDate`; refused for a node not active), which
  Edge Clients shows as "Set up". The offer's area names in Hello are read at start (restart the edge node after
  changing its areas); GetOffer is computed fresh, and a page waits for it at most 3 s. The edge node needs no mail domains to work (its MX
  and DKIM signing read the replicated accounts), so they are kept for the operator to see.
- **Wizard.** `Pair.xml` has five steps now; Connect's Next pairs (pairing now waits up to 5 s for the link) and goes
  on, and Finish is the last step's.
  - **Discover** (`Pair/Discover.xml`): the offer as a form, each area on or off: passthrough and terminate host names
    (this node's `webapp_http_host` and its names there now), mail (the hosted and shared accounts' domains, the
    system's mail, the accounts), chat (the bridges), PASS (the sites, from `mantle.public.PublicSite` when Mantle is
    there, else typed, and the SSO callback). Phones are listed as not available yet (XMPP and calls have their areas, below). The choices go in
    the session (`edgeSetup`), never a URL.
  - **Apply** (`Pair/Apply.xml`, `apply#EdgeSetup`): Apply (a button on the step; the wizard's Next then needs it done,
    and a new choice in Discover clears it, so a changed setup is applied again before Test) checks the whole setup
    first (`check_setup`, the host names, the PASS choice; Discover checks them too) and changes nothing when any is
    not right; then sends Configure, and changes nothing locally when the edge node does not take it; then writes the
    local rows and settings (`setup::apply_local`, each area apart, a line each): an `EmailServer` `EDGE`
    with `smtpHost` `edge:`; `SYSTEM`'s `smtpHost` set to `edge:` (the email templates name `SYSTEM`); each chosen
    hosted or shared account's own server, when it owns one with no incoming store, set to `edge:`, else the account
    pointed at `EDGE`; an account that syncs from another provider is left. The host or server each replaced is kept
    (new fields `EmailServer.edgePriorSmtpHost`, `EmailAccount.edgePriorServerId`) and put back at unpairing
    (`setup::restore_local`, from `unpair#EdgeNode`); while the node is not paired (no `edge_node_url` and no running
    link), a send through `edge:` is not queued but fails retryable and stays in the Outbox. Discover leaves the
    system's mail off by default when `SYSTEM` has a server of its own. `channels_public_url`, and
    `pass_edge_client` with `pass_client_edge_sites` and `_callback`, go into `runtime/.env`
    (`update_dotenv_file`; a process property that overrides one is named). Then Configure, and every replicated set
    whole again. The DNS records are checked live on each render (`check#EdgeDns`, in moqui-mail on E5b's resolver,
    whose `txt` now answers fixed records too): MX, SPF (`mx` only once the MX is the edge node, `a:` the MX host, a
    network with one of its addresses, or `include:` it, noted as needing its own SPF record), the DKIM key (the `p=`
    compared; under Postfix only that a record is there), DMARC, and A and AAAA against `edge_public_addresses`, else
    the MX host's addresses, a family the edge node lists none of not checked; each expected and found, with Check
    again. `_sips._tcp` is listed for later (the XMPP SRV records are checked, below).
  - **Test** (`Pair/Test.xml`, `test#EdgeSetup`, modeled on MailAccounts' Test with Continue anyway): the link (state
    and round trip), and round trips on the queue (a new kind, `edgeEcho`, `receive#EdgeEcho`, granted to `_EDGE_`;
    at most 20 a minute per node on the edge node): mail in (the edge node finds the first chosen account's address,
    else a hosted one's, in its copy of the recipients, and its answer comes back on the queue as inbound mail does;
    not checked with no account), mail out (the edge node can send: its DKIM key, or its Postfix relay answering; and
    `EDGE`, `SYSTEM` when chosen and the chosen accounts' servers are on `edge:`), webhooks (on, and the chosen bridges
    in its copy; not checked with none), PASS (the client properties name the sites, and the edge node has mr-pass).
    No outside mail or provider; the answers are waited for up to 10 s. The result is kept for the visit; Test again
    runs it again. Finish goes to Status and forgets the choices.
  - **Status** has a Setup box: the host names (from the offer while linked), mail out (the system and the accounts
    through `EDGE`), the chat address and the PASS sites, with Set up again, which opens the wizard at Discover.
- **Deviations.** Each area's host names are two lists (passthrough, terminate), not one row per name. Turning an
  area off leaves what an earlier Apply set; there is no undo (calls are the exception: off is a setting of its own,
  below). The mail-out test is a queue round trip, not a message
  sent to an outside address, and mail in checks the recipient copy rather than taking a message. Phone and
  monitoring are not in the form (E8 and §5.9 are not built); calls have their area since E7 ("As built (E10,
  calls)"). XMPP's domain is not chosen in the form but is the
  node's `xmpp_domain`, and Apply writes nothing local for it; its probe goes to the host of `edge_node_url`, not to the SRV target.
- **Tests.** `tests/it/edge_setup.rs` (about 4 s): the offer read, Configure applied (a name another node holds
  refused, the edge node's own names and their subdomains, an IP address, `localhost` and a one-label name refused, a
  stale name removed, input refused whole), Apply's local writes into the test runtime's own `.env` (the
  repository's is checked untouched), unpairing's restore of `SYSTEM` and the account, `check#EdgeDns` against fixed
  records, `test#EdgeSetup`'s round trips, and the wizard's steps after Connect walked as screens: Discover's default
  form (no `localhost`, the system's own server noted), a bad name refused, Apply, a choice made again needing Apply
  again, Test and Status, with the dialer behind the local node's services. Pairing itself (EdgeUrl, Connect) is not
  driven through the screens: the hook pairs only with a WebPKI certificate, which a test authority is not;
  `edge_link.rs` covers pairing. Unit tests: `edge/setup_tests.rs` (input checks, the mail rows, PASS refusals, the echo) and
  `moqui-mail` `edge_dns_tests.rs`. `EdgeScreens.spec.js` renders the new steps in its three cases and checks that
  Discover keeps its choices in the session.

**As built (E10, XMPP).** The wizard's XMPP area, on E9's tunnel.
- **Offer.** `Offer.xmpp` (field 8, `XmppOffer`): on (a tunnel port is), `edge_xmpp_port` and `edge_xmpp_tls_port`
  (0 while off), and `other_node` when `edge_xmpp_node` names another node. Hello's areas add `xmpp` when a port is
  on; `setup::NOT_AVAILABLE` was calls and SIP (SIP alone since the calls area). Each area is a message of its own, so calls and SIP add theirs the same
  way (Offer from 9, ConfigureRequest from 7).
- **Configure.** `ConfigureRequest.xmpp` (field 6, `XmppSetup`): the local node's XMPP domain, checked by
  `check_setup` as a host name (not an IP address, one label or `localhost`; a bad one refuses the request whole) and
  kept as `xmppDomain` in `setupJson`. The `rooms.` and `upload.` names are the domain's and are not sent; the edge
  node needs them only for federation (C3b). It tunnels by port, not by name, so the domain is kept for the operator,
  and a domain whose tunnels `edge_xmpp_node` sends elsewhere is logged.
- **Discover** offers XMPP when the edge node has a tunnel port and this node an `xmpp_domain` (on by default then),
  showing the domain and ports. Uploads (`https://<domain>`) and `/xmpp-websocket` ride on web access, so the domain
  must be an `EdgeHost`: Next adds it to the passthrough names when it is in neither list (terminate is taken too: its
  WebSocket signs in without `-PLUS`); with web access off it asks the offer and refuses unless the edge node has
  the name. `apply#EdgeSetup` (`xmppOn`) adds it the same way; with web access off and the name not there it gives a
  failed line and applies the rest; with no `xmpp_domain` or no tunnel port it changes nothing. It writes nothing
  local; its lines say when this node's `xmpp_port` or `xmpp_tls_port` (which Chat apps shows) differ from the edge
  node's ports, when the edge node did not take the name, and when the tunnels go to another node or to none
  (`needs_node`, field 5: `edge_xmpp_node` empty and the node not the only one linked, the tunnel ports' own rule).
- **DNS.** `edge_dns` checks `_xmpp-client._tcp` and `_xmpps-client._tcp` of the XMPP domain (not of each mail
  domain), each port that is on: every record must name one of the edge node's names (its public names, the host of
  `edge_node_url`, its MX host) and the tunnel port. `Lookup::srv` is hickory's SRV lookup (`dns::srv`), with fixed
  records (`Records::fix_srv`) as TXT has. `_xmpp-server._tcp` is listed for later, as federation's (C3b);
  `_sips._tcp` stays listed for later (E8).
- **Test.** `test#EdgeSetup` (`xmppOn`) calls `EdgeLink::xmpp_probe` for each port that is on (host
  `edge_link/probe.rs`): this node's server must take tunneled connections of the kind (`xmpp::running`), then a
  client connection to the edge node's port (direct TLS with ALPN `xmpp-client`, or the stream in the clear, its
  features from the domain with STARTTLS, then STARTTLS) must complete a TLS handshake pinned to this node's own
  certificate for the domain (`tls::leaf_for`, the whole certificate): only this node has its key, so that is the
  proof the connection came back here. Nothing more is sent, so no sign-in is counted. A port that is off is not
  checked. A third line checks the domain is one of the node's host names there. The checks run on a thread of
  their own beside the round trips' 10 s wait.
- **Status** shows XMPP: the domain through the ports, while the link is up and the name is there.
- **Tests.** `edge_setup.rs`: the offer's ports and `xmpp` in Hello, Configure keeping the domain and refusing a bad
  one, the SRV records against fixed ones, and the wizard's XMPP walk (Discover adding the domain, Apply, the edge
  node's row and `setupJson`, Test's probes failing on a node with no XMPP server and the name found, Status, Apply
  with web access off and the name kept giving no failed line, and Apply refused with no `xmpp_domain`). `edge_xmpp.rs`: the probe through both tunnel ports reaches the local node's
  server, and fails with the link down. Unit: `setup_tests.rs` (the domain's checks), `edge_dns_tests.rs` (the SRV
  rows, a fixed SRV record). `EdgeScreens.spec.js`: Discover adds the domain, or refuses with no `xmpp_domain`.

**As built (E10, calls).** The wizard's calls area, on E7's edge node as media node (§5.4, "As built (E7)").
- **Offer.** `Offer.calls` (field 9, `CallsOffer`, additive): on while the edge node's media socket is up, its media
  address and TURN URLs (what `rtc::start` advertises, `edge::media::advert`, the same as E7's Hello fields), and
  `other_nodes` when another local node is linked too, so the edge node takes call messages from none (decision 1).
  Hello's areas name `calls` while the media socket is up, as the Hello is made (`accept.rs` `hello`), not only as
  read at start, since the media socket can come up after the link. `setup::NOT_AVAILABLE` is `["sip"]`.
  `ConfigureRequest` field 7 is not used: the edge node needs nothing from the local node for calls.
- **What calls off means.** E7 keeps the edge node's `CallMediaNode` row from the link and `Calls.js`
  `liveMediaNodes` puts it first whenever paired. The choice is a local setting, `edge_calls_enabled` in
  `runtime/.env` (on unless `false`, `n`, `no`, `0` or `off`; unpairing removes it, so a pair again starts on, as E7
  does): while it is off the link keeps the row as `draining` (`edge::calls::register_row`, which the dialer's
  `keep_media_node` now calls, reading the setting each beat). `Calls.js` already gives a draining node no new call and
  needs no change; calls already on the edge node go on, since the row stays live (`expire#Calls` does not end them,
  and the delegated media events and recordings still find an `edgeLinked` row). Removing the row instead would have
  ended them as "media server lost".
- **Discover** offers calls when the edge node has a media node, on by default unless this node's
  `edge_calls_enabled` is off, showing the media address and TURN URLs, with a note: new calls have their media on
  the edge node, so office users in a call with each other send it to the cloud and back (decision 6), with no
  cascading to a media node of this node's own and one local node per edge node; and a warning when other local nodes
  are linked. Calls alone is a setup.
- **Apply** (`callsOn`, only when the offer has calls; asked without one it gives a failed line): `edge_calls_enabled`
  written, the row written again at once with it (the link writes it every 15 s after that), then a line each (core
  `edge/calls.rs` `apply`): the media address is public (`media_address_problem`: not loopback, private, link-local
  or unspecified; else set `rtc_public_address`), other local nodes linked, the TURN secret the link's Hello handed this
  node (`edge::media::turn_secret`; no TURN is a failed line too), and the row (`row_check`: there, `edgeLinked`, a
  heartbeat within 60 s, the Hello's media address, draining only while off). Nothing goes to the edge node.
- **DNS.** `check#EdgeDns` takes `turnHosts` (the hosts of the TURN URLs, `turn_public_host` on the edge node) and
  `mediaAddress`: each that is a name, and not one of the web host names, gets an A/AAAA row noted "TURN for calls",
  against `edge_public_addresses` and the media address's IP (`edge_dns.rs` `address_row`, the A/AAAA check shared
  with the host names). An address needs no record.
- **Test** (`callsOn`; not checked while `edge_calls_enabled` is off): `callsNode`, the row live and preferred
  (`row_check`, as `liveMediaNodes` orders: a live `edgeLinked` row not draining is first); `iceServers`,
  `make#IceServers` for a probe leg (`edgeSetupProbe.<random>`) with the row's URLs and the edge node's instance id
  gives exactly the edge node's TURN URLs, signed with the handed-off secret (refused when the link handed none);
  `turn`, on a thread beside the round trips, `EdgeLink::turn_probe`: that credential allocates over UDP on the first
  `turn:` URL, as a browser would (`turn-client-proto`, now an optional dependency of the host under `rtc`; host
  `rtc/turn_probe.rs`), a permission to the media address is granted (the edge node's TURN relays only to its media
  port), and the allocation is deleted; at most 3 s. No STUN binding to the media address itself: the media socket
  answers one only with a leg's ICE credentials, and the permission covers the address.
- **Status** shows calls: new calls on the edge node at its media address (with TURN or not) while its link-kept row
  is live, off by `edge_calls_enabled`, or no media node.
- **Tests.** `edge_setup.rs` (about 5 s now): the edge node runs a media socket and TURN in the test's process (the
  process's one media node; the local node, started first, has none), on loopback, advertised as `203.0.113.5` so the
  media address is public; the offer and `calls` in the Hello, the TURN host's A/AAAA against fixed records, and the
  calls walk: Discover offering calls, calls alone chosen, Apply (the setting, the row, every line passing), Test's
  three checks passing (a real allocation on the edge node's TURN with the credential `make#IceServers` signed),
  Status, calls off (the row draining, the setting written, Test not checking, Status saying so) and on again. Unit:
  `edge/calls_tests.rs` (the setting, the media address check, a link with no media node), `rtc/turn_probe_tests.rs`
  (an allocation and permission on the embedded TURN on loopback with the edge node's secret, another secret refused,
  the URL forms), `edge_link/tests.rs` (the offer's conversion), `edge_dns_tests.rs` (the TURN host rows).
  `EdgeScreens.spec.js`: Discover takes calls alone.
- **Not done.** Real browsers through a deployed edge node's TURN, with a real `turn_public_host` in DNS, are to be
  checked by hand. The TURN check uses UDP only; a TURN with only TCP or TLS listeners is not checked.

### 5.8 Security

- **What an edge node sees:**
  - in passthrough, no app content; in terminate mode, all of it;
  - always: mail and messages while they are queued; the media of calls that are not end-to-end encrypted (W8's
    encrypted calls stay encrypted); PASS traffic; the replicated sets.
- **What a compromised edge node can do:**
  - read and forge inbound mail and messages, and send mail as the domains;
  - call the delegated services as `_EDGE_`;
  - make PASS calls as the PASS client, though user actions still need the visitor's delegation;
  - answer the phones.

  It cannot log in as a user, or read business data beyond what the delegated services return.
- **Controls:**
  - a key per node, pinned both ways, and a link either side can revoke;
  - delegated services on an allowlist on both nodes, run as `_EDGE_` with narrow grants; no parameter the edge node
    sends (credentials, `_` names, a caller) changes the user a call runs as;
  - everything from the edge node checked as input from any client;
  - replicated fields kept to the minimum and encrypted at rest;
  - web protection on the edge node, and on the local node by the PROXY address.
- **Sealing, as an option.** Once the edge node has made its checks (recipient, signature, spam verdict), it seals each
  queued item to the local node's public key. A copy of the edge node's database then does not expose queued mail and
  messages.
- **Sensitive replicated data:** trunks' `authJson`, and bridge webhook secrets. SIP digest verifiers (HA1) are not
  replicated: the registrar delegates the digest check (decision 4).
- [Attack-Surface](../docs/security/Attack-Surface.md) gets the edge node's listeners and the link.

### 5.9 Operations

- **Monitoring.**
  - The edge node exports OTLP to the local node's Monitor over the link; the OTLP receiver already exists.
  - Metrics: link up, round-trip time, reconnects; queue depth and age by kind; replication lag; delegated call latency
    and errors.
  - Alerts: the link down for N minutes, a queued item older than N hours, a quota nearly reached.
- **Upgrades.** Either side can go first. Hello refuses a link more than one version apart, with a clear message.
- **When the edge node fails.** Mail senders retry for days, and providers' webhooks for hours. Calls fail over at the
  trunk provider, so configure a failover number there. A second edge node (a secondary MX, a second webhook URL) is
  later.
- **Bandwidth.** All media of calls with outside parties runs through the edge node, so size the edge node's network for
  calls, not for its database.

## 6. Phases

| Phase | What | Acceptance |
|---|---|---|
| **E0** | Settle §8. Audit each comms engine's entity access on a node without the business data. Measure the cost of the tonic client and hyper `http2` | Each access classified (edge node, delegate, queue, replicate, link, stays local) in §4a; the size and build-time numbers recorded in §5.6 |
| **E1** | The link: ALPN on 443, pairing, keys in `.env`, the up and down connections, Hello, liveness, reconnect, generations. `node_role`, MoquiEdgeConf.xml, and the status and Edge Clients screens, with no comms yet | A local node with no listening port links. An edge node restart, a change of the local address, and a dropped NAT mapping each recover within a minute. Unpairing cuts the link on both sides |
| **E2** | Web access: passthrough with PROXY v2 (new on the HTTPS listener), terminate with HTTP/2 and extended CONNECT, the offline page, the SSH tunnel | The apps, `/notws`, DAV and git work through the edge node. The client's address is right in the logs and in web protection |
| **E3** | Delegate, queue and replicate; `_EDGE_` and its group; metrics | With the local node stopped mid-stream, queued items arrive once, and in order, after it returns. Replication recovers when the edge node has lost its database |
| **E4** | mr-pass on gRPC: the adapters, the PassApi refactor, `PassAuthority`, the direct and link channels; REST kept | The local/remote compatibility tests pass over gRPC direct, gRPC on the link, and REST. REST on Lite stays a supported, tested mode |
| **E5a** | Mail through the edge node, with Postfix and Rspamd beside it: the recipient list, LMTP into the edge node queue, outbound submission, delivery reports | MX at the edge node. Mail sent while the local node is down arrives after. Unknown recipients are refused while it is down. Outbound mail is DKIM-signed and passes DMARC at a large provider. Bounces reach the Outbox |
| **E5b** | The MTA in moqui: port 25, SPF, DKIM, DMARC and ARC, DKIM signing, the MX delivery queue, DSNs, TLS policy; Rspamd still external, over HTTP. The same checks on internal mail. Becomes the default | E5a's cases pass with no Postfix. Internal mail carries verified DKIM and `Authentication-Results` |
| **E6** | Chat bridges and SMS through the edge node | An SMS sent while the local node is away is delivered after. MMS media is served by the edge node. Met in `tests/it/edge_chat.rs`, with a fake Telnyx; a real Telnyx round trip is pending |
| **E7** | Calls with the edge node as media node; TURN; recordings queued | A call between an outside browser and an office browser. Its recording reaches the local node. Built. Met in `tests/it/edge_call.rs` with str0m stand-ins (the outside one relayed by the edge node's TURN); real browsers over the internet are pending |
| **E8** | SIP on the edge node; the voicemail fallback | A trunk call is routed by the local node. With the local node away, a voicemail is recorded and delivered later |
| **E9** | The XMPP tunnel; design input to C3b and C6 | An outside XMPP client chats through the edge node. Built. Tested in `tests/it/edge_xmpp.rs`: clients over the STARTTLS and direct TLS tunnel ports sign in with SCRAM-SHA-256-PLUS and chat on the local node; refused with the link down. C3b's and C6b's split is designed in §5.4. Not tested: a real client through a deployed edge node |
| **E10** | The wizard's discovery, filled-out form, apply step, DNS checks and tests; the documents in §7 | A new local node is attached in one sitting from the wizard. Built, with the XMPP area on E9's tunnel (its SRV records checked, a client stream probed through each port) and the calls area on E7's media node (calls off a local setting that keeps the edge node's row draining; Test allocates on its TURN with a credential make#IceServers signed); SIP comes with E8. Tested in `tests/it/edge_setup.rs` on two servers in one process: pairing by the link's own code (not the EdgeUrl and Connect screens, which need a WebPKI certificate), then Discover, Apply and Test as screens with the services over the test's dialer, the offer, Configure, the local writes and unpairing's restore, and DNS checks against fixed records; the XMPP probe also in `edge_xmpp.rs`; the TURN check also against the embedded TURN in `rtc/turn_probe_tests.rs`. Not tested: the whole wizard on a served Maximal build with a real certificate and real DNS |
| **E11** (late; possibly after the first release) | Filtering in moqui: content and reputation scoring for mail, internal mail, public forms and outside chat; replaces the external Rspamd as the default | To be set when it is designed |
| **Later** | A PASS read cache on the edge node; several local nodes per edge node; a second edge node; QUIC; a local media node cascading to the edge node; an ACME client; DANE (RFC 7672) for the MTA's outbound TLS | |

E1 to E3 are the base, and the rest build on them. After E3 the order can follow need. E4 and E5a are the likely
first two.

## 7. Documents to change

When it is built (done in E10 unless marked):
- **Run-Deploy:** a topology row and a section, "A local server with an edge node"; the new properties; the PASS
  section's transport. Done.
- **Guides:** Attack-Surface (done); Mail-Inbound, the edge node as the gateway, then the MTA in moqui (done); SMS and
  Chat-Bridges, the webhook URL on the edge node and MMS (done); XMPP, the tunnel ports (done in E9); WebRTC, TURN and
  Call-Recording, the edge node as media node (done in E7); SIP, the edge node as SIP edge (not yet: with E8).
- **mr-pass:** `docs/Deployment.md`, the edge node as PASS server. Done.
- **Plans:**
  - Comms-Framework: C3b's and C6's split between edge node and local node. Its line "this plan adds no new mail
    features" should point here for the MTA and filtering work. Done; the split made concrete in E9 (§5.4).
  - The two mr-pass plan sentences in §5.5. Done in E4 (`design-shop.md` §13.1 and `implementation-forum.md`'s
    "Local and remote" row now name gRPC, direct or on an edge node's link).

## 8. Decisions

**Decided 2026-11-01:**

1. **Tenancy.** One local node per edge node first. The edge node's rows (queues, replicated sets, configuration) are
   keyed by the paired node, so several local nodes on one edge node can come later without a redesign.
2. **Web access default.** Both modes. Passthrough is the default for the local node's apps, and terminate can be
   chosen per host name.
3. **Mail edge.** Both layouts are options. The MTA in moqui is the default once built, and Postfix with Rspamd is the
   supported external alternative. Rspamd is likely to stay external for some time, so filtering in moqui is a late
   phase (E11), possibly after the first release. The same mail standards are used inside the organization too. Later,
   filtering in moqui serves internal comms as well: large organizations, and requests from public-facing forms. It is
   a high cost, but a high business need, which justifies the tight integration.
4. **Voice while the local node is away.** The voicemail fallback. Each number's box, greeting and outside fallback are
   replicated to the edge node. The registrar's digest check is delegated, and HA1 verifiers are not replicated.
5. **The name.** "Edge node", in place of "relay node": "relay" already names W7's relay links between media nodes
   (`sfu/relay.rs`, `RelayMessage`) and TURN relays.
   - W7 called a media node holding some of a call's legs "an edge", beside the call's origin. That term is now "leg
     node", in the code (`RelayRole::LegNode`), the specs and the guides, so "edge node" means only this plan's node.
   - Web protection's metrics are named `edge_*`, and `edge.rs` and `cluster_edge.rs` name the host's async runtime and
     the cluster listener. Each means the edge of one server, which does not conflict with an edge node.
   - The code names stay apart from those metric names: `node_role=edge`, `edge_node_*` for the link's properties,
     `moqui.edge` for the entities, and the ALPN `moqui-edge/1`.
   - The link's metrics are `moqui.edgelink.*`, since web protection already exports `moqui.edge.denied`
     (`crates/moqui-host/src/monitor/`).
6. **Office calls through the cloud.** Office-to-office calls that send their media through the edge node in the cloud
   are acceptable. This is likely a common layout, early on and later. A local media node cascading to the edge node
   stays a later option.
7. **gRPC payloads.** One generic operation message, with the input and the result as JSON, to start. Typed messages
   may come later, for efficiency or for what they make possible.
8. **Lite and gRPC.** Lite keeps REST for PASS, not gRPC, as a supported mode for the long term, and REST is the default
   transport on Lite (§5.5). Adding `grpc` to Lite would add 5 crates (§5.6).

## 9. References

- Run and deploy: [Run-Deploy](../docs/Run-Deploy.md) (topologies, the PASS and cluster sections),
  [Build-Test](../docs/Build-Test.md#packages).
- Comms: [Comms-Framework](Comms-Framework.md) (§6.3 signaling, §6.5 network, D-E1),
  [Mail-Inbound](../docs/interfaces/Mail-Inbound.md), [Chat-Bridges](../docs/interfaces/Chat-Bridges.md),
  [SMS](../docs/interfaces/SMS.md), [XMPP](../docs/interfaces/XMPP.md), [SIP](../docs/interfaces/SIP.md),
  [Voicemail](../docs/interfaces/Voicemail.md), [WebRTC](../docs/interfaces/WebRTC.md),
  [TURN](../docs/interfaces/TURN.md), [Call-Recording](../docs/interfaces/Call-Recording.md).
- PASS: `runtime/component/mr-pass/docs/Deployment.md`, `plan/design-forum.md` (the remote adapter, caches),
  `plan/design-shop.md` (authority outage, caches).
- Cluster: [Cluster](archive/Cluster.md), [Ssh](archive/Ssh.md) (the cluster channel).
- Code:
  - the link's models: `crates/moqui-core/src/cluster/`, `crates/moqui-host/src/cluster_edge.rs`,
    `crates/moqui-host/src/tui_remote.rs`;
  - gRPC: `crates/moqui-host/src/monitor/grpc.rs`;
  - the listener: `crates/moqui-host/src/http.rs`, `tls.rs`;
  - queues: `crates/moqui-core/src/service/chat_bridge.rs`;
  - sync: `crates/moqui-core/src/service/system_sync/`;
  - mail: `crates/moqui-host/src/smtp.rs`, `crates/moqui-mail/src/delivery.rs`;
  - comms: `crates/moqui-host/src/channels.rs`, `crates/moqui-host/src/rtc/`, `crates/moqui-host/src/sip/`.
- Messaging: [Messaging](proposals/Messaging.md), the FlatBuffers proposal for browsers and gateways. It rejected
  protobuf for that purpose. The link here is server to server, where gRPC's tooling and streaming fit.
