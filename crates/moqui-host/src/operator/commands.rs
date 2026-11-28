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

//! The operator socket's commands (plans/Sealed-Secrets.md §3.3): `status` for any peer; `init`, `unseal`, `seal`,
//! `rekey`, `rotate-root`, `export-seal` and `secret-put`, `secret-list` and `secret-rm` (S4), and `listeners` and
//! `listener` (plans/Runtime-Config.md §3.6, the ways back with no network listener) for root and the operator group
//! ([`super::peer`]). A request is a JSON object with `command` and that command's fields; the answer is
//! `{"ok": true, ...}` or `{"ok": false, "error"}`. Passphrases, shares and secret values are moved into zeroizing
//! strings as soon as they are parsed, and never logged or answered.

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_core::context::{Error, ExecutionContextFactory};
use moqui_core::listeners::{Actor, Change};
use moqui_core::seal::{secrets, Init, Keeper, MethodKind, Rekey, RotateRoot, UnsealInput};
use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::peer::Access;

/// A request as it arrives. No `Debug`: it carries passphrases and shares, which are moved out of the parsed JSON
/// into zeroizing strings, never copied.
struct Request {
    command: String,
    method: Option<String>,
    passphrase: Option<Zeroizing<String>>,
    share: Option<Zeroizing<String>>,
    recovery: bool,
    take_shares: bool,
    /// `secret put --force`: replace the crypt pass.
    force: bool,
    /// The recovery shares of `rekey` and `rotate-root`.
    shares: Vec<Zeroizing<String>>,
    /// A recovery bundle to restore (`init --from`).
    bundle: Option<String>,
    /// A secret's name and value (`secret put`, `secret rm`); a listener's name (`listener`).
    name: Option<String>,
    value: Option<Zeroizing<String>>,
    /// `listener`: on or off, a new port and host, SSH's access mode, the reason, whether to end its connections now,
    /// the confirmations, and whether to keep the change in `runtime/.env` (true unless `persist` is false).
    enabled: Option<bool>,
    port: Option<String>,
    host: Option<String>,
    ssh_access: Option<String>,
    reason: Option<String>,
    now: bool,
    confirm_public_http: bool,
    confirm_dependents: bool,
    persist: bool,
}

