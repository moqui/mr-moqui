# Cluster coordination

How instances that share a database find each other, and how cache invalidation and notifications cross the process boundary. Session bytes do not. The session struct and its flush modes are Sessions and login in `plans/archive/Architecture-Proposal.md`.

This follows that section and the cache row in the same proposal. `moqui-hazelcast` is the Java behavior being replaced. It is an add-on component, not part of the framework tree.

## Decision

A cluster exists when more than one instance uses the same database. Membership is a table in that database. Each instance upserts its own row on startup and refreshes a heartbeat. Peers are the other rows whose heartbeat is newer than a few intervals. There is no seed list and no multicast.

Peers call each other over HTTPS. The cluster endpoint requires a client certificate. The certificate and its private key are secret configuration, not database rows: `cluster_tls_cert` and `cluster_tls_key` `default-property`s in `MoquiDefaultConf.xml`, the `entity_ds_crypt_pass` pattern, set by config file or environment and generated into `runtime/.env` when left at the `CHANGEME` default (Decision 1 of `plans/archive/MimoReview.md`). Clustered deployments provision the same pair on every instance; the pair is base64 DER on single-line `.env` values. The connection between instances carries a TLS signature, and does not carry the key. The session-cookie HMAC key (`session_cookie_key`) is a different property. The two keys stay distinct.

The local publish stays a `SimpleTopic`. The local `MCache` drop runs on the writer. The peer message is the cache key, queued at commit and sent by one host thread after the enlisted client is back in the pool. The topic does not carry the entity body or the session map. The receiver applies the message locally and does not send it on. One live member means no fan-out.

The member row holds the cluster base URL, the address peers call. Only that endpoint requires the client certificate. Public application routes do not.

`moqui-host` owns the HTTPS listener and the client calls. The member table, the key-material loading, and the topic listener are in `moqui-core`, beside the cache and the session. There is no new crate, no Redis, no gossip library, no Raft, and no Hazelcast. `rustls` is already the TLS stack.

