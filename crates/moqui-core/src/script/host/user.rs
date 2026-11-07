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

//! `ec.user`: the session's user, locale and time zone, and the rest under their Java names: preferences, groups,
//! permissions and login keys, and the helpers `UserServices.xml` calls where the Groovy reached into `ec.ecfi` (hashes,
//! the `user-facade` conf) or a Java library (TOTP, `SecureRandom`). `loginUser` records a failure and returns false
//! instead of throwing.

use moqui_lib::object::Value;

use super::{method, truthy, with_ec, Call, HostClass, HostError, Ret};
use crate::context::{Error, ExecutionContext};

pub(super) static USER: HostClass = HostClass {
    name: "host:User",
    methods: &[
        method!("ec.user", "loginUser", [OptNullableText, OptNullableText], login_user),
        method!("ec.user", "logoutUser", [], logout_user),
        method!("ec.user", "internalLoginUser", [Text], internal_login_user),
        method!("ec.user", "getPreference", [Text], get_preference),
        method!("ec.user", "getLocale", [], locale),
        method!("ec.user", "getTimeZone", [], time_zone),
        method!("ec.user", "setTimeZone", [OptValue], set_time_zone),
        method!("ec.user", "setLocale", [OptValue], set_locale),
        method!("ec.user", "setEffectiveTime", [OptValue], set_effective_time),
        method!("ec.user", "getVisitId", [], visit_id),
        method!("ec.user", "getVisit", [], visit),
        method!("ec.user", "endVisit", [Text], end_visit),
        method!("ec.user", "hasPermission", [Value], has_permission),
        method!("ec.user", "isInGroup", [Value], is_in_group),
        method!("ec.user", "getUserGroupIdSet", [OptValue], group_id_set),
        method!("ec.user", "setPreference", [Value, OptValue], set_preference),
        method!("ec.user", "getClientIp", [], client_ip),
        method!("ec.user", "getLoginKey", [OptNumber], login_key),
        method!("ec.user", "hashPassword", [Text], hash_password),
        method!(
            "ec.user",
            "passwordsMatch",
            [Value, Value, OptValue, OptValue, OptValue],
            passwords_match
        ),
        method!("ec.user", "singleUseCodeHash", [Value], single_use_code_hash),
        method!("ec.user", "randomCode", [OptInt], random_code),
        method!("ec.user", "randomString", [Int], random_string),
        method!("ec.user", "totpSecret", [], totp_secret),
        method!("ec.user", "totpCodeValid", [Value, Value], totp_code_valid),
        method!("ec.user", "totpUri", [Value, Value, Value], totp_uri),
        method!("ec.user", "timestampMillis", [Value], timestamp_millis),
    ],
    getters: &[
        method!("ec.user", "passwordRules", [], password_rules),
        method!("ec.user", "locale", [], locale),
        method!("ec.user", "timeZone", [], time_zone),
        method!("ec.user", "machineTimeZone", [], machine_time_zone),
        method!("ec.user", "userAccount", [], user_account),
        method!("ec.user", "nowTimestamp", [], now_timestamp),
        method!("ec.user", "visitId", [], visit_id),
        method!("ec.user", "visit", [], visit),
        method!("ec.user", "username", [], username),
        method!("ec.user", "userId", [], user_id),
        method!("ec.user", "loginRules", [], login_rules),
    ],
    hidden: &[
        method!("ec.user.context", "read", [Text], context_read),
        method!("ec.user.context", "write", [Text, OptValue], context_write),
        method!("ec.user.context", "names", [], context_names),
        method!("ec.user", "getPreferences", [], all_preferences),
    ],
    shim_js: Some(USER_JS),
    shim_members: &["getPreferences", "context"],
};

