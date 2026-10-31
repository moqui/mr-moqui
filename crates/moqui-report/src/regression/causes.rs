//! Deterministic ranking of a regression's probable causes (plans/Monitoring.md, Later: Regressions and incidents;
//! requirement 5). Each candidate scores by how close it came before the onset, how well its scope fits the instances
//! that regressed and those that did not, and how much of the deviation it can explain. Every cause keeps the evidence
//! for and against it and stays a hypothesis until an operator confirms it.

use super::compare::Locality;

/// What regressed, where and when.
#[derive(Clone, Debug)]
pub struct Scope {
    pub service: String,
    pub candidate_version: String,
    /// The version compared against, when the baseline was the previous version.
    pub baseline_version: Option<String>,
    /// Epoch milliseconds.
    pub onset: i64,
    pub locality: Locality,
    pub affected: Vec<String>,
    pub healthy: Vec<String>,
    /// Server errors in the candidate window, which error groups' counts are a share of.
    pub errors: u64,
    /// Whether instances still on the baseline version regressed over the same window too: `Some(true)` says the
    /// release is not the cause.
    pub old_version_regressed: Option<bool>,
}

#[derive(Clone, Debug)]
pub enum Candidate {
    Release {
        version: String,
        revision: String,
        time: i64,
    },
    /// A deploy or config annotation.
    Change {
        id: String,
        source: String,
        time: i64,
        instance: Option<String>,
        text: String,
    },
    Instance {
        id: String,
    },
    /// Statements or calls to a dependency that got slower: its p95 over the baseline's.
    Dependency {
        name: String,
        ratio: f64,
    },
    /// An error group first seen near the onset.
    Error {
        fingerprint: String,
        kind: String,
        first_seen: i64,
        count: u64,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ranked {
    pub kind: &'static str,
    pub reference: String,
    pub title: String,
    /// When the change was made or the error first seen, for the incident's timeline.
    pub time: Option<i64>,
    pub score: f64,
    pub supporting: Vec<String>,
    pub contradicting: Vec<String>,
}

/// How long before the onset a change still counts, at a third of its weight by `lookback`.
fn proximity(time: i64, onset: i64, lookback: i64) -> f64 {
    if time > onset || lookback <= 0 {
        return 0.0;
    }
    (-((onset - time) as f64) / (lookback as f64 / 3.0)).exp()
}

fn when(time: i64) -> String {
    jiff::Timestamp::from_millisecond(time)
        .map(|at| at.strftime("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|_| time.to_string())
}

/// The candidates ranked, best first, without those that explain nothing; at most ten.
pub fn rank(scope: &Scope, candidates: &[Candidate], lookback: i64) -> Vec<Ranked> {
    let mut ranked: Vec<Ranked> = candidates
        .iter()
        .filter_map(|candidate| score(scope, candidate, lookback))
        .filter(|ranked| ranked.score > 0.01)
        .collect();
    ranked.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.kind.cmp(b.kind))
            .then_with(|| a.reference.cmp(&b.reference))
    });
    ranked.truncate(10);
    ranked
}

fn score(scope: &Scope, candidate: &Candidate, lookback: i64) -> Option<Ranked> {
    let mut supporting = Vec::new();
    let mut contradicting = Vec::new();
    let time = match candidate {
        Candidate::Release { time, .. } | Candidate::Change { time, .. } => Some(*time),
        Candidate::Error { first_seen, .. } => Some(*first_seen),
        Candidate::Instance { .. } | Candidate::Dependency { .. } => None,
    };
    let (kind, reference, title, score) = match candidate {
        Candidate::Release {
            version,
            revision,
            time,
        } => {
            let near = proximity(*time, scope.onset, lookback);
            if near > 0.0 {
                supporting.push(format!("released {} before the onset", span_text(scope.onset - time)));
            } else {
                contradicting.push(format!("released at {}, after the onset", when(*time)));
            }
            let mut fit = match scope.locality {
                Locality::Version => {
                    supporting.push(format!("every instance of {version} regressed"));
                    1.0
                }
                Locality::Instance => {
                    contradicting.push(format!(
                        "only {} of the instances of {version} regressed; {} are healthy",
                        scope.affected.len(),
                        scope.healthy.len()
                    ));
                    0.3
                }
                Locality::Unknown => 0.6,
            };
            match (scope.old_version_regressed, &scope.baseline_version) {
                (Some(true), Some(old)) => {
                    contradicting.push(format!("instances still on {old} regressed over the same time"));
                    fit *= 0.2;
                }
                (Some(false), Some(old)) => supporting.push(format!("instances still on {old} did not regress")),
                _ => {}
            }
            (
                "release",
                format!("{}:{version}", scope.service),
                format!("Release {version} ({revision})"),
                near * fit,
            )
        }
        Candidate::Change {
            id,
            source,
            time,
            instance,
            text,
        } => {
            let near = proximity(*time, scope.onset, lookback);
            if near > 0.0 {
                supporting.push(format!(
                    "{source} change {} before the onset",
                    span_text(scope.onset - time)
                ));
            } else {
                contradicting.push(format!("{source} change at {}, after the onset", when(*time)));
            }
            let fit = match instance {
                Some(instance) if scope.affected.contains(instance) => {
                    supporting.push(format!("made on {instance}, which regressed"));
                    1.0
                }
                Some(instance) if scope.healthy.contains(instance) => {
                    contradicting.push(format!("made on {instance}, which is healthy"));
                    0.1
                }
                Some(_) => 0.3,
                None if scope.locality == Locality::Instance => {
                    contradicting.push("made for the whole service, but only some instances regressed".into());
                    0.4
                }
                None => 0.7,
            };
            ("change", id.clone(), format!("{source}: {text}"), near * fit)
        }
        Candidate::Instance { id } => {
            if scope.locality != Locality::Instance || !scope.affected.contains(id) {
                return None;
            }
            supporting.push(format!(
                "regressed while {} other instances of {} are healthy",
                scope.healthy.len(),
                scope.candidate_version
            ));
            (
                "instance",
                id.clone(),
                format!("Instance {id}"),
                0.9 / scope.affected.len() as f64,
            )
        }
        Candidate::Dependency { name, ratio } => {
            if *ratio <= 1.0 {
                return None;
            }
            supporting.push(format!("p95 of {name} rose {ratio:.1}x over the baseline"));
            (
                "dependency",
                name.clone(),
                format!("Dependency {name}"),
                ((ratio - 1.0) / 2.0).min(1.0) * 0.8,
            )
        }
        Candidate::Error {
            fingerprint,
            kind,
            first_seen,
            count,
        } => {
            // An error group new near the onset: before it by up to the lookback, or after it in the window.
            let distance = (first_seen - scope.onset).abs();
            let near = if distance <= lookback {
                (-(distance as f64) / (lookback as f64 / 3.0).max(1.0)).exp()
            } else {
                0.0
            };
            if near > 0.0 {
                supporting.push(format!("first seen at {}, near the onset", when(*first_seen)));
            } else {
                contradicting.push(format!("first seen at {}, long before the onset", when(*first_seen)));
            }
            let share = if scope.errors > 0 {
                (*count as f64 / scope.errors as f64).min(1.0)
            } else {
                0.0
            };
            supporting.push(format!(
                "{count} occurrences, {:.0}% of the window's errors",
                share * 100.0
            ));
            (
                "error",
                fingerprint.clone(),
                format!("New error {kind}"),
                near * share * 0.9,
            )
        }
    };
    Some(Ranked {
        kind,
        reference,
        title,
        time,
        score: (score * 1000.0).round() / 1000.0,
        supporting,
        contradicting,
    })
}

fn span_text(millis: i64) -> String {
    let minutes = millis / 60_000;
    if minutes < 120 {
        format!("{minutes} min")
    } else {
        format!("{} h", minutes / 60)
    }
}

#[cfg(test)]
#[path = "causes_tests.rs"]
mod tests;
