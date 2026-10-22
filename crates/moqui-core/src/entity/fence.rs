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

//! The read fence (plans/LLM-A2A-Assist.md 4.6): a thread-scoped guard, like sim mode, under which nothing may change
//! data or reach outside. Every READ tool and every agent screen render runs fenced, so READ is a claim checked at
//! runtime: a mislabeled service fails instead of writing.
//!
//! [`cache::gate_write`](super::cache::gate_write), the single write path, refuses an entity write outside
//! [`ALLOWED_ENTITIES`] (the bookkeeping any request writes, and the run records the agent runtime writes while a tool
//! runs); raw SQL that is not a query is refused too ([`check_sql`]). The side effects sim mode stops are refused,
//! each where sim stops it ([`check_effect`]): an async service call and the tx-commit service rules (both run on
//! another thread, unfenced), every outbound HTTP call but a model provider's (remote REST, script HTTP, resource
//! HTTP, system sync and messages, link previews, mail sign-in and discovery), git fetches and imports, printing, mail
//! sending, polling and sync.
//!
//! The resource writes (a file, runtime or component location, a provider's, a database resource) and the publish of a
//! notification are refused too ([`Effect::Resource`], [`Effect::Notify`]), in [`crate::context::ResourceFacade`] and
//! the notification message's send; so are an SFTP connection (system messages, backups, scripts), a mail server
//! connection, the embedded database's backup and a program a script runs ([`Effect::Process`]).
//!
//! A refusal is an error whose message starts with [`FENCE_ERROR`], and it is kept on the fence
//! ([`FenceGuard::violations`]), so a caller learns of it even when the code under the fence caught the error. A fence
//! is entered with [`enter`] and lifted for a stretch with [`lift`] (an approved write inside a fenced render); both
//! nest. The fence is not part of the execution context: work handed to another thread is not fenced, which is why
//! the async calls are refused.
//!
//! The sim preview of an approval ([`enter_sim`], plans/LLM-A2A-Assist.md 4.8) is a fence of its own kind: entity
//! writes go on, into the sim overlay, but every side effect is refused (an async call excepted, which sim mode skips),
//! and so are writes of the agent's own records, which a sim leaves on production. A lift does not lift it: the
//! approved transition a preview renders writes into the overlay and still reaches nothing outside.
//!
//! A remote tool an admin marked READ (an MCP client tool, plans/LLM-A2A-Assist.md 13.12) is still an outbound effect:
//! it runs under the read fence with a permit for its own server's origin ([`enter_remote`], [`check_remote`]), so its
//! one call goes out while every write and every other effect is still refused. A permit is no lift: it lets through
//! nothing but a remote call to that origin, and a sim preview refuses that too.

use std::cell::RefCell;
use std::marker::PhantomData;

use moqui_lib::error::Error;

/// How a fence refusal's message starts.
pub const FENCE_ERROR: &str = "read fence:";

/// Entities written under the fence: the sequence banks, artifact hits and their bins, authorization failures and
/// tarpit locks, visits, visitors and sessions, and the login history a step job's login writes; and the run
/// bookkeeping the agent runtime writes while a tool runs (its events and the run's event sequence, the run's steps,
/// messages and tool calls, a provider call a tool makes and its usage). Nothing else of `moqui.agent` or `moqui.llm`:
/// approvals, policies, memory and endpoints are not a read tool's to write.
pub const ALLOWED_ENTITIES: &[&str] = &[
    "moqui.entity.SequenceValueItem",
    "moqui.server.ArtifactHit",
    "moqui.server.ArtifactHitBin",
    "moqui.security.ArtifactAuthzFailure",
    "moqui.security.ArtifactTarpitLock",
    "moqui.security.UserLoginHistory",
    "moqui.server.Visit",
    "moqui.server.Visitor",
    "moqui.server.HttpSession",
    "moqui.agent.AgentRun",
    "moqui.agent.AgentRunStep",
    "moqui.agent.AgentRunEvent",
    "moqui.agent.AgentMessage",
    "moqui.agent.AgentMessagePart",
    "moqui.agent.AgentToolCall",
    "moqui.llm.LlmCall",
    "moqui.llm.LlmUsageDaily",
];

/// A side effect sim mode also stops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// An async service call or the tx-commit service rules: work another thread runs unfenced.
    AsyncCall,
    /// An outbound call: HTTP, a git fetch, mail sign-in and discovery.
    RemoteCall,
    Print,
    /// Sending, polling or syncing mail.
    Mail,
    /// SQL that is not a query.
    Sql,
    /// Writing, moving or deleting a resource: a file, runtime or component location, a provider's (git, content
    /// storage), a database resource.
    Resource,
    /// Publishing a notification (to sockets, the cluster, alerts).
    Notify,
    /// Running a program of the host.
    Process,
}

