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

//! `ec.message`, the message facade of the entered execution context, as Java's `MessageFacade`: the lists are
//! arrays (`errors`, `messages`, `publicMessages` of texts; `messageInfos` and `publicMessageInfos` of
//! `{message, typeString}`; `validationErrors` of `{form, field, serviceName, message, fieldPretty, ...}`), empty
//! when there is nothing. Each getter has its method form (`getErrors()`), made at the engine's start.

use moqui_lib::object::{Value, ValueMap};

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
        method!("ec.message", "addPublic", [Text, OptText], None, leaf, add_public),
        method!("ec.message", "hasError", [], None, leaf, has_error),
        method!("ec.message", "clearErrors", [], None, leaf, clear_errors),
        method!("ec.message", "clearAll", [], None, leaf, clear_all),
        method!("ec.message", "pushErrors", [], None, leaf, push_errors),
        method!("ec.message", "popErrors", [], None, leaf, pop_errors),
        method!("ec.message", "getErrorsString", [], None, leaf, errors_string),
    ],
    getters: &[
        method!("ec.message", "errorsString", [], None, leaf, errors_string),
        method!("ec.message", "errors", [], None, leaf, errors),
        method!("ec.message", "messages", [], None, leaf, messages),
        method!("ec.message", "messagesString", [], None, leaf, messages_string),
        method!("ec.message", "messageInfos", [], None, leaf, message_infos),
        method!("ec.message", "publicMessages", [], None, leaf, public_messages),
        method!("ec.message", "publicMessageInfos", [], None, leaf, public_message_infos),
        method!("ec.message", "validationErrors", [], None, leaf, validation_errors),
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

/// `addPublic(message, type)`: a message a public site may show too; it is among the messages as well.
fn add_public(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let message = call.text(0).unwrap_or_default().to_string();
    let kind = call.text(1).unwrap_or_default().to_string();
    with_ec(|ec| ec.message().add_public(message, &kind))?;
    Ok(Ret::Undefined)
}

/// Java's `clearAll`: the errors, the messages and the public messages.
fn clear_all(_: &mut Call<'_>) -> Result<Ret, HostError> {
    with_ec(|ec| ec.message().clear())?;
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

/// Java's `getErrors()`: the error texts, a validation error's message among them.
fn errors(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(with_ec(|ec| ec.message().errors().to_vec())?.into())
}

fn messages(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(with_ec(|ec| ec.message().messages().to_vec())?.into())
}

/// Java's `getMessagesString()`: each message and a newline.
fn messages_string(_: &mut Call<'_>) -> Result<Ret, HostError> {
    let mut text = String::new();
    for message in with_ec(|ec| ec.message().messages().to_vec())? {
        text.push_str(&message);
        text.push('\n');
    }
    Ok(text.into())
}

fn info(message: String, kind: &str) -> Value {
    let mut row = ValueMap::default();
    row.insert("message".to_string(), Value::String(message));
    row.insert("typeString".to_string(), Value::string(kind));
    Value::Map(row)
}

/// Java's `getMessageInfos()`: the messages as `{message, typeString}`.
fn message_infos(_: &mut Call<'_>) -> Result<Ret, HostError> {
    let rows = with_ec(|ec| {
        let facade = ec.message();
        let mut rows = Vec::with_capacity(facade.messages().len());
        for (index, message) in facade.messages().iter().enumerate() {
            rows.push(info(message.clone(), facade.message_type(index)));
        }
        rows
    })?;
    Ok(Ret::Value(Value::List(rows)))
}

fn public_messages(_: &mut Call<'_>) -> Result<Ret, HostError> {
    let texts: Vec<String> = with_ec(|ec| {
        ec.message()
            .public_messages()
            .iter()
            .map(|(message, _)| message.clone())
            .collect()
    })?;
    Ok(texts.into())
}

fn public_message_infos(_: &mut Call<'_>) -> Result<Ret, HostError> {
    let rows = with_ec(|ec| {
        ec.message()
            .public_messages()
            .iter()
            .map(|(message, kind)| info(message.clone(), kind))
            .collect()
    })?;
    Ok(Ret::Value(Value::List(rows)))
}

/// Java's `getValidationErrors()`: each as a plain object, with the pretty names Java's `ValidationError` gives.
fn validation_errors(_: &mut Call<'_>) -> Result<Ret, HostError> {
    let pretty = moqui_lib::string::camel_case_to_pretty;
    let rows = with_ec(|ec| {
        let mut rows = Vec::new();
        for error in ec.message().validation_errors() {
            let mut row = ValueMap::default();
            for (name, text) in [
                ("form", error.form.clone()),
                ("field", error.field.clone()),
                ("serviceName", error.service.clone()),
                ("message", error.message.clone()),
                ("formPretty", pretty(&error.form)),
                ("fieldPretty", pretty(&error.field)),
                ("serviceNamePretty", pretty(&error.service)),
                ("stringPretty", error.to_string_pretty()),
            ] {
                row.insert(name.to_string(), Value::String(text));
            }
            rows.push(Value::Map(row));
        }
        rows
    })?;
    Ok(Ret::Value(Value::List(rows)))
}
