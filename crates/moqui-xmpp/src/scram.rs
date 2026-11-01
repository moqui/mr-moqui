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

//! SCRAM-SHA-256 and SCRAM-SHA-256-PLUS (RFC 5802, RFC 7677), with tls-exporter channel binding (RFC 9266, XEP-0440).
//!
//! In house on the workspace's `pbkdf2`, `hmac` and `sha2` rather than the `sasl` crate: its server side takes a
//! password provider, not stored verifiers, and it is on older RustCrypto releases than the workspace (a second hmac,
//! sha2 and pbkdf2). The spike is recorded in plans/archive/License-Review.md.
//!
//! The server keeps no password: an app password's verifiers are its `StoredKey` and `ServerKey` with the user's salt
//! and iteration count (`UserAppPassword.scram*`, made when the password is). A user's XMPP app passwords share one
//! salt, so the one server-first message fits each, and the client's proof is checked against every stored key in turn.
//! A user that does not exist gets a salt derived from the name, so the exchange does not tell it apart.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use hmac::{KeyInit, Mac};
use sha2::{Digest, Sha256};

/// The iteration count of new verifiers.
pub const ITERATIONS: u32 = 10_000;
/// The tls-exporter label (RFC 9266).
pub const EXPORTER_LABEL: &[u8] = b"EXPORTER-Channel-Binding";

/// An app password's verifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verifier {
    pub stored_key: Vec<u8>,
    pub server_key: Vec<u8>,
}

/// What the server has for a user: one salt and count, and a verifier per password.
#[derive(Debug, Clone)]
pub struct Credentials {
    pub salt: Vec<u8>,
    pub iterations: u32,
    pub verifiers: Vec<Verifier>,
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = hmac::Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes a key of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn salted_password(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    pbkdf2::pbkdf2_hmac_array::<Sha256, 32>(password, salt, iterations)
}

/// The verifiers of `password` with `salt` and `iterations`.
pub fn derive(password: &str, salt: &[u8], iterations: u32) -> Verifier {
    let salted = salted_password(password.as_bytes(), salt, iterations);
    let client_key = hmac(&salted, b"Client Key");
    Verifier {
        stored_key: Sha256::digest(&client_key).to_vec(),
        server_key: hmac(&salted, b"Server Key"),
    }
}

/// Equal in constant time for equal lengths.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0_u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A random nonce, printable.
fn nonce() -> String {
    let mut bytes = [0_u8; 18];
    getrandom::fill(&mut bytes).expect("the system random source works");
    STANDARD.encode(bytes)
}

fn unescape_name(name: &str) -> Option<String> {
    let mut out = String::new();
    let mut rest = name;
    while let Some((before, after)) = rest.split_once('=') {
        out.push_str(before);
        match after.get(..2) {
            Some("2C") => out.push(','),
            Some("3D") => out.push('='),
            _ => return None,
        }
        rest = &after[2..];
    }
    out.push_str(rest);
    Some(out)
}

fn escape_name(name: &str) -> String {
    name.replace('=', "=3D").replace(',', "=2C")
}

/// The binding a client asked for in its GS2 header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gs2 {
    /// `n`: the client does not bind.
    None,
    /// `y`: the client binds, but thinks the server cannot.
    ClientOnly,
    /// `p=tls-exporter`.
    Exporter,
}

/// A parsed client-first message.
#[derive(Debug, Clone)]
pub struct ClientFirst {
    pub gs2: Gs2,
    pub gs2_header: String,
    pub username: String,
    client_nonce: String,
    bare: String,
}

/// Why an exchange failed; each is `not-authorized` or `malformed-request` on the wire, and says no more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    Malformed,
    /// A downgrade (`y` where the server binds), a binding of another type, or binding data that does not match.
    Binding,
    NotAuthorized,
}

