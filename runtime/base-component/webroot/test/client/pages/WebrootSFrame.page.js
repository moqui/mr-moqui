/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
"use strict";
const S = window.moquiSFrame;
const hex = (bytes) => Array.from(bytes).map((b) => b.toString(16).padStart(2, "0")).join("");
const unhex = (text) => Uint8Array.from(String(text).match(/../g) || [], (h) => parseInt(h, 16));
// the vectors hold 64-bit KIDs and counters, past what a JSON number keeps: read them as text, then as BigInt
async function vectors() {
    const text = await (await fetch("/files/crates/moqui-rtc/testdata/sframe-rfc9605.json")).text();
    return JSON.parse(text.replace(/"(kid|ctr)": (\d+)/g, '"$1": "$2"'));
}

test("every header vector encodes and decodes (the KID and CTR up to 64 bits)", async () => {
    const v = await vectors();
    assert.equal(v.header.length, 289);
    for (const h of v.header) {
        assert.equal(hex(S.encodeHeader(BigInt(h.kid), BigInt(h.ctr))), h.encoded, "kid " + h.kid + " ctr " + h.ctr);
        const back = S.decodeHeader(unhex(h.encoded), 0);
        assert.equal(back.kid, BigInt(h.kid));
        assert.equal(back.ctr, BigInt(h.ctr));
        assert.equal(back.length * 2, h.encoded.length);
    }
    assert.equal(S.decodeHeader(unhex("ff0102"), 0), null);
});

test("the AES-CTR with HMAC AEAD of the three CTR suites", async () => {
    const v = await vectors();
    assert.equal(v.aes_ctr_hmac.length, 3);
    for (const a of v.aes_ctr_hmac) {
        const ct = await S.aeadEncrypt(a.cipher_suite, unhex(a.key), unhex(a.nonce), unhex(a.aad), unhex(a.pt));
        assert.equal(hex(ct), a.ct, "suite " + a.cipher_suite);
        assert.equal(hex(await S.aeadDecrypt(a.cipher_suite, unhex(a.key), unhex(a.nonce), unhex(a.aad), unhex(a.ct))), a.pt);
    }
});

test("each cipher suite's key and salt, and its frame, encrypt and decrypt as the RFC's vectors say; a changed byte fails", async () => {
    const v = await vectors();
    assert.equal(v.sframe.length, 5);
    for (const s of v.sframe) {
        const raw = await S.deriveRaw(s.cipher_suite, BigInt(s.kid), unhex(s.base_key));
        assert.equal(hex(raw.key), s.sframe_key, "suite " + s.cipher_suite);
        assert.equal(hex(raw.salt), s.sframe_salt);
        const key = await S.deriveKey(s.cipher_suite, BigInt(s.kid), unhex(s.base_key));
        assert.equal(hex(await S.encrypt(key, BigInt(s.ctr), unhex(s.metadata), unhex(s.pt))), s.ct, "suite " + s.cipher_suite);
        const out = await S.decrypt((kid) => (kid === BigInt(s.kid) ? key : null), unhex(s.metadata), unhex(s.ct));
        assert.equal(hex(out.plaintext), s.pt);
        assert.equal(out.ctr, BigInt(s.ctr));
        const tampered = unhex(s.ct);
        tampered[tampered.length - 1] ^= 1;
        await assert.rejects(S.decrypt(() => key, unhex(s.metadata), tampered));
        // other metadata (the clear bytes of a frame) fails too
        await assert.rejects(S.decrypt(() => key, unhex("00"), unhex(s.ct)));
        await assert.rejects(S.decrypt(() => null, unhex(s.metadata), unhex(s.ct)));
    }
});

test("a call's frame: VP8's payload header stays in the clear (10 bytes on a keyframe, 3 on others), Opus is encrypted whole", async () => {
    const key = await S.deriveKey(S.CALL_SUITE, 65537, new Uint8Array(16).fill(7));
    const keyframe = Uint8Array.from({ length: 40 }, (_, i) => (i === 0 ? 0x10 : i));
    const delta = Uint8Array.from({ length: 40 }, (_, i) => (i === 0 ? 0x11 : i));
    assert.equal(S.clearBytes("video", keyframe), 10);
    assert.equal(S.clearBytes("video", delta), 3);
    assert.equal(S.clearBytes("audio", keyframe), 0);
    assert.equal(S.clearBytes("video", new Uint8Array([0x10, 1])), 2);
    const clear = S.clearBytes("video", keyframe);
    const sealed = await S.encrypt(key, 5n, keyframe.subarray(0, clear), keyframe.subarray(clear));
    const frame = S.concat(keyframe.subarray(0, clear), sealed);
    // the SFU's view: the first ten bytes as sent, the keyframe bit clear
    assert.equal(hex(frame.subarray(0, 10)), hex(keyframe.subarray(0, 10)));
    assert.equal(frame[0] & 1, 0);
    assert.equal(frame.length, keyframe.length + S.encodeHeader(65537, 5).length + 16);
    const back = S.clearBytes("video", frame);
    const out = await S.decrypt(() => key, frame.subarray(0, back), frame.subarray(back));
    assert.equal(hex(S.concat(frame.subarray(0, back), out.plaintext)), hex(keyframe));
});

