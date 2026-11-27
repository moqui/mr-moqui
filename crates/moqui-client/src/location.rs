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

//! A live location share sent from this device (plans/Location-Sharing.md §3.4): the Android app's foreground service
//! hands each fix of a share it sends to the host (`report_location`), and a [`Reporter`] of that share keeps only the
//! newest point not yet sent and sends it to the account's server ([`REPORT_PATH`], `report#Position`) with the
//! account's device key, as the app's other requests go, so the share's device binding takes it. What the server
//! answers is handled as the browser's Maps page handles it (`Browse.qvue` `report`):
//!
//! - 200: sent; the next waits for the share's interval.
//! - 429: too soon; the point waits for `retryAfterSeconds`, a newer one taking its place.
//! - 409 `STALE` and another 400: that point is passed over.
//! - 409 `CLOCK_AHEAD` and 400 `BEFORE_SHARE`: the device's clock is off, so from then on points go without the
//!   device's time (the server's), the refused one again at once.
//! - 410 (ended or stopped), 404 (not the account's), 409 `OTHER_DEVICE` (another device sends it now), 401 and 403
//!   or an answer that is not the transition's JSON (signed out): the share is no longer sent here.
//! - No answer (no network) or a 5xx: the point is kept and tried again after the interval.
//!
//! A position is never logged: neither here nor by the callers, whose errors name the share and the status only.

use serde_json::Value;

/// The app's transition that reports a position of a share of the user's own (`webroot` `qapps.xml`).
pub const REPORT_PATH: &str = "/qapps/reportClientLocation";
/// The app's transition that stops a share of the user's own (`stop#Share`), from the notification's Stop.
pub const STOP_PATH: &str = "/qapps/stopClientLocation";

/// The shortest wait after a failure with no answer, in milliseconds, so a lost network is not asked in a loop.
const RETRY_MIN_MS: i64 = 5_000;
/// The wait after a 429 that is not the transition's (the server's tarpit, a proxy) and names no wait of its own.
const BUSY_WAIT_MS: i64 = 30_000;
/// The longest a report or a stop may take, its answer included: a half-open connection gives up then, and the
/// point waits for the next try ([`post_form`]), rather than the proxy's ten minutes for a page's request.
pub const POST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// Where [`post_form`] puts the `Retry-After` of an answer that is not JSON, for [`Reporter::answered`].
pub const RETRY_AFTER: &str = "retryAfterHeader";

/// One fix of the device's location, as the phone took it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fix {
    pub latitude: f64,
    pub longitude: f64,
    /// Metres.
    pub accuracy: Option<f64>,
    /// Degrees from north.
    pub heading: Option<f64>,
    /// Metres a second.
    pub speed: Option<f64>,
    /// When the device took it, epoch milliseconds.
    pub observed_ms: i64,
}

impl Fix {
    /// A fix from the native code's JSON (`latitude`, `longitude`, `accuracy`, `heading`, `speed`, `time`): none when
    /// the position is missing, not finite or out of range (the server checks the ranges again).
    pub fn from_json(json: &Value) -> Option<Self> {
        let number = |name: &str| json.get(name).and_then(Value::as_f64).filter(|value| value.is_finite());
        let latitude = number("latitude").filter(|value| (-90.0..=90.0).contains(value))?;
        let longitude = number("longitude").filter(|value| (-180.0..=180.0).contains(value))?;
        let observed_ms = json.get("time").and_then(Value::as_i64).filter(|time| *time > 0)?;
        Some(Self {
            latitude,
            longitude,
            accuracy: number("accuracy").filter(|value| *value >= 0.0),
            heading: number("heading").filter(|value| (0.0..=360.0).contains(value)),
            speed: number("speed").filter(|value| *value >= 0.0),
            observed_ms,
        })
    }
}

/// Why a share is no longer sent from here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stopped {
    /// 410: the share ended or was stopped.
    Ended,
    /// 404: not a share of the account's.
    NotFound,
    /// 409 `OTHER_DEVICE`: another device of the owner's sends it now (its Send from here).
    OtherDevice,
    /// 401, 403, or an answer that is not the transition's JSON: the account is signed out.
    SignedOut,
    /// 409 `AUDIENCE_CHANGED`: the share's audience changed after the person confirmed whom the phone sends to (its
    /// version): not sent until they confirm it again.
    AudienceChanged,
}

