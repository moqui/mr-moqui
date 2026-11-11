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

//! The host's globals outside `ec` and the helper objects' functions, as QuickJS's engine has them
//! (`runtime/helpers.rs`, `runtime/ec_extra.rs`'s `fill_helpers`, `System` in `runtime.rs`): `compare`,
//! `numberToWords`, `numberToWordsWithDecimal`, `orderMapList`, `filterMapList`, `filterMapListByDate`,
//! `elasticQueryAutoWildcard`, `System.getProperty`, and the functions of `StringUtilities`, `WebUtilities` and
//! `CollectionUtilities` whose arguments and results are host values, and `UserSecurityChecks`, the checks
//! `Security.eecas.xml` runs. The helper objects, their functions and `System` are frozen ([`freeze_helpers_js`]), so
//! a script cannot replace a check, or any helper, for the later scripts on the thread.
//!
//! [`SHIM`] is not a path of the table: the oxide engine installs it itself (`oxide::engine`), and QuickJS keeps its own
//! until Phase 4 removes it, when this shim takes their place.

use std::cmp::Ordering;

use moqui_lib::object::Value;

use super::{method, Call, HostError, Ret, Shim};

/// The function that freezes the helper objects and the functions on them, once every shim has filled them: a check
/// of `UserSecurityChecks` replaced by one script would otherwise stand for every later script on the thread. A frozen
/// object's assignment throws in strict code and does nothing in sloppy code. A value behind a getter is not read.
macro_rules! freeze_helpers_js {
    () => {
        r#"function freezeHelpers(objects) {
    for (const object of objects) {
        if (object === null || typeof object !== "object") continue;
        for (const name of Object.getOwnPropertyNames(object)) {
            const held = Object.getOwnPropertyDescriptor(object, name);
            if (typeof held.value === "function") Object.freeze(held.value);
        }
        Object.freeze(object);
    }
}"#
    };
}

