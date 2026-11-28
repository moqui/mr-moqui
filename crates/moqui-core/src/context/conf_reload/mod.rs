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

//! Live conf reload (plans/archive/Component-Deploy.md, Phase 1): the conf merged again from the files (the default conf, each
//! component's `MoquiConf.xml` in load order, the runtime conf on top), compared with the running conf unit by unit
//! ([`catalog`]), the changes that can be live put into the running conf and their state built again, and the rest
//! named as waiting for a restart. Because the whole conf is merged again, a setting a later component or the runtime
//! conf overrides shows no change. The running conf holds only what is in effect: a change that needs a restart stays
//! out of it, so the next reload reports it again until the restart. Guide: docs/tools/Component-Deploy.md.

mod diff;
pub mod view;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use moqui_lib::env;
use moqui_lib::node::path::{AttrPath, PathError, Target};
use moqui_lib::node::MNode;
use moqui_lib::sync::{lock, wait_timeout};
use serde::Serialize;

use super::conf_env::{self, RetiredValue, Source};
use super::conf_schema::{self as catalog, Class, Group};
use super::{conf, ComponentInfo, Error, ExecutionContextFactory};
use diff::{Kind, UnitChange};

/// What became of one change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// In effect now.
    Applied,
    /// A preview: it would be in effect once applied.
    WouldApply,
    /// Waits for a restart.
    PendingRestart,
    /// Changes nothing: a property the environment sets, or one that is empty.
    NoEffect,
    /// Its hook or property failed; the problem says why.
    Failed,
}

/// One change between the running conf and the files.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfChange {
    /// The unit's element path, keyed elements with their key: `webapp-list/webapp[webroot]/token-route[s]`.
    pub unit: String,
    /// The attribute that changed; none for the element added or removed, or its other content.
    pub attribute: Option<String>,
    /// `added`, `removed` or `changed`.
    pub kind: &'static str,
    /// `live`, `apply` or `restart`.
    pub class: &'static str,
    /// The group whose state is built again, for `apply`.
    pub group: Option<&'static str>,
    /// Why a change waits for a restart or has no effect.
    pub reason: String,
    pub old: Option<String>,
    pub new: Option<String>,
    pub outcome: Outcome,
}

/// What a conf reload found and did.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfReport {
    pub changes: Vec<ConfChange>,
    /// The changes in effect (or, for a preview, that would be).
    pub applied: usize,
    /// The changes that wait for a restart.
    pub pending_restart: usize,
    /// The conf generation after the reload ([`ExecutionContextFactory::conf_generation`]).
    pub generation: u64,
    pub problems: Vec<String>,
    pub millis: u64,
}

/// A hook a conf reload runs after it replaced the conf, for state outside core (the host's guard), told why.
type Hook = Arc<dyn Fn(&ExecutionContextFactory, &[HookCause]) + Send + Sync>;

#[derive(Default)]
struct Hooks(Mutex<Vec<(Group, Hook)>>);

/// Why a group's hooks run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookCause {
    /// A conf reload: the files merged again.
    Reload,
    /// Settings set while running ([`set`]), by their selectors.
    Set(Vec<String>),
}

/// How many runs of a group's hooks one caller makes in all: its own and 7 more, each because the conf moved on while
/// the last ran. Past it the caller leaves, and the requests left are run by a caller waiting for the run, at once, or
/// else by the next change.
const MAX_HOOK_RUNS: usize = 8;

/// How long a caller waits for another thread's run of a group's hooks to end: a hook that waits for a thread which
/// sets the conf would otherwise wait for itself. Past it the caller leaves with a problem; its change runs when that
/// run ends.
const WAIT_FOR_RUN: std::time::Duration = std::time::Duration::from_secs(45);

/// Each group's hook runs, one at a time (plans/Runtime-Config.md §8.2): two sets, or a set and a reload, that each
/// replaced the conf would otherwise run their hooks at once, and the one that read the older conf could finish last.
/// Signalled when a run ends.
#[derive(Default)]
struct HookRunner {
    groups: Mutex<HashMap<Group, GroupRuns>>,
    ended: Condvar,
}

#[derive(Default)]
struct GroupRuns {
    /// The requests to run so far; how many there were when the last run began, and when the last run ended. A request
    /// at or below `finished` was covered by a run that read the conf after it.
    requested: u64,
    started: u64,
    finished: u64,
    /// The causes of the requests since the last run began.
    causes: Vec<HookCause>,
    /// The thread running the hooks now.
    running_on: Option<std::thread::ThreadId>,
    /// What the last run reported, for a request another run covered.
    problems: Vec<String>,
}

