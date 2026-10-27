// The in-page runner (plans/archive/NodeDependency.md). assert.js is loaded first. test(name, fn) queues a case; the host
// calls __moquiRunPageTests after the test file has loaded. Each start and finish goes out through the moquiPageCase
// binding, so a hang still names the test that was running. frame(scripts, globals) is a same-origin iframe: globals
// are set, then each script loads with async = false, and the promise resolves to that window.
(function (g) {
    "use strict";
    const queue = [];
    let allowProblems = false;

    const report = (payload) => {
        if (typeof g.moquiPageCase === "function") g.moquiPageCase(JSON.stringify(payload));
    };

    g.test = (name, fn) => {
        if (typeof name !== "string" || !name) throw new Error("test() needs a name");
        if (typeof fn !== "function") throw new Error("test() needs a function");
        queue.push({ name, fn });
    };

    const frame = (scripts, globals) => {
        const iframe = document.createElement("iframe");
        iframe.setAttribute("style", "position:absolute;width:0;height:0;border:0");
        (document.body || document.documentElement).appendChild(iframe);
        const win = iframe.contentWindow;
        const doc = iframe.contentDocument;
        const values = globals && typeof globals === "object" ? globals : {};
        for (const key of Object.keys(values)) {
            try {
                win[key] = values[key];
            } catch (e) {
                Object.defineProperty(win, key, { value: values[key], configurable: true });
            }
        }
        const list = Array.isArray(scripts) ? scripts : [];
        const loadAt = (index) => new Promise((resolve, reject) => {
            if (index >= list.length) {
                resolve(win);
                return;
            }
            const src = String(list[index]);
            const script = doc.createElement("script");
            script.async = false;
            script.src = src;
            script.onload = () => { loadAt(index + 1).then(resolve, reject); };
            script.onerror = () => { reject(new Error("frame script did not load: " + src)); };
            (doc.head || doc.documentElement).appendChild(script);
        });
        return loadAt(0);
    };

    g.harness = {
        frame,
        // Page problems until this test finishes do not fail it. A missing script the test asks for is one of those.
        allowProblems() { allowProblems = true; },
    };

    g.__moquiRunPageTests = async (filter) => {
        const needle = filter == null ? "" : String(filter);
        for (const item of queue) {
            if (needle && !item.name.includes(needle)) continue;
            allowProblems = false;
            report({ event: "start", name: item.name });
            const started = performance.now();
            try {
                const value = item.fn();
                if (value && typeof value.then === "function") await value;
                report({ event: "pass", name: item.name, ms: performance.now() - started, allowProblems });
            } catch (error) {
                const message = error && error.message !== undefined ? String(error.message) : String(error);
                const stack = error && error.stack ? String(error.stack) : "";
                const kind = error && error.name === "ConditionNotSatisfied" ? "assertion" : "error";
                report({
                    event: "fail", name: item.name, ms: performance.now() - started, allowProblems,
                    message, stack, kind,
                });
            }
        }
    };
})(globalThis);
