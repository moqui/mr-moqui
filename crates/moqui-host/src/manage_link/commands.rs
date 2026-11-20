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

//! The manager's calls on a managed instance (plans/Vault-Instances.md §4.4, Manager to instance), on the instance:
//! [`run`] takes a command off the link and answers it. Only the commands of [`CommandKind`] run, as the link's one
//! system identity ([`Managed`]): status; seal; restart; drain and undrain; put a secret (`moqui operator secret put`'s
//! rules, Sealed-Secrets S4); Component-Deploy's attested deploy; export the seal bundle; the surface report (S12, "not
//! built" until then); and a changed approver set, taken only when an approver the instance pinned signed it. A command
//! that is none of these is refused, and while sealed only status runs.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use moqui_core::context::ExecutionContextFactory;
use moqui_lib::sync::lock;

use super::bundle::Values;
use super::proto::{command, Command, CommandReply, InstanceStatus};
use super::state::{set_from_proto, LinkState};
use super::{CommandKind, LINK_VERSION};

/// What the link does on the instance: the server's own ([`ServerManaged`]), or a test's.
pub trait Managed: Send + Sync + 'static {
    /// Version, seal state, schema hash, cluster members and health.
    fn status(&self) -> InstanceStatus;
    /// Whether the instance is sealed now.
    fn sealed(&self) -> bool {
        self.status().seal_state != "unsealed"
    }
    fn seal(&self) -> Result<(), String>;
    fn restart(&self) -> Result<(), String>;
    fn drain(&self, drained: bool) -> Result<(), String>;
    fn put_secret(&self, name: &str, value: &str, force: bool) -> Result<(), String>;
    /// Component-Deploy's attested deploy; its report.
    fn deploy_component(&self, deploy: &super::proto::DeployComponentCommand) -> Result<String, String>;
    /// The seal's recovery bundle (`moqui operator export-seal`): wrapped values only.
    fn export_seal(&self) -> Result<String, String>;
    /// Keep the bootstrap bundle's secrets in the seal's store.
    fn store_bundle(&self, values: &Values) -> Result<(), String>;

    // The managed sealed start (plans/Vault-Instances.md §4.5, M6). The defaults are an instance with no seal of its
    // own (a test's), which proves nothing and holds no share.

    /// The key derived from the root key, which proves on a link that this instance is unsealed; none while sealed.
    fn unsealed_proof_key(&self) -> Option<moqui_vault::sign::SigningKey> {
        None
    }
    /// The method seal.json records, and its root key wrapped to the manager's transit when that is its method.
    fn seal_method(&self) -> (String, Option<Vec<u8>>) {
        (String::new(), None)
    }
    /// Whether `root` is this seal's root key: the manager's transit answered the right one.
    fn is_root(&self, _root: &[u8; 32]) -> bool {
        false
    }
    /// Sealed after an automatic unseal failed (the manager down, or holding the unseal for an approval).
    fn auto_unseal_failed(&self) -> bool {
        false
    }
    /// The automatic unseal again.
    fn retry_unseal(&self) -> Result<(), String> {
        Err("this instance has no seal".to_string())
    }
    /// Sealed and opened by recovery shares now: an operator method, or an automatic one that failed.
    fn takes_shares(&self) -> bool {
        false
    }
    /// One recovery share toward the unseal.
    fn enter_share(&self, _share: &str) -> Result<(), String> {
        Err("this instance has no seal".to_string())
    }
    /// Whether the recovery shares of an automatic init are held, unread.
    fn shares_waiting(&self) -> bool {
        false
    }
    /// The held shares, once.
    fn take_shares(&self) -> Result<Vec<zeroize::Zeroizing<String>>, String> {
        Err("no recovery shares are held".to_string())
    }
    /// A new set of recovery shares in place of the old one (a holder was replaced).
    fn new_shares(&self) -> Result<Vec<zeroize::Zeroizing<String>>, String> {
        Err("this instance has no seal".to_string())
    }
    /// The recovery shares' threshold and count.
    fn recovery_counts(&self) -> (u32, u32) {
        (0, 0)
    }
}