impl Stopped {
    /// The name the native code reads.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ended => "ended",
            Self::NotFound => "notFound",
            Self::OtherDevice => "otherDevice",
            Self::SignedOut => "signedOut",
            Self::AudienceChanged => "audienceChanged",
        }
    }
}

/// What a report came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The point was sent and taken.
    Sent,
    /// A point waits: send again after this many milliseconds (the interval, a 429's `retryAfterSeconds`, a failure).
    Waiting(i64),
    /// The point was passed over (409 `STALE`, a 400): nothing waits.
    Dropped,
    /// Nothing to send.
    Idle,
    /// The share is no longer sent from here.
    Stopped(Stopped),
}

impl Outcome {
    /// The outcome as the native code reads it: `{"state": "sent" | "waiting" | "dropped" | "idle" | "stopped",
    /// "waitMs", "reason"}`.
    pub fn to_json(self) -> Value {
        match self {
            Self::Sent => serde_json::json!({ "state": "sent" }),
            Self::Waiting(ms) => serde_json::json!({ "state": "waiting", "waitMs": ms.max(0) }),
            Self::Dropped => serde_json::json!({ "state": "dropped" }),
            Self::Idle => serde_json::json!({ "state": "idle" }),
            Self::Stopped(why) => serde_json::json!({ "state": "stopped", "reason": why.as_str() }),
        }
    }
}

/// What a [`Reporter`] does next.
#[derive(Debug, PartialEq)]
pub enum Next {
    /// Post this form to [`REPORT_PATH`] for the point it holds, then hand the answer to [`Reporter::answered`].
    Send(Fix, Vec<(String, String)>),
    /// Not yet: the newest point waits this many milliseconds (or a report is under way).
    Wait(i64),
    /// Nothing to send.
    Idle,
    /// The share is no longer sent from here.
    Stopped(Stopped),
}

/// The reports of one share sent from this device: the newest point not yet sent, when the next may go, and whether
/// points go with the device's time.
#[derive(Debug)]
pub struct Reporter {
    share_id: String,
    interval_ms: i64,
    pending: Option<Fix>,
    /// The time of the newest point sent or passed over: an older one is never sent after it.
    last_ms: Option<i64>,
    next_at_ms: i64,
    server_time: bool,
    /// The audience version the person confirmed (the app's own dialog), sent with each report: the server refuses a
    /// report of another version (409 `AUDIENCE_CHANGED`) before it writes or pushes anything.
    audience_version: Option<i64>,
    in_flight: bool,
    stopped: Option<Stopped>,
    /// An `OTHER_DEVICE` across a renewal of the device key was tried again once.
    renewal_retried: bool,
}

impl Reporter {
    /// The reporter of `share_id`, sent at most once an `interval_ms` (the share's report interval:
    /// `location@min-interval-seconds`, or `location@approximate-interval-seconds` for an approximate share).
    pub fn new(share_id: &str, interval_ms: i64) -> Self {
        Self {
            share_id: share_id.to_string(),
            interval_ms: interval_ms.max(1_000),
            pending: None,
            last_ms: None,
            next_at_ms: 0,
            server_time: false,
            audience_version: None,
            in_flight: false,
            stopped: None,
            renewal_retried: false,
        }
    }

    /// The share's id.
    pub fn share_id(&self) -> &str {
        &self.share_id
    }

    /// Why it is no longer sent, once it is not.
    pub fn stopped(&self) -> Option<Stopped> {
        self.stopped
    }

    /// The interval changed (the share became approximate, the server's setting): from the next send on.
    /// The audience version the person confirmed for this share, sent from now on.
    pub fn set_audience_version(&mut self, version: i64) {
        self.audience_version = Some(version);
    }

    /// The interval changed (the share became approximate, the server's setting): from the next send on.
    pub fn set_interval(&mut self, interval_ms: i64) {
        self.interval_ms = interval_ms.max(1_000);
    }

    /// A new fix: kept as the point to send when it is newer than the one waiting and than the last one sent; an older
    /// one is dropped.
    pub fn offer(&mut self, fix: Fix) {
        if self.stopped.is_some() || self.last_ms.is_some_and(|last| fix.observed_ms <= last) {
            return;
        }
        if self
            .pending
            .is_none_or(|pending| fix.observed_ms >= pending.observed_ms)
        {
            self.pending = Some(fix);
        }
    }

