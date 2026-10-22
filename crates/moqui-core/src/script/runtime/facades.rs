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

//! Smaller `ec` facades: cache, message, logger, resource, user, and web.

use super::*;
use moqui_lib::object::Value;

/// A named framework cache. A name the conf does not define is made with the default settings on first use, as Java.
#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub(super) struct HostCache {
    #[qjs(skip_trace)]
    pub(super) name: String,
}

pub(super) fn cache_get_cache<'js>(ctx: Ctx<'js>, name: String) -> rquickjs::Result<Class<'js, HostCache>> {
    host_guard(&ctx, || Class::instance(ctx.clone(), HostCache { name }))
}

impl HostCache {
    fn with_cache<T>(
        &self,
        ctx: &Ctx<'_>,
        body: impl FnOnce(&moqui_lib::cache::MCache<String, Value>) -> Result<T, moqui_lib::error::Error>,
    ) -> rquickjs::Result<T> {
        // A name with no conf entry is made with the default settings on first use, as Java's `getCache`.
        let found = with_ec(ctx, |ec| ec.factory().cache().get_or_create(&self.name))?;
        match found {
            Ok(cache) => body(&cache).map_err(|err| throw_lib(ctx, &err)),
            Err(err) => Err(Exception::throw_message(ctx, err.message())),
        }
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl HostCache {
    fn get<'js>(&self, ctx: Ctx<'js>, key: String) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            let found = self.with_cache(&ctx, |cache| cache.get(&key))?;
            match found {
                Some(value) => host_to_js(&ctx, value),
                None => Null.into_js(&ctx),
            }
        })
    }

    fn put(&self, ctx: Ctx<'_>, key: String, value: JsValue<'_>) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            let value = js_to_host(&ctx, &value)?;
            self.with_cache(&ctx, |cache| cache.put(key, value))
        })
    }

    fn remove(&self, ctx: Ctx<'_>, key: String) -> rquickjs::Result<bool> {
        host_guard(&ctx, || self.with_cache(&ctx, |cache| cache.remove(&key)))
    }

    fn contains_key(&self, ctx: Ctx<'_>, key: String) -> rquickjs::Result<bool> {
        host_guard(&ctx, || self.with_cache(&ctx, |cache| cache.contains_key(&key)))
    }

    /// Clear the cache. A name that is not a conf cache may be the screen definitions or an entity list cache.
    fn clear(&self, ctx: Ctx<'_>) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            let cleared = with_ec(&ctx, |ec| ec.factory().cache().clear(&self.name))?;
            if let Err(err) = cleared {
                return Err(Exception::throw_message(&ctx, err.message()));
            }
            // Also the screen or entity list cache of that name: a `get` of it made an empty conf cache, which the
            // line above cleared.
            super::tools_host::clear_other_cache(&self.name)
                .map(|_| ())
                .map_err(|err| throw_lib(&ctx, &err))
        })
    }
}

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
        let source = read_script(&location).map_err(|err| throw_context(&ctx, &err))?;
        let script = super::ScriptText::named(source, location.as_str());
        eval_compiled_text::<JsValue>(&ctx, &script, false).map_err(|err| throw_context(&ctx, &err))
    })
}

