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

//! SIP Digest authentication (RFC 3261 §22, RFC 7616, RFC 8760; plans/Comms-Framework.md §5.2 "auth").
//!
//! **Answering a challenge.** A trunk's credentials are for its realm and the algorithms its profile names
//! ([`Credentials`]). [`answer`] picks the strongest challenge the credentials allow (SHA-512-256, SHA-256, then MD5),
//! and refuses one for another realm or only with an algorithm the credentials do not allow, so a server cannot have a
//! response computed for a realm it chose. The answer uses `qop=auth` when offered, with a fresh client nonce.
//!
//! **Challenging a request.** [`NonceBook`] issues nonces that carry their time, random bytes and a MAC under a secret
//! of this process, so they need no table to check, and two challenges in the same second differ; one older than its lifetime is answered `stale` (a client may retry with
//! a new one). Each nonce's last nonce count is kept for its lifetime, and a count that does not go up is a replay.
//! A request without `qop` may use a nonce once. Responses are compared in constant time.

use std::collections::HashMap;
use std::fmt::Write as _;

use base64::Engine as _;
use hmac::{KeyInit, Mac};
use md5::Md5;
use sha2::{Digest, Sha256, Sha512_256};

/// A digest algorithm. The `-sess` forms are not supported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Algorithm {
    Md5,
    Sha256,
    Sha512_256,
}

impl Algorithm {
    pub fn name(self) -> &'static str {
        match self {
            Algorithm::Md5 => "MD5",
            Algorithm::Sha256 => "SHA-256",
            Algorithm::Sha512_256 => "SHA-512-256",
        }
    }

    /// The algorithm of a challenge or a conf value; MD5 when a challenge names none (RFC 2617).
    pub fn parse(name: &str) -> Option<Algorithm> {
        match name.trim().to_ascii_uppercase().as_str() {
            "" | "MD5" => Some(Algorithm::Md5),
            "SHA-256" | "SHA256" => Some(Algorithm::Sha256),
            "SHA-512-256" | "SHA512-256" => Some(Algorithm::Sha512_256),
            _ => None,
        }
    }

    /// Stronger is larger.
    fn strength(self) -> u8 {
        match self {
            Algorithm::Md5 => 1,
            Algorithm::Sha256 => 2,
            Algorithm::Sha512_256 => 3,
        }
    }

    pub fn hash(self, text: &str) -> String {
        fn hex(bytes: &[u8]) -> String {
            let mut out = String::with_capacity(bytes.len() * 2);
            for b in bytes {
                let _ = write!(out, "{b:02x}");
            }
            out
        }
        match self {
            Algorithm::Md5 => hex(&Md5::digest(text.as_bytes())),
            Algorithm::Sha256 => hex(&Sha256::digest(text.as_bytes())),
            Algorithm::Sha512_256 => hex(&Sha512_256::digest(text.as_bytes())),
        }
    }
}

/// A trunk's credentials and what they may be used for.
#[derive(Debug, Clone, Default)]
pub struct Credentials {
    /// The user part of the address (From, To, Contact).
    pub username: String,
    /// The digest username, when the provider's differs from the address's.
    pub auth_username: Option<String>,
    pub password: String,
    /// The realm these credentials are for; `None` answers the realm of the trunk's own challenge.
    pub realm: Option<String>,
    /// The algorithms these credentials may answer; empty means SHA-256 and MD5 (the provider profiles' default).
    pub algorithms: Vec<Algorithm>,
}

impl Credentials {
    fn digest_username(&self) -> &str {
        match self.auth_username.as_deref() {
            Some(name) if !name.trim().is_empty() => name,
            _ => &self.username,
        }
    }

    fn allows(&self, algorithm: Algorithm) -> bool {
        if self.algorithms.is_empty() {
            matches!(algorithm, Algorithm::Md5 | Algorithm::Sha256)
        } else {
            self.algorithms.contains(&algorithm)
        }
    }
}

/// A `WWW-Authenticate` or `Proxy-Authenticate` challenge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    pub realm: String,
    pub nonce: String,
    pub opaque: Option<String>,
    /// `None` when the challenge names an algorithm this does not know.
    pub algorithm: Option<Algorithm>,
    pub qop_auth: bool,
    pub stale: bool,
}

