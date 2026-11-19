# V11 Cryptography

The framework's status for each OWASP ASVS 5.0.0 requirement of chapter V11 at levels 1 and 2. The topic is in our words;
the requirement's own text is in [ASVS 5.0.0](https://github.com/OWASP/ASVS/tree/v5.0.0/5.0/en). How to read this
table, and what a status claims, is in the [index](README.md).

## V11.1 Cryptographic Inventory and Documentation

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-11.1.1 | 2 | Written key management policy and key lifecycle | Gap | no lifecycle policy; rotation is by hand and planned only (`plans/Sealed-Secrets.md` §3.11); `session-config@cookie-key` has no previous key (`known_gap_a_new_session_cookie_key_ends_every_session_cookie`) | - |
| v5.0.0-11.1.2 | 2 | Maintained inventory of keys, algorithms and certificates | Gap | partial: keys listed in `docs/security/Attack-Surface-Data.md` Assets and `plans/Sealed-Secrets.md` §3.11 (a plan); no kept inventory of algorithms, certificates or permitted uses; the System Keys screen is not built | - |

## V11.2 Secure Cryptography Implementation

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-11.2.1 | 2 | Crypto from validated libraries, no home-made primitives | Framework | rustls, rcgen, DKIM and edge keys on `aws-lc-rs` (`moqui-host/src/tls.rs` `server_config`); RustCrypto `aes`, `cbc`, `pbkdf2`, `sha2`, `hmac`; `argon2`; `getrandom` (workspace `Cargo.toml`); no own primitive seen; aws-lc-rs without its FIPS feature; `rsa` (RUSTSEC-2023-0071) signs with blinding (`moqui-lib/src/signer.rs`) | `tls_tests.rs` `cryptography_comes_from_the_validated_libraries_and_none_is_home_made` (in-process) |
| v5.0.0-11.2.2 | 2 | Algorithms and keys swappable, data re-encryptable | Gap | stored field has no algorithm or key id (`moqui-entity/src/entity/crypto.rs` `encrypt`; `known_gap_equal_plaintexts_give_equal_ciphertext`); only AES-128-CBC PBE or DES selectable (`scheme_for`); rekey needs `entity-facade@crypt-pass-old` and a rewrite of every row; cookie key has no previous (`known_gap_a_new_session_cookie_key_ends_every_session_cookie`); argon2 parameters fixed in code (`user/password.rs`); legacy password hashes do move to argon2id | - |
| v5.0.0-11.2.3 | 2 | Every primitive gives at least 128-bit security | Gap | DES (56-bit) still decrypts through `decrypt-alt` (`known_gap_des_ciphertext_still_decrypts_through_the_alternates`); DKIM key made RSA-2048, about 112-bit (`moqui-mail/src/auth.rs` `generate`; seen, not in register); TLS suites, AES-128, P-256 and Ed25519 elsewhere | - |

## V11.3 Encryption Algorithms

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-11.3.1 | 1 | No ECB or weak padding schemes | Gap | field encryption is CBC with a constant IV (`crypto.rs` `CONSTANT_IV`), so deterministic, and PKCS#7 padding with no MAC (`known_gap_equal_plaintexts_give_equal_ciphertext`, `known_gap_a_shared_16_byte_prefix_gives_a_shared_first_block`); no ECB and no RSA PKCS#1 v1.5 encryption seen | - |
| v5.0.0-11.3.2 | 1 | Authenticated ciphers and modes for stored data | Gap | every encrypted field is AES-128-CBC PBE, DES-CBC on decrypt (`crypto.rs` `crypt_bytes`; `known_gap_des_ciphertext_still_decrypts_through_the_alternates`); the AEAD `v2` format is planned only (Sealed-Secrets §3.2) | - |
| v5.0.0-11.3.3 | 2 | Ciphertext tampering detected by AEAD or MAC | Gap | no MAC or AEAD on stored fields (`crypto.rs` `decode_stored`; `known_gap_a_changed_ciphertext_is_not_detected`, `known_gap_a_value_sealed_under_the_public_default_pass_is_read_as_valid`) | - |

## V11.4 Hashing and Hash-based Functions

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-11.4.1 | 1 | Approved hash functions only, never MD5 | Gap | MD5 in PBEWithMD5AndDES decrypt (`crypto.rs` `pbes1_md5_des`; `known_gap_des_ciphertext_still_decrypts_through_the_alternates`); QZ Tray signing is SHA1withRSA by default (`moqui-lib/src/signer.rs`, `web/mod.rs` `qz_signer`; seen, not in register); MD5 SIP digest for legacy phones (`service/sip_credential.rs`) | - |
| v5.0.0-11.4.2 | 2 | Passwords stored with a slow password hash | Framework | argon2id at the crate defaults m=19 MiB, t=2, p=1 (`moqui-core/src/user/password.rs` `hash_password`); a legacy SHA hash becomes argon2id at the next sign-in | `hash_password_is_an_argon2id_phc_string` (`password.rs`), `a_legacy_hash_verifies_and_the_stored_value_becomes_an_argon2_string` (`user/login_tests.rs`) |
| v5.0.0-11.4.3 | 2 | Signature and integrity hashes collision resistant, long enough | Gap | QZ Tray signatures use SHA-1 (`signer.rs` default `SHA1withRSA`, `web/mod.rs` `qz_signer`) once a QZ key is installed; seen, not in register; elsewhere HMAC-SHA256 session MAC (`user/session.rs` `sign_session`), SHA-256 certificate pins (`tls_pin.rs`) | - |
| v5.0.0-11.4.4 | 2 | Keys from passwords use a stretching KDF | Gap | field key is PBKDF2-HMAC-SHA256, 10 iterations, fixed salt (`crypto.rs` `pbes2_aes_key`; `known_gap_the_key_is_aes_128_from_10_pbkdf2_iterations_and_a_fixed_salt`); a generated pass is 32 random bytes, an operator's own pass is barely stretched | - |

## V11.5 Random Values

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-11.5.1 | 2 | Unguessable values from a CSPRNG, 128 bits or more | Gap | `getrandom` throughout, no UUID tokens: session id and token 160 bits (`session.rs` `random_hex`), login key 240 (`user/login.rs`), generated secrets 256 (`context/conf_env.rs` `generate_secrets`); not met: `reset#Password` makes a 12-character reset password, 72 bits (`UserServices.xml` `randomString(12)`; seen, not in register) | - |

## V11.6 Public Key Cryptography

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-11.6.1 | 2 | Approved key generation and signature algorithms | Gap | keys: ECDSA P-256 by `rcgen` (`cluster/mod.rs`, `tls.rs`, `acme/order.rs`), Ed25519 (`edge_link/member_key.rs`, SSH host key), RSA-2048 DKIM by aws-lc-rs; not met: QZ Tray signature SHA1withRSA by default (`signer.rs`; seen, not in register) | - |