thread_local! {
    /// How many group runs this thread is inside: such a thread never waits for another group's run, which could be
    /// waiting for this one's.
    static IN_HOOKS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// The changes the last reload left waiting for a restart.
#[derive(Default)]
struct Pending(Mutex<Vec<ConfChange>>);

/// Held while a reload applies, so a save on System › Conf, the dev file watcher and SIGHUP do not each read the
/// running conf and replace it at once.
#[derive(Default)]
struct Applying(Mutex<()>);

/// What each attribute [`set`] set held in the running conf before its first set, by its path's compare key.
#[derive(Default)]
struct Originals(Mutex<std::collections::HashMap<String, Option<String>>>);

/// The elements a [`set`] made (in neither the conf files nor the running conf), by their path's compare key: a put back takes the element away again once it is empty, so the running conf is the
/// files' again and a preview of them finds no change.
#[derive(Default)]
struct Created(Mutex<std::collections::HashSet<String>>);

/// Run `hook` after a conf reload that changed a unit of `group`, or a [`set`] of one of its attributes. A group's hooks
/// run one run at a time, and again when the conf moved on while they ran, so the last run reads the latest conf.
///
/// A hook may set the conf or reload it on its own thread: that is run after its run, never waited for. It must not
/// hand that to another thread and wait for it: that thread waits for this group's run to end, up to 45 s, and then
/// leaves with a problem, so the hook waits that long.
pub fn on_apply(
    factory: &ExecutionContextFactory,
    group: Group,
    hook: impl Fn(&ExecutionContextFactory) + Send + Sync + 'static,
) {
    on_apply_with_cause(factory, group, move |factory, _| hook(factory));
}

/// [`on_apply`] for a hook told why it runs: each cause since the group's last run began, once, in order.
pub fn on_apply_with_cause(
    factory: &ExecutionContextFactory,
    group: Group,
    hook: impl Fn(&ExecutionContextFactory, &[HookCause]) + Send + Sync + 'static,
) {
    lock(&factory.extension(|_| Hooks::default()).0).push((group, Arc::new(hook)));
}

/// Where [`set`] kept a setting (plans/Runtime-Config.md §3.6, "Kept where").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kept {
    /// In `runtime/.env` under the attribute's environment name, which the conf merge reads at the next start.
    DotEnv,
    /// In the seal's store: a secret (plans/Sealed-Secrets.md S4).
    SealStore,
    /// As a process property until the process ends: the OS environment names the attribute, which `runtime/.env`
    /// does not outrank, so the environment sets it again at the next start.
    UntilRestart,
    /// As a process property until the process ends, as asked (not persisted): a spec's `setConf`.
    Property,
}

impl Kept {
    /// Where it is kept, for an answer.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::DotEnv => "runtime/.env",
            Self::SealStore => "the seal's store",
            Self::UntilRestart => "this process until a restart",
            Self::Property => "a process property",
        }
    }
}

/// What [`set`] did with one setting (plans/Runtime-Config.md §8.2). Its `Debug` masks the values of a secret.
///
/// `queued`: the group's hooks were running on another call when this set asked for them (this set came from one of
/// its hooks, or the wait for the run in progress timed out), so the change applies when that run ends, not before the
/// set returned.
#[derive(Clone)]
pub struct SetOutcome {
    /// The attribute's selector, as the running conf spells it.
    pub selector: String,
    /// Its canonical environment name, the name it is kept under.
    pub env_name: String,
    /// Whether it holds a secret: `previous` and `before_kept` are then never shown.
    pub secret: bool,
    /// The running conf's value before the set; `None` when it had none.
    pub previous: Option<String>,
    /// What the place it is kept in held under its name before: the line of `runtime/.env` (read from the file), or the
    /// process property; `None` for the seal's store. What [`set_value`] answers.
    pub before_kept: Option<String>,
    pub kept: Kept,
    /// What sets the attribute over the kept value: a `-D` the boot took, which sets it again at the next start, or
    /// for a secret kept in the seal's store, the environment, which a reader takes first.
    pub outranked_by: Option<Source>,
    /// How the change takes effect, as a reload's diff takes it: live, by its group's hook (which [`set`] ran), or at
    /// the next start. A set that makes an element takes the class of that element added whole.
    pub class: Class,
    /// What failed once the value was kept: its group's hook, as the last run that covered this set reports it (that
    /// run may have read another caller's change too).
    pub problems: Vec<String>,
    /// The group's hooks run for this set once the run in progress ends, after the set returned.
    pub queued: bool,
}

impl SetOutcome {
    /// Where the value is kept, what outranks it, and when it applies, as a sentence for an answer.
    #[must_use]
    pub fn note(&self) -> String {
        let kept = match self.kept {
            Kept::DotEnv => "kept in runtime/.env".to_string(),
            Kept::SealStore => "kept in the seal's store".to_string(),
            Kept::UntilRestart => format!(
                "kept by this process until it restarts, since the environment names {}, which runtime/.env does not \
                 outrank",
                self.env_name
            ),
            Kept::Property => "set as a process property until the process ends".to_string(),
        };
        let next_start = match (self.outranked_by, self.kept) {
            (Some(Source::Property), _) => format!("; a -D of {} sets it over that at the next start", self.env_name),
            (Some(Source::Environment), _) => format!("; the environment's {} outranks it", self.env_name),
            (Some(Source::DotEnv) | None, Kept::UntilRestart) => {
                "; the environment sets it at the next start".to_string()
            }
            (Some(Source::DotEnv) | None, _) => String::new(),
        };
        let applies = match self.class {
            _ if !self.problems.is_empty() => format!("; applying it failed: {}", self.problems.join("; ")),
            Class::Restart(reason) => format!("; it applies at the next start ({reason})"),
            Class::Apply(group) if self.queued => format!(
                "; it applies when the run of the {} hooks in progress ends",
                group.name()
            ),
            Class::Live | Class::Apply(_) => "; it applies now".to_string(),
        };
        format!("{kept}{next_start}{applies}")
    }
}

impl std::fmt::Debug for SetOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let shown = |value: &Option<String>| {
            value.as_ref().map(|value| {
                if self.secret && !value.is_empty() {
                    view::MASKED.to_string()
                } else {
                    value.clone()
                }
            })
        };
        f.debug_struct("SetOutcome")
            .field("selector", &self.selector)
            .field("env_name", &self.env_name)
            .field("secret", &self.secret)
            .field("previous", &shown(&self.previous))
            .field("before_kept", &shown(&self.before_kept))
            .field("kept", &self.kept)
            .field("outranked_by", &self.outranked_by)
            .field("class", &self.class)
            .field("problems", &self.problems)
            .field("queued", &self.queued)
            .finish()
    }
}

