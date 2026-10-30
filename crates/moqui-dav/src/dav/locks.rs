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

//! WebDAV locks (class 2), which Finder and Windows need before they write: exclusive write locks on one file or
//! folder of a drive, held by [`moqui_core::resource::locks`] under the resource's location, so a lock taken by one user's
//! client holds another user's client (whose href names the drive's folder in their own home) and every other writer
//! of the drive (the Files screens, SFTP, the drive services) as well. The answers here are the WebDAV ones: 423, the
//! `lockdiscovery` property, LOCK, refresh and UNLOCK by token.

use moqui_core::resource::locks::{self as table, Lock};

use super::{href_element, xml, DavRequest, DavResponse};

const MAX_SECONDS: u64 = 3600;
const DEFAULT_SECONDS: u64 = 600;

/// 423 when another user holds a lock on `location`; with `tree` (a delete or move) on anything under it too.
pub(crate) fn check_write(location: &str, user_id: &str, tree: bool) -> Result<(), DavResponse> {
    table::check(location, Some(user_id), tree)
        .map_err(|_| DavResponse::condition(423, xml::DAV, "lock-token-submitted"))
}

/// The `lockdiscovery` property of the resource at `location`, answered at `href`.
pub(crate) fn discovery(location: &str, href: &str) -> String {
    table::holding(location)
        .map(|lock| active_lock(href, &lock))
        .unwrap_or_default()
}

pub(crate) const SUPPORTED: &str =
    "<D:lockentry><D:lockscope><D:exclusive/></D:lockscope><D:locktype><D:write/></D:locktype></D:lockentry>";

fn active_lock(href: &str, lock: &Lock) -> String {
    format!(
        "<D:activelock><D:locktype><D:write/></D:locktype><D:lockscope><D:exclusive/></D:lockscope><D:depth>{}</D:depth>{}<D:timeout>Second-{}</D:timeout><D:locktoken>{}</D:locktoken><D:lockroot>{}</D:lockroot></D:activelock>",
        if lock.depth_infinity { "infinity" } else { "0" },
        lock.owner,
        lock.seconds,
        href_element(&lock.token),
        href_element(href)
    )
}

fn timeout(request: &DavRequest) -> u64 {
    request
        .header("timeout")
        .and_then(|text| {
            text.split(',')
                .find_map(|part| part.trim().strip_prefix("Second-")?.parse::<u64>().ok())
        })
        .unwrap_or(DEFAULT_SECONDS)
        .clamp(1, MAX_SECONDS)
}

/// The tokens of an `If` header.
fn submitted_tokens(request: &DavRequest) -> Vec<String> {
    let Some(header) = request.header("if") else {
        return Vec::new();
    };
    header
        .split('<')
        .skip(1)
        .filter_map(|part| part.split('>').next())
        .filter(|token| token.starts_with("opaquelocktoken:") || token.starts_with("urn:uuid:"))
        .map(str::to_string)
        .collect()
}

/// LOCK of the resource at `location` (answered at `href`): a new lock, or a refresh of one whose token the `If`
/// header names. A lock of a location that is not there makes nothing (the client's PUT that follows makes the file).
pub(crate) fn lock(
    request: &DavRequest,
    location: &str,
    href: &str,
    user_id: &str,
    body: Option<&xml::Element>,
) -> DavResponse {
    let seconds = timeout(request);
    if body.is_none() {
        let tokens = submitted_tokens(request);
        let refreshed = table::update(location, |lock| {
            if !tokens.contains(&lock.token) || lock.user_id != user_id {
                return None;
            }
            lock.seconds = seconds;
            lock.expires = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
            Some(lock.clone())
        })
        .flatten();
        return match refreshed {
            Some(lock) => lock_answer(200, href, &lock),
            None => DavResponse::condition(412, xml::DAV, "lock-token-matches-request-uri"),
        };
    }
    if let Some(existing) = table::holding(location) {
        if existing.user_id != user_id {
            return DavResponse::condition(423, xml::DAV, "no-conflicting-lock");
        }
    }
    let owner = body
        .and_then(|body| body.child(xml::DAV, "owner"))
        .map(|owner| {
            let inner = owner
                .child(xml::DAV, "href")
                .map(|href| href_element(href.text.trim()))
                .unwrap_or_else(|| xml::escape(owner.text.trim()));
            format!("<D:owner>{inner}</D:owner>")
        })
        .unwrap_or_default();
    let lock = Lock {
        token: format!("opaquelocktoken:{}", random_uuid()),
        user_id: user_id.to_string(),
        owner,
        depth_infinity: !request.header("depth").is_some_and(|depth| depth.trim() == "0"),
        seconds,
        expires: std::time::Instant::now() + std::time::Duration::from_secs(seconds),
    };
    table::put(location, lock.clone());
    lock_answer(200, href, &lock)
}

fn lock_answer(status: u16, href: &str, lock: &Lock) -> DavResponse {
    DavResponse::xml(
        status,
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?><D:prop {}><D:lockdiscovery>{}</D:lockdiscovery></D:prop>"#,
            xml::ROOT_NAMESPACES,
            active_lock(href, lock)
        ),
    )
    .with_header("Lock-Token", format!("<{}>", lock.token))
}

/// UNLOCK of the resource at `location` by the `Lock-Token` header.
pub(crate) fn unlock(request: &DavRequest, location: &str, user_id: &str) -> DavResponse {
    let token = request
        .header("lock-token")
        .map(|text| text.trim().trim_start_matches('<').trim_end_matches('>').to_string())
        .unwrap_or_default();
    match table::get(location) {
        Some(lock) if lock.token == token && lock.user_id == user_id => {
            table::remove(location);
            DavResponse::status(204)
        }
        Some(_) => DavResponse::condition(403, xml::DAV, "lock-token-matches-request-uri"),
        None => DavResponse::condition(409, xml::DAV, "lock-token-matches-request-uri"),
    }
}

fn random_uuid() -> String {
    let mut bytes = [0_u8; 16];
    let _ = getrandom::fill(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}