impl Effect {
    pub fn as_str(self) -> &'static str {
        match self {
            Effect::AsyncCall => "an async service call",
            Effect::RemoteCall => "a remote call",
            Effect::Print => "printing",
            Effect::Mail => "mail",
            Effect::Sql => "SQL that may write",
            Effect::Resource => "a resource write",
            Effect::Notify => "a notification",
            Effect::Process => "running a program",
        }
    }
}

/// What the fence refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Violation {
    /// An entity write: the entity's full name and the operation (create, update, delete, upsert).
    Write { entity: String, operation: String },
    /// A side effect, with what it was for (a service name, a server).
    Effect { effect: Effect, target: String },
}

impl Violation {
    /// One line for a log or a tool result.
    pub fn describe(&self) -> String {
        match self {
            Violation::Write { entity, operation } => format!("{operation} of {entity}"),
            Violation::Effect { effect, target } if target.is_empty() => effect.as_str().to_string(),
            Violation::Effect { effect, target } => format!("{} ({target})", effect.as_str()),
        }
    }
}

/// What a frame of the fence stack is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// The read fence: no write, no side effect.
    Read,
    /// A lifted stretch of the read fence.
    Lifted,
    /// A sim preview: writes go to the overlay, no side effect; never lifted.
    Sim,
    /// A permit for a remote call to one origin, over the fence below it: it decides nothing else.
    Remote,
}

struct Frame {
    kind: Kind,
    label: Option<String>,
    violations: Vec<Violation>,
    /// The origin a [`Kind::Remote`] frame lets calls go to.
    origin: Option<String>,
}

thread_local! {
    static FENCE: RefCell<Vec<Frame>> = const { RefCell::new(Vec::new()) };
}

/// Holds the fence until dropped. Not `Send`: the fence belongs to the thread that entered it.
#[must_use = "the fence ends when the guard drops"]
pub struct FenceGuard {
    depth: usize,
    _thread: PhantomData<*const ()>,
}

impl FenceGuard {
    /// What this fence refused so far, oldest first.
    pub fn violations(&self) -> Vec<Violation> {
        FENCE.with(|stack| {
            stack
                .borrow()
                .get(self.depth - 1)
                .map(|frame| frame.violations.clone())
                .unwrap_or_default()
        })
    }
}

impl Drop for FenceGuard {
    fn drop(&mut self) {
        pop_to(self.depth);
    }
}

/// Holds a lifted stretch until dropped.
#[must_use = "the fence is back when the guard drops"]
pub struct LiftGuard {
    depth: usize,
    _thread: PhantomData<*const ()>,
}

impl Drop for LiftGuard {
    fn drop(&mut self) {
        pop_to(self.depth);
    }
}

fn push(kind: Kind, label: Option<String>) -> usize {
    push_frame(kind, label, None)
}

fn push_frame(kind: Kind, label: Option<String>, origin: Option<String>) -> usize {
    FENCE.with(|stack| {
        let mut stack = stack.borrow_mut();
        stack.push(Frame {
            kind,
            label,
            violations: Vec::new(),
            origin,
        });
        stack.len()
    })
}

/// Drop the frame at `depth` and any left above it. A frame already gone (a cleared thread) is not an error.
fn pop_to(depth: usize) {
    FENCE.with(|stack| {
        let mut stack = stack.borrow_mut();
        if stack.len() >= depth {
            stack.truncate(depth - 1);
        }
    });
}

/// Fence this thread until the guard drops. `label` names what runs fenced (a tool, a render) in the refusals.
pub fn enter(label: impl Into<String>) -> FenceGuard {
    FenceGuard {
        depth: push(Kind::Read, Some(label.into())),
        _thread: PhantomData,
    }
}

/// Fence this thread for a sim preview until the guard drops: entity writes go on (into the sim overlay the caller
/// entered), the agent's own records and every side effect but an async call (which sim skips) are refused, and no
/// [`lift`] lifts it.
pub fn enter_sim(label: impl Into<String>) -> FenceGuard {
    FenceGuard {
        depth: push(Kind::Sim, Some(label.into())),
        _thread: PhantomData,
    }
}

/// Lift the read fence until the guard drops: an approved write inside a fenced render. A sim preview stays fenced.
pub fn lift() -> LiftGuard {
    LiftGuard {
        depth: push(Kind::Lifted, None),
        _thread: PhantomData,
    }
}