impl Challenge {
    /// Parses a challenge's header value (`Digest realm="…", nonce="…", …`). `None` for another scheme or no nonce.
    pub fn parse(value: &str) -> Option<Challenge> {
        let rest = strip_scheme(value)?;
        let params = parse_params(rest);
        let get = |k: &str| params.get(k).cloned();
        let nonce = get("nonce")?;
        let algorithm = match get("algorithm") {
            Some(a) => Algorithm::parse(&a),
            None => Some(Algorithm::Md5),
        };
        Some(Challenge {
            realm: get("realm").unwrap_or_default(),
            nonce,
            opaque: get("opaque"),
            algorithm,
            qop_auth: get("qop")
                .map(|q| q.split(',').any(|v| v.trim().eq_ignore_ascii_case("auth")))
                .unwrap_or(false),
            stale: get("stale").is_some_and(|s| s.eq_ignore_ascii_case("true")),
        })
    }
}

/// Why a challenge was not answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// No challenge was for the credentials' realm.
    WrongRealm(String),
    /// Every challenge for the realm asked for an algorithm the credentials do not allow.
    Algorithm(String),
    /// Nothing parsed as a Digest challenge.
    NoChallenge,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::WrongRealm(realm) => write!(f, "the challenge is for realm {realm}, not the trunk's"),
            Refusal::Algorithm(name) => write!(
                f,
                "the challenge asks for {name}, which the trunk's profile does not allow"
            ),
            Refusal::NoChallenge => write!(f, "no digest challenge"),
        }
    }
}

/// The best challenge of several (a server may offer one per algorithm, RFC 8760 §2.4) for these credentials.
pub fn choose<'a>(challenges: &'a [Challenge], credentials: &Credentials) -> Result<&'a Challenge, Refusal> {
    if challenges.is_empty() {
        return Err(Refusal::NoChallenge);
    }
    let in_realm: Vec<&Challenge> = challenges
        .iter()
        .filter(|c| {
            credentials
                .realm
                .as_deref()
                .is_none_or(|r| r.eq_ignore_ascii_case(&c.realm))
        })
        .collect();
    if in_realm.is_empty() {
        return Err(Refusal::WrongRealm(challenges[0].realm.clone()));
    }
    in_realm
        .iter()
        .filter(|c| c.algorithm.is_some_and(|a| credentials.allows(a)))
        .max_by_key(|c| c.algorithm.map(Algorithm::strength).unwrap_or(0))
        .copied()
        .ok_or_else(|| {
            Refusal::Algorithm(
                in_realm[0]
                    .algorithm
                    .map(|a| a.name().to_string())
                    .unwrap_or_else(|| "an unknown algorithm".to_string()),
            )
        })
}

/// The `Authorization` (or `Proxy-Authorization`) value answering `challenge` for a request of `method` to `uri`.
/// `cnonce` is the client nonce for `qop=auth` (random; a parameter so tests can fix it).
pub fn answer(
    credentials: &Credentials,
    challenge: &Challenge,
    method: &str,
    uri: &str,
    cnonce: &str,
) -> Result<String, Refusal> {
    let algorithm = choose(std::slice::from_ref(challenge), credentials)?
        .algorithm
        .ok_or(Refusal::NoChallenge)?;
    let username = credentials.digest_username();
    let ha1 = algorithm.hash(&format!("{username}:{}:{}", challenge.realm, credentials.password));
    let ha2 = algorithm.hash(&format!("{method}:{uri}"));
    let mut value = format!(
        "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{uri}\", algorithm={}",
        quote(username),
        quote(&challenge.realm),
        quote(&challenge.nonce),
        algorithm.name()
    );
    let response = if challenge.qop_auth {
        let nc = "00000001";
        let _ = write!(value, ", qop=auth, nc={nc}, cnonce=\"{}\"", quote(cnonce));
        algorithm.hash(&format!("{ha1}:{}:{nc}:{cnonce}:auth:{ha2}", challenge.nonce))
    } else {
        algorithm.hash(&format!("{ha1}:{}:{ha2}", challenge.nonce))
    };
    let _ = write!(value, ", response=\"{response}\"");
    if let Some(opaque) = &challenge.opaque {
        let _ = write!(value, ", opaque=\"{}\"", quote(opaque));
    }
    Ok(value)
}

