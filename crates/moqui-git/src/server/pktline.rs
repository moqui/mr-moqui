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

//! pkt-line framing (git's `gitprotocol-common`): four hex digits of length, which count themselves, then the data;
//! `0000` is a flush, `0001` a delimiter and `0002` a response end (protocol v2). On a side-band the first data byte is
//! the band: 1 pack data, 2 progress, 3 a fatal error.

use std::io::{self, Read, Write};

/// The longest packet, its four length digits included.
pub(crate) const MAX_PACKET: usize = 65520;

/// The most data bytes in one `side-band-64k` packet: the length and the band byte take five.
pub(crate) const MAX_BAND_64K: usize = MAX_PACKET - 5;

/// The most data bytes in one `side-band` packet.
pub(crate) const MAX_BAND_SMALL: usize = 1000 - 5;

/// The band of pack data.
pub(crate) const BAND_DATA: u8 = 1;
/// The band of progress messages, which a client shows as `remote: ...`.
pub(crate) const BAND_PROGRESS: u8 = 2;
/// The band of a fatal error, after which the client stops.
pub(crate) const BAND_ERROR: u8 = 3;

/// One packet read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Packet {
    Data(Vec<u8>),
    Flush,
    Delim,
    ResponseEnd,
}

impl Packet {
    /// A data packet as text without its line feed; `None` for the special packets.
    pub(crate) fn text(&self) -> Option<String> {
        match self {
            Self::Data(data) => {
                let data = data.strip_suffix(b"\n").unwrap_or(data);
                Some(String::from_utf8_lossy(data).into_owned())
            }
            _ => None,
        }
    }
}

fn bad(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// Read one packet from `input`; `None` at the end of the input before a packet starts.
pub(crate) fn read(input: &mut dyn Read) -> io::Result<Option<Packet>> {
    let mut length = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        match input.read(&mut length[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the input ended inside a pkt-line",
                ))
            }
            Ok(n) => got += n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    let text = std::str::from_utf8(&length).map_err(|_| bad("a pkt-line length that is not hex"))?;
    let size =
        usize::from_str_radix(text, 16).map_err(|_| bad(format!("a pkt-line length that is not hex: {text}")))?;
    match size {
        0 => Ok(Some(Packet::Flush)),
        1 => Ok(Some(Packet::Delim)),
        2 => Ok(Some(Packet::ResponseEnd)),
        3 => Err(bad("a pkt-line of length 3")),
        size if size > MAX_PACKET => Err(bad(format!("a pkt-line of {size} bytes"))),
        size => {
            let mut data = vec![0u8; size - 4];
            input.read_exact(&mut data)?;
            Ok(Some(Packet::Data(data)))
        }
    }
}

/// Write a data packet of `data`, which must fit in one.
pub(crate) fn data(out: &mut dyn Write, data: &[u8]) -> io::Result<()> {
    if data.len() + 4 > MAX_PACKET {
        return Err(bad(format!("{} bytes do not fit in one pkt-line", data.len())));
    }
    write!(out, "{:04x}", data.len() + 4)?;
    out.write_all(data)
}

/// Write a line of text as a data packet, with a line feed.
pub(crate) fn text(out: &mut dyn Write, line: &str) -> io::Result<()> {
    let mut bytes = Vec::with_capacity(line.len() + 1);
    bytes.extend_from_slice(line.as_bytes());
    bytes.push(b'\n');
    data(out, &bytes)
}

pub(crate) fn flush(out: &mut dyn Write) -> io::Result<()> {
    out.write_all(b"0000")
}

pub(crate) fn delim(out: &mut dyn Write) -> io::Result<()> {
    out.write_all(b"0001")
}

/// Write `data` on side-band `band` in packets of at most `max` data bytes. Empty data writes nothing: git reads an
/// empty band packet as an error.
pub(crate) fn band(out: &mut dyn Write, band: u8, bytes: &[u8], max: usize) -> io::Result<()> {
    for chunk in bytes.chunks(max) {
        write!(out, "{:04x}", chunk.len() + 5)?;
        out.write_all(&[band])?;
        out.write_all(chunk)?;
    }
    Ok(())
}

/// A writer of pack data: on band 1 of a side-band in packets of up to `max` bytes, or raw when there is no side-band.
/// Bytes are gathered until a packet is full; [`Write::flush`] writes what is gathered and flushes the output.
pub(crate) struct PackWriter<'a> {
    out: &'a mut dyn Write,
    /// The side-band's packet size; `None` writes raw.
    max: Option<usize>,
    pending: Vec<u8>,
}

impl<'a> PackWriter<'a> {
    pub(crate) fn new(out: &'a mut dyn Write, max: Option<usize>) -> Self {
        Self {
            out,
            max,
            pending: Vec::with_capacity(max.unwrap_or(0)),
        }
    }

    /// A progress line (band 2), after the pack data gathered so far. Nothing without a side-band.
    pub(crate) fn progress(&mut self, message: &str) -> io::Result<()> {
        let Some(max) = self.max else {
            return Ok(());
        };
        self.send()?;
        band(self.out, BAND_PROGRESS, message.as_bytes(), max)?;
        self.out.flush()
    }

    fn send(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        if let Some(max) = self.max {
            band(self.out, BAND_DATA, &self.pending, max)?;
        } else {
            self.out.write_all(&self.pending)?;
        }
        self.pending.clear();
        Ok(())
    }
}

impl Write for PackWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let Some(max) = self.max else {
            return self.out.write(buf);
        };
        let room = max - self.pending.len();
        let taken = room.min(buf.len());
        self.pending.extend_from_slice(&buf[..taken]);
        if self.pending.len() == max {
            self.send()?;
        }
        Ok(taken)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.send()?;
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packets_round_trip() {
        let mut out = Vec::new();
        text(&mut out, "want abc").unwrap();
        flush(&mut out).unwrap();
        delim(&mut out).unwrap();
        data(&mut out, b"").unwrap();
        assert_eq!(out, b"000dwant abc\n000000010004");
        let mut input = &out[..];
        assert_eq!(read(&mut input).unwrap().unwrap().text().as_deref(), Some("want abc"));
        assert_eq!(read(&mut input).unwrap(), Some(Packet::Flush));
        assert_eq!(read(&mut input).unwrap(), Some(Packet::Delim));
        assert_eq!(read(&mut input).unwrap(), Some(Packet::Data(Vec::new())));
        assert_eq!(read(&mut input).unwrap(), None);
        let mut truncated = &b"00"[..];
        assert!(read(&mut truncated).is_err());
    }

    #[test]
    fn pack_data_is_split_into_band_packets() {
        let mut out = Vec::new();
        {
            let mut pack = PackWriter::new(&mut out, Some(4));
            pack.write_all(b"abcdefghij").unwrap();
            pack.flush().unwrap();
        }
        assert_eq!(out, b"0009\x01abcd0009\x01efgh0007\x01ij");
        let mut raw = Vec::new();
        PackWriter::new(&mut raw, None).write_all(b"PACK").unwrap();
        assert_eq!(raw, b"PACK");
    }
}
