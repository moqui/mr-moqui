// The globals of a client spec (`moqui test-client`, plans/archive/ClientTestHarness.md 5): the server under test, the params,
// HTTP sessions, HMAC, raw requests, WebSocket, the browser and the component's files, over __moquiTestHost.call. The host
// (test_client/mod.rs) evaluates this after the core prelude (spec, feature, the conditions, assert, dec, sum, skip),
// before each spec file. There is no execution context: `ec` and the server helpers throw, naming what to use instead.
(function (g) {
    const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
    const host = (name, args) => JSON.parse(__moquiTestHost.call(name, JSON.stringify(args === undefined ? null : args)));
    const config = host("config");
    const OUTSIDE = "a client test (test/client) runs outside the server";

    Object.defineProperty(g, "ec", {
        get() { throw new Error(`\`ec\` is the server's: ${OUTSIDE}; use http, ws or browser`); },
        configurable: true
    });
    for (const [name, instead] of [
        ["call", "http.post or http.get on the service's REST path (/rest/s1/...)"],
        ["checkData", "http.get on /rest/e1/... with conditions on the answer"],
        ["screenTest", "http.get on the screen path, or browser"],
        ["uploadedFile", "http.post with multipart: { name: { fileName, text, contentType } }"],
    ]) define(name, () => { throw new Error(`${name}() is the server's: ${OUTSIDE}; use ${instead}`); });
    define("server", Object.freeze(config.server));
    define("params", Object.freeze(config.params));

    const options = (opts) => (opts && typeof opts === "object") ? opts : {};
    const messageOf = (e) => e && e.message !== undefined ? `${e.name || "Error"}: ${e.message}` : String(e);

    // A function's text that evaluates on another thread: a method (`scenario(user, args) { ... }`) gets `function`.
    const sourceOf = (fn, what) => {
        if (fn == null) return null;
        if (typeof fn !== "function") throw new Error(`${what} is a function`);
        const text = String(fn).trim();
        return /^(function\b|\(|[A-Za-z_$][\w$]*\s*=>)/.test(text) ? text : `function ${text}`;
    };
    const plainArgs = (args) => args === undefined ? null : __moquiTest.plain(args, 0);

    // Virtual users (plans/archive/ClientTestHarness.md 5.6): each a thread with its own engine and default session (`http`)
    // runs setup once, scenario until the duration ends or for its iterations, then teardown. They run from their
    // source text, so they cannot use the spec file's variables; args are copied as JSON. The requests of scenario are
    // timed; a response of 400 or more (unless the request has expect: [...]), a transport error or a throw is an
    // error, which ends that iteration. Answers the statistics; the table goes after the feature's line and the whole
    // result to <out>/load.
    define("load", (opts) => {
        const o = options(opts);
        if (typeof o.scenario !== "function") throw new Error("load needs a scenario function");
        const think = o.think == null ? null : Array.isArray(o.think) ? [Number(o.think[0]), Number(o.think[1] ?? o.think[0])]
            : [Number(o.think), Number(o.think)];
        return host("load.run", {
            users: o.users == null ? 1 : Number(o.users), duration: o.duration == null ? null : Number(o.duration),
            iterations: o.iterations == null ? null : Number(o.iterations), rampUp: Number(o.rampUp || 0), think,
            timeout: o.timeout == null ? null : Number(o.timeout), args: plainArgs(o.args),
            setup: sourceOf(o.setup, "setup"), scenario: sourceOf(o.scenario, "scenario"), teardown: sourceOf(o.teardown, "teardown"),
            feature: __moquiTest.caseName || "load",
        });
    });
    // fn(index, args) on n threads, each with its own engine and default session, from its source text as in load;
    // { results, errors }, a thread that threw leaving null in results and its error in errors. For races such as
    // simultaneous logins.
    define("parallel", (n, fn, args, opts) => {
        const o = options(opts);
        return host("parallel", { n: Number(n), source: sourceOf(fn, "parallel's fn"), args: plainArgs(args),
            timeout: o.timeout == null ? null : Number(o.timeout) });
    });
    // What a load user's and a parallel thread run (the host's run_load and run_parallel).
    define("__moquiThread", Object.freeze({
        load(spec, setup, scenario, teardown) {
            host("thread.begin", { load: spec.load, user: spec.user });
            let iteration = 0;
            try {
                const wait = spec.startAt - Date.now();
                if (wait > 0) sleep(wait);
                if (setup) setup(spec.user, spec.args);
                host("load.timed", { on: true });
                const more = () => (spec.iterations == null || iteration < spec.iterations) && (spec.endAt == null || Date.now() < spec.endAt);
                while (more()) {
                    let stop = false;
                    try {
                        scenario(spec.user, spec.args, iteration);
                    } catch (e) {
                        stop = host("load.error", { message: messageOf(e), iteration }).stop;
                    }
                    iteration++;
                    if (stop) break;
                    if (spec.think && more()) {
                        let ms = spec.think[0] + Math.random() * (spec.think[1] - spec.think[0]);
                        if (spec.endAt != null) ms = Math.min(ms, spec.endAt - Date.now());
                        if (ms > 0) sleep(ms);
                    }
                }
                host("load.timed", { on: false, iterations: iteration });
                if (teardown) teardown(spec.user, spec.args);
            } finally {
                host("thread.end", { session: g.http.id });
            }
            return iteration;
        },
        parallel(index, args, fn) {
            host("thread.begin", { user: index });
            try {
                return fn(index, args);
            } finally {
                host("thread.end", { session: g.http.id });
            }
        }
    }));

    class Response {
        constructor(data) { Object.assign(this, data); }
        // The first value of a header, case-insensitive; null when the response has none.
        header(name) {
            const key = String(name).toLowerCase();
            const found = this.headerList.find(([n]) => n.toLowerCase() === key);
            return found ? found[1] : null;
        }
        // Every value of a header, as for Set-Cookie; with no name, every [name, value].
        headers(name) {
            if (name == null) return this.headerList.slice();
            const key = String(name).toLowerCase();
            return this.headerList.filter(([n]) => n.toLowerCase() === key).map(([, v]) => v);
        }
        json() {
            try {
                return JSON.parse(this.text);
            } catch (e) {
                throw new Error(`the response of ${this.url} is not JSON (status ${this.status}, ${this.contentType || "no content type"}): ` +
                    this.text.slice(0, 200));
            }
        }
        cookie(name) { return this.cookies.find((c) => c.name === name) || null; }
        assertContains(text) {
            assert(this.text.includes(text), `the response of ${this.url} (status ${this.status}) does not contain ` +
                `${JSON.stringify(text)}: ${this.text.slice(0, 300)}`);
            return true;
        }
        assertNotContains(text) {
            assert(!this.text.includes(text), `the response of ${this.url} (status ${this.status}) contains ${JSON.stringify(text)}`);
            return true;
        }
        // The body into the output directory; the path written.
        save(name) { return host("response.save", { id: this.id, name: String(name) }); }
    }

    class Session {
        constructor(id) { this.id = id; }
        request(method, path, opts) {
            return new Response(host("http.request", { session: this.id, method: String(method).toUpperCase(), path: String(path),
                opts: options(opts) }));
        }
        get(path, opts) { return this.request("GET", path, opts); }
        post(path, opts) { return this.request("POST", path, opts); }
        put(path, opts) { return this.request("PUT", path, opts); }
        patch(path, opts) { return this.request("PATCH", path, opts); }
        delete(path, opts) { return this.request("DELETE", path, opts); }
        options(path, opts) { return this.request("OPTIONS", path, opts); }
        head(path, opts) { return this.request("HEAD", path, opts); }
        // The request text as it is, over TCP or TLS, with this session's Cookie header unless cookies: false.
        raw(text, opts) { return new Response(host("http.raw", { session: this.id, text: String(text), opts: options(opts) })); }
        // The REST login, which needs no session token. Throws when it fails or needs a second factor.
        login(user, password) {
            const username = String(user);
            const res = this.post("/rest/login", { form: { username, password: password == null ? "moqui" : String(password) },
                accept: "json", csrf: false });
            let body = null;
            try { body = JSON.parse(res.text); } catch (e) { /* the status says what failed */ }
            if (res.status < 400 && body && body.loggedIn === true) return res;
            if (res.status < 400 && body && body.secondFactorRequired) throw new Error(`login of ${username} needs a second factor`);
            const why = body && (body.errors || body.error || body.message);
            throw new Error(`login of ${username} failed: HTTP ${res.status}${why ? ": " + why : ""}`);
        }
        logout() { return this.get("/rest/logout"); }
        // The login form: GET /Login for its cookie, then POST /Login/login with redirects off; that response.
        formLogin(user, password, opts) {
            const o = options(opts);
            this.get("/Login", { headers: o.headers });
            const form = { username: String(user), password: password == null ? "moqui" : String(password) };
            if (o.returnTo != null) form.returnTo = String(o.returnTo);
            return this.post("/Login/login", { form, headers: o.headers });
        }
        cookies() { return host("session.cookies", { session: this.id }); }
        clearCookies() { host("session.clearCookies", { session: this.id }); }
        token() { return host("session.token", { session: this.id }); }
    }

    // A new session with its own cookie jar and token; opts.headers go on each request, opts.user logs it in.
    const newSession = (opts) => {
        const o = options(opts);
        const s = new Session(host("session.new", { headers: o.headers || {} }));
        if (o.user != null) s.login(o.user, o.password);
        return s;
    };
    define("session", newSession);
    define("http", newSession());
    define("login", (user, password) => g.http.login(user, password));
    define("logout", () => g.http.logout());

    // System Message HMAC, the same signers the server uses for /rest/sm.
    define("hmac", Object.freeze({
        sha256Base64(secret, body) {
            return host("hmac.sha256Base64", {
                secret: secret == null ? "" : String(secret), body: body == null ? "" : String(body)
            });
        },
        // The t=<unixSeconds>,v1=<hex> header. unixSeconds is the current unix second when omitted.
        sha256Timestamp(secret, body, unixSeconds) {
            return host("hmac.sha256Timestamp", {
                secret: secret == null ? "" : String(secret), body: body == null ? "" : String(body),
                unixSeconds: unixSeconds == null ? null : Number(unixSeconds)
            });
        }
    }));

    class Socket {
        constructor(data) { Object.assign(this, data); }
        send(text) { host("ws.send", { id: this.id, text: String(text) }); }
        // The next text frame, or null on timeout (seconds) or close.
        receive(opts) {
            const o = options(opts);
            return host("ws.receive", { id: this.id, timeout: o.timeout == null ? null : Number(o.timeout) });
        }
        close() { if (this.open) host("ws.close", { id: this.id }); this.open = false; }
    }
    define("ws", Object.freeze({
        // A refused handshake does not throw: open is false and status is the HTTP answer.
        connect(path, opts) {
            const o = options(opts);
            return new Socket(host("ws.connect", { path: String(path), session: (o.session || g.http).id, headers: o.headers || {},
                origin: o.origin == null ? null : String(o.origin), timeout: o.timeout == null ? null : Number(o.timeout) }));
        }
    }));

    // Headless Chromium (plans/archive/ClientTestHarness.md 5.5, the host's browser.rs): one per spec file, started on the
    // first newPage and closed after the spec, each page in a browser context of its own. Calls block until done; the
    // actions wait for their element and then for the Moqui client to settle. A page's problems fail the feature unless
    // it or the spec has allowPageProblems. With no Chromium to start, newPage skips the feature (or the spec, in
    // setupSpec).
    const matcherOf = (value) => value instanceof RegExp ? { re: value.source, flags: value.flags }
        : value == null ? null : { s: String(value) };
    const shown = (value) => value instanceof RegExp ? String(value) : JSON.stringify(String(value));
    const pageFunction = (fn) => typeof fn === "function" ? { source: String(fn), function: true } : { source: String(fn), function: false };
    const timeoutOf = (o) => o.timeout == null ? null : Number(o.timeout);
    const scope = (matcher, opts) => {
        const o = options(opts);
        return { exact: !!o.exact, desc: o.exact ? `, { exact: true }` : "" };
    };
    class Locator {
        constructor(page, chain, desc) { this.page = page; this.chain = chain; this.desc = desc; }
        _host(name, args) { return host(name, Object.assign({ page: this.page.id, chain: this.chain, desc: this.desc }, args)); }
        _then(step, desc) { return new Locator(this.page, this.chain.concat([step]), this.desc + desc); }
        locator(css) { return this._then({ css: String(css) }, `.locator(${JSON.stringify(String(css))})`); }
        getByRole(role, opts) {
            const o = options(opts);
            const name = o.name == null ? "" : `, { name: ${shown(o.name)}${o.exact ? ", exact: true" : ""} }`;
            return this._then({ role: String(role), name: matcherOf(o.name), exact: !!o.exact }, `.getByRole(${JSON.stringify(String(role))}${name})`);
        }
        getByText(text, opts) {
            const { exact, desc } = scope(text, opts);
            return this._then({ text: matcherOf(text), exact }, `.getByText(${shown(text)}${desc})`);
        }
        getByLabel(text, opts) {
            const { exact, desc } = scope(text, opts);
            return this._then({ label: matcherOf(text), exact }, `.getByLabel(${shown(text)}${desc})`);
        }
        first() { return this._then({ nth: 0 }, ".first()"); }
        last() { return this._then({ nth: -1 }, ".last()"); }
        nth(i) { return this._then({ nth: Number(i) }, `.nth(${Number(i)})`); }
        // The actions wait (attached, visible, enabled, a stable box, and for a click nothing over it), act through
        // input events, then settle. Each answers true, so it may stand as a condition.
        click(opts) { this._host("locator.act", { action: "click", timeout: timeoutOf(options(opts)) }); return true; }
        hover(opts) { this._host("locator.act", { action: "hover", timeout: timeoutOf(options(opts)) }); return true; }
        focus(opts) { this._host("locator.act", { action: "focus", timeout: timeoutOf(options(opts)) }); return true; }
        fill(value, opts) {
            this._host("locator.act", { action: "fill", value: value == null ? "" : String(value), timeout: timeoutOf(options(opts)) });
            return true;
        }
        press(key, opts) { this._host("locator.act", { action: "press", key: String(key), timeout: timeoutOf(options(opts)) }); return true; }
        // Reads, at once: text, value and attribute throw when nothing matches; the others answer false, 0 or null.
        text() { return this._host("locator.read", { what: "text" }); }
        value() { return this._host("locator.read", { what: "value" }); }
        attribute(name) { return this._host("locator.read", { what: "attribute", arg: String(name) }); }
        isVisible() { return this._host("locator.read", { what: "visible" }); }
        isEnabled() { try { return this._host("locator.read", { what: "enabled" }); } catch (e) { return false; } }
        isEditable() { try { return this._host("locator.read", { what: "editable" }); } catch (e) { return false; } }
        count() { return this._host("locator.read", { what: "count" }); }
        boundingBox() { try { return this._host("locator.read", { what: "box" }); } catch (e) { return null; } }
        // visible (the default), hidden, attached or detached; true, or a throw with the page text and its problems.
        waitFor(opts) {
            const o = options(opts);
            return this._host("locator.waitFor", { state: o.state || "visible", timeout: timeoutOf(o) });
        }
        // fn(element, arg) in the page, from its source text; the answer as JSON.
        evaluate(fn, arg) {
            if (typeof fn !== "function") throw new Error("evaluate on a locator needs a function of the element");
            return this._host("locator.evaluate", { source: String(fn), arg: plainArgs(arg) });
        }
        toString() { return this.desc; }
    }
    class Page {
        constructor(id) {
            this.id = id;
            const page = this;
            this.keyboard = Object.freeze({
                press(key) { return page.press(key); },
                // Key events, one character at a time. fill inserts text and does not send keys.
                type(text) { return page._host("page.type", { text: text == null ? "" : String(text) }); },
            });
        }
        _host(name, args) { return host(name, Object.assign({ page: this.id }, args)); }
        // Navigate, wait for the load event, then settle: { status, url } of the document.
        goto(path, opts) { return this._host("page.goto", { path: String(path), timeout: timeoutOf(options(opts)) }); }
        // Offline only: the document at https://moqui.test/, so /js, /css and /libs resolve on that origin.
        setContent(html) { return this._host("page.setContent", { html: html == null ? "" : String(html) }); }
        // A path on the page's origin, or { content } for an inline script. A path waits until the script loads.
        addScript(pathOrContent) {
            const arg = pathOrContent && typeof pathOrContent === "object" ? { content: String(pathOrContent.content ?? "") }
                : { path: String(pathOrContent) };
            return this._host("page.addScript", arg);
        }
        addStyle(pathOrContent) {
            const arg = pathOrContent && typeof pathOrContent === "object" ? { content: String(pathOrContent.content ?? "") }
                : { path: String(pathOrContent) };
            return this._host("page.addStyle", arg);
        }
        url() { return this._host("page.url"); }
        title() { return this._host("page.title"); }
        text() { return this._host("page.text"); }
        html() { return this._host("page.html"); }
        locator(css) { return new Locator(this, [], "page").locator(css); }
        getByRole(role, opts) { return new Locator(this, [], "page").getByRole(role, opts); }
        getByText(text, opts) { return new Locator(this, [], "page").getByText(text, opts); }
        getByLabel(text, opts) { return new Locator(this, [], "page").getByLabel(text, opts); }
        fill(css, value, opts) { return this.locator(css).fill(value, opts); }
        click(css, opts) { return this.locator(css).click(opts); }
        press(key) { this._host("page.press", { key: String(key) }); return true; }
        // In the page: a function (called with arg, from its source text, so it cannot use the spec's variables) or an
        // expression; the answer as JSON.
        evaluate(fn, arg) { return this._host("page.evaluate", Object.assign(pageFunction(fn), { arg: plainArgs(arg) })); }
        // Poll in the page until truthy; its value.
        waitFor(fn, opts) {
            const o = options(opts);
            return this._host("page.waitFor", Object.assign(pageFunction(fn), { arg: plainArgs(o.arg), timeout: timeoutOf(o) }));
        }
        // Until the URL is the text (the whole URL, or its path with the query), matches the pattern, or the function of
        // it is truthy; then settle. The current URL counts, so a navigation that already happened passes.
        waitForURL(match, opts) {
            const o = options(opts);
            const test = typeof match === "function" ? match
                : match instanceof RegExp ? (url) => { match.lastIndex = 0; return match.test(url); }
                : (url) => {
                    const want = String(match);
                    const at = url.replace(/^[a-z]+:\/\/[^/]+/i, "");
                    return url === want || at === want || at.replace(/[?#].*$/, "") === want;
                };
            const timeout = o.timeout == null ? config.timeout : Number(o.timeout);
            const end = Date.now() + timeout * 1000;
            for (;;) {
                const url = this.url();
                if (test(url)) { this.settle(); return true; }
                if (Date.now() >= end) throw new Error(`waitForURL ${shown(match)}: the URL is still ${url} after ${timeout}s`);
                sleep(100);
            }
        }
        // Until the Moqui client is idle for 100 ms: no load pending, no loading bar, no fetch in flight.
        settle(opts) { return this._host("page.settle", { timeout: timeoutOf(options(opts)) }); }
        // Into the output directory; the path.
        screenshot(name, opts) { return this._host("page.screenshot", { name: String(name), fullPage: !!options(opts).fullPage }); }
        // Into the output directory; { path, size, magic }.
        pdf(name, opts) { return this._host("page.pdf", { name: String(name), landscape: !!options(opts).landscape }); }
        problems() { return this._host("page.problems"); }
        // The problems so far, which then do not fail the feature.
        takeProblems() { return this._host("page.takeProblems"); }
        // Ignore the problems that match, from now on and those already seen.
        allowProblems(pattern) {
            const re = pattern instanceof RegExp ? pattern : new RegExp(String(pattern).replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
            this._host("page.allowProblems", { re: re.source, flags: re.flags });
        }
        setViewport(width, height) { this._host("page.setViewport", { width: Number(width), height: Number(height) }); }
        close() { this._host("page.close"); }
    }
    define("browser", Object.freeze({
        // { user, password, viewport: { width, height }, colorScheme, timeout, problems: { warnings } }; with user, the
        // page signs in through the form at /Login.
        newPage(opts) {
            const o = options(opts);
            const opened = host("browser.newPage", { user: o.user == null ? null : String(o.user),
                password: o.password == null ? null : String(o.password), viewport: o.viewport || null,
                colorScheme: o.colorScheme == null ? null : String(o.colorScheme), timeout: timeoutOf(o),
                problems: o.problems || null, feature: __moquiTest.caseName || null });
            if (opened.skipped) skip(`no browser: ${opened.skipped}`);
            return new Page(opened.id);
        },
        // At the top level of the spec, not inside a feature. The page's test() calls are the cases.
        runInPage(opts) {
            if (__moquiTest.caseName) throw new Error("runInPage is called at the top level of the spec, not inside a feature");
            const o = options(opts);
            if (o.test == null || o.test === "") throw new Error("runInPage needs a test file");
            const list = (value) => value == null ? [] : Array.from(value).map(String);
            (__moquiTest.pageRuns = __moquiTest.pageRuns || []).push({
                scripts: list(o.scripts), styles: list(o.styles), test: String(o.test),
            });
        },
    }));

    // The spec's own component directory, read-only.
    define("files", Object.freeze({
        // The files under dir (relative to the component), with paths relative to it, matching regex when given.
        list(dir, regex) {
            const all = host("files.list", { dir: dir == null ? "" : String(dir) });
            if (regex == null) return all;
            const re = regex instanceof RegExp ? regex : new RegExp(String(regex));
            return all.filter((path) => { re.lastIndex = 0; return re.test(path); });
        },
        read(path) { return host("files.read", { path: String(path) }); }
    }));

    define("sleep", (ms) => { host("sleep", { ms: Number(ms) || 0 }); });
    // Poll fn until it is truthy and answer its value; else throw after timeout seconds (--timeout by default).
    define("waitUntil", (fn, opts) => {
        if (typeof fn !== "function") throw new Error("waitUntil needs a function");
        const o = options(opts);
        const timeout = o.timeout == null ? config.timeout : Number(o.timeout);
        const interval = o.interval == null ? 100 : Number(o.interval);
        const end = Date.now() + timeout * 1000;
        for (;;) {
            const value = fn();
            if (__truthy(value)) return value;
            if (Date.now() >= end) {
                throw new Error(`waitUntil: not true after ${timeout}s; the last value was ${__moquiTest.display(value, 100)}`);
            }
            sleep(interval);
        }
    });

    const logAt = (level) => (message) => { host("log", { level, message: typeof message === "string" ? message : __moquiTest.display(message, 2000) }); };
    define("log", Object.freeze({ trace: logAt("trace"), debug: logAt("debug"), info: logAt("info"), warn: logAt("warn"), error: logAt("error") }));
})(globalThis);
