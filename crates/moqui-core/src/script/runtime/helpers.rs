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

//! The helper functions every script runtime installs, and the values they hand out.

use super::*;

pub(crate) const HELPERS_JS: &str = r#"
(function () {
    function field(item, name) {
        if (item == null) return null;
        if (typeof item.get === "function") return item.get(name);
        return item[name];
    }
    globalThis.orderMapList = function (list, fields) {
        if (list == null) return;
        var specs = fields.map(function (text) {
            var name = String(text), desc = false;
            if (name.charAt(0) === "-") { desc = true; name = name.substring(1); }
            else if (name.charAt(0) === "+") name = name.substring(1);
            return { name: name, desc: desc };
        });
        list.sort(function (a, b) {
            for (var i = 0; i < specs.length; i++) {
                var c = __compareOrder(field(a, specs[i].name), field(b, specs[i].name));
                if (c !== 0) return specs[i].desc ? -c : c;
            }
            return 0;
        });
    };
    // keep the maps that match every value, or with `exclude` remove them
    globalThis.filterMapList = function (list, values, exclude) {
        if (list == null || values == null || Object.keys(values).length === 0) return;
        for (var i = list.length - 1; i >= 0; i--) {
            var matches = true;
            for (var key in values) {
                if (!compare(field(list[i], key), "equals", null, values[key], null, "Object")) {
                    matches = false;
                    break;
                }
            }
            if (matches === !!exclude) list.splice(i, 1);
        }
    };
    globalThis.filterMapListByDate = function (list, fromName, thruName, valid, ignoreIfEmpty) {
        if (list == null) return;
        if (ignoreIfEmpty && (valid == null || valid === "")) return;
        for (var i = list.length - 1; i >= 0; i--) {
            if (!__inDateRange(field(list[i], fromName), field(list[i], thruName), valid)) list.splice(i, 1);
        }
    };
    // `StringUtilities.elasticQueryAutoWildcard`: a trailing `*` on each plain word so partial words match, and with
    // `allFieldPrefix` a `*:` before it. AND, OR, NOT and words with query syntax characters are left as they are.
    var plainWord = /^[^*:\\?_~\/.\[\]{}+><="^-]*$/;
    // `toNumber(x)`: a JavaScript number from a decimal, a number or numeric text, and null from null, as a document
    // expression that works in numbers wants; a decimal does not convert itself, so arithmetic on one is an error.
    globalThis.toNumber = function (value) {
        if (value == null || value === "") return null;
        if (typeof value === "number") return value;
        return toDecimal(value).toNumber();
    };
    globalThis.elasticQueryAutoWildcard = function (query, allFieldPrefix) {
        if (query == null) return "*";
        var text = String(query).trim();
        if (text.length === 0) return "*";
        var out = [];
        text.split(" ").forEach(function (word) {
            var term = word.trim();
            if (term.length === 0) return;
            if (term === "AND" || term === "OR" || term === "NOT" || !plainWord.test(term)) out.push(term);
            else out.push((allFieldPrefix ? "*:" : "") + term + "*");
        });
        return out.join(" ");
    };
})();
"#;

/// Java type name of a value, so a string on the other side of a compare converts to it.
pub(crate) fn java_type_of(value: &Value) -> &'static str {
    match value {
        Value::String(_) => "String",
        Value::Bool(_) => "Boolean",
        Value::I32(_) => "Integer",
        Value::I64(_) => "Long",
        Value::F64(_) => "Double",
        Value::Decimal(_) => "BigDecimal",
        Value::Timestamp(_) => "Timestamp",
        Value::Date(_) => "Date",
        Value::Time(_) => "Time",
        _ => "Object",
    }
}

/// A time stamp from a script's value: epoch millis, a time stamp, or text, which is in the session time zone unless it
/// has an offset.
pub(crate) fn timestamp_of(value: &Value) -> Value {
    crate::l10n::basic_convert(value, "Timestamp").value
}

/// `ec.user.nowTimestamp`: the context's effective time, or the clock.
pub(crate) fn now_timestamp() -> Value {
    timestamp_of(&Value::I64(crate::user::now_effective_millis()))
}
