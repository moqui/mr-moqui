// The assert of a spec and of an in-page test (plans/archive/NodeDependency.md). Loaded after the core prelude, which sets
// __moquiTest.ConditionNotSatisfied, display and truthy. __moquiTestHost.settle, installed before a spec runs, runs a
// promise's jobs in QuickJS. In a page there is no settle: rejects returns a promise, and the test returns or awaits it.
(function (g) {
    const T = g.__moquiTest;
    class Fallback extends Error {
        constructor(message, line) {
            super(message);
            this.name = "ConditionNotSatisfied";
            this.line = line;
        }
    }
    const ConditionNotSatisfied = (T && T.ConditionNotSatisfied) || Fallback;
    const truthy = (value) => (T && T.truthy) ? T.truthy(value) : value != null && value !== false && value !== 0 && value !== "";

    // Own keys, so a value of undefined stays visible. Host values (a decimal) go through the prelude's display.
    const shown = (value, depth, seen) => {
        const at = depth || 0;
        if (at > 4) return "...";
        if (value instanceof Error) return `${value.name}: ${value.message}`;
        if (value === undefined) return "undefined";
        if (value === null) return "null";
        if (typeof value === "number") return Object.is(value, -0) ? "-0" : String(value);
        if (typeof value === "string") return JSON.stringify(value);
        if (typeof value === "boolean") return String(value);
        if (value instanceof RegExp) return String(value);
        if (value instanceof Date) return Number.isNaN(value.getTime()) ? "Invalid Date" : `Date(${value.toISOString()})`;
        if (value && typeof value === "object") {
            if (seen && seen.has(value)) return "...";
            const proto = Object.getPrototypeOf(value);
            if (Array.isArray(value) || proto === Object.prototype || proto === null) {
                const next = seen || new Set();
                next.add(value);
                if (Array.isArray(value)) return `[${value.map((item) => shown(item, at + 1, next)).join(", ")}]`;
                const body = Object.keys(value).map((key) => `${key}: ${shown(value[key], at + 1, next)}`).join(", ");
                return `{${body}}`;
            }
        }
        if (T && T.display) return T.display(value, 200);
        try { return JSON.stringify(value); } catch (e) { return String(value); }
    };

    const assertion = (what, lines, message) => {
        const body = [`Assertion failed: ${what}`, "", ...lines];
        if (message != null && message !== "") body.push("", String(message));
        throw new ConditionNotSatisfied(body.join("\n"), null);
    };

    // Groovy's assert: a condition anywhere, such as in a loop, where statements are not rewritten.
    function assert(condition, message) {
        if (!truthy(condition)) {
            throw new ConditionNotSatisfied(`Assertion failed${message == null ? "" : ": " + message}` +
                `\n\nvalue: ${shown(condition)}`, null);
        }
        return true;
    }

    assert.equal = (actual, expected, message) => {
        if (!Object.is(actual, expected)) assertion("assert.equal", [`actual:   ${shown(actual)}`, `expected: ${shown(expected)}`], message);
        return true;
    };
    assert.notEqual = (actual, expected, message) => {
        if (Object.is(actual, expected)) assertion("assert.notEqual", [`both: ${shown(actual)}`], message);
        return true;
    };
    assert.ok = (value, message) => {
        if (!truthy(value)) assertion("assert.ok", [`value: ${shown(value)}`], message);
        return true;
    };
    assert.fail = (message) => assertion("assert.fail", [message == null || message === "" ? "failed" : String(message)]);

    const same = (actual, expected, seen) => {
        if (Object.is(actual, expected)) return true;
        if (typeof actual !== "object" || typeof expected !== "object" || actual === null || expected === null) return false;
        if (seen.has(actual)) return seen.get(actual) === expected;
        seen.set(actual, expected);
        if (actual instanceof Date || expected instanceof Date) {
            return actual instanceof Date && expected instanceof Date && Object.is(actual.getTime(), expected.getTime());
        }
        if (actual instanceof RegExp || expected instanceof RegExp) {
            return actual instanceof RegExp && expected instanceof RegExp && actual.source === expected.source && actual.flags === expected.flags;
        }
        const actualList = Array.isArray(actual), expectedList = Array.isArray(expected);
        if (actualList || expectedList) {
            if (!actualList || !expectedList || actual.length !== expected.length) return false;
            for (let i = 0; i < actual.length; i++) if (!same(actual[i], expected[i], seen)) return false;
            return true;
        }
        if (Object.getPrototypeOf(actual) !== Object.getPrototypeOf(expected)) return false;
        const actualKeys = Object.keys(actual), expectedKeys = Object.keys(expected);
        if (actualKeys.length !== expectedKeys.length) return false;
        for (const key of actualKeys) {
            if (!Object.prototype.hasOwnProperty.call(expected, key) || !same(actual[key], expected[key], seen)) return false;
        }
        return true;
    };
    assert.deepEqual = (actual, expected, message) => {
        if (!same(actual, expected, new Map())) {
            assertion("assert.deepEqual", [`actual:   ${shown(actual)}`, `expected: ${shown(expected)}`], message);
        }
        return true;
    };

    const patternOf = (value, what, message) => {
        if (!(value instanceof RegExp)) assertion(what, ["the pattern is a regular expression", `got: ${shown(value)}`], message);
        value.lastIndex = 0;
        return value;
    };
    assert.match = (value, pattern, message) => {
        if (typeof value !== "string") assertion("assert.match", ["the value is a string", `got: ${shown(value)}`], message);
        const re = patternOf(pattern, "assert.match", message);
        if (!re.test(value)) assertion("assert.match", [`value:   ${shown(value)}`, `pattern: ${re}`], message);
        return true;
    };
    assert.doesNotMatch = (value, pattern, message) => {
        if (typeof value !== "string") assertion("assert.doesNotMatch", ["the value is a string", `got: ${shown(value)}`], message);
        const re = patternOf(pattern, "assert.doesNotMatch", message);
        if (re.test(value)) assertion("assert.doesNotMatch", [`value:   ${shown(value)}`, `pattern: ${re}`], message);
        return true;
    };

    // A thrown error matches no expectation, a regular expression (its message), an error constructor, or a function
    // that returns true for the error (the node:assert validator).
    const isErrorClass = (expected) => typeof expected === "function" &&
        (expected === Error || Error.isPrototypeOf(expected));
    const matchesThrown = (error, expected) => {
        if (expected == null) return true;
        if (expected instanceof RegExp) return expected.test(error instanceof Error ? error.message : String(error));
        if (isErrorClass(expected)) return error instanceof expected || (error && error.name === expected.name && expected.name !== "");
        if (typeof expected === "function") return expected(error) === true;
        return null;
    };
    const checkThrown = (what, error, expected, message) => {
        const matched = matchesThrown(error, expected);
        if (matched === null) assertion(what, ["the expectation is a regular expression or an error constructor", `got: ${shown(expected)}`], message);
        if (!matched) {
            const wanted = expected instanceof RegExp ? `pattern: ${expected}` : `expected: ${expected.name || shown(expected)}`;
            assertion(what, [wanted, `actual:   ${shown(error)}`], message);
        }
    };

    assert.throws = (fn, expected, message) => {
        if (typeof fn !== "function") assertion("assert.throws", ["the first argument is a function", `got: ${shown(fn)}`], message);
        try {
            fn();
        } catch (error) {
            checkThrown("assert.throws", error, expected, message);
            return true;
        }
        assertion("assert.throws", ["the function did not throw"], message);
    };

    const checkSettled = (settled, expected, message) => {
        if (!settled || settled.status === "fulfilled") {
            assertion("assert.rejects", ["the promise fulfilled", `value: ${shown(settled && settled.value)}`], message);
        }
        checkThrown("assert.rejects", settled.value, expected, message);
        return true;
    };
    assert.rejects = (fn, expected, message) => {
        let promise;
        if (typeof fn === "function") {
            try {
                promise = fn();
            } catch (error) {
                assertion("assert.rejects", ["the function threw instead of returning a promise", `actual: ${shown(error)}`], message);
            }
        } else if (fn && typeof fn.then === "function") {
            promise = fn;
        } else {
            assertion("assert.rejects", ["the first argument is a function or a promise", `got: ${shown(fn)}`], message);
        }
        if (!promise || typeof promise.then !== "function") {
            assertion("assert.rejects", ["the function did not return a promise", `got: ${shown(promise)}`], message);
        }
        const host = g.__moquiTestHost;
        if (host && typeof host.settle === "function") {
            let settled;
            try {
                settled = host.settle(promise);
            } catch (error) {
                assertion("assert.rejects", [error && error.message ? error.message : String(error)], message);
            }
            return checkSettled(settled, expected, message);
        }
        return promise.then(
            (value) => checkSettled({ status: "fulfilled", value }, expected, message),
            (error) => checkSettled({ status: "rejected", value: error }, expected, message)
        );
    };

    Object.defineProperty(g, "assert", { value: assert, writable: true, configurable: true });
})(globalThis);
