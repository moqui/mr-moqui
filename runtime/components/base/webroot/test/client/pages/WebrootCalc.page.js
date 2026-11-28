/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
"use strict";
const calc = moqui.calc;
test("columns are named as a spreadsheet names them", () => {
    assert.deepEqual([1, 26, 27, 52, 703, 16384].map(calc.columnName), ["A", "Z", "AA", "AZ", "AAA", "XFD"]);
});
test("a cell's css is its style, its fill and its type's alignment", () => {
    assert.equal(calc.cellCss({ style: { font: {}, border: {} } }, "", 2), null);
    assert.deepEqual(calc.cellCss({ style: { font: {}, border: {} } }, "", 1), { "text-align": "right" });
    assert.deepEqual(calc.cellCss({ style: { font: {}, border: {} } }, "", 16), { "text-align": "center" });
    assert.deepEqual(calc.cellCss({ style: { alignment: { horizontal: "left", wrap_text: true }, font: { b: true, i: true, u: true, strike: true, color: "#FF0000" },
        border: { bottom: { style: "thin", color: "#000000" }, top: { style: "double" } } } }, "#EEEEEE", 1),
        { "font-weight": "600", "font-style": "italic", "text-decoration": "underline line-through", color: "#FF0000", "background-color": "#EEEEEE", "text-align": "left",
            "white-space": "normal", "border-top": "3px double currentColor", "border-bottom": "1px solid #000000" });
    // a colour that is not #RRGGBB (a theme colour the engine did not resolve, or anything else) is not written into a style
    assert.equal(calc.cellCss({ style: { font: { color: [1, 0.5] }, border: {} } }, "red;x:y", 2), null);
});
test("a cell's css replaces what the last draw set and nothing else", () => {
    const element = document.createElement("div");
    element.style.width = "90px";
    calc.applyCss(element, { "font-weight": "600", color: "#FF0000" });
    assert.equal(element.style.fontWeight, "600");
    calc.applyCss(element, { color: "#00FF00" });
    assert.equal(element.style.fontWeight, "");
    assert.equal(element.style.color, "rgb(0, 255, 0)");
    calc.applyCss(element, null);
    assert.equal(element.style.color, "");
    assert.equal(element.style.width, "90px");
});
test("a block is rows of cells by column, with the fills of its runs", () => {
    const rows = calc.blockRows({ row: 5, column: 1, width: 3, height: 2,
        styles: [{ style: { font: { b: true }, border: {} } }, { style: { font: {}, border: {} } }], style_indices: [0, 1, 1, 1, 1, 1],
        texts: [{ row: 5, column: 1, text: "Name", cell_type: 2 }, { row: 6, column: 3, text: "12.5", cell_type: 1 }],
        fills: [{ row: 5, column: 1, width: 2, color: "#EEEEEE" }, { row: 5, column: 3, width: 1, color: "" }, { row: 6, column: 1, width: 3, color: "" }] });
    assert.deepEqual(Array.from(rows.keys()), [5, 6]);
    assert.deepEqual(rows.get(5)[0], { text: "Name", css: { "font-weight": "600", "background-color": "#EEEEEE" } });
    assert.deepEqual(rows.get(5)[1], { text: "", css: { "background-color": "#EEEEEE" } });
    assert.equal(rows.get(5)[2], undefined);
    assert.deepEqual(rows.get(6)[2], { text: "12.5", css: { "text-align": "right" } });
    assert.equal(rows.get(6)[0], undefined);
});
test("the rows asked for are the ones in view and a margin that are not loaded", () => {
    const have = new Set();
    assert.deepEqual(calc.neededRange(have, 1, 30, 100000, 60), { row: 1, height: 90 });
    for (let r = 1; r <= 90; r++) have.add(r);
    assert.equal(calc.neededRange(have, 1, 30, 100000, 60), null);
    assert.deepEqual(calc.neededRange(have, 20, 50, 100000, 60), { row: 91, height: 20 });
    // a far jump loads around the rows in view, nothing between
    assert.deepEqual(calc.neededRange(have, 90000, 90030, 100000, 60), { row: 89940, height: 151 });
    // the sheet's end, and a sheet shorter than the view
    assert.deepEqual(calc.neededRange(have, 99990, 100000, 100000, 60), { row: 99930, height: 71 });
    assert.deepEqual(calc.neededRange(new Set(), 1, 30, 12, 60), { row: 1, height: 12 });
    // more missing rows than one request takes: the request holds the rows in view
    const wide = calc.neededRange(new Set(), 1000, 1900, 100000, 60);
    assert.equal(wide.height, calc.MAX_REQUEST_ROWS);
    assert.ok(wide.row <= 1000 && wide.row + wide.height > 1000);
});
test("a chart of the document is a Chart.js config", () => {
    const bar = calc.chartConfig({ type: "column", labels: ["a", "b"], series: [{ name: "Amount", data: [1, 2] }] });
    assert.equal(bar.type, "bar");
    assert.deepEqual(bar.data.labels, ["a", "b"]);
    assert.deepEqual(bar.data.datasets[0].data, [1, 2]);
    assert.equal(bar.data.datasets[0].label, "Amount");
    assert.equal(bar.options.plugins.legend.display, false);
    assert.equal(calc.chartConfig({ type: "bar", series: [] }).options.indexAxis, "y");
    const pie = calc.chartConfig({ type: "pie", labels: ["a", "b"], series: [{ data: [1, 2] }] });
    assert.equal(pie.type, "pie");
    assert.equal(pie.data.datasets[0].backgroundColor.length, 2);
    assert.equal(calc.chartConfig({ type: "area", series: [{ data: [1] }, { data: [2] }] }).data.datasets[1].fill, true);
});
test("a formula being typed completes function and table names, not cell references", () => {
    assert.equal(calc.completions("SU", 2, []), null);
    assert.deepEqual(calc.completions("=SU", 3, []), { from: 1, prefix: "SU", names: ["SUBSTITUTE(", "SUBTOTAL(", "SUM(", "SUMIF(", "SUMIFS(", "SUMPRODUCT("] });
    assert.deepEqual(calc.completions("=A1+vl", 6, []).names, ["VLOOKUP("]);
    assert.deepEqual(calc.completions("=SUM(Ord", 8, ["Orders", "Totals"]).names, ["Orders["]);
    assert.equal(calc.completions("=SUM(B2", 7, []), null);
    assert.equal(calc.completions("=SUM(", 5, []), null);
    assert.equal(calc.completions("=ZZZ", 4, []), null);
    assert.equal(calc.completions("=\"su", 4, []), null);
});
test("a range is named as a formula names it, and pasted text is rows of cells", () => {
    assert.equal(calc.rangeName({ row: 2, column: 2, lastRow: 2, lastColumn: 2 }), "B2");
    assert.equal(calc.rangeName({ row: 2, column: 2, lastRow: 7, lastColumn: 4 }), "B2:D7");
    assert.equal(calc.rangeName({ row: 1, column: 27, lastRow: 3, lastColumn: 27 }, "Summary"), "Summary!AA1:AA3");
    assert.equal(calc.rangeName({ row: 1, column: 1, lastRow: 1, lastColumn: 1 }, "Joe's sheet"), "'Joe''s sheet'!A1");
    assert.deepEqual(calc.pasteRows("a\tb\r\n1\t2\n"), [["a", "b"], ["1", "2"]]);
    assert.deepEqual(calc.pasteRows("one"), [["one"]]);
    assert.equal(calc.tableName("Open orders 2026"), "Open_orders_2026");
    assert.equal(calc.tableName("2026"), "T2026");
});
test("the socket's frames are the others in the room and when to read it again", () => {
    let taken = calc.receive([], 4, { type: "joined", peers: [{ client: "c1", userId: "ann", name: "Ann" }] });
    assert.deepEqual(taken.peers.map((peer) => peer.name), ["Ann"]);
    taken = calc.receive(taken.peers, 4, { type: "select", client: "c2", userId: "otto", name: "Otto", sheet: 1, area: { row: 2, column: 3, width: 1, height: 1 } });
    assert.deepEqual(taken.peers.map((peer) => peer.name), ["Ann", "Otto"]);
    taken = calc.receive(taken.peers, 4, { type: "select", client: "c1", userId: "ann", name: "Ann", sheet: 1, area: { row: 5, column: 1, width: 2, height: 2 } });
    assert.equal(taken.peers[0].area.row, 5);
    const peers = calc.receive(taken.peers, 4, { type: "peer", client: "c2", joined: false }).peers;
    assert.deepEqual(peers.map((peer) => peer.client), ["c1"]);
    // another's edit past the view's own is read again; the view's own, or an older one, is not; a dropped room opens again
    assert.equal(calc.receive(peers, 4, { type: "changed", seq: 5, kind: "edit" }).reread, true);
    assert.equal(calc.receive(peers, 5, { type: "changed", seq: 5, kind: "edit" }).reread, false);
    assert.equal(calc.receive(peers, 5, { type: "changed", seq: 5, kind: "refresh" }).reread, true);
    const dropped = calc.receive(peers, 5, { type: "changed", seq: 6, kind: "revert", reopen: true });
    assert.deepEqual([dropped.reopen, dropped.reread], [true, false]);
});
test("an edit that changed values drops only the rows it changed from the cache", () => {
    const cache = new Map([[0, new Map([[1, ["a"]], [2, ["b"]], [3, ["c"]], [9, ["z"]]])], [1, new Map([[2, ["x"]]])]]);
    assert.equal(calc.forgetRows(cache, [{ sheet: 0, row: 2, column: 1, width: 3, height: 2 }, { sheet: 4, row: 1, column: 1, width: 1, height: 1 }]), true);
    assert.deepEqual([...cache.get(0).keys()], [1, 9]);
    assert.deepEqual([...cache.get(1).keys()], [2]);
    // anything may have changed, or too many rows: the caller empties the cache
    assert.equal(calc.forgetRows(cache, null), false);
    assert.equal(calc.forgetRows(cache, undefined), false);
    assert.equal(calc.forgetRows(cache, [{ sheet: 0, row: 1, column: 1, width: 1, height: calc.MAX_FORGET_ROWS + 1 }]), false);
    assert.deepEqual([...cache.get(0).keys()], [1, 9]);
});
test("the others' places are the cells of the sheet shown, in each one's colour, a large one at its first cell", () => {
    const cells = calc.peerCells([
        { client: "c1", sheet: 1, area: { row: 2, column: 2, width: 2, height: 1 } },
        { client: "c2", sheet: 0, area: { row: 1, column: 1, width: 1, height: 1 } },
        { client: "c3", sheet: 1, area: { row: 10, column: 1, width: 100, height: 100 } },
        { client: "c4", sheet: 1 },
    ], 1);
    assert.deepEqual(Array.from(cells.keys()), ["2:2", "2:3", "10:1"]);
    assert.equal(cells.get("2:2"), calc.peerColor(0));
    assert.equal(cells.get("10:1"), calc.peerColor(2));
    assert.notEqual(calc.peerColor(0), calc.peerColor(1));
});

