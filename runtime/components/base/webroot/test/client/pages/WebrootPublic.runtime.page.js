/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// A public island on the runtime build of Vue (SEC-PSF-04). WebrootPublic.island.bundle.js is the server's answer to
// /Island.qvue?_qvt=bundle for WebrootPublic.island.qvue (moqui-core's screen test
// a_qvue_asked_for_as_its_bundle_is_compiled_and_named_by_its_path keeps it current). The offline origin serves files
// only, so the island's bundle URL is pointed at that file; the page loads it with a script element as it would load
// the server's answer. The island's public path (/shop/Island) is not the screen path the bundle was served for
// (/Island), as through a PASS site route: the bundle registers under the path the page asked for.
"use strict";
(function () {
const BUNDLE = "/files/component/webroot/test/client/pages/WebrootPublic.island.bundle.js";

const settle = async (found, what) => {
    for (let i = 0; i < 200; i++) {
        const value = found();
        if (value) return value;
        await new Promise((resolve) => setTimeout(resolve, 25));
    }
    throw new Error("timed out waiting for " + what);
};

test("the bundle URL asks for the compiled form", () => {
    assert.equal(moqui.qvueBundleUrl("/Island.qvue"), "/Island.qvue?_qvt=bundle");
    assert.equal(moqui.qvueBundleUrl("/Island.qvue?a=1"), "/Island.qvue?a=1&_qvt=bundle");
});

test("a runtime-build page mounts a bundled .qvue island with no eval and no new Function", async () => {
    assert.equal(typeof globalThis.VueCompilerDOM, "undefined", "the page has no template compiler");
    const added = [];
    const add = (node) => { document.body.appendChild(node); added.push(node); return node; };
    const mount = add(document.createElement("div"));
    mount.id = "bundle-island";
    const config = add(document.createElement("script"));
    config.type = "application/json";
    config.setAttribute("data-moqui-island", "bundle-island");
    config.textContent = JSON.stringify({ shell: "island", locale: "en", timeZone: "UTC", quasarLang: "enUS",
        qvt: { build: "runtime", handles: ["static-template", "bundled-qvue"] },
        route: { name: "island", basePath: "/shop/Island", title: "Island", renderModes: ["qvue"] },
        context: { note: "from the page" }, labels: { loading: "Loading", notFound: "" } });

    // what a policy without 'unsafe-eval' refuses, refused here
    const evaluated = [];
    const realEval = globalThis.eval;
    const RealFunction = globalThis.Function;
    globalThis.eval = function () { evaluated.push("eval"); throw new Error("eval is refused on this page"); };
    globalThis.Function = new Proxy(RealFunction, {
        construct() { evaluated.push("new Function"); throw new Error("new Function is refused on this page"); },
        apply() { evaluated.push("Function"); throw new Error("Function is refused on this page"); },
    });
    const asked = [];
    const bundleUrl = moqui.qvueBundleUrl;
    moqui.qvueBundleUrl = (url) => { asked.push(bundleUrl(url)); return BUNDLE; };
    let islands = {};
    try {
        islands = moqui.startPublicIslands();
        const title = await settle(() => mount.querySelector(".island-title"), "the island");
        assert.equal(title.textContent, "Bundled island");
        assert.deepEqual(Array.from(mount.querySelectorAll("li")).map((li) => li.textContent), ["one", "two"]);
        assert.equal(mount.querySelector(".island-note").textContent, "from the page", "a nested component's render");
        assert.ok(!mount.querySelector(".q-layout"), "an island has no layout");
        mount.querySelector(".island-add").click();
        await settle(() => mount.querySelector(".island-add").textContent === "Added 1", "the click");
        assert.ok(Array.from(document.querySelectorAll("style[data-mq-style]")).some((style) => style.textContent.includes(".island-note")),
            "the island's style is added");
        assert.equal(asked.length, 1);
        assert.ok(/^\/shop\/Island\.qvue\?(.*&)?_qvt=bundle$/.test(asked[0]), asked[0]);
        assert.equal(typeof moqui.qvtBundled["/shop/Island"].make, "function", "registered under the island's public path");
        assert.ok(!moqui.qvtBundled["/Island"], "not under the screen path it was served for");
        // a second load of the screen makes it from the bundle, with no request
        let again = null;
        moqui.loadComponent({ path: "/shop/Island", renderModes: ["qvue"] }, (comp) => { again = comp; });
        assert.ok(again && typeof again.render === "function");
        assert.equal(asked.length, 1);
        assert.deepEqual(evaluated, []);
    } finally {
        globalThis.eval = realEval;
        globalThis.Function = RealFunction;
        moqui.qvueBundleUrl = bundleUrl;
        for (const vm of Object.values(islands)) vm.$.appContext.app.unmount();
        for (const node of added) node.remove();
    }
});

test("a bundle loaded with no name from its loader registers under the path it was loaded from", async () => {
    const script = document.createElement("script");
    await new Promise((resolve, reject) => {
        script.onload = resolve;
        script.onerror = () => reject(new Error("the bundle did not load"));
        script.src = BUNDLE + "?_qvt=bundle";
        document.head.appendChild(script);
    });
    script.remove();
    assert.equal(typeof (moqui.qvtBundled[BUNDLE] || {}).make, "function", Object.keys(moqui.qvtBundled).join(", "));
    delete moqui.qvtBundled[BUNDLE];
});

test("on the runtime build a container's screen the server answers as qvue loads as its bundle", async () => {
    const saved = { config: moqui.config, request: moqui.request, loadBundledQvue: moqui.loadBundledQvue };
    const requests = [];
    const bundles = [];
    const made = { name: "Negotiated", render() { return null; } };
    try {
        moqui.request = (url, options) => {
            requests.push([url, options && options.headers ? options.headers["X-Moqui-Client-Modes"] : undefined]);
            return Promise.resolve({ status: 200, text: "<template><p>x</p></template>", json: null,
                headers: { get: (name) => (name.toLowerCase() === "x-moqui-qvt" ? "qvue" : null) } });
        };
        moqui.loadBundledQvue = (url, path) => { bundles.push([url, path]); return Promise.resolve(made); };
        const load = () => new Promise((resolve) => moqui.loadComponent("/Negotiated?recordId=R1", resolve));
        moqui.config = { qvt: { build: "runtime", handles: ["static-template", "bundled-qvue"] } };
        assert.equal(await load(), made);
        // (a narrow window adds _uiType=mobile to the query)
        assert.ok(requests[0][0].startsWith("/Negotiated.qvt?recordId=R1") && requests[0][1] === "qvue,qjs", String(requests[0]));
        assert.equal(bundles.length, 1);
        assert.ok(bundles[0][0].startsWith("/Negotiated.qvue?recordId=R1") && bundles[0][1] === "/Negotiated", String(bundles[0]));
        // a runtime page that does not load bundles (the desktop shell) gets none, and evaluates nothing
        moqui.config = { qvt: { build: "runtime", handles: ["static-template"] } };
        assert.equal(await load(), null);
        assert.equal(bundles.length, 1);
    } finally {
        Object.assign(moqui, saved);
    }
});
})();