/// A fresh random client nonce.
pub fn cnonce() -> String {
    let mut bytes = [0u8; 12];
    let _ = getrandom::fill(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The fields of an `Authorization` value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Authorization {
    pub username: String,
    pub realm: String,
    pub nonce: String,
    pub uri: String,
    pub response: String,
    pub algorithm: Option<Algorithm>,
    pub qop: Option<String>,
    pub nc: Option<String>,
    pub cnonce: Option<String>,
}

impl Authorization {
    pub fn parse(value: &str) -> Option<Authorization> {
        let params = parse_params(strip_scheme(value)?);
        let get = |k: &str| params.get(k).cloned();
        Some(Authorization {
            username: get("username")?,
            realm: get("realm").unwrap_or_default(),
            nonce: get("nonce")?,
            uri: get("uri")?,
            response: get("response")?.to_ascii_lowercase(),
            algorithm: match get("algorithm") {
                Some(a) => Some(Algorithm::parse(&a)?),
                None => Some(Algorithm::Md5),
            },
            qop: get("qop"),
            nc: get("nc"),
            cnonce: get("cnonce"),
        })
    }
}

/// What checking a request's credentials found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Valid,
    /// Right credentials on an expired nonce: challenge again with `stale=true`.
    Stale,
    /// A nonce count that did not go up, or a nonce without `qop` used twice.
    Replay,
    /// A nonce this process did not issue, for another realm, or a wrong response: a failed attempt.
    Invalid,
    /// The answer could not be checked now: whoever holds the verifiers did not answer (an edge node's local node away,
    /// plans/Edge-Node.md §5.4 "Voice (SIP)"). Not a failed attempt; the request is answered 503.
    Unavailable,
}

/// The server side: nonces to challenge with, and the checks of an answer.
pub struct NonceBook {
    key: [u8; 32],
    realm: String,
    lifetime_secs: u64,
    /// Each nonce's highest nonce count seen (0 for one used without qop), until it expires.
    seen: HashMap<String, (u64, u32)>,
}

/// The most nonces whose counts are kept; past this the oldest go (a nonce they drop can be replayed only within
/// its lifetime, and only after this many newer ones were used).
const MAX_SEEN: usize = 100_000;
/// The random bytes in a nonce.
const NONCE_SALT: usize = 8;

impl NonceBook {
    pub fn new(realm: &str, lifetime_secs: u64) -> Self {
        let mut key = [0u8; 32];
        let _ = getrandom::fill(&mut key);
        NonceBook {
            key,
            realm: realm.to_string(),
            lifetime_secs: lifetime_secs.max(1),
            seen: HashMap::new(),
        }
    }

    pub fn realm(&self) -> &str {
        &self.realm
    }

    fn mac(&self, time: u64, salt: &[u8]) -> [u8; 16] {
        let mut mac = hmac::Hmac::<Sha256>::new_from_slice(&self.key).expect("HMAC takes a key of any length");
        mac.update(&time.to_be_bytes());
        mac.update(salt);
        mac.update(self.realm.as_bytes());
        let full = mac.finalize().into_bytes();
        let mut out = [0u8; 16];
        out.copy_from_slice(&full[..16]);
        out
    }

    /// A nonce issued at `now_secs`: the time, 8 random bytes (each challenge its own nonce, so the nonce counts of two
    /// clients challenged in the same second are not taken for a replay) and their MAC.
    pub fn nonce(&self, now_secs: u64) -> String {
        let mut salt = [0u8; NONCE_SALT];
        let _ = getrandom::fill(&mut salt);
        let mut bytes = Vec::with_capacity(8 + NONCE_SALT + 16);
        bytes.extend_from_slice(&now_secs.to_be_bytes());
        bytes.extend_from_slice(&salt);
        bytes.extend_from_slice(&self.mac(now_secs, &salt));
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    }

    /// The challenge header value, one per algorithm offered (strongest first, RFC 8760 §2.4).
    pub fn challenges(&self, now_secs: u64, algorithms: &[Algorithm], stale: bool) -> Vec<String> {
        let nonce = self.nonce(now_secs);
        algorithms
            .iter()
            .map(|a| {
                format!(
                    "Digest realm=\"{}\", nonce=\"{nonce}\", algorithm={}, qop=\"auth\"{}",
                    quote(&self.realm),
                    a.name(),
                    if stale { ", stale=true" } else { "" }
                )
            })
            .collect()
    }

    /// Checks an answer for `method`, with the password of its username (`None` when there is no such user, which is
    /// checked the same way so the time does not tell).
    pub fn verify(&mut self, auth: &Authorization, method: &str, password: Option<&str>, now_secs: u64) -> Verdict {
        let password = password.unwrap_or("\u{0}no user");
        let verifier = auth
            .algorithm
            .map(|algorithm| ha1(algorithm, &auth.username, &auth.realm, password));
        self.verify_ha1(auth, method, verifier.as_deref(), now_secs)
    }

    /// Checks an answer for `method` against a stored verifier: the HA1 of its username, this book's realm and the
    /// answer's algorithm ([`ha1`]), as the registrar keeps a phone's credentials (§5.2 "Verifiers"). `None` when there
    /// is no such user or no verifier for that algorithm, which is checked the same way so the time does not tell.
    pub fn verify_ha1(&mut self, auth: &Authorization, method: &str, verifier: Option<&str>, now_secs: u64) -> Verdict {
        let matched = ResponseCheck::of(auth, method).is_some_and(|check| check.matches(verifier));
        self.verify_checked(auth, matched, now_secs)
    }

    /// [`NonceBook::verify_ha1`] with the response already checked against the verifier elsewhere (`matched`, from
    /// [`ResponseCheck::matches`]): by the node that holds the verifiers, for a registrar that holds none (an edge
    /// node's, plans/Edge-Node.md §5.4 "Voice (SIP)"). The nonce, its realm, its lifetime and its counts are checked
    /// here, as they are this book's.
    pub fn verify_checked(&mut self, auth: &Authorization, matched: bool, now_secs: u64) -> Verdict {
        let Some(issued) = self.issued_at(&auth.nonce) else {
            return Verdict::Invalid;
        };
        if !auth.realm.eq_ignore_ascii_case(&self.realm) || auth.algorithm.is_none() {
            return Verdict::Invalid;
        }
        let Some(count) = nonce_count(auth) else {
            return Verdict::Invalid;
        };
        if !matched {
            return Verdict::Invalid;
        }
        if now_secs.saturating_sub(issued) > self.lifetime_secs {
            return Verdict::Stale;
        }
        self.forget_expired(now_secs);
        match self.seen.get(&auth.nonce) {
            Some(&(_, last)) if count == 0 || count <= last => return Verdict::Replay,
            _ => {}
        }
        if self.seen.len() >= MAX_SEEN {
            if let Some(oldest) = self.seen.iter().min_by_key(|(_, (t, _))| *t).map(|(k, _)| k.clone()) {
                self.seen.remove(&oldest);
            }
        }
        self.seen.insert(auth.nonce.clone(), (issued, count));
        Verdict::Valid
    }

    /// Whether an answer is to a challenge of this book: a nonce it issued, for its realm. One that is not fails
    /// [`NonceBook::verify_ha1`] whatever the verifier, so a caller may skip looking the username up for it.
    pub fn answers_ours(&self, auth: &Authorization) -> bool {
        self.issued_at(&auth.nonce).is_some() && auth.realm.eq_ignore_ascii_case(&self.realm)
    }

    fn issued_at(&self, nonce: &str) -> Option<u64> {
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(nonce).ok()?;
        let (time, rest) = bytes.split_at_checked(8)?;
        let (salt, mac) = rest.split_at_checked(NONCE_SALT)?;
        let time = u64::from_be_bytes(time.try_into().ok()?);
        constant_eq(&self.mac(time, salt), mac).then_some(time)
    }

    fn forget_expired(&mut self, now_secs: u64) {
        let lifetime = self.lifetime_secs;
        self.seen
            .retain(|_, (issued, _)| now_secs.saturating_sub(*issued) <= lifetime);
    }
}

/// An answer's nonce count: its `nc` with qop auth (hex), 0 without qop; none for any other shape, which never verifies.
fn nonce_count(auth: &Authorization) -> Option<u32> {
    match (&auth.qop, &auth.nc, &auth.cnonce) {
        (Some(qop), Some(nc), Some(_)) if qop.eq_ignore_ascii_case("auth") => u32::from_str_radix(nc, 16).ok(),
        (None, _, _) => Some(0),
        _ => None,
    }
}

/// What an answer's response is computed from besides the HA1, so that whoever holds the verifier can check it: the
/// local node, for an edge node's registrar, which keeps the nonces but no verifier (plans/Edge-Node.md §5.4 "Voice
/// (SIP)", decision 4). `ha2` is H(method:uri).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseCheck {
    pub algorithm: Algorithm,
    pub nonce: String,
    /// With qop auth: its `nc` and `cnonce`; none without qop.
    pub qop_auth: Option<(String, String)>,
    pub ha2: String,
    /// The answer's response, lower case.
    pub response: String,
}

