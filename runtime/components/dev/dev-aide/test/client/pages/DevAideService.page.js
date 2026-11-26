/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// The service writer's pure parts in the page (moqui.devAide.serviceOps from DevAideService.js): the composer's
// sentence table against the XSD of XML Actions, the chip and insert operations as the shared vectors have them
// (crates/moqui-lib/testdata/xml-edit-vectors.json, "composer: ..."), the names known before each block, a parameter
// (nested ones by their path of names, and the Parameters tab's rows) and a SECA rule as written, and a Try's parameters.
"use strict";

const ops = moqui.devAide.serviceOps;
const el = (name, path, attrs, children, text) => ({ name, path, attrs: attrs || [], children: children || [], text });

test("each sentence's chips are attributes the XSD gives its element", async () => {
    const response = await fetch("/files/component/framework/xsd/mr-xml-actions.xsd");
    const xsd = new DOMParser().parseFromString(await response.text(), "application/xml");
    const NS = "http://www.w3.org/2001/XMLSchema";
    const elements = {};
    for (const node of xsd.getElementsByTagNameNS(NS, "element")) if (node.getAttribute("name")) elements[node.getAttribute("name")] = node;
    const groups = {};
    for (const node of xsd.getElementsByTagNameNS(NS, "attributeGroup")) if (node.getAttribute("name")) groups[node.getAttribute("name")] = node;
    const attributesOf = (node, out) => {
        for (const attr of node.getElementsByTagNameNS(NS, "attribute")) out.add(attr.getAttribute("name") || (attr.getAttribute("ref") || "").replace(/^.*:/, ""));
        for (const ref of node.getElementsByTagNameNS(NS, "attributeGroup")) {
            const group = groups[(ref.getAttribute("ref") || "").replace(/^.*:/, "")];
            if (group && group !== node) attributesOf(group, out);
        }
        return out;
    };
    const missing = [];
    for (const [name, parts] of Object.entries(ops.SENTENCES)) {
        const element = elements[name];
        if (!element) { missing.push(name); continue; }
        const attrs = attributesOf(element, new Set());
        for (const part of parts) {
            if (typeof part !== "object" || part.text) continue;
            for (const attr of [part.attr, part.or].filter((value) => value)) if (!attrs.has(attr)) missing.push(name + "@" + attr);
        }
    }
    assert.deepEqual(missing, []);
});

test("a chip edit, a palette insert and a field-map are the vectors' operations", async () => {
    const response = await fetch("/files/crates/moqui-lib/testdata/xml-edit-vectors.json");
    const vectors = await response.json();
    const byName = Object.fromEntries(vectors.map((vector) => [vector.name, vector.ops]));
    const set = el("set", [0, 1, 0, 0, 0], [["field", "item.state"], ["value", "closed"]]);
    const chip = ops.sentence(set, new Set()).chips.find((item) => item.attr === "value");
    assert.deepEqual(ops.chipOp(set, chip, "done"), byName["composer: a chip edit three levels down"]);
    const actions = el("actions", [0], [], [el("entity-find-one", [0, 0])]);
    assert.deepEqual(ops.insertOp(actions, actions.children[0], "<check-errors/>"), byName["composer: a palette insert after the selected block"]);
    const script = el("script", [0, 2], [], [], "ec.logger.info(\"done\")");
    const code = ops.sentence(script).chips[0];
    assert.equal(code.text, "ec.logger.info(\"done\")");
    assert.deepEqual(ops.chipOp(script, code, "if (a < b) ec.logger.info(\"less\")"), byName["composer: a script's text stays CDATA"]);
    assert.deepEqual(ops.fieldMapOp(el("service-call", [0, 3]), "thingId"), byName["composer: a service-call's in-parameter as a field-map"]);
});

test("the names known before a block mark an unknown one in an expression", () => {
    const actions = el("actions", [0], [], [
        el("entity-find-one", [0, 0], [["entity-name", "Thing"], ["value-field", "thing"]]),
        el("iterate", [0, 1], [["list", "thing.items"], ["entry", "item"]], [el("set", [0, 1, 0], [["field", "total"], ["from", "item.amount + extra"]])]),
        el("set", [0, 2], [["field", "x"], ["from", "total + item"]])
    ]);
    const known = ops.knownBefore(actions, ["thingId"]);
    assert.deepEqual([...known.get("0.0")], ["thingId"]);
    assert.ok(known.get("0.1.0").has("item") && known.get("0.1.0").has("thing"));
    assert.ok(!known.get("0.2").has("item"), "an iterate's entry is not known after it");
    const inner = ops.sentence(actions.children[1].children[0], known.get("0.1.0"));
    assert.deepEqual(inner.chips.filter((chip) => chip.warn).map((chip) => chip.title), ["Not set before this block: extra"]);
    assert.deepEqual(ops.unknownNames("a.b + 'c d' + ec.user + Math.max(x, 1)", new Set(["a"])), ["x"]);
    assert.equal(ops.sentence(el("service-call", [0], [["name", "a#B"]])).chips.map((chip) => chip.text).join(" "), "a#B with its fields → …");
});

