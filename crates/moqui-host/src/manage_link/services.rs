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

//! `org.moqui.dev.InstanceLinkServices` (plans/Vault-Instances.md §4.4): on a manager, an instance's link as System ›
//! Instances shows it, its pairing code, a command over the link or SSH as its environment's transport says, a signed
//! approver set and a revoke, each checked against the instance's environment and the caller (INSTANCE_ADMIN, and the
//! environment's approver group when it names one); on a managed instance, pairing with a manager by its code and the
//! link's state.

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;
use moqui_core::extension::RustServiceFn;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

moqui_core::extension::submit! {
    moqui_core::extension::Extension {
        register_services,
        ..moqui_core::extension::Extension::EMPTY
    }
}

fn register_services(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    moqui_core::rust_service!(map, get_instance_link)?;
    moqui_core::rust_service!(map, create_instance_pairing_code)?;
    moqui_core::rust_service!(map, call_instance_command)?;
    moqui_core::rust_service!(map, update_instance_approvers)?;
    moqui_core::rust_service!(map, revoke_instance_link)?;
    moqui_core::rust_service!(map, pair_manager)?;
    moqui_core::rust_service!(map, get_manager_link)?;
    Ok(())
}

type Params = BTreeMap<String, Value>;
type Out = Result<BTreeMap<String, Value>, LibError>;

fn text(params: &Params, name: &str) -> String {
    params
        .get(name)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn required(params: &Params, name: &str) -> Result<String, LibError> {
    let value = text(params, name);
    if value.is_empty() {
        return Err(LibError::new(format!("{name} is required")));
    }
    Ok(value)
}

fn factory() -> Result<Arc<ExecutionContextFactory>, LibError> {
    ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))
        .ok_or_else(|| LibError::new("a manager link service needs an execution context"))
}

fn out<const N: usize>(pairs: [(&str, Value); N]) -> BTreeMap<String, Value> {
    pairs
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect()
}

/// INSTANCE_ADMIN on a managed instance (its own pairing), or authorization off.
fn require_instance_admin() -> Result<(), LibError> {
    let disabled = ExecutionContext::with_current(ExecutionContext::authz_disabled).unwrap_or(false);
    if disabled || moqui_core::user::has_permission("INSTANCE_ADMIN").unwrap_or(false) {
        return Ok(());
    }
    Err(LibError::new("The manager link needs the INSTANCE_ADMIN permission"))
}

/// `pair#Manager`: pair this server with a manager by a code made in System › Instances, pinning the manager's key
/// `pin`, then dial it.
fn pair_manager(params: &Params, _facade: &EntityFacade) -> Out {
    require_instance_admin()?;
    let url = required(params, "url")?;
    let pin = required(params, "pin")?;
    let code = zeroize::Zeroizing::new(required(params, "code")?);
    let factory = factory()?;
    let state = super::dial::pair_by_code(factory.runtime_dir(), &url, &pin, &code).map_err(LibError::new)?;
    Ok(out([
        ("appInstanceId", Value::string(&state.app_instance_id)),
        ("environmentId", Value::string(&state.environment_id)),
        (
            "approverCount",
            Value::I64(i64::try_from(state.approvers.len()).unwrap_or(0)),
        ),
    ]))
}

/// `get#ManagerLink`: this server's link to its manager, when it is managed.
fn get_manager_link(_params: &Params, _facade: &EntityFacade) -> Out {
    require_instance_admin()?;
    let factory = factory()?;
    let Some(dialer) = super::dial::dialer_of(factory.runtime_dir()) else {
        return Ok(out([("phase", Value::string("not managed"))]));
    };
    let status = dialer.status();
    let state = dialer.state();
    Ok(out([
        ("phase", Value::string(status.phase.name())),
        ("generation", Value::I64(i64::try_from(status.generation).unwrap_or(0))),
        ("appInstanceId", Value::string(&state.app_instance_id)),
        ("managerUrl", Value::string(&state.url)),
        ("managerFingerprint", Value::string(&state.pin)),
        ("serving", Value::Bool(status.serving)),
        (
            "approversVersion",
            Value::I64(i64::try_from(state.approvers_version).unwrap_or(0)),
        ),
        ("message", Value::string(&status.message)),
    ]))
}

