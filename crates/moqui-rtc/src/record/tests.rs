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

//! The recording writers and the recorder: Ogg pages read back with their checksums and granule positions, a WebM file
//! walked as EBML, and segments by period, cap and leg.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Cursor, Seek, SeekFrom, Write};
use std::rc::Rc;
use std::sync::Arc;

use super::ogg::{crc32, opus_packet_samples, OggOpusWriter};
use super::webm::{self, vp8_keyframe_size, WebmWriter};
use super::{RecordConfig, RecordError, Recorder, Spool, TrackKind};
use crate::audio::opus::OpusEncoder;
use crate::audio::FRAME;
use crate::sfu::Source;
use crate::tap::{Kind, OwnedFrame, TapEvent};

// ---------- reading the files back ----------

struct Page {
    header_type: u8,
    granule: u64,
    serial: u32,
    sequence: u32,
    packets: Vec<Vec<u8>>,
}

/// The pages of an Ogg stream, each checksum checked; panics on a malformed page.
fn pages(data: &[u8]) -> Vec<Page> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let page = &data[at..];
        assert_eq!(&page[..4], b"OggS", "capture pattern at {at}");
        assert_eq!(page[4], 0, "version");
        let count = page[26] as usize;
        let lacing = &page[27..27 + count];
        let body_len: usize = lacing.iter().map(|l| *l as usize).sum();
        let total = 27 + count + body_len;
        let mut copy = page[..total].to_vec();
        let crc = u32::from_le_bytes(copy[22..26].try_into().unwrap());
        copy[22..26].copy_from_slice(&[0; 4]);
        assert_eq!(crc32(&copy), crc, "page checksum at {at}");
        let mut packets = Vec::new();
        let mut current = Vec::new();
        let mut body_at = 27 + count;
        for l in lacing {
            current.extend_from_slice(&page[body_at..body_at + *l as usize]);
            body_at += *l as usize;
            if *l < 255 {
                packets.push(std::mem::take(&mut current));
            }
        }
        out.push(Page {
            header_type: page[5],
            granule: u64::from_le_bytes(page[6..14].try_into().unwrap()),
            serial: u32::from_le_bytes(page[14..18].try_into().unwrap()),
            sequence: u32::from_le_bytes(page[18..22].try_into().unwrap()),
            packets,
        });
        at += total;
    }
    out
}

#[derive(Debug)]
struct Element {
    id: u32,
    /// Where the element starts in the file, and its body.
    at: usize,
    body: Vec<u8>,
    children: Vec<Element>,
}

fn read_id(data: &[u8], at: usize) -> (u32, usize) {
    let first = data[at];
    let len = first.leading_zeros() as usize + 1;
    let mut id = 0u32;
    for b in &data[at..at + len] {
        id = (id << 8) | u32::from(*b);
    }
    (id, len)
}

fn read_size(data: &[u8], at: usize) -> (u64, usize) {
    let first = data[at];
    let len = first.leading_zeros() as usize + 1;
    let mut size = u64::from(first) & ((1u64 << (8 - len)) - 1);
    for b in &data[at + 1..at + len] {
        size = (size << 8) | u64::from(*b);
    }
    (size, len)
}

const MASTERS: [u32; 9] = [
    0x1A45_DFA3,
    webm::SEGMENT,
    0x114D_9B74,
    0x4DBB,
    webm::INFO,
    webm::TRACKS,
    0xAE,
    webm::CLUSTER,
    webm::CUES,
];

/// The EBML elements of `data[from..to]`, masters read into; every size must fit its parent.
fn elements(data: &[u8], from: usize, to: usize) -> Vec<Element> {
    let mut out = Vec::new();
    let mut at = from;
    while at < to {
        let (id, id_len) = read_id(data, at);
        let (size, size_len) = read_size(data, at + id_len);
        let start = at + id_len + size_len;
        let end = start + size as usize;
        assert!(
            end <= to,
            "element {id:x} at {at} ends at {end}, past its parent's {to}"
        );
        let children = if MASTERS.contains(&id) || [0xBB, 0xB7, 0xE0].contains(&id) {
            elements(data, start, end)
        } else {
            Vec::new()
        };
        out.push(Element {
            id,
            at,
            body: data[start..end].to_vec(),
            children,
        });
        at = end;
    }
    out
}

fn child(element: &Element, id: u32) -> &Element {
    element
        .children
        .iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("no {id:x} in {:x}", element.id))
}

