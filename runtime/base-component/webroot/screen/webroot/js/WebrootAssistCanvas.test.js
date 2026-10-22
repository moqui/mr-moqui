/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// Tests of the Assist canvas (WebrootAssistCanvas.js) that need no browser: the library spec golden (openui-library.spec.json
// is what the component declarations make), the signatures the skill quotes, the vendored lang-core parsing valid and invalid
// fixtures against that spec, the state reconciliation of a program that changes under the user's edits, the read proxy's
// parameters, the problems a render reports, the chart configurations, the drawer's canvas state (canvas.open,
// canvas.delta, a snapshot), and the parent's half of the Vue SFC sandbox (canvas.sfc: the blocks and actions, the validator
// of every frame message, read paths, link targets, the watchdog, the submit gate and the read limits). The canvas in a page is
// the Chromium smoke of Gate 3 (crates/moqui-host/tests/gate3_browser.rs).
// Run: node --test runtime/base-component/webroot/screen/webroot/js/WebrootAssistCanvas.test.js
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const webroot = path.join(__dirname, "..");
globalThis.moqui = require("./MoquiLib.js");
const assist = require("./WebrootAssist.js");
const canvas = require("./WebrootAssistCanvas.js");
vm.runInThisContext(fs.readFileSync(path.join(webroot, "libs/openui/lang-core.min.js"), "utf8"));
const O = globalThis.OpenUILang;
const spec = JSON.parse(fs.readFileSync(path.join(__dirname, "openui-library.spec.json"), "utf8"));

test("the library spec is what the component declarations make (UPDATE: node -e the librarySpecText)", () => {
    const made = canvas.librarySpecText();
    const kept = fs.readFileSync(path.join(__dirname, "openui-library.spec.json"), "utf8");
    assert.equal(kept, made, "openui-library.spec.json is stale: write canvas.librarySpecText() to it");
    assert.equal(Object.keys(spec.$defs).length, 46);
    assert.deepEqual(Object.keys(spec.$defs.Select.properties), ["name", "value", "items", "label"]);
    assert.deepEqual(spec.$defs.Select.required, ["name", "items"]);
    assert.equal(canvas.signature("Select"), "Select(name, value?, items, label?)");
    assert.equal(canvas.signature("Lookup"), "Lookup(name, value?, optionsUrl, valueField?, labelField?, dependsOn?, label?)");
    // every component is drawn, and every group is known
    canvas.library.forEach((def) => assert.ok(canvas.GROUPS.includes(def.group), def.name));
});

test("the skill gives every signature", () => {
    const skill = fs.readFileSync(path.join(webroot, "../../../framework/skill/openui-lang.md"), "utf8");
    canvas.library.forEach((def) => assert.ok(skill.includes(canvas.signature(def.name)), canvas.signature(def.name)));
});

test("lang-core parses valid programs against the spec and reports invalid ones", () => {
    const parser = O.createParser(spec, "Stack");
    const valid = parser.parse([
        "root = Stack([CardHeader(\"Hits\"), chart])",
        "hits = Query(\"request\", {path: \"/apps/system/ArtifactHitSummary\", query: {artifactType: $type}}, {rows: []})",
        "chart = BarChart(hits.rows.artifactName, [Series(\"Hits\", hits.rows.hitCount)])",
    ].join("\n"));
    assert.equal(valid.root.typeName, "Stack");
    assert.deepEqual(valid.meta.errors, []);
    assert.deepEqual(valid.queryStatements.map((q) => q.statementId), ["hits"]);
    assert.deepEqual(valid.stateDeclarations, { $type: null });
    // a Query whose options are not closed swallows the next line
    const open = parser.parse("root = Stack([t])\nq = Query(\"request\", {path: \"/apps/x\", query: {a: $a}, {rows: []})\nt = Table([Col(\"A\", q.rows.a)])");
    assert.equal(open.meta.incomplete, true);
    assert.ok(open.meta.unresolved.includes("t"));
    // an unknown component and a child that is not an array
    const bad = parser.parse("root = Stack([Chartt([1]), Card(Table([]))])");
    const codes = bad.meta.errors.map((error) => error.code).sort();
    assert.deepEqual(codes, ["type-mismatch", "unknown-component"]);
    // the streaming parser draws a program as it comes
    const streaming = O.createStreamingParser(spec, "Stack");
    const part = streaming.set("root = Stack([CardHeader(\"Hi\"), more])\nmore = TextContent(\"x");
    assert.equal(part.root.typeName, "Stack");
    // the patch merge of the server and the page
    const merged = O.mergeStatements("root = Stack([a, b])\na = TextContent(\"1\")\nb = TextContent(\"2\")", "b = null\nroot = Stack([a])");
    assert.equal(merged, "root = Stack([a])\na = TextContent(\"1\")");
});

