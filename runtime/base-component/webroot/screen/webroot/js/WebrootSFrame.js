/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * SFrame (RFC 9605) on WebCrypto, for end-to-end encrypted calls (plans/Comms-Framework.md §6.4, W8;
 * docs/interfaces/WebRTC.md "Encrypted calls"). No dependencies and no WebAssembly: HKDF, AES-GCM, AES-CTR and HMAC are
 * the browser's own. It runs in a page (moqui.sframe) and in the call's RTCRtpScriptTransform worker
 * (WebrootSFrameWorker.js imports it: self.moquiSFrame).
 *
 * Calls use the cipher suite AES_128_GCM_SHA256_128 (0x0004): a 16-byte key, a 12-byte nonce and a 16-byte tag. The other
 * four suites of the RFC are here too (AES-CTR with HMAC-SHA256 tags of 80, 64 and 32 bits, AES_256_GCM_SHA512_128), so
 * that every vector of the RFC's Appendix C runs against this code (the webroot client spec WebrootSFrame, the vectors in
 * crates/moqui-rtc/testdata/sframe-rfc9605.json, which crates/moqui-rtc/tests/sframe.rs also checks against the sframe
 * crate).
 *
 * The API:
 * - encodeHeader(kid, ctr): Uint8Array; decodeHeader(bytes, offset): {kid, ctr, length} (kid and ctr as BigInt), or null.
 * - deriveKey(suite, kid, baseKey): Promise<key>, the sframe_key and sframe_salt of RFC 9605 §4.4.2 for one KID, ready to use.
 * - encrypt(key, ctr, metadata, plaintext): Promise<Uint8Array>, header || ciphertext || tag, with metadata authenticated
 *   (in the AAD after the header) but not included.
 * - decrypt(lookup, metadata, data): Promise<{kid, ctr, plaintext}>; lookup(kid) is the key of a KID (or a promise of
 *   it), null when none: the promise rejects (unknown KID, or a frame that does not authenticate).
 * - aeadEncrypt(suite, keyBytes, nonce, aad, pt) and aeadDecrypt: the bare AEAD of §4.5, for the vectors.
 */