fn uint(body: &[u8]) -> u64 {
    body.iter().fold(0u64, |v, b| (v << 8) | u64::from(*b))
}

// ---------- test media ----------

/// A VP8 keyframe header of the given size, then filler.
fn vp8_keyframe(width: u16, height: u16, len: usize) -> Vec<u8> {
    let mut frame = vec![0x10, 0x02, 0x00, 0x9D, 0x01, 0x2A];
    frame.extend_from_slice(&width.to_le_bytes());
    frame.extend_from_slice(&height.to_le_bytes());
    frame.resize(len, 0x55);
    frame
}

fn vp8_delta(len: usize) -> Vec<u8> {
    let mut frame = vec![0x11, 0x02, 0x00];
    frame.resize(len, 0x66);
    frame
}

/// A 20 ms Opus packet of a tone, really encoded (so the mix decodes it).
fn opus_frames(count: usize) -> Vec<Vec<u8>> {
    let mut encoder = OpusEncoder::new(32_000).unwrap();
    (0..count)
        .map(|n| {
            let pcm: Vec<i16> = (0..FRAME)
                .map(|i| {
                    let t = (n * FRAME + i) as f64 / 48_000.0;
                    ((t * 440.0 * std::f64::consts::TAU).sin() * 8_000.0) as i16
                })
                .collect();
            encoder.encode(&pcm).unwrap().to_vec()
        })
        .collect()
}

fn audio(leg: &str, rtp_time: u64, data: &[u8]) -> TapEvent {
    TapEvent::Frame(OwnedFrame {
        leg: Arc::from(leg),
        kind: Kind::Audio,
        source: Source::Microphone,
        rid: None,
        codec: "opus",
        rtp_time,
        clock_rate: 48_000,
        data: data.to_vec(),
        keyframe: false,
        audio_level: None,
    })
}

fn video(leg: &str, rid: Option<&str>, rtp_time: u64, keyframe: bool, data: Vec<u8>) -> TapEvent {
    TapEvent::Frame(OwnedFrame {
        leg: Arc::from(leg),
        kind: Kind::Video,
        source: Source::Camera,
        rid: rid.map(str::to_string),
        codec: "vp8",
        rtp_time,
        clock_rate: 90_000,
        data,
        keyframe,
        audio_level: None,
    })
}

/// A file in memory that its spool can still read after the writer is done with it.
#[derive(Clone, Default)]
struct Shared(Rc<RefCell<Cursor<Vec<u8>>>>);

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Seek for Shared {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.0.borrow_mut().seek(pos)
    }
}

#[derive(Clone, Default)]
struct MemorySpool {
    files: Rc<RefCell<HashMap<String, Shared>>>,
    discarded: Rc<RefCell<Vec<String>>>,
    fail: bool,
}

impl MemorySpool {
    fn bytes(&self, name: &str) -> Vec<u8> {
        self.files.borrow()[name].0.borrow().get_ref().clone()
    }
}

impl Spool for MemorySpool {
    type File = Shared;
    fn create(&mut self, name: &str) -> std::io::Result<Shared> {
        if self.fail {
            return Err(std::io::Error::other("disk full"));
        }
        let file = Shared::default();
        self.files.borrow_mut().insert(name.to_string(), file.clone());
        Ok(file)
    }
    fn discard(&mut self, name: &str) {
        self.files.borrow_mut().remove(name);
        self.discarded.borrow_mut().push(name.to_string());
    }
}

// ---------- Ogg ----------

#[test]
fn opus_packet_samples_follow_the_toc() {
    assert_eq!(opus_packet_samples(&[0x78]), Some(960)); // config 15: hybrid FB 20 ms, one frame
    assert_eq!(opus_packet_samples(&[0xF8]), Some(960)); // config 31: CELT FB 20 ms
    assert_eq!(opus_packet_samples(&[0x08]), Some(960)); // config 1: SILK NB 20 ms
    assert_eq!(opus_packet_samples(&[0x19]), Some(5_760)); // config 3: 60 ms, two frames
    assert_eq!(opus_packet_samples(&[0xFB, 0x03]), Some(2_880)); // three frames of 20 ms
    assert_eq!(opus_packet_samples(&[0x1B, 0x03]), None); // three frames of 60 ms: past 120 ms
    assert_eq!(opus_packet_samples(&[]), None);
}