impl ResponseCheck {
    /// The check of `auth` for `method`; none for an answer no verifier makes right (no algorithm, another qop, a qop
    /// without its count and cnonce, a count that is not hex).
    pub fn of(auth: &Authorization, method: &str) -> Option<ResponseCheck> {
        let algorithm = auth.algorithm?;
        nonce_count(auth)?;
        let qop_auth = match (&auth.qop, &auth.nc, &auth.cnonce) {
            (Some(_), Some(nc), Some(cnonce)) => Some((nc.clone(), cnonce.clone())),
            _ => None,
        };
        Some(ResponseCheck {
            algorithm,
            nonce: auth.nonce.clone(),
            qop_auth,
            ha2: algorithm.hash(&format!("{method}:{}", auth.uri)),
            response: auth.response.to_ascii_lowercase(),
        })
    }

    /// The response a phone with this verifier (HA1) gives.
    pub fn expected(&self, ha1: &str) -> String {
        let ha1 = ha1.to_ascii_lowercase();
        match &self.qop_auth {
            Some((nc, cnonce)) => {
                let text = format!("{ha1}:{}:{nc}:{cnonce}:auth:{}", self.nonce, self.ha2);
                self.algorithm.hash(&text)
            }
            None => self.algorithm.hash(&format!("{ha1}:{}:{}", self.nonce, self.ha2)),
        }
    }