/// A script file's text by location, as the function [`crate::script::parse::file_function`] makes of it, with the
/// modified time it had and when that was last checked.
struct ScriptText {
    path: std::path::PathBuf,
    modified: Option<std::time::SystemTime>,
    checked: Instant,
    text: Arc<str>,
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

/// A script file to run, kept by location as Java's `resource.script*.location` caches and read again when the file
/// changes: its text as the body of a function called at once (see [`crate::script::parse::file_function`]), so it runs
/// in names of its own and evaluates to what it returns. The compiled form is kept by the runtime by text.
pub(super) fn read_script(location: &str) -> Result<Arc<str>, Error> {
    let production = crate::web::is_production();
    {
        let mut texts = script_texts().lock().unwrap_or_else(|err| err.into_inner());
        if let Some(found) = texts.get_mut(location) {
            if production || found.checked.elapsed() < SCRIPT_CHECK {
                return Ok(Arc::clone(&found.text));
            }
            if modified_of(&found.path) == found.modified {
                found.checked = Instant::now();
                return Ok(Arc::clone(&found.text));
            }
        }
    }
    let path = crate::context::locate(location)?;
    let modified = modified_of(&path);
    let text = std::fs::read_to_string(&path)
        .map_err(|err| Error::new(format!("could not read script {location}")).with_source(err))?;
    let text: Arc<str> = crate::script::parse::file_function(&text).into();
    script_texts().lock().unwrap_or_else(|err| err.into_inner()).insert(
        location.to_string(),
        ScriptText {
            path,
            modified,
            checked: Instant::now(),
            text: Arc::clone(&text),
        },
    );
    Ok(text)
}

/// Run a script file as a service body does: in the current context frame, with bare names.
///
/// # Errors
///
/// Returns an error when the file cannot be read or the script throws.
pub fn run_script_location(location: &str) -> Result<(), Error> {
    let source = read_script(location)?;
    eval_script_text(&super::ScriptText::named(source, location))
}

/// `ec.logger.log(level, message, thrown)`: the level is a name or one of Java's `LoggerFacade` numbers, `ERROR_INT`
/// 200 and `WARN_INT` 300 among them. `OFF_INT` 0 and `"off"` log nothing.
pub(super) fn logger_log(
    ctx: Ctx<'_>,
    level: JsValue<'_>,
    message: String,
    thrown: Opt<JsValue<'_>>,
) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        let message = with_thrown(message, thrown.0.as_ref());
        let level = match level.as_number() {
            Some(0.0) => "off".to_string(),
            Some(number) if number <= 300.0 || number >= f64::from(i32::MAX) => "warn".to_string(),
            Some(_) => "info".to_string(),
            None => level
                .as_string()
                .map(|text| text.to_string())
                .transpose()?
                .unwrap_or_default(),
        };
        match level.as_str() {
            "off" => {}
            "error" => tracing::error!("{message}"),
            "warn" | "warning" => tracing::warn!("{message}"),
            "debug" => tracing::debug!("{message}"),
            "trace" => tracing::trace!("{message}"),
            _ => tracing::info!("{message}"),
        }
        Ok(())
    })
}

pub(super) fn logger_warn(ctx: Ctx<'_>, message: String, thrown: Opt<JsValue<'_>>) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        let message = with_thrown(message, thrown.0.as_ref());
        tracing::warn!("{message}");
        Ok(())
    })
}

/// The message with the exception a script passed as the last argument of a log call, as `message: name: text`.
fn with_thrown(message: String, thrown: Option<&JsValue<'_>>) -> String {
    let Some(thrown) = thrown.filter(|value| !value.is_null() && !value.is_undefined()) else {
        return message;
    };
    let text = match thrown.as_object() {
        Some(object) => {
            let name: Option<String> = object.get("name").ok();
            let detail: Option<String> = object.get("message").ok();
            match (name, detail) {
                (Some(name), Some(detail)) => format!("{name}: {detail}"),
                (_, Some(detail)) => detail,
                _ => String::from("(exception)"),
            }
        }
        None => thrown
            .as_string()
            .and_then(|text| text.to_string().ok())
            .unwrap_or_default(),
    };
    format!("{message}: {text}")
}

/// `ec.logger.info(message, thrown)`, and `error`, `debug`, `trace`, `warn`: Java `LoggerFacade` by level.
pub(super) fn logger_info(ctx: Ctx<'_>, message: String, thrown: Opt<JsValue<'_>>) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        let message = with_thrown(message, thrown.0.as_ref());
        tracing::info!("{message}");
        Ok(())
    })
}

pub(super) fn logger_error(ctx: Ctx<'_>, message: String, thrown: Opt<JsValue<'_>>) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        let message = with_thrown(message, thrown.0.as_ref());
        tracing::error!("{message}");
        Ok(())
    })
}

pub(super) fn logger_debug(ctx: Ctx<'_>, message: String, thrown: Opt<JsValue<'_>>) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        let message = with_thrown(message, thrown.0.as_ref());
        tracing::debug!("{message}");
        Ok(())
    })
}

pub(super) fn logger_trace(ctx: Ctx<'_>, message: String, thrown: Opt<JsValue<'_>>) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        let message = with_thrown(message, thrown.0.as_ref());
        tracing::trace!("{message}");
        Ok(())
    })
}

/// Message facade for the entered execution context. There is no separate public-message list.
#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub(super) struct HostMessage {}

