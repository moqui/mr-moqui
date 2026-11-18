/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The browser's crypto device (plans/archive/Comms-Framework.md §4.5, C7 core; docs/interfaces/Chat-Keys.md): moqui.crypto.
 *
 * matrix-sdk-crypto's OlmMachine (the vendored @matrix-org/matrix-sdk-crypto-wasm, /libs/matrix-sdk-crypto/) holds this
 * browser's device: its identity keys, one-time keys and Olm sessions, in an IndexedDB store of its own per user and
 * device. It does no I/O; this file runs its outgoing requests against the /ChatKeys transitions (keys upload, query and
 * claim, to-device send) and feeds it the to-device sync. Nothing leaves the browser but public keys and ciphertext: the
 * plaintext of a message and every private key stay in the store.
 *
 * The module is 7.8 MB, so nothing loads until a caller asks for the device (ensureDevice, sendToDevices); a page that
 * never does loads none of it. The first time in a browser for a user, the server makes a device (/ChatKeys/device) and
 * the store is made for it; its deviceId is remembered in localStorage by userId, so the next page opens the same store.
 * A device the server no longer knows or has revoked (Settings, My devices) is forgotten here: its store is deleted, and
 * the next ensureDevice makes a new one. Logout and leaving the page close the store; a store belongs to one user and
 * device, and a page of another user never opens it. One tab at a time opens a store (a Web Lock on it): two OlmMachines
 * on one store would each keep their own copy of its Olm sessions and spoil them. The tab that holds the lock is the
 * leader: the user's other tabs of this browser are followers, which ask the leader over a BroadcastChannel named for the
 * user, device and a tag made from the device's store key (moqui-crypto:<userId>:<deviceId>:<tag>) to encrypt, decrypt and
 * send for them, and hear its to-device events; each waits for the lock, so when the leader's tab closes another takes it
 * and opens the store. A tab finds the channel only after fetching the store key as the device's signed-in user, so a page
 * of another account in the same browser (which can read the deviceId from localStorage) cannot ask the leader for anything.
 *
 * At rest the store is encrypted with its store key, a passphrase the server derives for the device under a platform
 * secret and does not store (/ChatKeys/storeKey, given only to the device's own signed-in user; a revoke changes it): a
 * copy of the browser profile alone does not open it. It does not protect the store from one who can also sign in as the
 * user (or holds the server's secret), nor from script running in a page of this origin while the user is signed in. A
 * store that fails to open is tried again; one that cannot be read (a wrong key, another device's data,
 * corruption) is kept, the user is told (state.problem) and offered the backup (startOver), never deleted unasked.
 *
 * Encrypted rooms (C7): Megolm room keys are shared with the room's members' devices over Olm to-device messages before a
 * message is encrypted (encryptRoom); the machine starts a new session when a member or device it shared with is gone,
 * and rotateRoom starts one at once (a member left or was removed). decryptRoom opens a room event. Room keys are backed
 * up encrypted to a recovery key the user keeps (enableBackup, restoreBackup; the server's room_keys endpoints), and
 * cross-signing and emoji or QR verification run over to-device (setupCrossSigning, verifyDevice, verifications), with a
 * device of this user's or of another person (Chat's Verify). A verified person's new device or changed identity refuses
 * the next encryptRoom (verifiedUserProblem, with the problems) until it is verified or let through for the room
 * (acceptProblems, kept in this browser, not as the SDK's trust);
 * roomTrust gives a room's people and devices as verified or not, and onTrust says when they change (Comms-Messaging
 * §6.1 rule 3).
 * Files are encrypted before they go up (encryptFile) and decrypted after they come down (decryptFile).
 *
 * Wake-ups: a to-device message for any of the user's devices pushes {kind: 'toDevice'} on chatUser:<me>; a loaded page
 * syncs then, and when its notification socket comes back. The push carries nothing of the message.
 *
 * The API (for encrypted calls, W8):
 * - ensureDevice(): Promise<{userId, deviceId, matrixUserId}>: loads the module and the store, uploads the keys; in a
 *   desktop client's app window (elsewhere()), whose shell owns the device, it refuses with reasonCode elsewhere.
 * - elsewhere(): whether this page leaves the device to the desktop client's shell (no device here).
 * - sendToDevices(targets, eventType, content): Promise<{sent, devices, skipped}>: Olm-encrypts content as an event of
 *   eventType for each device of the targets, a list of userIds (every device of each) or {userId, deviceIds} (those
 *   devices only), and sends it; not to this device. sent is how many the server queued (a revoked device, or a user
 *   who shares no room or call with this one, gets none); devices, those it was encrypted for, and skipped, those it
 *   could not be (no session could be made, blacklisted, unknown), are lists of {userId, deviceId}.
 * - onToDevice(fn): () => void: fn({type, content, senderUserId, senderMatrixId, senderDeviceId, senderCurve25519Key,
 *   verified}) for each to-device event that decrypted (only those); returns the unsubscribe.
 * - devicesOf(userId): Promise<[{deviceId, displayName, ed25519Key, verified}]>: the devices of a user as this store knows them.
 * - sync(), close(), forget(): run a sync now; close the store; revoke this device and delete its store (a revoke the
 *   server refuses, reauthRequired, deletes nothing).
 * - startOver(): after state.problem, delete the unreadable store and drop its device; the next ensureDevice makes another.
 * - settingsChanged(): the verified-devices-only settings are read again before the next room message.
 * - thisDeviceId(): the deviceId this browser uses for the signed-in user, without loading anything (null when none).
 * - state: {status: idle, loading, ready or error, deviceId, matrixUserId, error, problem ({reasonCode storeUnreadable,
 *   deviceId}: the store cannot be read)}.
 * - setupStatus(): whether this device reads what was sent before it (plans/Comms-Messaging.md §6.5): {deviceId, backup,
 *   restored, verified, otherDevices}. A verification with another of this user's devices asks it for the secrets this
 *   device lacks (requestSecrets asks again); a backup key it sends is used when it opens the current backup, and the
 *   backup is restored with it. ensureDevice({automatic: true}) is Chat's, as it opens: a device made so is marked.
 */
(function (root) {
    "use strict";
    const moqui = (root.moqui = root.moqui || {});
    const reactive = (obj) => (root.Vue && root.Vue.reactive ? root.Vue.reactive(obj) : obj);
    const LIB = "/libs/matrix-sdk-crypto/index.mjs";
    const BASE = "/ChatKeys/";
    const ENCRYPTED = "m.room.encrypted";

    const state = reactive({ status: "idle", deviceId: null, matrixUserId: null, error: null, problem: null });
    let sdk = null;
    let machine = null;
    let me = null;
    let loading = null;
    let queue = Promise.resolve();
    let cursor = 0;
    let listCursor = null;
    let unsubscribe = null;
    let stateOff = null;
    let releaseLock = null;
    const listeners = new Set();

    const currentUser = () => (moqui.webrootVue && moqui.webrootVue.userId) || null;
    // The desktop client's app windows (window.__moquiCryptoInShell, set by the host): the app's shell owns the chat crypto
    // device, so this page makes and opens none (ensureDevice, and each call that needs the device, refuse with reasonCode
    // elsewhere) and an encrypted room says it opens in Chat. Never on the web.
    const elsewhere = () => !!root.__moquiCryptoInShell;
    const elsewhereError = () => Object.assign(new Error("Encrypted messages open in Chat, in the app's main window"),
        { reasonCode: "elsewhere", errors: "Encrypted messages open in Chat, in the app's main window" });
    // under the storage scope: in the desktop client every server's pages share one origin (MoquiLib scopedKey); the
    // store's databases and lock follow its prefix
    const pointerKey = (userId) => moqui.scopedKey("moqui.crypto.device." + userId);
    const storePrefix = (userId, deviceId) => moqui.scopedKey("moqui-crypto-" + userId + "-" + deviceId);

    function readPointer(userId) {
        try {
            const text = root.localStorage.getItem(pointerKey(userId));
            const value = text ? JSON.parse(text) : null;
            return value && value.deviceId && value.user_id ? value : null;
        } catch (err) {
            return null;
        }
    }
    function writePointer(userId, value) {
        try {
            if (value) root.localStorage.setItem(pointerKey(userId), JSON.stringify(value));
            else root.localStorage.removeItem(pointerKey(userId));
        } catch (err) {
            /* storage off: the device is made again next time */
        }
    }

    /** One request at a time to the machine and the server, in order. */
    function serial(task) {
        const run = queue.then(task, task);
        queue = run.catch(() => null);
        return run;
    }

    /** A JSON post to a /ChatKeys transition; rejects with {reasonCode, errors} on a refusal. */
    function call(name, body, method) {
        const headers = { Accept: "application/json" };
        const init = { method: method || "POST", headers, credentials: "same-origin" };
        if (init.method !== "GET") {
            headers["Content-Type"] = "application/json";
            if (moqui.sessionToken) headers["X-CSRF-Token"] = moqui.sessionToken;
            init.body = JSON.stringify(body || {});
        }
        return root.fetch(BASE + name, init).then((response) => {
            const token = response.headers.get("X-CSRF-Token");
            if (token && moqui.onSessionToken) moqui.onSessionToken(token);
            return response.text().then((text) => {
                let json = null;
                try { json = text ? JSON.parse(text) : {}; } catch (err) { json = null; }
                if (!response.ok || !json) throw { reasonCode: response.status === 401 ? "notSignedIn" : "failed", status: response.status, errors: text };
                if (json.reasonCode) throw json;
                return json;
            });
        });
    }

    // ---- Matrix user IDs, as the server makes them (ChatKeys.js localpart) ----
    const PLAIN = /^[a-z0-9.\-]$/;
    function localpart(userId) {
        let out = "";
        for (const ch of String(userId)) {
            if (PLAIN.test(ch)) out += ch;
            else if (ch === "_") out += "__";
            else if (ch >= "A" && ch <= "Z") out += "_" + ch.toLowerCase();
            else {
                const escaped = encodeURIComponent(ch);
                if (escaped.length === 1) out += "=" + (ch.charCodeAt(0) < 16 ? "0" : "") + ch.charCodeAt(0).toString(16);
                else out += escaped.split("%").filter((part) => part).map((part) => "=" + part.toLowerCase()).join("");
            }
        }
        return out;
    }
    function fromLocalpart(local) {
        let out = "";
        let bytes = "";
        for (let i = 0; i < local.length; i++) {
            const ch = local[i];
            if (ch === "=") { bytes += "%" + local.slice(i + 1, i + 3); i += 2; continue; }
            if (bytes) { out += decodeURIComponent(bytes); bytes = ""; }
            if (ch === "_") { const next = local[i + 1]; out += next === "_" ? "_" : String(next || "").toUpperCase(); i++; }
            else out += ch;
        }
        if (bytes) out += decodeURIComponent(bytes);
        return out;
    }
    const serverName = () => (state.matrixUserId ? state.matrixUserId.slice(state.matrixUserId.indexOf(":") + 1) : "localhost");
    const matrixId = (userId) => "@" + localpart(userId) + ":" + serverName();
    function userIdOf(id) {
        const text = String(id || "");
        const colon = text.indexOf(":");
        if (text[0] !== "@" || colon < 2) return null;
        try { return fromLocalpart(text.slice(1, colon)); } catch (err) { return null; }
    }

    /** Deletes the IndexedDB databases of a store (the SDK names them after the prefix). */
    function deleteStore(prefix) {
        const names = [prefix + "::matrix-sdk-crypto", prefix + "::matrix-sdk-crypto-meta"];
        const listed = root.indexedDB && root.indexedDB.databases
            ? root.indexedDB.databases().then((dbs) => dbs.map((d) => d.name).filter((n) => n && n.indexOf(prefix) === 0), () => names)
            : Promise.resolve(names);
        return listed.then((list) => Promise.all(Array.from(new Set(list.concat(names))).map((name) => new Promise((resolve) => {
            try {
                const req = root.indexedDB.deleteDatabase(name);
                req.onsuccess = req.onerror = req.onblocked = () => resolve();
            } catch (err) {
                resolve();
            }
        }))));
    }

    function deviceName() {
        // the desktop client's shell names its device for the app and the computer ("MR Moqui desktop on <host>")
        const named = root.moquiDesktop && root.moquiDesktop.deviceName;
        if (named) return String(named);
        const ua = (root.navigator && root.navigator.userAgent) || "";
        const browser = /Edg\//.test(ua) ? "Edge" : /Firefox\//.test(ua) ? "Firefox" : /Chrome\//.test(ua) ? "Chrome" : /Safari\//.test(ua) ? "Safari" : "Browser";
        const os = /Android/.test(ua) ? "Android" : /iPhone|iPad/.test(ua) ? "iOS" : /Mac OS X/.test(ua) ? "macOS" : /Windows/.test(ua) ? "Windows" : /Linux/.test(ua) ? "Linux" : "";
        return browser + (os ? " on " + os : "");
    }

    function loadSdk() {
        if (sdk) return Promise.resolve(sdk);
        // a dynamic import of the vendored ES module; it fetches its .wasm beside itself
        return import(LIB).then((mod) => mod.initAsync().then(() => (sdk = mod)));
    }

    /** The machine's requests to the server, sent in order, each marked sent with the server's answer. */
    function flush() {
        return serial(() => flushNow());
    }
    async function flushNow() {
        if (!machine) return;
        const requests = await machine.outgoingRequests();
        for (const request of requests) {
            if (uploadsHalted && request.type === sdk.RequestType.KeysUpload) continue;
            await sendRequest(request);
            if (!machine) return;
        }
        // the room keys not yet in the backup, a batch at a time
        if (machine && await machine.isBackupEnabled()) {
            for (let round = 0; machine && round < 20; round++) {
                const batch = await machine.backupRoomKeys();
                if (!batch) break;
                await sendRequest(batch);
            }
        }
    }
    async function sendRequest(request) {
        const T = sdk.RequestType;
        let answer = {};
        try {
            if (request.type === T.KeysUpload) answer = await call("upload", Object.assign({ deviceId: state.deviceId }, JSON.parse(request.body)));
            else if (request.type === T.KeysQuery) answer = await call("query", JSON.parse(request.body));
            else if (request.type === T.KeysClaim) answer = await call("claim", JSON.parse(request.body));
            else if (request.type === T.ToDevice) {
                answer = await call("send", { deviceId: state.deviceId, event_type: request.event_type, txn_id: request.txn_id,
                    messages: JSON.parse(request.body).messages || {} });
                answer = {};
            } else if (request.type === T.SignatureUpload) answer = await call("signaturesUpload", { signed: JSON.parse(request.body) });
            else if (request.type === T.KeysBackup) answer = await call("backupKeysPut", Object.assign({ version: request.version }, JSON.parse(request.body)));
            else answer = {};
        } catch (err) {
            if (err && (err.reasonCode === "notFound" || err.reasonCode === "revoked")) await gone();
            else if (request.type === T.KeysUpload && err && err.reasonCode === "tooManyKeys") {
                // the server holds as many one-time keys as it takes: the machine stops uploading more until they are claimed
                if (machine) await machine.markRequestAsSent(request.id, request.type, JSON.stringify({ one_time_key_counts: { signed_curve25519: 500 } }));
                return;
            } else if (request.type === T.KeysUpload && err && ["keysChanged", "keyExists", "badKeys", "tooLarge"].includes(err.reasonCode)) {
                // the store and the server disagree about this device's keys: never retried as it is. The device is dropped
                // once and a new one made; a second refusal stops the uploads of this page (state error)
                console.error("This browser's crypto device was refused its keys (" + err.reasonCode + ")");
                if (!uploadReset) { uploadReset = true; await resetDevice(); }
                else { uploadsHalted = true; Object.assign(state, { status: "error", error: err.errors || err.reasonCode }); }
            } else if (request.type === T.KeysBackup && err && err.reasonCode === "wrongVersion") {
                // another device made a new backup: this one stops backing up to the old one
                if (machine) await machine.disableBackup();
                return;
            }
            throw err;
        }
        if (machine) await machine.markRequestAsSent(request.id, request.type, JSON.stringify(answer));
    }

    /**
     * The server refused this device (notFound or revoked): forget it and its store, but only when the session is still
     * this user's. A session that is now another user's, or none (signed out or in again in another tab), only closes the
     * store: it stays for this user's next page.
     */
    async function gone() {
        const user = me;
        const deviceId = state.deviceId;
        const matrixUserId = state.matrixUserId;
        const sameUser = await call("whoami", null, "GET").then((who) => who.user_id === matrixUserId, () => false);
        await closeNow();
        if (sameUser && user && deviceId) {
            writePointer(user, null);
            await deleteStore(storePrefix(user, deviceId));
        }
    }

    function emit(event) {
        if (role === "leader") post({ op: "toDevice", event });
        deliver(event);
    }
    function deliver(event) {
        for (const fn of Array.from(listeners)) {
            try { fn(event); } catch (err) { console.error("A to-device listener failed", err); }
        }
    }

    /** The to-device sync: acknowledges what was handled, decrypts what came, until nothing waits. */
    function sync() {
        return serial(() => syncNow());
    }
    async function syncNow() {
        for (let round = 0; machine && round < 20; round++) {
            let res;
            try {
                res = await call("sync", { deviceId: state.deviceId, since: cursor, limit: 100, deviceListSince: listCursor });
            } catch (err) {
                if (err && (err.reasonCode === "notFound" || err.reasonCode === "revoked")) await gone();
                throw err;
            }
            if (!machine) return;
            const events = (res.to_device && res.to_device.events) || [];
            const lists = res.device_lists || {};
            const changed = (lists.changed || []).map((id) => new sdk.UserId(id));
            const counts = new Map(Object.entries(res.device_one_time_keys_count || {}));
            const fallback = new Set(res.device_unused_fallback_key_types || []);
            const processed = await machine.receiveSyncChanges(JSON.stringify(events), new sdk.DeviceLists(changed, []), counts, fallback);
            for (const p of processed) {
                if (p.type === sdk.ProcessedToDeviceEventType.PlainText) {
                    // an emoji or QR verification asked of this device (m.key.verification.request)
                    try { noteVerification(JSON.parse(p.rawEvent)); } catch (err) { /* not one */ }
                    continue;
                }
                if (p.type !== sdk.ProcessedToDeviceEventType.Decrypted) continue;
                let raw = null;
                try { raw = JSON.parse(p.rawEvent); } catch (err) { continue; }
                const info = p.encryptionInfo;
                const senderDevice = info && info.senderDevice ? info.senderDevice.toString() : null;
                emit({ type: raw.type, content: raw.content || {}, senderMatrixId: raw.sender || null, senderUserId: userIdOf(raw.sender),
                    senderDeviceId: senderDevice, senderCurve25519Key: info ? info.senderCurve25519Key : null,
                    verified: info ? !!info.isSenderVerified() : false });
            }
            cursor = Number(res.next_batch) || cursor;
            if (res.device_lists_next) listCursor = res.device_lists_next;
            // an emoji verification the other device started is accepted here, so the keys and then the emoji follow
            await acceptIncomingSas();
            await flushNow();
            // the next round acknowledges what this one handled; it ends when nothing more waits
            if (!events.length && !res.has_more) return;
        }
    }

    function onPush(jsonObj) {
        let data = jsonObj ? jsonObj.message : null;
        if (typeof data === "string") {
            try { data = JSON.parse(data); } catch (err) { data = null; }
        }
        if (!data || !machine) return;
        if (data.kind === "toDevice") sync().catch((err) => console.info("Crypto sync failed", err));
        if (data.kind === "devices" && data.action === "revoked" && data.deviceId === state.deviceId) serial(() => gone());
    }

    function listen(user) {
        const client = moqui.webrootVue && moqui.webrootVue.notificationClient;
        if (!client || unsubscribe) return;
        const off = client.registerListener("chatUser:" + user, onPush);
        unsubscribe = typeof off === "function" ? off : () => null;
        // onState calls with "open" or "closed": a sync when the socket is back, for what it missed
        const offState = client.onState ? client.onState((socketState) => { if (socketState === "open" && machine) sync().catch(() => null); }) : null;
        stateOff = typeof offState === "function" ? offState : null;
    }

    /** Takes the store's Web Lock when no other tab holds it: the function that releases it, or null (another tab has it). */
    function lockStore(prefix) {
        const locks = root.navigator && root.navigator.locks;
        if (!locks || !locks.request) return Promise.resolve(() => null);
        return new Promise((resolve, reject) => {
            locks.request("moqui-crypto-store:" + prefix, { ifAvailable: true }, (lock) => {
                if (!lock) {
                    resolve(null);
                    return null;
                }
                // held until released (closeNow) or the page goes away
                return new Promise((release) => resolve(release));
            }).catch(reject);
        });
    }

    // ---- opening the store: retried on a passing failure, never deleted unasked ----

    const STORE_TRIES = 3;
    const pause = (millis) => new Promise((resolve) => setTimeout(resolve, millis));
    /**
     * Whether a failure to open a store says the store itself cannot be read with this key: a wrong key (the store cipher's
     * MAC does not pass), a store of another account or device, or data that does not decode. A passing failure (IndexedDB
     * busy, aborted, out of space for now) says none of these and is tried again.
     */
    function definiteStoreError(err) {
        const text = String((err && (err.message || err.description || err.errors)) || err || "");
        return /MAC|ciphertext|decrypt|store cipher|doesn't match the account|deserializ|unpickl|base64|invalid key|corrupt/i.test(text);
    }
    /** Opens the store, trying a passing failure again; a definite one is {reasonCode: storeUnreadable}, with the store kept. */
    async function initializeStore(user, pointer, storeKey) {
        let last = null;
        for (let attempt = 0; attempt < STORE_TRIES; attempt++) {
            try {
                return await sdk.OlmMachine.initialize(new sdk.UserId(pointer.user_id), new sdk.DeviceId(pointer.deviceId),
                    storePrefix(user, pointer.deviceId), storeKey);
            } catch (err) {
                last = err;
                if (definiteStoreError(err)) {
                    console.error("This browser's crypto store cannot be opened", err);
                    throw { reasonCode: "storeUnreadable", errors: "This browser's encryption keys cannot be opened", deviceId: pointer.deviceId };
                }
                console.info("The crypto store did not open; trying again", err);
                await pause(400 * (attempt + 1));
            }
        }
        throw { reasonCode: "storeBusy", errors: "This browser's encryption keys did not open: " + ((last && (last.message || last.description)) || String(last)) };
    }
    /** A store that cannot be read stays as it is: the user is told (state.problem) and offered the backup (startOver). */
    function storeProblem(user, pointer, err) {
        state.problem = { reasonCode: err.reasonCode, deviceId: pointer.deviceId, userId: user, errors: err.errors };
    }

    async function open(user, retry, automatic) {
        let pointer = readPointer(user);
        if (!pointer) {
            const made = await call("device", automatic ? { displayName: deviceName(), automatic: true } : { displayName: deviceName() });
            pointer = { deviceId: made.deviceId, user_id: made.user_id };
            writePointer(user, pointer);
        }
        if (state.problem && state.problem.deviceId === pointer.deviceId) throw state.problem;
        // the store key first: the leader opens the store with it, and a follower finds the leader's channel by it
        let keys = null;
        try {
            keys = await storeKeysOf(pointer);
        } catch (err) {
            if (err && (err.reasonCode === "notFound" || err.reasonCode === "revoked")) {
                writePointer(user, null);
                await deleteStore(storePrefix(user, pointer.deviceId));
                if (!retry) return open(user, true);
            }
            throw err;
        }
        const release = await lockStore(storePrefix(user, pointer.deviceId));
        // another tab holds the store: this one follows it, and takes the store when that tab goes
        if (!release) return follow(user, pointer);
        releaseLock = release;
        try {
            machine = await initializeStore(user, pointer, keys.storeKey);
            wire();
        } catch (err) {
            unlock();
            if (err && err.reasonCode === "storeUnreadable") storeProblem(user, pointer, err);
            throw err;
        }
        await started(user, pointer, retry);
    }

    /** The store is open as pointer's device: this tab leads, syncs and resumes the backup. */
    async function started(user, pointer, retry) {
        me = user;
        cursor = 0;
        listCursor = null;
        Object.assign(state, { status: "ready", deviceId: pointer.deviceId, matrixUserId: pointer.user_id, error: null, role: "leader" });
        listen(user);
        lead(user, pointer);
        try {
            await resumeBackup();
            await flushNow();
            await syncNow();
            await secretsWaiting();
        } catch (err) {
            if (!machine && !retry) return open(user, true);
            throw err;
        }
    }

    // options.automatic: Chat asks for the device as it opens (plans/Comms-Messaging.md §6.5); a device made by it is marked
    // so on the server, where it counts toward the encryption default only once it was used on another day or verified
    function ensureDevice(options) {
        if (elsewhere()) return Promise.reject(elsewhereError());
        const user = currentUser();
        if (!user) return Promise.reject(new Error("Sign in first"));
        if (machine && me === user && state.status === "ready") return Promise.resolve(identity());
        if (role === "follower" && me === user) return Promise.resolve(identity());
        if (loading) return loading;
        state.status = "loading";
        loading = serial(async () => {
            if (machine && me !== user) await closeNow();
            await loadSdk();
            if (!machine && role !== "follower") await open(user, false, !!(options && options.automatic));
            return identity();
        }).then((out) => {
            loading = null;
            return out;
        }, (err) => {
            loading = null;
            Object.assign(state, { status: "error", error: (err && (err.errors || err.message)) || String(err) });
            throw err;
        });
        return loading;
    }
    const identity = () => ({ userId: me, deviceId: state.deviceId, matrixUserId: state.matrixUserId });

    function normalize(targets) {
        const out = new Map();
        for (const t of Array.from(targets || [])) {
            const userId = typeof t === "string" ? t : t && t.userId;
            if (!userId) continue;
            const deviceIds = typeof t === "string" || !t.deviceIds ? null : Array.from(t.deviceIds).map(String);
            const prior = out.get(userId);
            if (prior === undefined) out.set(userId, deviceIds);
            else if (prior !== null) out.set(userId, deviceIds === null ? null : prior.concat(deviceIds));
        }
        return out;
    }

    async function sendToDevicesNow(targets, eventType, content) {
        await ensureDevice();
        const wanted = normalize(targets);
        return serial(async () => {
            if (!machine) throw new Error("The crypto device is closed");
            // a sync first: the device lists that changed (a device added or revoked) are queried again before encrypting
            await syncNow();
            if (!machine) throw new Error("The crypto device is closed");
            const users = Array.from(wanted.keys()).map((userId) => new sdk.UserId(matrixId(userId)));
            await machine.updateTrackedUsers(users.map((u) => u.clone()));
            await flushNow();
            const claim = await machine.getMissingSessions(users.map((u) => u.clone()));
            if (claim) await sendRequest(claim);
            const messages = {};
            const devices = [];
            const skipped = [];
            for (const [userId, deviceIds] of wanted) {
                const mid = matrixId(userId);
                const userDevices = await machine.getUserDevices(new sdk.UserId(mid), 5);
                for (const device of userDevices.devices()) {
                    const deviceId = device.deviceId.toString();
                    if (deviceIds && !deviceIds.includes(deviceId)) continue;
                    if (userId === me && deviceId === state.deviceId) continue;
                    if (device.isBlacklisted() || device.isDeleted()) {
                        skipped.push({ userId, deviceId });
                        continue;
                    }
                    try {
                        const encrypted = await device.encryptToDeviceEvent(eventType, content);
                        (messages[mid] = messages[mid] || {})[deviceId] = JSON.parse(encrypted);
                        devices.push({ userId, deviceId });
                    } catch (err) {
                        skipped.push({ userId, deviceId });
                    }
                }
                for (const deviceId of deviceIds || []) {
                    if (!devices.some((d) => d.userId === userId && d.deviceId === deviceId) && !skipped.some((d) => d.userId === userId && d.deviceId === deviceId)) {
                        skipped.push({ userId, deviceId });
                    }
                }
            }
            let sent = 0;
            if (devices.length) {
                const txn = "m" + Date.now().toString(36) + Math.random().toString(36).slice(2, 10);
                const answer = await call("send", { deviceId: state.deviceId, event_type: ENCRYPTED, txn_id: txn, messages });
                sent = Number(answer.queued) || 0;
            }
            return { sent, devices, skipped };
        });
    }

    async function devicesOfNow(userId) {
        await ensureDevice();
        return serial(async () => {
            const list = await machine.getUserDevices(new sdk.UserId(matrixId(userId)), 5);
            return list.devices().map((d) => ({ deviceId: d.deviceId.toString(), displayName: d.displayName || null,
                ed25519Key: d.ed25519Key ? d.ed25519Key.toBase64() : null,
                verified: !!d.isVerified() }));
        });
    }

    function onToDevice(fn) {
        if (typeof fn !== "function") return () => null;
        listeners.add(fn);
        return () => listeners.delete(fn);
    }

    function unlock() {
        const release = releaseLock;
        releaseLock = null;
        if (release) release();
    }
    async function closeNow() {
        if (unsubscribe) { try { unsubscribe(); } catch (err) { /* the socket is gone */ } }
        unsubscribe = null;
        if (stateOff) { try { stateOff(); } catch (err) { /* the socket is gone */ } }
        stateOff = null;
        const m = machine;
        machine = null;
        me = null;
        stopRelay();
        cursor = 0;
        listCursor = null;
        Object.assign(state, { status: "idle", deviceId: null, matrixUserId: null, error: null });
        if (m) { try { m.close(); } catch (err) { /* already closed */ } }
        unlock();
    }
    function close() {
        return serial(() => closeNow());
    }

    /**
     * Revokes this browser's device on the server and deletes its store; the next ensureDevice makes a new one. A revoke the
     * server refuses (reauthRequired: no sign-in in the last five minutes) deletes nothing.
     */
    function forgetNow() {
        const user = currentUser();
        const pointer = user ? readPointer(user) : null;
        return serial(async () => {
            if (pointer) {
                try {
                    await call("revoke", { deviceId: pointer.deviceId });
                } catch (err) {
                    if (!err || (err.reasonCode !== "notFound" && err.reasonCode !== "revoked")) throw err;
                }
            }
            await closeNow();
            if (pointer) {
                writePointer(user, null);
                await deleteStore(storePrefix(user, pointer.deviceId));
            }
        });
    }

    /**
     * After state.problem (this browser's store cannot be opened): its store is deleted and its device revoked, and the next
     * ensureDevice makes a new one, whose older messages come from the key backup (restoreBackup). Asked of the user first,
     * never done unasked; a revoke the server refuses (reauthRequired) leaves the old device in My devices.
     */
    function startOverNow() {
        const user = currentUser();
        const problem = state.problem;
        return serial(async () => {
            if (!problem || problem.userId !== user) return false;
            await closeNow();
            const pointer = readPointer(user);
            if (pointer && pointer.deviceId === problem.deviceId) writePointer(user, null);
            await deleteStore(storePrefix(user, problem.deviceId));
            state.problem = null;
            await call("revoke", { deviceId: problem.deviceId }).catch((err) => console.info("The unreadable device was not removed", err));
            return true;
        });
    }

    function thisDeviceId() {
        const user = currentUser();
        const pointer = user ? readPointer(user) : null;
        return pointer ? pointer.deviceId : null;
    }

    // ---- the leader tab and its followers ----

    // leader: this tab holds the store (its Web Lock); follower: another tab of this browser does, and this one asks it
    let role = null;
    let channel = null;
    let lockAbort = null;
    // a refused key upload is never retried as it is: the device is made again once, then the uploads stop (state error)
    let uploadReset = false;
    let uploadsHalted = false;
    const tabId = "t" + Math.random().toString(36).slice(2, 10);
    const pending = new Map();
    let callSeq = 0;
    const RELAY_WAIT = 30000;
    // the functions a follower may ask of the leader, by name (no prototype: a name such as constructor is no function here)
    const relayed = Object.create(null);
    // the channel's name ends with a tag made from the device's store key, so only a page that can fetch that key (signed
    // in as the device's user: /ChatKeys/storeKey) finds the leader; a page of another account of this browser does not
    let channelTag = null;
    const roomKeyListeners = new Set();

    function post(message) {
        if (!channel) return;
        try { channel.postMessage(message); } catch (err) { console.info("A crypto relay message was not posted", err); }
    }
    const plainError = (err) => (err && typeof err === "object"
        ? { reasonCode: err.reasonCode || "failed", errors: err.errors || err.message || String(err) } : { reasonCode: "failed", errors: String(err) });
    const closedError = () => ({ reasonCode: "closed", errors: "The crypto device is closed" });

    /** The channel tag of a store key: the first 128 bits of SHA-256("moqui-crypto-channel:" + key), in hex. */
    async function tagOf(storeKey) {
        const subtle = root.crypto && root.crypto.subtle;
        if (!storeKey || !subtle || !root.TextEncoder) return null;
        const digest = new Uint8Array(await subtle.digest("SHA-256", new root.TextEncoder().encode("moqui-crypto-channel:" + storeKey)));
        return Array.from(digest.slice(0, 16), (b) => b.toString(16).padStart(2, "0")).join("");
    }
    /**
     * The store key of this user's device, {storeKey} (a refusal rejects: notFound, revoked), and the
     * channel tag made from the store key.
     */
    async function storeKeysOf(pointer) {
        const answer = await call("storeKey", { deviceId: pointer.deviceId });
        const keys = { storeKey: answer.storeKey || null };
        channelTag = await tagOf(keys.storeKey);
        return keys;
    }

    function openChannel(user, deviceId, onMessage) {
        if (channel) { try { channel.close(); } catch (err) { /* closed */ } channel = null; }
        // no tag (no store key, or no Web Crypto): no relay; each tab waits for the store instead
        if (!root.BroadcastChannel || !channelTag) return;
        channel = new root.BroadcastChannel("moqui-crypto:" + user + ":" + deviceId + ":" + channelTag);
        channel.onmessage = (e) => onMessage(e.data && typeof e.data === "object" ? e.data : {});
    }

    /** This tab holds the store: it answers the other tabs' calls, and runs those it asked while it followed. */
    function lead(user, pointer) {
        role = "leader";
        openChannel(user, pointer.deviceId, (msg) => {
            if (msg.op !== "call" || typeof msg.name !== "string" || typeof relayed[msg.name] !== "function" || typeof msg.id !== "string"
                || typeof msg.tab !== "string" || (msg.args != null && !Array.isArray(msg.args))) return;
            Promise.resolve().then(() => relayed[msg.name].apply(null, msg.args || [])).then(
                (value) => post({ op: "result", id: msg.id, tab: msg.tab, ok: true, value }),
                (err) => post({ op: "result", id: msg.id, tab: msg.tab, ok: false, error: plainError(err) }));
        });
        post({ op: "leader" });
        for (const [id, item] of Array.from(pending)) {
            pending.delete(id);
            clearTimeout(item.timer);
            Promise.resolve().then(() => relayed[item.name].apply(null, item.args)).then(item.resolve, item.reject);
        }
        refreshVerifications();
    }

    /** Another tab holds the store: this one asks it, hears its events, and waits for the store to be free. */
    function follow(user, pointer) {
        role = "follower";
        me = user;
        Object.assign(state, { status: "ready", deviceId: pointer.deviceId, matrixUserId: pointer.user_id, error: null, role: "follower" });
        openChannel(user, pointer.deviceId, (msg) => {
            if (msg.op === "result" && msg.tab === tabId) {
                const item = pending.get(msg.id);
                if (!item) return;
                pending.delete(msg.id);
                clearTimeout(item.timer);
                if (msg.ok) item.resolve(msg.value);
                else item.reject(msg.error);
            } else if (msg.op === "leader") {
                // a new leader: what waited is asked again
                for (const [id, item] of pending) post({ op: "call", id, tab: tabId, name: item.name, args: item.args });
            } else if (msg.op === "toDevice") deliver(msg.event);
            else if (msg.op === "roomKeys") noticeRoomKeys(msg.sessions || []);
            else if (msg.op === "trust") noticeTrust(msg.userIds || []);
            else if (msg.op === "verifications") state.verifications = msg.list || [];
        });
        const locks = root.navigator && root.navigator.locks;
        if (locks && locks.request) {
            lockAbort = root.AbortController ? new root.AbortController() : null;
            locks.request("moqui-crypto-store:" + storePrefix(user, pointer.deviceId), lockAbort ? { signal: lockAbort.signal } : {},
                () => new Promise((release) => {
                    serial(() => takeOver(user, pointer, release)).catch((err) => {
                        console.error("This tab could not take over the crypto store", err);
                        release();
                    });
                })).catch(() => null);
        }
        post({ op: "hello" });
    }

    /** The leader's tab went: this one opens the store and leads. */
    async function takeOver(user, pointer, release) {
        lockAbort = null;
        if (role !== "follower" || me !== user) { release(); return; }
        releaseLock = release;
        await loadSdk();
        const keys = await storeKeysOf(pointer);
        try {
            machine = await initializeStore(user, pointer, keys.storeKey);
        } catch (err) {
            unlock();
            if (err && err.reasonCode === "storeUnreadable") storeProblem(user, pointer, err);
            throw err;
        }
        wire();
        cursor = 0;
        listCursor = null;
        Object.assign(state, { status: "ready", error: null, role: "leader" });
        listen(user);
        lead(user, pointer);
        await resumeBackup();
        await flushNow();
        await syncNow();
    }

    function stopRelay() {
        if (lockAbort) { try { lockAbort.abort(); } catch (err) { /* done */ } }
        lockAbort = null;
        if (channel) { try { channel.close(); } catch (err) { /* closed */ } }
        channel = null;
        channelTag = null;
        for (const item of pending.values()) { clearTimeout(item.timer); item.reject(closedError()); }
        pending.clear();
        flows.clear();
        role = null;
        state.role = null;
        state.verifications = [];
    }

    function relay(name, args) {
        return new Promise((resolve, reject) => {
            const id = tabId + ":" + (++callSeq);
            const timer = setTimeout(() => {
                pending.delete(id);
                reject({ reasonCode: "busy", errors: "The tab that holds this browser's encryption did not answer" });
            }, RELAY_WAIT);
            pending.set(id, { name, args, resolve, reject, timer });
            post({ op: "call", id, tab: tabId, name, args });
        });
    }

    /** A function of the API: run in this tab when it holds the store, else asked of the tab that does. */
    function api(name, impl) {
        relayed[name] = impl;
        return async (...args) => {
            if (role !== "leader") await ensureDevice();
            return role === "follower" ? relay(name, args) : impl(...args);
        };
    }

    /** A function that opens no store of its own: run here, or asked of the leader by a follower. */
    function relayOnly(name, impl) {
        relayed[name] = impl;
        return (...args) => (role === "follower" ? relay(name, args) : impl(...args));
    }

    /** The machine's callbacks: new room keys (from a member, or a backup) let the pages decrypt what waited. */
    function wire() {
        try {
            machine.registerRoomKeyUpdatedCallback(async (infos) => {
                const sessions = Array.from(infos || []).map((info) => ({ roomId: info.roomId.toString(), sessionId: info.sessionId }));
                noticeRoomKeys(sessions);
                post({ op: "roomKeys", sessions });
            });
        } catch (err) { console.info("Room key updates are not heard", err); }
        // a user's devices or identity changed (a new device, a verification, a new identity): the open rooms read their
        // trust again (§6.1 rule 3), in every tab
        const trustChanged = (userIds) => { noticeTrust(userIds); post({ op: "trust", userIds }); };
        try {
            machine.registerDevicesUpdatedCallback(async (userIds) => trustChanged(Array.from(userIds || []).map((id) => userIdOf(String(id)))));
            machine.registerUserIdentityUpdatedCallback(async (userId) => trustChanged([userIdOf(userId.toString())]));
        } catch (err) { console.info("Trust updates are not heard", err); }
        // the backup key from a verified device of this user's, out of the sync's serial section
        try {
            machine.registerReceiveSecretCallback(async (name, value) => { if (name === BACKUP_SECRET) setTimeout(() => backupFromSecret(value), 0); });
        } catch (err) { console.info("Secrets from other devices are not heard", err); }
    }
    const trustListeners = new Set();
    function noticeTrust(userIds) {
        for (const fn of Array.from(trustListeners)) {
            try { fn(userIds || []); } catch (err) { console.error("A trust listener failed", err); }
        }
    }
    /** fn([userId]) when users' devices or identities change (read the room's trust again); returns the unsubscribe. */
    function onTrust(fn) {
        if (typeof fn !== "function") return () => null;
        trustListeners.add(fn);
        return () => trustListeners.delete(fn);
    }
    function noticeRoomKeys(sessions) {
        for (const fn of Array.from(roomKeyListeners)) {
            try { fn(sessions); } catch (err) { console.error("A room key listener failed", err); }
        }
    }
    /** fn([{roomId, sessionId}]) when room keys arrive (decrypt again what did not); returns the unsubscribe. */
    function onRoomKeys(fn) {
        if (typeof fn !== "function") return () => null;
        roomKeyListeners.add(fn);
        return () => roomKeyListeners.delete(fn);
    }

    /** The store's device is not the server's (its keys were refused): revoked, its store deleted; the next open makes another. */
    async function resetDevice() {
        const user = me;
        const deviceId = state.deviceId;
        await call("revoke", { deviceId }).catch(() => null);
        await closeNow();
        if (user && deviceId) {
            writePointer(user, null);
            await deleteStore(storePrefix(user, deviceId));
        }
    }

    // ---- encrypted rooms (Megolm) ----

    const roomIdOf = (chatRoomId) => "!" + chatRoomId + ":" + serverName();
    const eventIdOf = (chatRoomId, eventSeq) => "$" + chatRoomId + "." + eventSeq + ":" + serverName();

    /**
     * The Megolm ciphertext (m.room.encrypted content) of an event of eventType with content, for the room's members
     * (userIds; this user is added). The room key goes first to every device of each member that has none (Olm to-device);
     * the machine starts a new session when a member or a device it was shared with is gone.
     */
    // Only verified devices: the user's setting (every encrypted room) or the room's (its moderators'), read at most every
    // 30 seconds per room. On, the room's keys go only to verified or cross-signed devices (onlyTrustedDevices); a change
    // starts a new session, so a device that had the key under the other setting gets none of what follows.
    const SETTINGS_MS = 30000;
    const roomSettings = new Map();
    const roomStrategy = new Map();
    // the devices the last message of each room was not shared with, under only verified devices (COM-UI-013)
    const roomLeftOut = new Map();
    async function verifiedOnly(chatRoomId) {
        const known = roomSettings.get(chatRoomId);
        if (known && Date.now() - known.at < SETTINGS_MS) return known.on;
        const out = await call("settings?chatRoomId=" + encodeURIComponent(chatRoomId), null, "GET").catch(() => null);
        const on = out ? !!(out.verifiedDevicesOnly || out.roomVerifiedDevicesOnly) : !!(known && known.on);
        roomSettings.set(chatRoomId, { on, at: Date.now() });
        return on;
    }
    /** The settings are read again before the next message (Settings, My devices and the room's settings call it). */
    function settingsChanged() { roomSettings.clear(); }

    async function encryptRoomNow(chatRoomId, memberUserIds, eventType, content) {
        await ensureDevice();
        const onlyVerified = await verifiedOnly(chatRoomId);
        return serial(async () => {
            if (!machine) throw closedError();
            await syncNow();
            if (!machine) throw closedError();
            const ids = Array.from(new Set(Array.from(memberUserIds || []).concat([me]).filter((id) => id)));
            const users = () => ids.map((id) => new sdk.UserId(matrixId(id)));
            await machine.updateTrackedUsers(users());
            await flushNow();
            const claim = await machine.getMissingSessions(users());
            if (claim) await sendRequest(claim);
            // a session this page did not start under the setting may have gone to other devices: a new one, once
            if (roomStrategy.has(chatRoomId) ? roomStrategy.get(chatRoomId) !== onlyVerified : onlyVerified) {
                await machine.invalidateGroupSession(new sdk.RoomId(roomIdOf(chatRoomId)));
            }
            roomStrategy.set(chatRoomId, onlyVerified);
            // §6.1 rule 3: a verified person's new device, or a new identity of a person who was verified, is said before the
            // send (refused with the problems: Verify, or Send anyway, acceptProblems); only verified devices leaves them out
            // This check is the gate: once it passes, the keys go to all devices (allDevices), so a device the user let
            // through for this room (acceptProblems, kept here, not as the SDK's trust) or verified by emoji gets them. Only
            // verified devices stays the SDK's strict onlyTrustedDevices.
            if (!onlyVerified) {
                const problems = await verifiedUserProblems(ids, chatRoomId);
                if (problems.length) throw { reasonCode: "verifiedUserProblem", errors: "A verified person has a new device or identity", problems };
            }
            const settings = new sdk.EncryptionSettings();
            settings.historyVisibility = sdk.HistoryVisibility.Shared;
            settings.sharingStrategy = onlyVerified ? sdk.CollectStrategy.onlyTrustedDevices() : sdk.CollectStrategy.allDevices();
            // a room with disappearing messages (plans/Comms-Messaging.md §18.3 phase 16): a session lasts the timer (a week at
            // most) or 100 messages, so no key outlives the messages it opens
            const timer = root.moqui && root.moqui.chat && root.moqui.chat.expiry ? root.moqui.chat.expiry.timerOf(chatRoomId) : 0;
            if (timer > 0) {
                settings.rotationPeriod = BigInt(Math.min(timer, 604800)) * 1000000n;
                settings.rotationPeriodMessages = 100n;
            }
            // who the strategy leaves out: the devices that are not verified (the machine tells each one why, m.unverified)
            roomLeftOut.set(chatRoomId, onlyVerified ? (await devicesIn(ids)).filter((device) => !device.verified)
                .map((device) => ({ userId: device.userId, deviceId: device.deviceId, displayName: device.displayName })) : []);
            let requests;
            try {
                requests = await machine.shareRoomKey(new sdk.RoomId(roomIdOf(chatRoomId)), users(), settings);
            } catch (err) {
                // the SDK's own words are not shown: logged, and said plainly
                console.error("The room key was not shared", err);
                throw { reasonCode: "shareFailed", errors: "This message could not be encrypted for the devices of this room" };
            }
            for (const request of requests) await sendRequest(request);
            const text = await machine.encryptRoomEvent(new sdk.RoomId(roomIdOf(chatRoomId)), eventType, JSON.stringify(content || {}));
            await flushNow();
            return JSON.parse(text);
        });
    }

    /**
     * An encrypted room event opened: item {eventSeq, sentDate, senderUserId, encrypted} as the timeline gives it. Resolves to
     * {type, content, verified} or, when it cannot be read (no key for it on this device), {error}. A key that waits in the
     * to-device queue is fetched once first.
     */
    async function decryptRoomNow(chatRoomId, item) {
        await ensureDevice();
        const event = JSON.stringify({ type: ENCRYPTED, event_id: eventIdOf(chatRoomId, item.eventSeq), sender: matrixId(item.senderUserId),
            origin_server_ts: Number(item.sentDate) || 0, room_id: roomIdOf(chatRoomId), content: item.encrypted || {} });
        const once = () => serial(async () => {
            if (!machine) throw closedError();
            const out = await machine.decryptRoomEvent(event, new sdk.RoomId(roomIdOf(chatRoomId)), new sdk.DecryptionSettings(sdk.TrustRequirement.Untrusted));
            const plain = JSON.parse(out.event);
            let verified = false;
            try { verified = out.shieldState(false).color === sdk.ShieldColor.None; } catch (err) { verified = false; }
            return { type: plain.type || null, content: plain.content || {}, verified };
        });
        // a key its sender withheld says why (m.unverified: this device is not verified for them; COM-UI-013)
        const withheldOf = (err) => { try { return (err && err.withheldCode) || null; } catch (e) { return null; } };
        let failed;
        try {
            return await once();
        } catch (err) {
            failed = err;
        }
        try {
            await sync();
        } catch (err) {
            // the sync failed: the first answer says why it could not be read
            return { error: (failed && (failed.description || failed.errors || failed.message)) || "This message cannot be read on this device", withheld: withheldOf(failed) };
        }
        try {
            return await once();
        } catch (again) {
            return { error: (again && (again.description || again.errors || again.message)) || "This message cannot be read on this device",
                withheld: withheldOf(again) || withheldOf(failed) };
        }
    }

    // Send anyway (§6.1 rule 3): what the user let through, per account and room, kept in this browser (never as the SDK's
    // trust, which onlyTrustedDevices would take as verified): {roomId: [{userId, deviceId, key} | {userId, masterKey}]}
    const allowedKey = () => (moqui.scopedKey ? moqui.scopedKey("moqui.crypto.allowed." + me) : "moqui.crypto.allowed." + me);
    function readAllowed() {
        try { return JSON.parse(root.localStorage.getItem(allowedKey()) || "{}") || {}; } catch (err) { return {}; }
    }
    function writeAllowed(all) {
        try { root.localStorage.setItem(allowedKey(), JSON.stringify(all)); } catch (err) { /* this page only: asked again next time */ }
    }
    const keyOf = (device) => { try { return device.ed25519Key ? device.ed25519Key.toBase64() : null; } catch (err) { return null; } };
    const masterKeyOf = (identity) => { try { return identity.masterKey ? String(identity.masterKey) : null; } catch (err) { return null; } };

    /**
     * The problems among userIds before a send to chatRoomId (§6.1 rule 3): a device that is not verified (by emoji here, or
     * signed by its owner whose identity is verified), of a person who is verified (their identity, or a device of theirs
     * verified by emoji), and not let through for this room with the same key (newDevice); and a person verified before
     * whose identity changed, not let through with this master key (changedIdentity). [{userId, deviceId, displayName, kind,
     * key | masterKey}].
     */
    async function verifiedUserProblems(userIds, chatRoomId) {
        const people = [];
        for (const userId of userIds) {
            const id = new sdk.UserId(matrixId(userId));
            const identity = await machine.getIdentity(id);
            const ask = (run) => { try { return !!run(); } catch (err) { return false; } };
            const devices = [];
            for (const device of (await machine.getUserDevices(id, 5)).devices()) {
                if (device.isDeleted() || device.isBlacklisted() || (userId === me && device.deviceId.toString() === state.deviceId)) continue;
                await unsetIgnored(device);
                devices.push({ deviceId: device.deviceId.toString(), displayName: device.displayName || null, key: keyOf(device),
                    verified: !!device.isVerified(), locallyTrusted: !!device.isLocallyTrusted() });
            }
            people.push({ userId, devices, identity: identity ? { verified: ask(() => identity.isVerified()),
                changed: ask(() => identity.hasVerificationViolation && identity.hasVerificationViolation()), masterKey: masterKeyOf(identity) } : null });
        }
        return problemsOf(people, readAllowed()[chatRoomId] || []);
    }
    /**
     * The rule of verifiedUserProblems on plain data (a regression test runs it): people [{userId, identity: {verified,
     * changed, masterKey} | null, devices: [{deviceId, displayName, key, verified, locallyTrusted}]}] (this device left
     * out), allowed the room's let-through list.
     */
    function problemsOf(people, allowed) {
        const out = [];
        for (const person of people || []) {
            const userId = person.userId;
            const identity = person.identity;
            if (identity && identity.changed) {
                if (!allowed.some((one) => one.userId === userId && one.masterKey && one.masterKey === identity.masterKey)) {
                    out.push({ userId, deviceId: null, displayName: null, kind: "changedIdentity", masterKey: identity.masterKey || null });
                }
                continue;
            }
            const devices = person.devices || [];
            const verifiedPerson = (!!identity && !!identity.verified) || devices.some((device) => device.locallyTrusted);
            if (!verifiedPerson) continue;
            for (const device of devices) {
                if (device.verified || device.locallyTrusted) continue;
                if (allowed.some((one) => one.userId === userId && one.deviceId === device.deviceId && one.key && one.key === device.key)) continue;
                out.push({ userId, deviceId: device.deviceId, displayName: device.displayName || null, kind: "newDevice", key: device.key || null });
            }
        }
        return out;
    }
    // the build of 1de64af65 let devices through as the SDK's Ignored, which onlyTrustedDevices shares with as if verified:
    // put back, so a device is verified only when someone verified it
    async function unsetIgnored(device) {
        try { if (device.localTrustState === sdk.LocalTrust.Ignored) await device.setLocalTrust(sdk.LocalTrust.Unset); } catch (err) { /* left as it is */ }
    }

    /**
     * Send anyway (§6.1 rule 3): each problem let through for chatRoomId, in this browser for this account: a new device
     * (by its key) then gets this room's keys from this message on and stays not verified; a changed identity (by its
     * master key) is let through here and stays not verified until verified again. The SDK's trust is not changed.
     */
    // localStorage is the browser's, so any tab writes it (no relay)
    async function acceptProblemsNow(chatRoomId, problems) {
        if (!chatRoomId || !me) throw { reasonCode: "badRequest", errors: "No room or no device" };
        const all = readAllowed();
        const list = all[chatRoomId] = Array.isArray(all[chatRoomId]) ? all[chatRoomId] : [];
        for (const problem of Array.from(problems || [])) {
            if (!problem || !problem.userId) continue;
            if (problem.kind === "changedIdentity" && problem.masterKey) list.push({ userId: problem.userId, masterKey: problem.masterKey });
            else if (problem.deviceId && problem.key) list.push({ userId: problem.userId, deviceId: problem.deviceId, key: problem.key });
        }
        // the newest 200 of a room
        all[chatRoomId] = list.slice(-200);
        writeAllowed(all);
        return true;
    }

    /** The devices of users (userIds) as this machine knows them, this device left out: [{userId, deviceId, displayName, verified}]. */
    async function devicesIn(userIds) {
        const out = [];
        for (const userId of userIds) {
            const list = await machine.getUserDevices(new sdk.UserId(matrixId(userId)), 5);
            for (const device of list.devices()) {
                const deviceId = device.deviceId.toString();
                if ((userId === me && deviceId === state.deviceId) || device.isDeleted()) continue;
                await unsetIgnored(device);
                out.push({ userId, deviceId, displayName: device.displayName || null, verified: !!device.isVerified() && !device.isBlacklisted() });
            }
        }
        return out;
    }
    /** A person's identity as this machine knows it: {verified, changed (a verified person's new identity), known}. */
    async function identityOf(userId) {
        const identity = await machine.getIdentity(new sdk.UserId(matrixId(userId)));
        if (!identity) return { known: false, verified: false, changed: false };
        const ask = (run) => { try { return !!run(); } catch (err) { return false; } };
        return { known: true, verified: ask(() => identity.isVerified()), changed: ask(() => identity.hasVerificationViolation && identity.hasVerificationViolation()) };
    }

    /**
     * The trust of an encrypted room's devices (§6.2's labels, §6.1 rule 3, COM-UI-013): {verifiedOnly (the user's or the
     * room's setting), users: [{userId, identity: {known, verified, changed}, devices: [{deviceId, displayName, verified}]}]},
     * the members' device lists queried first.
     */
    async function roomTrustNow(chatRoomId, memberUserIds) {
        await ensureDevice();
        const onlyVerified = await verifiedOnly(chatRoomId);
        return serial(async () => {
            if (!machine) throw closedError();
            const ids = Array.from(new Set(Array.from(memberUserIds || []).concat([me]).filter((id) => id)));
            await machine.updateTrackedUsers(ids.map((id) => new sdk.UserId(matrixId(id))));
            await flushNow();
            const devices = await devicesIn(ids);
            const identities = {};
            for (const userId of ids) identities[userId] = await identityOf(userId);
            return { verifiedOnly: onlyVerified, users: ids.map((userId) => ({ userId, identity: identities[userId], devices: devices.filter((device) => device.userId === userId)
                .map((device) => ({ deviceId: device.deviceId, displayName: device.displayName, verified: device.verified })) })) };
        });
    }
    /** The devices the last message this page encrypted for a room was not shared with ([] when it went to all). */
    function leftOutNow(chatRoomId) { return Promise.resolve(roomLeftOut.get(chatRoomId) || []); }

    /** A new Megolm session for the room's next message: a member left or was removed. */
    async function rotateRoomNow(chatRoomId) {
        await ensureDevice();
        return serial(async () => (machine ? !!(await machine.invalidateGroupSession(new sdk.RoomId(roomIdOf(chatRoomId)))) : false));
    }

    // ---- encrypted files (no store needed: any tab) ----

    /** A file encrypted for upload: {blob (application/octet-stream), info (the media encryption info: key, iv, hashes)}. */
    async function encryptFile(blob) {
        await loadSdk();
        const sealed = sdk.Attachment.encrypt(new Uint8Array(await blob.arrayBuffer()));
        return { blob: new root.Blob([sealed.encryptedData], { type: "application/octet-stream" }), info: JSON.parse(sealed.mediaEncryptionInfo) };
    }
    /** The bytes of an encrypted file (an ArrayBuffer or Uint8Array) with its info; rejects when its hash does not match. */
    async function decryptFile(data, info) {
        await loadSdk();
        const bytes = data instanceof Uint8Array ? data : new Uint8Array(data);
        return sdk.Attachment.decrypt(new sdk.EncryptedAttachment(bytes, JSON.stringify(info)));
    }

    // ---- key backup and the recovery key ----

    const BACKUP_ALGORITHM = "m.megolm_backup.v1.curve25519-aes-sha2";
    const B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    function base58(bytes) {
        const digits = [0];
        for (const byte of bytes) {
            let carry = byte;
            for (let j = 0; j < digits.length; j++) { carry += digits[j] << 8; digits[j] = carry % 58; carry = (carry / 58) | 0; }
            while (carry) { digits.push(carry % 58); carry = (carry / 58) | 0; }
        }
        let out = "";
        for (const byte of bytes) { if (byte === 0) out += "1"; else break; }
        for (let i = digits.length - 1; i >= 0; i--) out += B58[digits[i]];
        return out;
    }
    function unbase58(text) {
        const bytes = [0];
        for (const ch of text) {
            const value = B58.indexOf(ch);
            if (value < 0) return null;
            let carry = value;
            for (let j = 0; j < bytes.length; j++) { carry += bytes[j] * 58; bytes[j] = carry & 0xff; carry >>= 8; }
            while (carry) { bytes.push(carry & 0xff); carry >>= 8; }
        }
        for (const ch of text) { if (ch === "1") bytes.push(0); else break; }
        return bytes.reverse();
    }
    function fromBase64(text) {
        const plain = String(text).replace(/-/g, "+").replace(/_/g, "/");
        const binary = root.atob(plain + "===".slice((plain.length + 3) % 4));
        return Array.from(binary, (ch) => ch.charCodeAt(0));
    }
    const toBase64 = (bytes) => root.btoa(String.fromCharCode.apply(null, bytes)).replace(/=+$/, "");
    /** The recovery key as the user keeps it: Matrix's encoding (0x8B 0x01, the key, a parity byte, base58, in groups of 4). */
    function recoveryText(key) {
        const bytes = [0x8b, 0x01].concat(fromBase64(key.toBase64()));
        bytes.push(bytes.reduce((parity, byte) => parity ^ byte, 0));
        return base58(bytes).match(/.{1,4}/g).join(" ");
    }
    function recoveryKeyOf(text) {
        const bytes = unbase58(String(text || "").replace(/\s+/g, ""));
        const bad = { reasonCode: "badRecoveryKey", errors: "This is not a recovery key" };
        if (!bytes || bytes.length !== 35 || bytes[0] !== 0x8b || bytes[1] !== 0x01) throw bad;
        if (bytes.reduce((parity, byte) => parity ^ byte, 0) !== 0) throw bad;
        return sdk.BackupDecryptionKey.fromBase64(toBase64(bytes.slice(2, 34)));
    }

    /** The backup this device used before, when it is still the server's current one: room keys go on to it. */
    async function resumeBackup() {
        if (!machine) return;
        try {
            const keys = await machine.getBackupKeys();
            const key = keys ? keys.decryptionKey : null;
            const version = keys ? keys.backupVersion : null;
            if (!key || !version) return;
            const current = await call("backupVersion", {}).catch(() => null);
            const publicKey = key.megolmV1PublicKey.publicKeyBase64;
            if (!current || String(current.version) !== String(version) || !current.auth_data || current.auth_data.public_key !== publicKey) return;
            await machine.enableBackupV1(publicKey, String(version));
        } catch (err) {
            console.info("The key backup was not resumed", err);
        }
    }

    /** {version (the server's current backup, or null), enabled (this device backs up to it), total, backedUp (room keys)}. */
    async function backupStatusNow() {
        await ensureDevice();
        const current = await call("backupVersion", {}).catch(() => null);
        return serial(async () => {
            if (!machine) throw closedError();
            const counts = await machine.roomKeyCounts();
            return { version: current ? String(current.version) : null, enabled: !!(await machine.isBackupEnabled()),
                total: Number(counts.total) || 0, backedUp: Number(counts.backedUp) || 0 };
        });
    }

    /**
     * A new key backup with a new recovery key, which only the user keeps: answered once, as {version, recoveryKey}. Cross-signing
     * is set up first when the user has none (an identity of another device is left as it is: never a silent reset).
     */
    async function enableBackupNow() {
        await ensureDevice();
        try { await setupCrossSigningNow(false); } catch (err) { if (!err || err.reasonCode !== "keysChanged") console.info("Cross-signing not set up", err); }
        return serial(async () => {
            if (!machine) throw closedError();
            const key = sdk.BackupDecryptionKey.createRandomKey();
            const publicKey = key.megolmV1PublicKey.publicKeyBase64;
            const authData = { public_key: publicKey };
            try { authData.signatures = JSON.parse((await machine.sign(JSON.stringify(authData))).asJSON()); } catch (err) { authData.signatures = {}; }
            const made = await call("backupVersionCreate", { algorithm: BACKUP_ALGORITHM, auth_data: authData });
            const version = String(made.version);
            await machine.saveBackupDecryptionKey(key, version);
            await machine.enableBackupV1(publicKey, version);
            await flushNow();
            return { version, recoveryKey: recoveryText(key) };
        });
    }

    /**
     * The room keys of the key backup, decrypted with the recovery key and imported, so the history of encrypted rooms opens on
     * this device; this device backs up to it from now on. Refused: badRecoveryKey, wrongRecoveryKey (not this backup's), notFound
     * (no backup). Nothing is reset when it fails.
     */
    async function restoreBackupNow(text) {
        await ensureDevice();
        await loadSdk();
        const key = recoveryKeyOf(text);
        const publicKey = key.megolmV1PublicKey.publicKeyBase64;
        // the version this recovery key opens: the current one, or an older one "New recovery key" left (it keeps its keys)
        const listed = await call("backupVersions", null, "GET");
        const match = (listed.versionList || []).find((v) => v.publicKey === publicKey);
        if (!match) throw { reasonCode: "wrongRecoveryKey", errors: "This recovery key is not the one of any of your key backups" };
        return importBackup(key, String(match.version), !!match.current);
    }
    /**
     * The room keys of backup version, decrypted with its key and imported; when it is the current version this device keeps
     * the key and backs up to it from now on. A backed-up key that does not decrypt is counted (failed) and skipped.
     */
    async function importBackup(key, version, current) {
        const publicKey = key.megolmV1PublicKey.publicKeyBase64;
        let imported = 0;
        let total = 0;
        let failed = 0;
        let offset = 0;
        for (let page = 0; page < 50; page++) {
            const got = await call("backupKeys", { version, offset });
            const byRoom = new Map();
            let seen = 0;
            for (const roomId of Object.keys(got.rooms || {})) {
                const sessions = new Map();
                const listed = (got.rooms[roomId] && got.rooms[roomId].sessions) || {};
                for (const sessionId of Object.keys(listed)) {
                    seen++;
                    const data = listed[sessionId].session_data || {};
                    try {
                        sessions.set(sessionId, JSON.parse(key.decryptV1(data.ephemeral, data.mac, data.ciphertext)));
                    } catch (err) {
                        failed++;
                        console.error("A backed-up room key of " + roomId + " did not decrypt");
                    }
                }
                if (sessions.size) byRoom.set(new sdk.RoomId(roomId), sessions);
            }
            total += seen;
            if (byRoom.size) {
                const result = await serial(() => (machine ? machine.importBackedUpRoomKeys(byRoom, null, version) : Promise.reject(closedError())));
                imported += Number(result.importedCount) || 0;
            }
            if (!got.has_more || !seen) break;
            offset += seen;
        }
        // only the current version takes keys: an older one is read, and this device goes on backing up where it did
        if (current) {
            await serial(async () => {
                if (!machine) throw closedError();
                await machine.saveBackupDecryptionKey(key, version);
                await machine.enableBackupV1(publicKey, version);
            });
        }
        noticeRoomKeys([]);
        post({ op: "roomKeys", sessions: [] });
        return { version, imported, total, failed, current };
    }

    // ---- setting up a new device (plans/Comms-Messaging.md §6.5) ----

    const BACKUP_SECRET = "m.megolm_backup.v1";
    // this device was verified with another of its user's devices here (a user with no cross-signing has no other proof)
    const setupKey = (userId, deviceId) => moqui.scopedKey("moqui.crypto.setup." + userId + "." + deviceId);
    function markSetup() {
        try { if (me && state.deviceId) root.localStorage.setItem(setupKey(me, state.deviceId), "verified"); } catch (err) { /* storage off */ }
    }
    function setupMarked() {
        try { return !!(me && state.deviceId && root.localStorage.getItem(setupKey(me, state.deviceId))); } catch (err) { return false; }
    }
    /**
     * Whether this device is set up to read what was sent before it: {deviceId, backup (the server's current backup version, or
     * null), restored (this device holds that backup's key), verified (signed by the user's verified identity, or verified here
     * with another of their devices), otherDevices [{deviceId, displayName, lastSeenDate}] (the user's others with keys, the last
     * seen first)}. moqui.chat.setupNeeded is the rule on it.
     */
    async function setupStatusNow() {
        await ensureDevice();
        const current = await call("backupVersion", {}).catch(() => null);
        const listed = await call("devices", null, "GET").catch(() => ({ deviceList: [] }));
        return serial(async () => {
            if (!machine) throw closedError();
            let restored = false;
            try {
                const keys = await machine.getBackupKeys();
                const key = keys ? keys.decryptionKey : null;
                restored = !!(current && key && current.auth_data && String(keys.backupVersion) === String(current.version)
                    && key.megolmV1PublicKey.publicKeyBase64 === current.auth_data.public_key);
            } catch (err) { restored = false; }
            let signed = false;
            try {
                const own = await machine.getDevice(new sdk.UserId(state.matrixUserId), new sdk.DeviceId(state.deviceId), 5);
                signed = !!(own && own.isCrossSigningTrusted());
            } catch (err) { signed = false; }
            // the user's other devices in use (the server's active: seen in chat_device_active_days), the last seen first
            const others = (listed.deviceList || []).filter((d) => d && d.deviceId !== state.deviceId && d.hasKeys && !d.revoked && d.active !== false)
                .sort((a, b) => (Number(b.lastSeenDate) || 0) - (Number(a.lastSeenDate) || 0))
                .map((d) => ({ deviceId: d.deviceId, displayName: d.displayName || null, createdDate: d.createdDate || null, lastSeenDate: d.lastSeenDate || null }));
            return { deviceId: state.deviceId, backup: current ? String(current.version) : null, restored, verified: signed || setupMarked(), otherDevices: others };
        });
    }
    /**
     * The backup key another of this user's verified devices sent (m.secret.send, after this device verified with it): used
     * only when it opens the server's current backup, whose keys are then imported as a restore with the recovery key does.
     */
    async function backupFromSecret(value) {
        try {
            await loadSdk();
            const key = sdk.BackupDecryptionKey.fromBase64(String(value || ""));
            const current = await call("backupVersion", {}).catch(() => null);
            if (!current || !current.auth_data || current.auth_data.public_key !== key.megolmV1PublicKey.publicKeyBase64) {
                console.info("A backup key from another device does not open the current key backup: not used");
                return null;
            }
            const out = await importBackup(key, String(current.version), true);
            await serial(async () => { if (machine) await machine.deleteSecretsFromInbox(BACKUP_SECRET); });
            return out;
        } catch (err) {
            console.error("The backup key from another device was not used", err);
            return null;
        }
    }
    // a backup key received before this page handled it (the page closed first): used once the store is open
    async function secretsWaiting() {
        let values = [];
        try { values = machine ? Array.from((await machine.getSecretsFromInbox(BACKUP_SECRET)) || []) : []; } catch (err) { values = []; }
        if (values.length) setTimeout(() => backupFromSecret(values[values.length - 1]), 0);
    }
    // a verification with another of this user's devices is done: this device asks its verified devices for the secrets
    // it lacks (the backup key, the cross-signing keys), which the SDK sends only to a verified device of the same user
    function askSecrets() {
        markSetup();
        setTimeout(() => requestSecretsNow().catch((err) => console.info("The secrets were not asked for", err)), 0);
    }
    /** The secrets this device lacks asked of the user's verified devices again (the set-up card's Try again). */
    function requestSecretsNow() {
        return serial(async () => {
            if (!machine) throw closedError();
            const asked = !!(await machine.requestMissingSecretsIfNeeded());
            if (asked) await flushNow();
            return asked;
        });
    }

    // ---- cross-signing ----

    async function crossSigningNow() {
        const status = await machine.crossSigningStatus();
        const identity = await machine.getIdentity(new sdk.UserId(state.matrixUserId));
        return { hasMaster: !!status.hasMaster, hasSelfSigning: !!status.hasSelfSigning, hasUserSigning: !!status.hasUserSigning,
            identity: !!identity, verified: identity ? !!identity.isVerified() : false };
    }
    async function crossSigningStatusNow() {
        await ensureDevice();
        return serial(async () => { if (!machine) throw closedError(); return crossSigningNow(); });
    }
    /**
     * This user's cross-signing keys made on this device and uploaded (keys/device_signing/upload), and this device signed
     * with them. reset makes a new identity in place of one the server has (the user's explicit choice); without it, an
     * identity made elsewhere is refused (keysChanged) and kept.
     */
    async function setupCrossSigningNow(reset) {
        await ensureDevice();
        return serial(async () => {
            if (!machine) throw closedError();
            const requests = await machine.bootstrapCrossSigning(!!reset);
            if (requests.uploadKeysRequest) await sendRequest(requests.uploadKeysRequest);
            await call("signingUpload", Object.assign(JSON.parse(requests.uploadSigningKeysRequest.body), { reset: !!reset }));
            if (requests.uploadSignaturesRequest) await call("signaturesUpload", { signed: JSON.parse(requests.uploadSignaturesRequest.body) });
            await machine.markAllTrackedUsersAsDirty();
            await flushNow();
            return crossSigningNow();
        });
    }

    // ---- emoji and QR verification (over to-device) ----

    const flows = new Map();
    const PHASES = ["created", "requested", "ready", "transitioned", "done", "cancelled"];
    state.verifications = [];

    function track(request) {
        if (!request || flows.has(request.flowId)) return;
        flows.set(request.flowId, { request, sas: null, qr: null, qrImage: null, sasStartedHere: false, sasAccepted: false, sasWatched: false });
        try { request.registerChangesCallback(async () => refreshVerifications()); } catch (err) { /* polled on refresh */ }
        refreshVerifications();
        watchOpenFlows();
    }
    const openFlow = (flow) => {
        const sas = flow.sas;
        return !(flow.request.isDone() || flow.request.isCancelled() || (sas && (sas.isDone() || sas.isCancelled())));
    };
    /**
     * The emoji verification (SAS) the other device started: this device sends its m.key.verification.accept, which the
     * SDK leaves to the app (Sas.accept). Without it neither side gets the keys, so no emoji ever show (COM-UI-012). Every
     * SAS that was not started here is accepted, whichever device asked for the verification: the user already took the
     * request, and comparing the emoji is the confirmation. In the serial section.
     */
    async function acceptIncomingSas() {
        for (const flow of flows.values()) {
            if (flow.sasStartedHere || flow.sasAccepted) continue;
            let verification = null;
            try { verification = flow.request.getVerification(); } catch (err) { verification = null; }
            if (!(verification instanceof sdk.Sas) || verification.isCancelled() || verification.isDone()) continue;
            flow.sas = verification;
            flow.sasAccepted = true;
            let out = null;
            try { out = verification.accept(); } catch (err) { out = null; }
            if (out) await sendOutgoing(out);
        }
    }
    // while a verification is open, a sync every few seconds: a push that was lost does not stall it, and the SDK
    // sends the cancel of one that timed out
    let flowTimer = null;
    function watchOpenFlows() {
        if (flowTimer || !root.setInterval) return;
        flowTimer = root.setInterval(() => {
            if (!machine || ![...flows.values()].some(openFlow)) { root.clearInterval(flowTimer); flowTimer = null; return; }
            sync().then(() => refreshVerifications(), () => { /* the next round tries again */ });
        }, 5000);
    }
    /** A verification another device asked of this one (m.key.verification.request), seen in the sync. */
    function noteVerification(raw) {
        if (!machine || !raw || raw.type !== "m.key.verification.request" || !raw.content || !raw.sender) return;
        const flowId = raw.content.transaction_id;
        if (!flowId || flows.has(flowId)) return;
        const request = machine.getVerificationRequest(new sdk.UserId(raw.sender), flowId);
        // the machine drops a request from someone whose devices it does not track (this browser never opened a room with
        // them): it tracks them now, so the asker's Try again reaches it
        if (!request) {
            machine.updateTrackedUsers([new sdk.UserId(raw.sender)]).then(() => serial(() => flushNow())).catch(() => null);
            return;
        }
        track(request);
    }
    function refreshVerifications() {
        const list = [];
        for (const [flowId, flow] of flows) {
            const request = flow.request;
            let verification = null;
            try { verification = request.getVerification(); } catch (err) { verification = null; }
            // getVerification answers a new wrapper each time, so a flag (not the object) says the SAS is watched
            if (verification && verification instanceof sdk.Sas) {
                flow.sas = verification;
                if (!flow.sasWatched) {
                    flow.sasWatched = true;
                    try { verification.registerChangesCallback(async () => refreshVerifications()); } catch (err) { /* polled */ }
                }
                // one that came outside a sync (a change callback) is accepted too
                if (!flow.sasStartedHere && !flow.sasAccepted && machine) serial(() => acceptIncomingSas().then(flushNow)).then(refreshVerifications, () => null);
            }
            const sas = flow.sas;
            let emojis = null;
            try { if (sas && sas.canBePresented()) emojis = Array.from(sas.emoji() || []).map((e) => ({ symbol: e.symbol, description: e.description })); } catch (err) { emojis = null; }
            const done = request.isDone() || (!!sas && sas.isDone()) || (!!flow.qr && flow.qr.isDone());
            const otherUserId = userIdOf(request.otherUserId.toString());
            if (done && otherUserId === me && !flow.secretsAsked && machine) { flow.secretsAsked = true; askSecrets(); }
            const cancelled = request.isCancelled() || (!!sas && sas.isCancelled()) || (!!flow.qr && flow.qr.isCancelled());
            const ask = (run) => { try { return run(); } catch (err) { return null; } };
            const cancelInfo = (sas && ask(() => sas.cancelInfo())) || (flow.qr && ask(() => flow.qr.cancelInfo())) || null;
            const phase = PHASES[request.phase()] || String(request.phase());
            list.push({ flowId, otherUserId, otherDeviceId: request.otherDeviceId ? request.otherDeviceId.toString() : null,
                weStarted: !!request.weStarted(), phase, ready: !!request.isReady(), done, cancelled,
                emojis, confirmed: !!sas && !!sas.haveWeConfirmed(), qrImage: flow.qrImage, scanned: !!flow.qr && !!flow.qr.hasBeenScanned(),
                // the states between: started and waiting for the other device, its time left, why it ended
                started: phase === "transitioned" || !!sas, accepted: !!sas && !!ask(() => sas.hasBeenAccepted()),
                timedOut: !!ask(() => request.timedOut()) || (!!sas && !!ask(() => sas.timedOut())),
                remainingMs: done || cancelled ? null : ask(() => Number(request.timeRemainingMillis())),
                cancelReason: cancelInfo ? ask(() => cancelInfo.reason()) || ask(() => cancelInfo.cancelCode()) : null });
        }
        state.verifications = list;
        if (role === "leader") post({ op: "verifications", list });
        return list;
    }
    async function sendOutgoing(out) {
        if (!out) return;
        if (Array.isArray(out)) { for (const one of out) await sendOutgoing(one); return; }
        await sendRequest(out);
    }
    /** Asks a device (one of this user's own, or another user's) to verify with this one: the flowId of the new verification. */
    async function verifyDeviceNow(userId, deviceId) {
        await ensureDevice();
        return serial(async () => {
            if (!machine) throw closedError();
            const device = await machine.getDevice(new sdk.UserId(matrixId(userId)), new sdk.DeviceId(String(deviceId)), 5);
            if (!device) throw { reasonCode: "notFound", errors: "Device not found" };
            const M = sdk.VerificationMethod;
            const [request, out] = device.requestVerification([M.SasV1, M.QrCodeShowV1, M.QrCodeScanV1, M.ReciprocateV1]);
            await sendOutgoing(out);
            track(request);
            return request.flowId;
        });
    }
    /** QR code pixels (1 dark, 0 light) as a PNG data URL. */
    function qrImageOf(qr) {
        const pixels = qr.toQrCode().renderIntoBuffer();
        const width = Math.round(Math.sqrt(pixels.length));
        const scale = 6;
        const canvas = root.document.createElement("canvas");
        canvas.width = canvas.height = (width + 8) * scale;
        const ctx = canvas.getContext("2d");
        ctx.fillStyle = "#fff";
        ctx.fillRect(0, 0, canvas.width, canvas.height);
        ctx.fillStyle = "#000";
        for (let y = 0; y < width; y++) for (let x = 0; x < width; x++) if (pixels[y * width + x]) ctx.fillRect((x + 4) * scale, (y + 4) * scale, scale, scale);
        return canvas.toDataURL("image/png");
    }
    /**
     * A step of a verification: accept (an asked one), sas (start emoji), confirm (the emoji match), mismatch or cancel,
     * showQr (this device shows a code for the other to scan), scanned (the other side's code read here: arg its bytes),
     * qrConfirm (the other device says it scanned ours). Resolves to the verifications.
     */
    async function verificationNow(flowId, action, arg) {
        await ensureDevice();
        return serial(async () => {
            const flow = flows.get(flowId);
            if (!machine || !flow) throw { reasonCode: "notFound", errors: "This verification is over" };
            const request = flow.request;
            if (action === "accept") await sendOutgoing(request.accept());
            else if (action === "sas") {
                // started here: the other device accepts it (acceptIncomingSas there)
                flow.sasStartedHere = true;
                const started = await request.startSas();
                if (started) { flow.sas = started[0]; await sendOutgoing(started[1]); }
            } else if (action === "confirm" && flow.sas) await sendOutgoing(await flow.sas.confirm());
            else if (action === "mismatch" && flow.sas) await sendOutgoing(flow.sas.cancelWithCode("m.mismatched_sas"));
            else if (action === "cancel") await sendOutgoing((flow.sas && flow.sas.cancel()) || (flow.qr && flow.qr.cancel()) || request.cancel());
            else if (action === "showQr") {
                const qr = await request.generateQrCode();
                if (!qr) throw { reasonCode: "noQr", errors: "The other device cannot scan a code" };
                flow.qr = qr;
                flow.qrImage = qrImageOf(qr);
                try { qr.registerChangesCallback(async () => refreshVerifications()); } catch (err) { /* polled */ }
            } else if (action === "scanned") {
                const qr = await request.scanQrCode(sdk.QrCodeScan.fromBytes(new Uint8ClampedArray(Array.from(arg || []))));
                flow.qr = qr;
                await sendOutgoing(qr.reciprocate());
            } else if (action === "qrConfirm" && flow.qr) await sendOutgoing(flow.qr.confirmScanning());
            else throw { reasonCode: "badRequest", errors: "Unknown verification step " + action };
            await flushNow();
            if (action === "cancel" || action === "mismatch") flows.delete(flowId);
            return refreshVerifications();
        });
    }
    function dismissVerification(flowId) {
        flows.delete(flowId);
        return refreshVerifications();
    }

    // ---- what this device keeps sealed in the browser's storage (plans/Comms-Messaging.md phase 15) ----
    // An encrypted room's scheduled message is kept by the device that scheduled it until its time: sealed with AES-GCM
    // under a key derived (HKDF) from the device's store key, which the server gives only to the device's own signed-in
    // user and does not store. Neither the stored draft nor the server alone opens it. The key lives in this page only.
    let localKey = null;
    let localKeyOf = null;
    const b64 = (bytes) => root.btoa(Array.from(bytes, (b) => String.fromCharCode(b)).join(""));
    const unb64 = (text) => Uint8Array.from(root.atob(String(text || "")), (c) => c.charCodeAt(0));
    async function localKeyNow() {
        const subtle = root.crypto && root.crypto.subtle;
        const user = currentUser();
        const pointer = user ? readPointer(user) : null;
        if (!subtle || !root.TextEncoder) throw { reasonCode: "noCrypto", errors: "This browser cannot keep a sealed message" };
        if (!pointer) throw { reasonCode: "noDevice", errors: "This browser has no encryption device" };
        const id = user + "/" + pointer.deviceId;
        if (localKey && localKeyOf === id) return localKey;
        const answer = await call("storeKey", { deviceId: pointer.deviceId });
        if (!answer || !answer.storeKey) throw { reasonCode: "noDevice", errors: "This browser has no encryption device" };
        const encoder = new root.TextEncoder();
        const base = await subtle.importKey("raw", encoder.encode(answer.storeKey), "HKDF", false, ["deriveKey"]);
        localKey = await subtle.deriveKey({ name: "HKDF", hash: "SHA-256", salt: encoder.encode("moqui-chat-held-draft"), info: encoder.encode(id) },
            base, { name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]);
        localKeyOf = id;
        return localKey;
    }
    /** text sealed for this device: {v, iv, data} (base64), for the browser's storage. */
    async function sealLocal(text) {
        const key = await localKeyNow();
        const iv = root.crypto.getRandomValues(new Uint8Array(12));
        const data = await root.crypto.subtle.encrypt({ name: "AES-GCM", iv }, key, new root.TextEncoder().encode(String(text)));
        return { v: 1, iv: b64(iv), data: b64(new Uint8Array(data)) };
    }
    /** The text of what sealLocal made on this device; a rejection when it is not this device's or was changed. */
    async function openLocal(sealed) {
        if (!sealed || sealed.v !== 1) throw { reasonCode: "notSealed", errors: "Not a sealed draft" };
        const key = await localKeyNow();
        const plain = await root.crypto.subtle.decrypt({ name: "AES-GCM", iv: unb64(sealed.iv) }, key, unb64(sealed.data));
        return new root.TextDecoder().decode(plain);
    }

    // a page that goes away closes its store (logout closes it before the logout request, WebrootVue.qvt.js logoutClick)
    if (root.addEventListener) root.addEventListener("pagehide", () => { if (machine) closeNow(); });

    moqui.crypto = { state, ensureDevice, elsewhere, onToDevice, close, thisDeviceId, sealLocal, openLocal, matrixId, userIdOf, onRoomKeys, encryptFile, decryptFile,
        sendToDevices: api("sendToDevices", sendToDevicesNow), devicesOf: api("devicesOf", devicesOfNow), sync: relayOnly("sync", sync),
        forget: relayOnly("forget", forgetNow), startOver: startOverNow, settingsChanged: relayOnly("settingsChanged", settingsChanged),
        encryptRoom: api("encryptRoom", encryptRoomNow), decryptRoom: api("decryptRoom", decryptRoomNow), rotateRoom: api("rotateRoom", rotateRoomNow),
        roomTrust: api("roomTrust", roomTrustNow), leftOut: relayOnly("leftOut", leftOutNow), acceptProblems: acceptProblemsNow, problemsOf, readAllowed, onTrust,
        backupStatus: api("backupStatus", backupStatusNow), enableBackup: api("enableBackup", enableBackupNow),
        restoreBackup: api("restoreBackup", restoreBackupNow), setupStatus: api("setupStatus", setupStatusNow), requestSecrets: api("requestSecrets", requestSecretsNow), crossSigningStatus: api("crossSigningStatus", crossSigningStatusNow),
        setupCrossSigning: api("setupCrossSigning", setupCrossSigningNow), verifyDevice: api("verifyDevice", verifyDeviceNow),
        verification: api("verification", verificationNow), dismissVerification: api("dismissVerification", dismissVerification) };
})(window);