test("a parameter, a SECA rule and a Try's parameters as written", () => {
    const service = el("service", [0], [["verb", "get"], ["noun", "Thing"]], [el("actions", [0, 0])]);
    assert.deepEqual(ops.addParameter(service, "in-parameters", { name: "thingId", required: true }), [{ op: "insert", at: [0, 0], position: "before",
        xml: "<in-parameters><parameter name=\"thingId\" required=\"true\"/></in-parameters>" }]);
    assert.equal(ops.secaXml("create#Thing", "post-service"), "<seca service=\"create#Thing\" when=\"post-service\"><actions></actions></seca>");
    assert.deepEqual(ops.tryParameters({ a: "5", b: "x", c: "", d: "true" },
        [{ name: "a", type: "Integer" }, { name: "b", type: "String" }, { name: "c", type: "String" }, { name: "d", type: "Boolean" }]), { a: 5, b: "x", d: true });
    assert.deepEqual(ops.servicesOf(el("services", [], [], [service])).map((entry) => entry.name), ["get#Thing"]);
});

// nested parameters (D14): order (a Map with a description and a validation) holding items (a List holding nothing
// yet), an empty Map, and a plain parameter with a validation; the same cases as the terminal twin's builders
// (test/GuidedOps.spec.js), so the browser and the terminal make the same edits
const nested = () => el("service", [0], [], [el("in-parameters", [0, 0], [], [
    el("parameter", [0, 0, 0], [["name", "order"], ["type", "Map"]], [
        el("description", [0, 0, 0, 0], [], [], "The order"),
        el("parameter", [0, 0, 0, 1], [["name", "items"], ["type", "List"]]),
        el("matches", [0, 0, 0, 2], [["regexp", "x"]])]),
    el("parameter", [0, 0, 1], [["name", "extra"], ["type", "Map"]], [el("description", [0, 0, 1, 0], [], [], "More")]),
    el("parameter", [0, 0, 2], [["name", "plain"]], [el("matches", [0, 0, 2, 0], [["regexp", "y"]])])])]);

test("a nested parameter added in its parent, edited and removed by its path of names", () => {
    const service = nested();
    assert.equal(ops.parameterNode(service, "in-parameters", "order.items").path.join("."), "0.0.0.1");
    assert.equal(ops.parameterNode(service, "in-parameters", ["order", "items"]).path.join("."), "0.0.0.1");
    assert.equal(ops.parameterNode(service, "in-parameters", "items"), null);
    assert.equal(ops.parameterNode(service, "in-parameters", ""), null);
    assert.deepEqual(ops.addParameter(service, "in-parameters", { name: "quantity", type: "BigDecimal" }, "order"),
        [{ op: "insert", at: [0, 0, 0, 1], position: "after", xml: "<parameter name=\"quantity\" type=\"BigDecimal\"/>" }]);
    assert.deepEqual(ops.addParameter(service, "in-parameters", { name: "sku" }, "order.items"),
        [{ op: "insert", at: [0, 0, 0, 1], position: "last", xml: "<parameter name=\"sku\"/>" }]);
    assert.deepEqual(ops.addParameter(service, "in-parameters", { name: "note" }, "extra"),
        [{ op: "insert", at: [0, 0, 1], position: "last", xml: "<parameter name=\"note\"/>" }]);
    assert.deepEqual(ops.addParameter(service, "in-parameters", { name: "inner" }, "plain"),
        [{ op: "insert", at: [0, 0, 2, 0], position: "before", xml: "<parameter name=\"inner\"/>" }]);
    assert.deepEqual(ops.addParameter(service, "in-parameters", { name: "x" }, "order.nothing"), []);
    assert.deepEqual(ops.removeParameter(service, "in-parameters", "order.items"), [{ op: "remove", at: [0, 0, 0, 1] }]);
    // as JSON: the three operations share their element's path, which deepEqual takes for a cycle
    const edited = ops.editParameter(service, "in-parameters", "order.items", { type: "List", required: true, description: "Its lines" });
    assert.equal(JSON.stringify(edited), JSON.stringify([
        { op: "set-attribute", at: [0, 0, 0, 1], name: "type", value: "List" },
        { op: "set-attribute", at: [0, 0, 0, 1], name: "required", value: "true" },
        { op: "insert", at: [0, 0, 0, 1], position: "last", xml: "<description>Its lines</description>" }]));
    // a description goes first in a parameter that holds others
    assert.deepEqual(ops.editParameter(service, "in-parameters", "plain", { description: "Plain" }).pop(),
        { op: "insert", at: [0, 0, 2], position: "first", xml: "<description>Plain</description>" });
    assert.deepEqual(ops.editParameter(service, "in-parameters", "order", { type: "Map", description: "" }).pop(), { op: "remove", at: [0, 0, 0, 0] });
});

