# Vaults

A vault holds secrets under a key the server does not hold ([Vault-Instances](../../plans/Vault-Instances.md) has
the design). This guide grows with each phase; so far it covers the crypto every client and the server share, and what
the server keeps and serves (V1): personal keys, key checks, vaults of both locks, items, members, deliveries and
events, with the `moqui vault` command, and organization recovery split k of n (V4). Operational vaults' leases and
machine access (V2) and Commons › Vault (V3) are later phases.

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
  checked their key), and the seed is dropped. No one holds the whole key, and no k − 1 holders can open a vault. It is
  kept by user group or by instance (`userGroupId`, `instanceId`). A holder who replaces their personal key can no
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
  see what the session recovers and seal their shares to the ephemeral key (`moqui vault recovery share`). The
  recovering client combines the shares with its own, checks the result against the recovery key's signed public
  halves, opens the vault key through the recovery key's wrap, signs the next member list as the recovery key (giving
  the member the new key and changing nothing else, which every member's client checks) and wraps the vault key to the
  new key (`complete#VaultRecovery`). The shares, the recovery key and the vault key never leave the clients. A session
  takes shares for a day; the recovering holder or a manager of the vault can cancel it.
- **Every member is told.** A start, each share, the end and a cancel are `VaultEvent`s (`VeRecoveryStarted`,
  `VeRecoveryShared`, `VeRecovery`, `VeRecoveryCancelled`; a recovery key made is `VeRecoveryKeyMade`), and a start and
  the end are notices to every member of the vault (topic `VaultRecovery`), so a member can cancel a recovery they did
  not ask for.
- **The limit.** k holders who also hold a copy of the database could open every vault with recovery on with that
  recovery key; `VAULT_RECOVERY`, the recent sign-in, the event and the notice are rules the server keeps, not
  cryptography. For a personal shared-key vault with recovery on, k holders acting together are administrators who can
  open it (§3.12).

BOSS Admin › Security, which shows the recovery keys and holders for each organization, is mr-boss's, on these
services (`/rest/s1/vault/recoveryKeys` and `/rest/s1/vault/recoveries`).

## On the server

The tables are `moqui.security.vault` (`framework/entity/VaultEntities.xml`): `Vault`, `VaultKeyVersion` (the signed
member-list chain), `VaultMember` (a member's wrap of one key version, with their role), `VaultItem` and
`VaultItemHistory`, `VaultPersonalKey`, `VaultKeyCheck`, `VaultDelivery`, `VaultEvent`, the item types
(`VaultItemType`, `VaultItemTypeField`, seeded in `framework/data/VaultTypeData.xml`: login, note, card, identity,
contact, SSH key, API key, database credential, TLS key pair, registry login, recovery code, recovery share), and
recovery's `VaultRecoveryKey`, `VaultRecoveryHolder`, `VaultRecoverySession` and `VaultRecoveryDelivery`, and for
later phases `VaultLease` and `VaultMachineAccess`. No column is an entity `encrypt="true"` field: they hold ciphertext and signed records already.

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
| `recovery list` | The organization's recovery keys, and the recoveries you may seal a share to |
| `recovery make --k K [--group G] <username>...` | Makes a recovery key split K of these holders (each key checked first) |
| `recovery on <vaultId> [--key ID]` | As a manager: turns recovery on for a vault (the recovery key's maker checked first) |
| `recovery start <vaultId> <username> [--key ID]` | As a holder: recovers the vault to the member's new key (checked first); waits for the other shares |
| `recovery share <recoverySessionId>` | As another holder: shows what a recovery recovers and seals your share to it |

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
  integrity hash when given, starts the worker from a `blob:` URL, gives it the module's bytes, and has a typed method
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

The module is checked in with its stamp, `moqui_vault.wasm.json`: the SHA-256 over the crate's source files (not its
tests), the root `[workspace.dependencies]` lines of its workspace dependencies, the `[profile.release]` settings of
`.cargo/config.toml` and the Cargo.lock entries the module links; those entries; the rustc version; and the module's
own SHA-256. `scripts/vault-wasm.sh` builds both, with paths remapped so the same rustc builds the same bytes on any
machine. A change to any of those, or another `rustc -V`, fails `cargo test -p moqui-vault`
(`the_checked_in_wasm_is_what_the_source_builds`) until the module is rebuilt: run the script, or
`MOQUI_REGENERATE_WASM=1 cargo test -p moqui-vault the_checked_in_wasm`, and commit both files.

## Tests and vectors

`cargo test -p moqui-vault` runs the crate's tests, one for each refusal above, and compares the known-answer
vectors in `crates/moqui-vault/testdata/vectors.json` with what the code makes; it includes the CFRG test vector for
HPKE with KEM 0x20, KDF 1 and AEAD 2. After a deliberate format change, `MOQUI_REGENERATE_VECTORS=1 cargo test -p
moqui-vault` rewrites the file; read its diff before committing (and rebuild the module, whose source changed).

`moqui test-client framework --offline --spec VaultModule` loads the module in its worker in the test browser and
checks that it makes the same vectors, that an item seals, opens and is forgotten on lock there, and that a vault key
crosses to a second worker only through a member list checked there.

`cargo test -p moqui-vault-server` runs the server's rules against the in-memory entity facade, with clients that are
the vault crate's own session: no row of a personal vault holds an item's name, type or value in the clear; a
non-member gets no wrap and no body; a removed member has no wrap and a write at the old key version is refused; a
key version or wrap no manager signed is refused by the server and by a member's client; nothing is wrapped to a
changed personal key before it is checked again; a delivery goes only to its recipient; a wrapped personal key needs
a strong, recent sign-in; each change writes its event; k recovery holders recover a vault to a member's new key and
every member is told, k − 1 cannot, a single-secret vault never gets a recovery wrap, a holder's step without
`VAULT_RECOVERY` or a recent sign-in is refused, and an operational vault cannot drop to one member with recovery off.
`moqui test framework --spec VaultServices` checks the
services' access as a server spec, and `moqui test-client framework --serve --spec VaultShared` the same rules over
REST with the test client's `vault` member ([JS-Test-Client](../dev/JS-Test-Client.md)), which is the vault crate's
session run natively; `--spec VaultRecovery` runs a recovery end to end the same way.