/// The link's state on the instance: what it pinned, where that is kept.
pub(crate) struct LinkContext {
    pub state: Mutex<LinkState>,
    pub file: Option<PathBuf>,
    /// This instance's link key fingerprint, which an approver set is signed over with its id.
    pub link_key: String,
}

impl LinkContext {
    pub(crate) fn new(state: LinkState, file: Option<PathBuf>, link_key: &str) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(state),
            file,
            link_key: link_key.to_string(),
        })
    }

    pub(crate) fn snapshot(&self) -> LinkState {
        lock(&self.state).clone()
    }

    /// Change the state and keep it.
    pub(crate) fn update(&self, change: impl FnOnce(&mut LinkState)) -> Result<(), String> {
        let mut state = lock(&self.state);
        change(&mut state);
        match &self.file {
            Some(file) => state.save(file),
            None => Ok(()),
        }
    }
}

fn reply(id: u64, answer: Result<String, String>, status: Option<InstanceStatus>) -> CommandReply {
    match answer {
        Ok(text) => CommandReply {
            id,
            ok: true,
            error: String::new(),
            status,
            text,
        },
        Err(error) => CommandReply {
            id,
            ok: false,
            error,
            status,
            text: String::new(),
        },
    }
}

/// Answer one command of the manager (see the module doc).
pub(crate) fn run(managed: &dyn Managed, link: &LinkContext, command: Command) -> CommandReply {
    let id = command.id;
    let Some(kind) = CommandKind::of(&command) else {
        tracing::warn!("Manager link: a command that is not one of the link's was refused");
        return reply(id, Err("not a command of the manager link: refused".to_string()), None);
    };
    if managed.sealed() && !kind.sealed_ok() {
        return reply(
            id,
            Err(format!(
                "sealed: only status is answered until the instance is unsealed, not {}",
                kind.name()
            )),
            Some(managed.status()),
        );
    }
    tracing::info!("Manager link: the manager runs {}", kind.name());
    let answer = match command.body {
        Some(command::Body::Status(_)) => Ok(String::new()),
        Some(command::Body::Seal(_)) => managed.seal().map(|()| String::new()),
        Some(command::Body::Restart(_)) => managed.restart().map(|()| String::new()),
        Some(command::Body::Drain(drain)) => managed.drain(drain.drain).map(|()| String::new()),
        Some(command::Body::PutSecret(put)) => {
            let value = zeroize::Zeroizing::new(put.value);
            managed.put_secret(&put.name, &value, put.force).map(|()| String::new())
        }
        Some(command::Body::DeployComponent(deploy)) => managed.deploy_component(&deploy),
        Some(command::Body::ExportSeal(_)) => managed.export_seal(),
        Some(command::Body::SurfaceReport(_)) => {
            Err("the surface report is not built yet (Sealed-Secrets S12)".to_string())
        }
        Some(command::Body::Approvers(set)) => take_approvers(link, &set).map(|()| String::new()),
        None => Err("not a command of the manager link: refused".to_string()),
    };
    if let Err(err) = &answer {
        tracing::warn!("Manager link: {} failed: {err}", kind.name());
    }
    reply(id, answer, Some(managed.status()))
}

/// A changed approver set (§4.4): taken only when signed by an approver this instance pinned, for this instance, at a
/// higher version; then pinned in its place.
fn take_approvers(link: &LinkContext, set: &super::proto::ApproverSet) -> Result<(), String> {
    let state = link.snapshot();
    let new = set_from_proto(set);
    if new.approvers.len() != set.approvers.len() {
        return Err("the approver set holds a key that is not an Ed25519 key".to_string());
    }
    new.verify_update(
        &state.app_instance_id,
        &link.link_key,
        &state.approver_set(),
        &set.signer_public,
        &set.signature,
    )
    .map_err(|err| format!("the approver set is refused: {err}"))?;
    link.update(|state| state.pin_approvers(&new))?;
    tracing::info!("Manager link: pinned approver set version {}", new.version);
    Ok(())
}