#[rquickjs::methods(rename_all = "camelCase")]
impl HostMessage {
    fn add_error(&self, ctx: Ctx<'_>, message: String) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            with_ec(&ctx, |ec| ec.message().add_error(message))?;
            Ok(())
        })
    }

    /// `addValidationError(form, field, serviceName, message, throwable)`.
    fn add_validation_error(
        &self,
        ctx: Ctx<'_>,
        form: Opt<JsValue<'_>>,
        field: Opt<JsValue<'_>>,
        service: Opt<JsValue<'_>>,
        message: Opt<JsValue<'_>>,
    ) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            let text = |value: Opt<JsValue<'_>>| text_or_empty(&ctx, value.0.as_ref());
            let (form, field, service, message) = (text(form)?, text(field)?, text(service)?, text(message)?);
            with_ec(&ctx, |ec| {
                ec.message().add_validation_error(&form, &field, &service, &message)
            })?;
            Ok(())
        })
    }

    /// `addMessage(message, type)`: `info`, `success`, `warning` or `danger`.
    fn add_message(&self, ctx: Ctx<'_>, message: String, kind: Opt<String>) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            let kind = kind.0.unwrap_or_default();
            with_ec(&ctx, |ec| ec.message().add_message_typed(message, &kind))?;
            Ok(())
        })
    }

    fn add_public(&self, ctx: Ctx<'_>, message: String, kind: Opt<String>) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            let kind = kind.0.unwrap_or_default();
            with_ec(&ctx, |ec| ec.message().add_message_typed(message, &kind))?;
            Ok(())
        })
    }

    fn has_error(&self, ctx: Ctx<'_>) -> rquickjs::Result<bool> {
        host_guard(&ctx, || with_ec(&ctx, |ec| !ec.message().errors().is_empty()))
    }

    fn clear_errors(&self, ctx: Ctx<'_>) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            with_ec(&ctx, |ec| ec.message().clear_errors())?;
            Ok(())
        })
    }

    fn push_errors(&self, ctx: Ctx<'_>) -> rquickjs::Result<()> {
        host_guard(&ctx, || with_ec(&ctx, |ec| ec.message().push_errors()))
    }

    fn pop_errors(&self, ctx: Ctx<'_>) -> rquickjs::Result<()> {
        host_guard(&ctx, || with_ec(&ctx, |ec| ec.message().pop_errors()))
    }

    #[qjs(get, rename = "errorsString")]
    fn errors_string_property(&self, ctx: Ctx<'_>) -> rquickjs::Result<String> {
        self.get_errors_string(ctx)
    }

    fn get_errors_string(&self, ctx: Ctx<'_>) -> rquickjs::Result<String> {
        host_guard(&ctx, || with_ec(&ctx, |ec| ec.message().errors_string()))
    }

    #[qjs(get)]
    fn errors<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            let count = with_ec(&ctx, |ec| ec.message().errors().len())?;
            if count == 0 {
                return Null.into_js(&ctx);
            }
            let count = i32::try_from(count).map_err(|_| Exception::throw_message(&ctx, "error count is too large"))?;
            count.into_js(&ctx)
        })
    }
}

/// `ec.user`. `loginUser` records a failure and returns false instead of throwing.
#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub(super) struct HostUser {}