test("an Add of a name there already, and an Edit of a holder of parameters to a type that holds none, are refused", () => {
    const service = nested();
    assert.ok(ops.parameterRefusal(service, "in-parameters", { mode: "add", parent: "order", name: "items" }).includes("already"));
    assert.ok(ops.parameterRefusal(service, "in-parameters", { mode: "add", parent: "", name: "plain" }).includes("already"));
    assert.equal(ops.parameterRefusal(service, "in-parameters", { mode: "add", parent: "order", name: "plain" }), null);
    assert.ok(ops.parameterRefusal(service, "in-parameters", { mode: "edit", path: "order", type: "String" }).includes("holds parameters"));
    assert.equal(ops.parameterRefusal(service, "in-parameters", { mode: "edit", path: "order", type: "List" }), null);
    // an empty Map may become anything
    assert.equal(ops.parameterRefusal(service, "in-parameters", { mode: "edit", path: "extra", type: "String" }), null);
});

test("the parameter rows: the buffer's, nested by path, then the definition's implied ones", () => {
    const defined = [
        { name: "order", type: "Map", required: "false", checks: ["MatchesRegexp"], implied: false,
            children: [{ name: "items", type: "List", children: [] }, { name: "fromAuto", children: [] }] },
        { name: "partyId", type: "String", required: "true", checks: [], implied: true, children: [{ name: "inner", children: [] }] }];
    const rows = ops.parameterRows(nested(), "in-parameters", defined);
    assert.deepEqual(rows.map((row) => row.path), ["order", "order.items", "order.fromAuto", "extra", "plain", "partyId", "partyId.inner"]);
    const byPath = Object.fromEntries(rows.map((row) => [row.path, row]));
    assert.equal(byPath.order.checks, 1);
    assert.equal(byPath.order.description, "The order");
    assert.ok(byPath.order.holder && byPath["order.items"].holder && byPath.extra.holder && !byPath.plain.holder);
    assert.deepEqual([byPath["order.items"].depth, byPath["order.items"].parentPath], [1, "order"]);
    // what the definition implies is not the buffer's to edit, nor are its children
    assert.ok(!byPath.partyId.editable && !byPath["partyId.inner"].editable && byPath.partyId.required);
    // what the definition adds inside one's own Map (its auto-parameters) shows in it, not to be edited
    assert.ok(byPath["order.fromAuto"].implied && !byPath["order.fromAuto"].editable && byPath["order.fromAuto"].depth === 1);
    // with no service element in the buffer, the definition's
    assert.deepEqual(ops.parameterRows(null, "in-parameters", defined).map((row) => row.path + (row.editable ? "" : " (auto)")),
        ["order", "order.items", "order.fromAuto", "partyId (auto)", "partyId.inner (auto)"]);
});

test("a gitECA rule asks for the pushes its patterns take, and a change of the push log reads in a line", () => {
    const rule = el("giteca", [0], [["event", "commit"], ["repository-pattern", "app.*"], ["ref-pattern", "refs/heads/.*"]]);
    assert.deepEqual(ops.pushQuery(rule), { repositoryPattern: "app.*", refPattern: "refs/heads/.*", refType: "", defaultBranchOnly: false, source: "",
        event: "commit" });
    assert.deepEqual(ops.pushQuery(el("giteca", [0], [["ref-type", "tag"], ["default-branch-only", "true"], ["source", "push,write"]])),
        { repositoryPattern: "", refPattern: "", refType: "tag", defaultBranchOnly: true, source: "push,write", event: "post-receive" });
    const update = { updateId: "U1", updateDate: "2026-11-25T10:20:30", username: "jane", ecaStatus: "done",
        refList: [{ shortName: "main", result: "ok" }, { shortName: "v1", result: "refused" }] };
    assert.equal(ops.updateLabel(update), "2026-11-25 10:20 · jane · main, v1 (refused) · done");
    assert.equal(ops.updateLabel({ source: "fetch", refList: [] }), "fetch · no refs");
});

