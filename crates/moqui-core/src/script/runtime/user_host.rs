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

//! The rest of `ec.user`: preferences, groups, permissions, and login keys under their Java
//! names, and the helpers `UserServices.xml` calls where the Groovy reached into `ec.ecfi`
//! (hashes, the `user-facade` conf) or a Java library (TOTP, `SecureRandom`).
//!
//! Also `ec.web.sessionAttributes`, which writes through to the session.

use rquickjs::object::Accessor;

use super::*;

/// `getPreferences(keyRegexp)` filters in the script, so the key pattern is a JavaScript
/// regular expression matched against the whole key, as Java's `String.matches` does.
const GET_PREFERENCES: &str = r#"(function (user, all) {
    user.getPreferences = function (keyRegexp) {
        const prefs = all();
        if (keyRegexp == null || keyRegexp === "") return prefs;
        const pattern = new RegExp("^(?:" + keyRegexp + ")$");
        const out = {};
        for (const key of Object.keys(prefs)) if (pattern.test(key)) out[key] = prefs[key];
        return out;
    };
})"#;

/// A view of the session attributes. Reading a name gives its text; assigning, `put`, and
/// `remove` change the session. `ec.web.sessionAttributes.remove(name)` is the Java idiom the
/// screens use.
const SESSION_ATTRIBUTES: &str = r#"(function (snapshot, write) {
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
})"#;

/// `ec.user.context`: a map for the current execution context's user. Each read and write goes to the host, so the
/// entity filters see what `setup#UserOrganizationInfo` puts there. A list read from it is a copy: put it back to
/// change it.
const USER_CONTEXT: &str = r#"(function (read, write, names) {
    const methods = {
        get: (name) => read(String(name)),
        put: (name, value) => { const old = read(String(name)); write(String(name), value); return old; },
        putAll: (map) => { for (const [key, value] of Object.entries(map || {})) write(key, value); },
        remove: (name) => { const old = read(String(name)); write(String(name), undefined); return old; },
        containsKey: (name) => names().includes(String(name))
    };
    return new Proxy({}, {
        get(target, key) {
            if (typeof key !== "string") return undefined;
            if (Object.prototype.hasOwnProperty.call(methods, key)) return methods[key];
            const value = read(key);
            return value === null && !names().includes(key) ? undefined : value;
        },
        set(target, key, value) { write(String(key), value); return true; },
        deleteProperty(target, key) { write(String(key), undefined); return true; },
        has(target, key) { return typeof key === "string" && names().includes(key); },
        ownKeys() { return names(); },
        getOwnPropertyDescriptor(target, key) {
            return typeof key === "string" && names().includes(key)
                ? { value: read(key), writable: true, enumerable: true, configurable: true } : undefined;
        }
    });
})"#;

pub(super) fn install_user_extras<'js>(ctx: &Ctx<'js>, user: &Object<'js>) -> Result<(), Error> {
    macro_rules! method {
        ($name:literal, $function:expr) => {
            let function = caught(ctx, $name, Function::new(ctx.clone(), $function))?;
            caught(ctx, $name, user.set($name, function))?;
        };
    }
    method!("hasPermission", user_has_permission);
    method!("isInGroup", user_is_in_group);
    method!("getUserGroupIdSet", user_group_id_set);
    method!("setPreference", user_set_preference);
    method!("getClientIp", user_get_client_ip);
    method!("getLoginKey", user_get_login_key);
    method!("hashPassword", user_hash_password);
    method!("passwordsMatch", user_passwords_match);
    method!("singleUseCodeHash", user_single_use_code_hash);
    method!("randomCode", user_random_code);
    method!("randomString", user_random_string);
    method!("totpSecret", user_totp_secret);
    method!("totpCodeValid", user_totp_code_valid);
    method!("totpUri", user_totp_uri);
    method!("timestampMillis", user_timestamp_millis);
    let rules = Accessor::from(|ctx: Ctx<'js>| host_guard(&ctx, || login_rules(&ctx))).enumerable();
    caught(ctx, "loginRules", user.prop("loginRules", rules))?;
    let read = caught(ctx, "context", Function::new(ctx.clone(), user_context_read))?;
    let write = caught(ctx, "context", Function::new(ctx.clone(), user_context_write))?;
    let names = caught(ctx, "context", Function::new(ctx.clone(), user_context_names))?;
    let make: Function = eval_js(ctx, USER_CONTEXT)?;
    let context: JsValue = caught(ctx, "context", make.call((read, write, names)))?;
    caught(ctx, "context", user.set("context", context))?;
    let all = caught(ctx, "getPreferences", Function::new(ctx.clone(), user_all_preferences))?;
    let install: Function = eval_js(ctx, GET_PREFERENCES)?;
    caught(ctx, "getPreferences", install.call::<_, ()>((user.clone(), all)))
}