Operational state that instances must share, or that must survive a restart, goes in the database. Secrets do not. Key material is configuration (`runtime/.env` or the deployment's environment), never a row. Logs, the embedded Postgres data directory, and temp files stay on disk. The database files themselves stay files.

## What moqui-hazelcast wires

The component turns on five uses. Two stay, as topics. Three do not.

| Java use | Disposition |
|---|---|
| `entity-cache-invalidate` topic | Stays a `SimpleTopic`. The cluster listener fans it out. The local `MCache` drops the named entry. Cache entries are not stored in a distributed map. |
| `notification-message` topic | Stays a `SimpleTopic`. The cluster listener fans it out. The instance that holds that user's WebSocket applies it. The others ignore it. |
| Web session filter | Not ported. The session snapshot is a database row. That row is how instances share a session. Peers do not copy session bytes. |
| `service-executor` | Not ported. Service jobs already coordinate through the database. |
| `javax.cache` distributed cache | Not ported. There is one local cache. Invalidation is the cluster behavior. |

## Membership

The member row is the instance id, the cluster base URL, the heartbeat timestamp, and key fingerprints (SHA-256 of the session-cookie HMAC key, and the certificate fingerprint). Startup upserts it. A row that stops refreshing drops out of the peer set after a few intervals. The keys themselves are configuration, shared by provisioning. A member whose fingerprints differ from the live peer set fails startup with a clear message, so a copy-paste provisioning mistake is caught instead of surfacing as invalid cookies.

## Transport

The sender is a small trait. The default implementation is gRPC on HTTP/2 to the peer's cluster port, with the shared client certificate (2026-11-03; it was an HTTPS POST to `{base}/cluster/{topic}`, which stays beside it, below). The client keeps that connection. The send runs on one host thread, after commit, and only after the enlisted client has been returned. The message is the cache key and carries an origin id. An invalidation is safe to apply twice. A failed send is retried briefly. The optional senders are a `russh` session with a channel for each topic (`plans/archive/Ssh.md`) and Postgres `NOTIFY` (below).

A notification carries a message id so a retry does not deliver it twice.

As built (`moqui-core/src/cluster`, `moqui-host/src/cluster_edge/`):

- The publishing thread only queues the message (up to 10,000; past that a message is dropped and counted). One
  sender thread takes what is waiting, sends one invalidation for each entity (a peer clears the entity's whole
  cache for one), and posts the batch to each live peer, the peers at once (`Sender::post_batch`). A peer applies a
  batch in order and stops at the first message it cannot apply for now (5xx); that one and those after it are sent
  again, in order, each message tried three times with 100 ms and 500 ms waits. A peer that took nothing on the last
  try is skipped for five seconds. A message to one member (`cluster::send_to`, call signaling) goes from its own
  queue, all of a member's waiting messages as one batch, one try.
- **One port, two protocols.** On the cluster port TLS picks by ALPN (`cluster_edge/listen.rs`, on the host's
  runtime):
  - `h2`: gRPC, `moqui.cluster.v1.Cluster.Post` (`crates/moqui-host/proto/cluster.proto`), which carries a batch and
    answers a status for each message, the HTTP status the POST would give. The sender (`cluster_edge/grpc.rs`) keeps
    one HTTP/2 connection to each peer, with PINGs every 15 s.
  - `http/1.1`, or no ALPN: `POST /cluster/{topic}`, one message each; applied is `200` with an empty body (a `204`
    would carry no Content-Length, which a member built before gRPC reads by).
  - `cluster_protocol` (empty or `grpc`, the default; `rest`) sets what a member speaks as listener and sender. A
    sender offers `h2, http/1.1`, and posts over HTTP/1.1 to a peer that did not choose h2 (an older member, or one
    set to `rest`), trying gRPC again after five minutes. So members of different builds or settings still reach each
    other, and the base URL and the member row are the same for both.
  - Limits: 64 connections; the TLS handshake in 10 s; then, from a peer holding the cluster certificate, an HTTP/1.1
    request head within 60 s of the last exchange, and an HTTP/2 connection with no request for 60 s closed with a
    GOAWAY.
- The live peers are kept, and read again on the heartbeat tick. A member that joins is posted to from the next tick
  of each other member.
- Heartbeats and the liveness check use the database's clock (`SELECT CURRENT_TIMESTAMP`), so members whose clocks
  differ still see each other. A member row with no heartbeat for ten liveness windows is deleted.
- The listener binds `cluster_bind_address`, default `0.0.0.0`, on `cluster_https_port` (8444; 8443 is the HTTPS
  listener). `cluster_base_url` is how the others reach this member; its default `https://127.0.0.1:8444` is for one
  host only, and a cluster where a live member has a loopback base URL logs an error.
- A generated `instance_id` is written to `runtime/db/instance_id`, not to `runtime/.env`, so copying `.env` between
  members does not give two members one id. An `instance_id` property still wins.

## NOTIFY transport

`cluster_transport=notify` is for members that share the entity database and cannot open a connection to each other. It works on embedded and external Postgres. A MySQL entity database refuses it at startup. The HTTPS POST stays the default.

- A notify member advertises `pg-notify:` as its base URL, the default for this transport when `cluster_base_url` is empty. The fingerprint check is unchanged. The database credential is what reaches the channel, and the check catches a provisioning mistake.
- The peer list folds every `pg-notify:` member into one entry, so the sender thread posts each message once for all of them. One dispatching sender (`moqui-host/src/cluster_notify.rs`) routes by base URL: `pg-notify:` to `pg_notify('moqui_cluster', ...)` on a pool connection, `ssh://` to the SSH sender, and anything else to the HTTPS POST. A member on any transport reaches a notify member, and the reverse. `cluster_transport` only picks what this member advertises and listens on.
- The payload is the topic, the origin id, and the HTTPS body JSON, one per line. Postgres refuses a payload of 8000 bytes or more. A message past 7,900 bytes goes in a `moqui.server.ClusterMessage` row, and the payload carries `@` and the row id. Every member reads the row, and rows older than the liveness window are deleted once a minute by each notify member.
- A notify member holds one connection outside the pool in `LISTEN moqui_cluster`, on its own thread. The embedded cluster's `max_connections` has a slot for it. A payload with this member's origin is skipped. Anything else is applied through the same path as `POST /cluster/{topic}`, including the message-id check for notifications.
- A notification sent while the `LISTEN` connection is down is lost, as an HTTPS post that fails every try is. The thread reconnects with a backoff of up to five seconds and then clears the entity caches, so a missed invalidation cannot leave a stale row.

## Sessions

The share is the `moqui.server.HttpSession` row through the entity one-cache. A second instance loads that row by id. Guarantee mode bypasses cached clean state with primary reads; it does not rely on invalidation delivery. Invalidation uses the entity-cache topic, which drops the one-cache entry. The receiver does not apply a copy of the attribute map. Session bytes still do not cross the cluster connection. The cookie signature is HMAC-SHA256 over the session id. That key is its own secret property (`session_cookie_key`), generated into `runtime/.env` when left at the `CHANGEME` default. It is not the cluster certificate. A bad MAC is an unknown session. The merge, the reserve connection, and the flush modes are Sessions and login in `plans/archive/Architecture-Proposal.md`.

## When it lands

The session row is written in phase 7 of `plans/archive/Implementation-Phases.md`, with the web edge. The member heartbeat, the key fingerprints, and the topic fan-out land in phase 10, with notifications. A single-node gate does not need a second instance.

## Out of scope

- A distributed service executor.
- A replicated in-memory map or other Hazelcast primitive beyond the two topics.
- Sticky sessions as a requirement. A load balancer may be sticky. Failover without stickiness uses the session row and the guarantee flush.
- A file snapshot under `runtime/sessions`.
- A seed list, multicast, or any discovery that does not read the member table.
- A pre-shared key sent on the request. The credential on the wire is the TLS client certificate.
- Key material as database rows. Keys are configuration (`runtime/.env` or environment).