/** A table as the sheet builds one: the row numbers, then columns A to C, ten rows of no data. */
async function sheetTable(options) {
    const element = document.createElement("div");
    document.body.appendChild(element);
    const columns = [{ title: "", field: "id", width: 56, frozen: true, headerSort: false }]
        .concat([1, 2, 3].map((c) => ({ title: calc.columnName(c), field: "c" + c, calcColumn: c, headerSort: false })));
    const data = Array.from({ length: 10 }, (unused, at) => ({ id: at + 1 }));
    // calcColumn is the sheet's own column option, which Tabulator does not know
    const table = new Tabulator(element, Object.assign({ data, columns, index: "id", height: "320px", layout: "fitData", debugInvalidOptions: false,
        selectableRange: 1, selectableRangeColumns: true, selectableRangeRows: true }, options));
    await new Promise((resolve) => table.on("tableBuilt", resolve));
    return { table, done: () => { table.destroy(); element.remove(); } };
}

test("a drag of the fill handle is the engine's fill, down, up or across", () => {
    const rowAt = (at) => at + 1, columnAt = (at) => at;
    assert.deepEqual(calc.fillOp(1, { top: 0, bottom: 1, left: 1, right: 2 }, { top: 0, bottom: 4, left: 1, right: 2 }, rowAt, columnAt),
        { op: "fill", sheet: 1, row: 1, column: 1, width: 2, height: 2, toRow: 5 });
    assert.equal(calc.fillOp(1, { top: 3, bottom: 3, left: 2, right: 2 }, { top: 1, bottom: 3, left: 2, right: 2 }, rowAt, columnAt).toRow, 2);
    assert.equal(calc.fillOp(1, { top: 3, bottom: 3, left: 2, right: 2 }, { top: 3, bottom: 3, left: 2, right: 5 }, rowAt, columnAt).toColumn, 5);
    assert.equal(calc.fillOp(1, { top: 3, bottom: 3, left: 2, right: 2 }, { top: 3, bottom: 3, left: 1, right: 2 }, rowAt, columnAt).toColumn, 1);
    assert.equal(calc.fillOp(1, { top: 3, bottom: 3, left: 2, right: 2 }, { top: 3, bottom: 3, left: 2, right: 2 }, rowAt, columnAt), null);
});

