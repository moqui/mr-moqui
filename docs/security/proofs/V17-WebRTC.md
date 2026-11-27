# V17 WebRTC

The framework's status for each OWASP ASVS 5.0.0 requirement of chapter V17 at levels 1 and 2. The topic is in our words;
the requirement's own text is in [ASVS 5.0.0](https://github.com/OWASP/ASVS/tree/v5.0.0/5.0/en). How to read this
table, and what a status claims, is in the [index](README.md).

## V17.1 TURN Server

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-17.1.1 | 2 | TURN relays only to non-reserved addresses | Framework | `crates/moqui-rtc/src/turn/mod.rs` `peer_allowed`: media nodes' media ports and `allowed-peers` only; an XMPP user's credential only `is_public` addresses (no loopback, private, link-local, shared, multicast, broadcast, documentation; IPv4 inside IPv6 judged); TURN off until `media-server/turn-server@enabled`; SEC-COM-18 open (host's own public address) | `a_peer_that_is_no_media_node_is_refused`, `an_xmpp_users_credential_relays_to_public_addresses_only` (`crates/moqui-rtc/src/turn/tests.rs`) |

## V17.2 Media

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-17.2.1 | 2 | DTLS certificate key handled under a key policy | Gap | key made per leg in memory and never stored (str0m 0.25 `Rtc` build calls `generate_certificate` on aws-lc-rs; `sfu/mod.rs` sets none); no documented key policy names it | - |
| v5.0.0-17.2.2 | 2 | Approved DTLS cipher suites and SRTP profiles | Framework | str0m 0.25 with dimpl 0.9 on aws-lc-rs (`Cargo.toml` `str0m` features): DTLS 1.2; ECDHE-ECDSA AES-GCM and ChaCha20-Poly1305 suites; SRTP AEAD_AES_256_GCM, AEAD_AES_128_GCM, AES128_CM_HMAC_SHA1_80 (dependency source; no conf); DTLS offers only ECDHE-ECDSA AES-GCM and ChaCha20 with the extended master secret, SRTP AEAD and AES-CM profiles; a weak ClientHello gets no ServerHello; the DTLS suite follows the client's order; dimpl's PSK suite is filtered out as no PSK is configured | `moqui-rtc` `sfu/tests.rs` `the_media_servers_dtls_and_srtp_take_only_strong_suites` (in-process) |
| v5.0.0-17.2.3 | 2 | SRTP authentication checked against RTP injection | Framework | str0m `SrtpContext::unprotect_rtp` checks the tag (or AEAD) and drops a failing packet; a datagram no peer's ICE session takes is dropped (`crates/moqui-rtc/src/sfu/mod.rs` `handle_receive`); SIP SRTP: 64-packet replay window and tag (map) | `moqui-rtc` `sfu/tests.rs` `rtp_that_fails_its_srtp_authentication_is_dropped_at_the_sfu`; `packets_round_trip_across_a_rollover_and_tampering_and_replays_are_refused` (in-process) |
| v5.0.0-17.2.4 | 2 | Media server survives malformed SRTP packets | Gap | SRTP layer met: str0m's RTP receive returns no error, so a bad packet is dropped and the leg kept (`sfu/peer.rs` `handle_input` disconnects only on an error); the payload past it: SEC-COM-87 open (Opus decoder without bounds checks; captions and phone-leg threads catch no panic; to check) | - |

## V17.3 Signaling

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-17.3.1 | 2 | Signaling rate-limited to survive floods | Gap | `/callws`: relayed frames 20 then 5 a second, peer frames 60 (`crates/moqui-host/src/rtc/driver.rs` `RelayBucket`), 64 waiting sockets, 4 a source (SEC-COM-04 fixed); `join` and offer frames not rate-limited (likely); SIP 20 a second a source (map), but SEC-COM-83 open (a To tag passes it), SEC-COM-26 deferred | - |
| v5.0.0-17.3.2 | 2 | Signaling survives malformed messages | Gap | `/callws`: frame 128 KiB, call and leg ids 64 characters, a bad frame answered with an error code (`crates/moqui-host/src/notws/rtc.rs`), SDP 64 KiB (map); SIP message 65,535 bytes (map); SEC-COM-82 open (XMPP stanza past 256 KiB held in memory before sign-in, Medium lead) | - |