/// What [`set`] reads beyond the factory, which a test gives rather than set environment variables
/// (plans/Runtime-Config.md §8.2).
struct Inputs<'a> {
    /// The OS environment by name: its value when it names one that is not empty.
    environment: &'a dyn Fn(&str) -> Option<String>,
    /// The retired values refused ([`super::conf_env::RETIRED_VALUES`]).
    retired: &'a [RetiredValue],
    /// The runtime's `.env`.
    env_file: PathBuf,
    /// Whether a secret kept goes to `.env` rather than the seal's store: a test's factory, which has no seal
    /// ([`ExecutionContextFactory::keeps_secrets_in_env`]).
    secrets_in_env: bool,
}

impl Inputs<'static> {
    fn of(factory: &ExecutionContextFactory) -> Self {
        Self {
            environment: &os_environment,
            retired: conf_env::RETIRED_VALUES,
            env_file: factory.runtime_dir().join(".env"),
            secrets_in_env: factory.keeps_secrets_in_env(),
        }
    }
}

/// The OS environment's value of `name`, when it is set and not empty, as the conf merge takes it.
fn os_environment(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// Set the attribute at `selector` in the running conf, from now on and over the conf files, as an environment name
/// sets one (plans/archive/Conf-Paths.md §3); `value` `None` puts back what the files and the environment give now.
/// With `persist` the value is kept in `runtime/.env` under the attribute's environment name, a secret in the seal's
/// store, for a server that writes its own settings (edge pairing, the DKIM key made on first use, a listener switched
/// from System Home); a process property of that name, which would outrank it, is cleared. When the OS environment
/// names the attribute, `.env` would not outrank it, so it is set as a process property instead, which holds until the
/// restart (plans/Runtime-Config.md §3.6). Without `persist`, a process property of that name holds it until the
/// process ends (a spec's `setConf`). Readers of the conf see it at once; one that read it at start keeps what it read.
/// Once the running conf is replaced and its lock released, the hooks of the attribute's group run, as after a conf
/// reload ([`super::conf_schema::attribute_class`]); a setting that waits for a restart runs none.
///
/// # Errors
///
/// When `selector` does not name an attribute that has an environment name, `value` is a retired value
/// ([`super::conf_env::RETIRED_VALUES`]) or the attribute a renamed one ([`super::conf_env::RETIRED_ATTRIBUTES`]), the
/// running conf does not parse, or `.env` or the seal's store cannot be written.
pub fn set(
    factory: &ExecutionContextFactory,
    selector: &str,
    value: Option<&str>,
    persist: bool,
) -> Result<SetOutcome, Error> {
    let mut outcomes = set_with(factory, &[(selector, value)], persist, &Inputs::of(factory))?;
    outcomes
        .pop()
        .ok_or_else(|| Error::new(format!("{selector}: nothing was set")))
}

/// [`set`] for several settings at once, each `(selector, value)`: all are written, the running conf is replaced once,
/// and then the hooks of each group they reach run once, so a change of several settings (a listener's switch and its
/// port) takes effect as one. One outcome for each setting, in order.
///
/// The checks are all or nothing: a setting refused refuses the whole list before anything is written. The writes are
/// not one transaction: the seal's store is written first, one secret at a time, then `.env` once for all the rest;
/// a secret kept before a later write failed stays kept, and the error names it.
///
/// # Errors
///
/// As [`set`]; a setting refused (no environment name, a retired value, an attribute named twice) refuses the whole
/// list before anything is written.
pub fn set_many(
    factory: &ExecutionContextFactory,
    settings: &[(&str, Option<&str>)],
    persist: bool,
) -> Result<Vec<SetOutcome>, Error> {
    set_with(factory, settings, persist, &Inputs::of(factory))
}

/// [`set_many`], with the hooks of `held` not run: the caller applies that group's change itself, at once and as one,
/// as the listener services and the operator socket do, which then ask the listener registry for one pass that carries
/// their cause ([`crate::listeners::Cause::SetValue`]) and the listeners whose connections end now
/// (plans/Runtime-Config.md §3.6). The hooks of the other groups the settings reach run as [`set_many`] runs them.
///
/// # Errors
///
/// As [`set_many`].
pub fn set_many_holding(
    factory: &ExecutionContextFactory,
    settings: &[(&str, Option<&str>)],
    persist: bool,
    held: Group,
) -> Result<Vec<SetOutcome>, Error> {
    set_with_holding(factory, settings, persist, &Inputs::of(factory), Some(held))
}

/// [`set`], answering what the name held before where it is kept ([`SetOutcome::before_kept`]): the form the specs'
/// `setConf` and the DKIM key use.
///
/// # Errors
///
/// As [`set`].
pub fn set_value(
    factory: &ExecutionContextFactory,
    selector: &str,
    value: Option<&str>,
    persist: bool,
) -> Result<Option<String>, Error> {
    set(factory, selector, value, persist).map(|outcome| outcome.before_kept)
}

/// One setting of a [`set_with`], resolved and checked before anything is written: in the running conf, its elements
/// made when missing; a secret for the seal's store in a scratch tree when the running conf lacks its element, since
/// the running conf keeps no secret.
struct Planned {
    target: Target,
    name: String,
    value: Option<String>,
    secret: bool,
    kept: Kept,
    class: Class,
    previous: Option<String>,
    before_kept: Option<String>,
    outranked_by: Option<Source>,
}

fn set_with(
    factory: &ExecutionContextFactory,
    settings: &[(&str, Option<&str>)],
    persist: bool,
    inputs: &Inputs<'_>,
) -> Result<Vec<SetOutcome>, Error> {
    set_with_holding(factory, settings, persist, inputs, None)
}

/// [`set_with`], running no hook of `holding`.
fn set_with_holding(
    factory: &ExecutionContextFactory,
    settings: &[(&str, Option<&str>)],
    persist: bool,
    inputs: &Inputs<'_>,
    holding: Option<Group>,
) -> Result<Vec<SetOutcome>, Error> {
    let applying = factory.extension(|_| Applying::default());
    let held = lock(&applying.0);
    let running = MNode::parse_text("running", &factory.conf_text())?;
    // what .env holds now, from the file: the table loaded at start may be another runtime's of this process, and
    // misses what was written since
    let in_file: Vec<(String, String)> = std::fs::read_to_string(&inputs.env_file)
        .map(|text| env::parse_dotenv(&text))
        .unwrap_or_default();
    let mut planned: Vec<Planned> = Vec::with_capacity(settings.len());
    for (selector, value) in settings {
        let item = plan(factory, &running, selector, *value, persist, inputs, &in_file)?;
        if planned.iter().any(|earlier| earlier.name == item.name) {
            return Err(Error::new(format!("{selector} is given twice")));
        }
        planned.push(item);
    }

    // the seal's store first: a failure there leaves .env and the running conf as they were
    let mut stored: Vec<String> = Vec::new();
    for item in planned.iter().filter(|item| item.kept == Kept::SealStore) {
        let selector = item.target.path.selector();
        let written = match &item.value {
            Some(value) => crate::seal::secrets::put(factory, &selector, value),
            None => crate::seal::secrets::remove(factory, &selector).map(|_| ()),
        };
        written.map_err(|err| kept_before(&err, &stored))?;
        stored.push(selector);
    }
    // one write of .env for them all
    let mut put: Vec<(String, String)> = Vec::new();
    let mut removed: Vec<&str> = Vec::new();
    for item in &planned {
        match (item.kept, &item.value) {
            (Kept::DotEnv, Some(value)) => put.push((item.name.clone(), value.clone())),
            (Kept::DotEnv, None) => removed.push(&item.name),
            // a line an earlier build kept for a secret in .env would win over the store at a seal-less boot: it goes
            (Kept::SealStore, _) if inputs.env_file.is_file() => removed.push(&item.name),
            _ => {}
        }
    }
    if !put.is_empty() || !removed.is_empty() {
        env::update_dotenv_file(&inputs.env_file, &put, &removed).map_err(|err| kept_before(&err, &stored))?;
    }
    for item in &planned {
        match (item.kept, &item.value) {
            // a process property of the name outranks .env and the store, in readers and in a reload: a stale one goes
            (Kept::DotEnv | Kept::SealStore, _) => env::clear_property(&item.name),
            (Kept::UntilRestart | Kept::Property, Some(value)) => env::set_property(&item.name, value.clone()),
            (Kept::UntilRestart | Kept::Property, None) => env::clear_property(&item.name),
        }
    }

    // the store answers a secret while the conf leaves it empty or at CHANGEME, so the running conf keeps none
    let mut edited = false;
    let mut files: Option<Result<MNode, Error>> = None;
    for item in planned.iter().filter(|item| item.kept != Kept::SealStore) {
        put_in_running(factory, item, &mut files);
        edited = true;
    }
    if edited {
        factory.replace_conf(running.write_string());
    }
    // a hook may read or set the conf itself
    drop(held);
    // a listener's switch set live ends its boot upgrade warning (plans/Runtime-Config.md §8.2; remove with it)
    for item in &planned {
        crate::listeners::switch_set_live(factory, &item.target.path.selector());
    }

    let mut outcomes: Vec<SetOutcome> = planned
        .into_iter()
        .map(|item| SetOutcome {
            selector: item.target.path.selector(),
            env_name: item.name,
            secret: item.secret,
            previous: item.previous,
            before_kept: item.before_kept,
            kept: item.kept,
            outranked_by: item.outranked_by,
            class: item.class,
            problems: Vec::new(),
            queued: false,
        })
        .collect();
    let mut groups: Vec<Group> = Vec::new();
    for outcome in &outcomes {
        if let Class::Apply(group) = outcome.class {
            if !groups.contains(&group) && holding != Some(group) {
                groups.push(group);
            }
        }
    }
    for group in groups {
        let selectors: Vec<String> = outcomes
            .iter()
            .filter(|outcome| outcome.class == Class::Apply(group))
            .map(|outcome| outcome.selector.clone())
            .collect();
        let ran = run_group(factory, group, &HookCause::Set(selectors));
        for outcome in outcomes
            .iter_mut()
            .filter(|outcome| outcome.class == Class::Apply(group))
        {
            outcome.problems.extend(ran.problems.iter().cloned());
            outcome.queued = ran.queued;
        }
    }
    Ok(outcomes)
}

/// `err` of a write that failed after the secrets `stored` were kept, which stay kept.
fn kept_before(err: &impl std::fmt::Display, stored: &[String]) -> Error {
    if stored.is_empty() {
        return Error::new(err.to_string());
    }
    Error::new(format!(
        "{err}; kept in the seal's store before it failed, and kept still: {}",
        stored.join(", ")
    ))
}

/// `selector` resolved and checked: what [`set_with`] writes for it.
fn plan(
    factory: &ExecutionContextFactory,
    running: &MNode,
    selector: &str,
    value: Option<&str>,
    persist: bool,
    inputs: &Inputs<'_>,
    in_file: &[(String, String)],
) -> Result<Planned, Error> {
    let path = AttrPath::parse(selector).map_err(|err| Error::new(format!("{selector}: {err}")))?;
    let failed = |err: PathError| Error::new(format!("{selector}: {err}"));
    // a secret kept goes to the seal's store, never runtime/.env (plans/Sealed-Secrets.md S4); a test's factory keeps
    // it in .env as before
    let to_store = persist && conf_env::attribute_secret(&path) && !inputs.secrets_in_env;
    // the first element the running conf lacks, by its depth: the set adds it, as a reload adds an element
    let (target, made) = match moqui_lib::node::path::resolve(running, &path, &catalog::keying, false) {
        Ok(found) => (found, None),
        Err(PathError::Missing(missing)) => {
            // the element path up to the missing one, its names joined by `/` (a name has none)
            let made = missing.matches('/').count();
            // the running conf keeps no secret, so nothing is made in it for one the store keeps
            let tree = if to_store {
                MNode::new("moqui-conf")
            } else {
                running.clone()
            };
            let found = moqui_lib::node::path::resolve(&tree, &path, &catalog::keying, true).map_err(failed)?;
            (found, Some(made))
        }
        Err(err) => return Err(failed(err)),
    };
    let Some(name) = target.path.env_name() else {
        return Err(Error::new(format!(
            "{selector}: a key with no letter or digit has no environment name"
        )));
    };
    if let Some(refused) = value.and_then(|value| conf_env::retired_value_in(inputs.retired, &target.path, value)) {
        return Err(Error::new(refused));
    }
    // a renamed attribute the boot refuses by name ([`conf_env::RETIRED_ATTRIBUTES`]) is not set while running either,
    // so the next reload does not refuse the conf for it; putting it back (no value) is left alone
    if let Some(refused) = value.and_then(|_| conf_env::retired_attribute(&target.path)) {
        return Err(Error::new(refused));
    }
    let secret = conf_env::attribute_secret(&target.path);
    let boot = conf_env::boot_sources(factory, &target.path);
    let environment_names = (inputs.environment)(&name).is_some() || boot.contains(&Source::Environment);
    let kept = if !persist {
        Kept::Property
    } else if to_store {
        Kept::SealStore
    } else if environment_names {
        Kept::UntilRestart
    } else {
        Kept::DotEnv
    };
    let before_kept = match kept {
        Kept::SealStore => None,
        // the last line of the name, as loading the file takes it
        Kept::DotEnv => in_file
            .iter()
            .rev()
            .find(|(held, _)| *held == name)
            .map(|(_, held)| held.clone())
            .filter(|held| !held.is_empty()),
        Kept::UntilRestart | Kept::Property => env::get_property(&name),
    };
    let outranked_by = match kept {
        // a -D the boot took is back at the next start, over .env, the store and the environment alike
        Kept::DotEnv | Kept::SealStore | Kept::UntilRestart if boot.contains(&Source::Property) => {
            Some(Source::Property)
        }
        Kept::SealStore if environment_names => Some(Source::Environment),
        _ => None,
    };
    let names: Vec<&str> = target.path.steps.iter().map(|step| step.element.as_str()).collect();
    Ok(Planned {
        previous: if made.is_some() {
            None
        } else {
            target.node.attribute(&target.attribute)
        },
        class: set_class(&names, &target.attribute, made),
        name,
        value: value.map(str::to_string),
        secret,
        kept,
        before_kept,
        outranked_by,
        target,
    })
}

/// The class of setting `attr` at `path` (the element names) as a reload's diff takes the change, when the set makes
/// the elements from depth `made` down: those are added whole, so the change waits for a restart when anything added
/// needs one, and a cache's settings change only for a cache made now (`refine`). Of the groups built from the conf,
/// the attribute's own is taken first, then the innermost element's.
fn set_class(names: &[&str], attr: &str, made: Option<usize>) -> Class {
    let own = catalog::attribute_class(names, attr);
    let made = made.filter(|made| *made < names.len());
    let Some(made) = made else {
        // an attribute of an element the conf has
        return if own == Class::Apply(Group::Caches) {
            Class::Restart(CACHE_SETTINGS)
        } else {
            own
        };
    };
    let mut parts: Vec<Class> = (made..names.len())
        .map(|depth| catalog::element_class(&names[..=depth]))
        .collect();
    parts.push(own);
    if let Some(restart) = parts.iter().copied().find(|part| matches!(part, Class::Restart(_))) {
        return restart;
    }
    let cache_made = (made..names.len())
        .any(|depth| catalog::unit(&names[..=depth]).is_some_and(|unit| unit.class == Class::Apply(Group::Caches)));
    if parts.contains(&Class::Apply(Group::Caches)) && !cache_made {
        return Class::Restart(CACHE_SETTINGS);
    }
    if matches!(own, Class::Apply(_)) {
        return own;
    }
    parts
        .iter()
        .rev()
        .copied()
        .find(|part| matches!(part, Class::Apply(_)))
        .unwrap_or(own)
}

/// Put `item`'s value into the running conf it was resolved in, or with none what the conf files and the environment
/// give now (`files`, merged once for every put back), and keep what that needs to undo.
fn put_in_running(factory: &ExecutionContextFactory, item: &Planned, files: &mut Option<Result<MNode, Error>>) {
    let target = &item.target;
    let selector = target.path.selector();
    // the value the running conf had before the first set of this attribute since boot, for a put back with no conf
    // files to merge (a factory of a unit test)
    let originals = factory.extension(|_| Originals::default());
    let compare_key = conf_env::compare_key(&target.path);
    let created = factory.extension(|_| Created::default());
    let element_key = compare_key
        .rsplit_once('/')
        .map_or("", |(element, _)| element)
        .to_string();
    let made_here = {
        let mut held = lock(&created.0);
        if item.value.is_some() && target.created {
            held.insert(element_key.clone());
        }
        // a put back of an element a set made, or one this resolve made to reach an attribute no file names
        item.value.is_none() && (held.contains(&element_key) || target.created)
    };
    let original = {
        let mut held = lock(&originals.0);
        held.entry(compare_key).or_insert_with(|| item.previous.clone()).clone()
    };
    let live = match &item.value {
        Some(value) => Some(value.clone()),
        None => match files.get_or_insert_with(|| merged_from_files(factory, &factory.components())) {
            Ok(files) => moqui_lib::node::path::resolve(files, &target.path, &catalog::keying, false)
                .ok()
                .and_then(|found| found.node.attribute(&found.attribute)),
            Err(error) => {
                tracing::warn!(
                    "{selector}: the conf files did not merge ({error}); putting back the value before it was set"
                );
                original
            }
        },
    };
    match live {
        Some(live) => target.node.set_attribute(target.attribute.clone(), live),
        None => {
            target.node.remove_attribute(&target.attribute);
            // an element a set made, empty again, goes with its attribute
            if made_here && target.node.attributes().is_empty() && target.node.child_count() == 0 {
                if let Some(parent) = target.node.parent() {
                    parent.remove_where(|child| child.same_node(&target.node));
                    lock(&created.0).remove(&element_key);
                }
            }
        }
    }
}

/// The changes the last conf reload left waiting for a restart; empty before any reload.
#[must_use]
pub fn pending_restart(factory: &ExecutionContextFactory) -> Vec<ConfChange> {
    lock(&factory.extension(|_| Pending::default()).0).clone()
}

/// The merged conf the files make now for `components`, merged as boot merges it.
///
/// # Errors
///
/// When a conf file does not parse, or the runtime conf named is missing.
pub fn merged_from_files(factory: &ExecutionContextFactory, components: &[ComponentInfo]) -> Result<MNode, Error> {
    let env_file = factory.runtime_dir().join(".env");
    conf::merge_files_with(&conf_files(factory, components)?, None, Some(&env_file)).map(|(conf, _)| conf)
}

/// The files the conf of `components` is merged from, in merge order, as boot merges them.
///
/// # Errors
///
/// When the runtime conf named is missing.
pub fn conf_files(
    factory: &ExecutionContextFactory,
    components: &[ComponentInfo],
) -> Result<Vec<conf::ConfFile>, Error> {
    let named: Vec<(String, PathBuf)> = components
        .iter()
        .map(|info| (info.name.clone(), info.location.clone()))
        .collect();
    conf::conf_files_of(
        factory.framework_dir(),
        factory.runtime_dir(),
        &named,
        factory.test_confs(),
    )
}

/// What a reload of the conf files would change, changing nothing.
///
/// # Errors
///
/// When a conf file does not parse.
pub fn preview(factory: &ExecutionContextFactory) -> Result<ConfReport, Error> {
    preview_conf(factory, &merged_from_files(factory, &factory.components())?)
}

/// Reload the conf files: apply what can be live, and name the rest as waiting for a restart. The files are merged
/// under the lock a [`set`] holds while it writes `.env` and the running conf, so a set just before is in the merge
/// rather than undone by it.
///
/// # Errors
///
/// When a conf file does not parse; the running conf stays.
pub fn apply(factory: &ExecutionContextFactory) -> Result<ConfReport, Error> {
    let applying = factory.extension(|_| Applying::default());
    let held = lock(&applying.0);
    let disk = merged_from_files(factory, &factory.components())?;
    run(factory, &disk, Some(held))
}

/// [`preview`] against the merged conf `disk`: a component about to be loaded (plans/archive/Component-Deploy.md, Phase 2).
///
/// # Errors
///
/// When the running conf does not parse.
pub fn preview_conf(factory: &ExecutionContextFactory, disk: &MNode) -> Result<ConfReport, Error> {
    run(factory, disk, None)
}

/// [`apply`] with the merged conf `disk`. A [`set`] between the caller's merge and this call is not in `disk`, and a
/// change of it that can be live is put back: a caller that merges the conf files themselves calls [`apply`], which
/// merges under the lock.
///
/// # Errors
///
/// When the running conf does not parse.
pub fn apply_conf(factory: &ExecutionContextFactory, disk: &MNode) -> Result<ConfReport, Error> {
    let applying = factory.extension(|_| Applying::default());
    let held = lock(&applying.0);
    run(factory, disk, Some(held))
}

/// A change as the reload decided it: the class it takes, and why when it is not the unit's.
struct Decided {
    change: UnitChange,
    class: Class,
    reason: String,
    no_effect: bool,
}

/// Compare `disk` with the running conf; with `held`, the [`Applying`] lock, apply what can be live, let go of the lock
/// and then run the hooks of the groups reached.
fn run(factory: &ExecutionContextFactory, disk: &MNode, held: Option<MutexGuard<'_, ()>>) -> Result<ConfReport, Error> {
    let apply = held.is_some();
    conf_env::refuse_retired(disk)?;
    let started = std::time::Instant::now();
    let running = MNode::parse_text("running", &factory.conf_text())?;
    let mut decided: Vec<Decided> = diff::diff(&running, disk)
        .into_iter()
        .map(|change| Decided {
            class: change.class,
            reason: restart_reason(change.class),
            change,
            no_effect: false,
        })
        .collect();
    let mut report = ConfReport {
        generation: factory.conf_generation(),
        ..ConfReport::default()
    };
    for item in &mut decided {
        refine(item);
    }
    // An element added or removed whole waits for a restart when anything in it does.
    let mut outer_restart: Vec<Option<String>> = vec![None; decided.len()];
    for item in &decided {
        if let (Some(at), Class::Restart(_)) = (item.change.within, item.class) {
            outer_restart[at].get_or_insert_with(|| item.reason.clone());
        }
    }
    for (at, reason) in outer_restart.into_iter().enumerate() {
        if let Some(reason) = reason {
            if !matches!(decided[at].class, Class::Restart(_)) {
                decided[at].class = Class::Restart("something inside it needs a restart");
                decided[at].reason = reason;
            }
        }
    }
    for at in 0..decided.len() {
        if let Some(outer) = decided[at].change.within {
            if let Class::Restart(why) = decided[outer].class {
                if !matches!(decided[at].class, Class::Restart(_)) {
                    decided[at].class = Class::Restart(why);
                    decided[at].reason = decided[outer].reason.clone();
                }
            }
        }
    }

    let mut groups: Vec<Group> = Vec::new();
    let mut outcomes: Vec<Outcome> = Vec::with_capacity(decided.len());
    let mut changed = false;
    for item in &decided {
        let outcome = match item.class {
            Class::Restart(_) => Outcome::PendingRestart,
            _ if item.no_effect => Outcome::NoEffect,
            _ if apply => Outcome::Applied,
            _ => Outcome::WouldApply,
        };
        if apply && outcome != Outcome::PendingRestart && item.change.within.is_none() {
            item.change.edit.apply();
            changed = true;
        }
        if let (Class::Apply(group), Outcome::Applied | Outcome::WouldApply) = (item.class, outcome) {
            if !groups.contains(&group) {
                groups.push(group);
            }
        }
        outcomes.push(outcome);
    }

    if apply && changed {
        report.generation = factory.replace_conf(running.write_string());
        tracing::warn!(
            target: crate::live_settings::AUDIT,
            "Conf reloaded (generation {}): {} changes applied, {} wait for a restart",
            report.generation,
            outcomes.iter().filter(|outcome| **outcome == Outcome::Applied).count(),
            outcomes.iter().filter(|outcome| **outcome == Outcome::PendingRestart).count()
        );
    }

    for (item, outcome) in decided.into_iter().zip(outcomes) {
        let change = item.change;
        report.changes.push(ConfChange {
            unit: change.path,
            attribute: change.attr,
            kind: match change.kind {
                Kind::Added => "added",
                Kind::Removed => "removed",
                Kind::Changed => "changed",
            },
            class: item.class.name(),
            group: group_of(item.class),
            reason: item.reason,
            old: change.old,
            new: change.new,
            outcome,
        });
    }
    report.applied = report
        .changes
        .iter()
        .filter(|change| matches!(change.outcome, Outcome::Applied | Outcome::WouldApply))
        .count();
    let pending: Vec<ConfChange> = report
        .changes
        .iter()
        .filter(|change| change.outcome == Outcome::PendingRestart)
        .cloned()
        .collect();
    report.pending_restart = pending.len();
    if apply {
        *lock(&factory.extension(|_| Pending::default()).0) = pending;
    }
    // a hook may set the conf itself
    drop(held);
    if apply && changed {
        report.problems.extend(run_hooks(factory, &groups, &HookCause::Reload));
    }
    report.millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    Ok(report)
}

/// Why a change to a cache that exists waits for a restart.
const CACHE_SETTINGS: &str = "a cache's settings are read when it is made";

/// The class a change takes beyond its unit's: the rule of a `cache`.
fn refine(item: &mut Decided) {
    if item.class == Class::Apply(Group::Caches) {
        let added = item.change.kind == Kind::Added && item.change.attr.is_none();
        if !added && item.change.within.is_none() {
            item.class = Class::Restart(CACHE_SETTINGS);
            item.reason = CACHE_SETTINGS.to_string();
        }
    }
}

/// Build again the state of each of `groups` and run the hooks registered for it ([`on_apply`]), as after a conf
/// reload or a [`set`] that reached it, for `cause`; answers what failed, each problem named by its group. A group's
/// runs are one at a time: this waits for a run in progress (up to 45 s), and answers once a run that began after this
/// call has ended, so the last run reads the latest conf; the problems are that run's, which may have read another
/// caller's change too. Called from inside a run of any group's hooks (a hook that sets a setting) while the group runs
/// elsewhere, it leaves its request to the run in progress, which goes again, and answers at once with none.
pub fn run_hooks(factory: &ExecutionContextFactory, groups: &[Group], cause: &HookCause) -> Vec<String> {
    let mut problems = Vec::new();
    for group in groups {
        problems.extend(run_group(factory, *group, cause).problems);
    }
    problems
}

/// What [`run_group`] answers.
struct GroupRan {
    /// The problems of the run that covered the request, or why it was not waited for.
    problems: Vec<String>,
    /// The request was left to a run in progress, which covers it when it ends.
    queued: bool,
}

/// The run of a group's hooks this thread leads, from when it took the lead until it lets go: its drop, at the end or in
/// a panic, lets go of the lead and wakes the callers waiting for it, so none waits on a run that will never end.
struct Leading<'a> {
    runner: &'a HookRunner,
    group: Group,
}

