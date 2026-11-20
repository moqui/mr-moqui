/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The vault module's worker (plans/Vault-Instances.md §3.6; docs/security/Vaults.md): a module Worker that hosts
 * moqui_vault.wasm, the crypto of crates/moqui-vault built by scripts/vault-wasm.sh. The opened personal key and the
 * unwrapped vault keys stay in the module's memory: the page gets public keys, wraps and the one item it asked for.
 *
 * Messages from the page:
 *   {id, op: "init", bytes?: ArrayBuffer, url?: string}  the module, as bytes the page fetched (with its integrity
 *       hash) or an absolute URL to fetch here (this worker runs from a blob: URL, so nothing relative resolves); an
 *       earlier instance is zeroed first
 *   {id, op, args}  one call of the module (crates/moqui-vault/src/ops.rs): "personal_open", "item_open", ...
 *   {id, op: "lock"}  the module drops its keys, then its whole memory is overwritten with zeros and the instance is
 *       thrown away; the next call starts a fresh one
 * Each answer is {id, ok: true, result} or {id, ok: false, error}.
 *
 * Randomness for the module is crypto.getRandomValues, given as its one import, env.moqui_vault_random.
 */
"use strict";

let compiled = null; // the compiled WebAssembly.Module, kept across a lock
let instance = null;

function memoryBytes() {
    return new Uint8Array(instance.exports.memory.buffer);
}

async function instantiate() {
    const imports = {
        env: {
            moqui_vault_random(ptr, len) {
                const bytes = new Uint8Array(instance.exports.memory.buffer, ptr, len);
                // getRandomValues fills at most 65536 bytes at a time
                for (let at = 0; at < len; at += 65536) crypto.getRandomValues(bytes.subarray(at, Math.min(len, at + 65536)));
            },
        },
    };
    instance = await WebAssembly.instantiate(compiled, imports);
}

/** Overwrites the module's whole memory with zeros and drops the instance, whatever state it is in. */
function wipe() {
    if (!instance) return;
    try {
        memoryBytes().fill(0);
    } finally {
        instance = null;
    }
}

async function init(message) {
    let bytes = message.bytes;
    if (!bytes) {
        // This worker runs from a blob: URL, so a relative URL has nothing to resolve against: the page passes the
        // module's bytes, or its absolute URL.
        if (!/^https?:\/\//.test(message.url || "")) throw new Error("init needs the module's bytes or its absolute URL");
        const response = await fetch(message.url);
        if (!response.ok) throw new Error("the vault module did not load: " + response.status);
        bytes = await response.arrayBuffer();
    }
    wipe();
    compiled = await WebAssembly.compile(bytes);
    await instantiate();
    return { ready: true };
}

/** One call of the module: the request JSON in, the answer JSON out; both buffers are zeroed after. */
function call(request) {
    const exports = instance.exports;
    const text = new TextEncoder().encode(JSON.stringify(request));
    const ptr = exports.moqui_vault_alloc(text.length);
    memoryBytes().set(text, ptr);
    text.fill(0);
    let handle;
    try {
        handle = exports.moqui_vault_call(ptr, text.length);
    } finally {
        exports.moqui_vault_free(ptr, text.length);
    }
    try {
        const at = exports.moqui_vault_result_ptr(handle);
        const len = exports.moqui_vault_result_len(handle);
        // copied out before the free, which zeroes the module's copy
        const answer = new TextDecoder().decode(memoryBytes().slice(at, at + len));
        return JSON.parse(answer);
    } finally {
        exports.moqui_vault_result_free(handle);
    }
}

/**
 * The module drops its keys, then its memory is zeroed and the instance thrown away even when the module trapped
 * (a trapped instance cannot answer, but its memory still holds what it held). A fresh instance takes its place.
 */
async function lock() {
    try {
        if (instance) call({ op: "lock" });
    } catch (e) {
        // the memory is zeroed below all the same
    } finally {
        wipe();
    }
    if (compiled) await instantiate();
    return { locked: true };
}

self.onmessage = async (event) => {
    const message = event.data || {};
    const id = message.id;
    try {
        if (message.op === "init") {
            self.postMessage({ id, ok: true, result: await init(message) });
        } else if (message.op === "lock") {
            self.postMessage({ id, ok: true, result: await lock() });
        } else if (!instance) {
            self.postMessage({ id, ok: false, error: "the vault module is not loaded" });
        } else {
            const answer = call(Object.assign({}, message.args || {}, { op: message.op }));
            self.postMessage(answer.ok ? { id, ok: true, result: answer.result } : { id, ok: false, error: answer.error });
        }
    } catch (e) {
        let error = String((e && e.message) || e);
        if (message.op !== "init" && message.op !== "lock" && instance) {
            // A trap leaves the instance unusable with its keys still in memory: zero it and start a fresh, locked one.
            wipe();
            error += " (the vault module failed and was locked)";
            try {
                if (compiled) await instantiate();
            } catch (ignored) {
                // the next call reports that the module is not loaded
            }
        }
        self.postMessage({ id, ok: false, error });
    }
};