    /// What to do at `now_ms`: send the point waiting (marking a report under way), wait, or nothing.
    pub fn next(&mut self, now_ms: i64) -> Next {
        if let Some(why) = self.stopped {
            return Next::Stopped(why);
        }
        if self.in_flight {
            return Next::Wait(self.interval_ms);
        }
        let Some(fix) = self.pending else {
            return Next::Idle;
        };
        if now_ms < self.next_at_ms {
            return Next::Wait(self.next_at_ms - now_ms);
        }
        self.pending = None;
        self.in_flight = true;
        Next::Send(fix, self.form(&fix))
    }

    /// The form of a report of `fix`: no device time once the server's is used.
    fn form(&self, fix: &Fix) -> Vec<(String, String)> {
        let mut form = vec![
            ("shareId".to_string(), self.share_id.clone()),
            ("latitude".to_string(), fix.latitude.to_string()),
            ("longitude".to_string(), fix.longitude.to_string()),
        ];
        for (name, value) in [
            ("accuracy", fix.accuracy),
            ("heading", fix.heading),
            ("speed", fix.speed),
        ] {
            if let Some(value) = value {
                form.push((name.to_string(), value.to_string()));
            }
        }
        if let Some(version) = self.audience_version {
            form.push(("audienceVersion".to_string(), version.to_string()));
        }
        if !self.server_time {
            form.push(("observedDate".to_string(), fix.observed_ms.to_string()));
        }
        form
    }

    /// The server's answer to the report of `sent` (its HTTP status and JSON, or the failure of a request with no
    /// answer), at `now_ms`.
    pub fn answered(&mut self, sent: Fix, answer: Result<(u16, Value), String>, now_ms: i64) -> Outcome {
        self.answered_after(sent, answer, now_ms, false)
    }

    /// [`Self::answered`], with `key_changed` when the account's device key changed while the report was posted (a
    /// renewal: the server moves the device's shares to the new key before the client keeps it, so a report with the
    /// old key may be refused as another device's). Such an `OTHER_DEVICE` is tried again at once, once.
    pub fn answered_after(
        &mut self,
        sent: Fix,
        answer: Result<(u16, Value), String>,
        now_ms: i64,
        key_changed: bool,
    ) -> Outcome {
        self.in_flight = false;
        if let Some(why) = self.stopped {
            return Outcome::Stopped(why);
        }
        let (http, json) = match answer {
            Ok(answer) => answer,
            Err(_) => return self.again(sent, now_ms, self.interval_ms.max(RETRY_MIN_MS)),
        };
        if http == 401 || http == 403 {
            return self.stop(Stopped::SignedOut);
        }
        if http >= 500 {
            return self.again(sent, now_ms, self.interval_ms.max(RETRY_MIN_MS));
        }
        let Some(status) = json.get("statusCode").and_then(Value::as_i64) else {
            // a timeout or a limit of something between (the server's tarpit, a proxy): tried again after its wait
            if http == 408 || http == 429 {
                let default = if http == 429 {
                    BUSY_WAIT_MS
                } else {
                    self.interval_ms.max(RETRY_MIN_MS)
                };
                let wait = json
                    .get(RETRY_AFTER)
                    .and_then(Value::as_i64)
                    .map_or(default, |seconds| seconds.clamp(1, 3600) * 1000);
                return self.again(sent, now_ms, wait);
            }
            // an answer that is not the transition's JSON (a sign-in page): the key no longer signs in
            return self.stop(Stopped::SignedOut);
        };
        let reason = json.get("reasonCode").and_then(Value::as_str).unwrap_or("");
        match (status, reason) {
            (200, _) => {
                self.last_ms = Some(sent.observed_ms);
                self.next_at_ms = now_ms + self.interval_ms;
                self.waiting_or(Outcome::Sent)
            }
            (429, _) => {
                let seconds = json
                    .get("retryAfterSeconds")
                    .and_then(Value::as_i64)
                    .unwrap_or(1)
                    .clamp(1, 3600);
                self.again(sent, now_ms, seconds * 1000)
            }
            (410, _) => self.stop(Stopped::Ended),
            (404, _) => self.stop(Stopped::NotFound),
            (409, "OTHER_DEVICE") if key_changed && !self.renewal_retried => {
                self.renewal_retried = true;
                self.again(sent, now_ms, 0)
            }
            (409, "OTHER_DEVICE") => self.stop(Stopped::OtherDevice),
            // the audience is not the one the person confirmed: nothing was written or pushed, and nothing more goes
            (409, "AUDIENCE_CHANGED") => self.stop(Stopped::AudienceChanged),
            (409, "CLOCK_AHEAD") | (400, "BEFORE_SHARE") => {
                // the device's clock is off from the server's: the server's time from now on, this point again now
                self.server_time = true;
                self.again(sent, now_ms, 0)
            }
            (409 | 400, _) => {
                self.last_ms = Some(sent.observed_ms.max(self.last_ms.unwrap_or(i64::MIN)));
                self.next_at_ms = now_ms + self.interval_ms;
                self.waiting_or(Outcome::Dropped)
            }
            _ => self.again(sent, now_ms, self.interval_ms.max(RETRY_MIN_MS)),
        }
    }

