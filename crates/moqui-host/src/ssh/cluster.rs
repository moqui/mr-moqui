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

//! The optional SSH cluster channel (`plans/archive/Ssh.md`, `moqui_conf__cluster_member__transport=ssh`). The HTTPS POST of `plans/archive/Cluster.md`
//! stays the default; a peer whose `clusterBaseUrl` is `https://` still gets it.
//!
//! A member advertises `ssh://host:port`, its SSH server, and records its host key fingerprint on its `ClusterMember`
//! row; a peer pins that fingerprint. The sender logs in as `moqui-cluster` with an Ed25519 key derived from the shared
//! `cluster-member@tls-key`, so only a holder of the cluster material gets in, and keeps one SSH session to each peer with one
//! channel for each topic. A message is a frame: the payload length on a line, then the payload, the same topic JSON as
//! the HTTPS body. The peer answers each frame with a status line (204 applied, else the HTTP status the POST would
//! give), after applying it through the same path as a POST.
//!
//! **The channel proof** (SEC-CLU-04). The host key a sender pins is a value of the peer's `ClusterMember` row, which
//! whoever can write that table chooses, so it does not show that the peer is a member. Before any frame each side of a
//! `cluster <topic>` channel proves it holds the cluster material: the server sends a random challenge
//! (`challenge <nonce>`), the connecting member answers with a nonce of its own and its signature of both
//! (`proof <nonce> <signature>`), and the server, once it has checked that, answers with its own signature
//! (`proof <signature>`), or `403` and closes. Each signs with an Ed25519 key derived from `cluster-member@tls-key` (the key
//! of the certificate whose fingerprint `cluster::same_material` compares) under a label of its own, over its role, the
//! server's host key fingerprint and both nonces, so a proof is good for one channel to one host and for nothing else.
//! The server takes no frame before the connecting member's proof, and the sender sends none before the server's: a
//! host that has a row and a host key but not the cluster material gets no message. The host key itself is unchanged,
//! so git and SFTP clients' pins stay.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use aws_lc_rs::rand::{SecureRandom as _, SystemRandom};
use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair as _, UnparsedPublicKey, ED25519};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use moqui_core::cluster::Sender;
use moqui_core::context::{Error as CoreError, ExecutionContextFactory};
use moqui_lib::error::Error;
use moqui_lib::sync::lock;
use russh::client;
use russh::keys::{HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh::ChannelStream;
use sha2::Digest;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

/// The user a cluster peer logs in as.
pub const CLUSTER_USER: &str = "moqui-cluster";
/// Largest frame accepted.
const MAX_FRAME: usize = moqui_core::cluster::MAX_CLUSTER_BODY;
const TIMEOUT: Duration = Duration::from_secs(30);
/// How long a peer's session is kept with nothing sent on it.
const PEER_IDLE: Duration = Duration::from_secs(600);
/// What a channel proof signs before its fields, so the signature is good for nothing else.
const PROOF_CONTEXT: &[u8] = b"moqui-cluster-channel-proof-v1\0";
/// The bytes of a challenge's and of an answer's nonce.
const NONCE: usize = 32;
/// The longest line of the channel proof: a nonce and a signature in base64, with room to spare.
const MAX_PROOF_LINE: u64 = 512;

/// The login key every member derives from the shared `cluster-member@tls-key`.
///
/// # Errors
///
/// Returns an error when the cluster key is not configured.
pub fn cluster_login_key() -> Result<PrivateKey, Error> {
    let der = crate::cluster_edge::cluster_key_der()?;
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"moqui cluster ssh login\0");
    hasher.update(&der);
    let seed: [u8; 32] = hasher.finalize().into();
    Ok(PrivateKey::from(
        russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&seed),
    ))
}

/// The key a member proves the cluster material with on a cluster channel (SEC-CLU-04): Ed25519, derived from the
/// shared `cluster-member@tls-key` as the login key is, under a label of its own.
///
/// # Errors
///
/// Returns an error when the cluster key is not configured.
pub fn channel_proof_key() -> Result<Ed25519KeyPair, Error> {
    let der = crate::cluster_edge::cluster_key_der()?;
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"moqui cluster ssh channel proof\0");
    hasher.update(&der);
    let seed: [u8; 32] = hasher.finalize().into();
    Ed25519KeyPair::from_seed_unchecked(&seed)
        .map_err(|_| Error::new("the cluster channel proof key could not be made"))
}

