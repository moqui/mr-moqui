// The globals of a regression spec (moqui test-regression, plans/Regression-Test.md), after the client prelude
// (session, http, browser): `app`, the engine under test as a user reaches it. Each call is recorded for the
// comparison, with the engine's answer. A spec holds no expected values: the other engine's run is the expectation.
(function (g) {
    const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
    const host = (name, args) => JSON.parse(__moquiTestHost.call(name, JSON.stringify(args === undefined ? null : args)));
    const config = host("regression.config");
    const ERROR_KEYS = ["errors", "errorMessages", "error"];

    // One session for the spec, logged in through the login form, which both engines have.
    let signedIn = null;
    const admin = () => {
        if (signedIn) return signedIn;
        const s = session();
        const res = s.formLogin(config.user, config.password);
        if (res.status >= 400) throw new Error(`the login of ${config.user} failed: HTTP ${res.status}`);
        // a failed login also redirects: the screen the services run from must open
        const screen = s.get("/apps/tools/Service/ServiceRun");
        if (screen.status !== 200)
            throw new Error(`${config.user} could not open Tools > Service > Run after the login: HTTP ${screen.status}`
                + (screen.location ? ` to ${screen.location}` : ""));
        signedIn = s;
        return s;
    };

    // The body as JSON, else its text; null when empty. An empty object is null too: a service with no outputs
    // answers {} on mr-moqui and an empty body on Java, and both say the same.
    const bodyOf = (res) => {
        const text = res.text || "";
        if (!text.trim()) return null;
        let body;
        try { body = JSON.parse(text); } catch (e) { return { text: text.slice(0, 4000) }; }
        const empty = body && typeof body === "object" && !Array.isArray(body) && Object.keys(body).length === 0;
        return empty ? null : body;
    };

    const plain = (parameters) => {
        const out = {};
        for (const [key, value] of Object.entries(parameters || {})) out[key] = value == null ? "" : String(value);
        return out;
    };

    define("app", Object.freeze({
        user: config.user,
        // Run a service as Tools › Service › Run does (its runJson transition): { status, body, location, ok }. The
        // body is the service's output, or its errors; ok is a 2xx answer without errors.
        service(name, parameters) {
            const s = admin();
            s.get("/apps/tools/Service/ServiceRun", { params: { serviceName: String(name) } });
            const form = Object.assign({ moquiFormName: "ServiceParameters", serviceName: String(name), submitButton: "submitButton" },
                plain(parameters));
            const token = s.token();
            if (token) form.moquiSessionToken = token;
            const res = s.post("/apps/tools/Service/ServiceRun/runJson", { form, accept: "json",
                headers: { "X-Requested-With": "XMLHttpRequest" } });
            const body = bodyOf(res);
            // a redirect means the service did not run: the login, or a screen the user may not open
            const failed = res.status < 200 || res.status >= 300
                || (body && typeof body === "object" && ERROR_KEYS.some((key) => body[key]));
            const answer = { status: res.status, body };
            if (res.location) answer.location = res.location;
            host("regression.record", { call: "service", name: String(name), parameters: plain(parameters), answer });
            return Object.freeze(Object.assign({ ok: !failed }, answer));
        },
        // A value the spec read some other way, compared as it is.
        record(name, value) {
            host("regression.record", { call: "record", name: String(name), value: value === undefined ? null : value });
        },
    }));
})(globalThis);