const GLOBALS_JS: &str = concat!(
    r#"(function (global, hidden, helpers) {
    "use strict";
    for (const name of ["compare", "numberToWords", "numberToWordsWithDecimal", "__compareOrder", "__inDateRange"]) {
        global[name] = hidden[name];
    }
    global.System = { getProperty: hidden.getProperty };
    const web = helpers.WebUtilities, string = helpers.StringUtilities, collection = helpers.CollectionUtilities;
    for (const name of ["encodeHtml", "encodeHtmlJsSafe", "fieldValuesEncodeHtmlJsSafe", "ipMatches",
        "isSafeSinglePathSegment", "isSameOriginRedirect", "lastPathSegment"]) web[name] = hidden[name];
    for (const name of ["encodeForXmlAttribute", "base64UrlEncode", "base64UrlDecode", "base64Encode", "base64Decode",
        "cleanHtml",
        "urlEncodeIfNeeded"]) string[name] = hidden[name];
    // the checks Security.eecas.xml runs, a global of a whole runtime (`oxide::engine`) as the helpers are
    helpers.UserSecurityChecks = {
        assertPrivilegedGroupMembership: hidden.assertPrivilegedGroupMembership,
        assertSealedPermissionGrant: hidden.assertSealedPermissionGrant,
        assertDocumentExpression: hidden.assertDocumentExpression,
        assertSealedGroupMembership: hidden.assertSealedGroupMembership,
        privilegedGroups: hidden.privilegedGroups,
        sealedPermissions: hidden.sealedPermissions,
        endUserSessions: hidden.endUserSessions,
        newDeviceCode: hidden.newDeviceCode,
        redeemDeviceCode: hidden.redeemDeviceCode,
        displayAccountRefusal: hidden.displayAccountRefusal,
        currentDevice: hidden.currentDevice
    };
    collection.flattenNestedMap = hidden.flattenNestedMap;
    collection.getMapArrayListValues = hidden.getMapArrayListValues;
    // the page values go on the map passed too, as Java's does
    collection.paginateParameters = function paginateParameters(size, name, context) {
        const out = hidden.paginateParameters(size, name, context);
        if (context !== null && typeof context === "object") {
            for (const suffix of ["Count", "PageIndex", "PageSize", "PageMaxIndex", "PageRangeLow", "PageRangeHigh"]) {
                const key = name + suffix;
                if (out !== null && key in out) context[key] = out[key];
            }
        }
        return out;
    };
    const ec = global.ec;
    const factory = ec.factory || (ec.factory = {});
    factory.moquiVersion = hidden.moquiVersion();
    factory.hasFeature = hidden.hasFeature;
    // Java's TransactionFacade (`transaction.rs`)
    // body() in a transaction begun by `begin`: committed when it returns, rolled back (or, joined, marked
    // rollback-only) when it throws, and what it threw is thrown on
    const runIn = (begin) => (timeout, rollbackMessage, body) => {
        if (typeof body !== "function") throw new TypeError("the body of a transaction is a function");
        const began = begin(timeout ?? null);
        let result;
        try {
            result = body();
        } catch (err) {
            ec.transaction.rollback(began, rollbackMessage ?? "Error in transaction", err);
            throw err;
        }
        hidden.commit(began);
        return result;
    };
    const statusNames = { 0: "Active", 1: "Marked Rollback-Only", 6: "No Transaction" };
    ec.transaction = {
        isTransactionInPlace: hidden.isTransactionInPlace,
        isTransactionCacheActive: hidden.isTransactionCacheActive,
        begin: (timeout) => hidden.begin(timeout ?? null),
        commit: hidden.commit,
        // rollback(began, message, error) and rollback(message, error)
        rollback: function (first, second, third) {
            const text = (value) => value == null ? null : String(value instanceof Error ? value.message : value);
            if (typeof first === "boolean") return hidden.rollback(first, text(second), text(third));
            return hidden.rollback(true, text(first), text(second));
        },
        setRollbackOnly: (message, error) => hidden.setRollbackOnly(message == null ? null : String(message),
            error == null ? null : String(error instanceof Error ? error.message : error)),
        suspend: hidden.suspend,
        resume: hidden.resume,
        getStatus: hidden.getStatus,
        getStatusString: () => { const status = hidden.getStatus(); return statusNames[status] + " (" + status + ")"; },
        runUseOrBegin: runIn(hidden.begin),
        runRequireNew: runIn(hidden.beginNew),
        // Java's transaction cache calls: the entity facade's here
        initTransactionCache: (readOnly) => ec.entity.startTxCacheDb(readOnly === true),
        flushAndDisableTransactionCache: () => ec.entity.stopTxCache()
    };
    if (ec.entity) ec.entity.databaseTimeZone = "UTC";
    const resource = ec.resource || (ec.resource = {});
    for (const name of ["code128Svg", "barcodeSvg", "parseGs1", "isValidGs1CheckDigit", "template",
        "getTemplateRendererExtensionSet"]) resource[name] = hidden[name];
    // filled below with Java's `ObjectUtilities` statics
    helpers.ObjectUtilities = {};
    // the rest of the Java helper classes' statics (`utilities.rs`)
    ("#,
    super::utilities::utilities_js!(),
    r#")(global, helpers);
    // the last shim to fill them (`oxide::engine`, `finish_start`), so nothing writes them after this
    ("#,
    freeze_helpers_js!(),
    r#")([helpers.ObjectUtilities, helpers.CollectionUtilities, helpers.StringUtilities, helpers.WebUtilities,
        helpers.UserSecurityChecks, global.System]);
})"#
);

