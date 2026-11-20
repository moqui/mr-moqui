/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
"use strict";
// One worker for the file: each case asks it for what it needs.
const vault = new window.MoquiVaultClient({ idleMinutes: 0 });

async function nativeVectors() {
    return (await fetch("/files/crates/moqui-vault/testdata/vectors.json")).json();
}

test("the module in its worker makes the native known-answer vectors, the CFRG HPKE vector among them", async () => {
    const made = await vault.vectors();
    const native = await nativeVectors();
    assert.deepEqual(made, native);
    assert.equal(made.hpke_cfrg.computed_ct, made.hpke_cfrg.ct);
    assert.equal(made.hpke_cfrg.computed_enc, made.hpke_cfrg.enc);
});

test("the native shares combine in the module", async () => {
    const v = (await nativeVectors()).shamir;
    const combined = await vault.shamirCombine({ shares: [v.shares[4], v.shares[0], v.shares[2]] });
    assert.equal(combined.secret, v.secret);
});

test("an item seals and opens in the worker, and a lock forgets every key", async () => {
    const made = await vault.personalMake({ personal_key_id: "pk-1", user_id: "alice", passphrase: "a long passphrase" });
    assert.equal(made.fingerprint.split("-").length, 6);
    await vault.vaultKeyNew({ vault_id: "v", key_version: 1 });
    const label = { item_type: "login", binding: "b", name: "n" };
    const item = { vault_id: "v", item_id: "i", key_version: 1, label };
    const sealed = await vault.itemSeal(Object.assign({ body: '{"v":1}' }, item));
    const open = Object.assign({ wrapped_item_key: sealed.wrapped_item_key, sealed_body: sealed.sealed_body }, item);
    assert.equal((await vault.itemOpen(open)).body, '{"v":1}');
    const moved = Object.assign({}, open, { label: Object.assign({}, label, { binding: "other" }) });
    await assert.rejects(vault.itemOpen(moved), /does not open/);

    await vault.lock();
    await assert.rejects(vault.itemOpen(open), /locked/);
    assert.equal((await vault.status()).personal, null);
    const reopened = await vault.personalOpen({
        personal_key_id: "pk-1", user_id: "alice", public: made.public,
        passphrase_wrap: made.passphrase_wrap, passphrase: "a long passphrase",
    });
    assert.equal(reopened.public, made.public);
    await assert.rejects(
        vault.personalOpen({ personal_key_id: "pk-1", user_id: "alice", public: made.public, passphrase_wrap: made.passphrase_wrap, passphrase: "wrong" }),
        /does not open/,
    );
});