#[test]
fn ogg_opus_pages_carry_the_headers_checksums_and_granule_positions() {
    let mut writer = OggOpusWriter::new(Cursor::new(Vec::new()), 7, 312).unwrap();
    let packet = vec![0xF8u8; 300]; // two lacing values: 255 and 45
    for n in 0..60u64 {
        assert!(writer.write(n * 960, 960, &packet).unwrap());
    }
    // a late one is dropped, and so is one too big for a page (packets are never continued)
    assert!(!writer.write(10 * 960, 960, &packet).unwrap());
    assert!(!writer
        .write(60 * 960, 960, &vec![0xF8u8; super::ogg::MAX_PACKET + 1])
        .unwrap());
    let (out, bytes) = writer.finish().unwrap();
    let data = out.into_inner();
    assert_eq!(bytes, data.len() as u64);
    let pages = pages(&data);
    assert_eq!(pages[0].header_type, 0x02, "the first page begins the stream");
    assert_eq!(&pages[0].packets[0][..8], b"OpusHead");
    assert_eq!(pages[0].packets[0][9], 1, "mono");
    assert_eq!(
        u16::from_le_bytes([pages[0].packets[0][10], pages[0].packets[0][11]]),
        312
    );
    assert_eq!(&pages[1].packets[0][..8], b"OpusTags");
    assert_eq!((pages[0].granule, pages[1].granule), (0, 0));
    assert!(pages.iter().all(|p| p.serial == 7));
    assert!(pages.iter().enumerate().all(|(i, p)| p.sequence == i as u32));
    let audio = &pages[2..];
    assert_eq!(audio.iter().map(|p| p.packets.len()).sum::<usize>(), 60);
    assert!(audio.iter().all(|p| p.packets.iter().all(|x| x.len() == 300)));
    // one second a page: 50 packets, then the 10 left in the last page, which ends the stream
    assert_eq!(audio[0].packets.len(), 50);
    assert_eq!(audio[0].granule, 312 + 50 * 960);
    let last = audio.last().unwrap();
    assert_eq!(last.header_type, 0x04);
    assert_eq!(last.granule, 312 + 60 * 960);
}

#[test]
fn a_gap_is_a_jump_in_granule_positions_at_a_page_boundary() {
    let mut writer = OggOpusWriter::new(Cursor::new(Vec::new()), 1, 0).unwrap();
    for n in 0..10u64 {
        writer.write(n * 960, 960, &[0xF8, 1, 2]).unwrap();
    }
    // two seconds missing
    for n in 110..115u64 {
        writer.write(n * 960, 960, &[0xF8, 1, 2]).unwrap();
    }
    assert_eq!(writer.duration_ms(), 115 * 20);
    let (out, _) = writer.finish().unwrap();
    let pages = pages(&out.into_inner());
    let audio = &pages[2..];
    assert_eq!(audio.len(), 2, "the gap ends a page");
    assert_eq!((audio[0].packets.len(), audio[0].granule), (10, 10 * 960));
    assert_eq!((audio[1].packets.len(), audio[1].granule), (5, 115 * 960));
    assert_eq!(
        audio.iter().map(|p| p.packets.len()).sum::<usize>(),
        15,
        "no packet made up for the gap"
    );
}

// ---------- WebM ----------

