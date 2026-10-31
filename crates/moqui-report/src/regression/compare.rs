//! Deterministic comparison of a candidate's stats against a baseline's (plans/Monitoring.md, Later: Regressions and
//! incidents; requirement 4). Rates use a one-sided two-proportion test with Wilson intervals; latency compares the
//! p95 of the merged duration buckets, at the buckets' resolution, with a chi-square test over the bucket counts. Too
//! few samples is `insufficient` and gaps or lost telemetry `degraded`: neither is ever read as healthy.

use serde::{Deserialize, Serialize};

/// What a window of one service version measured, from its minute metric documents.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    pub requests: u64,
    /// Server errors (5xx) among the requests.
    pub errors: u64,
    /// Operation runs that ended (commit, rollback or retry).
    pub operation_runs: u64,
    /// Of those, the rollbacks and retries.
    pub operation_failures: u64,
    pub bounds: Vec<f64>,
    pub buckets: Vec<u64>,
    /// Minutes of the window with request metrics, and minutes in it.
    pub minutes_covered: u64,
    pub minutes_expected: u64,
    /// Telemetry records the service's exporter dropped in the window.
    pub telemetry_lost: u64,
}

impl Stats {
    fn coverage(&self) -> f64 {
        if self.minutes_expected == 0 {
            return 0.0;
        }
        self.minutes_covered as f64 / self.minutes_expected as f64
    }
}

/// The thresholds of telemetry/regressions.
#[derive(Clone, Debug, PartialEq)]
pub struct Thresholds {
    /// Requests (or operation runs) each side needs before a verdict is given.
    pub min_samples: u64,
    /// The share of a window's minutes that must have metrics, below which a verdict is degraded.
    pub min_coverage: f64,
    /// The one-sided p-value under which a difference counts.
    pub alpha: f64,
    /// The least rise of a rate (0.01 is one point) that is a regression.
    pub rate_delta: f64,
    /// The least p95 ratio that is a regression.
    pub latency_ratio: f64,
    /// A rate rise or p95 ratio from which a regression is critical.
    pub critical_rate: f64,
    pub critical_ratio: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            min_samples: 200,
            min_coverage: 0.8,
            alpha: 0.01,
            rate_delta: 0.01,
            latency_ratio: 1.5,
            critical_rate: 0.10,
            critical_ratio: 3.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    ErrorRate,
    LatencyP95,
    RollbackRate,
}

impl Metric {
    pub const ALL: [Metric; 3] = [Metric::ErrorRate, Metric::LatencyP95, Metric::RollbackRate];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ErrorRate => "error_rate",
            Self::LatencyP95 => "latency_p95",
            Self::RollbackRate => "rollback_rate",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Quality {
    Good,
    /// Enough samples, but gaps or lost telemetry: a regression still counts, health is not claimed.
    Degraded(String),
    /// Too few samples, or nothing to compare: no verdict.
    Insufficient(String),
}

impl Quality {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Good => "good",
            Self::Degraded(_) => "degraded",
            Self::Insufficient(_) => "insufficient",
        }
    }

    pub fn note(&self) -> &str {
        match self {
            Self::Good => "",
            Self::Degraded(note) | Self::Insufficient(note) => note,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Major,
    Critical,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Major => "major",
            Self::Critical => "critical",
        }
    }
}

/// The outcome of one metric's comparison, with its uncertainty.
#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    pub metric: Metric,
    pub baseline_value: f64,
    pub candidate_value: f64,
    /// Each value's interval: Wilson 95% for a rate, the p95's bucket for latency.
    pub baseline_interval: (f64, f64),
    pub candidate_interval: (f64, f64),
    /// The candidate value over the baseline value.
    pub magnitude: f64,
    /// 1 less the one-sided p-value.
    pub confidence: f64,
    pub baseline_samples: u64,
    pub candidate_samples: u64,
    pub quality: Quality,
    pub regressed: bool,
    pub severity: Option<Severity>,
}

impl Verdict {
    fn insufficient(metric: Metric, baseline_samples: u64, candidate_samples: u64, note: String) -> Self {
        Self {
            metric,
            baseline_value: 0.0,
            candidate_value: 0.0,
            baseline_interval: (0.0, 0.0),
            candidate_interval: (0.0, 0.0),
            magnitude: 0.0,
            confidence: 0.0,
            baseline_samples,
            candidate_samples,
            quality: Quality::Insufficient(note),
            regressed: false,
            severity: None,
        }
    }
}

