/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The page side of the vault module (plans/Vault-Instances.md §3.6; docs/security/Vaults.md). It starts
 * VaultWorker.js, which holds moqui_vault.wasm and every key; the page asks it for one item at a time.
 *
 *   const vault = new MoquiVaultClient({ wasmIntegrity: "sha256-...", workerIntegrity: "sha256-..." });
 *   await vault.start();
 *   const opened = await vault.personalOpen({ personal_key_id, user_id, public, passphrase_wrap, passphrase });
 *   const { body } = await vault.itemOpen({ vault_id, key_version, item_id, wrapped_item_key, sealed_body });
 *   await vault.copy(secretText);   // cleared from the clipboard after 30 s
 *
 * The page fetches the worker's script and the module itself, each with its integrity hash when one is given, then
 * starts the worker from a blob: URL (the vault screens' CSP allows worker-src blob: and 'wasm-unsafe-eval') and
 * hands it the module's bytes. The vault locks after `idleMinutes` with no call (15 by default) and on lock(); a lock
 * zeroes the module's memory in the worker.
 */
"use strict";
(function (global) {
    const here = (global.document && global.document.currentScript && global.document.currentScript.src) || "/js/vault/VaultClient.js";

    class MoquiVaultClient {
        constructor(options) {
            const o = options || {};
            this.workerUrl = o.workerUrl || new URL("VaultWorker.js", here).href;
            this.wasmUrl = o.wasmUrl || new URL("moqui_vault.wasm", here).href;
            this.workerIntegrity = o.workerIntegrity || null;
            this.wasmIntegrity = o.wasmIntegrity || null;
            this.idleMs = (o.idleMinutes == null ? 15 : o.idleMinutes) * 60000;
            this.clipboardMs = (o.clipboardSeconds == null ? 30 : o.clipboardSeconds) * 1000;
            this.onLock = o.onLock || null;
            this.worker = null;
            this.nextId = 1;
            this.waiting = new Map();
            this.idleTimer = null;
        }

        async fetchChecked(url, integrity) {
            const response = await fetch(url, integrity ? { integrity, credentials: "same-origin" } : { credentials: "same-origin" });
            if (!response.ok) throw new Error("could not load " + url + ": " + response.status);
            return response;
        }

        /** Loads the worker and the module; resolves once the module is ready. */
        async start() {
            if (this.worker) return;
            const script = await (await this.fetchChecked(this.workerUrl, this.workerIntegrity)).text();
            const blobUrl = URL.createObjectURL(new Blob([script], { type: "text/javascript" }));
            this.worker = new Worker(blobUrl, { type: "module" });
            URL.revokeObjectURL(blobUrl);
            this.worker.onmessage = (event) => {
                const { id, ok, result, error } = event.data || {};
                const pending = this.waiting.get(id);
                if (!pending) return;
                this.waiting.delete(id);
                if (ok) pending.resolve(result);
                else pending.reject(new Error(error));
            };
            this.worker.onerror = (event) => {
                for (const pending of this.waiting.values()) pending.reject(new Error(event.message || "the vault worker failed"));
                this.waiting.clear();
            };
            const bytes = await (await this.fetchChecked(this.wasmUrl, this.wasmIntegrity)).arrayBuffer();
            await this.post({ op: "init", bytes }, [bytes]);
        }

        post(message, transfer) {
            const id = this.nextId++;
            return new Promise((resolve, reject) => {
                this.waiting.set(id, { resolve, reject });
                this.worker.postMessage(Object.assign({ id }, message), transfer || []);
            });
        }

        /** One call of the module (crates/moqui-vault/src/ops.rs); each resets the idle timer. */
        async call(op, args) {
            if (!this.worker) await this.start();
            this.touch();
            return this.post({ op, args: args || {} });
        }

        touch() {
            if (this.idleTimer) clearTimeout(this.idleTimer);
            if (this.idleMs > 0) this.idleTimer = setTimeout(() => this.lock(), this.idleMs);
        }

        /** Drops every key: the module forgets them and the worker zeroes its memory. */
        async lock() {
            if (this.idleTimer) clearTimeout(this.idleTimer);
            this.idleTimer = null;
            if (!this.worker) return;
            await this.post({ op: "lock" });
            if (this.onLock) this.onLock();
        }

        /** Ends the worker; a later call starts a new one. */
        terminate() {
            if (this.idleTimer) clearTimeout(this.idleTimer);
            if (this.worker) this.worker.terminate();
            this.worker = null;
            for (const pending of this.waiting.values()) pending.reject(new Error("the vault worker was ended"));
            this.waiting.clear();
        }

        /** Copies a value, and clears the clipboard after clipboardSeconds when it still holds that value. */
        async copy(text) {
            await navigator.clipboard.writeText(text);
            setTimeout(async () => {
                try {
                    if ((await navigator.clipboard.readText()) !== text) return;
                } catch (e) {
                    // reading needs a permission the page may not have; clear anyway
                }
                try {
                    await navigator.clipboard.writeText("");
                } catch (e) {
                    // the page lost focus; nothing more can be done from here
                }
            }, this.clipboardMs);
        }

        vectors() { return this.call("vectors"); }
        status() { return this.call("status"); }
        personalMake(args) { return this.call("personal_make", args); }
        personalOpen(args) { return this.call("personal_open", args); }
        personalOpenRecovery(args) { return this.call("personal_open_recovery", args); }
        vaultKeyNew(args) { return this.call("vault_key_new", args); }
        vaultKeyWrapMember(args) { return this.call("vault_key_wrap_member", args); }
        vaultKeyOpenMember(args) { return this.call("vault_key_open_member", args); }
        vaultKeyPasswordWrap(args) { return this.call("vault_key_password_wrap", args); }
        vaultKeyPasswordOpen(args) { return this.call("vault_key_password_open", args); }
        chainVerify(args) { return this.call("chain_verify", args); }
        itemSeal(args) { return this.call("item_seal", args); }
        itemOpen(args) { return this.call("item_open", args); }
        sign(args) { return this.call("sign", args); }
        verify(args) { return this.call("verify", args); }
        fingerprint(args) { return this.call("fingerprint", args); }
        leaseApprove(args) { return this.call("lease_approve", args); }
        shamirSplit(args) { return this.call("shamir_split", args); }
        shamirCombine(args) { return this.call("shamir_combine", args); }
    }

    global.MoquiVaultClient = MoquiVaultClient;
})(typeof window !== "undefined" ? window : self);
