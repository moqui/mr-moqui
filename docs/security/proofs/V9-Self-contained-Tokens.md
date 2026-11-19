# V9 Self-contained Tokens

The framework's status for each OWASP ASVS 5.0.0 requirement of chapter V9 at levels 1 and 2. The topic is in our words;
the requirement's own text is in [ASVS 5.0.0](https://github.com/OWASP/ASVS/tree/v5.0.0/5.0/en). How to read this
table, and what a status claims, is in the [index](README.md).

## V9.1 Token source and integrity

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-9.1.1 | 1 | Signature or MAC checked before token contents used | Framework | the MAC first: LFS `MoquiLfs v1` (`moqui-git/src/server/lfs.rs` `read_token_with`), session cookie (`verify_session_cookie`), email view key (`web/mod.rs` `verified_email_view_id`), MCP `requestState` (`moqui-llm/src/mcp/state.rs` `open`), `/rest/sm` (`service/system_sync/message.rs`), SSO ID token (huskarl, JWKS); except the mail OAuth ID token, read unsigned from the token endpoint over TLS (`moqui-mail/src/oauth.rs` `jwt_claims`) | `tokens_are_signed_and_expire` (`lfs.rs` tests); `a_request_state_is_refused_on_tamper_expiry_another_principal_or_another_request` (`mcp/tests.rs`); V9SelfContainedTokens "v5.0.0-9.1.1: a system message with no HMAC signature header is refused"; V9SelfContainedTokens "v5.0.0-9.1.1: a system message with a bad HMAC is refused, one with a good HMAC accepted" |
| v5.0.0-9.1.2 | 1 | Signing algorithms from an allowlist, never none | Framework | the framework's own tokens fix HMAC-SHA256 in code and read no `alg`; SSO ID tokens: huskarl refuses `none` (`huskarl-core` `jwt/validator` `validate_header`) and verifies only with the provider's JWKS keys; mr-moqui sets no explicit list (`moqui-host/src/sso.rs`, ES512 refused); likely | to write: in-process |
| v5.0.0-9.1.3 | 1 | Verification keys only from configured trusted sources | Framework | HMAC keys from the conf (`git-server@lfs-token-key`, `webapp-list/webapp[webroot]/session-config@cookie-key` and its derived keys, `llm-facade/mcp@request-state-key`) or the remote's `sharedSecret` row; ID token keys from the flow's configured discovery `jwks_uri`; token-header `jku`, `x5u`, `jwk` not used (huskarl) | to write: in-process |

## V9.2 Token content

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-9.2.1 | 1 | Token used only inside its validity window | Framework | LFS `e` (`read_token_with`), MCP `exp` (`state.rs` `open`), SSO `exp`/`nbf`/`iat` (huskarl), `/rest/sm` timestamp 300 s (`message.rs`), deploy attestation `component-deploy@max-age-seconds`; the mail OAuth ID token checks `exp` with 5 min leeway and no `nbf`; the email view key has no window | `tokens_are_signed_and_expire` (`lfs.rs` tests); V9SelfContainedTokens "v5.0.0-9.2.1: a system message timestamped outside its window is refused, one inside accepted" |
| v5.0.0-9.2.2 | 2 | Token kind and purpose checked | Gap | LFS prefix and operation (a download token cannot upload, `lfs.rs`), MCP version, tool and digest, a key per purpose (`session.rs` `derived_cookie_key` labels), ID tokens for sign-in only; open: SEC-RTE-86 (`/rest/sm` takes any message type on one remote's check) | - |
| v5.0.0-9.2.3 | 2 | Token accepted only when meant for this service | Framework | an LFS token names its repository, which must be the request's (`lfs.rs` `serve_lfs`); MCP state its principal and call; the email key its message; SSO and mail ID tokens `aud` is the client id (huskarl; `oauth.rs` `check_id_token`); a deploy attestation one of `component-deploy@public-urls` | to write: in-process |
| v5.0.0-9.2.4 | 2 | Issued tokens name their audience | Framework | each token kind has its own key or a label-derived key (`derived_cookie_key`) and names what it is for (LFS repository and operation, MCP principal and tool); the JWTs mr-moqui signs carry `aud` (Apple client secret `moqui-host/src/sso.rs`, VAPID `notify/push_crypto.rs`, FCM `service/fcm.rs`) | to write: in-process |
