# Web load handling and basic WAF

Status: implemented 2026-10-19 (all five phases below); the "Later" items are open.

## Objective and scope

- Protect capacity and the login surface at the edge, before a session, a database connection or a request worker
  is spent on a request.
- Work with nothing in front (no nginx, load balancer or WAF), and as a second layer behind them.
- Per node and in memory; the edge never reads or writes the database.
- Configured in the Moqui conf XML (`<webapp>`), overridable from a component's `MoquiConf.xml` like other webapp
  settings.
- Not a full WAF: no request payload inspection or OWASP CRS style rule sets. The aim is cheap limits and coarse
  rules that hold up under load, leaving deep inspection to a dedicated WAF where one is needed.
- Out of scope here: request pool and database pool sizing (profiling and load testing come first).

## Before this work

| Layer | What existed | Where |
| --- | --- | --- |
| Connections | `webapp_max_connections` (1024) for HTTP and HTTPS together; past it the listeners stop accepting and new connections wait in the kernel backlog | `crates/moqui-host/src/http.rs` `ListenLimits`, `serve` |
| Slow clients | `webapp_header_timeout` 30s (head, kept-alive idle, TLS handshake), `webapp_body_timeout` 60s → 408, `webapp_upload_idle_timeout` 60s, `webapp_request_timeout` 300s → 504 | `ListenLimits`, `run_connection` |
| Body size | `webapp_max_body` 10 MiB, `webapp_max_upload` 2 GiB; 413 on Content-Length, and the read stops at the limit | `moqui_core::web::body_limit` |
| Workers | `request-pool-max` 8 workers, `request-queue` 64; a full queue is an immediate bare 503 | `crates/moqui-core/src/service/request_pool.rs` `try_submit` |
| Git, SSH | own pools and limits (`git_max_concurrent`/`git_max_queued`; SSH pre-auth slots, sessions, auth attempts and rejection delay) | `resource/git/server/mod.rs`, `crates/moqui-host/src/ssh/server.rs` |
| Login | lockout per account (`successiveFailedLogins`), `ipAllowed` per account | `crates/moqui-core/src/user/login.rs` |
| Business quotas | ArtifactTarpit (see below) | `crates/moqui-core/src/user/authz.rs` |

The gaps this closes:

- Nothing was per client. One address could hold all the connections or keep the queue full, and every client then
  got the 503.
- No 429, no bans, no IP, method, path or user-agent rules.
- `webapp_client_ip_header` took the first hop of the header from any peer, so the client address could be forged
  by the client itself; anything keyed by address on top of it would be too.
- Login failures were counted per account only: guessing across many usernames was not slowed.
- A 504 stopped the wait, not the job: a queued job still ran.
- The 503s had no `Retry-After`; `/metrics` counted no refusals.

Found on the way and fixed: on Postgres the account lockout never locked. `successiveFailedLogins` is
`NUMERIC(20,0)`, the driver reads it as a decimal, and the guarded update in `increment_failed` read that as 0, so
from the second failure in a row the update matched nothing and the login answered "could not count a failed login
... the account kept changing" with the count left at 1. `login.rs` `value_i64` now reads a whole decimal.

## Not the tarpit

ArtifactTarpit and web protection both answer 429, and both apply, but they are different tools for different jobs.

| | ArtifactTarpit | Web protection |
| --- | --- | --- |
| Purpose | business quotas: scraping prices, repeated orders, API plans, abuse of a particular service or screen | capacity and the attack surface: floods, scanners, password guessing, slow or greedy clients |
| Configured in | the database: `moqui.security.ArtifactTarpit` on an `ArtifactGroup`, by user group | the conf XML, `<web-protection>` under `<webapp>` |
| Keyed by | user (or anonymous) and artifact | client address (IPv6 by prefix) |
| Counts | executions of screens, services and entities | connections, requests, rule hits, auth failures |
| Checked | inside a request worker, after the session, login and authorization, through `check_tarpit`; locks are `ArtifactTarpitLock` rows | on the connection's async task, before routing, a session, a worker or the database |
| Answer | `ErrorKind::Tarpit` → 429 and the `too-many` error screen | a short plain-text response or a closed connection, with `Retry-After` where it helps; never an error screen, which would spend a worker |

The edge runs first and is cheap enough to run on every request. The tarpit knows who the user is and what they
are doing, which the edge does not. Neither replaces the other.

## Client address

`crates/moqui-core/src/web/address.rs`, `ClientAddressing`, one per factory (a factory extension):

- `trusted-proxies` on `<webapp>` (`webapp_trusted_proxies`), next to `client-ip-header` (`webapp_client_ip_header`):
  comma separated CIDR ranges (`10.0.0.0/8,fd00::/8`, or one address). Both are read from the merged conf, so a
  component can set them as literals.
