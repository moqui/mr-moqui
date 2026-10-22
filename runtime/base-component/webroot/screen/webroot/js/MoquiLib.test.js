/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// Tests of the pure functions of MoquiLib.js. Run: node --test runtime/base-component/webroot/screen/webroot/js/MoquiLib.test.js
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const moqui = require("./MoquiLib.js");

test("type checks", () => {
    assert.ok(moqui.isString("a") && !moqui.isString(1));
    assert.ok(moqui.isArray([]) && !moqui.isArray({}));
    assert.ok(moqui.isPlainObject({}) && !moqui.isPlainObject([]) && !moqui.isPlainObject(null) && !moqui.isPlainObject(new Date()));
    assert.ok(moqui.isNumber(1) && moqui.isBoolean(false) && moqui.isFunction(() => 1));
    assert.ok(moqui.isEmptyObject({}) && moqui.isEmptyObject(null) && !moqui.isEmptyObject({ a: 1 }));
});

test("deepCopy copies objects and lists and leaves values", () => {
    const source = { a: [1, { b: 2 }], c: "x", d: null };
    const copy = moqui.deepCopy(source);
    assert.deepEqual(copy, source);
    copy.a[1].b = 3;
    assert.equal(source.a[1].b, 2);
    assert.equal(moqui.deepCopy(5), 5);
    assert.equal(moqui.deepCopy(null), null);
});

test("lists are equal in order, or ignoring order, and empty is equal to missing", () => {
    assert.ok(moqui.arraysEqual([1, 2], [1, 2]));
    assert.ok(!moqui.arraysEqual([1, 2], [2, 1]));
    assert.ok(moqui.arraysEqual([1, 2], [2, 1], true));
    assert.ok(!moqui.arraysEqual([1, 2], [1], true));
    assert.ok(moqui.arraysEqual(null, []));
    assert.ok(moqui.arraysEqual([], undefined));
    assert.ok(!moqui.arraysEqual(["a"], null));
    assert.ok(moqui.arraysEqual("a", "a"));
});

test("a mask placeholder equals an empty value", () => {
    assert.ok(moqui.equalsOrPlaceholder("", null));
    assert.ok(moqui.equalsOrPlaceholder(undefined, ""));
    assert.ok(moqui.equalsOrPlaceholder("", "__-__"));
    assert.ok(moqui.equalsOrPlaceholder("__-__", ""));
    assert.ok(!moqui.equalsOrPlaceholder("a", "b"));
    assert.ok(moqui.equalsOrPlaceholder("a", "a"));
});

test("fieldValuesDiff finds a changed value or list", () => {
    assert.ok(!moqui.fieldValuesDiff({ a: "1", b: ["x", "y"], c: "" }, { a: "1", b: ["y", "x"], c: null }));
    assert.ok(moqui.fieldValuesDiff({ a: "1", b: ["x"] }, { a: "1", b: ["x", "y"] }));
    assert.ok(moqui.fieldValuesDiff({ a: "2" }, { a: "1" }));
    assert.ok(moqui.fieldChanged({ a: "2" }, { a: "1" }, "a"));
    assert.ok(!moqui.fieldChanged({ a: "1" }, { a: "1" }, "a"));
});

test("search text round trips with encoding and repeated names", () => {
    const text = moqui.objToSearch({ a: "x y&z", b: ["1", "2"], c: null, d: 0 });
    assert.equal(text, "a=x%20y%26z&b=1&b=2&d=0");
    assert.deepEqual(moqui.searchToObj(text), { a: "x y&z", b: ["1", "2"], d: "0" });
    assert.deepEqual(moqui.searchToObj(""), {});
    assert.deepEqual(moqui.searchToObj("a=b+c&=skip&novalue"), { a: "b c" });
    assert.equal(moqui.objToSearch(null), "");
});