/// Parse a client-first message for the mechanism `plus` (SCRAM-SHA-256-PLUS) or not, on a connection that can bind
/// (`binds`: the server advertised the PLUS mechanism).
pub fn client_first(data: &[u8], plus: bool, binds: bool) -> Result<ClientFirst, Refused> {
    let text = std::str::from_utf8(data).map_err(|_| Refused::Malformed)?;
    let (flag, rest) = text.split_once(',').ok_or(Refused::Malformed)?;
    let (authzid, bare) = rest.split_once(',').ok_or(Refused::Malformed)?;
    if !(authzid.is_empty() || authzid.starts_with("a=")) {
        return Err(Refused::Malformed);
    }
    let gs2 = match flag {
        "n" => Gs2::None,
        "y" => Gs2::ClientOnly,
        "p=tls-exporter" => Gs2::Exporter,
        _ if flag.starts_with("p=") => return Err(Refused::Binding),
        _ => return Err(Refused::Malformed),
    };
    match (&gs2, plus) {
        (Gs2::Exporter, true) => {}
        (Gs2::Exporter, false) | (Gs2::None | Gs2::ClientOnly, true) => return Err(Refused::Binding),
        // RFC 5802 6: a client that could bind but thinks the server cannot, on a connection that binds, is a downgrade
        (Gs2::ClientOnly, false) if binds => return Err(Refused::Binding),
        _ => {}
    }
    let mut username = None;
    let mut client_nonce = None;
    for attribute in bare.split(',') {
        if let Some(value) = attribute.strip_prefix("n=") {
            username = unescape_name(value);
        } else if let Some(value) = attribute.strip_prefix("r=") {
            client_nonce = Some(value.to_string());
        } else if attribute.starts_with("m=") {
            // a mandatory extension this server does not know
            return Err(Refused::Malformed);
        }
    }
    let (Some(username), Some(client_nonce)) = (username, client_nonce) else {
        return Err(Refused::Malformed);
    };
    if username.is_empty() || client_nonce.is_empty() {
        return Err(Refused::Malformed);
    }
    Ok(ClientFirst {
        gs2,
        gs2_header: format!("{flag},{authzid},"),
        username,
        client_nonce,
        bare: bare.to_string(),
    })
}

/// The server's side of one exchange after the client-first message.
pub struct ServerExchange {
    first: ClientFirst,
    nonce: String,
    server_first: String,
    credentials: Option<Credentials>,
    binding: Option<Vec<u8>>,
}

/// The name a decoy salt is derived from: the account lookup's form of it (XEP-0106 unescaped, trimmed, without case),
/// so every spelling that would find one account gets one salt, for a name that is no account's as for one that is.
fn decoy_name(username: &str) -> String {
    crate::jids::unescape_node(username).trim().to_lowercase()
}

impl ServerExchange {
    /// Start with what the server has for the user (None for one it does not know: the exchange goes on with a salt
    /// derived from `decoy_key` and the name, and fails at the end), and the connection's exporter, if it binds.
    pub fn start(
        first: ClientFirst,
        credentials: Option<Credentials>,
        binding: Option<Vec<u8>>,
        decoy_key: &[u8],
    ) -> (ServerExchange, Vec<u8>) {
        let nonce = format!("{}{}", first.client_nonce, nonce());
        let (salt, iterations) = match &credentials {
            Some(found) => (found.salt.clone(), found.iterations),
            None => (
                hmac(decoy_key, decoy_name(&first.username).as_bytes())[..16].to_vec(),
                ITERATIONS,
            ),
        };
        let server_first = format!("r={nonce},s={},i={iterations}", STANDARD.encode(salt));
        let out = server_first.clone().into_bytes();
        (
            ServerExchange {
                first,
                nonce,
                server_first,
                credentials,
                binding,
            },
            out,
        )
    }

    pub fn username(&self) -> &str {
        &self.first.username
    }

