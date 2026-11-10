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

//! `ec.web` for the request installed on this thread. The methods that coerce what a script passes (a text as
//! JavaScript's `String` makes it, the `message` of a thrown value, a flag that must be a boolean) and the
//! `sessionAttributes` proxy are written in JavaScript ([`WEB_SHIM_JS`]) over hidden functions that take plain values;
//! each keeps the `name` and `length` it had as a method of the QuickJS class. The tools shim adds `ec.web.request`,
//! `ec.web.response` and the rest (`host/web.rs`, `host/tools.rs`).

use moqui_lib::object::Value;

use super::{method, with_ec, Call, HostClass, HostError, Ret};

/// `(function (proto, hidden) { ... })`: the coercing methods and the `sessionAttributes` getter. A method called with
/// fewer arguments than it needs calls its hidden function with none, so the refusal is the one a typed method gave.
const WEB_SHIM_JS: &str = r#"(function (proto, hidden) {
    // a text as QuickJS's `Coerced<String>` makes it: JavaScript's ToString
    const coerced = (value) => value == null ? null : `${value}`;
    // the text of a message or a thrown value: a string, else an object's string `message`, else empty
    const textOf = (value) => {
        if (value == null) return "";
        if (typeof value === "string") return value;
        if ((typeof value === "object" || typeof value === "function") && typeof value.message === "string")
            return value.message;
        return "";
    };
    const flag = (value) => typeof value === "boolean" ? value : null;
    const define = (name, fn) => Object.defineProperty(proto, name,
        { value: fn, writable: true, enumerable: false, configurable: true });
    define("sendError", function sendError(status) {
        if (arguments.length < 1) return hidden.sendError();
        return hidden.sendError(status, textOf(arguments[1]), textOf(arguments[2]));
    });
    define("sendTextResponse", function sendTextResponse() {
        return hidden.sendTextResponse(coerced(arguments[0]), coerced(arguments[1]), coerced(arguments[2]));
    });
    define("sendResourceResponse", function sendResourceResponse(location) {
        if (arguments.length < 1) return hidden.sendResourceResponse();
        return hidden.sendResourceResponse(location, arguments[1] === true, coerced(arguments[2]));
    });
    define("saveScreenLastInfo", function saveScreenLastInfo() {
        return hidden.saveScreenLastInfo(typeof arguments[0] === "string" ? arguments[0] : null);
    });
    // Java's two forms: (requireFullUrl, useEncryption) and (webappName, servletContextPath, requireFullUrl, useEncryption)
    define("getWebappRootUrl", function getWebappRootUrl() {
        const named = typeof arguments[0] === "string";
        const webapp = named && arguments[0] !== "" ? arguments[0] : null;
        return named ? hidden.getWebappRootUrl(webapp, flag(arguments[2]), flag(arguments[3]))
            : hidden.getWebappRootUrl(null, flag(arguments[0]), flag(arguments[1]));
    });
    // A view of the session attributes. Reading a name gives its text; assigning, `put`, and `remove` change the
    // session. `ec.web.sessionAttributes.remove(name)` is the Java idiom the screens use.
    const sessionAttributes = (snapshot) => {
        const write = hidden.writeSessionAttribute;
        const methods = {
            get: (name) => snapshot[name],
            containsKey: (name) => Object.prototype.hasOwnProperty.call(snapshot, name),
            put: (name, value) => {
                const old = snapshot[name];
                if (value == null) delete snapshot[name]; else snapshot[name] = String(value);
                write(String(name), value == null ? null : String(value));
                return old;
            },
            remove: (name) => {
                const old = snapshot[name];
                delete snapshot[name];
                write(String(name), null);
                return old;
            }
        };
        return new Proxy(snapshot, {
            get(target, key) {
                if (typeof key === "string" && Object.prototype.hasOwnProperty.call(methods, key)) return methods[key];
                return target[key];
            },
            set(target, key, value) {
                methods.put(key, value);
                return true;
            },
            deleteProperty(target, key) {
                methods.remove(key);
                return true;
            }
        });
    };
    Object.defineProperty(proto, "sessionAttributes", {
        get: function () { return sessionAttributes(hidden.sessionSnapshot()); }, enumerable: false, configurable: true });
})"#;