test("a href splits and joins", () => {
    const info = moqui.parseHref("https://example.com:8443/qapps/tools/Foo?a=1&b=x%20y#top");
    assert.equal(info.protocol, "https");
    assert.equal(info.host, "example.com:8443");
    assert.equal(info.path, "/qapps/tools/Foo");
    assert.equal(info.name, "Foo");
    assert.equal(info.hash, "top");
    assert.deepEqual(info.query, { a: "1", b: "x y" });
    assert.equal(moqui.makeHref(info), "https://example.com:8443/qapps/tools/Foo?a=1&b=x%20y#top");
    assert.equal(moqui.makeHref({ path: "/p", query: { q: "1 2" } }), "/p?q=1%202");
    assert.equal(moqui.parseHref("/a/b").path, "/a/b");
    assert.equal(moqui.parseHref("http://host").host, "host");
    assert.equal(moqui.parseHref("/a?x=1?2").search, "x=1?2");
});

test("html encoding", () => {
    assert.equal(moqui.htmlEncode(`<a href="x">T&C's</a>`), "&lt;a href=&quot;x&quot;&gt;T&amp;C&#39;s&lt;/a&gt;");
    assert.equal(moqui.htmlEncode(null), "");
    assert.equal(moqui.htmlDecode("&lt;b&gt; &amp;amp; &quot;q&quot;"), '<b> &amp; "q"');
});

test("number text checks and parsing", () => {
    assert.ok(moqui.isStringNumber("1,234.50") && moqui.isStringNumber("-5") && !moqui.isStringNumber("1a") && !moqui.isStringNumber(5));
    assert.ok(moqui.isStringInteger("1,234") && !moqui.isStringInteger("1.5"));
    assert.equal(moqui.parseNumber("1,234.5"), 1234.5);
    assert.equal(moqui.parseNumber("1,234"), 1234);
    moqui.setNumberSeparators("de-DE");
    assert.equal(moqui.parseNumber("1.234,5"), 1234.5);
    moqui.setNumberSeparators("en-US");
    assert.equal(moqui.parseNumber("1,234.5"), 1234.5);
});

test("the $mq global is the library, for the field rules the server writes", () => {
    assert.equal(globalThis.$mq, moqui);
});

test("LruMap keeps the most recently used entries", () => {
    const map = new moqui.LruMap(2);
    map.put("a", 1);
    map.put("b", 2);
    assert.equal(map.get("a"), 1);
    map.put("c", 3);
    assert.ok(map.containsKey("a") && map.containsKey("c") && !map.containsKey("b"));
    map.remove("a");
    assert.ok(!map.containsKey("a"));
    assert.deepEqual(map.lruList, ["c"]);
});

test("debounce runs once after the wait", async () => {
    let calls = 0;
    const fn = moqui.debounce((value) => { calls += value; }, 20);
    fn(1); fn(1); fn(1);
    assert.equal(calls, 0);
    await new Promise((resolve) => setTimeout(resolve, 60));
    assert.equal(calls, 1);
});

test("a component file splits into template, script and style", () => {
    const styles = [];
    const comp = moqui.parseVue(`<template><div class="a">{{x}}</div></template>
        <script>
        module.exports = { data: function () { return { x: 3 }; }, name: "Demo" };
        </script>
        <style scoped>.a { color: red }</style>`, (text, scoped) => styles.push([text, scoped]));
    assert.equal(comp.template, '<div class="a">{{x}}</div>');
    assert.equal(comp.name, "Demo");
    assert.equal(comp.data().x, 3);
    assert.deepEqual(styles, [[".a { color: red }", true]]);

    const exported = moqui.parseVue("<template><p/></template><script>export default { props: ['id'] }</script>");
    assert.deepEqual(exported.props, ["id"]);
    const defined = moqui.parseVue("<script>define([], function () { return { methods: { go() {} } }; });</script>");
    assert.equal(typeof defined.methods.go, "function");
    assert.deepEqual(moqui.parseVue("no parts here"), {});
});

