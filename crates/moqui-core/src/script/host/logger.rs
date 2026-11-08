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
//! `message: name: text`.

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
    ],
    getters: &[],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

/// The message at `index` with the exception after it.
fn message(call: &Call<'_>, index: usize) -> String {
    let message = call.text(index).unwrap_or_default().to_string();
    match call.arg(index + 1) {
        Arg::Text(thrown) => format!("{message}: {thrown}"),
        _ => message,
    }
}

/// `log(level, message, thrown)`: the level is a name or one of Java's `LoggerFacade` numbers, `ERROR_INT` 200 and
/// `WARN_INT` 300 among them. `OFF_INT` 0 and `"off"` log nothing.
fn log(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let message = message(call, 1);
    let number = match call.value(0) {
        Some(Value::I32(number)) => Some(f64::from(*number)),
        #[allow(clippy::cast_precision_loss)] // a level number is small
        Some(Value::I64(number)) => Some(*number as f64),
        Some(Value::F64(number)) => Some(*number),
        _ => None,
    };
    let level = match number {
        Some(0.0) => "off".to_string(),
        Some(number) if number <= 300.0 || number >= f64::from(i32::MAX) => "warn".to_string(),
        Some(_) => "info".to_string(),
        None => match call.value(0) {
            Some(Value::String(text)) => text.clone(),
            _ => String::new(),
        },
    };
    match level.as_str() {
        "off" => {}
        "error" => tracing::error!("{message}"),
        "warn" | "warning" => tracing::warn!("{message}"),
        "debug" => tracing::debug!("{message}"),
        "trace" => tracing::trace!("{message}"),
        _ => tracing::info!("{message}"),
    }
    Ok(Ret::Undefined)
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