    /// Check the client-final message: the index of the verifier that matched and the server-final message.
    pub fn finish(&self, data: &[u8]) -> Result<(usize, Vec<u8>), Refused> {
        let text = std::str::from_utf8(data).map_err(|_| Refused::Malformed)?;
        let (without_proof, proof) = text.rsplit_once(",p=").ok_or(Refused::Malformed)?;
        let mut binding = None;
        let mut nonce = None;
        for attribute in without_proof.split(',') {
            if let Some(value) = attribute.strip_prefix("c=") {
                binding = Some(STANDARD.decode(value).map_err(|_| Refused::Malformed)?);
            } else if let Some(value) = attribute.strip_prefix("r=") {
                nonce = Some(value);
            }
        }
        let (Some(binding), Some(nonce)) = (binding, nonce) else {
            return Err(Refused::Malformed);
        };
        if nonce != self.nonce {
            return Err(Refused::NotAuthorized);
        }
        let mut expected = self.first.gs2_header.clone().into_bytes();
        if self.first.gs2 == Gs2::Exporter {
            let Some(data) = &self.binding else {
                return Err(Refused::Binding);
            };
            expected.extend_from_slice(data);
        }
        if !same(&binding, &expected) {
            return Err(Refused::Binding);
        }
        let proof = STANDARD.decode(proof).map_err(|_| Refused::Malformed)?;
        let Some(credentials) = &self.credentials else {
            return Err(Refused::NotAuthorized);
        };
        let auth_message = format!("{},{},{without_proof}", self.first.bare, self.server_first);
        for (index, verifier) in credentials.verifiers.iter().enumerate() {
            let signature = hmac(&verifier.stored_key, auth_message.as_bytes());
            if signature.len() != proof.len() {
                continue;
            }
            let client_key: Vec<u8> = proof.iter().zip(&signature).map(|(p, s)| p ^ s).collect();
            if same(&Sha256::digest(&client_key), &verifier.stored_key) {
                let server_signature = hmac(&verifier.server_key, auth_message.as_bytes());
                return Ok((index, format!("v={}", STANDARD.encode(server_signature)).into_bytes()));
            }
        }
        Err(Refused::NotAuthorized)
    }
}

/// The client's side, for the tests and the integration test's client.
pub struct ClientExchange {
    password: String,
    gs2_header: String,
    binding: Option<Vec<u8>>,
    bare: String,
    client_nonce: String,
    server_signature: Option<Vec<u8>>,
}

impl ClientExchange {
    /// A client-first message for `username`; with `binding` (the exporter) it is for the PLUS mechanism.
    pub fn new(username: &str, password: &str, binding: Option<Vec<u8>>) -> (ClientExchange, Vec<u8>) {
        let client_nonce = nonce();
        let gs2_header = if binding.is_some() { "p=tls-exporter,," } else { "n,," }.to_string();
        let bare = format!("n={},r={client_nonce}", escape_name(username));
        let out = format!("{gs2_header}{bare}").into_bytes();
        (
            ClientExchange {
                password: password.to_string(),
                gs2_header,
                binding,
                bare,
                client_nonce,
                server_signature: None,
            },
            out,
        )
    }

    /// The client-final message for the server-first one.
    pub fn respond(&mut self, server_first: &[u8]) -> Option<Vec<u8>> {
        let text = std::str::from_utf8(server_first).ok()?;
        let mut nonce = None;
        let mut salt = None;
        let mut iterations = None;
        for attribute in text.split(',') {
            if let Some(value) = attribute.strip_prefix("r=") {
                nonce = Some(value.to_string());
            } else if let Some(value) = attribute.strip_prefix("s=") {
                salt = STANDARD.decode(value).ok();
            } else if let Some(value) = attribute.strip_prefix("i=") {
                iterations = value.parse::<u32>().ok();
            }
        }
        let (nonce, salt, iterations) = (nonce?, salt?, iterations?);
        if !nonce.starts_with(&self.client_nonce) {
            return None;
        }
        let mut binding = self.gs2_header.clone().into_bytes();
        if let Some(data) = &self.binding {
            binding.extend_from_slice(data);
        }
        let without_proof = format!("c={},r={nonce}", STANDARD.encode(binding));
        let auth_message = format!("{},{text},{without_proof}", self.bare);
        let salted = salted_password(self.password.as_bytes(), &salt, iterations);
        let client_key = hmac(&salted, b"Client Key");
        let stored_key = Sha256::digest(&client_key);
        let signature = hmac(&stored_key, auth_message.as_bytes());
        let proof: Vec<u8> = client_key.iter().zip(&signature).map(|(k, s)| k ^ s).collect();
        self.server_signature = Some(hmac(&hmac(&salted, b"Server Key"), auth_message.as_bytes()));
        Some(format!("{without_proof},p={}", STANDARD.encode(proof)).into_bytes())
    }

    /// Whether the server-final message proves the server knew the verifier.
    pub fn verify(&self, server_final: &[u8]) -> bool {
        let Some(expected) = &self.server_signature else {
            return false;
        };
        let Some(value) = std::str::from_utf8(server_final)
            .ok()
            .and_then(|text| text.strip_prefix("v="))
        else {
            return false;
        };
        STANDARD.decode(value).is_ok_and(|got| same(&got, expected))
    }
}

#[cfg(test)]
#[path = "scram_tests.rs"]
mod tests;