test("request sends the token on writes, parses json, and rejects with the response on an error", async () => {
    const calls = [];
    globalThis.fetch = (url, init) => {
        calls.push([url, init]);
        const ok = !String(url).includes("fail");
        return Promise.resolve({
            ok, status: ok ? 200 : 500, statusText: "",
            headers: new Map([["X-CSRF-Token", "t2"], ["X-Moqui-Preferences", encodeURIComponent(JSON.stringify({ SHELL_DENSITY: "compact", KEYMAP: "É" }))]]),
            text: () => Promise.resolve(ok ? '{"a":1}' : '{"errors":"bad"}')
        });
    };
    moqui.sessionToken = "t1";
    let seen = "";
    moqui.onSessionToken = (token) => { seen = token; };
    // the preferences the request set, decoded for the page to apply
    const preferences = [];
    moqui.onPreferences = (map) => preferences.push(map);
    // a Map stands in for Headers: get() works on both
    const good = await moqui.request("/x", { method: "post", params: { a: "b c" } });
    assert.deepEqual(good.json, { a: 1 });
    assert.equal(calls[0][1].headers["X-CSRF-Token"], "t1");
    assert.equal(calls[0][1].body, "a=b%20c");
    assert.equal(seen, "t2");
    assert.deepEqual(preferences[0], { SHELL_DENSITY: "compact", KEYMAP: "É" });
    delete moqui.onPreferences;
    await moqui.request("/x?y=1", { params: { a: 1 } });
    assert.equal(calls[1][0], "/x?y=1&a=1");
    assert.equal(calls[1][1].headers["X-CSRF-Token"], undefined);
    await assert.rejects(moqui.request("/fail"), (err) => err.status === 500 && err.json.errors === "bad");
    delete globalThis.fetch;
});

test("a qjs response is an object expression, define, or statements", () => {
    assert.equal(moqui.evalComponentJs("{ data: function () { return { a: 1 }; } }").data().a, 1);
    assert.equal(moqui.evalComponentJs("define([], function () { return { name: 'D' }; });").name, "D");
    assert.equal(moqui.evalComponentJs("var x = { name: 'S' }; return x;").name, "S");
    assert.throws(() => moqui.evalComponentJs("{ nope"));
});

// A document with a head whose scripts load or fail when the test says (fail: the srcs that fail on their next load).
const fakeDocument = (fail) => {
    const head = { children: [], appendChild(node) { node.parentNode = head; head.children.push(node);
        setTimeout(() => { if (fail.has(node.src)) { fail.delete(node.src); node.onerror && node.onerror(); } else if (node.onload) node.onload(); }, 5); },
        removeChild(node) { head.children = head.children.filter((item) => item !== node); node.parentNode = null; } };
    return { head, created: 0,
        createElement() { this.created += 1; return { dataset: {}, getAttribute(name) { return name === "src" ? this.src : null; } }; },
        querySelectorAll() { return head.children.slice(); } };
};

test("loadScript: a failed load rejects every waiter and a later call tries again; a load is shared", async () => {
    const savedDocument = globalThis.document;
    const savedError = console.error;
    console.error = () => {};
    try {
        const fail = new Set(["/libs/x.js"]);
        globalThis.document = fakeDocument(fail);
        const calls = [];
        const first = moqui.loadScript("/libs/x.js", (error) => calls.push(error ? "error" : "ok"));
        const second = moqui.loadScript("/libs/x.js");
        await assert.rejects(first, /Error loading script \/libs\/x.js/);
        await assert.rejects(second, /Error loading script/);
        assert.deepEqual(calls, ["error"]);
        assert.equal(document.created, 1, "the waiters share one load");
        assert.equal(document.head.children.length, 0, "the failed element is removed");
        // a retry loads it, and a call after that is answered by the same load
        const script = await moqui.loadScript("/libs/x.js");
        assert.equal(script.dataset.mqState, "loaded");
        assert.equal(await moqui.loadScript("/libs/x.js"), script);
        assert.equal(document.created, 2);
        // a library that never sets its global rejects after the validate tries (shortened here)
        const savedRetry = moqui.retryValidateCallback;
        moqui.retryValidateCallback = (callback, validate, count, onFail) => (validate() ? callback() : onFail());
        try {
            await assert.rejects(moqui.loadScript("/libs/x.js", null, () => false), /library is not there/);
            assert.equal(await moqui.loadScript("/libs/x.js", null, () => true), script);
        } finally {
            moqui.retryValidateCallback = savedRetry;
        }
    } finally {
        moqui.scriptLoads.clear();
        globalThis.document = savedDocument;
        console.error = savedError;
    }
});

