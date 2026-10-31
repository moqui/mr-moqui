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

//! Opus in Ogg (RFC 3533, RFC 7845), written in house: the call recorder's audio files (plans/Comms-Framework.md §6.4,
//! §10.1). The first page holds `OpusHead`, the second `OpusTags`, then the packets as they came, never re-encoded.
//! Each page's granule position is the 48 kHz sample count at the end of its last packet. A gap (packets that never
//! came) is a jump in the granule positions at a page boundary, so a player keeps the timing and no audio is made up.

use std::io::{self, Write};

/// The most lacing values in a page.
const MAX_SEGMENTS: usize = 255;
/// A page is written once it holds this much audio: one second.
const PAGE_SAMPLES: u64 = 48_000;

const HEADER_TYPE_CONTINUED: u8 = 0x01;
const HEADER_TYPE_FIRST: u8 = 0x02;
const HEADER_TYPE_LAST: u8 = 0x04;

/// The Ogg page checksum: CRC-32 with polynomial 0x04C11DB7, not reflected, starting at 0, over the page with its
/// checksum field zero.
pub fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (i, entry) in table.iter_mut().enumerate() {
            let mut r = (i as u32) << 24;
            for _ in 0..8 {
                r = if r & 0x8000_0000 != 0 {
                    (r << 1) ^ 0x04C1_1DB7
                } else {
                    r << 1
                };
            }
            *entry = r;
        }
        table
    });
    data.iter()
        .fold(0u32, |crc, b| (crc << 8) ^ table[(((crc >> 24) as u8) ^ b) as usize])
}

/// The 48 kHz samples an Opus packet holds, from its TOC byte (RFC 6716 §3.1); `None` for a malformed packet.
pub fn opus_packet_samples(packet: &[u8]) -> Option<u32> {
    let toc = *packet.first()?;
    let config = toc >> 3;
    let frame: u32 = match config {
        0..=11 => [480, 960, 1_920, 2_880][usize::from(config % 4)],
        12..=15 => [480, 960][usize::from(config % 2)],
        _ => [120, 240, 480, 960][usize::from(config % 4)],
    };
    let frames: u32 = match toc & 0x03 {
        0 => 1,
        1 | 2 => 2,
        _ => u32::from(*packet.get(1)? & 0x3F),
    };
    let samples = frame * frames;
    (frames > 0 && samples <= 5_760).then_some(samples)
}

/// Writes one Opus stream (mono, 48 kHz) as Ogg pages.
pub struct OggOpusWriter<W: Write> {
    out: W,
    serial: u32,
    sequence: u32,
    lacing: Vec<u8>,
    body: Vec<u8>,
    /// The granule position at the end of the last packet taken (in the page being filled or written).
    granule: u64,
    /// The granule position at the start of the page being filled.
    page_start: u64,
    pre_skip: u16,
    bytes: u64,
    packets: u64,
}

impl<W: Write> OggOpusWriter<W> {
    /// Starts a stream: the `OpusHead` and `OpusTags` pages. `pre_skip` is the encoder's delay in samples (0 for packets
    /// taken from the middle of a browser's stream).
    pub fn new(out: W, serial: u32, pre_skip: u16) -> io::Result<Self> {
        let mut writer = OggOpusWriter {
            out,
            serial,
            sequence: 0,
            lacing: Vec::new(),
            body: Vec::new(),
            granule: u64::from(pre_skip),
            page_start: 0,
            pre_skip,
            bytes: 0,
            packets: 0,
        };
        let mut head = Vec::with_capacity(19);
        head.extend_from_slice(b"OpusHead");
        head.push(1); // version
        head.push(1); // channels
        head.extend_from_slice(&pre_skip.to_le_bytes());
        head.extend_from_slice(&48_000u32.to_le_bytes()); // the input's rate
        head.extend_from_slice(&0i16.to_le_bytes()); // output gain
        head.push(0); // channel mapping family: mono or stereo
        writer.page(HEADER_TYPE_FIRST, 0, &[&head])?;
        let vendor = b"moqui";
        let mut tags = Vec::with_capacity(8 + 4 + vendor.len() + 4);
        tags.extend_from_slice(b"OpusTags");
        tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        tags.extend_from_slice(vendor);
        tags.extend_from_slice(&0u32.to_le_bytes()); // no comments
        writer.page(0, 0, &[&tags])?;
        Ok(writer)
    }

