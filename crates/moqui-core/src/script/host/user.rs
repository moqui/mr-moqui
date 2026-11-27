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
        method!(
            "ec.user",
            "loginUser",
            [OptNullableText, OptNullableText],
            None,
            login_user
        ),
        method!("ec.user", "logoutUser", [], None, logout_user),
        method!("ec.user", "logoutLocal", [], None, logout_local),
        method!("ec.user", "internalLoginUser", [Text, OptBool], None, internal_login_user),
        method!("ec.user", "loginUserKey", [Text], None, login_user_key),
        method!(
            "ec.user",
            "loginAnonymousIfNoUser",
            [],
            None,
            login_anonymous_if_no_user
        ),
        method!("ec.user", "getVisitorId", [], None, leaf, visitor_id),
        method!("ec.user", "getPreference", [Text], None, get_preference),
        method!("ec.user", "getLocale", [], locale),
        method!("ec.user", "getTimeZone", [], time_zone),
        method!("ec.user", "setTimeZone", [OptValue], None, set_time_zone),
        method!("ec.user", "setLocale", [OptValue], None, set_locale),
        method!("ec.user", "setEffectiveTime", [OptValue], set_effective_time),
        method!("ec.user", "getVisitId", [], None, leaf, visit_id),
        method!("ec.user", "getVisit", [], None, visit),
        method!("ec.user", "endVisit", [Text], None, end_visit),
        method!("ec.user", "hasPermission", [Value], None, has_permission),
        method!("ec.user", "isInGroup", [Value], None, is_in_group),
        method!("ec.user", "heldBeyond", [Text], None, held_beyond),
        method!("ec.user", "getUserGroupIdSet", [OptValue], None, group_id_set),
        method!("ec.user", "setPreference", [Value, OptValue], None, set_preference),
        method!("ec.user", "getClientIp", [], client_ip),
        method!("ec.user", "getLoginKey", [OptNumber], login_key),
        method!("ec.user", "hashPassword", [Text], hash_password),
        method!(
            "ec.user",
            "checkOldPassword",
            [OptNullableText, OptNullableText],
            None,
            check_old_password
        ),
        method!(
            "ec.user",
            "checkCountedCode",
            [OptNullableText, OptNullableText],
            None,
            check_counted_code
        ),
        method!("ec.user", "countFailedLogin", [Text], None, count_failed_login),
        method!("ec.user", "takeFactorCode", [Text, OptValue], None, take_factor_code),
        method!(
            "ec.user",
            "initialAdminAllowed",
            [OptNullableText],
            None,
            initial_admin_allowed
        ),
        method!(
            "ec.user",
            "passwordsMatch",
            [Value, Value, OptValue, OptValue, OptValue],
            passwords_match
        ),
        method!("ec.user", "singleUseCodeHash", [Value], single_use_code_hash),
        method!("ec.user", "randomCode", [OptInt], random_code),
        method!("ec.user", "randomString", [Int], random_string),
        method!("ec.user", "serverGrant", [Text, Text, OptNumber], server_grant),
        method!(
            "ec.user",
            "checkServerGrant",
            [Text, Text, OptNullableText],
            check_server_grant
        ),
        method!("ec.user", "totpSecret", [], totp_secret),
        method!("ec.user", "totpCodeValid", [Value, Value], totp_code_valid),
        method!("ec.user", "totpCodeStep", [Value, Value, Value], totp_code_step),
        method!("ec.user", "totpUri", [Value, Value, Value], totp_uri),
        method!("ec.user", "timestampMillis", [Value], timestamp_millis),
    ],
    getters: &[
        method!("ec.user", "passwordRules", [], password_rules),
        method!("ec.user", "locale", [], locale),
        method!("ec.user", "timeZone", [], time_zone),
        method!("ec.user", "machineTimeZone", [], None, leaf, machine_time_zone),
        method!("ec.user", "userAccount", [], None, user_account),
        method!("ec.user", "nowTimestamp", [], None, leaf, now_timestamp),
        method!("ec.user", "visitId", [], None, leaf, visit_id),
        method!("ec.user", "visit", [], None, visit),
        method!("ec.user", "username", [], None, leaf, username),
        method!("ec.user", "userId", [], None, leaf, user_id),
        method!("ec.user", "loginKeyKind", [], None, leaf, login_key_kind),
        method!("ec.user", "loginKeyId", [], None, leaf, login_key_id),
        method!("ec.user", "loginRules", [], login_rules),
        method!("ec.user", "initialAdminLocal", [], None, leaf, initial_admin_local),
    ],
    hidden: &[
        method!("ec.user.context", "read", [Text], context_read),
        method!("ec.user.context", "write", [Text, OptValue], context_write),
        method!("ec.user.context", "names", [], context_names),
        method!("ec.user", "getPreferences", [OptText], None, all_preferences),
    ],
    shim_js: Some(USER_JS),
    shim_members: &[
        "getPreferences",
        "context",
        "getVisitUserId",
        "currencyUomId",
        "setCurrencyUomId",
        "getPeriodDescription",
        "getNowCalendar",
    ],
};