- With trusted proxies, the header and `X-Forwarded-Proto` are believed only when the socket peer is one of them, and
  the address taken is the rightmost hop of the header that is not itself trusted (the first hop is whatever the
  client wrote; proxies append). With none, the header is believed from any peer and its first hop taken, as
  before, for a proxy that overwrites it (`X-Real-IP`, `CF-Connecting-IP`); startup warns about it.
- A hop may be quoted, carry a port (`1.2.3.4:443`, `[::1]:443`, as `CloudFront-Viewer-Address` does) or be a
  `Forwarded` `for=` pair.
- One resolver for everything: `moqui_core::web::client_ip` (login `ipAllowed`, visits), the request log, git
  (`client_ip` of a git or LFS request), and the edge.
- IPv6 clients are counted by prefix (`client-ipv6-prefix`, 64), since one client usually holds a whole /64.

## Configuration

`<web-protection>` under `<webapp>`, as shipped in `MoquiDefaultConf.xml`. Each attribute left out or expanding to
nothing falls back to its `webapp_*` property and then to the built-in default, so the old properties keep working.

```xml
<webapp name="webroot" ... client-ip-header="${webapp_client_ip_header}"
        trusted-proxies="${webapp_trusted_proxies}">
    <web-protection enabled="${webapp_web_protection}" client-ipv6-prefix="64" max-tracked-clients="100000">
        <connections max="${webapp_max_connections}" max-per-client="64"
                header-timeout="${webapp_header_timeout}" shutdown-timeout="${webapp_shutdown_timeout}"
                http2-max-streams="${webapp_http2_max_streams}"/>
        <requests methods="GET,HEAD,POST,PUT,PATCH,DELETE,OPTIONS" max-uri-length="8192"
                max-header-bytes="65536" max-headers="100" max-body="${webapp_max_body}"
                max-upload="${webapp_max_upload}" body-timeout="${webapp_body_timeout}"
                upload-idle-timeout="${webapp_upload_idle_timeout}" request-timeout="${webapp_request_timeout}"/>
        <overload queue-timeout="10" retry-after="5"/>

        <rate-limit name="all" requests="50" seconds="1" burst="500"/>
        <rate-limit name="login" path="/Login/**,/rest/login,/rest/login/**,/SecondFactor/**" methods="POST"
                requests="10" seconds="60" burst="10"/>

        <ban name="auth-failure" events="auth-failure" threshold="20" seconds="600" duration="900"/>
        <ban name="abuse" events="rate-limited,denied" threshold="300" seconds="60" duration="300"/>

        <client-rule name="loopback" action="exempt" cidr="127.0.0.0/8,::1"/>
        <path-rule name="scanners" action="deny"
                path="/.env,/.git/**,/wp-admin/**,/wp-login.php,**/*.php,**/*.asp,**/*.aspx"/>
        <user-agent-rule name="scanners" action="deny" contains="sqlmap,nikto,masscan,zgrab"/>
    </web-protection>
</webapp>
```

- Global, always on: `connections` `max` and the timeouts, `requests` `max-uri-length`, `max-header-bytes`,
  `max-headers`, the body limits and timeouts, and `overload`.
- Per client, on when `enabled` is `true`, or when it is empty (the default) and `trusted-proxies` names one or more
  proxies: `max-per-client`, `methods`, the rate limits, bans and rules. Without trusted proxies a server behind a
  proxy would see every client as the proxy, so a server with nothing in front sets `enabled="true"`
  (`webapp_web_protection=true`). Startup logs which it is.
- `rate-limit`: `requests` per `seconds` sustained and `burst` at once (`requests` when left out), over the requests
  its `path` and `methods` match; a request takes a token from each limit that matches it, and none is taken when
  one refuses. GCRA, one arrival time per client and limit.
- `ban`: `threshold` of its `events` (`auth-failure`, `rate-limited`, `denied`) within `seconds` bans the client for
  `duration` seconds.
- Rule actions: `client-rule` `deny` or `exempt` (no rules, rate limits or bans; the global limits still apply);
  `path-rule` and `user-agent-rule` `deny`.
- Paths are `url-pattern` style globs (`*` within a segment, `**` across segments), matched without regard to ASCII
  case against the path with its `%XX` escapes decoded. Not regular expressions: there is no regex crate in the tree,
  and globs are cheaper on every request.
- Merging (`crates/moqui-core/src/context/conf.rs` `merge_web_protection`): `web-protection` and its single children
  merge attributes; `rate-limit`, `ban` and a rule with a `name` merge by `name`; a rule without one is added. Any of
  them takes `enabled="false"`, so a component can turn off a default, such as the loopback exemption.
- XSD: `trusted-proxies` and `web-protection` in `runtime/base-component/framework/xsd/moqui-conf-3.xsd`, "No Java
  counterpart" as `token-route` is.
- A bad entry (CIDR, number, event, action) logs an error naming it and is left out, and the rest apply.

## Order of checks

`crates/moqui-host/src/guard.rs` (`Guard`) and `crates/moqui-host/src/http.rs`. Cheapest first.

