/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// The screen view's pure parts in the page (moqui.devAide.screenOps from DevAideScreen.js) and the block tree's
// (moqui.devAide.blocks from DevAideBlocks.js): the anchors and the nearest one to an element with no name, the
// operations of the intents (a form merge with after=, a widgets-extend, a field in place with its layout and column),
// and the moves of a drag and of Alt+Up and Alt+Down.
"use strict";

const ops = moqui.devAide.screenOps;
const blocks = moqui.devAide.blocks;
const el = (name, path, attrs, children, line) => ({ name, path, attrs: attrs || [], children: children || [], line: line || 0 });

// a screen: widgets with a named box holding a list, an unnamed container, a named one, a form with a layout
const screen = el("screen", [], [], [
    el("transition", [0], [["name", "save"]]),
    el("widgets", [1], [], [
        el("container-box", [1, 0], [["name", "WidgetBox"]], [el("form-list", [1, 0, 0], [["name", "WidgetList"]], [
            el("field", [1, 0, 0, 0], [["name", "widgetId"]]), el("field", [1, 0, 0, 1], [["name", "widgetName"]], [], 9)])], 4),
        el("container", [1, 1], [], [el("label", [1, 1, 0], [["text", "x"]])], 12),
        el("container", [1, 2], [["name", "Notes"]], [], 14),
        el("form-single", [1, 3], [["name", "WidgetForm"]], [
            el("field", [1, 3, 0], [["name", "a"]]), el("field", [1, 3, 1], [["name", "b"]]),
            el("field-layout", [1, 3, 2], [], [el("field-row", [1, 3, 2, 0], [], [el("field-ref", [1, 3, 2, 0, 0], [["name", "a"]]),
                el("field-ref", [1, 3, 2, 0, 1], [["name", "b"]])])])])
    ])
]);

test("the extension points name the anchors, and the nearest anchor of one with no name", () => {
    const points = ops.extensionPoints(screen);
    const named = points.filter((point) => point.named).map((point) => point.kind + " " + point.name);
    assert.deepEqual(named, ["container-box WidgetBox", "form-list WidgetList", "container Notes", "form-single WidgetForm", "transition save"]);
    const unnamed = points.find((point) => !point.named);
    assert.deepEqual([unnamed.kind, unnamed.nearest.name, unnamed.nearest.where], ["container", "WidgetBox", "after"]);
    // the label in the unnamed container: its nearest anchor is past its parent
    assert.equal(ops.nearestAnchor(screen, [1, 1, 0]).name, "WidgetBox");
    assert.equal(ops.nodeAtLine(screen, 14).path.join("."), "1.2");
    assert.equal(ops.formOf(screen, [1, 0, 0, 1]).path.join("."), "1.0.0");
    assert.equal(pure().attr(ops.fieldOf(screen, [1, 0, 0, 1]), "name"), "widgetName");
});

function pure() { return moqui.devAide.pure; }

test("an upstream list gets a column through the site's form merge, placed by after", () => {
    const empty = el("screen-extend", [], [], []);
    const field = { name: "dueNote", title: "Due note", widget: "display", after: "widgetName" };
    assert.deepEqual(ops.extendFormField(empty, "form-list", "WidgetList", field), [{ op: "insert", at: [], position: "last",
        xml: "<form-list name=\"WidgetList\"><field name=\"dueNote\" after=\"widgetName\"><default-field title=\"Due note\"><display/></default-field></field></form-list>" }]);
    const has = el("screen-extend", [], [], [el("form-list", [0], [["name", "WidgetList"]])]);
    assert.equal(ops.extendFormField(has, "form-list", "WidgetList", field)[0].at.join("."), "0");
    assert.deepEqual(ops.extendWidgets(empty, "Notes", "after", "<label text=\"x\"/>"), [{ op: "insert", at: [], position: "last",
        xml: "<widgets-extend name=\"Notes\" where=\"after\"><label text=\"x\"/></widgets-extend>" }]);
});

test("in place, a field goes after the selected one with its field-ref in the layout", () => {
    const form = ops.formOf(screen, [1, 3]);
    const list = ops.insertFormField(form, { name: "c", title: "C", widget: "text-line" }, "a");
    assert.deepEqual(list.map((op) => [op.at.join("."), op.position]), [["1.3.0", "after"], ["1.3.2.0.0", "after"]]);
    assert.equal(list[1].xml, "<field-ref name=\"c\"/>");
    const columns = el("form-list", [0], [["name", "L"]], [el("field", [0, 0], [["name", "a"]]),
        el("form-list-column", [0, 1], [], [el("field-ref", [0, 1, 0], [["name", "a"]])])]);
    const added = ops.insertFormField(columns, { name: "b" }, "a");
    assert.deepEqual(added[1], { op: "insert", at: [0, 1], position: "after", xml: "<form-list-column><field-ref name=\"b\"/></form-list-column>" });
});

