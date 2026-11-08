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

//! `ec.message`, the message facade of the entered execution context. There is no separate public-message list.

use moqui_lib::object::Value;

use super::{method, with_ec, Call, HostClass, HostError, Ret};

pub(super) static MESSAGE: HostClass = HostClass {
    name: "host:Message",
    methods: &[
        method!("ec.message", "addError", [Text], None, leaf, add_error),
        method!(
            "ec.message",
            "addValidationError",
            [OptValue, OptValue, OptValue, OptValue],
            None,
            leaf,
            add_validation_error
        ),
        method!("ec.message", "addMessage", [Text, OptText], None, leaf, add_message),
        method!("ec.message", "addPublic", [Text, OptText], None, leaf, add_message),
        method!("ec.message", "hasError", [], None, leaf, has_error),
        method!("ec.message", "clearErrors", [], None, leaf, clear_errors),
        method!("ec.message", "pushErrors", [], None, leaf, push_errors),
        method!("ec.message", "popErrors", [], None, leaf, pop_errors),
        method!("ec.message", "getErrorsString", [], None, leaf, errors_string),
    ],
    getters: &[
        method!("ec.message", "errorsString", [], None, leaf, errors_string),
        method!("ec.message", "errors", [], None, leaf, errors),
    ],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

fn add_error(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let message = call.text(0).unwrap_or_default().to_string();
    with_ec(|ec| ec.message().add_error(message))?;
    Ok(Ret::Undefined)
}

/// `addValidationError(form, field, serviceName, message, throwable)`.
fn add_validation_error(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let (form, field, service, message) = (
        call.text_or_empty(0),
        call.text_or_empty(1),
        call.text_or_empty(2),
        call.text_or_empty(3),
    );
    with_ec(|ec| ec.message().add_validation_error(&form, &field, &service, &message))?;
    Ok(Ret::Undefined)
}

/// `addMessage(message, type)`: `info`, `success`, `warning` or `danger`.
fn add_message(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let message = call.text(0).unwrap_or_default().to_string();
    let kind = call.text(1).unwrap_or_default().to_string();
    with_ec(|ec| ec.message().add_message_typed(message, &kind))?;
    Ok(Ret::Undefined)
}

fn has_error(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(with_ec(|ec| !ec.message().errors().is_empty())?.into())
}

fn clear_errors(_: &mut Call<'_>) -> Result<Ret, HostError> {
    with_ec(|ec| ec.message().clear_errors())?;
    Ok(Ret::Undefined)
}

fn push_errors(_: &mut Call<'_>) -> Result<Ret, HostError> {
    with_ec(|ec| ec.message().push_errors())?;
    Ok(Ret::Undefined)
}

fn pop_errors(_: &mut Call<'_>) -> Result<Ret, HostError> {
    with_ec(|ec| ec.message().pop_errors())?;
    Ok(Ret::Undefined)
}

fn errors_string(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(with_ec(|ec| ec.message().errors_string())?.into())
}

/// The number of errors, or null when there are none.
fn errors(_: &mut Call<'_>) -> Result<Ret, HostError> {
    let count = with_ec(|ec| ec.message().errors().len())?;
    if count == 0 {
        return Ok(Ret::Value(Value::Null));
    }
    let count = i32::try_from(count).map_err(|_| HostError::message("error count is too large"))?;
    Ok(Ret::Value(Value::I32(count)))
}
