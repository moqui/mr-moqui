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
