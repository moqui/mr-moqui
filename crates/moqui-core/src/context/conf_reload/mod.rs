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

pub mod catalog;
mod diff;
pub mod view;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use moqui_lib::env;
use moqui_lib::node::MNode;
use moqui_lib::sync::lock;
use serde::Serialize;

use super::{conf, ComponentInfo, Error, ExecutionContextFactory};
use catalog::{Class, Group};
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
    /// `added`, `removed`, `changed`, or `property` for a unit whose `${}` property changed.
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

/// A hook a conf reload runs after it replaced the conf, for state outside core (the host's guard).
type Hook = Arc<dyn Fn(&ExecutionContextFactory) + Send + Sync>;

#[derive(Default)]
struct Hooks(Mutex<Vec<(Group, Hook)>>);

/// The changes the last reload left waiting for a restart.
#[derive(Default)]
struct Pending(Mutex<Vec<ConfChange>>);

/// Held while a reload applies, so a save on System › Conf, the dev file watcher and SIGHUP do not each read the
/// running conf and replace it at once.
#[derive(Default)]
struct Applying(Mutex<()>);

/// Run `hook` after a conf reload that changed a unit of `group`.
pub fn on_apply(
    factory: &ExecutionContextFactory,
    group: Group,
    hook: impl Fn(&ExecutionContextFactory) + Send + Sync + 'static,
) {
    lock(&factory.extension(|_| Hooks::default()).0).push((group, Arc::new(hook)));
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
    conf::merge_files(&conf_files(factory, components)?, None)
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

/// Reload the conf files: apply what can be live, and name the rest as waiting for a restart.
///
/// # Errors
///
/// When a conf file does not parse; the running conf stays.
pub fn apply(factory: &ExecutionContextFactory) -> Result<ConfReport, Error> {
    apply_conf(factory, &merged_from_files(factory, &factory.components())?)
}

/// [`preview`] against the merged conf `disk`: a component about to be loaded (plans/archive/Component-Deploy.md, Phase 2).
///
/// # Errors
///
/// When the running conf does not parse.
pub fn preview_conf(factory: &ExecutionContextFactory, disk: &MNode) -> Result<ConfReport, Error> {
    run(factory, disk, false)
}

/// [`apply`] with the merged conf `disk`.
///
/// # Errors
///
/// When the running conf does not parse.
pub fn apply_conf(factory: &ExecutionContextFactory, disk: &MNode) -> Result<ConfReport, Error> {
    run(factory, disk, true)
}

/// A change as the reload decided it: the class it takes, and why when it is not the unit's.
struct Decided {
    change: UnitChange,
    class: Class,
    reason: String,
    /// For a `default-property` the reload sets: its name, raw value and whether it is a secret.
    set_property: Option<(String, String, bool)>,
    no_effect: bool,
}

fn run(factory: &ExecutionContextFactory, disk: &MNode, apply: bool) -> Result<ConfReport, Error> {
    let applying = apply.then(|| factory.extension(|_| Applying::default()));
    let _applying = applying.as_ref().map(|held| lock(&held.0));
    let started = std::time::Instant::now();
    let running = MNode::parse_text("running", &factory.conf_text())?;
    let mut decided: Vec<Decided> = diff::diff(&running, disk)
        .into_iter()
        .map(|change| Decided {
            class: change.class,
            reason: restart_reason(change.class),
            change,
            set_property: None,
            no_effect: false,
        })
        .collect();
    let mut report = ConfReport {
        generation: factory.conf_generation(),
        ..ConfReport::default()
    };
    for item in &mut decided {
        refine(factory, disk, item);
    }
    // A property is set only when every unit that reads it is live or applied; one read at boot would see it changed
    // while what it set up keeps the old value.
    let names: Vec<String> = decided
        .iter()
        .filter_map(|item| item.set_property.as_ref().map(|(name, _, _)| name.clone()))
        .collect();
    let users = diff::property_users(disk, &names);
    for item in &mut decided {
        let Some((name, _, _)) = &item.set_property else {
            continue;
        };
        if let Some((path, attr, _, _)) = users
            .iter()
            .find(|(_, _, class, user)| user == name && matches!(class, Class::Restart(_)))
        {
            item.reason = format!("{path}@{attr} reads it, and needs a restart");
            item.class = Class::Restart("a unit that reads it needs a restart");
            item.set_property = None;
        }
    }
    let set: Vec<&String> = decided
        .iter()
        .filter_map(|item| item.set_property.as_ref().map(|(name, _, _)| name))
        .collect();
    let property_changes: Vec<(String, String, Class, String)> = users
        .into_iter()
        .filter(|(_, _, _, name)| set.contains(&name))
        .collect();
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
        let mut outcome = match item.class {
            Class::Restart(_) => Outcome::PendingRestart,
            _ if item.no_effect => Outcome::NoEffect,
            _ if apply => Outcome::Applied,
            _ => Outcome::WouldApply,
        };
        if apply && outcome != Outcome::PendingRestart {
            if let Some((name, raw, secret)) = &item.set_property {
                if let Err(err) = factory.set_conf_property(name, raw, *secret) {
                    tracing::error!("conf reload: could not set property {name}: {err}");
                    report.problems.push(format!("property {name}: {err}"));
                    outcome = Outcome::Failed;
                }
            }
            if item.change.within.is_none() {
                item.change.edit.apply();
                changed = true;
            }
        }
        if let (Class::Apply(group), Outcome::Applied | Outcome::WouldApply) = (item.class, outcome) {
            if !groups.contains(&group) {
                groups.push(group);
            }
        }
        outcomes.push(outcome);
    }
    for (_, _, class, _) in &property_changes {
        if let Class::Apply(group) = class {
            if !groups.contains(group) {
                groups.push(*group);
            }
        }
    }

    if apply && (changed || !property_changes.is_empty()) {
        report.generation = factory.replace_conf(running.write_string());
        run_hooks(factory, &groups, &mut report);
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
    let live = if apply { Outcome::Applied } else { Outcome::WouldApply };
    for (path, attr, class, name) in property_changes {
        report.changes.push(ConfChange {
            unit: path,
            attribute: Some(attr),
            kind: "property",
            class: class.name(),
            group: group_of(class),
            reason: format!("reads the property {name}"),
            old: None,
            new: None,
            outcome: live,
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
    report.millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    Ok(report)
}

/// The class a change takes beyond its unit's: the rules of a `default-property` and of a `cache`.
fn refine(factory: &ExecutionContextFactory, disk: &MNode, item: &mut Decided) {
    match item.class {
        Class::Apply(Group::Properties) => refine_property(factory, disk, item),
        Class::Apply(Group::Caches) => {
            let added = item.change.kind == Kind::Added && item.change.attr.is_none();
            if !added && item.change.within.is_none() {
                item.class = Class::Restart("a cache's settings are read when it is made");
                item.reason = "a cache's settings are read when it is made".to_string();
            }
        }
        _ => {}
    }
}

/// A `default-property` change: a new property is set as boot sets it unless the environment set it; a changed value
/// is set while the property still has the value the conf set; a removed one, a secret's change, and the properties
/// the process was set up with wait for a restart.
fn refine_property(factory: &ExecutionContextFactory, disk: &MNode, item: &mut Decided) {
    let path = &item.change.path;
    let Some(name) = path
        .strip_prefix("default-property[")
        .and_then(|rest| rest.strip_suffix(']'))
        .filter(|name| !name.contains('#'))
        .map(str::to_string)
    else {
        return;
    };
    let restart = |item: &mut Decided, reason: String| {
        item.class = Class::Restart("a property read at boot");
        item.reason = reason;
    };
    let node = disk
        .children_named("default-property")
        .into_iter()
        .find(|node| node.attribute("name").as_deref() == Some(name.as_str()));
    let secret = super::factory::property_is_secret(
        &name,
        node.as_ref().and_then(|node| node.attribute("is-secret")).as_deref(),
    );
    if let Some(reason) = catalog::fixed_property(&name) {
        restart(item, reason.to_string());
        return;
    }
    let raw = node
        .as_ref()
        .and_then(|node| node.attribute("value"))
        .unwrap_or_default();
    match (item.change.kind, item.change.attr.as_deref()) {
        (Kind::Removed, None) => {
            restart(
                item,
                "a property the conf no longer sets is cleared at the next start".to_string(),
            );
        }
        (Kind::Added, None) => {
            if env::get_prop_or_env(&name).is_empty() {
                if raw.is_empty() {
                    item.no_effect = true;
                    item.reason = "empty".to_string();
                } else {
                    item.set_property = Some((name, raw, secret));
                }
            } else {
                item.no_effect = true;
                item.reason = "set by the environment, runtime/.env or -D, which wins".to_string();
            }
        }
        (_, Some("value")) => {
            if secret {
                restart(item, "a secret changes at the next start".to_string());
                return;
            }
            let current = env::get_prop_or_env(&name);
            let set_by_conf = factory
                .conf_properties()
                .get(&name)
                .is_some_and(|value| *value == current);
            if set_by_conf || current.is_empty() {
                item.set_property = Some((name, raw, secret));
            } else {
                item.no_effect = true;
                item.reason = "set by the environment, runtime/.env or -D, which wins".to_string();
            }
        }
        _ => {
            item.no_effect = true;
            item.reason = "only the value sets the property".to_string();
        }
    }
}

/// Build again the state of each group a change reached.
fn run_hooks(factory: &ExecutionContextFactory, groups: &[Group], report: &mut ConfReport) {
    for group in groups {
        match group {
            Group::Screens => crate::screen::ScreenFacade::of(factory).reconf(factory),
            Group::Caches => match MNode::parse_text("conf", &factory.conf_text()) {
                Ok(conf) => match factory.cache().add_from_conf(&conf) {
                    Ok(made) if !made.is_empty() => tracing::info!("Conf reload made caches {}", made.join(", ")),
                    Ok(_) => {}
                    Err(err) => report.problems.push(format!("caches: {err}")),
                },
                Err(err) => report.problems.push(format!("caches: {err}")),
            },
            // built again on first use; properties were set above
            Group::Derived | Group::Properties | Group::WebProtection => {}
        }
        let hooks: Vec<Hook> = lock(&factory.extension(|_| Hooks::default()).0)
            .iter()
            .filter(|(of, _)| of == group)
            .map(|(_, hook)| Arc::clone(hook))
            .collect();
        for hook in hooks {
            let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hook(factory)));
            if ran.is_err() {
                tracing::error!("conf reload: a {} hook panicked", group.name());
                report.problems.push(format!("{}: the hook failed", group.name()));
            }
        }
    }
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