test("a call's frames come out as the sframe crate makes them (crates/moqui-rtc/testdata/sframe-call-frames.json)", async () => {
    const v = await (await fetch("/files/crates/moqui-rtc/testdata/sframe-call-frames.json")).json();
    assert.equal(v.frames.length, 4);
    for (const f of v.frames) {
        const key = await S.deriveKey(f.cipher_suite, f.kid, unhex(f.base_key));
        const frame = unhex(f.frame);
        const clear = S.clearBytes(f.kind, frame);
        assert.equal(clear, f.clear_bytes);
        const sealed = await S.encrypt(key, f.ctr, frame.subarray(0, clear), frame.subarray(clear));
        assert.equal(hex(S.concat(frame.subarray(0, clear), sealed)), f.encrypted, f.kind + " kid " + f.kid + " ctr " + f.ctr);
        const encrypted = unhex(f.encrypted);
        const out = await S.decrypt(() => key, encrypted.subarray(0, clear), encrypted.subarray(clear));
        assert.equal(hex(S.concat(encrypted.subarray(0, clear), out.plaintext)), f.frame);
    }
});

test("the worker's keys: a key that comes in for another leg, even under this sender's KID, is never sent with", async () => {
    const ring = S.keyRing(65536, 4);
    const mine = await S.deriveKey(S.CALL_SUITE, 65537, new Uint8Array(16).fill(1));
    const theirs = await S.deriveKey(S.CALL_SUITE, 65537, new Uint8Array(16).fill(2));
    assert.equal(ring.sendKey(), null);
    ring.setOwn(65537, mine, true);
    // a roster that gave another leg this sender's key index: its key lands under the same KID
    ring.setLeg("LEG_B", 65537, theirs);
    assert.equal(ring.sendKey(), mine);
    assert.equal(ring.useOwn(65537), true);
    assert.equal(ring.sendKey(), mine);
    // the sender moves only to a key of its own
    assert.equal(ring.useOwn(65538), false);
    ring.setLeg("LEG_B", 65538, theirs);
    assert.equal(ring.useOwn(65538), false);
    assert.equal(ring.sendKey(), mine);
    // a leg's frames decrypt only with that leg's keys, under its key index
    assert.equal(ring.lookup("LEG_B", 1, 65537n), theirs);
    assert.equal(ring.lookup("LEG_C", 1, 65537n), null);
    assert.equal(ring.lookup("LEG_B", 2, 65537n), null);
    assert.equal(ring.lookup("LEG_B", undefined, 65537n), null);
    assert.equal(ring.lookup("LEG_B", 1, 1n << 60n), null);
    // the last four epochs of a leg, and none once it is dropped
    for (let epoch = 3; epoch <= 6; epoch++) ring.setLeg("LEG_B", 65536 + epoch, theirs);
    assert.equal(ring.lookup("LEG_B", 1, 65538n), null);
    assert.equal(ring.lookup("LEG_B", 1, 65542n), theirs);
    ring.dropLeg("LEG_B");
    assert.equal(ring.lookup("LEG_B", 1, 65542n), null);
});

test("a receiver takes each counter once: a repeated frame or one far behind is refused, a little out of order is taken", async () => {
    const w = S.replayWindow(8);
    assert.equal(w.accept(0n), true);
    assert.equal(w.accept(0n), false);
    assert.equal(w.accept(5n), true);
    assert.equal(w.accept(3n), true);
    assert.equal(w.accept(3n), false);
    assert.equal(w.accept(5n), false);
    for (let c = 6n; c < 40n; c++) assert.equal(w.accept(c), true);
    // within the window below the top: taken once
    assert.equal(w.accept(32n), false);
    assert.equal(w.accept(31n), false);
    // older than the window: refused even when never seen
    assert.equal(w.accept(30n), false);
    assert.equal(w.accept(1n), false);
    assert.equal(w.accept(1000n), true);
});
