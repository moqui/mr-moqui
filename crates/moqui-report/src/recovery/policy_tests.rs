use super::*;

const NOW: i64 = 1_800_000_000_000;
const HOUR: i64 = 3_600_000;

fn step(action: &str, target: Option<&str>, adapter: Option<&str>) -> Step {
    Step {
        action: Action::parse(action).unwrap(),
        target: target.map(str::to_string),
        adapter: adapter.map(str::to_string),
    }
}

fn policy(mode: Mode, steps: Vec<Step>) -> Policy {
    Policy {
        id: "P".into(),
        mode,
        steps,
        adapter: Some("local".into()),
        min_confidence: 0.99,
        cooldown: HOUR,
        max_per_day: 3,
    }
}

fn facts() -> Facts {
    Facts {
        incident: "I1".into(),
        now: NOW,
        stopped: false,
        allow_automatic: true,
        confidence: 0.999,
        degraded: false,
        locality: "instance".into(),
        affected: vec!["a".into()],
        instances: 3,
        top_cause: Some(("release".into(), 0.8)),
        compatibility: Compatibility::Same,
        rollback_to: Some("1.9".into()),
        history: Vec::new(),
        adapters: vec![
            ("local".into(), AdapterKind::InProcess),
            ("deployer".into(), AdapterKind::Webhook),
            ("ops".into(), AdapterKind::Manual),
        ],
    }
}

fn ladder() -> Vec<Step> {
    // Out of order on purpose: the least disruptive allowed step wins.
    vec![
        step("rollback", None, Some("deployer")),
        step("isolate-instance", None, None),
        step("escalate", None, None),
    ]
}

#[test]
fn the_least_disruptive_allowed_action_is_chosen_and_advisory_only_proposes() {
    let outcome = decide(&policy(Mode::Advisory, ladder()), &facts());
    assert_eq!(outcome.step, step("isolate-instance", Some("a"), Some("local")));
    assert_eq!(outcome.status, Status::Proposed);
    assert_eq!(outcome.blast_radius, "1 of 3 instances");
    // Escalation is not chosen while an action is allowed; it is still considered, and observed first.
    assert_eq!(
        outcome.considered.iter().map(|step| step.action).collect::<Vec<_>>(),
        [Action::Escalate, Action::IsolateInstance, Action::Rollback]
    );
    assert_eq!(
        decide(&policy(Mode::Approval, ladder()), &facts()).status,
        Status::AwaitingApproval
    );
    assert_eq!(
        decide(&policy(Mode::Automatic, ladder()), &facts()).status,
        Status::Approved
    );
}

#[test]
fn a_version_wide_regression_rolls_back_only_when_compatible() {
    let mut version = facts();
    version.locality = "version".into();
    version.affected = vec!["a".into(), "b".into(), "c".into()];
    let outcome = decide(&policy(Mode::Automatic, ladder()), &version);
    assert_eq!(outcome.step, step("rollback", Some("1.9"), Some("deployer")));
    assert_eq!(outcome.status, Status::Approved);
    version.compatibility = Compatibility::Different;
    let outcome = decide(&policy(Mode::Automatic, ladder()), &version);
    assert_eq!(outcome.step.action, Action::Escalate);
    assert_eq!(outcome.status, Status::Blocked);
    let why = outcome.blocked.unwrap();
    assert!(
        why.contains("rollback: the entity schema differs")
            && why.contains("isolate-instance: the regression is not on some instances"),
        "{why}"
    );
}

#[test]
fn an_automatic_rollback_needs_the_release_ranked_first() {
    let mut version = facts();
    version.locality = "version".into();
    version.top_cause = Some(("dependency".into(), 0.9));
    let outcome = decide(
        &policy(Mode::Automatic, vec![step("rollback", None, Some("deployer"))]),
        &version,
    );
    assert_eq!(outcome.status, Status::AwaitingApproval);
    assert!(outcome.blocked.unwrap().contains("release ranked first"));
    version.top_cause = Some(("release".into(), 0.9));
    version.allow_automatic = false;
    let outcome = decide(
        &policy(Mode::Automatic, vec![step("rollback", None, Some("deployer"))]),
        &version,
    );
    assert_eq!(outcome.status, Status::AwaitingApproval);
    assert!(outcome.blocked.unwrap().contains("allow-automatic"));
}

