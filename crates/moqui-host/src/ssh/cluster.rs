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

//! The optional SSH cluster channel (`plans/archive/Ssh.md`, `cluster_transport=ssh`). The HTTPS POST of `plans/archive/Cluster.md`
//! stays the default; a peer whose `clusterBaseUrl` is `https://` still gets it.
//!
//! A member advertises `ssh://host:port`, its SSH server, and records its host key fingerprint on its `ClusterMember`
//! row; a peer pins that fingerprint. The sender logs in as `moqui-cluster` with an Ed25519 key derived from the shared
//! `cluster_tls_key`, so only a holder of the cluster material gets in, and keeps one SSH session to each peer with one
//! channel for each topic. A message is a frame: the payload length on a line, then the payload, the same topic JSON as
//! the HTTPS body. The peer answers each frame with a status line (204 applied, else the HTTP status the POST would
//! give), after applying it through the same path as a POST.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use moqui_core::cluster::Sender;
use moqui_core::context::{Error as CoreError, ExecutionContextFactory};
use moqui_lib::error::Error;
use russh::client;
use russh::keys::{HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh::ChannelStream;
use sha2::Digest;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

/// The user a cluster peer logs in as.
pub const CLUSTER_USER: &str = "moqui-cluster";
/// Largest frame accepted.
const MAX_FRAME: usize = moqui_core::cluster::MAX_CLUSTER_BODY;
const TIMEOUT: Duration = Duration::from_secs(30);
/// How long a peer's session is kept with nothing sent on it.
const PEER_IDLE: Duration = Duration::from_secs(600);

/// The login key every member derives from the shared `cluster_tls_key`.
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

/// True when `key` is the cluster login key.
pub fn is_cluster_key(key: &PublicKey) -> bool {
    cluster_login_key().is_ok_and(|own| own.public_key().key_data() == key.key_data())
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
        peers: Mutex::new(HashMap::new()),
    }))
}

/// One peer: the session and a channel for each topic.
struct Peer {
    handle: client::Handle<PinnedKey>,
    channels: HashMap<String, BufReader<ChannelStream<client::Msg>>>,
}

type PeerSlot = Arc<tokio::sync::Mutex<Option<Peer>>>;

pub struct SshSender {
    factory: Weak<ExecutionContextFactory>,
    https: Arc<dyn Sender>,
    key: Arc<PrivateKey>,
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
        let (address, topic, payload, key) = (
            address.to_string(),
            topic.to_string(),
            payload.to_vec(),
            Arc::clone(&self.key),
        );
        let status = crate::edge::block_on(async move {
            let mut peer = slot.lock().await;
            let sent = send_frame(&mut peer, &address, &fingerprint, &key, &topic, &payload).await;
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
        let peers = std::mem::take(&mut *self.peers.lock().unwrap_or_else(PoisonError::into_inner));
        if !peers.is_empty() {
            crate::edge::handle().spawn(async move { drop(peers) });
        }
    }
}

impl SshSender {
    /// The session slot of `base_url`, now marked used. A slot unused for [`PEER_IDLE`] is dropped, closing its
    /// session, so one to a member that left the cluster is not kept.
    fn slot(&self, base_url: &str) -> PeerSlot {
        let mut peers = self.peers.lock().unwrap_or_else(PoisonError::into_inner);
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

/// Accepts only the pinned host key.
struct PinnedKey {
    fingerprint: String,
}

impl client::Handler for PinnedKey {
    type Error = russh::Error;

    async fn check_server_key(&mut self, server_public_key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let actual = server_public_key.public_key().fingerprint(HashAlg::Sha256).to_string();
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
    key: &Arc<PrivateKey>,
    topic: &str,
    payload: &[u8],
) -> Result<u16, Error> {
    if peer.as_ref().is_none_or(|open| open.handle.is_closed()) {
        *peer = Some(connect(address, fingerprint, key).await?);
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
        open.channels
            .insert(topic.to_string(), BufReader::new(channel.into_stream()));
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
    let check = PinnedKey {
        fingerprint: fingerprint.to_string(),
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
    Ok(Peer {
        handle,
        channels: HashMap::new(),
    })
}

/// The receiving side of a `cluster <topic>` exec channel: read frames until the peer closes it, apply each, answer
/// each with its status.
pub(super) async fn serve_channel(
    stream: ChannelStream<russh::server::Msg>,
    factory: Weak<ExecutionContextFactory>,
    posts: Arc<Mutex<Vec<String>>>,
    topic: String,
) {
    let mut stream = BufReader::new(stream);
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
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, Weak};
    use std::time::{Duration, Instant};

    use moqui_core::cluster::Sender;
    use moqui_core::context::Error as CoreError;
    use russh::keys::PrivateKey;

    use super::{PeerSlot, SshSender, PEER_IDLE};

    struct NoSender;

    impl Sender for NoSender {
        fn post(
            &self,
            _base_url: &str,
            _topic: &str,
            _payload: &[u8],
        ) -> Result<moqui_core::cluster::PostOutcome, CoreError> {
            Err(CoreError::new("not in this test"))
        }
    }

    #[test]
    fn a_peer_left_idle_is_dropped_on_the_next_post() {
        let sender = SshSender {
            factory: Weak::new(),
            https: Arc::new(NoSender),
            key: Arc::new(PrivateKey::from(
                russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&[7; 32]),
            )),
            peers: Mutex::new(HashMap::new()),
        };
        let long_ago = Instant::now()
            .checked_sub(PEER_IDLE + Duration::from_secs(1))
            .expect("an instant before the idle limit");
        sender
            .peers
            .lock()
            .unwrap()
            .insert("ssh://gone:22".to_string(), (PeerSlot::default(), long_ago));
        let first = sender.slot("ssh://live:22");
        let again = sender.slot("ssh://live:22");
        assert!(Arc::ptr_eq(&first, &again));
        let peers = sender.peers.lock().unwrap();
        assert!(!peers.contains_key("ssh://gone:22"));
        assert!(peers.contains_key("ssh://live:22"));
    }
}
