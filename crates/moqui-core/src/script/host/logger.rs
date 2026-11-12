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

//! `ec.logger`: Java's `LoggerFacade` by level. The last argument of each call may be the exception, logged as
//! `message: name: text`. The level numbers are Java's (`ec.logger.ERROR_INT` and the rest); there is no fatal level
//! here, so `FATAL_INT` logs as an error and `ALL_INT` as a trace.

use moqui_lib::object::Value;

use super::{method, Arg, Call, HostClass, HostError, Ret};

pub(super) static LOGGER: HostClass = HostClass {
    name: "host:Logger",
    methods: &[
        method!("ec.logger", "log", [Value, Text, Thrown], None, leaf, log),
        method!("ec.logger", "warn", [Text, Thrown], None, leaf, warn),
        method!("ec.logger", "info", [Text, Thrown], None, leaf, info),
        method!("ec.logger", "error", [Text, Thrown], None, leaf, error),
        method!("ec.logger", "debug", [Text, Thrown], None, leaf, debug),
        method!("ec.logger", "trace", [Text, Thrown], None, leaf, trace),
        method!("ec.logger", "logEnabled", [Value], None, leaf, log_enabled),
    ],
    getters: &[],
    hidden: &[],
    shim_js: Some(LOGGER_JS),
    // the level constants are numbers, not members a script reaches below
    shim_members: &[],
};

/// Java's `LoggerFacade` level numbers, as its constants.
const LOGGER_JS: &str = r#"(function (proto) {
    const levels = { OFF_INT: 0, FATAL_INT: 100, ERROR_INT: 200, WARN_INT: 300, INFO_INT: 400, DEBUG_INT: 500,
        TRACE_INT: 600, ALL_INT: 2147483647 };
    for (const [name, value] of Object.entries(levels)) {
        Object.defineProperty(proto, name, { value: value, writable: false, enumerable: false, configurable: false });
    }
})"#;

/// The level of a `log` or `logEnabled` argument: a name, or one of Java's numbers, taken as the nearest level at or
/// above it in severity (250 is a warning). `None` is off.
fn level_of(value: Option<&Value>) -> Option<tracing::Level> {
    let number = match value {
        Some(Value::I32(number)) => Some(f64::from(*number)),
        #[allow(clippy::cast_precision_loss)] // a level number is small
        Some(Value::I64(number)) => Some(*number as f64),
        Some(Value::F64(number)) => Some(*number),
        _ => None,
    };
    if let Some(number) = number {
        return match number {
            number if number <= 0.0 => None,
            number if number <= 200.0 => Some(tracing::Level::ERROR),
            number if number <= 300.0 => Some(tracing::Level::WARN),
            number if number <= 400.0 => Some(tracing::Level::INFO),
            number if number <= 500.0 => Some(tracing::Level::DEBUG),
            _ => Some(tracing::Level::TRACE),
        };
    }
    let name = match value {
        Some(Value::String(text)) => text.to_ascii_lowercase(),
        _ => String::new(),
    };
    match name.as_str() {
        "off" => None,
        "fatal" | "error" => Some(tracing::Level::ERROR),
        "warn" | "warning" => Some(tracing::Level::WARN),
        "debug" => Some(tracing::Level::DEBUG),
        "trace" | "all" => Some(tracing::Level::TRACE),
        _ => Some(tracing::Level::INFO),
    }
}

/// The message at `index` with the exception after it.
fn message(call: &Call<'_>, index: usize) -> String {
    let message = call.text(index).unwrap_or_default().to_string();
    match call.arg(index + 1) {
        Arg::Text(thrown) => format!("{message}: {thrown}"),
        _ => message,
    }
}

/// `log(level, message, thrown)`: the level is a name or one of Java's `LoggerFacade` numbers ([`level_of`]).
/// `OFF_INT` 0 and `"off"` log nothing.
fn log(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let message = message(call, 1);
    match level_of(call.value(0)) {
        None => {}
        Some(tracing::Level::ERROR) => tracing::error!("{message}"),
        Some(tracing::Level::WARN) => tracing::warn!("{message}"),
        Some(tracing::Level::DEBUG) => tracing::debug!("{message}"),
        Some(tracing::Level::TRACE) => tracing::trace!("{message}"),
        Some(_) => tracing::info!("{message}"),
    }
    Ok(Ret::Undefined)
}

/// Java's `logEnabled(level)`: whether a message at the level would be written.
fn log_enabled(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(match level_of(call.value(0)) {
        None => false,
        Some(tracing::Level::ERROR) => tracing::enabled!(tracing::Level::ERROR),
        Some(tracing::Level::WARN) => tracing::enabled!(tracing::Level::WARN),
        Some(tracing::Level::DEBUG) => tracing::enabled!(tracing::Level::DEBUG),
        Some(tracing::Level::TRACE) => tracing::enabled!(tracing::Level::TRACE),
        Some(_) => tracing::enabled!(tracing::Level::INFO),
    }
    .into())
}

fn warn(call: &mut Call<'_>) -> Result<Ret, HostError> {
    tracing::warn!("{}", message(call, 0));
    Ok(Ret::Undefined)
}

fn info(call: &mut Call<'_>) -> Result<Ret, HostError> {
    tracing::info!("{}", message(call, 0));
    Ok(Ret::Undefined)
}

fn error(call: &mut Call<'_>) -> Result<Ret, HostError> {
    tracing::error!("{}", message(call, 0));
    Ok(Ret::Undefined)
}

fn debug(call: &mut Call<'_>) -> Result<Ret, HostError> {
    tracing::debug!("{}", message(call, 0));
    Ok(Ret::Undefined)
}

fn trace(call: &mut Call<'_>) -> Result<Ret, HostError> {
    tracing::trace!("{}", message(call, 0));
    Ok(Ret::Undefined)
}