/// The shim; its JavaScript after it is QuickJS's own (`runtime/helpers.rs`'s `HELPERS_JS`), run by the engine.
pub(crate) static SHIM: Shim = Shim {
    js: GLOBALS_JS,
    hidden: &[&[
        method!(
            "global",
            "compare",
            [Value, Text, Value, OptValue, OptValue, OptText],
            None,
            leaf,
            compare
        ),
        method!("global", "numberToWords", [Value, OptBool], None, leaf, number_to_words),
        method!(
            "global",
            "numberToWordsWithDecimal",
            [Value],
            None,
            leaf,
            number_to_words_with_decimal
        ),
        method!("global", "__compareOrder", [Value, Value], None, leaf, compare_order),
        method!(
            "global",
            "__inDateRange",
            [Value, Value, Value],
            None,
            leaf,
            in_date_range
        ),
        method!("System", "getProperty", [Text], None, leaf, get_property),
        method!("WebUtilities", "encodeHtml", [OptValue], None, leaf, encode_html),
        method!(
            "WebUtilities",
            "encodeHtmlJsSafe",
            [OptValue],
            None,
            leaf,
            encode_html_js_safe
        ),
        method!(
            "WebUtilities",
            "fieldValuesEncodeHtmlJsSafe",
            [Value],
            None,
            leaf,
            field_values_js_safe
        ),
        method!("WebUtilities", "ipMatches", [OptText, OptText], None, leaf, ip_matches),
        method!(
            "WebUtilities",
            "isSafeSinglePathSegment",
            [OptText],
            None,
            leaf,
            safe_segment
        ),
        method!(
            "WebUtilities",
            "isSameOriginRedirect",
            [OptText, OptValue],
            None,
            leaf,
            same_origin_redirect
        ),
        method!(
            "WebUtilities",
            "lastPathSegment",
            [OptText],
            None,
            leaf,
            last_path_segment
        ),
        method!(
            "StringUtilities",
            "encodeForXmlAttribute",
            [OptValue, OptBool],
            None,
            leaf,
            encode_for_xml_attribute
        ),
        method!(
            "StringUtilities",
            "base64UrlEncode",
            [OptText],
            None,
            leaf,
            base64_url_encode
        ),
        method!(
            "StringUtilities",
            "base64UrlDecode",
            [OptText],
            None,
            leaf,
            base64_url_decode
        ),
        method!("StringUtilities", "cleanHtml", [OptText], None, leaf, clean_html),
        method!(
            "StringUtilities",
            "urlEncodeIfNeeded",
            [OptText],
            None,
            leaf,
            url_encode_if_needed
        ),
        method!(
            "CollectionUtilities",
            "flattenNestedMap",
            [Value],
            None,
            leaf,
            flatten_nested_map
        ),
        method!("StringUtilities", "base64Encode", [OptValue], None, leaf, base64_encode),
        method!("StringUtilities", "base64Decode", [OptText], None, leaf, base64_decode),
        method!("CollectionUtilities", "getMapArrayListValues", [Value, Text, OptBool], None, leaf, map_list_values),
        method!("CollectionUtilities", "paginateParameters", [Int, Text, Value], None, leaf, paginate_parameters),
        method!("ec.resource", "code128Svg", [Value, OptValue], None, leaf, code128_svg),
        method!("ec.resource", "barcodeSvg", [Text, Value, OptValue], None, leaf, barcode_svg),
        method!("ec.resource", "parseGs1", [Text], None, leaf, parse_gs1),
        method!("ec.resource", "isValidGs1CheckDigit", [Text], None, leaf, gs1_check),
        method!("ec.resource", "template", [Text, OptValue], Release, template),
        method!("ec.resource", "getTemplateRendererExtensionSet", [], None, leaf, template_extensions),
        method!("ec.factory", "moquiVersion", [], None, leaf, moqui_version),
        method!("ec.factory", "hasFeature", [Text], None, leaf, has_feature),
        method!("ec.transaction", "isTransactionInPlace", [], None, leaf, in_transaction),
        method!(
            "ec.transaction",
            "isTransactionCacheActive",
            [],
            None,
            leaf,
            transaction_cache_active
        ),
        // Java's TransactionFacade (`transaction.rs`): a commit runs its hooks, so not leaves
        method!("ec.transaction", "begin", [OptValue], super::transaction::begin),
        method!("ec.transaction", "commit", [OptValue], super::transaction::commit),
        method!("ec.transaction", "rollback", [OptValue, OptValue, OptValue], super::transaction::rollback),
        method!("ec.transaction", "setRollbackOnly", [OptValue, OptValue], super::transaction::set_rollback_only),
        method!("ec.transaction", "suspend", [], super::transaction::suspend),
        method!("ec.transaction", "resume", [], super::transaction::resume),
        method!("ec.transaction", "getStatus", [], None, leaf, super::transaction::status),
        method!("ec.transaction", "beginNew", [OptValue], super::transaction::begin_new),
        // they read the entities, so not leaves
        method!("UserSecurityChecks", "assertPrivilegedGroupMembership", [OptValue], None, assert_privileged_group),
        method!("UserSecurityChecks", "assertSealedPermissionGrant", [OptValue], None, assert_sealed_grant),
        method!("UserSecurityChecks", "assertDocumentExpression", [OptValue], None, assert_document_expression),
        method!(
            "UserSecurityChecks",
            "assertSealedGroupMembership",
            [OptValue, OptValue, OptValue, OptValue],
            None,
            assert_sealed_group_membership
        ),
        method!("UserSecurityChecks", "privilegedGroups", [], None, leaf, privileged_groups),
        method!("UserSecurityChecks", "sealedPermissions", [], None, leaf, sealed_permissions),
        method!("UserSecurityChecks", "endUserSessions", [OptValue, OptValue], None, end_user_sessions),
        method!("UserSecurityChecks", "newDeviceCode", [], None, new_device_code),
        method!("UserSecurityChecks", "redeemDeviceCode", [OptValue], None, redeem_device_code),
        method!("UserSecurityChecks", "displayAccountRefusal", [OptValue], None, display_account_refusal),
        method!("UserSecurityChecks", "currentDevice", [], None, current_device),
    ]],
};

