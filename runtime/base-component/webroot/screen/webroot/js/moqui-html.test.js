/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// Tests of the pure functions of moqui-html.js. Run: node --test runtime/base-component/webroot/screen/webroot/js/moqui-html.test.js
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const u = require("./moqui-html.js");

test("a datetime-local value becomes a timestamp with a space", () => {
    assert.equal(u.normalizeDateTime("2024-03-05T14:07"), "2024-03-05 14:07");
    assert.equal(u.normalizeDateTime("2024-03-05T14:07:30"), "2024-03-05 14:07:30");
    assert.equal(u.normalizeDateTime("2024-03-05"), "2024-03-05");
    assert.equal(u.normalizeDateTime("12:30"), "12:30");
    assert.equal(u.normalizeDateTime("Tuesday"), "Tuesday");
    assert.equal(u.normalizeDateTime(""), "");
    assert.equal(u.normalizeDateTime(undefined), undefined);
});

test("parseJson falls back on empty or broken text", () => {
    assert.deepEqual(u.parseJson('{"a":1}', null), { a: 1 });
    assert.equal(u.parseJson("", 5), 5);
    assert.equal(u.parseJson("{nope", "x"), "x");
    assert.deepEqual(u.parseJson(undefined, []), []);
});

test("queryString encodes names and values and repeats lists", () => {
    assert.equal(u.queryString({ a: "x y", b: ["1", "2"], c: null, d: undefined, "e&f": "g=h" }), "a=x%20y&b=1&b=2&e%26f=g%3Dh");
    assert.equal(u.queryString(null), "");
    assert.equal(u.queryString({ n: 0, z: "" }), "n=0&z=");
});

test("optionList reads a list or an options object with the named fields", () => {
    assert.deepEqual(u.optionList([{ value: "a", label: "A" }, { value: "", label: "none" }]), [{ value: "a", text: "A" }, { value: " ", text: "none" }]);
    assert.deepEqual(u.optionList({ options: [{ id: 1, name: "One" }], count: 1 }, "id", "name"), [{ value: "1", text: "One" }]);
    assert.deepEqual(u.optionList({ nothing: true }), []);
    assert.deepEqual(u.optionList(null), []);
    assert.deepEqual(u.optionList([{ value: "k", text: "T" }]), [{ value: "k", text: "T" }]);
});

test("dependsParams needs every value unless the dependency is optional", () => {
    const values = { f1: "x", f2: "", f3: "z" };
    const read = (id) => values[id];
    assert.deepEqual(u.dependsParams([["f1", "one"], ["f3", "three"]], read, false), { one: "x", three: "z" });
    assert.equal(u.dependsParams([["f1", "one"], ["f2", "two"]], read, false), null);
    assert.deepEqual(u.dependsParams([["f1", "one"], ["f2", "two"]], read, true), { one: "x" });
    assert.deepEqual(u.dependsParams([], read, false), {});
    assert.deepEqual(u.dependsParams(undefined, read, false), {});
});

test("orderByValue keeps one direction for each column", () => {
    assert.equal(u.orderByValue(["name", "-qty"]), "name,-qty");
    assert.equal(u.orderByValue(["name", "-name", "qty"]), "name,qty");
    assert.equal(u.orderByValue(["^name", "-^name"]), "^name");
    assert.equal(u.orderByValue([]), "");
    assert.equal(u.orderByValue("one"), "one");
});

test("treeNodes reads the jstree shape the tree transitions return", () => {
    const nodes = u.treeNodes([
        { id: "a", text: "A", a_attr: { href: "/x?id=a" }, li_attr: { treeNodeName: "Top" }, state: { opened: true }, children: [{ id: "a1", text: "A1" }] },
        { id: "b", text: "B", children: true },
        { id: "c" }
    ]);
    assert.equal(nodes[0].href, "/x?id=a");
    assert.equal(nodes[0].name, "Top");
    assert.equal(nodes[0].opened, true);
    assert.equal(nodes[0].children[0].text, "A1");
    assert.equal(nodes[0].children[0].children, false);
    assert.equal(nodes[1].children, true);
    assert.equal(nodes[2].text, "");
    assert.deepEqual(u.treeNodes(null), []);
});

test("the options of an editable select from its editable-load answer", () => {
    assert.deepEqual(u.editableOptions([{ value: "A", label: "First" }, "B"]), { options: [{ value: "A", label: "First" }, { value: "B", label: "B" }] });
    assert.deepEqual(u.editableOptions({ E: "Letter E", selected: "E" }), { options: [{ value: "E", label: "Letter E" }], selected: "E" });
    assert.deepEqual(u.editableOptions(null), { options: [] });
});

test('saved-find changes navigate to the selected URL and ignore the placeholder and other selects', () => {
    const fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
    const listeners = {}, visited = [];
    const document = {
        documentElement: { classList: { add() {} } },
        querySelector: () => null, querySelectorAll: () => [],
        addEventListener: (name, handler) => { listeners[name] = handler; }
    };
    vm.runInNewContext(fs.readFileSync(path.join(__dirname, 'moqui-html.js'), 'utf8'), {
        window: { document, location: { assign: url => visited.push(url) } }
    });
    const change = url => listeners.change({ target: { closest: () => ({ selectedOptions: [{ dataset: { url } }] }) } });
    change('/apps/FindOrder?formListFindId=sales');
    change('/apps/FindOrder?formListFindId=_clear');
    change(undefined);
    listeners.change({ target: { closest: () => null } });
    assert.deepEqual(visited, ['/apps/FindOrder?formListFindId=sales', '/apps/FindOrder?formListFindId=_clear']);
});
