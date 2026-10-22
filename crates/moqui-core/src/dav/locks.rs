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

//! WebDAV locks (class 2), which Finder and Windows need before they write: exclusive write locks on one path, held in
//! memory on this server for their timeout (at most an hour). A lock keeps other users from changing the path, not its
//! owner, who may write without the token as clients that lose it do; the drives' own versions cover a lost update
//! between the owner's devices. Locks are per server: in a cluster a client stays on one server for its session.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use super::{href_element, xml, DavRequest, DavResponse};

const MAX_SECONDS: u64 = 3600;
const DEFAULT_SECONDS: u64 = 600;

#[derive(Clone, Debug)]
struct Lock {
    token: String,
    user_id: String,
    owner: String,
    depth_infinity: bool,
    seconds: u64,
    expires: std::time::Instant,
}

fn table() -> &'static Mutex<HashMap<String, Lock>> {
    static TABLE: OnceLock<Mutex<HashMap<String, Lock>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn locked() -> std::sync::MutexGuard<'static, HashMap<String, Lock>> {
    table().lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The lock key of an href: decoded, no trailing slash.
fn key(href: &str) -> String {
    href.split('/')
        .map(super::decode_segment)
        .collect::<Vec<_>>()
        .join("/")
        .trim_end_matches('/')
        .to_string()
}

/// The lock on `href` or a folder above it with depth infinity, when it is not expired.
fn holding(href: &str) -> Option<Lock> {
    let key = key(href);
    let mut table = locked();
    let now = std::time::Instant::now();
    table.retain(|_, lock| lock.expires > now);
    if let Some(lock) = table.get(&key) {
        return Some(lock.clone());
    }
    table
        .iter()
        .find(|(path, lock)| lock.depth_infinity && key.starts_with(&format!("{path}/")))
        .map(|(_, lock)| lock.clone())
}

/// 423 when another user holds a lock on `href`.
pub(crate) fn check_write(href: &str, user_id: &str) -> Result<(), DavResponse> {
    match holding(href) {
        Some(lock) if lock.user_id != user_id => Err(DavResponse::condition(423, xml::DAV, "lock-token-submitted")),
        _ => Ok(()),
    }
}

/// The `lockdiscovery` property of `href`.
pub(crate) fn discovery(href: &str) -> String {
    holding(href).map(|lock| active_lock(href, &lock)).unwrap_or_default()
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

/// LOCK: a new lock, or a refresh of one whose token the `If` header names. `exists` tells whether the path is there
/// (a lock of a path that is not makes nothing; the client's PUT that follows makes the file).
pub(crate) fn lock(request: &DavRequest, href: &str, user_id: &str, body: Option<&xml::Element>) -> DavResponse {
    let key = key(href);
    let seconds = timeout(request);
    if body.is_none() {
        let tokens = submitted_tokens(request);
        let mut table = locked();
        let Some(lock) = table
            .get_mut(&key)
            .filter(|lock| tokens.contains(&lock.token) && lock.user_id == user_id)
        else {
            return DavResponse::condition(412, xml::DAV, "lock-token-matches-request-uri");
        };
        lock.seconds = seconds;
        lock.expires = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
        let lock = lock.clone();
        drop(table);
        return lock_answer(200, href, &lock);
    }
    if let Some(existing) = holding(href) {
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
    locked().insert(key, lock.clone());
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

/// UNLOCK by the `Lock-Token` header.
pub(crate) fn unlock(request: &DavRequest, href: &str, user_id: &str) -> DavResponse {
    let token = request
        .header("lock-token")
        .map(|text| text.trim().trim_start_matches('<').trim_end_matches('>').to_string())
        .unwrap_or_default();
    let key = key(href);
    let mut table = locked();
    match table.get(&key) {
        Some(lock) if lock.token == token && lock.user_id == user_id => {
            table.remove(&key);
            DavResponse::status(204)
        }
        Some(_) => DavResponse::condition(403, xml::DAV, "lock-token-matches-request-uri"),
        None => DavResponse::condition(409, xml::DAV, "lock-token-matches-request-uri"),
    }
}

/// A moved or deleted path takes its lock with it.
pub(crate) fn release(href: &str) {
    let key = key(href);
    locked().retain(|path, _| path != &key && !path.starts_with(&format!("{key}/")));
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