#[cfg(feature = "instances")]
mod manager {
    use moqui_lib::object::Value;

    use super::super::proto::command::Body;
    use super::super::proto::{DeployComponentCommand, DrainCommand, PutSecretCommand};
    use super::super::CommandKind;
    use super::*;

    fn server(factory: &ExecutionContextFactory) -> Result<Arc<super::super::accept::ManagerServer>, LibError> {
        super::super::accept::server_of(factory).ok_or_else(|| {
            LibError::new("this server takes no manager links (its HTTPS listener is off, or it is an edge node)")
        })
    }

    fn checked(facade: &EntityFacade, instance_id: &str) -> Result<moqui_core::entity::EntityValue, LibError> {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let row = moqui_instance::link::instance(facade, instance_id)?;
        moqui_instance::link::check_caller(facade, &row)?;
        Ok(row)
    }

    fn field(row: &moqui_core::entity::EntityValue, name: &str) -> String {
        row.get(name)
            .filter(|value| !is_empty(value))
            .map(to_plain_string)
            .unwrap_or_default()
    }

    pub(super) fn get_instance_link(params: &Params, facade: &EntityFacade) -> Out {
        let instance_id = required(params, "appInstanceId")?;
        let row = checked(facade, &instance_id)?;
        let factory = factory()?;
        let server = super::super::accept::server_of(&factory);
        let view = server
            .as_ref()
            .map(|server| server.view(&instance_id))
            .unwrap_or_default();
        Ok(out([
            ("linked", Value::Bool(view.linked)),
            ("generation", Value::I64(i64::try_from(view.generation).unwrap_or(0))),
            ("serving", Value::Bool(view.serving)),
            ("remote", Value::string(&view.remote)),
            ("sealState", Value::string(&view.status.seal_state)),
            ("health", Value::string(&view.status.health)),
            ("binaryVersion", Value::string(&view.status.binary_version)),
            ("schemaHash", Value::string(&view.status.schema_hash)),
            ("clusterMembers", Value::string(view.status.cluster_members.join(", "))),
            ("linkState", Value::string(field(&row, "linkState"))),
            ("linkKeyFingerprint", Value::string(field(&row, "linkKeyFingerprint"))),
            (
                "managerFingerprint",
                Value::string(
                    server
                        .as_ref()
                        .map(|server| server.fingerprint().to_string())
                        .unwrap_or_default(),
                ),
            ),
            (
                "managerUrl",
                Value::string(moqui_instance::link::manager().map(|(url, _)| url).unwrap_or_default()),
            ),
        ]))
    }

    pub(super) fn create_instance_pairing_code(params: &Params, facade: &EntityFacade) -> Out {
        let instance_id = required(params, "appInstanceId")?;
        let factory = factory()?;
        let server = server(&factory)?;
        let code = moqui_instance::link::make_pairing_code(facade, &instance_id)?;
        Ok(out([
            ("code", Value::string(code.as_str())),
            (
                "expiresMinutes",
                Value::I64(moqui_instance::kinds::PAIRING_TOKEN_MINUTES),
            ),
            (
                "managerUrl",
                Value::string(moqui_instance::link::manager().map(|(url, _)| url).unwrap_or_default()),
            ),
            ("managerFingerprint", Value::string(server.fingerprint())),
        ]))
    }

