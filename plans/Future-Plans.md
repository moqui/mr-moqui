# Future plans

The plans for after v0.1.0, and a digest of the unbuilt proposals in [proposals/](proposals/). Work for the first
release is in [Remaining-Work](Remaining-Work.md). Each entry links to its detail; read it before starting.

## Comms

From [Comms-Framework](archive/Comms-Framework.md), archived 2026-11-12. BasicComms, calls, phone calls, SMS, encrypted chat and XMPP for local users are built ([§7.4](archive/Comms-Framework.md#74-built)). What
is left, by stream; the designs are in the archived plan and the interface guides under [docs/interfaces](../docs/interfaces/).

**Steps not built** (acceptance in [§7.1](archive/Comms-Framework.md#71-milestones-and-acceptance), designs in [§4.3](archive/Comms-Framework.md#43-proposals)):
- **C4**, WhatsApp, Messenger, Instagram and Telegram on the bridge layer. A network that allows one session at a time
  (Telegram's long poll) needs its adapter to run on one node only, by a leader election; today an adapter starts on
  every node that runs the worker.
- **C3b**, XMPP federation (server to server with TLS and SASL EXTERNAL, dialback), with its edge node split
  ([Edge-Node](archive/Edge-Node.md) E9). It is what XMPP push and a XEP-0479 level need.
- **C6a, C6b, C6c**: the Matrix client API (Element on C7's key endpoints), federation and the appservice API.
- **C2** (an appservice on an external homeserver) and **C5** (Slack, Teams, Discord), when needed.

**Left out of the built steps:**
- **Speech.** `speech-local` (Whisper on candle) is a stub; captions and voicemail use the `moqui-llm` endpoint. Nothing
  transcribes a call recording yet (voicemail's transcription is the model). No text to speech: phone prompts, the IVR
  and queue places are tones or the admin's recordings.
- **Recording** ([Call-Recording](../docs/interfaces/Call-Recording.md)): no composited video export; the quota can pass
  by what the drive gained since the last stored segment (an exact reservation would count the open recording in
  uploads).
- **Captions**: the terminal call shows whether they are on, not the lines (the server keeps none unless
  `call_captions_store`).
- **SIP** ([SIP](../docs/interfaces/SIP.md)): the INVITE without an offer, RTCP and SRTCP, IPv6, NAPTR; desk phone
  provisioning files and numeric extensions; a desk phone's attended transfer (REFER with Replaces).
- **Voicemail** ([Voicemail](../docs/interfaces/Voicemail.md)): forward and reply by email; a ring group box's PIN and
  phone access.
- **Call handling**: "press 1 to accept" on a forwarded mobile; presenting the caller's number on a forward; schedules
  from a calendar, and users' own schedules in the screens; queue skills, priorities, callbacks and reports.
- **Chat bridges** ([Chat-Bridges](../docs/interfaces/Chat-Bridges.md)): external actors' mentions, read markers, typing
  and presence; native endpoints (C3, C6).
- **SMS** ([SMS](../docs/interfaces/SMS.md)): group texts; calls through an IVR menu or a queue linked to the number
  pair's room (who sees the room is a decision to make: the number's routes reach no queue agents).
- **Encrypted chat** ([Chat-Keys](../docs/interfaces/Chat-Keys.md)): secret storage of the cross-signing keys (a restored
  device is not cross-signed); MSC4268 key bundles for members who join later; Create from on an encrypted message with
  the user's consent; a decrypted file's blob URL freed when its message scrolls out; a recent sign-in check that a
  stolen session cannot pass (today one within five minutes of the user's own sign-in does).
- **Encrypted calls**: guests (an ephemeral crypto device per guest); VP9, AV1 and H.264; a call across two media nodes
  in browsers.
- **Cascading media nodes**: a TCP or TURN fallback for the relay link, links per pair of nodes, bandwidth estimates
  across the link, and choosing a call's node by where its people are.
- **XMPP** ([XMPP](../docs/interfaces/XMPP.md)): a XEP-0479 level (Core needs C3b and XEP-0114; IM Core also vCard
  updates, IM Advanced XEP-0398, XEP-0153, XEP-0048, XEP-0049 and XEP-0223); MUC configuration, moderation and private
  messages; reactions in the archive; offline delivery outside MAM; XEP-0184 receipts; stanzas between XMPP sessions on
  different nodes; revoking an XMPP user's TURN credential.

**Not checked live.** The tests use fakes, loopback and headless Chromium; these need a deployment, a provider's
consent or real devices, and are listed in [§7.4](archive/Comms-Framework.md#74-built) "Left for later":
- Telnyx (calls, SMS, OPTIONS pings, forwards, simultaneous ring, `Privacy: id`), carrier delivery and 10DLC; an emergency
  call; desk phones and soft phone apps.
- TURN over TLS and on 443, hairpin NAT, a browser behind a UDP-blocking firewall; cascading media nodes between two hosts.
- Element against the key endpoints, XMPP apps (Conversations, Monal, Dino, Gajim), the XEP-0479 compliance tester, a push
  reaching an app server; Firefox and Safari for encrypted chat and calls.
- What headless Chromium cannot drive: screen share and the active speaker, captions and a recorded greeting (its
  microphone is a steady tone), QR verification (a camera); the Recordings player with real media.

## Edge node

[Edge-Node2](Edge-Node2.md)'s phases E13 to E19 are for the first release
([Remaining-Work](Remaining-Work.md#edge-node-and-deployment-topologies)). After it: E11, filtering in moqui (content
and reputation scoring for mail, internal mail, public forms and outside chat, in place of the external Rspamd), and
the plan's [Later row](Edge-Node2.md#5-phases): a PASS read cache on the edge node, monitoring over the link, sealing
queued items, media cascading between a local node and the edge node, the voicemail fallback following rules and
schedules, QUIC and DANE.

## Monitoring

From [Monitoring](archive/Monitoring.md), archived 2026-11-12. M1–M7 and the Later stages (browser RUM and replay,
day partitions, gRPC, business operations, regressions and incidents, recovery) are built; guide
[Monitoring](../docs/tools/Monitoring.md). The open limits of what is built are in
[Remaining-Work](Remaining-Work.md#monitoring). After v0.1.0:
- **Server-JS profiling** ([Later](archive/Monitoring.md#implementation-plan)): wall-time hooks at the function and
  selected block boundaries of generated JS, and of handwritten JS through an AST transform, mapped to the JS or XML
  source and the build and linked to the active trace, gated by script, request and an overhead budget, never
  capturing arguments or locals. True CPU or line sampling needs a QuickJS engine hook. It overlaps
  [Profiling3](Profiling3.md) M1, the operation profile (`MOQUI_OP_PROFILE`, built), which already times host calls and
  the interpreter between them per outermost call; start from that.
- **Tenant isolation** of monitoring evidence (incidents, regressions, recovery, the AI-assisted search the
  [3 a.m. incident](archive/Monitoring.md#operator-ux-a-3-am-incident) describes), once there is a tenant model. Today
  the build and runtime context are kept, with no tenant to record.
- **Public and anonymous dashboard links**, if they are wanted: deferred because ERP data on a self-hosted server
  should not get a bearer URL by default ([Sharing](archive/Monitoring.md#scheduled-reports-and-sharing)).

## Messaging — runtime-reflective FlatBuffers ([Messaging.md](proposals/Messaging.md), 1286 lines)
Binary request/response and WebSocket document subscriptions using FlatBuffers `.bfbs` schemas generated at runtime from service, screen-transition and Data Document definitions, with a Rust reflective reader, a browser reader/writer on Emscripten WASM, shared message buffers and large-scale fan-out. JSON and other contracts stay. Status: roadmap shows 0 of 6 phases done. Today `moqui-browser` speaks JSON ([`crates/moqui-browser/src/api.rs`](../crates/moqui-browser/src/api.rs)) and cites this plan for the binary forms. Prerequisites in place: Wasm browser build, cached QuickJS service functions, committed-update path. Go/no-go gates are in §19; capacity model in §12–13.

## Data load types ([Data.md](proposals/Data.md), 15 lines, "not fully verified")
Replace `seed`, `seed-initial`, `install`, `demo` with explicit ownership and sensitivity: seed (code-owned), configuration, secrets (deployment references only), transactions (PII needs fake replacements or exclusion), and a dev export that filters sensitive fields. Reuses the existing loader/writer and component ordering. Status: idea list; needs design before work.

## Cross-platform usability ([Usability.md](proposals/Usability.md), 15 lines)
Keep business behavior in `moqui-core`; platform hosts supply processes, storage, networking and lifecycle; desktop (native, webview or browser), embedded/POS, and browser-local WASM (built, see [Wasm](archive/Wasm.md)) with remote or local-only operation. Native-widget rendering would need a backend separate from the HTML/Vue macros. Status: direction only; the concrete desktop and mobile plan is [Mobile-Desktop](Mobile-Desktop.md).

## Non-SQL entity datasources (no document yet)
Java serves a `<datasource object-factory="...">` group from a store that is not SQL: `ElasticDatasourceFactory` (Elastic or OpenSearch, about 1,200 lines in `org.moqui.impl.entity.elastic`) for the `logging` group, and `OrientDatasourceFactory`, which Java's own conf leaves commented out and which is not planned here. Today the `DatasourceFactory` trait and registry ([`crates/moqui-entity/src/entity/datasource.rs`](../crates/moqui-entity/src/entity/datasource.rs)) open only a SQL `EntityGroup` (a driver and a dialect), and a datasource whose factory is not registered is logged and its entities use the `transactional` group. An Elastic store would also need a non-SQL route through find, count, iterator and value writes, built on the existing `search` client. Low priority: in Java only `TestNoSqlEntity` uses `group="logging"` by default (moving `ArtifactHit` there is commented out), and a second SQL database for a group already works through `inline-jdbc`. Status: proposal only.
