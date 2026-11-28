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

//! The `thread` kind (plans/Terminal-Fancy2.md §4.7, D9, D10): a host session in every way (a pty, the ring, the
//! holder, the tracker, viewers, the audit) that is also opened, attached and listed by its agent conversation. Who may
//! open one, in which directory, as which account and with what environment is the policy a crate gives
//! ([`moqui_core::terminal`]); with none, `thread` is not available. An opening of a live terminal of the same user,
//! conversation and client id answers it, so opening is idempotent. Its output is kept as a bounded history (TF5's
//! history mode), and started again after its program ended it shows that history first. Clear empties both.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::terminal::{ThreadOpening, ThreadPlan};
use moqui_lib::object::Value;
use moqui_lib::sync::lock;

use super::host::{Launch, Profile, RunAs};
use super::record::Keep;
use super::registry::{HostSpec, Registry};
use super::session::{Kind, Session, ThreadMeta};

/// The longest conversation and client terminal id.
const ID_MAX: usize = 128;
const LABEL_MAX: usize = 200;
/// The most names an opening's `env` has, and the longest value.
const ENV_NAMES_MAX: usize = 128;
const ENV_VALUE_MAX: usize = 8192;

/// What an opening of a thread terminal asks for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ThreadAsk {
    pub conversation_id: String,
    pub client_terminal_id: String,
    pub label: String,
    pub work_root_id: String,
    pub provider_instance_id: String,
    pub stop_on_settle: bool,
    pub directory: String,
    pub env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
}

fn text(parameters: &BTreeMap<String, Value>, key: &str) -> String {
    super::services::text(parameters, key)
}

impl ThreadAsk {
    /// From `open#TerminalSession`'s parameters, sized `cols` by `rows`; refused with why.
    pub fn of(parameters: &BTreeMap<String, Value>, cols: u16, rows: u16) -> Result<Self, String> {
        let ask = Self {
            conversation_id: text(parameters, "conversationId"),
            client_terminal_id: text(parameters, "clientTerminalId"),
            label: text(parameters, "label"),
            work_root_id: text(parameters, "workRootId"),
            provider_instance_id: text(parameters, "providerInstanceId"),
            stop_on_settle: matches!(text(parameters, "stopOnSettle").as_str(), "true" | "Y"),
            directory: text(parameters, "directory"),
            env: env_of(parameters.get("env"))?,
            cols,
            rows,
        };
        for (name, value) in [
            ("conversationId", &ask.conversation_id),
            ("clientTerminalId", &ask.client_terminal_id),
        ] {
            if value.is_empty() || value.chars().count() > ID_MAX {
                return Err(format!("A thread terminal has a {name} of 1 to {ID_MAX} characters"));
            }
        }
        if ask.label.chars().count() > LABEL_MAX {
            return Err(format!("A thread terminal's label is at most {LABEL_MAX} characters"));
        }
        Ok(ask)
    }
}

/// An opening's `env`: a map of names to text, at most [`ENV_NAMES_MAX`] names and values of [`ENV_VALUE_MAX`]
/// characters, with no NUL; the names of the Moqui conf's environment (`MOQUI_*`, `moqui_conf__*`) left out, as for any
/// host session.
pub(crate) fn env_of(value: Option<&Value>) -> Result<Vec<(String, String)>, String> {
    let Some(value) = value.filter(|value| !moqui_lib::object::is_empty(value)) else {
        return Ok(Vec::new());
    };
    let json = match moqui_lib::json::to_json(value) {
        serde_json::Value::String(text) => {
            serde_json::from_str(&text).map_err(|_| "env is a map of names to text".to_string())?
        }
        other => other,
    };
    let serde_json::Value::Object(map) = json else {
        return Err("env is a map of names to text".to_string());
    };
    if map.len() > ENV_NAMES_MAX {
        return Err(format!("env has at most {ENV_NAMES_MAX} names"));
    }
    let mut env = Vec::new();
    for (name, value) in map {
        let value = match value {
            serde_json::Value::String(text) => text,
            other => other.to_string(),
        };
        if name.is_empty()
            || name.contains(['=', '\0'])
            || value.contains('\0')
            || value.chars().count() > ENV_VALUE_MAX
        {
            return Err(format!(
                "env's {name:?} is not a name with a value of at most {ENV_VALUE_MAX} characters"
            ));
        }
        if name.to_ascii_uppercase().starts_with("MOQUI_") {
            continue;
        }
        env.push((name, value));
    }
    Ok(env)
}

/// The openings of thread terminals of this server, by session id, kept for a restart.
#[derive(Default)]
struct Openings(Mutex<HashMap<String, (String, ThreadAsk)>>);

