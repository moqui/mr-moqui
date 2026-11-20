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
            // lock() does not reject; a timer has no one to report to
            if (this.idleMs > 0) this.idleTimer = setTimeout(() => this.lock(), this.idleMs);
        }

        /**
         * Drops every key: the module forgets them and the worker zeroes its memory. When the worker cannot answer, it is
         * ended, which drops its memory with it. Never rejects.
         */
        async lock() {
            if (this.idleTimer) clearTimeout(this.idleTimer);
            this.idleTimer = null;
            if (!this.worker) return;
            try {
                await this.post({ op: "lock" });
            } catch (e) {
                this.terminate();
            }
            if (this.onLock) {
                try {
                    this.onLock();
                } catch (e) {
                    // the page's callback failing does not undo the lock
                }
            }
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
        fingerprint(args) { return this.call("fingerprint", args); }
        /** A signed check of a key, made only with the fingerprint the user compared ({fingerprint, ...}). */
        keyCheckMake(args) { return this.call("key_check_make", args); }
        verify(args) { return this.call("verify", args); }
        /** Each call that trusts a member list or wraps to a key takes the user's key_checks rows. */
        chainVerify(args) { return this.call("chain_verify", args); }
        keyVersionFirst(args) { return this.call("key_version_first", args); }
        keyVersionNext(args) { return this.call("key_version_next", args); }
        vaultKeyNew(args) { return this.call("vault_key_new", args); }
        vaultKeyWrapMember(args) { return this.call("vault_key_wrap_member", args); }
        vaultKeyOpen(args) { return this.call("vault_key_open", args); }
        vaultKeyPasswordWrap(args) { return this.call("vault_key_password_wrap", args); }
        vaultKeyPasswordOpen(args) { return this.call("vault_key_password_open", args); }
        vaultRotate(args) { return this.call("vault_rotate", args); }
        depositKeyNew(args) { return this.call("deposit_key_new", args); }
        /** A deposited item taken in: its key rewrapped by the vault key ({vault_id, item_id, key_version, label, deposit_wrapped_private, wrapped_item_key}). */
        depositAdopt(args) { return this.call("deposit_adopt", args); }
        itemSeal(args) { return this.call("item_seal", args); }
        itemOpen(args) { return this.call("item_open", args); }
        leaseApprove(args) { return this.call("lease_approve", args); }
        recoveryMake(args) { return this.call("recovery_make", args); }
        recoveryMember(args) { return this.call("recovery_member", args); }
        recoverySessionStart(args) { return this.call("recovery_session_start", args); }
        recoveryShareDeliver(args) { return this.call("recovery_share_deliver", args); }
        recoveryCombine(args) { return this.call("recovery_combine", args); }
        recoveryOpenVault(args) { return this.call("recovery_open_vault", args); }
        recoveryRekey(args) { return this.call("recovery_rekey", args); }
    }

    global.MoquiVaultClient = MoquiVaultClient;
})(typeof window !== "undefined" ? window : self);