/// `ec.web`.
pub(super) static WEB: HostClass = HostClass {
    name: "host:Web",
    methods: &[
        method!("ec.web", "sendJsonResponse", [Value], send_json_response),
        method!("ec.web", "sendJsonError", [Int, Text, OptValue], send_json_error),
        method!("ec.web", "sendQzSignedResponse", [Text], send_qz_signed_response),
        method!("ec.web", "viewEmailMessage", [], view_email_message),
        method!("ec.web", "getPageMessages", [], get_page_messages),
        method!("ec.web", "getScreenHistory", [], get_screen_history),
        method!("ec.web", "getHostName", [OptBool], get_host_name),
        method!("ec.web", "getResourceDistinctValue", [], get_resource_distinct_value),
        method!("ec.web", "rotateSession", [], rotate_session),
        method!("ec.web", "declineSession", [], decline_session),
    ],
    getters: &[
        method!("ec.web", "parameters", [], parameters),
        method!("ec.web", "savedMessages", [], saved_messages),
        method!("ec.web", "savedErrors", [], saved_errors),
        method!("ec.web", "savedValidationErrors", [], saved_validation_errors),
        method!("ec.web", "errorParameters", [], error_parameters),
        method!("ec.web", "multiPartParameters", [], multi_part_parameters),
        method!("ec.web", "requestParameters", [], parameters),
        method!("ec.web", "sessionToken", [], session_token),
    ],
    hidden: &[
        method!("ec.web", "sendError", [Int, OptText, OptText], send_error),
        method!(
            "ec.web",
            "sendTextResponse",
            [OptNullableText, OptNullableText, OptNullableText],
            send_text_response
        ),
        method!(
            "ec.web",
            "sendResourceResponse",
            [Text, OptBool, OptNullableText],
            send_resource_response
        ),
        method!("ec.web", "saveScreenLastInfo", [OptNullableText], save_screen_last_info),
        method!(
            "ec.web",
            "getWebappRootUrl",
            [OptNullableText, OptValue, OptValue],
            get_webapp_root_url
        ),
        method!("ec.web", "sessionSnapshot", [], session_snapshot),
        method!(
            "ec.web",
            "writeSessionAttribute",
            [Text, OptValue],
            write_session_attribute
        ),
    ],
    shim_js: Some(WEB_SHIM_JS),
    shim_members: &[
        "sendError",
        "sendTextResponse",
        "sendResourceResponse",
        "saveScreenLastInfo",
        "getWebappRootUrl",
        "sessionAttributes",
    ],
};

fn map_value(entries: std::collections::BTreeMap<String, Value>) -> Value {
    Value::Map(entries.into_iter().collect())
}

/// A value for a JSON response with its entity values and lists as plain maps and lists, at any depth, as Java's Jackson
/// writes them (`sendJsonResponse(enumList)` sends the rows, not "<object>").
fn json_plain(value: Value) -> Value {
    match value {
        Value::Object(_) => {
            if let Some(map) = crate::script::value_map(&value) {
                return Value::Map(map.into_iter().map(|(key, item)| (key, json_plain(item))).collect());
            }
            match crate::script::value_items(&value) {
                Some(items) => Value::List(items.into_iter().map(json_plain).collect()),
                None => value,
            }
        }
        Value::Map(entries) => Value::Map(entries.into_iter().map(|(key, item)| (key, json_plain(item))).collect()),
        Value::List(items) => Value::List(items.into_iter().map(json_plain).collect()),
        other => other,
    }
}

fn send_json_response(call: &mut Call<'_>) -> Result<Ret, HostError> {
    crate::web::send_json_response(json_plain(call.value_or_null(0)));
    Ok(Ret::Undefined)
}

fn send_json_error(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let status = u16::try_from(call.int(0).unwrap_or_default()).unwrap_or(500);
    crate::web::send_json_error(status, call.text(1).unwrap_or_default());
    Ok(Ret::Undefined)
}

/// `sendError(errorCode, message, throwable)`: the HTML, single-quoted JSON, or plain body `Accept` asks for. The shim
/// gives the texts of the message and the thrown value.
fn send_error(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let status = u16::try_from(call.int(0).unwrap_or_default()).unwrap_or(500);
    let throwable = call.text(2).unwrap_or_default();
    crate::web::send_error(
        status,
        call.text(1).unwrap_or_default(),
        (!throwable.is_empty()).then_some(throwable),
    );
    Ok(Ret::Undefined)
}

