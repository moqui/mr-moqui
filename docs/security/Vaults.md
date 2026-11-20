# Vaults

A vault holds secrets under a key the server does not hold ([Vault-Instances](../../plans/Vault-Instances.md) has
the design). It covers the crypto every client and the server share, what the server keeps and serves (V1): personal
keys, key checks, vaults of both locks, items, members, deliveries and events, with the `moqui vault` command;
operational vaults (V2): the deposit key, leases, machine access and reveal, with System › Vaults and the lease dialog;
organization recovery split k of n (V4); the browser module and the vault screens' policy; and the vault in the
desktop and mobile apps (V5). Commons › Vault (V3) is mr-boss's, on these services.

## What V1 offers

- **Your personal key.** Each user who shares a vault, or receives a delivery, has one personal key: a key pair to
  receive and one to sign. It is made in your client, under a **key passphrase** that is not your sign-in password and
  never reaches the server, with a **recovery code** to print or save. The server keeps the public halves and the two
  wraps, and no hash of either secret.
- **A strong, recent sign-in to set it up and fetch it.** The server sends your wrapped personal key only to you, and
  only after a sign-in with a password and a second factor, an SSO sign-in, or a request from a paired device (the
  desktop and mobile apps) whose key a strong sign-in made (or a strong re-authentication on it renewed), with a
  recent sign-in on it, within `vault@key-fetch-max-age` seconds (300 by default). An app password, a login, API or
  scoped key, a device's key made after a password alone, or a password alone is never enough: an account with a
  password and no second factor is told to set one up. Setting up a key (`setup#PersonalKey`), a new passphrase
  (`rewrap#PersonalKey`) and a new key (`replace#PersonalKey`) need the same sign-in, so a weak session cannot plant
  a key for someone who has none.
- **Fingerprints.** Before you share a vault with someone, or send them a value, compare their key's fingerprint (six
  pronounceable words) with them by another channel: in person, or on a call. Your client then signs a key check,
  which names the user you compared it with, and stores it; a check vouches for that key as that user's only. Nothing is wrapped to a key you have not checked, and when someone's key changes it is checked again:
  your client refuses, and so does the server, which takes a member's wrap only when the manager who made it has a
  stored check of that member's current key. A fingerprint accepted without comparing it lets a server swap in a key
  of its own (§3.12).
- **Vaults.** A vault is **single-secret** (one password or generated key, its owner only, no members, no recovery)
  or **shared-key** (each member's personal key holds the vault key). A personal vault's name and every item's name,
  type and value are sealed in the client; the server sees ids, sizes, dates, members and roles, and ciphertext.
- **Members.** A personal shared-key vault has manage, write and read; an operational one manage, approve and reveal.
  A manager adds a member with a wrap signed by them and a signed member list. A manager removes a member by rotating
  the vault key: a new key version without that member, every item key rewrapped and every remaining member's wrap
  remade, all in one change that names the vault's revision it was made from, so two managers cannot interleave and
  an item written meanwhile (which also moves the revision) is never overwritten. Demoting a manager, or changing a
  manager's key, remakes in the same change every wrap of this key version that manager signed. The
  removed member's wraps are deleted and a write at the old version is refused; the others keep their earlier wrap
  until they open the new version. Your client accepts a key version only through the chain of member lists, each
  signed by a manager of the one before, back to a signer whose key you checked, and `moqui vault` remembers the
  highest version and member list it accepted, so a server that hands back an older one is refused.
- **Deliveries.** A value sealed to one person's key and signed by its sender. Only the recipient lists and files it,
  and their client checks the signature against a key they checked first.
- **Events.** Every change writes a `VaultEvent` row in its own transaction: a vault made or deleted, a wrapped key
  fetched, a personal key set up, replaced or rewrapped, a member added or removed, a rotation and its acks, an item
  made, changed or deleted, a delivery sent or filed. A vault's members read its events; anyone reads their own.

### Single-secret vaults

Only the password or the generated key opens a single-secret vault. It is never sent to the server and it is not
stored anywhere: not an administrator, not your organization, not the makers of this software can open the vault
without it, and if it is lost, everything in the vault is lost for good. Keep it in a password manager you already
trust that is not on this server, or on paper with your important papers; keep two copies in two places, and never the
only copy inside this vault or anywhere on this server. A generated key is long and random, so it cannot be guessed:
download or print it, then keep it the same way. Commons › Vault (V3) walks through these steps when the vault is made.

A generated key is 256 random bits, so its wrap is HKDF, not Argon2id (`vault_key_generated_wrap`, which makes the key
in the module and answers it once to print, and `vault_key_generated_open`); a password's wrap is Argon2id at the
target. When an open finds a password's wrap under the target it answers a new wrap, which the owner stores with
`update#VaultSecretWrap` (`PUT /rest/s1/vault/vaults/{id}/secretWrap`): a strong, recent sign-in, the floor and
ceiling checked; the same call changes the password or moves the vault to a generated key.

### What a client keeps, and what a rotation carries

- A personal vault's name is its **header**, sealed by the vault key of the current version (`header_seal`,
  `header_open`; the vault's `headerCipher`). A removal's rotation seals it again under the new version
  (`vault_rotate` takes `header_cipher` and answers the new one), and `remove#VaultMember` refuses a rotation of a vault
  with a header that does not carry it.
- An item key is made once and rewrapped at each rotation, so an item row has two versions: `keyVersion`, the version
  its key is wrapped under, and `itemKeyVersion`, the version the key was made at, which a rotation leaves. The
  item's first write after a key-version change makes a new item key (decision (c) of §9): `item_seal` does when given
  `item_key_version` under the current version, and `update#VaultItem` refuses such a write without `newItemKey`.
- `vault_key_forget` drops one opened vault key version, so a page can undo a `vault_rotate` whose server call failed.
- A rename seals the header again (`header_seal` under the current key version) and stores it with
  `update#VaultHeader` (`PUT /rest/s1/vault/vaults/{id}/header`, `{headerCipher, expectedRevision}`): a manager of a
  shared-key vault or a single-secret vault's owner, from the revision they read, ciphertext only, with a
  `VeHeaderChanged` event; `keyVersion`, the version it is sealed under, is refused when it is not the vault's. Two
  renames at once: the last stored wins. An operational vault's name is in the clear and is not changed there.
- `item_summary` answers an item's name, type, URIs, tags and unmasked fields, never a masked field (by its type's
  `masked` names, which the page passes, or a field whose name says it holds a secret) nor its notes: a list view does
  not hold every item's secrets. `copy` clears the clipboard after 30 seconds, and when the page has no focus then,
  when the window has focus again.

