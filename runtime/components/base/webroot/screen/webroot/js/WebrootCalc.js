/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The sheet view of a CalcGrid document (plans/archive/IronCalc-DB.md, Thin client) as moqui.calc, loaded by the
 * screens that show one (component://tools/screen/Tools/Data/Grid.qvue) and tested in the page by
 * test/client/WebrootCalc.spec.js.
 * The workbook and the spreadsheet engine are in the server; this draws the cells on screen.
 *   - moqui.calc.sheet: a Vue component. Props: room (what open#CalcDocument answered: roomId, sheets, charts, ...) and
 *     cellsUrl (the transition that answers a rectangle of cells: roomId, sheet, row, column, width, height, and what
 *     params adds: an access profile). It emits
 *     gone when the server no longer has the room, and sheet (the index) when another sheet is chosen.
 *     A sheet is a Tabulator table (libs/tabulator) with a placeholder row for each sheet row and no data: a formatter
 *     draws each cell from the blocks the server sent, and a scroll asks for the rows in view and a margin, so a jump
 *     to a far row loads nothing between. Sheet tabs are below it and the document's charts (m-chart) below those.
 *     With editUrl (the transition that applies edits: roomId, opsJson) and contentUrl (a cell's content as typed) the
 *     sheet is edited: a toolbar (undo, redo, font, colours, borders, alignment, number format, rows and columns, a
 *     chart), a conditional formats dialog (cell values, texts, top and bottom, colour scales and data bars; a workbook's
 *     icons are drawn too), a formula bar that completes function and table names and takes a reference clicked or
 *     dragged on the grid (on any sheet; a data sheet's as its table's columns), a cell's editor on a double click,
 *     Delete, a paste of rows of
 *     cells, a column's width and a row's height, the fill handle dragged down, up or across (the engine's autofill),
 *     cells merged and unmerged (a merged cell drawn as one box, each row its own slice of it), and sheets added,
 *     renamed and deleted. Each edit is sent to the server, which answers the
 *     room's new state (emitted as changed, for the holder of room to take); the cells are then asked again. A data
 *     sheet is not edited. It also emits selection (sheet, row, column, lastRow, lastColumn and the range's name).
 *     A saved document's room (room.shared) is edited together: the sheet joins it on the notification socket
 *     ($root.notificationClient, frames with calc: join, select, leave; docs/ui/Qvt-Client.md, CalcGrid rooms), shows
 *     the others' places (a coloured outline and their names), sends its own, and on another's edit reads the room's
 *     state again (stateUrl) and its cells; with revertUrl an author discards everyone's unsaved edits.
 *   - the pure helpers: columnName(n), cellCss(style, fill, cellType) and applyCss(element, css), blockRows(block),
 *     neededRange(have, first, last, rows, margin), chartConfig(chart), completions(text, caret, tables, functions),
 *     rangeName(range, sheetName), pasteRows(text), tableName(sheetName), peerColor(index), receive(peers, seq, frame)
 *     and peerCells(peers, sheet), fillOp(sheet, source, target, rowAt, columnAt), canPoint(text, caret),
 *     referenceText(range, target, from), putReference(text, from, to, reference); and takeFillHandle(table, onFill),
 *     holdFocus(table, holding), rowHeight(row) and setRowHeight(row, height), which take Tabulator 6.7's fill handle,
 *     focus and rows (its internals, pinned by the spec).
 */
(function (root) {
    "use strict";

    const moqui = root.moqui || (root.moqui = {});
    const L = (text) => (moqui.l10n && moqui.l10n.localize ? moqui.l10n.localize(text) : text);
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

    /** What a cell's conditional format draws beside its text (from its extended style, plans/IronCalc-DB2.md CG2.5): a
     * data bar, an icon, a rating; and whether its value is hidden. Null when none. */
    calc.decoration = (extended) => {
        if (!extended) return null;
        const out = {}, share = (value) => Math.min(1, Math.max(0, Number(value) || 0));
        const bar = extended.data_bar;
        if (bar && typeof bar.value === "number") {
            const axis = share(bar.axis_position), value = share(bar.value), positive = value >= axis;
            out.bar = { from: Math.min(axis, value), to: Math.max(axis, value), gradient: !!bar.is_gradient,
                color: color(positive ? bar.positive_color : bar.negative_color) || (positive ? "#638EC6" : "#FF0000") };
            if (bar.show_value === false) out.hide = true;
        }
        if (extended.icon) {
            out.icon = { glyph: calc.ICONS[extended.icon.icon] || calc.ICONS.Circle, color: color(extended.icon.color) };
            if (extended.icon.show_value === false) out.hide = true;
        }
        const rating = extended.rating;
        if (rating && Number(rating.max) > 0) {
            // a rating is drawn as its icons, each cell: held to the ten a rule has at most
            const max = Math.min(calc.MAX_RATING, Math.floor(Number(rating.max)));
            out.rating = { glyph: calc.ICONS[rating.icon] || calc.ICONS.Star, count: Math.max(0, Math.min(Math.floor(Number(rating.count) || 0), max)), max,
                color: color(rating.color) };
            if (rating.show_value === false) out.hide = true;
        }
        return Object.keys(out).length ? out : null;
    };
    /** The most icons of a rating (the server holds a rule to nine thresholds). */
    calc.MAX_RATING = 10;
    /** The characters a conditional format's icons are drawn with, by the engine's names. */
    calc.ICONS = { ArrowUp: "\u2B06", ArrowRight: "\u27A1", ArrowDown: "\u2B07", ArrowAngleUp: "\u2B08", ArrowAngleDown: "\u2B0A", Circle: "\u25CF",
        TriangleUp: "\u25B2", TriangleDown: "\u25BC", TriangleUpFilled: "\u25B2", TriangleDownFilled: "\u25BC", FlatRectangle: "\u25AC", Rhombus: "\u25C6",
        Flag: "\u2691", Check: "\u2714", Cross: "\u2716", Exclamation: "!", Star: "\u2605", Heart: "\u2665", ThumbsUp: "\uD83D\uDC4D", ThumbsDown: "\uD83D\uDC4E" };
    /**
     * A cell's content as the table draws it: its text as a text node (never as HTML: a cell holds what anyone typed or
     * any row of the data), with its decorations (calc.decoration) when it has some.
     */
    calc.cellNode = (found) => {
        const doc = root.document;
        if (!found) return doc.createTextNode("");
        const decor = found.decor;
        if (!decor) return doc.createTextNode(found.text);
        const box = doc.createElement("span");
        box.className = "m-calc-decor";
        const part = (className, text, shade) => {
            const element = doc.createElement("span");
            element.className = className;
            if (text) element.textContent = text;
            if (shade) element.style.color = shade;
            box.appendChild(element);
            return element;
        };
        if (decor.bar) {
            const bar = part("m-calc-databar");
            bar.style.left = (decor.bar.from * 100).toFixed(2) + "%";
            bar.style.width = ((decor.bar.to - decor.bar.from) * 100).toFixed(2) + "%";
            bar.style.background = decor.bar.gradient ? "linear-gradient(to right, " + decor.bar.color + ", transparent)" : decor.bar.color;
        }
        if (decor.icon) part("m-calc-icon", decor.icon.glyph, decor.icon.color);
        if (decor.rating) {
            part("m-calc-icon", decor.rating.glyph.repeat(decor.rating.count), decor.rating.color);
            if (decor.rating.max > decor.rating.count) part("m-calc-icon m-calc-icon-off", decor.rating.glyph.repeat(decor.rating.max - decor.rating.count));
        }
        if (!decor.hide) part("m-calc-value", found.text);
        return box;
    };

    /** The merged cell (`row`, `column`, `width`, `height`, as the layout and a block give them) that holds a cell, or null. */
    calc.boxAt = (boxes, row, column) => (boxes || []).find((box) => row >= box.row && row < box.row + box.height && column >= box.column && column < box.column + box.width) || null;
    /** A range (row, column, lastRow, lastColumn) grown to the merged cells it meets, as a selection of one is the whole box. */
    calc.expandToMerges = (range, boxes) => {
        const out = Object.assign({}, range);
        for (let grown = true; grown;) {
            grown = false;
            for (const box of boxes || []) {
                const lastRow = box.row + box.height - 1, lastColumn = box.column + box.width - 1;
                if (box.row > out.lastRow || lastRow < out.row || box.column > out.lastColumn || lastColumn < out.column) continue;
                if (box.row < out.row) { out.row = box.row; grown = true; }
                if (box.column < out.column) { out.column = box.column; grown = true; }
                if (lastRow > out.lastRow) { out.lastRow = lastRow; grown = true; }
                if (lastColumn > out.lastColumn) { out.lastColumn = lastColumn; grown = true; }
            }
        }
        return out;
    };
    /**
     * What a cell of a merged cell draws (plans/IronCalc-DB2.md CG2.6): its slice of the box, the anchor's content (a
     * cell of the cache, calc.blockRows) laid over the whole box and shifted by the cell's place in it (`x`, `y`, pixels),
     * the box `width` and `height` wide and high. The slice reaches a pixel into the cell's right border and the row's
     * lower one where the box goes on (`right`, `below`), so no grid line crosses it. Each row draws its own, as the
     * table draws the rows as they come into view.
     */
    calc.sliceNode = (anchor, place) => {
        const doc = root.document, css = (anchor && anchor.css) || {};
        const slice = doc.createElement("div");
        slice.className = "m-calc-slice";
        slice.style.right = place.right ? "-1px" : "0";
        slice.style.bottom = place.below ? "-1px" : "0";
        if (css["background-color"]) slice.style.backgroundColor = css["background-color"];
        const content = doc.createElement("div");
        content.className = "m-calc-box";
        Object.assign(content.style, { left: -place.x + "px", top: -place.y + "px", width: Math.max(0, place.width) + "px", height: Math.max(0, place.height) + "px" });
        for (const name of ["text-align", "color", "font-weight", "font-style", "text-decoration", "white-space"]) if (css[name]) content.style.setProperty(name, css[name]);
        content.appendChild(calc.cellNode(anchor));
        slice.appendChild(content);
        return slice;
    };

    /** The kinds of rule the conditional formats dialog makes (calc.ruleOf), and a cell value rule's comparisons. */
    calc.RULE_KINDS = [{ label: "The value is", value: "cellIs" }, { label: "The text contains", value: "text" }, { label: "In the top", value: "top" },
        { label: "In the bottom", value: "bottom" }, { label: "A colour scale", value: "scale" }, { label: "A data bar", value: "bar" }];
    calc.RULE_OPERATORS = [{ label: "greater than", value: "GreaterThan", sign: ">" }, { label: "at least", value: "GreaterThanOrEqual", sign: ">=" },
        { label: "less than", value: "LessThan", sign: "<" }, { label: "at most", value: "LessThanOrEqual", sign: "<=" }, { label: "equal to", value: "Equal", sign: "=" },
        { label: "not equal to", value: "NotEqual", sign: "<>" }, { label: "between", value: "Between", sign: "between" }, { label: "not between", value: "NotBetween", sign: "not between" }];
    /** A new rule's form: the cells it covers, what it tests, and the format it gives. */
    calc.ruleForm = (range) => ({ range: range || "", kind: "cellIs", operator: "GreaterThan", value: "", value2: "", rank: 10, percent: false,
        fill: "#FFCDD2", font: "", bold: false, low: "#F8696B", high: "#63BE7B", bar: "#638EC6" });
    /** The engine's rule (the `rule` of an addRule or changeRule edit) of a dialog's form, or null when it is not one yet. */
    calc.ruleOf = (form) => {
        const format = () => {
            const made = {}, font = {};
            if (color(form.fill)) made.fill = { color: form.fill };
            if (color(form.font)) font.color = form.font;
            if (form.bold) font.b = true;
            if (Object.keys(font).length) made.font = font;
            return made;
        };
        const number = (value) => String(value == null ? "" : value).trim().replace(/^=/, "");
        switch (form.kind) {
            case "cellIs": {
                const between = form.operator === "Between" || form.operator === "NotBetween";
                if (!number(form.value) || (between && !number(form.value2))) return null;
                return { type: "CellIs", operator: form.operator, formula: number(form.value), formula2: between ? number(form.value2) : null, format: format(), stop_if_true: false };
            }
            case "text": return String(form.value || "") ? { type: "Text", operator: "Contains", value: String(form.value), format: format(), stop_if_true: false } : null;
            case "top": case "bottom":
                return { type: form.kind === "top" ? "Top10" : "Bottom10", rank: Math.max(1, Math.round(Number(form.rank) || 10)), percent: !!form.percent, format: format(), stop_if_true: false };
            case "scale": return { type: "ColorScale", thresholds: [{ cfvo: "Min", color: color(form.low) || "#F8696B" }, { cfvo: "Max", color: color(form.high) || "#63BE7B" }] };
            case "bar": return { type: "DataBar", min: null, max: null, positive_color: color(form.bar) || "#638EC6", negative_color: "#FF0000", is_gradient: false, show_value: true };
            default: return null;
        }
    };
    /** The form of a rule of the layout (range and engine rule), to change it; null for a kind the dialog does not make. */
    calc.formOf = (range, rule) => {
        const form = calc.ruleForm(range), format = rule.format || {};
        Object.assign(form, { fill: color(format.fill && format.fill.color), font: color(format.font && format.font.color), bold: !!(format.font && format.font.b) });
        if (rule.type === "CellIs") return Object.assign(form, { kind: "cellIs", operator: rule.operator, value: rule.formula || "", value2: rule.formula2 || "" });
        if (rule.type === "Text" && rule.operator === "Contains") return Object.assign(form, { kind: "text", value: rule.value || "" });
        if (rule.type === "Top10" || rule.type === "Bottom10") return Object.assign(form, { kind: rule.type === "Top10" ? "top" : "bottom", rank: rule.rank, percent: !!rule.percent });
        const thresholds = rule.thresholds || [];
        if (rule.type === "ColorScale" && thresholds.length === 2) return Object.assign(form, { kind: "scale", low: color(thresholds[0].color) || form.low, high: color(thresholds[1].color) || form.high });
        if (rule.type === "DataBar") return Object.assign(form, { kind: "bar", bar: color(rule.positive_color) || form.bar });
        return null;
    };
    /** What the sheet says when an undo passed by what another changed or deleted since (a conditional format, a sheet). */
    calc.passedText = (count) => count + " " + (count === 1 ? "change was" : "changes were") + " changed by others since and not undone";
    /** What an edit of a rule of the layout names it by, beside its place: its range and rule as the layout gave them. */
    calc.ruleExpect = (one) => ({ range: one.range, rule: one.rule });
    /** A rule as the dialog lists it. */
    calc.ruleLabel = (rule) => {
        const operator = calc.RULE_OPERATORS.find((one) => one.value === rule.operator);
        switch (rule.type) {
            case "CellIs": return "Value " + (operator ? operator.sign : rule.operator) + " " + rule.formula + (rule.formula2 ? " and " + rule.formula2 : "");
            case "Text": return "Text " + String(rule.operator || "").replace(/([a-z])([A-Z])/g, "$1 $2").toLowerCase() + " \u201C" + rule.value + "\u201D";
            case "Top10": case "Bottom10": return (rule.type === "Top10" ? "Top " : "Bottom ") + rule.rank + (rule.percent ? "%" : "");
            case "ColorScale": return "Colour scale";
            case "DataBar": return "Data bar";
            case "IconSet": case "IconRating": return "Icons";
            default: return String(rule.type || "Rule").replace(/([a-z])([A-Z])/g, "$1 $2");
        }
    };

    /**
     * A block as rows: a Map of row number to an array by column (from the block's first column) of {text, css} (and
     * decor, calc.decoration, when it has one), a cell with none left out.
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
                const extended = (block.styles || [])[(block.style_indices || [])[r * width + c]];
                const css = calc.cellCss(extended, fills.get(key), text ? text.cell_type : 0), decor = calc.decoration(extended);
                if (text || css || decor) cells[c] = decor ? { text: text ? text.text : "", css, decor } : { text: text ? text.text : "", css };
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
    /** The borders of the toolbar's menu: where (the `border` edit's kind), its icon and its name. */
    calc.BORDER_KINDS = [{ kind: "all", icon: "border_all", label: "All borders" }, { kind: "outer", icon: "border_outer", label: "Outside borders" },
        { kind: "inner", icon: "border_inner", label: "Inside borders" }, { kind: "top", icon: "border_top", label: "Top border" },
        { kind: "bottom", icon: "border_bottom", label: "Bottom border" }, { kind: "left", icon: "border_left", label: "Left border" },
        { kind: "right", icon: "border_right", label: "Right border" }, { kind: "none", icon: "border_clear", label: "No borders" }];
    calc.BORDER_STYLES = [{ label: "Thin", value: "thin" }, { label: "Medium", value: "medium" }, { label: "Thick", value: "thick" },
        { label: "Dashed", value: "mediumdashed" }, { label: "Dotted", value: "dotted" }, { label: "Double", value: "double" }];
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
    /**
     * Whether a click on the grid while `text` is in the formula bar, its caret at `caret`, puts a reference into it
     * (plans/IronCalc-DB2.md CG2.2): the text is a formula and the caret follows its =, an operator, an opening
     * parenthesis, a comma or a colon, with spaces between.
     */
    calc.canPoint = (text, caret) => typeof text === "string" && text[0] === "=" && /[=(,:;+\-*/^&<>]\s*$/.test(text.slice(0, caret));
    /**
     * A reference to `range` (row, column, lastRow, lastColumn) of the sheet `target` (of the layout: index, name, data,
     * headers) as a formula on the sheet `from` names it. A data sheet's columns are named as its table's, `Orders[amount]`
     * or `Orders[[region]:[amount]]`, whatever rows are chosen: the edit log replays a formula as typed in the rooms of
     * other scopes, whose data has other row counts, where cells of it would be other rows. Another sheet's cells are
     * named with its name. Null for a data sheet's column with no name.
     */
    calc.referenceText = (range, target, from) => {
        if (target && target.data) {
            const headers = target.headers || [], first = headers[range.column - 1], last = headers[range.lastColumn - 1];
            if (!first) return null;
            const table = calc.tableName(target.name), column = calc.columnReference;
            return range.lastColumn > range.column && last ? table + "[[" + column(first) + "]:[" + column(last) + "]]" : table + "[" + column(first) + "]";
        }
        return calc.rangeName(range, target && target.index !== from ? target.name : "");
    };
    /** A table column's name as a structured reference writes it between its brackets: [, ], #, @ and ' each after an
     * apostrophe, as the engine's lexer reads them back (a column of a SQL source may be named with any of them). */
    calc.columnReference = (name) => String(name).replace(/[[\]#@']/g, "'$&");
    /** `text` with `reference` in place of its characters `from` to `to`: the text, and the caret after the reference. */
    calc.putReference = (text, from, to, reference) => ({ text: text.slice(0, from) + reference + text.slice(to), caret: from + reference.length });
    /**
     * Keep a click on a table's cell from taking the focus while `holding()` (Tabulator 6.7's selectRange.initializeFocus,
     * which a cell's click calls to focus the table): the formula bar keeps it while a reference is pointed at. False
     * when the table has no range selection of that shape.
     */
    calc.holdFocus = (table, holding) => {
        const ranges = table && table.modules && table.modules.selectRange;
        if (!ranges || typeof ranges.initializeFocus !== "function") return false;
        const own = ranges.initializeFocus.bind(ranges);
        ranges.initializeFocus = (cell) => (holding() ? null : own(cell));
        return true;
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
    /** The height a row was given (by a drag of its edge, or calc.setRowHeight), as the engine keeps it: the cells', not
     * the row's outer height with its border. */
    calc.rowHeight = (row) => Math.round((row && row._row && row._row.height) || (row && row.getElement().offsetHeight) || 0);
    /** Give a table's row (a RowComponent) a height of its own, in pixels, through Tabulator 6.7's row (setHeight, as
     * its row resize does); none leaves it as Tabulator measured it. */
    calc.setRowHeight = (row, height) => {
        const inner = row && row._row;
        if (height > 0 && inner && typeof inner.setHeight === "function") inner.setHeight(height, true);
    };

    /**
     * The `fill` edit of a drag of the fill handle (plans/IronCalc-DB2.md CG2.1): `source` the range dragged from and
     * `target` the range it was stretched to along one axis, both as Tabulator's range rects (top, bottom, left, right:
     * positions among the table's rows and columns, from 0); `rowAt` and `columnAt` give a position's sheet row and
     * column. Null when the range did not grow.
     */
    calc.fillOp = (sheet, source, target, rowAt, columnAt) => {
        const top = rowAt(source.top), bottom = rowAt(source.bottom), left = columnAt(source.left), right = columnAt(source.right);
        if (!top || !bottom || !left || !right) return null;
        const op = { op: "fill", sheet, row: top, column: left, width: right - left + 1, height: bottom - top + 1 };
        if (target.top < source.top) op.toRow = rowAt(target.top);
        else if (target.bottom > source.bottom) op.toRow = rowAt(target.bottom);
        else if (target.left < source.left) op.toColumn = columnAt(target.left);
        else if (target.right > source.right) op.toColumn = columnAt(target.right);
        else return null;
        return op.toRow || op.toColumn ? op : null;
    };
    /** Take a table's fill handle away (when calc.takeFillHandle could not take it over). */
    calc.dropFillHandle = (table) => {
        const ranges = table && table.modules && table.modules.selectRange, handle = ranges && ranges.fillHandle;
        if (!handle) return;
        try { handle.attach = () => null; if (handle.element) handle.element.remove(); } catch (_) { /* not of that shape */ }
        ranges.fillHandle = null;
    };
    /**
     * Take the drop of a table's fill handle (Tabulator 6.7's `selectRange.fillHandle`, with `selectableRangeFill`):
     * in place of setting the cells' data in the browser, `onFill(source, target)` is given the range dragged from and
     * the range it reached (the rects of calc.fillOp), and the range reached is selected. The handle's own mousedown and
     * moves draw the preview as before. False when the table has no fill handle of that shape (another Tabulator).
     */
    calc.takeFillHandle = (table, onFill) => {
        const ranges = table && table.modules && table.modules.selectRange, handle = ranges && ranges.fillHandle;
        const extend = handle && handle.constructor && handle.constructor.extendAlongAxis;
        if (!handle || typeof handle.handleMouseUp !== "function" || typeof extend !== "function") return false;
        // bound once: the handle adds and removes this same function as the document's mouseup listener
        handle.handleMouseUp = function handleMouseUp() {
            this.isActive = false;
            this.unsubscribe("cell-mousemove", this.handleCellMouseMove);
            document.removeEventListener("mouseup", this.handleMouseUp);
            const preview = this.preview, source = this.source;
            this.preview = null;
            if (!preview || !source) return;
            const target = extend(source, this.pointerRow, this.pointerCol);
            try {
                this.rangeManager.clearRanges();
                this.rangeManager.addRange(preview.getStartCell(), preview.getEndCell());
            } catch (_) { /* the table changed meanwhile */ }
            preview.destroy();
            onFill(source, target);
        }.bind(handle);
        return true;
    };
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
            + '<q-btn flat dense round size="sm" icon="border_all" aria-label="Borders"><q-tooltip>Borders</q-tooltip><q-menu><div class="q-pa-xs" style="width:200px">'
            + '<div class="row"><q-btn v-for="one in borderKinds" :key="one.kind" flat dense round size="sm" :icon="one.icon" :aria-label="one.label" v-close-popup @click="border(one.kind)">'
            + '<q-tooltip>{{one.label}}</q-tooltip></q-btn></div>'
            + '<q-select dense outlined options-dense emit-value map-options class="q-mt-xs" v-model="borderStyle" :options="borderStyles" label="Line"></q-select>'
            + '<div class="row q-mt-xs"><div v-for="shade in colors" :key="shade" class="m-calc-swatch" :class="{\'m-calc-swatch-on\': shade === borderColor}" :style="{backgroundColor: shade}"'
            + ' :aria-label="\'Line colour \' + shade" @click="borderColor = shade"></div></div></div></q-menu></q-btn>'
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
            + '<q-separator></q-separator>'
            + '<q-item clickable v-close-popup @click="merge(true)"><q-item-section>Merge the cells selected</q-item-section></q-item>'
            + '<q-item clickable v-close-popup @click="merge(false)"><q-item-section>Unmerge the cells selected</q-item-section></q-item>'
            + '</q-list></q-btn-dropdown>'
            + '<q-btn flat dense round size="sm" icon="rule" aria-label="Conditional formats" @click="openRules"><q-tooltip>Conditional formats</q-tooltip></q-btn>'
            + '<q-btn flat dense no-caps size="sm" icon="insert_chart" label="Chart" @click="openChart"><q-tooltip>A chart of the cells selected</q-tooltip></q-btn>'
            + '</div>'
            + '<div v-if="editable || contentUrl" class="row items-center no-wrap q-mb-xs m-calc-bar">'
            + '<div class="m-calc-name text-caption q-px-sm">{{activeName}}</div>'
            + '<div class="col" style="position:relative">'
            + '<input ref="bar" class="m-calc-formula" :readonly="!editable || (activeIsData && !pointingOn)" :value="barText" aria-label="Cell content" spellcheck="false" autocomplete="off"'
            + ' @input="barInput" @keydown="barKey" @blur="suggestions = null">'
            + '<div v-if="suggestions" class="m-calc-suggest shadow-2"><div v-for="(name, index) in suggestions.names" :key="name" class="m-calc-suggest-item"'
            + ' :class="{\'m-calc-suggest-on\': index === suggestionIndex}" @mousedown.prevent="takeSuggestion(name)">{{name}}</div></div>'
            + '</div></div>'
            + '<div ref="grid" class="m-data-grid m-calc-grid" :class="{\'m-calc-pointing\': pointingOn}" tabindex="0" @keydown="gridKey" @paste="gridPaste" @mousedown.capture="pointDown"></div>'
            + '<div class="row items-center no-wrap q-mt-xs">'
            + '<q-tabs dense no-caps inline-label align="left" class="col" :model-value="sheetIndex" @update:model-value="showSheet" @mousedown.capture="tabDown">'
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
            + '<q-dialog v-model="rulesOpen"><q-card class="m-calc-rules" style="min-width:420px;max-width:92vw">'
            + '<q-card-section><div class="text-h6">Conditional formats</div><div class="text-caption" :class="mutedClass">{{(activeSheet || {}).name}}: the rule listed first wins where rules meet</div></q-card-section>'
            + '<q-card-section v-if="activeIsData" class="q-pt-none">A data sheet\'s rows are each reader\'s own: conditional formats go on a sheet beside it, over formulas that read it.</q-card-section>'
            + '<template v-else>'
            + '<q-list dense separator class="q-mx-md">'
            + '<q-item v-for="(one, at) in sheetRules" :key="one.index"><q-item-section><q-item-label>{{ruleLabel(one.rule)}}</q-item-label><q-item-label caption>{{one.range}}</q-item-label></q-item-section>'
            + '<q-item-section side><div class="row no-wrap">'
            + '<q-btn flat dense round size="sm" icon="arrow_upward" :disable="at === 0" :aria-label="\'Move up rule \' + (at + 1)" @click="ruleEdit({op: \'moveRule\', index: one.index, up: true}, one)"></q-btn>'
            + '<q-btn flat dense round size="sm" icon="arrow_downward" :disable="at === sheetRules.length - 1" :aria-label="\'Move down rule \' + (at + 1)" @click="ruleEdit({op: \'moveRule\', index: one.index, up: false}, one)"></q-btn>'
            + '<q-btn flat dense round size="sm" icon="edit" :disable="!formOf(one)" :aria-label="\'Change rule \' + (at + 1)" @click="changeRule(one)"></q-btn>'
            + '<q-btn flat dense round size="sm" icon="delete" :aria-label="\'Delete rule \' + (at + 1)" @click="ruleEdit({op: \'deleteRule\', index: one.index}, one)"></q-btn>'
            + '</div></q-item-section></q-item>'
            + '<q-item v-if="!sheetRules.length"><q-item-section :class="mutedClass">No conditional formats on this sheet</q-item-section></q-item>'
            + '</q-list>'
            + '<q-card-section class="q-gutter-sm">'
            + '<div class="text-subtitle2">{{ruleIndex == null ? "A rule for the cells" : "Change the rule"}}</div>'
            + '<q-input dense outlined v-model="rule.range" label="Cells" hint="B2:B20, or several with spaces between"></q-input>'
            + '<q-select dense outlined options-dense emit-value map-options v-model="rule.kind" :options="ruleKinds" label="When"></q-select>'
            + '<div v-if="rule.kind === \'cellIs\'" class="row q-col-gutter-sm"><q-select class="col-5" dense outlined options-dense emit-value map-options v-model="rule.operator" :options="ruleOperators" label="It is"></q-select>'
            + '<q-input class="col" dense outlined v-model="rule.value" label="Number"></q-input>'
            + '<q-input v-if="rule.operator === \'Between\' || rule.operator === \'NotBetween\'" class="col" dense outlined v-model="rule.value2" label="and"></q-input></div>'
            + '<q-input v-if="rule.kind === \'text\'" dense outlined v-model="rule.value" label="Text"></q-input>'
            + '<div v-if="rule.kind === \'top\' || rule.kind === \'bottom\'" class="row items-center q-col-gutter-sm"><q-input class="col-4" dense outlined type="number" min="1" v-model.number="rule.rank" label="How many"></q-input>'
            + '<q-toggle v-model="rule.percent" label="percent"></q-toggle></div>'
            + '<template v-if="rule.kind === \'scale\'"><div class="text-caption">Lowest</div><div class="row"><div v-for="shade in colors" :key="\'l\' + shade" class="m-calc-swatch" :class="{\'m-calc-swatch-on\': shade === rule.low}" :style="{backgroundColor: shade}" @click="rule.low = shade"></div></div>'
            + '<div class="text-caption">Highest</div><div class="row"><div v-for="shade in colors" :key="\'h\' + shade" class="m-calc-swatch" :class="{\'m-calc-swatch-on\': shade === rule.high}" :style="{backgroundColor: shade}" @click="rule.high = shade"></div></div></template>'
            + '<template v-if="rule.kind === \'bar\'"><div class="text-caption">Bar</div><div class="row"><div v-for="shade in colors" :key="\'b\' + shade" class="m-calc-swatch" :class="{\'m-calc-swatch-on\': shade === rule.bar}" :style="{backgroundColor: shade}" @click="rule.bar = shade"></div></div></template>'
            + '<template v-if="ruleFormats"><div class="text-caption">Fill</div><div class="row"><div v-for="shade in colors" :key="\'f\' + shade" class="m-calc-swatch" :class="{\'m-calc-swatch-on\': shade === rule.fill}" :style="{backgroundColor: shade}" @click="rule.fill = rule.fill === shade ? \'\' : shade"></div></div>'
            + '<div class="text-caption">Text</div><div class="row items-center"><div v-for="shade in colors" :key="\'t\' + shade" class="m-calc-swatch" :class="{\'m-calc-swatch-on\': shade === rule.font}" :style="{backgroundColor: shade}" @click="rule.font = rule.font === shade ? \'\' : shade"></div>'
            + '<q-toggle v-model="rule.bold" label="Bold"></q-toggle></div></template>'
            + '</q-card-section></template>'
            + '<q-card-actions align="right"><q-btn v-if="ruleIndex != null" flat no-caps label="New rule" @click="newRule"></q-btn><q-btn flat no-caps label="Close" v-close-popup></q-btn>'
            + '<q-btn v-if="!activeIsData" unelevated no-caps color="primary" :label="ruleIndex == null ? \'Add\' : \'Save\'" :disable="!ruleReady" @click="saveRule"></q-btn></q-card-actions>'
            + '</q-card></q-dialog>'
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
                chartOpen: false, chart: { title: "", type: "column", categories: "", values: "" }, peers: [], borderStyle: "thin", borderColor: "#000000",
                pointingOn: false, rulesOpen: false, rule: calc.ruleForm(""), ruleIndex: null };
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
            borderKinds() { return calc.BORDER_KINDS; },
            ruleKinds() { return calc.RULE_KINDS; },
            ruleOperators() { return calc.RULE_OPERATORS; },
            // the rules of the sheet shown, the one applied last (which wins) first, as the layout gives them
            sheetRules() { return (this.activeSheet && this.activeSheet.rules) || []; },
            ruleFormats() { return ["cellIs", "text", "top", "bottom"].includes(this.rule.kind); },
            ruleReady() { return !!(String(this.rule.range || "").trim() && calc.ruleOf(this.rule)); },
            borderStyles() { return calc.BORDER_STYLES; },
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
            // a reference being pointed at into the formula bar: the cell typed into (sheet, row, column), and where in the
            // bar's text the reference goes (from, to); armed once the grid of the sheet shown is pressed
            this.pointing = null; this.reselect = null; this.ruleShown = null; this.boxes = []; this.boxKeys = new Set(); this.widthsBuilt = []; this.rowDrawn = 0;
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
                this.$q.dialog({ title: L("Discard the unsaved edits"), message: L("Every edit since the document was saved, everyone's, is discarded."), cancel: true, persistent: true })
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
                this.sheetIndex = index;
                // pointing at another sheet: the cell typed into and the bar's text stay as they are
                if (this.pointing) { this.pointing.armed = false; this.peerMap = calc.peerCells(this.peers, index); this.$emit("sheet", index); this.build(); return; }
                this.active = { row: 1, column: 1, lastRow: 1, lastColumn: 1 };
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
                this.built = { frozenRows: Number(sheet.frozenRows || 0), frozenColumns: Number(sheet.frozenColumns || 0), widths: JSON.stringify((sheet.widths || []).slice(0, columnCount)), name: sheet.name,
                    heights: JSON.stringify(sheet.heights || []), merges: JSON.stringify(sheet.merges || []) };
                // the rows with a height of their own (fill::layout), in pixels
                this.heights = new Map((sheet.heights || []).map(([row, height]) => [Number(row), Number(height)]));
                // the merged cells: the layout's, and those of the blocks loaded (a sheet with more than the layout gives)
                this.boxes = (sheet.merges || []).slice(); this.boxKeys = new Set(this.boxes.map((box) => box.row + ":" + box.column));
                this.widthsBuilt = [];
                for (let c = 1; c <= columnCount; c++) this.widthsBuilt.push(Math.max(30, Number((sheet.widths || [])[c - 1]) || 90));
                const formatter = (cell) => {
                    const row = cell.getRow().getData().id, column = cell.getColumn().getDefinition().calcColumn;
                    const cells = vm.rows().get(row);
                    const found = cells ? cells[column - 1] : null;
                    const element = cell.getElement();
                    calc.applyCss(element, found ? found.css : null);
                    // another's place: an outline in their colour
                    const peer = vm.peerMap.get(row + ":" + column);
                    element.style.boxShadow = peer ? "inset 0 0 0 2px " + peer : "";
                    const box = vm.boxes.length ? calc.boxAt(vm.boxes, row, column) : null;
                    element.classList.toggle("m-calc-merged", !!box);
                    return box ? vm.boxSlice(box, row, column) : calc.cellNode(found);
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
                    editor: cellEdits ? vm.cellEditor : undefined, editable: cellEdits ? () => !vm.pointing : undefined });
                const data = new Array(rowCount);
                for (let r = 0; r < rowCount; r++) data[r] = { id: r + 1 };
                this.table = new root.Tabulator(element, { data, columns, index: "id", height: this.height, layout: "fitData", placeholder: "",
                    frozenRows: Number(sheet.frozenRows || 0), columnDefaults: { tooltip: false }, editTriggerEvent: "dblclick",
                    // a row's own height, after Tabulator measured it; a row is resized by dragging its lower edge
                    rowFormatter: (row) => calc.setRowHeight(row, vm.heights.get(row.getData().id)), resizableRows: this.editable,
                    selectableRange: 1, selectableRangeColumns: true, selectableRangeRows: true, selectableRangeClearCells: false, selectableRangeFill: cellEdits,
                    // a table built while pointing (another sheet chosen) leaves the focus in the formula bar
                    selectableRangeAutoFocus: !this.pointing,
                    clipboard: "copy", clipboardCopyStyled: false, clipboardCopyRowRange: "range", clipboardCopyConfig: { rowHeaders: false, columnHeaders: false } });
                this.table.on("tableBuilt", () => {
                    vm.table.on("scrollVertical", () => vm.load());
                    vm.table.on("rangeChanged", (range) => vm.rangeChanged(range));
                    if (vm.editable) vm.table.on("columnResized", (column) => {
                        const at = column.getDefinition().calcColumn;
                        if (at) vm.edit([{ op: "width", sheet: vm.sheetIndex, column: at, width: Math.round(column.getWidth()) }]);
                    });
                    if (vm.editable) vm.table.on("rowResized", (row) => {
                        const at = row.getData().id, height = calc.rowHeight(row);
                        if (at && height > 0) vm.edit([{ op: "height", sheet: vm.sheetIndex, row: at, height }]);
                    });
                    // a click on a cell keeps the formula bar's focus while a reference is pointed at
                    if (vm.editable) calc.holdFocus(vm.table, () => !!vm.pointing);
                    if (vm.reselect) vm.selectAgain();
                    // the height a row is drawn at, for the merged cells over rows not drawn yet
                    try { const first = vm.table.getRows()[0]; vm.rowDrawn = (first && first._row && first._row.getHeight()) || vm.rowDrawn; } catch (_) { /* no rows */ }
                    // the fill handle's drop is the engine's autofill, as Fill down is
                    if (cellEdits && !calc.takeFillHandle(vm.table, (source, target) => vm.fillTo(source, target))) {
                        // a Tabulator whose fill handle is not the one taken over: no handle, rather than one that sets
                        // the cells' data in the browser
                        calc.dropFillHandle(vm.table);
                        if (root.console) root.console.warn("CalcGrid: the table's fill handle is not Tabulator 6.7's; it is left out");
                    }
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
                    if (this.table === table) for (const box of block.merges || []) {
                        const key = box.row + ":" + box.column;
                        if (!this.boxKeys.has(key)) { this.boxKeys.add(key); this.boxes.push(box); }
                    }
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
                if (this.pointing) { this.pointAt(next); return; }
                // a merged cell is selected whole, its anchor the cell typed into
                if (this.boxes && this.boxes.length) Object.assign(next, calc.expandToMerges(next, this.boxes));
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
                // what is typed after a reference keeps it: a click puts the next one at the caret
                if (this.pointing) { this.pointing.from = event.target.selectionStart; this.pointing.to = this.pointing.from; }
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
                else if (event.key === "Escape") { this.barText = this.barOriginal; this.suggestions = null; this.endPointing(); }
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
                // the cell pointed from, on its own sheet
                const pointed = this.endPointing();
                const at = pointed ? { sheet: pointed.sheet, row: pointed.row, column: pointed.column } : { sheet: this.sheetIndex, row: this.active.row, column: this.active.column };
                if (!this.editable || this.barText === this.barOriginal) return;
                // committed: no longer what is being typed, so the cell's content as the engine keeps it comes back
                this.barOriginal = this.barText;
                this.edit([Object.assign({ op: "input", value: this.barText }, at)]);
            },

            // ---- pointing at references while a formula is typed (plans/IronCalc-DB2.md CG2.2) ----
            // where in the bar a click on the grid puts a reference: the caret, or the reference just put there, which the
            // click replaces; null when a click is not a pointing one
            pointPlace() {
                const bar = this.$refs.bar, session = this.pointing;
                if (!bar || !this.editable) return null;
                const focused = root.document && root.document.activeElement === bar;
                if (!focused && !session) return null;
                const caret = focused ? bar.selectionStart : session.to;
                if (session && caret === session.to && session.from !== session.to) return { from: session.from, to: session.to };
                return calc.canPoint(this.barText, caret) ? { from: caret, to: caret } : null;
            },
            // the grid pressed (in the capture phase, before Tabulator): a pointing press keeps the bar's focus and pins the
            // cell typed into, and the range Tabulator then selects is put into the bar
            pointDown(event) {
                const place = this.pointPlace();
                if (!place) {
                    // a press elsewhere than at an operator ends the formula as Enter does; on another sheet than the
                    // cell's, which is shown again, the press goes no further
                    if (!this.pointing) return;
                    if (this.pointing.sheet !== this.sheetIndex) { event.preventDefault(); event.stopPropagation(); }
                    this.commitBar();
                    return;
                }
                event.preventDefault();
                this.startPointing(place, true);
            },
            // a sheet's tab pressed while pointing: the bar keeps its focus, and the sheet chosen is pointed into
            tabDown(event) {
                const place = this.pointPlace();
                if (!place) return;
                event.preventDefault();
                this.startPointing(place, false);
            },
            startPointing(place, armed) {
                if (!this.pointing) {
                    this.pointing = { sheet: this.sheetIndex, row: this.active.row, column: this.active.column, from: place.from, to: place.to, armed };
                    this.pointingOn = true;
                    this.suggestions = null;
                } else Object.assign(this.pointing, { from: place.from, to: place.to, armed: armed || this.pointing.armed });
            },
            pointAt(range) {
                const session = this.pointing, target = this.activeSheet, bar = this.$refs.bar;
                if (!session.armed || !target) return;
                const reference = calc.referenceText(range, target, session.sheet);
                if (!reference) return;
                const put = calc.putReference(this.barText, session.from, session.to, reference);
                this.barText = put.text; session.to = put.caret;
                this.$nextTick(() => { if (bar) { bar.focus(); bar.setSelectionRange(put.caret, put.caret); } });
            },
            // pointing ends (Enter, Escape, a press that is not pointing): back to the sheet of the cell typed into
            endPointing() {
                const session = this.pointing;
                if (!session) return null;
                this.pointing = null; this.pointingOn = false;
                // the cell typed into is selected again, in the table of its sheet
                this.reselect = { row: session.row, column: session.column };
                if (this.sheetIndex !== session.sheet) {
                    this.showSheet(session.sheet);
                    this.active = { row: session.row, column: session.column, lastRow: session.row, lastColumn: session.column };
                } else this.selectAgain();
                return session;
            },
            selectAgain() {
                const at = this.reselect, table = this.table;
                this.reselect = null;
                if (!at || !table) return;
                try {
                    const row = table.getRow(at.row), cell = row && row.getCell("c" + at.column);
                    if (cell) table.addRange(cell, cell);
                } catch (_) { /* the row is not in the table */ }
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
                    // an undo passed by what another changed or deleted since: said, not an error
                    if (Number(json.rulesPassed) > 0) this.tell(calc.passedText(Number(json.rulesPassed)));
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
                        && built.widths === JSON.stringify((sheet.widths || []).slice(0, this.columnCount))
                        && built.heights === JSON.stringify(sheet.heights || [])
                        && built.merges === JSON.stringify(sheet.merges || []);
                    if (same) this.load(); else this.build(top);
                    // what is being typed in the bar is left as it is
                    const typing = this.$refs.bar && root.document && root.document.activeElement === this.$refs.bar && this.barText !== this.barOriginal;
                    if (!typing) this.readContent();
                });
            },
            selection() { return { sheet: this.sheetIndex, row: this.active.row, column: this.active.column, width: this.active.lastColumn - this.active.column + 1, height: this.active.lastRow - this.active.row + 1 }; },
            style(path, value) { return this.edit([Object.assign({ op: "style", path, value: String(value) }, this.selection())]); },
            // a border of the selection, in the line and colour chosen in the menu
            border(kind) { return this.edit([Object.assign({ op: "border", kind, style: this.borderStyle, color: this.borderColor }, this.selection())]); },
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
            merge(on) { return this.edit([Object.assign({ op: on ? "merge" : "unmerge" }, this.selection())]); },
            // a merged cell's slice for the cell at row, column of it (calc.sliceNode), from the widths the table was built
            // with and the rows' heights as drawn
            boxSlice(box, row, column) {
                const cells = this.rows().get(box.row), anchor = cells ? cells[box.column - 1] : null;
                const width = (c) => this.widthsBuilt[c - 1] || 90;
                const height = (r) => {
                    let drawn = 0;
                    try { const at = this.table && this.table.getRow(r); drawn = at && at._row ? at._row.getHeight() : 0; } catch (_) { drawn = 0; }
                    return drawn || (this.heights.get(r) ? this.heights.get(r) + 1 : this.rowDrawn || 29);
                };
                const place = { x: 0, y: 0, width: -1, height: -1, right: column < box.column + box.width - 1, below: row < box.row + box.height - 1 };
                for (let c = box.column; c < box.column + box.width; c++) { if (c < column) place.x += width(c); place.width += width(c); }
                for (let r = box.row; r < box.row + box.height; r++) { if (r < row) place.y += height(r); place.height += height(r); }
                return calc.sliceNode(anchor, place);
            },
            fillDown() {
                if (this.active.lastRow <= this.active.row) return null;
                return this.edit([{ op: "fill", sheet: this.sheetIndex, row: this.active.row, column: this.active.column, width: this.active.lastColumn - this.active.column + 1, height: 1, toRow: this.active.lastRow }]);
            },
            // a drag of the fill handle (calc.takeFillHandle): positions among the table's rows and columns
            fillTo(source, target) {
                const table = this.table;
                if (!table) return null;
                const ranges = table.modules.selectRange, rows = ranges.getTableRows(), columns = ranges.getTableColumns();
                const rowAt = (at) => (rows[at] && rows[at].getData().id) || null;
                // the row numbers' column is the first: a drag onto it stops at column A
                const columnAt = (at) => (columns[at] ? columns[at].getDefinition().calcColumn || 1 : null);
                const op = calc.fillOp(this.sheetIndex, source, target, rowAt, columnAt);
                return op ? this.edit([op]) : null;
            },
            clearSelection() { return this.edit([Object.assign({ op: "clear" }, this.selection())]); },
            // a cell's editor: what was typed into it (asked when it opens), set on Enter, Tab or leaving it
            cellEditor(cell, onRendered, success, cancel) {
                const vm = this, at = { sheet: this.sheetIndex, row: cell.getRow().getData().id, column: cell.getColumn().getDefinition().calcColumn };
                // a merged cell is typed into at its anchor
                const box = calc.boxAt(this.boxes, at.row, at.column);
                if (box) { at.row = box.row; at.column = box.column; }
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
                this.$q.dialog({ title: L("Rename the sheet"), prompt: { model: sheet.name, type: "text" }, cancel: true })
                    .onOk((typed) => { const name = String(typed || "").trim(); if (name && name !== sheet.name) this.edit([{ op: "renameSheet", sheet: sheet.index, name }]); });
            },
            deleteSheet(sheet) {
                this.$q.dialog({ title: "Delete the sheet", message: "Delete " + sheet.name + " and everything on it?", cancel: true, persistent: true })
                    .onOk(() => this.edit([{ op: "deleteSheet", sheet: sheet.index }]).then(() => { if (!this.sheet()) { this.sheetIndex = this.firstSheet(); this.build(); } }));
            },
            // ---- conditional formats (plans/IronCalc-DB2.md CG2.5) ----
            ruleLabel(rule) { return calc.ruleLabel(rule || {}); },
            formOf(one) { return calc.formOf(one.range, one.rule || {}); },
            // the sheet's rules, and a new one for the selection
            openRules() { this.newRule(); this.rulesOpen = true; },
            newRule() { this.rule = calc.ruleForm(calc.rangeName(this.active)); this.ruleIndex = null; this.ruleShown = null; },
            changeRule(one) { const form = this.formOf(one); if (form) { this.rule = form; this.ruleIndex = one.index; this.ruleShown = one; } },
            // an edit of a rule names it by its place and by what it is (calc.ruleExpect), so the server finds it after
            // another's edits of the sheet's rules, or refuses
            ruleEdit(op, one) {
                const sent = Object.assign({ sheet: this.sheetIndex }, op, one ? { expect: calc.ruleExpect(one) } : {});
                return this.edit([sent]).then((state) => { if (op.op === "deleteRule" && op.index === this.ruleIndex) this.newRule(); return state; });
            },
            saveRule() {
                const rule = calc.ruleOf(this.rule), range = String(this.rule.range || "").trim();
                if (!rule || !range) return null;
                const op = this.ruleIndex == null ? { op: "addRule", range, rule } : { op: "changeRule", index: this.ruleIndex, range, rule };
                return this.ruleEdit(op, this.ruleIndex == null ? null : this.ruleShown).then((state) => { if (state && !state.errors) this.newRule(); return state; });
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
            // each with the id the server gave it, which it keeps, so a dashboard panel of the chart still finds it
            chartDefinitions() { return this.charts.map((chart) => Object.assign(chart.id ? { id: chart.id } : {}, { title: chart.title, type: chart.type, categories: chart.categories, series: (chart.series || []).map((one) => ({ name: one.name, values: one.values })) })); },
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
