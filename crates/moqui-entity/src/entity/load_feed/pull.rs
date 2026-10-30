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

//! A UTF-8 pull buffer over a reader, which the XML and JSON scanners cut whole units from.

use std::io::BufRead;

use moqui_lib::error::Error;

/// Largest single XML element or JSON value the streaming loader holds at once.
const MAX_UNIT: usize = 64 * 1024 * 1024;

pub(super) struct Pull<R> {
    reader: R,
    buf: Vec<u8>,
    pub(super) pos: usize,
    eof: bool,
    /// When set, consumed bytes stay in `buf` so a non-data root can be parsed whole.
    pub(super) hold: bool,
}

impl<R: BufRead> Pull<R> {
    pub(super) fn new(reader: R) -> Self {
        Self {
            reader,
            buf: Vec::new(),
            pos: 0,
            eof: false,
            hold: false,
        }
    }

    pub(super) fn read_to_end(&mut self) -> Result<(), Error> {
        while !self.eof {
            self.fill()?;
        }
        Ok(())
    }

    pub(super) fn text_from(&self, mark: usize) -> Result<String, Error> {
        if mark > self.buf.len() {
            return Err(Error::new("xml root was dropped before it could be parsed"));
        }
        let bytes = &self.buf[mark..];
        std::str::from_utf8(bytes)
            .map(str::to_string)
            .map_err(|err| Error::new("data is not utf-8").with_source(err))
    }

    pub(super) fn release_hold(&mut self) {
        self.hold = false;
        if self.pos >= 8192 {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
    }

    pub(super) fn skip_bom(&mut self) -> Result<(), Error> {
        loop {
            let have = self.buf.len() - self.pos;
            if have >= 3 || self.eof {
                break;
            }
            let before = self.buf.len();
            self.fill()?;
            if self.buf.len() == before {
                break;
            }
        }
        if self.buf[self.pos..].starts_with(&[0xef, 0xbb, 0xbf]) {
            self.advance(3);
        }
        Ok(())
    }

    pub(super) fn starts_with(&mut self, prefix: &str) -> Result<bool, Error> {
        let needle = prefix.as_bytes();
        loop {
            let have = self.buf.len() - self.pos;
            if have >= needle.len() || self.eof {
                return Ok(self.buf[self.pos..].starts_with(needle));
            }
            let before = self.buf.len();
            self.fill()?;
            if self.buf.len() == before {
                return Ok(self.buf[self.pos..].starts_with(needle));
            }
        }
    }

    pub(super) fn consume(&mut self, n: usize) {
        self.advance(n);
    }

    pub(super) fn exhausted(&mut self) -> Result<bool, Error> {
        if self.pos < self.buf.len() {
            return Ok(false);
        }
        if !self.eof {
            self.fill()?;
        }
        Ok(self.pos >= self.buf.len())
    }

    pub(super) fn consume_match(
        &mut self,
        scan: impl Fn(&str, bool) -> Result<Option<usize>, Error>,
        label: &str,
    ) -> Result<(), Error> {
        self.drive(|text, eof| Ok(scan(text, eof)?.map(|n| (n, ()))), label)?;
        Ok(())
    }

    pub(super) fn take_text(
        &mut self,
        scan: impl Fn(&str, bool) -> Result<Option<usize>, Error>,
        label: &str,
    ) -> Result<String, Error> {
        self.drive(
            |text, eof| {
                let Some(n) = scan(text, eof)? else {
                    return Ok(None);
                };
                if n > text.len() || !text.is_char_boundary(n) {
                    return Err(Error::new("scan left the buffer"));
                }
                Ok(Some((n, text[..n].to_string())))
            },
            label,
        )
    }

    pub(super) fn take_with<T>(
        &mut self,
        scan: impl Fn(&str, bool) -> Result<Option<(usize, T)>, Error>,
        label: &str,
    ) -> Result<T, Error> {
        self.drive(scan, label)
    }

    pub(super) fn drive<T>(
        &mut self,
        mut scan: impl FnMut(&str, bool) -> Result<Option<(usize, T)>, Error>,
        label: &str,
    ) -> Result<T, Error> {
        loop {
            if self.pos >= self.buf.len() && !self.eof {
                self.fill()?;
            }
            let decision = {
                let (text, at_eof) = self.view()?;
                scan(text, at_eof)?
            };
            match decision {
                Some((n, value)) => {
                    let (text, _) = self.view()?;
                    if n > text.len() || !text.is_char_boundary(n) {
                        return Err(Error::new(format!("{label} scan left the buffer")));
                    }
                    self.advance(n);
                    return Ok(value);
                }
                None => {
                    let (_, at_eof) = self.view()?;
                    if at_eof {
                        return Err(Error::new(format!("{label} ended early")));
                    }
                    // Rescanning after every chunk is quadratic for one large element. Waiting for the
                    // pending text to double keeps the total scan linear.
                    let pending = self.buf.len() - self.pos;
                    if pending > MAX_UNIT {
                        return Err(Error::new(format!("{label} is larger than {MAX_UNIT} bytes")));
                    }
                    let target = pending.saturating_mul(2).max(pending + 8192);
                    while self.buf.len() - self.pos < target && !self.eof {
                        self.fill()?;
                    }
                }
            }
        }
    }

    pub(super) fn view(&self) -> Result<(&str, bool), Error> {
        let bytes = &self.buf[self.pos..];
        match std::str::from_utf8(bytes) {
            Ok(text) => Ok((text, self.eof)),
            Err(err) if err.error_len().is_none() && !self.eof => {
                let valid = err.valid_up_to();
                let text = std::str::from_utf8(&bytes[..valid]).map_err(|_| Error::new("data is not utf-8"))?;
                Ok((text, false))
            }
            Err(err) => Err(Error::new("data is not utf-8").with_source(err)),
        }
    }

    pub(super) fn fill(&mut self) -> Result<(), Error> {
        if self.eof {
            return Ok(());
        }
        let mut chunk = [0_u8; 8192];
        loop {
            match self.reader.read(&mut chunk) {
                Ok(0) => {
                    self.eof = true;
                    return Ok(());
                }
                Ok(n) => {
                    self.buf.extend_from_slice(&chunk[..n]);
                    return Ok(());
                }
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(Error::new("could not read data").with_source(err)),
            }
        }
    }

    pub(super) fn advance(&mut self, n: usize) {
        self.pos += n;
        if !self.hold && self.pos >= 8192 {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
    }
}
