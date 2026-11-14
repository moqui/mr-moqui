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