/// What one side of a channel signs: its role, the server's host key fingerprint and both nonces.
fn proof_message(role: &str, host: &str, challenge: &[u8], answer: &[u8]) -> Vec<u8> {
    [
        PROOF_CONTEXT,
        role.as_bytes(),
        b"\0",
        host.as_bytes(),
        b"\0",
        challenge,
        answer,
    ]
    .concat()
}

fn nonce() -> Result<[u8; NONCE], String> {
    let mut bytes = [0u8; NONCE];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "no random bytes for the channel proof".to_string())?;
    Ok(bytes)
}

/// Whether `signature` is the cluster proof key's (`key`'s public half) of `message`.
fn verified(key: &Ed25519KeyPair, message: &[u8], signature: &[u8]) -> bool {
    UnparsedPublicKey::new(&ED25519, key.public_key().as_ref())
        .verify(message, signature)
        .is_ok()
}

/// One line of the proof, at most [`MAX_PROOF_LINE`] bytes, within [`TIMEOUT`]; empty when the other side closed.
async fn proof_line<S: AsyncRead + Unpin>(stream: &mut BufReader<S>) -> Result<String, String> {
    let mut line = String::new();
    tokio::time::timeout(TIMEOUT, (&mut *stream).take(MAX_PROOF_LINE).read_line(&mut line))
        .await
        .map_err(|_| "no answer in time".to_string())?
        .map_err(|err| err.to_string())?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

async fn write_line<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut BufReader<S>, line: &str) -> Result<(), String> {
    let out = stream.get_mut();
    tokio::time::timeout(TIMEOUT, async {
        out.write_all(format!("{line}\n").as_bytes()).await?;
        out.flush().await
    })
    .await
    .map_err(|_| "the line was not sent in time".to_string())?
    .map_err(|err| err.to_string())
}

fn decoded(text: Option<&str>, what: &str) -> Result<Vec<u8>, String> {
    text.and_then(|text| STANDARD.decode(text).ok())
        .ok_or_else(|| format!("its {what} is not base64"))
}

/// The server's side of a cluster channel's proof (SEC-CLU-04): challenge the connecting member, check that it signed
/// the challenge with the cluster proof key `key` for this host (`host`, this server's host key fingerprint), and
/// answer with this side's own signature. On a refusal `403` is sent and the reason returned; no frame is read before.
///
/// # Errors
///
/// Why the member's proof is refused: none (it sent a frame or closed), malformed, or not of the cluster key.
pub async fn prove_server<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut BufReader<S>,
    key: &Ed25519KeyPair,
    host: &str,
) -> Result<(), String> {
    let challenge = nonce()?;
    write_line(stream, &format!("challenge {}", STANDARD.encode(challenge))).await?;
    let checked = async {
        let line = proof_line(stream).await?;
        let mut fields = line.split(' ');
        if fields.next() != Some("proof") {
            return Err(
                "it sent no proof of the cluster key (a member of an older version, or not a member)".to_string(),
            );
        }
        let answer = decoded(fields.next(), "nonce")?;
        let signature = decoded(fields.next(), "signature")?;
        if answer.len() != NONCE || fields.next().is_some() {
            return Err("its proof is malformed".to_string());
        }
        if !verified(key, &proof_message("client", host, &challenge, &answer), &signature) {
            return Err("its proof is not of the cluster key".to_string());
        }
        Ok(answer)
    }
    .await;
    let answer = match checked {
        Ok(answer) => answer,
        Err(why) => {
            let _ = write_line(stream, "403").await;
            return Err(why);
        }
    };
    let signature = key.sign(&proof_message("server", host, &challenge, &answer));
    write_line(stream, &format!("proof {}", STANDARD.encode(signature.as_ref()))).await
}

/// The connecting member's side of a cluster channel's proof (SEC-CLU-04): answer the server's challenge with a nonce
/// and the cluster proof key `key`'s signature for the host whose key it checked (`host`, its fingerprint), then check
/// the server's own signature. Nothing is sent on the channel before this succeeds.
///
/// # Errors
///
/// Why the server is not taken as a member: no challenge, its refusal of this member's proof, or no proof of the cluster
/// key of its own.
pub async fn prove_client<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut BufReader<S>,
    key: &Ed25519KeyPair,
    host: &str,
) -> Result<(), String> {
    let line = proof_line(stream).await?;
    let challenge = match line.strip_prefix("challenge ") {
        Some(text) => decoded(Some(text), "challenge")?,
        None => return Err("it sent no challenge (a member of an older version, or not a member)".to_string()),
    };
    let answer = nonce()?;
    let signature = key.sign(&proof_message("client", host, &challenge, &answer));
    write_line(
        stream,
        &format!(
            "proof {} {}",
            STANDARD.encode(answer),
            STANDARD.encode(signature.as_ref())
        ),
    )
    .await?;
    let line = proof_line(stream).await?;
    let Some(text) = line.strip_prefix("proof ") else {
        return Err(if line == "403" {
            "it refused this member's proof of the cluster key".to_string()
        } else {
            "it sent no proof of the cluster key".to_string()
        });
    };
    let theirs = decoded(Some(text), "proof")?;
    if !verified(key, &proof_message("server", host, &challenge, &answer), &theirs) {
        return Err("its proof is not of the cluster key".to_string());
    }
    Ok(())
}

