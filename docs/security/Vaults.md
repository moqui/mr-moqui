# Vaults

A vault holds secrets under a key the server does not hold ([Vault-Instances](../../plans/Vault-Instances.md) has
the design). This guide grows with each phase; so far it covers the crypto every client and the server share.

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
| Signatures | Ed25519, verified strictly, over a statement: `moqui-vault-signature/1`, its kind and its parts, length-prefixed. The kinds: member wrap, key version, deposit key, recovery key, recovery wrap, recovery share, delivery, lease, lease approval, key check |
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
- **Recovery key**: its public half signed by its maker; its private half split k of n, each share by HPKE to one
  holder and signed, and then dropped. A recovery takes k holders: each opens their share and delivers it, signed, to
  the recovering holder, whose client combines them and checks the result against the signed public half. A vault's
  wrap to the recovery key is signed by a manager of its key version.
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
