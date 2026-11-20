# Vaults

A vault holds secrets under a key the server does not hold ([Vault-Instances](../../plans/Vault-Instances.md) has
the design). This guide grows with each phase; so far it covers the crypto every client and the server share.

## The crypto

All of it is one crate with no I/O and no framework dependency, `crates/moqui-vault`, linked natively and built to a
wasm module for the browser, so a vault written on a phone opens the same way in a browser and on the server.

| What | How |
|---|---|
| Symmetric | AES-256-GCM with a random 96-bit nonce: `nonce ‖ ciphertext ‖ tag`. The associated data is a list of parts, each with a 4-byte length, so no two lists give the same bytes |
| Names | Every associated data and HPKE `info` starts with `moqui-vault/1`, then the purpose (`vault-key`, `item-key`, `body`, `deposit-key`, `lease`, `machine` and the others in `names.rs`), the vault id, the item id and the key version; an operational item adds its type, binding and clear name. A value copied to another row, vault, version or purpose, or an operational item whose clear columns changed, does not open |
| Bodies | Padded to 256-byte steps (`0x80` then zeros); at most 64 KiB before padding |
| Passwords | Argon2id v1.3. The floor m=64 MiB, t=3, p=1 and the ceiling m=1 GiB, t=10, p=4 are refused below and above; the target starts at the floor, and a wrap under it opens and is wrapped again at the target. The parameters and a 16-byte salt are stored with each wrap |
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
  before, with the key that entry lists for them, back to the maker. A client accepts the chain only when its user
  has checked one signer's key, accepts a member wrap only from a manager of that version to a member of it, and,
  with memory, refuses an entry lower than the highest it has seen, or the same entry with another member list.
- **Items**: an item key by the vault key; or, for an item the server makes in an operational vault, by HPKE to the
  deposit key, whose public half the server uses only after it checks a manager's signature on it.
- **Lease**: signed by the server's lease key over its id, vault, version, items (id, type, binding, name), operation,
  cluster member, time limit and ephemeral public key. The approver's client refuses a lease key its user has not
  checked, unwraps each item key under the lease's own type, binding and name, seals it by HPKE to the lease and signs
  the approval.
- **Machine access**: an item key under the machine key (HKDF of `entity-facade@crypt-pass` with `vault-machine` until
  Sealed-Secrets S1), with the environment, vault and item ids as associated data.
- **Recovery key**: its public half signed by its maker; its private half split k of n, each share by HPKE to one
  holder and signed, and then dropped. A recovery combines k opened shares in one client and checks the result against
  the signed public half. A vault's wrap to the recovery key is signed by the member who turned recovery on.
- **Delivery**: a value by HPKE to one user, signed by the sending user or instance.

## Tests and vectors

`cargo test -p moqui-vault` runs the crate's tests, one for each refusal above, and compares the known-answer
vectors in `crates/moqui-vault/testdata/vectors.json` with what the code makes; it includes the CFRG test vector for
HPKE with KEM 0x20, KDF 1 and AEAD 2. After a deliberate format change, `MOQUI_REGENERATE_VECTORS=1 cargo test -p
moqui-vault` rewrites the file; read its diff before committing.
