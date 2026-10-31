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

use std::collections::HashMap;

use super::*;

fn settings(pairs: &[(&str, &str)]) -> Result<Option<SipSettings>, String> {
    let map: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    settings_from(&|name| map.get(name).cloned().unwrap_or_default())
}

#[test]
fn the_listeners_are_off_until_a_port_is_set_and_take_their_hosts() {
    assert_eq!(settings(&[("sip_rtp_ports", "20000-20999")]), Ok(None));
    let s = settings(&[("sip_port", "5060")]).unwrap().unwrap();
    assert_eq!(s.listen, Some("127.0.0.1:5060".parse().unwrap()));
    assert_eq!(s.tls, None);
    assert_eq!(s.rtp_ports, "20000-20999");
    assert_eq!(s.rtp_host, "127.0.0.1".parse::<IpAddr>().unwrap());
    let s = settings(&[
        ("sip_tls_port", "5061"),
        ("sip_host", "0.0.0.0"),
        ("sip_tls_host", "127.0.0.2"),
        ("sip_rtp_host", "127.0.0.3"),
        ("sip_rtp_ports", "30000-30010"),
        ("sip_public_address", "198.51.100.20"),
        ("sip_tls_ca_file", "/nowhere/ca.pem"),
    ])
    .unwrap()
    .unwrap();
    assert_eq!(s.listen, None);
    assert_eq!(s.tls, Some("127.0.0.2:5061".parse().unwrap()));
    assert_eq!(s.rtp_host, "127.0.0.3".parse::<IpAddr>().unwrap());
    assert_eq!(s.public, Some("198.51.100.20".parse().unwrap()));
    assert_eq!(s.ca_file, Some(PathBuf::from("/nowhere/ca.pem")));
    assert!(settings(&[("sip_port", "fifty")]).is_err());
    assert!(settings(&[("sip_port", "5060"), ("sip_public_address", "sip.example")]).is_err());
}

#[test]
fn a_port_range_hands_out_free_ports_and_wraps() {
    let range = PortRange::parse("41850-41852", "127.0.0.1".parse().unwrap()).unwrap();
    let a = range.take().unwrap();
    let b = range.take().unwrap();
    assert_ne!(a.local_addr().unwrap().port(), b.local_addr().unwrap().port());
    let c = range.take();
    let d = range.take();
    // three ports, so a fourth is refused while the three are held (one may be taken by another process)
    assert!(c.is_none() || d.is_none());
    drop((a, b, c, d));
    assert!(range.take().is_some(), "freed ports come back");
    assert!(PortRange::parse("2000", "127.0.0.1".parse().unwrap()).is_err());
    assert!(PortRange::parse("3000-2000", "127.0.0.1".parse().unwrap()).is_err());
}

/// A WAV file of 16-bit PCM.
fn wav(rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2 * u32::from(channels)).to_le_bytes());
    out.extend_from_slice(&(2 * channels).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    // a chunk the reader skips, with its pad byte
    out.extend_from_slice(b"LIST");
    out.extend_from_slice(&3u32.to_le_bytes());
    out.extend_from_slice(b"abc\0");
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    out
}

#[test]
fn each_notice_is_its_own_tone_pattern_then_the_deployments_audio() {
    use prompt::{notice_prompt, Notice};
    let length = |notice, audio: Option<&[i16]>| notice_prompt(notice, audio).0.len();
    // 400 ms; 400 + 200 + 400 ms; 250 + 150 + 250 ms, at 48 kHz
    assert_eq!(length(Notice::Recording, None), 400 * 48);
    assert_eq!(length(Notice::RecordingAck, None), 1000 * 48);
    assert_eq!(length(Notice::Captions, None), 650 * 48);
    // the ack pattern is two bursts: silence in its middle, sound on both sides
    let ack = notice_prompt(Notice::RecordingAck, None).0;
    assert!(ack[500 * 48..600 * 48].iter().all(|s| *s == 0));
    assert!(ack[100 * 48..300 * 48].iter().any(|s| s.abs() > 3000));
    assert!(ack[700 * 48..900 * 48].iter().any(|s| s.abs() > 3000));
    // the audio follows a 300 ms gap
    let audio = vec![1234i16; 4800];
    let with = notice_prompt(Notice::Captions, Some(&audio)).0;
    assert_eq!(with.len(), (650 + 300) * 48 + 4800);
    assert!(with[with.len() - 4800..].iter().all(|s| *s == 1234));
    assert_eq!(Notice::from_code("recordingAck"), Notice::RecordingAck);
    assert_eq!(Notice::from_code("captions"), Notice::Captions);
    assert_eq!(Notice::from_code(""), Notice::Recording);
    assert_eq!(Notice::Captions.audio_property(), "call_captions_prompt_audio");
    assert_eq!(Notice::RecordingAck.audio_property(), "call_recording_prompt_audio");
}

#[test]
fn prompt_audio_is_a_wav_file_made_48_khz_mono() {
    use prompt::wav_pcm48;
    // 48 kHz mono as it is
    assert_eq!(wav_pcm48(&wav(48_000, 1, &[1, 2, 3])).unwrap(), vec![1, 2, 3]);
    // stereo is mixed down
    assert_eq!(
        wav_pcm48(&wav(48_000, 2, &[100, 300, -50, -150])).unwrap(),
        vec![200, -100]
    );
    // 8 kHz is six times as many samples
    assert_eq!(wav_pcm48(&wav(8_000, 1, &[1000; 800])).unwrap().len(), 4800);
    // what is not 16-bit PCM WAV is refused, whatever its name
    assert!(wav_pcm48(b"OggS not a wav").is_err());
    let mut eight_bit = wav(8_000, 1, &[0; 10]);
    eight_bit[34] = 8;
    assert!(wav_pcm48(&eight_bit).unwrap_err().contains("16-bit"));
    assert!(wav_pcm48(&wav(96_000, 1, &[0; 10])).is_err());
    assert!(wav_pcm48(&wav(48_000, 6, &[0; 12])).is_err());
    // a data chunk that claims more than the file has ends at the file
    let mut short = wav(48_000, 1, &[5, 6]);
    let n = short.len();
    short[n - 8..n - 4].copy_from_slice(&1000u32.to_le_bytes());
    assert_eq!(wav_pcm48(&short).unwrap(), vec![5, 6]);
}

#[test]
fn a_prompt_queues_after_the_one_playing() {
    let mut queue = vec![1, 2, 3, 4];
    let mut at = 3;
    leg::queue_prompt(&mut queue, &mut at, &[7, 8]);
    assert_eq!((queue.as_slice(), at), (&[4, 7, 8][..], 0));
    // one fully played is gone
    let mut at = 3;
    leg::queue_prompt(&mut queue, &mut at, &[9]);
    assert_eq!(queue, vec![9]);
}
