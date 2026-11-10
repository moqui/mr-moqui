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

//! The globals QuickJS's main context has that quickjs-oxide does not (`check::QUICKJS_ONLY_GLOBALS`' first group):
//! `queueMicrotask`, `atob`, `btoa` and `DOMException`, made in JavaScript for a whole runtime
//! (`script_engine=oxide` and the spec runtime) before the runtime's own globals are counted, so they are builtins as
//! on QuickJS: writable and configurable, not frozen as the host's globals are. What QuickJS shows of each (names,
//! lengths, descriptors, error names and codes) is what these show; `performance` is the fork's
//! (`Context::install_performance`), and so are explicit resource management's (`using`, `SuppressedError`,
//! `DisposableStack`, `AsyncDisposableStack`, `Symbol.dispose`, `Symbol.asyncDispose`). A small script's static check
//! refuses these names, so the small-script runtime does not have them. What they call when they run is taken when
//! they are made, so a script that later changes a builtin's prototype does not change them or reach their state.

use quickjs_oxide::engine::api::core::{Context, RuntimeError};

const QUICKJS_GLOBALS_JS: &str = r#"(function () {
    "use strict";
    // What the functions below call when they run, taken now: a script that later changes a builtin (an
    // `Array.prototype` setter, `Object.prototype.get`, `WeakMap.prototype.get`, `Promise.prototype.then`) does not
    // change what they do or reach their state. Descriptors have no prototype, and lists grow by definition.
    const { apply } = Reflect;
    const { defineProperty, setPrototypeOf } = Object;
    const OurError = Error, OurTypeError = TypeError, OurWeakMap = WeakMap;
    const toText = String, fromCharCode = String.fromCharCode;
    const uncurry = (method) => (self, a, b) => apply(method, self, [a, b]);
    const charCodeAt = uncurry(String.prototype.charCodeAt);
    const weakGet = uncurry(WeakMap.prototype.get), weakSet = uncurry(WeakMap.prototype.set);
    const data = (value, writable, enumerable, configurable) => ({ __proto__: null, value, writable, enumerable,
        configurable });
    const append = (list, value) => { defineProperty(list, list.length, data(value, true, true, true)); };
    const builtin = (name, value, enumerable) => defineProperty(globalThis, name, data(value, true, !!enumerable, true));
    const hidden = (target, name, value) => defineProperty(target, name, data(value, true, false, true));
    const tag = (target, name) => defineProperty(target, Symbol.toStringTag, data(name, false, false, true));
    const callable = (value) => typeof value === "function";

    // DOMException: the WebIDL names with a legacy code, and the code constants on the constructor and its prototype
    const legacy = ["INDEX_SIZE_ERR", "DOMSTRING_SIZE_ERR", "HIERARCHY_REQUEST_ERR", "WRONG_DOCUMENT_ERR",
        "INVALID_CHARACTER_ERR", "NO_DATA_ALLOWED_ERR", "NO_MODIFICATION_ALLOWED_ERR", "NOT_FOUND_ERR", "NOT_SUPPORTED_ERR",
        "INUSE_ATTRIBUTE_ERR", "INVALID_STATE_ERR", "SYNTAX_ERR", "INVALID_MODIFICATION_ERR", "NAMESPACE_ERR",
        "INVALID_ACCESS_ERR", "VALIDATION_ERR", "TYPE_MISMATCH_ERR", "SECURITY_ERR", "NETWORK_ERR", "ABORT_ERR",
        "URL_MISMATCH_ERR", "QUOTA_EXCEEDED_ERR", "TIMEOUT_ERR", "INVALID_NODE_TYPE_ERR", "DATA_CLONE_ERR"];
    const codes = { __proto__: null, IndexSizeError: 1, HierarchyRequestError: 3, WrongDocumentError: 4,
        InvalidCharacterError: 5, NoModificationAllowedError: 7, NotFoundError: 8, NotSupportedError: 9,
        InUseAttributeError: 10, InvalidStateError: 11, SyntaxError: 12, InvalidModificationError: 13, NamespaceError: 14,
        InvalidAccessError: 15, TypeMismatchError: 17, SecurityError: 18, NetworkError: 19, AbortError: 20,
        URLMismatchError: 21, QuotaExceededError: 22, TimeoutError: 23, InvalidNodeTypeError: 24, DataCloneError: 25 };
    const exceptions = new OurWeakMap();
    const slot = (self) => {
        const held = weakGet(exceptions, self);
        if (held === undefined) throw new OurTypeError("not a DOMException");
        return held;
    };
    class DOMException {
        constructor(message = "", name = "Error") {
            const text = toText(message), named = toText(name);
            weakSet(exceptions, this, { __proto__: null, message: text, name: named, code: codes[named] || 0 });
            const stack = new OurError().stack;
            if (stack !== undefined) hidden(this, "stack", stack);
        }
        get name() { return slot(this).name; }
        get message() { return slot(this).message; }
        get code() { return slot(this).code; }
    }
    setPrototypeOf(DOMException.prototype, Error.prototype);
    legacy.forEach((constant, index) => {
        for (const target of [DOMException, DOMException.prototype]) defineProperty(target, constant, data(index + 1, false, true, false));
    });
    tag(DOMException.prototype, "DOMException");
    builtin("DOMException", DOMException);

    // queueMicrotask, atob and btoa: methods, so neither has a prototype or constructs, as QuickJS's. The microtask
    // runs one job later through an await, which reads no `then` or `constructor` a script could change.
    const later = async (callback) => { await undefined; callback(); };
    const BASE64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    const encode = [], decode = [];
    for (let index = 0; index < 128; index++) decode[index] = -1;
    for (let index = 0; index < 64; index++) {
        encode[index] = BASE64[index];
        decode[BASE64.charCodeAt(index)] = index;
    }
    const invalid = (message) => new DOMException(message, "InvalidCharacterError");
    const space = (code) => code === 9 || code === 10 || code === 12 || code === 13 || code === 32;
    const functions = {
        queueMicrotask(callback) {
            if (!callable(callback)) throw new OurTypeError("not a function");
            later(callback);
        },
        btoa(input) {
            const text = toText(input);
            const length = text.length;
            for (let index = 0; index < length; index++) {
                if (charCodeAt(text, index) > 255) throw invalid("String contains an invalid character");
            }
            let out = "";
            for (let index = 0; index < length; index += 3) {
                const a = charCodeAt(text, index);
                const b = index + 1 < length ? charCodeAt(text, index + 1) : -1;
                const c = index + 2 < length ? charCodeAt(text, index + 2) : -1;
                out += encode[a >> 2] + encode[((a & 3) << 4) | (b < 0 ? 0 : b >> 4)]
                    + (b < 0 ? "=" : encode[((b & 15) << 2) | (c < 0 ? 0 : c >> 6)]) + (c < 0 ? "=" : encode[c & 63]);
            }
            return out;
        },
        atob(input) {
            const text = toText(input);
            // the codes without ASCII whitespace, then one or two `=` off the end of a length that is a multiple of 4
            const kept = [];
            for (let index = 0; index < text.length; index++) {
                const code = charCodeAt(text, index);
                if (!space(code)) append(kept, code);
            }
            let length = kept.length;
            if (length % 4 === 0 && length > 0 && kept[length - 1] === 61) {
                length--;
                if (kept[length - 1] === 61) length--;
            }
            let bad = length % 4 === 1;
            for (let index = 0; index < length && !bad; index++) bad = kept[index] > 127 || decode[kept[index]] < 0;
            if (bad) throw invalid("The string to be decoded is not correctly encoded");
            let out = "", bits = 0, count = 0;
            for (let index = 0; index < length; index++) {
                bits = ((bits << 6) | decode[kept[index]]) & 0xffffff;
                count += 6;
                if (count >= 8) {
                    count -= 8;
                    out += fromCharCode((bits >> count) & 255);
                }
            }
            return out;
        }
    };
    for (const name of ["queueMicrotask", "atob", "btoa"]) builtin(name, functions[name]);
})()"#;

/// Make the QuickJS-only builtins on a whole runtime's global object, before its own globals are counted.
pub(super) fn install(context: &Context) -> Result<(), RuntimeError> {
    // the fork's own, as the High Resolution Time standard has it: now() in fractional milliseconds on a monotonic
    // clock, and timeOrigin the epoch time at which it read 0
    context.install_performance()?;
    super::compiled::eval_start(&mut context.clone(), QUICKJS_GLOBALS_JS)?;
    Ok(())
}
