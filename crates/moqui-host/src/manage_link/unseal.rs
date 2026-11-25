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

//! Transit by the manager, on the instance (plans/archive/Vault-Instances.md §4.5, M6): a `seal::Method` of the `transit` kind
//! that a managed instance's gate installs ([`register`]), so core never calls the host.
//!
//! - **Wrap** (at init): the root key wrapped to this instance's transit key on the manager, whose public half the
//!   manager gave in the link's Hello (`moqui_vault::managed::wrap_root`, the instance id as associated data). The
//!   manager sees no root key at init.
//! - **Unwrap** (at each start, and again when an unseal the manager refused or held is retried over the link): the
//!   wrapped root key goes up the link with a key made for this unseal alone; the manager opens it with this
//!   instance's transit key and seals it to that key. While the environment's approval policy holds it, the answer
//!   says so, the unwrap fails, and the instance stays sealed and asks again ([`super::dial`]).
//!
//! A seal.json whose transit is OpenBao's (`backend: openbao`), or a conf that names an OpenBao (`seal@transit-url`),
//! is the core's own transit's.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moqui_core::context::Error;
use moqui_core::seal::methods::{Method, MethodKind};
use moqui_core::seal::{RootKey, SealConf};
use moqui_vault::hpke::{PublicKey, SecretKey};
use moqui_vault::managed;
use moqui_vault::rng::OsRng;
use serde_json::{Map, Value};

use super::proto::UnsealRequest;

/// What this method writes as `backend`, so OpenBao's transit and the manager's each tell a seal of the other's.
pub const BACKEND: &str = "moqui-manager";

/// How long an unseal or a wrap waits for the link to be up.
const LINK_WAIT: Duration = Duration::from_secs(30);

/// The manager's transit of the instance whose runtime is `runtime_dir`.
pub struct ManagerTransit {
    runtime_dir: PathBuf,
}

/// Install the manager's transit for the managed instance of `runtime_dir` (its gate does, when it starts the dialer).
pub(crate) fn register(runtime_dir: &Path) {
    moqui_core::seal::methods::register(Arc::new(ManagerTransit {
        runtime_dir: runtime_dir.to_path_buf(),
    }));
}

fn error(text: impl Into<String>) -> Error {
    Error::new(text.into())
}

impl ManagerTransit {
    fn dialer(&self) -> Result<Arc<super::dial::Dialer>, Error> {
        super::dial::dialer_of(&self.runtime_dir).ok_or_else(|| {
            error("this instance has no link to its manager (manage@url), whose transit wraps its root key")
        })
    }
}

impl Method for ManagerTransit {
    fn kind(&self) -> MethodKind {
        MethodKind::Transit
    }

    fn wrap(&self, root: &RootKey, conf: &SealConf) -> Result<Map<String, Value>, Error> {
        if !conf.cloud.transit_url.is_empty() {
            // an OpenBao or Vault the conf names is its transit
            return moqui_core::seal::methods::transit::Transit.wrap(root, conf);
        }
        let dialer = self.dialer()?;
        let (instance_id, transit_public) = dialer.transit_key(LINK_WAIT).map_err(error)?;
        let public = PublicKey::from_bytes(&transit_public).map_err(|err| error(format!("the transit key: {err}")))?;
        let wrapped = managed::wrap_root(&public, &instance_id, root.expose(), &mut OsRng)
            .map_err(|err| error(format!("the root key was not wrapped to the manager's transit: {err}")))?;
        let mut metadata = Map::new();
        metadata.insert("backend".to_string(), Value::String(BACKEND.to_string()));
        metadata.insert("instance".to_string(), Value::String(instance_id));
        metadata.insert("manager".to_string(), Value::String(dialer.state().pin));
        metadata.insert(
            "transit_public".to_string(),
            Value::String(moqui_vault::text::hex(&transit_public)),
        );
        metadata.insert("wrapped".to_string(), Value::String(moqui_core::seal::encode(&wrapped)));
        tracing::info!("Manager link: the root key is wrapped to the manager's transit");
        Ok(metadata)
    }

    fn unwrap(&self, metadata: &Map<String, Value>, conf: &SealConf) -> Result<RootKey, Error> {
        if metadata.get("backend").and_then(Value::as_str) != Some(BACKEND) {
            return moqui_core::seal::methods::transit::Transit.unwrap(metadata, conf);
        }
        let instance_id = metadata
            .get("instance")
            .and_then(Value::as_str)
            .ok_or_else(|| error("seal.json's transit names no instance"))?;
        let wrapped = wrapped_root(metadata)?;
        let dialer = self.dialer()?;
        let paired = dialer.state().app_instance_id;
        if !paired.is_empty() && paired != instance_id {
            return Err(error(format!(
                "seal.json was wrapped for AppInstance {instance_id}, and this link is AppInstance {paired}'s"
            )));
        }
        let ephemeral = SecretKey::random(&mut OsRng);
        let ephemeral_public = ephemeral
            .public()
            .map_err(|err| error(format!("the unseal's key: {err}")))?;
        let reply = dialer
            .unseal_call(
                UnsealRequest {
                    wrapped_root: wrapped,
                    ephemeral_public: ephemeral_public.as_bytes().to_vec(),
                },
                LINK_WAIT,
            )
            .map_err(error)?;
        if !reply.waiting.is_empty() {
            return Err(error(format!("the manager holds the unseal: {}", reply.waiting)));
        }
        let root = managed::open_reply(&ephemeral, instance_id, &reply.sealed_root)
            .map_err(|_| error("the manager's answer does not open with this unseal's key"))?;
        tracing::info!("Manager link: unsealed by the manager's transit");
        RootKey::from_slice(root.as_slice())
    }
}

/// The root key a seal.json of this method holds wrapped to the manager's transit.
pub(crate) fn wrapped_root(metadata: &Map<String, Value>) -> Result<Vec<u8>, Error> {
    let text = metadata
        .get("wrapped")
        .and_then(Value::as_str)
        .ok_or_else(|| error("seal.json's transit has no wrapped root key"))?;
    moqui_core::seal::decode(text).map_err(|err| error(format!("seal.json's wrapped root key: {err}")))
}
