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

//! Usage and cost (plan 4.2, 4.19): one `LlmCall` row per provider request ([`record_call`]), with its content only
//! when `content-log` is on and redacted ([`redact`]); the daily rollup into `LlmUsageDaily` ([`rollup_day`]); the
//! clearing of old content ([`clean_call_content`]); and the quota check before a run and each step ([`check_quota`]).
//!
//! Days are in the server's time zone. A quota counts today's calls from `LlmCall` itself, so it does not wait for the
//! rollup; a monthly quota adds the earlier days of the month from `LlmUsageDaily`.
//!
//! Requests per minute (an `LlmQuota`'s or an `LlmApiClient`'s `requestsPerMinute`) are counted per node in memory, in a
//! sliding minute ([`take_request`]): a cluster of `n` nodes lets up to `n` times the limit through.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Mutex;

use moqui_db::sql::Condition;
use moqui_lib::error::Error;
use moqui_lib::object::Value;
use rust_decimal::Decimal;

use super::conf::{ContentLogConf, LimitsConf, Prices};
use super::registry::Target;
use super::{decimal_of, int_of, text_of};
use crate::entity::EntityFacade;

pub(crate) const LLM_CALL: &str = "moqui.llm.LlmCall";
const LLM_USAGE_DAILY: &str = "moqui.llm.LlmUsageDaily";
const LLM_QUOTA: &str = "moqui.llm.LlmQuota";

/// The user of a call that has none, in `LlmUsageDaily`.
pub const NO_USER: &str = "_NA_";

/// The tokens of one provider call, normalized across the protocols: `input` is the whole prompt, of which
/// `cached_input` was read from the provider's cache and `cache_write` written to it; `reasoning` is the part of
/// `output` spent reasoning. (Anthropic reports cache reads and writes beside its input tokens; its decoder adds them in.)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Usage {
    pub input: u64,
    pub cached_input: u64,
    pub cache_write: u64,
    pub output: u64,
    pub reasoning: u64,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.input.saturating_add(self.output)
    }

    pub fn is_zero(&self) -> bool {
        *self == Usage::default()
    }

    pub fn add(&mut self, other: &Usage) {
        self.input = self.input.saturating_add(other.input);
        self.cached_input = self.cached_input.saturating_add(other.cached_input);
        self.cache_write = self.cache_write.saturating_add(other.cache_write);
        self.output = self.output.saturating_add(other.output);
        self.reasoning = self.reasoning.saturating_add(other.reasoning);
    }
}

/// The cost of `usage` at `prices` (per million tokens): the input that was neither read from nor written to the cache
/// at the input price, cache reads and writes at theirs (the input price when they have none), and the output at the
/// output price. `None` when the model has no prices.
pub fn cost(prices: &Prices, usage: &Usage) -> Option<Decimal> {
    if prices.is_unknown() {
        return None;
    }
    let input = prices.input.unwrap_or_default();
    let cached = prices.cached_input.unwrap_or(input);
    let write = prices.cache_write.unwrap_or(input);
    let output = prices.output.unwrap_or_default();
    let plain = usage.input.saturating_sub(usage.cached_input).saturating_sub(usage.cache_write);
    let tokens = |count: u64, price: Decimal| Decimal::from(count).checked_mul(price).unwrap_or_default();
    let total = tokens(plain, input) + tokens(usage.cached_input, cached) + tokens(usage.cache_write, write)
        + tokens(usage.output, output);
    Some((total / Decimal::from(1_000_000)).round_dp(8).normalize())
}

/// What a call was made for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CallSource {
    #[default]
    Agent,
    Gateway,
    /// `ec.llm` and the AI tasks of app code.
    Api,
    Probe,
    Embed,
    /// `test#Endpoint`.
    Test,
}

impl CallSource {
    pub const ALL: [CallSource; 6] = [
        CallSource::Agent,
        CallSource::Gateway,
        CallSource::Api,
        CallSource::Probe,
        CallSource::Embed,
        CallSource::Test,
    ];

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|source| source.as_str() == text)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            CallSource::Agent => "agent",
            CallSource::Gateway => "gateway",
            CallSource::Api => "api",
            CallSource::Probe => "probe",
            CallSource::Embed => "embed",
            CallSource::Test => "test",
        }
    }
}

