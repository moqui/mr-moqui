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

//! Static files under a screen: the directory next to a screen file (`webroot.xml` and `webroot/`)
//! holds `css`, `js`, `libs`, `images` and `favicon.ico`. The path walk finds the file when a path name
//! is neither a subscreen nor a transition, as `ScreenUrlInfo.initUrl` does. This module decides whether
//! the file may be sent and writes the response: media type, `ETag` and a 304, `Cache-Control`, and no body
//! for HEAD. Compression is the host's job.

use std::path::Path;

use super::ScreenFacade;
use crate::context::{Error, ErrorKind, ExecutionContextFactory};
use crate::web::{self, WebRequest};

/// Extensions that are sources, not assets. A screen, template or script is never sent as a file; under a screen that
/// says `include-child-content` a template that has a renderer is rendered inside it (see [`is_template`]).
const BLOCKED_EXTENSIONS: [&str; 9] = [
    "xml",
    "ftl",
    "fancy",
    "fancy-locked",
    "groovy",
    "java",
    "kt",
    "properties",
    "sql",
];

/// A versioned or minified file: its name changes with its content, so it is kept a year (not in Java).
const LONG_CACHE: &str = "public, max-age=31536000, immutable";

/// Whether every name of a path under a screen is a plain file or directory name: not empty, not a
/// dot name or hidden file, and without a separator or control character, also when percent-decoded.
pub fn safe_segments(segments: &[String]) -> bool {
    !segments.is_empty()
        && segments.iter().all(|segment| {
            let decoded = web::percent_decode(segment);
            let plain = |name: &str| {
                !name.is_empty()
                    && !name.starts_with('.')
                    && !name.contains(['/', '\\', '\0', ':'])
                    && !name.chars().any(char::is_control)
            };
            plain(segment) && plain(&decoded)
        })
}

/// The location of the file under `screen_location` named by `segments`, when it is a file a request may get.
pub fn file_under(
    factory: &ExecutionContextFactory,
    facade: &ScreenFacade,
    screen_location: &str,
    segments: &[String],
) -> Option<String> {
    if !safe_segments(segments) {
        return None;
    }
    let base = screen_location.strip_suffix(".xml")?;
    let location = format!("{base}/{}", segments.join("/"));
    let resource = factory.resource().get(&location).ok()?;
    if !resource.exists() || !resource.is_file() || facade.is_screen(factory, &location) {
        return None;
    }
    // A template is never a file to send. It is the child content of a screen that says include-child-content, which
    // renders it inside the screen; under any other screen it is refused, as a source.
    if is_blocked(&location)
        && !(is_template(factory, &location)
            && facade
                .screen_def(factory, screen_location)
                .is_ok_and(|def| def.include_child_content))
    {
        return None;
    }
    // A link in the directory must not lead out of it.
    if let (Some(file), Some(dir)) = (
        resource.file_path(),
        factory
            .resource()
            .get(base)
            .ok()
            .and_then(|dir| dir.file_path().map(Path::to_path_buf)),
    ) {
        let inside = file
            .canonicalize()
            .ok()
            .zip(dir.canonicalize().ok())
            .is_some_and(|(file, dir)| file.starts_with(dir));
        if !inside {
            return None;
        }
    }
    Some(location)
}

/// Whether a file under a screen is a template a renderer of the conf renders (`.md`, `.html.fancy` and the like), as
/// Java's `getTemplateRendererByLocation`. FreeMarker has no renderer, so a `.ftl` stays refused.
pub fn is_template(factory: &ExecutionContextFactory, location: &str) -> bool {
    !location.ends_with(".ftl")
        && crate::script::template::TemplateRegistry::of(factory)
            .renderer_for(location)
            .is_some()
}

/// Whether a file is text by its media type, so a screen with `include-child-content` includes it; a binary file is sent.
pub fn is_text(location: &str) -> bool {
    let name = location.rsplit('/').next().unwrap_or(location);
    let kind = media_type(name);
    kind.starts_with("text/") || kind.contains("json") || kind.contains("xml") || kind.contains("javascript")
}

fn is_blocked(location: &str) -> bool {
    let name = location.rsplit('/').next().unwrap_or(location).to_ascii_lowercase();
    name.rsplit('.')
        .next()
        .is_some_and(|extension| name.contains('.') && BLOCKED_EXTENSIONS.contains(&extension))
}