#[test]
fn webm_is_ebml_with_the_vp8_track_keyframe_clusters_and_cues() {
    let mut writer = WebmWriter::new(Cursor::new(Vec::new()));
    assert!(
        !writer.write(0, false, &vp8_delta(50)).unwrap(),
        "nothing before a keyframe"
    );
    assert_eq!(vp8_keyframe_size(&vp8_keyframe(640, 360, 40)), Some((640, 360)));
    let mut time = 0;
    for n in 0..90u64 {
        let key = n % 30 == 0;
        let frame = if key {
            vp8_keyframe(640, 360, 400)
        } else {
            vp8_delta(120)
        };
        time = n * 33 + if n >= 60 { 5_000 } else { 0 }; // a five-second gap before the third keyframe
        assert!(writer.write(time, key, &frame).unwrap());
    }
    assert!(
        !writer.write(time - 1, false, &vp8_delta(10)).unwrap(),
        "a frame from the past is dropped"
    );
    let (out, bytes) = writer.finish().unwrap();
    let data = out.into_inner();
    assert_eq!(bytes, data.len() as u64);
    let top = elements(&data, 0, data.len());
    assert_eq!(top.len(), 2, "the EBML header and the Segment");
    assert_eq!(top[0].id, 0x1A45_DFA3);
    assert_eq!(child(&top[0], 0x4282).body, b"webm");
    let segment = &top[1];
    assert_eq!(segment.id, webm::SEGMENT);
    let segment_start = segment.at + 4 + 8;
    let info = child(segment, webm::INFO);
    assert_eq!(uint(&child(info, 0x2A_D7B1).body), 1_000_000);
    let duration = f64::from_be_bytes(child(info, webm::DURATION).body[..].try_into().unwrap());
    assert!((duration - (time + 33) as f64).abs() < 0.5, "duration {duration}");
    let entry = child(child(segment, webm::TRACKS), 0xAE);
    assert_eq!(child(entry, 0x86).body, b"V_VP8");
    let video = child(entry, 0xE0);
    assert_eq!(
        (uint(&child(video, 0xB0).body), uint(&child(video, 0xBA).body)),
        (640, 360)
    );
    let clusters: Vec<&Element> = segment.children.iter().filter(|e| e.id == webm::CLUSTER).collect();
    assert_eq!(clusters.len(), 3, "a cluster at each keyframe");
    let mut blocks = 0;
    for (i, cluster) in clusters.iter().enumerate() {
        let start = uint(&child(cluster, webm::TIMECODE).body);
        assert_eq!(start, [0, 30 * 33, 60 * 33 + 5_000][i]);
        for (j, block) in cluster
            .children
            .iter()
            .filter(|e| e.id == webm::SIMPLE_BLOCK)
            .enumerate()
        {
            assert_eq!(block.body[0], 0x81, "track 1");
            assert_eq!(
                block.body[3] & 0x80 != 0,
                j == 0,
                "only the first block of a cluster is a keyframe"
            );
            let relative = i16::from_be_bytes([block.body[1], block.body[2]]);
            assert_eq!(relative as u64, j as u64 * 33);
            blocks += 1;
        }
    }
    assert_eq!(blocks, 90);
    // the cues point at the clusters, and the SeekHead at the cues
    let cues = child(segment, webm::CUES);
    let places: Vec<u64> = cues
        .children
        .iter()
        .map(|point| uint(&child(child(point, 0xB7), 0xF1).body))
        .collect();
    let cluster_places: Vec<u64> = clusters.iter().map(|c| (c.at - segment_start) as u64).collect();
    assert_eq!(places, cluster_places);
    let seek_head = child(segment, 0x114D_9B74);
    let cue_seek = seek_head
        .children
        .iter()
        .find(|s| child(s, 0x53AB).body == [0x1C, 0x53, 0xBB, 0x6B])
        .expect("a seek to the cues");
    assert_eq!(uint(&child(cue_seek, 0x53AC).body), (cues.at - segment_start) as u64);
}

// ---------- the recorder ----------

fn recorder(config: RecordConfig, spool: &MemorySpool) -> Recorder<MemorySpool> {
    Recorder::new(config, spool.clone(), 1, false, 0).unwrap()
}