/// `getPreferences(keyRegexp)`: the key pattern is matched against the whole key, as Java's `String.matches` does, by
/// the host ([`key_matcher`]).
///
/// `ec.user.context`: a map for the current execution context's user. Each read and write goes to the host, so the
/// entity filters see what `setup#UserOrganizationInfo` puts there. A list read from it is a copy: put it back to
/// change it.
const USER_JS: &str = r#"(function (proto, hidden) {
    const all = hidden.getPreferences;
    proto.getPreferences = function (keyRegexp) {
        return all(keyRegexp == null ? null : String(keyRegexp));
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
    // Java's getVisitUserId: the user of the visit, which a later login in the same visit does not change
    proto.getVisitUserId = function () { const visit = this.visit; return visit ? (visit.userId ?? null) : null; };
    // The user's currency, on the account (Java keeps it there too). Setting it with nobody logged in changes nothing.
    Object.defineProperty(proto, "currencyUomId", {
        get() { const account = this.userAccount; return account ? (account.currencyUomId ?? null) : null; },
        enumerable: false, configurable: true
    });
    proto.setCurrencyUomId = function (uomId) {
        const userId = this.userId;
        if (!userId) return;
        const ec = globalThis.ec;
        const already = ec.artifactExecution.disableAuthz();
        try {
            const account = ec.entity.find("moqui.security.UserAccount").condition("userId", userId).one();
            if (account) { account.set("currencyUomId", uomId == null ? null : String(uomId)); account.update(); }
        } finally { if (!already) ec.artifactExecution.enableAuthz(); }
    };
    // Java's getPeriodDescription: "This Month (2026-11-01 to 2026-11-30)", the end date inclusive
    proto.getPeriodDescription = function (period, poffset, pdate) {
        const ec = globalThis.ec, l10n = ec.l10n;
        period = String(period ?? ""); poffset = String(poffset ?? "0");
        const range = this.getPeriodRange(period, poffset, pdate ?? "");
        const offsets = { "0": "This", "-1": "Last", "1": "Next" };
        const periods = { day: "Day", week: "Week", weeks: "Weeks", month: "Month", months: "Months",
            quarter: "Quarter", year: "Year" };
        let text = (offsets[poffset] ? l10n.localize(offsets[poffset]) : poffset) + " ";
        if (period === "7d" || period === "30d") text += period.slice(0, -1) + " " + l10n.localize("Days");
        else if (period === "7r" || period === "30r") text += "+/-" + period.slice(0, -1) + "d";
        else if (periods[period]) text += l10n.localize(periods[period]);
        if (pdate) text += " " + l10n.localize("from##period") + " " + pdate;
        const day = 24 * 60 * 60 * 1000;
        const from = l10n.format(this.timestampMillis(range[0]), "yyyy-MM-dd");
        const thru = l10n.format(this.timestampMillis(range[1]) - day, "yyyy-MM-dd");
        return text + " (" + from + " " + l10n.localize("to##period") + " " + thru + ")";
    };
    // Java's getNowCalendar, as a Date of the effective time
    proto.getNowCalendar = function () { return new Date(this.timestampMillis(this.nowTimestamp)); };
})"#;

/// Java's `loginUserKey`: whether the login key logged its user in. A key that does not is an error message, as a
/// failed `loginUser` is.
fn login_user_key(call: &mut Call<'_>) -> Result<Ret, HostError> {
    // a preview's proposed text does not change who it runs as (SEC-AI-102)
    super::refuse_locked_authz()?;
    let key = call.text_or_empty(0);
    match crate::user::login_user_key(&key) {
        Ok(()) => Ok(true.into()),
        Err(err) => {
            with_ec(|ec| ec.message().add_error(err.message().to_string()))?;
            Ok(false.into())
        }
    }
}