/// One provider request (one attempt) and how it went, for [`record_call`]. [`CallRecord::new`] fills in the target;
/// the provider code sets the timing, the status, the finish, the usage and, for the content log, the bodies.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CallRecord {
    pub run_id: Option<String>,
    pub step_seq: Option<i64>,
    pub user_id: Option<String>,
    pub feature: Option<String>,
    pub model_id: String,
    pub endpoint_id: String,
    pub provider_model: String,
    pub protocol: String,
    pub source: CallSource,
    pub client_id: Option<String>,
    /// Milliseconds since the epoch.
    pub start_millis: i64,
    pub first_token_ms: Option<i64>,
    pub duration_ms: i64,
    pub http_status: Option<u16>,
    /// The finish as the provider module names it (stop, tool_calls, length, content_filter, ...).
    pub finish: Option<String>,
    /// The error class (rate_limited, overloaded, server, timeout, network, auth, bad_request, quota, ...).
    pub error_class: Option<String>,
    pub error_text: Option<String>,
    /// 1 for the first try.
    pub attempt: u32,
    pub usage: Usage,
    /// The request body sent; kept only with content logging, redacted.
    pub request_json: Option<String>,
    /// The response (or the decoded events), kept only with content logging, redacted.
    pub response_json: Option<String>,
    /// The usage is the gateway's estimate (a stream cut before its usage came), not the provider's count.
    pub usage_estimated: bool,
}

impl CallRecord {
    /// A call to `target` starting now.
    pub fn new(source: CallSource, target: &Target) -> Self {
        Self {
            feature: target.feature.clone(),
            model_id: target.model.id.clone(),
            endpoint_id: target.endpoint.id.clone(),
            provider_model: target.model.name.clone(),
            protocol: target.endpoint.protocol.as_str().to_string(),
            source,
            start_millis: jiff::Timestamp::now().as_millisecond(),
            attempt: 1,
            ..Self::default()
        }
    }
}

/// Write `call` as an `LlmCall` row, with its cost from `prices`, in a transaction of its own, so the caller's
/// rolling back does not lose what a call used. The content goes in only when `content_log` is enabled, redacted
/// unless it says not to. Out: the `callId`.
///
/// # Errors
///
/// Returns an error when the row cannot be written.
pub fn record_call(
    facade: &EntityFacade,
    content_log: &ContentLogConf,
    prices: &Prices,
    call: &CallRecord,
) -> Result<String, Error> {
    // the framework's own bookkeeping: written for any user, not by the user's entity permissions
    let _authz = crate::context::ExecutionContext::with_current(crate::context::ExecutionContext::disable_authz);
    facade
        .transactions_for(LLM_CALL)?
        .run(crate::transaction::TransactionAttribute::ForceNew, None, || {
            write_call(facade, content_log, prices, call)
        })
}

fn write_call(facade: &EntityFacade, content_log: &ContentLogConf, prices: &Prices, call: &CallRecord) -> Result<String, Error> {
    let mut row = facade.make_value(LLM_CALL)?;
    row.set_sequenced_id_primary(facade)?;
    let call_id = text_of(&row, "callId").unwrap_or_default();
    let text = |value: &Option<String>| value.as_ref().map_or(Value::Null, Value::string);
    let count = |value: u64| Value::I64(i64::try_from(value).unwrap_or(i64::MAX));
    row.set("runId", text(&call.run_id));
    row.set("stepSeq", call.step_seq.map_or(Value::Null, Value::I64));
    row.set("userId", text(&call.user_id));
    row.set("featureId", text(&call.feature));
    row.set("modelId", Value::string(&call.model_id));
    row.set("endpointId", Value::string(&call.endpoint_id));
    row.set("providerModel", Value::string(&call.provider_model));
    row.set("protocol", Value::string(&call.protocol));
    row.set("source", Value::string(call.source.as_str()));
    row.set("clientId", text(&call.client_id));
    row.set("startDate", crate::user::stamp_value(call.start_millis));
    row.set("firstTokenMs", call.first_token_ms.map_or(Value::Null, Value::I64));
    row.set("durationMs", Value::I64(call.duration_ms));
    row.set("httpStatus", call.http_status.map_or(Value::Null, |status| Value::I64(i64::from(status))));
    row.set("finishReason", text(&call.finish));
    row.set("errorClass", text(&call.error_class));
    row.set(
        "errorText",
        call.error_text
            .as_ref()
            .map_or(Value::Null, |error| Value::string(redact(error, &content_log.redact_keys))),
    );
    row.set("attempt", Value::I64(i64::from(call.attempt)));
    row.set("inputTokens", count(call.usage.input));
    row.set("cachedInputTokens", count(call.usage.cached_input));
    row.set("cacheWriteTokens", count(call.usage.cache_write));
    row.set("outputTokens", count(call.usage.output));
    row.set("reasoningTokens", count(call.usage.reasoning));
    if call.usage_estimated {
        row.set("usageEstimated", Value::string("Y"));
    }
    if let Some(cost) = cost(prices, &call.usage) {
        row.set("cost", Value::Decimal(cost));
        row.set("currencyUomId", Value::string(&prices.currency));
    }
    if content_log.enabled {
        let keep = |body: &Option<String>| match body {
            None => Value::Null,
            Some(body) if content_log.redact => Value::string(redact(body, &content_log.redact_keys)),
            Some(body) => Value::string(body),
        };
        row.set("requestJson", keep(&call.request_json));
        row.set("responseJson", keep(&call.response_json));
    }
    row.create(facade)?;
    Ok(call_id)
}

