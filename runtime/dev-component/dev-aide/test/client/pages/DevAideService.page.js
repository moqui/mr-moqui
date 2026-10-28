/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// The service writer's pure parts in the page (moqui.devAide.serviceOps from DevAideService.js): the composer's
// sentence table against the XSD of XML Actions, the chip and insert operations as the shared vectors have them
// (crates/moqui-lib/testdata/xml-edit-vectors.json, "composer: ..."), the names known before each block, a parameter
// and a SECA rule as written, and a Try's parameters.
"use strict";

const ops = moqui.devAide.serviceOps;
const el = (name, path, attrs, children, text) => ({ name, path, attrs: attrs || [], children: children || [], text });

test("each sentence's chips are attributes the XSD gives its element", async () => {
    const response = await fetch("/files/runtime/base-component/framework/xsd/xml-actions-3.xsd");
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
