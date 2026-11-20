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
 * The page fetches the worker's script and the module itself, each with its integrity hash when one is given (and that
 * hash in its URL, ?v=, so a copy cached before an upgrade is not taken), then starts the worker from a blob: URL (the
 * vault screens' policy, vault@screen-policy, allows worker-src blob: and 'wasm-unsafe-eval') and
 * hands it the module's bytes. The vault locks after `idleMinutes` with no call (15 by default) and on lock(); a lock
 * zeroes the module's memory in the worker.
 *
 * In the desktop and mobile apps (plans/Vault-Instances.md V5) the page bridge has window.moquiDesktop.vault: the app
 * runs the same module natively, one session per account, and the client calls it there with the same ops. It then
 * loads no worker and no module from the server (the app refuses them), the app keeps what it has seen of each vault
 * (rollback memory), and device memory (deviceMemory, remember, restore, forget) can keep the personal key open for a
 * time under a key in the device's key store.
 */
"use strict";
(function (global) {
    // the app's native vault, when the page runs in the desktop or mobile app
    const nativeVault = () => {
        const bridge = global.moquiDesktop;
        return bridge && bridge.vault && typeof bridge.vault.call === "function" ? bridge.vault : null;
    };
    const here = (global.document && global.document.currentScript && global.document.currentScript.src) || "/js/vault/VaultClient.js";
    // A file beside this one, named by its integrity hash when it has one (?v=<hash>): a copy the browser cached before an
    // upgrade is then never asked for under the new hash, which it would fail.
    const beside = (file, integrity) => {
        const url = new URL(file, typeof location !== "undefined" && location.href ? new URL(here, location.href) : here);
        if (integrity) url.searchParams.set("v", String(integrity).replace(/^sha256-/, ""));
        return url.href;
    };

    class MoquiVaultClient {
        constructor(options) {
            const o = options || {};
            this.workerIntegrity = o.workerIntegrity || null;
            this.wasmIntegrity = o.wasmIntegrity || null;
            this.workerUrl = o.workerUrl || beside("VaultWorker.js", this.workerIntegrity);
            this.wasmUrl = o.wasmUrl || beside("moqui_vault.wasm", this.wasmIntegrity);
            this.idleMs = (o.idleMinutes == null ? 15 : o.idleMinutes) * 60000;
            this.clipboardMs = (o.clipboardSeconds == null ? 30 : o.clipboardSeconds) * 1000;
            this.onLock = o.onLock || null;
            this.worker = null;
            this.nextId = 1;
            this.waiting = new Map();
            this.idleTimer = null;
            this.native = nativeVault();
            this.nativeReady = false;
            // a browser keeps no memory of item revisions past this client (§3.12); the app keeps it
            this.revisions = new Map();
        }

        async fetchChecked(url, integrity) {
            const response = await fetch(url, integrity ? { integrity, credentials: "same-origin" } : { credentials: "same-origin" });
            if (!response.ok) throw new Error("could not load " + url + ": " + response.status);
            return response;
        }

        /** Loads the worker and the module; resolves once the module is ready. */
        async start() {
            if (this.native) {
                if (!this.nativeReady) {
                    await this.native.call("configure", { idle_minutes: Math.max(1, Math.round(this.idleMs / 60000)) });
                    this.nativeReady = true;
                }
                return;
            }
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
            if (this.native) {
                await this.start();
                this.touch();
                return this.native.call(op, args || {});
            }
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
        async lock(options) {
            if (this.idleTimer) clearTimeout(this.idleTimer);
            this.idleTimer = null;
            if (this.native) {
                // with {forget: true} (the page's Lock), the app's device memory of the key goes too
                try {
                    await this.native.call("lock", { forget: !!(options && options.forget) });
                } catch (e) {
                    // the app locks the account's session on its own as well (idle, sign-out, the device's lock)
                }
            } else if (!this.worker) {
                return;
            } else {
                try {
                    await this.post({ op: "lock" });
                } catch (e) {
                    this.terminate();
                }
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
            if (this.native) {
                this.native.call("lock", {}).catch(() => {});
                return;
            }
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
                    // a page without focus may not write the clipboard: clear it when the window has focus again
                    const clear = async () => {
                        window.removeEventListener("focus", clear);
                        try {
                            if ((await navigator.clipboard.readText()) !== text) return;
                        } catch (e2) {
                            // no permission to read: clear anyway
                        }
                        try { await navigator.clipboard.writeText(""); } catch (e3) { /* nothing more to do */ }
                    };
                    if (typeof window !== "undefined") window.addEventListener("focus", clear);
                }
            }, this.clipboardMs);
        }

        /**
         * The items of a list ({vault_id, items: [{item_id, revision}]}) whose revision is lower than one seen before
         * ({older: [item_id]}), the highest of each kept: by the app across runs, in a browser for this client only.
         */
        async revisionsSeen(args) {
            if (this.native) return this.call("revisions_seen", args);
            const vault = (args && args.vault_id) || "";
            const seen = this.revisions.get(vault) || new Map();
            this.revisions.set(vault, seen);
            const older = [];
            for (const row of (args && args.items) || []) {
                const kept = seen.get(row.item_id) || 0;
                if (kept > (row.revision || 0)) older.push(row.item_id);
                else seen.set(row.item_id, row.revision || 0);
            }
            return { older };
        }

        /** Device memory in the app: {available, kind, until, open, maxMinutes}; a browser has none. */
        async deviceMemory() {
            if (!this.native) return { available: false, kind: null, until: null, open: false };
            return this.native.memory("state");
        }
        /** The app remembers the open personal key for minutes (at most what the server allows): {until, kind}. */
        remember(minutes) { return this.native ? this.native.memory("remember", minutes) : Promise.reject(new Error("Only the desktop and mobile apps remember the key")); }
        /**
         * The personal key opened from the app's memory, once the server confirms it (and, on a phone, the device's lock
         * or biometrics): {opened: true, result} as personalOpen answers, or {opened: false, reason: none, ended,
         * unlock or unconfirmed}.
         */
        restore() { return this.native ? this.native.memory("restore") : Promise.resolve({ opened: false, reason: "none" }); }
        /** The app forgets the key it remembers, and the server's record of it. */
        forget() { return this.native ? this.native.memory("forget") : Promise.resolve({ forgotten: false }); }

        vectors() { return this.call("vectors"); }
        status() { return this.call("status"); }
        personalMake(args) { return this.call("personal_make", args); }
        personalOpen(args) { return this.call("personal_open", args); }
        personalOpenRecovery(args) { return this.call("personal_open_recovery", args); }
        /** A new key passphrase for the open personal key ({passphrase}): its new wrap to store (rewrap#PersonalKey). */
        personalRewrap(args) { return this.call("personal_rewrap", args); }
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
        /** A single-secret vault's generated key (HKDF, no Argon2id): made in the module unless given; answers {generated, wrap}. */
        vaultKeyGeneratedWrap(args) { return this.call("vault_key_generated_wrap", args); }
        vaultKeyGeneratedOpen(args) { return this.call("vault_key_generated_open", args); }
        /** Forgets one opened vault key version ({vault_id, key_version}): undoes a vault_rotate whose server call failed. */
        vaultKeyForget(args) { return this.call("vault_key_forget", args); }
        /** A personal vault's header (its name) sealed by, or opened with, the vault key of a version. */
        headerSeal(args) { return this.call("header_seal", args); }
        headerOpen(args) { return this.call("header_open", args); }
        vaultRotate(args) { return this.call("vault_rotate", args); }
        depositKeyNew(args) { return this.call("deposit_key_new", args); }
        /** A deposited item taken in: its key rewrapped by the vault key ({vault_id, item_id, key_version, label, deposit_wrapped_private, wrapped_item_key}). */
        depositAdopt(args) { return this.call("deposit_adopt", args); }
        itemSeal(args) { return this.call("item_seal", args); }
        itemOpen(args) { return this.call("item_open", args); }
        /** An item's name, type, URIs, tags and unmasked fields for a list ({...item_open's args, masked: [field names]}). */
        itemSummary(args) { return this.call("item_summary", args); }
        deliverySend(args) { return this.call("delivery_send", args); }
        deliveryOpen(args) { return this.call("delivery_open", args); }
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