/// The server's own link: its seal's keeper and its factory, by its runtime directory.
pub(crate) struct ServerManaged {
    runtime_dir: PathBuf,
}

impl ServerManaged {
    pub(crate) fn new(runtime_dir: &Path) -> Self {
        Self {
            runtime_dir: runtime_dir.to_path_buf(),
        }
    }

    fn keeper(&self) -> Option<Arc<moqui_core::seal::Keeper>> {
        moqui_core::seal::keeper_of(&self.runtime_dir)
    }

    fn factory(&self) -> Option<Arc<ExecutionContextFactory>> {
        let canonical = std::fs::canonicalize(&self.runtime_dir).unwrap_or_else(|_| self.runtime_dir.clone());
        ExecutionContextFactory::running().filter(|factory| {
            let dir = factory.runtime_dir();
            dir == self.runtime_dir || std::fs::canonicalize(dir).is_ok_and(|dir| dir == canonical)
        })
    }

    fn keeper_or_err(&self) -> Result<Arc<moqui_core::seal::Keeper>, String> {
        self.keeper()
            .ok_or_else(|| "this server has no seal keeper (it booted without its gate)".to_string())
    }

    /// Run `work` in a context of the factory as the link's identity: no user, authorization off for the commands'
    /// own bookkeeping, which each command's code bounds.
    fn in_context<T>(&self, work: impl FnOnce(&ExecutionContextFactory) -> Result<T, String>) -> Result<T, String> {
        let factory = self
            .factory()
            .ok_or_else(|| "the server is not running yet".to_string())?;
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        work(&factory)
    }
}

/// SHA-256 of the entity model: every entity's full name and table, and each field's name, column, type and key flag,
/// in order. Two servers with the same hash have the same schema (a rollback across a change is refused, M3).
pub(crate) fn schema_hash(factory: &ExecutionContextFactory) -> String {
    use sha2::Digest as _;
    let Some(facade) = factory.entity() else {
        return String::new();
    };
    let mut hasher = sha2::Sha256::new();
    facade.with_model(|model| {
        let mut entities: Vec<_> = model.entities().iter().filter(|entity| !entity.is_view).collect();
        entities.sort_by(|a, b| a.full_name.cmp(&b.full_name));
        for entity in entities {
            hasher.update(entity.full_name.as_bytes());
            hasher.update([0]);
            hasher.update(entity.table_name.as_bytes());
            hasher.update([0]);
            for field in &entity.fields {
                hasher.update(field.name.as_bytes());
                hasher.update([0]);
                hasher.update(field.column_name.as_bytes());
                hasher.update([0]);
                hasher.update(field.field_type.as_bytes());
                hasher.update([u8::from(field.is_pk), 1]);
            }
            hasher.update([2]);
        }
    });
    moqui_vault::text::hex(&hasher.finalize())
}

