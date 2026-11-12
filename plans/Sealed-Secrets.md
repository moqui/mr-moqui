# Sealed Secrets

Status 2026-11-12: plan; nothing built. The threat model this plan answers is in
[Attack-Surface, Threat model: stored secrets](../docs/security/Attack-Surface.md#threat-model-stored-secrets). Its `known_gap_*` tests
(`crates/moqui-entity/src/entity/crypto_tests.rs`) pin today's weaknesses. Each phase below that closes a gap flips its
test and updates the threat model.

The goal: MR Moqui keeps its keys the way HashiCorp Vault does. No usable key is on disk. A server starts **sealed**.
It opens its listeners only after it is unsealed, and its keys then exist only in memory. A stolen disk, `.env`,
database dump or backup gives ciphertext only. The plan also closes the attack classes that hit Vault and Bitwarden in
practice: sign-in bypass at the edges, a feature that turns into code execution, tampering and side channels.

Sections: [1 deployment zones](#1-deployment-zones) · [2 what is there now](#2-what-is-there-now) ·
[3 design](#3-design) · [4 phases](#4-phases) · [5 tests](#5-tests) · [6 decisions](#6-decisions) ·
[7 not in scope](#7-not-in-scope) · [8 inspiration](#8-inspiration)

## 1. Deployment zones

MR Moqui runs as two parts. They have different attackers, so they get different keys.

| Part | Components | What it does | Zone |
|---|---|---|---|
| **BOSS** | `mr-boss` + `mr-mantle` | The back office. It holds the business database, jobs, mail and media. | **Inner.** The system of record. Often a local node behind a firewall ([Edge-Node](Edge-Node.md)). |
| **PASS** | `mr-pass` | The public sites: Forum, Account, Members, Sites. It holds no business data. | **Outer.** Faces the internet. Often on the edge node. |

Rules for the two zones:

1. **Each zone has its own keyring.** A PASS or edge node never holds a BOSS data key. A PASS node that is fully
   compromised gives no key that decrypts a BOSS row, backup or field.
2. **The outer zone holds the fewest secrets.** It holds only what it needs to answer while BOSS is away: its own
   session key, TLS keys, the link key, its PASS client credential, and the secrets that BOSS replicates to it (a chat
   bridge's webhook secret, a trunk's `authJson`). The [Edge-Node §4a](Edge-Node.md#4a-entity-access-audit) audit
   lists them. Each replicated secret is re-encrypted under the outer keyring when it arrives. It is never stored in
   BOSS's format.
3. **Credentials go inward and are scoped.** PASS reaches BOSS's authority (`moqui.pass.v1.PassAuthority`) with a
   client credential, and each call carries the visitor's delegation. A stolen PASS credential acts only as PASS, only
   for visitors who sign in through it, and only for the operations the authority routes.
4. **The inner zone unseals with a key the outer zone never sees.** The BOSS unseal method (TPM, passphrase, shares)
   never crosses the link.
5. **An all-in-one node is both zones in one process.** It has one keyring. The threat model says so, and the
   [heavy lock-down](../docs/security/Attack-Surface.md#heavy-lock-down) advice applies.

What a compromise of each zone gives:

| Compromised | The attacker gets | The attacker does not get |
|---|---|---|
| PASS / edge node, disk only | Ciphertext of the outer keyring's secrets, visitor session rows | Any BOSS key or business row |
| PASS / edge node, running process | Outer secrets in memory, the PASS client credential, live visitor sessions | BOSS keys. A BOSS call only with a visitor's delegation. |
| BOSS, disk or backup only | Business rows that are not encrypted fields (use disk encryption for these), ciphertext of the encrypted fields | Keys, encrypted fields, external credentials |
| BOSS, running process (root) | Everything. This is the same limit Vault has. | – |

## 2. What is there now

| Area | Now | Where |
|---|---|---|
| Field encryption | AES-128-CBC, a constant IV, no MAC, PBKDF2 with 10 iterations and a fixed salt; DES decrypt kept | `crates/moqui-entity/src/entity/crypto.rs` |
| Public alternate pass | `entity_ds_crypt_pass_old` keeps the public sentinel `MoquiDefaultPassword:CHANGEME` and is a `decrypt-alt`, so a value a database writer seals under it is read as valid | `MoquiDefaultConf.xml` (`decrypt-alt`), `factory.rs` (`leave_sentinel`) |
| Empty pass | An empty `crypt-pass` turns field encryption off with no error: `encrypt="true"` fields are stored as plaintext | `crates/moqui-entity/src/entity/facade.rs` (about line 478) |
| One key | `entity_ds_crypt_pass` encrypts every `encrypt="true"` field (46 uses) | [Field encryption](../docs/security/Attack-Surface.md#field-encryption) |
| Lookup by value | `CalendarShare.publishToken` is found by its ciphertext, so it needs deterministic encryption | same |
| Secrets on disk | Each secret `default-property` still at `CHANGEME` is generated (32 CSPRNG bytes) and written to `runtime/.env`, mode 0600 | `crates/moqui-core/src/context/factory.rs` (`set_conf_property`, about line 1020, and the boot path about line 1823) |
| Secret properties | `email_oauth_*_client_secret`, `ssh_host_key`, `git_lfs_token_key(_previous)`, `mail_dkim_key`, `chat_store_key_secret`, `turn_auth_secret`, `channels_media_key`, `xmpp_upload_secret`, `xmpp_scram_decoy_key`, `entity_ds_password`, `entity_ds_crypt_pass(_old)`, `content_ds_password`, `session_cookie_key`, `web_push_vapid_key`, `fcm_service_account`, `cluster_tls_cert`, `cluster_tls_key`, `edge_node_key`, `edge_link_key`, `entity_ds_c1_password`, `elasticsearch_password`, `llm_openai_api_key`, `mcp_request_state_key` | `MoquiDefaultConf.xml`, `is-secret="true"` |
| Derived keys | Some keys are already derived from `session_cookie_key` as HMAC-SHA256 of a label | `user/session.rs` (about line 2949), `pass_authority.rs` (about line 383) |
| Rotation | Only `git_lfs_token_key` has a previous key. `session_cookie_key` has none, so rotating it signs everyone out. | conf |
| Memory | No `zeroize` or `secrecy`, no `mlock`, core dumps not disabled | – |
| Sandbox | `ProtectSystem=strict`, but `ReadWritePaths=/opt/moqui` makes the binary, components and conf writable by the server | `systemd/moqui.service` |
| Sign-in paths | Password, TOTP, SSO (OIDC, OAuth, SAML), API key, login key, Basic, app passwords, SSH keys, SIP digest, XMPP, device cookie, PASS client secret, `/rest/sm` HMAC, token routes. They compare usernames in different ways. Example: an app password compares ASCII case-insensitively (`user/login.rs`, about line 652). | `crates/moqui-core/src/user/`, `service/sso.rs`, `service/user_keys.rs` |
| Backups | `pg_dump` output, without `.env`. A backup alone gives no key. | `crates/moqui-drivers/src/backup.rs` |

## 3. Design

### 3.1 Key hierarchy

```
Unseal method (TPM / passphrase / Shamir / KMS / Transit)
        │ unwraps
        ▼
Root key: 256-bit random, made once at init, in memory only
        │ unwraps (AES-256-GCM)
        ▼
Keyring: versioned data keys, stored wrapped in the database
        │ HKDF-SHA256(key, label)
        ▼
Purpose keys: "field", "field-index", "session", "lfs-token", "mcp-state", "pass-handle", ...
```

- **On disk:** `runtime/seal.json` holds the wrapped root key and the unseal method's public metadata: TPM policy,
  KMS key ARN, share threshold. The file is useless alone.
- **In the database:** `moqui.security.SecretKeyring` holds each data key version, wrapped by the root key, with its
  id, purpose, state (`active`, `decrypt-only`, `retired`) and dates.
- **Stored secrets:** `moqui.security.SecretValue` holds each secret that is now a property (the list in §2), in the
  `v2` format below, by name. A generated secret is made into this table, never into `.env`.
- **Purpose keys** come from HKDF with a fixed label, so one leaked purpose key does not give another.

### 3.2 The `v2` field format

```
v2:<keyId>:<nonce, base64url>:<ciphertext+tag, base64url>
```

- AES-256-GCM, a random 96-bit nonce for each value. See [§6](#6-decisions) for the choice against XChaCha20-Poly1305.
- **Associated data** = entity name, field name and primary key. Then a ciphertext copied into another row or field
  does not decrypt. This also stops an attacker with database write access from moving a secret into a field they can
  read.
- **Blind index** for a field found by value. A separate column holds HMAC-SHA256 under the `field-index` key. Such a
  field is marked `encrypt="true" index="blind"`. `CalendarShare.publishToken` is the first one.
- Old formats (`:`, `<iv>:`, hex, DES) are **decrypt-only**. A rewrap job moves each row to `v2`. A count of the rows
  still in an old format is shown in System, and goes to zero before DES decrypt is removed.

### 3.3 Sealed start

States: **sealed → unsealing → unsealed**, and back to sealed on `moqui operator seal`.

| While sealed | While unsealed |
|---|---|
| No HTTP, SSH, SIP, XMPP, TURN or gRPC listener. No jobs. | All as configured |
| The operator socket only: `runtime/moqui.sock`, mode 0600, owned by the service user | Same socket, plus the System screens |
| systemd `STATUS=sealed`, `READY=1` not sent | `READY=1` |

Unseal methods (`seal` element in the conf):

| Method | Unattended reboot | For |
|---|---|---|
| `tpm` through `systemd-creds` (TPM2, bound to PCRs) | Yes | **A home or office server. The default for BOSS.** |
| `kms` (AWS, GCP, Azure) | Yes | Cloud hosts. The default for an edge node in the cloud. |
| `transit` (OpenBao or Vault Transit) | Yes | Where a Vault runs already |
| `passphrase` (Argon2id, m=256 MiB, t=3) | No | A laptop, or the highest care |
| `shamir` (k of n shares) | No | Separation of duties |
| `dev` (key in `.env`, as today) | Yes | Development only. Refused when `instance_purpose` is `production`. |

**Recovery shares** (Shamir, 3 of 5 by default) are made at init for every method. They are printed once and are never
stored. They unseal when the TPM, KMS or passphrase is lost, and they authorize `rekey` and a new root token.

Commands, on the socket or in the SSH admin shell:

```
moqui operator init      # make the root key, seal.json and recovery shares
moqui operator unseal    # give a passphrase or a share; auto methods unseal at start
moqui operator seal      # drop every key from memory now
moqui operator status
moqui operator rotate    # add a data key version; the rewrap job follows
moqui operator rekey     # change the unseal method or the shares (needs recovery shares)
moqui operator secret put|list|rm <name>   # external credentials; values are never printed
moqui operator migrate   # move .env secrets into SecretValue (§4, S4)
```

### 3.4 A cluster and an edge link

- Each BOSS member unseals with the same auto method, or an unsealed member sends the keyring to a joining member over
  cluster mTLS after the member proves its pinned certificate. `session_cookie_key`'s "must match a live member" check
  (`cluster/mod.rs`) becomes a check of the keyring id.
- An edge node has its own keyring and seal. The link carries no key of either side. A replicated secret crosses the
  link inside the link's TLS, and the edge node encrypts it under its own `field` key at once.

### 3.5 Memory

- Key material is held in `secrecy::SecretBox` or `zeroize::Zeroizing`. `Debug` stays redacted, as `CryptConfig`'s is.
- The keyring pages are `mlock`ed. The process sets `PR_SET_DUMPABLE=0` and `RLIMIT_CORE=0`, and the unit sets
  `LimitCORE=0`.
- `seal` overwrites every key in memory, closes the listeners and stops the jobs.

### 3.6 Sign-in core

One function decides whether an identity may sign in. Every path calls it after the path checks its own credential.

```
path verifies its credential ─► canonical identity ─► one check:
   disabled? end date? lockout (shared count)? ipAllowed? factor required for this path?
   credential scope allows this route?  ─► session or refusal (one shared refusal text)
```

- **Canonical identity:** Unicode NFKC, trim, then case fold, applied once before lookup and before the lockout count.
  The same form on every path.
- **One lockout count** for the identity and for the client address, across HTTP, SSH, SIP, XMPP and gRPC.
- **The request never chooses the verifier.** Issuers, keys, endpoints and audiences come from configuration. This
  already holds for SSO (the endpoint-change rule, `PublicOnly`), and the core checks it for every path.
- **Credential scope.** A credential that is not a password plus a factor (app password, login key, device cookie, SSH
  key, PASS client) has a scope. A privileged area refuses it. `check#RecentAuth` already does this for sensitive
  actions; the core makes it the default.
- **Constant time** for every comparison of a secret, a MAC, a hash or a token (`subtle`, or `Mac::verify_slice`).

### 3.7 Code and data apart

- The server writes only under a data directory: `/var/lib/moqui` (database, uploads, tmp, logs, exports, Git, DAV,
  `seal.json`). The binary, `base-component`, components and conf are read-only at runtime.
- The unit sets `ReadWritePaths=/var/lib/moqui` in place of `/opt/moqui`, and adds `CapabilityBoundingSet=`,
  `LockPersonality=yes`, `RestrictSUIDSGID=yes`, `SystemCallFilter=@system-service` and `LimitCORE=0`. Check headless
  Chromium (the `pdf` feature) under each setting before you add it.
- Component deploy writes to a deploy directory under the data directory. It loads a component from there only after a
  second administrator approves it, and records the approval and the component's digest.
- No setting that an administrator can change at runtime names a path the server writes to. Paths come from the conf
  file only.

### 3.8 Unsafe production starts are refused

On every instance, start refuses an empty `crypt-pass` while an entity has `encrypt="true"` fields. When
`instance_purpose` is `production`, start also refuses, and names the reason, for: the `dev` seal,
`webapp_allow_origins=*`, a secret still at `CHANGEME` that the generator cannot replace, the tarpits off, a public bind
with no TLS and no believed proxy, and `create#InitialAdminAccount` reachable from other than loopback.

## 4. Phases

Each phase is done when its regression tests pass, `./moq test-framework` passes, and the threat model in
Attack-Surface says what changed.

| Phase | Work | Closes | Tests (suite) |
|---|---|---|---|
| **S0** | Threat model and `known_gap_*` tests (in progress). The secret inventory of §2 as a table in Attack-Surface: each secret's zone, kind (generated, external, derived) and where it goes. | – | the `known_gap_*` and control tests (regression) |
| **S1** | `v2` AEAD field format with associated data; keyring table; the field key from the keyring, which still starts from `entity_ds_crypt_pass` until S3; blind index; rewrap job; old-format count in System | Tampering, equal-value leaks, row swap, AES-128. Also, at once: a `decrypt-alt` whose pass is still the `CHANGEME` sentinel is skipped | `known_gap_*` for constant IV and no MAC flip to control tests; a swapped ciphertext fails; the blind index finds the row; a rewrap leaves no old rows (regression) |
| **S2** | Purpose keys by HKDF; `session_cookie_key` gets a previous key and a rotation, so sessions survive a rotation; `operator rotate` | No rotation | A session signed by the previous key is accepted until it expires, then refused (regression) |
| **S3** | Sealed start; `seal.json`; operator socket and commands; `passphrase`, `shamir`, `tpm` (`systemd-creds`) and `dev` methods; recovery shares; `seal` | Key beside the data | Sealed: no port answers; a wrong share is refused and counted; seal drops keys and closes listeners (regression). A full TPM reboot on a real host (acceptance, `SealedStart`) |
| **S4** | `operator migrate`: secrets from `.env` into `SecretValue`; generator writes there; external credentials by `operator secret put` and Admin's write-only fields; the embedded Postgres password generated into the keyring | `.env` secrets | After migrate `.env` holds no secret and the server starts; a generated secret is never in `.env` (regression) |
| **S5** | Memory hygiene of §3.5 | Keys in dumps and swap | `seal` zeroes the key buffers (unit test on the type); the process is not dumpable (regression) |
| **S6** | Zones: an outer keyring for PASS and edge nodes; replicated secrets re-encrypted on arrival; PASS's client credential to mTLS with a pinned key, the header secret kept for REST until PASS moves; a zone check that refuses a BOSS data key on an outer node | Outer node gives inner keys | An edge database dump and its `seal.json` decrypt no BOSS row; a replicated secret on the edge is in the edge's format (regression). Attach and replicate on two runtimes (acceptance, in Edge's suite) |
| **S7** | Sign-in core of §3.6; canonical identity; shared lockout; scope; a lint that finds `==` on a secret | Edge sign-in bypass | Canonicalization, the shared count and the scope refusal, one test each (regression). The matrix of every sign-in path × wrong, expired, replay, disabled, end date, case and Unicode variant, factor skipped, other organization (acceptance, `SignInPaths`) |
| **S8** | Code and data apart (§3.7); new unit; deploy approval | A feature turns into code execution | No upload, export, log or audit path resolves under a code directory (regression). The packaged server starts and serves under the new unit (acceptance) |
| **S9** | Unsafe production starts (§3.8); an empty `crypt-pass` while an entity has `encrypt="true"` fields is refused on every instance | Unsafe settings | One test for each refusal (regression) |
| **S10** | Outside review of the design and of `crypto.rs`, the keyring and the seal code; publish the result in `docs/security/` | Trust you can check | – |
| **S11** | `kms` and `transit` methods; remove DES and the `:` and hex formats once the count is zero for one release | Legacy cipher | The old formats are refused (regression) |

Order: S1 and S2 make the stored data safe even before sealing. S3 and S4 remove keys from disk. S6 follows S3,
because the outer zone needs its own seal. S7 and S8 do not depend on the others and can run beside them.

## 5. Tests

- **Regression** (`cargo test` for the crate, and framework specs where a service or screen is involved): one test for
  each rule a phase adds, under a second. The `known_gap_*` tests are flipped, not deleted, when a gap closes: the test
  keeps its subject and asserts the control.
- **Acceptance** (`./moq accept framework`): `SealedStart` (a real reboot with the TPM method), `SignInPaths` (the
  matrix of §4 S7) and the edge attach with replicated secrets. These run only when asked or when changed.
- **Proof catalog:** each test is a row in [Proof-Tests, Stored secrets](../docs/security/Proof-Tests.md).

## 6. Decisions

| Question | Choice | Why |
|---|---|---|
| AEAD | AES-256-GCM, random 96-bit nonce | Hardware support everywhere, the `aes-gcm` crate is audited. Under one key the nonce limit is about 2^32 values. Rotation (S2) keeps each key well under it. XChaCha20-Poly1305 is the fallback if a platform has no AES instructions. |
| Encrypt the whole database? | No. Encrypted fields plus disk encryption (LUKS) | Postgres has no transparent encryption that MR Moqui controls. The keyring protects the secrets, and LUKS protects the rest at rest. |
| Default unseal for BOSS | `tpm` through `systemd-creds` | Unattended reboot without a key on disk. Recovery shares cover a lost TPM. |
| Where the operator talks while sealed | A Unix socket and the SSH admin shell, never HTTP | While sealed, nothing faces the network. |
| Client-side (zero-knowledge) vault for personal secrets | Not here | It is a new product (Bitwarden's model), not a fix. A separate proposal if wanted. |

## 7. Not in scope

- Root on a running, unsealed BOSS process. No design protects against it. Vault states the same limit.
- Business rows that are not encrypted fields. Use disk encryption.
- A personal password manager. MR Moqui is not one, and this plan does not make it one.

## 8. Inspiration

| From | Idea | Here |
|---|---|---|
| Vault | Barrier: storage is untrusted, AES-256-GCM on everything that crosses it | `v2` format, associated data (S1) |
| Vault | Root key sealed by Shamir shares or a KMS; nothing usable on disk | Sealed start (S3) |
| Vault | Versioned keyring, `rotate`, `rekey`, Transit `rewrap` | Keyring, rewrap job (S1, S2) |
| Vault | `mlock`, seal drops keys | Memory hygiene, `seal` (S3, S5) |
| Vault | Fail closed when the audit log cannot be written | A later phase may refuse privileged actions when the audit log cannot be written |
| Vault 2020, 2024, 2025 | Bypasses in AWS/GCP, cert and lockout code; an audit device that wrote a plugin | One sign-in core, canonical identity (S7); code and data apart (S8) |
| Bitwarden | The key that encrypts data is wrapped, so a password change re-wraps one key | Root key wraps the keyring; `rekey` re-wraps only the root key |
| Bitwarden | Encrypt-then-MAC on every field | AEAD (S1) |
| Bitwarden, LastPass breach | Old accounts kept weak KDF settings after the default changed | Count the rows in old formats and migrate them (S1, S11) |
| Bitwarden | The server holds no key | The outer zone holds no inner key (S6) |
