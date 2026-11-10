// The globals of a client spec (`moqui test-client`, plans/archive/ClientTestHarness.md 5): the server under test, the params,
// HTTP sessions, HMAC, raw requests, WebSocket, the browser and the component's files, over __moquiTestHost.call. The host
// (test_client/mod.rs) evaluates this after the core prelude (spec, feature, the conditions, assert, dec, sum, skip),
// before each spec file. There is no execution context: `ec` and the server helpers throw, naming what to use instead.
(function (g) {
    const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
    // A load script's state in this engine (plans/LoadRunner.md section 4): the run's raw params, its plan and scenarios,
    // and whether its top level is running, where a request is refused.
    const loadState = { topLevel: false, raw: {}, plan: null, scenarios: {}, order: [] };
    const TOP_LEVEL_REFUSED = new Set(["http.request", "http.raw", "ws.connect", "browser.newPage", "service.call"]);
    const host = (name, args) => {
        if (loadState.topLevel && TOP_LEVEL_REFUSED.has(name)) throw new Error("a load script makes no request at its top level");
        return JSON.parse(__moquiTestHost.call(name, JSON.stringify(args === undefined ? null : args)));
    };
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

    // Virtual users (plans/archive/ClientTestHarness.md 5.6, plans/LoadRunner.md): each a thread with its own engine and
    // default session (`http`) runs setup once, scenario until the duration ends or for its iterations, then teardown.
    // They run from their source text, so they cannot use the spec file's variables; args are copied as JSON. With
    // `script`, the users run a load script's scenarios instead, the file evaluated whole in each user's engine. The
    // requests of the scenario are timed; a response of 400 or more (unless the request has expect: [...]), a
    // transport error or a throw is an error, which ends that iteration. Answers the statistics; the table goes after
    // the feature's line and the whole result to <out>/load.
    const numberOr = (value) => value == null ? null : Number(value);
    define("load", (opts) => {
        const o = options(opts);
        const common = {
            users: numberOr(o.users), duration: numberOr(o.duration), iterations: numberOr(o.iterations),
            rampUp: numberOr(o.rampUp), think: o.think ?? null, pause: o.pause ?? null,
            newSession: o.newSession == null ? null : !!o.newSession, timeout: numberOr(o.timeout),
            thresholds: plainArgs(o.thresholds), label: o.label == null ? null : String(o.label),
            stopAfter: numberOr(o.stopAfter), stopNowAfter: numberOr(o.stopNowAfter),
            feature: __moquiTest.caseName || "load",
        };
        if (o.script != null) {
            return host("load.run", Object.assign(common, { script: String(o.script), params: plainArgs(o.params) ?? {},
                scenarios: plainArgs(o.scenarios) }));
        }
        if (typeof o.scenario !== "function") throw new Error("load needs a scenario function, or a script");
        return host("load.run", Object.assign(common, { users: common.users ?? 1, args: plainArgs(o.args),
            setup: sourceOf(o.setup, "setup"), scenario: sourceOf(o.scenario, "scenario"), teardown: sourceOf(o.teardown, "teardown") }));
    });

    // A load script's top level (plans/LoadRunner.md section 4): plan() resolves the run's params over its defaults,
    // of the defaults' types, into `params`; scenario() registers a group of users.
    const paramDef = (def) => def && typeof def === "object" && !Array.isArray(def) ? def : { default: def };
    const typeOf = (value) => typeof value === "number" ? "number" : typeof value === "boolean" ? "boolean" : "text";
    // A value for an error message; a secret's is left out.
    const shownValue = (def, value) => def.secret ? "the value given" : JSON.stringify(value);
    // `env` (the command's MOQUI_TEST_<NAME> variables) fills a declared param the run does not give, and is not
    // refused when the script has no such param.
    const resolveParams = (defs, raw, env) => {
        const out = {};
        const declared = Object.entries(defs || {});
        for (const [name, given] of declared) {
            const def = paramDef(given);
            let value = raw[name];
            if ((value == null || value === "") && env) value = env[name];
            if (value == null || value === "") {
                if (def.required && def.default == null && !loadState.lenient) throw new Error(`the load script's parameter ${name} is required`);
                value = def.default ?? null;
            } else if (typeOf(def.default) === "number") {
                const number = Number(value);
                if (!Number.isFinite(number)) throw new Error(`the load script's parameter ${name} is a number, not ${shownValue(def, value)}`);
                value = number;
            } else if (typeOf(def.default) === "boolean" && typeof value !== "boolean") {
                const text = String(value).toLowerCase();
                if (["true", "yes", "1"].includes(text)) value = true;
                else if (["false", "no", "0"].includes(text)) value = false;
                else throw new Error(`the load script's parameter ${name} is true or false, not ${shownValue(def, value)}`);
            } else if (typeof value !== "string" && typeOf(def.default) === "text") {
                value = String(value);
            }
            if (Array.isArray(def.options) && value != null && !def.options.map(String).includes(String(value))) {
                throw new Error(`the load script's parameter ${name} is one of ${def.options.join(", ")}, not ${shownValue(def, value)}`);
            }
            out[name] = value;
        }
        for (const name of Object.keys(raw)) {
            if (!(name in out)) {
                if (declared.length > 0) {
                    throw new Error(`the load script has no parameter ${name} (it has ${declared.map(([n]) => n).join(", ")})`);
                }
                out[name] = raw[name];
            }
        }
        return out;
    };
    define("plan", (opts) => {
        if (!loadState.topLevel) throw new Error("plan() is called at the top level of a load script");
        if (loadState.plan) throw new Error("a load script has one plan()");
        loadState.plan = options(opts);
        define("params", Object.freeze(resolveParams(loadState.plan.params, loadState.raw, loadState.env)));
    });
    define("scenario", (name, opts) => {
        if (!loadState.topLevel) throw new Error("scenario() is called at the top level of a load script");
        const key = String(name);
        const o = options(opts);
        if (typeof o.run !== "function") throw new Error(`scenario ${key} needs a run(user, iteration) function`);
        if (loadState.scenarios[key]) throw new Error(`a load script has one scenario named ${key}`);
        loadState.scenarios[key] = o;
        loadState.order.push(key);
    });
    // What the host evaluates around a load script (the host's load/script.rs).
    define("__moquiLoad", Object.freeze({
        // lenient: a script list's read, where a required param with no value is null
        begin(raw, env, opts) {
            loadState.topLevel = true;
            loadState.lenient = !!(opts && opts.lenient);
            loadState.raw = raw && typeof raw === "object" ? raw : {};
            loadState.env = env && typeof env === "object" ? env : {};
            loadState.plan = null;
            loadState.scenarios = {};
            loadState.order = [];
            define("params", Object.freeze(Object.assign({}, loadState.raw)));
        },
        // After the thread's work, whether it ran or threw: the default session goes, which thread.end does not
        // close when the file threw before the user began.
        close() {
            loadState.topLevel = false;
            if (g.http && g.http.id != null) host("session.close", { session: g.http.id });
        },
        describe() {
            loadState.topLevel = false;
            const p = loadState.plan || {};
            const params = {};
            for (const [name, given] of Object.entries(p.params || {})) {
                const def = paramDef(given);
                params[name] = { default: def.secret ? null : (def.default ?? null), description: def.description ?? null,
                    options: Array.isArray(def.options) ? def.options : null, secret: !!def.secret, required: !!def.required,
                    type: typeOf(def.default) };
            }
            const scenarios = loadState.order.map((name) => {
                const s = loadState.scenarios[name];
                return { name, users: s.users ?? null, rampUp: s.rampUp ?? null, duration: s.duration ?? null,
                    iterations: s.iterations ?? null, think: s.think ?? null, pause: s.pause ?? null, newSession: !!s.newSession };
            });
            return {
                plan: { title: p.title ?? null, description: p.description ?? null, uses: p.uses || ["http"], params,
                    thresholds: plainArgs(p.thresholds) },
                params: plainArgs(g.params), scenarios,
            };
        },
    }));

    // step(name, fn): fn's whole time as a sample of kind step, its requests counted under their own names too.
    define("step", (name, fn) => {
        if (typeof fn !== "function") throw new Error("step needs a name and a function");
        host("step.begin");
        let value;
        try {
            value = fn();
        } catch (e) {
            host("step.end", { name: String(name), error: messageOf(e) });
            throw e;
        }
        host("step.end", { name: String(name) });
        return value;
    });
    // A wait that is not timed: milliseconds, [min, max], or { gaussian: [mean, deviation] }.
    define("pause", (spec) => { host("load.pause", { pause: spec ?? null }); });
    // Ends the iteration as an error with the message.
    define("fail", (message) => { throw new Error(message == null || message === "" ? "failed" : String(message)); });
    define("service", Object.freeze({
        call(name) {
            throw new Error(`service.call(${JSON.stringify(String(name))}) runs in a server's own process (a load run from Tools › Load); ` +
                "from a client use http.post on the service's REST path (/rest/s1/...)");
        },
    }));

    // fn(index, args) on n threads, each with its own engine and default session, from its source text as in load;
    // { results, errors }, a thread that threw leaving null in results and its error in errors. For races such as
    // simultaneous logins.
    define("parallel", (n, fn, args, opts) => {
        const o = options(opts);
        return host("parallel", { n: Number(n), source: sourceOf(fn, "parallel's fn"), args: plainArgs(args),
            timeout: o.timeout == null ? null : Number(o.timeout) });
    });
    // A load user: setup once, run(iteration) until the duration ends, its iterations are done or the load stops, then
    // teardown. With newSession each iteration starts on a fresh `http`.
    const runUser = (spec, setup, run, teardown) => {
        host("thread.begin", { load: spec.load, scenario: spec.scenario, user: spec.user, pause: spec.pause ?? null });
        let iteration = 0;
        try {
            // the ramp ends on any stop, and a user stopped before its start runs neither setup nor teardown
            if (host("load.ramp", { until: spec.startAt }).stop) return iteration;
            if (setup) setup();
            let stop = host("load.timed", { on: true }).stop;
            const more = () => !stop && (spec.iterations == null || iteration < spec.iterations) && (spec.endAt == null || Date.now() < spec.endAt);
            while (more()) {
                if (spec.newSession) {
                    const old = g.http;
                    define("http", newSession());
                    host("session.close", { session: old.id });
                }
                try {
                    run(iteration);
                } catch (e) {
                    host("load.error", { message: messageOf(e), iteration });
                }
                iteration++;
                stop = host("load.iterated", { iteration }).stop;
                if (spec.think && more()) host("load.pause", { pause: spec.think, until: spec.endAt });
            }
            host("load.timed", { on: false });
            if (teardown) teardown();
        } finally {
            host("thread.end", { session: g.http.id });
        }
        return iteration;
    };
    // What a load user's and a parallel thread run (the host's load module and run_parallel).
    define("__moquiThread", Object.freeze({
        load(spec, setup, scenario, teardown) {
            return runUser(spec, setup && (() => setup(spec.user, spec.args)), (iteration) => scenario(spec.user, spec.args, iteration),
                teardown && (() => teardown(spec.user, spec.args)));
        },
        // A load script's user, once the file has run: its scenario's functions, with `this` an object of the user's own.
        script(spec) {
            loadState.topLevel = false;
            const s = loadState.scenarios[spec.scenarioName];
            if (!s) throw new Error(`the load script has no scenario ${spec.scenarioName}`);
            const self = {};
            return runUser(spec, s.setup && (() => s.setup.call(self, spec.user)), (iteration) => s.run.call(self, spec.user, iteration),
                s.teardown && (() => s.teardown.call(self, spec.user)));
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
        // A query parameter of the final url, else of location (a redirect not followed); null when neither has it.
        urlParam(name) {
            const key = String(name);
            const decode = (text) => { try { return decodeURIComponent(text.replace(/\+/g, " ")); } catch (e) { return text; } };
            const find = (url) => {
                if (url == null) return null;
                const text = String(url);
                const at = text.indexOf("?");
                if (at < 0) return null;
                for (const part of text.slice(at + 1).split("#")[0].split("&")) {
                    if (part === "") continue;
                    const eq = part.indexOf("=");
                    if (decode(eq < 0 ? part : part.slice(0, eq)) === key) return decode(eq < 0 ? "" : part.slice(eq + 1));
                }
                return null;
            };
            const found = find(this.url);
            return found != null ? found : find(this.location);
        }
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
        // { colorScheme, reducedMotion, javaScript }: what is named changes, the rest stays.
        setEmulation(opts) {
            const o = options(opts);
            this._host("page.setEmulation", { colorScheme: o.colorScheme == null ? null : String(o.colorScheme),
                reducedMotion: o.reducedMotion == null ? null : String(o.reducedMotion),
                javaScript: o.javaScript == null ? null : !!o.javaScript });
        }
        close() { this._host("page.close"); }
    }
    define("browser", Object.freeze({
        // { user, password, viewport: { width, height }, colorScheme, reducedMotion, javaScript, timeout,
        // problems: { warnings } }; with user, the page signs in through the form at /Login (before javaScript: false
        // turns the page's scripts off).
        newPage(opts) {
            const o = options(opts);
            const opened = host("browser.newPage", { user: o.user == null ? null : String(o.user),
                password: o.password == null ? null : String(o.password), viewport: o.viewport || null,
                colorScheme: o.colorScheme == null ? null : String(o.colorScheme),
                reducedMotion: o.reducedMotion == null ? null : String(o.reducedMotion),
                javaScript: o.javaScript == null ? null : !!o.javaScript, timeout: timeoutOf(o),
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