impl Managed for ServerManaged {
    fn status(&self) -> InstanceStatus {
        let seal_state = self
            .keeper()
            .map_or("unsealed", |keeper| keeper.status().state.name())
            .to_string();
        let factory = (seal_state == "unsealed").then(|| self.factory()).flatten();
        let (schema_hash, cluster_members, drained) = match &factory {
            Some(factory) => {
                let members = factory
                    .entity()
                    .and_then(|facade| moqui_core::cluster::member_report(factory, &facade).ok())
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|member| member.live)
                    .map(|member| member.instance_id)
                    .collect();
                (schema_hash(factory), members, moqui_core::drain::is_draining(factory))
            }
            None => (String::new(), Vec::new(), false),
        };
        let health = match (seal_state.as_str(), &factory, drained) {
            ("unsealed", Some(_), false) => "ok",
            ("unsealed", Some(_), true) => "draining",
            ("unsealed", None, _) => "starting",
            _ => "sealed",
        };
        // from the keeper's published status: this runs while an unseal holds the keeper's state (a transit unwrap
        // that waits on this link), so it takes no lock of the keeper's
        let seal_method = self.keeper().map(|keeper| keeper.status().method).unwrap_or_default();
        InstanceStatus {
            binary_version: env!("CARGO_PKG_VERSION").to_string(),
            seal_state,
            schema_hash,
            cluster_members,
            health: health.to_string(),
            drained,
            first_start: false,
            link_version: LINK_VERSION,
            proof_public: Vec::new(),
            proof: Vec::new(),
            seal_method,
        }
    }

    fn unsealed_proof_key(&self) -> Option<moqui_vault::sign::SigningKey> {
        let seed = self.keeper()?.unsealed_proof_seed().ok()?;
        Some(moqui_vault::sign::SigningKey::from_seed(&seed))
    }

    fn seal_method(&self) -> (String, Option<Vec<u8>>) {
        let Some(file) = self.keeper().and_then(|keeper| keeper.seal_file()) else {
            return (String::new(), None);
        };
        let transit = (file.method.get("backend").and_then(serde_json::Value::as_str) == Some(super::unseal::BACKEND))
            .then(|| super::unseal::wrapped_root(&file.method).ok())
            .flatten();
        (file.method_kind().to_string(), transit)
    }

    fn is_root(&self, root: &[u8; 32]) -> bool {
        let (Some(file), Ok(root)) = (
            self.keeper().and_then(|keeper| keeper.seal_file()),
            moqui_core::seal::RootKey::from_slice(root),
        ) else {
            return false;
        };
        // the root id is public (seal.json and `moqui operator status` show it)
        root.id() == file.root_id
    }

    fn auto_unseal_failed(&self) -> bool {
        self.keeper().is_some_and(|keeper| {
            let status = keeper.status();
            status.state == moqui_core::seal::State::Sealed && status.reason.starts_with("auto-unseal failed")
        })
    }

    fn retry_unseal(&self) -> Result<(), String> {
        let keeper = self.keeper_or_err()?;
        keeper
            .unseal(moqui_core::seal::UnsealInput::Automatic)
            .map(|_| ())
            .map_err(|err| err.to_string())
    }

    fn takes_shares(&self) -> bool {
        self.keeper().is_some_and(|keeper| {
            let status = keeper.status();
            status.state != moqui_core::seal::State::Unsealed
                && status.recovery.is_some()
                && (matches!(status.method.as_str(), "passphrase" | "shamir")
                    || status.reason.starts_with("auto-unseal failed"))
        })
    }

    fn enter_share(&self, share: &str) -> Result<(), String> {
        let keeper = self.keeper_or_err()?;
        keeper
            .unseal(moqui_core::seal::UnsealInput::Recovery(zeroize::Zeroizing::new(
                share.trim().to_string(),
            )))
            .map(|_| ())
            .map_err(|err| err.to_string())
    }

    fn shares_waiting(&self) -> bool {
        self.keeper().is_some_and(|keeper| keeper.status().shares_waiting)
    }

    fn take_shares(&self) -> Result<Vec<zeroize::Zeroizing<String>>, String> {
        self.keeper_or_err()?.take_shares().map_err(|err| err.to_string())
    }

    fn new_shares(&self) -> Result<Vec<zeroize::Zeroizing<String>>, String> {
        self.keeper_or_err()?
            .new_recovery_shares()
            .map_err(|err| err.to_string())
    }

    fn recovery_counts(&self) -> (u32, u32) {
        self.keeper()
            .and_then(|keeper| keeper.status().recovery)
            .map_or((0, 0), |(threshold, shares)| (u32::from(threshold), u32::from(shares)))
    }

    fn seal(&self) -> Result<(), String> {
        self.keeper_or_err()?.seal();
        Ok(())
    }

    fn restart(&self) -> Result<(), String> {
        if moqui_core::devtools::restart::request_restart() {
            Ok(())
        } else {
            Err("this server cannot restart itself (no host that can shut down)".to_string())
        }
    }

    fn drain(&self, drained: bool) -> Result<(), String> {
        self.in_context(|factory| {
            let instance = moqui_core::drain::instance_id(factory);
            moqui_core::drain::set_drained(factory, &instance, drained, "the manager link", None)
                .map_err(|err| err.to_string())
        })
    }

    fn put_secret(&self, name: &str, value: &str, force: bool) -> Result<(), String> {
        let keeper = self.keeper_or_err()?;
        let facade = self.factory().and_then(|factory| factory.entity());
        moqui_core::seal::secrets::operator_put(&keeper, facade.as_ref(), name, value, force)
            .map_err(|err| err.to_string())?;
        tracing::info!(
            "Manager link: the secret {} was put",
            moqui_core::seal::secrets::canonical(name)
        );
        Ok(())
    }

    fn deploy_component(&self, deploy: &super::proto::DeployComponentCommand) -> Result<String, String> {
        // the attestation stands for the permission (Component-Deploy, "The gate"): the link runs no deploy without one
        self.in_context(|_| {
            let parameters: std::collections::BTreeMap<String, moqui_lib::object::Value> = [
                ("attestation", &deploy.attestation),
                ("signature", &deploy.signature),
                ("dataTypes", &deploy.data_types),
            ]
            .into_iter()
            .filter(|(_, value)| !value.is_empty())
            .map(|(name, value)| (name.to_string(), moqui_lib::object::Value::string(value)))
            .collect();
            let out =
                moqui_core::service::sync("org.moqui.dev.ComponentServices.deploy#AttestedComponent", &parameters)
                    .map_err(|err| err.to_string())?;
            let errors = moqui_core::context::ExecutionContext::with_current(|ec| ec.message().errors().join("\n"))
                .unwrap_or_default();
            if !errors.is_empty() {
                return Err(errors);
            }
            Ok(out
                .get("report")
                .map(moqui_lib::object::to_plain_string)
                .unwrap_or_default())
        })
    }

    fn export_seal(&self) -> Result<String, String> {
        self.keeper_or_err()?.export_bundle().map_err(|err| err.to_string())
    }

    fn store_bundle(&self, values: &Values) -> Result<(), String> {
        let keeper = self.keeper_or_err()?;
        store_bundle_in(
            &keeper,
            self.factory().and_then(|factory| factory.entity()).as_ref(),
            values,
        )
    }
}

