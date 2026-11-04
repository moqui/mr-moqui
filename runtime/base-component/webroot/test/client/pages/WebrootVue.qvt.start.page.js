/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
"use strict";
test("startup sets the session token before mounting and history preserves fragment-only navigation", async () => {
    const config = document.createElement("script");
    config.type = "application/json"; config.id = "moqui-config";
    config.textContent = JSON.stringify({ sessionToken: "token-before-mount", locale: "en", basePath: "/apps", linkBasePath: "/qapps" });
    document.body.appendChild(config);
    // The mount waits for the catalog, but a post can already use the page's token.
    let releaseCatalog;
    moqui.request = () => new Promise((resolve) => { releaseCatalog = resolve; });
    let mounted = false;
    const visits = [];
    const screenUrl = "/qapps/dev/Artifacts/Refs?artifactId=entity%3AUserAccount&name=User%20Account";
    const vm = { currentLinkUrl: screenUrl, bodyParameters: { draft: "unsaved" },
        setUrl(url, body, done, push) { visits.push({ url, body, done, push }); this.currentLinkUrl = url; } };
    const app = { use() {}, component() {}, config: { globalProperties: {} }, mount() { mounted = true; return vm; } };
    window.Vue = { createApp: (options) => { vm.onPopState = options.methods.onPopState.bind(vm); return app; } };
    window.Quasar = {};
    const started = moqui.qvt.start();
    assert.equal(mounted, false);
    assert.equal(moqui.sessionToken, "token-before-mount", "a post made before the mount carries the page's token");
    releaseCatalog({ json: {} });
    await started;
    assert.equal(mounted, true);

    const originalUrl = location.href;
    const fixture = document.createElement("div");
    fixture.innerHTML = '<input id="history-draft"><a id="history-jump" href="#history-end">Uses</a>' +
        '<div id="history-start"></div><div style="height:1600px"></div><div id="history-end">Uses</div>';
    document.body.appendChild(fixture);
    const input = fixture.querySelector("input");
    input.value = "keep this unsaved draft";
    const move = (action) => new Promise((resolve, reject) => {
        const finish = () => { clearTimeout(timer); requestAnimationFrame(() => resolve()); };
        const timer = setTimeout(() => { removeEventListener("popstate", finish); reject(new Error("history event did not arrive")); }, 2000);
        addEventListener("popstate", finish, { once: true });
        action();
    });
    try {
        // The browser keeps '+' and a trailing slash; the shell stores the same query/path in canonical form.
        history.replaceState(null, "", screenUrl.replace("Refs?", "Refs/?").replace("User%20Account", "User+Account"));
        await move(() => fixture.querySelector("a").click());
        assert.equal(location.hash, "#history-end");
        assert.ok(window.scrollY > 0, "the native fragment scroll remains in place");
        fixture.querySelector("a").href = "#history-start";
        await move(() => fixture.querySelector("a").click());
        await move(() => history.back());
        assert.equal(location.hash, "#history-end");
        await move(() => history.forward());
        assert.equal(location.hash, "#history-start");
        assert.deepEqual(visits, [], "fragment navigation never calls the screen loader");
        assert.equal(fixture.querySelector("input"), input, "the form DOM survives");
        assert.equal(input.value, "keep this unsaved draft");
        assert.deepEqual(vm.bodyParameters, { draft: "unsaved" });

        const otherQuery = "/qapps/dev/Artifacts/Refs?artifactId=entity%3AUserGroup";
        history.pushState(null, "", otherQuery);
        vm.currentLinkUrl = otherQuery;
        await move(() => history.back());
        assert.equal(visits.length, 1);
        assert.equal(visits[0].url, location.pathname + location.search);
        assert.equal(visits[0].push, false, "history does not push a new entry");
        await move(() => history.forward());
        assert.equal(visits.length, 2);
        assert.equal(visits[1].url, otherQuery);
        const otherPath = "/qapps/dev/Artifacts/Search";
        history.pushState(null, "", otherPath);
        vm.currentLinkUrl = otherPath;
        await move(() => history.back());
        assert.equal(visits[2].url, otherQuery);
        await move(() => history.forward());
        assert.equal(visits[3].url, otherPath);
    } finally {
        history.replaceState(null, "", originalUrl);
        fixture.remove();
    }
});
