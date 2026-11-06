/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// The public shell in the page. moqui.publicRootOptions comes from /js/WebrootPublic.qvt.js.
// The library starts only when #moqui-public-config, Vue and Quasar are all present.
"use strict";

const config = { route: { name: "s", basePath: "/s/TOKEN1234", renderModes: ["qvue"] }, context: { name: "Plans" },
    sessionToken: "st", labels: { notFound: "gone" } };

const instance = () => {
    const options = moqui.publicRootOptions(config);
    const vm = Object.assign({}, options.data(), options.methods);
    Object.defineProperty(vm, "currentLinkPath", { get: () => options.computed.currentLinkPath.call(vm) });
    return { vm, options };
};

test("the root provides the route's screen as mScreen", () => {
    const options = moqui.publicRootOptions(config);
    const vm = { basePath: "/s/TOKEN1234", currentParameters: { folder: "a b" } };
    const screen = options.provide.call(vm).mScreen;
    assert.deepEqual([screen.url, screen.path, screen.parameters], ["/s/TOKEN1234?folder=a%20b", "/s/TOKEN1234", { folder: "a b" }]);
});

test("the root loads the route's screen at its base path with its render modes", () => {
    const { vm } = instance();
    const loaded = [];
    const original = moqui.loadComponent;
    moqui.loadComponent = (urlInfo, callback) => { loaded.push(urlInfo); callback({ template: "<div/>" }); return { abort() {} }; };
    try {
        vm.currentParameters = { folder: "a b" };
        vm.load();
    } finally {
        moqui.loadComponent = original;
    }
    assert.equal(loaded.length, 1);
    assert.equal(loaded[0].path, "/s/TOKEN1234");
    assert.deepEqual(loaded[0].renderModes, ["qvue"]);
    assert.equal(loaded[0].search, "folder=a%20b");
    assert.deepEqual(vm.activeComponent, { template: "<div/>" });
    assert.equal(vm.failed, false);
    assert.equal(vm.basePath, "/s/TOKEN1234");
    assert.equal(vm.currentLinkPath, "/s/TOKEN1234");
    assert.deepEqual(vm.context, { name: "Plans" });
});

test("a screen that is not found shows the not-found text", () => {
    const { vm } = instance();
    const original = moqui.loadComponent;
    moqui.loadComponent = (urlInfo, callback) => { callback(moqui.NotFound); return { abort() {} }; };
    try { vm.load(); } finally { moqui.loadComponent = original; }
    assert.equal(vm.failed, true);
    assert.equal(vm.activeComponent, null);
});

test("a link under the route reloads the screen with its parameters; another URL leaves the page", async () => {
    // The browser will not replace location. The library closes over the global it was called with, so this case
    // loads it again where that global's location.assign only records the URL. A direct eval sees this function's globalThis.
    const assigned = [];
    const host = { moqui, URLSearchParams, location: { assign: (href) => assigned.push(href), search: "" } };
    const source = await (await fetch("/js/WebrootPublic.qvt.js")).text();
    (function (globalThis) { eval(source); })(host);
    const { vm } = instance();
    let loads = 0;
    vm.load = () => { loads++; };
    vm.setUrl("/s/TOKEN1234?path=docs");
    assert.equal(loads, 1);
    assert.deepEqual(vm.currentParameters, { path: "docs" });
    vm.setUrl("/qapps/Hub");
    assert.deepEqual(assigned, ["/qapps/Hub"]);
    assert.equal(loads, 1);
    // the parts of the /qapps root the components call are there and do nothing
    for (const name of ["addContainer", "removeContainer", "setScreenContext", "removeScreenContext", "setPreference"]) vm[name]();
    vm.addNotify("Saved", "positive");
    assert.equal(vm.notifyHistoryList[0].message, "Saved");
});

test("the islands of a page are found by their configs, each at its own mount element", () => {
    const added = [];
    const add = (node) => { document.body.appendChild(node); added.push(node); return node; };
    const script = (mount, config) => {
        const node = document.createElement("script");
        node.type = "application/json";
        node.setAttribute("data-moqui-island", mount);
        node.textContent = JSON.stringify(config);
        return add(node);
    };
    try {
        for (const id of ["cart-island", "review-island"]) { const div = document.createElement("div"); div.id = id; add(div); }
        script("cart-island", { route: { name: "cart", basePath: "/shop/cart", renderModes: ["qvue"] } });
        script("review-island", { route: { name: "reviews", basePath: "/shop/reviews/10000" } });
        script("missing-island", { route: { basePath: "/x" } });
        const found = moqui.publicIslandConfigs(document);
        assert.deepEqual(found.map((config) => [config.mount, config.shell, config.route.basePath]),
            [["cart-island", "island", "/shop/cart"], ["review-island", "island", "/shop/reviews/10000"]]);
        assert.ok(!moqui.publicIslandTemplate.includes("q-layout"), "an island has no layout");
    } finally {
        for (const node of added) node.remove();
    }
});

test("a link under an island's route reloads the island and leaves the page's URL", () => {
    const options = moqui.publicRootOptions(Object.assign({}, config, { shell: "island", mount: "cart-island" }));
    const vm = Object.assign({}, options.data(), options.methods);
    let loads = 0;
    vm.load = () => { loads++; };
    const before = location.href;
    const original = history.replaceState;
    let replaced = 0;
    history.replaceState = function () { replaced++; return original.apply(this, arguments); };
    try {
        vm.setUrl("/s/TOKEN1234?page=2");
    } finally {
        history.replaceState = original;
    }
    assert.equal(loads, 1);
    assert.deepEqual(vm.currentParameters, { page: "2" });
    assert.equal(replaced, 0);
    assert.equal(location.href, before);
});