/// The JSON keys whose values are always redacted, compared without case, `-` or `_`.
const SECRET_KEYS: [&str; 17] = [
    "apikey",
    "xapikey",
    "authorization",
    "proxyauthorization",
    "password",
    "passwd",
    "secret",
    "clientsecret",
    "token",
    "accesstoken",
    "refreshtoken",
    "idtoken",
    "sessiontoken",
    "bearer",
    "cookie",
    "setcookie",
    "privatekey",
];

const REDACTED: &str = "[redacted]";

fn normalized_key(key: &str) -> String {
    key.chars()
        .filter(|ch| *ch != '-' && *ch != '_')
        .flat_map(char::to_lowercase)
        .collect()
}

/// `text` with secrets replaced by `[redacted]`: in JSON, the values of secret-looking keys (`api_key`, `authorization`,
/// `password`, `token` and the like, not `max_tokens`) and of `extra_keys`; in every string, token-shaped words (`sk-…`,
/// `Bearer …`, GitHub and Slack tokens, AWS access key ids, JWTs). Text that is not JSON gets the word check only.
pub fn redact(text: &str, extra_keys: &[String]) -> String {
    let extra: Vec<String> = extra_keys.iter().map(|key| normalized_key(key)).collect();
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(mut value @ (serde_json::Value::Object(_) | serde_json::Value::Array(_))) => {
            // unchanged JSON keeps its own text (and key order)
            if redact_value(&mut value, &extra) {
                value.to_string()
            } else {
                text.to_string()
            }
        }
        _ => redact_words(text),
    }
}

/// Redact in place; true when anything changed.
fn redact_value(value: &mut serde_json::Value, extra: &[String]) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            let mut changed = false;
            for (key, child) in map.iter_mut() {
                let key = normalized_key(key);
                if SECRET_KEYS.contains(&key.as_str()) || extra.contains(&key) {
                    if !child.is_null() {
                        *child = serde_json::Value::String(REDACTED.to_string());
                        changed = true;
                    }
                } else {
                    changed |= redact_value(child, extra);
                }
            }
            changed
        }
        serde_json::Value::Array(items) => {
            let mut changed = false;
            for item in items {
                changed |= redact_value(item, extra);
            }
            changed
        }
        serde_json::Value::String(text) => {
            let redacted = redact_words(text);
            let changed = redacted != *text;
            if changed {
                *text = redacted;
            }
            changed
        }
        _ => false,
    }
}

fn token_shaped(word: &str, after_bearer: bool) -> bool {
    let long = |min: usize| word.len() >= min;
    (after_bearer && long(16))
        || (word.starts_with("sk-") && long(20))
        || (word.starts_with("sk_live_") && long(20))
        || ["ghp_", "gho_", "ghs_", "ghu_", "github_pat_", "xoxb-", "xoxp-", "xoxa-", "glpat-"]
            .iter()
            .any(|prefix| word.starts_with(prefix) && long(prefix.len() + 12))
        || (word.len() == 20
            && word.starts_with("AKIA")
            && word.chars().all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit()))
        || (word.starts_with("eyJ") && word.matches('.').count() == 2 && long(30))
}