// Tabulator 6.7's internals the sheet uses: the fill handle of selectRange (its mousedown, the cell moves, the document's
// mouseup it listens for, extendAlongAxis), and a row's setHeight
test("the fill handle's drop is taken from Tabulator and the range reached is selected", async () => {
    const { table, done } = await sheetTable({ selectableRangeFill: true });
    try {
        let dropped = null;
        assert.equal(calc.takeFillHandle(table, (source, target) => { dropped = { source, target }; }), true);
        table.addRange(table.getRow(2).getCell("c1"), table.getRow(3).getCell("c1"));
        await new Promise((resolve) => setTimeout(resolve, 20));
        const handle = table.modules.selectRange.fillHandle;
        handle.handleMouseDown({ button: 0, preventDefault() {}, stopPropagation() {} });
        handle.handleCellMouseMove(null, table.getRow(6).getCell("c1")._cell);
        document.dispatchEvent(new MouseEvent("mouseup"));
        assert.ok(dropped, "the drop was taken");
        assert.deepEqual([dropped.source.top, dropped.source.bottom, dropped.target.top, dropped.target.bottom], [1, 2, 1, 5]);
        const ranges = table.modules.selectRange, rows = ranges.getTableRows(), columns = ranges.getTableColumns();
        assert.deepEqual(calc.fillOp(0, dropped.source, dropped.target, (at) => rows[at].getData().id, (at) => columns[at].getDefinition().calcColumn),
            { op: "fill", sheet: 0, row: 2, column: 1, width: 1, height: 2, toRow: 6 });
        await new Promise((resolve) => setTimeout(resolve, 20));
        const bounds = table.getRanges()[0].getBounds();
        assert.deepEqual([bounds.start.row.data.id, bounds.end.row.data.id], [2, 6]);
        assert.equal(handle.preview, null, "the preview is gone");
        assert.equal(calc.takeFillHandle({ modules: {} }, () => null), false, "a table with no fill handle");
    } finally { done(); }
});