/// Keep a bundle's secrets with the seal's bootstrap API in `seal.json`: only bootstrap secrets (what the boot needs
/// before its database opens), and never a crypt pass other than one kept already (as `moqui operator secret put`
/// refuses one without force). Each is kept or its refusal named; one that fails does not stop the rest.
pub(crate) fn store_bundle_in(
    keeper: &moqui_core::seal::Keeper,
    facade: Option<&moqui_core::entity::EntityFacade>,
    values: &Values,
) -> Result<(), String> {
    use moqui_core::seal::secrets;
    if !keeper.is_unsealed() {
        return Err("sealed: the bundle is kept once the instance is unsealed".to_string());
    }
    if keeper.is_ephemeral() {
        let message = "this server has no seal (its root key is in memory only): the bootstrap bundle cannot be kept; \
                       run `moqui operator init`";
        tracing::error!("Manager link: {message}");
        return Err(message.to_string());
    }
    let mut failed = Vec::new();
    for (name, value) in values {
        let name = secrets::canonical(name);
        let kept = if !secrets::is_bootstrap(&name) {
            Err(format!(
                "{name} is not a bootstrap secret, which is all a bundle carries"
            ))
        } else if name == secrets::CRYPT_PASS || name == secrets::CRYPT_PASS_OLD {
            match keeper.bootstrap_secret(&name) {
                Ok(Some(old)) if old.as_slice() != value.as_bytes() => Err(format!(
                    "{name} is kept already with another value, which a bundle does not replace"
                )),
                Ok(_) => secrets::write_to(keeper, facade, &name, value).map_err(|err| err.to_string()),
                Err(err) => Err(err.to_string()),
            }
        } else {
            secrets::write_to(keeper, facade, &name, value).map_err(|err| err.to_string())
        };
        match kept {
            Ok(()) => tracing::info!("Manager link: kept the bootstrap secret {name}"),
            Err(err) => {
                tracing::error!("Manager link: the bootstrap secret {name} was not kept: {err}");
                failed.push(name);
            }
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("not kept: {}", failed.join(", ")))
    }
}