    /// Report `fix` now if it may go (the newest point kept otherwise), posting with `post`: the one call of the native
    /// code for each fix, and with no fix for a point that waited.
    pub fn report(
        &mut self,
        fix: Option<Fix>,
        now_ms: i64,
        post: impl FnOnce(&[(String, String)]) -> Result<(u16, Value), String>,
    ) -> Outcome {
        if let Some(fix) = fix {
            self.offer(fix);
        }
        match self.next(now_ms) {
            Next::Send(sent, form) => {
                let answer = post(&form);
                self.answered(sent, answer, now_ms)
            }
            Next::Wait(ms) => Outcome::Waiting(ms),
            Next::Idle => Outcome::Idle,
            Next::Stopped(why) => Outcome::Stopped(why),
        }
    }

    /// The share is no longer sent from here (the user's Stop, a sign-out): what waits is dropped.
    pub fn stop(&mut self, why: Stopped) -> Outcome {
        self.stopped = Some(why);
        self.pending = None;
        Outcome::Stopped(why)
    }

    /// `sent` is sent again `wait_ms` after `now_ms`, unless a newer point came meanwhile, which goes then instead.
    fn again(&mut self, sent: Fix, now_ms: i64, wait_ms: i64) -> Outcome {
        if self
            .pending
            .is_none_or(|pending| pending.observed_ms < sent.observed_ms)
        {
            self.pending = Some(sent);
        }
        self.next_at_ms = now_ms + wait_ms;
        Outcome::Waiting(wait_ms)
    }

    /// After a point went (or was passed over): a newer one that came while it was sent waits for the interval.
    fn waiting_or(&self, done: Outcome) -> Outcome {
        match self.pending {
            Some(_) => Outcome::Waiting(self.interval_ms),
            None => done,
        }
    }
}

/// A form posted to `path` on the account's server at `base`, signed by its device key `key`, through `proxy` within
/// `timeout` ([`POST_TIMEOUT`]): the answer's HTTP status and JSON (null when it is not JSON; then its `Retry-After`
/// in seconds, if any, as [`RETRY_AFTER`]), or why there was none. The error names the server, never the form.
///
/// # Errors
///
/// When no answer came within `timeout` (or none at all).
pub fn post_form(
    proxy: &crate::proxy::Proxy,
    base: &str,
    key: &str,
    path: &str,
    form: &[(&str, &str)],
    timeout: std::time::Duration,
) -> Result<(u16, Value), String> {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form.iter().copied())
        .finish();
    let request = crate::proxy::Request {
        method: "POST".to_string(),
        path: path.to_string(),
        headers: vec![
            ("Accept".to_string(), "application/json".to_string()),
            (
                "Content-Type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            ),
            ("X-Requested-With".to_string(), "XMLHttpRequest".to_string()),
        ],
        body: body.into_bytes(),
    };
    let upstream = proxy
        .send_within(base, Some(key), crate::SHELL_ORIGIN, request, Some(timeout))
        .map_err(|err| err.to_string())?;
    let status = upstream.status;
    let retry_after = upstream
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
        .and_then(|(_, value)| value.trim().parse::<i64>().ok());
    let bytes = upstream.body().map_err(|err| err.to_string())?;
    let json = match serde_json::from_slice::<Value>(&bytes) {
        Ok(json) => json,
        Err(_) => retry_after.map_or(Value::Null, |seconds| serde_json::json!({ RETRY_AFTER: seconds })),
    };
    Ok((status, json))
}

#[cfg(test)]
#[path = "location_tests.rs"]
mod tests;