/// Permit a remote call to `origin` (`scheme://host:port`) until the guard drops: under the read fence a READ remote
/// tool's call goes out ([`check_remote`]); everything else stays as the fence below has it, and a sim refuses it too.
pub fn enter_remote(label: impl Into<String>, origin: &str) -> LiftGuard {
    LiftGuard {
        depth: push_frame(Kind::Remote, Some(label.into()), Some(origin.to_ascii_lowercase())),
        _thread: PhantomData,
    }
}

/// The frame that decides now: the read fence on top (a remote permit above it aside), else a sim preview anywhere
/// below, with its index.
fn in_force() -> Option<(usize, Kind)> {
    FENCE.with(|stack| {
        let stack = stack.borrow();
        let last = stack.iter().rposition(|frame| frame.kind != Kind::Remote)?;
        if stack[last].kind == Kind::Read {
            return Some((last, Kind::Read));
        }
        stack
            .iter()
            .rposition(|frame| frame.kind == Kind::Sim)
            .map(|at| (at, Kind::Sim))
    })
}

/// Whether this thread is fenced now: the read fence, or a sim preview.
pub fn active() -> bool {
    in_force().is_some()
}

/// The label of the fence in force.
pub fn label() -> Option<String> {
    let (at, _) = in_force()?;
    FENCE.with(|stack| stack.borrow().get(at).and_then(|frame| frame.label.clone()))
}

/// Forget every fence of this thread: the pools call it when a unit of work is over, so a guard that was leaked does
/// not fence the thread's later work.
pub(crate) fn clear() {
    FENCE.with(|stack| stack.borrow_mut().clear());
}

/// Whether an entity (its full name) is written under the fence.
pub fn allows_entity(full_name: &str) -> bool {
    ALLOWED_ENTITIES.contains(&full_name)
}

/// Whether `error` is a fence refusal.
pub fn is_fence_error(error: &Error) -> bool {
    error.message().contains(FENCE_ERROR)
}

/// Refuse a write of `full_name` when fenced and the entity is not allowed.
///
/// # Errors
///
/// Returns the refusal, which is also kept on the fence.
pub fn check_write(full_name: &str, operation: &str) -> Result<(), Error> {
    let allowed = match in_force() {
        None => true,
        Some((_, Kind::Read)) => allows_entity(full_name),
        // the overlay takes the write, but the agent's records stay on production in a sim: a preview leaves them
        Some((_, _)) => !(full_name.starts_with("moqui.agent.") || full_name.starts_with("moqui.llm.")),
    };
    if allowed {
        return Ok(());
    }
    refuse(Violation::Write {
        entity: full_name.to_string(),
        operation: operation.to_string(),
    })
}

/// Refuse a side effect when fenced. `target` says what it was for: a service name, a printer, a mail server.
///
/// # Errors
///
/// Returns the refusal, which is also kept on the fence.
pub fn check_effect(effect: Effect, target: &str) -> Result<(), Error> {
    match in_force() {
        None => return Ok(()),
        // sim mode skips an async call and the tx-commit rules itself
        Some((_, Kind::Sim)) if effect == Effect::AsyncCall => return Ok(()),
        Some(_) => {}
    }
    refuse(Violation::Effect {
        effect,
        target: target.to_string(),
    })
}

/// Refuse a remote call to `origin` (`scheme://host:port`) when fenced, unless a permit for that origin
/// ([`enter_remote`]) is above the read fence; a sim preview refuses it always. `target` says what it was for.
///
/// # Errors
///
/// Returns the refusal, which is also kept on the fence.
pub fn check_remote(origin: &str, target: &str) -> Result<(), Error> {
    let Some((at, kind)) = in_force() else {
        return Ok(());
    };
    let origin = origin.to_ascii_lowercase();
    // a render inside a sim preview is a read fence over the sim: the permit never lifts the sim
    let permitted = kind == Kind::Read
        && FENCE.with(|stack| {
            let stack = stack.borrow();
            !stack.iter().any(|frame| frame.kind == Kind::Sim)
                && stack
                    .iter()
                    .skip(at + 1)
                    .any(|frame| frame.kind == Kind::Remote && frame.origin.as_deref() == Some(origin.as_str()))
        });
    if permitted {
        return Ok(());
    }
    refuse(Violation::Effect {
        effect: Effect::RemoteCall,
        target: target.to_string(),
    })
}

/// [`check_effect`] for code that skips the effect rather than failing, as it does in sim mode: true when fenced, the
/// refusal kept on the fence and logged.
pub fn refuses(effect: Effect, target: &str) -> bool {
    check_effect(effect, target).is_err()
}