/// An id argument as text, empty when missing, null or undefined (`runtime/ec_extra.rs` `opt_id_text`).
fn id_text(call: &Call<'_>, index: usize) -> String {
    call.opt_text(index).unwrap_or_default()
}

/// `UserSecurityChecks.assertPrivilegedGroupMembership(userGroupId)`, `Security.eecas.xml`'s check before a write of
/// a group membership: a denied check throws, so the write fails.
fn assert_privileged_group(call: &mut Call<'_>) -> Result<Ret, HostError> {
    crate::service::assert_privileged_group_membership(&super::entity_facade()?, &id_text(call, 0)).map_err(lib)?;
    Ok(Ret::Undefined)
}

/// `UserSecurityChecks.assertSealedPermissionGrant(userPermissionId)`.
fn assert_sealed_grant(call: &mut Call<'_>) -> Result<Ret, HostError> {
    crate::service::assert_sealed_permission_grant(&super::entity_facade()?, &id_text(call, 0)).map_err(lib)?;
    Ok(Ret::Undefined)
}

/// `UserSecurityChecks.assertDocumentExpression(fieldPath)`.
fn assert_document_expression(call: &mut Call<'_>) -> Result<Ret, HostError> {
    crate::service::assert_document_expression(&super::entity_facade()?, &id_text(call, 0)).map_err(lib)?;
    Ok(Ret::Undefined)
}

/// `UserSecurityChecks.assertSealedGroupMembership(userGroupId, thruDate, hasStored, storedThruDate)`: the
/// `UserGroupMemberSealed` rule's check (`Security.eecas.xml`); `hasStored` is true for an update that found the stored
/// row, whose end is `storedThruDate`, so a change that only ends the membership sooner passes.
fn assert_sealed_group_membership(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let has_stored = matches!(value(call, 2), Value::Bool(true));
    let stored = value(call, 3);
    let ends_sooner = crate::service::membership_ends_sooner(&value(call, 1), has_stored.then_some(&stored));
    crate::service::assert_sealed_group_membership(&super::entity_facade()?, &id_text(call, 0), ends_sooner)
        .map_err(lib)?;
    Ok(Ret::Undefined)
}

/// `UserSecurityChecks.privilegedGroups()`: the effective privileged group ids, sorted.
fn privileged_groups(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let ids = crate::service::privileged_groups().into_iter().map(Value::String).collect();
    Ok(Ret::from(Value::List(ids)))
}

/// `UserSecurityChecks.sealedPermissions()`: the effective sealed permission ids, sorted.
fn sealed_permissions(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let ids = crate::service::sealed_permissions().into_iter().map(Value::String).collect();
    Ok(Ret::from(Value::List(ids)))
}

