/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
"use strict";
test("the session token is the page's before the app mounts, while the language catalog is still loading", () => {
    const config = document.createElement("script");
    config.type = "application/json"; config.id = "moqui-config";
    config.textContent = JSON.stringify({ sessionToken: "token-before-mount", locale: "en", basePath: "/apps", linkBasePath: "/qapps" });
    document.body.appendChild(config);
    // the catalog never answers here, so the mount waits (up to its 3 second limit)
    moqui.request = () => new Promise(() => {});
    let mounted = false;
    const app = { use() {}, component() {}, config: { globalProperties: {} }, mount() { mounted = true; return {}; } };
    window.Vue = { createApp: () => app };
    window.Quasar = {};
    moqui.qvt.start();
    assert.equal(mounted, false);
    assert.equal(moqui.sessionToken, "token-before-mount", "a post made before the mount carries the page's token");
});