test("a row is given its own height, and the height it was given is read back", async () => {
    const heights = new Map([[3, 44]]);
    const { table, done } = await sheetTable({ rowFormatter: (row) => calc.setRowHeight(row, heights.get(row.getData().id)), resizableRows: true });
    try {
        assert.equal(calc.rowHeight(table.getRow(3)), 44);
        assert.ok(Math.abs(table.getRow(3).getElement().offsetHeight - 44) <= 2, String(table.getRow(3).getElement().offsetHeight));
        // drawn again (new cells), it keeps it
        table.getRow(3).reformat();
        assert.ok(Math.abs(table.getRow(3).getElement().offsetHeight - 44) <= 2);
        assert.ok(table.getRow(2).getElement().offsetHeight < 40, "another row keeps Tabulator's height");
        assert.ok(table.getRow(3).getElement().querySelector(".tabulator-row-resize-handle"), "a row can be resized");
    } finally { done(); }
});

test("the sheet's template compiles, and its border menu names the engine's borders", () => {
    const errors = [];
    const render = Vue.compile(calc.sheet.template, { onError: (err) => errors.push(err.message), onWarn: (err) => errors.push(err.message) });
    assert.deepEqual(errors, []);
    assert.equal(typeof render, "function");
    assert.deepEqual(calc.BORDER_KINDS.map((one) => one.kind), ["all", "outer", "inner", "top", "bottom", "left", "right", "none"]);
    assert.ok(calc.BORDER_STYLES.every((one) => /^[a-z]+$/.test(one.value)));
});

