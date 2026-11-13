# Sealed Secrets

Status 2026-11-12: plan; nothing built. The threat model this plan answers is in
[Attack-Surface, Threat model: stored secrets](../docs/security/Attack-Surface.md#threat-model-stored-secrets). Its `known_gap_*` tests
(`crates/moqui-entity/src/entity/crypto_tests.rs`) pin today's weaknesses. Each phase below that closes a gap flips its
test and updates the threat model.

The goal: MR Moqui keeps its keys the way HashiCorp Vault does. No usable key is on disk. A server starts **sealed**.
It opens its listeners only after it is unsealed, and its keys then exist only in memory. A stolen disk, `.env`,
database dump or backup gives ciphertext only. The plan also closes the attack classes that hit Vault and Bitwarden in
practice: sign-in bypass at the edges, a feature that turns into code execution, tampering and side channels. An
operator can see at any time who has access to which secret, can rotate every key, and has a runbook for a breach.

Sections: [1 deployment zones](#1-deployment-zones) · [2 what is there now](#2-what-is-there-now) ·
[3 design](#3-design) ([3.9 less surface](#39-less-attack-surface), [3.10 access](#310-who-has-access-to-what),
[3.11 keys](#311-key-inventory-and-rotation), [3.12 incidents](#312-incident-response)) · [4 phases](#4-phases) · [5 tests](#5-tests) · [6 decisions](#6-decisions) ·
[7 not in scope](#7-not-in-scope) · [8 inspiration](#8-inspiration)

## 1. Deployment zones

MR Moqui runs as two parts. They have different attackers, so they get different keys.

| Part | Components | What it does | Zone |
|---|---|---|---|
| **BOSS** | `mr-boss` + `mr-mantle` | The back office. It holds the business database, jobs, mail and media. | **Inner.** The system of record. Often a local node behind a firewall ([Edge-Node](archive/Edge-Node.md)). |
| **PASS** | `mr-pass` | The public sites: Forum, Account, Members, Sites. It holds no business data. | **Outer.** Faces the internet. Often on the edge node. |

Rules for the two zones:

1. **Each zone has its own keyring.** A PASS or edge node never holds a BOSS data key. A PASS node that is fully
   compromised gives no key that decrypts a BOSS row, backup or field.
2. **The outer zone holds the fewest secrets.** It holds only what it needs to answer while BOSS is away: its own
   session key, TLS keys, the link key, its PASS client credential, and the secrets that BOSS replicates to it (a chat
   bridge's webhook secret, a trunk's `authJson`). The [Edge-Node §4a](archive/Edge-Node.md#4a-entity-access-audit) audit
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
  KMS key ARN, share threshold. It also holds the **bootstrap secrets**, wrapped by the root key: the secrets needed
  before the database is open (`entity_ds_password`, `content_ds_password`, `entity_ds_c1_password` and the embedded
  Postgres password, which today is a plaintext `pgpass` file, `crates/moqui-drivers/src/embedded.rs` about line 751).
  The file is useless alone.
- **In the database:** `moqui.security.SecretKeyring` holds each data key version, wrapped by the root key, with its
  id, purpose, state (`active`, `decrypt-only`, `retired`) and dates.
- **Stored secrets:** `moqui.security.SecretValue` holds each other secret that is now a property (the list in §2),
  in the `v2` format below, by name. A generated secret is made into this table, never into `.env`.
- **Purpose keys** come from HKDF with a fixed label, so one leaked purpose key does not give another.
- **Start order:** unseal, unwrap the root key, unwrap the bootstrap secrets, open the database, unwrap the keyring,
  read `SecretValue`, then open the listeners.
- **Secret handles.** Code gets a secret through a handle from the keyring, never through a property. Today a secret
  property is copied into the process properties and the factory state (`context/factory.rs` about lines 339 and 387),
  and `session.rs` decodes its key into a plain `Vec<u8>` (about line 2963). These copies go in S4 and S5.

### 3.2 The `v2` field format

```
v2:<keyId>:<nonce, base64url>:<ciphertext+tag, base64url>
```

- AES-256-GCM, a random 96-bit nonce for each value. See [§6](#6-decisions) for the choice against XChaCha20-Poly1305.
- **Associated data** = entity name, field name and primary key, in a fixed encoding (length-prefixed parts, the key
  fields in definition order). Then a ciphertext copied into another row or field does not decrypt. A read must know
  the source entity, field and full primary key. A find that selects only some fields, and a view entity that has an
  encrypted field as an alias (`entity/find.rs` about line 964, `field_rw.rs` about line 262), selects the source
  primary key as hidden columns. A distinct or aggregate select of an encrypted field is refused, as it gives nothing
  useful. This also stops an attacker with database write access from moving a secret into a field they can
  read.
- **Blind index** for a field found by value. A separate column holds `<keyId>:` and HMAC-SHA256 under that version
  of the `field-index` key. Such a field is marked `encrypt="true" index="blind"`. `CalendarShare.publishToken` is the
  first one. A lookup computes the index under each key version that is not `retired`, and then decrypts each match and
  compares it with the value asked for. The rewrap job writes the ciphertext and the index of a row in one
  transaction. A `field-index` version is retired only when no row uses it.
- **No fallback.** Today a value that fails to decrypt and has no colon and is not hex is read as plaintext
  (`entity/field_rw.rs` about line 191), so a database writer can put a chosen plaintext in place of a ciphertext. A
  field accepts only `v2` once its entity holds no old-format value: from the start on a new database, and after the
  rewrap on one loaded with Moqui Java data (its old-format count is zero). A value that is not `v2`, or that fails
  authentication, is an error, never plaintext and never an old format.
- Old formats (`:`, `<iv>:`, hex, DES) are **decrypt-only**, for data written by Moqui Java. Before the first release
  there are no MR Moqui installs to move: `v2` replaces the write path and a clean reload is fine. The rewrap job moves
  loaded Java data to `v2`, and is also part of key rotation (S2, S14); a count of the rows still in an old format is
  shown in System. Whether to drop the Java formats is decided after the first release.

### 3.3 Sealed start

States: **sealed → unsealing → unsealed**, and back to sealed on `moqui operator seal`.

| While sealed | While unsealed |
|---|---|
| No HTTP, SSH, SIP, XMPP, TURN or gRPC listener. No jobs. | All as configured |
| The operator socket only: `runtime/moqui.sock`, mode 0600, owned by the service user | Same socket, plus the System screens |
| systemd `READY=1` and `STATUS=sealed` once the socket is open | `STATUS=unsealed` |

Unseal methods (`seal` element in the conf):

| Method | Unattended reboot | For |
|---|---|---|
| `tpm` through `systemd-creds` (TPM2, bound to PCRs) | Yes | **A home or office server. The default for BOSS.** |
| `kms` (AWS, GCP, Azure) | Yes | Cloud hosts. The default for an edge node in the cloud. |
| `transit` (OpenBao or Vault Transit) | Yes | Where a Vault runs already |
| `passphrase` (Argon2id, m=256 MiB, t=3) | No | A laptop, or the highest care |
| `shamir` (k of n shares) | No | Separation of duties |
| `dev` (key in `.env`, as today) | Yes | Development only. Refused when `instance_purpose` is `production`. |

The unit's `TimeoutStartSec=300` (`systemd/moqui.service`) does not end a server that waits for a passphrase or
shares, since `READY=1` is sent when the socket is open. Health checks and load balancers use the HTTP listener, which
opens only after unseal.

**Recovery shares** (Shamir, 3 of 5 by default) are made at init for every method. They are printed once and are never
stored. They unseal when the TPM, KMS or passphrase is lost, and they authorize `rekey` and a new root token.

`rekey` keeps the root key and wraps it again. `rotate-root` makes a new root key: it writes the new wrapped
keyring and bootstrap secrets beside the old ones, switches `seal.json` in one atomic rename, then deletes the old
ones. A crash before the rename leaves the old root in use. In a cluster every member must be unsealed and acknowledge
the new root before the switch. After a process compromise, recovery runs `rotate-root` before it rotates data keys,
since an attacker may hold the old root key.

**Backup of the seal.** A database backup is useless without the matching `seal.json`. `export-seal` writes a
recovery bundle: `seal.json` (it holds only wrapped values), the database identity and the keyring id. The bundle is
made at init, after each `rekey`, `rotate-root` and bootstrap-secret change, and goes with the offline backups. A
restore on a clean host needs the bundle and the recovery shares; the TPM of the old host is not needed.

Commands, on the socket or in the SSH admin shell:

```
moqui operator init      # make the root key, seal.json and recovery shares
moqui operator unseal    # give a passphrase or a share; auto methods unseal at start
moqui operator seal      # drop every key from memory now
moqui operator status
moqui operator rotate    # add a data key version; the rewrap job follows
moqui operator rekey     # change the unseal method or the shares (needs recovery shares)
moqui operator rotate-root   # new root key: rewrap the keyring and the bootstrap secrets (needs recovery shares)
moqui operator export-seal   # write the recovery bundle (§3.3, Backup of the seal)
moqui operator secret put|list|rm <name>   # external credentials; values are never printed
```

### 3.4 A cluster and an edge link

- Each BOSS member unseals on its own, with its own copy of `seal.json` and the same root key (an auto method, or
  shares). No member sends keys to another: the cluster's TLS key is itself a secret, so a sealed member could not
  open the cluster transport (`crates/moqui-host/src/cluster_edge/mod.rs`, about line 525). The cluster's TLS key and
  certificate are bootstrap secrets in `seal.json`. `session_cookie_key`'s "must match a live member" check
  (`cluster/mod.rs`) becomes a check of the keyring id.
- **Rotation across members.** A key version moves `pending → active → decrypt-only → retired`. A new version becomes
  `active` only after every live member has loaded it and acknowledged; a member that has not acknowledged may not
  write encrypted fields. A member that comes back with an older keyring generation reloads before it writes. The
  rewrap job updates a row only if its ciphertext is unchanged since it read it (compare-and-swap), so it never
  overwrites a concurrent update. A version is retired only after the final scan, run once no member can write with
  it, finds no row.
- An edge node has its own keyring and seal. The link carries no key of either side. A replicated secret crosses the
  link inside the link's TLS, and the edge node encrypts it under its own `field` key at once.

### 3.5 Memory

- Key material is held in `secrecy::SecretBox` or `zeroize::Zeroizing`. `Debug` stays redacted, as `CryptConfig`'s is.
- The keyring pages are `mlock`ed. The process sets `PR_SET_DUMPABLE=0` and `RLIMIT_CORE=0`, and the unit sets
  `LimitCORE=0`.
- **The entity cache holds ciphertext** for `encrypt="true"` fields. Today a find decrypts before it caches
  (`entity/find.rs` about line 994), so the plaintext stays in the cache and later readers get it with no decrypt.
  With ciphertext in the cache, each read decrypts and each read is in the decrypt log (§3.10).
- `seal` refuses new work, waits for running requests, streams, jobs and background tasks to end (with a time limit),
  then overwrites every key and secret handle in memory, clears the caches, closes the listeners and stops the jobs.
  Decrypted values in entity values that work still holds (`entity/value.rs`, a cloneable `Arc<FieldMap>`) cannot be
  found and cleared. So when work is still running at the time limit, `seal` ends the process; systemd starts it again
  sealed. Only a clean drain gives a sealed process that stays running.

### 3.6 Sign-in core

One function decides whether an identity may sign in. Every path calls it after the path checks its own credential.

```
path verifies its credential ─► canonical identity ─► one check:
   disabled? end date? lockout (shared count)? ipAllowed? factor required for this path?
   credential scope allows this route?  ─► session or refusal (one shared refusal text)
```

- **Canonical identity:** Unicode NFKC, trim, then case fold, applied once before lookup and before the lockout count.
  The same form on every path. A unique `canonicalUsername` column holds it, set on create and on rename. Before the
  core uses it, a check lists existing accounts whose names give the same form. Start refuses the new lookup until an
  administrator renames or merges them.
- **One lockout count** for the identity and for the client address, across HTTP, SSH, SIP, XMPP and gRPC.
- **The request never chooses the verifier.** Issuers, keys, endpoints and audiences come from configuration. This
  already holds for SSO (the endpoint-change rule, `PublicOnly`), and the core checks it for every path.
- **Credential scope.** A credential that is not a password plus a factor (app password, login key, device cookie, SSH
  key, PASS client) has a scope. A privileged area refuses it. `check#RecentAuth` already does this for sensitive
  actions; the core makes it the default.
- **Constant time** for every comparison of a secret, a MAC, a hash or a token (`subtle`, or `Mac::verify_slice`).

### 3.7 Code and data apart

- The server writes only under a data directory: `/var/lib/moqui` (database, uploads, tmp, logs, exports, Git, DAV,
  `seal.json`). The binary, `components/base`, components and conf are read-only at runtime.
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

### 3.9 Less attack surface

What is not built, not bound or not reachable cannot be attacked. Apply these in this order:

1. **Build less.** A node gets only the cargo features its role needs ([Packages](../docs/Build-Test.md#packages)). A
   BOSS node without phones, chat or mail hosting leaves out `ssh`, `sip`, `xmpp`, `rtc`, `inbound-smtp`, `channels`
   and `devtools`.
2. **Bind less.** Every optional listener stays off until its port is set. This is the default now; a test keeps it so.
3. **Split by role.** Only PASS or the edge node faces the internet. BOSS dials out to it and opens no inbound port
   ([Edge-Node](archive/Edge-Node.md)).
4. **One public port.** A reverse proxy on 443, with `webapp_trusted_proxies` set. The cluster port stays on a private
   network.
5. **Fewer privileged accounts.** An administrator uses a separate admin account with a second factor.
6. **A role report.** `moqui operator surface` lists, for the running node, each feature built in, each listener bound
   (address and port), each enabled integration and each account with a privileged group. An operator compares it with
   what the node's role needs.

### 3.10 Who has access to what

An operator can answer at any time, and for any past date: who can read this secret, and who did?

- **What exists:** `AccessServices.get#GrantReview` (groups, grants, app gates and members as of a date, for print and
  CSV), `get#Capabilities` and the Capabilities screen, `IdentityServices.get#AccountCredentials` (each user's keys,
  app passwords, SSH keys and sessions, metadata only), and the audit rows of grants and secret changes.
- **A secret access map.** For each secret in the inventory (§3.11, and each `encrypt="true"` field): the entity that
  holds it, the permissions and artifact groups that can read that entity, the services that decrypt it, and then the
  users, groups, jobs, agents, PASS clients and edge nodes that hold those permissions. As of a date, from the same
  dated grants `get#GrantReview` reads. Today `get#GrantReview` leaves out sealed permissions and shows app gates as
  they are now, not as of the date (`AccessCapabilities.js` about lines 460 and 510). S13 adds a dated history of the
  authorization rows (artifact groups, authz, filters, app gates, sealed permissions) so the report can show a past
  date. Where a service decides access in code, the report names the service and says the result is not
  reconstructed.
- **A use log for other secrets.** Each use of a secret handle (§3.1) writes one line: caller, purpose and key
  version. A use that repeats often, such as a session check, is counted per minute and caller, not logged one by one.
- **A decrypt log.** Each decrypt of an encrypted field, from the database or from the cache (§3.5), writes one line: user, session or job, service, entity, field
  and primary key, and the key version. The line never holds the value. A line of a secret's own value is HMAC-SHA256
  under the `audit` purpose key, as Vault's audit devices write, so a known value can be matched without being stored.
  The log is kept as long as the audit log, not seven days. A decrypt of a secret marked `audit="required"` fails when
  its log line cannot be written, as Vault fails closed. Other decrypts go on, and a gap row records the time and count
  that were not logged, so the report shows the gap.
- **The Access report screen** in System joins the map, the decrypt log and the credentials, with "as of", a filter by
  secret, by user and by machine identity, and a CSV export. It needs `ADMIN_ACCESS_VIEW`; a secret's value is never
  shown.

### 3.11 Key inventory and rotation

Every key and credential is one row of an inventory: what it protects, its zone, its current and previous versions,
its age, its rotation policy, and what a rotation breaks.

| Key | Rotation now | What a rotation breaks now | Target |
|---|---|---|---|
| Field key (`entity_ds_crypt_pass`) | Set `_old` by hand, re-save rows | Nothing if done right | Keyring version; rewrap job |
| The same key, in the audit log | Not rotated | Old audit values of encrypted fields no longer decrypt | `EntityAuditLog.oldValueText` and `newValueText` hold ciphertext of audited encrypted fields (`entity/field_rw.rs`, `audit_text`) with no `encrypt="true"` on the column. The rewrap job and the old-format count include them, with the source entity, field and key as associated data |
| Root key | `rekey` only rewraps it | – | `rotate-root` (§3.3) |
| `session_cookie_key` and its derived keys | Change it | Every session and every derived signature | Current and previous (S2) |
| `git_lfs_token_key` | `_previous` | Nothing | Already the model |
| `ssh_host_key` | Replace it | Clients warn of a changed host key | Publish the new key first; keep both for a period |
| Cluster TLS key and certificate | Replace on every member | Members must match | Keyring id check (§3.4) |
| `edge_link_key` | Replace it | Every local node pairs again | A second link key announced over the link before the change |
| `mail_dkim_key` | New selector in DNS first | DKIM fails if DNS lags | Two selectors; switch after DNS answers |
| `web_push_vapid_key` | Replace it | Browser subscriptions stop | Clients subscribe again on their next visit |
| `chat_store_key_secret` | Replace it | Each browser device store is encrypted under a passphrase derived from it (`service/chat_keys.rs`); a device cannot open its store | The derived passphrase carries the key version; a device opens its store with the previous version and re-encrypts it under the current one |
| `field-index` key | None | Lookups by value miss old rows | Versioned index (§3.2) |
| External credentials (OAuth, SMTP, API keys, FCM) | At the provider, then Admin's write-only field | That integration until updated | A reminder from the policy age |

- **Every key gets versions**, as `git_lfs_token_key` has a previous one. New use takes the current version. A new
  signature carries its key id; today `sign_session` does not (`user/session.rs` about line 2932). A version that
  verifies is kept until the longest lifetime of what it signed has passed, so two rotations in a row do not end a
  valid session. A signature with no key id is checked against the version that was current before the first key id.
- **Two kinds of key retire in two ways.** A *signing* key (session, LFS token, MCP state, PASS handle): checks accept
  the previous version until it expires, or not at all after an emergency. An *encryption* key (field, field index,
  stored secrets, chat store): the previous version stays `decrypt-only` until the rewrap job has moved every row and
  device to the current one. No rotation, emergency or not, deletes an encryption key that data still needs. Only
  `retired` keys are deleted, and only after the next backup.
- `moqui operator rotate <name>|all` rotates in one step and starts the rewrap job where there is one.
- **The Keys screen** in System shows the inventory: version, age, last rotation, policy, and a warning past the
  policy. Rotation needs a sealed permission and a recent sign-in, and writes an audit row with no value.
- **Emergency rotation.** `moqui operator rotate all --emergency` rotates every generated key, refuses the previous
  signing keys at once, makes the previous encryption keys `decrypt-only` and starts the rewrap, ends every session, and lists the external credentials to change at their providers.

### 3.12 Incident response

**Before an incident** (a runbook in `docs/security/Incident-Response.md`):

1. Who decides, who acts, and how to reach them outside MR Moqui.
2. Offline copies: backups, recovery shares, the key inventory and the runbook.
3. Alerts to a channel outside the server, so a compromised server cannot silence them: failed sign-ins of one
   account across protocols, a new privileged member, a deploy, a seal or unseal, a rotation, a decrypt-log rate above
   normal.

**During an incident:**

1. **Contain.** `moqui operator seal`, or stop the service. `revoke#AllAccess` ends every session and login key and
   pauses jobs and agent triggers (today `revoke#AccountAccess` does this for one account).
2. **Preserve.** Take a disk snapshot and copy the logs and the decrypt log off the host before anything changes.
3. **Scope.** The access report and the decrypt log (§3.10) for the window: which secrets were read, by whom.
4. **Rotate by zone:**
   - BOSS process: `rotate-root`, then every key (emergency rotation), then every external credential at its provider.
   - BOSS disk or backup only: nothing secret, but only for a disk or backup made since S4 keeps every secret out of
     `.env` and the runtime directory. An older snapshot or backup, or a copy of an old `.env`, holds the keys of its
     time: rotate every key and credential it held.
   - PASS or edge node only: the outer zone's keys and the PASS credential. BOSS keys are safe (S6).
5. **Recover** from a known-good binary and backup on a clean host. Do not clean a compromised host.
6. **Notify** the users and the authorities the law names, within its deadline (72 hours under the GDPR).
7. **Learn.** A review, a test for the hole, and a change to the threat model.

## 4. Phases

Each phase is done when its regression tests pass, `./moq test-framework` passes, and the threat model in
Attack-Surface says what changed.

| Phase | Work | Closes | Tests (suite) |
|---|---|---|---|
| **S0** | Threat model and `known_gap_*` tests (in progress). The secret inventory of §2 as a table in Attack-Surface: each secret's zone, kind (generated, external, derived) and where it goes. | – | the `known_gap_*` and control tests (regression) |
| **S1** | `v2` AEAD field format with associated data; keyring table; the field key from the keyring, which still starts from `entity_ds_crypt_pass` until S3; blind index; rewrap job; old-format count in System | Tampering, equal-value leaks, row swap, AES-128. Also, at once: a `decrypt-alt` whose pass is still the `CHANGEME` sentinel is skipped | `known_gap_*` for constant IV and no MAC flip to control tests; a swapped ciphertext fails; a partial select and a view alias of an encrypted field decrypt; once an entity holds no old-format value, a plaintext or an old-format value put in place of a `v2` value is refused through the entity facade; the blind index finds a row under the previous and the current index key during a rewrap; a rewrap leaves no old rows, audit-log values included, survives a restart midway and does not overwrite a concurrent update (regression) |
| **S2** | Purpose keys by HKDF; `session_cookie_key` gets a previous key and a rotation, so sessions survive a rotation; `operator rotate` | No rotation | A session signed by the previous key is accepted until it expires, then refused; a session made before two rotations in a row is still accepted; a session cookie with no key id is read (regression) |
| **S3** | Sealed start; `seal.json`; operator socket and commands; `passphrase`, `shamir`, `tpm` (`systemd-creds`) and `dev` methods; recovery shares; `seal`; `rotate-root`; `export-seal` | Key beside the data | Sealed: no port answers; a wrong share is refused and counted; seal drops keys and closes listeners; a server that stays sealed past `TimeoutStartSec` then unseals and serves; a `rotate-root` stopped before the rename keeps the old root; a database restore with the recovery bundle and shares on a new runtime directory unseals and reads every row (regression). A full TPM reboot on a real host (acceptance, `SealedStart`) |
| **S4** | The generator writes bootstrap secrets to `seal.json` and the others to `SecretValue`, not `.env` (before the first release a clean start; no move of an existing `.env`); external credentials by `operator secret put` and Admin's write-only fields; the embedded Postgres password into `seal.json`, with no `pgpass`; secret handles in place of secret properties | `.env` secrets | A new runtime's `.env` and runtime directory hold no secret, and a cold start with a password-protected database works; a generated secret is never in `.env` (regression) |
| **S5** | Memory hygiene of §3.5; ciphertext in the entity cache | Keys and secrets in dumps, swap and caches | `seal` zeroes the key buffers (unit test on the type); after `seal` the caches hold no plaintext of an encrypted field and an old handle is refused; a `seal` while a request is blocked past the time limit ends the process; the process is not dumpable (regression) |
| **S6** | Zones: an outer keyring for PASS and edge nodes; replicated secrets re-encrypted on arrival; PASS's client credential to mTLS with a pinned key, the header secret kept for REST until PASS moves; a zone check that refuses a BOSS data key on an outer node | Outer node gives inner keys | An edge database dump and its `seal.json` decrypt no BOSS row; a replicated secret on the edge is in the edge's format (regression). Attach and replicate on two runtimes (acceptance, in Edge's suite) |
| **S7** | Sign-in core of §3.6; canonical identity with its column and the collision check; shared lockout; scope; a lint that finds `==` on a secret | Edge sign-in bypass | Canonicalization, two accounts with the same canonical form refused, the shared count and the scope refusal, one test each (regression). The matrix of every sign-in path × wrong, expired, replay, disabled, end date, case and Unicode variant, factor skipped, other organization (acceptance, `SignInPaths`) |
| **S8** | Code and data apart (§3.7); new unit; deploy approval | A feature turns into code execution | No upload, export, log or audit path resolves under a code directory (regression). The packaged server starts and serves under the new unit (acceptance) |
| **S9** | Unsafe production starts (§3.8); an empty `crypt-pass` while an entity has `encrypt="true"` fields is refused on every instance | Unsafe settings | One test for each refusal (regression) |
| **S10** | Outside review of the design and of `crypto.rs`, the keyring and the seal code; publish the result in `docs/security/` | Trust you can check | – |
| **S11** | `kms` and `transit` methods | Root key held on the host | Unseal by each method against a stub (regression) |
| **S12** | Less attack surface (§3.9): `moqui operator surface`; a test that every optional listener is off by default | Unneeded surface | The default conf binds no optional listener; the report names each bound listener (regression) |
| **S13** | Access (§3.10): the secret access map, the dated authorization history, the decrypt log with HMAC'd values, the use log of secret handles, fail-closed `audit="required"`, the Access report screen | Not knowing who can read or did read a secret | A decrypt writes one line with no value, and two users who read one cached row write two lines; a handle use is counted; with the log store down an `audit="required"` decrypt fails and other decrypts leave a gap row; an artifact rule changed with no change of membership shows on its date; the map names a user who gains the permission and drops one who loses it, as of each date; a user without `ADMIN_ACCESS_VIEW` is refused (regression) |
| **S14** | Key inventory and rotation (§3.11): current and previous for every generated key, `operator rotate`, the Keys screen, emergency rotation | Keys that cannot be rotated without an outage | For each signing key: a value signed by the previous version is accepted until it expires; emergency rotation refuses it at once and ends every session. For each encryption key: emergency rotation leaves every row readable; a device store opens after `chat_store_key_secret` rotates; a backup made before a rotation restores and reads (regression) |
| **S15** | Incident response (§3.12): `revoke#AllAccess`, the alerts, the runbook | No way to contain and scope a breach | `revoke#AllAccess` ends every session and key and pauses jobs (regression). A tabletop run of the runbook on a scratch server (acceptance, by hand) |

Order: S1 and S2 make the stored data safe even before sealing. S3 and S4 remove keys from disk. S6 follows S3,
because the outer zone needs its own seal. S7 and S8 do not depend on the others and can run beside them. S12 and S13
need nothing else. S5 follows S4 (secret handles). S13's decrypt log follows S5 (ciphertext in the cache). S14 follows S1 and S2. S15 follows S3 (for `seal`), S13 (to scope) and S14 (to rotate).

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
- Provenance of data: the source of each write, a hash-chained audit log, signed imports. The decrypt log (§3.10)
  records reads of secrets only.

## 8. Inspiration

| From | Idea | Here |
|---|---|---|
| Vault | Barrier: storage is untrusted, AES-256-GCM on everything that crosses it | `v2` format, associated data (S1) |
| Vault | Root key sealed by Shamir shares or a KMS; nothing usable on disk | Sealed start (S3) |
| Vault | Versioned keyring, `rotate`, `rekey`, Transit `rewrap` | Keyring, rewrap job (S1, S2) |
| Vault | `mlock`, seal drops keys | Memory hygiene, `seal` (S3, S5) |
| Vault | Fail closed when the audit log cannot be written | A decrypt of an `audit="required"` secret fails when its line cannot be written (S13) |
| Vault 2020, 2024, 2025 | Bypasses in AWS/GCP, cert and lockout code; an audit device that wrote a plugin | One sign-in core, canonical identity (S7); code and data apart (S8) |
| Bitwarden | The key that encrypts data is wrapped, so a password change re-wraps one key | Root key wraps the keyring; `rekey` re-wraps only the root key; `rotate-root` replaces it |
| Bitwarden | Encrypt-then-MAC on every field | AEAD (S1) |
| Bitwarden, LastPass breach | Old accounts kept weak KDF settings after the default changed | The rewrap job writes each row in the current format and key version (S1, S2) |
| Bitwarden | The server holds no key | The outer zone holds no inner key (S6) |