/// Replace the token-shaped words of `text`; a word is a run of letters, digits and `-_.`.
fn redact_words(text: &str) -> String {
    let is_word = |ch: char| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.');
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    let mut previous = String::new();
    let flush = |word: &mut String, previous: &mut String, out: &mut String| {
        if word.is_empty() {
            return;
        }
        let trimmed = word.trim_end_matches('.');
        if token_shaped(trimmed, previous.eq_ignore_ascii_case("bearer")) {
            out.push_str(REDACTED);
            out.push_str(&word[trimmed.len()..]);
        } else {
            out.push_str(word);
        }
        *previous = std::mem::take(word);
    };
    for ch in text.chars() {
        if is_word(ch) {
            word.push(ch);
        } else {
            flush(&mut word, &mut previous, &mut out);
            out.push(ch);
        }
    }
    flush(&mut word, &mut previous, &mut out);
    out
}

/// A day in the server's time zone and the milliseconds it starts and ends at.
pub fn day_bounds(date: jiff::civil::Date) -> Result<(i64, i64), Error> {
    let zone = jiff::tz::TimeZone::system();
    let start = date
        .to_zoned(zone.clone())
        .map_err(|error| Error::new(format!("the start of {date}: {error}")))?;
    let end = date
        .tomorrow()
        .and_then(|next| next.to_zoned(zone))
        .map_err(|error| Error::new(format!("the end of {date}: {error}")))?;
    Ok((start.timestamp().as_millisecond(), end.timestamp().as_millisecond()))
}

/// The day of `millis` in the server's time zone.
pub fn day_of(millis: i64) -> Result<jiff::civil::Date, Error> {
    let stamp = jiff::Timestamp::from_millisecond(millis).map_err(|error| Error::new(error.to_string()))?;
    Ok(stamp.to_zoned(jiff::tz::TimeZone::system()).date())
}

/// The totals of a set of calls.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub calls: u64,
    pub errors: u64,
    pub usage: Usage,
    pub cost: Decimal,
    pub currency: Option<String>,
}

impl Totals {
    fn add_row(&mut self, row: &crate::entity::EntityValue) {
        let tokens = |field: &str| int_of(row, field).and_then(|n| u64::try_from(n).ok()).unwrap_or(0);
        self.calls += 1;
        if text_of(row, "errorClass").is_some() {
            self.errors += 1;
        }
        self.usage.add(&Usage {
            input: tokens("inputTokens"),
            cached_input: tokens("cachedInputTokens"),
            cache_write: tokens("cacheWriteTokens"),
            output: tokens("outputTokens"),
            reasoning: tokens("reasoningTokens"),
        });
        if let Some(cost) = decimal_of(row, "cost") {
            self.cost += cost;
        }
        if self.currency.is_none() {
            self.currency = text_of(row, "currencyUomId");
        }
    }

    fn add_daily_row(&mut self, row: &crate::entity::EntityValue) {
        let count = |field: &str| int_of(row, field).and_then(|n| u64::try_from(n).ok()).unwrap_or(0);
        self.calls += count("callCount");
        self.errors += count("errorCount");
        self.usage.add(&Usage {
            input: count("inputTokens"),
            cached_input: count("cachedInputTokens"),
            cache_write: count("cacheWriteTokens"),
            output: count("outputTokens"),
            reasoning: count("reasoningTokens"),
        });
        self.cost += decimal_of(row, "cost").unwrap_or_default();
        if self.currency.is_none() {
            self.currency = text_of(row, "currencyUomId");
        }
    }
}