#[rquickjs::methods(rename_all = "camelCase")]
impl HostUser {
    /// The password policy of `user-facade/password`: `{minLength, minDigits, minOthers}`.
    #[qjs(get, rename = "passwordRules")]
    fn password_rules<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            let (length, digits, others) = crate::user::policy_password_rules();
            let mut map = indexmap::IndexMap::new();
            map.insert("minLength".to_string(), Value::I64(length));
            map.insert("minDigits".to_string(), Value::I64(digits));
            map.insert("minOthers".to_string(), Value::I64(others));
            host_to_js(&ctx, Value::Map(map))
        })
    }

    /// False with no error when the password was right and a second factor is still needed.
    /// A null or missing username or password is an empty one, a failed login as Java's: false and the message.
    fn login_user(
        &self,
        ctx: Ctx<'_>,
        username: Opt<Option<String>>,
        password: Opt<Option<String>>,
    ) -> rquickjs::Result<bool> {
        let username = username.0.flatten().unwrap_or_default();
        let password = password.0.flatten().unwrap_or_default();
        host_guard(&ctx, || match crate::user::login_user(&username, &password) {
            Ok(()) => Ok(!crate::user::session::session_attribute_flag(
                crate::user::AUTHC_FACTOR_REQUIRED,
            )),
            Err(err) => {
                with_ec(&ctx, |ec| ec.message().add_error(err.message().to_string()))?;
                Ok(false)
            }
        })
    }

    fn logout_user(&self, ctx: Ctx<'_>) -> rquickjs::Result<bool> {
        host_guard(&ctx, || match crate::user::logout() {
            Ok(()) => Ok(true),
            Err(err) => {
                with_ec(&ctx, |ec| ec.message().add_error(err.message().to_string()))?;
                Ok(false)
            }
        })
    }

    fn internal_login_user(&self, ctx: Ctx<'_>, username: String) -> rquickjs::Result<bool> {
        host_guard(&ctx, || match crate::user::internal_login_user(&username) {
            Ok(()) => Ok(true),
            Err(err) => {
                with_ec(&ctx, |ec| ec.message().add_error(err.message().to_string()))?;
                Ok(false)
            }
        })
    }

    /// The logged-in user's preference: a property, the user's `UserPreference`, or a group's, or null.
    fn get_preference<'js>(&self, ctx: Ctx<'js>, key: String) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || match user_preference(&key) {
            Ok(Some(value)) => value.into_js(&ctx),
            Ok(None) => Null.into_js(&ctx),
            Err(err) => Err(Exception::throw_message(&ctx, err.message())),
        })
    }

    #[qjs(get, rename = "locale")]
    fn locale_property(&self, ctx: Ctx<'_>) -> rquickjs::Result<String> {
        host_guard(&ctx, || Ok(crate::l10n::locale()))
    }

    fn get_locale(&self, ctx: Ctx<'_>) -> rquickjs::Result<String> {
        host_guard(&ctx, || Ok(crate::l10n::locale()))
    }

    /// The time zone of the session, the user's own when there is no other, else the server's (Java's
    /// `ec.user.getTimeZone()`).
    #[qjs(get, rename = "timeZone")]
    fn time_zone_property(&self, ctx: Ctx<'_>) -> rquickjs::Result<String> {
        host_guard(&ctx, || Ok(session_time_zone_name()))
    }

    /// The machine's time zone, the zone a ServiceJob cron is read in. A digest whose account has no
    /// `UserAccount.timeZone` sets the session to this so the hour is the machine's. Not the server zone.
    #[qjs(get, rename = "machineTimeZone")]
    fn machine_time_zone(&self, ctx: Ctx<'_>) -> rquickjs::Result<String> {
        host_guard(&ctx, || {
            Ok(crate::service::job_time_zone().iana_name().unwrap_or("UTC").to_string())
        })
    }

    fn get_time_zone(&self, ctx: Ctx<'_>) -> rquickjs::Result<String> {
        host_guard(&ctx, || Ok(session_time_zone_name()))
    }

    /// `setTimeZone(name)`, Java's `UserFacade.setTimeZone`: the session's time zone from now on, also saved on the
    /// signed-in user's `UserAccount`; with nobody signed in (a job) the session's only. An unknown zone is an error;
    /// null or empty clears it.
    fn set_time_zone(&self, ctx: Ctx<'_>, zone: Opt<JsValue<'_>>) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            let zone = optional_text(&ctx, zone.0.as_ref())?.filter(|zone| !zone.trim().is_empty());
            if let Some(zone) = zone.as_deref() {
                if jiff::tz::TimeZone::get(zone).is_err() {
                    return Err(Exception::throw_message(
                        &ctx,
                        &format!("Time zone {zone} is not known"),
                    ));
                }
            }
            save_user_setting("timeZone", zone.as_deref())
                .map_err(|err| Exception::throw_message(&ctx, err.message()))?;
            crate::user::session::set_time_zone(zone.as_deref());
            Ok(())
        })
    }

    /// `setLocale(name)`, Java's `UserFacade.setLocale`: as [`Self::set_time_zone`], for the locale (`en_US`, `de`).
    fn set_locale(&self, ctx: Ctx<'_>, locale: Opt<JsValue<'_>>) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            let locale = optional_text(&ctx, locale.0.as_ref())?
                .map(|locale| locale.trim().replace('-', "_"))
                .filter(|locale| !locale.is_empty());
            save_user_setting("locale", locale.as_deref())
                .map_err(|err| Exception::throw_message(&ctx, err.message()))?;
            crate::user::session::set_locale(locale.as_deref());
            Ok(())
        })
    }

    /// The logged-in user's `UserAccount` without the password and lockout columns, or null.
    #[qjs(get, rename = "userAccount")]
    fn user_account_property<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || match user_account_snapshot() {
            Ok(Some(map)) => host_to_js(&ctx, Value::Map(map)),
            Ok(None) => Null.into_js(&ctx),
            Err(err) => Err(Exception::throw_message(&ctx, err.message())),
        })
    }

    /// The current time. A script sees time stamps as text, so this is the text of the stamp.
    #[qjs(get, rename = "nowTimestamp")]
    fn now_timestamp_property<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || host_to_js(&ctx, now_timestamp()))
    }

    /// `setEffectiveTime(time)`: `nowTimestamp`, date filters with no date and the defaults Java takes from it use
    /// `time` (epoch millis, a `Date` or time stamp text) for this execution context. Null goes back to the clock.
    fn set_effective_time<'js>(&self, ctx: Ctx<'js>, time: Opt<JsValue<'js>>) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            let value = match time.0.as_ref() {
                Some(value) => js_to_host(&ctx, value)?,
                None => Value::Null,
            };
            let millis = if moqui_lib::object::is_empty(&value) {
                None
            } else {
                let stamp = timestamp_of(&value);
                let millis = crate::user::value_millis(&stamp).ok_or_else(|| {
                    Exception::throw_message(&ctx, &format!("setEffectiveTime: {value:?} is not a time"))
                })?;
                Some(millis)
            };
            with_ec(&ctx, |ec| ec.set_effective_time(millis))
        })
    }

    /// The `moqui.server.Visit` id of this session, or null (Java's `getVisitId`).
    #[qjs(get, rename = "visitId")]
    fn visit_id_property<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || match crate::user::visit_id() {
            Ok(Some(id)) => id.into_js(&ctx),
            Ok(None) => Null.into_js(&ctx),
            Err(err) => Err(Exception::throw_message(&ctx, err.message())),
        })
    }

    fn get_visit_id<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        self.visit_id_property(ctx)
    }

    /// The `moqui.server.Visit` of this session as a map, or null (Java's `getVisit`).
    #[qjs(get, rename = "visit")]
    fn visit_property<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            let found = (|| -> Result<Option<Value>, Error> {
                let Some(visit_id) = crate::user::visit_id()? else {
                    return Ok(None);
                };
                let Some(facade) = ExecutionContext::with_current(|ec| ec.factory().entity()).flatten() else {
                    return Ok(None);
                };
                let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
                let row = facade
                    .find("moqui.server.Visit")?
                    .eq("visitId", Value::string(visit_id))
                    .one()?;
                Ok(row.map(|row| {
                    Value::Map(
                        row.fields()
                            .iter()
                            .map(|(key, value)| (key.to_string(), value.clone()))
                            .collect(),
                    )
                }))
            })();
            match found {
                Ok(Some(map)) => host_to_js(&ctx, map),
                Ok(None) => Null.into_js(&ctx),
                Err(err) => Err(Exception::throw_message(&ctx, err.message())),
            }
        })
    }

    fn get_visit<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        self.visit_property(ctx)
    }

    /// `endVisit(visitId)`: end another session of the signed-in user (`org.moqui.impl.UserServices.end#UserVisit`);
    /// the number of web sessions ended. An error for the current visit or one that is not the user's.
    fn end_visit(&self, ctx: Ctx<'_>, visit_id: String) -> rquickjs::Result<i64> {
        host_guard(&ctx, || {
            crate::user::end_user_visit(&visit_id)
                .map(|count| i64::try_from(count).unwrap_or(i64::MAX))
                .map_err(|err| Exception::throw_message(&ctx, err.message()))
        })
    }

    #[qjs(get)]
    fn username<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || match crate::user::current_username() {
            Ok(Some(name)) => name.into_js(&ctx),
            Ok(None) => Null.into_js(&ctx),
            Err(err) => Err(Exception::throw_message(&ctx, err.message())),
        })
    }

    #[qjs(get)]
    fn user_id<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || match crate::user::current_user_id() {
            Ok(Some(name)) => name.into_js(&ctx),
            Ok(None) => Null.into_js(&ctx),
            Err(err) => Err(Exception::throw_message(&ctx, err.message())),
        })
    }
}