/// `ec.web.sessionAttributes` over the current session.
pub(super) fn session_attributes_js<'js>(ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
    let mut map = indexmap::IndexMap::new();
    for (key, value) in crate::user::session::session_attributes() {
        map.insert(key, Value::string(value));
    }
    let snapshot = host_to_js(ctx, Value::Map(map))?;
    let write = Function::new(ctx.clone(), write_session_attribute)?;
    let make: Function =
        eval_js(ctx, SESSION_ATTRIBUTES).map_err(|err| Exception::throw_message(ctx, err.message()))?;
    make.call((snapshot, write))
}

fn user_context_read<'js>(ctx: Ctx<'js>, name: String) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let value = with_ec(&ctx, |ec| ec.user_context().get(&name).cloned())?;
        host_to_js(&ctx, value.unwrap_or(Value::Null))
    })
}

/// Undefined removes the name; null keeps it with a null value, as a Java map put of null does.
fn user_context_write(ctx: Ctx<'_>, name: String, value: Opt<JsValue<'_>>) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        let value = match value.0 {
            None => None,
            Some(value) if value.is_undefined() => None,
            Some(value) => Some(js_to_host(&ctx, &value)?),
        };
        with_ec(&ctx, |ec| {
            let mut map = ec.user_context();
            match value {
                Some(value) => map.insert(name, value),
                None => map.remove(&name),
            };
        })
    })
}

fn user_context_names<'js>(ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let names: Vec<Value> = with_ec(&ctx, |ec| {
            ec.user_context().keys().cloned().map(Value::String).collect()
        })?;
        host_to_js(&ctx, Value::List(names))
    })
}

fn write_session_attribute(ctx: Ctx<'_>, name: String, value: Opt<JsValue<'_>>) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        let text = optional_text(&ctx, value.0.as_ref())?;
        crate::user::session::set_session_attribute(&name, text.as_deref());
        Ok(())
    })
}

fn user_error(ctx: &Ctx<'_>, err: &Error) -> JsError {
    Exception::throw_message(ctx, err.message())
}

fn user_has_permission(ctx: Ctx<'_>, permission: JsValue<'_>) -> rquickjs::Result<bool> {
    host_guard(&ctx, || {
        let permission = text_or_empty(&ctx, Some(&permission))?;
        crate::user::has_permission(&permission).map_err(|err| user_error(&ctx, &err))
    })
}

fn user_is_in_group(ctx: Ctx<'_>, group: JsValue<'_>) -> rquickjs::Result<bool> {
    host_guard(&ctx, || {
        let group = text_or_empty(&ctx, Some(&group))?;
        crate::user::is_in_group(&group).map_err(|err| user_error(&ctx, &err))
    })
}

/// The group ids as a list, so the result works as the `from` of an `in` condition.
fn user_group_id_set<'js>(ctx: Ctx<'js>, user_id: Opt<JsValue<'js>>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let user_id = match user_id.0.as_ref() {
            Some(_) => optional_text(&ctx, user_id.0.as_ref())?,
            None => crate::user::current_user_id().map_err(|err| user_error(&ctx, &err))?,
        };
        let groups = crate::user::user_group_ids(user_id.as_deref()).map_err(|err| user_error(&ctx, &err))?;
        host_to_js(&ctx, Value::List(groups.into_iter().map(Value::String).collect()))
    })
}

fn user_set_preference(ctx: Ctx<'_>, key: JsValue<'_>, value: Opt<JsValue<'_>>) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        let key = text_or_empty(&ctx, Some(&key))?;
        let value = optional_text(&ctx, value.0.as_ref())?;
        crate::user::set_preference(&key, value.as_deref()).map_err(|err| user_error(&ctx, &err))
    })
}

fn user_all_preferences<'js>(ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let prefs = crate::user::get_preferences().map_err(|err| user_error(&ctx, &err))?;
        host_to_js(
            &ctx,
            Value::Map(
                prefs
                    .into_iter()
                    .map(|(key, value)| (key, Value::String(value)))
                    .collect(),
            ),
        )
    })
}

/// Java `UserFacade.getClientIp`: the request's client address, empty when there is no request.
fn user_get_client_ip(ctx: Ctx<'_>) -> rquickjs::Result<String> {
    host_guard(&ctx, || Ok(crate::web::current_client_ip()))
}

fn user_get_login_key(ctx: Ctx<'_>, hours: Opt<f64>) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        crate::user::get_login_key(hours.0).map_err(|err| user_error(&ctx, &err))
    })
}