/// Recompute the `LlmUsageDaily` rows of `date` from its `LlmCall` rows, by user, model and source: the day's rows are
/// replaced whole, so running it again gives the same rows. It reads with a cursor, so it runs in a transaction. Out:
/// the number of rows written.
///
/// # Errors
///
/// Returns an error when the calls cannot be read or the rows written.
pub fn rollup_day(facade: &EntityFacade, date: jiff::civil::Date) -> Result<usize, Error> {
    let (start, end) = day_bounds(date)?;
    let mut groups: BTreeMap<(String, String, String), Totals> = BTreeMap::new();
    let mut calls = facade
        .find(LLM_CALL)?
        .condition(Condition::greater_equal("startDate", crate::user::stamp_value(start)))
        .condition(Condition::less("startDate", crate::user::stamp_value(end)))
        .disable_authz()
        .iterator()?;
    while let Some(row) = calls.next_value()? {
        let key = (
            text_of(&row, "userId").unwrap_or_else(|| NO_USER.to_string()),
            text_of(&row, "modelId").unwrap_or_else(|| NO_USER.to_string()),
            text_of(&row, "source").unwrap_or_else(|| CallSource::Agent.as_str().to_string()),
        );
        groups.entry(key).or_default().add_row(&row);
    }
    calls.close()?;
    facade
        .find(LLM_USAGE_DAILY)?
        .eq("usageDate", Value::Date(date))
        .disable_authz()
        .delete_all()?;
    let now = crate::user::now_value();
    let count = |value: u64| Value::I64(i64::try_from(value).unwrap_or(i64::MAX));
    for ((user_id, model_id, source), totals) in &groups {
        let mut row = facade.make_value(LLM_USAGE_DAILY)?;
        row.set("userId", Value::string(user_id));
        row.set("usageDate", Value::Date(date));
        row.set("modelId", Value::string(model_id));
        row.set("source", Value::string(source));
        row.set("callCount", count(totals.calls));
        row.set("errorCount", count(totals.errors));
        row.set("inputTokens", count(totals.usage.input));
        row.set("cachedInputTokens", count(totals.usage.cached_input));
        row.set("cacheWriteTokens", count(totals.usage.cache_write));
        row.set("outputTokens", count(totals.usage.output));
        row.set("reasoningTokens", count(totals.usage.reasoning));
        row.set("totalTokens", count(totals.usage.total()));
        row.set("cost", Value::Decimal(totals.cost));
        row.set(
            "currencyUomId",
            totals.currency.as_ref().map_or(Value::Null, Value::string),
        );
        row.set("rolledUpDate", now.clone());
        row.create(facade)?;
    }
    Ok(groups.len())
}

/// The totals of all calls that started in `[start, end)` (milliseconds), from `LlmCall`, read with a cursor, so in a
/// transaction.
///
/// # Errors
///
/// Returns an error when the calls cannot be read.
pub fn totals_between(facade: &EntityFacade, start: i64, end: i64) -> Result<Totals, Error> {
    let mut totals = Totals::default();
    let mut calls = facade
        .find(LLM_CALL)?
        .condition(Condition::greater_equal("startDate", crate::user::stamp_value(start)))
        .condition(Condition::less("startDate", crate::user::stamp_value(end)))
        .disable_authz()
        .iterator()?;
    while let Some(row) = calls.next_value()? {
        totals.add_row(&row);
    }
    calls.close()?;
    Ok(totals)
}

/// The totals of the `LlmUsageDaily` rows from `from` up to but not including `to`.
///
/// # Errors
///
/// Returns an error when the rows cannot be read.
pub fn daily_totals_between(facade: &EntityFacade, from: jiff::civil::Date, to: jiff::civil::Date) -> Result<Totals, Error> {
    let mut totals = Totals::default();
    if from >= to {
        return Ok(totals);
    }
    for row in facade
        .find(LLM_USAGE_DAILY)?
        .condition(Condition::greater_equal("usageDate", Value::Date(from)))
        .condition(Condition::less("usageDate", Value::Date(to)))
        .disable_authz()
        .list()?
        .iter()
    {
        totals.add_daily_row(row);
    }
    Ok(totals)
}

/// Clear the request and response content of the calls that started more than `retention_days` before `now_millis`.
/// Out: the number of calls cleared.
///
/// # Errors
///
/// Returns an error when the update fails.
pub fn clean_call_content(facade: &EntityFacade, retention_days: u32, now_millis: i64) -> Result<i64, Error> {
    let cutoff = now_millis - i64::from(retention_days) * 86_400_000;
    facade
        .find(LLM_CALL)?
        .condition(Condition::less("startDate", crate::user::stamp_value(cutoff)))
        .condition(Condition::or(vec![
            Condition::is_not_null("requestJson"),
            Condition::is_not_null("responseJson"),
        ]))
        .disable_authz()
        .update_all([
            ("requestJson".to_string(), Value::Null),
            ("responseJson".to_string(), Value::Null),
        ])
}

/// The limit that applies to a user for one scope and period.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuotaLimit {
    /// `day` or `month`.
    pub period: String,
    /// `None` is no limit.
    pub max_tokens: Option<u64>,
    pub max_cost: Option<Decimal>,
    pub requests_per_minute: Option<u32>,
    /// `conf`, or the `quotaId` of the row.
    pub from: String,
    /// The limit is for all sources together (a row with scope `all` or none, or the conf's), so every source's calls
    /// count against it, not only the scope's being checked.
    pub all_sources: bool,
}