/// The media type by file name. A text type names its character set.
pub fn media_type(name: &str) -> &'static str {
    let extension = name
        .rsplit_once('.')
        .map_or("", |(_, extension)| extension)
        .to_ascii_lowercase();
    match extension.as_str() {
        "css" => "text/css; charset=UTF-8",
        "js" | "mjs" => "text/javascript; charset=UTF-8",
        "json" | "map" => "application/json; charset=UTF-8",
        "html" | "htm" => "text/html; charset=UTF-8",
        "txt" | "md" | "csv" => "text/plain; charset=UTF-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "eot" => "application/vnd.ms-fontobject",
        "pdf" => "application/pdf",
        "wasm" => "application/wasm",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        _ => "application/octet-stream",
    }
}

/// A weak validator from the size and modification time of a file.
fn file_tag(modified: Option<i64>, size: Option<u64>, bytes: Option<&[u8]>) -> String {
    match (modified, size) {
        (Some(modified), Some(size)) => format!("W/\"{modified:x}-{size:x}\""),
        _ => {
            // FNV-1a over the content, for a file kept in the database.
            let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
            for byte in bytes.unwrap_or_default() {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x0100_0000_01b3);
            }
            format!("W/\"{hash:x}\"")
        }
    }
}

fn matches_tag(header: &str, tag: &str) -> bool {
    let strip = |text: &str| text.trim().trim_start_matches("W/").to_string();
    let ours = strip(tag);
    header
        .split(',')
        .any(|candidate| candidate.trim() == "*" || strip(candidate) == ours)
}

/// Bodies up to this size are sent from memory; a larger one streams, read a window at a time as the client takes it.
const INLINE_BODY: u64 = 256 * 1024;

/// The method and conditional headers of a request, which a file response answers.
#[derive(Clone, Debug, Default)]
pub struct Conditional {
    method: String,
    range: Option<String>,
    if_range: Option<String>,
    if_none_match: Option<String>,
}

impl Conditional {
    pub fn of(request: &WebRequest) -> Self {
        Self {
            method: request.method.to_ascii_uppercase(),
            range: request.header("range").map(str::to_string),
            if_range: request.header("if-range").map(str::to_string),
            if_none_match: request.header("if-none-match").map(str::to_string),
        }
    }
}

/// Write the response for the file at `location` into the thread's web slot.
///
/// # Errors
///
/// Returns `ErrorKind::NotFound` when the file is gone.
pub fn respond(
    factory: &ExecutionContextFactory,
    request: &WebRequest,
    location: &str,
    versioned: bool,
) -> Result<(), Error> {
    let Some(opened) = factory.resource().open(location)? else {
        return Err(Error::of_kind(ErrorKind::NotFound, format!("no such file: {location}")));
    };
    let name = location.rsplit('/').next().unwrap_or(location);
    let method = request.method.to_ascii_uppercase();
    if method != "GET" && method != "HEAD" {
        web::send_json_error(405, &format!("Method {method} is not supported for a file"));
        web::add_header("Allow", "GET, HEAD");
        return Ok(());
    }
    let media = media_type(name);
    let mut headers = vec![
        ("Content-Type".to_string(), media.to_string()),
        ("Vary".to_string(), "Accept-Encoding".to_string()),
    ];
    // The conf's headers of a text or a binary file under a screen, as `ScreenRenderImpl` sends them.
    let kind = if moqui_lib::resource::is_text_content_type(media) {
        "screen-resource-text"
    } else {
        "screen-resource-binary"
    };
    web::apply_conf_headers(Some(factory), kind, &mut headers, false);
    if versioned || name.contains(".min.") {
        headers.retain(|(key, _)| !key.eq_ignore_ascii_case("Cache-Control"));
        headers.push(("Cache-Control".to_string(), LONG_CACHE.to_string()));
    }
    respond_opened(&Conditional::of(request), headers, &opened)
}

