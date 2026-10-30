/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
"use strict";
const otlp = moqui.rum.otlp;
test("spans and logs are OTLP/JSON with nanosecond text times and typed attributes", () => {
    assert.equal(otlp.nanos(1761800000123.5), "1761800000123500000");
    const span = otlp.span({ traceId: "a".repeat(32), spanId: "b".repeat(16), parentSpanId: "c".repeat(16), name: "GET", kind: "client",
        start: 1000, end: 1001.25, error: "status 500", attributes: { "url.path": "/apps/x", "http.response.status_code": 500, ratio: 0.5, skip: "", none: null } });
    assert.equal(span.kind, 3);
    assert.equal(span.startTimeUnixNano, "1000000000");
    assert.equal(span.endTimeUnixNano, "1001250000");
    assert.equal(span.parentSpanId, "c".repeat(16));
    assert.equal(span.status.code, 2);
    assert.deepEqual(span.attributes, [{ key: "url.path", value: { stringValue: "/apps/x" } },
        { key: "http.response.status_code", value: { intValue: "500" } }, { key: "ratio", value: { doubleValue: 0.5 } }]);
    const traces = otlp.traces([span], { "session.id": "s" });
    assert.equal(traces.resourceSpans[0].scopeSpans[0].spans[0], span);
    const log = otlp.log({ time: 2000, message: "boom", attributes: { "exception.type": "TypeError" } });
    assert.equal(log.severityNumber, 17);
    assert.equal(log.body.stringValue, "boom");
    assert.equal(otlp.logs([log], {}).resourceLogs[0].scopeLogs[0].logRecords.length, 1);
});
test("only the path of an address is kept, and sampling is decided from the id", () => {
    assert.equal(otlp.pathOf("/qapps/system?token=secret#x"), "/qapps/system");
    assert.equal(otlp.pathOf("https://elsewhere.test/a?b=1"), "https://elsewhere.test/a");
    assert.equal(otlp.sampled("00000000ffff", 0.01), true);
    assert.equal(otlp.sampled("ffffffff0000", 0.99), false);
    assert.equal(otlp.sampled("anything", 1), true);
    assert.equal(otlp.sampled("00000000", 0), false);
    assert.match(otlp.randomHex(32), /^[0-9a-f]{32}$/);
});
test("a replay masks every input and all text but what the unmask selector names", () => {
    const options = otlp.replayOptions({ replayUnmaskSelector: ".m-menu", replayBlockSelector: ".secret" }, () => {});
    assert.equal(options.maskAllInputs, true);
    assert.equal(options.maskTextSelector, "*");
    assert.equal(options.recordCanvas, false);
    assert.match(options.blockSelector, /\[data-moqui-block\]/);
    assert.match(options.blockSelector, /\.secret/);
    const menu = document.createElement("div"); menu.className = "m-menu"; const label = document.createElement("span"); menu.appendChild(label);
    const other = document.createElement("span");
    assert.equal(options.maskTextFn("Orders", label), "Orders");
    assert.equal(options.maskTextFn("Jane Doe 42", other), "**** *** **");
});
test("with RUM on, a request carries a traceparent and its CLIENT span is posted with the session token", async () => {
    const config = document.createElement("script");
    config.type = "application/json"; config.id = "moqui-config";
    config.textContent = JSON.stringify({ rum: { sampleRatio: 1, errorSampleRatio: 1, replay: false } });
    document.body.appendChild(config);
    const sent = [];
    moqui.request = (url, options) => Promise.resolve({ status: 200, url, options });
    window.fetch = (url, init) => { sent.push({ url, init }); return Promise.resolve({ ok: true }); };
    await new Promise((resolve, reject) => {
        const script = document.createElement("script");
        script.src = "/js/WebrootRum.js?rum-on"; script.onload = resolve; script.onerror = reject;
        document.head.appendChild(script);
    });
    const result = await moqui.request("/apps/system/dashboard?x=1", { method: "get" });
    assert.match(result.options.headers.traceparent, /^00-[0-9a-f]{32}-[0-9a-f]{16}-01$/);
    moqui.sessionToken = "token-1";
    moqui.rum.flush();
    const post = sent.find((item) => item.url === "/rum/traces");
    assert.ok(post, JSON.stringify(sent.map((item) => item.url)));
    assert.equal(post.init.headers["X-CSRF-Token"], "token-1");
    const body = JSON.parse(post.init.body);
    const span = body.resourceSpans[0].scopeSpans[0].spans.find((item) => item.name === "GET");
    assert.equal(span.kind, 3);
    assert.equal(span.traceId + "-" + span.spanId, result.options.headers.traceparent.slice(3, 52));
    assert.ok(span.attributes.some((item) => item.key === "url.path" && item.value.stringValue === "/apps/system/dashboard"));
    moqui.rum.error(new TypeError("rum fixture"), "test");
    moqui.rum.flush();
    const logs = sent.find((item) => item.url === "/rum/logs");
    assert.ok(logs);
    assert.equal(JSON.parse(logs.init.body).resourceLogs[0].scopeLogs[0].logRecords[0].body.stringValue, "rum fixture");
});