// A browser window for oauthPopup: open() gives a popup whose location and closed state a test drives, window
// messages go to the listeners added, and a BroadcastChannel stand-in delivers what a test posts on it.
function oauthWindow({ blocked } = {}) {
    const saved = {};
    const names = ["open", "location", "addEventListener", "removeEventListener", "BroadcastChannel"];
    names.forEach((name) => { saved[name] = Object.getOwnPropertyDescriptor(globalThis, name); });
    const listeners = new Set();
    const channels = new Set();
    const popup = { closed: false, location: { href: "" }, close() { this.closed = true; } };
    const win = {
        popup, listeners, channels,
        message: (data, origin, source) => listeners.forEach((listener) => listener({ data, origin, source })),
        broadcast: (data) => channels.forEach((channel) => channel.onmessage && channel.onmessage({ data })),
        restore: () => names.forEach((name) => {
            if (saved[name]) Object.defineProperty(globalThis, name, saved[name]);
            else delete globalThis[name];
        }),
    };
    globalThis.open = () => (blocked ? null : popup);
    globalThis.location = { origin: "https://erp.example.com" };
    globalThis.addEventListener = (type, listener) => { if (type === "message") listeners.add(listener); };
    globalThis.removeEventListener = (type, listener) => { listeners.delete(listener); };
    globalThis.BroadcastChannel = class {
        constructor(name) { this.name = name; channels.add(this); }
        postMessage() {}
        close() { channels.delete(this); }
    };
    return win;
}

const waitFor = async (check) => {
    for (let i = 0; i < 200 && !check(); i++) await new Promise((resolve) => setTimeout(resolve, 5));
};

test("oauthPopup takes the outcome only from its own origin and window, with no extra fields", async () => {
    const win = oauthWindow();
    try {
        const started = moqui.oauthPopup(async () => ({ authUrl: "https://accounts.example.com/auth?x=1", signInId: "S1" }),
            async () => ({ status: "pending" }), { pollMs: 5 });
        await waitFor(() => win.popup.location.href);
        assert.equal(win.popup.location.href, "https://accounts.example.com/auth?x=1");
        const outcome = { type: "moqui-email-oauth", signInId: "S1", status: "success", emailAccountId: "A1",
            emailAddress: "ann@example.com", accessToken: "never" };
        win.message(outcome, "https://evil.example.com", win.popup);
        win.message(outcome, "https://erp.example.com", {});
        win.message({ ...outcome, signInId: "OTHER" }, "https://erp.example.com", win.popup);
        win.message(outcome, "https://erp.example.com", win.popup);
        const result = await started;
        assert.equal(result.status, "success");
        assert.equal(result.emailAccountId, "A1");
        assert.equal(result.accessToken, undefined);
        assert.equal(win.listeners.size, 0);
        assert.equal(win.channels.size, 0);
    } finally {
        win.restore();
    }
});

test("oauthPopup hears a window cut off from the page on the channel, and asks the status once it is closed", async () => {
    let win = oauthWindow();
    try {
        const started = moqui.oauthPopup(async () => ({ authUrl: "https://login.example.com/a", signInId: "S2" }),
            async () => ({ status: "pending" }), { pollMs: 5 });
        await waitFor(() => win.popup.location.href);
        win.broadcast({ type: "moqui-email-oauth", signInId: "S2", status: "error", errorCode: "addressMismatch", expectedAddress: "a@x.com" });
        const result = await started;
        assert.equal(result.errorCode, "addressMismatch");
        assert.equal(result.expectedAddress, "a@x.com");
    } finally {
        win.restore();
    }
    // the window closed with no message: the status says how it ended, or after a short wait that it was closed
    win = oauthWindow();
    try {
        let asked = 0;
        const started = moqui.oauthPopup(async () => ({ authUrl: "https://login.example.com/a", signInId: "S3" }),
            async (id) => { asked++; assert.equal(id, "S3"); return { status: "success", emailAccountId: "A3" }; },
            { pollMs: 5, cutOffMs: 0 });
        await waitFor(() => win.popup.location.href);
        win.popup.closed = true;
        const result = await started;
        assert.equal(result.status, "success");
        assert.equal(result.emailAccountId, "A3");
        assert.ok(asked >= 1);
    } finally {
        win.restore();
    }
    win = oauthWindow();
    try {
        const started = moqui.oauthPopup(async () => ({ authUrl: "https://login.example.com/a", signInId: "S4" }),
            async () => ({ status: "pending" }), { pollMs: 5, cutOffMs: 0, closedWaitMs: 20 });
        await waitFor(() => win.popup.location.href);
        win.popup.closed = true;
        const result = await started;
        assert.equal(result.status, "cancelled");
        assert.equal(result.errorCode, "closed");
    } finally {
        win.restore();
    }
});