test("a click points at a reference only where a formula takes one, and a data sheet's is its table's columns", () => {
    assert.equal(calc.canPoint("=SUM(", 5), true);
    assert.equal(calc.canPoint("=", 1), true);
    assert.equal(calc.canPoint("=A1+ ", 5), true);
    assert.equal(calc.canPoint("=A1", 3), false);
    assert.equal(calc.canPoint("=SUM(A1", 7), false);
    assert.equal(calc.canPoint("12+", 3), false, "not a formula");
    assert.equal(calc.canPoint("=IF(A1>", 7), true);
    const orders = { index: 0, name: "Orders", data: true, headers: ["region", "amount", "placed"] };
    const summary = { index: 1, name: "Summary" }, other = { index: 2, name: "Joe's sheet" };
    // whatever rows are chosen: another scope's room has other rows
    assert.equal(calc.referenceText({ row: 2, column: 2, lastRow: 9, lastColumn: 2 }, orders, 1), "Orders[amount]");
    assert.equal(calc.referenceText({ row: 1, column: 1, lastRow: 1, lastColumn: 3 }, orders, 1), "Orders[[region]:[placed]]");
    assert.equal(calc.referenceText({ row: 2, column: 5, lastRow: 2, lastColumn: 5 }, orders, 1), null, "a column the data has not");
    assert.equal(calc.referenceText({ row: 2, column: 2, lastRow: 4, lastColumn: 3 }, summary, 1), "B2:C4");
    assert.equal(calc.referenceText({ row: 1, column: 1, lastRow: 1, lastColumn: 1 }, other, 1), "'Joe''s sheet'!A1");
    assert.deepEqual(calc.putReference("=SUM()", 5, 5, "B2:B5"), { text: "=SUM(B2:B5)", caret: 10 });
    assert.deepEqual(calc.putReference("=SUM(B2)", 5, 7, "B2:B5"), { text: "=SUM(B2:B5)", caret: 10 });
});

test("a click on a cell selects it and leaves the focus where it is while holding", async () => {
    const { table, done } = await sheetTable({});
    const input = document.createElement("input");
    document.body.appendChild(input);
    try {
        let holding = true;
        assert.equal(calc.holdFocus(table, () => holding), true);
        input.focus();
        const cell = table.getRow(4).getCell("c2").getElement();
        // the sheet's capture-phase press keeps the focus (its default prevented); Tabulator still selects
        cell.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0 }));
        cell.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, cancelable: true, button: 0 }));
        cell.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 }));
        await new Promise((resolve) => setTimeout(resolve, 20));
        assert.equal(document.activeElement, input, "the bar keeps the focus");
        const bounds = table.getRanges()[0].getBounds();
        assert.deepEqual([bounds.start.row.data.id, bounds.start.column.definition.calcColumn], [4, 2]);
        holding = false;
        cell.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 }));
        assert.notEqual(document.activeElement, input, "not holding, the table takes it");
        assert.equal(calc.holdFocus({ modules: {} }, () => true), false);
    } finally { input.remove(); done(); }
});