/// True when `key` is the cluster login key.
pub fn is_cluster_key(key: &PublicKey) -> bool {
    cluster_login_key().is_ok_and(|own| own.public_key().key_data() == key.key_data())
}

/// Whether this member takes the SSH cluster channel: its base URL is `ssh://` (`moqui_conf__cluster_member__transport=ssh`, or a
/// `cluster-member@base-url` of `ssh://`). Peers reach any other member over the cluster port.
pub(crate) fn takes_ssh_channel() -> bool {
    moqui_core::context::conf_env::setting("cluster-member@transport") == "ssh"
        || moqui_core::context::conf_env::setting("cluster-member@base-url").starts_with("ssh://")
}

/// Why a login with the cluster key is refused, or `None` when it is taken (SEC-CLU-03): only on a member that takes
/// the SSH channel ([`takes_ssh_channel`]), never through an edge node's tunnel, and only from an address one of the
/// members' base URLs resolves to (`members`).
pub(crate) fn login_refused(
    takes_ssh: bool,
    via_edge: bool,
    source: std::net::IpAddr,
    members: &std::collections::HashSet<std::net::IpAddr>,
) -> Option<&'static str> {
    if via_edge {
        Some("it came through an edge node's tunnel")
    } else if !takes_ssh {
        Some("this member's cluster transport is not ssh")
    } else if !members.contains(&source.to_canonical()) {
        Some("its address is no cluster member's")
    } else {
        None
    }
}

/// Install the SSH sender, which keeps the HTTPS one for `https://` peers.
///
/// # Errors
///
/// Returns an error when the cluster material is missing.
pub fn install_sender(factory: &Arc<ExecutionContextFactory>) -> Result<(), Error> {
    moqui_core::cluster::install_sender(factory, sender(factory)?);
    Ok(())
}

/// The SSH sender for `factory`'s cluster.
///
/// # Errors
///
/// Returns an error when the cluster material is missing.
pub fn sender(factory: &Arc<ExecutionContextFactory>) -> Result<Arc<dyn Sender>, Error> {
    Ok(Arc::new(SshSender {
        factory: Arc::downgrade(factory),
        https: crate::cluster_edge::https_sender()?,
        key: Arc::new(cluster_login_key()?),
        proof: Arc::new(channel_proof_key()?),
        peers: Mutex::new(HashMap::new()),
    }))
}

/// One peer: the session, the fingerprint of the host key it showed, and a channel for each topic, each proven.
struct Peer {
    handle: client::Handle<PinnedKey>,
    host: String,
    channels: HashMap<String, BufReader<ChannelStream<client::Msg>>>,
}

type PeerSlot = Arc<tokio::sync::Mutex<Option<Peer>>>;

pub struct SshSender {
    factory: Weak<ExecutionContextFactory>,
    https: Arc<dyn Sender>,
    key: Arc<PrivateKey>,
    /// The key each channel's proof is signed with (SEC-CLU-04).
    proof: Arc<Ed25519KeyPair>,
    /// The session slot of each peer and when it was last used.
    peers: Mutex<HashMap<String, (PeerSlot, Instant)>>,
}

impl Sender for SshSender {
    fn post(&self, base_url: &str, topic: &str, payload: &[u8]) -> Result<moqui_core::cluster::PostOutcome, CoreError> {
        if payload.len() > moqui_core::cluster::MAX_CLUSTER_BODY {
            return Ok(moqui_core::cluster::PostOutcome::Rejected(413));
        }
        let Some(address) = base_url.strip_prefix("ssh://") else {
            return self.https.post(base_url, topic, payload);
        };
        let fingerprint = self.pinned(base_url)?;
        let slot = self.slot(base_url);
        let (address, topic, payload, keys) = (
            address.to_string(),
            topic.to_string(),
            payload.to_vec(),
            Keys {
                login: Arc::clone(&self.key),
                proof: Arc::clone(&self.proof),
            },
        );
        let status = crate::edge::block_on(async move {
            let mut peer = slot.lock().await;
            let sent = send_frame(&mut peer, &address, &fingerprint, &keys, &topic, &payload).await;
            if sent.is_err() {
                // A broken session is opened again on the next post.
                peer.take();
            }
            sent
        })
        .map_err(|err| CoreError::new(err.to_string()))?
        .map_err(|err| CoreError::new(err.to_string()))?;
        moqui_core::cluster::PostOutcome::from_status(status)
    }
}