/// Compare one metric of a candidate against its baseline.
pub fn compare(metric: Metric, baseline: &Stats, candidate: &Stats, thresholds: &Thresholds) -> Verdict {
    let (samples_b, samples_c) = match metric {
        Metric::ErrorRate | Metric::LatencyP95 => (baseline.requests, candidate.requests),
        Metric::RollbackRate => (baseline.operation_runs, candidate.operation_runs),
    };
    if samples_b < thresholds.min_samples || samples_c < thresholds.min_samples {
        return Verdict::insufficient(
            metric,
            samples_b,
            samples_c,
            format!(
                "{} baseline and {} candidate samples; {} each are needed",
                samples_b, samples_c, thresholds.min_samples
            ),
        );
    }
    let quality = quality(baseline, candidate, thresholds);
    match metric {
        Metric::ErrorRate => rate(
            metric,
            (baseline.errors, baseline.requests),
            (candidate.errors, candidate.requests),
            quality,
            thresholds,
        ),
        Metric::RollbackRate => rate(
            metric,
            (baseline.operation_failures, baseline.operation_runs),
            (candidate.operation_failures, candidate.operation_runs),
            quality,
            thresholds,
        ),
        Metric::LatencyP95 => latency(baseline, candidate, quality, thresholds),
    }
}

/// Gaps in either window, or telemetry the candidate lost, degrade a verdict.
fn quality(baseline: &Stats, candidate: &Stats, thresholds: &Thresholds) -> Quality {
    let mut notes = Vec::new();
    for (side, stats) in [("baseline", baseline), ("candidate", candidate)] {
        if stats.coverage() < thresholds.min_coverage {
            notes.push(format!(
                "the {side} has metrics for {} of {} minutes",
                stats.minutes_covered, stats.minutes_expected
            ));
        }
    }
    if candidate.telemetry_lost > 0 {
        notes.push(format!(
            "{} telemetry records were dropped in the candidate window",
            candidate.telemetry_lost
        ));
    }
    if notes.is_empty() {
        Quality::Good
    } else {
        Quality::Degraded(notes.join("; "))
    }
}

fn rate(metric: Metric, baseline: (u64, u64), candidate: (u64, u64), quality: Quality, th: &Thresholds) -> Verdict {
    let (x_b, n_b) = (baseline.0 as f64, baseline.1 as f64);
    let (x_c, n_c) = (candidate.0 as f64, candidate.1 as f64);
    let (p_b, p_c) = (x_b / n_b, x_c / n_c);
    let pooled = (x_b + x_c) / (n_b + n_c);
    let se = (pooled * (1.0 - pooled) * (1.0 / n_b + 1.0 / n_c)).sqrt();
    let z = if se > 0.0 { (p_c - p_b) / se } else { 0.0 };
    let p_value = 1.0 - normal_cdf(z);
    // A baseline of no errors is half an error, so the ratio stays finite and grows with the candidate's rate.
    let magnitude = p_c / p_b.max(0.5 / n_b);
    let regressed = p_value < th.alpha && p_c - p_b >= th.rate_delta;
    Verdict {
        metric,
        baseline_value: p_b,
        candidate_value: p_c,
        baseline_interval: wilson(baseline.0, baseline.1),
        candidate_interval: wilson(candidate.0, candidate.1),
        magnitude,
        confidence: 1.0 - p_value,
        baseline_samples: baseline.1,
        candidate_samples: candidate.1,
        quality,
        regressed,
        severity: regressed.then_some(if p_c - p_b >= th.critical_rate {
            Severity::Critical
        } else {
            Severity::Major
        }),
    }
}