impl<'a> Leading<'a> {
    /// Lead `group`'s runs; the caller has set `running_on` to this thread under the lock.
    fn new(runner: &'a HookRunner, group: Group) -> Self {
        IN_HOOKS.with(|depth| depth.set(depth.get() + 1));
        Self { runner, group }
    }
}

impl Drop for Leading<'_> {
    fn drop(&mut self) {
        IN_HOOKS.with(|depth| depth.set(depth.get().saturating_sub(1)));
        lock(&self.runner.groups).entry(self.group).or_default().running_on = None;
        self.runner.ended.notify_all();
    }
}

/// [`run_hooks`] of one group. The run in progress covers a request that came while it ran by running again; a thread
/// inside a run of any group's hooks (a hook that set a setting) leaves its request to the run in progress, never
/// waiting for it; any other waits for the run in progress, up to [`WAIT_FOR_RUN`], and leads the next run itself when
/// nobody covered its request.
fn run_group(factory: &ExecutionContextFactory, group: Group, cause: &HookCause) -> GroupRan {
    let runner = factory.extension(|_| HookRunner::default());
    let me = std::thread::current().id();
    let inside = IN_HOOKS.with(|depth| depth.get() > 0);
    let deadline = std::time::Instant::now() + WAIT_FOR_RUN;
    let mut groups = lock(&runner.groups);
    let runs = groups.entry(group).or_default();
    runs.requested += 1;
    if !runs.causes.contains(cause) {
        runs.causes.push(cause.clone());
    }
    let ticket = runs.requested;
    loop {
        let runs = groups.entry(group).or_default();
        if runs.finished >= ticket {
            // a run that began after this request read the conf it made
            return GroupRan {
                problems: runs.problems.clone(),
                queued: false,
            };
        }
        let Some(running) = runs.running_on else {
            break;
        };
        // the run in progress sees the request when it ends, under this lock, and runs again
        if running == me || inside {
            return GroupRan {
                problems: Vec::new(),
                queued: true,
            };
        }
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            tracing::error!(
                "conf reload: a run of the {} hooks did not end within {} s; this change runs when it ends (is a hook \
                 waiting for a thread that sets the conf?)",
                group.name(),
                WAIT_FOR_RUN.as_secs()
            );
            return GroupRan {
                problems: vec![format!(
                    "{}: a run of its hooks did not end within {} s; this change runs when it ends",
                    group.name(),
                    WAIT_FOR_RUN.as_secs()
                )],
                queued: true,
            };
        }
        groups = wait_timeout(&runner.ended, groups, left).0;
    }
    groups.entry(group).or_default().running_on = Some(me);
    drop(groups);
    GroupRan {
        problems: lead(factory, &runner, group),
        queued: false,
    }
}