## Operational vaults

An operational vault holds the secrets of deployed instances: database passwords, registry logins, TLS keys. Its
members are managers, approvers and revealers (§3.9), each with their own wrap of the vault key, and it keeps
recovery on or at least two members. Making one needs `VAULT_ADMIN`. In normal use no member sees a value and no value
goes to a page: the server uses an item for one operation under a lease a member approves, then forgets it.

### The deposit key

The server makes some credentials itself, while provisioning, and must put them in the vault without being able to
read the vault. A manager makes the vault's **deposit key** in their client: a key pair whose public half is stored
on the vault in the clear, signed by them, and whose private half is wrapped by the vault key
(`set#VaultDepositKey`). The server deposits a new item by wrapping its item key to the public half (`VwDeposit`),
after checking that signature against the vault's signed member list: a deposit key with no manager's signature, or of
an earlier key version, is refused when it is stored and at each deposit. The server can write the item and cannot
read it back. A removal's rotation makes and stores the new version's deposit key. A member's client checks the
deposit key's signature on each open (`get#Vault` gives it), and takes a deposited item in by unwrapping its key with
the deposit key's private half and wrapping it by the vault key (`deposit_adopt`); a lease cannot name an item until
then. A removal never waits on one: its rotation takes every deposited item in on the way (`vault_rotate` with the old
deposit key's wrapped private half).

### Leases

A lease is how the server uses operational items (§3.7):

1. An operation asks for a lease of the items it needs, by binding (`AppInstance:demo:dbPassword`), with its time
   limit: at most `vault@lease-max-hours` (4). The server makes an ephemeral X25519 key pair for it, keeps the
   private half in the memory of the cluster member that runs the operation, and signs the lease with its **lease
   key**: the lease id, the vault and key version, each item's id, type, binding and name, the operation, the cluster
   member, the time limit and the lease's public key. The vault's approvers get a notification (topic
   `VaultLeaseWaiting`).
2. A member with the approve role opens the **lease dialog** (System › Vaults › Lease, from the notification, a deploy's
   page or an instance's page), which shows what the signed lease names, never a column. Their client checks the
   lease's signature against a lease key they checked, opens their personal key with the key passphrase, opens the
   vault key through the member-list chain, unwraps each named item key under the type, binding and name the lease
   gives (so an item that is not the one named does not unwrap), seals them to the lease's key and signs the approval.
   The server needs the approve role and a sign-in in the last five minutes (`approve#VaultLease`), checks its own
   signature on the lease and the approver's, and stores the approval with its event in one transaction, on any
   cluster member; the member that holds the lease is woken by the `vault-lease` cluster topic and also reads the row
   every two seconds. Before it opens the sealed keys, that member checks again that the approver's key is their
   active one and that they hold the approve role at the lease's key version. Two people for a lease: its requester
   does not approve it while another member holds the approve role (the dialog says so); a vault with one approver
   lets that approver approve their own.
3. The operation reads a value by binding: in Rust through the lease registry, in a kind's script service run under
   the lease with `ec.vault.leased(binding)` ([JS-Core-API](../dev/JS-Core-API.md#ecvault)). The member opens the
   sealed item keys with the lease's private key, checks that the item row is the one the lease names, writes a
   `VeLeaseUsed` event in a transaction of its own, checks the lease is still held, and only then decrypts and
   returns the value. When that event cannot be written, nothing is returned. The lease is the thread's while the
   operation runs: every script on that thread under it (a rule a service call fires, a template it renders, another
   service it calls synchronously) can read its items by binding, not only the service the operation called. That is a
   limit: an operation runs code it trusts under its lease.
4. The lease ends when the operation ends, at its time limit (a timer drops it within 30 seconds, read again or
   not), on Revoke (`revoke#VaultLease`, by a manager, an approver, its requester or `VAULT_ADMIN`), and when the
   server stops or seals: its keys are dropped, which zeroes them, and nothing stored can open the sealed keys on its
   row. A restart ends the member's leases: a use after it is refused with "lease ended; ask again", and once the
   cluster has started and named the member, it marks the leases that member had asked for ended (only its own: a
   lease names its cluster member, and another member's leases are that member's).

A lease's operation, and an operational item's binding, name and type, hold no control character (a newline, an
escape): the server refuses one, and `moqui vault approve` and the shell show any it finds escaped, so a name cannot
forge a line of the approval prompt.

A leased value stays in its operation. It is never a parameter of a queued or scheduled call: an async call, a call
registered for the end of a transaction, a job run, a call queued when the service queue is full and a call's tx
rules kept in the outbox (their parameters and results) each refuse a value, or a map key, that holds a secret the
lease handed to the operation on that thread. The secrets it looks for are the item's masked fields, by its type
(`VaultItemTypeField.masked`), or, for a type that registers no fields, the fields named like `password`,
`passphrase`, `secret`, `key`, `token` or `pin`, of at least 8 characters: a host, a user name or a short PIN would
match ordinary text. Another thread's operation keeps its own out. A leased value is never written to a log or an
event.

**The lease key** is an Ed25519 key of the server. Its seed is the generated secret `vault@lease-key`, which the
seal's store keeps as a `SecretValue` (Sealed-Secrets S4, made at its first read); with S1 it becomes the keyring's
`vault-lease` purpose key. Its fingerprint changes once at the upgrade to S4, from a seed that was in `runtime/.env`
to one in the store, and at every boot of an ephemeral runtime whose store does not outlive it: approvers check the
new key once. Each approver checks its fingerprint once (System › Vaults shows it, as does
`get#VaultLeaseKey`), comparing it with the server's operator by another channel, and their client stores the check
as a `VaultKeyCheck` row. A client refuses a lease that is not signed, or signed by a key its user has not checked, so
a database writer cannot put its own key or items on a lease. A new seed is a new key, which every approver checks
again.

### Machine access

An environment that needs unattended work (a demo that deploys after every green merge, a runner host whose key the
engine uses for queued jobs) can hold **machine access** to named items (§3.8). A holder of `VAULT_ADMIN` asks for it
(`ask#VaultMachineAccess`), which makes a lease for the grant; an approver approves it as any lease; then the server
seals each item key under its **machine key** with the vault crate's AES-256-GCM and the environment, vault and item
ids as associated data, so a row moved to another environment or item does not open. Until Sealed-Secrets S1 the
machine key is HKDF of `entity-facade@crypt-pass` with the label `vault-machine`, read through the factory's secret
handle as the entity facade reads it (since S4 from `seal.json` when the conf leaves it at `CHANGEME`): whoever holds
that pass and the database opens these items, so they are as strong as the server's own keys, and the vault's page
says so. After `operator rotate` of the pass, a row sealed under the one before (`entity-facade@crypt-pass-old`)
still opens and is sealed again under the new one at its next use. The column is not an entity `encrypt="true"`
field, so no find, cache, export or sync decrypts it. An operation of the environment then uses a lease approved by
"machine", recorded and used as any other; the environment is the operation's own, which the provision, deploy and
rotation operations (M1, M3, M5) pass from the `InstanceEnvironment` of the instance they act on, never one a request
names; a CI run reaches its https runner host with the key the host's environment (`InstanceHost.environmentId`) holds,
under a machine lease for that one call (`moqui_core::vault_hooks::machine_run`). When an item
is written with a new item key (a client's `item_seal` answers `new_item_key`, and the write says
`newItemKey`), its machine access goes stale: a lease under it is refused with `machineAccessStale` and the vault's
page shows it, until machine access is given again. Revoking machine access (`revoke#VaultMachineAccess`) deletes the
sealed keys, ends the environment's machine leases open on the vault, and names the items to rotate at their source,
since the server could have kept them.

### Reveal

A member with the reveal role can see one value: `reveal#VaultItem` sends that item's wrapped key and ciphertext to
their client, which opens it with the vault key, and writes a `VeReveal` event first. No other call sends an
operational body to a client.

### Rotation of operational items

An operational item has a rotation age: its own (`rotationDays`, which a manager of the vault sets with
`set#VaultItemRotation`), else its type's (`VaultItemType.rotationDays`), counted from `rotatedDate`, when its value
last changed at its source. Making or depositing an item sets it, and so does a write marked `rotated`
(`update#VaultItem`, which its event says) after the value was changed by hand; taking a deposit in or changing notes
does not. An item written before rotation ages is given the time of the next start (once), so its age counts from the
upgrade. Past its age the item is due.

The instance manager rotates the credentials it can change at their source (Vault-Instances §4.7,
[Run-Deploy](../Run-Deploy.md#rotation)). Before it changes the source it stages the new value: the lease opens the
item's body on the server, only the rotated field is changed (its other fields, notes, tags and URIs stay), and the body
is sealed under a new item key wrapped to the deposit key, a `VaultItemPending` row (`stage_revision`); no caller gets
the body. Once the source and the instances took the value, the staged value becomes the item's next revision
(`promote_revision`, event `VeItemRotated`), refused when the item was written since the rotation read it; the revision
before is kept in `VaultItemHistory` with its wrapped key and body as they were, the way back that a member opens in
their client. When the rotation puts the old value back, the staged one is dropped; when it cannot put it back
everywhere, the staged one is kept (`keptDate`), since the source may hold it: `list#VaultItemHistory` lists it
(`pendingList`, no body) and `reveal#VaultItem` with its `pendingId` sends it to the reveal role. Staging and the new
revision move the vault's revision, so a member removal computed before is refused. Like any deposit, a member takes the
new revision in before a lease names the item again; an environment's machine access to it is marked stale, since its
sealed key is of the old item key. A rotation checks the deposit key before it changes anything at a source. The
provision's undo (`withdraw_item`) takes only a first revision, never a rotated one.

**Taking a deposit in.** `take#VaultDeposit` takes a member's client's rewrap of a deposited item's key from the deposit
key to the vault key (`deposit_adopt`), at the vault's current key version, naming the revision it was made from; the
body stays as it is. A vault's page in System › Vaults offers *Take in* for its deposited items: the vault worker loads
as the lease dialog's does, the key passphrase opens the personal key there, and only the wrapped item keys come back.

Removing a member from an operational vault rotates the vault key, but the removed member may have kept item keys or
values, so the removal's answer (`rotationOffer`) lists the vault's items: which the instance manager can rotate at
their source, and what to do by hand for the rest (§3.9).

### System › Vaults

The operational vaults the user is a member of; a vault's members and roles, its items (names, bindings, types,
revisions and the days since each changed, never a value), its leases (waiting, open, ended) with Revoke, its machine
access with Revoke, and its events; the leases waiting for the user's approval; and the lease key's fingerprint. Its
pages run under [the vault screens' policy](#the-vault-screens-policy). The SSH admin shell lists leases (`leases [vaultId]`) and revokes one (`lease revoke
<leaseId>`) but never approves: the key passphrase would be typed on the server.

## Organization recovery

A shared-key vault can have the organization's **recovery key** on, so that a member who loses both their key
passphrase and their recovery code gets the vault back (§3.10). Recovery is **off by default for a personal shared-key
vault** and **on by default for an operational one**, which keeps recovery on or at least two members: making one, and
removing a member, refuse to leave it with neither. A single-secret vault has no members and never has recovery: only
its password or key opens it. A vault shows whether recovery is on (`recoveryOn`, and the recovery key in its member
list), so a person asked to join sees it.

- **The recovery key is split k of n.** A holder with the sealed permission `VAULT_RECOVERY` and a recent sign-in
  makes it in their client (`moqui vault recovery make`, or `create#VaultRecoveryKey` from a client): the key's seed is
  split into n shares, one wrapped to each holder's personal key (each holder holds `VAULT_RECOVERY`, and the maker has
  checked their key), and the seed is dropped. No one holds the whole key between recoveries, and no k − 1 holders can
  open a vault (a recovery rebuilds it in one client: see the limit below). It is kept by user group or by instance (`userGroupId`, `instanceId`). A holder who replaces their personal key can no
  longer open their share: make a new recovery key while k of the others still can.
- **Turning recovery on.** A manager checks the key of the holder who made the recovery key, then lists the recovery
  key in the vault's member list (under `recovery:<id>`, with the manage role) and wraps the vault key to it
  (`add#VaultMember`, or `moqui vault recovery on`). A client refuses a recovery key whose maker's key its user has not
  checked, so a server cannot hand out a recovery key of its own. Every rotation wraps the new vault key to the
  recovery key as to any member. Removing that member (`remove#VaultMember`) turns recovery off and rotates the key.
- **A recovery takes k holders, each with `VAULT_RECOVERY` and a recent sign-in.** The member makes a new personal
  key. A holder who has checked that new key (and the key of one of the vault's managers, which anchors the vault's
  member list for their client) starts a recovery (`moqui vault recovery start`): their client makes an ephemeral key,
  kept only in its memory, and signs a session naming it, the vault, the member and the new key. k − 1 other holders
  see what the session recovers, check that its starter is a holder (the maker's signature over the starter's share)
  and the member's new key themselves, and seal their shares to the ephemeral key (`moqui vault recovery share`). The
  recovering client combines the shares with its own, checks the result against the recovery key's signed public
  halves, opens the vault key through the recovery key's wrap of that session's vault only, signs the next member list
  as the recovery key (giving the member the new key, with the same members each once and nothing else changed, which
  every member's client checks) and wraps the vault key to the new key (`complete#VaultRecovery`). The combined key
  lives in that module only from the combine to the rekey: any other call, the rekey itself and a lock drop it, with
  the vault key it opened. A holder who is also a member may have that key version open already, and needs to forget
  nothing first: the recovery key's wrap must then open the same key (another is refused), and the version stays open
  after the rekey as it was before. The server takes the end only with k − 1 other holders' shares from holders who still hold
  `VAULT_RECOVERY`, and the new key still the member's active one. A session takes shares for a day; the recovering
  holder, the member, or any other person in the vault's member list can cancel it.
- **Replacing a used recovery key.** Shamir rebuilds the recovery key in the recovering holder's client, so after a
  recovery that client has held the whole key and could have kept it. The key is marked used (`usedDate`;
  `list#VaultRecoveryKeys` answers `used`, and a screen shows it as "used, replace"), the managers of every vault that
  lists it are told to replace it, and `moqui vault recovery start` offers to make a new one for the same holders.
  A manager replaces it with `moqui vault recovery replace <vaultId> --key <new>`: one rotation that removes the used
  key and lists the new one (`remove#VaultMember` of the old recovery member, with the new one in the entry), so the
  vault key the recovering client saw is replaced too and an operational vault is never without recovery.
- **Every member is told.** A start, each share, the end and a cancel are `VaultEvent`s (`VeRecoveryStarted`,
  `VeRecoveryShared`, `VeRecovery`, `VeRecoveryCancelled`; a recovery key made is `VeRecoveryKeyMade`), and a start and
  the end are notices to every member of the vault (topic `VaultRecovery`), so a member can cancel a recovery they did
  not ask for.
- **The limit.** k holders who also hold a copy of the database could open every vault with recovery on with that
  recovery key; `VAULT_RECOVERY`, the recent sign-in, the event and the notice are rules the server keeps, not
  cryptography. For a personal shared-key vault with recovery on, k holders acting together are administrators who can
  open it (§3.12). Shamir rebuilds the whole key in the recovering holder's client at each recovery: a client that
  has been changed (or a holder who changed it) keeps it, and opens every vault that lists that key until each has
  replaced it. That is why a used key is replaced, not kept.

BOSS Admin › Security, which shows the recovery keys and holders for each organization, is mr-boss's, on these
services (`/rest/s1/vault/recoveryKeys` and `/rest/s1/vault/recoveries`).

## On the server

The tables are `moqui.security.vault` (`framework/entity/VaultEntities.xml`): `Vault`, `VaultKeyVersion` (the signed
member-list chain), `VaultMember` (a member's wrap of one key version, with their role), `VaultItem` and
`VaultItemHistory`, `VaultPersonalKey`, `VaultKeyCheck`, `VaultDelivery`, `VaultEvent`, the item types
(`VaultItemType`, `VaultItemTypeField`, seeded in `framework/data/VaultTypeData.xml`: login, note, card, identity,
contact, SSH key, API key, database credential, TLS key pair, registry login, recovery code, recovery share),
`VaultLease` and `VaultMachineAccess` (V2), and recovery's `VaultRecoveryKey`, `VaultRecoveryHolder`,
`VaultRecoverySession` and `VaultRecoveryDelivery` (V4). No column is an entity `encrypt="true"` field: they hold
ciphertext and signed records already.

The services are `org.moqui.security.VaultServices` (`crates/moqui-vault-server`), also at `/rest/s1/vault`
(`framework/service/vault.rest.xml`). Records cross in the vault crate's JSON shapes, so a client hands an answer to
its vault module as it came. Each service checks its caller itself: a vault and its items are "not found" to anyone
but its members (and a single-secret vault's owner), writes need the write or manage role, the member list and a
deletion the manage role and a recent sign-in, an operational vault `VAULT_ADMIN` to make or delete; a delivery is
listed and filed only by its recipient. Ids are made by the client (the vault crate binds them into what it seals);
an id that is taken, or that a deleted record had (its events stay), is refused in the same words, so a deleted
vault's events never show under a new vault. The server checks what it can (each key version follows the latest and is signed by a manager of it, with
the caller's active key; each wrap by a manager of its version; each key check and delivery by its maker's active
key; a write only at the current key version, an update only from the current item revision), and the client checks
it all again, since a server that has been changed would not.

## The `moqui vault` command

`moqui vault --server URL --user USERNAME <command>` is a vault client on your own machine. It runs the same vault
crate natively, so the key passphrase and every key stay in its process; each run signs in with your password and
second factor and keeps nothing of the sign-in.

| Command | What it does |
|---|---|
| `key init` | Makes your personal key here (the passphrase twice) and prints its recovery code and fingerprint |
| `key show` | Your key's fingerprint |
| `key check <username>` | Shows a user's fingerprint to compare with them; on yes, stores your signed check |
| `list` | Your vaults |
| `open <vaultId> [--item ID]` | Opens a vault through its member list and lists its items, or prints one |
| `leases [--vault ID]` | The leases waiting for your approval and those you asked for, or one vault's |
| `approve <leaseId>` | Shows what the signed lease names, has you compare the lease key's fingerprint the first time, and on yes approves it here |
| `recovery list` | The organization's recovery keys, and the recoveries you may seal a share to |
| `recovery make --k K [--group G] <username>...` | Makes a recovery key split K of these holders (each key checked first) |
| `recovery on <vaultId> [--key ID]` | As a manager: turns recovery on for a vault (the recovery key's maker checked first) |
| `recovery start <vaultId> <username> [--key ID]` | As a holder: recovers the vault to the member's new key (checked first); waits for the other shares |
| `recovery share <recoverySessionId>` | As another holder: shows what a recovery recovers and seals your share to it |
| `recovery replace <vaultId> --key ID` | As a manager: lists a new recovery key in place of a used one, in one rotation |
| `export-open FILE [--item ID]` | Opens an encrypted export file of Commons › Vault here, with its password: no server, no sign-in |
| `shares [--vault ID]` | As a managed instance's recovery holder: checks each share an instance delivered to you against its link key as the manager pinned it, files it into your personal vault ID as a `recovery-share` item with that link key's fingerprint (else prints both once), and sends your signed confirmation ([Run-Deploy](../Run-Deploy.md#managed-sealed-start)) |
| `seal-share <appInstanceId> --vault ID` | As a recovery holder: takes the share you filed for the instance in vault ID, checks the key the sealed instance made for this unseal against the link key fingerprint filed with it (not one the manager gives) and its expiry, and seals the share to it; the manager relays only the ciphertext |

What it has accepted of each vault's member list is kept in `~/.config/moqui/vault/state.json` (mode 0600, no keys).

## The crypto

All of it is one crate with no I/O and no framework dependency, `crates/moqui-vault`, linked natively and built to a
wasm module for the browser, so a vault written on a phone opens the same way in a browser and on the server.

| What | How |
|---|---|
| Symmetric | AES-256-GCM with a random 96-bit nonce: `nonce ‖ ciphertext ‖ tag`. The associated data is a list of parts, each with a 4-byte length, so no two lists give the same bytes |
| Names | Every associated data and HPKE `info` starts with `moqui-vault/1`, then the purpose (`vault-key`, `item-key`, `body`, `deposit-key`, `lease`, `machine` and the others in `names.rs`), the vault id, the item id, the key version, and a byte that says whether an operational item's type, binding and clear name follow (so names with no label never encode as labelled ones). A value copied to another row, vault, version or purpose, or an operational item whose clear columns changed, does not open |
| Bodies | Padded to 256-byte steps (`0x80` then zeros); at most 64 KiB before padding. A body's names carry no key version (it is 0): a rotation rewraps item keys, not bodies, and the item key's wrap names the version |
| Text | Base32 and hex of keys, shares and codes are encoded and decoded in constant time: arithmetic on masks, no table indexed by a secret, and a bad character refused only after the whole text |
| Passwords | Argon2id v1.3. The floor m=64 MiB, t=3, p=1 and the ceiling m=256 MiB, t=6, p=4 are refused below and above; the target starts at the floor, and a wrap under it opens and is wrapped again at the target. The parameters and a 16-byte salt are stored with each wrap |
| Keys from keys | HKDF-SHA256. A recovery code or a generated key is 256 random bits as Crockford base32 in groups of four, so it needs no Argon2id |
| Public-key wraps | HPKE (RFC 9180) base mode: DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, AES-256-GCM, single shot, `enc ‖ ciphertext` |
| Signatures | Ed25519, verified strictly, over a statement: `moqui-vault-signature/1`, its kind and its parts, length-prefixed. The kinds: member wrap, key version, deposit key, recovery key, recovery share, recovery session, recovery session share, delivery, lease, lease approval, key check |
| Shares | Shamir over GF(2^8), constant time, k of n with 1 < k ≤ n ≤ 255. A share's text is `share-<index>-` and base32 of its value with a SHA-256 checksum, so a typo is caught |
| Fingerprints | Six proquints (five-letter pronounceable words) over 96 bits of SHA-256 of both public keys of a personal key, or of the lease key |
| Memory | Keys are `Key32` and the types over it: cleared on drop, compared in constant time, `Debug` redacted |

What each wrap carries and who signs it:

- **Personal key**: an X25519 key pair to receive and an Ed25519 key pair to sign. The two private halves are sealed
  together by the key passphrase (Argon2id) and, separately, by the recovery code (HKDF), with the personal key's id
  and both public keys in the associated data; opening checks the private halves against the stored public ones.
- **Single-secret vault**: the vault key under the password (Argon2id) or the generated key (HKDF).
- **Member wrap**: the vault key by HPKE to the member's personal key, signed by the manager who made it over the
  vault, the version, the member, both of their public keys and the wrap's hash.
- **Key versions**: each entry of a vault's member list (a version and a revision) is signed by a manager of the entry
  before, with the key that entry lists for them, back to the maker; a list that names one personal key twice is
  refused. A client accepts the chain only when its user has checked one signer's key, and trusts it only from the
  first entry such a signer signed (the entries before could be a forged history): a member wrap is accepted only
  from a manager of that version, in a trusted entry, to a member of it. A client with memory keeps the highest entry
  and its member-list hash, and refuses a chain that does not hold that entry with that list: a rollback, or a fork
  however high it goes.
- **Key checks**: a `VaultKeyCheck` row is signed by the checker over the checked key's id, both its public keys (or
  the lease key) and the time. A check covers one key; nothing is wrapped to a personal key, and no chain is anchored
  in one, before a check signed by the user's own key covers it.
- **Items**: an item key by the vault key; or, for an item the server makes in an operational vault, by HPKE to the
  deposit key, which is accepted only for the current key version and only with a manager's signature of that version
  on it. A rotation (§3.9, Remove) rewraps every item key, wraps to each remaining member and makes a new deposit key;
  an item's first write after a key-version change gets a new item key (decision (c)).
- **Lease**: signed by the server's lease key over its id, vault, version, items (id, type, binding, name), operation,
  cluster member, time limit and ephemeral public key. The approver's client refuses a lease key its user has not
  checked and a lease past its time limit or more than 4 hours away, unwraps each item key under the lease's own type,
  binding and name, seals it by HPKE to the lease and signs the approval.
- **Machine access**: an item key under the machine key (HKDF of `entity-facade@crypt-pass` with `vault-machine` until
  Sealed-Secrets S1), with the environment, vault and item ids as associated data.
- **Recovery key**: a key pair of the personal key's shape (X25519 and Ed25519, both by HKDF from one 32-byte seed),
  its public halves signed by its maker; the seed split k of n, each share by HPKE to one holder and signed, and then
  dropped. A vault with recovery on lists it as a manager in its member list, and the vault key is wrapped to it as a
  member wrap. A recovery session is signed by the recovering holder over its ephemeral X25519 key, the vault and the
  member's new key; each other holder's share goes by HPKE to that ephemeral key, signed by the holder over the
  session; the recovering client combines them with its own and checks the result against the signed public halves.
  An entry of a member list signed by the recovery key may only give one person a new key.
- **Delivery**: a value by HPKE to one user, signed by the sending user or instance. A delivery carries no freshness,
  so a client files each `delivery_id` once and drops a repeat (`delivery::Filed`).

## The browser module

The browser runs the same crate as a wasm module, `runtime/components/base/webroot/screen/webroot/js/vault/moqui_vault.wasm`
(about 600 KB), built for `wasm32-unknown-unknown` with no wasm-bindgen and no getrandom: its one import is
`env.moqui_vault_random`, which the worker fills from `crypto.getRandomValues`.

- `VaultWorker.js` is a module Worker that hosts the module. The opened personal key, the unwrapped vault keys and a
  combined recovery key stay in the module's memory; the page gets public keys, wraps and the one item it asked for.
  On lock, or when a call traps, the worker overwrites the module's whole memory with zeros (even when the module
  could not answer) and starts a fresh, locked instance; an `init` zeroes the instance it replaces.
- `VaultClient.js` (`MoquiVaultClient`) is the page side: it fetches the worker's script and the module, each with its
  integrity hash when given (and that hash in its URL, `?v=`, as the pages load `VaultClient.js` itself, so a copy the
  browser cached before an upgrade is never asked for under the new hash, which it would fail), starts the worker from a `blob:` URL, gives it the module's bytes, and has a typed method
  for each call (`personalOpen`, `vaultKeyOpen`, `itemOpen`, `leaseApprove`, ...). It locks after 15 minutes with no
  call (a lock never rejects; a worker that cannot answer is ended), and `copy(text)` clears the clipboard after 30
  seconds when it still holds that text.
- The calls are JSON, one request and one answer (`crates/moqui-vault/src/ops.rs`); the raw ABI is `src/wasm.rs`, the
  crate's only `unsafe`. The module decides what it trusts itself, never by the page's say-so: a trust anchor or a key
  to wrap to is only one named by a key check its own personal key signed; a vault key opens only through the
  member-list chain (verified, checked against the remembered entry, the wrap accepted by it); each signed statement
  is built in the module from its parts (a key check only with the fingerprint the user compared), and there is no
  call that signs arbitrary bytes; a recovery key is combined and used in the module, and no share or combined key
  goes back to the page. An opened vault key version is never replaced. The request's strings are overwritten after
  each call, and the answer is written to a buffer of its exact size, which the worker's read frees and zeroes.

The module is not kept in git: `scripts/vault-wasm.sh` builds it on demand, with paths remapped so the same rustc
builds the same bytes on any machine, and writes beside it its stamp, `moqui_vault.wasm.json`, the build record: the
SHA-256 over the crate's source files (not its tests), the root `[workspace.dependencies]` lines of its workspace
dependencies, the `[profile.release]` settings of `.cargo/config.toml` and the Cargo.lock entries the module links;
those entries; the rustc version; and the module's own SHA-256. The script rebuilds only when one of those changed
(`--check` reports it without building), refuses a dependency tree with getrandom or wasm-bindgen, and adds the
`wasm32-unknown-unknown` target with rustup when it is missing. `./moq build`, `./moq archive`, `podman/build.sh` and
the moq test tasks run it first ([Build-Test, Build](../Build-Test.md#build)); a build with the production or all
runtime built in refuses to build without the module. On a runtime without it, the lease dialog says the module is not
built, and Commons › Vault gets no integrity hash for it and does not load it. The desktop and mobile shells
carry their own copy (plans/Vault-Instances.md §3.6): a shell build that bundles it runs the script first.

### The vault screens' policy

The worker runs in the page's document, and the `/qapps` shell loads every screen into the one document it was served
as, so a policy a screen's own answer carried would never reach it. The policy goes by the screen a request resolves
to instead (`crates/moqui-core/src/web/vault_policy.rs`): a request whose path names one of `vault@screen-names`
(`Vault`, `Vaults`, `VaultKey`, `VaultContacts`, `RecoveryKeys`: the apps' fixed list too) among its screens or their
default subscreens, never an extra-path name such as a Knowledge page called `Vault`, gets the site's `screen-render`
policy with `vault@screen-policy`'s directives added, when the response sets no policy of its own. The `/qapps` page is
resolved as the same path under `/apps`, and a host's own root screen is resolved for its host. A directive the site
sets is kept and given only the worker's `blob:` and `'wasm-unsafe-eval'`; a fetch directive the site leaves to its
`default-src` starts from it. The site's other headers, `X-Frame-Options` among them, stay. By default the added
directives are:

```
default-src 'self'; script-src 'self' 'unsafe-eval' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline';
img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; media-src 'self' data: blob:;
worker-src 'self' blob:; frame-src 'self'; object-src 'none'; base-uri 'self'
```

Everything comes from this origin, so a theme's script, style sheet or font from another host does not load on a vault
screen. There is no inline script: the shell's page has none (its settings are a JSON block, which runs nothing).
`'unsafe-eval'` stays because the shell compiles server-merged templates and `.qvue` components at run time (the
runtime build of `qvt-static-only` would not need it), and `'unsafe-inline'` for styles because Quasar and the
components set inline styles. So the policy keeps out a script from another host, not one the server sends or markup
its templates run (§3.12 of the plan). WebSockets go to this origin under `connect-src 'self'`.

The page is told whether it was served that way (`qvtConfig.vaultDocument`, from the request attribute
`moqui.vault.document`), and the shell's client (`WebrootVue.qvt.js`, `setUrl`) loads a vault screen as a new page
unless it was, keeping a navigation's body parameters for that page and calling its `onComplete` first. Leaving a vault
screen keeps the page; Commons › Vault locks its keys when the page goes anywhere but the vault screens and Commons ›
Contacts, whose `VaultContacts` container reads the open vault and starts no worker, as the apps lock when their page
leaves the vault screens. Both settings are read at each response; an empty `screen-policy` leaves the site's policy.
A response that sets its own policy keeps it: only the screen builder's preview of proposed screen text and dev-aide's
preview overlay do (`frame-ancestors 'self'`, so the dev tools can frame it), and only on a dev instance.

The files carry their integrity hash as `?v=`. A file asked for with a `v` that is a SHA-256 digest gets the year-long
cache only when it is that file's (`static_files::version_holds`), so a request made with a new hash before the new file
reached this server does not keep the old bytes under the new name; any other `v` (a start stamp) is cached as before.

## The desktop and mobile apps

The apps (the desktop app's client mode and the mobile app, [Desktop-Client](../ui/Desktop-Client.md)) run the vault
crate natively (`moqui_client::vault`) in place of the browser's worker, one session per page (an app window, or the
phone's one webview), as a browser has one worker per tab. The vault screens are still the server's, but
`VaultClient.js` finds the page bridge's `moquiDesktop.vault` and sends it the same calls; the keys stay in the app's
process.

- **The module is the app's.** The app never loads the server's module: its schemes refuse `VaultWorker.js` and
  `moqui_vault.wasm` on every origin, the shell's too (`moqui_client_tauri::scheme`, by path, so best effort), the
  shell's staged files leave out `js/vault`, and `VaultClient.js` in an app loads no worker. The page is still code the
  server sends (§3.12's limit), so the module decides what it trusts by itself, as in the browser.
- **Only a vault screen calls it.** Every screen of every app shares the app origin, so the app lets a page call the
  vault only while its window's top-level document was loaded at a vault screen (a path under `/qapps/` or `/apps/`
  naming `Vault`, `Vaults`, `VaultKey`, `VaultContacts` or `RecoveryKeys`) and it has loaded no other screen or frame
  since (WebrootVue's screen loads and frames are seen by the scheme), and its current URL is still a vault screen.
  Leaving, or loading anything else into the page, locks its session; each new document starts a fresh one. The app's
  scripts run in the main frame only, and since a same-origin frame could reach its parent's adapter, any frame that
  is not a vault screen ends the page's right too. A script injected into the vault screen itself is still §3.12's
  limit.
- **Locking.** A page's session locks after its idle time with no call (the page's `idleMinutes`, 15 by default, at
  most 60; the app sweeps every 30 seconds whether or not a page calls), on the page's Lock, when the page leaves the
  vault screens or its window closes, on sign-out, revocation and removal, and when another account becomes current. On
  macOS also when the screen locks, the session is switched, or the computer goes to sleep (the OS's notices); on
  macOS, Linux and Android when the client's clocks show a sleep; on the phone when the app leaves the screen (the phone
  locked or another app in front). Not done yet: the screen lock on Linux and Windows, and a sleep on Windows (whose
  monotonic clock counts through it), where only the idle time locks.
- **Rollback memory** (decision (d) of §9). The app keeps, per account and vault, the highest key version and revision
  it accepted with the hash of that member list (`vault/<account>.seen.json` in the client's directory, owner-only),
  and hands it to `chain_verify`, `vault_key_open`, `key_version_next`, `vault_rotate`, `recovery_open_vault` and
  `recovery_rekey` in place of the page's `remembered`: an older chain, or one that went another way, is refused
  across restarts. It also keeps the highest revision seen of each item (at most 20,000 items of each of 1,000 vaults):
  `item_open` and `item_summary` given a `revision` lower than one seen are refused before the module opens anything,
  and `revisionsSeen({vault_id, items})` answers a list's older ones. A kept value is never lowered. In a browser
  `revisionsSeen` keeps the highest revisions in `localStorage` per user (the client's `userId`) and vault, which tells
  an older copy but does not refuse it.
- **Device memory** (§3.5). With the personal key open, the user may have the app remember it for a time
  (`remember(minutes)`, at most 24 hours and at most `vault@device-memory-max-hours`, 8 by default; 0 offers none). It
  needs a click in the vault (the page's user activation, read with the getter the bridge took before any page script
  ran). The private halves are wrapped (`personal::wrap_with_device_key`; the memory's id, user and end time in the
  associated data) into `vault/<account>.memory.json`, under a key made by HKDF of two halves
  (`personal::device_wrap_key`): the device's, kept in its key store, and the server record's 32-byte secret
  (`memorySecret`, an encrypted field), which `check#VaultDeviceMemory` hands only to a device's key and only while the
  record is live. The device's key store is the desktop's OS key store (Keychain, Credential Manager, Secret Service),
  opened by the computer's sign-in, or on Android a Keystore key the user authorizes at each use with a strong
  biometric or the lock screen (BiometricPrompt with the cipher), only while the phone is unlocked, and which a new
  biometric enrolled invalidates (Android 11 and later, with a secure lock screen). Where the keys are in a plain file
  (a desktop with no key store, a development run), no device memory is offered. The user's Vault key settings list
  the devices that remember the key (`list#VaultDeviceMemories`) and can take one off (`forget#VaultDeviceMemory`).
- **When it ends.** At its time (the memory, its key and the sessions opened from it go), on the page's Lock with
  `forget`, on `forget()`, on sign-out and removal, and whenever the server's record ends: taken off the list, past its
  time, the personal key replaced, or the device's key no longer taken. Since the record's secret is half of the key,
  this is cryptographic, not only the app's rule: a revoked device opens nothing even when the app was not running at
  the revocation. While a session opened from memory is open the app asks the server every five minutes and locks it
  when the record has ended. A server that cannot be reached gives no secret: the memory is kept and not opened. A
  restore needs a click too; on the desktop it opens with no further prompt (macOS LocalAuthentication and Windows Hello
  would need bindings the app does not carry), so it relies on that click, the vault-screen rule and the OS key store's
  own access control. There is no iOS app yet; the passkey slot (WebAuthn PRF) waits for passkeys, as another kind of
  memory key store (`MemoryKeys`).

The desktop app's node modes (the node in this process or as a sidecar) serve the vault screens from the app's own
bundled runtime, so their pages use the browser module from the app's own files.

## Tests and vectors

`cargo test -p moqui-vault` runs the crate's tests, one for each refusal above, and compares the known-answer
vectors in `crates/moqui-vault/testdata/vectors.json` with what the code makes; it includes the CFRG test vector for
HPKE with KEM 0x20, KDF 1 and AEAD 2. After a deliberate format change, `MOQUI_REGENERATE_VECTORS=1 cargo test -p
moqui-vault` rewrites the file; read its diff before committing. The module, whose source changed, rebuilds on the
next `./moq build` or test task.

`cargo test -p moqui-core vault_policy` checks which paths are vault screens' and that the page at one, and its
screens' answers, get the vault screens' policy while others keep the conf's.

`moqui test-client framework --offline --spec VaultModule` loads the module in its worker in the test browser and
checks that it makes the same vectors, that an item seals, opens and is forgotten on lock there, and that a vault key
crosses to a second worker only through a member list checked there; and that in an app the client calls
`moquiDesktop.vault` and fetches no worker or module.

The apps' vault: `cargo test -p moqui-client vault` (device memory ends at its time and when the device is revoked,
needs a key store and an open key, waits for the device's lock; rollback memory is the app's, not the page's; idle
locking), `cargo test -p moqui-vault-server device_memory` (the server's record), `./moq test-clients tauri` (the
server's module refused in the app), `./moq test-clients desktop` and `./moq test-clients mobile` (the phone's
Keystore key of the device's lock, with a stand-in for the Keystore). The hand checks on installed apps are in
[Mobile-Desktop-Exit-Checks](../../plans/Mobile-Desktop-Exit-Checks.md#v5-the-vault-in-the-apps).

`cargo test -p moqui-vault-server` runs the server's rules against the in-memory entity facade, with clients that are
the vault crate's own session: no row of a personal vault holds an item's name, type or value in the clear; a
non-member gets no wrap and no body; a removed member has no wrap and a write at the old key version is refused; a
key version or wrap no manager signed is refused by the server and by a member's client; nothing is wrapped to a
changed personal key before it is checked again; a delivery goes only to its recipient; a wrapped personal key needs
a strong, recent sign-in; each change writes its event; and V2's: a lease opens only the items it names and nothing
it held opens after it ends; an approval without the approve role or a recent sign-in is refused; a lease whose event
cannot be written does not open; a lease approved through one cluster member runs on the member that asked; machine
access opens only its items for its environment, and its sealed keys moved to another environment's row do not open;
a client refuses a lease whose public key or items changed after it was signed, and one signed by a lease key its user
has not checked; a binding pointed at another item fails at the unwrap; a deposit key with no manager's signature is
refused; a leased value is not in a job's stored parameters; a restart ends the member's leases; a reveal goes only to
the reveal role; a rename seals the header again, for a manager or a single-secret vault's owner from the revision
read. `moqui test-client framework --serve --spec VaultLease` approves a lease end to end over REST with the
test client's `vault` member. Recovery's: k recovery holders recover a vault to a member's new key and every member
is told, k − 1 cannot, a single-secret vault never gets a recovery wrap, a holder's step without `VAULT_RECOVERY` or a
recent sign-in is refused, and an operational vault cannot drop to one member with recovery off. `moqui test
framework --spec VaultServices` checks the
services' access as a server spec, and `moqui test-client framework --serve --spec VaultShared` the same rules over
REST with the test client's `vault` member ([JS-Test-Client](../dev/JS-Test-Client.md)), which is the vault crate's
session run natively; `--spec VaultRecovery` runs a recovery end to end the same way.
