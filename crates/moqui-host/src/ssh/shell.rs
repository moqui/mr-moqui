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

//! The admin command shell of the built-in SSH server, for `ssh host command` and an interactive session. It replaces
//! the Groovy shell; it is not a web endpoint. Every command runs on a request worker in a context logged in as the
//! SSH user, so artifact authorization applies to each service and entity call the command makes. `service`, `entity`
//! and `js` run inside the Tools app root screen, as the Groovy shell did, so they need the user's authorization for
//! that app.
//!
//! Commands: `help`, `status`, `service <name> [json]`, `entity <name> [json conditions]`, `cache clear [name]`,
//! `js <code>` (a JavaScript expression or statements, with `ec`; `js` alone reads lines until a line with `.`), and
//! `exit`. Output is text, and JSON for values.

use std::collections::BTreeMap;
use std::sync::Weak;

use moqui_core::context::{Error, ExecutionContext, ExecutionContextFactory};
use moqui_lib::object::Value;

use super::on_pool;

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
  apps                          the applications in the terminal; quit them to come back here
  (exec) agent-apps ...         the applications for a program, a command at a time: agent-apps help
  exit                          end the session";

/// What a command printed, and the exit status for `exec`.
pub(super) struct Outcome {
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
pub(super) fn starts_block(line: &str) -> bool {
    line.trim() == "js"
}

/// Run one command line as `username`.
pub(super) async fn run(factory: &Weak<ExecutionContextFactory>, username: &str, line: &str) -> Outcome {
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
        "status" | "service" | "entity" | "cache" | "js" => {
            let command = command.to_string();
            let username = username.to_string();
            let ran = on_pool(factory, move |factory| {
                moqui_core::user::internal_login_user(&username)?;
                if matches!(command.as_str(), "service" | "entity" | "js") {
                    in_tools_app(|| execute(factory, &username, &command, &rest))
                } else {
                    execute(factory, &username, &command, &rest)
                }
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

/// Run `body` inside the Tools app root screen, as Java's Groovy shell ran within the Tools app: the services and
/// entities a command calls inherit the user's authorization for that screen (`TOOLS_APP`, granted to ADMIN in the seed
/// data), and a user without it is refused.
fn in_tools_app<T>(body: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    ExecutionContext::with_current(|ec| ec.push_checked("AT_XML_SCREEN", TOOLS_SCREEN, "view", true, false, None))
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
        other => Ok(Outcome::failed(format!("unknown command {other}"))),
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
