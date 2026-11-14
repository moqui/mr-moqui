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

//! A pool member's own key (plans/Edge-Node2.md §4.11). The members of a local cluster share the pairing's key, from
//! the `.env` they share, which says only which pairing a link is for. Each member also has an Ed25519 key of its own,
//! which its Hello proves it holds by signing the TLS connection's exporter value; the edge node pins the key for the
//! member's instance id on its first link, so a node with the shared key alone (a stolen `.env`, a staging copy) cannot
//! take a member's id.
//!
//! The key is kept in `runtime/db/edge_member_key`, beside `runtime/db/instance_id`, and for the same reason: it is
//! this instance's. Not in `.env`, which every member and a standby share; not in the database, which a standby
//! restores from a backup. A standby with a key of its own is then a new node to the edge node, which a takeover's
//! notice and hold are for.

use std::path::Path;
use std::sync::Arc;

use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair as _, UnparsedPublicKey, ED25519};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use sha2::{Digest as _, Sha256};

/// The TLS exporter label the member's proof signs the value of (RFC 5705).
pub(crate) const EXPORTER_LABEL: &[u8] = b"EXPORTER-moqui-edge-member";
/// What the proof signs before the exporter value, so the signature is good for nothing else.
const PROOF_CONTEXT: &[u8] = b"moqui-edge-member-hello:";
/// Where the key is kept, under the runtime directory.
const KEY_FILE: &str = "db/edge_member_key";

/// A member's own key.
pub struct MemberKey {
    pair: Ed25519KeyPair,
}

impl std::fmt::Debug for MemberKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemberKey")
            .field("fingerprint", &self.fingerprint())
            .finish()
    }
}

impl MemberKey {
    /// A new key, kept in memory only (a dialer with no server, as in a test).
    pub fn generate() -> Result<Self, String> {
        let rng = aws_lc_rs::rand::SystemRandom::new();
        let der = Ed25519KeyPair::generate_pkcs8(&rng).map_err(|_| "could not make a member key".to_string())?;
        Self::from_pkcs8(der.as_ref())
    }

    fn from_pkcs8(der: &[u8]) -> Result<Self, String> {
        let pair = Ed25519KeyPair::from_pkcs8(der).map_err(|_| "the member key is not an Ed25519 key".to_string())?;
        Ok(Self { pair })
    }

    /// The key kept in `file`, or what is wrong with it and what to do.
    fn read(file: &Path, text: &str) -> Result<Self, String> {
        let remedy = |what: &str| {
            format!(
                "{} {what}: move it aside to make a new key at the next start; the edge node then takes this member as a \
                 new node (a takeover notice or hold), or its operator runs forget#EdgeMember for this instance first",
                file.display()
            )
        };
        let der = STANDARD.decode(text.trim()).map_err(|_| remedy("is not base64"))?;
        Self::from_pkcs8(&der).map_err(|_| remedy("is not an Ed25519 key"))
    }

    /// This instance's key, from `runtime/db/edge_member_key` under `runtime`, made there on first use (owner read
    /// and write only).
    pub fn load_or_create(runtime: &Path) -> Result<Arc<Self>, String> {
        let file = runtime.join(KEY_FILE);
        if let Ok(text) = std::fs::read_to_string(&file) {
            return Self::read(&file, &text).map(Arc::new);
        }
        let rng = aws_lc_rs::rand::SystemRandom::new();
        let der = Ed25519KeyPair::generate_pkcs8(&rng).map_err(|_| "could not make a member key".to_string())?;
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).map_err(|err| format!("could not make {}: {err}", parent.display()))?;
        }
        // written whole to a file of its own (owner only), then put in place only where none is: a reader never sees a
        // part, and of two processes making one at once both keep the one put in place first
        let part = file.with_extension(format!("part-{}", std::process::id()));
        write_private(&part, &format!("{}\n", STANDARD.encode(der.as_ref())))?;
        let placed = std::fs::hard_link(&part, &file);
        let _ = std::fs::remove_file(&part);
        match placed {
            Ok(()) => tracing::info!("Edge link: this member's own key is made in {}", file.display()),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(format!("could not write {}: {err}", file.display())),
        }
        let text = std::fs::read_to_string(&file).map_err(|err| format!("could not read {}: {err}", file.display()))?;
        Self::read(&file, &text).map(Arc::new)
    }

    /// The public key, 32 bytes.
    pub fn public(&self) -> Vec<u8> {
        self.pair.public_key().as_ref().to_vec()
    }

    /// SHA-256 of the public key, hex: what the edge node pins.
    pub fn fingerprint(&self) -> String {
        fingerprint(self.pair.public_key().as_ref())
    }

    /// The proof of holding this key on the connection whose exporter value is `binding`.
    pub(crate) fn prove(&self, binding: &[u8]) -> Vec<u8> {
        self.pair.sign(&[PROOF_CONTEXT, binding].concat()).as_ref().to_vec()
    }
}

#[cfg(unix)]
fn write_private(file: &Path, text: &str) -> Result<(), String> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(file)
        .map_err(|err| format!("could not write {}: {err}", file.display()))?;
    out.write_all(text.as_bytes())
        .and_then(|()| out.sync_all())
        .map_err(|err| format!("could not write {}: {err}", file.display()))
}

#[cfg(not(unix))]
fn write_private(file: &Path, text: &str) -> Result<(), String> {
    std::fs::write(file, text).map_err(|err| format!("could not write {}: {err}", file.display()))
}

/// SHA-256 of a member's public key, hex.
pub(crate) fn fingerprint(public: &[u8]) -> String {
    let digest: [u8; 32] = Sha256::digest(public).into();
    crate::tls_pin::hex(&digest)
}

/// Whether `proof` is `public`'s signature of the connection whose exporter value is `binding`.
pub(crate) fn verify(public: &[u8], binding: &[u8], proof: &[u8]) -> bool {
    public.len() == 32
        && UnparsedPublicKey::new(&ED25519, public)
            .verify(&[PROOF_CONTEXT, binding].concat(), proof)
            .is_ok()
}

/// The exporter value of a TLS connection, which a member's proof is bound to; `None` before its handshake ended.
pub(crate) fn binding<D>(connection: &rustls::ConnectionCommon<D>) -> Option<[u8; 32]> {
    connection.export_keying_material([0_u8; 32], EXPORTER_LABEL, None).ok()
}
