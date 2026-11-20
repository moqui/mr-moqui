/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
"use strict";
// One worker for Alice and one for Bob; each case asks them for what it needs.
const vault = new window.MoquiVaultClient({ idleMinutes: 0 });
const bob = new window.MoquiVaultClient({ idleMinutes: 0 });
// The module is built on demand (scripts/vault-wasm.sh, which the moq test tasks run), not kept in git: a run of
// `moqui test-client` straight after a checkout says so rather than fail on a 404.
const built = fetch("/js/vault/moqui_vault.wasm").then((response) => response.ok, () => false);
async function needModule() {
    if (!(await built)) throw new Error("the vault module is not built: ./moq build or scripts/vault-wasm.sh");
}

async function nativeVectors() {
    return (await fetch("/files/crates/moqui-vault/testdata/vectors.json")).json();
}

async function check(client, keyId, publicKey) {
    const { fingerprint } = await client.fingerprint({ public: publicKey });
    return client.keyCheckMake({ checked_key_id: keyId, checked_user_id: keyId.replace(/^pk-/, ""), checked_public: publicKey, fingerprint, checked_at: 1 });
}

test("the module in its worker makes the native known-answer vectors, the CFRG HPKE vector among them", async () => {
    await needModule();
    const made = await vault.vectors();
    const native = await nativeVectors();
    assert.deepEqual(made, native);
    assert.equal(made.hpke_cfrg.computed_ct, made.hpke_cfrg.ct);
    assert.equal(made.hpke_cfrg.computed_enc, made.hpke_cfrg.enc);
});

test("an item seals and opens in the worker, and a lock forgets every key", async () => {
    await needModule();
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

test("a vault key crosses from one worker to another only through a checked member list", async () => {
    await needModule();
    await vault.lock();
    const alice = await vault.personalMake({ personal_key_id: "pk-alice", user_id: "alice", passphrase: "pass phrase" });
    const b = await bob.personalMake({ personal_key_id: "pk-bob", user_id: "bob", passphrase: "pass phrase" });
    const aliceChecksBob = await check(vault, "pk-bob", b.public);
    const members = [
        { user_id: "alice", role: "manage", key_id: "pk-alice", public: alice.public },
        { user_id: "bob", role: "read", key_id: "pk-bob", public: b.public },
    ];
    const entry = await vault.keyVersionFirst({ vault_id: "w", members, key_checks: [aliceChecksBob] });
    await vault.vaultKeyNew({ vault_id: "w", key_version: 1 });
    const wrap = await vault.vaultKeyWrapMember({
        vault_id: "w", key_version: 1, member_user_id: "bob", member_key_id: "pk-bob", member_public: b.public,
        key_checks: [aliceChecksBob],
    });
    await assert.rejects(bob.vaultKeyOpen({ entries: [entry], key_checks: [], wrap }), /checked/);
    const bobChecksAlice = await check(bob, "pk-alice", alice.public);
    const opened = await bob.vaultKeyOpen({ entries: [entry], key_checks: [bobChecksAlice], wrap });
    assert.equal(opened.vault_id, "w");
    assert.equal(opened.remembered.version, 1);
    bob.terminate();
});

test("in the desktop or mobile app the client calls the app's native vault and loads no module from the server", async () => {
    const calls = [];
    const fetched = [];
    const pageFetch = window.fetch;
    window.fetch = (url, init) => { fetched.push(String(url)); return pageFetch(url, init); };
    window.moquiDesktop = { vault: {
        call: async (op, args) => { calls.push([op, args]); return op === "status" ? { personal: "pk-app", vaults: [] } : { op }; },
        memory: async (action, minutes) => ({ action, minutes }),
    } };
    try {
        const app = new window.MoquiVaultClient({ idleMinutes: 5 });
        assert.equal((await app.status()).personal, "pk-app");
        await app.itemOpen({ vault_id: "v" });
        await app.lock({ forget: true });
        assert.deepEqual(calls.map(([op]) => op), ["configure", "status", "item_open", "lock"]);
        assert.equal(calls[0][1].idle_minutes, 5);
        assert.equal(calls[3][1].forget, true);
        assert.deepEqual(await app.remember(30), { action: "remember", minutes: 30 });
        assert.equal(fetched.filter((url) => /\/js\/vault\//.test(url)).length, 0, "no worker or module fetched");
        // in a browser there is no device memory, and an older item revision is told for this client's life
        assert.equal((await vault.deviceMemory()).available, false);
        const list = (revision) => ({ vault_id: "v", items: [{ item_id: "i", revision }] });
        assert.deepEqual((await vault.revisionsSeen(list(3))).older, []);
        assert.deepEqual((await vault.revisionsSeen(list(2))).older, ["i"]);
    } finally {
        delete window.moquiDesktop;
        window.fetch = pageFetch;
    }
});
