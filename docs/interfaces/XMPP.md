# XMPP for local users

The people of a Moqui server can use an XMPP app with their chat: Conversations and Monal on phones, Dino and Gajim on
desktops, a web client over WebSocket. Moqui is the XMPP server for its own users (C3a); server-to-server federation
is C3b, not built. The XMPP view is of the same rooms, messages and presence the web has: there is no second store.

- The protocol is `crates/moqui-xmpp`, on Tokio: the stream reader and writer (`framing.rs`), SCRAM (`scram.rs`), FAST
  (`fast.rs`), the session (`session/`: sign-in, stream management, the iq handlers, chat, the archive, PEP, push),
  the session registry (`registry.rs`) and the connections (`conn.rs`). It knows no ExecutionContext: what it reads
  and writes it asks of a backend by service name.
- The host's side is `crates/moqui-host/src/xmpp/`: the listeners, the certificates, `/xmpp-websocket` (`ws.rs`),
  `/xmpp-upload/` (`upload.rs`), the backend (each call on a request worker) and the subscription to the notification
  bus.
- The services are `org.moqui.impl.XmppServices`, with `framework/entity/XmppEntities.xml`; chat is
  `org.moqui.impl.ChatServices`, called as the signed-in user.
- The design is [plans/Comms-Framework.md](../../plans/Comms-Framework.md) §4.3 "XMPP server (C3)", §6.5 and D-C3.

## Settings

The `xmpp` feature is in the Maximal, Enterprise and Debug packages.
Everything is off until `xmpp_domain` is set; each listener is off until its port is set.