#[test]
fn tracks_and_the_mix_are_files_and_a_pause_starts_the_next_period() {
    let spool = MemorySpool::default();
    let mut r = recorder(RecordConfig::default(), &spool);
    let voice = opus_frames(150);
    let mut now = 0;
    // a second of two voices and a camera, the mix ticking
    for n in 0..50u64 {
        r.event(now, &audio("a", 1_000 + n * 960, &voice[n as usize])).unwrap();
        r.event(now, &audio("b", 7_000_000 + n * 960, &voice[n as usize]))
            .unwrap();
        if n % 2 == 0 {
            let frame = if n == 0 {
                vp8_keyframe(320, 180, 300)
            } else {
                vp8_delta(100)
            };
            r.event(now, &video("a", None, 500 + n * 1_800, n == 0, frame)).unwrap();
        }
        r.tick(now).unwrap();
        now += 20;
    }
    r.pause(now).unwrap();
    let first = r.take_closed();
    let kinds: Vec<(TrackKind, Option<&str>)> = {
        let mut k: Vec<_> = first.iter().map(|s| (s.track.kind, s.track.leg.as_deref())).collect();
        k.sort_by_key(|(kind, leg)| (kind.as_str(), leg.map(str::to_string)));
        k
    };
    assert_eq!(
        kinds,
        vec![
            (TrackKind::Audio, Some("a")),
            (TrackKind::Audio, Some("b")),
            (TrackKind::Mix, None),
            (TrackKind::Video, Some("a")),
        ]
    );
    assert!(first.iter().all(|s| s.period == 1 && s.seq == 1 && !s.track_ended));
    let voice_a = first
        .iter()
        .find(|s| s.track.kind == TrackKind::Audio && s.track.leg.as_deref() == Some("a"))
        .unwrap();
    assert_eq!(voice_a.duration_ms, 1_000);
    let read = pages(&spool.bytes(&voice_a.file));
    assert_eq!(read[2..].iter().map(|p| p.packets.len()).sum::<usize>(), 50);
    assert_eq!(read.last().unwrap().granule, 50 * 960);
    let mix = first.iter().find(|s| s.track.kind == TrackKind::Mix).unwrap();
    let mix_pages = pages(&spool.bytes(&mix.file));
    assert_eq!(
        mix_pages[2..].iter().map(|p| p.packets.len()).sum::<usize>(),
        50,
        "a mixed packet each tick"
    );
    assert_eq!(mix_pages.last().unwrap().granule, 312 + 50 * 960);
    let camera = first.iter().find(|s| s.track.kind == TrackKind::Video).unwrap();
    let data = spool.bytes(&camera.file);
    assert_eq!(elements(&data, 0, data.len())[1].id, webm::SEGMENT);
    assert_eq!(camera.bytes, data.len() as u64);

    // paused: nothing is written
    for n in 50..60u64 {
        r.event(now, &audio("a", 1_000 + n * 960, &voice[n as usize])).unwrap();
        r.tick(now).unwrap();
        now += 20;
    }
    assert!(r.take_closed().is_empty());
    assert_eq!(r.open_files().len(), 0);

    // resumed in period 2, 200 ms later: new segments, offsets from the resume
    r.resume(now, 2);
    let resumed_at = now;
    for n in 60..70u64 {
        r.tick(now).unwrap();
        now += 20;
        r.event(now, &audio("a", 1_000 + n * 960, &voice[n as usize])).unwrap();
    }
    // a leaves: its track ends
    r.event(now, &TapEvent::Left(Arc::from("a"))).unwrap();
    let left = r.take_closed();
    assert_eq!(left.len(), 1);
    assert_eq!((left[0].period, left[0].seq, left[0].track_ended), (2, 2, true));
    assert_eq!(left[0].start_offset_ms, 20);
    assert_eq!(left[0].from_ms, resumed_at + 20);
    r.finish(now).unwrap();
    let last = r.take_closed();
    assert!(last.iter().all(|s| s.period == 2 && s.track_ended));
    assert!(last.iter().any(|s| s.track.kind == TrackKind::Mix && s.seq == 2));
}

#[test]
fn a_segment_past_its_cap_is_cut_and_the_total_stops_the_recording() {
    let spool = MemorySpool::default();
    let config = RecordConfig {
        mix: false,
        max_segment_bytes: 2_000,
        max_total_bytes: 9_000,
        ..RecordConfig::default()
    };
    let mut r = recorder(config, &spool);
    let packet = vec![0xF8u8; 100];
    let mut error = None;
    for n in 0..200u64 {
        if let Err(err) = r.event(n * 20, &audio("a", n * 960, &packet)) {
            error = Some(err);
            break;
        }
    }
    assert!(matches!(error, Some(RecordError::Limit)), "{error:?}");
    let closed = r.take_closed();
    assert!(closed.len() >= 3, "{}", closed.len());
    assert!(closed.windows(2).all(|w| w[1].seq == w[0].seq + 1));
    assert!(closed.iter().all(|s| s.bytes >= 2_000 && s.bytes < 2_400));
    // each segment's timing starts at its own first packet
    assert!(closed.iter().all(|s| pages(&spool.bytes(&s.file))[2].granule <= 48_000));
}

#[test]
fn a_lowered_total_limit_stops_the_recording_there_and_a_higher_one_changes_nothing() {
    let spool = MemorySpool::default();
    let config = RecordConfig {
        mix: false,
        max_segment_bytes: 2_000,
        max_total_bytes: 1_000_000,
        ..RecordConfig::default()
    };
    let mut r = recorder(config, &spool);
    let packet = vec![0xF8u8; 100];
    for n in 0..20u64 {
        r.event(n * 20, &audio("a", n * 960, &packet)).unwrap();
    }
    // the owner's drive filled meanwhile: a limit above what was recorded holds, and a higher one later is ignored
    r.lower_total_limit(4_000).unwrap();
    r.lower_total_limit(2_000_000).unwrap();
    let mut error = None;
    for n in 20..200u64 {
        if let Err(err) = r.event(n * 20, &audio("a", n * 960, &packet)) {
            error = Some(err);
            break;
        }
    }
    assert!(matches!(error, Some(RecordError::Limit)), "{error:?}");
    let total: u64 = r.take_closed().iter().map(|s| s.bytes).sum();
    assert!(total < 4_400, "{total}");
    // a limit below what is recorded already ends it at once
    let mut r = recorder(
        RecordConfig {
            mix: false,
            ..RecordConfig::default()
        },
        &spool,
    );
    for n in 0..20u64 {
        r.event(n * 20, &audio("b", n * 960, &packet)).unwrap();
    }
    assert!(matches!(r.lower_total_limit(10), Err(RecordError::Limit)));
}

