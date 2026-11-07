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
//! `queueMicrotask`, `atob`, `btoa`, `performance`, `DOMException`, `SuppressedError`, `DisposableStack` and
//! `AsyncDisposableStack`, with `Symbol.dispose` and `Symbol.asyncDispose`, made in JavaScript for a whole runtime
//! (`script_engine=oxide` and the spec runtime) before the runtime's own globals are counted, so they are builtins as
//! on QuickJS: writable and configurable, not frozen as the host's globals are. What QuickJS shows of each (names,
//! lengths, descriptors, error names and codes) is what these show; the differences left are in
//! plans/Script-Engine-Oxide.md (`using` declarations, `performance.timeOrigin`). A small script's static check
//! refuses these names, so the small-script runtime does not have them.

use quickjs_oxide::engine::api::core::{Context, RuntimeError};

const QUICKJS_GLOBALS_JS: &str = r#"(function () {
    "use strict";
    const builtin = (name, value, enumerable) => Object.defineProperty(globalThis, name,
        { value, writable: true, enumerable: !!enumerable, configurable: true });
    const hidden = (target, name, value) => Object.defineProperty(target, name,
        { value, writable: true, enumerable: false, configurable: true });
    const tag = (target, name) => Object.defineProperty(target, Symbol.toStringTag,
        { value: name, writable: false, enumerable: false, configurable: true });
    for (const name of ["dispose", "asyncDispose"]) {
        if (typeof Symbol[name] !== "symbol") {
            Object.defineProperty(Symbol, name, { value: Symbol("Symbol." + name), writable: false, enumerable: false,
                configurable: false });
        }
    }
    const callable = (value) => typeof value === "function";

    // DOMException: the WebIDL names with a legacy code, and the code constants on the constructor and its prototype
    const legacy = ["INDEX_SIZE_ERR", "DOMSTRING_SIZE_ERR", "HIERARCHY_REQUEST_ERR", "WRONG_DOCUMENT_ERR",
        "INVALID_CHARACTER_ERR", "NO_DATA_ALLOWED_ERR", "NO_MODIFICATION_ALLOWED_ERR", "NOT_FOUND_ERR", "NOT_SUPPORTED_ERR",
        "INUSE_ATTRIBUTE_ERR", "INVALID_STATE_ERR", "SYNTAX_ERR", "INVALID_MODIFICATION_ERR", "NAMESPACE_ERR",
        "INVALID_ACCESS_ERR", "VALIDATION_ERR", "TYPE_MISMATCH_ERR", "SECURITY_ERR", "NETWORK_ERR", "ABORT_ERR",
        "URL_MISMATCH_ERR", "QUOTA_EXCEEDED_ERR", "TIMEOUT_ERR", "INVALID_NODE_TYPE_ERR", "DATA_CLONE_ERR"];
    const codes = { IndexSizeError: 1, HierarchyRequestError: 3, WrongDocumentError: 4, InvalidCharacterError: 5,
        NoModificationAllowedError: 7, NotFoundError: 8, NotSupportedError: 9, InUseAttributeError: 10,
        InvalidStateError: 11, SyntaxError: 12, InvalidModificationError: 13, NamespaceError: 14, InvalidAccessError: 15,
        TypeMismatchError: 17, SecurityError: 18, NetworkError: 19, AbortError: 20, URLMismatchError: 21,
        QuotaExceededError: 22, TimeoutError: 23, InvalidNodeTypeError: 24, DataCloneError: 25 };
    const exceptions = new WeakMap();
    const slot = (self) => {
        const held = exceptions.get(self);
        if (held === undefined) throw new TypeError("not a DOMException");
        return held;
    };
    class DOMException {
        constructor(message = "", name = "Error") {
            const text = String(message), named = String(name);
            exceptions.set(this, { message: text, name: named, code: Object.hasOwn(codes, named) ? codes[named] : 0 });
            const stack = new Error().stack;
            if (stack !== undefined) hidden(this, "stack", stack);
        }
        get name() { return slot(this).name; }
        get message() { return slot(this).message; }
        get code() { return slot(this).code; }
    }
    Object.setPrototypeOf(DOMException.prototype, Error.prototype);
    legacy.forEach((constant, index) => {
        for (const target of [DOMException, DOMException.prototype]) {
            Object.defineProperty(target, constant, { value: index + 1, writable: false, enumerable: true,
                configurable: false });
        }
    });
    tag(DOMException.prototype, "DOMException");
    builtin("DOMException", DOMException);

    // SuppressedError(error, suppressed, message), callable without `new` as the native errors are
    const SuppressedError = function SuppressedError(error, suppressed, message) {
        const made = Reflect.construct(Error, message === undefined ? [] : [message], new.target || SuppressedError);
        hidden(made, "error", error);
        hidden(made, "suppressed", suppressed);
        return made;
    };
    Object.setPrototypeOf(SuppressedError, Error);
    SuppressedError.prototype = Object.create(Error.prototype);
    Object.defineProperty(SuppressedError, "prototype", { writable: false, enumerable: false, configurable: false });
    hidden(SuppressedError.prototype, "name", "SuppressedError");
    hidden(SuppressedError.prototype, "message", "");
    hidden(SuppressedError.prototype, "constructor", SuppressedError);
    builtin("SuppressedError", SuppressedError);

    // DisposableStack and AsyncDisposableStack (explicit resource management): resources disposed last first, a later
    // error wrapping the earlier one as its `suppressed`
    const stacks = new WeakMap();
    const live = (self, kind) => {
        const state = stacks.get(self);
        if (state === undefined || state.kind !== kind) throw new TypeError("not a " + kind);
        if (state.disposed) throw new ReferenceError(kind + " has been disposed");
        return state;
    };
    const disposer = (value, symbol, fallback) => {
        if (value === null || typeof value !== "object" && typeof value !== "function") throw new TypeError("not an object");
        let method = value[symbol];
        if (method === undefined && fallback !== undefined) {
            const sync = value[fallback];
            if (callable(sync)) method = function () { sync.call(this); };
        }
        if (!callable(method)) throw new TypeError("not a function");
        return () => method.call(value);
    };
    const define = (kind, symbol, fallback, disposeName, dispose) => {
        const made = {
            [kind]: class {
                constructor() { stacks.set(this, { kind, disposed: false, resources: [] }); }
                get disposed() {
                    const state = stacks.get(this);
                    if (state === undefined || state.kind !== kind) throw new TypeError("not a " + kind);
                    return state.disposed;
                }
                use(value) {
                    const state = live(this, kind);
                    if (value !== null && value !== undefined) state.resources.push(disposer(value, symbol, fallback));
                    return value;
                }
                adopt(value, onDispose) {
                    const state = live(this, kind);
                    if (!callable(onDispose)) throw new TypeError("not a function");
                    state.resources.push(() => onDispose(value));
                    return value;
                }
                defer(onDispose) {
                    const state = live(this, kind);
                    if (!callable(onDispose)) throw new TypeError("not a function");
                    state.resources.push(() => onDispose());
                }
                move() {
                    const state = live(this, kind);
                    const moved = new made();
                    stacks.get(moved).resources = state.resources;
                    state.resources = [];
                    state.disposed = true;
                    return moved;
                }
            }
        }[kind];
        const method = dispose(made);
        Object.defineProperty(method, "name", { value: disposeName, configurable: true });
        hidden(made.prototype, disposeName, method);
        hidden(made.prototype, symbol, method);
        tag(made.prototype, kind);
        builtin(kind, made);
    };
    // the resources of `state` disposed in turn by `run` (which awaits each for the async stack)
    const taken = (self, kind) => {
        const state = stacks.get(self);
        if (state === undefined || state.kind !== kind) throw new TypeError("not a " + kind);
        if (state.disposed) return null;
        state.disposed = true;
        const resources = state.resources;
        state.resources = [];
        return resources;
    };
    const failed = (thrown, earlier) => (earlier.has ? new SuppressedError(thrown, earlier.error) : thrown);
    define("DisposableStack", Symbol.dispose, undefined, "dispose", () => function () {
        const resources = taken(this, "DisposableStack");
        if (resources === null) return undefined;
        const earlier = { has: false, error: undefined };
        for (let index = resources.length - 1; index >= 0; index--) {
            try { resources[index](); } catch (thrown) { earlier.error = failed(thrown, earlier); earlier.has = true; }
        }
        if (earlier.has) throw earlier.error;
        return undefined;
    });
    define("AsyncDisposableStack", Symbol.asyncDispose, Symbol.dispose, "disposeAsync", () => async function () {
        const resources = taken(this, "AsyncDisposableStack");
        if (resources === null) return undefined;
        const earlier = { has: false, error: undefined };
        for (let index = resources.length - 1; index >= 0; index--) {
            try { await resources[index](); } catch (thrown) { earlier.error = failed(thrown, earlier); earlier.has = true; }
        }
        if (earlier.has) throw earlier.error;
        return undefined;
    });

    // queueMicrotask, atob and btoa: methods, so neither has a prototype or constructs, as QuickJS's
    const BASE64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    const invalid = (message) => new DOMException(message, "InvalidCharacterError");
    const functions = {
        queueMicrotask(callback) {
            if (!callable(callback)) throw new TypeError("not a function");
            Promise.resolve().then(() => { callback(); });
        },
        btoa(data) {
            const text = String(data);
            let out = "";
            for (let index = 0; index < text.length; index += 3) {
                const codes = [0, 1, 2].map((at) => (index + at < text.length ? text.charCodeAt(index + at) : -1));
                if (codes.some((code) => code > 255)) throw invalid("String contains an invalid character");
                const [a, b, c] = codes;
                out += BASE64[a >> 2] + BASE64[((a & 3) << 4) | (b < 0 ? 0 : b >> 4)]
                    + (b < 0 ? "=" : BASE64[((b & 15) << 2) | (c < 0 ? 0 : c >> 6)]) + (c < 0 ? "=" : BASE64[c & 63]);
            }
            return out;
        },
        atob(data) {
            let text = String(data).replace(/[\t\n\f\r ]/g, "");
            if (text.length % 4 === 0) text = text.replace(/==?$/, "");
            if (text.length % 4 === 1 || /[^A-Za-z0-9+/]/.test(text)) {
                throw invalid("The string to be decoded is not correctly encoded");
            }
            let out = "", bits = 0, count = 0;
            for (const char of text) {
                bits = (bits << 6) | BASE64.indexOf(char);
                count += 6;
                if (count >= 8) {
                    count -= 8;
                    out += String.fromCharCode((bits >> count) & 255);
                }
            }
            return out;
        }
    };
    for (const name of ["queueMicrotask", "atob", "btoa"]) builtin(name, functions[name]);

    // performance: now() in milliseconds since the runtime started, and timeOrigin, its start as epoch milliseconds
    const origin = Date.now();
    builtin("performance", { now: { now() { return Date.now() - origin; } }.now, timeOrigin: origin }, true);
})()"#;

/// Make the QuickJS-only builtins on a whole runtime's global object, before its own globals are counted.
pub(super) fn install(context: &Context) -> Result<(), RuntimeError> {
    context.eval(QUICKJS_GLOBALS_JS)?;
    Ok(())
}