/// `field` of the signed-in user's `UserAccount`, as Java's `setLocale` and `setTimeZone` save it. Nothing with nobody
/// signed in.
fn save_user_setting(field: &str, value: Option<&str>) -> Result<(), Error> {
    let Some(user_id) = crate::user::current_user_id()?.filter(|id| !id.is_empty()) else {
        return Ok(());
    };
    let facade = ExecutionContext::with_current(|ec| ec.factory().entity())
        .flatten()
        .ok_or_else(|| Error::new("no entity facade is installed"))?;
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut account = facade.make_value("moqui.security.UserAccount")?;
    account.set("userId", Value::string(user_id));
    account.set(field, value.map_or(Value::Null, Value::string));
    account.update(&facade)?;
    Ok(())
}

fn session_time_zone_name() -> String {
    crate::user::time_zone()
        .ok()
        .flatten()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(crate::l10n::server_time_zone_name)
}

/// The columns of `UserAccount` that never leave the server.
const USER_ACCOUNT_PRIVATE: [&str; 8] = [
    "currentPassword",
    "passwordSalt",
    "passwordHashType",
    "passwordSetDate",
    "passwordHint",
    "successiveFailedLogins",
    "disabledDateTime",
    "requirePasswordChange",
];

fn user_account_snapshot() -> Result<Option<indexmap::IndexMap<String, Value>>, crate::context::Error> {
    let Some(user_id) = crate::user::current_user_id()? else {
        return Ok(None);
    };
    let Some(facade) = crate::context::ExecutionContext::with_current(|ec| ec.factory().entity()).flatten() else {
        return Ok(None);
    };
    let _authz = crate::context::ExecutionContext::with_current(|ec| ec.disable_authz());
    let found = facade
        .find("moqui.security.UserAccount")
        .map_err(crate::context::Error::from)?
        .eq("userId", Value::string(&user_id))
        .one()
        .map_err(crate::context::Error::from)?;
    Ok(found.map(|row| {
        row.fields()
            .iter()
            .filter(|(key, _)| !USER_ACCOUNT_PRIVATE.contains(&&***key))
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect()
    }))
}