test("state reconciliation: a changed default replaces only what the user left alone", () => {
    // the user typed into $q; $days still holds the old default; the program changes both defaults
    const prev = { $q: "", $days: "7" };
    const decls = { $q: "x", $days: "14" };
    const persisted = canvas.persistedState(decls, prev, { q: "typed", days: "7" });
    assert.deepEqual(persisted, { $q: "typed", $days: "14" });
    // the first parse keeps what the page had
    assert.deepEqual(canvas.persistedState({ $q: "" }, null, { q: "kept" }), { $q: "kept" });
    // edits that only hold a default, or of a name the program dropped, no longer count
    const edited = canvas.reconcileUserEdits({ $q: true, $days: true, $gone: true }, prev, decls, { $q: "typed", $days: "14" });
    assert.deepEqual(edited, { $q: true });
    assert.equal(canvas.sameLiteral({ value: 3 }, "3"), true);
    assert.deepEqual(canvas.valuesOf({ $q: "a", form: { name: { value: "n", componentType: "Input" } }, __openui_loading: [] }), { q: "a", name: "n" });
});

test("reads go to the proxy with GET only, and paths of the apps only", () => {
    assert.deepEqual(canvas.readParams("A1", { path: "/apps/system/Visit/VisitList", list: "VisitList", query: { q: "x", empty: "", n: 3 } }),
        { artifactId: "A1", path: "/apps/system/Visit/VisitList", list: "VisitList", query: "{\"q\":\"x\",\"n\":\"3\"}" });
    assert.ok(canvas.readParams("A1", { method: "POST", path: "/apps/x" }).error);
    assert.ok(canvas.readParams("A1", { path: "https://example.com/" }).error);
    assert.ok(canvas.readParams("A1", { path: "/rest/s1/x" }).error);
    assert.equal(canvas.siteImage("/static/a.png"), "/static/a.png");
    assert.equal(canvas.siteImage("//evil/a.png"), null);
    assert.equal(canvas.siteImage("https://evil/a.png"), null);
    // never a screen, a transition or a call of the apps (a GET of one runs it with the cookie), never a query
    assert.equal(canvas.siteImage("/apps/x/deleteThing.png"), null);
    assert.equal(canvas.siteImage("/rest/s1/x.png"), null);
    assert.equal(canvas.siteImage("/static/a.png?d=x"), null);
    assert.equal(canvas.siteImage("/static/a"), null);
    // no url() or other style reaches a component's size
    assert.equal(canvas.cssLength("320px", "300px"), "320px");
    assert.equal(canvas.cssLength("1px;background:url(https://evil/)", "300px"), "300px");
});

