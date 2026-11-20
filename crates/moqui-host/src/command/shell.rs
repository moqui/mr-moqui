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

//! The commands of the `moqui>` command line, for `ssh host command`, SSH's interactive admin shell and the browser
//! terminal's command line on a dev instance (plans/Terminal-Fancy.md §6). It replaces the Groovy shell; it is not a web
//! endpoint. Every command runs on a request worker in a context logged in as the
//! SSH user, so artifact authorization applies to each service and entity call the command makes. `service`, `entity`
//! and `js` run inside the Tools app root screen, as the Groovy shell did, so they need the user's authorization for
//! that app.
//!
//! Commands: `help`, `status`, `service <name> [json]`, `entity <name> [json conditions]`, `cache clear [name]`,
//! `js <code>` (a JavaScript expression or statements, with `ec`; `js` alone reads lines until a line with `.`),
//! `leases [vaultId]` and `lease revoke <leaseId>` (plans/Vault-Instances.md §3.7: the shell lists and revokes vault
//! leases but never approves one, since the key passphrase would be typed on the server), and `exit`. Output is text,
//! and JSON for values.

use std::collections::BTreeMap;
use std::sync::Weak;

use moqui_core::context::{Authz, Error, ExecutionContext, ExecutionContextFactory, Tarpit};
use moqui_lib::object::Value;

use crate::worker::on_pool;

/// Up to this many rows from `entity`.
const ENTITY_LIMIT: i64 = 100;

/// The screen whose authorization `service`, `entity` and `js` run under: the root of the Tools app, which held Java's
/// Groovy shell.
const TOOLS_SCREEN: &str = "component://tools/screen/Tools.xml";

const HELP: &str = "Commands:
  help                          this list
  status                        the server, the instance, and you
  service <name> [json]         call a service with a JSON object of parameters
  entity <name> [json]          find up to 100 rows, with a JSON object of field conditions
  cache clear [name]            clear one cache, or all of them
  js <code>                     run JavaScript with ec; 'js' alone reads lines until a line with '.'
  leases [vaultId]              vault leases waiting for your approval and those you asked for, or a vault's
  lease revoke <leaseId>        revoke a vault lease (approve one in your own client: the browser or moqui vault)
  apps                          the applications in the terminal; quit them to come back here
  sh [profile]                  a program on the server's operating system (dev instances, TERMINAL_HOST)
  profiles                      the programs sh runs
  sessions                      your kept terminal sessions, from the browser and here
  attach <id>                   (SSH) be the window of one of your sessions; Ctrl+\\ detaches, and it goes on
  (exec) agent-apps ...         the applications for a program, a command at a time: agent-apps help
  exit                          end the session";

/// What a command printed, and the exit status for `exec`.
pub(crate) struct Outcome {
    pub text: String,
    pub status: u32,
    pub exit: bool,
}

impl Outcome {
    fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            status: 0,
            exit: false,
        }
    }

    fn failed(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            status: 1,
            exit: false,
        }
    }
}

/// True when `line` starts a multi-line `js` block.
pub(crate) fn starts_block(line: &str) -> bool {
    line.trim() == "js"
}