impl Drop for SshSender {
    /// The sessions and channel streams need the runtime they were opened on to close.
    fn drop(&mut self) {
        let peers = std::mem::take(&mut *lock(&self.peers));
        if !peers.is_empty() {
            crate::edge::handle().spawn(async move { drop(peers) });
        }
    }
}

impl SshSender {
    /// The session slot of `base_url`, now marked used. A slot unused for [`PEER_IDLE`] is dropped, closing its
    /// session, so one to a member that left the cluster is not kept.
    fn slot(&self, base_url: &str) -> PeerSlot {
        let mut peers = lock(&self.peers);
        let idle: Vec<_> = peers.extract_if(|_, (_, used)| used.elapsed() >= PEER_IDLE).collect();
        let (slot, used) = peers
            .entry(base_url.to_string())
            .or_insert_with(|| (PeerSlot::default(), Instant::now()));
        *used = Instant::now();
        let slot = Arc::clone(slot);
        drop(peers);
        if !idle.is_empty() {
            // As in `Drop`: the sessions close on the runtime they were opened on.
            crate::edge::handle().spawn(async move { drop(idle) });
        }
        slot
    }

    /// The host key fingerprint the peer registered on its `ClusterMember` row.
    fn pinned(&self, base_url: &str) -> Result<String, CoreError> {
        let factory = self
            .factory
            .upgrade()
            .ok_or_else(|| CoreError::new("the factory is gone"))?;
        let facade = factory
            .entity()
            .ok_or_else(|| CoreError::new("the entity facade is not installed"))?;
        moqui_core::cluster::peer_ssh_fingerprint(&facade, base_url)?
            .ok_or_else(|| CoreError::new(format!("cluster peer {base_url} has no SSH host key fingerprint")))
    }
}

/// This member's keys: the login key and the channel proof key.
struct Keys {
    login: Arc<PrivateKey>,
    proof: Arc<Ed25519KeyPair>,
}

/// Accepts only the pinned host key, and keeps its fingerprint as the server showed it, for the channel proof.
struct PinnedKey {
    fingerprint: String,
    seen: Arc<OnceLock<String>>,
}

impl client::Handler for PinnedKey {
    type Error = russh::Error;

    async fn check_server_key(&mut self, server_public_key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let actual = server_public_key.public_key().fingerprint(HashAlg::Sha256).to_string();
        let _ = self.seen.set(actual.clone());
        let trusted = super::client::same_fingerprint(&self.fingerprint, &actual);
        if !trusted {
            tracing::warn!(
                "cluster peer host key {actual} is not the registered {}",
                self.fingerprint
            );
        }
        Ok(trusted)
    }
}

async fn send_frame(
    peer: &mut Option<Peer>,
    address: &str,
    fingerprint: &str,
    keys: &Keys,
    topic: &str,
    payload: &[u8],
) -> Result<u16, Error> {
    if peer.as_ref().is_none_or(|open| open.handle.is_closed()) {
        *peer = Some(connect(address, fingerprint, &keys.login).await?);
    }
    let Some(open) = peer.as_mut() else {
        return Err(Error::new("no cluster session"));
    };
    if !open.channels.contains_key(topic) {
        let channel = open
            .handle
            .channel_open_session()
            .await
            .map_err(|err| Error::new(format!("cluster channel to {address}: {err}")))?;
        channel
            .exec(true, format!("cluster {topic}"))
            .await
            .map_err(|err| Error::new(format!("cluster channel to {address}: {err}")))?;
        let mut stream = BufReader::new(channel.into_stream());
        // no frame goes to a host that does not prove the cluster material (SEC-CLU-04)
        if let Err(why) = prove_client(&mut stream, &keys.proof, &open.host).await {
            tracing::warn!("cluster peer {address} is not sent {topic}: {why}");
            return Err(Error::new(format!(
                "cluster peer {address} did not prove the cluster key: {why}"
            )));
        }
        open.channels.insert(topic.to_string(), stream);
    }
    let Some(stream) = open.channels.get_mut(topic) else {
        return Err(Error::new("no cluster channel"));
    };
    let exchange = async {
        let stream = stream.get_mut();
        stream.write_all(format!("{}\n", payload.len()).as_bytes()).await?;
        stream.write_all(payload).await?;
        stream.flush().await
    };
    tokio::time::timeout(TIMEOUT, exchange)
        .await
        .map_err(|_| Error::new(format!("cluster send to {address} timed out")))?
        .map_err(|err| Error::new(format!("cluster send to {address}: {err}")))?;
    let mut line = String::new();
    let read = tokio::time::timeout(TIMEOUT, stream.read_line(&mut line))
        .await
        .map_err(|_| Error::new(format!("cluster peer {address} did not answer")))?
        .map_err(|err| Error::new(format!("cluster peer {address}: {err}")))?;
    if read == 0 {
        open.channels.remove(topic);
        return Err(Error::new(format!("cluster peer {address} closed the channel")));
    }
    line.trim()
        .parse::<u16>()
        .map_err(|_| Error::new(format!("cluster peer {address} answered {line}")))
}