/// The runs of `group`'s hooks this thread leads, its own and those of the requests that came while it ran, up to
/// [`MAX_HOOK_RUNS`] in all; answers the last run's problems.
fn lead(factory: &ExecutionContextFactory, runner: &HookRunner, group: Group) -> Vec<String> {
    let _leading = Leading::new(runner, group);
    for _ in 1..MAX_HOOK_RUNS {
        let problems = run_once(factory, runner, group);
        let mut groups = lock(&runner.groups);
        let runs = groups.entry(group).or_default();
        if runs.requested == runs.started {
            return problems;
        }
        // the conf moved on while the hooks ran (a set or a reload, on another thread or from a hook): they run again,
        // so the last run reads the latest conf
    }
    let problems = run_once(factory, runner, group);
    let mut groups = lock(&runner.groups);
    let runs = groups.entry(group).or_default();
    if runs.requested != runs.started {
        tracing::error!(
            "conf reload: the {} hooks ran {MAX_HOOK_RUNS} times in all while the conf kept changing; a caller waiting \
             runs them again at once, else the next change does",
            group.name()
        );
    }
    problems
}

/// One run of `group`'s hooks for the requests so far, its state kept.
fn run_once(factory: &ExecutionContextFactory, runner: &HookRunner, group: Group) -> Vec<String> {
    let causes = {
        let mut groups = lock(&runner.groups);
        let runs = groups.entry(group).or_default();
        runs.started = runs.requested;
        std::mem::take(&mut runs.causes)
    };
    let problems = build_group(factory, group, &causes);
    let mut groups = lock(&runner.groups);
    let runs = groups.entry(group).or_default();
    runs.finished = runs.started;
    runs.problems.clone_from(&problems);
    problems
}