    /// Whether the response is right for `verifier`; with none it is checked against a stand-in the same way, so the
    /// time does not tell whether there is one.
    pub fn matches(&self, verifier: Option<&str>) -> bool {
        let known = verifier.is_some();
        let ha1 = match verifier {
            Some(v) => v.to_string(),
            None => self.algorithm.hash("\u{0}no verifier"),
        };
        constant_eq(self.expected(&ha1).as_bytes(), self.response.as_bytes()) && known
    }
}

/// A digest verifier: `H(username:realm:password)` (RFC 7616 §3.4.2), what the registrar stores for a phone instead of
/// its password, one per algorithm and realm.
pub fn ha1(algorithm: Algorithm, username: &str, realm: &str, password: &str) -> String {
    algorithm.hash(&format!("{username}:{realm}:{password}"))
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn quote(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

fn strip_scheme(value: &str) -> Option<&str> {
    let value = value.trim();
    let (scheme, rest) = value.split_once(char::is_whitespace)?;
    scheme.eq_ignore_ascii_case("digest").then_some(rest)
}

/// `k=v, k="v", …` with quoted strings and escapes; keys lowercased.
fn parse_params(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        let Some((key, after)) = rest.split_once('=') else {
            break;
        };
        let key = key.trim().trim_start_matches(',').trim().to_ascii_lowercase();
        let after = after.trim_start();
        let (value, next) = if let Some(quoted) = after.strip_prefix('"') {
            let mut value = String::new();
            let mut chars = quoted.char_indices();
            let mut end = quoted.len();
            while let Some((i, c)) = chars.next() {
                match c {
                    '\\' => {
                        if let Some((_, escaped)) = chars.next() {
                            value.push(escaped);
                        }
                    }
                    '"' => {
                        end = i + 1;
                        break;
                    }
                    _ => value.push(c),
                }
            }
            (value, quoted.get(end..).unwrap_or(""))
        } else {
            match after.split_once(',') {
                Some((v, n)) => (v.trim().to_string(), n),
                None => (after.trim().to_string(), ""),
            }
        };
        out.insert(key, value);
        rest = next.trim_start().trim_start_matches(',').trim_start();
    }
    out
}

#[cfg(test)]
#[path = "digest_tests.rs"]
mod tests;