(function (root) {
    "use strict";
    const subtle = root.crypto && root.crypto.subtle;
    // RFC 9605 §8.1: Nh is the hash, Nka the AES key length within Nk (CTR suites carry an HMAC key after it)
    const SUITES = {
        1: { id: 1, hash: "SHA-256", mode: "ctr", nk: 48, nka: 16, nn: 12, nt: 10 },
        2: { id: 2, hash: "SHA-256", mode: "ctr", nk: 48, nka: 16, nn: 12, nt: 8 },
        3: { id: 3, hash: "SHA-256", mode: "ctr", nk: 48, nka: 16, nn: 12, nt: 4 },
        4: { id: 4, hash: "SHA-256", mode: "gcm", nk: 16, nn: 12, nt: 16 },
        5: { id: 5, hash: "SHA-512", mode: "gcm", nk: 32, nn: 12, nt: 16 }
    };
    const CALL_SUITE = 4;
    const utf8 = (text) => new TextEncoder().encode(text);

    function suiteOf(suite) {
        const s = SUITES[Number(suite)];
        if (!s) throw new Error("Unknown SFrame cipher suite " + suite);
        return s;
    }
    function big(value) {
        if (typeof value === "bigint") return value;
        if (!Number.isSafeInteger(value) || value < 0) throw new Error("Not a counter or key id: " + value);
        return BigInt(value);
    }
    /** value as n big-endian bytes */
    function beBytes(value, n) {
        let v = big(value);
        const out = new Uint8Array(n);
        for (let i = n - 1; i >= 0; i--) {
            out[i] = Number(v & 0xffn);
            v >>= 8n;
        }
        return out;
    }
    /** the fewest bytes that hold value (at least one) */
    function minBytes(value) {
        let v = big(value);
        let n = 1;
        while (v > 0xffn) {
            v >>= 8n;
            n++;
        }
        return n;
    }
    function concat(...parts) {
        const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
        let at = 0;
        for (const p of parts) {
            out.set(p, at);
            at += p.length;
        }
        return out;
    }
    const view = (data) => (data instanceof Uint8Array ? data : new Uint8Array(data));

    // ---- the header (RFC 9605 §4.3): X K Y C, then the KID and CTR when they do not fit in three bits ----
    function encodeHeader(kid, ctr) {
        const k = big(kid);
        const c = big(ctr);
        let config = 0;
        const tail = [];
        if (k < 8n) config |= Number(k) << 4;
        else {
            const n = minBytes(k);
            if (n > 8) throw new Error("KID too large");
            config |= 0x80 | ((n - 1) << 4);
            tail.push(beBytes(k, n));
        }
        if (c < 8n) config |= Number(c);
        else {
            const n = minBytes(c);
            if (n > 8) throw new Error("CTR too large");
            config |= 0x08 | (n - 1);
            tail.push(beBytes(c, n));
        }
        return concat(Uint8Array.of(config), ...tail);
    }
    function readBig(bytes, at, n) {
        let v = 0n;
        for (let i = 0; i < n; i++) v = (v << 8n) | BigInt(bytes[at + i]);
        return v;
    }
    function decodeHeader(data, offset) {
        const bytes = view(data);
        let at = offset || 0;
        if (at >= bytes.length) return null;
        const config = bytes[at++];
        let kid;
        let ctr;
        if (config & 0x80) {
            const n = ((config >> 4) & 7) + 1;
            if (at + n > bytes.length) return null;
            kid = readBig(bytes, at, n);
            at += n;
        } else kid = BigInt((config >> 4) & 7);
        if (config & 0x08) {
            const n = (config & 7) + 1;
            if (at + n > bytes.length) return null;
            ctr = readBig(bytes, at, n);
            at += n;
        } else ctr = BigInt(config & 7);
        return { kid, ctr, length: at - (offset || 0) };
    }

    // ---- key derivation (§4.4.2): HKDF-Extract with an empty salt, then Expand with the labels ----
    function labels(suite, kid) {
        const tail = concat(beBytes(kid, 8), beBytes(suite.id, 2));
        return { key: concat(utf8("SFrame 1.0 Secret key "), tail), salt: concat(utf8("SFrame 1.0 Secret salt "), tail) };
    }
    async function hkdf(hash, secret, info, bytes) {
        const ikm = await subtle.importKey("raw", secret, "HKDF", false, ["deriveBits"]);
        // an empty salt is RFC 5869's default salt: HMAC pads its key with zeros either way
        const bits = await subtle.deriveBits({ name: "HKDF", hash, salt: new Uint8Array(0), info }, ikm, bytes * 8);
        return new Uint8Array(bits);
    }
    /** sframe_key and sframe_salt as bytes, for the vectors */
    async function deriveRaw(suiteId, kid, baseKey) {
        const suite = suiteOf(suiteId);
        const l = labels(suite, kid);
        const base = view(baseKey);
        return { key: await hkdf(suite.hash, base, l.key, suite.nk), salt: await hkdf(suite.hash, base, l.salt, suite.nn) };
    }
    async function importAead(suite, keyBytes) {
        if (suite.mode === "gcm") return { gcm: await subtle.importKey("raw", keyBytes, "AES-GCM", false, ["encrypt", "decrypt"]) };
        return {
            ctr: await subtle.importKey("raw", keyBytes.slice(0, suite.nka), "AES-CTR", false, ["encrypt", "decrypt"]),
            hmac: await subtle.importKey("raw", keyBytes.slice(suite.nka), { name: "HMAC", hash: suite.hash }, false, ["sign"])
        };
    }
    async function deriveKey(suiteId, kid, baseKey) {
        const suite = suiteOf(suiteId);
        const raw = await deriveRaw(suiteId, kid, baseKey);
        return { suite, kid: big(kid), salt: raw.salt, aead: await importAead(suite, raw.key) };
    }

    // ---- the AEAD (§4.5): AES-GCM, or AES-CTR then an HMAC tag over the lengths, the nonce, the AAD and the ciphertext ----
    async function ctrTag(suite, aead, nonce, aad, ct) {
        const authData = concat(beBytes(aad.length, 8), beBytes(ct.length, 8), beBytes(suite.nt, 8), nonce, aad, ct);
        const mac = new Uint8Array(await subtle.sign("HMAC", aead.hmac, authData));
        return mac.slice(0, suite.nt);
    }
    async function seal(suite, aead, nonce, aad, pt) {
        if (suite.mode === "gcm") {
            return new Uint8Array(await subtle.encrypt({ name: "AES-GCM", iv: nonce, additionalData: aad, tagLength: suite.nt * 8 }, aead.gcm, pt));
        }
        const counter = concat(nonce, new Uint8Array(4));
        const ct = new Uint8Array(await subtle.encrypt({ name: "AES-CTR", counter, length: 32 }, aead.ctr, pt));
        return concat(ct, await ctrTag(suite, aead, nonce, aad, ct));
    }
    async function open(suite, aead, nonce, aad, sealed) {
        if (suite.mode === "gcm") {
            return new Uint8Array(await subtle.decrypt({ name: "AES-GCM", iv: nonce, additionalData: aad, tagLength: suite.nt * 8 }, aead.gcm, sealed));
        }
        if (sealed.length < suite.nt) throw new Error("SFrame: too short");
        const ct = sealed.slice(0, sealed.length - suite.nt);
        const tag = sealed.slice(sealed.length - suite.nt);
        const want = await ctrTag(suite, aead, nonce, aad, ct);
        let diff = 0;
        for (let i = 0; i < want.length; i++) diff |= want[i] ^ tag[i];
        if (diff) throw new Error("SFrame: the frame does not authenticate");
        const counter = concat(nonce, new Uint8Array(4));
        return new Uint8Array(await subtle.decrypt({ name: "AES-CTR", counter, length: 32 }, aead.ctr, ct));
    }
    async function aeadEncrypt(suiteId, keyBytes, nonce, aad, pt) {
        const suite = suiteOf(suiteId);
        return seal(suite, await importAead(suite, view(keyBytes)), view(nonce), view(aad), view(pt));
    }
    async function aeadDecrypt(suiteId, keyBytes, nonce, aad, ct) {
        const suite = suiteOf(suiteId);
        return open(suite, await importAead(suite, view(keyBytes)), view(nonce), view(aad), view(ct));
    }

    /** nonce = sframe_salt XOR CTR (§4.4.3) */
    function nonceOf(key, ctr) {
        const c = beBytes(ctr, key.suite.nn);
        const out = new Uint8Array(key.suite.nn);
        for (let i = 0; i < out.length; i++) out[i] = key.salt[i] ^ c[i];
        return out;
    }

    async function encrypt(key, ctr, metadata, plaintext) {
        const header = encodeHeader(key.kid, ctr);
        const aad = concat(header, view(metadata || new Uint8Array(0)));
        const sealed = await seal(key.suite, key.aead, nonceOf(key, ctr), aad, view(plaintext));
        return concat(header, sealed);
    }
    async function decrypt(lookup, metadata, data) {
        const bytes = view(data);
        const header = decodeHeader(bytes, 0);
        if (!header) throw new Error("SFrame: no header");
        const key = await lookup(header.kid);
        if (!key) throw new Error("SFrame: unknown key id");
        const aad = concat(bytes.subarray(0, header.length), view(metadata || new Uint8Array(0)));
        const plaintext = await open(key.suite, key.aead, nonceOf(key, header.ctr), aad, bytes.subarray(header.length));
        return { kid: header.kid, ctr: header.ctr, plaintext };
    }

    // ---- the call's frame layout: the codec's leading bytes in the clear (they are SFrame's metadata), then the SFrame ----
    /**
     * How many leading bytes of a frame stay in the clear: none of an audio frame (Opus is fully encrypted); of a VP8 frame
     * its payload header, 10 bytes on a keyframe (the frame tag and the start code with the size) and 3 on others, as other
     * SFrame-over-SFU deployments keep them, so the SFU still sees keyframes and the size. A VP8 keyframe has the P bit
     * (bit 0 of byte 0) clear.
     */
    function clearBytes(kind, data) {
        const bytes = view(data);
        if (kind !== "video" || !bytes.length) return 0;
        return Math.min(bytes.length, (bytes[0] & 1) === 0 ? 10 : 3);
    }

    root.moquiSFrame = { SUITES, CALL_SUITE, encodeHeader, decodeHeader, deriveKey, deriveRaw, encrypt, decrypt, aeadEncrypt,
        aeadDecrypt, clearBytes, concat };
    if (root.moqui) root.moqui.sframe = root.moquiSFrame;
})(globalThis);