/// `UserSecurityChecks.endUserSessions(userId, visitIds)`: end the web sessions of a user, those of the visits named
/// or with none every session and open visit of the user (`user::end_user_sessions`); the number ended. The services
/// that call it check who may first: this decides no authority.
fn end_user_sessions(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let visits = match value(call, 1) {
        Value::Null => None,
        list => Some(
            crate::script::runtime::value_items(&list)
                .unwrap_or_default()
                .iter()
                .map(moqui_lib::object::to_plain_string)
                .filter(|id| !id.trim().is_empty())
                .collect::<Vec<String>>(),
        ),
    };
    // as QuickJS's binding: the message only, a plain `Error`
    let count = crate::user::end_user_sessions(&id_text(call, 0), visits.as_deref())
        .map_err(|err| HostError::Message(err.message().to_string()))?;
    Ok(Ret::from(Value::I64(i64::try_from(count).unwrap_or(i64::MAX))))
}

/// `UserSecurityChecks.newDeviceCode()`: a new shared-device registration code and the hash to store,
/// `{code, codeHash}` (`user::device::new_registration_code`). The caller shows the code once and keeps only the hash.
fn new_device_code(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let (code, hash) =
        crate::user::device::new_registration_code().map_err(|err| HostError::Message(err.message().to_string()))?;
    let mut out = moqui_lib::object::ValueMap::default();
    out.insert("code".to_string(), Value::String(code));
    out.insert("codeHash".to_string(), Value::String(hash));
    Ok(Ret::from(Value::Map(out)))
}

/// `UserSecurityChecks.redeemDeviceCode(code)`: redeem a registration code for the browser of the request under way
/// (`user::device::redeem`): `{redeemed, reasonCode, deviceId, routeGroupId}`. The device cookie goes on the response
/// and is never given to the script. Outside a request nothing is redeemed.
fn redeem_device_code(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let mut out = moqui_lib::object::ValueMap::default();
    let Some(request) = crate::web::current_request() else {
        out.insert("redeemed".to_string(), Value::Bool(false));
        out.insert("reasonCode".to_string(), Value::string("NO_REQUEST"));
        return Ok(Ret::from(Value::Map(out)));
    };
    let outcome = crate::user::device::redeem(
        &id_text(call, 0),
        &crate::web::client_ip(&request),
        request.header("user-agent").unwrap_or(""),
        request.https,
    )
    .map_err(|err| HostError::Message(err.message().to_string()))?;
    match outcome {
        Ok(redeemed) => {
            crate::web::add_header("Set-Cookie", redeemed.set_cookie);
            out.insert("redeemed".to_string(), Value::Bool(true));
            out.insert("deviceId".to_string(), Value::String(redeemed.device_id));
            out.insert("routeGroupId".to_string(), Value::String(redeemed.route_group_id));
        }
        Err(refusal) => {
            out.insert("redeemed".to_string(), Value::Bool(false));
            out.insert("reasonCode".to_string(), Value::string(refusal.code()));
        }
    }
    Ok(Ret::from(Value::Map(out)))
}

/// `UserSecurityChecks.displayAccountRefusal(userId)`: why the account may not serve a shared device, or null
/// (`user::device::display_account_refusal`).
fn display_account_refusal(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let refusal = crate::user::device::display_account_refusal(&super::entity_facade()?, &id_text(call, 0))
        .map_err(|err| HostError::Message(err.message().to_string()))?;
    Ok(Ret::from(refusal.map_or(Value::Null, Value::String)))
}

/// `UserSecurityChecks.currentDevice()`: `{deviceId, routeGroupId}` when the request under way is a shared device's,
/// else null.
fn current_device(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let Some(scope) = crate::user::device::current_scope() else {
        return Ok(Ret::from(Value::Null));
    };
    let mut out = moqui_lib::object::ValueMap::default();
    out.insert("deviceId".to_string(), Value::string(&scope.device_id));
    out.insert("routeGroupId".to_string(), Value::string(&scope.route_group_id));
    Ok(Ret::from(Value::Map(out)))
}

fn value(call: &Call<'_>, index: usize) -> Value {
    call.value_or_null(index)
}

fn lib(err: moqui_lib::error::Error) -> HostError {
    HostError::Lib(err)
}

