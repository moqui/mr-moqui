/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! The transcription call against a fake endpoint on loopback: the form it sends, the text it reads, a refusal and a
//! timeout. The quota and the `LlmCall` row of [`super::transcribe`] are the drawer's (`tests/it/assist.rs`).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use moqui_core::llm_hooks::SpeechRequest;

use super::*;
use crate::conf::{EndpointConf, ModelConf, ProtocolId, ReasoningLevel};

/// A one-request endpoint: it reads the request, waits `delay`, answers `status` with `body`, and hands back what it
/// read.
fn endpoint(status: u16, body: &'static str, delay: Duration) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else { return };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut seen = Vec::new();
        let mut chunk = [0_u8; 8192];
        while let Ok(count) = stream.read(&mut chunk) {
            if count == 0 {
                break;
            }
            seen.extend_from_slice(&chunk[..count]);
            let text = String::from_utf8_lossy(&seen);
            if let Some(at) = text.find("\r\n\r\n") {
                let length: usize = text[..at]
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().ok())?
                    })
                    .unwrap_or(0);
                if seen.len() >= at + 4 + length {
                    break;
                }
            }
        }
        let _ = tx.send(String::from_utf8_lossy(&seen).into_owned());
        std::thread::sleep(delay);
        let _ = write!(
            stream,
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
    });
    (url, rx)
}

fn target(url: &str) -> Target {
    Target {
        endpoint: EndpointConf::new("speech", ProtocolId::ChatCompletions, url),
        model: ModelConf::new("speech-model", "speech", "whisper-test"),
        reasoning: ReasoningLevel::None,
        feature: Some(FEATURE.to_string()),
        fell_back_from: None,
    }
}

fn request(timeout: Duration) -> SpeechRequest<'static> {
    SpeechRequest {
        audio: b"RIFF\x24\x00\x00\x00WAVEfmt ",
        media_type: "audio/wav",
        language: Some("en"),
        prompt: Some("Moqui, Ann"),
        user_id: None,
        purpose: "captions:TEST",
        timeout: Some(timeout),
    }
}

#[test]
fn a_transcription_posts_the_form_and_reads_the_text() {
    let (url, seen) = endpoint(200, r#"{"text":"  Hello from the call "}"#, Duration::ZERO);
    let text = transcribe_with(&target(&url), &request(Duration::from_secs(5))).unwrap();
    assert_eq!(text, "Hello from the call");
    let seen = seen.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(seen.starts_with("POST /v1/audio/transcriptions "), "{seen}");
    for part in [
        "name=\"model\"\r\n\r\nwhisper-test\r\n",
        "name=\"response_format\"\r\n\r\njson\r\n",
        "name=\"language\"\r\n\r\nen\r\n",
        "name=\"prompt\"\r\n\r\nMoqui, Ann\r\n",
        "filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\nRIFF",
    ] {
        assert!(seen.contains(part), "{part:?} in {seen}");
    }
}

#[test]
fn a_refusal_is_an_error_naming_the_status() {
    let (url, _seen) = endpoint(429, r#"{"error":"slow down"}"#, Duration::ZERO);
    let error = transcribe_with(&target(&url), &request(Duration::from_secs(5))).unwrap_err();
    assert!(error.message().contains("HTTP 429"), "{error}");
}

#[test]
fn a_provider_slower_than_the_timeout_fails_in_time() {
    let (url, _seen) = endpoint(200, r#"{"text":"late"}"#, Duration::from_millis(1_500));
    let started = Instant::now();
    let answered = transcribe_with(&target(&url), &request(Duration::from_millis(200)));
    assert!(answered.is_err(), "{answered:?}");
    assert!(
        started.elapsed() < Duration::from_millis(1_200),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn only_plain_language_tags_and_audio_types_go_out() {
    assert!(plain_language("en") && plain_language("pt-BR"));
    assert!(!plain_language("") && !plain_language("en\r\nX: y") && !plain_language("toolonglang"));
    assert_eq!(audio_type("audio/WAV; codecs=1").as_deref(), Some("audio/wav"));
    assert_eq!(audio_type("video/webm").as_deref(), Some("video/webm"));
    assert_eq!(audio_type("text/plain"), None);
}
