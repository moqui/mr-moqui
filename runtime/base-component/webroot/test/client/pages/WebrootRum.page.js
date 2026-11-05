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
test("replay request limits admit a default full snapshot and preserve explicit caps", async () => {
    const sent=[],stops=[],pending=[];
    const options={replayId:'ui-review-replay-0001',now:()=>1000,url:()=>'/qapps/gallery',
        send:body=>{sent.push(body);return new Promise(resolve=>pending.push(resolve));},onStop:why=>stops.push(why)};
    const meta={type:4,timestamp:1000,data:{href:'/qapps/gallery',width:1920,height:1080}};
    const snapshot={type:2,timestamp:1001,data:{node:{type:0,childNodes:[{type:3,textContent:'*'.repeat(303008)}]}}};
    const capture=otlp.replayQueue(options);
    assert.equal(capture.add(meta),true);assert.equal(capture.add(snapshot),true);capture.flush();
    assert.equal(sent.length,1);assert.deepEqual(JSON.parse(sent[0]).events,[meta,snapshot]);
    assert.ok(new TextEncoder().encode(sent[0]).length>262144);
    assert.ok(new TextEncoder().encode(sent[0]).length<=1048576);
    assert.equal(JSON.parse(sent[0]).seq,0);assert.deepEqual(stops,[]);
    pending[0]({ok:true,status:200});await Promise.resolve();
    assert.equal(capture.status.inFlightBytes,0);
    const capped=otlp.replayQueue(Object.assign({},options,{bodyLimit:262144}));
    capped.add(meta);assert.equal(capped.add(snapshot),false);
    assert.equal(capped.status.queuedBytes,0,'an oversized first snapshot does not leave a standalone Meta chunk');
    assert.equal(stops[0],'one event exceeds the request size limit');assert.equal(sent.length,1);
    const zero=otlp.replayQueue(Object.assign({},options,{bodyLimit:0}));
    assert.equal(zero.add(meta),false,'an explicit zero is not replaced by the default');
});
test("replay chunks keep UTF-8 events ordered with bounded pending and in-flight data", async () => {
    const sent=[],pending=[],stops=[],bodyLimit=700;
    const capture=otlp.replayQueue({replayId:'ui-review-replay-0002',bodyLimit,maxBytes:10000,now:()=>1000,url:()=>'/qapps/gallery',
        send:body=>{sent.push(body);return new Promise(resolve=>pending.push(resolve));},onStop:why=>stops.push(why)});
    const events=Array.from({length:5},(_,index)=>({type:3,timestamp:1000+index,data:{index,text:'界'.repeat(60)}}));
    for(let index=0;index<4;index++)assert.equal(capture.add(events[index]),true);
    assert.equal(sent.length,1,'only one request is sent before its response');
    assert.ok(capture.status.inFlightBytes<=bodyLimit);
    assert.ok(capture.status.queuedBytes<bodyLimit);
    assert.equal(capture.add(events[4]),false);assert.deepEqual(stops,['pending replay buffer is full']);
    pending[0]({ok:true,status:200});await Promise.resolve();
    assert.equal(sent.length,2);assert.deepEqual(sent.map(body=>JSON.parse(body).seq),[0,1]);
    assert.deepEqual(sent.flatMap(body=>JSON.parse(body).events),events.slice(0,4));
    assert.ok(sent.every(body=>new TextEncoder().encode(body).length<=bodyLimit));
    pending[1]({ok:true,status:200});await Promise.resolve();assert.equal(capture.status.queuedBytes,0);
    // Old trace context must not make an event that fits a fresh chunk appear indivisibly oversized.
    let url='/qapps/old/'+ 'x'.repeat(1000);
    const edgeSent=[],edgePending=[],edgeStops=[];
    const edge=otlp.replayQueue({replayId:'ui-review-replay-0004',bodyLimit:8500,now:()=>1000,url:()=>url,
        send:body=>{edgeSent.push(body);return new Promise(resolve=>edgePending.push(resolve));},onStop:why=>edgeStops.push(why)});
    const earlier={type:3,timestamp:1000,data:{text:'masked'}};
    edge.add(earlier);for(let index=0;index<200;index++)edge.trace(index.toString(16).padStart(32,'0'));
    url='/qapps/new';const nearLimit={type:2,timestamp:1001,data:{text:'*'.repeat(8200)}};
    assert.equal(edge.add(nearLimit),true);assert.equal(edgeSent.length,1);
    assert.equal(JSON.parse(edgeSent[0]).traceIds.length,200);
    edgePending[0]({ok:true,status:200});await Promise.resolve();
    assert.equal(edgeSent.length,2);assert.deepEqual(edgeSent.flatMap(body=>JSON.parse(body).events),[earlier,nearLimit]);
    assert.deepEqual(edgeStops,[]);assert.ok(edgeSent.every(body=>new TextEncoder().encode(body).length<=8500));
    edgePending[1]({ok:true,status:200});await Promise.resolve();
});
test("replay reserves cumulative event bytes before responses and discards pending data on failure", async () => {
    const event={type:3,timestamp:1000,data:{text:'masked'}};
    const eventBytes=new TextEncoder().encode(JSON.stringify([event])).length;
    const sent=[],pending=[],stops=[];
    const options={replayId:'ui-review-replay-0003',maxBytes:eventBytes*2,now:()=>1000,url:()=>'/qapps/gallery',
        send:body=>{sent.push(body);return new Promise(resolve=>pending.push(resolve));},onStop:why=>stops.push(why)};
    const capture=otlp.replayQueue(options);
    capture.add(event);capture.flush();assert.equal(capture.status.reservedBytes,eventBytes);
    assert.equal(capture.add(event),true);assert.equal(capture.add(event),false);
    assert.equal(capture.status.reservedBytes+capture.status.queuedBytes,eventBytes*2);
    pending[0]({ok:true,status:200});await Promise.resolve();pending[1]({ok:true,status:200});await Promise.resolve();
    assert.equal(capture.status.reservedBytes,eventBytes*2);assert.equal(sent.length,2);
    assert.deepEqual(stops,['event size limit reached']);
    const failed=otlp.replayQueue(Object.assign({},options,{maxBytes:10000}));
    failed.add(event);failed.flush();failed.add(event);
    pending[2]({ok:false,status:413,text:'DO NOT LOG CAPTURED CONTENT'});await Promise.resolve();
    assert.equal(sent.length,3);assert.equal(failed.status.queuedBytes,0);assert.equal(failed.add(event),false);
    assert.equal(stops[1],'request rejected (HTTP 413)');
    assert.equal(stops.join(' ').includes('DO NOT LOG'),false);
});
test("with RUM on, a request carries a traceparent and its CLIENT span is posted with the session token", async () => {
    const config = document.createElement("script");
    config.type = "application/json"; config.id = "moqui-config";
    config.textContent = JSON.stringify({ rum: { sampleRatio: 1, errorSampleRatio: 1, replay: true, replaySampleRatio: 1 } });
    document.body.appendChild(config);
    const sent = [];
    moqui.request = (url, options) => Promise.resolve({ status: 200, url, options });
    window.fetch = (url, init) => { sent.push({ url, init }); return Promise.resolve({ ok: true }); };
    const previous={loadScript:moqui.loadScript,rrweb:window.rrweb};
    moqui.loadScript=()=>Promise.resolve();
    window.rrweb={record(options){
        options.emit({type:4,timestamp:1000,data:{href:'/qapps/gallery'}});
        options.emit({type:2,timestamp:1001,data:{node:{attributes:{_cssText:'界'.repeat(25000)}}}});
        return ()=>{};
    }};
    try {
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
        const replay=sent.find(item=>item.url==='/rum/replay');
        assert.ok(replay);
        assert.ok(new TextEncoder().encode(replay.init.body).length>60000);
        assert.equal(replay.init.keepalive,false,'UTF-8 replay bytes determine the keepalive limit');
        assert.deepEqual(JSON.parse(replay.init.body).events.map(event=>event.type),[4,2]);
    } finally {moqui.loadScript=previous.loadScript;window.rrweb=previous.rrweb;}

});
test("a synchronous oversized initial snapshot disposes its recorder after startup returns", async () => {
    const config=document.getElementById('moqui-config');
    const previous={config:config.textContent,loadScript:moqui.loadScript,rrweb:window.rrweb,request:moqui.request,info:console.info};
    const notices=[];let disposed=0;
    config.textContent=JSON.stringify({rum:{sampleRatio:0,errorSampleRatio:0,replay:true,replaySampleRatio:1,replayBodyLimit:512}});
    moqui.loadScript=()=>Promise.resolve();console.info=message=>notices.push(message);
    window.rrweb={record(options){
        options.emit({type:4,timestamp:1000,data:{href:'/qapps/gallery'}});
        options.emit({type:2,timestamp:1001,data:{text:'*'.repeat(2000)}});
        return ()=>disposed++;
    }};
    const script=document.createElement('script');
    try {
        script.src='/js/WebrootRum.js?replay-startup';
        await new Promise((resolve,reject)=>{script.onload=resolve;script.onerror=reject;document.head.appendChild(script);});
        await Promise.resolve();await Promise.resolve();
        assert.equal(disposed,1);
        assert.equal(moqui.rum.replayStatus.state,'stopped');
        assert.equal(moqui.rum.replayStatus.reason,'one event exceeds the request size limit');
        assert.deepEqual(notices,['Session replay stopped: one event exceeds the request size limit']);
    } finally {
        script.remove();config.textContent=previous.config;moqui.loadScript=previous.loadScript;
        window.rrweb=previous.rrweb;moqui.request=previous.request;console.info=previous.info;
    }
});