/// `compare(field, operator, value, toField, format, type)`, as Java's `ObjectUtilities.compare`.
fn compare(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let field = value(call, 0);
    let operator = call.text_or_empty(1);
    let given = value(call, 2);
    let target = if given.is_null() { value(call, 3) } else { given };
    let kind = call.opt_text(5).filter(|name| !name.is_empty() && name != "Object");
    let kind = kind.unwrap_or_else(|| crate::script::runtime::java_type_of(&field).to_string());
    crate::l10n::compare(&field, &operator, &target, &kind)
        .map(Ret::from)
        .map_err(lib)
}

fn whole_of(value: &Value) -> Result<i64, HostError> {
    match value {
        Value::I32(number) => Ok(i64::from(*number)),
        Value::I64(number) => Ok(*number),
        #[allow(clippy::cast_possible_truncation)]
        Value::F64(number) if number.is_finite() => Ok(number.trunc() as i64),
        Value::Decimal(number) => number
            .trunc()
            .to_string()
            .parse()
            .map_err(|_| HostError::message("the number is too large")),
        Value::String(text) => text
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|number| number.is_finite())
            .map(|number| {
                #[allow(clippy::cast_possible_truncation)]
                let whole = number.trunc() as i64;
                whole
            })
            .ok_or_else(|| HostError::Type("Error converting from js 'string' into type 'i64'".to_string())),
        _ => Err(HostError::Type(
            "Error converting from js 'object' into type 'i64'".to_string(),
        )),
    }
}

/// `numberToWords(value, useAnd)` for a whole number.
fn number_to_words(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let whole = whole_of(&value(call, 0))?;
    Ok(moqui_lib::string::number_to_words(whole, call.bool(1).unwrap_or(false)).into())
}

/// `numberToWordsWithDecimal(amount)`: the whole part in words, then `and NN/100` of the amount rounded to cents.
fn number_to_words_with_decimal(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let Some(Value::Decimal(amount)) = crate::script::runtime::decimal_of(&value(call, 0)) else {
        return Err(HostError::message(
            "numberToWordsWithDecimal takes a decimal or a number",
        ));
    };
    let whole: i64 = amount
        .trunc()
        .to_string()
        .parse()
        .map_err(|_| HostError::message("the amount is too large"))?;
    let mut cents = amount.round_dp_with_strategy(2, moqui_lib::format::RoundingStrategy::MidpointAwayFromZero);
    cents.rescale(2);
    let text = cents.to_string();
    let fraction = text.rsplit('.').next().unwrap_or("00");
    Ok(format!(
        "{} and {fraction}/100",
        moqui_lib::string::number_to_words(whole, false)
    )
    .into())
}