/// `getPreferences(keyRegexp)` filters in the script, so the key pattern is a JavaScript regular expression matched
/// against the whole key, as Java's `String.matches` does.
///
/// `ec.user.context`: a map for the current execution context's user. Each read and write goes to the host, so the
/// entity filters see what `setup#UserOrganizationInfo` puts there. A list read from it is a copy: put it back to
/// change it.
const USER_JS: &str = r#"(function (proto, hidden) {
    const all = hidden.getPreferences;
    proto.getPreferences = function (keyRegexp) {
        const prefs = all();
        if (keyRegexp == null || keyRegexp === "") return prefs;
        const pattern = new RegExp("^(?:" + keyRegexp + ")$");
        const out = {};
        for (const key of Object.keys(prefs)) if (pattern.test(key)) out[key] = prefs[key];
        return out;
    };
    const read = hidden.read, write = hidden.write, names = hidden.names;
    const methods = {
        get: (name) => read(String(name)),
        put: (name, value) => { const old = read(String(name)); write(String(name), value); return old; },
        putAll: (map) => { for (const [key, value] of Object.entries(map || {})) write(key, value); },
        remove: (name) => { const old = read(String(name)); write(String(name), undefined); return old; },
        containsKey: (name) => names().includes(String(name))
    };
    proto.context = new Proxy({}, {
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

fn user_error(err: &Error) -> HostError {
    HostError::message(err.message())
}

fn text_or_null(found: Result<Option<String>, Error>) -> Result<Ret, HostError> {
    match found {
        Ok(found) => Ok(found.into()),
        Err(err) => Err(user_error(&err)),
    }
}

/// The password policy of `user-facade/password`: `{minLength, minDigits, minOthers}`.
fn password_rules(_: &mut Call<'_>) -> Result<Ret, HostError> {
    let rules = crate::user::policy_password_rules();
    let mut map = indexmap::IndexMap::new();
    map.insert("minLength".to_string(), Value::I64(rules.min_length));
    map.insert("minDigits".to_string(), Value::I64(rules.min_digits));
    map.insert("minOthers".to_string(), Value::I64(rules.min_others));
    Ok(Ret::Value(Value::Map(map)))
}

/// False with no error when the password was right and a second factor is still needed. A null or missing username
/// or password is an empty one, a failed login as Java's: false and the message.
fn login_user(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let username = call.text(0).unwrap_or_default().to_string();
    let password = call.text(1).unwrap_or_default().to_string();
    match crate::user::login_user(&username, &password) {
        Ok(()) => Ok((!crate::user::session::session_attribute_flag(crate::user::AUTHC_FACTOR_REQUIRED)).into()),
        Err(err) => {
            with_ec(|ec| ec.message().add_error(err.message().to_string()))?;
            Ok(false.into())
        }
    }
}

fn logout_user(_: &mut Call<'_>) -> Result<Ret, HostError> {
    match crate::user::logout() {
        Ok(()) => Ok(true.into()),
        Err(err) => {
            with_ec(|ec| ec.message().add_error(err.message().to_string()))?;
            Ok(false.into())
        }
    }
}

fn internal_login_user(call: &mut Call<'_>) -> Result<Ret, HostError> {
    match crate::user::internal_login_user(call.text(0).unwrap_or_default()) {
        Ok(()) => Ok(true.into()),
        Err(err) => {
            with_ec(|ec| ec.message().add_error(err.message().to_string()))?;
            Ok(false.into())
        }
    }
}

/// The logged-in user's preference: a property, the user's `UserPreference`, or a group's, or null.
fn get_preference(call: &mut Call<'_>) -> Result<Ret, HostError> {
    text_or_null(user_preference(call.text(0).unwrap_or_default()))
}

fn locale(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(crate::l10n::locale().into())
}

/// The time zone of the session, the user's own when there is no other, else the server's (Java's
/// `ec.user.getTimeZone()`).
fn time_zone(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(session_time_zone_name().into())
}

/// The machine's time zone, the zone a ServiceJob cron is read in. A digest whose account has no
/// `UserAccount.timeZone` sets the session to this so the hour is the machine's. Not the server zone.
fn machine_time_zone(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(crate::service::job_time_zone()
        .iana_name()
        .unwrap_or("UTC")
        .to_string()
        .into())
}

/// `setTimeZone(name)`, Java's `UserFacade.setTimeZone`: the session's time zone from now on, also saved on the
/// signed-in user's `UserAccount`; with nobody signed in (a job) the session's only. An unknown zone is an error;
/// null or empty clears it.
fn set_time_zone(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let zone = call.opt_text(0).filter(|zone| !zone.trim().is_empty());
    if let Some(zone) = zone.as_deref() {
        if jiff::tz::TimeZone::get(zone).is_err() {
            return Err(HostError::message(format!("Time zone {zone} is not known")));
        }
    }
    save_user_setting("timeZone", zone.as_deref()).map_err(|err| user_error(&err))?;
    crate::user::session::set_time_zone(zone.as_deref());
    Ok(Ret::Undefined)
}

/// `setLocale(name)`, Java's `UserFacade.setLocale`: as [`set_time_zone`], for the locale (`en_US`, `de`).
fn set_locale(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let locale = call
        .opt_text(0)
        .map(|locale| locale.trim().replace('-', "_"))
        .filter(|locale| !locale.is_empty());
    save_user_setting("locale", locale.as_deref()).map_err(|err| user_error(&err))?;
    crate::user::session::set_locale(locale.as_deref());
    Ok(Ret::Undefined)
}

/// The logged-in user's `UserAccount` without the password and lockout columns, or null.
fn user_account(_: &mut Call<'_>) -> Result<Ret, HostError> {
    match user_account_snapshot() {
        Ok(found) => Ok(Ret::Value(found.map_or(Value::Null, Value::Map))),
        Err(err) => Err(user_error(&err)),
    }
}

/// The current time. A script sees time stamps as text, so this is the text of the stamp.
fn now_timestamp(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::Value(timestamp_of(&Value::I64(
        crate::user::now_effective_millis(),
    ))))
}

/// A time stamp from a script's value: epoch millis, a time stamp, or text, which is in the session time zone unless it
/// has an offset.
fn timestamp_of(value: &Value) -> Value {
    crate::l10n::basic_convert(value, "java.sql.Timestamp").value
}

/// `setEffectiveTime(time)`: `nowTimestamp`, date filters with no date and the defaults Java takes from it use `time`
/// (epoch millis, a `Date` or time stamp text) for this execution context. Null goes back to the clock.
fn set_effective_time(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let value = call.value_or_null(0);
    let millis = if moqui_lib::object::is_empty(&value) {
        None
    } else {
        let stamp = timestamp_of(&value);
        let millis = crate::user::value_millis(&stamp)
            .ok_or_else(|| HostError::message(format!("setEffectiveTime: {value:?} is not a time")))?;
        Some(millis)
    };
    with_ec(|ec| ec.set_effective_time(millis))?;
    Ok(Ret::Undefined)
}

/// The `moqui.server.Visit` id of this session, or null (Java's `getVisitId`).
fn visit_id(_: &mut Call<'_>) -> Result<Ret, HostError> {
    text_or_null(crate::user::visit_id())
}

/// The `moqui.server.Visit` of this session as a map, or null (Java's `getVisit`).
fn visit(_: &mut Call<'_>) -> Result<Ret, HostError> {
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
        Ok(found) => Ok(Ret::Value(found.unwrap_or(Value::Null))),
        Err(err) => Err(user_error(&err)),
    }
}

