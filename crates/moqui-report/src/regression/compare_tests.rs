use super::*;

const BOUNDS: [f64; 4] = [0.1, 0.25, 0.5, 1.0];

fn stats(requests: u64, errors: u64, buckets: [u64; 5]) -> Stats {
    Stats {
        requests,
        errors,
        operation_runs: requests,
        operation_failures: errors,
        bounds: BOUNDS.to_vec(),
        buckets: buckets.to_vec(),
        minutes_covered: 60,
        minutes_expected: 60,
        telemetry_lost: 0,
    }
}

#[test]
fn the_statistics_match_known_values() {
    assert!((normal_cdf(0.0) - 0.5).abs() < 1e-7);
    assert!((normal_cdf(1.959_964) - 0.975).abs() < 1e-6);
    assert!((normal_cdf(-2.326_348) - 0.01).abs() < 1e-6);
    // chi-square with 1 degree of freedom at 3.841 is p = 0.05; with 3 at 7.815 also.
    assert!((upper_gamma_regularized(0.5, 3.841_459 / 2.0) - 0.05).abs() < 1e-6);
    assert!((upper_gamma_regularized(1.5, 7.814_728 / 2.0) - 0.05).abs() < 1e-6);
    assert!((ln_gamma(5.0) - 24f64.ln()).abs() < 1e-12);
    let (low, high) = wilson(10, 100);
    assert!(
        (low - 0.0552).abs() < 1e-3 && (high - 0.1744).abs() < 1e-3,
        "{low} {high}"
    );
}

#[test]
fn an_error_rate_rise_is_a_regression_with_its_confidence_and_severity() {
    let th = Thresholds::default();
    let baseline = stats(10_000, 50, [9000, 800, 150, 40, 10]);
    let candidate = stats(5_000, 150, [4500, 400, 75, 20, 5]);
    let verdict = compare(Metric::ErrorRate, &baseline, &candidate, &th);
    assert!(verdict.regressed, "{verdict:?}");
    assert_eq!(verdict.quality, Quality::Good);
    assert_eq!(verdict.severity, Some(Severity::Major));
    assert!((verdict.baseline_value - 0.005).abs() < 1e-12 && (verdict.candidate_value - 0.03).abs() < 1e-12);
    assert!((verdict.magnitude - 6.0).abs() < 1e-9);
    assert!(verdict.confidence > 0.999);
    assert!(verdict.candidate_interval.0 > verdict.baseline_interval.1);
    // The same rise in a tenth of the traffic is a critical one when it is large enough.
    let worse = stats(5_000, 1_000, [4500, 400, 75, 20, 5]);
    assert_eq!(
        compare(Metric::ErrorRate, &baseline, &worse, &th).severity,
        Some(Severity::Critical)
    );
    // A rise under rate-delta, however certain, is not a regression.
    let slight = stats(100_000, 900, [90000, 8000, 1500, 400, 100]);
    let same = stats(100_000, 500, [90000, 8000, 1500, 400, 100]);
    let verdict = compare(Metric::ErrorRate, &same, &slight, &th);
    assert!(verdict.confidence > 0.999 && !verdict.regressed, "{verdict:?}");
}

#[test]
fn no_rise_and_noise_are_not_regressions() {
    let th = Thresholds::default();
    let baseline = stats(1_000, 10, [900, 80, 15, 4, 1]);
    assert!(
        !compare(
            Metric::ErrorRate,
            &baseline,
            &stats(1_000, 12, [900, 80, 15, 4, 1]),
            &th
        )
        .regressed
    );
    assert!(!compare(Metric::ErrorRate, &baseline, &stats(1_000, 3, [900, 80, 15, 4, 1]), &th).regressed);
    let none = stats(1_000, 0, [900, 80, 15, 4, 1]);
    let verdict = compare(Metric::ErrorRate, &none, &none, &th);
    // Equal rates are a z of 0: no evidence either way (to the normal function's 1e-7).
    assert!(
        !verdict.regressed && (verdict.confidence - 0.5).abs() < 1e-6,
        "{verdict:?}"
    );
}