1. At accept, by socket address, before the TLS handshake (`Guard::admit`): a denied or banned client, or one over
   `max-per-client`, is closed. Trusted proxies and exempt clients are not counted. Past `max`, today's backpressure.
2. Per request, once hyper has read the head (`max_buf_size` and `max_headers` from `max-header-bytes` and
   `max-headers`, → 431), before the router (`run_connection`, then `Guard::check`):
   - path and query over `max-uri-length` → 414;
   - the client resolved; an exempt one skips the rest;
   - banned, denied, or a denied user agent → 403;
   - a method not in `methods` → 405 with `Allow`;
   - a denied path → 404, giving no hint that a rule matched;
   - each matching `rate-limit` takes a token; none left → 429 with `Retry-After`.

   A refusal of a client that came straight to the listener carries `Connection: close`, or on HTTP/2 ends the
   connection with a GOAWAY; through a trusted proxy it does not, since that connection carries other clients. On
   HTTPS over HTTP/2 the request head is bounded by `max-header-bytes` (as `max_header_list_size`), a connection has at
   most `http2-max-streams` open streams (100), and one with no request for `header-timeout` gets a GOAWAY. This covers every route, `/git`, `/elastic`, `/kibana` and
   `/notws` included; `/health` and `/metrics` keep their `status_allowed` check.
3. In the request job (`dispatch_screen`): a full queue → 503 with `Retry-After`; a job whose waiter is gone (the
   504 fired, the client went away) returns at once when a worker takes it; a job that waited past `queue-timeout`
   is answered 503 with `Retry-After` instead of run. The busy 503s of git, the search proxy and `/notws` carry
   `Retry-After` too.
4. Events feed the bans: `rate-limited` and `denied` from the edge, `auth-failure` from core. `login.rs` notes
   credentials that were not valid (a login form, Basic auth, `login_key` or API key, git) in a thread-local
   (`moqui_core::web::take_auth_failure`); the request job and the git jobs take it after the work, and a worker's
   `WorkCleanup` clears it so it never reaches the next job. Core has no coupling to the host. A ban is logged once
   at warn, with the address, the ban's name and its duration; single refusals log at debug.

## State and limits

- One in-memory table keyed by client address, in 32 locked shards. Each entry holds a GCRA arrival time per rate
  limit, the window and count of each ban, a ban-until time, and the open connection count.
- Bounded by `max-tracked-clients`; a sweep every 30 seconds drops entries idle past their longest window. When the
  table is full, new clients are served untracked (fail open: the global limits still hold) and `edge_untracked`
  rises.
- Per node. With N nodes behind a balancer the effective per-client rate is N times the configured one.

## Visibility

`/metrics` lines: `edge_connections_refused`, `edge_denied`, `edge_rate_limited`, `edge_banned_clients`,
`edge_tracked_clients`, `edge_untracked`, `request_queue_full`, `request_queue_expired`, `request_abandoned`.

## Phases

1. `Retry-After` on the 503s, abandoned jobs skipped, `queue-timeout`, the `/metrics` counters.
2. Trusted-proxy client address and the one resolver.
3. `<web-protection>` parsing; `ListenLimits` and the body limits read from it with the properties as defaults;
   header and URI limits; the method, path, user-agent and client rules.
4. Per-client connection caps and rate limits, with 429.
5. The auth-failure marker and bans.

Later, separately:

- a cheap lane for static resources, so they do not take request workers;
- a small worker reserve for admin and health;
- cluster-shared bans;
- adaptive shedding by queue latency;
- 404 runs as a ban event, with a much higher threshold than `abuse`, if scanners call for it (legitimate clients hit
  404s too, so they are not counted now).

## Testing

- Unit, `address.rs`: CIDR ranges, IPv6 masks, hop parsing, the rightmost untrusted hop, the header ignored from an
  untrusted peer.
- Unit, `guard.rs`: globs and decoding; off without trusted proxies unless enabled; rate limits by path and method
  with `Retry-After`; client, method, path and user-agent rules and exemption; bans from events and at accept;
  connection caps given back; IPv6 prefixes; a full table; bad entries; the shipped `MoquiDefaultConf.xml` parsed.
- Unit, `conf.rs`: merging by name, `enabled="false"` on a default, adding where there is none. `login.rs`: the
  marker, a decimal count.
- Sockets, `http.rs` tests through `serve`: 404, 429 with `Retry-After`, 414, 431; a client over its connections
  closed and served again once one closes.
- Seeded server, `tests/it/web_protection.rs`, behind a trusted proxy (loopback, `X-Forwarded-For`): wrong Basic
  credentials ban that client and no other, the rightmost untrusted hop is the client, a rate limit gives 429 with
  `Retry-After`, `/metrics` shows it, and the account lockout counts failures in a row.
- Not covered end to end: a saturated request pool (queue full, `queue-timeout`, an abandoned job), a ban expiring,
  and the sweep. Those paths are small and reviewed, not exercised by a test.
