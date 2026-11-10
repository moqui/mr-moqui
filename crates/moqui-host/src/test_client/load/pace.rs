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

//! The pace of a load's users (plans/LoadRunner.md section 5): the think time between iterations and the pause before
//! each request, untimed, drawn from a generator of the user's own thread. A Gaussian pause (JMeter's Gaussian Random
//! Timer) is clamped at zero.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// The longest pause: an hour.
const MAX_MS: f64 = 3_600_000.0;

/// How often a pause looks at the load's stop.
const SLICE: Duration = Duration::from_millis(50);

/// The longest time a load setting gives, in seconds: 30 days. Longer ones (from a form or a script) are cut to it,
/// so that a time added to now cannot overflow.
pub(crate) const MAX_SECONDS: f64 = 30.0 * 86_400.0;

/// A time in seconds, from 0 to [`MAX_SECONDS`]; 0 for one that is not a number.
pub(crate) fn seconds(value: f64) -> Duration {
    if value.is_nan() {
        return Duration::ZERO;
    }
    Duration::from_secs_f64(value.clamp(0.0, MAX_SECONDS))
}

/// SplitMix64: small and fast, and good enough for spreading pauses.
pub(crate) struct Rng(u64);

impl Rng {
    #[cfg(test)]
    pub(crate) fn with_seed(seed: u64) -> Rng {
        Rng(seed)
    }

    /// Seeded from the system's generator, or the time and thread when it has none.
    pub(crate) fn seeded() -> Rng {
        let mut bytes = [0u8; 8];
        if getrandom::fill(&mut bytes).is_ok() {
            return Rng(u64::from_le_bytes(bytes));
        }
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos() as u64);
        let thread = format!("{:?}", std::thread::current().id());
        Rng(nanos
            ^ thread
                .bytes()
                .fold(0u64, |hash, byte| hash.rotate_left(5) ^ u64::from(byte)))
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in [0, 1).
    pub(crate) fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// A standard normal number, by Box–Muller.
    pub(crate) fn gaussian(&mut self) -> f64 {
        let u1 = 1.0 - self.unit();
        let u2 = self.unit();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
}

/// A wait: `think` between iterations, `pause` before each request or in a script's `pause(...)`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Pause {
    /// Milliseconds.
    Fixed(f64),
    /// `[min, max]` milliseconds, uniform.
    Uniform(f64, f64),
    /// `{ gaussian: [mean, deviation] }` milliseconds.
    Gaussian { mean: f64, deviation: f64 },
}

fn milliseconds(value: &Value, what: &str) -> Result<f64, String> {
    value
        .as_f64()
        .filter(|ms| ms.is_finite() && *ms >= 0.0)
        .map(|ms| ms.min(MAX_MS))
        .ok_or_else(|| format!("{what} is milliseconds from 0, not {value}"))
}

impl Pause {
    /// A pause from milliseconds, `[min, max]` or `{ gaussian: [mean, deviation] }`; none for null. `what` names it in
    /// an error.
    pub(crate) fn parse(value: &Value, what: &str) -> Result<Option<Pause>, String> {
        let pause = match value {
            Value::Null => return Ok(None),
            Value::Number(_) => Pause::Fixed(milliseconds(value, what)?),
            Value::Array(pair) => match pair.as_slice() {
                [only] => Pause::Fixed(milliseconds(only, what)?),
                [min, max] => {
                    let (min, max) = (milliseconds(min, what)?, milliseconds(max, what)?);
                    Pause::Uniform(min.min(max), min.max(max))
                }
                _ => return Err(format!("{what} is [min, max], not {value}")),
            },
            Value::Object(map) => match map.get("gaussian").and_then(Value::as_array).map(Vec::as_slice) {
                Some([mean, deviation]) => Pause::Gaussian {
                    mean: milliseconds(mean, what)?,
                    deviation: milliseconds(deviation, what)?,
                },
                _ => return Err(format!("{what} is {{ gaussian: [mean, deviation] }}, not {value}")),
            },
            other => {
                return Err(format!(
                    "{what} is milliseconds, [min, max] or {{ gaussian: [mean, deviation] }}, not {other}"
                ))
            }
        };
        Ok(Some(pause))
    }

    /// Milliseconds drawn from `rng`, never negative.
    pub(crate) fn sample_ms(&self, rng: &mut Rng) -> f64 {
        match self {
            Pause::Fixed(ms) => *ms,
            Pause::Uniform(min, max) => min + rng.unit() * (max - min),
            Pause::Gaussian { mean, deviation } => (mean + rng.gaussian() * deviation).clamp(0.0, MAX_MS),
        }
    }

    pub(crate) fn to_json(&self) -> Value {
        match self {
            Pause::Fixed(ms) => json!(ms),
            Pause::Uniform(min, max) => json!([min, max]),
            Pause::Gaussian { mean, deviation } => json!({ "gaussian": [mean, deviation] }),
        }
    }
}

thread_local! {
    static RNG: RefCell<Option<Rng>> = const { RefCell::new(None) };
}

/// A wait drawn from this thread's generator.
pub(crate) fn draw(pause: &Pause) -> Duration {
    let ms = RNG.with(|cell| pause.sample_ms(cell.borrow_mut().get_or_insert_with(Rng::seeded)));
    Duration::from_secs_f64(ms / 1000.0)
}

/// Sleep for `wait`, or until `until`, or until `stopped` answers true, looking at it every 50 ms. Answers the time
/// slept.
pub(crate) fn sleep(wait: Duration, until: Option<Instant>, stopped: &dyn Fn() -> bool) -> Duration {
    let started = Instant::now();
    let mut end = started + wait;
    if let Some(until) = until {
        end = end.min(until);
    }
    loop {
        let now = Instant::now();
        if now >= end || stopped() {
            return now - started;
        }
        std::thread::sleep((end - now).min(SLICE));
    }
}