/// The use against the limits, and whether one is over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuotaStatus {
    /// The limit that is over, else the first checked.
    pub limit: QuotaLimit,
    /// The use counted against [`Self::limit`].
    pub used: Totals,
    /// Why the user is over a limit; `None` while within all of them.
    pub exceeded: Option<String>,
}

impl QuotaStatus {
    pub fn is_within(&self) -> bool {
        self.exceeded.is_none()
    }
}

fn period_of(row: &crate::entity::EntityValue) -> String {
    text_of(row, "period").unwrap_or_else(|| "day".to_string())
}

fn limit_of(row: &crate::entity::EntityValue) -> QuotaLimit {
    QuotaLimit {
        period: period_of(row),
        max_tokens: int_of(row, "maxTokens").and_then(|n| u64::try_from(n).ok()).filter(|n| *n > 0),
        max_cost: decimal_of(row, "maxCost").filter(|cost| !cost.is_zero()),
        requests_per_minute: int_of(row, "requestsPerMinute").and_then(|n| u32::try_from(n).ok()).filter(|n| *n > 0),
        from: text_of(row, "quotaId").unwrap_or_default(),
        all_sources: text_of(row, "scope").is_none_or(|scope| scope == "all"),
    }
}

/// Per period, the rows for `scope` itself if there are any, else the `all` ones.
fn by_period(rows: &[crate::entity::EntityValue], scope: &str) -> BTreeMap<String, Vec<QuotaLimit>> {
    let mut out: BTreeMap<String, Vec<QuotaLimit>> = BTreeMap::new();
    let mut periods: Vec<String> = rows.iter().map(period_of).collect();
    periods.sort();
    periods.dedup();
    for period in periods {
        let in_period: Vec<&crate::entity::EntityValue> = rows.iter().filter(|row| period_of(row) == period).collect();
        let own_scope: Vec<QuotaLimit> = in_period
            .iter()
            .filter(|row| text_of(row, "scope").as_deref() == Some(scope) && scope != "all")
            .map(|row| limit_of(row))
            .collect();
        let chosen = if own_scope.is_empty() {
            in_period.iter().map(|row| limit_of(row)).collect()
        } else {
            own_scope
        };
        out.insert(period, chosen);
    }
    out
}

/// The limits for `user_id` in `scope` (all, agent, gateway, api), one per period (`day`, `month`), each checked on
/// its own: the user's own `LlmQuota` rows (per period, the row for the scope, else the one for all scopes), else the
/// largest of the rows of the user's groups in each period, else the conf's daily limits for all sources.
///
/// # Errors
///
/// Returns an error when the rows cannot be read.
pub fn quota_limits(facade: &EntityFacade, limits: &LimitsConf, user_id: &str, scope: &str) -> Result<Vec<QuotaLimit>, Error> {
    let applies = |row: &crate::entity::EntityValue| {
        crate::llm::flag_of(row, "disabled") != Some(true)
            && text_of(row, "scope").is_none_or(|known| known == "all" || known == scope)
    };
    let own: Vec<crate::entity::EntityValue> = facade
        .find(LLM_QUOTA)?
        .eq("userId", Value::string(user_id))
        .use_cache(true)
        .disable_authz()
        .list()?
        .iter()
        .filter(|row| applies(row))
        .cloned()
        .collect();
    if !own.is_empty() {
        return Ok(by_period(&own, scope)
            .into_values()
            .filter_map(|limits| limits.into_iter().next())
            .collect());
    }
    let now = crate::user::now_value();
    let mut groups: Vec<Value> = facade
        .find("moqui.security.UserGroupMember")?
        .eq("userId", Value::string(user_id))
        .disable_authz()
        .list()?
        .iter()
        .filter(|row| {
            row.get("thruDate").is_none_or(|thru| {
                thru.is_null() || moqui_lib::object::compare_obj(thru, &now).is_ok_and(|order| order.is_gt())
            })
        })
        .filter_map(|row| row.get("userGroupId").cloned())
        .collect();
    groups.push(Value::string("ALL_USERS"));
    let group_rows: Vec<crate::entity::EntityValue> = facade
        .find(LLM_QUOTA)?
        .condition(Condition::is_in("userGroupId", groups))
        .use_cache(true)
        .disable_authz()
        .list()?
        .iter()
        .filter(|row| applies(row))
        .cloned()
        .collect();
    if !group_rows.is_empty() {
        return Ok(by_period(&group_rows, scope)
            .into_values()
            .filter_map(|limits| limits.into_iter().reduce(larger))
            .collect());
    }
    Ok(vec![QuotaLimit {
        period: "day".to_string(),
        max_tokens: Some(limits.user_daily_tokens).filter(|n| *n > 0),
        max_cost: limits.user_daily_cost,
        requests_per_minute: None,
        from: "conf".to_string(),
        all_sources: true,
    }])
}

