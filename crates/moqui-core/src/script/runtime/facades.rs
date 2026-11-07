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

//! Smaller `ec` facades: resource and web. Cache, message, logger and user are in the host table (`script/host`).

use super::*;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;

pub(super) fn resource_expand(ctx: Ctx<'_>, template: String) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        expand_here(&ctx, &template).map_err(|err| Exception::throw_message(&ctx, err.message()))
    })
}

/// `sri.sendRedirectAndStopRender(url)`: the one `sri` method the framework screens call before
/// slice 2 adds the rest.
pub(super) fn sri_send_redirect_and_stop_render(ctx: Ctx<'_>, url: String) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        crate::screen::send_redirect_and_stop_render(&url);
        Ok(())
    })
}

/// `ec.resource.script(location)`: run the file in the script's own names and return its value, as Java does: what it
/// returns, or its last expression statement.
pub(super) fn resource_script<'js>(ctx: Ctx<'js>, location: String) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let (script, inert) = read_module(&location).map_err(|err| throw_context(&ctx, &err))?;
        let source = script.shared_text();
        let inert = inert && module_cache_on();
        // A module whose top level only declares gives the same value each time it runs: the value it gave earlier in
        // this outermost run is given again, its functions not made again (`parse::module_is_inert`).
        if inert {
            if let Some(kept) = super::kept_module(&ctx, &location, source) {
                return Ok(kept);
            }
        }
        let value = eval_compiled_text::<JsValue>(&ctx, &script, false).map_err(|err| throw_context(&ctx, &err))?;
        if inert {
            super::keep_module(&ctx, &location, Arc::clone(source), &value);
        }
        Ok(value)
    })
}

/// Whether module values are kept for the run: the `script_module_cache` property, true unless `false` (read once).
fn module_cache_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| moqui_lib::env::get_prop_or_env("script_module_cache") != "false")
}

/// A script file's text by location, as the function [`crate::script::parse::file_function`] makes of it, with the
/// modified time it had and when that was last checked.
struct ScriptText {
    path: std::path::PathBuf,
    modified: Option<std::time::SystemTime>,
    checked: Instant,
    /// The text, with its hash made once and the location as its name.
    text: super::ScriptText,
    /// Whether the file's top level only declares ([`crate::script::parse::module_is_inert`]).
    inert: bool,
}

/// How long a script file's text is used before its modified time is looked at again.
const SCRIPT_CHECK: Duration = Duration::from_secs(2);

fn script_texts() -> &'static Mutex<HashMap<String, ScriptText>> {
    static TEXTS: std::sync::OnceLock<Mutex<HashMap<String, ScriptText>>> = std::sync::OnceLock::new();
    TEXTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn modified_of(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|meta| meta.modified()).ok()
}

/// Drop the kept text of every script file among `files` at once, rather than at the next check (plans/design-dev-aide.md
/// 6.1, F6). Answers the script locations dropped.
pub fn forget_scripts(files: &[std::path::PathBuf]) -> Vec<String> {
    let canonical = |path: &std::path::Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let files: Vec<std::path::PathBuf> = files.iter().map(|file| canonical(file)).collect();
    let mut dropped = Vec::new();
    lock(script_texts()).retain(|location, kept| {
        let stale = files.contains(&canonical(&kept.path));
        if stale {
            dropped.push(location.clone());
        }
        !stale
    });
    dropped
}

/// A script file to run, kept by location as Java's `resource.script*.location` caches and read again when the file
/// changes: its text as the body of a function called at once (see [`crate::script::parse::file_function`]), so it runs
/// in names of its own and evaluates to what it returns. The compiled form is kept by the runtime by text.
pub(super) fn read_script(location: &str) -> Result<Arc<str>, Error> {
    read_module(location).map(|(script, _)| Arc::clone(script.shared_text()))
}

/// [`read_script`], and whether the file's top level only declares, so its value may be kept for the run.
pub(super) fn read_module(location: &str) -> Result<(super::ScriptText, bool), Error> {
    let production = crate::web::is_production();
    {
        let mut texts = lock(script_texts());
        if let Some(found) = texts.get_mut(location) {
            if production || found.checked.elapsed() < SCRIPT_CHECK {
                return Ok((found.text.clone(), found.inert));
            }
            if modified_of(&found.path) == found.modified {
                found.checked = Instant::now();
                return Ok((found.text.clone(), found.inert));
            }
        }
    }
    let path = crate::context::locate(location)?;
    let modified = modified_of(&path);
    let text = std::fs::read_to_string(&path)
        .map_err(|err| Error::new(format!("could not read script {location}")).with_source(err))?;
    let inert = crate::script::parse::module_is_inert(&text);
    let text: Arc<str> = crate::script::parse::file_function(&text).into();
    let script = super::ScriptText::named(text, location);
    lock(script_texts()).insert(
        location.to_string(),
        ScriptText {
            path,
            modified,
            checked: Instant::now(),
            text: script.clone(),
            inert,
        },
    );
    Ok((script, inert))
}