/// The QZ Tray signature of `message`, as `text/plain`. See `apps.xml` transition `qzSign`.
fn send_qz_signed_response(call: &mut Call<'_>) -> Result<Ret, HostError> {
    crate::web::send_qz_signed_response(call.text(0).unwrap_or_default());
    Ok(Ret::Undefined)
}

/// `sendTextResponse(text, contentType, filename)`: a null text is empty, the content type is `text/plain` by default,
/// and a file name makes the text an attachment of that name.
fn send_text_response(call: &mut Call<'_>) -> Result<Ret, HostError> {
    crate::web::send_text_response_with(call.text(0).unwrap_or_default(), call.text(1), call.text(2));
    Ok(Ret::Undefined)
}

/// `viewEmailMessage()`: the 1x1 tracking PNG for `/email/{emailMessageId}`. The id is the context path parameter.
fn view_email_message(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    crate::web::view_email_message();
    Ok(Ret::Undefined)
}

/// `sendResourceResponse(location, inline, fileName)`: a `#versionName` suffix sends that version, and a file name
/// names the attachment (a zip or a copy kept under a random name).
fn send_resource_response(call: &mut Call<'_>) -> Result<Ret, HostError> {
    crate::web::send_resource_response(
        call.text(0).unwrap_or_default(),
        call.bool(1).unwrap_or(false),
        call.text(2),
    );
    Ok(Ret::Undefined)
}

/// Remember the current screen path (or `path`) so a login can go back to it.
fn save_screen_last_info(call: &mut Call<'_>) -> Result<Ret, HostError> {
    crate::screen::save_screen_last_info(call.text(0));
    Ok(Ret::Undefined)
}

fn parameters(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(map_value(crate::web::parameters()).into())
}

/// A message line of the page: `{message, typeString}`.
fn entry(text: &str, kind: &str) -> Value {
    let mut map = indexmap::IndexMap::default();
    map.insert("message".to_string(), Value::string(text));
    map.insert("typeString".to_string(), Value::string(kind));
    Value::Map(map)
}

/// A validation error of the page: its message with the field it is for, and `field` and `form`.
fn invalid(message: &str, field: &str, form: &str) -> Value {
    let pretty = moqui_lib::string::camel_case_to_pretty(field);
    let text = if pretty.is_empty() {
        message.to_string()
    } else {
        format!("{message} (for field {pretty})")
    };
    let mut map = indexmap::IndexMap::default();
    map.insert("message".to_string(), Value::string(text));
    map.insert("typeString".to_string(), Value::string("danger"));
    map.insert("field".to_string(), Value::string(field));
    map.insert("form".to_string(), Value::string(form));
    Value::Map(map)
}

/// Messages, errors and validation errors for the page: what the last response saved, then what this request added.
/// Each is `{message, typeString}` with `info` or `danger`. A validation error is its message with the field it is
/// for, and also has `field` and `form`, so the html client can drop the line for one it shows by its field.
fn get_page_messages(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let saved = crate::web::saved_for_page();
    let (messages, errors, validation) = with_ec(|ec| {
        let facade = ec.message();
        let messages: Vec<(String, String)> = facade
            .messages()
            .iter()
            .enumerate()
            .map(|(index, text)| (text.clone(), facade.message_type(index).to_string()))
            .collect();
        let validation: Vec<(String, String, String)> = facade
            .validation_errors()
            .iter()
            .map(|error| (error.message.clone(), error.field.clone(), error.form.clone()))
            .collect();
        // a validation error is also a line of errors; it is shown once, with its field
        let errors: Vec<String> = facade
            .errors()
            .iter()
            .filter(|text| !validation.iter().any(|(message, _, _)| message == *text))
            .cloned()
            .collect();
        (messages, errors, validation)
    })?;
    let mut list = Vec::new();
    list.extend(
        saved
            .messages
            .iter()
            .zip(saved.message_types.iter())
            .map(|(text, kind)| entry(text, kind)),
    );
    list.extend(saved.errors.iter().map(|text| entry(text, "danger")));
    list.extend(saved.validation_errors.iter().map(|error| {
        let get = |key: &str| error.get(key).map(String::as_str).unwrap_or("");
        invalid(get("message"), get("field"), get("form"))
    }));
    list.extend(messages.iter().map(|(text, kind)| entry(text, kind)));
    list.extend(errors.iter().map(|text| entry(text, "danger")));
    list.extend(
        validation
            .iter()
            .map(|(message, field, form)| invalid(message, field, form)),
    );
    Ok(Value::List(list).into())
}

