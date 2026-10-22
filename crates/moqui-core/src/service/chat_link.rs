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

//! The link previews of chat messages, the Rust parts of `org.moqui.impl.ChatServices` (no Java counterpart; the cache
//! and the message's previews are the scripts `get#LinkPreview` and `fetch#LinkPreviews`):
//!
//! - `extract#LinkUrls` finds the links of a message to preview: http or https, at most 3, not our own `/qapps` links,
//!   not a URL with a user or password, not one on a port other than 80 or 443.
//! - `fetch#LinkPage` reads the head of one page and answers its title, description and site name: with the SSRF guard
//!   of [`moqui_lib::http::RestClient`] (no internal address, also after a redirect), 5 seconds, the first 512 KB, 3
//!   redirects to any host, `text/html` only. Text only: no image is read, so a reader's address never reaches a third
//!   party. The property `chat_link_preview_service` names a service to ask in place of the network (for tests): it gets
//!   `url` and answers `status`, `contentType` and `body`.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::states::RawKind;
use html5ever::tokenizer::{BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts};
use indexmap::IndexMap;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};
use sha2::{Digest, Sha256};

use crate::entity::EntityFacade;

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, extract_link_urls)?;
    rust_service!(map, fetch_link_page)?;
    Ok(())
}

/// The links of one message that are previewed.
const MAX_LINKS: usize = 3;
/// The part of a page read for its head.
const MAX_PAGE_BYTES: u64 = 512 * 1024;
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const MAX_TITLE: usize = 200;
const MAX_DESCRIPTION: usize = 300;
const MAX_SITE_NAME: usize = 80;
const MAX_URL: usize = 2048;

/// The text a link preview shows. Empty fields are left out of a card.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preview {
    pub title: String,
    pub description: String,
    pub site_name: String,
}

