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

//! Overture's documents and files over HTTP (plans/archive/Maps-Overture.md §3.2), through `moqui_core::outbound` with its
//! internal-address guard, so a spec's `HttpFixtures` answer for them: a catalog document read whole (at most
//! [`MAX_DOCUMENT`]), and a GeoParquet file read by `Range` ([`HttpSource`]), its end first (one request that also says
//! its length), each later range pinned to that answer's `ETag` (or `Last-Modified`) so a file replaced while it is read
//! is refused rather than mixed, each request bounded in bytes and time and counted. Everything is https: the catalog's
//! documents on its own host (the caller checks), the files on whatever public host its items name (no internal address).

use std::cell::Cell;
use std::io::Read;

use moqui_core::outbound::{self, Method, OpenRequest};
use moqui_lib::error::Error;

use crate::parquet::RangeSource;

/// A catalog document's most bytes.
pub const MAX_DOCUMENT: u64 = 16 << 20;
/// One request's most bytes (a column chunk is a few MiB).
pub const MAX_RANGE: usize = 64 << 20;
/// The end of a file read first: most footers of a small file, and the length of any.
pub const TAIL: usize = 64 * 1024;
/// The client name sent with every request.
pub const USER_AGENT: &str = "MR-Moqui-Maps";
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

fn request(
    url: &str,
    range: Option<String>,
    pinned: Option<&(String, String)>,
) -> Result<outbound::OpenResponse, Error> {
    // a generic client name: Overture's CDN refuses a request with no User-Agent (403)
    let mut headers = vec![
        ("Accept-Encoding".to_string(), "identity".to_string()),
        ("User-Agent".to_string(), USER_AGENT.to_string()),
    ];
    if let Some(range) = range {
        headers.push(("Range".to_string(), range));
    }
    if let Some(pinned) = pinned {
        headers.push(pinned.clone());
    }
    outbound::open(OpenRequest {
        method: Method::Get,
        url: url.to_string(),
        headers,
        body: None,
        timeout: Some(TIMEOUT),
        follow_redirects: true,
        // a public catalog and its files: never an internal address
        deny_internal: true,
    })
    .map_err(|err| Error::new(format!("could not read {}: {}", outbound::redacted(url), err.message())))
}

fn read_body(response: outbound::OpenResponse, max: u64, url: &str) -> Result<Vec<u8>, Error> {
    if response.length.is_some_and(|length| length > max) {
        return Err(Error::new(format!(
            "{} answered {} bytes, more than {max}",
            outbound::redacted(url),
            response.length.unwrap_or_default()
        )));
    }
    let mut out = Vec::new();
    response
        .reader
        .take(max + 1)
        .read_to_end(&mut out)
        .map_err(|err| Error::new(format!("the answer of {} was cut off", outbound::redacted(url))).with_source(err))?;
    if out.len() as u64 > max {
        return Err(Error::new(format!(
            "{} answered more than {max} bytes",
            outbound::redacted(url)
        )));
    }
    Ok(out)
}

/// A catalog document at `url`, as JSON.
///
/// # Errors
///
/// The request fails, answers other than 200, more than [`MAX_DOCUMENT`] bytes, or not JSON.
pub fn fetch_json(url: &str) -> Result<serde_json::Value, Error> {
    if !url.starts_with("https://") {
        return Err(Error::new(format!("{} is not an https URL", outbound::redacted(url))));
    }
    let response = request(url, None, None)?;
    if response.status != 200 {
        return Err(Error::new(format!(
            "{} answered HTTP {}",
            outbound::redacted(url),
            response.status
        )));
    }
    let bytes = read_body(response, MAX_DOCUMENT, url)?;
    serde_json::from_slice(&bytes).map_err(|err| Error::new(format!("{} is not JSON: {err}", outbound::redacted(url))))
}

fn header(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty() && value.len() <= 255)
}

/// The total of a `Content-Range: bytes a-b/total` header.
fn content_range(headers: &[(String, String)]) -> Option<(u64, u64, u64)> {
    let value = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-range"))
        .map(|(_, value)| value.trim())?;
    let rest = value.strip_prefix("bytes ")?;
    let (range, total) = rest.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    Some((
        start.trim().parse().ok()?,
        end.trim().parse().ok()?,
        total.trim().parse().ok()?,
    ))
}

/// A GeoParquet file read by range.
pub struct HttpSource {
    url: String,
    size: u64,
    /// The file's last bytes, read when it was opened.
    tail: Vec<u8>,
    /// Bytes read so far, the tail's included.
    read: Cell<u64>,
    requests: Cell<u64>,
    /// The condition every later range is sent with, from the first answer: `If-Match` its strong `ETag`, else
    /// `If-Unmodified-Since` its `Last-Modified`, so a file replaced while it is read is refused (412), never mixed.
    pinned: Option<(String, String)>,
}

