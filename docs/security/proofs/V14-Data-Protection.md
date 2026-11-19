# V14 Data Protection

The framework's status for each OWASP ASVS 5.0.0 requirement of chapter V14 at levels 1 and 2. The topic is in our words;
the requirement's own text is in [ASVS 5.0.0](https://github.com/OWASP/ASVS/tree/v5.0.0/5.0/en). How to read this
table, and what a status claims, is in the [index](README.md).

## V14.1 Data Protection Documentation

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-14.1.1 | 2 | Sensitive data inventoried and given protection levels | Application | field `encrypt="true"`, `entity/sensitive.rs` lists, conf secrets (`conf_schema::attribute_secret`), `context/identity.rs` secret names; own secrets in Attack-Surface-Data Assets (map) | - |
| v5.0.0-14.1.2 | 2 | Each protection level has written handling rules | Application | none; Attack-Surface-Data "Threat model: stored secrets" (map) | - |

## V14.2 General Data Protection

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-14.2.1 | 1 | Secrets never in URLs or query strings | Gap | seen: `screen/mod.rs:1164-1165` signs in from `authUsername`/`authPassword` in the query; SEC-FIL-64; `api_key` in the query is refused | - |
| v5.0.0-14.2.2 | 2 | Server-side caches keep no sensitive data | Gap | likely: the entity cache keeps decrypted encrypted fields with no expiry (Attack-Surface-Data "Where a decrypted value goes" (map); `entity/cache.rs` excludes none); proxies held off by `screen-render` `Cache-Control` | - |
| v5.0.0-14.2.3 | 2 | No sensitive data to third-party trackers | Framework | no third-party script in the shell; RUM posts to the server's own `/rum/*` with inputs and text masked (`WebrootRum.js`); mail remote images hidden, `no-referrer` (map) | to write: black-box |
| v5.0.0-14.2.4 | 2 | Data protection controls implemented as documented | Gap | `known_gap_a_changed_ciphertext_is_not_detected`, `known_gap_the_key_is_aes_128_from_10_pbkdf2_iterations_and_a_fixed_salt` (`crypto_tests.rs`); `ServiceJobRun.parameters` kept unencrypted (map) | - |

## V14.3 Client-side Data Protection

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-14.3.1 | 1 | Client storage wiped when session ends | Gap | no `Clear-Site-Data` in the code; `WebrootVue.qvt.js` `logoutClick` closes the crypto store and push only; drafts (`MoquiLib.js` `drafts`) and the chat outbox (`WebrootChat.js`) stay in `localStorage` | - |
| v5.0.0-14.3.2 | 2 | No-store headers on sensitive responses | Framework | `MoquiDefaultConf.xml` `screen-render` `Cache-Control: no-cache, no-store, must-revalidate, private` by `web/mod.rs` `finish_response`; user files `private, no-store`; `/dav` sets none (likely) | V14DataProtection "v5.0.0-14.3.2: a screen's answer is Cache-Control no-store and never public"; PASS: mr-pass `Shop/ShopCatalog` "the catalog routes are public for a minute only for an anonymous request; ..." |
| v5.0.0-14.3.3 | 2 | Browser storage holds no sensitive data | Gap | likely: chat and mail drafts (`MoquiLib.js` `drafts`) and unsent chat messages (`WebrootChat.js`) in `localStorage`; chat keys in IndexedDB are encrypted (`WebrootCrypto.js`) | - |