async fn connect(address: &str, fingerprint: &str, key: &Arc<PrivateKey>) -> Result<Peer, Error> {
    let config = Arc::new(client::Config {
        inactivity_timeout: None,
        keepalive_interval: Some(Duration::from_secs(60)),
        ..client::Config::default()
    });
    let seen = Arc::new(OnceLock::new());
    let check = PinnedKey {
        fingerprint: fingerprint.to_string(),
        seen: Arc::clone(&seen),
    };
    let mut handle = tokio::time::timeout(TIMEOUT, client::connect(config, address, check))
        .await
        .map_err(|_| Error::new(format!("timed out connecting to cluster peer {address}")))?
        .map_err(|err| Error::new(format!("could not connect to cluster peer {address}: {err}")))?;
    let with_hash = PrivateKeyWithHashAlg::new(Arc::clone(key), None);
    let auth = handle
        .authenticate_publickey(CLUSTER_USER, with_hash)
        .await
        .map_err(|err| Error::new(format!("cluster login to {address}: {err}")))?;
    if !auth.success() {
        return Err(Error::new(format!("cluster peer {address} refused the cluster key")));
    }
    let Some(host) = seen.get().cloned() else {
        return Err(Error::new(format!("cluster peer {address} showed no host key")));
    };
    Ok(Peer {
        handle,
        host,
        channels: HashMap::new(),
    })
}

/// What the server of a cluster channel knows of it: who connected, and its own host key's fingerprint.
pub(super) struct ChannelSide {
    pub(super) peer: std::net::SocketAddr,
    pub(super) host: String,
}

/// The receiving side of a `cluster <topic>` exec channel: the channel proof first (SEC-CLU-04), then read frames until
/// the peer closes it, apply each, answer each with its status.
pub(super) async fn serve_channel(
    stream: ChannelStream<russh::server::Msg>,
    factory: Weak<ExecutionContextFactory>,
    posts: Arc<Mutex<Vec<String>>>,
    topic: String,
    side: ChannelSide,
) {
    let mut stream = BufReader::new(stream);
    let proven = match channel_proof_key() {
        Ok(key) => prove_server(&mut stream, &key, &side.host).await,
        Err(err) => Err(format!("this member has no cluster key: {err}")),
    };
    if let Err(why) = proven {
        tracing::warn!(
            "cluster channel {topic} from {} refused before any message: {why}",
            side.peer
        );
        return;
    }
    loop {
        let mut line = String::new();
        match stream.read_line(&mut line).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let Ok(length) = line.trim().parse::<usize>() else {
            let _ = stream.get_mut().write_all(b"400\n").await;
            return;
        };
        if length > MAX_FRAME {
            let _ = stream.get_mut().write_all(b"413\n").await;
            return;
        }
        let mut body = vec![0u8; length];
        if stream.read_exact(&mut body).await.is_err() {
            return;
        }
        let (factory, posts, topic_name) = (Weak::clone(&factory), Arc::clone(&posts), topic.clone());
        let status = tokio::task::spawn_blocking(move || match factory.upgrade() {
            Some(factory) => crate::cluster_edge::apply_topic(&factory, &posts, &topic_name, &body),
            None => 503,
        })
        .await
        .unwrap_or(500);
        if stream
            .get_mut()
            .write_all(format!("{status}\n").as_bytes())
            .await
            .is_err()
        {
            return;
        }
        let _ = stream.get_mut().flush().await;
    }
}

#[cfg(test)]
#[path = "cluster_tests.rs"]
mod tests;