/// `endVisit(visitId)`: end another session of the signed-in user (`org.moqui.impl.UserServices.end#UserVisit`); the
/// number of web sessions ended. An error for the current visit or one that is not the user's.
fn end_visit(call: &mut Call<'_>) -> Result<Ret, HostError> {
    crate::user::end_user_visit(call.text(0).unwrap_or_default())
        .map(|count| Ret::Value(Value::I64(i64::try_from(count).unwrap_or(i64::MAX))))
        .map_err(|err| user_error(&err))
}

fn username(_: &mut Call<'_>) -> Result<Ret, HostError> {
    text_or_null(crate::user::current_username())
}

fn user_id(_: &mut Call<'_>) -> Result<Ret, HostError> {
    text_or_null(crate::user::current_user_id())
}

fn has_permission(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let permission = call.text_or_empty(0);
    crate::user::has_permission(&permission)
        .map(Ret::from)
        .map_err(|err| user_error(&err))
}

fn is_in_group(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let group = call.text_or_empty(0);
    crate::user::is_in_group(&group)
        .map(Ret::from)
        .map_err(|err| user_error(&err))
}

/// The group ids as a list, so the result works as the `from` of an `in` condition.
fn group_id_set(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let user_id = if call.passed(0) {
        call.opt_text(0)
    } else {
        crate::user::current_user_id().map_err(|err| user_error(&err))?
    };
    let groups = crate::user::user_group_ids(user_id.as_deref()).map_err(|err| user_error(&err))?;
    Ok(Ret::Value(Value::List(groups.into_iter().map(Value::String).collect())))
}

fn set_preference(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let key = call.text_or_empty(0);
    let value = call.opt_text(1);
    crate::user::set_preference(&key, value.as_deref()).map_err(|err| user_error(&err))?;
    Ok(Ret::Undefined)
}

fn all_preferences(_: &mut Call<'_>) -> Result<Ret, HostError> {
    let prefs = crate::user::get_preferences().map_err(|err| user_error(&err))?;
    Ok(Ret::Value(Value::Map(
        prefs
            .into_iter()
            .map(|(key, value)| (key, Value::String(value)))
            .collect(),
    )))
}

/// Java `UserFacade.getClientIp`: the request's client address, empty when there is no request.
fn client_ip(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(crate::web::current_client_ip().into())
}

fn login_key(call: &mut Call<'_>) -> Result<Ret, HostError> {
    crate::user::get_login_key(call.number(0))
        .map(Ret::from)
        .map_err(|err| user_error(&err))
}

/// The `user-facade` attributes the user services read from `ec.ecfi.confXmlRoot` in Java.
fn login_rules(_: &mut Call<'_>) -> Result<Ret, HostError> {
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
    Ok(Ret::Value(Value::Map(map)))
}

