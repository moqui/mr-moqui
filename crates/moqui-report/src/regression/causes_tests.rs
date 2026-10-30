use super::*;

const HOUR: i64 = 3_600_000;
const ONSET: i64 = 1_800_000_000_000;

fn scope(locality: Locality, affected: &[&str], healthy: &[&str]) -> Scope {
    Scope {
        service: "shop".into(),
        candidate_version: "2.0".into(),
        baseline_version: Some("1.9".into()),
        onset: ONSET,
        locality,
        affected: affected.iter().map(|id| id.to_string()).collect(),
        healthy: healthy.iter().map(|id| id.to_string()).collect(),
        errors: 1_000,
        old_version_regressed: None,
    }
}

fn release() -> Candidate {
    Candidate::Release {
        version: "2.0".into(),
        revision: "abc123".into(),
        time: ONSET - 10 * 60_000,
    }
}

fn config(instance: Option<&str>, before: i64) -> Candidate {
    Candidate::Change {
        id: "marker-1".into(),
        source: "config".into(),
        time: ONSET - before,
        instance: instance.map(str::to_string),
        text: "pool size 8".into(),
    }
}

#[test]
fn a_version_wide_regression_ranks_the_release_first() {
    let ranked = rank(
        &scope(Locality::Version, &["a", "b"], &[]),
        &[
            config(None, 20 * HOUR),
            release(),
            Candidate::Instance { id: "a".into() },
        ],
        24 * HOUR,
    );
    assert_eq!(ranked[0].kind, "release", "{ranked:?}");
    assert!(ranked[0]
        .supporting
        .iter()
        .any(|text| text.contains("every instance of 2.0")));
    assert!(ranked[0].contradicting.is_empty());
    assert!(
        !ranked.iter().any(|cause| cause.kind == "instance"),
        "no instance fault in a version regression"
    );
    // The same ranking every time.
    assert_eq!(
        ranked,
        rank(
            &scope(Locality::Version, &["a", "b"], &[]),
            &[
                release(),
                Candidate::Instance { id: "a".into() },
                config(None, 20 * HOUR)
            ],
            24 * HOUR
        )
    );
}

#[test]
fn an_instance_fault_ranks_the_instance_and_contradicts_the_release() {
    let ranked = rank(
        &scope(Locality::Instance, &["a"], &["b", "c"]),
        &[release(), Candidate::Instance { id: "a".into() }],
        24 * HOUR,
    );
    assert_eq!(ranked[0].kind, "instance", "{ranked:?}");
    let release = ranked
        .iter()
        .find(|cause| cause.kind == "release")
        .expect("the release is still a cause");
    assert!(release
        .contradicting
        .iter()
        .any(|text| text.contains("only 1 of the instances")));
}

#[test]
fn a_config_change_on_the_failing_instance_outranks_an_older_release() {
    let mut old_release = release();
    if let Candidate::Release { time, .. } = &mut old_release {
        *time = ONSET - 20 * HOUR;
    }
    let ranked = rank(
        &scope(Locality::Instance, &["a"], &["b"]),
        &[old_release, config(Some("a"), 5 * 60_000), config(Some("b"), 60_000)],
        24 * HOUR,
    );
    assert_eq!(ranked[0].kind, "change", "{ranked:?}");
    assert!(ranked[0]
        .supporting
        .iter()
        .any(|text| text.contains("made on a, which regressed")));
}

#[test]
fn old_instances_regressing_too_contradict_the_release() {
    let mut both = scope(Locality::Version, &["a", "b"], &[]);
    both.old_version_regressed = Some(true);
    let ranked = rank(
        &both,
        &[
            release(),
            Candidate::Dependency {
                name: "postgresql".into(),
                ratio: 3.0,
            },
        ],
        24 * HOUR,
    );
    assert_eq!(ranked[0].kind, "dependency", "{ranked:?}");
    let release = ranked.iter().find(|cause| cause.kind == "release").unwrap();
    assert!(release
        .contradicting
        .iter()
        .any(|text| text.contains("still on 1.9 regressed")));
}

#[test]
fn new_error_groups_score_by_their_share_and_nearness() {
    let ranked = rank(
        &scope(Locality::Version, &["a", "b"], &[]),
        &[
            Candidate::Error {
                fingerprint: "fp-new".into(),
                kind: "NullPointer".into(),
                first_seen: ONSET + 60_000,
                count: 800,
            },
            Candidate::Error {
                fingerprint: "fp-old".into(),
                kind: "Timeout".into(),
                first_seen: ONSET - 72 * HOUR,
                count: 900,
            },
        ],
        24 * HOUR,
    );
    assert_eq!(
        ranked.len(),
        1,
        "an error group long before the onset explains nothing: {ranked:?}"
    );
    assert_eq!(ranked[0].reference, "fp-new");
    // Changes after the onset are not causes.
    assert!(rank(
        &scope(Locality::Version, &["a"], &[]),
        &[config(None, -HOUR)],
        24 * HOUR
    )
    .is_empty());
}
