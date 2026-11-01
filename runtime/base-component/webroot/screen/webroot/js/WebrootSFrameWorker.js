/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The RTCRtpScriptTransform worker of an end-to-end encrypted call (W8; docs/interfaces/WebRTC.md "Encrypted calls"): it
 * SFrame-encrypts each frame this page sends and decrypts each frame it receives (WebrootSFrame.js). The page
 * (WebrootCallKeys.js) gives it the keys; the keys never leave the page and this worker.
 *
 * - A sent frame goes out under this sender's current key (useKey), with one counter for every track and layer, so no two
 *   frames under a key share a nonce. With no key yet a frame is dropped: nothing leaves unencrypted.
 * - A received frame decrypts with the key of its KID, when that KID belongs to the leg the track came from (legs: the
 *   roster's key index of each leg; a KID is keyIndex * 65536 + epoch). A frame that does not decrypt is dropped, never
 *   played as it came: a call does not fall back to plain media.
 * - The leading bytes of a VP8 frame stay in the clear (WebrootSFrame.clearBytes); Opus is encrypted whole.
 *
 * Messages from the page: {type: 'setKey', legId, kid, baseKey}, {type: 'useKey', kid}, {type: 'legs', legs: {legId:
 * keyIndex}}, {type: 'dropLeg', legId}. To the page, every second while frames flow: {type: 'stats', sent, dropped,
 * received: {legId: n}, failed: {legId: n}}.
 */
/* global importScripts, moquiSFrame */
"use strict";
importScripts("/js/WebrootSFrame.js");

const S = self.moquiSFrame;
const KID_SPAN = 65536;
// each leg's keys, the last few epochs: a receiver still has the previous one while a sender moves to its next
const KEEP_PER_LEG = 4;
const keys = new Map(); // kid (Number) -> {legId, key}
const byLeg = new Map(); // legId -> [kid, ...] oldest first
let legs = {};
let sendKey = null;
let counter = 0n;
let stats = { sent: 0, dropped: 0, received: {}, failed: {} };

function bump(map, legId) {
    map[legId] = (map[legId] || 0) + 1;
}

setInterval(() => {
    if (!stats.sent && !stats.dropped && !Object.keys(stats.received).length && !Object.keys(stats.failed).length) return;
    self.postMessage(Object.assign({ type: "stats" }, stats));
    stats = { sent: 0, dropped: 0, received: {}, failed: {} };
}, 1000);

// the page's messages one at a time, in order: a key is derived before a later useKey looks for it
let chain = Promise.resolve();
self.onmessage = (event) => {
    chain = chain.then(() => handle(event.data || {})).catch((err) => console.warn("call key message failed", err));
};
async function handle(m) {
    if (m.type === "setKey") {
        const kid = Number(m.kid);
        const key = await S.deriveKey(S.CALL_SUITE, kid, new Uint8Array(m.baseKey));
        keys.set(kid, { legId: m.legId, key });
        const list = (byLeg.get(m.legId) || []).filter((k) => k !== kid);
        list.push(kid);
        while (list.length > KEEP_PER_LEG) keys.delete(list.shift());
        byLeg.set(m.legId, list);
        if (m.use) sendKey = key;
    } else if (m.type === "useKey") {
        const entry = keys.get(Number(m.kid));
        if (entry) sendKey = entry.key;
    } else if (m.type === "legs") {
        legs = Object.assign({}, m.legs || {});
    } else if (m.type === "dropLeg") {
        for (const kid of byLeg.get(m.legId) || []) keys.delete(kid);
        byLeg.delete(m.legId);
    }
}

function sender(kind) {
    return new TransformStream({
        async transform(frame, controller) {
            const key = sendKey;
            if (!key) {
                stats.dropped++;
                return;
            }
            const data = new Uint8Array(frame.data);
            const clear = S.clearBytes(kind, data);
            const ctr = counter++;
            const sealed = await S.encrypt(key, ctr, data.subarray(0, clear), data.subarray(clear));
            frame.data = S.concat(data.subarray(0, clear), sealed).buffer;
            stats.sent++;
            controller.enqueue(frame);
        }
    });
}

function receiver(kind, legId) {
    const lookup = (kid) => {
        if (kid > BigInt(Number.MAX_SAFE_INTEGER)) return null;
        const n = Number(kid);
        const entry = keys.get(n);
        // the key of this KID, only when it is the key of the leg this track comes from
        if (!entry || entry.legId !== legId || Math.floor(n / KID_SPAN) !== Number(legs[legId])) return null;
        return entry.key;
    };
    return new TransformStream({
        async transform(frame, controller) {
            const data = new Uint8Array(frame.data);
            const clear = S.clearBytes(kind, data);
            try {
                const out = await S.decrypt(lookup, data.subarray(0, clear), data.subarray(clear));
                frame.data = S.concat(data.subarray(0, clear), out.plaintext).buffer;
            } catch (err) {
                bump(stats.failed, legId);
                return;
            }
            bump(stats.received, legId);
            controller.enqueue(frame);
        }
    });
}

self.onrtctransform = (event) => {
    const t = event.transformer;
    const o = t.options || {};
    const stream = o.side === "send" ? sender(o.kind) : receiver(o.kind, String(o.legId || ""));
    t.readable.pipeThrough(stream).pipeTo(t.writable).catch(() => null);
};
