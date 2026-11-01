/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The call-key protocol of end-to-end encrypted calls (plans/Comms-Framework.md §6.4, W8; docs/interfaces/WebRTC.md
 * "Encrypted calls"): moqui.callKeys, which the page's call store (WebrootCall.js) uses for a call whose encryptionMode is
 * e2ee-sframe.
 *
 * - **Who gets keys.** The server publishes the call's roster in its view: each answered browser leg with its crypto device
 *   (deviceId, a ChatDevice of C7 core), its keyIndex and its keyGeneration (the leg's connection generation). This page
 *   sends its keys to the devices of the others on it, and takes keys only from them: a key event must come from the
 *   device the roster names for its leg (the Olm sender, which the server cannot forge), and its KID must carry that leg's
 *   keyIndex.
 * - **Keys.** Each sender (this page's leg) makes a random 16-byte SFrame base key per call and epoch. Its KID is
 *   keyIndex * 65536 + epoch (WebrootSFrameWorker.js), the cipher suite AES_128_GCM_SHA256_128. It goes to every other
 *   device on the roster as an Olm to-device event (moqui.crypto.sendToDevices, 'io.moqui.call.key'), never through a chat
 *   room's Megolm session. The server passes on only Olm ciphertext.
 * - **Rotation.** A new key, a new epoch, whenever the roster changes: a leg joins, leaves or is removed, a device is
 *   revoked (its leg leaves), or a leg connects again (a new keyGeneration, this page's too). The sender moves to the new
 *   key a second after sending it, so the others have it first (the first key at once); receivers keep a leg's last four.
 *   A leg that left does not get the new key, so it cannot read what is said after it left. Rotation cannot take back a
 *   key a leg already had.
 * - **One store.** The keys travel over the user's crypto device, whose store one tab of a browser holds at a time; the
 *   user's other tabs ask that tab (moqui.crypto's leader, C7), so a call is encrypted in any of them. reasonCode busy:
 *   the leader did not answer; prepare() says so.
 * - **No downgrade.** Nothing is sent before the first key, and a frame that does not decrypt is dropped, never played.
 *
 * The API: supported(), prepare() (Promise<{cryptoDeviceId, scriptTransform: true}>, or a rejection with reasonCode and
 * message), begin(callSessionId, legId), onView(view), attachSender(sender, kind), attachReceiver(receiver, kind, legId),
 * end(), and state (reactive): {active, callSessionId, legId, epoch, kid, rotations, error, keys: {legId: {kid, epoch,
 * verified, at}}, frames: {legId: {received, failed}}, sent, dropped, lastSend: {kid, devices}}.
 */
(function (root) {
    "use strict";
    const moqui = (root.moqui = root.moqui || {});
    const reactive = (obj) => (root.Vue && root.Vue.reactive ? root.Vue.reactive(obj) : obj);
    const t = (text) => (moqui.l10n && moqui.l10n.localize ? moqui.l10n.localize(text) : text);
    const EVENT = "io.moqui.call.key";
    const KID_SPAN = 65536;
    const SWITCH_MS = 1000;
    const SETTLE_MS = 150;
    const PENDING_MS = 15000;

    const state = reactive({ active: false, callSessionId: null, legId: null, epoch: 0, kid: null, rotations: 0, error: "",
        keys: {}, frames: {}, sent: 0, dropped: 0, lastSend: null });
    let worker = null;
    let roster = new Map(); // legId -> {legId, userId, deviceId, keyIndex, keyGeneration}
    let rosterKey = "";
    let off = null;
    let pending = []; // key events whose leg is not on the roster yet
    let rotating = Promise.resolve();
    let again = false;
    let settle = null;

    function supported() {
        return typeof root.RTCRtpScriptTransform === "function" && typeof root.Worker === "function" &&
            !!(root.crypto && root.crypto.subtle) && !!moqui.crypto;
    }

    /** The device this browser takes part with; rejects with {reasonCode, message} when it cannot. */
    function prepare() {
        if (!supported()) {
            return Promise.reject({ reasonCode: "unsupportedClient",
                message: t("This call is end-to-end encrypted, and this browser cannot encrypt call media (it has no RTCRtpScriptTransform). Join it from a current Chrome, Edge, Firefox or Safari.") });
        }
        return moqui.crypto.ensureDevice().then((device) => ({ cryptoDeviceId: device.deviceId, scriptTransform: true }), (err) => {
            const busy = err && err.reasonCode === "busy";
            throw { reasonCode: busy ? "busy" : "noCryptoDevice",
                message: busy ? t("This call is end-to-end encrypted, and its keys are open in another tab of this browser: close that tab (or its call or My devices) to take the call here.")
                    : t("This call is end-to-end encrypted, and this browser could not set up its encryption device") + ": " + ((err && (err.errors || err.message)) || err) };
        });
    }

    function toBase64(bytes) {
        let text = "";
        for (const b of bytes) text += String.fromCharCode(b);
        return root.btoa(text);
    }
    function fromBase64(text) {
        const raw = root.atob(String(text || ""));
        const out = new Uint8Array(raw.length);
        for (let i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i);
        return out;
    }

    function begin(callSessionId, legId) {
        if (state.active && state.callSessionId === callSessionId && state.legId === legId && worker) return;
        end();
        worker = new root.Worker("/js/WebrootSFrameWorker.js");
        worker.onmessage = (event) => onStats(event.data || {});
        Object.assign(state, { active: true, callSessionId, legId, epoch: 0, kid: null, rotations: 0, error: "", keys: {}, frames: {},
            sent: 0, dropped: 0, lastSend: null });
        roster = new Map();
        rosterKey = "";
        pending = [];
        off = moqui.crypto.onToDevice(onKeyEvent);
    }

    function end() {
        if (off) off();
        off = null;
        if (settle) clearTimeout(settle);
        settle = null;
        if (worker) worker.terminate();
        worker = null;
        roster = new Map();
        rosterKey = "";
        pending = [];
        Object.assign(state, { active: false, callSessionId: null, legId: null, epoch: 0, kid: null, keys: {}, frames: {}, lastSend: null });
    }

    function onStats(m) {
        if (m.type !== "stats") return;
        state.sent += m.sent || 0;
        state.dropped += m.dropped || 0;
        for (const [legId, n] of Object.entries(m.received || {})) {
            const f = (state.frames[legId] = state.frames[legId] || { received: 0, failed: 0 });
            f.received += n;
        }
        for (const [legId, n] of Object.entries(m.failed || {})) {
            const f = (state.frames[legId] = state.frames[legId] || { received: 0, failed: 0 });
            f.failed += n;
        }
    }

    /** The legs of the call that take part in its keys: answered browser legs with a device and a key index. */
    function rosterOf(view) {
        const out = new Map();
        for (const p of view.participants || []) {
            for (const leg of p.legs || []) {
                if (leg.status !== "answered" || !leg.deviceId || leg.keyIndex == null || !p.userId) continue;
                out.set(leg.callLegId, { legId: leg.callLegId, userId: p.userId, deviceId: leg.deviceId, keyIndex: Number(leg.keyIndex),
                    keyGeneration: Number(leg.keyGeneration || 0) });
            }
        }
        return out;
    }

    /** The call's view changed: a new roster gets this sender's new key (after a short settle, as changes come together). */
    function onView(view) {
        if (!state.active || !view || view.callSessionId !== state.callSessionId) return;
        if (view.status === "ended") return;
        const next = rosterOf(view);
        const key = Array.from(next.values()).map((r) => r.legId + ":" + r.deviceId + ":" + r.keyGeneration).sort().join(",");
        // a leg gone, or given another device or key index, loses the keys it had: they came from another device
        for (const [legId, was] of roster) {
            const now = next.get(legId);
            if ((!now || now.deviceId !== was.deviceId || now.keyIndex !== was.keyIndex) && worker) {
                worker.postMessage({ type: "dropLeg", legId });
                delete state.keys[legId];
            }
        }
        roster = next;
        if (worker) {
            const legs = {};
            for (const r of roster.values()) legs[r.legId] = r.keyIndex;
            worker.postMessage({ type: "legs", legs });
        }
        flushPending();
        if (key === rosterKey) return;
        rosterKey = key;
        if (!roster.has(state.legId)) return;
        if (settle) clearTimeout(settle);
        settle = setTimeout(() => { settle = null; rotate(); }, SETTLE_MS);
    }

    /** A new key for this sender, sent to the roster's other devices; rotations run one at a time, the last change wins. */
    function rotate() {
        if (!state.active) return Promise.resolve();
        again = true;
        rotating = rotating.then(async () => {
            if (!again || !state.active || !worker) return;
            again = false;
            const me = roster.get(state.legId);
            if (!me) return;
            const epoch = (state.epoch % (KID_SPAN - 1)) + 1;
            const kid = me.keyIndex * KID_SPAN + epoch;
            const base = root.crypto.getRandomValues(new Uint8Array(16));
            const first = state.kid == null;
            const legId = state.legId;
            worker.postMessage({ type: "setOwnKey", kid, baseKey: base.buffer.slice(0), use: first });
            const others = new Map();
            for (const r of roster.values()) {
                if (r.legId === legId) continue;
                const list = others.get(r.userId) || [];
                if (!list.includes(r.deviceId)) list.push(r.deviceId);
                others.set(r.userId, list);
            }
            const targets = Array.from(others.entries()).map(([userId, deviceIds]) => ({ userId, deviceIds }));
            const content = { callSessionId: state.callSessionId, callLegId: legId, kid, epoch, suite: 4, key: toBase64(base) };
            let sent = { sent: 0, devices: [], skipped: [] };
            if (targets.length) {
                try {
                    sent = await moqui.crypto.sendToDevices(targets, EVENT, content);
                } catch (err) {
                    state.error = t("The call's keys could not be sent") + ": " + ((err && (err.errors || err.message)) || err);
                }
            }
            base.fill(0);
            if (!state.active || state.legId !== legId) return;
            state.epoch = epoch;
            state.rotations++;
            state.lastSend = { kid, devices: sent.devices || [], skipped: sent.skipped || [] };
            if (first) {
                state.kid = kid;
                return;
            }
            // the others get the key first; then this sender moves to it, unless a newer one came meanwhile
            setTimeout(() => {
                if (!state.active || !worker || state.epoch !== epoch) return;
                worker.postMessage({ type: "useKey", kid });
                state.kid = kid;
            }, SWITCH_MS);
        }).catch((err) => { state.error = String((err && err.message) || err); });
        return rotating;
    }

    /**
     * A key event: taken only from the device the roster names for its leg, with that leg's key index in its KID, and only
     * while no other leg of the roster (this page's included) has that key index: a roster that gives two legs one
     * index would have two senders' keys under one KID.
     */
    function onKeyEvent(event) {
        if (!event || event.type !== EVENT) return;
        const c = event.content || {};
        if (!state.active || c.callSessionId !== state.callSessionId) return;
        if (!take(event)) {
            pending.push({ event, at: Date.now() });
            if (pending.length > 50) pending.shift();
        }
    }
    function take(event) {
        const c = event.content || {};
        const leg = roster.get(c.callLegId);
        const kid = Number(c.kid);
        if (!leg || c.callLegId === state.legId) return false;
        if (leg.userId !== event.senderUserId || leg.deviceId !== event.senderDeviceId) return false;
        if (!Number.isSafeInteger(kid) || Math.floor(kid / KID_SPAN) !== leg.keyIndex || Number(c.suite || 4) !== 4) return false;
        for (const r of roster.values()) if (r.legId !== leg.legId && r.keyIndex === leg.keyIndex) return false;
        const key = fromBase64(c.key);
        if (key.length !== 16 || !worker) return false;
        worker.postMessage({ type: "setKey", legId: c.callLegId, kid, baseKey: key.buffer });
        state.keys[c.callLegId] = { kid, epoch: kid % KID_SPAN, verified: !!event.verified, at: Date.now() };
        return true;
    }
    function flushPending() {
        const now = Date.now();
        pending = pending.filter((p) => now - p.at < PENDING_MS && !take(p.event));
    }

    function attachSender(sender, kind) {
        if (!worker || !sender) return;
        sender.transform = new root.RTCRtpScriptTransform(worker, { side: "send", kind });
    }
    function attachReceiver(receiver, kind, legId) {
        if (!worker || !receiver) return;
        receiver.transform = new root.RTCRtpScriptTransform(worker, { side: "recv", kind, legId });
    }

    /** The key state of a leg as the roster shows it: keys (its frames decrypt), waiting (none yet), or failing. */
    function legState(legId) {
        const k = state.keys[legId];
        const f = state.frames[legId] || { received: 0, failed: 0 };
        if (!k) return "waiting";
        if (f.failed && !f.received) return "failing";
        return "keys";
    }

    moqui.callKeys = { state, supported, prepare, begin, end, onView, attachSender, attachReceiver, legState, rotate };
})(globalThis);
