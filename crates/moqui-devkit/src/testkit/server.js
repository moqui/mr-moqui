// The server helpers of a spec file (`moqui test`): services, data checks, logins and screen tests in the entered
// execution context. The harness (testkit/run.rs) evaluates this after prelude.js when no client host runs the spec;
// a client spec (`moqui test-client`) gets the host's own globals instead.
(function (g) {
    const define = (name, value) => Object.defineProperty(g, name, { value, writable: true, configurable: true });
    const T = g.__moquiTest;

    // A service call that throws, naming the service and its errors, when the service fails.
    define("call", (name, params) => {
        const before = ec.message.errors || 0;
        const out = ec.service.sync().name(name).parameters(params || {}).call();
        const after = ec.message.errors || 0;
        if (after > before || out == null) {
            const errors = ec.message.getErrorsString();
            throw new Error(`service ${name} failed${errors ? ": " + errors : ""}`);
        }
        return out;
    });

    // Compare entity XML with the database; the messages of what did not match, which a failure also shows.
    define("checkData", (xml) => {
        const errors = ec.entity.makeDataLoader().xmlText(xml).check();
        for (const error of errors) {
            ec.logger.info(`checkData: ${error}`);
            T.dataErrors.push(error);
        }
        return errors;
    });

    define("login", (username, password) => {
        if (!ec.user.loginUser(username, password == null ? "moqui" : password)) {
            throw new Error(`login of ${username} failed: ${ec.message.getErrorsString() || "a second factor is required"}`);
        }
    });
    define("logout", () => { ec.user.logoutUser(); });

    define("screenTest", (baseScreenPath) => {
        const test = ec.screen.makeTest();
        return baseScreenPath ? test.baseScreenPath(baseScreenPath) : test;
    });

    // Run fn(threadNum, args) on n threads, each with its own execution context. fn runs from its source text in each
    // thread's own engine, so it must not use variables of the spec file; args are copied as JSON.
    define("parallel", (n, fn, args, opts) => {
        if (typeof fn !== "function") throw new Error("parallel needs a function");
        const options = opts || {};
        const out = JSON.parse(__moquiTestHost.parallel(Number(n), String(fn), JSON.stringify(args === undefined ? null : T.plain(args, 0)),
            options.transaction !== false, options.timeout == null ? 120 : Number(options.timeout)));
        if (out.errors.length) throw new Error(`parallel: ${out.errors.length} of ${n} threads failed: ${out.errors.join("; ")}`);
        return out.results;
    });

    // An uploaded file with the text as its content, for the services that take a FileItem: uploadedFile("a.txt", "text", "text/plain").
    define("uploadedFile", (fileName, text, contentType) => __moquiTestHost.uploadedFile(String(fileName), String(text == null ? "" : text),
        contentType == null ? "text/plain" : String(contentType)));

    Object.defineProperty(g, "log", { get: () => ec.logger, configurable: true });

    // A spec with database: false sees no entity facade, service calls, or screen tests, even when this run opened a
    // database. ec.resource stays. The error names the option. The engine is one per thread and the next spec does not
    // install ec again, so the saved properties go back when the spec ends.
    T.denyDatabase = () => {
        if (T.databaseDenied) return;
        T.databaseDenied = {
            entity: Object.getOwnPropertyDescriptor(ec, "entity"),
            call: Object.getOwnPropertyDescriptor(g, "call"),
            screenTest: Object.getOwnPropertyDescriptor(g, "screenTest"),
        };
        const deny = (name) => { throw new Error(name + " is not available in a spec with database: false"); };
        Object.defineProperty(ec, "entity", { configurable: true, get() { deny("ec.entity"); } });
        define("call", () => { deny("call"); });
        define("screenTest", () => { deny("screenTest"); });
    };
    T.restoreDatabase = () => {
        const saved = T.databaseDenied;
        if (!saved) return;
        T.databaseDenied = null;
        // A name that had no own property (one from a prototype) is deleted, so the deny goes and the inherited one shows.
        const put = (target, name, descriptor) => {
            if (descriptor) Object.defineProperty(target, name, descriptor);
            else delete target[name];
        };
        put(ec, "entity", saved.entity);
        put(g, "call", saved.call);
        put(g, "screenTest", saved.screenTest);
    };

    // Evaluate source with sandbox as its global. The spec global is the context-stack proxy and stores only data, so
    // a stub that holds a function (sri.write) cannot live there. This function is non-strict: with keeps the sandbox
    // on the scope chain of the functions the source creates, and the shadowed globalThis is what
    // Object.defineProperty(globalThis, ...) writes. A free name the sandbox lacks continues to the real global.
    define("runInContext", (source, sandbox) => {
        const box = sandbox == null ? {} : sandbox;
        return (function (sandbox) {
            const globalThis = sandbox;
            with (sandbox) { return eval(source); }
        })(box);
    });
})(globalThis);
