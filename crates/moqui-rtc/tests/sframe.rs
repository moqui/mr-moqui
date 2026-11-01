//! SFrame (RFC 9605) for end-to-end encrypted calls (W8, docs/interfaces/WebRTC.md "Encrypted calls"). The browser does
//! the encryption (webroot `js/WebrootSFrame.js`, in its RTCRtpScriptTransform worker); no Rust code encrypts call media.
//! These tests hold the vectors the browser's code is checked with (the webroot client spec `WebrootSFrame`) to an
//! independent implementation, the `sframe` crate: the RFC's Appendix C vectors, and the call's frame layout (the codec's
//! leading bytes in the clear as SFrame metadata, then the header and the ciphertext).

use serde_json::Value;
use sframe::{
    frame::{EncryptedFrameView, MediaFrame, MonotonicCounter},
    header::SframeHeader,
    key::{DecryptionKey, EncryptionKey},
    CipherSuite,
};

const RFC_VECTORS: &str = include_str!("../testdata/sframe-rfc9605.json");
const CALL_FRAMES: &str = include_str!("../testdata/sframe-call-frames.json");

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn suite(id: u64) -> CipherSuite {
    match id {
        1 => CipherSuite::AesCtr128HmacSha256_80,
        2 => CipherSuite::AesCtr128HmacSha256_64,
        3 => CipherSuite::AesCtr128HmacSha256_32,
        4 => CipherSuite::AesGcm128Sha256,
        5 => CipherSuite::AesGcm256Sha512,
        other => panic!("unknown cipher suite {other}"),
    }
}

fn u64_of(value: &Value) -> u64 {
    value.as_u64().unwrap_or_else(|| value.to_string().parse().unwrap())
}

/// Encrypts `payload` with `metadata` in the clear before it, as a call's sender does; the frame as it goes out.
fn encrypt(suite_id: u64, kid: u64, base_key: &[u8], ctr: u64, metadata: &[u8], payload: &[u8]) -> Vec<u8> {
    let key = EncryptionKey::derive_from(suite(suite_id), kid, base_key).unwrap();
    let mut counter = MonotonicCounter::with_start_value(ctr, u64::MAX);
    let frame = MediaFrame::try_with_meta_data(&mut counter, payload, metadata).unwrap();
    frame.encrypt(&key).unwrap().as_ref().to_vec()
}

fn decrypt(suite_id: u64, kid: u64, base_key: &[u8], metadata: &[u8], sframe: &[u8]) -> Option<Vec<u8>> {
    let key = DecryptionKey::derive_from(suite(suite_id), kid, base_key).unwrap();
    let view = EncryptedFrameView::try_with_meta_data(sframe, metadata).ok()?;
    let mut buffer = Vec::new();
    let out = view.decrypt_into(&key, &mut buffer).ok()?;
    Some(out.payload().to_vec())
}

#[test]
fn the_rfc_header_vectors_encode_and_decode() {
    let vectors: Value = serde_json::from_str(RFC_VECTORS).unwrap();
    let headers = vectors["header"].as_array().unwrap();
    assert_eq!(headers.len(), 289);
    for h in headers {
        let (kid, ctr, encoded) = (
            u64_of(&h["kid"]),
            u64_of(&h["ctr"]),
            unhex(h["encoded"].as_str().unwrap()),
        );
        let header = SframeHeader::new(kid, ctr);
        let mut buffer = vec![0u8; header.len()];
        header.serialize(&mut buffer).unwrap();
        assert_eq!(buffer, encoded, "kid {kid} ctr {ctr}");
        let back = SframeHeader::deserialize(&encoded).unwrap();
        assert_eq!((back.key_id(), back.counter(), back.len()), (kid, ctr, encoded.len()));
    }
}

#[test]
fn the_rfc_frame_vectors_encrypt_and_decrypt_in_every_suite() {
    let vectors: Value = serde_json::from_str(RFC_VECTORS).unwrap();
    let frames = vectors["sframe"].as_array().unwrap();
    assert_eq!(frames.len(), 5);
    for v in frames {
        let id = u64_of(&v["cipher_suite"]);
        let (kid, ctr) = (u64_of(&v["kid"]), u64_of(&v["ctr"]));
        let base = unhex(v["base_key"].as_str().unwrap());
        let metadata = unhex(v["metadata"].as_str().unwrap());
        let pt = unhex(v["pt"].as_str().unwrap());
        let ct = unhex(v["ct"].as_str().unwrap());
        // the crate puts the metadata before the SFrame, as a call's frame keeps the codec's bytes before it
        let out = encrypt(id, kid, &base, ctr, &metadata, &pt);
        assert_eq!(hex(&out[..metadata.len()]), hex(&metadata), "suite {id}");
        assert_eq!(hex(&out[metadata.len()..]), hex(&ct), "suite {id}");
        assert_eq!(
            decrypt(id, kid, &base, &metadata, &ct).as_deref(),
            Some(&pt[..]),
            "suite {id}"
        );
        let mut tampered = ct.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert_eq!(
            decrypt(id, kid, &base, &metadata, &tampered),
            None,
            "suite {id}: a changed tag"
        );
        assert_eq!(
            decrypt(id, kid, &base, b"other", &ct),
            None,
            "suite {id}: other metadata"
        );
    }
}

#[test]
fn a_call_s_frames_are_what_the_browser_makes() {
    let vectors: Value = serde_json::from_str(CALL_FRAMES).unwrap();
    let frames = vectors["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 4);
    for f in frames {
        let id = u64_of(&f["cipher_suite"]);
        assert_eq!(id, 4, "calls use AES_128_GCM_SHA256_128");
        let (kid, ctr) = (u64_of(&f["kid"]), u64_of(&f["ctr"]));
        let base = unhex(f["base_key"].as_str().unwrap());
        let frame = unhex(f["frame"].as_str().unwrap());
        let encrypted = unhex(f["encrypted"].as_str().unwrap());
        // VP8: the payload header in the clear, 10 bytes on a keyframe (P bit clear) and 3 on others; Opus none
        let clear = match f["kind"].as_str().unwrap() {
            "video" if frame[0] & 1 == 0 => 10.min(frame.len()),
            "video" => 3.min(frame.len()),
            _ => 0,
        };
        assert_eq!(clear as u64, u64_of(&f["clear_bytes"]));
        let out = encrypt(id, kid, &base, ctr, &frame[..clear], &frame[clear..]);
        assert_eq!(hex(&out), hex(&encrypted), "{} kid {kid} ctr {ctr}", f["kind"]);
        let plain = decrypt(id, kid, &base, &encrypted[..clear], &encrypted[clear..]).unwrap();
        assert_eq!(hex(&[&encrypted[..clear], &plain[..]].concat()), hex(&frame));
    }
}