| Property | Default | |
| --- | --- | --- |
| `xmpp_domain` | empty | The domain of the users' JIDs, `user@<domain>` |
| `xmpp_rooms_domain`, `xmpp_upload_domain` | `rooms.<domain>`, `upload.<domain>` | The MUC service and the upload service |
| `xmpp_port`, `xmpp_host` | empty, `0.0.0.0` | STARTTLS (5222 by convention) |
| `xmpp_tls_port`, `xmpp_tls_host` | empty, `xmpp_host` | Direct TLS with ALPN `xmpp-client` (5223, XEP-0368) |
| `xmpp_websocket_enabled` | `false` | `/xmpp-websocket` (RFC 7395) on the HTTP listener |
| `xmpp_upload_url` | `https://<domain>` (and `webapp_https_port`) | The base of upload URLs |
| `xmpp_upload_max_bytes` | 20971520 | The largest upload |
| `xmpp_upload_secret` | generated | Derives the URLs of chat files sent to XMPP clients |
| `xmpp_resume_seconds` | 300 | How long a dropped session waits for resumption |
| `xmpp_fast_token_days` | 14 | How long a FAST token lasts |
| `xmpp_upload_seconds` | 300 | The longest a PUT to an upload slot may take |
| `xmpp_turn_enabled` | `false` | External services (XEP-0215) hand out TURN credentials ([Calls](#calls)) |
| `xmpp_turn_per_hour` | 30 | TURN credentials a user gets an hour |
| `xmpp_scram_decoy_key` | generated | Derives the SCRAM salts of unknown users, the same across restarts and nodes |

**DNS and certificates.** `_xmpp-client._tcp.<domain>` (5222) and `_xmpps-client._tcp.<domain>` (5223) name the host.
The TLS listeners use `runtime/cert` by the name a client asks for (the domain, `rooms.<domain>`, `upload.<domain>`,
else `default`; [Run-Deploy](../Run-Deploy.md)). The WebSocket and uploads are on the HTTPS listener and its
certificate. In dev an empty `runtime/cert` gets a self-signed `localhost` certificate, which clients will not trust.

## Behind an edge node

A local server behind an edge node ([Run-Deploy](../Run-Deploy.md), [plans/Edge-Node.md](../../plans/Edge-Node.md)
§5.4) keeps its XMPP server; the edge node starts none, and its tunnel ports pipe clients down the link.

- On the edge node, `edge_xmpp_port` (STARTTLS, 5222) and `edge_xmpp_tls_port` (direct TLS, 5223) on
  `edge_xmpp_host`; `edge_xmpp_node` names the local node when more than one is linked. At most 256 tunneled XMPP
  connections at once.
- The local node serves each as a connection from the client's address: its blocks for failed sign-ins and its
  limits per source apply. So the edge node must see the clients' own addresses: behind a TCP load balancer that
  sends no PROXY header, every client would share one address, one per-source limit (20) and one sign-in block. The TLS ends there, with its certificates for the domain, so SCRAM-SHA-256-PLUS and FAST
  bound to the TLS session work as on its own ports. It needs no XMPP port of its own for the tunnel; Chat apps shows
  its own `xmpp_port` and `xmpp_tls_port`, so give them the edge node's numbers.
- The SRV records name the edge node in public DNS, the local node in the office's.
- `/xmpp-websocket` and uploads (`xmpp_upload_url`, `https://<domain>`) ride on web access: the XMPP domain must be
  one of the node's host names on the edge node. In terminate mode the WebSocket has no exporter, so clients sign in
  with SCRAM-SHA-256 there.
- The Edge Node wizard on the local node sets this up: Discover offers XMPP when the edge node has a tunnel port and
  this node an `xmpp_domain`, and adds the domain to the passthrough names; Apply sends the domain to the edge node
  and checks `_xmpp-client._tcp` and `_xmpps-client._tcp` against the edge node's name and ports; Test opens a client
  connection through each tunnel port, whose TLS must be this node's own certificate, sending no credentials.
- While the link is down clients cannot connect; they retry, and what was sent to them waits in the archive.
- On an all-in-one node ([Run-Deploy](../Run-Deploy.md#an-all-in-one-node)) the server's own XMPP server keeps
  `xmpp_port` and `xmpp_tls_port`, and the tunnel ports, which must differ, serve the local node's domain. A tunnel
  port the same as one of the server's is off, with an error logged, and not offered to the local node.
- Federation (C3b) will take 5269 (`xmpp_s2s_port`, off) on the edge node, with stanzas to local users queued while
  the local node is away ([plans/Comms-Framework.md](../../plans/Comms-Framework.md) §4.3).

## Addresses

- A user is `username@<domain>`, in lower case. The characters a localpart cannot hold are escaped as XEP-0106 has
  them: `jane@example.com` is `jane\40example.com@<domain>`.
- A room is `<chatRoomId>@rooms.<domain>` when the id is already a lower-case localpart, else `x-` and the id's
  bytes in hex.
- A room occupant's nickname is the member's username. A client that asks for another nickname gets this one (status
  210).

## Sign-in

A client signs in with an **app password**, as DAV and SFTP clients do. Settings, Chat apps (basic-comms,
`XmppServices.get#ClientSettings`) shows what to enter (the JID, the server and its ports, the WebSocket) and the user's
app passwords, each saying whether it works with XMPP apps, with New app password for XMPP. The account password
is never taken: a failure would count toward the account's lockout, and it cannot carry a second factor. An account
with a second factor signs in with an app password, which is made after it.

- **SCRAM-SHA-256-PLUS** (tls-exporter channel binding, RFC 9266, XEP-0440) where the connection has its own TLS
  session (TLS 1.3): the STARTTLS and direct TLS listeners, and the WebSocket over this process's own HTTPS. Behind a
  proxy that ends TLS there is no binding, and **SCRAM-SHA-256** is offered. A client that says it could bind but the
  server could not (`y`), where the server binds, is refused as a downgrade.
- The server keeps no password. `create#UserAppPassword` stores SCRAM verifiers with the password
  (`UserAppPassword.scram*`): salted, 10,000 iterations, one for the password as shown and one without its dashes. A
  user's app passwords share one salt, so one challenge fits each. App passwords made before C3a have none; PLAIN
  takes them. `get#UserAppPasswords` says which have them (`xmppReady`), and Chat apps marks the others.
- **Unknown users** go through the SCRAM exchange with a salt derived from their name and `xmpp_scram_decoy_key`
  (is-secret, generated when unset), and fail at its end as a wrong password does: the salt is the same across
  restarts and nodes, and for every spelling of the name the account lookup takes as one (without case, XEP-0106
  escaped), so it does not tell a name that is no user's apart.
- **STARTTLS is required** on `xmpp_port`: before it, anything but `<starttls/>` (a SASL `<auth>` included) ends the
  stream, so a client whose STARTTLS offer was stripped on the way cannot be led to sign in in the clear.
- **PLAIN** only over TLS, as a fallback for a client without SCRAM.
- **SASL2** (XEP-0388) with **Bind 2** (XEP-0386): one round trip binds a resource from the client's tag and turns on
  carbons, stream management and client state. RFC 6120 SASL and resource binding work too.
- **FAST** (XEP-0484): a client asks for a token at a SASL2 sign-in and comes back with one round trip,
  `HT-SHA-256-EXPR` (bound to the TLS session) or `HT-SHA-256-NONE` (whose count must grow each time). A token is kept
  per user and client (the client's user-agent id), encrypted (the HMAC needs it), and expires. A token stands for the
  app password the client signed in with when it asked: revoking that app password refuses the token.
- **Revocation.** A session remembers its app password and checks every minute (`check#Credential`) that it is not
  revoked and the account may still sign in; when either fails the session ends (`not-authorized`).
- **Failures** count against the source address, as the SIP engine counts failed digests (moqui-lib
  `source_guard.rs`): 5 in 10 minutes block the address (an IPv6 /64) for an hour. The account is never locked.

## Limits

- A stanza (or a stream header) is at most 256 KiB, nested at most 32 deep (`framing.rs` `MAX_DEPTH`; minidom works
  on elements recursively, so a deeper one would overflow a thread's stack); past either the stream ends
  (`policy-violation`). A WebSocket frame is at most 256 KiB, and parsed with the same depth limit.
- A connection must sign in and bind a resource within a minute (closed at the next 30-second tick), and one with
  nothing from the client for 10 minutes is closed.
- At most 20 connections from one address (over WebSocket, the client a trusted proxy names) and 10,000 in all, TCP
  and WebSocket counted together. Failed sign-ins are counted per address, as above.
- Stream management keeps at most 1,000 unacknowledged stanzas; past that the oldest go and the session cannot be
  resumed. A user has at most 10 sessions waiting for resumption.
- A stanza acts on at most 20 reactions, invitations or linked uploads; a session asks a client about at most 4
  capability hashes at a time.
- MAM pages are at most 250 messages, a room join's history 100. PEP: 200 nodes a user, 256 items a node, 256 KiB an
  item. Uploads: 60 slots a user an hour, asking for at most ten times `xmpp_upload_max_bytes` in all; a slot takes
  its PUT within an hour.

## Chat

Each stanza becomes a service call as the signed-in user, with the user's authorization (`XmppClientServices`), so
the chat rules are the web's.

- **Direct messages.** A message to `user@<domain>` goes to the direct room with that user, made on first use. The
  roster is the people of the user's direct rooms; adding a local user opens a direct room with them.
- **Rooms** are MUC rooms (XEP-0045) on `rooms.<domain>`: a room the user is a member of, or a public one, which a join
  joins in Moqui. The members are the occupants, with an affiliation from their role (owner, admin, member). A join
  gets the last 20 messages (or what `<history>` asks) and the topic as the subject. Invitations (XEP-0249 and
  mediated) add the invitee; the invitee's apps get a direct invitation. Self-ping (XEP-0410) answers whether the
  session is still joined. Room configuration and private messages in a room are not offered.
- **Messages** in, through ChatServices: a body is `send#Message` (the client's message id is its clientRef, so the
  reflection carries the client's id), a correction (XEP-0308) `edit#Message`, a retraction (XEP-0424)
  `redact#Message`, reactions (XEP-0444) `react#Message` for each one added or taken back, a reply (XEP-0461)
  `replyToEventSeq` with the fallback quote taken out, a displayed marker (XEP-0333) `mark#Read`, a chat state
  (XEP-0085) `signal#Typing`. A message to a user that carries none of these (Jingle Message Initiation) goes to their
  sessions as it is; so does an iq to a full JID (Jingle).
- **Messages** out: the chat pushes the web client gets (`chatRoom:`, `chatUser:`, `userStatus`) reach every XMPP
  session of their users through the session registry, which subscribes to the notification bus as `/notws` does. A
  direct room's message comes from the other user; the user's own messages from elsewhere come as carbons (XEP-0280).
  A room's messages go to the sessions that joined it, from `room/username`. Each has its stanza-id (XEP-0359): the
  eventSeq in a room, `<room>:<seq>` in the user's own archive.
- **History** (XEP-0313): a room's archive, and the user's own (the direct rooms; `with` one person), paged with RSM,
  by time with `start` and `end`. Edits come applied under the message's id; deleted messages are left out; reactions
  are not in the archive.
- **Presence** is the user's Moqui status: available, away and dnd set it, the session refreshes it each minute
  (`heartbeat#UserStatus`), and the last session to go marks the user offline. The presence of the people of the
  user's rooms comes from the `userStatus` pushes. Client state (XEP-0352): while inactive, presence waits (the latest
  per person) and chat states are dropped.
- **Blocking** (XEP-0191): a blocked JID's messages and presence do not reach the user's XMPP sessions.
- **Sessions** are per node: in a cluster every node gets the chat pushes, but a stanza between two XMPP sessions
  (Jingle, a PEP notification) reaches only one on the same node.

## OMEMO and encrypted rooms

- **OMEMO** (XEP-0384) is between XMPP clients. A message encrypted with it (either namespace) is kept as it came
  (`store#EncryptedMessage`, content `xmppEncrypted`), delivered and archived as it came, and marked unreadable: the web
  shows "Encrypted message (OMEMO)", and it has no text in snippets, search, notifications or agents. A room with a
  bridge refuses it, as its networks could not read it.
- PEP serves both namespaces' device lists and bundles (`eu.siacs.conversations.axolotl.devicelist`,
  `...bundles:<id>`, `urn:xmpp:omemo:2:devices`, `urn:xmpp:omemo:2:bundles`), open to read by default.
- **A Moqui room that is end-to-end encrypted** (C7, Megolm) is not served: the XMPP server could neither read nor
  write its messages. Joining it, or a message to it, is refused with that reason.

## PEP

PEP (XEP-0163) keeps each user's nodes with their items (`XmppPepNode`, `XmppPepItem`): avatars (XEP-0084), bookmarks
(XEP-0402) and the OMEMO nodes. A node is made on first publish with the publish-options the client gives
(`pubsub#access_model`, `pubsub#max_items`); a later publish whose options differ is refused (`precondition-not-met`),
and the owner can configure the node or delete it. Access: `open` is any user of this server, `presence` the people
the owner shares a room with, `whitelist` the owner alone. A publish notifies the sessions of the users who may read
the node and asked for it (`<node>+notify` in the capabilities, XEP-0115, which the server resolves by asking the
client). The bookmarks node lists the user's rooms too, with autojoin, so an app shows them.

## Files

An upload slot (XEP-0363, `upload.<domain>`) is a URL under `/xmpp-upload/` that takes one PUT of the size asked
for, within an hour, then serves the file. A PUT is checked (the token, an open slot, a declared length and type that
are the slot's) before any of its body is read; a second PUT of a slot while one is coming to this node is 409 at
once. The body is spooled as it arrives (`runtime/tmp/upload`, so a slow client holds no request worker, database
connection or memory), never past the slot's size (a longer one is 413, a shorter one 400), within
`xmpp_upload_seconds` (408 after); then stored with the slot's row locked for update in the transaction that marks it
done, so of PUTs to two nodes one writes and the other gets 409. A PUT that fails leaves no file and the slot open. A message that links it gets it as a chat file of the room (`attach#Upload`), so the web
shows it. A chat file sent to an XMPP client gets such a URL too, derived from the file and `xmpp_upload_secret`. The
URL is the capability, as XEP-0363 has it: anyone it is passed to can read the file.

## Calls

External services (XEP-0215) hand an XMPP client the embedded TURN server (and `turn_external_urls`) with a credential
keyed to the user, for its own Jingle calls. Such a credential may relay to public addresses only ([TURN](TURN.md)).
This is off unless `xmpp_turn_enabled` is true: while off, the feature is not advertised (service discovery's list goes
through `features::offered`) and a request is answered with an empty list. Even on, an external user (`EXTERNAL_USER`)
gets none, nor a user past `xmpp_turn_per_hour` credentials in the hour; each is the same empty list. Moqui's own calls
stay WebRTC in the browser.

## Push

A client registers its app server (XEP-0357, `XmppPushRegistration`, the server's secret encrypted). When its session
is detached (the phone suspended the app) and a message comes, the server sends a notification with the message
count only, at most once a minute. The app servers are other XMPP servers, so the notification needs federation
(C3b): until then it is logged as not sent.

## What is served

Service discovery answers one list (`moqui-xmpp` `features.rs`), and `session::tests::every_advertised_feature_answers`
sends a request for each advertised feature and checks it is served.

| | Served |
| --- | --- |
| Streams | RFC 6120 c2s (STARTTLS, SASL, binding), RFC 7395 WebSocket, XEP-0368 direct TLS, XEP-0198 stream management with resumption |
| Sign-in | SCRAM-SHA-256(-PLUS), PLAIN over TLS, XEP-0388 SASL2, XEP-0386 Bind 2, XEP-0484 FAST, XEP-0440 tls-exporter |
| IM | RFC 6121 (roster of direct rooms, presence), XEP-0030, XEP-0115 (resolved for PEP notifications), XEP-0199, XEP-0092, XEP-0280, XEP-0191, XEP-0054 (read only: the name) |
| Rooms | XEP-0045 (members-only and public rooms, no configuration or private messages), XEP-0249, XEP-0410 |
| History | XEP-0313 (rooms and the user's archive, RSM, `start` and `end`) |
| Messages | XEP-0308, XEP-0424, XEP-0444, XEP-0461, XEP-0333 (displayed), XEP-0085, XEP-0359, XEP-0066 links to uploads |
| PEP | XEP-0163 with publish-options, owner configure and delete; XEP-0084, XEP-0402, both OMEMO namespaces |
| Mobile | XEP-0352, XEP-0357 (registration and the notification; delivery needs federation) |
| Files and calls | XEP-0363, XEP-0215 (with `xmpp_turn_enabled`) |

**XEP-0479 levels claimed: none.** Every suite builds on the Core suite, whose server requirements include RFC 6120
server-to-server and XEP-0114 components, which C3a does not have (federation is C3b); IM Core also needs vCard
updates (XEP-0054 set), and IM Advanced XEP-0398, XEP-0153, XEP-0048, XEP-0049 and XEP-0223, which are not served.
The rows above are what the session answers, each checked by a test; the suites' compliance tester was not run.

Not served: server-to-server and components; XEP-0054 updates, XEP-0153 and XEP-0398 (avatars are XEP-0084 only);
XEP-0048, XEP-0049 and XEP-0223 (bookmarks are XEP-0402); MUC configuration, moderation and private messages; offline
message delivery outside the archive; reactions in the archive; receipts (XEP-0184) generated by the server.

## Tests

- `cargo test -p moqui-xmpp` (23, under a second): the stream reader (stanzas split across reads, entities refused,
  the size limit, a deep stanza and a long stream header refused, WebSocket frames), SCRAM (RFC 7677's example, the binding to the exporter, a downgrade refused, an
  unknown user like a wrong password), FAST (bound to its TLS session, its count), and sessions on recorded stanzas
  against an in-memory backend: STARTTLS first, and SASL before it refused; SASL2 with SCRAM-PLUS, Bind 2 and a FAST token, then FAST, and the
  token refused from another TLS session; a revoked app password ending its session and refusing its token; failed
  sign-ins blocking the source and not the account, then RFC 6120
  PLAIN and binding; a room message reflected with its id and reaching the other member, a correction, a reaction and
  the archive paged; a direct message, a carbon, and the roster; OMEMO kept as it came, refused in a bridged room, and
  an encrypted Moqui room refused; PEP for both OMEMO namespaces with notifications to who asked and a
  precondition refused; every advertised feature answering (TURN on); TURN off by default, neither advertised nor
  issued; an unknown user's salt the same from a server with the same decoy key; stream management counts and resumption inside SASL2; a
  detached session's text-free push; at most 10 detached sessions a user, and no resumption once unacknowledged
  stanzas were let go.
- `cargo test -p moqui-host --test it edge_xmpp` (about 4 s, two servers): outside clients through an edge node's
  tunnel ports, tokio-xmpp over direct TLS and a raw SASL2 client over STARTTLS, both with SCRAM-SHA-256-PLUS on the
  local node's TLS, trading direct messages; refused with the link down; no XMPP server on the edge node.
- `cargo test -p moqui-host --test it -- xmpp::` (about 6 s, two servers): an upload PUT answered before its body for
  an unknown token and a wrong length, a body over the slot's size 413, one past `xmpp_upload_seconds` 408, and two PUTs
  at once, the second waiting on the row and then 409; and a real server with tokio-xmpp as one client
  (SCRAM-SHA-256-PLUS over tls-exporter on direct TLS; a room joined, its message read on the web side and the web's
  answer back; the room's archive paged; an upload put and fetched over HTTP; an OMEMO device list published), and a
  raw SASL2 client for the other (SCRAM-PLUS with Bind 2, stream management and a FAST token; the device list read over
  PEP; the connection dropped, a message while away, FAST and resumption inside SASL2 with the message replayed).
- `./moq test framework --spec XmppServices` (7): TURN off by default, and on, an external user and a user past the
  hour's rate refused with the parameters an allowed user has; an app password's verifiers sharing the user's salt, PLAIN taking
  app passwords only, a FAST token and the session check refused once the app password is revoked, and the archive, an event by clientRef, an OMEMO message, PEP by access model and an upload's
  attachment, each refused to another user with the same parameters; the calls with authorization on, as the session
  makes them.
- `cargo test -p moqui-rtc turn`: an XMPP user's TURN credential relays to public addresses only.
- `./moq test basic-comms --spec BasicComms`: Chat apps in qvt and tui, marking an app password made before XMPP
  sign-in, and saying XMPP is not set up while `xmpp_domain` is empty.

**Not checked live:** Conversations, Monal, Dino and Gajim against a deployment, the XEP-0479 compliance tester, a
web client over `/xmpp-websocket` with channel binding behind this process's HTTPS, and push notifications reaching an
app server (which needs federation).
