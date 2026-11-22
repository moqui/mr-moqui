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

//! Small helpers the modules share: parameter and field reads, the current user, the clock and the transaction.

use std::collections::BTreeMap;

use moqui_core::context::ExecutionContext;
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::transaction::TransactionAttribute;
use moqui_lib::error::Error;
use moqui_lib::object::{is_empty, to_plain_string, Value};

pub(crate) type Parameters = BTreeMap<String, Value>;

/// The trimmed text of a parameter, `None` when it is absent or empty.
pub(crate) fn text(parameters: &Parameters, key: &str) -> Option<String> {
    parameters
        .get(key)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// The trimmed text of a field, `None` when it is null or empty.
pub(crate) fn text_of(row: &EntityValue, field: &str) -> Option<String> {
    row.get(field)
        .filter(|value| !value.is_null())
        .map(to_plain_string)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// A whole-number field.
pub(crate) fn int_of(row: &EntityValue, field: &str) -> Option<i64> {
    match row.get(field)? {
        Value::I32(n) => Some(i64::from(*n)),
        Value::I64(n) => Some(*n),
        Value::Null => None,
        other => to_plain_string(other).trim().parse().ok(),
    }
}

/// A date-time field as epoch milliseconds.
pub(crate) fn millis_of(row: &EntityValue, field: &str) -> Option<i64> {
    row.get(field).and_then(moqui_core::user::value_millis)
}

/// An error on the current context's message list.
pub(crate) fn add_error(text: impl Into<String>) {
    let text = text.into();
    ExecutionContext::with_current(|ctx| ctx.message().add_error(text));
}

/// The output map of a service.
pub(crate) fn out(entries: impl IntoIterator<Item = (&'static str, Value)>) -> BTreeMap<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

pub(crate) fn now_millis() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

/// A date-time field value for epoch milliseconds.
pub(crate) fn stamp(millis: i64) -> Value {
    moqui_core::user::stamp_value(millis)
}

/// ISO 8601 text for epoch milliseconds, as the wire carries dates (T3 `IsoDateTime`).
pub(crate) fn iso(millis: i64) -> String {
    jiff::Timestamp::from_millisecond(millis)
        .map(|stamp| stamp.to_string())
        .unwrap_or_default()
}

/// Epoch milliseconds of ISO 8601 text.
pub(crate) fn parse_iso(text: &str) -> Option<i64> {
    text.parse::<jiff::Timestamp>()
        .ok()
        .map(jiff::Timestamp::as_millisecond)
}

/// The logged-in user, `None` for an anonymous context.
pub(crate) fn current_user() -> Option<String> {
    moqui_core::user::current_user_id()
        .ok()
        .flatten()
        .filter(|id| !id.is_empty())
}

pub(crate) fn has_permission(permission: &str) -> bool {
    moqui_core::user::has_permission(permission).unwrap_or(false)
}

/// Run `body` in the transaction of `entity`'s group, joining one that is open.
pub(crate) fn in_tx<T>(
    facade: &EntityFacade,
    entity: &str,
    body: impl FnOnce() -> Result<T, Error>,
) -> Result<T, Error> {
    facade
        .transactions_for(entity)?
        .run(TransactionAttribute::UseOrBegin, None, body)
}

/// A JSON object as text, for the `*Json` fields.
pub(crate) fn json_text(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

/// A `*Json` field, `Null` when it is empty or not JSON.
pub(crate) fn json_of(row: &EntityValue, field: &str) -> serde_json::Value {
    text_of(row, field)
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(serde_json::Value::Null)
}