    /// Takes one packet that starts at granule position `start` (in samples from the stream's start, without the
    /// pre-skip) and lasts `samples`. A start past the end of the last packet is a gap: the page so far is written
    /// first, so the jump falls between pages. A start before it (a late or repeated packet) is dropped: false.
    pub fn write(&mut self, start: u64, samples: u32, packet: &[u8]) -> io::Result<bool> {
        let start = start + u64::from(self.pre_skip);
        if start < self.granule {
            return Ok(false);
        }
        if start > self.granule && !self.lacing.is_empty() {
            self.flush(0)?;
        }
        let needed = packet.len() / 255 + 1;
        if self.lacing.len() + needed > MAX_SEGMENTS {
            self.flush(0)?;
        }
        if self.lacing.is_empty() {
            self.page_start = start;
        }
        let mut left = packet.len();
        while left >= 255 {
            self.lacing.push(255);
            left -= 255;
        }
        self.lacing.push(left as u8);
        self.body.extend_from_slice(packet);
        self.granule = start + u64::from(samples);
        self.packets += 1;
        if self.granule - self.page_start >= PAGE_SAMPLES {
            self.flush(0)?;
        }
        Ok(true)
    }

    /// Writes the last page (end of stream) and gives the output back, with the bytes written in all.
    pub fn finish(mut self) -> io::Result<(W, u64)> {
        self.flush(HEADER_TYPE_LAST)?;
        self.out.flush()?;
        Ok((self.out, self.bytes))
    }

    /// The bytes written so far, and those waiting for their page.
    pub fn bytes(&self) -> u64 {
        self.bytes + self.body.len() as u64
    }

    /// The audio taken, in milliseconds: the last packet's end, gaps included.
    pub fn duration_ms(&self) -> u64 {
        (self.granule - u64::from(self.pre_skip)) / 48
    }

    /// The packets taken.
    pub fn packets(&self) -> u64 {
        self.packets
    }

    fn flush(&mut self, header_type: u8) -> io::Result<()> {
        if self.lacing.is_empty() && header_type != HEADER_TYPE_LAST {
            return Ok(());
        }
        let lacing = std::mem::take(&mut self.lacing);
        let body = std::mem::take(&mut self.body);
        self.page_laced(header_type, self.granule, &lacing, &body)
    }

    /// A page of whole packets.
    fn page(&mut self, header_type: u8, granule: u64, packets: &[&[u8]]) -> io::Result<()> {
        let mut lacing = Vec::new();
        let mut body = Vec::new();
        for packet in packets {
            let mut left = packet.len();
            while left >= 255 {
                lacing.push(255);
                left -= 255;
            }
            lacing.push(left as u8);
            body.extend_from_slice(packet);
        }
        self.page_laced(header_type, granule, &lacing, &body)
    }

    fn page_laced(&mut self, header_type: u8, granule: u64, lacing: &[u8], body: &[u8]) -> io::Result<()> {
        debug_assert!(
            lacing.len() <= MAX_SEGMENTS,
            "an Ogg page has at most 255 segments, got {}",
            lacing.len()
        );
        debug_assert!(
            header_type & HEADER_TYPE_CONTINUED == 0,
            "packets are never split across pages"
        );
        let mut page = Vec::with_capacity(27 + lacing.len() + body.len());
        page.extend_from_slice(b"OggS");
        page.push(0); // version
        page.push(header_type);
        page.extend_from_slice(&granule.to_le_bytes());
        page.extend_from_slice(&self.serial.to_le_bytes());
        page.extend_from_slice(&self.sequence.to_le_bytes());
        page.extend_from_slice(&[0; 4]); // the checksum, below
        page.push(lacing.len() as u8);
        page.extend_from_slice(lacing);
        page.extend_from_slice(body);
        let crc = crc32(&page);
        page[22..26].copy_from_slice(&crc.to_le_bytes());
        self.out.write_all(&page)?;
        self.sequence += 1;
        self.bytes += page.len() as u64;
        Ok(())
    }
}
