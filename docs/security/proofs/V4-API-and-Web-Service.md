# V4 API and Web Service

The framework's status for each OWASP ASVS 5.0.0 requirement of chapter V4 at levels 1 and 2. The topic is in our words;
the requirement's own text is in [ASVS 5.0.0](https://github.com/OWASP/ASVS/tree/v5.0.0/5.0/en). How to read this
table, and what a status claims, is in the [index](README.md).

## V4.1 Generic Web Service Security

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-4.1.1 | 1 | Content-Type matches the body and names a charset | Gap | screen output (`screen/sri/mod.rs` `content_type_header`) and static files (`static_files.rs` `media_type`) carry one; some `text/plain` answers do not (`web/mod.rs` `send_resource_response` 404 and 500, the qz `send_bytes`) | - |
| v5.0.0-4.1.2 | 2 | Only browser pages redirect HTTP to HTTPS | Gap | with `@https-enabled` true, `render.rs` `redirect_to_https` 302s every screen path, `/rest` included (`require-encryption` defaults true, `rest.xml` sets none); with it false nothing redirects | - |
| v5.0.0-4.1.3 | 2 | Clients cannot override headers a proxy sets | Configuration | no header believed by default (`web/address.rs` `ClientAddressing`, `web::client_ip`); behind a proxy `@client-ip-header` with `@trusted-proxies` (a header with no list is believed from any peer; ignored on front and edge roles (map)); `X-Forwarded-Proto` behind the same gate | - |

## V4.2 HTTP Message Structure Validation

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-4.2.1 | 2 | HTTP message boundaries resist request smuggling | Framework | hyper 1.12 (HTTP/1.1) and h2 0.4.20 frame each request (`moqui-host` listeners, `Cargo.lock`); library behaviour not read; a proxy in front is the deployer's (likely) | to write: black-box |

## V4.3 GraphQL

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-4.3.1 | 2 | GraphQL queries limited by depth, amount or cost | N/A | no GraphQL endpoint (see NOTES on entity REST `dependentLevels`) | - |
| v5.0.0-4.3.2 | 2 | GraphQL introspection off in production | N/A | no GraphQL endpoint | - |

## V4.4 WebSocket

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-4.4.1 | 1 | WebSockets only over TLS | Configuration | TLS listener (a certificate in `runtime/cert` or ACME) or a TLS proxy; pages open `wss` when served over https (`WebrootVue.qvt.js:10670`, `WebrootCall.js:1550`, `WebrootTerminal.js:172`); plain http serves `ws` | - |
| v5.0.0-4.4.2 | 2 | WebSocket handshake Origin checked against an allowlist | Framework | `moqui-host/src/notws.rs` `origin_allowed` for `/notws`, `/termws` and `/callws` (`notws/rtc.rs` `guest_upgrade`), `*` not counted; `/xmpp-websocket` (off) checks none and reads no cookie | SecurityAccess "A01 a /notws upgrade from a foreign Origin is refused, with a logged-in session" |
| v5.0.0-4.4.3 | 2 | Dedicated WebSocket tokens follow session rules | Framework | `/notws`, `/termws` take the session cookie or a login key (`notws.rs` `upgrade`), rechecked each 60 s (map); `/callws` a guest credential of 40 characters, hash kept, 4 hours, rechecked each 30 s (`notws/rtc.rs` `guest_upgrade`; map) | Calls "revoking the link removes its guest, whose credential then works nowhere" |
| v5.0.0-4.4.4 | 2 | WebSocket tokens come from the authenticated HTTPS session | Framework | `/notws` uses the page's own session cookie (`notws.rs` `upgrade`); the `/callws` credential is issued by `joinGuest` after the link token check and never put in a URL (map) | to write: black-box |