    /// The command a service call names, as the link carries it.
    fn body(kind: CommandKind, params: &Params) -> Result<Body, LibError> {
        Ok(match kind {
            CommandKind::Status => Body::Status(Default::default()),
            CommandKind::Seal => Body::Seal(Default::default()),
            CommandKind::Restart => Body::Restart(Default::default()),
            CommandKind::Drain => Body::Drain(DrainCommand { drain: true }),
            CommandKind::Undrain => Body::Drain(DrainCommand { drain: false }),
            CommandKind::PutSecret => Body::PutSecret(PutSecretCommand {
                name: required(params, "name")?,
                value: required(params, "value")?,
                force: matches!(text(params, "force").as_str(), "Y" | "true"),
            }),
            CommandKind::DeployComponent => Body::DeployComponent(DeployComponentCommand {
                attestation: required(params, "attestation")?,
                signature: required(params, "signature")?,
                data_types: text(params, "dataTypes"),
            }),
            CommandKind::ExportSeal => Body::ExportSeal(Default::default()),
            CommandKind::SurfaceReport => Body::SurfaceReport(Default::default()),
            CommandKind::Approvers => {
                return Err(LibError::new("a changed approver set goes by update#InstanceApprovers"))
            }
        })
    }

    pub(super) fn call_instance_command(params: &Params, facade: &EntityFacade) -> Out {
        let instance_id = required(params, "appInstanceId")?;
        let command = required(params, "command")?;
        let kind = CommandKind::parse(&command)
            .ok_or_else(|| LibError::new(format!("{command} is not a command of the manager link")))?;
        let row = checked(facade, &instance_id)?;
        let factory = factory()?;
        let transport = moqui_instance::link::transport(facade, &row)?;
        let linked = super::super::accept::server_of(&factory).is_some_and(|server| server.view(&instance_id).linked);
        if transport == "ssh" || (transport == "both" && !linked) {
            let ssh = match kind {
                CommandKind::Status => moqui_instance::ssh::SshCommand::Status,
                CommandKind::Seal => moqui_instance::ssh::SshCommand::Seal,
                CommandKind::PutSecret => moqui_instance::ssh::SshCommand::PutSecret {
                    name: required(params, "name")?,
                    value: zeroize::Zeroizing::new(required(params, "value")?),
                    force: matches!(text(params, "force").as_str(), "Y" | "true"),
                },
                other => {
                    return Err(LibError::new(format!(
                        "{} is not an operator command: over SSH only status, seal and put-secret; the link carries it",
                        other.name()
                    )))
                }
            };
            let registry = moqui_vault_server::lease::registry(&factory);
            let lease_id = Some(text(params, "leaseId")).filter(|lease| !lease.is_empty());
            let printed = moqui_instance::ssh::call(facade, &registry, &instance_id, &ssh, lease_id.as_deref())?;
            return Ok(out([
                ("ok", Value::Bool(true)),
                ("transport", Value::string("ssh")),
                ("text", Value::string(printed)),
            ]));
        }
        let body = body(kind, params)?;
        let reply = server(&factory)?.call(&instance_id, body).map_err(LibError::new)?;
        if !reply.ok {
            return Err(LibError::new(format!(
                "AppInstance {instance_id} refused {}: {}",
                kind.name(),
                reply.error
            )));
        }
        let status = reply.status.unwrap_or_default();
        Ok(out([
            ("ok", Value::Bool(true)),
            ("transport", Value::string("link")),
            ("text", Value::string(reply.text)),
            ("sealState", Value::string(status.seal_state)),
            ("health", Value::string(status.health)),
            ("binaryVersion", Value::string(status.binary_version)),
            ("schemaHash", Value::string(status.schema_hash)),
        ]))
    }