fn latency(baseline: &Stats, candidate: &Stats, quality: Quality, th: &Thresholds) -> Verdict {
    let metric = Metric::LatencyP95;
    if baseline.bounds != candidate.bounds || baseline.buckets.len() != baseline.bounds.len() + 1 {
        return Verdict::insufficient(
            metric,
            baseline.requests,
            candidate.requests,
            "the duration histograms have different or missing bucket bounds".into(),
        );
    }
    let (Some(b), Some(c)) = (
        p95(&baseline.bounds, &baseline.buckets),
        p95(&candidate.bounds, &candidate.buckets),
    ) else {
        return Verdict::insufficient(
            metric,
            baseline.requests,
            candidate.requests,
            "the duration histograms are empty".into(),
        );
    };
    let p_value = chi_square_p(&baseline.buckets, &candidate.buckets);
    let magnitude = if b.value > 0.0 { c.value / b.value } else { 1.0 };
    // A rise inside one bucket is below the histogram's resolution.
    let regressed = c.bucket > b.bucket && magnitude >= th.latency_ratio && p_value < th.alpha;
    Verdict {
        metric,
        baseline_value: b.value,
        candidate_value: c.value,
        baseline_interval: (b.lower, b.upper),
        candidate_interval: (c.lower, c.upper),
        magnitude,
        confidence: 1.0 - p_value,
        baseline_samples: baseline.requests,
        candidate_samples: candidate.requests,
        quality,
        regressed,
        severity: regressed.then_some(if magnitude >= th.critical_ratio {
            Severity::Critical
        } else {
            Severity::Major
        }),
    }
}

/// The p95 of merged buckets, interpolated, with the bucket it falls in.
struct Quantile {
    value: f64,
    bucket: usize,
    lower: f64,
    upper: f64,
}

fn p95(bounds: &[f64], buckets: &[u64]) -> Option<Quantile> {
    let value = moqui_core::telemetry::maintenance::histogram_percentile(bounds, buckets, 0.95)?;
    let total: u64 = buckets.iter().sum();
    let target = 0.95 * total as f64;
    let mut seen = 0_u64;
    let bucket = buckets.iter().position(|count| {
        seen += count;
        *count > 0 && seen as f64 >= target
    })?;
    let lower = bucket
        .checked_sub(1)
        .and_then(|index| bounds.get(index))
        .copied()
        .unwrap_or(0.0);
    let upper = bounds.get(bucket).copied().unwrap_or(f64::INFINITY);
    Some(Quantile {
        value,
        bucket,
        lower,
        upper,
    })
}

/// The Wilson 95% interval of `successes` in `trials`.
pub fn wilson(successes: u64, trials: u64) -> (f64, f64) {
    if trials == 0 {
        return (0.0, 1.0);
    }
    const Z: f64 = 1.959_963_984_540_054;
    let n = trials as f64;
    let p = successes as f64 / n;
    let denominator = 1.0 + Z * Z / n;
    let centre = (p + Z * Z / (2.0 * n)) / denominator;
    let half = Z * (p * (1.0 - p) / n + Z * Z / (4.0 * n * n)).sqrt() / denominator;
    ((centre - half).max(0.0), (centre + half).min(1.0))
}

/// The standard normal distribution function.
pub fn normal_cdf(z: f64) -> f64 {
    0.5 * erfc(-z / std::f64::consts::SQRT_2)
}

/// The complementary error function, to about 1.2e-7 (Numerical Recipes' Chebyshev fit).
fn erfc(x: f64) -> f64 {
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.5 * z);
    let poly = -z * z - 1.265_512_23
        + t * (1.000_023_68
            + t * (0.374_091_96
                + t * (0.096_784_18
                    + t * (-0.186_288_06
                        + t * (0.278_868_07
                            + t * (-1.135_203_98 + t * (1.488_515_87 + t * (-0.822_152_23 + t * 0.170_872_77))))))));
    let value = t * poly.exp();
    if x >= 0.0 {
        value
    } else {
        2.0 - value
    }
}