/// One run of `group`: its own state built again, then its hooks, each failing alone.
fn build_group(factory: &ExecutionContextFactory, group: Group, causes: &[HookCause]) -> Vec<String> {
    let mut problems = Vec::new();
    let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match group {
        Group::Screens => crate::screen::ScreenFacade::of(factory).reconf(factory),
        Group::Caches => match MNode::parse_text("conf", &factory.conf_text()) {
            Ok(conf) => match factory.cache().add_from_conf(&conf) {
                Ok(made) if !made.is_empty() => tracing::info!("Conf reload made caches {}", made.join(", ")),
                Ok(_) => {}
                Err(err) => problems.push(format!("caches: {err}")),
            },
            Err(err) => problems.push(format!("caches: {err}")),
        },
        // built again on first use; properties were set above
        Group::Derived | Group::WebProtection => {}
        // the host's hook queues a reconcile of its listener registry, which reads the running conf
        Group::Listeners => {}
    }));
    if built.is_err() {
        tracing::error!("conf reload: building the {} again panicked", group.name());
        problems.push(format!("{}: building it again failed", group.name()));
    }
    let hooks: Vec<Hook> = lock(&factory.extension(|_| Hooks::default()).0)
        .iter()
        .filter(|(of, _)| *of == group)
        .map(|(_, hook)| Arc::clone(hook))
        .collect();
    for hook in hooks {
        let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hook(factory, causes)));
        if ran.is_err() {
            tracing::error!("conf reload: a {} hook panicked", group.name());
            problems.push(format!("{}: the hook failed", group.name()));
        }
    }
    problems
}

fn restart_reason(class: Class) -> String {
    match class {
        Class::Restart(reason) => reason.to_string(),
        _ => String::new(),
    }
}

fn group_of(class: Class) -> Option<&'static str> {
    match class {
        Class::Apply(group) => Some(group.name()),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