#[test]
fn too_few_samples_or_gaps_never_read_as_healthy() {
    let th = Thresholds::default();
    let baseline = stats(10_000, 50, [9000, 800, 150, 40, 10]);
    let verdict = compare(Metric::ErrorRate, &baseline, &stats(150, 100, [100, 30, 10, 5, 5]), &th);
    assert!(matches!(verdict.quality, Quality::Insufficient(_)) && !verdict.regressed);
    let mut gappy = stats(5_000, 150, [4500, 400, 75, 20, 5]);
    gappy.minutes_covered = 30;
    gappy.telemetry_lost = 7;
    let verdict = compare(Metric::ErrorRate, &baseline, &gappy, &th);
    assert!(verdict.regressed, "a regression in degraded data still counts");
    let Quality::Degraded(note) = &verdict.quality else {
        panic!("{verdict:?}")
    };
    assert!(
        note.contains("30 of 60 minutes") && note.contains("7 telemetry records"),
        "{note}"
    );
    let mut fine = stats(5_000, 25, [4500, 400, 75, 20, 5]);
    fine.minutes_covered = 30;
    assert_eq!(
        compare(Metric::ErrorRate, &baseline, &fine, &th).quality.as_str(),
        "degraded"
    );
}

#[test]
fn a_latency_rise_counts_only_past_the_ratio_and_the_buckets_resolution() {
    let th = Thresholds::default();
    let baseline = stats(2_000, 0, [1900, 80, 15, 4, 1]);
    let slower = stats(2_000, 0, [1000, 400, 300, 200, 100]);
    let verdict = compare(Metric::LatencyP95, &baseline, &slower, &th);
    assert!(verdict.regressed, "{verdict:?}");
    assert!(verdict.candidate_interval.0 >= verdict.baseline_interval.1);
    assert_eq!(verdict.severity, Some(Severity::Critical));
    // The same distribution is not.
    assert!(!compare(Metric::LatencyP95, &baseline, &baseline.clone(), &th).regressed);
    // Different bounds cannot be compared.
    let mut other = slower.clone();
    other.bounds = vec![0.2, 0.4, 0.8, 1.6];
    assert!(matches!(
        compare(Metric::LatencyP95, &baseline, &other, &th).quality,
        Quality::Insufficient(_)
    ));
}

#[test]
fn rollback_rates_compare_operation_runs() {
    let th = Thresholds::default();
    let mut baseline = stats(0, 0, [0; 5]);
    baseline.operation_runs = 1_000;
    baseline.operation_failures = 5;
    let mut candidate = baseline.clone();
    candidate.operation_failures = 80;
    let verdict = compare(Metric::RollbackRate, &baseline, &candidate, &th);
    assert!(verdict.regressed && verdict.candidate_samples == 1_000, "{verdict:?}");
}

#[test]
fn locality_tells_a_version_from_an_instance() {
    let th = Thresholds::default();
    let baseline = stats(10_000, 50, [9000, 800, 150, 40, 10]);
    let bad = compare(
        Metric::ErrorRate,
        &baseline,
        &stats(3_000, 300, [2700, 240, 45, 12, 3]),
        &th,
    );
    let good = compare(
        Metric::ErrorRate,
        &baseline,
        &stats(3_000, 15, [2700, 240, 45, 12, 3]),
        &th,
    );
    let thin = compare(Metric::ErrorRate, &baseline, &stats(20, 10, [20, 0, 0, 0, 0]), &th);
    let at = |name: &str, verdict: &Verdict| (name.to_string(), verdict.clone());
    let (locality, affected, healthy) = locality(&[at("b", &bad), at("a", &bad), at("c", &thin)]);
    assert_eq!(
        (locality, affected, healthy),
        (Locality::Version, vec!["a".into(), "b".into()], vec![])
    );
    let (locality, affected, healthy) = super::locality(&[at("a", &bad), at("b", &good)]);
    assert_eq!(
        (locality, affected, healthy),
        (Locality::Instance, vec!["a".into()], vec!["b".into()])
    );
    assert_eq!(super::locality(&[at("a", &bad)]).0, Locality::Unknown);
}
