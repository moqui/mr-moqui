/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// MoquiLib in the page. moqui is /js/MoquiLib.js. assert and test come from the in-page harness.
// location stays the offline origin: a browser will not replace it, and oauthPopup reads that origin.
// loadScript uses the real document. A missing URL is the failed load; harness.allowProblems covers that 404.
"use strict";

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

test("loadScript: a failed load rejects every waiter and a later call tries again; a load is shared", async () => {
    // document cannot be replaced. A missing URL is the failed load, and a real file is the one that loads.
    harness.allowProblems();
    const savedError = console.error;
    console.error = () => {};
    const realCreate = document.createElement.bind(document);
    let created = 0;
    document.createElement = function (tag) {
        const node = realCreate(tag);
        if (String(tag).toLowerCase() === "script") created += 1;
        return node;
    };
    const missing = "/js/__moqui_load_script_missing__.js";
    const present = "/files/runtime/base-component/webroot/test/client/pages/load-ok.js";
    try {
        moqui.scriptLoads.clear();
        const calls = [];
        const first = moqui.loadScript(missing, (error) => calls.push(error ? "error" : "ok"));
        const second = moqui.loadScript(missing);
        await assert.rejects(first, /Error loading script \/js\/__moqui_load_script_missing__\.js/);
        await assert.rejects(second, /Error loading script/);
        assert.deepEqual(calls, ["error"]);
        assert.equal(created, 1, "the waiters share one load");
        assert.equal(document.head.querySelectorAll('script[src*="__moqui_load_script_missing__"]').length, 0, "the failed element is removed");
        const script = await moqui.loadScript(present);
        assert.equal(script.dataset.mqState, "loaded");
        assert.equal(await moqui.loadScript(present), script);
        assert.equal(created, 2);
        const savedRetry = moqui.retryValidateCallback;
        moqui.retryValidateCallback = (callback, validate, count, onFail) => (validate() ? callback() : onFail());
        try {
            await assert.rejects(moqui.loadScript(present, null, () => false), /library is not there/);
            assert.equal(await moqui.loadScript(present, null, () => true), script);
        } finally {
            moqui.retryValidateCallback = savedRetry;
        }
    } finally {
        moqui.scriptLoads.clear();
        document.createElement = realCreate;
        console.error = savedError;
    }
});