fn user_preference(key: &str) -> Result<Option<String>, crate::context::Error> {
    if crate::context::ExecutionContext::with_current(|ec| ec.factory().entity())
        .flatten()
        .is_none()
    {
        return Ok(None);
    }
    crate::user::get_preference(key)
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
            crate::web::send_json_response(js_to_host(&ctx, &value)?);
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

    /// Messages and errors for the page: what the last response saved, then what this request added.
    /// Each is `{message, typeString}` with `info` or `danger`.
    #[qjs(rename = "getPageMessages")]
    fn get_page_messages<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            let saved = crate::web::saved_for_page();
            let (messages, errors) = with_ec(&ctx, |ec| {
                let facade = ec.message();
                let messages: Vec<(String, String)> = facade
                    .messages()
                    .iter()
                    .enumerate()
                    .map(|(index, text)| (text.clone(), facade.message_type(index).to_string()))
                    .collect();
                (messages, facade.errors().to_vec())
            })?;
            let entry = |text: &str, kind: &str| {
                let mut map = indexmap::IndexMap::new();
                map.insert("message".to_string(), Value::string(text));
                map.insert("typeString".to_string(), Value::string(kind));
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
            list.extend(messages.iter().map(|(text, kind)| entry(text, kind)));
            list.extend(errors.iter().map(|text| entry(text, "danger")));
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
            let text = |value: &Option<JsValue<'_>>| {
                value
                    .as_ref()
                    .and_then(|value| value.as_string())
                    .and_then(|text| text.to_string().ok())
                    .filter(|text| !text.is_empty())
            };
            let flag = |value: &Option<JsValue<'_>>| value.as_ref().and_then(JsValue::as_bool);
            let named = first.0.as_ref().is_some_and(JsValue::is_string);
            let (webapp, full, encryption) = if named {
                (text(&first.0), flag(&third.0), flag(&fourth.0))
            } else {
                (None, flag(&first.0), flag(&second.0))
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