/// The response for an opened file, with `headers` (its type and caching): an `ETag` (the content's own tag, else one
/// from its size and modification time), a 304 for a matching `If-None-Match`, one byte range for `Range` (unless
/// `If-Range` names another version), no body for `HEAD`. A body over [`INLINE_BODY`] streams.
pub(crate) fn respond_opened(
    request: &Conditional,
    mut headers: Vec<(String, String)>,
    opened: &crate::context::Opened,
) -> Result<(), Error> {
    let source = &opened.source;
    let total = source.len();
    headers.push(("Accept-Ranges".to_string(), "bytes".to_string()));
    let tag = match source.tag() {
        Some(tag) => Some(format!("\"{tag}\"")),
        None if opened.stat.last_modified.is_some() => Some(file_tag(opened.stat.last_modified, Some(total), None)),
        // no version to name, so a small file's content names it
        None if total <= INLINE_BODY => {
            let bytes = source.read_at(0, usize::try_from(total).unwrap_or(0))?;
            Some(file_tag(None, None, Some(&bytes)))
        }
        None => None,
    };
    if let Some(tag) = &tag {
        headers.push(("ETag".to_string(), tag.clone()));
        if request
            .if_none_match
            .as_deref()
            .is_some_and(|presented| matches_tag(presented, tag))
        {
            web::send_bytes_with_headers(304, headers, Vec::new());
            return Ok(());
        }
    }
    // One byte range, unless `If-Range` names another version of the file: then the whole file, as for a request
    // without a range.
    let range = request.range.as_deref().filter(|_| {
        request
            .if_range
            .as_deref()
            .is_none_or(|presented| tag.as_deref().is_some_and(|tag| presented.trim() == tag))
    });
    let (status, start, length) = match range.map(|range| byte_range(range, total)) {
        None | Some(ByteRange::Whole) => (200, 0, total),
        Some(ByteRange::Unsatisfiable) => {
            headers.push(("Content-Range".to_string(), format!("bytes */{total}")));
            web::send_bytes_with_headers(416, headers, Vec::new());
            return Ok(());
        }
        Some(ByteRange::Span(start, end)) => {
            headers.push(("Content-Range".to_string(), format!("bytes {start}-{end}/{total}")));
            (206, start, end - start + 1)
        }
    };
    headers.push(("Content-Length".to_string(), length.to_string()));
    if request.method == "HEAD" {
        web::send_bytes_with_headers(status, headers, Vec::new());
    } else if length <= INLINE_BODY {
        let body = source.read_at(start, usize::try_from(length).unwrap_or(0))?;
        web::send_bytes_with_headers(status, headers, body);
    } else {
        let stream = web::ResponseStream {
            source: std::sync::Arc::clone(source),
            offset: start,
            length,
        };
        web::send_stream(status, headers, stream);
    }
    Ok(())
}

/// What a `Range` header asks of a file of a known size.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ByteRange {
    /// No range this answers: not `bytes=`, more than one range, or not well formed. The whole file is sent.
    Whole,
    /// A range that starts past the end, or an empty suffix: 416.
    Unsatisfiable,
    /// The first and last byte, inclusive.
    Span(u64, u64),
}

/// One range of RFC 9110: `bytes=first-last`, `bytes=first-` or `bytes=-suffix`. A last byte past the end is the end.
pub(crate) fn byte_range(header: &str, total: u64) -> ByteRange {
    let Some((unit, spec)) = header.trim().split_once('=') else {
        return ByteRange::Whole;
    };
    if !unit.trim().eq_ignore_ascii_case("bytes") || spec.contains(',') {
        return ByteRange::Whole;
    }
    let Some((first, last)) = spec.trim().split_once('-') else {
        return ByteRange::Whole;
    };
    let number = |text: &str| -> Option<u64> {
        let text = text.trim();
        (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| text.parse().ok())
            .flatten()
    };
    match (number(first), number(last)) {
        (Some(first), last) if last.map_or(last_is_empty(spec), |last| first <= last) => {
            if first >= total {
                return ByteRange::Unsatisfiable;
            }
            ByteRange::Span(first, last.map_or(total - 1, |last| last.min(total - 1)))
        }
        (None, Some(suffix)) if first.trim().is_empty() => {
            if suffix == 0 || total == 0 {
                return ByteRange::Unsatisfiable;
            }
            ByteRange::Span(total.saturating_sub(suffix), total - 1)
        }
        _ => ByteRange::Whole,
    }
}

/// `first-` with nothing after the dash.
fn last_is_empty(spec: &str) -> bool {
    spec.split_once('-').is_some_and(|(_, last)| last.trim().is_empty())
}