test("the problems of a render are each once, and the screen's refusals are errors", () => {
    const problems = canvas.problemsOf({ root: null, meta: { errors: [{ code: "unknown-component", message: "Unknown X", statementId: "root" }], unresolved: ["t", "t"] } },
        [{ code: "runtime-error", message: "bad" }], [{ source: "query", code: "tool-error", message: "no list", statementId: "q" }]);
    assert.deepEqual(problems.map((one) => one.code), ["unknown-component", "unresolved", "no-root", "runtime-error", "tool-error"]);
    const refused = canvas.responseProblems({ validationErrors: [{ message: "is required", field: "description" }], messageInfos: [{ type: "warning", message: "careful" }] });
    assert.deepEqual(refused, { errors: ["is required (for field description)"], warnings: ["careful"] });
});

test("charts: Chart.js 4 configurations from the series, numbers as a list shows them", () => {
    const bar = canvas.xyChart("bar", { labels: ["a", "b"], series: [{ type: "element", props: { category: "Hits", values: ["1,200", "3"] } }], variant: "stacked", yLabel: "Hits" });
    assert.deepEqual(bar.data, { labels: ["a", "b"], datasets: [{ label: "Hits", data: [1200, 3], backgroundColor: "#1976d2", borderColor: "#1976d2" }] });
    assert.equal(bar.options.scales.x.stacked, true);
    assert.equal(bar.options.scales.y.title.text, "Hits");
    const line = canvas.xyChart("line", { labels: [1], series: [{ category: "A", values: [2] }], variant: "step" }, true);
    assert.equal(line.data.datasets[0].stepped, true);
    assert.equal(line.data.datasets[0].fill, true);
    const pie = canvas.pieChart({ labels: ["x", "y", "z"], values: [1, 2], variant: "donut" });
    assert.equal(pie.type, "doughnut");
    assert.deepEqual(pie.data.labels, ["x", "y"]);
    assert.equal(canvas.dateValue("2026-10-22 10:00", "date-time"), "2026-10-22 10:00");
    assert.match(canvas.dateValue("now", "date"), /^\d{4}-\d{2}-\d{2}$/);
});

test("the drawer's canvas: canvas.open replaces it, canvas.delta streams a whole program, a snapshot keeps what the tab did", () => {
    const state = assist.emptyState();
    state.conversationId = "C1";
    state.activeRunId = "R1";
    state.seq.R1 = 0;
    const ctx = { clientId: "tab-a", followOn: true };
    // a whole program streams in: drawn as it comes
    let effects = assist.apply(state, "R1", { seq: null, type: "canvas.delta", payload: { messageId: "M1", index: 0, patch: false, offset: 0, text: "root = Stack([" } }, ctx);
    assert.deepEqual(effects, [{ kind: "canvas" }]);
    assist.apply(state, "R1", { seq: null, type: "canvas.delta", payload: { messageId: "M1", index: 0, offset: 14, text: "a])" } }, ctx);
    assert.equal(assist.canvasView(state).program, "root = Stack([a])");
    assert.equal(assist.canvasView(state).streaming, true);
    effects = assist.apply(state, "R1", { seq: 1, type: "canvas.open", payload: { artifactId: 7, version: 1, title: "Hits", kind: "openui", program: "root = Stack([a])\na = TextContent(\"x\")", problems: [] } }, ctx);
    assert.deepEqual(effects, [{ kind: "canvas" }]);
    assert.equal(state.canvasDraft, null);
    const view = assist.canvasView(state);
    assert.deepEqual([view.artifactId, view.version, view.streaming, view.title], ["7", 1, false, "Hits"]);
    // the tab hides it; the same version from a snapshot stays hidden, a newer one shows
    state.canvas.hidden = true;
    assist.takeCanvas(state, { artifactId: "7", version: 1, title: "Hits", program: "x" });
    assert.equal(state.canvas.hidden, true);
    assist.takeCanvas(state, { artifactId: "7", version: 0, title: "Old", program: "y" });
    assert.equal(state.canvas.title, "Hits", "an older version does not replace a newer one");
    assist.takeCanvas(state, { artifactId: "7", version: 2, title: "Hits", program: "z" });
    assert.equal(state.canvas.hidden, false);
    // a patch streams with no drawing: the canvas shows that it updates
    assist.apply(state, "R1", { seq: null, type: "canvas.delta", payload: { messageId: "M2", index: 0, patch: true, offset: 0, text: "a = null" } }, ctx);
    assert.equal(assist.canvasView(state).program, "z");
    assert.equal(assist.canvasView(state).updating, true);
    // a closed canvas is not drawn
    state.canvasDraft = null;
    state.canvas.closed = true;
    assert.equal(assist.canvasView(state), null);
    assist.snapshot(state, { conversation: { conversationId: "C1" }, items: [], canvas: null }, ctx);
    assert.equal(state.canvas, null);
});