test("oauthPopup asks the status while a window cut off from the page is open, gives up after cutOffWaitMs, and cancels", async () => {
    // Google's page cuts the window off (it reads as closed at once): the first status that is not pending ends the wait
    let win = oauthWindow();
    try {
        let asked = 0;
        const started = moqui.oauthPopup(async () => ({ authUrl: "https://accounts.example.com/a", signInId: "S5" }),
            async () => (++asked < 3 ? { status: "pending" } : { status: "success", emailAccountId: "A5" }), { pollMs: 5, statusPollMs: 5 });
        await waitFor(() => win.popup.location.href);
        win.popup.closed = true;
        const result = await started;
        assert.equal(result.status, "success");
        assert.equal(result.emailAccountId, "A5");
        assert.equal(asked, 3);
    } finally {
        win.restore();
    }
    // the status is asked while the window is open too, so a lost message does not leave it waiting
    win = oauthWindow();
    try {
        const started = moqui.oauthPopup(async () => ({ authUrl: "https://accounts.example.com/a", signInId: "S6" }),
            async () => ({ status: "error", errorCode: "expired", message: "The sign-in took too long." }), { pollMs: 5, statusPollMs: 5 });
        const result = await started;
        assert.equal(result.errorCode, "expired");
        assert.equal(win.popup.closed, false);
    } finally {
        win.restore();
    }
    // cut off and never answered: timedOut after cutOffWaitMs, the window closed
    win = oauthWindow();
    try {
        const started = moqui.oauthPopup(async () => ({ authUrl: "https://accounts.example.com/a", signInId: "S7" }),
            async () => ({ status: "pending" }), { pollMs: 5, statusPollMs: 5, cutOffWaitMs: 40 });
        await waitFor(() => win.popup.location.href);
        win.popup.closed = true;
        const result = await started;
        assert.equal(result.status, "cancelled");
        assert.equal(result.errorCode, "timedOut");
        assert.equal(win.listeners.size, 0);
    } finally {
        win.restore();
    }
    // the handle's cancel(): cancelled, the window closed, nothing left listening
    win = oauthWindow();
    try {
        const started = moqui.oauthPopup(async () => ({ authUrl: "https://accounts.example.com/a", signInId: "S8" }),
            async () => ({ status: "pending" }), { pollMs: 5 });
        assert.equal(typeof started.cancel, "function");
        await waitFor(() => win.popup.location.href);
        started.cancel();
        const result = await started;
        assert.equal(result.status, "cancelled");
        assert.equal(result.errorCode, "cancelled");
        assert.equal(result.signInId, "S8");
        assert.ok(win.popup.closed);
        assert.equal(win.listeners.size, 0);
        assert.equal(win.channels.size, 0);
    } finally {
        win.restore();
    }
});

test("oauthPopup reports a blocked window, a start that failed, and a URL that is not http", async () => {
    let win = oauthWindow({ blocked: true });
    try {
        const result = await moqui.oauthPopup(async () => ({ authUrl: "https://x.example.com", signInId: "S" }), async () => ({}));
        assert.equal(result.errorCode, "popupBlocked");
    } finally {
        win.restore();
    }
    win = oauthWindow();
    try {
        const result = await moqui.oauthPopup(async () => { throw new Error("Sign-in with Google is not set up"); }, async () => ({}));
        assert.equal(result.errorCode, "startFailed");
        assert.match(result.message, /not set up/);
        assert.ok(win.popup.closed);
    } finally {
        win.restore();
    }
    win = oauthWindow();
    try {
        const result = await moqui.oauthPopup(async () => ({ authUrl: "javascript:alert(1)", signInId: "S" }), async () => ({}));
        assert.equal(result.errorCode, "startFailed");
        assert.equal(win.popup.location.href, "");
    } finally {
        win.restore();
    }
});