/// Write a response made in memory with the validators of a file: `ETag`, a 304 when the request has it, and no body for HEAD. The
/// caller gives the tag, since it knows what the bytes are made from. Used for the server-static qvt template.
pub fn respond_bytes(
    request: &WebRequest,
    content_type: &str,
    cache_control: &str,
    extra_headers: &[(String, String)],
    etag: &str,
    bytes: Vec<u8>,
) {
    let method = request.method.to_ascii_uppercase();
    if method != "GET" && method != "HEAD" {
        web::send_json_error(405, &format!("Method {method} is not supported here"));
        web::add_header("Allow", "GET, HEAD");
        return;
    }
    let mut headers = vec![
        ("Content-Type".to_string(), content_type.to_string()),
        ("Cache-Control".to_string(), cache_control.to_string()),
        ("Vary".to_string(), "Accept-Encoding".to_string()),
        ("ETag".to_string(), etag.to_string()),
    ];
    headers.extend(extra_headers.iter().cloned());
    if request
        .header("if-none-match")
        .is_some_and(|presented| matches_tag(presented, etag))
    {
        web::send_bytes_with_headers(304, headers, Vec::new());
        return;
    }
    headers.push(("Content-Length".to_string(), bytes.len().to_string()));
    web::send_bytes_with_headers(200, headers, if method == "HEAD" { Vec::new() } else { bytes });
}

#[cfg(test)]
mod tests {
    use super::{is_blocked, matches_tag, media_type, safe_segments};

    fn names(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_string()).collect()
    }

    #[test]
    fn traversal_hidden_and_odd_names_are_refused() {
        assert!(safe_segments(&names(&["css", "moqui-html.css"])));
        for bad in [
            &["..", "etc", "passwd"][..],
            &["css", "..", "..", "x"],
            &["."],
            &[".hidden"],
            &["css", ".git", "config"],
            &["%2e%2e", "x"],
            &["css", "%2E%2e", "x"],
            &["a%2Fb"],
            &["a%5Cb"],
            &["a\\b"],
            &["a%00b"],
            &[""],
            &["c:", "x"],
            &["a%0Ab"],
        ] {
            assert!(!safe_segments(&names(bad)), "{bad:?}");
        }
        assert!(!safe_segments(&[]));
    }

    #[test]
    fn sources_are_blocked_and_assets_are_not() {
        for name in [
            "a/Login.xml",
            "a/x.ftl",
            "a/Y.html.fancy",
            "a/z.groovy",
            "a/CONF.XML",
            "a/b.properties",
        ] {
            assert!(is_blocked(name), "{name}");
        }
        for name in ["a/x.css", "a/x.min.js", "a/readme", "a/xml", "a/font.woff2"] {
            assert!(!is_blocked(name), "{name}");
        }
    }

    #[test]
    fn media_types_name_a_charset_for_text() {
        assert_eq!(media_type("a.css"), "text/css; charset=UTF-8");
        assert_eq!(media_type("a.min.js"), "text/javascript; charset=UTF-8");
        assert_eq!(media_type("a.WOFF2"), "font/woff2");
        assert_eq!(media_type("favicon.ico"), "image/x-icon");
        assert_eq!(media_type("noext"), "application/octet-stream");
    }

    #[test]
    fn a_range_header_picks_one_span_and_ignores_what_it_cannot_answer() {
        use super::{byte_range, ByteRange};
        assert_eq!(byte_range("bytes=0-99", 1000), ByteRange::Span(0, 99));
        assert_eq!(byte_range("bytes=900-", 1000), ByteRange::Span(900, 999));
        assert_eq!(byte_range("bytes=990-2000", 1000), ByteRange::Span(990, 999));
        assert_eq!(byte_range("bytes=-100", 1000), ByteRange::Span(900, 999));
        assert_eq!(byte_range("bytes=-5000", 1000), ByteRange::Span(0, 999));
        assert_eq!(byte_range("Bytes = 5-5", 1000), ByteRange::Span(5, 5));
        assert_eq!(byte_range("bytes=1000-", 1000), ByteRange::Unsatisfiable);
        assert_eq!(byte_range("bytes=-0", 1000), ByteRange::Unsatisfiable);
        assert_eq!(byte_range("bytes=0-", 0), ByteRange::Unsatisfiable);
        for ignored in [
            "bytes=5-1",
            "bytes=0-1,5-9",
            "items=0-1",
            "bytes=a-b",
            "bytes=-",
            "bytes=1",
            "0-1",
        ] {
            assert_eq!(byte_range(ignored, 1000), ByteRange::Whole, "{ignored}");
        }
    }

    #[test]
    fn if_none_match_compares_weakly_and_takes_a_list() {
        assert!(matches_tag("W/\"a-1\"", "W/\"a-1\""));
        assert!(matches_tag("\"a-1\"", "W/\"a-1\""));
        assert!(matches_tag("\"x\", W/\"a-1\"", "W/\"a-1\""));
        assert!(matches_tag("*", "W/\"a-1\""));
        assert!(!matches_tag("\"b\"", "W/\"a-1\""));
    }
}
