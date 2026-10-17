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

//! Small helpers the service bodies share: the current message errors, the clock, parameter and field
//! text, and a unit of work on a reserve connection.

use std::collections::BTreeMap;

use indexmap::IndexMap;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{basic_convert, is_empty, to_plain_string, Value};

use crate::context::{Error, ExecutionContext};
use crate::driver::DbConnection;
use crate::entity::EntityValue;
use crate::transaction::TransactionManager;

/// True when the current context's message facade holds an error. False with no current context.
pub(crate) fn has_error() -> bool {
    ExecutionContext::with_current(|ctx| !ctx.message().errors().is_empty()).unwrap_or(false)
}

/// Java's `getErrorsString()` then `clearErrors()`, for a caller that consumes the errors a call left: their text, or
/// `None` when there are none.
pub(crate) fn take_errors_string() -> Option<String> {
    ExecutionContext::with_current(|ctx| {
        let mut messages = ctx.message();
        if messages.errors().is_empty() {
            return None;
        }
        let text = messages.errors_string();
        messages.clear_errors();
        Some(text)
    })
    .flatten()
}

/// Now, in epoch milliseconds.
pub(crate) fn now_millis() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

/// Now as a `Timestamp` value, to the millisecond.
pub(crate) fn now_stamp() -> Value {
    basic_convert(&Value::I64(now_millis()), "Timestamp").value
}

/// The plain text of `value`. Empty for a missing or empty value.
pub(crate) fn text_of(value: Option<&Value>) -> String {
    match value {
        Some(value) if !is_empty(value) => to_plain_string(value),
        _ => String::new(),
    }
}

/// The plain text of parameter `name`. Empty when it is missing or empty.
pub(crate) fn param_text(parameters: &BTreeMap<String, Value>, name: &str) -> String {
    text_of(parameters.get(name))
}

/// The plain text of entry `name` of `map`. Empty when it is missing or empty.
pub(crate) fn map_text(map: &IndexMap<String, Value>, name: &str) -> String {
    text_of(map.get(name))
}

/// The plain text of field `name` of `row`. Empty when it is missing or empty.
pub(crate) fn field_text(row: &EntityValue, name: &str) -> String {
    text_of(row.get(name))
}

/// Run `body` in its own transaction on a reserve connection: begin, then commit when `body` succeeds,
/// else roll back. A failed commit is rolled back too. The caller's transaction is not touched.
pub(crate) fn reserve<T>(
    tx: &TransactionManager,
    body: impl FnOnce(&mut dyn DbConnection) -> Result<T, LibError>,
) -> Result<T, Error> {
    tx.with_reserve(|conn| {
        conn.begin()?;
        match body(conn) {
            Ok(value) => {
                if let Err(err) = conn.commit() {
                    let _ = conn.rollback();
                    return Err(err);
                }
                Ok(value)
            }
            Err(err) => {
                let _ = conn.rollback();
                Err(err)
            }
        }
    })
    .map_err(Error::from)
}
