# Vaults and Instances

Status 2026-11-19: being built, with Sealed-Secrets S3, S4 and S11; the open points were decided on Nov 19
([§9](#9-decisions)). Before: 2026-11-14, plan; nothing built. Reviewed 2026-11-15 against the code in the security review: the facts of §2
are corrected, the design of §3.2, §3.3, §3.5, §3.7 to §3.10, §3.12, §4.2, §4.4 to §4.6 and §4.9 changed (signing
keys and signed wraps, the signed lease, machine access, recovery shares, the manager's limits), and the choices left
to the owner are in [Open points](#open-points-from-the-security-review). It expands the user's notes of Nov 14, 2026
and their answers the same day ([§9](#9-decisions)). [Sealed-Secrets](Sealed-Secrets.md) keeps the server's own keys.
This plan keeps the keys people hold, and the keys of the instances a server manages, and it covers the deployment
side of a sealed start: how a managed instance gets its secrets, unseals, keeps its recovery shares and rotates its
credentials.

The plan has two parts, and they share one problem: secrets that a person must open.

1. **Vaults.** A vault holds secrets under a key that the server does not hold.
   - A **personal** vault holds a person's secrets (logins, notes, private contacts, keys). It opens only in a client:
     the browser, the desktop and mobile apps, or the `moqui` command on the person's own machine. The server stores
     ciphertext, and its administrators cannot open it.
   - An **operational** vault holds the secrets of deployed instances. A member opens it in their client for one
     operation, and the server uses the items that operation needs for as long as it runs, then forgets them.
2. **Instances and CD.** Instance management provisions, deploys, watches and rotates MR Moqui, Postgres, MySQL and
   OpenSearch instances on Docker and Podman hosts. Every credential is in an operational vault, a managed MR Moqui
   instance talks to its manager over a link like the edge link, and Monitor knows the fleet.

Paths: `fw/` is `runtime/components/base/framework/`, `tools/` is `runtime/components/base/tools/`, `boss/` is
`runtime/components/addon/mr-boss/`, `host/` is `crates/moqui-host/src/`. Every other path is relative to this
repository.

Sections: [1 three tiers](#1-three-tiers-of-secrets) · [2 what is there now](#2-what-is-there-now) ·
[3 vaults](#3-vaults) · [4 instances](#4-instances) · [5 screens](#5-screens) · [6 phases](#6-phases) ·
[7 tests](#7-tests) · [8 other plans and docs](#8-other-plans-and-docs) · [9 decisions](#9-decisions) ·
[open points](#open-points-from-the-security-review) · [10 not in scope](#10-not-in-scope) ·
[11 inspiration](#11-inspiration)

## 1. Three tiers of secrets

| | Platform secrets | Operational vaults | Personal vaults |
|---|---|---|---|
| Plan | [Sealed-Secrets](Sealed-Secrets.md) | this plan, §3 and §4 | this plan, §3 |
| Who decrypts | The server, with nobody present | The server, during a lease a member approves, or under an environment's machine access | A client only |
| Opened by | The seal: TPM, KMS, transit, passphrase or shares | A member's personal key, in the member's client | The vault's password or key (single-secret), or a member's personal key (shared-key) |
| Holds | Bootstrap secrets, generated keys, and credentials the server uses all the time: SMTP, OAuth client secrets, FCM, model API keys | A managed instance's database, search and link credentials, host TLS keys, registry and database admin logins, deploy tokens, recovery bundles | Logins, notes, private contacts, cards, SSH keys, TOTP seeds, recovery codes and shares |
| Stored in | `seal.json`, `SecretKeyring`, `SecretValue`, `v2` fields | `Vault`, `VaultItem` rows, as ciphertext | The same rows |

A secret's tier follows from one question:

- **The server must use it with nobody present:** a platform secret.
- **It is used by an operation a person starts or approves,** or by an environment set for machine access: an
  operational vault item.
- **The server never needs it:** a personal vault item.

Integration credentials that the server uses all the time stay platform secrets. design-admin's "not a new
credential-vault implementation" (`boss/plan/design-admin.md`, Credentials) still holds for them.

## 2. What is there now

| Area | Now | Where |
|---|---|---|
| Instance tables | `moqui.server.instance`: `InstanceHost`, `InstanceHostType` (services by name, with the CI run services DG5 added), `InstanceImage`, `InstanceImageType` with its env, link, volume and host config templates, `DatabaseHost`, `DatabaseType`, `AppInstance` with its env, links, volumes and host config. Ported from Moqui Java. The seeds name Moqui Java's images (`moqui/moquidemo` and others), its volume paths (`runtime/elasticsearch`, `runtime/txlog`) and a `LocalMySQL` admin password, the literal `moquiroot` | `fw/entity/ServerEntities.xml` |
| Instance services | `provision#AppInstance` creates the database and user, then inits and starts the container. The Docker services (`init#InstanceDocker` and the rest) are JS over the Engine API v1.24, on plain HTTP (port 2375 by default); they do not use the host's TLS fields. `create#DatabasePostgres` and `create#DatabaseMySQL` run SQL with the database host's admin login. `get#AppInstanceEnv` returns that admin password in its `adminMap` out-parameter | `fw/service/org/moqui/dev/InstanceServices.xml` |
| CI driver | Rust, on Docker and Podman: a Unix socket, HTTP, or HTTPS with the host's `tlsCaCert`, `tlsClientCert` and `tlsClientKey`; runs that end, with limits; a `fake` host type for tests. Since M1 the Engine API client is shared with managed instances as `moqui_lib::docker` (containers that stay, volumes, networks, secret files, a fake engine), and CI keeps its run policy | `crates/moqui-lib/src/docker/`, `crates/moqui-git/src/ci/driver/docker.rs`, `fake.rs` |
| Instance screens | System › Instance: InstanceList, InstanceDetail | `tools/screen/System/Instance/` |
| Secrets in instance rows | `InstanceHost.password` and `tlsClientKey`, `InstanceImage.password`, `DatabaseHost.adminPassword`, `InstanceImageTypeEnv.envValue` and `AppInstanceEnv.envValue` are `encrypt="true"` fields. Any find decrypts them, and the env values reach the container as environment variables, which `docker inspect` and `/proc/*/environ` show | `fw/entity/ServerEntities.xml` |
| CI secrets and environments | `GitCiSecret` (encrypted, scoped to protected refs, tags or an environment), `GitCiEnvironment` (`targetUrl`, an encrypted `targetToken`, `requiredPipeline`, `approvalRequired`), `GitDeployment`. A deployment of a component repository is built: `request#GitDeployment` queues the environment's pipeline for the branch's head with the `deploy` event; when that run passes, the deployment waits for `decide#GitDeployment` if the environment requires approval or no branch rule protects the branch (never the requester); then the deploy step posts an attestation to the target's Component-Deploy REST (`deploy#AttestedComponent`, called with no user signed in). The attestation is JSON signed with HMAC-SHA256 under `targetToken`: the audience (the target's URL), the repository URL, branch, commit, component, pipeline and run, environment, deployment id, time and data types. The target holds the same secret as `component_deploy_attestation_key`, checks the signature, the age and the audience, fetches the branch and deploys that commit. A CI job's secrets reach its container as environment variables | `fw/entity/GitCiEntities.xml`, `fw/service/org/moqui/git/GitCiServices.xml`, `crates/moqui-git/src/ci/deploy.rs`, `attest.rs`, `trust.rs`; [Pipelines](../docs/interfaces/Pipelines.md#deployments) |
| Deploy plan | Environments, the deploy event, the gate and the approval: built for a component repository, as above. Not built: an owner's environment, a private repository's fetch by the target, a tag, a framework archive or image, and deploys to managed instances | [Dogfood §8.6](Dogfood.md#86-deploys), DG10; [Pipelines](../docs/interfaces/Pipelines.md#not-built) |
| Component deploy | Fetch, preview and deploy from git; the database storage reaches every cluster member | [Component-Deploy](../docs/tools/Component-Deploy.md#deploying-from-git) |
| Monitor | OTLP in with `service.instance.id`; Services and Releases by instance; regressions compared across instances; `RecoveryPolicy`, `RecoveryAdapter` (`in-process`, `webhook`, `manual`), `RecoveryAction`; `InstanceDrain`; `ScrapeTarget`; `IngestKey`; annotations by `POST /v1/annotations` | `fw/entity/MonitorEntities.xml`; [Monitoring](../docs/tools/Monitoring.md#recovery) |
| Edge link | A local node dials its edge node's HTTPS port. Pairing runs on a separate ALPN over the web certificate, then both link keys are pinned. gRPC runs both ways on HTTP/2, with generations, PINGs and redial with backoff. The members of a local cluster share the pairing's key in the `.env` they share, and each member also has an Ed25519 key of its own in `runtime/db/edge_member_key`, which the edge node pins on the member's first link | `host/edge_link/` (`member_key.rs`); [Run-Deploy](../docs/Run-Deploy.md#a-local-server-with-an-edge-node) |
| Client crypto | Chat's crypto device (matrix-sdk-crypto in wasm, loaded only when asked). Its store is encrypted under a key the server derives, so it is not zero-knowledge. Room keys are backed up to a recovery key the user keeps | `runtime/components/base/webroot/screen/webroot/js/WebrootCrypto.js`, `crates/moqui-core/src/service/chat_keys.rs`; [Chat-Keys](../docs/interfaces/Chat-Keys.md) |
| Device key stores | Desktop: the OS key store, or a file only the user can read. Android: an AES-GCM Keystore key | `crates/moqui-desktop/src/client/keystore.rs`, `crates/moqui-mobile/gen/android/app/src/main/java/org/moqui/mobile/KeyVault.kt` |
| Crypto crates | `argon2`, `aes-gcm`, `hkdf`, `curve25519-dalek` and `chacha20poly1305` are in the workspace. No HPKE, Shamir, `zeroize`, `secrecy` or `subtle` crate is a workspace dependency. An age implementation already wraps a file key to X25519 recipients (X25519, HKDF, ChaCha20-Poly1305) | `Cargo.toml`, `crates/moqui-lib/src/age.rs` |
| Passkeys | Not built | [Dogfood §15](Dogfood.md#15-later), Remaining-Work |

## 3. Vaults

### 3.1 Kinds and locks

A vault has a **kind** and a **lock**. Both are chosen when the vault is made, and neither changes.

| Lock | Opened by | Members | Recovery | Kinds |
|---|---|---|---|---|
| **Single-secret** | One password, or one generated key that the owner keeps | Its owner only | None | Personal only |
| **Shared-key** | Each member's personal key: the vault key is wrapped to each member's public key | One or more | The organization's recovery key, when the vault has it on ([§3.10](#310-recovery)) | Personal and operational |

- A **personal** vault opens only in a client. It is either single-secret or shared-key (a family's, a team's).
- An **operational** vault is always shared-key, so each member has access of their own and one member can be removed
  without the others changing anything. Its items are used on the server under leases ([§3.7](#37-leases-operational-items-used-on-the-server)).
- Items move between two personal vaults only in a client that has both open. Operational items stay in operational
  vaults.

### 3.2 Keys and formats

```
password (Argon2id) or generated key (HKDF)          single-secret
member's personal key, recovery key (HPKE)           shared-key
        │ unwraps
        ▼
vault key: 256 bits, versioned
        │ unwraps                  (operational: also the deposit key's private half)
        ▼
item key: 256 bits, one per item   (operational: or wrapped to the deposit key's public half)
        │ AES-256-GCM
        ▼
item body: the item's JSON, encrypted whole
```

- **Item keys.** Each item has its own random key, wrapped by the vault key. There are three reasons:
  - A lease carries only the item keys of its operation.
  - Rotating the vault key rewraps the item keys, not the bodies.
  - One item can be sent to another person ([§3.3](#33-tables), deliveries).
- **AEAD everywhere.** Each wrapped key and each body is AES-256-GCM with a random 96-bit nonce. Its associated data
  names what it is: the vault id, the item id, the key version and the purpose (`vault-key`, `item-key`, `body`). As
  with Sealed-Secrets' `v2` format, a value copied to another row does not open. For an operational item, the
  associated data of the item-key wrap and of the body also names the item's type, binding and clear name. An item
  whose binding, type or name column is changed, or whose wrap is put under another binding, does not open. Bodies
  are padded to 256-byte steps, and an item is at most 64 KiB.
- **Public-key wraps** use HPKE (RFC 9180, base mode: DHKEM(X25519), HKDF-SHA256, AES-256-GCM). The HPKE `info`
  carries the same names as the associated data above.
- **Signatures.** Base mode names no sender: anyone who has a public key can write a valid wrap to it. So each
  public-key wrap is signed by its maker's signing key (Ed25519, [§3.5](#35-the-personal-key)) over the wrap and its
  names: a member's wrap of the vault key, a new key version with its member list, the deposit public key, a recovery
  key and a vault's wrap to it, and a delivery. A lease is signed by the server's lease key
  ([§3.7](#37-leases-operational-items-used-on-the-server)). A client refuses a wrap or a lease whose signature is
  missing or wrong, or whose signer it cannot place ([§3.9](#39-members-removal-and-rotation)).
- **Argon2id** for passwords and passphrases: at least m=64 MiB, t=3 and p=1, so a phone can open a vault in about a
  second. The parameters are stored with each wrap, and the crate has three numbers for them:
  - the **floor**, m=64 MiB, t=3 and p=1: no client writes less, so a wrap under it is refused;
  - the **target**, which starts at the floor and is raised over time: a client that finds a wrap under it wraps again
    with the target on that open, which is the lesson of the LastPass breach ([Sealed-Secrets §8](Sealed-Secrets.md#8-inspiration));
  - the **ceiling**: a wrap over it is refused, since the server stores the parameters and a huge value stalls a phone.
- **The deposit key** (operational only) is a key pair. Its public half is stored in the clear on the vault, signed by
  the member with the manage role who made it, and its private half is wrapped by the vault key. The server makes a
  new item with no lease (a password it generates while provisioning) by wrapping the item key to the deposit key,
  after it checks that signature against the vault's signed member list. It can write the item but cannot read it
  back. A member's client checks the deposit key's signature on each open
  ([§3.12](#312-what-the-server-sees-and-the-limits) has the limit).
- **One implementation.** A small crate with no I/O, `crates/moqui-vault`: the formats, the KDFs, the wraps, the
  signatures, the item AEAD, the lease seal and zeroized key types. The browser module must build without the
  framework, which is why it is its own crate. V0 adds it by the Crate-Split rules
  ([Practices-Rust](../docs/dev/Practices-Rust.md)). It is linked natively into the server, the desktop and mobile
  shells and the `moqui` command, and built to a small wasm module for the browser, since WebCrypto has no Argon2id.
  A vault written on a phone opens the same way in a browser and on the server.

### 3.3 Tables

Framework entities, package `moqui.security.vault`, in a new `fw/entity/VaultEntities.xml`. There is one row per vault
item, many to one with the vault, as the user asked.

| Entity | What it holds |
|---|---|
| `Vault` | `vaultId`, `vaultKindEnumId` (personal, operational), `lockEnumId` (single-secret, shared-key), `ownerUserId`, `keyVersion`, `revision`. A single-secret vault: its wrapped vault key and KDF parameters. An operational vault: `vaultName` and the deposit public key with its maker's signature, in the clear. Each key version: the member list (users, roles, personal key ids) and the signature of the member who made it ([§3.9](#39-members-removal-and-rotation)). A personal vault's name is encrypted, as part of a small header body |
| `VaultItem` | `vaultItemId`, `vaultId`, `wrappedItemKey`, `itemKeyWrapEnumId` (vault, deposit), `keyVersion`, `bodyCipher`, `revision`, dates, `createdByUserId`. Operational only, in the clear: `itemTypeEnumId` and `bindingKey` (`AppInstance:<id>:dbPassword`), which is unique in its vault, so a service finds an item without opening the vault, and the item's name. All three are in the associated data ([§3.2](#32-keys-and-formats)) |
| `VaultItemHistory` | Earlier revisions of an item, with their wrapped key and body: a password's history, and a rotation's way back ([§4.7](#47-rotation)). Kept for a retention period |
| `VaultMember` | `vaultId`, `userId`, `roleEnumId`, the vault key wrapped to the member's personal key, `personalKeyId`, the key version, who made the wrap and their signature, from and thru dates |
| `VaultPersonalKey` | `personalKeyId`, `userId`, the two public keys (X25519 to receive, Ed25519 to sign); the two private keys wrapped together by the key passphrase (with its KDF parameters) and by the user's recovery code; `statusId` |
| `VaultKeyCheck` | A key that was checked ([§3.5](#35-the-personal-key)): the checker's `userId` and personal key, the checked key (a `personalKeyId` with both its public keys, or the server's lease key), the date and the checker's signature |
| `VaultRecoveryKey`, `VaultRecoveryHolder` | An organization's recovery key: its public key with its maker's signature, and its private key wrapped to each holder's personal key, each wrap signed by its maker |
| `VaultDelivery` | A sealed value sent to one user: HPKE to the recipient's personal key, from a user or an instance, with its purpose and its sender's signature (a user's signing key, or an instance's link key). The recipient's client checks the signature and files it into a vault |
| `VaultLease` | A lease ([§3.7](#37-leases-operational-items-used-on-the-server)): the vault, the item bindings, the operation, the cluster member that runs it, the lease's public key, the server's signature over all of these, the item keys sealed to it once approved, who approved with their signature, the time limit and the state. It never holds a key that opens anything by itself |
| `VaultMachineAccess` | An environment's machine access ([§3.8](#38-machine-access)): named item keys, each sealed by the vault crate's AEAD under a server key, with the environment, vault and item ids as associated data. The column is not an entity `encrypt="true"` field |
| `VaultEvent` | Each event of [§3.13](#313-events-and-the-access-report). Rows, not log lines, kept as long as the audit log |

### 3.4 Item types

`VaultItemType` is seed data that registers each type's fields: name, label, kind (text, secret, URL, TOTP, multi-line,
date, file name) and whether a value is masked. The client draws forms from it. For operational types, the server checks
an item's shape when it uses it. The framework ships login, note, card, identity, contact, SSH key, API key, database
credential, TLS key pair, registry login, recovery code and recovery share. Components add their own: an instance kind
adds the items it needs ([§4.1](#41-the-model)).

A body is versioned JSON: `{"v": 1, "type": "login", "name": "…", "fields": {…}, "notes": "…", "tags": […], "uris": […]}`.
In a personal vault the type and name are inside the body. In an operational vault they are also in the clear.

### 3.5 The personal key

- Each member of a shared-key vault, personal or operational, has a **personal key**: an X25519 key pair to receive
  wraps and an Ed25519 key pair to sign what the member makes ([§3.2](#32-keys-and-formats)). Its private halves are
  wrapped together by the **key passphrase**, which is independent of the sign-in password and never reaches the server
  (the user's answer, [§9](#9-decisions)). The server stores no hash of it: a wrong passphrase is a failed unwrap in
  the client.
- Setting up a personal key gives a **recovery code** to print or save, and asks the user to type part of it back.
  Changing the passphrase rewraps one value.
- A user who loses both the passphrase and the recovery code makes a new personal key. Their single-secret vaults are
  unaffected. For their shared-key vaults, another member or the recovery key ([§3.10](#310-recovery)) wraps the vault
  key to the new personal key, after the new key's fingerprint is checked.
- The server sends a wrapped private key only to its signed-in user, so an online guess needs a sign-in first. A
  database thief can still guess offline, so the passphrase has a length and strength check in the client.
- **Fingerprints.** When a member is added, the dialog shows the new member's key fingerprint as a phrase to compare
  with them by another channel. A server that swapped in a key of its own would otherwise get the vault key. The
  fingerprint covers both public keys. A key that has been checked is remembered as a `VaultKeyCheck` row, signed by
  the checker's signing key, and a client verifies that signature before it relies on the check.
- **A changed key is checked again.** A check is for one key. When a member's personal key changes, nothing is wrapped
  to the new key, by a member or by the recovery key, until its fingerprint is checked again.
- **Device memory** (V5): the desktop and mobile apps can keep the personal key unlocked for a set time, wrapped by a
  key in the OS key store and opened with the device's lock or biometrics. In a browser the key stays only in the
  worker's memory, until the vault locks or the tab closes. A passkey slot (WebAuthn PRF) comes once passkeys are
  built.

### 3.6 Opening a vault in a client

- The vault module runs in a worker: a Web Worker in the browser, Rust in the shells. Keys never leave it. The page asks
  it for one item at a time, to show or to copy, and a copied value is cleared from the clipboard after 30 s where the
  platform allows.
- A vault locks after 15 minutes idle (a setting), on sign-out, on the device's lock and when the tab closes.
- The browser module is loaded only by the vault screens, with integrity hashes, and those screens get a strict content
  security policy. The hashes come from the same server, so they guard against a cache or a proxy, not the server.
  The desktop and mobile apps carry their own copy of the module and do not take it from the server.
- Search runs in the client over the open vault (names, URLs, tags). The server has no index of personal items.
- Private contacts are `contact` items. Commons › Contacts merges them in, in the client, while the vault is open, and
  marks them. They never reach CardDAV, mail address completion or the server's search.

### 3.7 Leases: operational items used on the server

In normal use a member never sees an operational value. The server decrypts it for the operation and then forgets it.

1. An operation (a provision, a deploy run, a rotation, a recovery action) asks for a **lease**: the vault, the items
   it needs by binding, the operation and a time limit. The default limit is the operation's own, at most 4 hours (an
   environment setting). The server makes an ephemeral X25519 key pair for the lease and holds it in memory only, on
   the cluster member that runs the operation. It signs the lease with its lease key: the lease id, the vault, each
   item's id, type, binding and name, the operation, the cluster member, the time limit and the lease's public key.
2. A member with the approve role approves in their client, with a recent sign-in (`check#RecentAuth`). The client
   checks the lease's signature, opens the member's personal key (asking for the key passphrase if it is locked),
   unwraps the vault key, unwraps the item keys the lease names, and seals them to the lease's public key, with the
   member's signature. Each item key is unwrapped with the type, binding and name the lease gives as associated data
   ([§3.2](#32-keys-and-formats)), so an item that is not the one the lease names does not unwrap. The sealed keys are
   written to the lease row, which is safe: only the lease's private key, in one member's memory, opens them.
3. The server opens the sealed item keys and decrypts the items when the operation uses them. Each use is an event.
4. The lease ends when the operation ends, at its time limit, on Revoke, or on a seal. The lease's keys and the item keys
   are zeroized (the key types of [Sealed-Secrets §3.5](Sealed-Secrets.md#35-memory)), and nothing stored can open them
   again.

- **What it gains.** The member's passphrase, their personal key and the vault key never reach the server. A server
  that is compromised during a lease gets that lease's items and no others.
- Before approving, the member sees what the lease names: each item's name and binding, the operation and the time limit.
  The dialog shows them from the signed lease, never from a column, and the unwrap of step 2 proves each one.
- **The lease key** is an Ed25519 key of the server: the `vault-lease` purpose key of the keyring
  ([Sealed-Secrets §3.1](Sealed-Secrets.md#31-key-hierarchy)), and until Sealed-Secrets S4 a generated secret, kept as
  today's generated secrets are. A member checks its fingerprint once, as they check a member's key, and the check is
  a `VaultKeyCheck` row. A client refuses a lease that is not signed, or signed by a key its user has not checked, and
  shows a changed lease key as a changed member key is shown. A writer of the database cannot put its own public key
  on a lease.
- **A leased value stays in the operation.** It is never a parameter of a queued or scheduled service call, and never
  a log or event field. A restart of the cluster member ends its leases, since their private keys are in memory only:
  the operation fails and asks for a new lease.
- **Approving a deploy is opening its lease:** one step, as Dogfood §8.6 and `GitCiEnvironment.approvalRequired` have
  it.
- **Reveal.** A member with the reveal role can see one value. The server sends that item's ciphertext to the member's
  client, which decrypts it there, and the reveal is an event. Otherwise the server sends no operational body to any
  client.
- **Where a lease is approved:** in the browser, the apps, or `moqui vault approve <lease>` on the member's own machine.
  The SSH admin shell lists and revokes leases but does not approve them, because the passphrase would be typed on the
  server.
- Scripts get no `ec` member that hands out a vault value at will. A kind's service reads a value from the lease it runs
  under, by binding. That host member is added to [JS-Core-API](../docs/dev/JS-Core-API.md) and
  `crates/moqui-devkit/src/typegen/moqui-host.d.ts` when it is built.

### 3.8 Machine access

Some environments need unattended work: the demo that deploys main after every green merge, or a CI runner host whose
TLS key the engine uses for queued jobs. An environment may hold **machine access** to the items its operations need
(the user's answer: allowed per environment).

- A member gives it in their client: they unwrap the named item keys and seal them to a lease the server makes for
  the grant ([§3.7](#37-leases-operational-items-used-on-the-server)). The server then seals each item key under its
  machine key, with the vault crate's AEAD (AES-256-GCM) and the environment, vault and item ids as associated data,
  so a row copied to another environment or item does not open.
- **The machine key** is the `vault-machine` purpose key of the keyring
  ([Sealed-Secrets §3.1](Sealed-Secrets.md#31-key-hierarchy), S1). Until S1 it is derived by HKDF from today's field
  pass (`entity-facade@crypt-pass`) with the label `vault-machine`, and the screen says that this pass is on the server's
  disk.
- The sealed item keys are not an entity `encrypt="true"` field. Today's field format has no MAC and no associated
  data ([Sealed-Secrets §2](Sealed-Secrets.md#2-what-is-there-now)), and the entity facade decrypts such a field on
  every find. The facade never decrypts this column, so no find, cache, export or sync holds an item key in the clear.
  Only the lease code of the vault services opens it.
- It covers named items only, for the environment's own operations. The environment and the vault both show it, with
  the warning that these items are now as strong as the server's own keys.
- A lease under machine access is recorded as any other lease, with "machine" as its approver.
- Revoking machine access deletes the wrapped keys and offers to rotate the items, since the server could have kept them.

### 3.9 Members, removal and rotation

- **Roles.** A personal shared-key vault has manage, write and read. An operational vault has manage, approve and
  reveal. Every member holds the vault key, so the roles are rules on what the server serves and accepts: wraps,
  bodies, writes and leases.
- **Add.** A member who can manage wraps the vault key to the new member's personal key, after the fingerprint check,
  and signs the wrap and the new member list.
- **Remove.** The remover's client deletes the member's wrap and rotates the vault key: a new key version, every item
  key rewrapped under it, and every remaining member's wrap remade, with the new member list, all signed by the
  remover. These are a few kilobytes, done in one transaction. A write at the old version is then refused.
- **Which key version a client accepts.** Each key version carries its member list and the signature of the member who
  made it. A client accepts a version, and a wrap of it, only when the signer held the manage role in the version
  before, back to the vault's maker, and the client's user has checked the signer's key or a key in that chain. A
  server or a database writer cannot then give a member a vault key of its own choice.
- A removed member may have kept item keys or values. So removal from an operational vault offers to rotate the
  credentials themselves at their source ([§4.7](#47-rotation)), which is the real revocation.

### 3.10 Recovery

| Vault | Recovery |
|---|---|
| Single-secret | None. Only its password or key opens it ([§3.11](#311-creating-a-single-secret-vault)) |
| Personal key | The user's own recovery code ([§3.5](#35-the-personal-key)) |
| Shared-key, personal or operational | Another member wraps the vault key to a member's new personal key. Or, when the vault has it on, the organization's recovery key does |

- An organization **recovery key** is a key pair. Its public half wraps the vault key of each shared-key vault that has
  recovery on, and its private half is wrapped to each holder's personal key. A holder needs the sealed permission
  `VAULT_RECOVERY` and a recent sign-in. Every recovery is an event and a notice to all the vault's members.
- **The recovery key is signed.** Its public half is signed by the holder who made it. A member who turns recovery on
  for a vault checks that holder's key as they check a new member's ([§3.5](#35-the-personal-key)), then signs the
  vault's wrap to the recovery key. A client refuses a recovery key with no signature it can check, so a server cannot
  hand out a recovery key of its own.
- Each holder holds the whole private key. [§3.12](#312-what-the-server-sees-and-the-limits) states that limit, and
  splitting the key is an [open point](#open-points-from-the-security-review).
- A vault shows whether recovery is on, and a person asked to join sees it before they accept.
- An operational vault must have recovery on or at least two members. Making one, and removing a member, refuse to
  leave it with neither.
- The framework keeps the recovery keys and their holders, by instance or by user group. BOSS Admin › Security shows
  them for each organization (V4).

### 3.11 Creating a single-secret vault

A single-secret vault cannot be opened without its password or key by the server, the administrators, the organization
or the people who make the software. The create screen says this before anything is made, in plain words:

> Only this password opens this vault. It is never sent to the server and it is not stored anywhere. Nobody can open
> the vault without it: not an administrator, not your organization, not the makers of this software. If you lose it,
> everything in the vault is lost for good.

Then it says how to keep the password or key safely:

- Keep it in a password manager you already trust that is not on this server, or write it down and keep the paper with
  your important papers (a safe, a safe-deposit box).
- Keep two copies in two places. Never keep the only copy inside this vault, or anywhere on this server.
- A **generated key** is long and random, so it cannot be guessed. Download or print it, then keep it as above.

The screen also checks the choice:

- A password must pass a length and common-password check in the client. It is typed twice.
- For a generated key, the user types the last 8 characters of the printed or saved key.
- A box to tick: "I have stored it outside this server."
- After 30 days, a reminder asks the owner to open the vault once, which shows the password is still kept right.

### 3.12 What the server sees, and the limits

- **What the server sees:**
  - For every vault: its id, kind, lock, owner and members, item counts, sizes and dates.
  - For operational vaults: also the name, item types and bindings.
  - Ciphertext.
- **What it never sees:**
  - A personal item's name, type or value.
  - A key passphrase, a vault password or key, a personal private key, a vault key.
  - An operational value, except inside a lease and only for that lease's items.

The limits, which the vault's About and [Attack-Surface](../docs/security/Attack-Surface-Data.md#threat-model-stored-secrets) state:

- **The web form runs code the server sends.** A compromised server can send code that takes the passphrase, which is
  the limit of every web password manager. The desktop and mobile apps carry their own code and are the strong form.
- **Script in a page of the same origin** while a vault is open can ask the worker for items, as with chat's crypto
  device. Every screen of every app on this server shares the origin, so a script injected into any of them reaches an
  open vault ([open point](#open-points-from-the-security-review)).
- **The server can withhold or roll back ciphertext.** The apps remember the highest revision they have seen of each
  item and warn when a lower one comes back.
- **The server cannot help with personal items:** no server search, no CardDAV, and no restore without the keys. A
  database backup holds ciphertext only. The user's own backup is an encrypted export file, made in the client.
- **Root on a running server during a lease** sees the leased items, which is Sealed-Secrets' limit. It also holds the
  lease key and the machine key, so it can sign a lease for any item and wait for an approver who does not read the
  dialog. A compromised manager gets more from the instances it manages ([§4.9](#49-where-the-manager-runs)).
- **A database writer** with no code on the server cannot point a lease, a wrap, a deposit or a delivery at a key of
  its own: each needs a signature it cannot make. It can still delete rows and put back older ones. The server itself
  has no memory outside its database, so a writer who replaces a manager's signing key and signs the vault's records
  again gets each item the server deposits after that. The next member's client to open the vault finds a key it has
  not checked and refuses, and the items deposited since are rotated.
- **A fingerprint that is accepted without comparing it** gives a server the swap [§3.5](#35-the-personal-key)
  describes. The signatures bind a wrap to a key, and only the check binds the key to a person.
- **One recovery holder with a copy of the database** opens every vault that has recovery on. The `VAULT_RECOVERY`
  permission, the recent sign-in, the event and the notice are rules the server keeps, not cryptography. For a
  personal shared-key vault with recovery on, that holder is an administrator who can open it.
- **A removed member who kept the vault key** and later gets a copy of the database reads each item whose item key has
  not changed since. The server can also put back a removed member's wrap or an older key version, and a browser with
  no memory cannot tell. Rotating the credentials at their source ([§4.7](#47-rotation)) is the remedy for an
  operational vault; a personal shared-key vault has none yet ([open point](#open-points-from-the-security-review)).
- **A database thief can guess passwords offline** against the Argon2id wraps. That is why there are the strength checks
  and the generated-key option.

### 3.13 Events and the access report

A `VaultEvent` row is written for each of these:

- a vault made or deleted;
- a wrapped key fetched;
- a member added or removed, and a key rotation;
- a recovery;
- a lease asked for, approved, used, ended or revoked;
- machine access given, used or revoked;
- a reveal;
- a delivery sent or filed.

A lease does not open when its event cannot be written (fail closed, as [Sealed-Secrets §3.10](Sealed-Secrets.md#310-who-has-access-to-what)
has it for `audit="required"`). Sealed-Secrets' access report (S13) reads these rows: who can approve leases on an
environment's vault, who did and for which run, and where machine access is held.

## 4. Instances

### 4.1 The model

The `moqui.server.instance` tables are reshaped. Before 0.1.0 there is no migration.

| Entity | Change |
|---|---|
| `InstanceHost` | Kept: a Docker or Podman host (Unix socket, HTTP, HTTPS with TLS). Later host types: SSH with systemd for an archive or a `.deb` ([Debian-Packaging](Debian-Packaging.md)), Quadlet (`podman/quadlet/`), Kubernetes. Its password and TLS key become vault item bindings in the vault it names. A host that serves a runner pool needs machine access for them, since CI runs unattended |
| `InstanceHostType` | Kept: the services by name are the driver contract |
| `InstanceKind` | New, in place of `InstanceImageType` and `DatabaseType`: `mr-moqui`, `postgres`, `mysql`, `opensearch`. It keeps the template rows (env, volume, host config). Its services: provision, configure, start, stop, check, backup, upgrade, `rotate#Credential`, `create#Database`, `check#Database` and remove. `InstanceKindSecret` lists the items it needs, by type and binding. Its dependencies: what an instance of it needs (a `mr-moqui` instance needs a database, unless it uses embedded Postgres, and may use search) |
| `InstanceImage` | Kept: an image of a kind, with its registry. The registry login becomes a vault item. MR Moqui's image is the one `podman/build.sh` makes, with one data volume, in place of Moqui Java's images and paths |
| `DatabaseHost` | Kept: a database server that is not managed here (a cloud database, a host's own Postgres). It names a kind, whose `create#Database` serves it. The admin login becomes a vault item |
| `AppInstance` | Gains `instanceKindId`, `environmentId`, `clusterId`, the version (image digest and component commits), a desired state (running, stopped, removed), the observed state and seal state with when each was last checked, and the link state. `AppInstanceEnv` keeps settings that are not secret. `AppInstanceSecret` maps a setting to an item binding. `AppInstanceLink` becomes `AppInstanceDependency`: an instance or a database host, with its role (database, content database, search) |
| `InstanceEnvironment` | New: production, staging, demo. Its instances and clusters, its operational vault, its approvers, its required pipeline (the gate), its machine access, its unseal policy ([§4.5](#45-sealed-start-of-a-managed-instance)), its transport (link, SSH or both) and its lease time limit. `GitCiEnvironment` names one (`instanceEnvironmentId`) for a deploy to managed instances. Its `targetUrl` and `targetToken` stay for a target that is not managed here, and the token becomes a vault item |
| `InstanceCluster` | New: the instances that are members of one cluster, for rolling deploys. Their `moqui.server.ClusterMember` rows come back over the link |
| `InstanceAction` | New: each change to an instance (provision, deploy, upgrade, start, stop, restart, drain, rotate, unseal, remove), with its state, lease, `GitDeployment` when there is one, who did it, the versions before and after, and a report |

A reconcile job compares each instance's desired state with what its host and its link report. It writes the observed
state and raises the alerts of [§4.8](#48-monitor). It changes nothing by itself.

### 4.2 One driver; secrets as files

- Instances move onto the Rust Docker and Podman driver that CI uses (`crates/moqui-git/src/ci/driver/docker.rs`). It
  gains create, start, stop, inspect, logs and remove for containers that stay, with volumes and networks. The JS
  services `init#InstanceDocker` and the rest are removed, and the `InstanceHostType` service names stay the
  contract. M1 decides where the shared driver lives, by the Crate-Split rules.
- **No secret goes into a container's environment, labels or command.** A secret is a file:
  - **On a Podman host:** Podman secrets.
  - **For a third-party image on Docker:** a file copied into the created container before it starts, and read through
    the image's `*_FILE` settings (`POSTGRES_PASSWORD_FILE`, `MYSQL_ROOT_PASSWORD_FILE`). It is on the host's disk
    only while the container exists.
  - **For MR Moqui:** the bootstrap bundle over the manager link ([§4.4](#44-the-manager-link)), so no secret is in its
    container at all.

  M1 checks each mechanism on both hosts.
- The only start value that comes near a secret is a managed MR Moqui instance's one-time pairing token. It works once,
  within 15 minutes, for that instance only. It is given as a file too, by the mechanisms above, since `docker inspect`
  shows a command as it shows the environment.
- This rule is for instances. A CI job's secrets are environment variables of its container today
  ([§2](#2-what-is-there-now)); whether they become files is an [open point](#open-points-from-the-security-review).

### 4.3 Provisioning

1. **Choose.** The operator picks the kind and version, the host, the environment, a cluster if any, and the
   dependencies. A database can be a new Postgres or MySQL instance, a database on a database host, or embedded
   Postgres. Search can be a new OpenSearch instance, embedded OpenSearch (`--embed-opensearch`, [Build-Test](../docs/Build-Test.md)),
   or Postgres search.
2. **Preview.** The screen shows what will be made: containers, volumes, networks, databases and users, vault items and
   host names. It also shows the lease the provision needs: the host's TLS key, the database admin login and the
   registry login.
3. **Approve.** A member approves the lease, or the environment has machine access.
4. **The manager does the work:**
   1. It generates the new credentials (the database user's password, the search user's, the instance's ingest key) and
      deposits them as vault items. Writing them needs no lease.
   2. It creates the database and user with the admin login.
   3. It makes the volumes, network and container, with secrets as files.
   4. It starts the container and checks it.
5. **A new MR Moqui instance** dials the manager with its pairing token, takes its bootstrap bundle, makes its seal
   ([§4.5](#45-sealed-start-of-a-managed-instance)), loads its data as its first start does
   ([Run-Deploy](../docs/Run-Deploy.md#data-and-the-first-start)) and reports ready.
6. **Register.** Monitor's identity, ingest key and scrape targets are set ([§4.8](#48-monitor)). The action is
   recorded, with an annotation.

### 4.4 The manager link

The user's answer: a link like the edge link, with SSH as a fallback, or as the mode an environment prefers.

- **Transport.** The managed instance dials the manager's HTTPS port with an ALPN of its own (`moqui-manage/1`) and keeps
  one link open, with gRPC both ways on HTTP/2. PINGs, redial with backoff and generations work as in `host/edge_link/`,
  and M2 decides how much of that code the two links share.
- **Pairing.** At provision, the pairing token does it, given to the new container as a file
  ([§4.2](#42-one-driver-secrets-as-files)). An instance the manager did not make (an existing server being adopted)
  pairs with a code made in System › Instances, as an edge client pairs. Both link keys are then pinned, by SHA-256 of
  the public key. At pairing the instance also takes and pins the signing keys of its environment's approvers
  ([§3.5](#35-the-personal-key)), which it uses to check its recovery holders' keys
  ([§4.5](#45-sealed-start-of-a-managed-instance)). A changed set of approvers reaches the instance signed by an
  approver it has pinned.
- **A manager behind a firewall** is reached through its own edge node, by a passthrough host name, the way a local
  node's web access is. The manager still opens no inbound port.
- **While the instance is sealed**, dialing out opens no listener ([Sealed-Secrets §3.3](Sealed-Secrets.md#33-sealed-start)).
  Only three calls are allowed then: Hello and status, the bootstrap bundle on a first start, and unseal. Every other
  call needs the instance unsealed.
- **Manager to instance:**
  - status: version, seal state, schema hash, cluster members, health;
  - seal; restart; drain and undrain;
  - put a secret (`operator secret put`, Sealed-Secrets S4);
  - deploy and update a component (Component-Deploy's services, as the link's caller);
  - export the seal bundle;
  - the surface report (S12).
- **Instance to manager:**
  - unseal by transit;
  - events: started, sealed, unsealed, failed;
  - its recovery shares, sealed to their holders.
- **Authorization.** On the manager, each call is checked against the instance's environment and the caller's
  permission. On the instance, link calls run as one system identity that may run only the commands listed here.
  Nothing on the link runs an arbitrary service.
- **SSH.** The manager can run the same operator commands over SSH instead: the instance's SSH admin shell (`moqui>`,
  [TUI](../docs/ui/TUI.md)), or the host's own sshd running `moqui operator …`. The SSH key is a vault item. An
  environment chooses link, SSH or both.
- **The link key on the instance** must work while the instance is sealed, so it cannot be under the root key. It sits
  in `runtime/` with mode 0600, or in `systemd-creds` on a host with a TPM. With it, the instance can ask the manager
  only for what a sealed instance may ask ([§4.5](#45-sealed-start-of-a-managed-instance)).
- **The limit of that key.** Without a TPM it is a usable key on disk, an exception to Sealed-Secrets' goal that the
  plan names. Under the automatic unseal policy, a stolen disk with the link key and `seal.json` gets the root key from
  the manager until someone revokes the instance. The manager alerts on an unseal from an address the instance has not
  used before ([§4.8](#48-monitor)), and production uses the approval policy or a TPM-bound link key.

### 4.5 Sealed start of a managed instance

Sealed-Secrets S3 and S11 make the seal; this section is its deployment side.

- **The unseal method.** On its first start, the instance runs `moqui operator init` with the method its environment
  names:
  - `transit` against the manager, the default for a container, which usually has no TPM;
  - `tpm` on an SSH host type with one;
  - `passphrase` or `shamir` for the highest care. The passphrase is then a vault item used under a lease. Shares are
    never typed into the manager: a holder enters a share on the instance's operator socket, or runs `moqui vault` on
    their own machine, which seals the share to a key the sealed instance makes for that unseal and signs with its
    link key. The manager relays the sealed share over the link and cannot open it
    ([Sealed-Secrets §6](Sealed-Secrets.md#6-decisions): never HTTP).
- **Transit by the manager.** The manager keeps one transit key for each instance, in its own keyring. It is a platform
  secret, because an unattended unseal needs it. Unsealing works like this:
  1. The instance sends its wrapped root key over the link, with an ephemeral public key.
  2. The manager unwraps the root key with that instance's transit key.
  3. The manager returns the root key sealed to the ephemeral key.

  As with every transit seal, the manager sees the root key during the unwrap.
- **The unseal policy** (per environment):
  - automatic: the manager answers while the link key is pinned and the instance is not revoked;
  - approval: each unseal waits for a member's approval, for production, where an unexpected restart deserves a look.

  Either way, each unseal is a Monitor event. A stolen disk and its link key can only ask the manager, which can
  refuse, require approval, or revoke. Under the automatic policy the manager answers it until the instance is revoked
  ([§4.4](#44-the-manager-link), the limit of the link key). The policy is a rule the manager keeps: a compromised
  manager holds the transit keys whatever the policy says ([§4.9](#49-where-the-manager-runs)).
- **Recovery shares.** Sealed-Secrets makes them at init, 3 of 5 by default. The environment names the holders.
  1. The instance takes each holder's public keys over the link, each with a key check (`VaultKeyCheck`) signed by one
     of the environment's approvers. It verifies that signature with the approvers' signing keys it pinned at pairing
     ([§4.4](#44-the-manager-link)) and refuses a holder's key without one. Nobody is present on the instance to
     compare a fingerprint, so the approver's check stands in for it.
  2. It seals each share to one holder and sends it as a delivery, signed with its link key.
  3. The holder's client checks the delivery's signature against the instance's pinned link key, files the share into
     their personal vault ("Recovery share 2 of 5, prod-1") and sends back a confirmation signed with the holder's
     signing key.
  4. Init completes when every share is confirmed or printed. An unconfirmed share is reported, and the instance
     makes a new set when a holder is replaced.

  No place holds two shares, and the manager sees none: it cannot put a key of its own in a holder's place without an
  approver's signature. A holder with no personal key gets their share on the operator socket, printed, as
  Sealed-Secrets has it. What a manager that was already compromised at pairing can do is in
  [§4.9](#49-where-the-manager-runs).
- **The recovery bundle** (`export-seal`) goes to the manager over the link. It is kept as an operational item beside
  the instance's backups, and it holds only wrapped values.
- **Bootstrap secrets.** The bootstrap bundle carries the instance's database password and the other secrets it needs
  before its database is open. It is sealed to the instance's link key. The instance seals them into its `seal.json`.
  Before Sealed-Secrets S4, the bundle writes them to the instance's `runtime/.env` (mode 0600), as the generator
  does today.
- **A managed instance whose manager is gone** stays sealed. Its holders' shares unseal it, and `rekey` moves it to
  another method.

### 4.6 Deploys

This builds Dogfood §8.6 and DG10 for managed instances.

What is built today is the deployment of a component repository to a target that is not managed here
([§2](#2-what-is-there-now)): `request#GitDeployment`, the environment's pipeline as the gate, `decide#GitDeployment`,
and an attestation signed with `targetToken` and posted to the target's `deploy#AttestedComponent`. M3 keeps the
request, the gate and the decision, and changes what follows them:

- **A `GitCiEnvironment` that names an `InstanceEnvironment`** deploys by the steps below, over the manager link or
  SSH. No attestation is posted, and the managed instance needs no `component_deploy_attestation_key`: the link's
  pinned keys name the caller, and the manager checks the passed run itself. `decide#GitDeployment` is the approval
  of step 3, so one decision approves the deployment and opens its lease.
- **A target that is not managed here** keeps the attested call. Its `targetToken` becomes an operational vault item
  ([§4.1](#41-the-model)), so signing an attestation needs the deployment's lease or the environment's machine access.
  The token is a secret both ends share, which signs but does not encrypt, and the target deploys with no second
  person: its gate is the signature and the passed run it names.

1. **Trigger.** A deploy event from a pipeline (`GitDeployment`), or one made by hand: a new image version, or a
   component update.
2. **Gate.** Without a passed run of the environment's required pipeline for the commit, the deploy is refused.
3. **Approval.** Approving opens the lease, unless the environment has machine access.
4. **Plan.** For each instance: the image digest and component commits before and after, and whether the schema hash
   changes.
5. **Back up first.** The kind's backup service runs before a change that touches the database.
6. **Roll.** One cluster member at a time:
   1. drain it (`InstanceDrain`, the member drain of [Edge-Node2](Edge-Node2.md));
   2. change it: a new container from the new image with the same volumes and secrets, or Component-Deploy's update
      over the link;
   3. wait until it reports healthy and unsealed;
   4. undrain it.

   A single instance is replaced in place. Behind a terminate host, the edge node's offline page covers the gap.
7. **Mark.** The deploy posts its own annotations (service, instance, version, revision). Monitor's Releases compares
   the new version, and a regression goes to recovery ([§4.8](#48-monitor)).
8. **Rollback** goes to the image digest and component commits recorded on the `InstanceAction`. It is refused when
   the schema hash differs, as Monitor's rollback is.

For Postgres, MySQL and OpenSearch, a minor version is a container replacement. A major version upgrade is the kind's
own procedure, and comes later.

### 4.7 Rotation

- An operational item may have a rotation age. Rotating needs a lease or machine access, and runs in this order:
  1. The kind's `rotate#Credential` makes a new value and sets it at the source (Postgres `ALTER ROLE … PASSWORD`,
     MySQL `ALTER USER`, OpenSearch's security API).
  2. It puts the value on each instance that uses it, over the link or SSH, then reloads or restarts as the kind says.
  3. It checks the instances.
  4. It writes the new item revision. The old one stays in `VaultItemHistory` for a rollback.

  When the check fails, the old value is put back at the source.
- **When:**
  - at the policy age;
  - on removing a member (offered, [§3.9](#39-members-removal-and-rotation));
  - after an incident: the manager lists each item of an environment and rotates all that have a rotate service
    ([Sealed-Secrets §3.12](Sealed-Secrets.md#312-incident-response)).
- A credential the manager cannot change (an outside provider's) shows as due, with what to do by hand.

### 4.8 Monitor

- **Identity.** At provision, the manager sets each instance's `service.instance.id` to its `AppInstance` id, sets
  `deployment.environment`, sets `service.version` at each deploy, and points its OTLP export at the manager's Monitor
  with an ingest key (a vault item). Monitor's Services, Releases and regressions then line up with the fleet.
- **The fleet screen.** Environments and their instances, each with its kind, version, health (the kind's check and how
  fresh its telemetry is), seal and link state, cluster, last deploy, open incidents and items past their rotation
  age. It links to Monitor › Services and Releases, narrowed to the instance.
- **Postgres, MySQL and OpenSearch** report through scrape targets that the manager adds at provision
  (`ScrapeTarget`): their exporters, or OpenSearch's stats.
- **Shipped alert rules:**
  - an instance sealed longer than a set time;
  - a link down;
  - an unseal;
  - an unseal from an address the instance has not used before;
  - a failing check;
  - a failed deploy;
  - an item past its rotation age;
  - machine access used outside a deploy.
- **Recovery.** A new `RecoveryAdapter.adapterType`, `instance`, runs restart, isolate (drain), pause-rollout, promote
  and rollback through the manager's own actions, not a webhook to an outside controller. A step that needs a lease
  waits for one. In Monitor's approval mode, Approve (`MONITOR_RECOVER`) opens the lease as well. Automatic mode needs
  the environment's machine access.
- **Clusters.** A managed cluster's `ClusterMember` rows come back over the link, so Monitor's comparison of one
  instance against the rest works on them.

### 4.9 Where the manager runs

- The manager is framework code (System › Instances, the services, the link), so any instance can manage others. A
  small business can run it inside its BOSS instance, as the all-in-one node of Sealed-Secrets §1 does.
- For a fleet in production, run it as an ops instance of its own, in the inner zone and without the business apps.
  It holds every managed instance's transit key and the deposit keys of their vaults, so it should hold as little else
  as it can.
- It seals with `tpm`, `kms`, `passphrase` or `shamir` ([Sealed-Secrets §3.3](Sealed-Secrets.md#33-sealed-start)), and
  never by transit to an instance it manages.
- **What a compromised manager gets.** More than its leases. It sees each managed instance's root key at that
  instance's next unseal, and it can unwrap the root key in any recovery bundle it reads, since it holds the transit
  key. With a root key and that instance's database or backup it reads every bootstrap secret and every row, with no
  lease and whatever the unseal policy says. "A compromised server gets that lease's items and no others"
  ([§3.7](#37-leases-operational-items-used-on-the-server)) is true of vault items and not of managed instances. The
  recovery bundle is deposited ([§3.2](#32-keys-and-formats)), so the manager does not read it back without a lease.
  Where an instance's backups are kept, and whether the transit key needs a lease under the approval policy, are
  [open points](#open-points-from-the-security-review).
- **A manager that is compromised when an instance pairs** gives that instance approvers' signing keys of its own
  choice, and then gets its recovery shares. The holders' confirmations ([§4.5](#45-sealed-start-of-a-managed-instance))
  show a holder who received nothing. A manager compromised later cannot change the pinned approvers.
- **Zones.** The manager is in the inner zone, and it may manage outer instances (PASS, edge nodes). Their transit keys
  open only their own root keys, so an outer instance gets no inner key ([Sealed-Secrets §1](Sealed-Secrets.md#1-deployment-zones)).

## 5. Screens

**System app** (framework, `tools/screen/System/`):

- **System › Instances** (from Instance): Environments, Instances (the fleet), Hosts, Images, Database hosts and Kinds.
  An instance's page shows its state, link, seal, dependencies, secret bindings (never values), actions and versions,
  with Deploy, Rotate, Restart, Drain and Remove. Each of these asks for a lease when it needs one.
- **System › Vaults:** operational vaults, their members and roles, items (names, bindings, types, revisions, rotation
  age; no values), leases (waiting, open, ended), the lease key's fingerprint, machine access, recovery keys and
  holders, and events.
- **The lease dialog** opens from a notification, a deploy's page, an instance's page or a Monitor incident. It lists
  what the lease names and asks for the key passphrase when the personal key is locked.
- **Vault key** in the user's settings: set up the personal key and its recovery code, change the passphrase, see the
  fingerprint, the keys this user has checked, and the devices that remember the key.

**Commons** (mr-boss, `boss/screen/Commons/`):

- **Commons › Vault:** the user's vaults of both locks, items by type, search, an item's page (show, copy, history),
  making a vault (choosing its lock, with [§3.11](#311-creating-a-single-secret-vault)'s steps for single-secret),
  members with fingerprints, incoming deliveries, import, encrypted export, and Lock.
- **Commons › Contacts** shows private contacts while the vault is open, marked as such.

**Desktop and mobile apps:** the vault in the shell with the native crate, and device memory (V5).

## 6. Phases

Each phase is done when its regression tests pass, `./moq test-framework` passes (`./moq test-apps` for the Commons
phases), and [Attack-Surface](../docs/security/Attack-Surface.md) says what changed.

| Phase | Work | Tests (suite) |
|---|---|---|
| **V0** | The vault crate (`crates/moqui-vault`, by the Crate-Split rules): the formats, Argon2id and HKDF, the HPKE wraps, Ed25519 signatures on wraps and leases, item AEAD with associated data (an operational item's type, binding and name in it), the lease seal, the machine-access seal, zeroized key types; the native build and a small wasm build for the browser | Known-answer vectors give the same bytes natively and in wasm; a wrong password or key fails; a wrapped key or body moved to another item, vault or version fails; an operational item key unwrapped under another binding, type or name fails; a wrap or a lease with a missing or wrong signature fails; KDF parameters under the floor or over the ceiling are refused, and parameters under the target open and are wrapped again (regression: the crate's tests, and a client spec that loads the wasm module) |
| **V1** | Framework tables and services (`moqui.security.vault`): personal keys with both key pairs, key checks, vaults of both locks, items with history, members with signed wraps and signed key versions, signed deliveries, events; what the server serves and to whom | No row of a personal vault holds an item's name, type or value in the clear; a non-member gets no wrap and no body; a delivery goes only to its recipient; a removed member has no wrap, and a write at the old key version is refused; a client refuses a wrap or a key version that no member with the manage role signed, and a delivery whose signature is wrong; nothing is wrapped to a changed personal key before it is checked again; each change writes its event (regression) |
| **V2** | Operational vaults: the signed deposit key, the lease key and signed leases (the ephemeral key, approval with sealed item keys, use, end, revoke), reveal by role, machine access per environment under the vault crate's AEAD, the lease dialog, `moqui vault approve` | A lease opens only the items it names, and nothing it held opens after it ends; an approval without the approve role or a recent sign-in is refused; a lease whose event cannot be written does not open; a lease approved through one cluster member runs on another; machine access opens only its items for its environment, and its sealed keys moved to another environment's row do not open; a client refuses a lease whose public key or item list was changed after it was signed, and one signed by a lease key its user has not checked; a binding pointed at another item fails at the unwrap; the server refuses a deposit key with no manager's signature; a leased value is not in a job's stored parameters; a restart ends the member's leases (regression) |
| **V3** | Commons › Vault (mr-boss): the module in a worker, item types and forms, single-secret creation with §3.11's steps, shared-key vaults with members and fingerprints, deliveries, private contacts in Contacts, import from Bitwarden and KeePass exports in the client, encrypted export, lock on idle | A single-secret vault refuses a member and a recovery key; a private contact never reaches CardDAV (regression, mr-boss); a value shows only when asked for, and the page locks when idle (one client spec). `VaultPersonal`: both locks, a share, a removal, a recovery (acceptance) |
| **V4** | Recovery keys and holders; recovery on shared-key vaults; BOSS Admin › Security | A holder recovers a vault to a member's new personal key, and every member is told; a single-secret vault never gets a recovery wrap; recovery without `VAULT_RECOVERY` or a recent sign-in is refused; an operational vault cannot drop to one member with recovery off (regression) |
| **V5** | The vault in the desktop and mobile shells, with the native crate; device memory in the OS key store; the passkey slot once passkeys are built | Device memory ends at its time limit and when the device is revoked (regression, the shells' crates). Hand checks in [Mobile-Desktop-Exit-Checks](Mobile-Desktop-Exit-Checks.md) |
| **M1** | Instances on one driver: the Rust driver for containers that stay; `InstanceKind` for `mr-moqui`, `postgres`, `mysql` and `opensearch`; the reshaped tables; secrets as bindings and files; provisioning under a lease; MR Moqui's own image and volume; System › Instances | Against the fake host type: a provision deposits its items and puts no secret in any container's environment, labels or command; a provision without a lease or machine access is refused; dependencies are made first (regression). Against real Docker and Podman: opt-in, as CI's driver tests are |
| **M2** | The manager link: pairing at provision (the token as a file) and by code, the approvers' signing keys pinned at pairing, the calls, the bootstrap bundle (to `runtime/.env` until Sealed-Secrets S4, then to `seal.json` and `SecretValue`); SSH as the other transport | An unpinned key is refused; a pairing token works once and is in no container's command, environment or labels; a sealed instance can make only the three sealed calls; a call not on the list is refused on the instance (regression, with a two-runtime link test as `edge_link`'s) |
| **M3** | Environments and deploys (Dogfood DG10): `InstanceEnvironment` and `GitCiEnvironment`'s link to it, the deploy event, the gate, approval with its lease, rolling deploys with drain, rollback to recorded versions, annotations | The gate refuses a commit with no passed run; a deploy uses only its lease's items; members change one at a time, each drained; a rollback across a schema hash change is refused (regression). `InstanceDeploy` on a real Podman host: provision Postgres and MR Moqui, deploy a component and an image, roll back (acceptance) |
| **M4** | Monitor: the instance identity, the fleet screen, scrape targets per kind, the shipped alert rules, the `instance` recovery adapter | A provision sets the instance id and an ingest key; the adapter's restart and rollback take the deploy path and wait for a lease in approval mode; a sealed instance raises its alert (regression) |
| **M5** | Rotation: rotation ages, `rotate#Credential` per kind, the put over the link or SSH, item history, rotation on member removal, the incident list | A rotation changes the source, then the instances, then the item, and puts the old value back when the check fails; an item past its age shows on the fleet screen (regression) |
| **M6** | Managed sealed start (needs Sealed-Secrets S3 and S11): transit by the manager, the unseal policy, recovery shares to holders' personal vaults with approver-signed holder keys and holders' confirmations, a share sealed to the instance by `moqui vault`, the recovery bundle as an operational item | Transit unseals an instance only for its pinned link key and not after a revoke; the approval policy waits; each share is sealed to one holder only; a holder's key with no pinned approver's signature is refused; a share sealed for one unseal does not open at another, and the manager's relay holds ciphertext only; an unseal from a new address raises its alert (regression). `ManagedSealedStart`: a restart unseals by transit; with the manager down the instance stays sealed and the alert fires; shares unseal it (acceptance) |

The order:

- V0, V1 and V2 come first.
- M1 needs V2. M2 follows M1, and M3 follows M2.
- M4 and M5 follow M2, and M5 also needs V2.
- M6 follows M2 and Sealed-Secrets S3 and S11.
- V3 needs only V1, so it can run beside the M phases. V4 follows V1, and V5 follows V3.
- Machine access waits for nothing: until Sealed-Secrets S1 its key is derived from today's field pass
  ([§3.8](#38-machine-access)). Whether it waits for S1 instead is an
  [open point](#open-points-from-the-security-review).
- The lease key is a generated secret until Sealed-Secrets S4, then a purpose key of the keyring.

## 7. Tests

- **Regression:** one test for each rule a phase adds, under a second:
  - the crate's own tests, with known-answer vectors;
  - framework specs for the vault and instance services;
  - mr-boss specs for Commons' rules.

  A screen is rendered at most once in each mode, and there is no screen sweep ([Build-Test](../docs/Build-Test.md#rendering-screens-in-tests)).
- **Opt-in:** the driver against a real Docker and a real Podman, as CI's driver tests are.
- **Acceptance:** `VaultPersonal` (mr-boss), `InstanceDeploy` and `ManagedSealedStart` (framework). Each is written with
  its phase and run only when asked, or when it is added or changed.
- **Proof catalog:** each vault rule is a row in [Proof-Tests](../docs/security/Proof-Tests.md), beside Stored secrets.

## 8. Other plans and docs

- **[Sealed-Secrets](Sealed-Secrets.md)** is changed with this plan. It names this plan for:
  - people's secrets;
  - transit by a manager, recovery shares in holders' vaults and the recovery bundle (§3.3);
  - a manager's place in the zones (§1);
  - operational vaults in the access report (§3.10);
  - the managed keys in the inventory (§3.11);
  - a manager in incident response (§3.12);
  - S4 and S11.
- **[Dogfood](Dogfood.md):** M3 builds §8.6 and DG10 for managed instances. Dogfood gets a pointer when M3 starts.
- **[Pipelines](../docs/interfaces/Pipelines.md):** environments, deploy secrets as vault items, and runner hosts'
  credentials under machine access, as M1 and M3 are built.
- **[Attack-Surface](../docs/security/Attack-Surface-Data.md#threat-model-stored-secrets):** vaults and managed instances in the threat model. The rows
  "the server sees every plaintext" and the client-side vault in "What MR Moqui can take" point here once V1 is built.
- **Guides, as each phase is built:**
  - [Run-Deploy](../docs/Run-Deploy.md): a Managed instances section (M1, M2);
  - [Monitoring](../docs/tools/Monitoring.md): the fleet screen and the `instance` adapter (M4);
  - [Component-Deploy](../docs/tools/Component-Deploy.md): a deploy over the link (M3);
  - a new guide for vaults.
- **[Remaining-Work](Remaining-Work.md):** items leave it as they are built.

## 9. Decisions

| Question | Choice | Why |
|---|---|---|
| Access to operational vaults | Shared-key only: each member's personal key wraps the vault key | The user's answer, Nov 14, 2026. Each member has access of their own, so one leaves without the others changing anything, and every lease is tied to a person |
| Locking a personal vault | Single-secret (one password or key) or shared-key, chosen when it is made and fixed | The user's answer, Nov 14, 2026 |
| Key passphrase | Independent of the sign-in password, never sent to the server | The user's answer, Nov 14, 2026. The sign-in password goes to the server, and many users sign in by SSO, keys or app passwords |
| Storage | Encrypted per item, one `VaultItem` row per item, many to one with `Vault` | The user's answer, Nov 14, 2026. Two devices editing two items do not conflict, a lease carries one item, and a rotation rewraps keys, not bodies |
| Machine access | Allowed per environment | The user's answer, Nov 14, 2026: some will want or need unattended deploys |
| Organization recovery | Yes for shared-key vaults; never for single-secret vaults, whose create screen explains how to keep the key outside the server | The user's answer, Nov 14, 2026: a single-secret vault stays opaque to the server and its administrators |
| Manager to instance | A link like the edge link; SSH as a fallback or the mode an environment prefers | The user's answer, Nov 14, 2026 |
| Operational values in a client | Never in normal use. The client unwraps only item keys, sealed to a lease; a reveal is by role and logged | The server needs the values, and a browser that also holds them adds exposure. The member's passphrase, personal key and vault key never reach the server |
| Item keys | One per item, wrapped by the vault key | Least privilege for leases; cheap rotation; one item can be sent |
| The deposit key | Operational vaults only | The server makes credentials while provisioning without opening the vault. Personal vaults have none, so the server cannot plant an item in them |
| Crypto code | One crate with no I/O, native and wasm | One implementation, the same bytes everywhere, tested with cargo |
| Public-key wraps | HPKE (RFC 9180) with X25519 | A standard construction, not a home-made one |
| Argon2id | At least 64 MiB, t=3 and p=1, raised on a later open | Phones must open a vault; Sealed-Secrets' 256 MiB is for a server's passphrase |
| Secrets to containers | Files, never environment, labels or command | `docker inspect` and `/proc/*/environ` show the environment |
| Where the manager runs | Framework code; a separate ops instance for a production fleet | It holds every managed instance's transit key |
| A sender for every wrap | A personal key is two key pairs (X25519 to receive, Ed25519 to sign). Each member wrap, key version, deposit key, recovery key and delivery is signed by its maker, and a client accepts a key version only from a signer with the manage role | Security review, 2026-11-15: HPKE base mode names no sender, so a server or a database writer could wrap a vault key, a deposit key or a recovery key of its own. HPKE stays |
| Key checks | A `VaultKeyCheck` row signed by the checker; a changed key is checked again before anything is wrapped to it | Security review, 2026-11-15: an X25519 key cannot sign and no table held the check, and a new personal key was the path for a server to swap a key |
| The lease | Signed by the server's lease key, which members check once; it names each item's id, type, binding and name, and those are in the item's associated data | Security review, 2026-11-15: a database writer could replace a lease's public key, or point a harmless binding at another item, and read what the approver sealed |
| Leased values | Never a parameter of a queued or scheduled call, never a log or event field; a restart ends the lease | Security review, 2026-11-15: "nothing stored can open them again" was about keys, not the values an operation passes on |
| Machine access storage | The vault crate's AEAD with the environment, vault and item ids as associated data, in a column the entity facade never decrypts; the key from today's field pass by HKDF until S1 | Security review, 2026-11-15: an entity `encrypt="true"` field has no associated data, so a row moved to another environment would open, and every find and export would hold the item keys in the clear |
| Argon2id numbers | A floor that is refused, a target that is wrapped again to, a ceiling that is refused | Security review, 2026-11-15: "under the minimum wraps again" and "under the minimum is refused" could not both hold |
| Recovery shares of a managed instance | Holders' keys carry an approver's signed check, and the instance pins the approvers' signing keys at pairing; holders confirm; a share is entered on the operator socket or sealed to the instance by `moqui vault`, never typed into the manager | Security review, 2026-11-15: the manager chose the holders' keys, and a share typed into its screen crossed HTTP and was seen by it, against Sealed-Secrets §6 |
| The pairing token | A file, as the other secrets are | Security review, 2026-11-15: a start command is as visible in `docker inspect` as the environment |
| Deploys to a managed instance | The request, gate and decision of the built `GitDeployment` path, then the roll over the link in place of the attested call | Security review, 2026-11-15: §2 did not know the attested deploy was built. One decision approves the deployment and opens its lease |
| The manager's own seal | `tpm`, `kms`, `passphrase` or `shamir` | Sealed-Secrets §3.3 has `kms` for cloud hosts, and this plan left it out |
| Splitting the recovery key (open point) | Split k of n across the holders with the Shamir code of Sealed-Secrets S3, so a recovery takes k holders' clients; recovery off by default for a personal shared-key vault, on by default for an operational one | The user's answer, Nov 19, 2026 |
| HPKE beside age (open point) | HPKE for vault wraps; age stays for files | The user's answer, Nov 19, 2026 |
| Machine access before S1 (open point) | As §3.8: waits for nothing, its key by HKDF from `entity-facade@crypt-pass` | The user's answer, Nov 19, 2026 |
| Vault screens' origin (open point) | One origin, with the limit of §3.12 | The user's answer, Nov 19, 2026 |
| Fetching a wrapped personal key (open point) | Only a strong sign-in (a password and second factor, SSO, or a paired device's key; never an app password, an API key or a scoped key), and a recent one | The user's answer, Nov 19, 2026 |
| Vault-key rotation on removal (open point) | All four: a compare-and-swap on `Vault.revision` for the whole change; the previous version's wraps kept until each member has opened the new one; a new item key at an item's first write after a key-version change, with an offer at removal to re-encrypt every body now; clients with memory keep the highest key version and a hash of the signed member list | The user's answer, Nov 19, 2026 |
| The transit key and backups (open point) | As written, with the limit of §4.9 | The user's answer, Nov 19, 2026 |
| CI job secrets (open point) | Environment variables, as now: a CI job's container ends with the job | The user's answer, Nov 19, 2026 |
| Sealed-Secrets with this plan | S3, S4 and S11 are built with it; S1 is not | The user's answer, Nov 19, 2026 |

## Open points from the security review

The security review of 2026-11-15 found these. The owner decided each on Nov 19, 2026; the choices are rows of
[§9](#9-decisions).

| Point | What the review found | Options |
|---|---|---|
| Splitting the organization recovery key, and recovery by default (§3.10, §1) | The recovery private key is wrapped whole to each holder. One holder who also reads the database opens every vault with recovery on; the permission, the recent sign-in and the notice are server rules. For a personal shared-key vault this is against §1's "its administrators cannot open it" | (a) Keep one holder is enough, with the limit stated in §3.12. (b) Split the private key k of n across the holders, with the Shamir code Sealed-Secrets S3 needs, so a recovery takes k holders' clients. (c) With either, recovery off by default for a personal shared-key vault and on by default for an operational one |
| HPKE beside the age recipient wraps (§3.2) | `crates/moqui-lib/src/age.rs` already wraps a key to an X25519 recipient (X25519, HKDF, ChaCha20-Poly1305). HPKE is a second public-key construction, and no HPKE crate is in the workspace | (a) HPKE for vaults and age for files, two constructions. (b) The age recipient wrap for vault wraps too, with the names in the HKDF info: one construction, not RFC 9180. (c) HPKE for both, with age kept only as a file format |
| Machine access before Sealed-Secrets S1 (§3.8, §6) | Until S1 the machine key comes from `entity-facade@crypt-pass`, which is in `.env`. §3.8 now seals the item keys with the vault crate's AEAD under a key derived from that pass, which keeps them out of finds and exports and binds them to their environment, but a reader of `.env` and the database still opens them | (a) As §3.8 has it: machine access waits for nothing. (b) Machine access needs S1, and V2 ships leases only |
| Vault screens on their own origin (§3.6, §3.12) | Every screen of every app shares the origin, so a script injection anywhere on the server reaches an open vault's worker | (a) Keep one origin and the limit of §3.12. (b) Serve Commons › Vault and the lease dialog from their own host name, with their own session hand-off, which costs a second certificate name and a sign-in step |
| Which sign-ins may fetch a wrapped personal key (§3.5) | "An online guess needs a sign-in first" does not say which kind. An app password, a login key or a stolen device key would fetch the wrapped key for an offline guess | (a) Any session of the user. (b) Only a password and second factor, or SSO, session and a paired device's key, never an app password, an API key or a scoped key (Sealed-Secrets §3.6, credential scope). (c) As (b), with a recent sign-in as well |
| Vault-key rotation on removal (§3.9, `VaultItemHistory`) | Nothing says the server checks `Vault.revision` and `keyVersion` when a rotation commits, so two managers can interleave. A remover can commit bad wraps for some members. Item keys do not change, so a removed member with a later database copy reads items not re-keyed since. The server can put back an old key version | (a) A compare-and-swap on `Vault.revision` for the whole change. (b) The previous version's wraps kept until each member has opened the new one. (c) A new item key at an item's first write after a key-version change, and an offer at removal to re-encrypt every body now. (d) Clients that have memory keep the highest key version and a hash of the signed member list. These can be taken together |
| The transit key and backups on the manager (§4.5, §4.9) | The transit key is a platform secret of the manager, so a compromised manager gets each instance's root key under either unseal policy, and with a backup every row | (a) As written, with the limit of §4.9. (b) Under the approval policy the transit key is an operational item, so each unseal needs a lease and a member's key. (c) With either, a managed instance's backups go to a store the manager cannot read |
| CI job secrets as environment variables (§4.2) | A `GitCiSecret` reaches its job's container as an environment variable. §4.2's rule is for instances | (a) Keep the environment for CI jobs, whose containers end with the job. (b) Files for CI jobs too, when the driver gains them in M1 |

## 10. Not in scope

- Autofill: a browser extension, and system autofill in the desktop and mobile apps. Later.
- Attachments in a vault (files encrypted with the item). Later.
- Server search, CardDAV sync or server-side restore of personal items. Never: they need the plaintext.
- Dynamic credentials made for one lease and dropped after it, as Vault's database engine makes them. Later.
- Kubernetes, AWS and other providers' APIs ([Dogfood §15](Dogfood.md#15-later), D19).
- Major version upgrades of Postgres, MySQL and OpenSearch. Later.

## 11. Inspiration

| From | Idea | Here |
|---|---|---|
| Bitwarden | A user key wrapped by a key from the master password; an organization key wrapped to each member's public key; account recovery through the organization's key; a key per item; fingerprint phrases; Send | The personal key (§3.5), shared-key vaults, the recovery key (§3.10), item keys (§3.2), fingerprints, deliveries |
| 1Password | Vault keys wrapped to members' public keys; the Secret Key, a random value beside the password; service accounts | Shared-key vaults; the generated key of a single-secret vault (§3.11); machine access (§3.8) |
| KeePass | One file opened by one password or a key file | The single-secret lock |
| LUKS | Several key slots wrap one volume key | Members and the recovery key wrap one vault key |
| Bitnami Sealed Secrets | Anyone may encrypt to a controller's public key, and only the controller decrypts | The deposit key (§3.2) |
| HashiCorp Vault | Response wrapping; transit auto-unseal; leases; fail closed when the audit cannot be written | The lease seal (§3.7); transit by the manager (§4.5); leases; `VaultEvent` (§3.13) |
| Edge link | A node that dials out, pinned keys both ways, gRPC both ways | The manager link (§4.4) |