// a Try's trace on the composer's blocks (9.2): the actions at line 12 of service/a/Things.xml, paths under them
const traced = () => el("actions", [0, 1], [], [
    el("set", [0, 1, 0], [["field", "total"], ["from", "1"]]),
    el("if", [0, 1, 1], [["condition", "total > 1"]], [
        el("then", [0, 1, 1, 0], [], [el("message", [0, 1, 1, 0, 0], [], [], "big")]),
        el("else", [0, 1, 1, 1], [], [el("set", [0, 1, 1, 1, 0], [["field", "size"], ["value", "small"]])])]),
    el("service-call", [0, 1, 2], [["name", "a#B"]], [el("field-map", [0, 1, 2, 0], [["field-name", "x"]])]),
    el("set", [0, 1, 3], [["field", "after"], ["value", "never"]])]);
const trace = { blocks: [
    { rootId: "other", path: "service/a/Things.xml", line: 12, elements: {} },
    { rootId: "app", path: "service/a/Things.xml", line: 12, elements: {
        "0": { count: 1, values: { total: "1" } }, "1": { count: 1, values: {} }, "1.1.0": { count: 1, values: { size: "small" } },
        "2": { count: 1, values: {}, failed: "no service a#B" } } }],
failed: { rootId: "app", path: "service/a/Things.xml", line: 12, element: "2", message: "no service a#B" } };

test("a Try's trace marks the composer's blocks run, not run or failed, with the counts and values on hover", () => {
    const block = ops.traceBlock(trace, "app", "service/a/Things.xml", 12);
    assert.equal(block, trace.blocks[1]);
    assert.equal(ops.traceBlock(trace, "app", "service/a/Things.xml", 30), null);
    const marks = ops.traceMarks(traced(), block);
    const states = Object.fromEntries([...marks].map(([path, mark]) => [path, mark.state]));
    assert.equal(JSON.stringify(states), JSON.stringify({ "0": "run", "1.0.0": "not-run", "1.0": "not-run", "1.1.0": "run", "1.1": "run", "1": "run",
        "2.0": "run", "2": "failed", "3": "not-run" }));
    assert.equal(ops.traceTitle(marks.get("0")), "Ran 1 time\ntotal = 1");
    assert.equal(ops.traceTitle(marks.get("2")), "Ran 1 time\nFailed: no service a#B");
    assert.equal(ops.traceTitle(marks.get("3")), "Not run in the last Try");
    const run = ops.markLabel(ops.sentence(traced().children[0]), marks.get("0"));
    assert.equal([run.headClass, run.badge, run.badgeClass].join(" "), "m-dev-trace-run ×1 m-dev-trace-run");
    const failed = ops.markLabel(ops.sentence(traced().children[2]), marks.get("2"));
    assert.equal([failed.headClass, failed.badge].join(" "), "m-dev-trace-failed failed");
    const notRun = ops.markLabel(ops.sentence(traced().children[3]), marks.get("3"));
    assert.equal([notRun.headClass, notRun.badge || ""].join(" "), "m-dev-trace-not-run ");
    // no trace, no marks
    assert.equal(ops.traceMarks(traced(), null).size, 0);
});

test("a Try runs over the unsaved service and script buffers, and a rule's spec names its trigger", () => {
    const files = ops.tryFiles([{ rootId: "app", path: "service/a/Things.xml", text: "s" }, { rootId: "app", path: "service/a/Things.secas.xml", text: "r" },
        { rootId: "app", path: "script/run.js", text: "j" }, { rootId: "app", path: "screen/A.xml", text: "x" }, { rootId: "app", path: "entity/E.xml", text: "e" }]);
    assert.deepEqual(files.map((file) => file.path), ["service/a/Things.xml", "script/run.js"]);
    assert.deepEqual(ops.ruleSpecQuery(el("seca", [0], [["service", "a.B.make#C"], ["when", "post-service"]]), ""),
        { kind: "seca", target: "a.B.make#C", operation: "", condition: "" });
    assert.deepEqual(ops.ruleSpecQuery(el("eeca", [1], [["entity", "up.Widget"], ["on-update", "true"], ["on-delete", "true"]]), "x > 1"),
        { kind: "eeca", target: "up.Widget", operation: "update", condition: "x > 1" });
});
