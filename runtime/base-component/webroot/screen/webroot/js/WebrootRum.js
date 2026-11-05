/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * Real-user monitoring of a signed-in qvt page (plans/Monitoring.md, Later: Browser RUM; <telemetry><browser> in the conf):
 *   - a documentLoad span from Navigation Timing, with its web vitals (FCP, LCP, CLS, INP, TTFB) and a child span for
 *     each slow resource,
 *   - a navigation span for each in-app page change ($root.setUrl), whose requests are its children,
 *   - a CLIENT span for each moqui.request to this site, which sends a W3C traceparent, so the server's request span
 *     joins the trace as the browser span's child (X-Moqui-Page-Trace still names the page's own request),
 *   - the page's JavaScript errors (window error, unhandledrejection, Vue's errorHandler) as OTLP logs,
 * batched as OTLP/JSON and posted to /rum/traces and /rum/logs every 10 seconds and when the page is hidden, with the
 * session's CSRF token. The server replaces the page's identity with the session's (moqui.monitor receive#BrowserTelemetry).
 * With replay on and this tab session sampled, rrweb (libs/rrweb) records a replay with every input and all text masked,
 * posted in chunks to /rum/replay until its time or size limit. qapps.xml loads this only when RUM is on for a signed-in
 * user, right after MoquiLib.js. Nothing here may break the page: every hook catches its own errors, and a failed post
 * is dropped. The pure builders are moqui.rum.otlp, for the specs (test/client/WebrootRum.spec.js).
 */