/// `__compareOrder(left, right)`: -1, 0 or 1, a null on the right after a value.
fn compare_order(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let (left, right) = (value(call, 0), value(call, 1));
    let ordering = match moqui_lib::object::compare_obj(&left, &right) {
        Ok(ordering) => ordering,
        Err(_) if right.is_null() => Ordering::Less,
        Err(_) => Ordering::Equal,
    };
    Ok(match ordering {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
    .into())
}

/// `__inDateRange(from, thru, valid)`: whether `valid` (now when empty) is in `[from, thru)`.
fn in_date_range(call: &mut Call<'_>) -> Result<Ret, HostError> {
    use crate::script::runtime::{now_timestamp, timestamp_of};
    let from = timestamp_of(&value(call, 0));
    let thru = timestamp_of(&value(call, 1));
    let valid = value(call, 2);
    let valid = if moqui_lib::object::is_empty(&valid) {
        now_timestamp()
    } else {
        timestamp_of(&valid)
    };
    let started = from.is_null()
        || moqui_lib::object::compare_obj(&from, &valid).is_ok_and(|ordering| ordering != Ordering::Greater);
    let open = thru.is_null()
        || moqui_lib::object::compare_obj(&thru, &valid).is_ok_and(|ordering| ordering == Ordering::Greater);
    Ok((started && open).into())
}

/// `System.getProperty(name)`: a property or environment value, null when it is not set.
fn get_property(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let found = moqui_lib::env::get_prop_or_env(&call.text_or_empty(0));
    Ok(Some(found).filter(|value| !value.is_empty()).into())
}

/// Text of an optional argument: empty for null or undefined, a number's or boolean's text.
fn text_or_empty(call: &Call<'_>, index: usize) -> String {
    match call.value(index) {
        None | Some(Value::Null) => String::new(),
        Some(other) => moqui_lib::object::to_plain_string(other),
    }
}

fn encode_html(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(crate::screen::encode_html(&text_or_empty(call, 0)).into())
}

fn encode_html_js_safe(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(crate::script::runtime::encode_html_js_safe(&text_or_empty(call, 0)).into())
}

fn field_values_js_safe(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(match value(call, 0) {
        Value::Map(entries) => crate::script::runtime::js_safe_map(&entries),
        _ => "null".to_string(),
    }
    .into())
}

fn ip_matches(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let patterns = call.opt_text(0).unwrap_or_default();
    let address = call.opt_text(1).unwrap_or_default();
    Ok((patterns.is_empty() || crate::web::ip_matches(&patterns, &address)).into())
}

fn safe_segment(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(call
        .opt_text(0)
        .as_deref()
        .is_some_and(crate::script::runtime::url_safe_chars)
        .into())
}

fn same_origin_redirect(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let configured = match call.value(1) {
        Some(Value::String(text)) => Some(text.clone()),
        _ => None,
    };
    Ok(crate::web::is_same_origin_redirect(&call.opt_text(0).unwrap_or_default(), configured.as_deref()).into())
}

fn last_path_segment(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(call
        .opt_text(0)
        .map(|name| name.replace('\\', "/").rsplit('/').next().unwrap_or("").to_string())
        .into())
}

fn encode_for_xml_attribute(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let text = match call.value(0) {
        Some(Value::String(text)) => text.clone(),
        _ => String::new(),
    };
    Ok(moqui_lib::string::encode_for_xml_attribute(&text, call.bool(1).unwrap_or(false)).into())
}

fn base64_url_encode(call: &mut Call<'_>) -> Result<Ret, HostError> {
    use base64::Engine;
    let text = call.opt_text(0).unwrap_or_default();
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(text.as_bytes())
        .into())
}

fn base64_url_decode(call: &mut Call<'_>) -> Result<Ret, HostError> {
    use base64::Engine;
    let text = call.opt_text(0).unwrap_or_default();
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text.trim_end_matches('='))
        .map_err(|err| HostError::message(format!("not base64url text: {err}")))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned().into())
}

fn clean_html(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(crate::service::clean_safe_html(&call.opt_text(0).unwrap_or_default()).into())
}

fn url_encode_if_needed(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(moqui_lib::string::url_encode_if_needed(&call.opt_text(0).unwrap_or_default()).into())
}

/// The SVG options of argument `index`, the defaults when it is null or undefined.
fn svg_options(call: &Call<'_>, index: usize, what: &str) -> Result<moqui_lib::barcode::SvgOptions, HostError> {
    match call.value(index) {
        None | Some(Value::Null) => Ok(moqui_lib::barcode::SvgOptions::default()),
        Some(options) => crate::script::runtime::code128_options(options)
            .map_err(|message| HostError::message(format!("{what}{message}"))),
    }
}

/// `ec.resource.code128Svg(text, options)`: the Code 128 barcode as an inline `<svg>`; nothing for no text or text Code
/// 128 cannot hold (logged), so the rest of the document still prints.
fn code128_svg(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let text = moqui_lib::object::to_plain_string(&value(call, 0));
    let options = svg_options(call, 1, "code128Svg: ")?;
    if text.is_empty() {
        return Ok(String::new().into());
    }
    Ok(match moqui_lib::barcode::code128(&text) {
        Ok(barcode) => barcode.svg(&options),
        Err(err) => {
            tracing::error!("No Code 128 barcode in the document: {}", err.message());
            String::new()
        }
    }
    .into())
}

/// `ec.resource.barcodeSvg(symbology, text, options)`: strict, for labels.
fn barcode_svg(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let symbology = call.text_or_empty(0);
    let text = moqui_lib::object::to_plain_string(&value(call, 1));
    let options = svg_options(call, 2, "")?;
    moqui_lib::barcode::barcode_svg(&symbology, &text, &options).map(Ret::from).map_err(lib)
}