impl Request {
    fn parse(line: &str) -> Result<Self, String> {
        let mut value: Value = serde_json::from_str(line).map_err(|err| format!("not a request: {err}"))?;
        let mut text = |name: &str| match value.get_mut(name).map(Value::take) {
            Some(Value::String(text)) => Some(text),
            _ => None,
        };
        let command = text("command").ok_or("a request names its command")?;
        let method = text("method");
        let passphrase = text("passphrase").map(Zeroizing::new);
        let share = text("share").map(Zeroizing::new);
        let bundle = text("bundle");
        let name = text("name");
        let secret_value = text("value").map(Zeroizing::new);
        let port = text("port");
        let host = text("host");
        let ssh_access = text("access");
        let reason = text("reason");
        let shares = match value.get_mut("shares").map(Value::take) {
            Some(Value::Array(items)) => items
                .into_iter()
                .filter_map(|item| match item {
                    Value::String(text) => Some(Zeroizing::new(text)),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        Ok(Self {
            command,
            method,
            passphrase,
            share,
            recovery: value["recovery"] == Value::Bool(true),
            take_shares: value["take_shares"] == Value::Bool(true),
            force: value["force"] == Value::Bool(true),
            shares,
            bundle,
            name,
            value: secret_value,
            enabled: value["enabled"].as_bool(),
            port,
            host,
            ssh_access,
            reason,
            now: value["now"] == Value::Bool(true),
            confirm_public_http: value["confirm_public_http"] == Value::Bool(true),
            confirm_dependents: value["confirm_dependents"] == Value::Bool(true),
            persist: value["persist"] != Value::Bool(false),
        })
    }
}

/// Run one request line for a peer with `access` whose user id is `uid` (on the audit log of a listener change).
pub(crate) fn run(keeper: &Keeper, access: Access, uid: u32, line: &str) -> Value {
    let request = match Request::parse(line) {
        Ok(request) => request,
        Err(err) => return refusal(&err),
    };
    if request.command != "status" && access != Access::All {
        tracing::warn!(
            "Operator socket: {} refused to a peer that may run only status (the server's own user)",
            request.command
        );
        if request.command == "listener" {
            audit_refused(&request, uid, "statusOnly");
        }
        return refusal("refused: this user may run only status (root or the operator group may run the others)");
    }
    match command(keeper, uid, request) {
        Ok(answer) => answer,
        Err(err) => refusal(&err.to_string()),
    }
}

fn refusal(error: &str) -> Value {
    json!({"ok": false, "error": error})
}

/// A refused `listener` request on the audit log, as a refused change is, with the socket's peer
/// ([`moqui_core::listeners::audit_refused`]).
fn audit_refused(request: &Request, uid: u32, code: &str) {
    moqui_core::listeners::audit_refused(
        request.name.as_deref().unwrap_or_default(),
        &Actor::Operator { uid },
        request.enabled,
        code,
        request.reason.as_deref().unwrap_or_default(),
    );
}

fn command(keeper: &Keeper, uid: u32, request: Request) -> Result<Value, Error> {
    let status = |keeper: &Keeper| serde_json::to_value(keeper.status()).unwrap_or(Value::Null);
    match request.command.as_str() {
        "status" => Ok(json!({"ok": true, "status": status(keeper)})),
        "init" => {
            if request.take_shares {
                let shares = keeper.take_shares()?;
                return Ok(json!({"ok": true, "recovery_shares": texts(&shares), "status": status(keeper)}));
            }
            if let Some(bundle) = request.bundle {
                keeper.restore(&bundle)?;
                return Ok(json!({"ok": true, "status": status(keeper)}));
            }
            let method = match request.method.filter(|method| !method.is_empty()) {
                Some(method) => method,
                None if !keeper.conf().method.is_empty() => keeper.conf().method,
                None => {
                    return Err(Error::new(
                        "name the unseal method: init --method tpm|kms|transit|passphrase|shamir|dev",
                    ))
                }
            };
            let issued = keeper.init(Init {
                method: MethodKind::parse(&method)?,
                passphrase: request.passphrase,
            })?;
            tracing::info!("Operator: the seal was made ({method})");
            Ok(json!({
                "ok": true,
                "recovery_shares": texts(&issued.recovery_shares),
                "method_shares": texts(&issued.method_shares),
                "status": status(keeper),
            }))
        }
        "unseal" => {
            let input = match (request.passphrase, request.share) {
                (Some(passphrase), _) => UnsealInput::Passphrase(passphrase),
                (None, Some(share)) if request.recovery => UnsealInput::Recovery(share),
                (None, Some(share)) => UnsealInput::Share(share),
                (None, None) => UnsealInput::Automatic,
            };
            let status = keeper.unseal(input)?;
            Ok(json!({"ok": true, "status": status}))
        }
        "seal" => {
            keeper.seal();
            Ok(json!({"ok": true, "status": status(keeper)}))
        }
        "rekey" => {
            let method = request.method.filter(|method| !method.is_empty()).ok_or_else(|| {
                Error::new("name the new unseal method: rekey --method tpm|kms|transit|passphrase|shamir|dev")
            })?;
            let issued = keeper.rekey(Rekey {
                shares: request.shares,
                method: MethodKind::parse(&method)?,
                passphrase: request.passphrase,
            })?;
            Ok(json!({"ok": true, "method_shares": texts(&issued.method_shares), "status": status(keeper)}))
        }
        "rotate-root" => {
            let issued = keeper.rotate_root(RotateRoot {
                shares: request.shares,
                passphrase: request.passphrase,
            })?;
            Ok(json!({"ok": true, "method_shares": texts(&issued.method_shares), "status": status(keeper)}))
        }
        "export-seal" => {
            let bundle = keeper.export_bundle()?;
            // the embedded cluster's password is made at the database's first open, after init: a bundle made before it
            // has none, and restores a seal that cannot reach that cluster
            let has_password = keeper
                .seal_file()
                .is_some_and(|seal| seal.bootstrap.contains_key(secrets::DATASOURCE_PASSWORD));
            let warning = (!has_password).then_some(
                "the bundle holds no datasource password: on the embedded cluster, export it again once the server \
                 serves (its password is made at the database's first open)",
            );
            Ok(json!({"ok": true, "bundle": bundle, "warning": warning}))
        }
        "secret-put" => {
            let name = secret_name(request.name)?;
            let value = request
                .value
                .filter(|value| !value.is_empty())
                .ok_or_else(|| Error::new("secret put needs a value"))?;
            secrets::operator_put(keeper, database(keeper).as_ref(), &name, &value, request.force)?;
            tracing::info!("Operator: the secret {name} was put");
            Ok(json!({"ok": true, "name": secrets::canonical(&name)}))
        }
        "secret-list" => {
            let listed = secrets::list_from(keeper, database(keeper).as_ref())?;
            Ok(json!({"ok": true, "secrets": listed}))
        }
        "secret-rm" => {
            let name = secret_name(request.name)?;
            let removed = secrets::operator_remove(keeper, database(keeper).as_ref(), &name)?;
            if removed {
                tracing::info!("Operator: the secret {name} was removed");
            }
            Ok(json!({"ok": true, "removed": removed}))
        }
        "listeners" => {
            let factory = serving(keeper)?;
            Ok(json!({"ok": true, "listeners": moqui_core::listeners::rows(&factory, None)}))
        }
        "listener" => listener(keeper, uid, request),
        other => Err(Error::new(format!("unknown operator command {other:?}"))),
    }
}

/// `listener`: one listener turned on or off, moved or set, as the listener services do it
/// ([`moqui_core::listeners::change`]), by the operator socket: its own access is the authorization, and as it needs
/// no network listener it is itself a way back, so the lockout guard does not apply; the public HTTP and dependents
/// confirmations do, and turning one off needs a reason. A refusal answers `reason_code` and, for a guard, `confirm`.
fn listener(keeper: &Keeper, uid: u32, request: Request) -> Result<Value, Error> {
    let refused = |code: &str, error: Error| {
        audit_refused(&request, uid, code);
        Err(error)
    };
    let factory = match serving(keeper) {
        Ok(factory) => factory,
        Err(err) => return refused("notServing", err),
    };
    let Some(name) = request.name.clone().filter(|name| !name.trim().is_empty()) else {
        return refused("invalid", Error::new("name the listener: listener <name> on|off"));
    };
    let Some(enabled) = request.enabled else {
        return refused(
            "invalid",
            Error::new("say whether to turn it on or off: listener <name> on|off"),
        );
    };
    let settings: BTreeMap<String, String> = request
        .ssh_access
        .into_iter()
        .map(|access| ("access".to_string(), access))
        .collect();
    let change = Change {
        name,
        enabled,
        port: request.port,
        host: request.host,
        settings,
        reason: request.reason.unwrap_or_default(),
        persist: request.persist,
        end_now: request.now,
        confirm_lockout: false,
        confirm_public_http: request.confirm_public_http,
        confirm_dependents: request.confirm_dependents,
    };
    match moqui_core::listeners::change(&factory, &change, &Actor::Operator { uid }) {
        Ok(answer) => Ok(json!({"ok": true, "answer": answer.to_json()})),
        Err(refused) => {
            let flag = refused.kind.confirmation().map(confirm_flag);
            let error = match flag {
                Some(flag) => format!("{}. From the operator socket: add {flag}", refused.message),
                None => refused.message,
            };
            Ok(json!({"ok": false, "error": error, "reason_code": refused.kind.code(), "confirm": flag}))
        }
    }
}

/// The client's flag of a confirmation the services name by parameter.
fn confirm_flag(parameter: &str) -> &'static str {
    match parameter {
        "confirmPublicHttp" => "--confirm-public-http",
        "confirmDependents" => "--confirm-dependents",
        _ => "",
    }
}

/// The server this keeper's runtime runs, once it serves: its listeners are what `listeners` and `listener` act on.
fn serving(keeper: &Keeper) -> Result<Arc<ExecutionContextFactory>, Error> {
    keeper
        .path()
        .parent()
        .and_then(|db| db.parent())
        .and_then(|runtime| ExecutionContextFactory::running().filter(|factory| factory.runtime_dir() == runtime))
        .ok_or_else(|| {
            Error::new(
                "this server does not serve yet (it is sealed or starting): its listeners open once it is unsealed, \
                 as the conf and runtime/.env say",
            )
        })
}

/// The name a `secret` command gives.
fn secret_name(name: Option<String>) -> Result<String, Error> {
    name.map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| Error::new("name the secret: secret put|rm <name>"))
}

/// The entity facade of the server this keeper's runtime runs, once its database is open: where a SecretValue is kept.
fn database(keeper: &Keeper) -> Option<moqui_core::entity::EntityFacade> {
    serving(keeper).ok().and_then(|factory| factory.entity())
}

/// The shares as the texts an answer carries.
fn texts(shares: &[Zeroizing<String>]) -> Vec<&str> {
    shares.iter().map(|share| share.as_str()).collect()
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