/// The `user-facade` attributes the user services read from `ec.ecfi.confXmlRoot` in Java.
fn login_rules<'js>(ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
    let crate::user::LoginRules {
        max_failures,
        disable_minutes,
        history_limit,
        email_expire_hours,
        email_require_change,
    } = crate::user::policy_login_rules();
    let mut map = indexmap::IndexMap::new();
    map.insert("maxFailures".to_string(), Value::I64(max_failures));
    map.insert("disableMinutes".to_string(), Value::I64(disable_minutes));
    map.insert("historyLimit".to_string(), Value::I64(history_limit));
    map.insert("emailExpireHours".to_string(), Value::I64(email_expire_hours));
    map.insert("emailRequireChange".to_string(), Value::Bool(email_require_change));
    host_to_js(ctx, Value::Map(map))
}

/// A new password hash. Always argon2; the account's `passwordHashType` becomes `argon2`.
fn user_hash_password(ctx: Ctx<'_>, plain: String) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        crate::user::hash_password(&plain).map_err(|err| user_error(&ctx, &err))
    })
}

/// `getCredentialsMatcher(hashType, base64).doCredentialsMatch`: `plain` against a stored hash,
/// argon2 or a legacy digest. An empty stored hash never matches.
fn user_passwords_match(
    ctx: Ctx<'_>,
    plain: JsValue<'_>,
    stored: JsValue<'_>,
    hash_type: Opt<JsValue<'_>>,
    salt: Opt<JsValue<'_>>,
    base64: Opt<JsValue<'_>>,
) -> rquickjs::Result<bool> {
    host_guard(&ctx, || {
        let plain = text_or_empty(&ctx, Some(&plain))?;
        let stored = text_or_empty(&ctx, Some(&stored))?;
        let hash_type = text_or_empty(&ctx, hash_type.0.as_ref())?;
        let salt = text_or_empty(&ctx, salt.0.as_ref())?;
        let base64 = base64.0.as_ref().is_some_and(js_truthy);
        if plain.is_empty() {
            return Ok(false);
        }
        crate::user::stored_password_matches(&plain, &stored, &hash_type, &salt, base64)
            .map_err(|err| user_error(&ctx, &err))
    })
}

fn user_single_use_code_hash(ctx: Ctx<'_>, code: JsValue<'_>) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        let code = text_or_empty(&ctx, Some(&code))?;
        crate::user::single_use_hash(&code).map_err(|err| user_error(&ctx, &err))
    })
}

fn user_random_code(ctx: Ctx<'_>, width: Opt<i32>) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        let width = usize::try_from(width.0.unwrap_or(6)).unwrap_or(6);
        crate::user::random_code(width).map_err(|err| user_error(&ctx, &err))
    })
}

fn user_random_string(ctx: Ctx<'_>, length: i32) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        let length = usize::try_from(length).unwrap_or(0);
        crate::user::random_string(length).map_err(|err| user_error(&ctx, &err))
    })
}

fn user_totp_secret(ctx: Ctx<'_>) -> rquickjs::Result<String> {
    host_guard(&ctx, || Ok(crate::user::totp_secret()))
}

fn user_totp_code_valid(ctx: Ctx<'_>, secret: JsValue<'_>, code: JsValue<'_>) -> rquickjs::Result<bool> {
    host_guard(&ctx, || {
        let secret = text_or_empty(&ctx, Some(&secret))?;
        let code = text_or_empty(&ctx, Some(&code))?;
        Ok(crate::user::totp_code_valid(&secret, &code))
    })
}

fn user_totp_uri<'js>(
    ctx: Ctx<'js>,
    label: JsValue<'js>,
    issuer: JsValue<'js>,
    secret: JsValue<'js>,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let label = text_or_empty(&ctx, Some(&label))?;
        let issuer = text_or_empty(&ctx, Some(&issuer))?;
        let secret = text_or_empty(&ctx, Some(&secret))?;
        match crate::user::totp_uri(&label, &issuer, &secret) {
            Some(uri) => uri.into_js(&ctx),
            None => Null.into_js(&ctx),
        }
    })
}

/// Epoch milliseconds of a time stamp, which a script holds as text. Null when it is not one.
fn user_timestamp_millis<'js>(ctx: Ctx<'js>, value: JsValue<'js>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let host = js_to_host(&ctx, &value)?;
        if moqui_lib::object::is_empty(&host) {
            return Null.into_js(&ctx);
        }
        match crate::l10n::basic_convert(&host, "java.sql.Timestamp").value {
            #[allow(clippy::cast_precision_loss)] // Milliseconds of real dates fit a double exactly.
            Value::Timestamp(stamp) => (stamp.as_millisecond() as f64).into_js(&ctx),
            _ => Null.into_js(&ctx),
        }
    })
}