fn saved_messages(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let saved = crate::web::saved_for_page();
    let list = saved
        .messages
        .iter()
        .zip(saved.message_types.iter())
        .map(|(text, kind)| entry(text, kind))
        .collect();
    Ok(Value::List(list).into())
}

fn saved_errors(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let list = crate::web::saved_for_page()
        .errors
        .iter()
        .map(|text| Value::string(text.as_str()))
        .collect();
    Ok(Value::List(list).into())
}

/// The validation errors of the submit that led here: `{form, field, serviceName, message}` each.
fn saved_validation_errors(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let list = crate::web::saved_for_page()
        .validation_errors
        .into_iter()
        .map(|error| {
            Value::Map(
                error
                    .into_iter()
                    .map(|(key, value)| (key, Value::string(value)))
                    .collect(),
            )
        })
        .collect();
    Ok(Value::List(list).into())
}

fn error_parameters(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let map = crate::web::saved_for_page()
        .error_parameters
        .into_iter()
        .map(|(key, value)| (key, Value::string(value)))
        .collect();
    Ok(Value::Map(map).into())
}

/// The pages the user got in `html`, newest first, as `{name, url, urlNoParams}`.
fn get_screen_history(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let base = crate::web::base_url();
    let base = base.trim_end_matches('/');
    let history = crate::user::screen_history().unwrap_or_default();
    let list = history
        .iter()
        .map(|path| {
            let bare = path.split('?').next().unwrap_or(path);
            let name = bare
                .rsplit('/')
                .find(|part| !part.is_empty())
                .map_or_else(|| "Home".to_string(), crate::web::percent_decode);
            let mut map = indexmap::IndexMap::default();
            map.insert("name".to_string(), Value::string(name));
            map.insert("url".to_string(), Value::string(format!("{base}{path}")));
            map.insert("urlNoParams".to_string(), Value::string(format!("{base}{bare}")));
            map.insert("image".to_string(), Value::string(""));
            map.insert("imageType".to_string(), Value::string(""));
            Value::Map(map)
        })
        .collect();
    Ok(Value::List(list).into())
}

/// The host the request was for. `true` keeps the port.
fn get_host_name(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(crate::web::host_name(call.bool(0).unwrap_or(false)).into())
}

/// A value that changes when the server starts, for the `?v=` on asset links so a browser refetches them.
fn get_resource_distinct_value(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(crate::web::resource_distinct_value().into())
}

fn multi_part_parameters(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(map_value(crate::web::multipart_parameters()).into())
}

/// The root URL of the web app ([`crate::web::webapp_root_url`]): the shim gives the webapp name (none for
/// `webroot`) and the two flags, each a boolean or none.
fn get_webapp_root_url(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let flag = |value: Option<&Value>| match value {
        Some(Value::Bool(flag)) => Some(*flag),
        _ => None,
    };
    let webapp = call.text(0).unwrap_or("webroot").to_string();
    let (full, encryption) = (flag(call.value(1)), flag(call.value(2)));
    Ok(crate::web::webapp_root_url(&webapp, full.unwrap_or(false), encryption).into())
}

/// The session attributes now, each as text, for the `sessionAttributes` proxy.
fn session_snapshot(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let map = crate::user::session::session_attributes()
        .into_iter()
        .map(|(key, value)| (key, Value::string(value)))
        .collect();
    Ok(Value::Map(map).into())
}

fn write_session_attribute(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.text_or_empty(0);
    crate::user::session::set_session_attribute(&name, call.opt_text(1).as_deref());
    Ok(Ret::Undefined)
}

/// Rotate a frontend session after its authority verifies a delegated login.
fn rotate_session(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    crate::user::session::rotate_on_password_login().map_err(HostError::Context)?;
    Ok(Ret::Undefined)
}

/// Send this request's response with no session cookie or token (impl-commerce F86): see
/// [`crate::user::session::decline_session`].
fn decline_session(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    crate::user::session::decline_session();
    Ok(Ret::Undefined)
}

fn session_token(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(crate::user::csrf_token()
        .map_err(|err| HostError::message(err.message()))?
        .into())
}