test("a drag moves a block before the one that ends up after it, or last", () => {
    const shown = [[1, 0], [1, 1], [1, 2]];
    assert.deepEqual(blocks.dropOp([1, 2], [1], 0, shown), { op: "move", at: [1, 2], to: [1, 0], position: "before" });
    assert.equal(blocks.dropOp([1, 1], [1], 1, shown), null, "dropped where it was");
    assert.deepEqual(blocks.dropOp([1, 0], [1], 2, shown), { op: "move", at: [1, 0], to: [1, 2], position: "after" });
    // into another container, three levels down, and into an empty one
    assert.deepEqual(blocks.dropOp([1, 2], [1, 0, 0], 1, [[1, 0, 0, 0], [1, 0, 0, 1]]), { op: "move", at: [1, 2], to: [1, 0, 0, 1], position: "before" });
    assert.deepEqual(blocks.dropOp([1, 0], [1, 1], 0, []), { op: "move", at: [1, 0], to: [1, 1], position: "last" });
    assert.equal(blocks.dropOp([1, 0], [1, 0, 0], 0, []), null, "not into itself");
    assert.deepEqual(blocks.stepOp([1, 1], shown, -1), { op: "move", at: [1, 1], to: [1, 0], position: "before" });
    assert.equal(blocks.stepOp([1, 2], shown, 1), null);
    assert.equal(blocks.flatten(screen.children, (node) => node.children).length, 16);
});

test("a section or transition is copied into the site's screen-extend to replace it; actions run before or after", () => {
    const text = "<screen>\n    <transition name=\"save\"><actions/></transition>\n</screen>\n";
    const node = { name: "transition", path: [0], attrs: [["name", "save"]], from16: text.indexOf("<transition"), to16: text.indexOf("</screen>") - 1 };
    const ext = el("screen-extend", [], [], []);
    assert.deepEqual(ops.copyInto(ext, text, node), [{ op: "insert", at: [], position: "last", xml: "<transition name=\"save\"><actions/></transition>" }]);
    assert.equal(ops.actionsExtend(ext, "before")[0].xml, "<actions-extend type=\"actions\" when=\"before\"><log level=\"info\" message=\"before the screen's actions\"/></actions-extend>");
});

test("a filter, a section, a button and a row action as written", () => {
    const actions = el("actions", [0], [], [el("entity-find", [0, 0], [["entity-name", "W"], ["list", "wList"]], [el("search-form-inputs", [0, 0, 0])])]);
    const outline = el("screen", [], [], [actions, el("widgets", [1], [], [el("form-list", [1, 0], [["name", "L"], ["list", "wList"]], [el("field", [1, 0, 0], [["name", "a"]])])])]);
    assert.ok(ops.hasSearchInputs(ops.listFindOf(outline, "wList")));
    assert.equal(ops.listFindOf(outline, "other"), null);
    const form = outline.children[1].children[0];
    assert.deepEqual(ops.filterOps(true, outline, form, "a"), [{ op: "insert", at: [1, 0, 0], position: "first",
        xml: "<header-field show-order-by=\"true\"><text-find/></header-field>" }]);
    assert.deepEqual(ops.filterOps(false, el("screen-extend", [], [], []), form, "a"), [{ op: "insert", at: [], position: "last",
        xml: "<form-list name=\"L\"><field name=\"a\"><header-field show-order-by=\"true\"><text-find/></header-field></field></form-list>" }]);
    assert.equal(ops.sectionXml({ kind: "box", name: "Notes", title: "Notes" }), "<container-box name=\"Notes\"><box-header title=\"Notes\"/><box-body></box-body></container-box>");
    const related = ops.sectionXml({ kind: "related", name: "Lines", title: "Lines", entity: "mantle.order.OrderItem", keyField: "orderId" });
    assert.ok(related.startsWith("<section name=\"Lines\"><actions><entity-find entity-name=\"mantle.order.OrderItem\" list=\"orderItemList\"><econdition field-name=\"orderId\" from=\"orderId\"/>"));
    assert.ok(related.includes("<auto-fields-entity entity-name=\"mantle.order.OrderItem\" field-type=\"display\"/>"));
    // in place the link's path moves by one: the transition goes in before the actions, ahead of the widgets it is under
    const node = el("label", [1, 0], [["text", "x"]]);
    const button = ops.buttonOps(false, outline, { transition: "approve", service: "a.B.approve#C", text: "Approve" },
        () => [{ op: "insert", at: node.path, position: "after", xml: "<link/>" }], node);
    assert.deepEqual(button.map((op) => [op.at.join("."), op.position]), [["0", "before"], ["2.0", "after"]]);
    assert.equal(button[0].xml, "<transition name=\"approve\"><service-call name=\"a.B.approve#C\" in-map=\"true\"/><default-response url=\".\"/></transition>");
    assert.equal(ops.rowActionXml({ name: "approveRow", transition: "approve", text: "Approve", keyField: "orderId" }),
        "<field name=\"approveRow\"><default-field><link url=\"approve\" text=\"Approve\" link-type=\"anchor\" parameter-map=\"{orderId: orderId}\"/></default-field></field>");
    assert.equal(ops.textAttribute(el("label", [0])), "text");
    assert.equal(ops.textAttribute(el("form-list", [0])), null);
});