/// Run a script file as a service body does: in the current context frame, with bare names.
///
/// # Errors
///
/// Returns an error when the file cannot be read or the script throws.
pub fn run_script_location(location: &str) -> Result<(), Error> {
    let (script, _) = read_module(location)?;
    eval_script_text(&script)
}

/// `System.getProperty(name)`: a property or environment value, null when it is not set.
pub(super) fn system_get_property(ctx: Ctx<'_>, name: String) -> rquickjs::Result<Option<String>> {
    host_guard(&ctx, || {
        Ok(Some(moqui_lib::env::get_prop_or_env(&name)).filter(|value| !value.is_empty()))
    })
}

/// `ec.web` for the request installed on this thread.
#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub(super) struct HostWeb {}

#[rquickjs::methods(rename_all = "camelCase")]
impl HostWeb {
    fn send_json_response(&self, ctx: Ctx<'_>, value: JsValue<'_>) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            crate::web::send_json_response(json_plain(js_to_host(&ctx, &value)?));
            Ok(())
        })
    }

    fn send_json_error(
        &self,
        ctx: Ctx<'_>,
        status: i32,
        message: String,
        _extra: Opt<JsValue<'_>>,
    ) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            let status = u16::try_from(status).unwrap_or(500);
            crate::web::send_json_error(status, &message);
            Ok(())
        })
    }

    /// `sendError(errorCode, message, throwable)`: the HTML, single-quoted JSON, or plain body `Accept` asks for.
    fn send_error<'js>(
        &self,
        ctx: Ctx<'js>,
        status: i32,
        message: Opt<JsValue<'js>>,
        throwable: Opt<JsValue<'js>>,
    ) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            let text_of = |value: &Opt<JsValue<'js>>| -> String {
                let Some(value) = value.0.as_ref() else {
                    return String::new();
                };
                if value.is_null() || value.is_undefined() {
                    return String::new();
                }
                if let Some(text) = value.as_string().and_then(|text| text.to_string().ok()) {
                    return text;
                }
                value
                    .as_object()
                    .and_then(|object| object.get::<_, rquickjs::String>("message").ok())
                    .and_then(|text| text.to_string().ok())
                    .unwrap_or_default()
            };
            let message = text_of(&message);
            let throwable = text_of(&throwable);
            let status = u16::try_from(status).unwrap_or(500);
            crate::web::send_error(status, &message, (!throwable.is_empty()).then_some(throwable.as_str()));
            Ok(())
        })
    }

    /// The QZ Tray signature of `message`, as `text/plain`. See `apps.xml` transition `qzSign`.
    fn send_qz_signed_response(&self, ctx: Ctx<'_>, message: String) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            crate::web::send_qz_signed_response(&message);
            Ok(())
        })
    }

    /// `sendTextResponse(text, contentType, filename)`: a null text is empty, the content type is `text/plain` by default,
    /// and a file name makes the text an attachment of that name.
    fn send_text_response<'js>(
        &self,
        ctx: Ctx<'js>,
        text: Opt<JsValue<'js>>,
        content_type: Opt<JsValue<'js>>,
        file_name: Opt<JsValue<'js>>,
    ) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            let given = |value: &Opt<JsValue<'js>>| -> rquickjs::Result<Option<String>> {
                match value.0.as_ref() {
                    None => Ok(None),
                    Some(value) if value.is_null() || value.is_undefined() => Ok(None),
                    Some(value) => Ok(Some(
                        rquickjs::convert::Coerced::<String>::from_js(&ctx, value.clone())?.0,
                    )),
                }
            };
            let text = given(&text)?.unwrap_or_default();
            crate::web::send_text_response_with(&text, given(&content_type)?.as_deref(), given(&file_name)?.as_deref());
            Ok(())
        })
    }

    /// `viewEmailMessage()`: the 1x1 tracking PNG for `/email/{emailMessageId}`. The id is the context path parameter.
    fn view_email_message(&self, ctx: Ctx<'_>) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            crate::web::view_email_message();
            Ok(())
        })
    }

    /// `sendResourceResponse(location, inline, fileName)`: a `#versionName` suffix sends that version, and a file name
    /// names the attachment (a zip or a copy kept under a random name).
    fn send_resource_response<'js>(
        &self,
        ctx: Ctx<'js>,
        location: String,
        inline: Opt<JsValue<'js>>,
        file_name: Opt<JsValue<'js>>,
    ) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            let inline = inline.0.as_ref().is_some_and(|value| value.as_bool() == Some(true));
            let file_name = match file_name.0.as_ref() {
                Some(value) if !value.is_null() && !value.is_undefined() => {
                    Some(rquickjs::convert::Coerced::<String>::from_js(&ctx, value.clone())?.0)
                }
                _ => None,
            };
            crate::web::send_resource_response(&location, inline, file_name.as_deref());
            Ok(())
        })
    }

    /// Remember the current screen path (or `path`) so a login can go back to it.
    fn save_screen_last_info(
        &self,
        ctx: Ctx<'_>,
        path: Opt<JsValue<'_>>,
        _parameters: Opt<JsValue<'_>>,
    ) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            let given = path
                .0
                .and_then(|value| value.as_string().and_then(|text| text.to_string().ok()));
            crate::screen::save_screen_last_info(given.as_deref());
            Ok(())
        })
    }

    #[qjs(get)]
    fn parameters<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || host_to_js(&ctx, map_value(crate::web::parameters())))
    }

    /// Messages, errors and validation errors for the page: what the last response saved, then what this request added.
    /// Each is `{message, typeString}` with `info` or `danger`. A validation error is its message with the field it is
    /// for, and also has `field` and `form`, so the html client can drop the line for one it shows by its field.
    #[qjs(rename = "getPageMessages")]
    fn get_page_messages<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            let saved = crate::web::saved_for_page();
            let (messages, errors, validation) = with_ec(&ctx, |ec| {
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
            let entry = |text: &str, kind: &str| {
                let mut map = indexmap::IndexMap::new();
                map.insert("message".to_string(), Value::string(text));
                map.insert("typeString".to_string(), Value::string(kind));
                Value::Map(map)
            };
            let invalid = |message: &str, field: &str, form: &str| {
                let pretty = moqui_lib::string::camel_case_to_pretty(field);
                let text = if pretty.is_empty() {
                    message.to_string()
                } else {
                    format!("{message} (for field {pretty})")
                };
                let mut map = indexmap::IndexMap::new();
                map.insert("message".to_string(), Value::string(text));
                map.insert("typeString".to_string(), Value::string("danger"));
                map.insert("field".to_string(), Value::string(field));
                map.insert("form".to_string(), Value::string(form));
                Value::Map(map)
            };
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
            host_to_js(&ctx, Value::List(list))
        })
    }

    #[qjs(get, rename = "savedMessages")]
    fn saved_messages<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            let saved = crate::web::saved_for_page();
            let list = saved
                .messages
                .iter()
                .zip(saved.message_types.iter())
                .map(|(text, kind)| {
                    let mut map = indexmap::IndexMap::new();
                    map.insert("message".to_string(), Value::string(text.as_str()));
                    map.insert("typeString".to_string(), Value::string(kind.as_str()));
                    Value::Map(map)
                })
                .collect();
            host_to_js(&ctx, Value::List(list))
        })
    }

    #[qjs(get, rename = "savedErrors")]
    fn saved_errors<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            let list = crate::web::saved_for_page()
                .errors
                .iter()
                .map(|text| Value::string(text.as_str()))
                .collect();
            host_to_js(&ctx, Value::List(list))
        })
    }

    /// The validation errors of the submit that led here: `{form, field, serviceName, message}` each.
    #[qjs(get, rename = "savedValidationErrors")]
    fn saved_validation_errors<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
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
            host_to_js(&ctx, Value::List(list))
        })
    }

    #[qjs(get, rename = "errorParameters")]
    fn error_parameters<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            let map = crate::web::saved_for_page()
                .error_parameters
                .into_iter()
                .map(|(key, value)| (key, Value::string(value)))
                .collect();
            host_to_js(&ctx, Value::Map(map))
        })
    }

    /// The pages the user got in `html`, newest first, as `{name, url, urlNoParams}`.
    #[qjs(rename = "getScreenHistory")]
    fn get_screen_history<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
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
                    let mut map = indexmap::IndexMap::new();
                    map.insert("name".to_string(), Value::string(name));
                    map.insert("url".to_string(), Value::string(format!("{base}{path}")));
                    map.insert("urlNoParams".to_string(), Value::string(format!("{base}{bare}")));
                    map.insert("image".to_string(), Value::string(""));
                    map.insert("imageType".to_string(), Value::string(""));
                    Value::Map(map)
                })
                .collect();
            host_to_js(&ctx, Value::List(list))
        })
    }

    /// The host the request was for. `true` keeps the port.
    #[qjs(rename = "getHostName")]
    fn get_host_name(&self, ctx: Ctx<'_>, with_port: Opt<bool>) -> rquickjs::Result<String> {
        host_guard(&ctx, || Ok(crate::web::host_name(with_port.0.unwrap_or(false))))
    }

    /// A value that changes when the server starts, for the `?v=` on asset links so a browser refetches them.
    #[qjs(rename = "getResourceDistinctValue")]
    fn get_resource_distinct_value(&self, ctx: Ctx<'_>) -> rquickjs::Result<String> {
        host_guard(&ctx, || Ok(crate::web::resource_distinct_value()))
    }

    #[qjs(get, rename = "multiPartParameters")]
    fn multi_part_parameters<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || host_to_js(&ctx, map_value(crate::web::multipart_parameters())))
    }

    #[qjs(get, rename = "requestParameters")]
    fn request_parameters_property<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || host_to_js(&ctx, map_value(crate::web::parameters())))
    }

    /// The root URL of the web app, as Java's two forms: `(requireFullUrl, useEncryption)` for the `webroot` webapp, and
    /// `(webappName, servletContextPath, requireFullUrl, useEncryption)`; no arguments is `(false, null)`. See
    /// [`crate::web::webapp_root_url`].
    fn get_webapp_root_url(
        &self,
        ctx: Ctx<'_>,
        first: Opt<JsValue<'_>>,
        second: Opt<JsValue<'_>>,
        third: Opt<JsValue<'_>>,
        fourth: Opt<JsValue<'_>>,
    ) -> rquickjs::Result<String> {
        host_guard(&ctx, || {
            let text = |value: Option<&JsValue<'_>>| {
                value
                    .and_then(|value| value.as_string())
                    .and_then(|text| text.to_string().ok())
                    .filter(|text| !text.is_empty())
            };
            let flag = |value: Option<&JsValue<'_>>| value.and_then(JsValue::as_bool);
            let named = first.0.as_ref().is_some_and(JsValue::is_string);
            let (webapp, full, encryption) = if named {
                (text(first.0.as_ref()), flag(third.0.as_ref()), flag(fourth.0.as_ref()))
            } else {
                (None, flag(first.0.as_ref()), flag(second.0.as_ref()))
            };
            let webapp = webapp.unwrap_or_else(|| "webroot".to_string());
            Ok(crate::web::webapp_root_url(&webapp, full.unwrap_or(false), encryption))
        })
    }

    /// Reads and writes go to the session, with `remove` and `put` as on the Java map.
    #[qjs(get)]
    fn session_attributes<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || super::user_host::session_attributes_js(&ctx))
    }

    /// Rotate a frontend session after its authority verifies a delegated login.
    fn rotate_session(&self, ctx: Ctx<'_>) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            crate::user::session::rotate_on_password_login().map_err(|err| throw_context(&ctx, &err))
        })
    }

    /// Send this request's response with no session cookie or token (impl-commerce F86): see
    /// [`crate::user::session::decline_session`].
    fn decline_session(&self, ctx: Ctx<'_>) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            crate::user::session::decline_session();
            Ok(())
        })
    }

    #[qjs(get)]
    fn session_token(&self, ctx: Ctx<'_>) -> rquickjs::Result<String> {
        host_guard(&ctx, || {
            crate::user::csrf_token().map_err(|err| Exception::throw_message(&ctx, err.message()))
        })
    }
}

fn map_value(entries: std::collections::BTreeMap<String, Value>) -> Value {
    Value::Map(entries.into_iter().collect())
}

/// A value for a JSON response with its entity values and lists as plain maps and lists, at any depth, as Java's Jackson
/// writes them (`sendJsonResponse(enumList)` sends the rows, not "<object>").
fn json_plain(value: Value) -> Value {
    match value {
        Value::Object(_) => {
            if let Some(map) = super::values::value_map(&value) {
                return Value::Map(map.into_iter().map(|(key, item)| (key, json_plain(item))).collect());
            }
            match super::values::value_items(&value) {
                Some(items) => Value::List(items.into_iter().map(json_plain).collect()),
                None => value,
            }
        }
        Value::Map(entries) => Value::Map(entries.into_iter().map(|(key, item)| (key, json_plain(item))).collect()),
        Value::List(items) => Value::List(items.into_iter().map(json_plain).collect()),
        other => other,
    }
}
