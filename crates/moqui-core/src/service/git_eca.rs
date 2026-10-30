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

//! gitECA rules: `<giteca>` elements of the `*.gitecas.xml` files in a component's `service/` directory, run on the
//! events of a git repository as email ECA rules run on a received message. No Java counterpart. A rule names its
//! `event` and is narrowed by `source`, `repository-pattern`, `ref-pattern`, `ref-type` and `default-branch-only`;
//! its condition and actions are xml-actions, with the event's values in the context (moqui-git's `events`). The
//! pre events (`pre-receive`, `pre-ref-update`) run in a push before it is accepted: an error a rule leaves in the
//! message facade refuses the push, or that ref. The post events (`ref-update`, `commit`, `tag`, `post-receive`) run
//! after refs changed, each rule in a transaction of its own; one that fails is logged and recorded on the
//! `GitUpdate`, and the others still run. Rules run with authorization off: they are a component's own code, and the
//! repository was checked when it was reached. Schema: `framework/xsd/git-eca-3.xsd`; guide: `docs/interfaces/Git-Hosting.md`.
//!
//! The git events are moqui-git's. A build without it still reads and checks the rules, so a component's files load
//! the same; nothing runs them, since there are no git events.

use std::collections::BTreeMap;

use moqui_lib::node::MNode;
use moqui_lib::object::{CompiledRegex, Value};

use super::eca_rules;
use crate::context::{Error, ExecutionContext};

/// The events a rule runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GitEvent {
    /// Once per push, before any ref changes: an error refuses the whole push.
    PreReceive,
    /// Once per ref of a push, before it changes: an error refuses that ref (all of them in an atomic push).
    PreRefUpdate,
    /// Once per ref that changed.
    RefUpdate,
    /// Once per commit new to the repository.
    Commit,
    /// Once per tag created.
    Tag,
    /// Once per change, after the others.
    PostReceive,
}

impl GitEvent {
    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "pre-receive" => Self::PreReceive,
            "pre-ref-update" => Self::PreRefUpdate,
            "ref-update" => Self::RefUpdate,
            "commit" => Self::Commit,
            "tag" => Self::Tag,
            "post-receive" => Self::PostReceive,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::PreReceive => "pre-receive",
            Self::PreRefUpdate => "pre-ref-update",
            Self::RefUpdate => "ref-update",
            Self::Commit => "commit",
            Self::Tag => "tag",
            Self::PostReceive => "post-receive",
        }
    }
}

/// The sources a rule without `source` runs for: a write through `git://` (a wiki save, say) is opted into.
const DEFAULT_SOURCES: [&str; 3] = ["push", "fetch", "import"];

pub struct GitRule {
    pub(crate) id: Option<String>,
    pub(crate) name: String,
    pub(crate) priority: i64,
    event: GitEvent,
    sources: Vec<String>,
    repository: Option<CompiledRegex>,
    ref_pattern: Option<CompiledRegex>,
    ref_type: Option<String>,
    default_branch_only: bool,
    run_on_error: bool,
    condition: String,
    actions: String,
}

impl std::fmt::Debug for GitRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitRule")
            .field("name", &self.name)
            .field("event", &self.event)
            .finish()
    }
}

fn pattern(node: &MNode, attribute: &str) -> Result<Option<CompiledRegex>, Error> {
    match node.attribute(attribute).filter(|text| !text.trim().is_empty()) {
        Some(text) => CompiledRegex::new(text.trim())
            .map(Some)
            .map_err(|err| Error::new(format!("{attribute} {text} is not a valid pattern: {}", err.message()))),
        None => Ok(None),
    }
}

/// A rule from its `<giteca>` element. An unknown event or a pattern that does not parse is an error, so the loader
/// skips the rule.
pub(super) fn git_rule_from(node: &MNode) -> Result<GitRule, Error> {
    let name = node.attribute("rule-name").unwrap_or_default();
    let event_text = node.attribute("event").unwrap_or_default();
    let event = GitEvent::parse(event_text.trim())
        .ok_or_else(|| Error::new(format!("gitECA rule {name} has event '{event_text}'")))?;
    let sources: Vec<String> = match node.attribute("source").filter(|text| !text.trim().is_empty()) {
        Some(text) => text
            .split(',')
            .map(|source| source.trim().to_string())
            .filter(|source| !source.is_empty())
            .collect(),
        None => DEFAULT_SOURCES.iter().map(|source| (*source).to_string()).collect(),
    };
    Ok(GitRule {
        id: node.attribute("id").filter(|id| !id.is_empty()),
        priority: node
            .attribute("priority")
            .and_then(|priority| priority.trim().parse().ok())
            .unwrap_or(5),
        event,
        sources,
        repository: pattern(node, "repository-pattern")?,
        ref_pattern: pattern(node, "ref-pattern")?,
        ref_type: node.attribute("ref-type").filter(|text| !text.trim().is_empty()),
        default_branch_only: node.attribute_is("default-branch-only", "true"),
        run_on_error: node.attribute_is("run-on-error", "true"),
        condition: super::rule_actions::first_condition(node),
        actions: node
            .first_named("actions")
            .map(|child| child.write_string())
            .unwrap_or_default(),
        name,
    })
}

