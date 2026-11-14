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
 *     With editUrl (the transition that applies edits: roomId, opsJson) and contentUrl (a cell's content as typed) the
 *     sheet is edited: a toolbar (undo, redo, font, colours, alignment, number format, rows and columns, a chart), a
 *     formula bar that completes function and table names, a cell's editor on a double click, Delete, a paste of rows of
 *     cells, a column's width, and sheets added, renamed and deleted. Each edit is sent to the server, which answers the
 *     room's new state (emitted as changed, for the holder of room to take); the cells are then asked again. A data
 *     sheet is not edited. It also emits selection (sheet, row, column, lastRow, lastColumn and the range's name).
 *     A saved document's room (room.shared) is edited together: the sheet joins it on the notification socket
 *     ($root.notificationClient, frames with calc: join, select, leave; docs/ui/Qvt-Client.md, CalcGrid rooms), shows
 *     the others' places (a coloured outline and their names), sends its own, and on another's edit reads the room's
 *     state again (stateUrl) and its cells; with revertUrl an author discards everyone's unsaved edits.
 *   - the pure helpers: columnName(n), cellCss(style, fill, cellType) and applyCss(element, css), blockRows(block),
 *     neededRange(have, first, last, rows, margin), chartConfig(chart), completions(text, caret, tables, functions),
 *     rangeName(range, sheetName), pasteRows(text), tableName(sheetName), peerColor(index), receive(peers, seq, frame)
 *     and peerCells(peers, sheet).
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

    /** The functions the formula bar offers by their first letters. The engine has about 500; these are the common ones. */
    calc.FUNCTIONS = ["ABS", "AND", "AVERAGE", "AVERAGEIF", "AVERAGEIFS", "CEILING", "CHOOSE", "CONCAT", "CONCATENATE", "COUNT", "COUNTA", "COUNTBLANK", "COUNTIF",
        "COUNTIFS", "DATE", "DATEDIF", "DAY", "DAYS", "EDATE", "EOMONTH", "EXACT", "FILTER", "FIND", "FLOOR", "HLOOKUP", "HOUR", "IF", "IFERROR", "IFNA", "IFS",
        "INDEX", "INT", "ISBLANK", "ISERROR", "ISNUMBER", "ISTEXT", "LARGE", "LEFT", "LEN", "LOWER", "MATCH", "MAX", "MAXIFS", "MEDIAN", "MID", "MIN", "MINIFS",
        "MINUTE", "MOD", "MONTH", "NETWORKDAYS", "NOT", "NOW", "OR", "POWER", "PRODUCT", "PROPER", "RAND", "RANDBETWEEN", "RANK", "REPLACE", "RIGHT", "ROUND",
        "ROUNDDOWN", "ROUNDUP", "ROWS", "SEARCH", "SECOND", "SMALL", "SORT", "SQRT", "STDEV", "SUBSTITUTE", "SUBTOTAL", "SUM", "SUMIF", "SUMIFS", "SUMPRODUCT",
        "SWITCH", "TEXT", "TEXTJOIN", "TIME", "TODAY", "TRIM", "UNIQUE", "UPPER", "VALUE", "VLOOKUP", "WEEKDAY", "WEEKNUM", "XLOOKUP", "YEAR"];
    calc.NUMBER_FORMATS = [{ label: "General", value: "general" }, { label: "1234", value: "0" }, { label: "1234.50", value: "0.00" }, { label: "1,234.50", value: "#,##0.00" },
        { label: "12.5%", value: "0.0%" }, { label: "2026-11-13", value: "yyyy-mm-dd" }, { label: "2026-11-13 14:30", value: "yyyy-mm-dd hh:mm" }];
    calc.COLORS = ["#000000", "#C62828", "#EF6C00", "#F9A825", "#2E7D32", "#1565C0", "#6A1B9A", "#757575", "#FFFFFF", "#FFCDD2", "#FFE0B2", "#FFF9C4", "#C8E6C9", "#BBDEFB", "#E1BEE7", "#EEEEEE"];

    /**
     * What a formula being typed may complete to: null when text is not a formula or the caret is not after the letters
     * of a name; else {from, prefix, names}: the names (functions, and tables as Name[) that begin with the letters
     * before the caret, and where those letters start.
     */
    calc.completions = (text, caret, tables, functions) => {
        if (!text || text[0] !== "=") return null;
        const before = text.slice(0, caret);
        const match = /[A-Za-z_][A-Za-z0-9_.]*$/.exec(before);
        if (!match || (match.index > 0 && /[0-9$!"\]]/.test(before[match.index - 1]))) return null;
        const prefix = match[0].toUpperCase();
        // a cell reference (B2, AA10) is not a name to complete
        if (/^[A-Z]{1,3}[0-9]+$/.test(prefix)) return null;
        const names = (functions && functions.length ? functions : calc.FUNCTIONS).filter((name) => name.startsWith(prefix)).map((name) => name + "(")
            .concat((tables || []).filter((name) => name.toUpperCase().startsWith(prefix)).map((name) => name + "["));
        return names.length ? { from: match.index, prefix: match[0], names: names.slice(0, 8) } : null;
    };
    /** A cell or a range as a formula names it: B2, B2:D7; with a sheet name, quoted when it needs it. */
    calc.rangeName = (range, sheetName) => {
        const cells = calc.columnName(range.column) + range.row
            + (range.lastRow > range.row || range.lastColumn > range.column ? ":" + calc.columnName(range.lastColumn) + range.lastRow : "");
        if (!sheetName) return cells;
        return (/^[A-Za-z_][A-Za-z0-9_]*$/.test(sheetName) ? sheetName : "'" + sheetName.replace(/'/g, "''") + "'") + "!" + cells;
    };
    /** Clipboard text as rows of cells: lines of tab-separated values, a last empty line dropped. */
    calc.pasteRows = (text) => {
        const lines = String(text || "").replace(/\r\n?/g, "\n").split("\n");
        if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
        return lines.map((line) => line.split("\t"));
    };
    /** The table a sheet gives its formulas, as the server names it: the sheet's name as an identifier. */
    calc.tableName = (sheetName) => { const name = String(sheetName).replace(/[^\p{L}\p{N}]/gu, "_"); return /^\p{L}/u.test(name) ? name : "T" + name; };

    /** The colours the others' places are drawn in, by their order. */
    calc.PEER_COLORS = ["#e67c00", "#1e88e5", "#8e24aa", "#00897b", "#d81b60", "#6d4c41"];
    calc.peerColor = (index) => calc.PEER_COLORS[Math.abs(Number(index) || 0) % calc.PEER_COLORS.length];
    /** The most cells of another's selection drawn; a larger one is drawn at its first cell. */
    calc.MAX_PEER_CELLS = 400;
    /**
     * A frame of the room's socket topic taken into the others in the room (`peers`: client, userId, name, sheet, area)
     * and the view's last edit (`seq`): the others now, whether to read the room again (another's edit, a refresh, a
     * save), and whether to open the document again (its room was dropped).
     */
    calc.receive = (peers, seq, frame) => {
        const list = (peers || []).slice();
        const at = list.findIndex((peer) => peer.client === frame.client);
        const out = { peers: list, reread: false, reopen: false };
        if (frame.type === "joined") out.peers = (frame.peers || []).map((peer) => Object.assign({}, peer));
        else if (frame.type === "peer" && frame.joined === false) { if (at >= 0) list.splice(at, 1); }
        else if (frame.type === "peer" || frame.type === "select") {
            const peer = Object.assign(at >= 0 ? list[at] : {}, { client: frame.client, userId: frame.userId, name: frame.name });
            if (frame.type === "select") { peer.sheet = frame.sheet; peer.area = frame.area; }
            if (at < 0) list.push(peer); else list[at] = peer;
        } else if (frame.type === "changed") {
            out.reopen = frame.reopen === true;
            out.reread = !out.reopen && (frame.kind === "refresh" || frame.kind === "save" || Number(frame.seq) > Number(seq || 0));
        }
        return out;
    };
    /** The rows that hold changed cells, dropped from the cache of loaded rows (sheet to row to cells), so that the next
     * load asks for them again and keeps the rest. `changed` is the areas an edit and its evaluation changed (sheet,
     * row, column, width, height); false when there is none, or too many rows to go one by one: the cache is then
     * emptied instead. */
    calc.forgetRows = (cache, changed) => {
        if (!Array.isArray(changed)) return false;
        if (changed.reduce((rows, area) => rows + Number(area.height || 0), 0) > calc.MAX_FORGET_ROWS) return false;
        for (const area of changed) {
            const rows = cache.get(Number(area.sheet));
            if (!rows) continue;
            for (let row = Number(area.row); row < Number(area.row) + Number(area.height); row++) rows.delete(row);
        }
        return true;
    };
    calc.MAX_FORGET_ROWS = 5000;
    /** The cells of `sheet` the others are on: "row:column" to the colour of whoever is there. */
    calc.peerCells = (peers, sheet) => {
        const cells = new Map();
        (peers || []).forEach((peer, index) => {
            const area = peer.area;
            if (Number(peer.sheet) !== Number(sheet) || !area || !area.row || !area.column) return;
            const width = Math.max(1, Number(area.width) || 1), height = Math.max(1, Number(area.height) || 1);
            const whole = width * height <= calc.MAX_PEER_CELLS;
            for (let r = 0; r < (whole ? height : 1); r++)
                for (let c = 0; c < (whole ? width : 1); c++) cells.set((area.row + r) + ":" + (area.column + c), calc.peerColor(index));
        });
        return cells;
    };

    calc.sheet = {
        name: "CalcSheet",
        props: { room: { type: Object, required: true }, cellsUrl: { type: String, required: true }, height: { type: String, default: "62vh" },
            // what each request carries beside the room and the rectangle (an access profile)
            params: { type: Object, default: () => ({}) },
            // the transitions that edit the workbook and answer a cell's content; with editUrl the sheet is edited
            editUrl: { type: String, default: "" }, contentUrl: { type: String, default: "" },
            // the transitions that answer the room's state (read again on another's edit) and discard the unsaved edits
            stateUrl: { type: String, default: "" }, revertUrl: { type: String, default: "" } },
        emits: ["gone", "sheet", "changed", "selection"],
        template: '<div class="m-calc-sheet">'
            + '<div v-if="editable" class="row items-center q-gutter-xs q-mb-xs m-calc-toolbar">'
            + '<q-btn flat dense round size="sm" icon="undo" aria-label="Undo" :disable="!room.canUndo" @click="edit([{op: \'undo\'}])"><q-tooltip>Undo</q-tooltip></q-btn>'
            + '<q-btn flat dense round size="sm" icon="redo" aria-label="Redo" :disable="!room.canRedo" @click="edit([{op: \'redo\'}])"><q-tooltip>Redo</q-tooltip></q-btn>'
            + '<q-separator vertical></q-separator>'
            + '<q-btn flat dense round size="sm" icon="format_bold" aria-label="Bold" @click="toggleStyle(\'font.b\', \'font-weight\')"><q-tooltip>Bold</q-tooltip></q-btn>'
            + '<q-btn flat dense round size="sm" icon="format_italic" aria-label="Italic" @click="toggleStyle(\'font.i\', \'font-style\')"><q-tooltip>Italic</q-tooltip></q-btn>'
            + '<q-btn flat dense round size="sm" icon="format_underlined" aria-label="Underline" @click="toggleStyle(\'font.u\', \'text-decoration\')"><q-tooltip>Underline</q-tooltip></q-btn>'
            + '<q-btn flat dense round size="sm" icon="format_color_text" aria-label="Text colour"><q-tooltip>Text colour</q-tooltip><q-menu><div class="row q-pa-xs" style="width:176px">'
            + '<div v-for="shade in colors" :key="shade" v-close-popup class="m-calc-swatch" :style="{backgroundColor: shade}" @click="style(\'font.color\', shade)"></div></div></q-menu></q-btn>'
            + '<q-btn flat dense round size="sm" icon="format_color_fill" aria-label="Fill colour"><q-tooltip>Fill colour</q-tooltip><q-menu><div class="row q-pa-xs" style="width:176px">'
            + '<div v-for="shade in colors" :key="shade" v-close-popup class="m-calc-swatch" :style="{backgroundColor: shade}" @click="style(\'fill.color\', shade)"></div></div></q-menu></q-btn>'
            + '<q-separator vertical></q-separator>'
            + '<q-btn flat dense round size="sm" icon="format_align_left" aria-label="Align left" @click="style(\'alignment.horizontal\', \'left\')"></q-btn>'
            + '<q-btn flat dense round size="sm" icon="format_align_center" aria-label="Align center" @click="style(\'alignment.horizontal\', \'center\')"></q-btn>'
            + '<q-btn flat dense round size="sm" icon="format_align_right" aria-label="Align right" @click="style(\'alignment.horizontal\', \'right\')"></q-btn>'
            + '<q-btn-dropdown flat dense no-caps size="sm" label="Format"><q-list dense>'
            + '<q-item v-for="format in numberFormats" :key="format.value" clickable v-close-popup @click="style(\'num_fmt\', format.value)"><q-item-section>{{format.label}}</q-item-section></q-item>'
            + '</q-list></q-btn-dropdown>'
            + '<q-btn-dropdown flat dense no-caps size="sm" label="Rows and columns"><q-list dense>'
            + '<q-item clickable v-close-popup @click="structure(\'insertRows\')"><q-item-section>Insert a row above</q-item-section></q-item>'
            + '<q-item clickable v-close-popup @click="structure(\'deleteRows\')"><q-item-section>Delete the rows selected</q-item-section></q-item>'
            + '<q-item clickable v-close-popup @click="structure(\'insertColumns\')"><q-item-section>Insert a column to the left</q-item-section></q-item>'
            + '<q-item clickable v-close-popup @click="structure(\'deleteColumns\')"><q-item-section>Delete the columns selected</q-item-section></q-item>'
            + '<q-separator></q-separator>'
            + '<q-item clickable v-close-popup @click="freeze(true)"><q-item-section>Freeze down to this row and column</q-item-section></q-item>'
            + '<q-item clickable v-close-popup @click="freeze(false)"><q-item-section>Unfreeze</q-item-section></q-item>'
            + '<q-item clickable v-close-popup @click="fillDown"><q-item-section>Fill the first row selected down</q-item-section></q-item>'
            + '</q-list></q-btn-dropdown>'
            + '<q-btn flat dense no-caps size="sm" icon="insert_chart" label="Chart" @click="openChart"><q-tooltip>A chart of the cells selected</q-tooltip></q-btn>'
            + '</div>'
            + '<div v-if="editable || contentUrl" class="row items-center no-wrap q-mb-xs m-calc-bar">'
            + '<div class="m-calc-name text-caption q-px-sm">{{activeName}}</div>'
            + '<div class="col" style="position:relative">'
            + '<input ref="bar" class="m-calc-formula" :readonly="!editable || activeIsData" :value="barText" aria-label="Cell content" spellcheck="false" autocomplete="off"'
            + ' @input="barInput" @keydown="barKey" @blur="suggestions = null">'
            + '<div v-if="suggestions" class="m-calc-suggest shadow-2"><div v-for="(name, index) in suggestions.names" :key="name" class="m-calc-suggest-item"'
            + ' :class="{\'m-calc-suggest-on\': index === suggestionIndex}" @mousedown.prevent="takeSuggestion(name)">{{name}}</div></div>'
            + '</div></div>'
            + '<div ref="grid" class="m-data-grid m-calc-grid" tabindex="0" @keydown="gridKey" @paste="gridPaste"></div>'
            + '<div class="row items-center no-wrap q-mt-xs">'
            + '<q-tabs dense no-caps inline-label align="left" class="col" :model-value="sheetIndex" @update:model-value="showSheet">'
            + '<q-tab v-for="sheet in visibleSheets" :key="sheet.index" :name="sheet.index" :label="sheet.name" :icon="sheet.data ? \'table_rows\' : undefined">'
            + '<q-tooltip v-if="sheet.data">A data sheet: {{Number(sheet.dataRows).toLocaleString()}} rows from the database, read only</q-tooltip>'
            + '<q-menu v-if="editable && !sheet.data" context-menu><q-list dense>'
            + '<q-item clickable v-close-popup @click="renameSheet(sheet)"><q-item-section>Rename</q-item-section></q-item>'
            + '<q-item clickable v-close-popup @click="deleteSheet(sheet)"><q-item-section>Delete</q-item-section></q-item></q-list></q-menu></q-tab>'
            + '</q-tabs>'
            + '<q-btn v-if="editable" flat dense round size="sm" icon="add" aria-label="Add a sheet" @click="addSheet"><q-tooltip>Add a sheet. Right-click a sheet to rename or delete it</q-tooltip></q-btn>'
            + '<q-chip v-for="(peer, index) in peers" :key="peer.client" dense square size="sm" text-color="white" :style="{backgroundColor: peerColor(index)}">{{peer.name}}'
            + '<q-tooltip>{{peer.name}}{{peer.area ? " is at " + peerPlace(peer) : " has the document open"}}</q-tooltip></q-chip>'
            + '<div v-if="room.unsaved" class="col-auto text-caption q-px-xs" :class="mutedClass">{{room.unsaved}} unsaved {{room.unsaved === 1 ? "edit" : "edits"}}</div>'
            + '<q-btn v-if="editable && revertUrl && room.unsaved" flat dense no-caps size="sm" label="Discard" @click="discard"><q-tooltip>Discard every unsaved edit, everyone\'s, back to the saved document</q-tooltip></q-btn>'
            + '<div class="col-auto text-caption q-px-sm" :class="mutedClass">{{notice || status}}</div>'
            + '</div>'
            + '<div v-if="charts.length" class="row q-col-gutter-sm q-mt-xs">'
            + '<div v-for="(chart, index) in charts" :key="index" class="col-12 col-md-6">'
            + '<div class="row items-center"><div class="text-subtitle2 col">{{chart.title || "Chart " + (index + 1)}}</div>'
            + '<q-btn v-if="editable" flat dense round size="sm" icon="close" :aria-label="\'Remove chart \' + (index + 1)" @click="removeChart(index)"></q-btn></div>'
            + '<div v-if="chart.error" class="text-caption text-negative">{{chart.error}}</div>'
            + '<m-chart v-else height="260px" :config="chartConfig(chart)"></m-chart>'
            + '</div></div>'
            + '<q-dialog v-model="chartOpen"><q-card style="min-width:400px;max-width:90vw">'
            + '<q-card-section><div class="text-h6">Chart</div></q-card-section>'
            + '<q-card-section class="q-pt-none q-gutter-sm">'
            + '<q-input dense outlined v-model="chart.title" label="Title"></q-input>'
            + '<q-option-group v-model="chart.type" inline dense :options="[{label: \'Columns\', value: \'column\'}, {label: \'Bars\', value: \'bar\'}, {label: \'Line\', value: \'line\'}, {label: \'Area\', value: \'area\'}, {label: \'Pie\', value: \'pie\'}]"></q-option-group>'
            + '<q-input dense outlined v-model="chart.categories" label="Labels" hint="A range: Summary!A2:A8"></q-input>'
            + '<q-input dense outlined v-model="chart.values" label="Values" hint="A range for each series, with commas between: Summary!B2:B8, Summary!C2:C8"></q-input>'
            + '</q-card-section>'
            + '<q-card-actions align="right"><q-btn flat no-caps label="Cancel" v-close-popup></q-btn>'
            + '<q-btn unelevated no-caps color="primary" label="Add" :disable="!chart.values" @click="addChart"></q-btn></q-card-actions>'
            + '</q-card></q-dialog>'
            + '</div>',
        data() {
            return { sheetIndex: 0, status: "", notice: "", active: { row: 1, column: 1, lastRow: 1, lastColumn: 1 }, barText: "", barOriginal: "", suggestions: null, suggestionIndex: 0,
                chartOpen: false, chart: { title: "", type: "column", categories: "", values: "" }, peers: [] };
        },
        computed: {
            sheets() { return (this.room && this.room.sheets) || []; },
            visibleSheets() { return this.sheets.filter((sheet) => !sheet.hidden); },
            charts() { return (this.room && this.room.charts) || []; },
            mutedClass() { return this.$q.dark.isActive ? "text-grey-5" : "text-grey-7"; },
            editable() { return !!this.editUrl; },
            activeSheet() { return this.sheets.find((sheet) => sheet.index === this.sheetIndex) || null; },
            activeIsData() { return !!(this.activeSheet && this.activeSheet.data); },
            activeName() { return calc.rangeName(this.active); },
            colors() { return calc.COLORS; },
            numberFormats() { return calc.NUMBER_FORMATS; },
            tableNames() { return this.sheets.filter((sheet) => sheet.data).map((sheet) => calc.tableName(sheet.name)); }
        },
        watch: {
            // another room (a document opened, or opened again after a refresh): every sheet is drawn anew
            room() {
                this.cache = new Map(); this.seq = Number(this.room.seq || 0);
                if (!this.sheets.some((sheet) => sheet.index === this.sheetIndex && !sheet.hidden)) this.sheetIndex = this.firstSheet();
                this.build(); this.connect();
            },
            // a scratch workbook saved is the document's room, under a new id
            "room.roomId"() { this.connect(); }
        },
        created() {
            this.cache = new Map(); this.table = null; this.busy = false; this.again = false; this.sheetIndex = this.firstSheet(); this.contentTimer = null;
            this.seq = Number((this.room && this.room.seq) || 0); this.socket = null; this.peerMap = new Map(); this.editingCell = false; this.rereadWaiting = false;
            this.clientId = "calc-" + Date.now().toString(36) + Math.random().toString(36).slice(2, 8);
        },
        mounted() {
            moqui.loadStylesheet("/libs/tabulator/tabulator.min.css");
            moqui.loadStylesheet("/css/DataGrid.css");
            moqui.loadScript("/libs/tabulator/tabulator.min.js", null, () => !!root.Tabulator).then(() => this.build());
            this.connect();
        },
        beforeUnmount() { clearTimeout(this.contentTimer); clearTimeout(this.selectTimer); clearTimeout(this.noticeTimer); this.disconnect(); this.destroy(); },
        methods: {
            chartConfig(chart) { return calc.chartConfig(chart); },
            peerColor(index) { return calc.peerColor(index); },
            peerPlace(peer) {
                const sheet = this.sheets.find((one) => one.index === Number(peer.sheet));
                const area = peer.area || {};
                return calc.rangeName({ row: area.row, column: area.column, lastRow: area.row + Math.max(1, area.height || 1) - 1,
                    lastColumn: area.column + Math.max(1, area.width || 1) - 1 }, sheet ? sheet.name : "");
            },

            // ---- editing together: the room on the notification socket ----
            connect() {
                const client = this.$root && this.$root.notificationClient, room = this.room;
                if (this.socket && room && this.socket.roomId === room.roomId) return;
                this.disconnect();
                if (!client || !client.registerListener || !room || !room.shared) return;
                const roomId = room.roomId, join = () => client.send({ calc: "join", room: roomId, client: this.clientId });
                this.socket = { client, roomId,
                    off: client.registerListener("calc:" + roomId, (frame) => this.frame(frame)),
                    // a socket opened again joins again; while it is closed nobody is shown
                    offState: client.onState ? client.onState((state) => { if (state === "open") join(); else this.peers = []; }) : () => {} };
                join();
            },
            disconnect() {
                const socket = this.socket;
                if (!socket) return;
                this.socket = null; this.peers = []; this.peerMap = new Map();
                socket.client.send({ calc: "leave", room: socket.roomId, client: this.clientId });
                socket.off(); socket.offState();
            },
            frame(frame) {
                if (!frame || !frame.type) return;
                const taken = calc.receive(this.peers, this.seq, frame);
                if (taken.peers !== this.peers) { this.peers = taken.peers; this.markPeers(); }
                if (taken.reopen) { this.$emit("gone"); return; }
                if (taken.reread) {
                    if (frame.name && frame.userId) this.tell(frame.kind === "save" ? frame.name + " saved the document" : frame.kind === "refresh" ? frame.name + " read the data again" : frame.name + " edited");
                    // the cells it changed, when this view held the room's state just before it
                    this.reread(frame.changed && Number(frame.from) === this.seq ? frame.changed : null);
                }
            },
            tell(text) { clearTimeout(this.noticeTimer); this.notice = text; this.noticeTimer = setTimeout(() => { this.notice = ""; }, 4000); },
            // the room's state and cells again, after another's edit; waits for a cell being typed in
            reread(changed) {
                if (this.editingCell) { this.rereadWaiting = true; return Promise.resolve(); }
                if (!this.stateUrl) { if (!calc.forgetRows(this.cache, changed)) this.cache = new Map(); return this.load(); }
                return moqui.request(this.stateUrl, { params: Object.assign({}, this.params, { roomId: this.room.roomId }) }).then((result) => {
                    const json = result.json || {};
                    if (json.errors) { if (String(json.errors).includes("ROOM_GONE")) this.$emit("gone"); return; }
                    if (json.roomId) this.applied(json, changed);
                }, () => null);
            },
            markPeers() {
                this.peerMap = calc.peerCells(this.peers, this.sheetIndex);
                if (!this.table) return;
                try { for (const row of this.table.getRows("visible")) row.reformat(); } catch (_) { /* rebuilt meanwhile */ }
            },
            // the place of this view, for the others, a moment after the selection settles
            sendSelection() {
                if (!this.socket) return;
                clearTimeout(this.selectTimer);
                this.selectTimer = setTimeout(() => {
                    const socket = this.socket;
                    if (!socket) return;
                    const at = this.selection();
                    socket.client.send({ calc: "select", room: socket.roomId, client: this.clientId, sheet: this.sheetIndex,
                        area: { row: at.row, column: at.column, width: at.width, height: at.height } });
                }, 200);
            },
            discard() {
                if (!this.revertUrl) return;
                this.$q.dialog({ title: "Discard the unsaved edits", message: "Every edit since the document was saved, everyone's, is discarded.", cancel: true, persistent: true })
                    .onOk(() => moqui.request(this.revertUrl, { method: "POST", params: Object.assign({}, this.params, { roomId: this.room.roomId }) }).then((result) => {
                        const json = result.json || {};
                        if (json.errors) { moqui.notifyMessages(null, json.errors); return; }
                        if (json.roomId) this.applied(json);
                    }, (err) => moqui.handleAjaxError(err)));
            },
            firstSheet() { const first = this.visibleSheets[0]; return first ? first.index : 0; },
            sheet() { return this.activeSheet; },
            showSheet(index) {
                if (index === this.sheetIndex) return;
                this.sheetIndex = index; this.active = { row: 1, column: 1, lastRow: 1, lastColumn: 1 };
                this.peerMap = calc.peerCells(this.peers, index);
                this.$emit("sheet", index); this.build(); this.readContent(); this.sendSelection();
            },
            destroy() { if (this.table) { try { this.table.destroy(); } catch (_) { /* gone */ } this.table = null; } },
            // the loaded rows of the sheet shown: row number to its cells by column
            rows() {
                let rows = this.cache.get(this.sheetIndex);
                if (!rows) { rows = new Map(); this.cache.set(this.sheetIndex, rows); }
                return rows;
            },
            // the rows and columns the table of a sheet has: its used area, with room to type beyond it where it is edited
            extent(sheet) {
                const spare = this.editable && !sheet.data;
                return { rows: Math.max((Number(sheet.rows) || 0) + (spare ? 60 : 0), calc.MIN_ROWS), columns: Math.max((Number(sheet.columns) || 0) + (spare ? 6 : 0), calc.MIN_COLUMNS) };
            },
            build(topRow) {
                this.destroy();
                const element = this.$refs.grid, sheet = this.sheet();
                if (!element || !root.Tabulator || !sheet) return;
                const vm = this;
                const extent = this.extent(sheet);
                const rowCount = extent.rows, columnCount = extent.columns;
                this.columnCount = columnCount; this.rowCount = rowCount;
                this.built = { frozenRows: Number(sheet.frozenRows || 0), frozenColumns: Number(sheet.frozenColumns || 0), widths: JSON.stringify((sheet.widths || []).slice(0, columnCount)), name: sheet.name };
                const formatter = (cell) => {
                    const row = cell.getRow().getData().id, column = cell.getColumn().getDefinition().calcColumn;
                    const cells = vm.rows().get(row);
                    const found = cells ? cells[column - 1] : null;
                    const element = cell.getElement();
                    calc.applyCss(element, found ? found.css : null);
                    // another's place: an outline in their colour
                    const peer = vm.peerMap.get(row + ":" + column);
                    element.style.boxShadow = peer ? "inset 0 0 0 2px " + peer : "";
                    return found ? found.text : "";
                };
                // a copy takes the texts on screen: the table holds no data of its own
                const accessorClipboard = (value, data, type, params, column) => {
                    const cells = vm.rows().get(data.id), at = column && column.getDefinition().calcColumn;
                    return cells && at && cells[at - 1] ? cells[at - 1].text : "";
                };
                const cellEdits = this.editable && !sheet.data;
                const columns = [{ title: "", field: "id", width: 56, frozen: true, headerSort: false, resizable: false, hozAlign: "right", cssClass: "m-calc-row-header" }];
                for (let c = 1; c <= columnCount; c++) columns.push({ title: calc.columnName(c), field: "c" + c, calcColumn: c, headerSort: false, resizable: true,
                    headerHozAlign: "center", width: Math.max(30, Number((sheet.widths || [])[c - 1]) || 90), frozen: c <= Number(sheet.frozenColumns || 0), formatter, accessorClipboard,
                    editor: cellEdits ? vm.cellEditor : undefined });
                const data = new Array(rowCount);
                for (let r = 0; r < rowCount; r++) data[r] = { id: r + 1 };
                this.table = new root.Tabulator(element, { data, columns, index: "id", height: this.height, layout: "fitData", placeholder: "",
                    frozenRows: Number(sheet.frozenRows || 0), columnDefaults: { tooltip: false }, editTriggerEvent: "dblclick",
                    selectableRange: 1, selectableRangeColumns: true, selectableRangeRows: true, selectableRangeClearCells: false,
                    clipboard: "copy", clipboardCopyStyled: false, clipboardCopyRowRange: "range", clipboardCopyConfig: { rowHeaders: false, columnHeaders: false } });
                this.table.on("tableBuilt", () => {
                    vm.table.on("scrollVertical", () => vm.load());
                    vm.table.on("rangeChanged", (range) => vm.rangeChanged(range));
                    if (vm.editable) vm.table.on("columnResized", (column) => {
                        const at = column.getDefinition().calcColumn;
                        if (at) vm.edit([{ op: "width", sheet: vm.sheetIndex, column: at, width: Math.round(column.getWidth()) }]);
                    });
                    if (topRow && topRow > 1) vm.table.scrollToRow(Math.min(topRow, rowCount), "top", false).then(() => vm.load(), () => vm.load()); else vm.load();
                });
            },
            // the first and last sheet rows in view
            visible() {
                let rows = [];
                // a table taken away while a request ran has no rows to ask for
                try { rows = this.table ? this.table.getRows("visible") : []; } catch (_) { rows = []; }
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
                    // the cache may have been emptied by an edit while the request ran: its rows go into the cache of now
                    const target = this.cache.get(sheetIndex) || rows;
                    if (!this.cache.has(sheetIndex)) this.cache.set(sheetIndex, target);
                    for (const [row, cells] of calc.blockRows(block)) target.set(row, cells);
                    // the sheet may have changed while the request ran: its rows are kept, and nothing of this table is drawn
                    if (this.table !== table || this.sheetIndex !== sheetIndex) return;
                    for (const row of table.getRows("visible")) row.reformat();
                    // the frozen rows are in the table's header, not among the rows in view
                    const sheetNow = this.sheet();
                    for (let r = 1; r <= Number((sheetNow && sheetNow.frozenRows) || 0); r++) { const frozen = table.getRow(r); if (frozen) frozen.reformat(); }
                    const sheet = this.sheet();
                    this.status = sheet ? Number(sheet.rows).toLocaleString() + " rows × " + Number(sheet.columns).toLocaleString() + " columns" : "";
                }, (err) => { moqui.handleAjaxError(err); });
            },

            // ---- the selection and the formula bar ----
            rangeChanged(range) {
                let bounds = null;
                try { bounds = range.getBounds(); } catch (_) { bounds = null; }
                if (!bounds || !bounds.start || !bounds.end) return;
                // the bounds are the table's own cells, not components: a row's data and a column's definition are on them
                const rowOf = (cell) => (cell.getRow ? cell.getRow().getData() : cell.row.data).id;
                const columnOf = (cell) => (cell.getColumn ? cell.getColumn().getDefinition() : cell.column.definition).calcColumn || 1;
                const next = { row: Math.min(rowOf(bounds.start), rowOf(bounds.end)), column: Math.min(columnOf(bounds.start), columnOf(bounds.end)),
                    lastRow: Math.max(rowOf(bounds.start), rowOf(bounds.end)), lastColumn: Math.max(columnOf(bounds.start), columnOf(bounds.end)) };
                const moved = next.row !== this.active.row || next.column !== this.active.column;
                this.active = next;
                this.$emit("selection", Object.assign({ sheet: this.sheetIndex, name: calc.rangeName(next, (this.sheet() || {}).name) }, next));
                if (moved) this.readContent();
                this.sendSelection();
            },
            // what was typed into the active cell, for the bar; asked a moment after the selection settles
            readContent() {
                if (!this.contentUrl) return;
                clearTimeout(this.contentTimer);
                const at = { sheet: this.sheetIndex, row: this.active.row, column: this.active.column };
                this.contentTimer = setTimeout(() => this.content(at).then((text) => {
                    if (at.sheet !== this.sheetIndex || at.row !== this.active.row || at.column !== this.active.column) return;
                    this.barText = text; this.barOriginal = text;
                }), 60);
            },
            content(at) {
                if (!this.contentUrl) return Promise.resolve("");
                return moqui.request(this.contentUrl, { params: Object.assign({}, this.params, { roomId: this.room.roomId }, at) })
                    .then((result) => String((result.json && result.json.content) || ""), () => "");
            },
            barInput(event) {
                this.barText = event.target.value;
                this.suggestions = calc.completions(this.barText, event.target.selectionStart, this.tableNames, this.room.functions);
                this.suggestionIndex = 0;
            },
            barKey(event) {
                if (this.suggestions && (event.key === "ArrowDown" || event.key === "ArrowUp")) {
                    const count = this.suggestions.names.length;
                    this.suggestionIndex = (this.suggestionIndex + (event.key === "ArrowDown" ? 1 : count - 1)) % count;
                    event.preventDefault();
                } else if (this.suggestions && event.key === "Tab") { this.takeSuggestion(this.suggestions.names[this.suggestionIndex]); event.preventDefault(); }
                else if (event.key === "Enter") { event.preventDefault(); this.commitBar(); }
                else if (event.key === "Escape") { this.barText = this.barOriginal; this.suggestions = null; }
            },
            takeSuggestion(name) {
                const input = this.$refs.bar, found = this.suggestions;
                if (!input || !found) return;
                const caret = input.selectionStart;
                this.barText = this.barText.slice(0, found.from) + name + this.barText.slice(caret);
                this.suggestions = null;
                this.$nextTick(() => { input.focus(); const at = found.from + name.length; input.setSelectionRange(at, at); });
            },
            commitBar() {
                this.suggestions = null;
                if (!this.editable || this.barText === this.barOriginal) return;
                // committed: no longer what is being typed, so the cell's content as the engine keeps it comes back
                this.barOriginal = this.barText;
                this.edit([{ op: "input", sheet: this.sheetIndex, row: this.active.row, column: this.active.column, value: this.barText }]);
            },

            // ---- edits ----
            // send edits; the room's new state comes back, with an edit's refusal as an error beside it
            edit(ops) {
                if (!this.editUrl) return Promise.resolve(null);
                const before = this.seq;
                return moqui.request(this.editUrl, { method: "POST", params: Object.assign({}, this.params, { roomId: this.room.roomId, opsJson: JSON.stringify(ops) }) }).then((result) => {
                    const json = result.json || {};
                    if (json.errors) {
                        if (String(json.errors).includes("ROOM_GONE")) { this.$emit("gone"); return null; }
                        moqui.notifyMessages(null, json.errors);
                    }
                    // what changed is this edit's alone when the room did not follow another's edit first
                    if (json.roomId) this.applied(json, !json.shared || Number(json.seq) === before + 1 ? json.changed : null);
                    return json;
                }, (err) => { moqui.handleAjaxError(err); return null; });
            },
            // after an edit: the holder of the room takes its new state, the cells are asked again (the rows `changed`
            // names when the edit changed values only, else every sheet's, since a formula anywhere may have changed), and
            // the table is made again only when its shape changed
            applied(state, changed) {
                const top = (this.visible() || [1])[0];
                if (state.seq != null) this.seq = Math.max(this.seq, Number(state.seq));
                this.$emit("changed", state);
                if (!calc.forgetRows(this.cache, changed)) this.cache = new Map();
                this.$nextTick(() => {
                    const sheet = this.sheet();
                    if (!sheet) { this.sheetIndex = this.firstSheet(); this.build(); return; }
                    const extent = this.extent(sheet), built = this.built || {};
                    const same = this.table && extent.rows <= this.rowCount && extent.columns <= this.columnCount && built.name === sheet.name
                        && built.frozenRows === Number(sheet.frozenRows || 0) && built.frozenColumns === Number(sheet.frozenColumns || 0)
                        && built.widths === JSON.stringify((sheet.widths || []).slice(0, this.columnCount));
                    if (same) this.load(); else this.build(top);
                    // what is being typed in the bar is left as it is
                    const typing = this.$refs.bar && root.document && root.document.activeElement === this.$refs.bar && this.barText !== this.barOriginal;
                    if (!typing) this.readContent();
                });
            },
            selection() { return { sheet: this.sheetIndex, row: this.active.row, column: this.active.column, width: this.active.lastColumn - this.active.column + 1, height: this.active.lastRow - this.active.row + 1 }; },
            style(path, value) { return this.edit([Object.assign({ op: "style", path, value: String(value) }, this.selection())]); },
            // on when the active cell does not have it yet
            toggleStyle(path, cssName) {
                const cells = this.rows().get(this.active.row), cell = cells && cells[this.active.column - 1];
                const has = !!(cell && cell.css && cell.css[cssName] && (cssName !== "text-decoration" || cell.css[cssName].includes("underline")));
                return this.style(path, !has);
            },
            structure(op) {
                const rows = op.endsWith("Rows");
                const edit = { op, sheet: this.sheetIndex };
                if (rows) { edit.row = this.active.row; edit.count = op.startsWith("delete") ? this.active.lastRow - this.active.row + 1 : 1; }
                else { edit.column = this.active.column; edit.count = op.startsWith("delete") ? this.active.lastColumn - this.active.column + 1 : 1; }
                return this.edit([edit]);
            },
            freeze(on) { return this.edit([{ op: "freeze", sheet: this.sheetIndex, rows: on ? this.active.row : 0, columns: on ? this.active.column - 1 : 0 }]); },
            fillDown() {
                if (this.active.lastRow <= this.active.row) return null;
                return this.edit([{ op: "fill", sheet: this.sheetIndex, row: this.active.row, column: this.active.column, width: this.active.lastColumn - this.active.column + 1, height: 1, toRow: this.active.lastRow }]);
            },
            clearSelection() { return this.edit([Object.assign({ op: "clear" }, this.selection())]); },
            // a cell's editor: what was typed into it (asked when it opens), set on Enter, Tab or leaving it
            cellEditor(cell, onRendered, success, cancel) {
                const vm = this, at = { sheet: this.sheetIndex, row: cell.getRow().getData().id, column: cell.getColumn().getDefinition().calcColumn };
                const input = document.createElement("input");
                input.className = "m-calc-cell-editor"; input.spellcheck = false; input.autocomplete = "off";
                const shown = (vm.rows().get(at.row) || [])[at.column - 1];
                let original = shown ? shown.text : "", typed = false, done = false;
                vm.editingCell = true;
                input.value = original;
                vm.content(at).then((text) => { original = text; if (!typed && !done) { input.value = text; input.select(); } });
                onRendered(() => { input.focus(); input.select(); });
                const finish = (commit) => {
                    if (done) return;
                    done = true;
                    vm.editingCell = false;
                    const value = input.value;
                    cancel();
                    const edited = commit && value !== original ? vm.edit([{ op: "input", sheet: at.sheet, row: at.row, column: at.column, value }]) : Promise.resolve();
                    // another's edit that came while typing is read once this one is in
                    if (vm.rereadWaiting) { vm.rereadWaiting = false; edited.then(() => vm.reread()); }
                };
                input.addEventListener("input", () => { typed = true; });
                input.addEventListener("blur", () => finish(true));
                input.addEventListener("keydown", (event) => {
                    if (event.key === "Enter" || event.key === "Tab") { event.preventDefault(); finish(true); }
                    else if (event.key === "Escape") finish(false);
                    event.stopPropagation();
                });
                return input;
            },
            // keys on the grid, not in an editor: Delete clears the selection, F2 goes to the bar, Ctrl+Z and Ctrl+Y undo and redo
            gridKey(event) {
                if (!this.editable || (event.target && /^(INPUT|TEXTAREA)$/.test(event.target.tagName))) return;
                const command = event.ctrlKey || event.metaKey;
                if (event.key === "Delete" || event.key === "Backspace") { event.preventDefault(); this.clearSelection(); }
                else if (event.key === "F2") { event.preventDefault(); if (this.$refs.bar) this.$refs.bar.focus(); }
                else if (command && (event.key === "z" || event.key === "Z")) { event.preventDefault(); this.edit([{ op: event.shiftKey ? "redo" : "undo" }]); }
                else if (command && (event.key === "y" || event.key === "Y")) { event.preventDefault(); this.edit([{ op: "redo" }]); }
            },
            // text pasted on the grid: rows of tab-separated cells, set from the active cell
            gridPaste(event) {
                if (!this.editable || (event.target && /^(INPUT|TEXTAREA)$/.test(event.target.tagName))) return;
                const text = event.clipboardData ? event.clipboardData.getData("text/plain") : "";
                if (!text) return;
                event.preventDefault();
                this.edit([{ op: "paste", sheet: this.sheetIndex, row: this.active.row, column: this.active.column, rows: calc.pasteRows(text) }]);
            },

            // ---- sheets and charts ----
            addSheet() { return this.edit([{ op: "addSheet" }]).then((state) => { if (state && state.sheets && state.sheets.length) this.showSheet(state.sheets[state.sheets.length - 1].index); }); },
            renameSheet(sheet) {
                this.$q.dialog({ title: "Rename the sheet", prompt: { model: sheet.name, type: "text" }, cancel: true })
                    .onOk((name) => { if (name && name !== sheet.name) this.edit([{ op: "renameSheet", sheet: sheet.index, name }]); });
            },
            deleteSheet(sheet) {
                this.$q.dialog({ title: "Delete the sheet", message: "Delete " + sheet.name + " and everything on it?", cancel: true, persistent: true })
                    .onOk(() => this.edit([{ op: "deleteSheet", sheet: sheet.index }]).then(() => { if (!this.sheet()) { this.sheetIndex = this.firstSheet(); this.build(); } }));
            },
            // a chart of the selection: its first column the labels and each other column a series, when it has more than one
            openChart() {
                const sheetName = (this.sheet() || {}).name, range = this.active;
                const column = (from, to) => calc.rangeName({ row: range.row, column: from, lastRow: range.lastRow, lastColumn: to }, sheetName);
                const several = range.lastColumn > range.column;
                const values = [];
                for (let c = several ? range.column + 1 : range.column; c <= range.lastColumn; c++) values.push(column(c, c));
                this.chart = { title: "", type: "column", categories: several ? column(range.column, range.column) : "", values: range.lastRow > range.row ? values.join(", ") : "" };
                this.chartOpen = true;
            },
            chartDefinitions() { return this.charts.map((chart) => ({ title: chart.title, type: chart.type, categories: chart.categories, series: (chart.series || []).map((one) => ({ name: one.name, values: one.values })) })); },
            addChart() {
                const series = this.chart.values.split(",").map((text) => text.trim()).filter(Boolean).map((values, index) => ({ name: "Series " + (index + 1), values }));
                const charts = this.chartDefinitions().concat([{ title: this.chart.title, type: this.chart.type, categories: this.chart.categories.trim(), series }]);
                this.chartOpen = false;
                return this.edit([{ op: "charts", charts }]);
            },
            removeChart(index) { return this.edit([{ op: "charts", charts: this.chartDefinitions().filter((chart, at) => at !== index) }]); }
        }
    };
})(typeof window !== "undefined" ? window : globalThis);