test("a notification listener unregisters by its handle or by topic and function, and the last one unsubscribes", () => {
    const saved = Object.getOwnPropertyDescriptor(globalThis, "WebSocket");
    const sockets = [];
    function FakeSocket(url) { this.url = url; this.readyState = 0; this.sent = []; sockets.push(this); }
    FakeSocket.prototype.send = function (text) { this.sent.push(text); };
    Object.assign(FakeSocket, { CONNECTING: 0, OPEN: 1, CLOSING: 2, CLOSED: 3 });
    Object.defineProperty(globalThis, "WebSocket", { value: FakeSocket, configurable: true, writable: true });
    try {
        const client = new moqui.NotificationClient("ws://erp.example.com/notws");
        const seen = [];
        const first = (message) => seen.push("first " + message.title);
        const second = (message) => seen.push("second " + message.title);
        const unregisterFirst = client.registerListener("orders", first);
        client.registerListener("orders", second);
        assert.equal(typeof unregisterFirst, "function");
        const socket = sockets[0];
        socket.readyState = 1;
        socket.onopen();
        assert.deepEqual(socket.sent, ["subscribe:orders"]);
        const deliver = (title) => socket.onmessage({ data: JSON.stringify({ topic: "orders", title }) });
        deliver("one");
        assert.deepEqual(seen, ["first one", "second one"]);
        // by the handle; a second call finds nothing
        assert.equal(unregisterFirst(), true);
        assert.equal(unregisterFirst(), false);
        deliver("two");
        assert.deepEqual(seen.slice(2), ["second two"]);
        assert.deepEqual(socket.sent, ["subscribe:orders"], "the topic still has a listener");
        // by topic and function: the last one gone, the socket stops asking for the topic
        assert.equal(client.unregisterListener("orders", second), true);
        assert.deepEqual(socket.sent, ["subscribe:orders", "unsubscribe:orders"]);
        assert.equal(client.topicListeners.orders, undefined);
        deliver("three");
        assert.equal(seen.length, 3);
        // a listener that unregisters itself while called: the others of the message still get it
        const once = client.registerListener("ALL", (message) => { seen.push("once " + message.title); once(); });
        client.registerListener("ALL", (message) => seen.push("all " + message.title));
        deliver("four");
        deliver("five");
        assert.deepEqual(seen.slice(3), ["once four", "all four", "all five"]);
        // the display listener (no callback) too
        client.registerListener("alerts");
        assert.equal(client.unregisterListener("alerts"), true);
        assert.equal(client.unregisterListener("nothing", first), false);
    } finally {
        if (saved) Object.defineProperty(globalThis, "WebSocket", saved);
        else delete globalThis.WebSocket;
    }
});