#[test]
fn a_video_cut_waits_for_a_keyframe_and_layers_switch_at_keyframes() {
    let spool = MemorySpool::default();
    let config = RecordConfig {
        mix: false,
        max_segment_bytes: 3_000,
        ..RecordConfig::default()
    };
    let mut r = recorder(config, &spool);
    // a delta frame with no file yet waits for a keyframe, which the caller asks the browsers for
    r.event(0, &video("v", Some("l"), 7_030, false, vp8_delta(100)))
        .unwrap();
    assert!(r.take_keyframe_wanted());
    assert!(!r.take_keyframe_wanted(), "taken once");
    // the low layer first; the high one's keyframe takes over; its delta frames only
    r.event(0, &video("v", Some("l"), 10_000, true, vp8_keyframe(160, 90, 100)))
        .unwrap();
    r.event(33, &video("v", Some("l"), 12_970, false, vp8_delta(100)))
        .unwrap();
    assert!(!r.take_keyframe_wanted(), "the layer recorded needs none");
    r.event(40, &video("v", Some("h"), 900_000, false, vp8_delta(500)))
        .unwrap();
    assert!(r.take_keyframe_wanted(), "a better layer waits for its keyframe");
    r.event(66, &video("v", Some("h"), 902_970, true, vp8_keyframe(640, 360, 500)))
        .unwrap();
    r.event(70, &video("v", Some("l"), 15_940, false, vp8_delta(100)))
        .unwrap();
    for n in 2..12u64 {
        r.event(
            66 + n * 33,
            &video("v", Some("h"), 900_000 + n * 2_970, false, vp8_delta(300)),
        )
        .unwrap();
    }
    assert!(r.take_closed().is_empty(), "past the cap, but no keyframe yet");
    assert!(r.take_keyframe_wanted(), "the cut waits for a keyframe");
    r.event(500, &video("v", Some("h"), 940_000, true, vp8_keyframe(640, 360, 500)))
        .unwrap();
    let closed = r.take_closed();
    assert_eq!(closed.len(), 1);
    let data = spool.bytes(&closed[0].file);
    let top = elements(&data, 0, data.len());
    let blocks: Vec<Vec<u8>> = top[1]
        .children
        .iter()
        .filter(|e| e.id == webm::CLUSTER)
        .flat_map(|c| {
            c.children
                .iter()
                .filter(|b| b.id == webm::SIMPLE_BLOCK)
                .map(|b| b.body.clone())
        })
        .collect();
    // two low frames, then the high layer from its keyframe: 2 + 1 + 10, nothing of the other layer once switched
    assert_eq!(blocks.len(), 13);
    assert_eq!(blocks[2].len(), 4 + 500, "the high layer's keyframe");
    r.finish(600).unwrap();
    let last = r.take_closed();
    assert_eq!(last.len(), 1);
    assert_eq!((last[0].seq, last[0].track_ended), (2, true));
}

#[test]
fn mixed_only_writes_no_tracks_and_a_failing_spool_is_an_error() {
    let spool = MemorySpool::default();
    let config = RecordConfig {
        tracks: false,
        ..RecordConfig::default()
    };
    let mut r = recorder(config, &spool);
    let voice = opus_frames(20);
    for n in 0..20u64 {
        r.event(n * 20, &audio("a", n * 960, &voice[n as usize])).unwrap();
        r.tick(n * 20).unwrap();
    }
    r.finish(400).unwrap();
    let closed = r.take_closed();
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].track.kind, TrackKind::Mix);

    let failing = MemorySpool {
        fail: true,
        ..MemorySpool::default()
    };
    let mut r = recorder(RecordConfig::default(), &failing);
    let err = r.event(0, &audio("a", 0, &voice[0])).unwrap_err();
    assert!(matches!(err, RecordError::Io(_)), "{err}");
}