fn parse_gs1(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let parsed = moqui_lib::barcode::parse_gs1(&call.text_or_empty(0)).map_err(lib)?;
    Ok(Ret::Value(parsed.to_value()))
}

fn gs1_check(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(moqui_lib::barcode::valid_check_digit(&call.text_or_empty(0)).into())
}

/// `ec.resource.template(location, data)`: the file rendered by the renderer for its extension; a string second
/// argument is the default extension.
fn template(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let (data, default_extension) = match call.value(1) {
        Some(Value::String(extension)) => (None, extension.clone()),
        Some(Value::Null) | None => (None, String::new()),
        Some(other) => (Some(other.clone()), String::new()),
    };
    let factory = super::factory()?;
    crate::script::template::TemplateRegistry::of(&factory)
        .render_location_default(&factory, &location, data.as_ref(), &default_extension)
        .map(Ret::from)
        .map_err(HostError::Context)
}

fn template_extensions(_: &mut Call<'_>) -> Result<Ret, HostError> {
    let factory = super::factory()?;
    Ok(crate::script::template::TemplateRegistry::of(&factory).extensions().into())
}

/// `StringUtilities.base64Encode(value)`: base64 of text (its UTF-8) or of bytes as they are; empty for null.
fn base64_encode(call: &mut Call<'_>) -> Result<Ret, HostError> {
    use base64::Engine;
    let bytes = match call.value(0) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Bytes(bytes)) => bytes.clone(),
        Some(other) => moqui_lib::object::to_plain_string(other).into_bytes(),
    };
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes).into())
}

/// `StringUtilities.base64Decode(text)`: the bytes (a `Uint8Array`), white space skipped and the padding optional.
fn base64_decode(call: &mut Call<'_>) -> Result<Ret, HostError> {
    use base64::Engine;
    let compact: String = call
        .opt_text(0)
        .unwrap_or_default()
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(compact.trim_end_matches('='))
        .map_err(|err| HostError::message(format!("not base64 text: {err}")))?;
    Ok(Ret::Value(Value::Bytes(bytes)))
}

fn map_list_values(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let Value::List(items) = value(call, 0) else {
        return Ok(Ret::Value(Value::Null));
    };
    let maps: Vec<moqui_lib::object::ValueMap> = items
        .into_iter()
        .filter_map(|item| match item {
            Value::Map(map) => Some(map),
            _ => None,
        })
        .collect();
    let key = call.text_or_empty(1);
    let values = moqui_lib::collection::get_map_list_values(&maps, &key, call.bool(2).unwrap_or(false));
    Ok(Ret::Value(Value::List(values)))
}

fn paginate_parameters(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let mut entries = match value(call, 2) {
        Value::Map(entries) => entries,
        _ => indexmap::IndexMap::default(),
    };
    let size = call.int(0).unwrap_or(0);
    moqui_lib::collection::paginate_parameters(size, &call.text_or_empty(1), &mut entries).map_err(lib)?;
    Ok(Ret::Value(Value::Map(entries)))
}

fn moqui_version(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(env!("CARGO_PKG_VERSION").to_string().into())
}

/// `ec.factory.hasFeature(name)`: whether this binary has a build feature.
fn has_feature(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(crate::build_info::has_feature(&call.text_or_empty(0)).into())
}

/// The transaction manager of the default entity group.
fn default_transactions() -> Result<crate::transaction::TransactionManager, HostError> {
    let facade = super::entity_facade()?;
    let group = facade.default_group();
    let found = facade
        .groups()
        .find(|(name, _, _)| *name == group)
        .map(|(_, _, transactions)| transactions.clone());
    found.ok_or_else(|| HostError::message(format!("no transaction manager for entity group {group}")))
}

fn in_transaction(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(default_transactions()?.in_transaction().into())
}

fn transaction_cache_active(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(default_transactions()?.transaction_cache_active().into())
}

fn flatten_nested_map(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::Value(crate::script::runtime::flatten_nested_map(&value(call, 0))))
}