test("the dialect registers itself", () => {
    assert.equal(typeof assist.dialects.register, "function");
    // both dialects register their components in a browser only (the renderer needs Vue)
    assert.equal(assist.dialects.get("vue-sfc"), null);
    const source = fs.readFileSync(path.join(__dirname, "WebrootAssistCanvas.js"), "utf8");
    assert.match(source, /dialects\.register\("vue-sfc", \{ label: "Vue SFC", component: "m-assist-vue-sfc"/);
});

test("a Mutation runs only from a trusted click, and its Button says what it saves", () => {
    const plan = { steps: [{ type: "set", target: "$a" }, { type: "run", statementId: "save", refType: "mutation" }, { type: "run", statementId: "q", refType: "query" }] };
    assert.deepEqual(canvas.mutationSteps(plan).map((step) => step.statementId), ["save"]);
    assert.deepEqual(canvas.mutationSteps({ steps: [{ type: "run", statementId: "q", refType: "query" }] }), []);
    assert.equal(canvas.savesText("/apps/assistgate/Edit/updateEnumType"), "updateEnumType");
    const active = { userActivation: { isActive: true } };
    assert.equal(canvas.trustedClick({ isTrusted: true }, active), true);
    assert.equal(canvas.trustedClick({ isTrusted: false }, active), false, "a scripted click");
    assert.equal(canvas.trustedClick({ isTrusted: true }, { userActivation: { isActive: false } }), false);
    assert.equal(canvas.trustedClick(null, active), false, "a refresh or a render has no click");
    // the renderer has no other way to a Mutation: a Query's arguments go to the read proxy
    const source = fs.readFileSync(path.join(__dirname, "WebrootAssistCanvas.js"), "utf8");
    assert.doesNotMatch(source, /__mutation|this\.activation/);
    assert.match(source, /args\[RUN\]/);
});

// ---- Vue SFC: the parent's half of the sandbox ----

const sfc = canvas.sfc;
const PROGRAM = [
    "<template>",
    "  <div><template v-if=\"rows.length\"><b>{{ rows.length }}</b></template><q-btn label=\"Save\" @click=\"save\"></q-btn></div>",
    "</template>",
    "",
    "<script>",
    "module.exports = { data: function () { return { rows: [] }; }, methods: { save: function () { this.$bridge.submit('save', { description: 'x' }); } } };",
    "</script>",
    "",
    "<actions>",
    "[{\"name\": \"save\", \"label\": \"Save it\", \"path\": \"/apps/assistgate/Edit/updateEnumType\", \"fields\": [\"description\"], \"parameters\": {\"enumTypeId\": \"AST_GATE\"}}]",
    "</actions>",
].join("\n");

test("the blocks and the actions of a program are the server's", () => {
    const blocks = sfc.blocks(PROGRAM);
    assert.deepEqual(Object.keys(blocks), ["template", "script", "actions"]);
    assert.ok(blocks.template.indexOf("<template v-if=\"rows.length\">") >= 0, "a nested template is the template's own");
    assert.ok(blocks.script.indexOf("module.exports") >= 0);
    assert.equal(sfc.blocks("<div></div>").error, "text outside the blocks");
    assert.equal(sfc.blocks("<script>module.exports = {}</script>").error, "no <template>");
    assert.equal(sfc.blocks("<template><div></div></template><template><b></b></template>").error, "a second <template> block");
    assert.equal(sfc.blocks("<template><div>").error, "the <template> block does not close");
    const actions = sfc.actions(PROGRAM);
    assert.deepEqual(actions.map((action) => [action.name, action.label, action.method, action.path]), [["save", "Save it", "POST", "/apps/assistgate/Edit/updateEnumType"]]);
    // a confirm shows exactly what the server posts: the fixed parameters and the action's fields, nothing else
    assert.deepEqual(sfc.confirmRows(actions[0], { description: "New", other: "not sent" }),
        [{ name: "enumTypeId", value: "AST_GATE", fixed: true }, { name: "description", value: "New", fixed: false }]);
    assert.deepEqual(sfc.confirmRows(actions[0], {})[1], { name: "description", value: null, fixed: false });
    assert.deepEqual(sfc.actions("<template><div></div></template><actions>not json</actions>"), []);
});

test("every frame message is validated, and only its own fields pass", () => {
    const pass = (data) => { const out = sfc.validate(data); assert.equal(out.ok, true, JSON.stringify(data) + " " + out.reason); return out.message; };
    const fail = (data, reason) => { const out = sfc.validate(data); assert.equal(out.ok, false, JSON.stringify(data)); assert.equal(out.reason, reason, JSON.stringify(data)); return out; };
    fail(null, "shape");
    fail("ready", "shape");
    fail([{ t: "ready" }], "shape");
    fail({ type: "ready" }, "shape");
    fail({ t: "navigate", url: "https://x" }, "type");
    fail({ t: "port", nonce: "x" }, "type");
    fail({ t: "input", values: { a: "x".repeat(300000) } }, "size");
    assert.deepEqual(pass({ t: "ready", extra: "dropped" }), { t: "ready" });
    assert.deepEqual(pass({ t: "ping" }), { t: "ping" });
    // input: flat values only, plain names, no __, at most 100
    assert.deepEqual(pass({ t: "input", values: { a: "x", n: 2, ok: true, none: null, list: ["p", 1] } }).values, { a: "x", n: 2, ok: true, none: null, list: ["p", 1] });
    fail({ t: "input", values: { a: { b: 1 } } }, "values");
    fail({ t: "input", values: { "__proto__x": 1 } }, "values");
    fail({ t: "input", values: { "a b": 1 } }, "values");
    fail({ t: "input", values: ["a"] }, "values");
    fail({ t: "input", values: { a: "x".repeat(4001) } }, "values");
    const many = {};
    for (let i = 0; i < 101; i++) many["v" + i] = i;
    fail({ t: "input", values: many }, "values");
    fail({ t: "input", values: { n: Infinity } }, "values");
    // reads: an /apps path of plain characters, a flat query, a list name; the /qapps twin reads as /apps
    assert.deepEqual(pass({ t: "get", id: "r1", path: "/qapps/system/Visit/VisitList", query: { pageSize: 20, none: null }, list: "VisitList" }),
        { t: "get", id: "r1", path: "/apps/system/Visit/VisitList", query: { pageSize: "20" }, list: "VisitList" });
    for (const bad of ["https://evil.example.com/x", "//evil.example.com/x", "/apps/../etc/passwd", "/apps/%2e%2e/x", "/rest/s1/moqui/x", "/assist/read",
        "/apps/a b", "/apps/x?y=1", "apps/x", "/apps//x", "/apps/x\\y"]) {
        const out = fail({ t: "get", id: "r2", path: bad }, "path");
        assert.equal(out.id, "r2", "a refused read is answered to its id");
    }
    fail({ t: "get", id: "r3", path: "/apps/x", query: { a: { b: 1 } } }, "query");
    fail({ t: "get", id: "r3", path: "/apps/x", query: ["a"] }, "query");
    fail({ t: "get", id: "r3", path: "/apps/x", query: { "a b": "1" } }, "query");
    fail({ t: "get", id: "r3", path: "/apps/x", list: "a;b" }, "list");
    fail({ t: "get", id: "no spaces", path: "/apps/x" }, "id");
    // a submit names an action and carries flat values
    assert.deepEqual(pass({ t: "submit", id: "r4", action: "save", values: { description: "x" }, extra: 1 }), { t: "submit", id: "r4", action: "save", values: { description: "x" } });
    fail({ t: "submit", id: "r4", action: "save(1)" }, "action");
    fail({ t: "submit", id: "r4", action: "save", values: { a: [{ b: 1 }] } }, "values");
    // the rest
    assert.equal(pass({ t: "resize", height: 99999 }).height, 4000);
    assert.equal(pass({ t: "resize", height: 5 }).height, 60);
    fail({ t: "resize", height: "tall" }, "height");
    assert.deepEqual(pass({ t: "error", phase: "boom", code: "<b>", message: 7 }), { t: "error", phase: "runtime", code: "error", message: "7" });
    assert.equal(pass({ t: "error", phase: "compile", code: "template", message: "x".repeat(3000) }).message.length, 2000);
    fail({ t: "link", href: 5 }, "href");
});

test("a read path, a link target and the frame's URL", () => {
    assert.deepEqual(sfc.readPath("/apps/system/Visit"), { ok: true, path: "/apps/system/Visit" });
    assert.equal(sfc.readPath("/apps/x/..").ok, false);
    const origin = "https://moqui.example.com";
    assert.deepEqual(sfc.linkTarget("https://moqui.example.com/apps/system/Visit/VisitDetail?visitId=V1", origin, false),
        { kind: "internal", url: "/qapps/system/Visit/VisitDetail?visitId=V1", path: "/apps/system/Visit/VisitDetail" });
    assert.deepEqual(sfc.linkTarget("https://evil.example.com/?d=secret", origin, false), { blocked: "another site" });
    assert.deepEqual(sfc.linkTarget("https://evil.example.com/?d=secret", origin, true), { kind: "external", url: "https://evil.example.com/?d=secret" });
    assert.deepEqual(sfc.linkTarget("javascript:alert(1)", origin, true), { blocked: "a javascript link" });
    assert.deepEqual(sfc.linkTarget("data:text/html,x", origin, true), { blocked: "a data link" });
    assert.deepEqual(sfc.linkTarget("https://moqui.example.com/assist/read?path=/apps/x", origin, true), { blocked: "not a screen of the apps" });
    assert.deepEqual(sfc.linkTarget("https://moqui.example.com/rest/s1/x", origin, true), { blocked: "not a screen of the apps" });
    const nonce = sfc.nonce();
    assert.match(nonce, /^[0-9a-f]{32}$/);
    assert.notEqual(sfc.nonce(), nonce);
    assert.equal(sfc.frameUrl("/assist/sandbox", "3-0", nonce, false), "/assist/sandbox?v=3-0#n=" + nonce);
    assert.equal(sfc.frameUrl("/assist/sandbox", "3-0", nonce, true), "/assist/sandbox?v=3-0&x=1#n=" + nonce);
});

test("the watchdog kills a frame that navigates, does not draw, goes silent or floods", () => {
    const dog = sfc.watchdog();
    assert.equal(dog.check(0), null, "nothing loaded yet");
    assert.equal(dog.loaded(0), null);
    assert.equal(dog.loaded(100), null, "a load before the frame took its port is a retry");
    dog.readied();
    assert.equal(dog.check(5000), null);
    assert.equal(dog.check(15001), "timeout", "not rendered in 15 s");
    dog.rendered(1000);
    dog.message(1000);
    assert.equal(dog.check(10999), null);
    assert.equal(dog.check(11001), "unresponsive", "no ping for 10 s");
    assert.equal(dog.loaded(12000), "navigation", "a load after the port was taken");
    const flood = sfc.watchdog({ floodMax: 5, floodWindowMs: 1000 });
    for (let i = 0; i < 5; i++) assert.equal(flood.message(i), null);
    assert.equal(flood.message(5), "flood");
    const calm = sfc.watchdog({ floodMax: 5, floodWindowMs: 1000 });
    for (let i = 0; i < 20; i++) assert.equal(calm.message(i * 300), null, "five a second is not a flood");
});

test("a submit counts only with the user's activation in the frame, once every two seconds; reads are limited", () => {
    assert.deepEqual(sfc.submitAllowed({ activation: false, focused: true, now: 10000, last: null }), { ok: false, reason: "activation" });
    assert.deepEqual(sfc.submitAllowed({ activation: true, focused: false, now: 10000, last: null }), { ok: false, reason: "focus" },
        "the activation came from the drawer, not from a click in the frame");
    assert.deepEqual(sfc.submitAllowed({ activation: true, focused: true, now: 10000, last: null }), { ok: true });
    assert.deepEqual(sfc.submitAllowed({ activation: true, focused: true, now: 11000, last: 10000 }), { ok: false, reason: "rate" });
    assert.deepEqual(sfc.submitAllowed({ activation: true, focused: true, now: 12001, last: 10000 }), { ok: true });
    const reads = sfc.reads({ maxInflight: 2, readsPerWindow: 3, readWindowMs: 1000 });
    assert.equal(reads.begin(0), null);
    assert.equal(reads.begin(1), null);
    assert.equal(reads.begin(2), "too many reads at once");
    reads.end();
    assert.equal(reads.begin(3), null);
    reads.end();
    assert.equal(reads.begin(4), "too many reads, slow down");
    assert.equal(reads.begin(1001), null, "a new window");
});

test("the frame runtime takes its port once, from its parent, with its nonce, and removes what CSP does not govern", () => {
    const source = fs.readFileSync(path.join(__dirname, "WebrootAssistSandbox.js"), "utf8");
    assert.match(source, /if \(port \|\| event\.source !== window\.parent\) return;/);
    assert.match(source, /data\.nonce !== nonce/);
    assert.match(source, /window\.removeEventListener\("message", onPort\)/);
    // a link is sent only for a real click, and never navigates the frame; a document that goes away says so
    assert.match(source, /document\.addEventListener\("click", onLink, true\)/);
    assert.match(source, /if \(!event\.isTrusted\) return;/);
    assert.match(source, /addEventListener\("pagehide", \(\) => send\(\{ t: "leaving" \}\)\)/);
    // WebRTC is not governed by connect-src, so it is deleted before the component runs
    assert.match(source, /delete window\[name\]/);
    assert.match(source, /"RTCPeerConnection", "webkitRTCPeerConnection", "RTCDataChannel"/);
    // the parent posts the port with the nonce and listens only on its end of the channel, never on window messages
    const parent = fs.readFileSync(path.join(__dirname, "WebrootAssistCanvas.js"), "utf8");
    assert.match(parent, /postMessage\(\{ t: "port", nonce: this\.nonce \}, "\*", \[channel\.port2\]\)/);
    assert.doesNotMatch(parent, /addEventListener\("message"/);
    assert.match(parent, /sandbox: "allow-scripts"/);
    assert.doesNotMatch(parent, /allow-same-origin/);
    // the Save of a submit's confirm is dead for a moment (anti-clickjacking) and reads are off while other sites are on
    assert.ok(sfc.LIMITS.confirmArmMs >= 300, "the confirm arms after a delay");
    assert.match(parent, /this\.confirmArmed = false;/);
    assert.match(parent, /disable: !this\.confirmArmed/);
    assert.match(parent, /case "resize": if \(!this\.confirm\) this\.height = m\.height;/);
    assert.match(parent, /if \(this\.external\) \{/);
    // an internal link needs the user's gesture too
    assert.match(parent, /if \(!this\.framedGesture\(\)\) \{ this\.notice/);
});