/// Words that make a query-shaped statement write: a data-modifying `WITH`, `SELECT … INTO`, `FOR UPDATE`, and the
/// functions with effects. Other writes (DDL, `SET`, `COPY`, `VACUUM`, `DO`) start with their own word, and a second
/// statement after a `;` is refused.
const WRITE_WORDS: &[&str] = &[
    "insert", "update", "delete", "merge", "upsert", "into", "call", "exec", "execute", "nextval", "setval",
    "pg_advisory_lock", "pg_advisory_xact_lock", "pg_terminate_backend", "pg_cancel_backend", "lo_import",
    "lo_export", "dblink", "dblink_exec",
];

/// Refuse raw SQL when fenced unless it is a plain query: it starts with `select`, `with`, `values`, `table`, `show`
/// or `explain` and has none of [`WRITE_WORDS`] (so `WITH d AS (DELETE …)`, `SELECT … INTO` and `nextval` are refused
/// too). A query that a word misjudges is refused, never a write let through.
///
/// # Errors
///
/// Returns the refusal, which is also kept on the fence.
pub fn check_sql(sql: &str) -> Result<(), Error> {
    if !active() {
        return Ok(());
    }
    let lower = sql.to_ascii_lowercase();
    let mut words = lower.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')).filter(|word| !word.is_empty());
    let first = words.next().unwrap_or("");
    let query = matches!(first, "select" | "with" | "values" | "table" | "show" | "explain")
        && !words.any(|word| WRITE_WORDS.contains(&word))
        && !lower.contains(';');
    if query {
        return Ok(());
    }
    let shown: String = sql.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(60).collect();
    check_effect(Effect::Sql, &shown)
}

