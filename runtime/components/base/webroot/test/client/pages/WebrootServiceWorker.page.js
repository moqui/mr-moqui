/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
"use strict";
const origin = "https://moqui.example.test";
const get = (path, headers) => new Request(origin + path, { headers: headers || {} });
test("only a library file or a versioned static file, asked with no credential header, is cacheable", () => {
    assert.equal(cacheable(get("/libs/quasar/quasar.js"), origin), true);
    assert.equal(cacheable(get("/js/MoquiLib.js?v=abc"), origin), true);
    assert.equal(cacheable(get("/css/WebrootVue.qvt.css?v=1"), origin), true);
    // a versioned answer that is no static file, or under a signed-in path, is not
    assert.equal(cacheable(get("/rest/s1/moqui/users?v=1"), origin), false);
    assert.equal(cacheable(get("/apps/system/Export.js?v=1"), origin), false);
    assert.equal(cacheable(get("/qapps/x?v=1"), origin), false);
    assert.equal(cacheable(get("/report.csv?v=1"), origin), false);
    assert.equal(cacheable(get("/dav/files/a.png?v=1"), origin), false);
    assert.equal(cacheable(get("/js/MoquiLib.js"), origin), false);
    assert.equal(cacheable(get("/js/MoquiLib.js?v=1", { Authorization: "Bearer x" }), origin), false);
    assert.equal(cacheable(new Request("https://other.example.test/libs/a.js"), origin), false);
    assert.equal(cacheable(new Request(origin + "/libs/a.js", { method: "POST", body: "x" }), origin), false);
});
test("only a whole answer marked public is kept", () => {
    const answer = (control, status) => new Response("x", { status: status || 200, headers: control ? { "Cache-Control": control } : {} });
    // a Response made in the page is of type default, not basic: the rule on the headers is what this checks
    const kept = (response) => keepable(Object.defineProperty(response, "type", { value: "basic" }));
    assert.equal(kept(answer("max-age=86400, must-revalidate, public")), true);
    assert.equal(kept(answer("public, max-age=31536000, immutable")), true);
    assert.equal(kept(answer("")), false);
    assert.equal(kept(answer("max-age=60")), false);
    assert.equal(kept(answer("public, no-store")), false);
    assert.equal(kept(answer("no-cache, no-store, must-revalidate, private")), false);
    assert.equal(kept(answer("public", 206)), false);
    assert.equal(keepable(answer("public")), false);
});