/// What an event is matched against.
#[derive(Clone, Copy, Debug)]
pub struct GitEventKey<'a> {
    pub event: GitEvent,
    pub source: &'a str,
    pub repository_id: &'a str,
    /// The ref of a per-ref event, or the one a commit arrived on.
    pub ref_name: Option<&'a str>,
    pub is_default_branch: bool,
    /// Errors are already recorded for this change (a rule with `run-on-error` still runs).
    pub has_errors: bool,
}

impl GitRule {
    fn matches(&self, key: &GitEventKey<'_>) -> bool {
        if self.event != key.event
            || !self.sources.iter().any(|source| source == key.source)
            || (key.has_errors && !self.run_on_error)
        {
            return false;
        }
        if self
            .repository
            .as_ref()
            .is_some_and(|pattern| !pattern.is_full_match(key.repository_id))
        {
            return false;
        }
        if self.default_branch_only && !key.is_default_branch {
            return false;
        }
        let ref_name = key.ref_name.unwrap_or_default();
        if let Some(ref_type) = &self.ref_type {
            let kind = if ref_name.starts_with("refs/heads/") {
                "branch"
            } else if ref_name.starts_with("refs/tags/") {
                "tag"
            } else {
                "other"
            };
            if ref_type != kind {
                return false;
            }
        }
        !self
            .ref_pattern
            .as_ref()
            .is_some_and(|pattern| key.ref_name.is_none() || !pattern.is_full_match(ref_name))
    }
}

/// Whether any rule of the entered context's factory runs on `event` from `source`, so the caller can skip working
/// out the values of an event no rule reads.
pub fn listening(event: GitEvent, source: &str) -> bool {
    eca_rules::current().is_some_and(|rules| {
        rules
            .git
            .iter()
            .any(|rule| rule.event == event && rule.sources.iter().any(|candidate| candidate == source))
    })
}

/// Whether any rule runs on a post event (ref-update, commit, tag, post-receive) from `source`.
pub fn listening_after(source: &str) -> bool {
    [
        GitEvent::RefUpdate,
        GitEvent::Commit,
        GitEvent::Tag,
        GitEvent::PostReceive,
    ]
    .into_iter()
    .any(|event| listening(event, source))
}

/// What the rules of one event said: the errors (which refuse a pre event) and the other messages.
#[derive(Debug, Default)]
pub struct RuleOutcome {
    pub errors: Vec<String>,
    pub messages: Vec<String>,
}

/// Take the message facade's errors and messages, leaving it empty.
fn take_messages(outcome: &mut RuleOutcome) {
    ExecutionContext::with_current(|ec| {
        let mut message = ec.message();
        outcome.errors.extend(message.errors_text());
        outcome.messages.extend(message.messages().iter().cloned());
        message.clear();
    });
}

/// Run the rules that match `key`, each with `fields` in its context, with authorization off; with `transactions` each
/// in a transaction of its own that an error rolls back. A rule that fails is logged and the others still run.
pub fn run_git_rules(
    key: &GitEventKey<'_>,
    fields: &BTreeMap<String, Value>,
    transactions: Option<&crate::transaction::TransactionManager>,
) -> RuleOutcome {
    let mut outcome = RuleOutcome::default();
    let Some(rules) = eca_rules::current() else {
        return outcome;
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    for rule in rules.git.iter().filter(|rule| rule.matches(key)) {
        let body = || -> Result<(), Error> {
            let _frame = super::rule_actions::publish_names(fields)?;
            if super::rule_actions::condition_allows(&rule.condition)? {
                super::rule_actions::run_stored_actions(&rule.actions)?;
            }
            let failed = ExecutionContext::with_current(|ec| !ec.message().errors_text().is_empty()).unwrap_or(false);
            if failed {
                return Err(Error::new(format!("gitECA rule {} left errors", rule.name)));
            }
            Ok(())
        };
        let ran = match transactions {
            Some(transactions) => transactions
                .run(crate::transaction::TransactionAttribute::ForceNew, None, || {
                    body().map_err(moqui_lib::error::Error::from)
                })
                .map_err(Error::from),
            None => body(),
        };
        let before = outcome.errors.len();
        take_messages(&mut outcome);
        if let Err(err) = ran {
            if outcome.errors.len() == before {
                outcome.errors.push(err.message().to_string());
            }
            tracing::error!(
                rule = %rule.name,
                event = key.event.name(),
                repository = key.repository_id,
                "gitECA rule failed: {}",
                outcome.errors[before..].join("; ")
            );
        }
    }
    outcome
}

/// Install rules from XML on a factory, for tests.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn install(factory: &std::sync::Arc<crate::context::ExecutionContextFactory>, xml: &str) {
    eca_rules::install(factory, eca_rules::from_xml("", "").with_git(xml));
}