/// The more generous of two group limits of the same period and kind, field by field (no limit is the largest).
fn larger(left: QuotaLimit, right: QuotaLimit) -> QuotaLimit {
    fn max_of<T: Ord>(left: Option<T>, right: Option<T>) -> Option<T> {
        match (left, right) {
            (Some(left), Some(right)) => Some(left.max(right)),
            _ => None,
        }
    }
    QuotaLimit {
        period: left.period.clone(),
        max_tokens: max_of(left.max_tokens, right.max_tokens),
        max_cost: max_of(left.max_cost, right.max_cost),
        requests_per_minute: max_of(left.requests_per_minute, right.requests_per_minute),
        from: format!("{}, {}", left.from, right.from),
        all_sources: left.all_sources && right.all_sources,
    }
}

/// The calls of `user_id` in `scope` (all for every source) from `start` until now, from `LlmCall`.
fn calls_since(facade: &EntityFacade, user_id: &str, scope: &str, start: i64) -> Result<Totals, Error> {
    let mut find = facade
        .find(LLM_CALL)?
        .eq("userId", Value::string(user_id))
        .condition(Condition::greater_equal("startDate", crate::user::stamp_value(start)))
        .disable_authz();
    if scope != "all" {
        find = find.eq("source", Value::string(scope));
    }
    let mut totals = Totals::default();
    for row in find.list()?.iter() {
        totals.add_row(row);
    }
    Ok(totals)
}

/// Whether `user_id` may make more calls in `scope` (all, agent, gateway, api) at `now_millis`: each limit of
/// [`quota_limits`] against today's calls (for a monthly limit with the month's earlier days, from `LlmUsageDaily`), of
/// every source for a limit for all sources, else of the scope's.
///
/// # Errors
///
/// Returns an error when the usage or the limits cannot be read.
pub fn check_quota(
    facade: &EntityFacade,
    limits: &LimitsConf,
    user_id: &str,
    scope: &str,
    now_millis: i64,
) -> Result<QuotaStatus, Error> {
    check_quota_reserving(facade, limits, user_id, scope, now_millis, &Reserve::default())
}

/// What a call may use at most, counted against the quotas before it is made ([`check_quota_reserving`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reserve {
    pub tokens: u64,
    pub cost: Decimal,
}

impl Reserve {
    /// Whether `used` and this reserve go over `max_tokens` or `max_cost`: what is used reaching the limit, or with the
    /// reserve going past it.
    pub fn exceeds(&self, used: &Totals, max_tokens: Option<u64>, max_cost: Option<Decimal>) -> Option<String> {
        let tokens = used.usage.total();
        match (max_tokens, max_cost) {
            (Some(max), _) if tokens >= max || (self.tokens > 0 && tokens.saturating_add(self.tokens) > max) => Some(
                if tokens >= max {
                    format!("the token quota of {max} is used up ({tokens} used)")
                } else {
                    format!(
                        "the token quota of {max} has {} left, and this call may use {}",
                        max - tokens,
                        self.tokens
                    )
                },
            ),
            (_, Some(max)) if used.cost >= max || (!self.cost.is_zero() && used.cost + self.cost > max) => Some(
                if used.cost >= max {
                    format!("the cost quota of {max} is used up ({} used)", used.cost.normalize())
                } else {
                    format!(
                        "the cost quota of {max} has {} left, and this call may cost {}",
                        (max - used.cost).normalize(),
                        self.cost.normalize()
                    )
                },
            ),
            _ => None,
        }
    }
}