fn refuse(violation: Violation) -> Result<(), Error> {
    let Some((at, kind)) = in_force() else {
        return Ok(());
    };
    let label = FENCE.with(|stack| {
        let mut stack = stack.borrow_mut();
        let frame = stack.get_mut(at)?;
        frame.violations.push(violation.clone());
        frame.label.clone()
    });
    let label = label.unwrap_or_default();
    let message = match kind {
        Kind::Sim => format!(
            "{FENCE_ERROR} {} is not run in {label}, which writes only to the sim",
            violation.describe()
        ),
        _ => format!(
            "{FENCE_ERROR} {} is not allowed in {label}, which may only read",
            violation.describe()
        ),
    };
    tracing::warn!("{message}");
    Err(Error::new(message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fence_nests_lifts_and_keeps_what_it_refused() {
        assert!(!active());
        assert!(check_write("moqui.basic.Enumeration", "create").is_ok());
        let outer = enter("tool data");
        assert!(active());
        assert_eq!(label().as_deref(), Some("tool data"));
        let refused = check_write("moqui.basic.Enumeration", "create").unwrap_err();
        assert!(is_fence_error(&refused), "{refused}");
        assert!(refused.message().contains("create of moqui.basic.Enumeration"), "{refused}");
        assert!(refused.message().contains("tool data"), "{refused}");
        assert!(check_write("moqui.agent.AgentRunEvent", "create").is_ok());
        assert!(check_write("moqui.llm.LlmCall", "create").is_ok());
        // the agent's configuration and the user's decisions are not run bookkeeping
        for entity in ["moqui.agent.AgentApproval", "moqui.agent.AgentToolPolicy", "moqui.agent.AgentMemory", "moqui.llm.LlmEndpoint"] {
            assert!(check_write(entity, "update").is_err(), "{entity}");
        }
        assert!(check_write("moqui.entity.SequenceValueItem", "update").is_ok());
        assert!(check_write("moqui.server.ArtifactHit", "create").is_ok());
        assert!(check_write("moqui.agentx.Other", "create").is_err());
        {
            let lifted = lift();
            assert!(!active());
            assert!(check_effect(Effect::Mail, "send#EmailTemplate").is_ok());
            {
                let inner = enter("render /apps/x");
                assert!(check_effect(Effect::AsyncCall, "a#B").is_err());
                assert_eq!(inner.violations().len(), 1);
            }
            assert!(!active());
            drop(lifted);
        }
        assert!(active());
        assert_eq!(
            outer.violations(),
            vec![
                Violation::Write {
                    entity: "moqui.basic.Enumeration".to_string(),
                    operation: "create".to_string()
                },
                Violation::Write {
                    entity: "moqui.agent.AgentApproval".to_string(),
                    operation: "update".to_string()
                },
                Violation::Write {
                    entity: "moqui.agent.AgentToolPolicy".to_string(),
                    operation: "update".to_string()
                },
                Violation::Write {
                    entity: "moqui.agent.AgentMemory".to_string(),
                    operation: "update".to_string()
                },
                Violation::Write {
                    entity: "moqui.llm.LlmEndpoint".to_string(),
                    operation: "update".to_string()
                },
                Violation::Write {
                    entity: "moqui.agentx.Other".to_string(),
                    operation: "create".to_string()
                },
            ]
        );
        drop(outer);
        assert!(!active());
    }

    #[test]
    fn a_sim_preview_writes_entities_and_refuses_every_effect_through_a_lift() {
        let guard = enter_sim("the preview of approval A1");
        assert!(active());
        assert!(check_write("moqui.basic.Enumeration", "create").is_ok(), "into the overlay");
        assert!(check_write("moqui.agent.AgentApproval", "update").is_err(), "the agent's records stay real");
        assert!(check_effect(Effect::AsyncCall, "a#B").is_ok(), "sim skips it");
        {
            // an approved transition inside the preview's render: the read fence lifts, the sim fence stays
            let _render = enter("render /apps/x");
            assert!(check_write("moqui.basic.Enumeration", "update").is_err());
            let _lifted = lift();
            assert!(check_write("moqui.basic.Enumeration", "update").is_ok());
            for effect in [Effect::Resource, Effect::Notify, Effect::Mail, Effect::RemoteCall, Effect::Print] {
                let refused = check_effect(effect, "x").unwrap_err();
                assert!(refused.message().contains("writes only to the sim"), "{refused}");
            }
            assert!(check_sql("delete from enumeration").is_err());
        }
        assert_eq!(guard.violations().len(), 7, "{:?}", guard.violations());
        drop(guard);
        assert!(!active());
        // the read fence refuses the new effects as well
        let read = enter("tool read_thing");
        assert!(check_effect(Effect::Resource, "runtime://x").is_err());
        assert!(check_effect(Effect::Notify, "Topic").is_err());
        assert_eq!(read.violations().len(), 2);
    }

    #[test]
    fn only_a_plain_query_is_run_under_the_fence() {
        assert!(check_sql("DELETE FROM x").is_ok(), "not fenced");
        let guard = enter("tool data");
        for query in ["SELECT * FROM moqui_basic_enumeration", "with t as (select 1) select * from t", "EXPLAIN SELECT 1", "show search_path"] {
            assert!(check_sql(query).is_ok(), "{query}");
        }
        for write in [
            "UPDATE enumeration SET description = 'x'",
            "with d as (delete from enumeration returning *) select * from d",
            "select * into copy_of from enumeration",
            "select nextval('seq')",
            "select 1; drop table enumeration",
            "INSERT INTO t VALUES (1) RETURNING id",
            "vacuum",
        ] {
            let refused = check_sql(write).unwrap_err();
            assert!(is_fence_error(&refused), "{write}");
        }
        assert_eq!(guard.violations().len(), 7);
    }

    #[test]
    fn a_remote_permit_lets_only_its_origin_through_and_never_in_a_sim() {
        assert!(check_remote("https://a.example:443", "x").is_ok(), "not fenced");
        let read = enter("tool run_tool");
        assert!(check_remote("https://a.example:443", "MCP a").is_err(), "no permit");
        {
            let _permit = enter_remote("MCP a", "https://A.example:443");
            assert!(active(), "a permit is no lift");
            assert!(check_remote("https://a.example:443", "MCP a").is_ok());
            assert!(check_remote("https://b.example:443", "MCP b").is_err(), "another origin");
            assert!(check_write("moqui.basic.Enumeration", "create").is_err(), "still no writes");
            assert!(check_effect(Effect::RemoteCall, "HTTP GET x").is_err(), "nor other remote calls");
            assert_eq!(label().as_deref(), Some("tool run_tool"));
        }
        assert!(check_remote("https://a.example:443", "MCP a").is_err(), "the permit is gone");
        assert_eq!(read.violations().len(), 5, "kept on the read fence: {:?}", read.violations());
        drop(read);
        let sim = enter_sim("the preview of approval A1");
        {
            let _permit = enter_remote("MCP a", "https://a.example:443");
            assert!(check_remote("https://a.example:443", "MCP a").is_err(), "a sim refuses it");
        }
        {
            let _render = enter("render");
            let _permit = enter_remote("MCP a", "https://a.example:443");
            assert!(check_remote("https://a.example:443", "MCP a").is_err(), "a render in a sim too");
        }
        assert_eq!(sim.violations().len(), 1);
    }

    #[test]
    fn a_cleared_thread_is_not_fenced_and_a_late_guard_drop_is_harmless() {
        let guard = enter("leaked");
        clear();
        assert!(!active());
        drop(guard);
        let again = enter("next");
        assert!(active());
        drop(again);
        assert!(!active());
    }
}
