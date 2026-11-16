# Attack surface: data and stored secrets

The database, field encryption, generated secrets, and the threat model for stored secrets. This is one file of the attack surface map. The index, how to read a row, the defaults and the lock-down advice are in [Attack-Surface.md](Attack-Surface.md).

## Database

`entity_ds_db_conf` default `h2` (and an empty name) selects the embedded Postgres cluster. There is no H2 driver and no H2 TCP server.

Embedded Postgres is on when the entity datasource is embedded and the build has `embedded-postgres` (Dev, Lite, Maximal). The data directory is `runtime/db/embedded`. On Unix there is no TCP: `listen_addresses` is empty, and the socket directory is the data directory. `PORT` 5432 names the socket file `.s.PGSQL.5432`. A second postmaster (search, or the sim overlay) uses the same socket recipe. The client uses the bootstrap superuser, the password in the cluster `pgpass` file, and `NoTls`. This tree does not set `unix_socket_permissions` or `pg_hba.conf`. On Windows the same default listens on `127.0.0.1` and an ephemeral port.

External Postgres or MySQL is off unless `entity_ds_db_conf` is `postgres`, `mysql`, or `mysql8`. This process is a client. Omitted ports default to 5432 and 3306. TLS is on unless `entity_ds_sslmode=disable`.

## Field encryption

`entity_ds_crypt_pass` ships as `MoquiDefaultPassword:CHANGEME` with `is-secret="true"`. It is the `entity-facade` `crypt-pass`. The algorithm is `PBEWithHmacSHA256AndAES_128`, salt `20201202`, 10 iterations. The same secret encrypts an OAuth refresh token and a client secret stored on `EmailOAuthProvider`, the SSO client secrets (`OauthFlow.secret`, `OidcFlow.secret`), the SAML keystore and private key passwords, and a calendar publish link's token (`CalendarShare.publishToken`, still found by value since the encryption is deterministic). Alternate decrypt still tries `PBEWithMD5AndDES` with the current pass and with `entity_ds_crypt_pass_old`, and the AES algorithm with the old pass. The old pass ships as the same `CHANGEME` sentinel and is not replaced by the generator. What this encryption protects, and its known gaps, are in [Threat model: stored secrets](#threat-model-stored-secrets).

`entity_ds_user` and `entity_ds_password` default to `sa` and `sa`. Those are the old datasource properties. The embedded Postgres client does not connect with them.

## Secrets

