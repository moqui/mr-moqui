/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The browser's crypto device (plans/Comms-Framework.md §4.5, C7 core; docs/interfaces/Chat-Keys.md): moqui.crypto.
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
 * device, and a page of another user never opens it.
 *
 * Wake-ups: a to-device message for any of the user's devices pushes {kind: 'toDevice'} on chatUser:<me>; a loaded page
 * syncs then, and when its notification socket comes back. The push carries nothing of the message.
 *
 * The API (for encrypted calls, W8):
 * - ensureDevice(): Promise<{userId, deviceId, matrixUserId}>: loads the module and the store, uploads the keys.
 * - sendToDevices(targets, eventType, content): Promise<{sent, devices, skipped}>: Olm-encrypts content as an event of
 *   eventType for each device of the targets, a list of userIds (every device of each) or {userId, deviceIds} (those
 *   devices only), and sends it; not to this device. sent is how many the server queued (a revoked device, or a user
 *   who shares no room or call with this one, gets none); devices, those it was encrypted for, and skipped, those it
 *   could not be (no session could be made, blacklisted, unknown), are lists of {userId, deviceId}.
 * - onToDevice(fn): () => void: fn({type, content, senderUserId, senderMatrixId, senderDeviceId, senderCurve25519Key,
 *   verified}) for each to-device event that decrypted (only those); returns the unsubscribe.
 * - devicesOf(userId): Promise<[{deviceId, displayName, ed25519Key, verified}]>: the devices of a user as this store knows them.
 * - sync(), close(), forget(): run a sync now; close the store; revoke this device and delete its store.
 * - thisDeviceId(): the deviceId this browser uses for the signed-in user, without loading anything (null when none).
 * - state: {status: idle, loading, ready or error, deviceId, matrixUserId, error}.
 */
(function (root) {
    "use strict";
    const moqui = (root.moqui = root.moqui || {});
    const reactive = (obj) => (root.Vue && root.Vue.reactive ? root.Vue.reactive(obj) : obj);
    const LIB = "/libs/matrix-sdk-crypto/index.mjs";
    const BASE = "/ChatKeys/";
    const ENCRYPTED = "m.room.encrypted";

    const state = reactive({ status: "idle", deviceId: null, matrixUserId: null, error: null });
    let sdk = null;
    let machine = null;
    let me = null;
    let loading = null;
    let queue = Promise.resolve();
    let cursor = 0;
    let listCursor = null;
    let unsubscribe = null;
    const listeners = new Set();

    const currentUser = () => (moqui.webrootVue && moqui.webrootVue.userId) || null;
    const pointerKey = (userId) => "moqui.crypto.device." + userId;
    const storePrefix = (userId, deviceId) => "moqui-crypto-" + userId + "-" + deviceId;

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
        for (const request of requests) await sendRequest(request);
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
            } else if (request.type === T.SignatureUpload) answer = { failures: {} };
            else answer = {};
        } catch (err) {
            if (err && (err.reasonCode === "notFound" || err.reasonCode === "revoked")) await gone();
            throw err;
        }
        if (machine) await machine.markRequestAsSent(request.id, request.type, JSON.stringify(answer));
    }

    /** The server no longer knows this device (revoked, or another account's session): forget it and its store. */
    async function gone() {
        const user = me;
        const deviceId = state.deviceId;
        await closeNow();
        if (user && deviceId) {
            writePointer(user, null);
            await deleteStore(storePrefix(user, deviceId));
        }
    }

    function emit(event) {
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
        if (client.onState) client.onState((open) => { if (open && machine) sync().catch(() => null); });
    }

    async function open(user, retry) {
        let pointer = readPointer(user);
        if (!pointer) {
            const made = await call("device", { displayName: deviceName() });
            pointer = { deviceId: made.deviceId, user_id: made.user_id };
            writePointer(user, pointer);
        }
        try {
            machine = await sdk.OlmMachine.initialize(new sdk.UserId(pointer.user_id), new sdk.DeviceId(pointer.deviceId),
                storePrefix(user, pointer.deviceId));
        } catch (err) {
            // a store that is not this device's (another server name, a broken store): start over once with a new device
            writePointer(user, null);
            await deleteStore(storePrefix(user, pointer.deviceId));
            if (retry) throw err;
            return open(user, true);
        }
        me = user;
        cursor = 0;
        listCursor = null;
        Object.assign(state, { status: "ready", deviceId: pointer.deviceId, matrixUserId: pointer.user_id, error: null });
        listen(user);
        try {
            await flushNow();
            await syncNow();
        } catch (err) {
            if (!machine && !retry) return open(user, true);
            throw err;
        }
    }

    function ensureDevice() {
        const user = currentUser();
        if (!user) return Promise.reject(new Error("Sign in first"));
        if (machine && me === user && state.status === "ready") return Promise.resolve(identity());
        if (loading) return loading;
        state.status = "loading";
        loading = serial(async () => {
            if (machine && me !== user) await closeNow();
            await loadSdk();
            if (!machine) await open(user, false);
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

    async function sendToDevices(targets, eventType, content) {
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

    async function devicesOf(userId) {
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

    async function closeNow() {
        if (unsubscribe) { try { unsubscribe(); } catch (err) { /* the socket is gone */ } }
        unsubscribe = null;
        const m = machine;
        machine = null;
        me = null;
        cursor = 0;
        listCursor = null;
        Object.assign(state, { status: "idle", deviceId: null, matrixUserId: null, error: null });
        if (m) { try { m.close(); } catch (err) { /* already closed */ } }
    }
    function close() {
        return serial(() => closeNow());
    }

    /** Revokes this browser's device on the server and deletes its store; the next ensureDevice makes a new one. */
    function forget() {
        const user = currentUser();
        const pointer = user ? readPointer(user) : null;
        return serial(async () => {
            if (pointer) await call("revoke", { deviceId: pointer.deviceId }).catch(() => null);
            await closeNow();
            if (pointer) {
                writePointer(user, null);
                await deleteStore(storePrefix(user, pointer.deviceId));
            }
        });
    }

    function thisDeviceId() {
        const user = currentUser();
        const pointer = user ? readPointer(user) : null;
        return pointer ? pointer.deviceId : null;
    }

    // a page that goes away closes its store (logout closes it before the logout request, WebrootVue.qvt.js logoutClick)
    if (root.addEventListener) root.addEventListener("pagehide", () => { if (machine) closeNow(); });

    moqui.crypto = { state, ensureDevice, sendToDevices, onToDevice, devicesOf, sync, close, forget, thisDeviceId,
        matrixId, userIdOf };
})(window);