/// Java's `loginAnonymousIfNoUser`: with nobody logged in the session becomes anonymous; whether this call did that.
fn login_anonymous_if_no_user(_: &mut Call<'_>) -> Result<Ret, HostError> {
    // a preview's proposed text does not change who it runs as (SEC-AI-102)
    super::refuse_locked_authz()?;
    crate::user::login_anonymous()
        .map(Ret::from)
        .map_err(|err| user_error(&err))
}

fn visitor_id(_: &mut Call<'_>) -> Result<Ret, HostError> {
    text_or_null(crate::user::visitor_id())
}

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
    let mut map = indexmap::IndexMap::default();
    map.insert("minLength".to_string(), Value::I64(rules.min_length));
    map.insert("minDigits".to_string(), Value::I64(rules.min_digits));
    map.insert("minOthers".to_string(), Value::I64(rules.min_others));
    Ok(Ret::Value(Value::Map(map)))
}

/// False with no error when the password was right and a second factor is still needed. A null or missing username
/// or password is an empty one, a failed login as Java's: false and the message.
fn login_user(call: &mut Call<'_>) -> Result<Ret, HostError> {
    // a preview's proposed text does not change who it runs as (SEC-AI-102)
    super::refuse_locked_authz()?;
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
    // a preview's proposed text does not change who it runs as (SEC-AI-102)
    super::refuse_locked_authz()?;
    match crate::user::logout() {
        Ok(()) => Ok(true.into()),
        Err(err) => {
            with_ec(|ec| ec.message().add_error(err.message().to_string()))?;
            Ok(false.into())
        }
    }
}

/// `ec.user.logoutLocal()`: end this context's login only, writing nothing to the account (a job's borrowed sign-in).
fn logout_local(_: &mut Call<'_>) -> Result<Ret, HostError> {
    // a preview's proposed text does not change who it runs as (SEC-AI-102)
    super::refuse_locked_authz()?;
    match crate::user::logout_local() {
        Ok(()) => Ok(true.into()),
        Err(err) => {
            with_ec(|ec| ec.message().add_error(err.message().to_string()))?;
            Ok(false.into())
        }
    }
}

