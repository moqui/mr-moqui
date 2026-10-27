/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// The pure functions of moqui-html.js. A script loaded first sets module.exports so the file's Node export
// is the same object here. The file also starts its client because the page has a document.
"use strict";

const u = module.exports;

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

test("keptValue keeps a value still offered and otherwise takes the first, as a native select", () => {
    const options = [{ value: "a", text: "A" }, { value: "b", text: "B" }];
    assert.equal(u.keptValue("b", options, false, false), "b");
    assert.equal(u.keptValue("gone", options, false, false), "a");
    assert.equal(u.keptValue("gone", options, false, true), "");
    assert.equal(u.keptValue("", [], false, false), "");
    assert.deepEqual(u.keptValue(["b", "gone"], options, true, false), ["b"]);
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

test("datePattern reads the numeric fields of a pattern and refuses one a user types letters into", () => {
    assert.deepEqual(u.datePattern("yyyy-MM-dd").map((p) => p.field || p.text), ["year", "-", "month", "-", "day"]);
    assert.deepEqual(u.datePattern("yyyy-MM-dd'T'HH:mm").map((p) => p.field || p.text), ["year", "-", "month", "-", "day", "T", "hour", ":", "minute"]);
    assert.equal(u.datePattern("dd MMM yyyy"), null);
    assert.equal(u.datePattern("h:mm a"), null);
    assert.equal(u.datePattern("EEE dd"), null);
    assert.equal(u.datePattern(""), null);
});

test("dateKey: t is today or now, letters step a field, Shift steps back, a month keeps its last day", () => {
    const now = new Date(2026, 9, 25, 14, 37, 12);
    assert.equal(u.dateKey("yyyy-MM-dd", "", "t", false, 5, now), "2026-10-25");
    assert.equal(u.dateKey("yyyy-MM-dd HH:mm", "", "T", true, 5, now), "2026-10-25 14:37");
    assert.equal(u.dateKey("HH:mm:ss", "", "t", false, 5, now), "14:37:12");
    assert.equal(u.dateKey("yyyy-MM-dd", "2026-10-25", "d", false), "2026-10-26");
    assert.equal(u.dateKey("yyyy-MM-dd", "2026-10-25", "D", true), "2026-10-24");
    assert.equal(u.dateKey("yyyy-MM-dd", "2026-10-25", "w", false), "2026-11-01");
    assert.equal(u.dateKey("yyyy-MM-dd", "2026-01-31", "m", false), "2026-02-28");
    assert.equal(u.dateKey("yyyy-MM-dd", "2024-02-29", "y", false), "2025-02-28");
    assert.equal(u.dateKey("yyyy-MM-dd", "2026-01-01", "M", true), "2025-12-01");
    assert.equal(u.dateKey("yyyy-MM-dd", "2026-10-25", "ArrowUp", false), "2026-10-26");
    assert.equal(u.dateKey("yyyy-MM-dd", "2026-10-25", "ArrowDown", false), "2026-10-24");
    assert.equal(u.dateKey("dd/MM/yy", "31/12/26", "d", false), "01/01/27");
    assert.equal(u.dateKey("yyyy-MM-dd'T'HH:mm", "2026-10-25T23:55", "i", false, 5), "2026-10-26T00:00");
    assert.equal(u.dateKey("yyyy-MM-dd HH:mm", "2026-10-25 10:00", "h", true, 5), "2026-10-25 09:00");
    assert.equal(u.dateKey("HH:mm", "10:00", "i", true, 15), "09:45");
});

test("dateKey changes nothing without the field, without a value, or for another key", () => {
    assert.equal(u.dateKey("yyyy-MM-dd", "2026-10-25", "h", false), null);
    assert.equal(u.dateKey("HH:mm", "10:00", "d", false), null);
    assert.equal(u.dateKey("HH:mm", "10:00", "s", false), null);
    assert.equal(u.dateKey("yyyy-MM-dd", "", "d", false), null);
    assert.equal(u.dateKey("yyyy-MM-dd", "not a date", "d", false), null);
    assert.equal(u.dateKey("yyyy-MM-dd", "2026-10-25", "x", false), null);
    assert.equal(u.dateKey("dd MMM yyyy", "25 Oct 2026", "d", false), null);
});

test("quickKeyWaits holds ENTER and TAB while options load and until the typing stops", () => {
    assert.equal(u.quickKeyWaits(0, 1000, 1200), true);
    assert.equal(u.quickKeyWaits(0, 1000, 1600), false);
    assert.equal(u.quickKeyWaits(1, 1000, 9000), true);
    assert.equal(u.quickKeyWaits(0, 0, 50), false);
});

test('saved-find changes navigate to the selected URL and ignore the placeholder and other selects', async () => {
    // The real page will not replace location.assign, so this case runs the library the way the Node vm did:
    // a direct eval sees this function's window, and the stand-in records assign. The client starts on that document.
    const source = await (await fetch("/js/moqui-html.js")).text();
    const listeners = {}, visited = [];
    const document = {
        documentElement: { classList: { add() {} } },
        querySelector: () => null, querySelectorAll: () => [],
        addEventListener: (name, handler) => { listeners[name] = handler; }
    };
    const window = { document, location: { assign: url => visited.push(url) } };
    eval(source);
    const change = url => listeners.change({ target: { closest: () => ({ selectedOptions: [{ dataset: { url } }] }) } });
    change('/apps/FindOrder?formListFindId=sales');
    change('/apps/FindOrder?formListFindId=_clear');
    change(undefined);
    listeners.change({ target: { closest: () => null } });
    assert.deepEqual(visited, ['/apps/FindOrder?formListFindId=sales', '/apps/FindOrder?formListFindId=_clear']);
});

test("empty-field exclusion preserves whitespace, zero, false, and multiple values", () => {
    const data = new FormData();
    for (const [name, value] of [["empty", ""], ["blank", " "], ["zero", "0"], ["false", "false"], ["tags", "a"], ["tags", "b"]]) data.append(name, value);
    u.excludeEmptyFields(data);
    assert.equal(data.has("empty"), false);
    assert.equal(data.get("blank"), " ");
    assert.equal(data.get("zero"), "0");
    assert.equal(data.get("false"), "false");
    assert.deepEqual(data.getAll("tags"), ["a", "b"]);
});