test("a cell's conditional format draws a data bar, icons and a rating, and its text is never HTML", () => {
    assert.equal(calc.decoration({ style: {} }), null);
    const bar = calc.decoration({ style: {}, data_bar: { value: 0.75, axis_position: 0.25, positive_color: "#638EC6", negative_color: "#FF0000", is_gradient: true, show_value: true } });
    assert.deepEqual(bar, { bar: { from: 0.25, to: 0.75, gradient: true, color: "#638EC6" } });
    assert.equal(calc.decoration({ data_bar: { value: 0.1, axis_position: 0.25, positive_color: "#638EC6", negative_color: "#C00000" } }).bar.color, "#C00000", "below the axis");
    const icons = calc.decoration({ icon: { icon: "ArrowUp", color: "#84cb1f", show_value: false }, rating: { icon: "Star", count: 2, max: 5, color: [4, 0.2], show_value: true } });
    assert.deepEqual(icons.icon, { glyph: calc.ICONS.ArrowUp, color: "#84cb1f" });
    assert.deepEqual(icons.rating, { glyph: calc.ICONS.Star, count: 2, max: 5, color: "" }, "a theme colour is not written into a style");
    assert.equal(icons.hide, true);
    // a block keeps a cell's decoration, and a cell with none is as before
    const rows = calc.blockRows({ row: 1, column: 1, width: 2, height: 1, styles: [{ style: {}, data_bar: { value: 0.5, axis_position: 0 } }, { style: {} }],
        style_indices: [0, 1], texts: [{ row: 1, column: 1, text: "10", cell_type: 1 }], fills: [] });
    assert.deepEqual(rows.get(1)[0].decor.bar, { from: 0, to: 0.5, gradient: false, color: "#638EC6" });
    assert.equal(rows.get(1)[1], undefined);
    const drawn = calc.cellNode(rows.get(1)[0]);
    assert.equal(drawn.querySelector(".m-calc-databar").style.width, "50%");
    assert.equal(drawn.querySelector(".m-calc-value").textContent, "10");
    const rated = calc.cellNode({ text: "3", decor: icons });
    assert.equal(rated.querySelector(".m-calc-value"), null, "its value hidden");
    assert.equal(rated.textContent, calc.ICONS.ArrowUp + calc.ICONS.Star.repeat(2) + calc.ICONS.Star.repeat(3));
    const typed = calc.cellNode({ text: "<img src=x onerror=alert(1)>" });
    assert.equal(typed.nodeType, Node.TEXT_NODE);
    assert.equal(typed.textContent, "<img src=x onerror=alert(1)>");
});

test("a conditional format's form is the engine's rule, and a rule of the sheet reads back into it", () => {
    const form = calc.ruleForm("B2:B9");
    assert.equal(calc.ruleOf(form), null, "no number yet");
    form.value = "=5";
    assert.deepEqual(calc.ruleOf(form), { type: "CellIs", operator: "GreaterThan", formula: "5", formula2: null, format: { fill: { color: "#FFCDD2" } }, stop_if_true: false });
    Object.assign(form, { operator: "Between", value2: "10", font: "#C62828", bold: true, fill: "" });
    const between = calc.ruleOf(form);
    assert.deepEqual([between.formula2, between.format], ["10", { font: { color: "#C62828", b: true } }]);
    assert.equal(calc.ruleLabel(between), "Value between 5 and 10");
    assert.deepEqual(calc.formOf("B2:B9", between), Object.assign(calc.ruleForm("B2:B9"), { operator: "Between", value: "5", value2: "10", fill: "", font: "#C62828", bold: true }));
    const top = calc.ruleOf(Object.assign(calc.ruleForm("A1:A5"), { kind: "top", rank: 3, percent: true }));
    assert.deepEqual([top.type, top.rank, top.percent, calc.ruleLabel(top)], ["Top10", 3, true, "Top 3%"]);
    const text = calc.ruleOf(Object.assign(calc.ruleForm("A1:A5"), { kind: "text", value: "West" }));
    assert.deepEqual([text.type, text.operator, calc.ruleLabel(text)], ["Text", "Contains", "Text contains \u201CWest\u201D"]);
    const scale = calc.ruleOf(Object.assign(calc.ruleForm("A1:A5"), { kind: "scale", low: "#FFFFFF", high: "#2E7D32" }));
    assert.deepEqual(scale.thresholds, [{ cfvo: "Min", color: "#FFFFFF" }, { cfvo: "Max", color: "#2E7D32" }]);
    assert.equal(calc.formOf("A1:A5", scale).high, "#2E7D32");
    assert.equal(calc.ruleOf(Object.assign(calc.ruleForm("A1:A5"), { kind: "bar" })).type, "DataBar");
    // a workbook's own rule the dialog does not make is listed, moved and deleted, not changed
    assert.equal(calc.formOf("A1:A5", { type: "IconSet", thresholds: [] }), null);
    assert.equal(calc.ruleLabel({ type: "IconSet" }), "Icons");
    assert.equal(calc.ruleLabel({ type: "DuplicateValues" }), "Duplicate Values");
});