fn internal_login_user(call: &mut Call<'_>) -> Result<Ret, HostError> {
    // a preview's proposed text does not change who it runs as (SEC-AI-102)
    super::refuse_locked_authz()?;
    // a second argument true: the caller verified this sign-in's second factor code (a strong sign-in)
    let verified = call.bool(1).unwrap_or(false);
    match crate::user::internal_login_user_verified(call.text(0).unwrap_or_default(), verified) {
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
    let millis = crate::user::now_effective_millis();
    // a time stamp made from the millis, as the conversion makes it, without its text
    match jiff::Timestamp::from_millisecond(millis) {
        Ok(timestamp) => Ok(Ret::Value(Value::Timestamp(timestamp))),
        Err(_) => Ok(Ret::Value(timestamp_of(&Value::I64(millis)))),
    }
}

/// A time stamp from a script's value: epoch millis, a time stamp, or text, which is in the session time zone unless it
/// has an offset.
fn timestamp_of(value: &Value) -> Value {
    crate::l10n::basic_convert(value, "Timestamp").value
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

/// `loginKeyKind`: `device`, `token`, `session` or `scoped` when this request's user signed in by a login key (the
/// `api_key` or `login_key` header, or `loginUserKey`); null for any other login and with nobody signed in.
fn login_key_kind(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(crate::user::login_key_kind()
        .map(|kind| kind.as_str().to_string())
        .into())
}

/// `loginKeyId`: the id of the `UserLoginKey` this request's user signed in with (its stored hash, as the device list
/// names a device; never the key itself), with `loginKeyKind`; null for any other login. A location share records the
/// device it is sent from by it (plans/archive/Location-Sharing.md §3.1).
fn login_key_id(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(crate::user::login_key_id().into())
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

/// `user::held_beyond`: why the current user may not act as or on the account, or null.
fn held_beyond(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let user_id = call.text_or_empty(0);
    crate::user::held_beyond(&user_id)
        .map(|reason| Ret::from(reason.map_or(Value::Null, Value::String)))
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

fn all_preferences(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let matcher = match call.opt_text(0).filter(|pattern| !pattern.is_empty()) {
        Some(pattern) => Some(key_matcher(&pattern).map_err(|err| user_error(&err))?),
        None => None,
    };
    let prefs = crate::user::get_preferences().map_err(|err| user_error(&err))?;
    Ok(Ret::Value(Value::Map(
        prefs
            .into_iter()
            .filter(|(key, _)| matcher.as_ref().is_none_or(|matcher| matcher.is_match(key)))
            .map(|(key, value)| (key, Value::String(value)))
            .collect(),
    )))
}

/// The longest key pattern `getPreferences` takes.
const KEY_PATTERN_MAX: usize = 256;

/// A `getPreferences` key pattern, matched against the whole key. The pattern comes from a request (`/apps/getPreferences`),
/// so it is compiled by the `regex` crate, whose matching time is linear in the key: a backtracking pattern cannot hold a
/// worker as it did in the script's engine (SEC-RTE-15). Its syntax is the common one (no back-references or look-around).
pub(crate) fn key_matcher(pattern: &str) -> Result<regex::Regex, Error> {
    if pattern.len() > KEY_PATTERN_MAX {
        return Err(Error::new(format!(
            "a preference key pattern has at most {KEY_PATTERN_MAX} characters"
        )));
    }
    regex::RegexBuilder::new(&format!("^(?:{pattern})$"))
        .size_limit(1 << 20)
        .build()
        .map_err(|_| Error::new("the preference key pattern is not a valid regular expression"))
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
    let mut map = indexmap::IndexMap::default();
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
/// `ec.user.checkOldPassword(userId, oldPassword)`: the old password of a password change, behind the login gate
/// ([`crate::user::check_old_password`]). A null or unknown `userId` is refused after the same work as a wrong password.
fn check_old_password(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let user_id = call.text_or_empty(0);
    let old_password = call.text_or_empty(1);
    crate::user::check_old_password(&user_id, &old_password)
        .map(Ret::from)
        .map_err(|err| user_error(&err))
}

/// `ec.user.checkCountedCode(userId, code)`: a second factor's code that stands in for a login, behind the login gate
/// and counted when wrong ([`crate::user::check_counted_code`]): the id of the factor it verified, else null.
fn check_counted_code(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let user_id = call.text_or_empty(0);
    let code = call.text_or_empty(1);
    text_or_null(crate::user::check_counted_code(&user_id, &code))
}

/// `ec.user.countFailedLogin(userId)`: count a failed login of the account as a wrong password is counted
/// ([`crate::user::count_failed_login`]).
fn count_failed_login(call: &mut Call<'_>) -> Result<Ret, HostError> {
    crate::user::count_failed_login(&call.text_or_empty(0))
        .map(|()| Ret::Undefined)
        .map_err(|err| user_error(&err))
}

/// `ec.user.takeFactorCode(factorId, step)`: take a factor's code once, a single-use code with no `step`, an
/// authenticator code for its step ([`crate::user::take_factor_code`]): true when this call took it.
fn take_factor_code(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let factor_id = call.text_or_empty(0);
    // a step from totpCodeStep is a number; it may come as its text (12345 or 12345.0)
    let text = call.text_or_empty(1);
    let text = text.trim();
    let step = text
        .split_once('.')
        .map_or(text, |(whole, _)| whole)
        .parse::<i64>()
        .ok();
    crate::user::take_factor_code(&factor_id, step)
        .map(Ret::from)
        .map_err(|err| user_error(&err))
}

/// `ec.user.initialAdminAllowed(setupSecret)`: whether this request may create the first administrator (from this
/// machine, or with the setup secret of the startup log; [`crate::user::initial_admin_allowed`]).
fn initial_admin_allowed(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(crate::user::initial_admin_allowed(&call.text_or_empty(0)).into())
}

/// `ec.user.initialAdminLocal`: whether this request comes from this machine, so the first administrator's form needs
/// no setup secret ([`crate::user::initial_admin_local`]).
fn initial_admin_local(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(crate::user::initial_admin_local().into())
}

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

/// What a server grant's key is derived under (`user::derived_cookie_key`), the purpose after it: a key of its own for
/// each purpose, which is no session tag and no other derived key, and which no script reads (SEC-PSS-25).
const SERVER_GRANT_LABEL: &str = "moqui.script.serverGrant\n";

/// The key of a server grant for `purpose`, as hex: the HMAC key `files::make_grant` and `check_grant` take.
fn server_grant_key(purpose: &str) -> Result<String, Error> {
    if purpose.trim().is_empty() {
        return Err(Error::new("a server grant needs a purpose"));
    }
    let key = crate::user::derived_cookie_key(&format!("{SERVER_GRANT_LABEL}{purpose}"))?;
    Ok(moqui_lib::codec::hex_encode(&key))
}

/// `ec.user.serverGrant(purpose, subject, ttlSeconds)`: a token that [`check_server_grant`] accepts for the same purpose
/// and subject until `ttlSeconds` (3600; 1 second to 7 days) have passed, `<expiry millis>.<HMAC-SHA256 hex>`, keyed by
/// the server's own secret (`webapp-list/webapp[webroot]/session-config@cookie-key`, through a key derived for the purpose), so a script can sign a value
/// it hands a client (a page's nonce) with no key of its own. An error with no purpose or no `webapp-list/webapp[webroot]/session-config@cookie-key`.
fn server_grant(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let key = server_grant_key(&call.text_or_empty(0)).map_err(|err| user_error(&err))?;
    let ttl_seconds = call.number(2).filter(|seconds| seconds.is_finite()).unwrap_or(3600.0);
    #[allow(clippy::cast_possible_truncation)]
    let ttl = ttl_seconds as i64;
    super::files::make_grant(&key, &call.text_or_empty(1), ttl)
        .map(Ret::from)
        .ok_or_else(|| HostError::message("the server grant key is empty"))
}

/// `ec.user.checkServerGrant(purpose, subject, token)`: whether `token` is a [`server_grant`] of this purpose and
/// subject that has not expired, compared in constant time; false for a null token, no purpose or no key.
fn check_server_grant(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let (Ok(key), Some(token)) = (server_grant_key(&call.text_or_empty(0)), call.opt_text(2)) else {
        return Ok(false.into());
    };
    Ok(super::files::check_grant(&key, &call.text_or_empty(1), &token).into())
}

fn totp_secret(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(crate::user::totp_secret().into())
}

fn totp_code_valid(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(crate::user::totp_code_valid(&call.text_or_empty(0), &call.text_or_empty(1)).into())
}

/// `totpCodeStep(secret, code, afterStep)`: the step the code is valid for now when it is after `afterStep` (the factor's
/// last one taken; null for none), else null (`user::totp_code_step`).
fn totp_code_step(call: &mut Call<'_>) -> Result<Ret, HostError> {
    // a step read from an entity may come as a decimal's text (12345 or 12345.0)
    let text = call.text_or_empty(2);
    let text = text.trim();
    let after = text
        .split_once('.')
        .map_or(text, |(whole, _)| whole)
        .parse::<u64>()
        .ok();
    Ok(Ret::Value(
        crate::user::totp_code_step(&call.text_or_empty(0), &call.text_or_empty(1), after)
            .map_or(Value::Null, |step| Value::I64(i64::try_from(step).unwrap_or(i64::MAX))),
    ))
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
    match crate::l10n::basic_convert(&host, "Timestamp").value {
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

fn user_account_snapshot() -> Result<Option<moqui_lib::object::ValueMap>, Error> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preference_key_pattern_matches_whole_keys_in_linear_time_and_is_capped() {
        let print = key_matcher("qz\\.print.*").unwrap();
        assert!(print.is_match("qz.print.label"));
        assert!(!print.is_match("xqz.print"));
        assert!(key_matcher("a|b").unwrap().is_match("b"));
        assert!(!key_matcher("a|b").unwrap().is_match("ab"));
        // a pattern that backtracks without end in a script's engine answers at once
        let nested = key_matcher("(a+)+$").unwrap();
        let started = std::time::Instant::now();
        assert!(!nested.is_match(&format!("{}!", "a".repeat(10_000))));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert!(key_matcher(&"a".repeat(KEY_PATTERN_MAX + 1)).is_err());
        assert!(key_matcher("(unclosed").is_err());
    }
}
