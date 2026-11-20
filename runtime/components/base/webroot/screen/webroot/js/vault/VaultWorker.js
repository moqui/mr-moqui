/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The vault module's worker (plans/Vault-Instances.md §3.6; docs/security/Vaults.md): a module Worker that hosts
 * moqui_vault.wasm, the crypto of crates/moqui-vault built by scripts/vault-wasm.sh. The opened personal key and the
 * unwrapped vault keys stay in the module's memory: the page gets public keys, wraps and the one item it asked for.
 *
 * Messages from the page:
 *   {id, op: "init", bytes?: ArrayBuffer, url?: string}  the module, as bytes the page fetched (with its integrity
 *       hash) or a URL to fetch here (moqui_vault.wasm beside this file by default)
 *   {id, op, args}  one call of the module (crates/moqui-vault/src/ops.rs): "personal_open", "item_open", ...
 *   {id, op: "lock"}  the module drops its keys, then its whole memory is overwritten with zeros and the instance is
 *       thrown away; the next call starts a fresh one
 * Each answer is {id, ok: true, result} or {id, ok: false, error}.
 *
 * Randomness for the module is crypto.getRandomValues, given as its one import, env.moqui_vault_random.
 */
"use strict";

let module = null; // the compiled WebAssembly.Module, kept across a lock
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
    instance = await WebAssembly.instantiate(module, imports);
}

async function init(message) {
    let bytes = message.bytes;
    if (!bytes) {
        const response = await fetch(message.url || new URL("moqui_vault.wasm", self.location.href).href);
        if (!response.ok) throw new Error("the vault module did not load: " + response.status);
        bytes = await response.arrayBuffer();
    }
    module = await WebAssembly.compile(bytes);
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

async function lock() {
    if (!instance) return { locked: true };
    call({ op: "lock" });
    memoryBytes().fill(0);
    instance = null;
    if (module) await instantiate();
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
        self.postMessage({ id, ok: false, error: String((e && e.message) || e) });
    }
};
