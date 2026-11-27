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

//! A package's files served to the shell (plans/Maps-Offline.md §2.4, Serving to the page):
//! `/__moqui-app/maps/<packageId>/<path>` answered from the installed revision's directory. The apps' scheme answers
//! take a whole body (`Vec<u8>`), so a file is never read whole past [`WHOLE_MAX`]: one `Range` is answered as a 206
//! slice of at most [`SLICE_MAX`] bytes read from the file (a range asking more gets a shorter slice, which its
//! `Content-Range` says, as HTTP allows), and a GET of a larger file with no range is 413, to be asked by range
//! (PMTiles always is). The member's SHA-256 is its `ETag`; a path the manifest does not list is 404. No 304: Android's
//! WebView takes no 3xx from a scheme.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use super::Packages;

/// The path prefix on the shell's origin.
pub const PATH_PREFIX: &str = "/__moqui-app/maps/";
/// The largest file answered whole, with no range.
pub const WHOLE_MAX: u64 = 16 * 1024 * 1024;
/// The largest slice one range is answered with.
pub const SLICE_MAX: u64 = 16 * 1024 * 1024;

/// An answer of the package path: its status, headers and body.
#[derive(Debug)]
pub struct Served {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

impl Served {
    fn text(status: u16, text: &str) -> Self {
        Self {
            status,
            headers: vec![
                ("Content-Type", "text/plain; charset=utf-8".to_string()),
                ("Cache-Control", "no-store".to_string()),
            ],
            body: text.as_bytes().to_vec(),
        }
    }
}

/// The answer to `method` of `path` (no query, starting with [`PATH_PREFIX`]) for `account`'s packages, with the
/// request's `Range` and `If-Range` when it sent them.
pub fn answer(
    packages: &Packages,
    account: &str,
    method: &str,
    path: &str,
    range: Option<&str>,
    if_range: Option<&str>,
) -> Served {
    let head = method.eq_ignore_ascii_case("HEAD");
    if !head && !method.eq_ignore_ascii_case("GET") {
        return Served::text(405, "An offline map's files are read only");
    }
    let Some((package_id, member)) = path.strip_prefix(PATH_PREFIX).and_then(|rest| rest.split_once('/')) else {
        return Served::text(404, "Not found");
    };
    let Some((revision, dir)) = packages.installed_dir(account, package_id) else {
        return Served::text(404, "No such offline map on this device");
    };
    let Some(manifest) = packages.manifest(&dir) else {
        return Served::text(404, "No such offline map on this device");
    };
    // the manifest itself (its configuration is the offline screen's), else a member it lists
    let (tag, size) = if member == "manifest.json" {
        (format!("\"manifest-{package_id}-{revision}\""), None)
    } else {
        match manifest.members.get(member) {
            Some(listed) => (
                format!("\"sha256-{}\"", listed.sha256.to_ascii_lowercase()),
                Some(listed.size),
            ),
            None => return Served::text(404, "Not in this offline map"),
        }
    };
    // a listed member is a plain relative path (parse_manifest): it stays under dir
    let Ok(mut file) = File::open(dir.join(member)) else {
        return Served::text(404, "Not in this offline map");
    };
    let Ok(len) = file.metadata().map(|meta| meta.len()) else {
        return Served::text(500, "Not read");
    };
    if size.is_some_and(|size| size != len) {
        tracing::error!("A file of the offline map {package_id} is not the size its manifest says");
        return Served::text(500, "This offline map's file is damaged: download it again");
    }
    let mut headers = vec![
        ("Content-Type", content_type(member).to_string()),
        ("ETag", tag.clone()),
        ("Accept-Ranges", "bytes".to_string()),
        // never kept by the webview: the URL names no account, and the next account's shell asks the same URLs
        ("Cache-Control", "no-store".to_string()),
        ("X-Content-Type-Options", "nosniff".to_string()),
    ];
    // an If-Range of another tag takes the whole file, as HTTP has it
    let range = range.filter(|_| if_range.is_none_or(|sent| sent.trim() == tag));
    let slice = match range.map(|text| parse_range(text, len)) {
        None => None,
        Some(Some(slice)) => Some(slice),
        Some(None) => {
            let mut refused = Served::text(416, "Not a range of this file");
            refused.headers.push(("Content-Range", format!("bytes */{len}")));
            return refused;
        }
    };
    let (status, start, count) = match slice {
        Some((start, end)) => (206, start, (end - start + 1).min(SLICE_MAX)),
        None if len > WHOLE_MAX && !head => {
            let mut refused = Served::text(413, "Ask for this file by range");
            refused.headers.push(("Accept-Ranges", "bytes".to_string()));
            return refused;
        }
        None => (200, 0, len),
    };
    if status == 206 {
        headers.push(("Content-Range", format!("bytes {start}-{}/{len}", start + count - 1)));
    }
    headers.push(("Content-Length", count.to_string()));
    if head {
        return Served {
            status,
            headers,
            body: Vec::new(),
        };
    }
    let mut body = vec![0u8; usize::try_from(count).unwrap_or(0)];
    let read = file
        .seek(SeekFrom::Start(start))
        .and_then(|_| file.read_exact(&mut body));
    if let Err(err) = read {
        tracing::error!("A file of the offline map {package_id} was not read: {err}");
        return Served::text(500, "Not read");
    }
    Served { status, headers, body }
}

/// The first and last byte of one `bytes=` range of a file of `len` bytes (`a-b`, `a-`, `-n`), or none for a value
/// that is not one range of it (several ranges included).
fn parse_range(text: &str, len: u64) -> Option<(u64, u64)> {
    let spec = text.trim().strip_prefix("bytes=")?.trim();
    if spec.contains(',') || len == 0 {
        return None;
    }
    let (first, last) = spec.split_once('-')?;
    let (first, last) = (first.trim(), last.trim());
    if first.is_empty() {
        let suffix: u64 = last.parse().ok().filter(|n| *n > 0)?;
        return Some((len.saturating_sub(suffix), len - 1));
    }
    let start: u64 = first.parse().ok()?;
    if start >= len {
        return None;
    }
    let end = if last.is_empty() {
        len - 1
    } else {
        last.parse::<u64>().ok()?.min(len - 1)
    };
    (end >= start).then_some((start, end))
}

/// The content type of a member by its extension.
fn content_type(path: &str) -> &'static str {
    let extension = path
        .rsplit_once('.')
        .map_or("", |(_, extension)| extension)
        .to_ascii_lowercase();
    match extension.as_str() {
        "json" => "application/json",
        "geojson" => "application/geo+json",
        "pmtiles" => "application/vnd.pmtiles",
        "css" => "text/css; charset=utf-8",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "png" => "image/png",
        "webp" => "image/webp",
        "jpg" | "jpeg" => "image/jpeg",
        // never a document or script type: an SVG or HTML file would run as the shell's
        _ => "application/octet-stream",
    }
}