/// The URLs of a message to preview, in the order they appear, each once: from its HTML (links and text alike, as
/// `&amp;` and the like are decoded), http or https only, with no user or password, on the default port, not a `/qapps`
/// link (our own screens, which the message shows as record cards), at most [`MAX_LINKS`].
pub fn extract_urls(body: &str) -> Vec<String> {
    let text = body
        .replace("&amp;", "&")
        .replace("&#38;", "&")
        .replace("&#x26;", "&");
    let mut out: Vec<String> = Vec::new();
    let mut rest = text.as_str();
    while out.len() < MAX_LINKS {
        let lower = rest.to_ascii_lowercase();
        let Some(at) = [lower.find("http://"), lower.find("https://")].into_iter().flatten().min() else {
            break;
        };
        let candidate = &rest[at..];
        let end = candidate
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`' | '\u{a0}'))
            .unwrap_or(candidate.len());
        let url = candidate[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}']);
        rest = &candidate[end.max(1)..];
        if let Some(url) = previewable(url) {
            if !out.contains(&url) {
                out.push(url);
            }
        }
    }
    out
}

/// `url` as it is fetched, or `None` when it is not previewed: see [`extract_urls`].
fn previewable(url: &str) -> Option<String> {
    if url.len() > MAX_URL || url.chars().any(char::is_control) {
        return None;
    }
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(authority_end);
    if authority.is_empty() || authority.contains('@') || authority.contains('\\') {
        return None;
    }
    // the port: after the host, which for IPv6 is in brackets
    let host_end = if authority.starts_with('[') {
        authority.find(']').map(|end| end + 1)?
    } else {
        authority.find(':').unwrap_or(authority.len())
    };
    let host = &authority[..host_end];
    let port = authority[host_end..].strip_prefix(':');
    if host.is_empty() || host.trim_matches(['[', ']']).is_empty() {
        return None;
    }
    match port {
        None | Some("") => {}
        Some(port) => {
            let number: u16 = port.parse().ok()?;
            if !(number == 80 && scheme == "http" || number == 443 && scheme == "https") {
                return None;
            }
        }
    }
    let path_only = path.split(['?', '#']).next().unwrap_or_default();
    if path_only == "/qapps" || path_only.starts_with("/qapps/") {
        return None;
    }
    Some(format!("{scheme}://{}{path}", host.to_ascii_lowercase()))
}

/// The SHA-256 of `url`, hex: the key of its cached preview.
pub fn url_hash(url: &str) -> String {
    Sha256::digest(url.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The preview of a page from its first bytes: `og:` and `twitter:` meta values and `<title>`, the text decoded by the
/// charset of `content_type`, else of a `<meta>` in the first kilobyte, else UTF-8 (a byte order mark wins), whitespace
/// collapsed and each field cut to its length.
pub fn parse_preview(bytes: &[u8], content_type: Option<&str>) -> Preview {
    let label = content_type
        .and_then(charset_of)
        .or_else(|| meta_charset(&bytes[..bytes.len().min(1024)]));
    let encoding = label
        .and_then(|label| encoding_rs::Encoding::for_label(label.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    let (text, _, _) = encoding.decode(bytes);
    let sink = HeadSink::default();
    let tokenizer = Tokenizer::new(sink, TokenizerOpts::default());
    let input = BufferQueue::default();
    input.push_back(StrTendril::from_slice(&text));
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    let head = tokenizer.sink.head.into_inner();
    let first = |keys: &[&str]| {
        keys.iter()
            .filter_map(|key| head.meta.get(*key))
            .map(|value| collapse(value))
            .find(|value| !value.is_empty())
            .unwrap_or_default()
    };
    let mut title = first(&["og:title", "twitter:title"]);
    if title.is_empty() {
        title = collapse(&head.title);
    }
    Preview {
        title: cut(&title, MAX_TITLE),
        description: cut(&first(&["og:description", "twitter:description", "description"]), MAX_DESCRIPTION),
        site_name: cut(&first(&["og:site_name", "application-name"]), MAX_SITE_NAME),
    }
}

/// The charset parameter of a Content-Type.
fn charset_of(content_type: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|part| {
        let (name, value) = part.split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches(['"', '\'']).to_string())
    })
}

/// The charset a page names in a `<meta charset>` or `<meta http-equiv content="...; charset=...">`.
fn meta_charset(start: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(start).to_ascii_lowercase();
    let at = text.find("charset=")? + "charset=".len();
    let value: String = text[at..]
        .trim_start_matches(['"', '\''])
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
        .collect();
    (!value.is_empty()).then_some(value)
}

fn collapse(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .collect()
}

/// `text` cut to `max` characters, with an ellipsis when cut.
fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[derive(Default)]
struct Head {
    title: String,
    meta: IndexMap<String, String>,
    in_title: bool,
    seen_title: bool,
    in_body: bool,
}

#[derive(Default)]
struct HeadSink {
    head: std::cell::RefCell<Head>,
}

impl TokenSink for HeadSink {
    type Handle = ();

    fn process_token(&self, token: Token, _line_number: u64) -> TokenSinkResult<()> {
        let mut head = self.head.borrow_mut();
        match token {
            Token::TagToken(tag) => return head.tag(&tag),
            Token::CharacterTokens(text) if head.in_title => head.title.push_str(&text),
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

impl Head {
    fn tag(&mut self, tag: &Tag) -> TokenSinkResult<()> {
        let name: &str = &tag.name;
        let start = tag.kind == TagKind::StartTag;
        match (name, start) {
            ("title", true) if !self.seen_title && !self.in_body && !tag.self_closing => {
                self.in_title = true;
                return TokenSinkResult::RawData(RawKind::Rcdata);
            }
            ("title", false) if self.in_title => {
                self.in_title = false;
                self.seen_title = true;
            }
            // an svg's title, a script's text: not the page's
            ("script", true) if !tag.self_closing => return TokenSinkResult::RawData(RawKind::ScriptData),
            ("style" | "noscript" | "iframe", true) if !tag.self_closing => {
                return TokenSinkResult::RawData(RawKind::Rawtext)
            }
            ("body" | "svg", true) => self.in_body = true,
            ("meta", true) => {
                let key = [attribute(tag, "property"), attribute(tag, "name")]
                    .into_iter()
                    .map(|key| key.trim().to_ascii_lowercase())
                    .find(|key| !key.is_empty());
                if let Some(key) = key {
                    let value = attribute(tag, "content");
                    if !value.trim().is_empty() && !self.meta.contains_key(&key) {
                        self.meta.insert(key, value);
                    }
                }
            }
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

fn attribute(tag: &Tag, name: &str) -> String {
    tag.attrs
        .iter()
        .find(|attr| &*attr.name.local == name)
        .map(|attr| attr.value.to_string())
        .unwrap_or_default()
}

fn text(parameters: &BTreeMap<String, Value>, key: &str) -> Option<String> {
    parameters
        .get(key)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// `extract#LinkUrls`: the URLs of `body` to preview ([`extract_urls`]), as `urlList` [{url, urlHash}].
fn extract_link_urls(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let body = text(parameters, "body").unwrap_or_default();
    let list = extract_urls(&body)
        .into_iter()
        .map(|url| {
            let mut map = IndexMap::new();
            map.insert("urlHash".to_string(), Value::string(url_hash(&url)));
            map.insert("url".to_string(), Value::string(url));
            Value::Map(map)
        })
        .collect();
    Ok(BTreeMap::from([("urlList".to_string(), Value::List(list))]))
}

/// A page as read: its status, Content-Type and first bytes.
struct Page {
    status: u16,
    content_type: String,
    bytes: Vec<u8>,
}

/// `fetch#LinkPage`: the preview of the page at `url` (see the module notes). Out: `ok` (the page was read: an HTML page
/// with a 2xx status, or a page that is not HTML, which has no preview), `title`, `description` and `siteName`, and
/// `error` when it could not be read. It adds no error: a link that cannot be read has no card.
fn fetch_link_page(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    let Some(url) = text(parameters, "url").and_then(|url| previewable(&url)) else {
        out.insert("ok".to_string(), Value::Bool(false));
        out.insert("error".to_string(), Value::string("not a link to preview"));
        return Ok(out);
    };
    let page = match moqui_lib::env::get_property("chat_link_preview_service").filter(|name| !name.trim().is_empty()) {
        Some(service) => ask_service(service.trim(), &url),
        None => fetch(&url),
    };
    match page {
        Ok(page) if (200..300).contains(&page.status) => {
            out.insert("ok".to_string(), Value::Bool(true));
            let media = page.content_type.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
            if media == "text/html" || media == "application/xhtml+xml" {
                let preview = parse_preview(&page.bytes, Some(&page.content_type));
                out.insert("title".to_string(), Value::string(preview.title));
                out.insert("description".to_string(), Value::string(preview.description));
                out.insert("siteName".to_string(), Value::string(preview.site_name));
            }
        }
        Ok(page) => {
            out.insert("ok".to_string(), Value::Bool(false));
            out.insert("error".to_string(), Value::string(format!("HTTP {}", page.status)));
        }
        Err(err) => {
            tracing::info!("Chat link preview of {} not read: {err}", redacted(&url));
            out.insert("ok".to_string(), Value::Bool(false));
            out.insert("error".to_string(), Value::string(err));
        }
    }
    Ok(out)
}

fn redacted(url: &str) -> String {
    crate::outbound::redacted(url)
}

/// The page from the service the property `chat_link_preview_service` names, in place of the network.
fn ask_service(service: &str, url: &str) -> Result<Page, String> {
    let parameters = BTreeMap::from([("url".to_string(), Value::string(url))]);
    let out = crate::service::sync(service, &parameters).map_err(|err| err.to_string())?;
    let field = |key: &str| out.get(key).map(to_plain_string).unwrap_or_default();
    Ok(Page {
        status: field("status").parse().unwrap_or(200),
        content_type: field("contentType"),
        bytes: field("body").into_bytes(),
    })
}

#[cfg(feature = "http")]
fn fetch(url: &str) -> Result<Page, String> {
    crate::entity::fence::check_effect(crate::entity::fence::Effect::RemoteCall, "a link preview")
        .map_err(|err| err.message().to_string())?;
    static CLIENT: OnceLock<moqui_lib::http::RestClient> = OnceLock::new();
    let client = CLIENT.get_or_init(|| {
        moqui_lib::http::RestClient::new()
            .deny_internal_addresses(true)
            .timeout(TIMEOUT)
            .max_response_bytes(MAX_PAGE_BYTES)
            .truncate_body(true)
            .redirects(3, true)
    });
    let response = client
        .get(url)
        .header("accept", "text/html,application/xhtml+xml;q=0.9,*/*;q=0.1")
        .header("user-agent", "Mozilla/5.0 (compatible; MoquiLinkPreview/1.0)")
        .send(false)
        .map_err(|err| err.to_string())?;
    let content_type = response
        .response_headers()
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    Ok(Page {
        status: response.status(),
        content_type,
        bytes: response.bytes().to_vec(),
    })
}

#[cfg(not(feature = "http"))]
fn fetch(_url: &str) -> Result<Page, String> {
    let _ = (&TIMEOUT, &MAX_PAGE_BYTES, OnceLock::<()>::new());
    Err("this build has no HTTP client".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_links_to_preview_are_http_on_the_default_port_with_no_user_and_not_our_own() {
        let body = r#"<p>See <a href="https://example.com/a?x=1&amp;y=2">https://example.com/a?x=1&amp;y=2</a>, and
            http://Example.org/page. Also https://user:pw@example.net/secret https://example.com:8443/odd
            https://example.com:443/ok <a href="/qapps/Commons/Tasks?workEffortId=1">a task</a>
            https://erp.example.com/qapps/Comms/Chat?chatRoomId=1 ftp://example.com/file javascript:alert(1)
            (https://example.edu/in-parens) https://fourth.example/</p>"#;
        assert_eq!(
            extract_urls(body),
            vec![
                "https://example.com/a?x=1&y=2".to_string(),
                "http://example.org/page".to_string(),
                "https://example.com/ok".to_string(),
            ]
        );
        assert_eq!(extract_urls("no links here"), Vec::<String>::new());
        assert_eq!(
            extract_urls("<p>(https://example.edu/in-parens) http://[::1]:80/x</p>"),
            vec!["https://example.edu/in-parens".to_string(), "http://[::1]/x".to_string()]
        );
        assert!(previewable("https://example.com:99999/").is_none());
        assert!(previewable("https://:443/").is_none());
        assert_eq!(url_hash("https://example.com/").len(), 64);
    }

    #[test]
    fn a_preview_comes_from_the_open_graph_values_then_the_title() {
        let page = br#"<!doctype html><html><head>
            <title> The  page
               title </title>
            <meta property="og:title" content="Open &amp; Graph">
            <meta name="description" content="Plain description">
            <meta property="og:site_name" content="Example">
            <script>var t = "<title>not this</title>";</script>
            </head><body><svg><title>nor this</title></svg></body></html>"#;
        let preview = parse_preview(page, Some("text/html; charset=utf-8"));
        assert_eq!(preview.title, "Open & Graph");
        assert_eq!(preview.description, "Plain description");
        assert_eq!(preview.site_name, "Example");
        let plain = parse_preview(b"<html><head><title>Only a\n title</title></head><body><title>x</title></body>", None);
        assert_eq!(plain.title, "Only a title");
        assert_eq!(plain.description, "");
        // the charset of the header, else of a meta; the text cut to its length
        let latin = b"<html><head><meta charset=\"iso-8859-1\"><title>Caf\xe9</title></head></html>";
        assert_eq!(parse_preview(latin, None).title, "Café");
        assert_eq!(parse_preview(latin, Some("text/html; charset=windows-1252")).title, "Café");
        let long = format!("<title>{}</title>", "x".repeat(500));
        let cut_title = parse_preview(long.as_bytes(), None).title;
        assert_eq!(cut_title.chars().count(), MAX_TITLE);
        assert!(cut_title.ends_with('…'));
    }
}
