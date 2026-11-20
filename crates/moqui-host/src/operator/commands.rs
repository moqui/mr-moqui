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
//! `rekey`, `rotate-root` and `export-seal` for root and the operator group ([`super::peer`]). A request is a JSON
//! object with `command` and that command's fields; the answer is `{"ok": true, ...}` or `{"ok": false, "error"}`.
//! Passphrases and shares are moved into zeroizing strings as soon as they are parsed, and never logged.

use moqui_core::context::Error;
use moqui_core::seal::{Init, Keeper, MethodKind, Rekey, RotateRoot, UnsealInput};
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
    /// The recovery shares of `rekey` and `rotate-root`.
    shares: Vec<Zeroizing<String>>,
    /// A recovery bundle to restore (`init --from`).
    bundle: Option<String>,
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
            shares,
            bundle,
        })
    }
}

/// Run one request line for a peer with `access`.
pub(crate) fn run(keeper: &Keeper, access: Access, line: &str) -> Value {
    let request = match Request::parse(line) {
        Ok(request) => request,
        Err(err) => return refusal(&err),
    };
    if request.command != "status" && access != Access::All {
        tracing::warn!(
            "Operator socket: {} refused to a peer that may run only status (the server's own user)",
            request.command
        );
        return refusal("refused: this user may run only status (root or the operator group may run the others)");
    }
    match command(keeper, request) {
        Ok(answer) => answer,
        Err(err) => refusal(&err.to_string()),
    }
}

fn refusal(error: &str) -> Value {
    json!({"ok": false, "error": error})
}

fn command(keeper: &Keeper, request: Request) -> Result<Value, Error> {
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
                        "name the unseal method: init --method tpm|passphrase|shamir|dev",
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
            let method = request
                .method
                .filter(|method| !method.is_empty())
                .ok_or_else(|| Error::new("name the new unseal method: rekey --method tpm|passphrase|shamir|dev"))?;
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
        "export-seal" => Ok(json!({"ok": true, "bundle": keeper.export_bundle()?})),
        other => Err(Error::new(format!("unknown operator command {other:?}"))),
    }
}

/// The shares as the texts an answer carries.
fn texts(shares: &[Zeroizing<String>]) -> Vec<&str> {
    shares.iter().map(|share| share.as_str()).collect()
}
