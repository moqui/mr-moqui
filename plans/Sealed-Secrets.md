# Sealed Secrets

Status 2026-11-19: S3, S4 and S11 are being built with [Vault-Instances](Vault-Instances.md); the rest is a plan.
Before: 2026-11-14, plan; nothing built. Reviewed 2026-11-15 against the code in the security review: the facts of §1,
§2 and §3.11 are corrected, the design changed in §3.1 to §3.8, §3.10 and §3.12 (each change is a row of
[§6](#6-decisions)), and what is left to decide is in [Open points](#open-points-from-the-security-review). The threat
model this plan answers is in
[Attack-Surface, Threat model: stored secrets](../docs/security/Attack-Surface-Data.md#threat-model-stored-secrets).
Its `known_gap_*` tests (`crypto_tests.rs` and `tests.rs` in `crates/moqui-entity/src/entity/`, and
`crates/moqui-core/src/user/session_tests.rs`) pin today's weaknesses. Each phase below that closes a gap flips its
test and updates the threat model.

The goal: MR Moqui keeps its keys the way HashiCorp Vault does. No usable key is on disk. A server starts **sealed**.
It opens its listeners only after it is unsealed, and its keys then exist only in memory. A stolen disk, `.env`,
database dump or backup gives ciphertext only. The plan also closes the attack classes that hit Vault and Bitwarden in
practice: sign-in bypass at the edges, a feature that turns into code execution, tampering and side channels. An
operator can see at any time who has access to which secret, can rotate every key, and has a runbook for a breach.

This plan keeps the server's own keys. [Vault-Instances](Vault-Instances.md) keeps the other secrets:
- the secrets people hold, in personal vaults that only a client opens;
- the secrets of the instances a server manages, in operational vaults that are used on the server under leases.

It is also the deployment side of this plan. Its manager provisions MR Moqui instances that start sealed, unseals them
by `transit` (§3.3), files their recovery shares in their holders' personal vaults, and puts and rotates their secrets
over a manager link.

Sections: [1 deployment zones](#1-deployment-zones) · [2 what is there now](#2-what-is-there-now) ·
[3 design](#3-design) ([3.9 less surface](#39-less-attack-surface), [3.10 access](#310-who-has-access-to-what),
[3.11 keys](#311-key-inventory-and-rotation), [3.12 incidents](#312-incident-response)) · [4 phases](#4-phases) · [5 tests](#5-tests) · [6 decisions](#6-decisions) ·
[open points](#open-points-from-the-security-review) · [7 not in scope](#7-not-in-scope) ·
[8 inspiration](#8-inspiration)

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
   session key, its SSH host key, its ACME account key and TLS keys, the TURN secret, the DKIM key when it is the mail
   server, the link key, its PASS client credential, and the secrets that BOSS replicates to it (today only a trunk's
   `authJson`; a chat bridge sends its webhook public key, not its `configJson`). The
   [Edge-Node §4a](archive/Edge-Node.md#4a-entity-access-audit) audit lists them. Each replicated secret is
   re-encrypted under the outer keyring when it arrives. It is never stored in BOSS's format. An edge node already
   stores a replica as one encrypted value under its own field key (`EdgeReplicaValue.fieldsJson`).
3. **Credentials go inward and are scoped.** PASS reaches BOSS's authority (`moqui.pass.v1.PassAuthority`) with a
   client credential, and each call carries the visitor's delegation. A stolen PASS credential acts only as PASS, only
   for visitors who sign in through it, and only for the operations the authority routes.
4. **The inner zone unseals with a key the outer zone never sees.** The BOSS unseal method (TPM, passphrase, shares)
   never crosses the edge link in a form the edge node can read. A managed instance on `transit` sends its wrapped
   root key to its manager over the manager link (§3.3), which an edge node may carry as a passthrough.
5. **An all-in-one node is both zones in one process.** It has one keyring. The threat model says so, and the
   [heavy lock-down](../docs/security/Attack-Surface.md#heavy-lock-down) advice applies.
6. **A manager is in the inner zone** ([Vault-Instances §4.9](Vault-Instances.md#49-where-the-manager-runs)). It holds
   a transit key for each instance it manages, and their credentials in operational vaults. An outer instance it
   manages gets no inner key from it: its transit key opens only its own root key. At each unseal by `transit` the
   manager sees that instance's root key, and it receives the instance's recovery bundle (§3.3);
   [§3.12](#312-incident-response) says what a compromised manager gets.

What a compromise of each zone gives:

| Compromised | The attacker gets | The attacker does not get |
|---|---|---|
| PASS / edge node, disk only | Ciphertext of the outer keyring's secrets, visitor session rows | Any BOSS key or business row |
| PASS / edge node, running process | Outer secrets in memory, the PASS client credential, live visitor sessions, each visitor password and second-factor code typed while it is compromised | BOSS keys. A BOSS call only with a visitor's delegation. |
| BOSS, disk or backup only | Business rows that are not encrypted fields (use disk encryption for these), ciphertext of the encrypted fields. Until S4 and S5 also the TLS key files in `runtime/cert` and the decrypted fields of an entity snapshot or export file left on the disk (§2) | Keys, encrypted fields, external credentials |
| BOSS, running process (root) | Everything. This is the same limit Vault has. | – |
| A managed instance, disk with its link key | Its root key from its manager, while the unseal policy is automatic and the manager has not revoked it ([Vault-Instances §4.5](Vault-Instances.md#45-sealed-start-of-a-managed-instance)) | Anything under the approval policy, or after a revoke |
| A manager, running process | Its own keys. Each managed instance's root key at that instance's next unseal by `transit`, and with a backup of that instance every row of it. The items of open leases and of machine access | Vault items with no lease or machine access |

## 2. What is there now

| Area | Now | Where |
|---|---|---|
| Field encryption | AES-128-CBC, a constant IV on every write (a stored value that carries its own IV is read with it), no MAC, PBKDF2 with 10 iterations and a fixed salt; DES decrypt kept | `crates/moqui-entity/src/entity/crypto.rs` |
| Public alternate pass | `entity-facade@crypt-pass-old` keeps the public sentinel `MoquiDefaultPassword:CHANGEME` and is a `decrypt-alt`, so a value a database writer seals under it is read as valid | `MoquiDefaultConf.xml` (`decrypt-alt`), `factory.rs` (`leave_sentinel`) |
| Empty pass | An empty `crypt-pass` turns field encryption off with no error: `encrypt="true"` fields are stored as plaintext, and so is their audit copy | `crates/moqui-entity/src/entity/facade.rs` (about line 478), `field_rw.rs` (`audit_text`, about line 397) |
| One key | `entity-facade@crypt-pass` encrypts every `encrypt="true"` field: 47 in the framework, 12 in mr-mantle, 5 in dev-aide. They include business fields (card numbers, PINs, government ids), seven fields that are also audited, a TLS private key (`AcmeCertificate.privateKeyPem`) and the edge replica rows, which hold other entities' fields | [Field encryption](../docs/security/Attack-Surface-Data.md#field-encryption) |
| Lookup by value | `CalendarShare.publishToken` and `PartyIdentification.idValue` are found by their ciphertext (equals, and `in` for `idValue`), so they need deterministic encryption. Every bound value of an encrypted field is encrypted, whatever the operator | same; `entity/field_rw.rs` (`encrypt_condition`, about line 204) |
| Secrets on disk | Each conf attribute the schema marks a generated secret, still at `CHANGEME`, is generated (32 CSPRNG bytes) and written to `runtime/.env` under its environment name, mode 0600 (plans/archive/Conf-Paths.md). An attribute is secret by the conf schema or by its name. Pairing, the edge setup and the first DKIM use also write secrets to `.env` at runtime (`conf_env::keep_in_env`) | `crates/moqui-core/src/context/conf_env.rs` (`generate_secrets`, `keep_in_env`), `conf_schema.rs` (`attribute_secret`); `moqui-host/src/edge_link/hook.rs`, `moqui-core/src/edge/setup.rs`, `moqui-mail/src/auth.rs` |
| Secret attributes | Marked by the conf schema: `acme@account-key`, `email-client/oauth-provider[id]@client-secret`, `ssh-server@host-key`, `git-server@lfs-token-key` and `@lfs-token-key-previous`, `mail-delivery/dkim@key`, `chat@store-key-secret`, `media-server/turn-server@auth-secret`, `channels@media-key`, `xmpp-server/upload@secret`, `xmpp-server@scram-decoy-key`, a datasource's `password`, `entity-facade@crypt-pass` and `@crypt-pass-old`, `webapp-list/webapp[webroot]/session-config@cookie-key`, `push/web@vapid-key`, `push/fcm@service-account`, `cluster-member@tls-cert` and `@tls-key`, `edge/node@key`, `edge@link-key`, `edge/front-node@key`, `search-facade/cluster@password`, the LLM endpoints' `api-key`, `mcp@request-state-key`, `pass-authority/client[id]@secret`. Secret by name: `component-deploy@attestation-secret`, mr-pass's `pass@client-secret`. Read by code from a process property or environment variable: any an LLM endpoint's `api-key-env` names, the one a `RecoveryAdapter.secretProperty` names. A script reads the conf only with `ec.factory.getConfValue`, which gives no secret, and `System.getProperty` refuses every conf environment name | `conf_schema.rs` (`CATALOG`, `secret_name`); `script/host/globals.rs`; `context/identity.rs` (`script_refused_property`) |
| Derived keys | Some keys are already derived from `webapp-list/webapp[webroot]/session-config@cookie-key` as HMAC-SHA256 of a label: the PASS session handle key, and the MCP `requestState` key when `llm-facade/mcp@request-state-key` is not set (with neither it is a random key of the process). The ACME account key is derived from the `acme@account-key` seed, and each pairing's TURN secret from `media-server/turn-server@auth-secret` | `user/session.rs` (about line 3050), `pass_authority.rs` (about line 391), `moqui-llm/src/mcp/state.rs` (about line 139), `host/acme/order.rs`, `host/edge_link/accept.rs` |
| Rotation | Only `git-server@lfs-token-key` has a previous key. `webapp-list/webapp[webroot]/session-config@cookie-key` has none, so rotating it signs everyone out. | conf |
| Memory | No `zeroize`, `secrecy` or `subtle` as a workspace dependency, no `mlock`, core dumps not disabled | `Cargo.toml` |
| Sandbox | `ProtectSystem=strict`, but `ReadWritePaths=/opt/moqui` makes the binary, components and conf writable by the server. `EnvironmentFile=-/etc/moqui/environment` is a second place an operator puts secrets. `Restart=on-failure`, `TimeoutStopSec=60` | `systemd/moqui.service` |
| Sign-in paths | 43 paths in eleven classes, listed in [§2.1](#21-sign-in-paths). They compare usernames in three ways and read the lock in three ways, and some skip the end date or `ipAllowed`. A SAML flow is refused (`service/sso.rs`, about line 226) | `crates/moqui-core/src/user/`, `service/sso.rs`, `service/user_keys.rs`, and the files §2.1 names |
| Backups | `pg_dump` or `pg_basebackup` archives, without `.env`, with an optional mirror off the host. A backup alone gives no key. The tools get the superuser password in their environment (`PGPASSWORD`). An entity snapshot (`runtime/db/snapshot/*.zip`) is not a backup: it holds decrypted fields | `crates/moqui-drivers/src/backup.rs` (about line 638), `embedded.rs` (about line 1080); `framework/service/org/moqui/impl/EntityServices.xml` (about line 282) |
| Instance credentials | `InstanceHost.password` and `tlsClientKey`, `InstanceImage.password`, `DatabaseHost.adminPassword`, `InstanceImageTypeEnv.envValue` and `AppInstanceEnv.envValue` are `encrypt="true"` fields under the one field key. Any find decrypts them, and the env values reach containers as environment variables | `runtime/components/base/framework/entity/ServerEntities.xml`; [Vault-Instances §2](Vault-Instances.md#2-what-is-there-now) |
| Key files | TLS private keys in `runtime/cert` (`<name>/privkey.pem`, which ACME writes in clear from its encrypted row, and an operator's `<domain>.key`); a cluster member's own edge link key in `runtime/db/edge_member_key`; the embedded superuser password in a `pgpass` file; the QZ Tray signing key `qz-private-key.pem` | `moqui-host/src/acme/store.rs` (about line 429), `tls.rs`, `edge_link/member_key.rs` (about line 39); `moqui-drivers/src/embedded.rs` (about line 769); `moqui-core/src/web/mod.rs` (about line 1566) |
| Plaintext copies | A find decrypts at row read, so decrypted fields are in the entity cache and the transaction cache. They leave through Tools › Data › Export, entity snapshots, generic entity REST (except fields on a name list), EntitySync's remote `get#EntitySyncData`, and whatever a caller passes in a job's stored parameters. A CI job's secrets reach its container as environment variables | `entity/find.rs` (about line 994), `entity/cache/tx.rs` (about line 1042), `entity/data_writer.rs`, `entity/query_rest.rs` (about line 228), `EntitySyncServices.xml`, `service/jobs.rs`, `moqui-git/src/ci/trust.rs` (about line 281) |
| Hashed credentials | Passwords and their history; login, API, device and gateway keys; app passwords and their SCRAM keys; app SSO codes; shared-device credentials; PASS registration, delegation and hand-off tokens; call links and guests; share links; CI job, runner and pool tokens; ingest keys; chat bridge, upload, SQL run and pairing tokens; voicemail PINs. All are hashes in rows, under no key, so a database dump gives each to an offline guesser. `HttpSession.sessionToken`, `RecoveryAction.leaseToken` and a DAV `lockToken` are stored in clear | `SecurityEntities.xml`, `PublicAccountEntities.xml`, `CallEntities.xml`, `ResourceEntities.xml`, `GitCiEntities.xml`, `MonitorEntities.xml`, `ChatEntities.xml`, `XmppEntities.xml`, `DatabaseEntities.xml`, `EdgeEntities.xml`, `VoiceEntities.xml`, `ServerEntities.xml` |
| Re-encrypt service | `rewrite#EntityEncryptedFields` reads every encrypted field and writes it again under the current pass. It needs no sign-in (`anonymous-all`; `rewrite#EntityEncryptedFieldsAll` is `authenticate="false"`) | `framework/service/org/moqui/impl/EntityServices.xml` (about lines 321 and 350) |
| Component deploy | A deploy from git needs the sealed `COMPONENT_DEPLOY` permission, and in production `component-deploy@enabled`. An attested deploy has no person on the target: its gate is a passed run and an HMAC under a secret both ends hold (`component-deploy@attestation-secret` on the target, `GitCiEnvironment.targetToken` on the source). Components are deployed into `runtime/components/addon` | `MoquiDefaultConf.xml` (about line 17), `moqui-git/src/ci/attest.rs`, `moqui-devkit/src/deploy/git.rs` |
| Paths set at runtime | `GitRepository.localPath` (a row; an absolute path is used as given), a backup's `backupDir` parameter and the Export screen's `path` name where the server writes. System › Conf (sealed `CONF_EDIT`) writes the conf files | `moqui-git/src/lib.rs` (about line 475), `moqui-drivers/src/backup.rs` (about line 522), `tools/screen/Tools/Data/Export.xml`, `moqui-devkit/src/deploy/conf_edit.rs` |
| Crates | `argon2`, `hkdf` and `aes-gcm` are workspace dependencies. There is no Shamir crate and no Unicode normalization or PRECIS crate | `Cargo.toml` |

### 2.1 Sign-in paths

The paths by class. S7's `SignInPaths` matrix is built from this list.

| Class | Paths | Username | Counts toward the account lock | Second factor |
|---|---|---|---|---|
| Account password | The login form and `/rest/login`; HTTP Basic and the `authUsername` and `authPassword` parameters on any screen or REST request; SSH password and the SSH login dialog; Git over HTTP; DAV when `account-password` is on (off by default); PASS's public account sign-in | Exact | Yes | Asked for (form, SSH dialog, PASS); refused (SSH password, Git, DAV); left signed out (Basic, parameters) |
| Second-factor code | After a password | The session's | A wrong code counts | – |
| Keys | The `api_key` and `login_key` headers; device keys of the desktop and mobile apps; scoped gateway keys; a key as a Git password or a Bearer token; the notification socket and `/termws` by login key; MCP, A2A and the LLM gateway | None: the key's SHA-256 finds the row | No | Skipped |
| App passwords | DAV, SFTP and SSH, Git over HTTP, XMPP PLAIN, MCP, A2A and the LLM gateway (there with no username) | ASCII case-insensitive, trimmed (`user/login.rs`, about line 718) | No, on purpose (`login.rs`, about line 662) | Skipped |
| SSO | OIDC and OAuth; app SSO for installed apps (a code and PKCE). A SAML flow is refused | The linked external id, else exact | No | Skipped |
| SSH keys | A user's public key; Git over SSH; the cluster peer's key (a fixed name, no account) | Exact | No | Skipped |
| XMPP | SCRAM, PLAIN and FAST tokens | Trimmed, exact, then case-insensitive (`chat/XmppServices.xml`, about line 43) | No: counted per source | Skipped |
| SIP digest | A phone's requests | `SipDevice.username`, which is not an account name | No: per source address, never per account (`crates/moqui-sip/src/lib.rs`, about line 26) | None |
| Devices and tokens | The shared-device cookie; PASS delegations and the authority's session handle; Git LFS tokens; call guests on `/callws`; token routes; Monitor ingest keys; CI job, runner and pool tokens | None | No | Skipped or none |
| Machine peers | The PASS client secret (`x-pass-client-secret`); `/rest/sm` by login, HMAC, URL token or none; the edge link (`_edge_`, pinned keys); cluster TLS | A client or remote id | No | – |
| No credential | The first administrator (`create#InitialAdminAccount`, `anonymous-all`, guarded only by "no account yet"); anonymous login; internal run-as from a service or a script | As given | – | – |

What the paths do differently today:

- **The lock.** With `max-failures="3"` and `disable-minutes="5"`, the fourth wrong password locks the account for five
  minutes. Each attempt during the lock starts the five minutes again, and only a sign-in that passes resets the count
  (`user/login.rs`: `gate_account` about line 1303, `still_disabled` about line 1512, the failed-login update about
  line 1736). An attempt during a lock counts on every path that reaches `gate_account`, with a valid key as with a
  wrong password.
- **Disabled for good is not kept.** An account with `disabled` set and no `disabledDateTime` is disabled for good.
  Failed attempts on it still count, and past the limit they set `disabledDateTime`. Five minutes later it signs in
  again.
- **The lock is read three ways.** The core adds `disable-minutes` to `disabledDateTime`. XMPP compares
  `disabledDateTime` itself with now, so a locked account may sign in (`chat/XmppServices.xml`, about line 51). SIP and
  a PASS delegation refuse any account with `disabled` set (`call/SipDevices.js`, about line 276;
  `public/Account.js`, about line 341).
- **The end date** is not checked by XMPP or SIP.
- **`ipAllowed`** is checked only where a client address is bound (`login.rs`, `check_ip_allowed` about line 1454). An
  SSH public key binds none (`moqui-host/src/ssh/server.rs`, about line 969), and other paths with no address skip it
  the same way.
- **Comparisons that are not constant time:** the SIP digest response (`SipDevices.js`, about line 323), the PASS
  client secret, compared as two digests (`Account.js`, about line 35), and the app SSO code challenge
  (`service/sso_app.rs`, about line 511).
- **`/rest/sm`** with `SmatNone` signs a caller in as anonymous with no credential
  (`service/system_sync/message.rs`, about line 394).

## 3. Design

### 3.1 Key hierarchy

```
Unseal method (TPM / passphrase / Shamir / KMS / Transit)
        │ unwraps
        ▼
Root key: 256-bit random, made once at init, in memory only
        │ unwraps (AES-256-GCM)
        ▼
Keyring: versioned data keys, one per purpose, stored wrapped in the database
        │ HKDF-SHA256(key, label)
        ▼
Purpose keys: "field", "field-index", "session", "lfs-token", "mcp-state", "pass-handle", ...
```

- **On disk:** `seal.json`, in the data directory (§3.7), holds the wrapped root key and the unseal method's public
  metadata: TPM policy, KMS key ARN, share threshold. It also holds the **bootstrap secrets**, wrapped by the root
  key: the secrets needed before the database is open (`entity-facade/datasource[transactional]@password`, `entity-facade/datasource[content]@password`,
  `entity-facade/datasource[transactional#clone1]@password` and the embedded Postgres password, which today is a plaintext `pgpass` file,
  `crates/moqui-drivers/src/embedded.rs` about line 769). The file is useless alone. Each cluster member has its own
  `seal.json`: the same root key under that member's own unseal method, the bootstrap secrets every member shares,
  and that member's own edge link key (today the file `runtime/db/edge_member_key`). The embedded Postgres tools
  still get the superuser password in their environment while they run (`PGPASSWORD`).
- **In the database:** `moqui.security.SecretKeyring` holds each data key version, wrapped by the root key, with its
  id, purpose, state (`active`, `decrypt-only`, `retired`) and dates.
- **Stored secrets:** `moqui.security.SecretValue` holds each other secret that is now a property (the list in §2),
  in the `v2` format below, by name. A generated secret is made into this table, never into `.env`.
- **Purpose keys** come from HKDF with a fixed label, so one leaked purpose key does not give another. Each purpose
  has its own data key and its own versions in the keyring, so refusing a signing key in an emergency does not rotate
  the field key.
- **Start order:** unseal, unwrap the root key, unwrap the bootstrap secrets, open the database, unwrap the keyring,
  read `SecretValue`, then open the listeners.
- **Secret handles.** Code gets a secret through a handle from the keyring, never through a property. Today a secret
  property is copied into the process properties and the factory state (`context/factory.rs` about lines 339 and 387),
  and `session.rs` decodes its key into a plain `Vec<u8>` (about line 3058). These copies go in S4 and S5. A script
  gets no secret from `System.getProperty`, which refuses a secret name; today it returns any property (§2). A script
  that checks a secret, as `Account.js` checks a PASS client's, calls a host member that compares and does not return
  it.
- **TLS keys.** The listeners open after the unseal, so they take their keys from memory. An ACME key stays in its
  encrypted row and is not written to `runtime/cert`. An operator's own key goes in by `operator secret put`; a key
  file left on disk is named in the surface report (§3.9).

### 3.2 The `v2` field format

```
v2:<keyId>:<nonce, base64url>:<ciphertext+tag, base64url>
```

- AES-256-GCM, a random 96-bit nonce for each value. See [§6](#6-decisions) for the choice against XChaCha20-Poly1305.
- **Associated data** = entity name, field name and primary key, in a fixed encoding (length-prefixed parts, the key
  fields in definition order). Then a ciphertext copied into another row or field does not decrypt. A read must know
  the source entity, field and full primary key. A find that selects only some fields, and a view entity that has an
  encrypted field as an alias (`entity/find.rs` about line 964, `field_rw.rs` about line 263), selects the source
  primary key as hidden columns. A distinct or aggregate select of an encrypted field is refused, as it gives nothing
  useful. This also stops an attacker with database write access from moving a secret into a field they can
  read.
- **Blind index** for a field found by value. A separate column holds `<keyId>:` and HMAC-SHA256 under that version
  of the `field-index` key. Such a field is marked `encrypt="true" index="blind"`. `CalendarShare.publishToken` and
  `PartyIdentification.idValue` are the first two. A lookup computes the index under each key version that is not
  `retired`, and then decrypts each match and compares it with the value asked for. The rewrap job writes the
  ciphertext and the index of a row in one transaction. A `field-index` version is retired only when no row uses it.
  The HMAC input holds the entity and field name, so equal values in two fields give different indexes. A token is
  random; an `idValue` is a short tax or government id. With the index key such a value can be guessed offline, and
  without it the index still shows which rows hold equal values ([open point](#open-points-from-the-security-review)).
- **No fallback.** Today a value that fails to decrypt and has no colon and is not hex is read as plaintext
  (`entity/field_rw.rs` about line 192), so a database writer can put a chosen plaintext in place of a ciphertext. A
  field accepts only `v2` once its entity holds no old-format value: from the start on a new database, and after the
  rewrap on one loaded with Moqui Java data (its old-format count is zero). A value that is not `v2`, or that fails
  authentication, is an error, never plaintext and never an old format.
- Old formats (`:`, `<iv>:`, hex, DES) are **decrypt-only**, for data written by Moqui Java. Before the first release
  there are no MR Moqui installs to move: `v2` replaces the write path and a clean reload is fine. The rewrap job moves
  loaded Java data to `v2`, and is also part of key rotation (S2, S14); a count of the rows still in an old format is
  shown in System. Whether to drop the Java formats is decided after the first release.

- **Rules for the associated data:**
  - An audit copy (§3.11) has its own domain: a label and the audit row's key. A value copied from the audit log into
    the live row then fails. A ciphertext is never clipped: where the audit text would pass its 4,000 characters
    (`entity/field_rw.rs`, `TEXT_LIMIT`), the audit row holds a marker and the length.
  - An update that changes a primary key re-encrypts every encrypted field of the row. A key change by raw SQL is not
    supported for an entity with encrypted fields.
  - A value read from the cache (§3.5) keeps the entity and key it was read under, since a caller may set new key
    fields on a clone before it reads the field.
  - An edge replica row uses its own primary key (for `EdgeReplicaValue`: `pairedNodeId`, `setName`, `entityName`,
    `pkHash`), not the source row's.
  - A keyring row's purpose, state and generation are in the associated data of its wrap, and `seal.json` holds the
    highest generation. A database writer then cannot bring back a retired key or drop a new one unseen.
- **Conditions.** Only equals and `in` work on an encrypted field, through the blind index. `like`, a range, order-by
  and group-by on one are refused with an error that names the field. Today every bound value is encrypted, whatever
  the operator (§2).
- **The "no fallback" state is a flag,** not a count of rows. It is in the keyring's wrapped metadata, set once for
  an entity and never cleared by data, so a database writer who inserts an old-format row does not bring the
  fallback back.

### 3.3 Sealed start

States: **sealed → unsealing → unsealed**, and back to sealed on `moqui operator seal`.

| While sealed | While unsealed |
|---|---|
| No HTTP, SSH, SIP, XMPP, TURN or gRPC listener. No jobs. | All as configured |
| The operator socket only: `/run/moqui/moqui.sock`, mode 0660, owned by the service user and the operator group | Same socket, plus the SSH admin shell and the System screens |
| systemd `READY=1` and `STATUS=sealed` once the socket is open (`passphrase`, `shamir`). An automatic method sends `READY=1` after its unseal | `STATUS=unsealed` |

Unseal methods (`seal` element in the conf):

| Method | Unattended reboot | For |
|---|---|---|
| `tpm` through `systemd-creds` (TPM2, bound to PCRs) | Yes | **A home or office server. The default for BOSS.** |
| `kms` (AWS, GCP, Azure) | Yes | Cloud hosts. The default for an edge node in the cloud. |
| `transit` (OpenBao or Vault Transit, or the instance's manager) | Yes | Where a Vault runs already. A managed instance, whose manager keeps its transit key ([Vault-Instances §4.5](Vault-Instances.md#45-sealed-start-of-a-managed-instance)) |
| `passphrase` (Argon2id, m=256 MiB, t=3) | No | A laptop, or the highest care |
| `shamir` (k of n shares) | No | Separation of duties |
| `dev` (key in `.env`, as today) | Yes | Development only. Refused when `instance@purpose` is `production`. |

The unit's `TimeoutStartSec=300` (`systemd/moqui.service`) does not end a server that waits for a passphrase or
shares, since `READY=1` is sent when the socket is open. Health checks and load balancers use the HTTP listener, which
opens only after unseal.

**Recovery shares** (Shamir, 3 of 5 by default) are made at init for every method. They are printed once and are never
stored on the server. On an instance with a manager, each share is instead sealed on the instance to one named
holder's personal key and filed in that holder's personal vault
([Vault-Instances §4.5](Vault-Instances.md#45-sealed-start-of-a-managed-instance)). No place then holds two shares, and
the manager sees none. That holds only while the holders' keys the manager sends are the holders' own
([open point](#open-points-from-the-security-review)). A share is entered on the instance's operator socket, never in
a manager's screen. Shares unseal when the TPM, KMS, passphrase or manager is lost, and they authorize `rekey` and
`rotate-root`.

`rekey` keeps the root key and wraps it again. `rotate-root` makes a new root key: it writes the new wrapped
keyring and bootstrap secrets beside the old ones, switches `seal.json` in one atomic rename, then deletes the old
ones. A crash before the rename leaves the old root in use. In a cluster every member must be unsealed and acknowledge
the new root before the switch. After a process compromise, recovery runs `rotate-root` before it rotates data keys,
since an attacker may hold the old root key.

**Backup of the seal.** A database backup is useless without the matching `seal.json`. `export-seal` writes a
recovery bundle: `seal.json` (it holds only wrapped values), the database identity and the keyring id. The bundle is
made at init, after each `rekey`, `rotate-root` and bootstrap-secret change, and goes with the offline backups. A
restore on a clean host needs the bundle and the recovery shares; the TPM of the old host is not needed. A managed
instance sends its bundle to its manager over the link, and the manager keeps it as an operational vault item beside
the instance's backups.

Commands, on the socket, and once unsealed also in the SSH admin shell:

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

**Who may use the socket.** The socket is in `/run/moqui` (the unit's `RuntimeDirectory=`), outside the code and data
directories. It checks its peer's credentials (`SO_PEERCRED`). The service user, and so every child and job of the
server, may run only `status`. Every other command needs root or a member of the operator group. While sealed an
operator reaches it at the console, or through the host's own sshd running `moqui operator`. The SSH admin shell is
part of the server and answers only once it is unsealed.

**Unseal attempts.** One attempt runs at a time. After a wrong passphrase or share the wait before the next one
doubles, and an alert goes out (§3.12).

**The method is fixed at init.** `seal.json` records the unseal method. A conf, `.env` or environment that names
another is ignored and logged, so a writer of those files cannot move an instance to the `dev` seal. Only `rekey`
changes it.

**An automatic unseal that fails** (`tpm`, `kms`, `transit`) leaves the server sealed and ready, with
`STATUS=sealed: auto-unseal failed`, so an operator can unseal it with shares.

**Child processes.** The embedded Postgres and OpenSearch children start after the unseal, when the database opens,
and `seal` stops them.

### 3.4 A cluster and an edge link

- Each BOSS member unseals on its own, with its own `seal.json` and the same root key (an auto method, or
  shares). No member sends keys to another. The cluster transport starts after the database opens
  (`crates/moqui-host/src/lib.rs`, about lines 1847 and 1880), so the cluster's TLS key and certificate are
  `SecretValue` rows that every member reads from the shared database. Today they are properties
  (`crates/moqui-host/src/cluster_edge/mod.rs`, about line 525). `webapp-list/webapp[webroot]/session-config@cookie-key`'s "must match a live member"
  check (`cluster/mod.rs`, about line 958) becomes a check of the keyring id.
- **A member that joins,** or a standby, cannot open a `seal.json` wrapped by another host's TPM. It is unsealed the
  first time with recovery shares, and `rekey` then wraps the root key under its own method. With `kms` or `transit`
  its `seal.json` starts from the same wrapped root key.
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
  With ciphertext in the cache, each read decrypts and each read is in the decrypt log (§3.10). A decrypt happens
  when a field is read from a value, not when its row is read, so a find that never reads the field decrypts
  nothing. A row in the transaction cache is held the same way.
- `seal` refuses new work, waits for running requests, streams, jobs and background tasks to end (with a time limit),
  then overwrites every key and secret handle in memory, clears the caches, closes the listeners and stops the jobs.
  Decrypted values in entity values that work still holds (`entity/value.rs`, a cloneable `Arc<FieldMap>`) cannot be
  found and cleared. So when work is still running at the time limit, `seal` ends the process with a failure status,
  and the unit's `Restart=on-failure` starts it again sealed. Only a clean drain gives a sealed process that stays
  running.

### 3.6 Sign-in core

One function decides whether an identity may sign in. Every path calls it after the path checks its own credential.

```
path verifies its credential ─► canonical identity ─► one check:
   disabled? end date? lockout (shared count)? ipAllowed? factor required for this path?
   credential scope allows this route?  ─► session or refusal (one shared refusal text)
```

- **Canonical identity:** Unicode NFKC, trim, then case fold, applied once before lookup and before the lockout count
  (the form is an [open point](#open-points-from-the-security-review)). The same form on every path. A unique
  `canonicalUsername` column holds it, set on create and on rename, and from S7 on a create or rename whose form
  another account has is refused. Before the core uses it, a check lists existing accounts whose names give the same
  form. The old exact lookup stays until an administrator renames or merges them and the check passes. The server
  starts either way.
- **One lockout count for guessable secrets** (a password, a PIN, a digest of one), for the identity and for the
  client address, across every path that takes one. A lock stops that class of credential for the identity, not the
  account: its sessions, keys and app passwords go on working, a use of one adds nothing to the count, and a success
  with one does not clear the lock. A high-entropy credential (a key, an app password, a token, an SSH key) and the
  SIP and XMPP sign-ins are limited by source and by credential, never by account, as they are today (§2.1). An
  anonymous caller can still keep a named identity's password locked; nothing else of the account stops.
- **The one check fails closed.** An account disabled for good stays disabled: failed attempts never turn it into a
  timed lock. The lock and the end date are read one way on every path. A path that gives no client address is
  refused for an identity that has `ipAllowed` set.
- **Identity kinds.** An identity is an account, a SIP device (`SipDevice.username` is not an account name; its
  checks are its account's) or a machine peer (a PASS client, a remote system, an edge or cluster member). The
  canonical form and the count are per kind.
- **The factor rule is one table** in the core. It starts from today's three answers by path (§2.1): ask, refuse or
  skip.
- **The request never chooses the verifier.** Issuers, keys, endpoints and audiences come from configuration. This
  already holds for SSO (the endpoint-change rule, `PublicOnly`), and the core checks it for every path.
- **Credential scope.** A credential that is not a password plus a factor (app password, login key, device cookie, SSH
  key, PASS client) has a scope. A privileged area refuses it. `check#RecentAuth` already does this for sensitive
  actions; the core makes it the default.
- **Constant time** for every comparison of a secret, a MAC, a hash or a token (`subtle`, or `Mac::verify_slice`). A
  script compares through a host member made for it, since the lint reads only Rust.

### 3.7 Code and data apart

- The server writes only under a data directory: `/var/lib/moqui` (database, search data, uploads, tmp, logs,
  exports, snapshots, backups, Git, DAV, certificates, deploy staging, the Chromium profile, the member's own
  `db/instance_id`, and `seal.json`). The binary, `components/base`, components and conf are read-only at runtime.
- The unit sets `ReadWritePaths=/var/lib/moqui` in place of `/opt/moqui`, and adds `CapabilityBoundingSet=`,
  `LockPersonality=yes`, `RestrictSUIDSGID=yes`, `SystemCallFilter=@system-service`, `LimitCORE=0` and
  `RuntimeDirectory=moqui` (the operator socket, §3.3). Check headless
  Chromium (the `pdf` feature) under each setting before you add it.
- Component deploy writes to a deploy directory under the data directory. It loads a component from there only after a
  second administrator approves it, and records the approval and the component's digest. An attested deploy (§2) has
  no second person on the target; how it meets this rule is an [open point](#open-points-from-the-security-review).
- No setting that an administrator can change at runtime names a path the server writes to. Paths come from the conf
  file only. Today three do (§2): a repository's `localPath`, a backup's `backupDir` and the Export `path`. Each
  becomes a name under a fixed root of the data directory (`git/`, `db/backup/`, `export/`), and an absolute path for
  one comes from the conf only. What the server writes at runtime beside the conf (pairing's URL and pin, the edge
  setup) goes to the data directory, not to `.env`. System › Conf's editor writes the conf files, which this section
  makes read-only ([open point](#open-points-from-the-security-review)).

### 3.8 Unsafe production starts are refused

On every instance, start refuses an empty `crypt-pass` while an entity has `encrypt="true"` fields. When
`instance@purpose` is `production`, which is its default, start also refuses, and names the reason, for:

- the `dev` seal;
- `webapp-list/webapp[webroot]@allow-origins=*`;
- a secret still at `CHANGEME` that the generator cannot replace. A `decrypt-alt` pass is not one:
  `entity-facade@crypt-pass-old` keeps its sentinel on purpose, and S1 skips such an alternate;
- the tarpits off for screens, transitions, services or REST paths (entities and Git repositories ship with theirs
  off);
- a public bind with no TLS and no believed proxy, judged from what is bound and which certificates are loaded, since
  the HTTPS listener stands by while `runtime/cert` is empty.

On a production instance `create#InitialAdminAccount` answers only a loopback caller. That is a check in the service,
not at start: today it is `anonymous-all`, guarded only by "no account yet".

### 3.9 Less attack surface

What is not built, not bound or not reachable cannot be attacked. Apply these in this order:

1. **Build less.** A node gets only the cargo features its role needs ([Packages](../docs/Build-Test.md#packages)). A
   BOSS node without phones, chat or mail hosting leaves out `ssh`, `sip`, `xmpp`, `rtc`, `inbound-smtp`, `channels`
   and `devtools`.
2. **Bind less.** Every optional listener stays off until its port is set. This is the default now; a test keeps it so.
3. **Split by role.** Only PASS or the edge node faces the internet. BOSS dials out to it and opens no inbound port
   ([Edge-Node](archive/Edge-Node.md)).
4. **One public port.** A reverse proxy on 443, with `webapp-list/webapp[webroot]@trusted-proxies` set. The cluster port stays on a private
   network.
5. **Fewer privileged accounts.** An administrator uses a separate admin account with a second factor.
6. **A role report.** `moqui operator surface` lists, for the running node, each feature built in, each listener bound
   (address and port), each enabled integration, each key file on disk (§3.1) and each account with a privileged
   group. An operator compares it with what the node's role needs.

### 3.10 Who has access to what

An operator can answer at any time, and for any past date: who can read this secret, and who did?

- **What exists:** `AccessServices.get#GrantReview` (groups, grants, app gates and members as of a date, for print and
  CSV), `get#Capabilities` and the Capabilities screen, `IdentityServices.get#AccountCredentials` (each user's keys,
  app passwords, SSH keys and sessions, metadata only), and the audit rows of grants and secret changes.
- **A secret access map.** For each secret in the inventory (§3.11, and each `encrypt="true"` field): the entity that
  holds it, the permissions and artifact groups that can read that entity, the services that decrypt it, and then the
  users, groups, jobs, agents, PASS clients and edge nodes that hold those permissions. As of a date, from the same
  dated grants `get#GrantReview` reads. Today `get#GrantReview` leaves out sealed permissions and shows app gates as
  they are now, not as of the date (`AccessCapabilities.js` about lines 460 and 515). S13 adds a dated history of the
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
  that were not logged, so the report shows the gap. A field read on a hot path (a second factor at sign-in, a SIP
  verifier, a push subscription, a FAST token, a mail server's password) is counted per minute and caller, as the
  use log counts. A low-entropy value (a PIN, a card security code) gets no HMAC: a holder of the `audit` key could
  guess it, and equal values would show to anyone.
- **Bulk paths carry no plaintext.** Export, entity snapshots, generic entity REST, EntitySync and the Auto screens
  leave an encrypted field out or carry its ciphertext, which only the same keyring reads. A queued or scheduled
  call's stored parameters never hold a secret.
- **Operational vaults** ([Vault-Instances §3.13](Vault-Instances.md#313-events-and-the-access-report)): their members,
  approvers, leases and machine access join the map, and their `VaultEvent` rows join the decrypt log. The report then
  answers who can approve the use of an environment's secrets, who did, and for which run.
- **The Access report screen** in System joins the map, the decrypt log and the credentials, with "as of", a filter by
  secret, by user and by machine identity, and a CSV export. It needs `ADMIN_ACCESS_VIEW`; a secret's value is never
  shown.

### 3.11 Key inventory and rotation

Every key and credential is one row of an inventory: what it protects, its zone, its current and previous versions,
its age, its rotation policy, and what a rotation breaks.

| Key | Rotation now | What a rotation breaks now | Target |
|---|---|---|---|
| Field key (`entity-facade@crypt-pass`) | Set `_old` by hand, re-save rows | Nothing if done right | Keyring version; rewrap job |
| The same key, in the audit log | Not rotated | Old audit values of encrypted fields no longer decrypt | `EntityAuditLog.oldValueText` and `newValueText` hold ciphertext of audited encrypted fields (`entity/field_rw.rs`, `audit_text`, about line 397) with no `encrypt="true"` on the column. The rewrap job and the old-format count include them, with a label and the audit row's key as associated data (§3.2) |
| Root key | `rekey` only rewraps it | – | `rotate-root` (§3.3) |
| `webapp-list/webapp[webroot]/session-config@cookie-key` and its derived keys | Change it | Every session and every derived signature | Current and previous (S2) |
| `git-server@lfs-token-key` | `_previous` | Nothing | Already the model |
| `ssh-server@host-key` | Replace it | Clients warn of a changed host key | Publish the new key first; keep both for a period |
| Cluster TLS key and certificate | Replace on every member | Members must match | One `SecretValue` that every member reads (§3.4); the previous certificate is accepted until every live member has loaded the new one |
| `edge@link-key` | Replace it | Every local node pairs again | A second link key announced over the link before the change |
| `mail-delivery/dkim@key` | New selector in DNS first | DKIM fails if DNS lags | Two selectors; switch after DNS answers |
| `push/web@vapid-key` | Replace it | Browser subscriptions stop | Clients subscribe again on their next visit |
| `chat@store-key-secret` | Replace it | Each browser device store is encrypted under a passphrase derived from it (`service/chat_keys.rs`); a device cannot open its store | The derived passphrase carries the key version; a device opens its store with the previous version and re-encrypts it under the current one |
| `field-index` key | None | Lookups by value miss old rows | Versioned index (§3.2) |
| External credentials (OAuth, SMTP, API keys, FCM) | At the provider, then Admin's write-only field | That integration until updated | A reminder from the policy age |
| Credentials of managed instances (database, search, registry, host TLS) | Operational vault items (M1). The manager rotates a database user's and a database's or OpenSearch's admin password at the source, then on each instance, then deposits the new revision, the old one kept in the item's history (M5, `rotate#Credential`); a registry token, a host's TLS key and a database host's admin login are due by their age, rotated by hand | That instance, restarted to read it; the old value is put back if it does not come back | Operational vault items, rotated by the manager at the source and then on each instance ([Vault-Instances §4.7](Vault-Instances.md#47-rotation)) |
| A managed instance's transit key, on its manager | Not built | – | `rekey` of the instance onto a new transit key ([Vault-Instances §4.5](Vault-Instances.md#45-sealed-start-of-a-managed-instance)) |
| Manager link keys | Not built | – | A new pairing; pinned both ways, as the edge link's are ([Vault-Instances §4.4](Vault-Instances.md#44-the-manager-link)) |
| `acme@account-key` (a seed; the account key is derived from it) | Replace it | The node registers a new ACME account at its next order | Keyring version |
| TLS keys of the web, SMTP, SIP, XMPP and TURN listeners | ACME renews; an operator replaces a file | Nothing while the chain is valid | Served from the encrypted row or a `SecretValue`; no key file (§3.1) |
| `media-server/turn-server@auth-secret` and each pairing's TURN secret derived from it | Replace it | TURN credentials already handed out stop | Current and previous, for a credential's lifetime |
| `channels@media-key` | Replace it | Media addresses already signed stop | Current and previous, for an address's lifetime |
| `xmpp-server/upload@secret`, `xmpp-server@scram-decoy-key` | Replace it | Upload slots already handed out stop | Current and previous for the upload secret |
| `llm-facade/mcp@request-state-key` | Replace it; unset, it is derived from `webapp-list/webapp[webroot]/session-config@cookie-key` | Open MCP request states fail | The `mcp-state` purpose key; no per-process fallback |
| `edge/node@key`, `edge/front-node@key` and a member's own link key | Pair again | The link, until it is paired again | A second key announced over the link before the change, as for `edge@link-key` |
| Deploy attestation secret (`component-deploy@attestation-secret` on the target, `GitCiEnvironment.targetToken` on the source) | Set both ends by hand | Attested deploys, until both ends match | Current and previous on the target |
| PASS client secret (`pass-authority/client[<id>]@secret` on the authority, in clear; `pass@client-secret` on PASS) | Set both ends by hand | That PASS node's calls, until both ends match | The authority stores a hash, until S6's mTLS replaces the secret |
| Database passwords (`entity-facade/datasource[transactional]@password`, `entity-facade/datasource[content]@password`, `entity-facade/datasource[transactional#clone1]@password`, the embedded superuser's) and `search-facade/cluster@password` | At the database or search server, then the property | The server, until both match | Bootstrap secrets in `seal.json` (the search password a `SecretValue`): change at the source, then `operator secret put`, then a new recovery bundle |
| Password and credential hashes (§2) | – | – | No key. `revoke#AllAccess` (§3.12) ends the credentials |

- **Every key gets versions**, as `git-server@lfs-token-key` has a previous one. New use takes the current version. A new
  signature carries its key id; today `sign_session` does not (`user/session.rs` about line 3027). A version that
  verifies is kept until the longest lifetime of what it signed has passed, so two rotations in a row do not end a
  valid session. A signature with no key id is checked against the version that was current before the first key id.
- **Two kinds of key retire in two ways.** A *signing* key (session, LFS token, MCP state, PASS handle): checks accept
  the previous version until it expires, or not at all after an emergency. An *encryption* key (field, field index,
  stored secrets, chat store): the previous version stays `decrypt-only` until the rewrap job has moved every row and
  device to the current one. No rotation, emergency or not, deletes an encryption key that data still needs. Only
  `retired` keys are deleted, and only after the next backup.
- `moqui operator rotate <name>|all` rotates in one step and starts the rewrap job where there is one.
- **The Keys screen** in System shows the inventory: version, age, last rotation, policy, and a warning past the
  policy. For an encryption key it also shows a count of writes under each version, with a warning well under the
  nonce limit (§6). Rotation needs a sealed permission and a recent sign-in, and writes an audit row with no value.
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

1. **Contain.** `moqui operator seal`, or stop the service. `revoke#AllAccess` ends every session, every login, API and
   device key, app password, SSH key, SIP device, FAST token, shared-device credential, PASS delegation, CI runner and
   job token, ingest key, and share and call link, and pauses jobs and agent triggers (today `revoke#AccountAccess`
   ends sessions, login keys, app passwords, SSH keys, SIP devices and agents for one account).
2. **Preserve.** Take a disk snapshot and copy the logs and the decrypt log off the host before anything changes.
3. **Scope.** The access report and the decrypt log (§3.10) for the window: which secrets were read, by whom.
4. **Rotate by zone:**
   - BOSS process: `rotate-root`, then every key (emergency rotation), then every external credential at its provider.
   - BOSS disk or backup only: nothing secret, but only for a disk or backup made since S4 keeps every secret out of
     `.env` and the runtime directory. An older snapshot or backup, or a copy of an old `.env`, holds the keys of its
     time: rotate every key and credential it held.
   - PASS or edge node only: the outer zone's keys and the PASS credential. BOSS keys are safe (S6).
   - A manager ([Vault-Instances §4.9](Vault-Instances.md#49-where-the-manager-runs)): `rotate-root` on the manager,
     then `rekey` of each managed instance onto a new transit key, then every operational vault item that was leased
     or under machine access in the window, at its source. The vault events name those items. A compromised manager
     process also saw the root key of each instance that unsealed by `transit` in the window, and each recovery
     bundle that arrived. With a backup of such an instance it reads every row, with no lease, so that instance runs
     `rotate-root` and rotates every key too.
5. **Recover** from a known-good binary and backup on a clean host. Do not clean a compromised host.
6. **Notify** the users and the authorities the law names, within its deadline (72 hours under the GDPR).
7. **Learn.** A review, a test for the hole, and a change to the threat model.

## 4. Phases

Each phase is done when its regression tests pass, `./moq test-framework` passes, and the threat model in
[Attack-Surface-Data](../docs/security/Attack-Surface-Data.md#threat-model-stored-secrets) says what changed.

| Phase | Work | Closes | Tests (suite) |
|---|---|---|---|
| **S0** | Threat model and `known_gap_*` tests (in progress). The secret inventory of §2 as a table in `docs/security/Attack-Surface-Data.md`: each secret's zone, kind (generated, external, derived) and where it goes. | – | the `known_gap_*` and control tests (regression) |
| **S1** | `v2` AEAD field format with associated data; keyring table; the field key from the keyring, which still starts from `entity-facade@crypt-pass` until S3; blind index; the associated-data rules, the condition rule and the "no fallback" flag of §3.2; rewrap job, which replaces `rewrite#EntityEncryptedFields` (that service needs a sign-in from then on); old-format count in System | Tampering, equal-value leaks, row swap, AES-128. Also, at once: a `decrypt-alt` whose pass is still the `CHANGEME` sentinel is skipped | `known_gap_*` for constant IV and no MAC flip to control tests; a swapped ciphertext fails; a partial select and a view alias of an encrypted field decrypt; once an entity holds no old-format value, a plaintext or an old-format value put in place of a `v2` value is refused through the entity facade, also after a database writer inserts an old-format row; an audit copy put into the live row fails; a `like` or a range on an encrypted field is refused; a keyring row put back to an earlier state is refused; the blind index finds a row under the previous and the current index key during a rewrap; a rewrap leaves no old rows, audit-log values included, survives a restart midway and does not overwrite a concurrent update (regression) |
| **S2** | Purpose keys by HKDF; `webapp-list/webapp[webroot]/session-config@cookie-key` gets a previous key and a rotation, so sessions survive a rotation; `operator rotate` | No rotation | A session signed by the previous key is accepted until it expires, then refused; a session made before two rotations in a row is still accepted; a session cookie with no key id is read (regression) |
| **S3** | Sealed start; `seal.json`; operator socket with its peer check, and commands; the attempt delay; the method fixed at init; `passphrase`, `shamir`, `tpm` (`systemd-creds`) and `dev` methods; recovery shares; `seal`; `rotate-root`; `export-seal` | Key beside the data | Sealed: no port answers; a wrong share is refused and counted, and the next attempt waits longer; the service user is refused every command but `status`; a conf that names another seal method is ignored; seal drops keys and closes listeners; a server that stays sealed past `TimeoutStartSec` then unseals and serves; a `rotate-root` stopped before the rename keeps the old root; a database restore with the recovery bundle and shares on a new runtime directory unseals and reads every row (regression). A full TPM reboot on a real host (acceptance, `SealedStart`) |
| **S4** | The generator writes bootstrap secrets to `seal.json` and the others to `SecretValue`, not `.env` (before the first release a clean start; no move of an existing `.env`); external credentials by `operator secret put` and Admin's write-only fields, and on a managed instance by its manager over the link ([Vault-Instances](Vault-Instances.md) M2); the embedded Postgres password (and a dedicated search cluster's, `search-cluster-password`) into `seal.json` as bootstrap secrets, with no `pgpass`; the cluster TLS key as a `SecretValue`; ACME keys served from their row, with no key file; secret handles in place of secret properties, and `System.getProperty` refuses a secret name | `.env` secrets | A new runtime's `.env` and runtime directory hold no secret (a key file an operator put there is the one exception, and the surface report names it), and a cold start with a password-protected database works; a generated secret is never in `.env`; a script that asks `System.getProperty` for a secret name gets nothing (regression) |
| **S5** | Memory hygiene of §3.5; ciphertext in the entity cache, with the decrypt at field read; bulk paths without plaintext (§3.10) | Keys and secrets in dumps, swap and caches | `seal` zeroes the key buffers (unit test on the type); after `seal` the caches hold no plaintext of an encrypted field and an old handle is refused; a find that reads no encrypted field decrypts nothing; an export, a snapshot and an entity REST read hold no plaintext of an encrypted field; a `seal` while a request is blocked past the time limit ends the process; the process is not dumpable (regression) |
| **S6** | Zones: an outer keyring for PASS and edge nodes; replicated secrets re-encrypted on arrival; PASS's client credential to mTLS with a pinned key, the header secret kept for REST until PASS moves; a zone check that refuses a BOSS data key on an outer node | Outer node gives inner keys | An edge database dump and its `seal.json` decrypt no BOSS row; a replicated secret on the edge is in the edge's format (regression). Attach and replicate on two runtimes (acceptance, in Edge's suite) |
| **S7** | Sign-in core of §3.6; canonical identity with its column and the collision check; the lockout count for guessable secrets; the fail-closed checks; identity kinds; the factor table; scope; a lint that finds `==` on a secret, and a host member for a script's compare | Edge sign-in bypass | Canonicalization, a create whose canonical form another account has refused, a start with two such accounts that serves with the old lookup, the shared count, a locked password that leaves a valid key working and uncounted, an account disabled for good that stays disabled after failed attempts, `ipAllowed` with no address refused, and the scope refusal, one test each (regression). The matrix of every sign-in path of §2.1 × wrong, expired, replay, disabled, end date, case and Unicode variant, factor skipped, other organization (acceptance, `SignInPaths`) |
| **S8** | Code and data apart (§3.7); new unit; deploy approval | A feature turns into code execution | No upload, export, log or audit path resolves under a code directory (regression). The packaged server starts and serves under the new unit (acceptance) |
| **S9** | Unsafe production starts (§3.8); an empty `crypt-pass` while an entity has `encrypt="true"` fields is refused on every instance | Unsafe settings | One test for each refusal; a default production start, with `entity-facade@crypt-pass-old` at its sentinel, is not refused (regression) |
| **S10** | Outside review of the design and of `crypto.rs`, the keyring and the seal code; publish the result in `docs/security/` | Trust you can check | – |
| **S11** | `kms` and `transit` methods, with a manager's transit for the instances it manages ([Vault-Instances](Vault-Instances.md) M6) | Root key held on the host | Unseal by each method against a stub (regression) |
| **S12** | Less attack surface (§3.9): `moqui operator surface`; a test that every optional listener is off by default | Unneeded surface | The default conf binds no optional listener; the report names each bound listener (regression) |
| **S13** | Access (§3.10): the secret access map, the dated authorization history, the decrypt log with HMAC'd values, the use log of secret handles, fail-closed `audit="required"`, the Access report screen | Not knowing who can read or did read a secret | A decrypt writes one line with no value, and two users who read one cached row write two lines; a hot-path field is counted, not logged line by line; a handle use is counted; with the log store down an `audit="required"` decrypt fails and other decrypts leave a gap row; an artifact rule changed with no change of membership shows on its date; the map names a user who gains the permission and drops one who loses it, as of each date; a user without `ADMIN_ACCESS_VIEW` is refused (regression) |
| **S14** | Key inventory and rotation (§3.11): current and previous for every generated key, `operator rotate`, the Keys screen with its write counts, emergency rotation | Keys that cannot be rotated without an outage | For each signing key: a value signed by the previous version is accepted until it expires; emergency rotation refuses it at once and ends every session. For each encryption key: emergency rotation leaves every row readable; a device store opens after `chat@store-key-secret` rotates; a backup made before a rotation restores and reads (regression) |
| **S15** | Incident response (§3.12): `revoke#AllAccess`, the alerts, the runbook | No way to contain and scope a breach | `revoke#AllAccess` ends every credential kind of §3.12 and pauses jobs (regression). A tabletop run of the runbook on a scratch server (acceptance, by hand) |

Order: S1 and S2 make the stored data safe even before sealing. S3 and S4 remove keys from disk. S6 follows S3,
because the outer zone needs its own seal. S7 and S8 do not depend on the others and can run beside them. S12 and S13
need nothing else. S5 follows S4 (secret handles). S13's decrypt log follows S5 (ciphertext in the cache). S14 follows S1 and S2. S15 follows S3 (for `seal`), S13 (to scope) and S14 (to rotate). Vault-Instances' M6 (managed sealed start) follows S3 and S11. Its machine access moves onto S1's keyring once S1 is built, and its vault events join S13's report.

## 5. Tests

- **Regression** (`cargo test` for the crate, and framework specs where a service or screen is involved): one test for
  each rule a phase adds, under a second. The `known_gap_*` tests are flipped, not deleted, when a gap closes: the test
  keeps its subject and asserts the control.
- **Acceptance** (`./moq accept framework`): `SealedStart` (a real reboot with the TPM method), `SignInPaths` (the
  matrix of §4 S7, built from §2.1) and the edge attach with replicated secrets. These run only when asked or when
  changed.
- **Proof catalog:** each test is a row in [Proof-Tests, Stored secrets](../docs/security/Proof-Tests.md).

## 6. Decisions

| Question | Choice | Why |
|---|---|---|
| AEAD | AES-256-GCM, random 96-bit nonce | Hardware support everywhere, the `aes-gcm` crate is audited. Under one key the nonce limit is about 2^32 values. Rotation (S2) keeps each key well under it. XChaCha20-Poly1305 is the fallback if a platform has no AES instructions. |
| Encrypt the whole database? | No. Encrypted fields plus disk encryption (LUKS) | Postgres has no transparent encryption that MR Moqui controls. The keyring protects the secrets, and LUKS protects the rest at rest. |
| Default unseal for BOSS | `tpm` through `systemd-creds` | Unattended reboot without a key on disk. Recovery shares cover a lost TPM. |
| Where the operator talks while sealed | A Unix socket, reached at the console or through the host's own sshd running `moqui operator`; never HTTP. The SSH admin shell only once unsealed | While sealed, nothing faces the network, and the SSH admin shell is one of the server's own listeners (§3.3). |
| Client-side (zero-knowledge) vault for personal secrets | In [Vault-Instances](Vault-Instances.md), not here | Decided with the user, Nov 14, 2026: personal vaults that only a client opens, and operational vaults for the secrets of managed instances. This plan keeps the server's own keys; that one keeps the keys people hold. |
| Lockout scope (§3.6) | One count for guessable secrets. A lock stops that class of credential, not the account. High-entropy credentials, SIP and XMPP are limited by source and by credential | Review of 2026-11-15: one count across SIP and XMPP let an anonymous caller lock any account (a SIP source address can be forged) and reversed choices the code makes on purpose. |
| Colliding usernames at S7 (§3.6) | The old lookup stays until the check passes; the server starts | PASS visitors register their own names, so a refusal to start would let a visitor stop the server. |
| Checks of the sign-in core (§3.6) | Fail closed: disabled for good stays disabled, one reading of the lock and the end date, `ipAllowed` with no address is refused; identity kinds; one factor table | Today failed attempts turn a permanent disable into a five-minute lock, XMPP and SIP read the lock their own way, and a path with no address skips `ipAllowed` (§2.1). |
| Production refusals (§3.8) | The kept `decrypt-alt` sentinel is not a refusal; the tarpit refusal names its artifact types; the first-administrator rule is a check in the service | As written every default production start was refused: `entity-facade@crypt-pass-old` keeps `CHANGEME` on purpose and `instance@purpose` defaults to `production`. |
| Where `seal.json` and the socket are (§3.1, §3.3, §3.7) | `seal.json` in the data directory; the socket in `/run/moqui` | §3.1 and §3.7 named two places for `seal.json`, and `runtime/` is under the code directory, which the server does not write. |
| Who may use the socket (§3.3) | Peer credentials: the service user gets `status` only; other commands need root or the operator group | Mode 0600 for the service user let every child, job and terminal session of the server seal it or replace a secret. |
| Seal method (§3.3) | Recorded in `seal.json` at init; only `rekey` changes it; an automatic method sends `READY=1` after its unseal | The server can write the conf and `.env` that name the method and the purpose. `READY=1` while sealed told systemd that a server which serves nothing had started. |
| Data keys (§3.1) | One data key per purpose | With one key for all purposes, refusing a signing key in an emergency also rotates the field key. |
| Associated data of an audit copy (§3.2) | Its own label and the audit row's key; a ciphertext is never clipped | With the live row's associated data, a database writer could copy an old audit value back into the live field. A clipped ciphertext cannot be read or rewrapped. |
| The "no fallback" state and the keyring rows (§3.2) | A flag in the keyring's wrapped metadata; a row's state and generation in its wrap's associated data | A row count or a plain column is data that a database writer can change. |
| Conditions on an encrypted field (§3.2) | Equals and `in` through the blind index; the others refused | A random nonce leaves nothing to compare. Today the bound value is encrypted for every operator. |
| Cluster TLS key (§3.4) | A `SecretValue`, not a bootstrap secret | The cluster transport starts after the database opens, and a copy in each member's `seal.json` would have to be kept equal by hand. |
| TLS key files (§3.1) | ACME keys served from their row; an operator's key by `operator secret put` | A key file in `runtime/cert` is a usable key on disk, against the goal. |
| Decrypt time and bulk paths (§3.5, §3.10) | A decrypt is at field read; bulk paths carry no plaintext | A decrypt at row read would log a line for each request on hot paths. Export, snapshots, entity REST and EntitySync hand out decrypted fields in bulk. |
| Staying under the nonce limit (§3.11) | A write count for each encryption key version on the Keys screen | Rotation is by hand, and nothing counted writes. |
| What the root key wraps before S1 (§3.1) | `seal.json` holds the root key wrapped by the method and by a recovery key whose shares are issued; the bootstrap secrets (the datasource and embedded Postgres passwords, `entity-facade@crypt-pass` and `-old`, the edge member key) under a key derived from the root key; and one data key for `SecretValue`, wrapped by the root key | The user's choice of S3, S4 and S11 without S1, Nov 19, 2026: the field key still comes from `entity-facade@crypt-pass`, so that pass is a bootstrap secret |
| A first start with no `seal.json` (§3.3) | A purpose other than `production` makes the `dev` seal by itself. A production instance waits sealed for `moqui operator init`, unless `seal@method` names an automatic method: then it makes its seal and keeps the recovery shares in memory until `moqui operator init --take-shares` reads them once, with an alert until then | The user's answer, Nov 19, 2026 |
| Scripts and secrets (§3.1, §3.6) | `System.getProperty` refuses a secret name; a host member compares in constant time | Any script could read any secret property, and the lint reads only Rust. |

## Open points from the security review

The review of 2026-11-15 found these. Each would change a recorded choice or has options with real trade-offs, so the
design above stays as written until they are decided.

| Point | What the review found | Options |
|---|---|---|
| The canonical form of a username (§3.6) | NFKC, trim and case fold is a rule of our own. XMPP and SIP clients already apply PRECIS `UsernameCaseMapped` (RFC 8265). Neither has a crate in the workspace | NFKC with case fold, as written. PRECIS `UsernameCaseMapped` on every path |
| A key for each entity and field (§3.1, §3.2, §6 AEAD) | One `field` key encrypts every field, every audit copy and every rewrap, and the limit of about 2^32 values is per key | One field key with the write count of §3.11, as written. A key for each entity and field, by HKDF with the label `field:<entity>.<field>` |
| Low-entropy fields found by value (§3.2) | `PartyIdentification.idValue` holds short tax and government ids. With the index key they are guessed offline; without it the index shows which rows hold equal values, which is today's leak kept | The blind index, as written. No lookup by value (a scan). A keyed hash of a normalized value with a salt for each organization |
| `SamlFlow`'s encrypted fields while SAML is refused (§2.1, §3.11) | `SamlFlow.keystorePassword` and `privateKeyPassword` are encrypted fields of a flow that the port refuses | Keep them and rewrap them. Drop the fields until SAML is built. Leave them out of the rewrap and count them as old format |
| Component deploy approval and the attested deploy (§3.7) | §3.7 loads a component only after a second administrator approves. The attested deploy has no person on the target: its gate is a passed run and a signature under a shared secret | The attestation counts as the approval where the source environment requires one. The target also asks a second administrator. Attested deploys are off where the two-person rule is on |
| System › Conf's editor with a read-only conf (§3.7) | The editor (sealed `CONF_EDIT`) writes the conf files, which §3.7 makes read-only, and a conf that can be edited at runtime can name a path, the seal or the instance purpose | Remove the editor on a packaged server. An overlay file in the data directory that cannot set a path, the seal or the purpose |
| Recovery shares of a managed instance (§3.3; Vault-Instances §4.5), decided: holders' keys signed by the approvers | The manager supplies the holders' public keys, and nobody on the instance compares a fingerprint, so a manager that sends keys of its own gets every share. Vault-Instances §4.5 also lets shares be entered in the manager's screen, against §6 "never HTTP" | Holders' keys signed by the environment's approvers, whose signing keys the instance pins at pairing, with each holder confirming its share. Holders enrolled on the instance's own socket. Decided in Vault-Instances |
| The manager and a managed instance's root key (§1 rule 6, §3.12) | At each unseal by `transit` the manager sees the root key, and it receives the recovery bundle. A compromised manager process with a backup reads every row of that instance | As written, with the limit stated. The approval unseal policy as the default for production. Backups and recovery bundles kept off the manager |
| The link key of a managed instance (§1; Vault-Instances §4.4) | It must work while sealed, so it is not under the root key and sits on disk. With the automatic unseal policy a stolen disk gets the root key until the manager revokes it, against "no usable key is on disk" | A named exception, as §1's table has it. Bound to the TPM where there is one. The manager alerts on an unseal from a new address |
| Machine access before S1 (§4 order; Vault-Instances §3.8) | Under today's field key a machine-access ciphertext copied to another environment's row decrypts, and the item keys leave by the bulk paths of §2 | Machine access waits for S1. Until S1 it is sealed by the vault crate's own AEAD with the environment, vault and item ids as associated data |

## 7. Not in scope

- Root on a running, unsealed BOSS process. No design protects against it. Vault states the same limit.
- Business rows that are not encrypted fields. Use disk encryption.
- A database writer, with no key. It cannot read, move or forge an encrypted field (§3.2). It can still delete or
  withhold rows, change any field that is not encrypted, and put a row's older ciphertext back in place of its newer
  one: associated data stops a move, not a rollback.
- A compromised manager, for the instances it unseals by `transit` (§3.12).
- People's secrets and the secrets of managed instances: [Vault-Instances](Vault-Instances.md).
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
| Bitwarden | The server holds no key | The outer zone holds no inner key (S6); personal vaults ([Vault-Instances](Vault-Instances.md)) |