fn openings(factory: &ExecutionContextFactory) -> Arc<Openings> {
    factory.extension(|_| Openings::default())
}

/// The live thread terminal of `username` for `ask`'s conversation and client id, if one runs.
pub(crate) fn find_open(registry: &Registry, username: &str, ask: &ThreadAsk) -> Option<Arc<Session>> {
    registry.list(username).into_iter().find(|session| {
        session.kind == Kind::Thread
            && session.info().ended.is_none()
            && session.thread().is_some_and(|thread| {
                thread.conversation_id == ask.conversation_id && thread.client_terminal_id == ask.client_terminal_id
            })
    })
}

/// The policies' plan for `opening`: the first that allows it; `Err` with a reason code and why when none does.
fn plan(factory: &ExecutionContextFactory, opening: &ThreadOpening) -> Result<ThreadPlan, (&'static str, String)> {
    let policies = moqui_core::terminal::policies(factory);
    if policies.is_empty() {
        return Err((
            super::services::NOT_AVAILABLE,
            "That terminal is not available here".to_string(),
        ));
    }
    let mut refusal = String::new();
    for policy in policies {
        match policy.plan(opening) {
            Ok(plan) => return Ok(plan),
            Err(why) if refusal.is_empty() => refusal = why,
            Err(_) => {}
        }
    }
    Err(("notAllowed", refusal))
}

/// Where a thread terminal `id` of `username` keeps its history.
fn history_keep(factory: &ExecutionContextFactory, username: &str, id: &str) -> Keep {
    let safe: String = username
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '@') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    Keep::History {
        dir: factory.runtime_dir().join("terminal").join("history").join(safe),
        stem: format!("{id}-history"),
    }
}

/// Open a thread terminal for `username` as `ask` says, under the policy: as session `id` when given (a restart),
/// showing `prelude` first. The caller checked the place (`access::check`).
pub(crate) fn open(
    factory: &Arc<ExecutionContextFactory>,
    registry: &Arc<Registry>,
    username: &str,
    ask: &ThreadAsk,
    facts: &super::access::Facts,
    id: Option<String>,
    prelude: Vec<u8>,
) -> Result<Arc<Session>, (&'static str, String)> {
    let opening = ThreadOpening {
        username: username.to_string(),
        conversation_id: ask.conversation_id.clone(),
        client_terminal_id: ask.client_terminal_id.clone(),
        directory: ask.directory.clone(),
        work_root_id: ask.work_root_id.clone(),
        provider_instance_id: ask.provider_instance_id.clone(),
    };
    let plan = plan(factory, &opening)?;
    if plan.needs_dev_instance && !moqui_core::devtools::is_dev_instance() {
        return Err((
            super::services::NOT_AVAILABLE,
            "That terminal is not available here".to_string(),
        ));
    }
    let mut env = plan.env.clone();
    env.extend(ask.env.iter().cloned());
    let (command, args) = match plan.argv.split_first() {
        Some((command, args)) => (Some(command.clone()), args.to_vec()),
        None => (None, Vec::new()),
    };
    let shell = command.is_none();
    let profile = Profile {
        name: "thread".to_string(),
        title: if ask.label.is_empty() {
            "Thread terminal".to_string()
        } else {
            ask.label.clone()
        },
        command,
        args,
        login: shell,
        integration: shell,
        env,
        ..Profile::default()
    };
    let run_as = plan.run_as.as_ref().map(|account| RunAs {
        sudo: Some(factory.conf_setting("terminal", "run-as-sudo"))
            .map(|sudo| sudo.trim().to_string())
            .filter(|sudo| !sudo.is_empty())
            .unwrap_or_else(|| "sudo".to_string()),
        account: account.clone(),
    });
    let id = match id {
        Some(id) => id,
        None => super::registry::new_id().map_err(|err| ("limit", err))?,
    };
    let directory: PathBuf = plan.directory.clone();
    let launch = Launch::of(
        factory,
        &profile,
        &id,
        username,
        Some(&directory.to_string_lossy()),
        false,
        (ask.cols, ask.rows),
        run_as,
    );
    let keep = history_keep(factory, username, &id);
    let Keep::History { dir, stem } = &keep else {
        unreachable!("a history is kept as one")
    };
    let meta = ThreadMeta {
        conversation_id: ask.conversation_id.clone(),
        client_terminal_id: ask.client_terminal_id.clone(),
        label: ask.label.clone(),
        work_root_id: ask.work_root_id.clone(),
        provider_instance_id: ask.provider_instance_id.clone(),
        stop_on_settle: ask.stop_on_settle,
        history_dir: dir.to_string_lossy().into_owned(),
        history_stem: stem.clone(),
    };
    let command_line = launch.command_line();
    let directory_text = launch.directory.to_string_lossy().into_owned();
    let spec = HostSpec {
        id: id.clone(),
        username: username.to_string(),
        profile: profile.clone(),
        title: profile.title.clone(),
        launch,
        on_end: Some(super::audit::on_end(Arc::downgrade(factory), id.clone())),
        kind: Kind::Thread,
        record: Some(keep.clone()),
        prelude,
        thread: Some(meta.clone()),
    };
    let session = registry.open_host(factory, spec).map_err(|err| ("limit", err))?;
    super::audit::record_open(&super::audit::Opened {
        id: &id,
        username,
        kind: Kind::Thread,
        profile: &profile.name,
        command_line: &command_line,
        directory: &directory_text,
        facts,
    });
    super::audit::record_thread(&id, &meta);
    lock(&openings(factory).0).insert(id, (username.to_string(), ask.clone()));
    Ok(session)
}

