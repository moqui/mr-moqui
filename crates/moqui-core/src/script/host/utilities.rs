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

//! The rest of the static methods of Java's `StringUtilities`, `ObjectUtilities`, `CollectionUtilities` and
//! `WebUtilities` that a ported script calls, written in JavaScript over what the host already has
//! (docs/dev/JS-Globals.md). The globals shim runs [`utilities_js`] just before it freezes the helper objects
//! (`globals.rs`), so these are frozen with the rest. The bare helper functions (`compare`, `orderMapList`, ...) are
//! also put on the Java class that has each; some are made after this runs, so they are read when called.

/// `(function (global, helpers) { ... })`: the additions to the four helper objects.
macro_rules! utilities_js {
    () => {
        r#"(function (global, helpers) {
    const su = helpers.StringUtilities, ou = helpers.ObjectUtilities, cu = helpers.CollectionUtilities, wu = helpers.WebUtilities;
    // a bare helper function by name, read when it is called
    const bare = (name) => (...args) => global[name](...args);
    const ec = global.ec;
    const text = (value) => value == null ? "" : String(value);
    const empty = (value) => value == null || value === "" || (Array.isArray(value) && value.length === 0)
        || (value instanceof Set && value.size === 0) || (value instanceof Map && value.size === 0)
        || (typeof value === "object" && !(value instanceof Date) && typeof value.length === "number" && value.length === 0)
        || (typeof value === "object" && Object.getPrototypeOf(value) === Object.prototype && Object.keys(value).length === 0);

    // ---- StringUtilities ----
    // paddedNumber(number, length): the whole number, zeros before it up to the length
    su.paddedNumber = (number, desiredLength) => {
        const digits = text(Math.trunc(Number(number)));
        return digits.length >= desiredLength ? digits : "0".repeat(desiredLength - digits.length) + digits;
    };
    // paddedString(input, length, rightPad) and paddedString(input, length, padChar, rightPad): cut or padded to the length
    su.paddedString = (input, desiredLength, third, fourth) => {
        const padChar = typeof third === "string" && third.length ? third[0] : " ";
        const rightPad = typeof third === "boolean" ? third : fourth === true;
        const value = text(input);
        if (desiredLength == null || desiredLength <= 0) return value;
        if (value.length >= desiredLength) return value.substring(0, desiredLength);
        const fill = padChar.repeat(desiredLength - value.length);
        return rightPad ? value + fill : fill + value;
    };
    su.removeNonAlphaNumeric = (original) => original == null ? null : String(original).replace(/[^\p{L}\p{Nd}]/gu, "");
    su.replaceNonAlphaNumeric = (original, replacement) => original == null ? null
        : String(original).replace(/[^\p{L}\p{Nd}]/gu, text(replacement));
    su.isAlphaNumeric = (value, also) => {
        const others = text(also);
        for (const ch of text(value)) if (!/[\p{L}\p{Nd}]/u.test(ch) && !others.includes(ch)) return false;
        return true;
    };
    su.findFirstNumber = (original) => {
        const found = /\d+/.exec(text(original));
        return found ? found[0] : null;
    };
    su.getRandomString = (length) => ec.user.randomString(Number(length));
    su.pathStringToList = (path, skipSegments) => text(path).split("/").filter((part) => part !== "").slice(Number(skipSegments ?? 0));
    su.decodeFromXml = (original) => original == null ? null : String(original).replace(/&lt;/g, "<").replace(/&gt;/g, ">")
        .replace(/&quot;/g, '"').replace(/&apos;/g, "'").replace(/&amp;/g, "&");
    su.escapeElasticQueryString = (queryString) => text(queryString).replace(/([+\-=!(){}\[\]^"~*?:\\\/]|&&|\|\|)/g, "\\$1");
    su.toStringCleanBom = (value) => { const cleaned = text(value); return cleaned.charCodeAt(0) === 0xFEFF ? cleaned.substring(1) : cleaned; };
    su.elasticQueryAutoWildcard = bare("elasticQueryAutoWildcard");
    su.numberToWords = bare("numberToWords");
    su.numberToWordsWithDecimal = bare("numberToWordsWithDecimal");

    // ---- ObjectUtilities ----
    // isEmpty(value): null, empty text, an empty list, set or map
    ou.isEmpty = empty;
    ou.compare = bare("compare");
    ou.basicConvert = bare("basicConvert");
    // compareLike(value, pattern): SQL LIKE, `%` any run and `_` any one character
    ou.compareLike = (value, pattern) => {
        if (value == null || pattern == null) return false;
        const source = String(pattern).replace(/[.*+?^${}()|[\]\\]/g, "\\$&").replace(/%/g, ".*").replace(/_/g, ".");
        return new RegExp("^" + source + "$", "s").test(String(value));
    };
    // countChars(text, digits, letters, other): how many of its characters are of the kinds asked for
    ou.countChars = (value, countDigits, countLetters, countOther) => {
        let count = 0;
        for (const ch of text(value)) {
            if (/\p{Nd}/u.test(ch)) { if (countDigits) count++; }
            else if (/\p{L}/u.test(ch)) { if (countLetters) count++; }
            else if (countOther) count++;
        }
        return count;
    };
    ou.toPlainString = (value) => value == null ? null : typeof value.toPlainString === "function" ? value.toPlainString() : String(value);
    // addNumbers(a, b): decimals when either is one, else numbers
    ou.addNumbers = (left, right) => {
        if (left == null) return right ?? null;
        if (right == null) return left;
        if (typeof left === "number" && typeof right === "number") return left + right;
        return global.toDecimal(left).add(global.toDecimal(right));
    };

    // ---- CollectionUtilities ----
    cu.filterMapList = bare("filterMapList");
    cu.filterMapListByDate = bare("filterMapListByDate");
    // addToBigDecimalInMap(key, value, map): the decimal under the key, plus the value
    cu.addToBigDecimalInMap = (key, value, theMap) => {
        if (value == null || theMap == null) return;
        const held = theMap[key];
        theMap[key] = held == null ? global.toDecimal(value) : global.toDecimal(held).add(global.toDecimal(value));
    };
    cu.addToListInMap = (key, value, theMap) => { if (theMap != null) (theMap[key] ||= []).push(value); };
    cu.addToSetInMap = (key, value, theMap) => { if (theMap != null) (theMap[key] ||= new Set()).add(value); };
    cu.addToMapInMap = (keyOuter, keyInner, value, theMap) => { if (theMap != null) (theMap[keyOuter] ||= {})[keyInner] = value; };
    cu.addToBigDecimalInMapInMap = (keyOuter, keyInner, value, theMap) => {
        if (theMap != null) cu.addToBigDecimalInMap(keyInner, value, theMap[keyOuter] ||= {});
    };
    // removeNullsFromMap(map): the null entries removed, in nested maps and lists too; the map itself
    cu.removeNullsFromMap = (theMap) => {
        if (theMap == null) return theMap;
        for (const key of Object.keys(theMap)) {
            const value = theMap[key];
            if (value == null) delete theMap[key];
            else if (Array.isArray(value)) value.forEach((item) => { if (item && typeof item === "object") cu.removeNullsFromMap(item); });
            else if (typeof value === "object" && Object.getPrototypeOf(value) === Object.prototype) cu.removeNullsFromMap(value);
        }
        return theMap;
    };
    // mergeNestedMap(base, overrides, overrideEmpty): the overrides put into base, nested maps merged and lists joined
    cu.mergeNestedMap = (baseMap, overrideMap, overrideEmpty) => {
        if (baseMap == null || overrideMap == null) return;
        for (const [key, value] of Object.entries(overrideMap)) {
            const held = baseMap[key];
            if (held == null) { if (overrideEmpty || !empty(value)) baseMap[key] = value; continue; }
            const plain = (item) => item !== null && typeof item === "object" && Object.getPrototypeOf(item) === Object.prototype;
            if (plain(held) && plain(value)) cu.mergeNestedMap(held, value, overrideEmpty);
            else if (Array.isArray(held) && Array.isArray(value)) held.push(...value);
            else if (overrideEmpty || !empty(value)) baseMap[key] = value;
        }
    };
    cu.toHashMap = (theMap) => Object.assign({}, theMap || {});
    // mapMatchesFields(map, fields): each entry of fields equal to the map's
    cu.mapMatchesFields = (baseMap, compareMap) => {
        for (const [key, value] of Object.entries(compareMap || {})) {
            if (global.compare(baseMap == null ? null : baseMap[key], "equals", null, value, null, "Object") !== true) return false;
        }
        return true;
    };
    // findFieldNestedMap(key, map): the first value under the key, in the map or a map or list nested in it
    cu.findFieldNestedMap = (key, theMap) => {
        if (theMap == null || typeof theMap !== "object") return null;
        if (!Array.isArray(theMap) && theMap[key] != null) return theMap[key];
        for (const value of Array.isArray(theMap) ? theMap : Object.values(theMap)) {
            if (value !== null && typeof value === "object") {
                const found = cu.findFieldNestedMap(key, value);
                if (found != null) return found;
            }
        }
        return null;
    };

    // ---- WebUtilities ----
    // simpleHttpStringRequest(location, body, contentType): the response text of a POST of the body, or of a GET with none
    wu.simpleHttpStringRequest = (location, requestBody, contentType) => {
        const client = ec.service.rest().uri(String(location)).method(empty(requestBody) ? "GET" : "POST");
        if (contentType) client.contentType(String(contentType));
        if (!empty(requestBody)) client.text(String(requestBody));
        return client.call().text();
    };
    // simpleHttpMapRequest(location, parameters): the response text of a POST of the parameters as a form
    wu.simpleHttpMapRequest = (location, requestMap) => {
        const body = Object.entries(requestMap || {}).filter(([, value]) => value != null)
            .map(([name, value]) => encodeURIComponent(name) + "=" + encodeURIComponent(String(value))).join("&");
        return ec.service.rest().uri(String(location)).method("POST").contentType("application/x-www-form-urlencoded")
            .text(body).call().text();
    };
})"#
    };
}
pub(super) use utilities_js;