/// The p-value of a chi-square test of two histograms over the same buckets, after merging neighbouring buckets until
/// each column expects at least 5 in both rows. 1 when there is nothing to test.
pub fn chi_square_p(baseline: &[u64], candidate: &[u64]) -> f64 {
    let (n_b, n_c) = (
        baseline.iter().sum::<u64>() as f64,
        candidate.iter().sum::<u64>() as f64,
    );
    let total = n_b + n_c;
    if n_b == 0.0 || n_c == 0.0 {
        return 1.0;
    }
    let mut columns: Vec<(f64, f64)> = Vec::new();
    let mut pending = (0.0, 0.0);
    for (b, c) in baseline.iter().zip(candidate) {
        pending.0 += *b as f64;
        pending.1 += *c as f64;
        let column_total = pending.0 + pending.1;
        if column_total * n_b / total >= 5.0 && column_total * n_c / total >= 5.0 {
            columns.push(pending);
            pending = (0.0, 0.0);
        }
    }
    if pending.0 + pending.1 > 0.0 {
        match columns.last_mut() {
            Some(last) => {
                last.0 += pending.0;
                last.1 += pending.1;
            }
            None => columns.push(pending),
        }
    }
    if columns.len() < 2 {
        return 1.0;
    }
    let statistic: f64 = columns
        .iter()
        .map(|(b, c)| {
            let column_total = b + c;
            let expected_b = column_total * n_b / total;
            let expected_c = column_total * n_c / total;
            (b - expected_b).powi(2) / expected_b + (c - expected_c).powi(2) / expected_c
        })
        .sum();
    let df = (columns.len() - 1) as f64;
    upper_gamma_regularized(df / 2.0, statistic / 2.0)
}

/// Q(a, x), the regularized upper incomplete gamma function (Numerical Recipes' gammq).
fn upper_gamma_regularized(a: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 1.0;
    }
    if x < a + 1.0 {
        // The series for P, then Q = 1 - P.
        let mut sum = 1.0 / a;
        let mut term = sum;
        let mut ap = a;
        for _ in 0..500 {
            ap += 1.0;
            term *= x / ap;
            sum += term;
            if term.abs() < sum.abs() * 1e-15 {
                break;
            }
        }
        let p = sum * (-x + a * x.ln() - ln_gamma(a)).exp();
        (1.0 - p).clamp(0.0, 1.0)
    } else {
        // The continued fraction for Q (modified Lentz).
        const TINY: f64 = 1e-300;
        let mut b = x + 1.0 - a;
        let mut c = 1.0 / TINY;
        let mut d = 1.0 / b;
        let mut h = d;
        for i in 1..500 {
            let an = -(i as f64) * (i as f64 - a);
            b += 2.0;
            d = an * d + b;
            if d.abs() < TINY {
                d = TINY;
            }
            c = b + an / c;
            if c.abs() < TINY {
                c = TINY;
            }
            d = 1.0 / d;
            let delta = d * c;
            h *= delta;
            if (delta - 1.0).abs() < 1e-15 {
                break;
            }
        }
        ((-x + a * x.ln() - ln_gamma(a)).exp() * h).clamp(0.0, 1.0)
    }
}

/// ln Γ(x) for x > 0 (Lanczos, g = 7).
fn ln_gamma(x: f64) -> f64 {
    const COEFFICIENTS: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    if x < 0.5 {
        // Reflection.
        let pi = std::f64::consts::PI;
        return (pi / (pi * x).sin()).ln() - ln_gamma(1.0 - x);
    }
    let x = x - 1.0;
    let mut sum = COEFFICIENTS[0];
    for (i, coefficient) in COEFFICIENTS.iter().enumerate().skip(1) {
        sum += coefficient / (x + i as f64);
    }
    let t = x + 7.5;
    0.5 * (2.0 * std::f64::consts::PI).ln() + (x + 0.5) * t.ln() - t + sum.ln()
}

/// Where a version's regression is: on every instance of it, on some while others are healthy, or not known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Locality {
    Version,
    Instance,
    Unknown,
}

impl Locality {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Version => "version",
            Self::Instance => "instance",
            Self::Unknown => "unknown",
        }
    }
}

/// The locality of a regression from each candidate instance's own verdict against the baseline. Instances with too
/// few samples take no part. One instance alone cannot tell an instance fault from the version's.
pub fn locality(instances: &[(String, Verdict)]) -> (Locality, Vec<String>, Vec<String>) {
    let mut affected = Vec::new();
    let mut healthy = Vec::new();
    for (instance, verdict) in instances {
        match (&verdict.quality, verdict.regressed) {
            (Quality::Insufficient(_), _) => {}
            (_, true) => affected.push(instance.clone()),
            (_, false) => healthy.push(instance.clone()),
        }
    }
    affected.sort();
    healthy.sort();
    let locality = match (affected.len(), healthy.len()) {
        (0, _) | (1, 0) => Locality::Unknown,
        (_, 0) => Locality::Version,
        _ => Locality::Instance,
    };
    (locality, affected, healthy)
}

#[cfg(test)]
#[path = "compare_tests.rs"]
mod tests;