// A browser window for oauthPopup: open() gives a popup whose location and closed state a test drives, window
// messages go to the listeners added, and a BroadcastChannel stand-in delivers what a test posts on it.
function oauthWindow({ blocked } = {}) {
    const saved = {};
    const names = ["open", "addEventListener", "removeEventListener", "BroadcastChannel"];
    names.forEach((name) => { saved[name] = Object.getOwnPropertyDescriptor(globalThis, name); });
    const listeners = new Set();
    const channels = new Set();
    const popup = { closed: false, location: { href: "" }, close() { this.closed = true; } };
    const win = {
        popup, listeners, channels, origin: location.origin,
        message: (data, origin, source) => listeners.forEach((listener) => listener({ data, origin, source })),
        broadcast: (data) => channels.forEach((channel) => channel.onmessage && channel.onmessage({ data })),
        restore: () => names.forEach((name) => {
            if (saved[name]) Object.defineProperty(globalThis, name, saved[name]);
            else delete globalThis[name];
        }),
    };
    globalThis.open = () => (blocked ? null : popup);
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
        win.message(outcome, win.origin, {});
        win.message({ ...outcome, signInId: "OTHER" }, win.origin, win.popup);
        win.message(outcome, win.origin, win.popup);
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
    // The library logs a state listener that throws. That log is the point of the catch, and the page treats
    // console.error as a problem, so the test keeps the log quiet and checks that the other listener still ran.
    const savedError = console.error;
    console.error = () => {};
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
        console.error = savedError;
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

test("a notification client in the desktop client uses the host's socket, with no WebSocket of its own", () => {
    const saved = Object.getOwnPropertyDescriptor(globalThis, "WebSocket");
    // no browser socket at all: the page of a desktop client cannot open one to its server
    Object.defineProperty(globalThis, "WebSocket", { value: () => { throw new Error("no page socket"); }, configurable: true, writable: true });
    const savedTimeout = globalThis.setTimeout;
    globalThis.setTimeout = () => 0;
    const opened = [];
    globalThis.moquiDesktop = {
        socket(url) {
            const socket = { url, readyState: 0, sent: [], send(text) { this.sent.push(text); }, close() { this.readyState = 3; } };
            opened.push(socket);
            return socket;
        }
    };
    try {
        const client = new moqui.NotificationClient("ws://moqui-shell.localhost/notws");
        const heard = [];
        client.registerListener("shellCounts", (message) => heard.push(message.counts));
        assert.equal(opened.length, 1);
        assert.equal(opened[0].url, "ws://moqui-shell.localhost/notws");
        assert.equal(client.isOpen(), false);
        opened[0].readyState = 1;
        opened[0].onopen();
        assert.equal(client.isOpen(), true);
        assert.deepEqual(opened[0].sent, ["subscribe:shellCounts"]);
        opened[0].onmessage({ data: '{"topic":"shellCounts","counts":{"chat":2}}' });
        assert.deepEqual(heard, [{ chat: 2 }]);
        client.registerListener("chatRoom:R1", () => {});
        assert.equal(opened[0].sent[1], "subscribe:chatRoom:R1");
        // closed by the host: the page reopens as it would a browser socket, through the host again
        opened[0].readyState = 3;
        opened[0].onclose();
        client.tryReopen();
        assert.equal(opened.length, 2);
    } finally {
        delete globalThis.moquiDesktop;
        if (saved) Object.defineProperty(globalThis, "WebSocket", saved);
        else delete globalThis.WebSocket;
        globalThis.setTimeout = savedTimeout;
    }
});

test("storage keys are under the storage scope only when one is set", () => {
    const saved = moqui.storageScope;
    try {
        moqui.storageScope = "";
        assert.equal(moqui.scopedKey("moqui.chat.outbox.U1"), "moqui.chat.outbox.U1");
        moqui.storageScope = "erp.example.com-0123abcd";
        assert.equal(moqui.scopedKey("moqui.chat.outbox.U1"), "erp.example.com-0123abcd:moqui.chat.outbox.U1");
        // none set here: the desktop host's, read at each use, so a scope the host sets after the page loads still counts
        moqui.storageScope = null;
        assert.equal(moqui.scopedKey("k"), "k");
        globalThis.moquiDesktop = { storageScope: "other.example.com-4567" };
        assert.equal(moqui.scopedKey("k"), "other.example.com-4567:k");
    } finally {
        delete globalThis.moquiDesktop;
        moqui.storageScope = saved;
    }
});

test("an app around the page (window.moquiDesktop) gets the notifications and the saved files, and a browser keeps its own", async () => {
    const calls = [];
    const savedAnchorClick = HTMLAnchorElement.prototype.click;
    const savedCreate = URL.createObjectURL;
    const savedRevoke = URL.revokeObjectURL;
    const shown = [];
    globalThis.moquiDesktop = {
        notify: (options) => calls.push(["notify", options]),
        saveFile: (file) => calls.push(["saveFile", file])
    };
    try {
        let fellBack = false;
        moqui.notifyNotification({ title: "New mail", link: "/qapps/comms/Inbox", notificationMessageId: "M1" }, () => { fellBack = true; });
        moqui.notifyNotification({ title: "Topic only", topic: "chat" }, () => { fellBack = true; });
        assert.ok(!fellBack, "no page toast when the app shows it");
        assert.deepEqual(calls.slice(0, 2), [
            ["notify", { title: "New mail", link: "/qapps/comms/Inbox", tag: "M1" }],
            ["notify", { title: "Topic only", link: null, tag: "chat" }]
        ]);
        moqui.downloadData("a,b\n", "x.csv", "text/csv");
        assert.equal(calls[2][0], "saveFile");
        assert.equal(calls[2][1].name, "x.csv");
        assert.equal(await calls[2][1].blob.text(), "a,b\n");
        assert.equal(calls[2][1].blob.type, "text/csv");

        // without the app: a download link, and the fallback while notifications are not allowed
        delete globalThis.moquiDesktop;
        URL.createObjectURL = () => "blob:fixture";
        URL.revokeObjectURL = () => {};
        HTMLAnchorElement.prototype.click = function () { shown.push([this.download, this.href]); };
        moqui.saveBlob("y.txt", new Blob(["y"]));
        assert.deepEqual(shown, [["y.txt", "blob:fixture"]]);
        if (!globalThis.Notification || Notification.permission !== "granted") {
            moqui.notifyNotification({ title: "Plain" }, () => { fellBack = true; });
            assert.ok(fellBack);
        }
    } finally {
        delete globalThis.moquiDesktop;
        HTMLAnchorElement.prototype.click = savedAnchorClick;
        URL.createObjectURL = savedCreate;
        URL.revokeObjectURL = savedRevoke;
    }
});

test("drafts: this browser's storage under the scope on the web", async () => {
    const savedScope = moqui.storageScope;
    delete globalThis.moquiDesktop;
    try {
        moqui.storageScope = "s1";
        await moqui.drafts.ready();
        await moqui.drafts.put("ironstone.compose.t1", "{\"a\":1}");
        assert.equal(localStorage.getItem("s1:ironstone.compose.t1"), "{\"a\":1}");
        assert.equal(moqui.drafts.get("ironstone.compose.t1"), "{\"a\":1}");
        assert.deepEqual(moqui.drafts.keys("ironstone.compose."), ["ironstone.compose.t1"]);
        assert.deepEqual(moqui.drafts.files("ironstone.compose.t1"), []);
        await moqui.drafts.put("ironstone.compose.t1", "");
        assert.equal(localStorage.getItem("s1:ironstone.compose.t1"), null, "an empty draft is removed");
        await moqui.drafts.put("ironstone.compose.t2", "x");
        await moqui.drafts.remove("ironstone.compose.t2");
        assert.equal(moqui.drafts.get("ironstone.compose.t2"), null);
        assert.equal(await moqui.drafts.saveFile(new Blob(["x"])), null, "no files kept on the web");
        assert.equal(moqui.hostOutbox(), null);
    } finally {
        moqui.storageScope = savedScope;
        localStorage.removeItem("s1:ironstone.compose.t1");
        localStorage.removeItem("s1:ironstone.compose.t2");
    }
});

test("drafts: the desktop client's store, with this browser's drafts moved in once", async () => {
    const savedScope = moqui.storageScope;
    const kept = new Map([["ironstone.compose.x", { value: "{\"newer\":true}", files: [] }]]);
    const calls = [];
    const host = {
        list: (prefix) => Promise.resolve(Array.from(kept.entries()).filter(([key]) => key.indexOf(prefix) === 0)
            .map(([key, draft]) => ({ key, value: draft.value, files: draft.files, updatedMs: 1000 }))),
        put: (key, value, files) => {
            if (key === "ironstone.compose.big") return Promise.reject(new Error("The draft is too large to keep"));
            calls.push(["put", key]);
            kept.set(key, { value, files: files || [] });
            return Promise.resolve();
        },
        remove: (key) => { calls.push(["remove", key]); return Promise.resolve(kept.delete(key)); },
        saveFile: (file, name) => Promise.resolve("ab12-" + name)
    };
    try {
        moqui.storageScope = null;
        globalThis.moquiDesktop = { storageScope: "acct1", drafts: host, outbox: { add() {} } };
        moqui.drafts.cache = null;
        moqui.drafts.loading = null;
        localStorage.setItem("acct1:boss.chat.draft.R1", "<p>old</p>");
        localStorage.setItem("acct1:ironstone.compose.x", "{\"older\":true}");
        localStorage.setItem("acct2:boss.chat.draft.R9", "another account's");
        // before the host's drafts are read: nothing (the callers wait for ready), never this browser's stale copy
        assert.equal(moqui.drafts.get("boss.chat.draft.R1"), null);
        assert.deepEqual(moqui.drafts.keys("boss.chat.draft."), []);
        // one refused for good is dropped; a newer copy of this browser's (by its time) replaces the host's
        localStorage.setItem("acct1:ironstone.compose.big", "x".repeat(10));
        kept.set("ironstone.compose.n", { value: "{\"at\":5}", files: [] });
        localStorage.setItem("acct1:ironstone.compose.n", JSON.stringify({ at: Date.now() + 1000 }));
        assert.ok(moqui.hostOutbox(), "the host's outbox");
        const warn = console.warn;
        const warned = [];
        console.warn = (...args) => warned.push(String(args[0]));
        try { await moqui.drafts.ready(); } finally { console.warn = warn; }
        assert.equal(warned.length, 1);
        assert.ok(warned[0].includes("ironstone.compose.big"));
        assert.equal(kept.get("boss.chat.draft.R1").value, "<p>old</p>", "moved into the host");
        assert.equal(kept.get("ironstone.compose.x").value, "{\"newer\":true}", "the host's own is kept");
        assert.equal(localStorage.getItem("acct1:boss.chat.draft.R1"), null);
        assert.equal(localStorage.getItem("acct1:ironstone.compose.x"), null);
        assert.equal(localStorage.getItem("acct2:boss.chat.draft.R9"), "another account's", "other scopes stay");
        assert.equal(localStorage.getItem("acct1:ironstone.compose.big"), null, "refused for good: dropped");
        assert.ok(JSON.parse(kept.get("ironstone.compose.n").value).at > 5, "this browser's newer copy");
        assert.deepEqual(calls.map((call) => call[1]).sort(), ["boss.chat.draft.R1", "ironstone.compose.n"]);
        calls.length = 1;
        // read once; a key left in this browser again (a crash before it was removed) is only removed
        assert.equal(moqui.drafts.ready(), moqui.drafts.loading);
        localStorage.setItem("acct1:boss.chat.draft.R1", "<p>old</p>");
        assert.equal(await moqui.drafts.importLocal(host), 0);
        assert.equal(localStorage.getItem("acct1:boss.chat.draft.R1"), null);
        assert.equal(calls.length, 1);
        // through the host from now on
        assert.equal(moqui.drafts.get("boss.chat.draft.R1"), "<p>old</p>");
        await moqui.drafts.put("ironstone.compose.y", "{}", ["ab12-a.txt"]);
        assert.deepEqual(kept.get("ironstone.compose.y"), { value: "{}", files: ["ab12-a.txt"] });
        assert.deepEqual(moqui.drafts.files("ironstone.compose.y"), ["ab12-a.txt"]);
        assert.deepEqual(moqui.drafts.keys("ironstone.compose.").sort(), ["ironstone.compose.n", "ironstone.compose.x", "ironstone.compose.y"]);
        assert.equal(localStorage.getItem("acct1:ironstone.compose.y"), null, "not in this browser");
        await moqui.drafts.remove("ironstone.compose.y");
        assert.equal(kept.has("ironstone.compose.y"), false);
        assert.equal(moqui.drafts.get("ironstone.compose.y"), null);
        assert.equal(await moqui.drafts.saveFile(new File(["x"], "a.txt")), "ab12-a.txt");
    } finally {
        delete globalThis.moquiDesktop;
        moqui.storageScope = savedScope;
        moqui.drafts.cache = null;
        moqui.drafts.loading = null;
        ["acct1:boss.chat.draft.R1", "acct1:ironstone.compose.x", "acct2:boss.chat.draft.R9", "acct1:ironstone.compose.big",
            "acct1:ironstone.compose.n"].forEach((key) => localStorage.removeItem(key));
    }
});

test("the desktop client's connection follows its host's answers, and the web never goes offline", async () => {
    const headers = (mark) => ({ get: (name) => (mark && name === "X-Moqui-Offline" ? mark : null) });
    const heard = [];
    const off = moqui.onConnection((offline) => heard.push(offline));
    const realFetch = globalThis.fetch;
    try {
        // the web: no host outbox, so no answer or failure makes it offline
        delete globalThis.moquiDesktop;
        moqui.noteAnswer(502, headers("unreachable"));
        moqui.noteAnswer(200, headers("1"));
        moqui.noteAnswer(0, null);
        assert.equal(moqui.connection.offline, false);
        assert.equal(moqui.unreachable({ status: 502, headers: headers("unreachable") }), false);
        assert.equal(moqui.unreachable({ status: 0 }), false);
        assert.deepEqual(heard, []);
        // the desktop client: the host's answer from the store, its own marked 502, or none is offline; the server's own
        // answer, a 502 or 504 of the server's too, is online
        globalThis.moquiDesktop = { outbox: { add() {} } };
        moqui.noteAnswer(200, headers("1"));
        assert.equal(moqui.connection.offline, true);
        moqui.noteAnswer(502, headers("unreachable"));
        assert.equal(moqui.connection.offline, true);
        moqui.noteAnswer(502, headers(null));
        assert.equal(moqui.connection.offline, false, "a 502 of the server's (a proxy, a transition) is the server there");
        moqui.noteAnswer(0, null);
        moqui.noteAnswer(504, headers(null));
        assert.deepEqual(heard, [true, false, true, false], "each change heard once");
        assert.equal(moqui.unreachable({ status: 502, headers: headers("unreachable") }), true);
        assert.equal(moqui.unreachable({ status: 502, headers: headers(null) }), false);
        assert.equal(moqui.unreachable({ status: 504 }), false);
        assert.equal(moqui.unreachable({ status: 0 }), true);
        assert.equal(moqui.unreachable({ status: 0, aborted: true }), false);
        // through moqui.request: the host's offline answer, the server's, the host's marked 502 (kept on the error), none
        globalThis.fetch = () => Promise.resolve(new Response("{\"a\":1}", { status: 200, headers: { "X-Moqui-Offline": "1" } }));
        const answer = await moqui.request("/apps/x/chatData");
        assert.equal(answer.json.a, 1);
        assert.equal(moqui.connection.offline, true);
        globalThis.fetch = () => Promise.resolve(new Response("{}", { status: 200 }));
        await moqui.request("/apps/x/chatData");
        assert.equal(moqui.connection.offline, false);
        globalThis.fetch = () => Promise.resolve(new Response("The server could not be reached", { status: 502, headers: { "X-Moqui-Offline": "unreachable" } }));
        await assert.rejects(moqui.request("/apps/x/send"), (err) => moqui.unreachable(err));
        assert.equal(moqui.connection.offline, true);
        globalThis.fetch = () => Promise.resolve(new Response("Bad gateway", { status: 502 }));
        await assert.rejects(moqui.request("/apps/x/send"), (err) => !moqui.unreachable(err));
        assert.equal(moqui.connection.offline, false);
        globalThis.fetch = () => Promise.reject(new TypeError("Failed to fetch"));
        await assert.rejects(moqui.request("/apps/x/chatData"), (err) => err.status === 0);
        assert.equal(moqui.connection.offline, true);
        // the host gone (another page of the web): not offline
        delete globalThis.moquiDesktop;
        moqui.setOffline(true);
        assert.equal(moqui.connection.offline, false);
    } finally {
        off();
        globalThis.fetch = realFetch;
        delete globalThis.moquiDesktop;
        moqui.connection.offline = false;
    }
});