test("a merged cell is selected whole and each of its cells draws its slice of the box", () => {
    const boxes = [{ row: 2, column: 2, width: 2, height: 3 }, { row: 1, column: 4, width: 3, height: 1 }];
    assert.deepEqual(calc.boxAt(boxes, 4, 3), boxes[0]);
    assert.equal(calc.boxAt(boxes, 5, 3), null);
    assert.equal(calc.boxAt([], 1, 1), null);
    // a selection meeting a box takes it whole, and a box it then meets
    assert.deepEqual(calc.expandToMerges({ row: 3, column: 3, lastRow: 3, lastColumn: 3 }, boxes), { row: 2, column: 2, lastRow: 4, lastColumn: 3 });
    assert.deepEqual(calc.expandToMerges({ row: 1, column: 3, lastRow: 2, lastColumn: 4 }, boxes), { row: 1, column: 2, lastRow: 4, lastColumn: 6 });
    assert.deepEqual(calc.expandToMerges({ row: 9, column: 9, lastRow: 9, lastColumn: 9 }, boxes), { row: 9, column: 9, lastRow: 9, lastColumn: 9 });
    // the anchor's content over the box, shifted to the cell's place in it, a pixel over the lines inside it
    const anchor = { text: "Total", css: { "font-weight": "600", "background-color": "#FFF9C4", "text-align": "center" } };
    const slice = calc.sliceNode(anchor, { x: 90, y: 28, width: 179, height: 83, right: false, below: true });
    assert.equal(slice.className, "m-calc-slice");
    assert.deepEqual([slice.style.right, slice.style.bottom, slice.style.backgroundColor], ["0px", "-1px", "rgb(255, 249, 196)"]);
    const box = slice.firstChild;
    assert.deepEqual([box.style.left, box.style.top, box.style.width, box.style.height, box.style.textAlign, box.style.fontWeight],
        ["-90px", "-28px", "179px", "83px", "center", "600"]);
    assert.equal(box.textContent, "Total");
    assert.equal(calc.sliceNode(null, { x: 0, y: 0, width: 10, height: 10, right: true, below: false }).firstChild.textContent, "", "an anchor not loaded yet");
});

test("a data sheet's column is escaped in its reference, a rating's icons are held to ten, and a rule is named by what it is", () => {
    assert.equal(calc.columnReference("total[usd]#1 @x 'y'"), "total'[usd']'#1 '@x ''y''");
    const odd = { index: 0, name: "Orders", data: true, headers: ["a[b", "c#d"] };
    assert.equal(calc.referenceText({ row: 2, column: 1, lastRow: 2, lastColumn: 1 }, odd, 1), "Orders[a'[b]");
    assert.equal(calc.referenceText({ row: 2, column: 1, lastRow: 2, lastColumn: 2 }, odd, 1), "Orders[[a'[b]:[c'#d]]");
    const rating = calc.decoration({ rating: { icon: "Star", count: 1e6, max: 1e6, color: "#FFC000" } }).rating;
    assert.deepEqual([rating.count, rating.max], [calc.MAX_RATING, calc.MAX_RATING]);
    assert.equal(calc.cellNode({ text: "", decor: { rating } }).textContent.length, calc.MAX_RATING);
    const one = { index: 2, range: "B1:B5", priority: 3, rule: { type: "Top10", rank: 1 } };
    assert.deepEqual(calc.ruleExpect(one), { range: "B1:B5", rule: { type: "Top10", rank: 1 } });
});

test("a fill handle not taken over is taken away", async () => {
    const { table, done } = await sheetTable({ selectableRangeFill: true });
    try {
        const ranges = table.modules.selectRange, element = ranges.fillHandle.element;
        calc.dropFillHandle(table);
        assert.equal(ranges.fillHandle, null);
        assert.equal(element.parentNode, null);
        table.addRange(table.getRow(2).getCell("c1"), table.getRow(2).getCell("c1"));
        await new Promise((resolve) => setTimeout(resolve, 20));
        assert.equal(document.querySelector(".tabulator-range-fill-handle"), null);
    } finally { done(); }
});