/// The condition that pins later ranges to the file the first answer was of.
fn pin_of(headers: &[(String, String)]) -> Option<(String, String)> {
    header(headers, "ETag")
        .filter(|tag| !tag.starts_with("W/"))
        .map(|tag| ("If-Match".to_string(), tag))
        .or_else(|| header(headers, "Last-Modified").map(|date| ("If-Unmodified-Since".to_string(), date)))
}

impl HttpSource {
    /// Open the file at `url` (https only): one request for its last [`TAIL`] bytes, which also gives its length.
    ///
    /// # Errors
    ///
    /// The request fails or is not answered with a range.
    pub fn open(url: &str) -> Result<Self, Error> {
        if !url.starts_with("https://") {
            return Err(Error::new(format!("{} is not an https URL", outbound::redacted(url))));
        }
        let response = request(url, Some(format!("bytes=-{TAIL}")), None)?;
        let (status, range) = (response.status, content_range(&response.headers));
        let pinned = pin_of(&response.headers);
        let tail = match (status, range) {
            (206, Some((start, end, total))) if end.checked_add(1) == Some(total) && end >= start => {
                let tail = read_body(response, TAIL as u64, url)?;
                if tail.len() as u64 != end - start + 1 {
                    return Err(Error::new(format!("{} sent a short range", outbound::redacted(url))));
                }
                return Ok(Self {
                    url: url.to_string(),
                    size: total,
                    read: Cell::new(tail.len() as u64),
                    tail,
                    requests: Cell::new(1),
                    pinned,
                });
            }
            // a whole small file (a server that ignores ranges): taken only within the tail's size
            (200, _) => read_body(response, TAIL as u64, url)?,
            _ => {
                return Err(Error::new(format!(
                    "{} answered HTTP {status} to a range request",
                    outbound::redacted(url)
                )))
            }
        };
        Ok(Self {
            url: url.to_string(),
            size: tail.len() as u64,
            read: Cell::new(tail.len() as u64),
            tail,
            requests: Cell::new(1),
            pinned,
        })
    }

    /// The bytes read so far.
    #[must_use]
    pub fn bytes_read(&self) -> u64 {
        self.read.get()
    }

    #[must_use]
    pub fn requests(&self) -> u64 {
        self.requests.get()
    }
}

impl RangeSource for HttpSource {
    fn size(&self) -> u64 {
        self.size
    }

    fn read(&self, offset: u64, length: usize) -> Result<Vec<u8>, Error> {
        let end = offset.saturating_add(length as u64).min(self.size);
        if offset >= end {
            return Ok(Vec::new());
        }
        let tail_start = self.size - self.tail.len() as u64;
        if offset >= tail_start {
            let from = usize::try_from(offset - tail_start).unwrap_or(usize::MAX);
            let to = usize::try_from(end - tail_start).unwrap_or(usize::MAX);
            return Ok(self.tail.get(from..to).unwrap_or_default().to_vec());
        }
        let wanted = usize::try_from(end - offset).unwrap_or(usize::MAX);
        if wanted > MAX_RANGE {
            return Err(Error::new(format!(
                "a read of {wanted} bytes of {}, more than {MAX_RANGE}",
                outbound::redacted(&self.url)
            )));
        }
        let response = request(
            &self.url,
            Some(format!("bytes={offset}-{}", end - 1)),
            self.pinned.as_ref(),
        )?;
        self.requests.set(self.requests.get() + 1);
        match (response.status, content_range(&response.headers)) {
            (206, Some((start, _, total))) if start == offset && total == self.size => {}
            // the condition failed, or the whole file came back: not the file the first answer was of
            (412 | 200, _) => {
                return Err(Error::new(format!(
                    "{} changed while it was read",
                    outbound::redacted(&self.url)
                )))
            }
            (status, _) => {
                return Err(Error::new(format!(
                    "{} answered HTTP {status} to the range {offset}-{}",
                    outbound::redacted(&self.url),
                    end - 1
                )))
            }
        }
        let bytes = read_body(response, wanted as u64, &self.url)?;
        self.read.set(self.read.get() + bytes.len() as u64);
        if bytes.len() != wanted {
            return Err(Error::new(format!(
                "{} sent {} bytes of the range {offset}-{}",
                outbound::redacted(&self.url),
                bytes.len(),
                end - 1
            )));
        }
        Ok(bytes)
    }
}