test("a notification client tells its listeners when the socket opens and closes, and reopens at once back online", () => {
    const saved = Object.getOwnPropertyDescriptor(globalThis, "WebSocket");
    const savedAdd = globalThis.addEventListener;
    const sockets = [];
    const windowListeners = {};
    function FakeSocket(url) { this.url = url; this.readyState = 0; this.sent = []; sockets.push(this); }
    FakeSocket.prototype.send = function (text) { this.sent.push(text); };
    Object.assign(FakeSocket, { CONNECTING: 0, OPEN: 1, CLOSING: 2, CLOSED: 3 });
    Object.defineProperty(globalThis, "WebSocket", { value: FakeSocket, configurable: true, writable: true });
    globalThis.addEventListener = (name, fn) => { (windowListeners[name] = windowListeners[name] || []).push(fn); };
    const timers = [];
    const savedTimeout = globalThis.setTimeout;
    globalThis.setTimeout = (fn, ms) => { timers.push(ms); return 0; };
    try {
        const client = new moqui.NotificationClient("ws://erp.example.com/notws");
        const states = [];
        const off = client.onState((state) => states.push(state));
        client.onState(() => { throw new Error("a listener that fails stops no other"); });
        // online before any socket: nothing to reopen
        windowListeners.online.forEach((fn) => fn());
        assert.equal(sockets.length, 0);
        client.registerListener("orders", () => {});
        const first = sockets[0];
        first.readyState = 1;
        first.onopen();
        assert.equal(client.isOpen(), true);
        first.readyState = 3;
        first.onclose();
        assert.deepEqual(states, ["open", "closed"]);
        assert.deepEqual(timers, [30000], "the usual reopen waits");
        // used up its tries: back online opens it at once, with the tries counted again
        client.tryReopenCount = 6;
        client.tryReopen();
        assert.equal(sockets.length, 1);
        windowListeners.online.forEach((fn) => fn());
        assert.equal(sockets.length, 2);
        assert.equal(client.tryReopenCount, 1);
        sockets[1].readyState = 1;
        sockets[1].onopen();
        assert.deepEqual(sockets[1].sent, ["subscribe:orders"]);
        assert.deepEqual(states, ["open", "closed", "open"]);
        // the old socket closing late is not the client's state
        first.onclose();
        assert.deepEqual(states, ["open", "closed", "open"]);
        off();
        sockets[1].onclose();
        assert.equal(states.length, 3);
    } finally {
        if (saved) Object.defineProperty(globalThis, "WebSocket", saved);
        else delete globalThis.WebSocket;
        if (savedAdd) globalThis.addEventListener = savedAdd; else delete globalThis.addEventListener;
        globalThis.setTimeout = savedTimeout;
    }
});

test("a notification client sends JSON when open, and keeps what is sent while closed for the next open", () => {
    const saved = Object.getOwnPropertyDescriptor(globalThis, "WebSocket");
    const sockets = [];
    function FakeSocket(url) { this.url = url; this.readyState = 0; this.sent = []; sockets.push(this); }
    FakeSocket.prototype.send = function (text) { this.sent.push(text); };
    Object.assign(FakeSocket, { CONNECTING: 0, OPEN: 1, CLOSING: 2, CLOSED: 3 });
    Object.defineProperty(globalThis, "WebSocket", { value: FakeSocket, configurable: true, writable: true });
    const savedTimeout = globalThis.setTimeout;
    globalThis.setTimeout = () => 0;
    try {
        const client = new moqui.NotificationClient("ws://erp.example.com/notws");
        // the first send without a socket opens one, and waits for it
        assert.equal(client.send({ collab: "join", doc: "WikiPage:1" }), false);
        assert.equal(sockets.length, 1);
        client.registerListener("collab:WikiPage:1", () => {});
        assert.equal(client.send({ collab: "update", id: 1 }), false);
        const first = sockets[0];
        first.readyState = 1;
        // the subscribe first, then what waited, in order
        first.onopen();
        assert.deepEqual(first.sent, ["subscribe:collab:WikiPage:1", '{"collab":"join","doc":"WikiPage:1"}', '{"collab":"update","id":1}']);
        assert.equal(client.sendQueue.length, 0);
        assert.equal(client.send({ collab: "leave" }), true);
        assert.equal(first.sent[3], '{"collab":"leave"}');
        // closed: it waits again, at most 200, the oldest dropped
        first.readyState = 3;
        first.onclose();
        for (let i = 0; i < 205; i++) client.send({ n: i });
        assert.equal(client.sendQueue.length, 200);
        assert.equal(client.sendQueue[0], '{"n":5}');
        assert.equal(sockets.length, 1, "a socket that was made reopens by its own retry");
        client.tryReopen();
        sockets[1].readyState = 1;
        sockets[1].onopen();
        assert.equal(sockets[1].sent.length, 201);
        assert.equal(sockets[1].sent[200], '{"n":204}');
    } finally {
        if (saved) Object.defineProperty(globalThis, "WebSocket", saved);
        else delete globalThis.WebSocket;
        globalThis.setTimeout = savedTimeout;
    }
});