(function (root) {
    "use strict";
    const moqui = root.moqui;
    if (!moqui) return;

    const hex = (bytes) => Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
    const randomHex = (length) => {
        const bytes = new Uint8Array(length / 2);
        if (root.crypto && root.crypto.getRandomValues) root.crypto.getRandomValues(bytes);
        else for (let i = 0; i < bytes.length; i++) bytes[i] = Math.floor(Math.random() * 256);
        return /^0+$/.test(hex(bytes)) ? randomHex(length) : hex(bytes);
    };
    // Epoch milliseconds (fractional) as the decimal nanoseconds OTLP/JSON takes; past 2^53, so through BigInt.
    const nanos = (ms) => (BigInt(Math.floor(ms)) * 1000000n + BigInt(Math.round((ms - Math.floor(ms)) * 1e6))).toString();
    const attribute = (key, value) => {
        if (typeof value === "boolean") return { key, value: { boolValue: value } };
        if (typeof value === "number") return Number.isInteger(value) ? { key, value: { intValue: String(value) } } : { key, value: { doubleValue: value } };
        return { key, value: { stringValue: String(value) } };
    };
    const attributes = (values) => Object.keys(values || {}).filter((key) => values[key] !== undefined && values[key] !== null && values[key] !== "")
        .map((key) => attribute(key, values[key]));
    // The path only: a query or fragment can carry what the page was given.
    const pathOf = (url) => {
        try { const parsed = new URL(url, root.location.href); return parsed.origin === root.location.origin ? parsed.pathname : parsed.origin + parsed.pathname; } catch (_) { return ""; }
    };
    // A share of sessions, decided from an id so one tab session is in or out for all its signals.
    const sampled = (id, ratio) => ratio >= 1 || (ratio > 0 && parseInt(String(id).slice(0, 8), 16) / 0xffffffff < ratio);

    const utf8Length = (text) => new root.TextEncoder().encode(text).byteLength;
    // One in-flight request and one bounded pending chunk. Serialized events are immutable snapshots; never split one.
    const replayQueue = (options) => {
        const cap = (value, fallback) => {
            if (value == null) return fallback;
            const number = Number(value);
            return Number.isFinite(number) ? Math.max(0, Math.floor(number)) : 0;
        };
        const bodyLimit = cap(options.bodyLimit, 1048576), maxBytes = cap(options.maxBytes, 5242880);
        const clock = options.now || Date.now, started = clock(), maxMs = cap(options.maxMinutes, 30) * 60000;
        let parts = [], partBytes = 0, reservedBytes = 0, inFlight = false, inFlightBytes = 0, seq = 0;
        let accepting = true, failed = false, reason = '', chunkStart = started, chunkUrl = '', traces = new Set();
        const arrayBytes = (count = parts.length, bytes = partBytes) => 2 + bytes + Math.max(0, count - 1);
        const header = (reserve, url = chunkUrl, ids = Array.from(traces)) => JSON.stringify({ replayId: options.replayId,
            seq: reserve ? 999999 : seq, start: reserve ? Number.MAX_SAFE_INTEGER : chunkStart,
            end: reserve ? Number.MAX_SAFE_INTEGER : clock(), url, traceIds: ids, events: [] });
        const requestBytes = (count, bytes) => utf8Length(header(true)) + bytes + Math.max(0, count - 1);
        const clear = () => { parts = []; partBytes = 0; traces.clear(); };
        const stop = (why, discard = true) => {
            const firstFailure = discard && !failed;
            if (discard) { failed = true; clear(); }
            if (accepting || firstFailure) { accepting = false; reason = why; options.onStop && options.onStop(why); }
            if (!discard) flush();
        };
        const flush = () => {
            if (failed || inFlight) return false;
            if (accepting && clock() - started >= maxMs) { stop('time limit reached', false); return false; }
            if (!parts.length || (options.ready && !options.ready())) return false;
            if (seq >= 1000000) { stop('chunk limit reached'); return false; }
            const eventsBytes = arrayBytes(), body = header(false).slice(0, -3) + '[' + parts.join(',') + ']}';
            const bytes = utf8Length(body);
            if (bytes > bodyLimit) { stop('request size limit reached'); return false; }
            if (reservedBytes + eventsBytes > maxBytes) { stop('event size limit reached'); return false; }
            // Reserve before fetch: a slow response must not permit more than the cumulative budget.
            reservedBytes += eventsBytes; inFlight = true; inFlightBytes = bytes; seq++; clear();
            let sent;
            try { sent = options.send(body); } catch (_) { sent = Promise.reject(); }
            Promise.resolve(sent).then((result) => {
                inFlight = false; inFlightBytes = 0;
                if (!result || !result.ok) {
                    const status = Number(result && result.status) || 0;
                    stop(status ? 'request rejected (HTTP ' + status + ')' : 'request failed');
                } else flush();
            }, () => { inFlight = false; inFlightBytes = 0; stop('request failed'); });
            return true;
        };
        return {
            get accepting() { return accepting; },
            get status() { return { accepting, reason, queuedBytes: parts.length ? arrayBytes() : 0, reservedBytes, inFlightBytes }; },
            add(event) {
                if (!accepting) return false;
                if (clock() - started >= maxMs) { stop('time limit reached', false); return false; }
                let encoded;
                try { encoded = JSON.stringify(event); } catch (_) { stop('event could not be encoded'); return false; }
                if (typeof encoded !== 'string') { stop('event could not be encoded'); return false; }
                const bytes = utf8Length(encoded);
                const nextUrl = String(options.url()).slice(0, 2048);
                if (!parts.length) { chunkStart = clock(); chunkUrl = nextUrl; }
                if (requestBytes(parts.length + 1, partBytes + bytes) > bodyLimit) {
                    // An event may fit a fresh envelope even when the preceding chunk has many optional traces.
                    if (utf8Length(header(true, nextUrl, [])) + bytes > bodyLimit) {
                        stop('one event exceeds the request size limit'); return false;
                    }
                    if (parts.length && !flush()) { stop('pending replay buffer is full', false); return false; }
                    chunkStart = clock(); chunkUrl = nextUrl; traces.clear();
                }
                if (reservedBytes + arrayBytes(parts.length + 1, partBytes + bytes) > maxBytes) {
                    stop('event size limit reached', false); return false;
                }
                parts.push(encoded); partBytes += bytes;
                if (parts.length >= 300) flush();
                return true;
            },
            trace(id) {
                if (!accepting || !/^[0-9a-f]{32}$/i.test(String(id)) || traces.size >= 200 || traces.has(id)) return;
                traces.add(id);
                // Trace context is optional; do not make an already queued event exceed the request cap.
                if (requestBytes(parts.length, partBytes) > bodyLimit) traces.delete(id);
            },
            flush,
            stop
        };
    };

    const otlp = {
        nanos, pathOf, sampled, randomHex, replayQueue,
        span(item) {
            const span = { traceId: item.traceId, spanId: item.spanId, name: item.name, kind: item.kind === "client" ? 3 : 1,
                startTimeUnixNano: nanos(item.start), endTimeUnixNano: nanos(Math.max(item.end, item.start)), attributes: attributes(item.attributes) };
            if (item.parentSpanId) span.parentSpanId = item.parentSpanId;
            if (item.error) span.status = { code: 2, message: String(item.error).slice(0, 500) };
            return span;
        },
        log(item) {
            const record = { timeUnixNano: nanos(item.time), severityNumber: 17, severityText: "ERROR", body: { stringValue: String(item.message).slice(0, 2000) },
                attributes: attributes(item.attributes) };
            if (item.traceId) record.traceId = item.traceId;
            if (item.spanId) record.spanId = item.spanId;
            return record;
        },
        traces(spans, resource) {
            return { resourceSpans: [{ resource: { attributes: attributes(resource) }, scopeSpans: [{ scope: { name: "moqui-rum" }, spans }] }] };
        },
        logs(records, resource) {
            return { resourceLogs: [{ resource: { attributes: attributes(resource) }, scopeLogs: [{ scope: { name: "moqui-rum" }, logRecords: records }] }] };
        },
        // rrweb's masking: all text masked unless an element matches unmask; inputs always.
        replayOptions(settings, emit) {
            const unmask = settings.replayUnmaskSelector || "";
            const block = ["[data-moqui-block]", "canvas", "video", "iframe"].concat(settings.replayBlockSelector ? [settings.replayBlockSelector] : []).join(", ");
            return { emit, maskAllInputs: true, maskTextSelector: "*", blockSelector: block, recordCanvas: false, collectFonts: false,
                inlineImages: false, inlineStylesheet: true, recordCrossOriginIframes: false, checkoutEveryNms: 5 * 60 * 1000,
                maskTextFn: (text, element) => {
                    try { if (unmask && element && element.closest && element.closest(unmask)) return text; } catch (_) { /* a bad selector masks */ }
                    return String(text).replace(/\S/g, "*");
                } };
        }
    };
    const rum = moqui.rum = { otlp };

    let settings = null;
    try {
        const block = root.document && root.document.getElementById("moqui-config");
        settings = block ? (JSON.parse(block.textContent || "{}").rum || null) : null;
    } catch (_) { settings = null; }
    if (!settings || !root.fetch || !root.performance) return;

    let sessionId = "";
    try { sessionId = root.sessionStorage.getItem("moqui.rum.session") || ""; } catch (_) { /* storage blocked */ }
    if (!/^[0-9a-f]{32}$/.test(sessionId)) {
        sessionId = randomHex(32);
        try { root.sessionStorage.setItem("moqui.rum.session", sessionId); } catch (_) { /* storage blocked */ }
    }
    const traced = sampled(sessionId, Number(settings.sampleRatio));
    const errorsKept = sampled(sessionId.slice(8), Number(settings.errorSampleRatio));
    const nav = root.navigator || {};
    const resource = { "session.id": sessionId, "user_agent.original": nav.userAgent, "browser.language": nav.language,
        "browser.platform": nav.userAgentData ? nav.userAgentData.platform : nav.platform, "telemetry.sdk.name": "moqui-rum" };
    const now = () => root.performance.timeOrigin + root.performance.now();
    const spans = [];
    const logs = [];
    // the sampled tab session's replay recorder, set below
    let replay = null;
    const MAX_QUEUE = 500;
    const queue = (list, item) => { if (list.length < MAX_QUEUE) list.push(item); };
    rum.sessionId = sessionId;

    // ---- posting ----
    const post = (path, body, details = false) => {
        const token = moqui.sessionToken;
        if (!token) return Promise.resolve(details ? { ok: false, status: 0 } : false);
        return root.fetch(path, { method: "POST", keepalive: utf8Length(body) < 60000, credentials: "same-origin",
            headers: { "Content-Type": "application/json", "X-CSRF-Token": token }, body })
            .then((response) => details ? { ok: response.ok, status: response.status } : response.ok,
                () => details ? { ok: false, status: 0 } : false);
    };
    // Batches small enough for a keepalive post.
    const batches = (items, build) => {
        const out = [];
        let size = 100;
        for (let at = 0; at < items.length;) {
            const part = items.slice(at, at + size);
            const body = JSON.stringify(build(part));
            if (body.length > 60000 && size > 1) { size = Math.max(1, Math.floor(size / 2)); continue; }
            out.push(body);
            at += part.length;
        }
        return out;
    };
    const flush = () => {
        try {
            if (!moqui.sessionToken) return;
            if (spans.length) for (const body of batches(spans.splice(0), (part) => otlp.traces(part, resource))) post("/rum/traces", body);
            if (logs.length) for (const body of batches(logs.splice(0), (part) => otlp.logs(part, resource))) post("/rum/logs", body);
            if (replay) replay.flush();
        } catch (error) { console.info("RUM flush failed", error); }
    };
    rum.flush = flush;
    root.setInterval(flush, 10000);
    root.addEventListener("visibilitychange", () => { if (root.document.visibilityState === "hidden") { finishLoad(); flush(); } });
    root.addEventListener("pagehide", () => { finishLoad(); flush(); });

    // ---- the page load and its vitals ----
    const vitals = { lcp: 0, cls: 0, inp: 0, fcp: 0 };
    const observe = (type, handle) => {
        try { new root.PerformanceObserver((list) => list.getEntries().forEach(handle)).observe({ type, buffered: true }); } catch (_) { /* not supported */ }
    };
    observe("largest-contentful-paint", (entry) => { vitals.lcp = entry.startTime; });
    observe("paint", (entry) => { if (entry.name === "first-contentful-paint") vitals.fcp = entry.startTime; });
    observe("layout-shift", (entry) => { if (!entry.hadRecentInput) vitals.cls += entry.value; });
    observe("event", (entry) => { if (entry.interactionId && entry.duration > vitals.inp) vitals.inp = entry.duration; });
    const loadTrace = randomHex(32);
    const loadSpan = randomHex(16);
    let loadDone = false;
    const finishLoad = () => {
        if (loadDone || !traced) return;
        const entry = root.performance.getEntriesByType ? root.performance.getEntriesByType("navigation")[0] : null;
        if (!entry || !entry.loadEventEnd) return;
        loadDone = true;
        const origin = root.performance.timeOrigin;
        queue(spans, otlp.span({ traceId: loadTrace, spanId: loadSpan, name: "documentLoad", start: origin, end: origin + entry.loadEventEnd,
            attributes: { "url.path": pathOf(root.location.href), "moqui.page.trace_id": moqui.pageTraceId || undefined,
                "browser.web_vital.ttfb": Math.round(entry.responseStart), "browser.web_vital.fcp": Math.round(vitals.fcp),
                "browser.web_vital.lcp": Math.round(vitals.lcp), "browser.web_vital.cls": Math.round(vitals.cls * 1000) / 1000,
                "browser.web_vital.inp": Math.round(vitals.inp), "browser.dom_content_loaded": Math.round(entry.domContentLoadedEventEnd),
                "http.response.body.size": entry.transferSize || undefined } }));
        const slow = (root.performance.getEntriesByType("resource") || []).filter((item) => item.duration > 1000).slice(0, 20);
        for (const item of slow) {
            queue(spans, otlp.span({ traceId: loadTrace, spanId: randomHex(16), parentSpanId: loadSpan, name: "resource " + item.initiatorType,
                start: origin + item.startTime, end: origin + item.responseEnd, attributes: { "url.path": pathOf(item.name), "http.response.body.size": item.transferSize || undefined } }));
        }
    };
    // LCP settles once the user acts or the page has shown for a while.
    root.addEventListener("load", () => root.setTimeout(finishLoad, 5000));

    // ---- in-app navigation ----
    let current = null;
    const endNavigation = (navigation) => {
        if (current !== navigation || navigation.pending > 0) return;
        current = null;
        root.clearTimeout(navigation.timer);
        queue(spans, otlp.span({ traceId: navigation.traceId, spanId: navigation.spanId, name: "navigation", start: navigation.start, end: now(),
            attributes: { "url.path": navigation.path, "moqui.navigation.requests": navigation.requests } }));
    };
    rum.navigate = (url) => {
        try {
            if (!traced) return;
            if (current) { current.pending = 0; endNavigation(current); }
            const navigation = { traceId: randomHex(32), spanId: randomHex(16), start: now(), path: pathOf(url), pending: 0, requests: 0 };
            navigation.timer = root.setTimeout(() => { navigation.pending = 0; endNavigation(navigation); }, 30000);
            current = navigation;
            // a navigation with no request ends after the next frame
            root.setTimeout(() => endNavigation(navigation), 0);
            if (replay) replay.trace(navigation.traceId);
        } catch (error) { console.info("RUM navigation not recorded", error); }
    };

    // ---- requests ----
    const request = moqui.request;
    moqui.request = (url, options) => {
        let item = null;
        try {
            if (traced && new URL(url, root.location.href).origin === root.location.origin) {
                const navigation = current;
                item = { traceId: navigation ? navigation.traceId : randomHex(32), spanId: randomHex(16), parentSpanId: navigation ? navigation.spanId : undefined,
                    start: now(), navigation, method: ((options && options.method) || "GET").toUpperCase(), path: pathOf(url) };
                if (navigation) { navigation.pending++; navigation.requests++; }
                options = Object.assign({}, options, { headers: Object.assign({}, options && options.headers, { traceparent: "00-" + item.traceId + "-" + item.spanId + "-01" }) });
                if (replay && !navigation) replay.trace(item.traceId);
            }
        } catch (_) { item = null; }
        const done = (status, error) => {
            if (!item) return;
            try {
                queue(spans, otlp.span({ traceId: item.traceId, spanId: item.spanId, parentSpanId: item.parentSpanId, name: item.method, kind: "client",
                    start: item.start, end: now(), error, attributes: { "http.request.method": item.method, "url.path": item.path, "http.response.status_code": status || undefined } }));
                if (item.navigation) { item.navigation.pending--; root.setTimeout(() => endNavigation(item.navigation), 0); }
            } catch (_) { /* never in the page's way */ }
        };
        return request(url, options).then((result) => { done(result.status); return result; },
            (failure) => { done(failure && failure.status, failure && failure.aborted ? "aborted" : "status " + ((failure && failure.status) || "network")); throw failure; });
    };

    // ---- errors ----
    let errorCount = 0;
    const seen = new Set();
    rum.error = (error, where) => {
        try {
            if (!errorsKept || errorCount >= 50) return;
            const message = (error && error.message) || String(error);
            const stack = (error && error.stack) ? String(error.stack).slice(0, 8000) : "";
            const key = message + "\n" + stack.slice(0, 500);
            if (seen.has(key)) return;
            seen.add(key);
            errorCount++;
            const navigation = current;
            queue(logs, otlp.log({ time: now(), message, traceId: navigation ? navigation.traceId : undefined, spanId: navigation ? navigation.spanId : undefined,
                attributes: { "exception.type": (error && error.name) || "Error", "exception.message": message, "exception.stacktrace": stack,
                    "url.path": pathOf(root.location.href), "moqui.error.where": where } }));
        } catch (_) { /* never in the page's way */ }
    };
    root.addEventListener("error", (event) => { if (event && (event.error || event.message)) rum.error(event.error || event.message, "window"); });
    root.addEventListener("unhandledrejection", (event) => rum.error(event && event.reason, "promise"));

    // ---- replay ----
    if (settings.replay && sampled(sessionId.slice(16), Number(settings.replaySampleRatio))) {
        const replayId = sessionId.slice(0, 8) + "-" + randomHex(24);
        let dispose = null, reported = false;
        const recorder = otlp.replayQueue({ replayId, bodyLimit: settings.replayBodyLimit,
            maxBytes: settings.replayMaxBytes, maxMinutes: settings.replayMaxMinutes,
            url: () => pathOf(root.location.href), ready: () => !!moqui.sessionToken,
            send: (body) => post("/rum/replay", body, true),
            onStop(reason) {
                if (dispose) { try { dispose(); } catch (_) { /* gone */ } dispose = null; }
                rum.replayStatus = { state: "stopped", reason };
                // Fixed reasons/status only: never report a captured event or a server response body.
                if (!reported) { console.info("Session replay stopped: " + reason); reported = true; }
            }
        });
        replay = recorder;
        rum.replayId = replayId;
        rum.replayStatus = { state: "starting" };
        moqui.loadScript("/libs/rrweb/rrweb.umd.min.js", null, () => !!root.rrweb).then(() => {
            if (!recorder.accepting) return;
            // record() can synchronously emit a FullSnapshot that stops capture before it returns its disposer.
            const stop = root.rrweb.record(otlp.replayOptions(settings, (event) => recorder.add(event)));
            if (typeof stop !== "function") { recorder.stop("recorder could not start"); return; }
            if (!recorder.accepting) { stop(); }
            else { dispose = stop; rum.replayStatus = { state: "recording" }; }
        }).catch(() => recorder.stop("recorder could not start"));
    }
})(typeof window !== "undefined" ? window : globalThis);