Startup, before listeners, replaces each secret `default-property` that still ends in `CHANGEME` after the environment and `runtime/.env` are read. The value is 32 CSPRNG bytes, base64, written into `runtime/.env`. A new file is mode `0600`. An unwritable `.env` fails startup. `entity_ds_crypt_pass_old` is the exception: the sentinel is kept. `cluster_tls_cert` and `cluster_tls_key` are generated as a pair when `cluster-tls` is on, which it is in every server package. The same path fills `ssh_host_key`, `session_cookie_key`, `git_lfs_token_key` and `acme_account_key`. The key is then on the same host as the data it protects; see [Threat model: stored secrets](#threat-model-stored-secrets).

## Threat model: stored secrets

This section says what a stored secret is protected from, and what it is not protected from. Each claim names its test; [Proof-Tests, Stored secrets](Proof-Tests.md#stored-secrets) lists them all. A test whose name starts with `known_gap_` pins a weakness: when the weakness is fixed, the test fails, and this section must change with it.

The short answer: field encryption keeps secrets out of a stolen database dump or a database backup that does not include `runtime/.env`. It does not protect them from anyone who has that file, the host, or code that runs in the server. It is not suitable as a personal password manager.

### Assets

| Asset | Where | Protection |
| --- | --- | --- |
| Encrypted fields | Entity fields with `encrypt="true"` (46 in `runtime/` here: OAuth refresh and access tokens, client secrets, mail and SMTP passwords, private keys and their passwords, API keys, bearer tokens, shared secrets, auth JSON) | AES-128-CBC under `entity_ds_crypt_pass` ([Field encryption](#field-encryption)) |
| Generated secrets | `runtime/.env`: `entity_ds_crypt_pass`, `session_cookie_key`, `ssh_host_key`, the cluster TLS key, `git_lfs_token_key` | Mode `0600` on Unix ([Secrets](#secrets)) |
| Session key | `session_cookie_key`; other signing keys derive from it as HMAC-SHA256 of a label (`derived_cookie_key`, `crates/moqui-core/src/user/session.rs:2955`) | Kept only in `.env` and process memory |
| Password and login-key hashes | `UserAccount.currentPassword`, `UserLoginKey` | argon2id for passwords; SHA-256 for login keys ([Login lockout and passwords](Attack-Surface-Web.md#login-lockout-and-passwords)) |
| Client device keys | Desktop and terminal clients | OS key store, else `FileKeyStore` files mode `0600` (`crates/moqui-desktop/src/client/keystore.rs:101`, `crates/moqui-client/src/keys.rs:48`) |

### Attackers

| Attacker | What is protected | What is not protected | Tests |
| --- | --- | --- | --- |
| Stolen database dump or database backup, without `.env` | Encrypted fields are ciphertext; the stored text and its bytes do not hold the plaintext. Password hashes are argon2id | Which secrets are equal, and which share their first 16 bytes. Login-key hashes are fast SHA-256, but a login key is random and expires in 144 hours. Fields of an install with an empty `crypt-pass` are plaintext | `a_stored_value_does_not_hold_its_plaintext`, `try_insert_seals_the_password_before_sql`, `hash_password_is_an_argon2id_phc_string`, `hash_login_key_stays_sha256_hex`, `known_gap_equal_plaintexts_give_equal_ciphertext`, `known_gap_a_shared_16_byte_prefix_gives_a_shared_first_block`, `known_gap_an_empty_crypt_pass_turns_field_encryption_off` |
| Stolen host disk or host backup, with `.env` | Password hashes | Every encrypted field: the key is in `.env` beside the data (`runtime/db/embedded` is on the same disk). One pass guess costs 10 PBKDF2 iterations. With `session_cookie_key` and the session table, a live session's cookie can be forged | `known_gap_the_key_is_aes_128_from_10_pbkdf2_iterations_and_a_fixed_salt`, `only_the_same_pass_salt_and_iterations_decrypt` |
| Database write access, no key (tamper) | A random change to a stored value usually fails at padding or UTF-8 | A change is not detected: with no MAC, a writer sets the first block of a value to text of their choice, or cuts a value to its tail. A writer stores a value sealed under the public default pass, and the server reads it as valid | `known_gap_a_changed_ciphertext_is_not_detected`, `known_gap_a_value_sealed_under_the_public_default_pass_is_read_as_valid` |
| Host root, or a reader of process memory | Nothing | Every secret: the pass, the session key and decrypted values are in memory, and no buffer is zeroed (no `zeroize` or `secrecy` in `crates/`) | None: there is no control to test |
| Administrator of the instance | Admin services treat a secret as write-only: a read answers only whether it is set, and the audit row of a change holds no value. REST and screen JSON omit password fields | The entity facade decrypts for any caller with read access, so code that runs in the server (a service, a script, an ETL expression) reads plaintext. `ADMIN_ADV` can open a terminal on the host | `AdminConnectionServices` "an endpoint change without the secret clears it …" (`runtime/components/base/framework/test/AdminConnectionServices.spec.js`), `SecurityRest` "password fields omitted" ([Proof-Tests](Proof-Tests.md#catalog)) |
| Compromised client device | The device key is in the OS key store when there is one, else in a file only its user can read | Anyone who runs as that user reads the key and signs in as the device | `a_file_key_is_the_owners_alone_and_can_be_forgotten` (`crates/moqui-client/src/keys_tests.rs`), `the_variable_chooses_and_a_development_run_keeps_its_keys_in_a_file` (`keystore.rs`) |
| Network attacker | TLS to the client (rustls) and to an external database (`entity_ds_sslmode`). The session cookie is HttpOnly, SameSite=Lax, Secure over https, and a bad MAC is no session | Plain HTTP when no TLS listener or proxy is in front | `a_bad_mac_is_an_unknown_session`, `SecurityHeaders` (cookie flags) |

### Controls that hold

- Field encryption with the wrong pass, salt or iteration count does not give the plaintext (`wrong_or_empty_pass_fails`, `only_the_same_pass_salt_and_iterations_decrypt`).
- An empty pass is refused by `encrypt` and `decrypt`, and a pass that is not printable ASCII is refused (`crates/moqui-entity/src/entity/crypto.rs:104`; `wrong_or_empty_pass_fails`, `a_pass_phrase_that_is_not_ascii_is_an_error`).
- `CryptConfig`'s `Debug` does not show the pass (`crypto.rs:50`; `debug_leaves_out_the_pass`).
- Every bound value of an encrypted field in a query is encrypted before SQL (`every_bound_value_of_an_encrypted_field_is_sealed`).
- Startup replaces each secret `CHANGEME` default with 32 CSPRNG bytes, a new value each time, written to `.env` with mode `0600` (`crates/moqui-core/src/context/factory.rs:1823`, `:1974`; `crates/moqui-lib/src/env.rs:147`). A `default-property` is a secret by `is-secret="true"`, or by `pass`, `pw` or `key` in its name (`factory.rs:1954`). Tests: `generated_secrets_are_32_random_bytes_and_secret_names_are_known`, `generated_secret_is_stable_and_unwritable_env_fails`, `update_dotenv_file_replaces_in_place_appends_and_removes`.
- Password hashes are argon2id, and a legacy hash becomes argon2id at the next sign-in (`a_legacy_hash_verifies_and_the_stored_value_becomes_an_argon2_string`).

### Known gaps

| Gap | Where | Test |
| --- | --- | --- |
| Constant IV: encryption is deterministic. Equal plaintexts give equal ciphertext, and a shared 16-byte prefix gives a shared first block. Lookup by value (`CalendarShare.publishToken`) depends on this | `crypto.rs:28`, `:61` | `known_gap_equal_plaintexts_give_equal_ciphertext`, `known_gap_a_shared_16_byte_prefix_gives_a_shared_first_block` |
| No MAC or AEAD: a changed value is not detected, and a writer controls the first block through the `<iv>:<ct>` form | `crypto.rs` `decode_stored` | `known_gap_a_changed_ciphertext_is_not_detected` |
| AES-128 from PBKDF2-HMAC-SHA256 with 10 iterations and a fixed salt from the conf (`20201202`) | `crypto.rs:36`, `MoquiDefaultConf.xml` `entity-facade` | `known_gap_the_key_is_aes_128_from_10_pbkdf2_iterations_and_a_fixed_salt` |
| PBEWithMD5AndDES still decrypts, with the current pass and with the old pass | `crypto.rs:35`, `decrypt-alt` in `MoquiDefaultConf.xml` | `known_gap_des_ciphertext_still_decrypts_through_the_alternates` |
| `entity_ds_crypt_pass_old` keeps the public sentinel and is a `decrypt-alt`, so a value sealed under `MoquiDefaultPassword:CHANGEME` is read as valid | `factory.rs:1959`, `MoquiDefaultConf.xml` | `known_gap_a_value_sealed_under_the_public_default_pass_is_read_as_valid` |
| An empty `crypt-pass` turns field encryption off with no error | `crates/moqui-entity/src/entity/facade.rs:478` | `known_gap_an_empty_crypt_pass_turns_field_encryption_off` |
| The key is in `runtime/.env`, on the same host and often in the same backup as the database | [Secrets](#secrets) | None: deployment |
| One key for all fields, with no key id or version in the stored value; changing the pass needs the `_old` alternate and a rewrite of every row | `crypto.rs:61` | `known_gap_equal_plaintexts_give_equal_ciphertext` shows the format has no key id |
| `session_cookie_key` has no `_old` key: a new key ends every session and every value signed with a derived key | `session.rs:2963` | `known_gap_a_new_session_cookie_key_ends_every_session_cookie` |
| No memory zeroing of passes, keys or decrypted values | `crates/` | None: absence |
| The server sees every plaintext: there is no client-side (zero-knowledge) encryption | Entity facade | None: absence |

### Inspiration

Two designs show what MR Moqui does not do yet.

**HashiCorp Vault** (server-side secrets):

- A barrier encrypts all storage with AES-256-GCM. A random nonce on each write gives different ciphertext for equal values, and the GCM tag detects a change.
- The keyring is versioned. `vault operator rotate` adds a key term; old terms still decrypt, and new writes use the new term.
- A root key encrypts the keyring. Shamir's secret sharing (k of n unseal keys) or a KMS or HSM (auto-unseal) seals the root key. The key is not on the disk beside the data.
- Leases and dynamic secrets: Vault makes a short-lived credential on request and revokes it when its lease ends.
- Transit encrypts as a service: the application never holds the key. A rewrap moves ciphertext to the newest key version without exposing the plaintext. Convergent encryption is an opt-in for lookup by value.
- Response wrapping hands a secret over through a single-use token.
- Audit devices log each request with secret values as HMAC-SHA256. A request fails when no audit device can write it (fail closed).
- `mlock` keeps secret memory out of swap.

**Bitwarden** (client-side, zero-knowledge):

- The client derives a master key from the master password with PBKDF2-SHA256 (600,000 iterations by default) or Argon2id.
- HKDF stretches the master key into an encryption key and a MAC key.
- A random user key encrypts the vault. The stretched master key wraps the user key, so a password change rewraps one key, not every item.
- Each field is AES-256-CBC with HMAC-SHA256, encrypt-then-MAC, with a random IV.
- An organization key is shared by encrypting it to each member's RSA public key.
- The client sends only a hash of the master key, and the server hashes it again with a random salt. The server never sees the master password or a plaintext item.

**What MR Moqui can take from them** (also in [Remaining-Work](../../plans/Remaining-Work.md#sessions-notifications-and-security)):

| Improvement | From | Closes |
| --- | --- | --- |
| A v2 stored format: AEAD (AES-256-GCM or XChaCha20-Poly1305), a random nonce, and a key id, such as `v2:<kid>:<nonce>:<ct+tag>`. Decrypt keeps the Java forms, decrypt-only | Vault barrier, Bitwarden EncString | Determinism, no MAC, AES-128, weak PBE |
| A blind index for a lookup-by-value field: an HMAC of the value under its own key, in a separate column | Vault convergent Transit | Lookup by value without deterministic ciphertext |
| Envelope keys: random data keys wrapped by a root key from a KMS, OpenBao or Vault Transit, a TPM, or `systemd-creds`, never from `.env` | Vault seal | The key beside the data |
| A versioned keyring with rotation, and a rewrap job that rewrites old rows to the newest key | Vault rotate and rewrap | One unversioned key; `session_cookie_key` with no `_old` |
| Skip a `decrypt-alt` whose pass is the public `_old` sentinel; PBEWithMD5AndDES decrypt-only, and whether to drop it decided after the first release | Vault keyring terms | DES; planted values under the public pass |
| Refuse to start when an entity has `encrypt="true"` fields and `crypt-pass` is empty | Vault seal (no storage until unsealed) | Encryption off with no error |
| `zeroize` (and `secrecy` types) for passes, keys and decrypted values; `mlock` where available | Vault `mlock` | Secrets left in memory |
| An optional client-side vault for personal secrets: a user key wrapped by a key from the user's password (Argon2id), items AEAD-encrypted in the client | Bitwarden | The server sees plaintext |

## Open questions

These are gaps in this pass. A line that was read is stated above and is not repeated here.

- Unix embedded Postgres: this tree sets an empty `listen_addresses` and password auth, and it does not set the socket-directory mode or `pg_hba.conf`. Whether another local user can open `.s.PGSQL.5432` is whatever `postgresql_embedded` 0.21 defaults to. The superuser password is in the cluster `pgpass` file.