/// [`check_quota`] with what the call may use at most: a limit is over when the use reaches it, or when the use and
/// `reserve` go past it.
///
/// # Errors
///
/// Returns an error when the usage or the limits cannot be read.
pub fn check_quota_reserving(
    facade: &EntityFacade,
    limits: &LimitsConf,
    user_id: &str,
    scope: &str,
    now_millis: i64,
    reserve: &Reserve,
) -> Result<QuotaStatus, Error> {
    let today = day_of(now_millis)?;
    let (today_start, _) = day_bounds(today)?;
    let mut first: Option<QuotaStatus> = None;
    for limit in quota_limits(facade, limits, user_id, scope)? {
        let counted = if limit.all_sources { "all" } else { scope };
        let mut used = calls_since(facade, user_id, counted, today_start)?;
        if limit.period == "month" && today.day() > 1 {
            let mut find = facade
                .find(LLM_USAGE_DAILY)?
                .eq("userId", Value::string(user_id))
                .condition(Condition::greater_equal("usageDate", Value::Date(today.first_of_month())))
                .condition(Condition::less("usageDate", Value::Date(today)))
                .disable_authz();
            if counted != "all" {
                find = find.eq("source", Value::string(counted));
            }
            for row in find.list()?.iter() {
                used.add_daily_row(row);
            }
        }
        let period = if limit.period == "month" { "this month" } else { "today" };
        let exceeded = reserve
            .exceeds(&used, limit.max_tokens, limit.max_cost)
            .map(|reason| reason.replacen(" is used up", &format!(" {period} is used up"), 1).replacen(" has ", &format!(" {period} has "), 1));
        let status = QuotaStatus { limit, used, exceeded };
        if !status.is_within() {
            return Ok(status);
        }
        first.get_or_insert(status);
    }
    Ok(first.unwrap_or_else(|| QuotaStatus {
        limit: QuotaLimit::default(),
        used: Totals::default(),
        exceeded: None,
    }))
}

/// The smallest requests-per-minute limit of `limits`, if any sets one.
pub fn requests_per_minute(limits: &[QuotaLimit]) -> Option<u32> {
    limits.iter().filter_map(|limit| limit.requests_per_minute).min()
}

/// The requests of the last minute per key, on this node.
static REQUESTS: Mutex<Option<HashMap<String, VecDeque<i64>>>> = Mutex::new(None);

/// Count one request of `key` (`user:<id>`, `client:<id>`) at `now_millis` against `per_minute` in the minute before
/// it. Out: `Err(seconds)` to wait when the minute already holds `per_minute` requests (this one is not counted).
///
/// # Errors
///
/// Returns the whole seconds until a request of the minute ages out.
pub fn take_request(key: &str, per_minute: u32, now_millis: i64) -> Result<(), u64> {
    let mut all = REQUESTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let all = all.get_or_insert_with(HashMap::new);
    if all.len() > 10_000 {
        all.retain(|_, times| times.back().is_some_and(|last| now_millis - last < 60_000));
    }
    let times = all.entry(key.to_string()).or_default();
    while times.front().is_some_and(|first| now_millis - first >= 60_000) {
        times.pop_front();
    }
    if times.len() >= per_minute.max(1) as usize {
        let oldest = times.front().copied().unwrap_or(now_millis);
        let wait = (oldest + 60_000 - now_millis).max(1);
        return Err(u64::try_from(wait).unwrap_or(1).div_ceil(1000));
    }
    times.push_back(now_millis);
    Ok(())
}

/// The calls made through the `LlmApiClient` `client_id` from `start` until now.
///
/// # Errors
///
/// Returns an error when the calls cannot be read.
pub fn client_calls_since(facade: &EntityFacade, client_id: &str, start: i64) -> Result<Totals, Error> {
    let mut totals = Totals::default();
    for row in facade
        .find(LLM_CALL)?
        .eq("clientId", Value::string(client_id))
        .condition(Condition::greater_equal("startDate", crate::user::stamp_value(start)))
        .disable_authz()
        .list()?
        .iter()
    {
        totals.add_row(row);
    }
    Ok(totals)
}

/// The seconds from `now_millis` until the period of `limit` (`day` or `month`) ends and its use starts again from 0.
///
/// # Errors
///
/// Returns an error when the server's calendar cannot place `now_millis`.
pub fn seconds_until_reset(limit: &QuotaLimit, now_millis: i64) -> Result<u64, Error> {
    let today = day_of(now_millis)?;
    let last_day = if limit.period == "month" { today.last_of_month() } else { today };
    let (_, end) = day_bounds(last_day)?;
    Ok(u64::try_from((end - now_millis).max(1_000)).unwrap_or(1_000).div_ceil(1000))
}