/// A new password hash. Always argon2; the account's `passwordHashType` becomes `argon2`.
fn hash_password(call: &mut Call<'_>) -> Result<Ret, HostError> {
    crate::user::hash_password(call.text(0).unwrap_or_default())
        .map(Ret::from)
        .map_err(|err| user_error(&err))
}

/// `getCredentialsMatcher(hashType, base64).doCredentialsMatch`: `plain` against a stored hash, argon2 or a legacy
/// digest. An empty stored hash never matches.
fn passwords_match(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let plain = call.text_or_empty(0);
    let stored = call.text_or_empty(1);
    let hash_type = call.text_or_empty(2);
    let salt = call.text_or_empty(3);
    let base64 = call.value(4).is_some_and(truthy);
    if plain.is_empty() {
        return Ok(false.into());
    }
    crate::user::stored_password_matches(&plain, &stored, &hash_type, &salt, base64)
        .map(Ret::from)
        .map_err(|err| user_error(&err))
}

fn single_use_code_hash(call: &mut Call<'_>) -> Result<Ret, HostError> {
    crate::user::single_use_hash(&call.text_or_empty(0))
        .map(Ret::from)
        .map_err(|err| user_error(&err))
}

fn random_code(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let width = usize::try_from(call.int(0).unwrap_or(6)).unwrap_or(6);
    crate::user::random_code(width)
        .map(Ret::from)
        .map_err(|err| user_error(&err))
}

fn random_string(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let length = usize::try_from(call.int(0).unwrap_or(0)).unwrap_or(0);
    crate::user::random_string(length)
        .map(Ret::from)
        .map_err(|err| user_error(&err))
}

fn totp_secret(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(crate::user::totp_secret().into())
}

fn totp_code_valid(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(crate::user::totp_code_valid(&call.text_or_empty(0), &call.text_or_empty(1)).into())
}

fn totp_uri(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(crate::user::totp_uri(&call.text_or_empty(0), &call.text_or_empty(1), &call.text_or_empty(2)).into())
}

/// Epoch milliseconds of a time stamp, which a script holds as text. Null when it is not one.
fn timestamp_millis(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let host = call.value_or_null(0);
    if moqui_lib::object::is_empty(&host) {
        return Ok(Ret::Value(Value::Null));
    }
    match crate::l10n::basic_convert(&host, "java.sql.Timestamp").value {
        #[allow(clippy::cast_precision_loss)] // Milliseconds of real dates fit a double exactly.
        Value::Timestamp(stamp) => Ok(Ret::Value(Value::F64(stamp.as_millisecond() as f64))),
        _ => Ok(Ret::Value(Value::Null)),
    }
}

fn context_read(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.text(0).unwrap_or_default();
    let value = with_ec(|ec| ec.user_context().get(name).cloned())?;
    Ok(Ret::Value(value.unwrap_or(Value::Null)))
}

/// Undefined removes the name; null keeps it with a null value, as a Java map put of null does.
fn context_write(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.text(0).unwrap_or_default().to_string();
    let value = call.value(1).cloned();
    with_ec(|ec| {
        let mut map = ec.user_context();
        match value {
            Some(value) => map.insert(name, value),
            None => map.remove(&name),
        };
    })?;
    Ok(Ret::Undefined)
}

fn context_names(_: &mut Call<'_>) -> Result<Ret, HostError> {
    let names: Vec<Value> = with_ec(|ec| ec.user_context().keys().cloned().map(Value::String).collect())?;
    Ok(Ret::Value(Value::List(names)))
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

fn user_account_snapshot() -> Result<Option<indexmap::IndexMap<String, Value>>, Error> {
    let Some(user_id) = crate::user::current_user_id()? else {
        return Ok(None);
    };
    let Some(facade) = ExecutionContext::with_current(|ec| ec.factory().entity()).flatten() else {
        return Ok(None);
    };
    let _authz = ExecutionContext::with_current(|ec| ec.disable_authz());
    let found = facade
        .find("moqui.security.UserAccount")
        .map_err(Error::from)?
        .eq("userId", Value::string(&user_id))
        .one()
        .map_err(Error::from)?;
    Ok(found.map(|row| {
        row.fields()
            .iter()
            .filter(|(key, _)| !USER_ACCOUNT_PRIVATE.contains(&&***key))
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect()
    }))
}

fn user_preference(key: &str) -> Result<Option<String>, Error> {
    if ExecutionContext::with_current(|ec| ec.factory().entity())
        .flatten()
        .is_none()
    {
        return Ok(None);
    }
    crate::user::get_preference(key)
}