/// Run one command line as `username`, who is at `from` (the client's address, for the audit).
pub(crate) async fn run(factory: &Weak<ExecutionContextFactory>, username: &str, from: &str, line: &str) -> Outcome {
    let line = line.trim();
    let (command, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    let rest = rest.trim().to_string();
    match command {
        "" => Outcome::ok(""),
        "help" | "?" => Outcome::ok(HELP),
        "exit" | "quit" | "logout" => Outcome {
            text: String::new(),
            status: 0,
            exit: true,
        },
        "status" | "service" | "entity" | "cache" | "js" | "leases" | "lease" => {
            let command = command.to_string();
            let username = username.to_string();
            let from = from.to_string();
            let ran = on_pool(factory, move |factory| {
                moqui_core::user::internal_login_user(&username)?;
                let ran = if matches!(command.as_str(), "service" | "entity" | "js") {
                    in_tools_app(|| execute(factory, &username, &command, &rest))
                } else {
                    execute(factory, &username, &command, &rest)
                };
                if command != "status" {
                    let status = ran.as_ref().map_or(1, |outcome| outcome.status);
                    record(&username, &from, &audit_text(&command, &rest), status);
                }
                ran
            })
            .await;
            match ran {
                Ok(outcome) => outcome,
                Err(err) => Outcome::failed(format!("error: {}", err.message())),
            }
        }
        other => Outcome::failed(format!("unknown command {other}; try help")),
    }
}

/// What the audit keeps of a command line (SEC-FIL-62): the command word with the service or entity name, or the cache
/// command, and for `js` a SHA-256 digest of the code; never the code itself or a call's parameters, which can carry
/// secrets.
fn audit_text(command: &str, rest: &str) -> String {
    match command {
        "service" | "entity" => format!("{command} {}", rest.split_whitespace().next().unwrap_or_default()),
        "js" => {
            use sha2::Digest as _;
            let digest = sha2::Sha256::digest(rest.as_bytes());
            let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
            format!("js sha256:{hex}")
        }
        _ => format!("{command} {rest}").trim().to_string(),
    }
}

/// The audit of one command line that ran (SEC-FIL-62), as the terminal's sessions have: an INFO line, and a
/// `moqui.terminal.TerminalSession` row of kind `command` (who, from where, what, its exit status), as the signed-in
/// user on this thread. A row that cannot be written is logged and the command's answer stands.
fn record(username: &str, from: &str, text: &str, status: u32) {
    tracing::info!("admin shell of {username} from {from}: {text} (exit {status})");
    let written = ExecutionContext::with_current(|ec| -> Result<(), Error> {
        let _authz = ec.disable_authz();
        let facade = ec
            .factory()
            .entity()
            .ok_or_else(|| Error::new("the entity facade is not installed"))?;
        let mut row = facade.make_value("moqui.terminal.TerminalSession")?;
        row.set(
            "terminalSessionId",
            Value::string(&moqui_core::user::random_string(22)?),
        );
        if let Ok(Some(user_id)) = moqui_core::user::current_user_id() {
            row.set("userId", Value::string(&user_id));
        }
        row.set("username", Value::string(username));
        row.set("kind", Value::string("command"));
        row.set("commandLine", Value::string(text));
        row.set("clientIp", Value::string(from));
        let now = moqui_core::user::now_value();
        row.set("fromDate", now.clone());
        row.set("thruDate", now);
        row.set("exitStatus", Value::I64(i64::from(status)));
        row.create(&facade)?;
        Ok(())
    });
    if let Some(Err(err)) = written {
        tracing::error!(
            "admin shell of {username}: the audit row of '{text}' was not written: {}",
            err.message()
        );
    }
}

/// Run `body` inside the Tools app root screen, as Java's Groovy shell ran within the Tools app: the services and
/// entities a command calls inherit the user's authorization for that screen (`TOOLS_APP`, granted to ADMIN in the seed
/// data), and a user without it is refused.
fn in_tools_app<T>(body: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    ExecutionContext::with_current(|ec| {
        ec.push_checked(
            "AT_XML_SCREEN",
            TOOLS_SCREEN,
            "view",
            Authz::Required,
            Tarpit::Skip,
            None,
        )
    })
    .ok_or_else(|| Error::new("no execution context is entered"))??;
    let frame = ToolsFrame { pushed: true };
    let result = body();
    let popped = frame.pop();
    let value = result?;
    popped?;
    Ok(value)
}

/// The pushed Tools app frame: [`ToolsFrame::pop`] pops it and gives the error, and a drop that comes first (when
/// `body` panics) pops it too.
struct ToolsFrame {
    pushed: bool,
}

impl ToolsFrame {
    fn pop(mut self) -> Result<(), Error> {
        self.pushed = false;
        ExecutionContext::with_current(ExecutionContext::pop)
            .transpose()
            .map(|_| ())
    }
}

impl Drop for ToolsFrame {
    fn drop(&mut self) {
        if self.pushed {
            let _ = ExecutionContext::with_current(ExecutionContext::pop);
        }
    }
}

fn execute(factory: &ExecutionContextFactory, username: &str, command: &str, rest: &str) -> Result<Outcome, Error> {
    match command {
        "status" => Ok(Outcome::ok(format!(
            "MR-Moqui {}\ninstance {}\nuser {username}",
            env!("CARGO_PKG_VERSION"),
            moqui_core::cluster::instance_id(factory).unwrap_or_default(),
        ))),
        "service" => {
            let (name, json) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            if name.is_empty() {
                return Ok(Outcome::failed("usage: service <name> [json]"));
            }
            let parameters = json_map(json)?;
            let called = moqui_core::service::sync(name, &parameters);
            let errors = message_errors();
            match called {
                Ok(results) if errors.is_empty() => {
                    Ok(Outcome::ok(to_json(&Value::Map(results.into_iter().collect()))))
                }
                Ok(_) => Ok(Outcome::failed(errors)),
                Err(err) if errors.is_empty() => Ok(Outcome::failed(err.message().to_string())),
                Err(_) => Ok(Outcome::failed(errors)),
            }
        }
        "entity" => {
            let (name, json) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            if name.is_empty() {
                return Ok(Outcome::failed("usage: entity <name> [json]"));
            }
            let facade = factory
                .entity()
                .ok_or_else(|| Error::new("the entity facade is not installed"))?;
            let mut find = facade.find(name)?.limit(ENTITY_LIMIT);
            for (field, value) in json_map(json)? {
                find = find.eq(&field, value);
            }
            let rows = find.list()?;
            let list = rows
                .values()
                .iter()
                .map(|row| {
                    Value::Map(
                        row.fields()
                            .iter()
                            .map(|(name, value)| (name.to_string(), value.clone()))
                            .collect(),
                    )
                })
                .collect();
            Ok(Outcome::ok(to_json(&Value::List(list))))
        }
        "cache" => {
            let mut words = rest.split_whitespace();
            if words.next() != Some("clear") {
                return Ok(Outcome::failed("usage: cache clear [name]"));
            }
            match words.next() {
                Some(name) => {
                    let found = factory.cache().clear(name)?;
                    Ok(if found {
                        Outcome::ok(format!("cleared {name}"))
                    } else {
                        Outcome::failed(format!("no cache {name}"))
                    })
                }
                None => {
                    factory.cache().clear_all()?;
                    Ok(Outcome::ok("cleared all caches"))
                }
            }
        }
        "js" => {
            if rest.is_empty() {
                return Ok(Outcome::failed("usage: js <code>"));
            }
            // One line with no `;` is an expression and gives its value; anything else runs as a function body, where
            // `return` gives the value. Decided from the text, so nothing runs twice.
            let statements = rest.contains(['\n', ';']) || rest.split_whitespace().any(|word| word == "return");
            let value = if statements {
                moqui_core::script::eval_value(&format!("(function () {{\n{rest}\n}})()"))?
            } else {
                moqui_core::script::eval_value(rest)?
            };
            let errors = message_errors();
            if errors.is_empty() {
                Ok(Outcome::ok(to_json(&value)))
            } else {
                Ok(Outcome::failed(errors))
            }
        }
        "leases" => leases(rest),
        "lease" => lease(rest),
        other => Ok(Outcome::failed(format!("unknown command {other}"))),
    }
}

const LEASE_SERVICES: &str = "org.moqui.security.VaultServices";

/// A vault service's results, or its errors as the command's failure.
fn vault_service(
    verb_noun: &str,
    parameters: BTreeMap<String, Value>,
) -> Result<Result<BTreeMap<String, Value>, Outcome>, Error> {
    let called = moqui_core::service::sync(&format!("{LEASE_SERVICES}.{verb_noun}"), &parameters);
    let errors = message_errors();
    Ok(match called {
        Ok(results) if errors.is_empty() => Ok(results.into_iter().collect()),
        Ok(_) => Err(Outcome::failed(errors)),
        Err(err) if errors.is_empty() => Err(Outcome::failed(err.message().to_string())),
        Err(_) => Err(Outcome::failed(errors)),
    })
}

/// `leases [vaultId]`: one line a lease, never a key.
fn leases(rest: &str) -> Result<Outcome, Error> {
    let mut parameters = BTreeMap::new();
    if let Some(vault_id) = rest.split_whitespace().next() {
        parameters.insert("vaultId".to_string(), Value::string(vault_id));
    }
    let results = match vault_service("list#VaultLeases", parameters)? {
        Ok(results) => results,
        Err(failed) => return Ok(failed),
    };
    let text = |row: &Value, name: &str| match row {
        Value::Map(map) => map
            .get(name)
            .map(moqui_lib::object::to_plain_string)
            .unwrap_or_default(),
        _ => String::new(),
    };
    let lines: Vec<String> = match results.get("leaseList") {
        Some(Value::List(rows)) => rows
            .iter()
            .map(|row| {
                format!(
                    "{}  {}  {}  {}  {}",
                    text(row, "leaseId"),
                    text(row, "statusId"),
                    text(row, "vaultId"),
                    text(row, "operation"),
                    text(row, "bindings")
                )
            })
            .collect(),
        _ => Vec::new(),
    };
    Ok(Outcome::ok(if lines.is_empty() {
        "no leases".to_string()
    } else {
        lines.join("\n")
    }))
}

/// `lease revoke <leaseId>`; `lease approve` is refused: a lease is approved in the member's own client.
fn lease(rest: &str) -> Result<Outcome, Error> {
    let mut words = rest.split_whitespace();
    match (words.next(), words.next()) {
        (Some("revoke"), Some(lease_id)) => {
            let mut parameters = BTreeMap::new();
            parameters.insert("leaseId".to_string(), Value::string(lease_id));
            Ok(match vault_service("revoke#VaultLease", parameters)? {
                Ok(_) => Outcome::ok(format!("revoked {lease_id}")),
                Err(failed) => failed,
            })
        }
        (Some("approve"), _) => Ok(Outcome::failed(
            "a lease is approved in your own client (the browser, the apps, or moqui vault approve <leaseId> on your \
             machine), never on the server: your key passphrase would be typed here",
        )),
        _ => Ok(Outcome::failed("usage: lease revoke <leaseId>")),
    }
}

fn json_map(text: &str) -> Result<BTreeMap<String, Value>, Error> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(BTreeMap::new());
    }
    let parsed: serde_json::Value =
        serde_json::from_str(text).map_err(|err| Error::new(format!("parameters are not JSON: {err}")))?;
    match moqui_lib::json::from_json(&parsed) {
        Value::Map(map) => Ok(map.into_iter().collect()),
        _ => Err(Error::new("parameters must be a JSON object")),
    }
}

fn to_json(value: &Value) -> String {
    serde_json::to_string_pretty(&moqui_lib::json::to_json(value)).unwrap_or_default()
}

fn message_errors() -> String {
    ExecutionContext::with_current(|ctx| ctx.message().errors().join("\n")).unwrap_or_default()
}