    pub(super) fn update_instance_approvers(params: &Params, facade: &EntityFacade) -> Out {
        use moqui_vault::approvers::{Approver, ApproverSet};
        let instance_id = required(params, "appInstanceId")?;
        let version: u64 = required(params, "version")?
            .parse()
            .map_err(|_| LibError::new("version is a whole number"))?;
        let Some(Value::List(list)) = params.get("approverList") else {
            return Err(LibError::new(
                "approverList is required: a list of userId and verifyPublic (hex)",
            ));
        };
        let mut approvers = Vec::new();
        for item in list {
            let Value::Map(item) = item else {
                return Err(LibError::new("each approver is a map of userId and verifyPublic"));
            };
            let user_id = item.get("userId").map(to_plain_string).unwrap_or_default();
            let key = moqui_vault::text::unhex(&item.get("verifyPublic").map(to_plain_string).unwrap_or_default())
                .ok()
                .and_then(|key| <[u8; 32]>::try_from(key.as_slice()).ok())
                .ok_or_else(|| LibError::new(format!("the key of approver {user_id} is not an Ed25519 key")))?;
            approvers.push(Approver {
                user_id,
                verify_public: key,
            });
        }
        let set = ApproverSet { version, approvers };
        let signer = moqui_vault::text::unhex(&required(params, "signerPublic")?)
            .map_err(|_| LibError::new("signerPublic is hex"))?;
        let signature =
            moqui_vault::text::unhex(&required(params, "signature")?).map_err(|_| LibError::new("signature is hex"))?;
        moqui_instance::link::check_approver_update(facade, &instance_id, &set, &signer, &signature)?;
        let factory = factory()?;
        let mut proto_set = super::super::state::set_to_proto(&set);
        proto_set.signer_public = signer;
        proto_set.signature = signature;
        let reply = server(&factory)?
            .call(&instance_id, Body::Approvers(proto_set))
            .map_err(LibError::new)?;
        if !reply.ok {
            return Err(LibError::new(format!(
                "AppInstance {instance_id} refused the approver set: {}",
                reply.error
            )));
        }
        moqui_instance::link::store_approvers(facade, &instance_id, &set)?;
        Ok(out([("version", Value::I64(i64::try_from(version).unwrap_or(0)))]))
    }

    pub(super) fn revoke_instance_link(params: &Params, facade: &EntityFacade) -> Out {
        let instance_id = required(params, "appInstanceId")?;
        moqui_instance::link::revoke(facade, &instance_id)?;
        let factory = factory()?;
        if let Some(server) = super::super::accept::server_of(&factory) {
            server.close(&instance_id);
        }
        Ok(BTreeMap::new())
    }
}

#[cfg(not(feature = "instances"))]
mod manager {
    use super::*;

    fn not_built() -> Out {
        Err(LibError::new(
            "managing instances is not built into this host (the instances feature: Maximal or Enterprise)",
        ))
    }

    pub(super) fn get_instance_link(_params: &Params, _facade: &EntityFacade) -> Out {
        not_built()
    }
    pub(super) fn create_instance_pairing_code(_params: &Params, _facade: &EntityFacade) -> Out {
        not_built()
    }
    pub(super) fn call_instance_command(_params: &Params, _facade: &EntityFacade) -> Out {
        not_built()
    }
    pub(super) fn update_instance_approvers(_params: &Params, _facade: &EntityFacade) -> Out {
        not_built()
    }
    pub(super) fn revoke_instance_link(_params: &Params, _facade: &EntityFacade) -> Out {
        not_built()
    }
}

/// `get#InstanceLink`: an instance's link as the manager sees it, and the manager's key.
fn get_instance_link(params: &Params, facade: &EntityFacade) -> Out {
    manager::get_instance_link(params, facade)
}

/// `create#InstancePairingCode`: a code for an instance the manager did not make, shown once.
fn create_instance_pairing_code(params: &Params, facade: &EntityFacade) -> Out {
    manager::create_instance_pairing_code(params, facade)
}

/// `call#InstanceCommand`: one of the link's commands on an instance, over its environment's transport.
fn call_instance_command(params: &Params, facade: &EntityFacade) -> Out {
    manager::call_instance_command(params, facade)
}

/// `update#InstanceApprovers`: a changed approver set, signed by an approver the instance pinned.
fn update_instance_approvers(params: &Params, facade: &EntityFacade) -> Out {
    manager::update_instance_approvers(params, facade)
}

/// `revoke#InstanceLink`: the instance's link key is no longer pinned, and its link closes.
fn revoke_instance_link(params: &Params, facade: &EntityFacade) -> Out {
    manager::revoke_instance_link(params, facade)
}