/// Start the thread terminal `id` of `username` again after its program ended: under the same id, with its history
/// first. A running one is answered as it is.
pub(crate) fn restart(
    factory: &Arc<ExecutionContextFactory>,
    registry: &Arc<Registry>,
    username: &str,
    id: &str,
    facts: &super::access::Facts,
) -> Result<Arc<Session>, (&'static str, String)> {
    if let Some(running) = registry
        .find(id, username)
        .filter(|session| session.info().ended.is_none())
    {
        return Ok(running);
    }
    let saved = lock(&openings(factory).0).get(id).cloned();
    let Some((owner, ask)) = saved.filter(|(owner, _)| owner == username) else {
        return Err(("notFound", "No such thread terminal to start again".to_string()));
    };
    let prelude = match history_keep(factory, &owner, id) {
        Keep::History { dir, stem } => super::record::history_output(&dir, &stem),
        Keep::File { .. } => Vec::new(),
    };
    tracing::info!(
        "thread terminal {id} of {owner} started again, its history first ({} bytes)",
        prelude.len()
    );
    open(factory, registry, &owner, &ask, facts, Some(id.to_string()), prelude)
}

/// Empty a thread terminal's kept output, screen and history: its windows clear theirs.
pub(crate) fn clear(factory: &Arc<ExecutionContextFactory>, session: &Arc<Session>) -> Result<(), String> {
    session.clear();
    let keep = history_keep(factory, &session.username, &session.id);
    if let Keep::History { dir, stem } = &keep {
        super::record::remove_history(dir, stem);
    }
    let (factory, session) = (Arc::clone(factory), Arc::clone(session));
    let restarted = crate::edge::block_on(async move {
        if session.held {
            super::held::Holder::of(&factory)
                .record(&session.id, Some(&keep))
                .await
                .map_err(|err| err.to_string())
        } else {
            session.record(Some(keep)).await
        }
    });
    restarted.map_err(|err| err.message().to_string())?
}

/// A thread terminal closed by its user: nothing to start again, and its history goes.
pub(crate) fn forget(factory: &ExecutionContextFactory, session: &Session) {
    lock(&openings(factory).0).remove(&session.id);
    if let Keep::History { dir, stem } = history_keep(factory, &session.username, &session.id) {
        super::record::remove_history(&dir, &stem);
    }
}

#[cfg(test)]
mod tests {
    use super::{env_of, ThreadAsk};
    use moqui_lib::object::Value;
    use std::collections::BTreeMap;

    #[test]
    fn an_opening_is_bounded_and_its_env_keeps_out_the_confs_names() {
        let env = env_of(Some(&Value::string(
            r#"{"EDITOR":"vi","MOQUI_CONF":"x","moqui_conf__a__b":"y"}"#,
        )))
        .unwrap();
        assert_eq!(env, [("EDITOR".to_string(), "vi".to_string())]);
        assert!(env_of(Some(&Value::string("[1]"))).is_err());
        assert!(env_of(Some(&Value::string(format!(r#"{{"A":"{}"}}"#, "x".repeat(8193))))).is_err());
        let too_many: String = (0..129).map(|n| format!(r#""N{n}":"v""#)).collect::<Vec<_>>().join(",");
        assert!(env_of(Some(&Value::string(format!("{{{too_many}}}")))).is_err());
        let mut parameters = BTreeMap::from([
            ("conversationId".to_string(), Value::string("c1")),
            ("clientTerminalId".to_string(), Value::string("t1")),
        ]);
        assert!(ThreadAsk::of(&parameters, 80, 24).is_ok());
        parameters.insert("clientTerminalId".to_string(), Value::string("x".repeat(129)));
        assert!(ThreadAsk::of(&parameters, 80, 24).is_err());
        parameters.remove("clientTerminalId");
        assert!(ThreadAsk::of(&parameters, 80, 24).is_err(), "needs its client id");
    }
}
