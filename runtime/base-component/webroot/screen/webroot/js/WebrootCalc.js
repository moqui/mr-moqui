/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The sheet view of a CalcGrid document (plans/IronCalc-DB.md, Thin client) as moqui.calc, loaded by the screens that
 * show one (component://tools/screen/Tools/Data/Grid.qvue) and tested in the page by test/client/WebrootCalc.spec.js.
 * The workbook and the spreadsheet engine are in the server; this draws the cells on screen.
 *   - moqui.calc.sheet: a Vue component. Props: room (what open#CalcDocument answered: roomId, sheets, charts, ...) and
 *     cellsUrl (the transition that answers a rectangle of cells: roomId, sheet, row, column, width, height, and what
 *     params adds: an access profile). It emits
 *     gone when the server no longer has the room, and sheet (the index) when another sheet is chosen.
 *     A sheet is a Tabulator table (libs/tabulator) with a placeholder row for each sheet row and no data: a formatter
 *     draws each cell from the blocks the server sent, and a scroll asks for the rows in view and a margin, so a jump
 *     to a far row loads nothing between. Sheet tabs are below it and the document's charts (m-chart) below those.
 *   - the pure helpers: columnName(n), cellCss(style, fill, cellType) and applyCss(element, css), blockRows(block),
 *     neededRange(have, first, last, rows, margin) and chartConfig(chart).
 */
(function (root) {
    "use strict";

    const moqui = root.moqui || (root.moqui = {});
    const calc = moqui.calc = moqui.calc || {};

    /** How many rows beyond the ones in view are asked for, each way. */
    calc.MARGIN = 60;
    /** The most rows of one request (the server takes 20,000 cells). */
    calc.MAX_REQUEST_ROWS = 400;
    /** The rows and columns an empty sheet shows. */
    calc.MIN_ROWS = 40;
    calc.MIN_COLUMNS = 12;

    /** 1 is A, 27 is AA. */
    calc.columnName = (number) => {
        let name = "";
        for (let n = Number(number); n > 0; n = Math.floor((n - 1) / 26)) name = String.fromCharCode(65 + (n - 1) % 26) + name;
        return name;
    };

    const BORDER_WIDTH = { thin: "1px", medium: "2px", thick: "3px", double: "3px", dotted: "1px", slantdashdot: "1px", mediumdashed: "2px",
        mediumdashdotdot: "2px", mediumdashdot: "2px" };
    const BORDER_LINE = { double: "double", dotted: "dotted", mediumdashed: "dashed", mediumdashdotdot: "dashed", mediumdashdot: "dashed", slantdashdot: "dashed" };
    const color = (value) => (typeof value === "string" && /^#[0-9a-fA-F]{6}$/.test(value) ? value : "");

    /**
     * The CSS properties of a cell, or null when it has none: the engine's extended style (its font, alignment and
     * borders), the fill of the run the cell is in, and the cell's type (a number with no alignment of its own is
     * right-aligned, a boolean or an error centered).
     */
    calc.cellCss = (extended, fill, cellType) => {
        const style = (extended && extended.style) || {};
        const font = style.font || {};
        const css = {};
        if (font.b) css["font-weight"] = "600";
        if (font.i) css["font-style"] = "italic";
        const lines = [font.u ? "underline" : "", font.strike ? "line-through" : ""].filter(Boolean);
        if (lines.length) css["text-decoration"] = lines.join(" ");
        if (color(font.color)) css.color = font.color;
        if (color(fill)) css["background-color"] = fill;
        const alignment = style.alignment || {};
        const horizontal = alignment.horizontal && alignment.horizontal !== "general" ? alignment.horizontal
            : cellType === 1 ? "right" : cellType === 4 || cellType === 16 ? "center" : "";
        if (horizontal === "right" || horizontal === "center" || horizontal === "left") css["text-align"] = horizontal;
        if (alignment.wrap_text) css["white-space"] = "normal";
        const border = style.border || {};
        for (const side of ["top", "right", "bottom", "left"]) {
            const item = border[side];
            if (!item || !item.style) continue;
            css["border-" + side] = (BORDER_WIDTH[item.style] || "1px") + " " + (BORDER_LINE[item.style] || "solid") + " " + (color(item.color) || "currentColor");
        }
        return Object.keys(css).length ? css : null;
    };

    /** Set a cell element's CSS to `css` (of cellCss), taking away what an earlier call set and this one does not. */
    calc.applyCss = (element, css) => {
        for (const name of element.calcProperties || []) if (!css || !(name in css)) element.style.removeProperty(name);
        if (css) for (const [name, value] of Object.entries(css)) element.style.setProperty(name, value);
        element.calcProperties = css ? Object.keys(css) : null;
    };

    /**
     * A block as rows: a Map of row number to an array by column (from the block's first column) of {text, css}, a
     * cell with neither left out.
     */
    calc.blockRows = (block) => {
        const rows = new Map();
        const width = Number(block.width), first = Number(block.column), top = Number(block.row);
        const fills = new Map();
        for (const run of block.fills || []) if (run.color) for (let c = 0; c < run.width; c++) fills.set(run.row + ":" + (run.column + c), run.color);
        const texts = new Map();
        for (const text of block.texts || []) texts.set(text.row + ":" + text.column, text);
        for (let r = 0; r < Number(block.height); r++) {
            const cells = new Array(width);
            for (let c = 0; c < width; c++) {
                const key = (top + r) + ":" + (first + c);
                const text = texts.get(key);
                const css = calc.cellCss((block.styles || [])[(block.style_indices || [])[r * width + c]], fills.get(key), text ? text.cell_type : 0);
                if (text || css) cells[c] = { text: text ? text.text : "", css };
            }
            rows.set(top + r, cells);
        }
        return rows;
    };

    /**
     * The rows to ask for so that first to last and a margin each way are loaded: null when have (a Set or Map of the
     * loaded row numbers) holds them all, else {row, height}, from the first row missing to the last, at most
     * MAX_REQUEST_ROWS.
     */
    calc.neededRange = (have, first, last, rows, margin) => {
        const from = Math.max(1, first - margin), to = Math.min(rows, last + margin);
        let low = null, high = null;
        for (let r = from; r <= to; r++) if (!have.has(r)) { if (low == null) low = r; high = r; }
        if (low == null) return null;
        // the rows in view first when the missing ones are more than one request takes
        if (high - low + 1 > calc.MAX_REQUEST_ROWS) { low = Math.max(low, Math.min(first, high - calc.MAX_REQUEST_ROWS + 1)); high = Math.min(high, low + calc.MAX_REQUEST_ROWS - 1); }
        return { row: low, height: high - low + 1 };
    };

    const CHART_COLORS = ["#1976d2", "#26a69a", "#ef6c00", "#9c27b0", "#c62828", "#558b2f", "#00838f", "#6d4c41"];
    /** A chart of the document (type bar, line, area or pie; labels; series of name and data) as a Chart.js config. */
    calc.chartConfig = (chart) => {
        const type = chart.type === "pie" ? "pie" : chart.type === "bar" || chart.type === "column" ? "bar" : "line";
        const series = chart.series || [];
        return { type, data: { labels: chart.labels || [], datasets: series.map((one, index) => {
            const shade = CHART_COLORS[index % CHART_COLORS.length];
            return { label: one.name || "Series " + (index + 1), data: one.data || [], borderColor: type === "pie" ? "#fff" : shade,
                backgroundColor: type === "pie" ? (one.data || []).map((unused, at) => CHART_COLORS[at % CHART_COLORS.length]) : shade,
                fill: chart.type === "area", tension: 0, pointRadius: type === "line" ? 2 : 0 };
        }) }, options: { maintainAspectRatio: false, animation: false, indexAxis: chart.type === "bar" ? "y" : "x",
            plugins: { legend: { display: type === "pie" || series.length > 1 } }, scales: type === "pie" ? {} : { y: { beginAtZero: true } } } };
    };

    calc.sheet = {
        name: "CalcSheet",
        props: { room: { type: Object, required: true }, cellsUrl: { type: String, required: true }, height: { type: String, default: "62vh" },
            // what each request for cells carries beside the room and the rectangle (an access profile)
            params: { type: Object, default: () => ({}) } },
        emits: ["gone", "sheet"],
        template: '<div class="m-calc-sheet">'
            + '<div ref="grid" class="m-data-grid m-calc-grid"></div>'
            + '<div class="row items-center no-wrap q-mt-xs">'
            + '<q-tabs dense no-caps inline-label align="left" class="col" :model-value="sheetIndex" @update:model-value="showSheet">'
            + '<q-tab v-for="sheet in visibleSheets" :key="sheet.index" :name="sheet.index" :label="sheet.name" :icon="sheet.data ? \'table_rows\' : undefined">'
            + '<q-tooltip v-if="sheet.data">A data sheet: {{Number(sheet.dataRows).toLocaleString()}} rows from the database, read only</q-tooltip></q-tab>'
            + '</q-tabs>'
            + '<div class="col-auto text-caption q-px-sm" :class="mutedClass">{{status}}</div>'
            + '</div>'
            + '<div v-if="charts.length" class="row q-col-gutter-sm q-mt-xs">'
            + '<div v-for="(chart, index) in charts" :key="index" class="col-12 col-md-6">'
            + '<div class="text-subtitle2">{{chart.title || "Chart " + (index + 1)}}</div>'
            + '<div v-if="chart.error" class="text-caption text-negative">{{chart.error}}</div>'
            + '<m-chart v-else height="260px" :config="chartConfig(chart)"></m-chart>'
            + '</div></div>'
            + '</div>',
        data() { return { sheetIndex: 0, status: "" }; },
        computed: {
            sheets() { return (this.room && this.room.sheets) || []; },
            visibleSheets() { return this.sheets.filter((sheet) => !sheet.hidden); },
            charts() { return (this.room && this.room.charts) || []; },
            mutedClass() { return this.$q.dark.isActive ? "text-grey-5" : "text-grey-7"; }
        },
        watch: {
            // another room (a document opened, or opened again after a refresh): every sheet is drawn anew
            room() { this.cache = new Map(); if (!this.sheets.some((sheet) => sheet.index === this.sheetIndex && !sheet.hidden)) this.sheetIndex = this.firstSheet(); this.build(); }
        },
        created() { this.cache = new Map(); this.table = null; this.busy = false; this.again = false; this.sheetIndex = this.firstSheet(); },
        mounted() {
            moqui.loadStylesheet("/libs/tabulator/tabulator.min.css");
            moqui.loadStylesheet("/css/DataGrid.css");
            moqui.loadScript("/libs/tabulator/tabulator.min.js", null, () => !!root.Tabulator).then(() => this.build());
        },
        beforeUnmount() { this.destroy(); },
        methods: {
            chartConfig(chart) { return calc.chartConfig(chart); },
            firstSheet() { const first = this.visibleSheets[0]; return first ? first.index : 0; },
            sheet() { return this.sheets.find((sheet) => sheet.index === this.sheetIndex) || null; },
            showSheet(index) { if (index === this.sheetIndex) return; this.sheetIndex = index; this.$emit("sheet", index); this.build(); },
            destroy() { if (this.table) { try { this.table.destroy(); } catch (_) { /* gone */ } this.table = null; } },
            // the loaded rows of the sheet shown: row number to its cells by column
            rows() {
                let rows = this.cache.get(this.sheetIndex);
                if (!rows) { rows = new Map(); this.cache.set(this.sheetIndex, rows); }
                return rows;
            },
            build() {
                this.destroy();
                const element = this.$refs.grid, sheet = this.sheet();
                if (!element || !root.Tabulator || !sheet) return;
                const vm = this;
                const rowCount = Math.max(Number(sheet.rows) || 0, calc.MIN_ROWS), columnCount = Math.max(Number(sheet.columns) || 0, calc.MIN_COLUMNS);
                this.columnCount = columnCount; this.rowCount = rowCount;
                const formatter = (cell) => {
                    const cells = vm.rows().get(cell.getRow().getData().id);
                    const found = cells ? cells[cell.getColumn().getDefinition().calcColumn - 1] : null;
                    calc.applyCss(cell.getElement(), found ? found.css : null);
                    return found ? found.text : "";
                };
                // a copy takes the texts on screen: the table holds no data of its own
                const accessorClipboard = (value, data, type, params, column) => {
                    const cells = vm.rows().get(data.id), at = column && column.getDefinition().calcColumn;
                    return cells && at && cells[at - 1] ? cells[at - 1].text : "";
                };
                const columns = [{ title: "", field: "id", width: 56, frozen: true, headerSort: false, resizable: false, hozAlign: "right", cssClass: "m-calc-row-header" }];
                for (let c = 1; c <= columnCount; c++) columns.push({ title: calc.columnName(c), field: "c" + c, calcColumn: c, headerSort: false, resizable: true,
                    headerHozAlign: "center", width: Math.max(30, Number((sheet.widths || [])[c - 1]) || 90), frozen: c <= Number(sheet.frozenColumns || 0), formatter, accessorClipboard });
                const data = new Array(rowCount);
                for (let r = 0; r < rowCount; r++) data[r] = { id: r + 1 };
                this.table = new root.Tabulator(element, { data, columns, index: "id", height: this.height, layout: "fitData", placeholder: "",
                    frozenRows: Number(sheet.frozenRows || 0), columnDefaults: { tooltip: false },
                    selectableRange: 1, selectableRangeColumns: true, selectableRangeRows: true, selectableRangeClearCells: false,
                    clipboard: "copy", clipboardCopyStyled: false, clipboardCopyRowRange: "range", clipboardCopyConfig: { rowHeaders: false, columnHeaders: false } });
                this.table.on("tableBuilt", () => { vm.table.on("scrollVertical", () => vm.load()); vm.load(); });
            },
            // the first and last sheet rows in view
            visible() {
                const rows = this.table ? this.table.getRows("visible") : [];
                return rows.length ? [rows[0].getData().id, rows[rows.length - 1].getData().id] : null;
            },
            // one request at a time; a scroll while it runs asks again when it answers
            load() {
                if (this.busy) { this.again = true; return Promise.resolve(); }
                this.busy = true;
                const step = () => { this.again = false; return this.fetchVisible().then(() => (this.again ? step() : null)); };
                return step().then(() => { this.busy = false; }, () => { this.busy = false; });
            },
            fetchVisible() {
                const table = this.table, sheetIndex = this.sheetIndex, view = this.visible();
                if (!table || !view) return Promise.resolve();
                const rows = this.rows();
                const need = calc.neededRange(rows, view[0], view[1], this.rowCount, calc.MARGIN);
                if (!need) return Promise.resolve();
                return moqui.request(this.cellsUrl, { params: Object.assign({}, this.params, { roomId: this.room.roomId, sheet: sheetIndex, row: need.row, column: 1, width: this.columnCount, height: need.height }) }).then((result) => {
                    const block = result.json || {};
                    if (block.errors) {
                        if (String(block.errors).includes("ROOM_GONE")) this.$emit("gone"); else moqui.notifyMessages(null, block.errors);
                        return;
                    }
                    for (const [row, cells] of calc.blockRows(block)) rows.set(row, cells);
                    // the sheet may have changed while the request ran: its rows are kept, and nothing of this table is drawn
                    if (this.table !== table || this.sheetIndex !== sheetIndex) return;
                    for (const row of table.getRows("visible")) row.reformat();
                    // the frozen rows are in the table's header, not among the rows in view
                    const sheetNow = this.sheet();
                    for (let r = 1; r <= Number((sheetNow && sheetNow.frozenRows) || 0); r++) { const frozen = table.getRow(r); if (frozen) frozen.reformat(); }
                    const sheet = this.sheet();
                    this.status = sheet ? Number(sheet.rows).toLocaleString() + " rows × " + Number(sheet.columns).toLocaleString() + " columns" : "";
                }, (err) => { moqui.handleAjaxError(err); });
            }
        }
    };
})(typeof window !== "undefined" ? window : globalThis);