#[test]
fn the_stop_low_confidence_degraded_data_and_blast_radius_block() {
    let mut stopped = facts();
    stopped.stopped = true;
    let outcome = decide(&policy(Mode::Automatic, ladder()), &stopped);
    assert_eq!(
        (outcome.step.action, outcome.status),
        (Action::Escalate, Status::Blocked)
    );
    assert!(outcome.blocked.unwrap().contains("emergency stop"));
    let mut unsure = facts();
    unsure.confidence = 0.9;
    assert!(decide(&policy(Mode::Automatic, ladder()), &unsure)
        .blocked
        .unwrap()
        .contains("confidence 90.0%"));
    let mut degraded = facts();
    degraded.degraded = true;
    assert!(decide(&policy(Mode::Automatic, ladder()), &degraded)
        .blocked
        .unwrap()
        .contains("degraded"));
    let mut half = facts();
    half.affected = vec!["a".into(), "b".into()];
    half.instances = 4;
    assert!(decide(&policy(Mode::Automatic, ladder()), &half)
        .blocked
        .unwrap()
        .contains("half or more"));
}

#[test]
fn one_active_action_cooldown_reversal_and_the_daily_limit() {
    let past = |action: &str, incident: &str, ago: i64, active: bool| Past {
        action: Action::parse(action).unwrap(),
        target: None,
        incident: incident.into(),
        at: NOW - ago,
        active,
    };
    let mut running = facts();
    running.history = vec![past("pause-job", "I0", HOUR * 5, true)];
    assert!(decide(&policy(Mode::Automatic, ladder()), &running)
        .blocked
        .unwrap()
        .contains("another action is running"));

    let mut promoted = facts();
    promoted.locality = "version".into();
    promoted.history = vec![past("promote", "I0", HOUR / 2, false)];
    let why = decide(&policy(Mode::Automatic, ladder()), &promoted).blocked.unwrap();
    assert!(why.contains("would reverse the promote"), "{why}");

    let mut cooled = facts();
    cooled.history = vec![past("pause-job", "I0", HOUR / 4, false)];
    assert!(decide(&policy(Mode::Automatic, ladder()), &cooled)
        .blocked
        .unwrap()
        .contains("cooldown"));

    let mut busy = facts();
    busy.history = (1..=3)
        .map(|hours| past("pause-job", "I0", HOUR * (hours + 1), false))
        .collect();
    assert!(decide(&policy(Mode::Automatic, ladder()), &busy)
        .blocked
        .unwrap()
        .contains("3 actions in this scope today"));

    // A step tried for this incident is not tried again; escalation repeats only after the cooldown.
    let mut tried = facts();
    tried.history = vec![
        Past {
            action: Action::IsolateInstance,
            target: Some("a".into()),
            incident: "I1".into(),
            at: NOW - 2 * HOUR,
            active: false,
        },
        past("escalate", "I1", HOUR / 2, false),
    ];
    let outcome = decide(
        &policy(
            Mode::Automatic,
            vec![step("isolate-instance", None, None), step("escalate", None, None)],
        ),
        &tried,
    );
    assert_eq!(outcome.step.action, Action::Observe);
    assert!(outcome.blocked.unwrap().contains("already tried"));
}

#[test]
fn adapters_and_targets_must_fit_the_action() {
    let outcome = decide(
        &policy(
            Mode::Automatic,
            vec![step("rollback", None, Some("local")), step("pause-job", None, None)],
        ),
        &{
            let mut version = facts();
            version.locality = "version".into();
            version
        },
    );
    let why = outcome.blocked.unwrap();
    assert!(
        why.contains("adapter local cannot carry it out") && why.contains("names no target"),
        "{why}"
    );
    let manual = decide(
        &policy(Mode::Approval, vec![step("restart", None, Some("ops"))]),
        &facts(),
    );
    assert_eq!(
        (manual.step.action, manual.status),
        (Action::Restart, Status::AwaitingApproval)
    );
}
