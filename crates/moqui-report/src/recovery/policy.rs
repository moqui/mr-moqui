//! The recovery decision (plans/Monitoring.md, Later: Recovery; requirement 6): of a policy's ladder, the least
//! disruptive action that the evidence, the incident's locality, the compatibility checks, the cooldown, the reversal
//! rule, the daily limit, the scope's active action and the emergency stop all allow. Every step considered keeps the
//! reason it was or was not taken, which is what an operator sees as why automation stopped; when nothing beyond
//! escalation is allowed, the decision is to escalate. There is no action that reverses committed business
//! transactions. Pure, so the same facts always give the same decision.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Action {
    Observe,
    Escalate,
    PauseAlertRule,
    PauseJob,
    DisableArtifact,
    IsolateInstance,
    PauseRollout,
    Promote,
    Restart,
    Rollback,
}

impl Action {
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "observe" => Self::Observe,
            "escalate" => Self::Escalate,
            "pause-alert-rule" => Self::PauseAlertRule,
            "pause-job" => Self::PauseJob,
            "disable-artifact" => Self::DisableArtifact,
            "isolate-instance" => Self::IsolateInstance,
            "pause-rollout" => Self::PauseRollout,
            "promote" => Self::Promote,
            "restart" => Self::Restart,
            "rollback" => Self::Rollback,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::Escalate => "escalate",
            Self::PauseAlertRule => "pause-alert-rule",
            Self::PauseJob => "pause-job",
            Self::DisableArtifact => "disable-artifact",
            Self::IsolateInstance => "isolate-instance",
            Self::PauseRollout => "pause-rollout",
            Self::Promote => "promote",
            Self::Restart => "restart",
            Self::Rollback => "rollback",
        }
    }

    /// How disruptive the action is: the ladder is tried in this order.
    pub fn disruption(self) -> u8 {
        match self {
            Self::Observe => 0,
            Self::Escalate => 1,
            Self::PauseAlertRule => 2,
            Self::PauseJob => 3,
            Self::DisableArtifact => 4,
            Self::IsolateInstance | Self::PauseRollout | Self::Promote => 5,
            Self::Restart => 6,
            Self::Rollback => 7,
        }
    }

    /// Observing and escalating change nothing in the system, so they need no evidence, approval or cooldown.
    pub fn changes_system(self) -> bool {
        !matches!(self, Self::Observe | Self::Escalate)
    }

    /// Whether this action undoes `other`, which the reversal rule keeps from following it inside the cooldown.
    pub fn reverses(self, other: Action) -> bool {
        matches!(
            (self, other),
            (Self::Rollback, Self::Promote) | (Self::Promote, Self::Rollback) | (Self::Promote, Self::PauseRollout)
        )
    }

    /// Whether an adapter of `kind` can carry the action out.
    pub fn supported_by(self, kind: AdapterKind) -> bool {
        match kind {
            AdapterKind::InProcess => matches!(
                self,
                Self::Observe
                    | Self::Escalate
                    | Self::PauseAlertRule
                    | Self::PauseJob
                    | Self::DisableArtifact
                    | Self::IsolateInstance
            ),
            AdapterKind::Webhook => self != Self::Observe,
            AdapterKind::Manual => true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdapterKind {
    InProcess,
    Webhook,
    Manual,
}

impl AdapterKind {
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "in-process" => Self::InProcess,
            "webhook" => Self::Webhook,
            "manual" => Self::Manual,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Advisory,
    Approval,
    Automatic,
}

impl Mode {
    pub fn parse(text: &str) -> Self {
        match text {
            "approval" => Self::Approval,
            "automatic" => Self::Automatic,
            _ => Self::Advisory,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub action: Action,
    pub target: Option<String>,
    pub adapter: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Policy {
    pub id: String,
    pub mode: Mode,
    pub steps: Vec<Step>,
    pub adapter: Option<String>,
    pub min_confidence: f64,
    pub cooldown: i64,
    pub max_per_day: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compatibility {
    Same,
    Different,
    Unknown,
}

/// An earlier action in the scope.
#[derive(Clone, Debug)]
pub struct Past {
    pub action: Action,
    pub target: Option<String>,
    pub incident: String,
    pub at: i64,
    /// Persisted, Running or waiting for an operator.
    pub active: bool,
}

/// What a decision is made from.
#[derive(Clone, Debug)]
pub struct Facts {
    pub incident: String,
    pub now: i64,
    pub stopped: bool,
    pub allow_automatic: bool,
    /// The highest confidence of the incident's open regressions; 0 with none.
    pub confidence: f64,
    pub degraded: bool,
    /// version, instance or unknown.
    pub locality: String,
    pub affected: Vec<String>,
    /// Instances of the version, regressed or not.
    pub instances: usize,
    /// The first-ranked cause: its kind and score.
    pub top_cause: Option<(String, f64)>,
    pub compatibility: Compatibility,
    /// The version a rollback goes back to.
    pub rollback_to: Option<String>,
    pub history: Vec<Past>,
    /// The adapters there are, by id.
    pub adapters: Vec<(String, AdapterKind)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Advisory: recorded for an operator, who may approve it.
    Proposed,
    AwaitingApproval,
    Approved,
    /// Nothing beyond escalation was allowed.
    Blocked,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "Proposed",
            Self::AwaitingApproval => "AwaitingApproval",
            Self::Approved => "Approved",
            Self::Blocked => "Blocked",
        }
    }
}

/// One step of the ladder and why it was or was not taken.
#[derive(Clone, Debug, PartialEq)]
pub struct Considered {
    pub action: Action,
    pub target: Option<String>,
    pub refused: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub step: Step,
    pub status: Status,
    /// Why automation stopped: the reasons the steps beyond escalation were not taken, or why an automatic policy
    /// waits for approval.
    pub blocked: Option<String>,
    pub blast_radius: String,
    pub considered: Vec<Considered>,
}

const DAY: i64 = 86_400_000;

/// The decision for an incident under a policy.
pub fn decide(policy: &Policy, facts: &Facts) -> Outcome {
    let mut steps = policy.steps.clone();
    steps.sort_by_key(|step| step.action.disruption());
    let mut considered = Vec::new();
    // The first allowed step that changes the system, else the first allowed escalation or observation. The ladder is
    // read to its end, so the screen can show every step's reason.
    let mut chosen: Option<Step> = None;
    let mut fallback: Option<Step> = None;
    for step in steps {
        let target = resolve_target(&step, facts);
        let adapter = step.adapter.clone().or_else(|| policy.adapter.clone());
        let refused = refusal(policy, facts, step.action, target.as_deref(), adapter.as_deref());
        considered.push(Considered {
            action: step.action,
            target: target.clone(),
            refused: refused.clone(),
        });
        if refused.is_none() {
            let slot = if step.action.changes_system() {
                &mut chosen
            } else {
                &mut fallback
            };
            slot.get_or_insert(Step {
                action: step.action,
                target,
                adapter,
            });
        }
    }
    let chosen = chosen.or(fallback);
    let reasons: Vec<String> = considered
        .iter()
        .filter(|step| step.action.changes_system())
        .filter_map(|step| {
            step.refused
                .as_ref()
                .map(|reason| format!("{}: {reason}", step.action.as_str()))
        })
        .collect();
    let step = match chosen {
        Some(step) if step.action.changes_system() => step,
        // Nothing that changes the system is allowed: escalate, or observe when escalation is not on the ladder or
        // was just done.
        other => {
            let step = other.unwrap_or(Step {
                action: Action::Observe,
                target: None,
                adapter: None,
            });
            let status = if reasons.is_empty() {
                Status::Approved
            } else {
                Status::Blocked
            };
            return Outcome {
                blast_radius: "none".into(),
                step,
                status,
                blocked: (!reasons.is_empty()).then(|| format!("No safe, authorized action: {}", reasons.join("; "))),
                considered,
            };
        }
    };
    let blast_radius = match step.action {
        Action::IsolateInstance => format!("1 of {} instances", facts.instances.max(1)),
        Action::PauseJob | Action::PauseAlertRule | Action::DisableArtifact => {
            format!("{} {}", step.action.as_str(), step.target.clone().unwrap_or_default())
        }
        _ => "the service".into(),
    };
    let (status, blocked) = match policy.mode {
        Mode::Advisory => (Status::Proposed, None),
        Mode::Approval => (Status::AwaitingApproval, None),
        Mode::Automatic => match automatic_refusal(facts, step.action) {
            None => (Status::Approved, None),
            Some(reason) => (Status::AwaitingApproval, Some(reason)),
        },
    };
    Outcome {
        step,
        status,
        blocked,
        blast_radius,
        considered,
    }
}

fn resolve_target(step: &Step, facts: &Facts) -> Option<String> {
    match step.action {
        Action::IsolateInstance => step.target.clone().or_else(|| facts.affected.first().cloned()),
        Action::Rollback => step.target.clone().or_else(|| facts.rollback_to.clone()),
        _ => step.target.clone(),
    }
}

/// Why a step may not be taken now, or None when it may.
fn refusal(
    policy: &Policy,
    facts: &Facts,
    action: Action,
    target: Option<&str>,
    adapter: Option<&str>,
) -> Option<String> {
    let in_scope = |past: &&Past| past.action.changes_system();
    if action == Action::Observe {
        return None;
    }
    if action == Action::Escalate {
        let recent = facts.history.iter().any(|past| {
            past.action == Action::Escalate && past.incident == facts.incident && facts.now - past.at < policy.cooldown
        });
        return recent.then(|| "escalated within the cooldown".into());
    }
    if facts.stopped {
        return Some("the emergency stop is on".into());
    }
    if facts.history.iter().filter(in_scope).any(|past| past.active) {
        return Some("another action is running in this scope".into());
    }
    if facts
        .history
        .iter()
        .any(|past| past.incident == facts.incident && past.action == action && past.target.as_deref() == target)
    {
        return Some("already tried for this incident".into());
    }
    if facts.confidence < policy.min_confidence {
        return Some(format!(
            "regression confidence {:.1}% is under {:.1}%",
            facts.confidence * 100.0,
            policy.min_confidence * 100.0
        ));
    }
    if facts.degraded && action.disruption() >= 5 {
        return Some("the evidence is from degraded telemetry".into());
    }
    let Some(adapter) = adapter else {
        return Some("no adapter".into());
    };
    let Some((_, kind)) = facts.adapters.iter().find(|(id, _)| id == adapter) else {
        return Some(format!("adapter {adapter} does not exist"));
    };
    if !action.supported_by(*kind) {
        return Some(format!("adapter {adapter} cannot carry it out"));
    }
    match action {
        Action::PauseJob | Action::PauseAlertRule | Action::DisableArtifact if target.is_none() => {
            return Some("the step names no target".into());
        }
        Action::IsolateInstance => {
            if facts.locality != "instance" || target.is_none() {
                return Some("the regression is not on some instances only".into());
            }
            if facts.affected.len() * 2 >= facts.instances.max(1) {
                return Some("it would isolate half or more of the instances".into());
            }
        }
        Action::Rollback => {
            if facts.locality == "instance" {
                return Some("only some instances regressed; a rollback is not the least disruptive fix".into());
            }
            if target.is_none() {
                return Some("there is no previous version".into());
            }
            match facts.compatibility {
                Compatibility::Same => {}
                Compatibility::Different => return Some("the entity schema differs between the versions".into()),
                Compatibility::Unknown => return Some("schema compatibility of the versions is not known".into()),
            }
        }
        _ => {}
    }
    let recent: Vec<&Past> = facts
        .history
        .iter()
        .filter(in_scope)
        .filter(|past| facts.now - past.at < policy.cooldown)
        .collect();
    if let Some(past) = recent.iter().find(|past| action.reverses(past.action)) {
        return Some(format!(
            "it would reverse the {} inside the cooldown",
            past.action.as_str()
        ));
    }
    if !recent.is_empty() {
        return Some("another action finished inside the cooldown".into());
    }
    let today = facts
        .history
        .iter()
        .filter(in_scope)
        .filter(|past| facts.now - past.at < DAY)
        .count();
    if today >= policy.max_per_day {
        return Some(format!("{today} actions in this scope today, the most allowed"));
    }
    None
}

/// Why an automatic policy's action still waits for approval, or None when it may run.
fn automatic_refusal(facts: &Facts, action: Action) -> Option<String> {
    if !facts.allow_automatic {
        return Some("automatic actions are not allowed (telemetry/recovery allow-automatic)".into());
    }
    if action == Action::Rollback {
        // An automatic rollback needs the release to be the leading cause, by a clear score.
        match &facts.top_cause {
            Some((kind, score)) if kind == "release" && *score >= 0.5 => {}
            _ => {
                return Some("an automatic rollback needs the release ranked first with a score of 0.5 or more".into())
            }
        }
    }
    None
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
