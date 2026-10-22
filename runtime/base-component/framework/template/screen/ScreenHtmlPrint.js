// The html-print render mode: an HTML document with print CSS, for a browser's print or a PDF renderer.
// This follows DefaultScreenMacros.xsl-fo.ftl and Header.xsl-fo.ftl, with HTML and CSS where the FreeMarker
// macros wrote XSL-FO. xsl-fo is the old name of this mode and is rewritten to html-print before a macro
// is looked up.
//
// The page is chosen by the layoutMaster context field (letter-portrait when unset): letter, legal,
// tabloid, a4 or a3, then portrait or landscape. The document title is documentTitle. Page breaks are
// classes on a container's style: page-break-before, page-break-after and no-break. Tables repeat their
// header on each page and do not split a row.
//
// Differences from the FreeMarker macros: elements the mode does not name render nothing, where the
// FreeMarker macros wrote a block saying the element was not implemented. A link is an HTML anchor and its
// URL is not rewritten from /apps/ to /vapps/. A form-list column width is a share of the table.
(function (T, S) {
    "use strict";

    const { attr, has, jsString, child, childrenNamed, js, raw, stmt, noops } = T;
    const { nodeLit, fieldChain, widgetsSource, setCode, widgetless, localizedTitle, expandExpr } = S;

    const MODE = "html-print";

    // Page sizes from Header.xsl-fo.ftl, with the width in inches the macros left for content.
    // Runtime for every unit: $e encodes text, $layout is the master chosen by layoutMaster and $formList
    // writes a table for a form-list.
    const prelude = `const $e = (value) => sri.encodeHtml(String(value == null ? "" : value));
const $masters = {
    "letter-portrait": ["8.5in", "11in", "0.25in", 8], "letter-landscape": ["11in", "8.5in", "0.25in", 10.5],
    "legal-portrait": ["8.5in", "14in", "0.25in", 8], "legal-landscape": ["14in", "8.5in", "0.25in", 13.5],
    "tabloid-portrait": ["11in", "17in", "0.25in", 10.5], "tabloid-landscape": ["17in", "11in", "0.25in", 16.5],
    "a4-portrait": ["210mm", "297mm", "15mm", 7.8], "a4-landscape": ["297mm", "210mm", "15mm", 11.2],
    "a3-portrait": ["297mm", "420mm", "15mm", 11.2], "a3-landscape": ["420mm", "297mm", "15mm", 16]
};
const $layoutName = $masters[layoutMaster] ? layoutMaster : "letter-portrait";
const $layout = $masters[$layoutName];
const $lineChars = Math.floor($layout[3] * 72 / 6);
const $formList = (name, range, cols) => {
    const info = sri.openFormList(name);
    if (!info.count) { sri.safeCloseList(info); return; }
    const visible = cols.map((col) => col.filter((f) => !f.hide || !f.hide())).filter((col) => col.length > 0);
    const widths = sri.formListColumnCharWidths(visible.map((col) => col.map((f) => [f.printWidth, f.printWidthType])), $lineChars);
    const shown = visible.map((col, index) => [col, widths[index]]).filter((pair) => pair[1] > 0);
    const total = shown.reduce((sum, pair) => sum + pair[1], 0) || 1;
    const page = range ? range() : null;
    if (page && page.count > 0) sri.write('<div class="page-range">' + page.low + " - " + page.high + " / " + page.count + "</div>\\n");
    sri.write('<table class="form-list">\\n<colgroup>');
    shown.forEach((pair) => sri.write('<col style="width:' + (pair[1] * 100 / total).toFixed(2) + '%">'));
    sri.write("</colgroup>\\n<thead><tr>");
    shown.forEach((pair) => {
        sri.write("<th>");
        pair[0].forEach((f) => sri.write('<div style="text-align:' + (f.align || "left") + '">' + $e(f.header()) + "</div>"));
        sri.write("</th>");
    });
    sri.write("</tr></thead>\\n<tbody>\\n");
    for (let index = 0; index < info.count; index++) {
        sri.startFormListRow(info, index);
        try {
            sri.write("<tr>");
            shown.forEach((pair) => {
                sri.write("<td>");
                pair[0].forEach((f) => {
                    sri.write('<div style="text-align:' + (f.align || "left") + '">');
                    f.cell();
                    sri.write("</div>");
                });
                sri.write("</td>");
            });
            sri.write("</tr>\\n");
        } finally {
            sri.endFormListRow();
        }
    }
    sri.write("</tbody>\\n</table>\\n");
    sri.safeCloseList(info);
};
`;

    const e = (expression) => `sri.write($e(${expression}));\n`;

    // The document around the outermost screen's widgets. Only the first widgets of a render writes it. The title in
    // the @top-center margin box is a CSS string in the style element: no quote or backslash, and no < or > that
    // could end the element.
    const documentCode = (children) => js`
        const $document = sri.once("html-print-document");
        if ($document) {
            const $title = documentTitle ? String(documentTitle) : "";
            const $cssTitle = $title.replace(/[\\\\"\\n\\r<>]/g, " ");
            let $pages = "";
            for (const $name of Object.keys($masters)) {
                const $m = $masters[$name];
                $pages += "@page " + $name + " { size: " + $m[0] + " " + $m[1] + "; margin: " + $m[2] + "; }\\n";
            }
            sri.write("<!DOCTYPE html>\\n<html><head><meta charset=\\"utf-8\\"><title>" + $e($title) + "</title>\\n<style>\\n" + $pages +
                "@page { size: " + $layout[0] + " " + $layout[1] + "; margin: " + $layout[2] + ";" +
                ($title ? ' @top-center { content: "' + $cssTitle + '"; font-size: 14pt; }' : "") +
                ' @bottom-center { content: "- " counter(page) " -"; font-size: 8pt; } }\\n' +
                ${jsString(printCss)} + "</style></head>\\n<body class=\\"" + $layoutName + "\\">\\n");
        }
        ${children}
        if ($document) sri.write("</body></html>\\n");
    `;

    const printCss = `body { font-family: Helvetica, Arial, sans-serif; font-size: 9pt; margin: 0; }
table { border-collapse: collapse; table-layout: fixed; width: 100%; }
td, th { padding: 2pt; vertical-align: top; overflow-wrap: anywhere; }
table.form-list thead { display: table-header-group; border-bottom: thin solid black; }
table.form-list tr { break-inside: avoid; page-break-inside: avoid; }
table.form-list tbody tr:nth-child(odd) { background-color: #EEEEEE; }
table.form-list th { font-weight: bold; text-align: left; }
.field-title { width: 20%; text-align: right; font-weight: bold; }
.field-value { width: 80%; }
.field-wide { width: 100%; }
.box-header { font-size: 12pt; font-weight: bold; }
.group-title { font-weight: bold; }
table.panel { border: 1px solid black; }
.page-break-before { break-before: page; page-break-before: always; }
.page-break-after { break-after: page; page-break-after: always; }
.no-break { break-inside: avoid; page-break-inside: avoid; }
a { color: blue; }
.dashboard { margin: 4pt 0; }
.html-view { white-space: pre-wrap; overflow-wrap: anywhere; margin: 0 0 6pt; }
.number-card { display: inline-block; vertical-align: top; min-width: 90pt; margin: 0 6pt 6pt 0; padding: 4pt 6pt; border: thin solid #999999; break-inside: avoid; }
.number-card-title { font-size: 7pt; text-transform: uppercase; letter-spacing: 0.5pt; }
.number-card-value { font-size: 14pt; font-weight: bold; }
`;

    const widgetsMacro = stmt((node, children) => {
        const root = node.parent && node.parent.name === "screen";
        return root ? documentCode(children) : children;
    });

    const optionWidget = stmt((node) => {
        const key = has(node, "no-current-selected-key") ? jsString(attr(node, "no-current-selected-key")) : '""';
        return js`
            {
                const $options = sri.getFieldOptions(${nodeLit(node, true)});
                let $current = sri.getFieldValueString(${nodeLit(node, false)});
                if (!$current) $current = ${key};
                const $text = $options.get($current);
                sri.write($e($text != null ? $text : $current));
            }
        `;
    });

    // A link: an anchor in a block, or only the anchor inside a field.
    const linkMacro = stmt((node) => {
        const inField = node.parent && node.parent.name.endsWith("-field");
        const condition = attr(node, "condition");
        const urlType = attr(node, "url-type") || "transition";
        const withParams = attr(node, "url-noparam") !== "true";
        const anchor = js`
            {
                let $linkText = ${S.linkTextExpr(node)};
                if ($linkText === "null") $linkText = "";
                const $url = sri.makeUrlByType(${jsString(attr(node, "url"))}, ${jsString(urlType)}, ${nodeLit(node, true)}, ${jsString(attr(node, "expand-transition-url") || "true")});
                sri.write('<a href="' + $e($url.${withParams ? "getUrlWithParams()" : "url"}) + '">' + $e($linkText) + "</a>");
            }
        `;
        const body = inField ? anchor : js`
            sri.write("<div>");
            ${anchor}
            sri.write("</div>\\n");
        `;
        return condition ? js`
            if (${raw(condition)}) {
                ${body}
            }
        ` : body;
    });

    // A number-card printed as its title, value and delta in a small bordered box; the boxes of a dashboard flow in rows.
    const numberCardMacro = stmt((node) => {
        const condition = attr(node, "condition");
        const body = js`
            {
                const $delta = String(${S.expandNoL10nExpr(attr(node, "delta") || "")}).trim();
                sri.write("<div class=\\"number-card\\"><div class=\\"number-card-title\\">" + $e(${expandExpr(attr(node, "title") || "")}) +
                    "</div><div class=\\"number-card-value\\">" + $e(${S.expandNoL10nExpr(attr(node, "value") || "")}) + "</div>" +
                    ($delta ? "<div>" + $e($delta) + "</div>" : "") + "</div>\\n");
            }
        `;
        return condition ? js`
            if (${raw(condition)}) {
                ${body}
            }
        ` : body;
    });

    const labelMacro = stmt((node) => {
        const condition = attr(node, "condition");
        const textMap = attr(node, "text-map");
        const text = textMap ? `ec.resource.expand(${jsString(attr(node, "text"))}, "", ${raw(textMap)})` : expandExpr(attr(node, "text"));
        const body = js`
            {
                const $label = ${text};
                if (String($label).trim() !== "") sri.write("<div>" + $e($label) + "</div>\\n");
            }
        `;
        return condition ? js`
            if (${raw(condition)}) {
                ${body}
            }
        ` : body;
    });

    const imageMacro = stmt((node) => {
        const urlType = attr(node, "url-type") || "content";
        // A content URL is not resolved by makeUrlByType yet, so the plain URL is written.
        const type = urlType === "content" ? "plain" : urlType;
        const optional = ["id", "width", "height"].filter((name) => attr(node, name))
            .map((name) => `' ${name}="' + $e(${jsString(attr(node, name))}) + '"'`).join(" + ");
        return js`
            {
                const $url = sri.makeUrlByType(${jsString(attr(node, "url"))}, ${jsString(type)}, null, "true");
                sri.write('<img src="' + $e($url.url) + '" alt="' + $e(${jsString(attr(node, "alt") || "image")}) + '"'${optional ? " + " + optional : ""} + "/>\\n");
            }
        `;
    });

    // ---- forms ----

    // The code for one field of a form-single: a title cell and a value cell. A field always has a title,
    // from its title attribute or its name, so the FreeMarker macro's case of a big row cell with no title
    // cell never happens.
    const singleFieldCode = (field) => fieldChain(field, (sub) => {
        if (widgetless(sub)) return "";
        return js`
            ${setCode(sub)}
            sri.write('<table class="field"><tr><td class="field-title">' + $e(${localizedTitle(sub)}) + '</td><td class="field-value">');
            ${widgetsSource(sub)}
            sri.write("</td></tr></table>\\n");
        `;
    });

    // The code for the children of a field-layout element, with the fields of the form by name.
    const layoutCode = (node, fields, referenced) => node.children.map((kid) => {
        const name = kid.name;
        if (name === "field-ref") {
            const field = fields[attr(kid, "name")];
            if (!field) return `sri.write(${jsString("<div>Error: could not find field with name " + attr(kid, "name") + " referred to in a field-ref.name attribute.</div>\n")});\n`;
            return singleFieldCode(field);
        }
        if (name === "fields-not-referenced") {
            return Object.keys(fields).filter((fieldName) => !referenced.has(fieldName))
                .map((fieldName) => singleFieldCode(fields[fieldName])).join("");
        }
        if (name === "field-row" || name === "field-col-row") return rowCode(kid, fields, referenced, false);
        if (name === "field-row-big") return rowCode(kid, fields, referenced, true);
        if (name === "field-group") {
            const title = attr(kid, "title") || "Fields";
            return js`
                sri.write('<div class="group-title">' + $e(ec.l10n.localize(${jsString(title)})) + "</div>\\n");
                ${layoutCode(kid, fields, referenced)}
            `;
        }
        if (name === "field-accordion" || name === "field-col") return layoutCode(kid, fields, referenced);
        return "";
    }).join("");

    // A row of cells side by side. A big row has an optional title cell of a fifth of the width.
    const rowCode = (node, fields, referenced, big) => {
        const title = big && attr(node, "title");
        const cells = node.children.map((kid) => js`
            sri.write("<td>");
            ${layoutCode({ children: [kid] }, fields, referenced)}
            sri.write("</td>");
        `).join("");
        return js`
            sri.write('<table class="field-row"><tr>');
            ${title ? `sri.write('<td class="field-title">' + $e(${expandExpr(title)}) + "</td>");` : ""}
            ${cells}
            sri.write("</tr></table>\\n");
        `;
    };

    const collectRefs = (node, set) => {
        node.children.forEach((kid) => {
            if (kid.name === "field-ref") set.add(attr(kid, "name"));
            collectRefs(kid, set);
        });
        return set;
    };

    const formSingleMacro = stmt((node) => {
        const map = attr(node, "map") || "fieldValues";
        const fields = {};
        childrenNamed(node, "field").forEach((field) => { fields[attr(field, "name")] = field; });
        const layout = child(node, "field-layout");
        const body = layout ?
            layoutCode(layout, fields, collectRefs(layout, new Set())) :
            Object.keys(fields).map((name) => singleFieldCode(fields[name])).join("");
        return js`
            sri.pushSingleFormMapContext(${raw(map)});
            try {
                ${body}
            } finally {
                sri.popContext();
            }
        `;
    });

    const columnField = (field) => {
        const fieldHide = attr(field, "hide");
        const header = localizedTitle(S.headerSub(field));
        const cell = fieldChain(field, (sub) => widgetless(sub) ? "" : setCode(sub) + widgetsSource(sub));
        return js`
            {
                "name": ${jsString(attr(field, "name"))},
                "align": ${jsString(attr(field, "align"))},
                "printWidth": ${jsString(attr(field, "print-width"))},
                "printWidthType": ${jsString(attr(field, "print-width-type"))},
                ${fieldHide ? `"hide": () => (${raw(fieldHide)}),` : ""}
                "header": () => ${header},
                "cell": () => {
                    ${cell}
                }
            }
        `.replace(/\n$/, "");
    };

    const formListMacro = stmt((node) => {
        const columns = S.formListColumns(node).map((column) =>
            "[" + column.map(columnField).join(",\n") + "]").join(",\n");
        // The page range is read from the context after the list is opened, which sets it.
        const list = attr(node, "list");
        const range = attr(node, "paginate") !== "false" && /^\w+$/.test(list) ?
            `() => ({low: ${list}PageRangeLow, high: ${list}PageRangeHigh, count: ${list}Count})` : "null";
        return js`
            $formList(${jsString(attr(node, "name"))}, ${range}, [
                ${columns}
            ]);
        `;
    });

    const textWidget = stmt((node) => e(`sri.getFieldValueString(${nodeLit(node, false)})`));

    const macros = Object.assign({}, noops(S.allElements), S.anyMacros(MODE), {
        "screen": stmt((node, children) => children),
        "widgets": widgetsMacro,
        "fail-widgets": widgetsMacro,
        "dynamic-form": stmt((node, children) => children),
        "container": stmt((node, children) => {
            const style = attr(node, "style");
            const id = attr(node, "id");
            const open = `<div${id ? ` id="${id}"` : ""}${style ? ` class="${style}"` : ""}>`;
            return `sri.write(${jsString(open)});\n${children}sri.write("</div>\\n");\n`;
        }),
        "container-box": stmt((node) => {
            const header = child(node, "box-header");
            const body = child(node, "box-body");
            const nopad = child(node, "box-body-nopad");
            return js`
                sri.write('<div class="box-header">');
                ${header ? header.source : ""}
                sri.write("</div>\\n");
                ${body ? body.source : ""}
                ${nopad ? nopad.source : ""}
            `;
        }),
        "box-header": stmt((node, children) => children),
        "box-body": stmt((node, children) => children),
        "box-body-nopad": stmt((node, children) => children),
        "container-row": stmt((node, children) => children),
        "row-col": stmt((node, children) => children),
        "container-panel": stmt((node) => {
            const part = (name) => { const kid = child(node, name); return kid ? kid.source : ""; };
            const cell = (name) => child(node, name) ? `sri.write("<td>");\n${part(name)}sri.write("</td>");\n` : "";
            return js`
                ${child(node, "panel-header") ? `sri.write("<div>");\n${part("panel-header")}sri.write("</div>\\n");\n` : ""}
                sri.write('<table class="panel"><tr>');
                ${cell("panel-left")}
                ${cell("panel-center")}
                ${cell("panel-right")}
                sri.write("</tr></table>\\n");
                ${child(node, "panel-footer") ? `sri.write("<div>");\n${part("panel-footer")}sri.write("</div>\\n");\n` : ""}
            `;
        }),
        "panel-header": stmt((node, children) => children),
        "panel-left": stmt((node, children) => children),
        // a printed split is its side and then its main pane, one after the other
        "container-split": stmt((node) => ["split-side", "split-main"].map((name) => { const kid = child(node, name); return kid ? kid.source : ""; }).join("")),
        "split-side": stmt((node, children) => children),
        "split-main": stmt((node, children) => children),
        "panel-center": stmt((node, children) => children),
        "panel-right": stmt((node, children) => children),
        "panel-footer": stmt((node, children) => children),
        "section-include": stmt((node) => `sri.renderSectionInclude(${S.bareLit(node)});`),
        "link": linkMacro,
        "image": imageMacro,
        "label": labelMacro,
        "number-card": numberCardMacro,
        // a status-bar printed as the current status's description
        "status-bar": stmt((node) => {
            const line = `{ const $v = ${S.statusTextExpr(node)}; if ($v) sri.write("<div class=\\"status-bar\\">" + $e(ec.l10n.localize("Status")) + ": " + $e($v) + "</div>\\n"); }\n`;
            const condition = attr(node, "condition");
            return condition ? `if (${condition}) {\n${line}}\n` : line;
        }),
        "dashboard": stmt((node, children) => `sri.write("<div class=\\"dashboard\\">");\n${children}sri.write("</div>\\n");\n`),
        // an html-view printed as its text (no remote image is fetched for a print)
        "html-view": stmt((node) => {
            const line = `{ const $v = ${S.htmlViewTextExpr(node)}; if ($v) sri.write("<div class=\\"html-view\\">" + $e($v) + "</div>\\n"); }\n`;
            const condition = attr(node, "condition");
            return condition ? `if (${condition}) {\n${line}}\n` : line;
        }),

        // a code-view printed as a pre of its code
        "code-view": stmt((node) => {
            const line = `{ const $v = ((t) => (t && t !== "null" ? String(t).replace(/\\s+$/, "") : ""))(ec.resource.expandNoL10n(${jsString(attr(node, "text") || "")})); if ($v) sri.write("<pre class=\\"code-view\\"><code>" + $e($v) + "</code></pre>\\n"); }\n`;
            const condition = attr(node, "condition");
            return condition ? `if (${condition}) {\n${line}}\n` : line;
        }),

        // a markdown-view printed as its markdown text
        "markdown-view": stmt((node) => {
            const line = `{ const $v = ${S.markdownTextExpr(node)}; if ($v) sri.write("<div class=\\"markdown-view\\" style=\\"white-space:pre-wrap\\">" + $e($v) + "</div>\\n"); }\n`;
            const condition = attr(node, "condition");
            return condition ? `if (${condition}) {\n${line}}\n` : line;
        }),

        "form-single": formSingleMacro,
        "form-list": formListMacro,
        "field": stmt((node, children) => children),
        "conditional-field": stmt((node, children) => children),
        "default-field": stmt((node, children) => children),
        "header-field": stmt((node, children) => children),

        "check": optionWidget,
        "drop-down": optionWidget,
        "radio": optionWidget,
        "date-time": stmt((node) => e(
            `sri.getFieldValueString(${S.fieldLit(node)}, ${jsString(attr(node, "default-value"))}, ${jsString(S.dateTimeFormat(node))}, ${jsString(attr(node, "type") || "date-time")})`)),
        "display": stmt((node) => e(S.displayValueExpr(node))),
        "display-entity": stmt((node) => e(`sri.getFieldEntityValue(${nodeLit(node, false)})`)),
        "submit": stmt((node) => e(localizedTitle(node.parent))),
        "text-area": textWidget,
        "text-line": textWidget,
        "text-find": textWidget
    });

    return { macros, opaque: S.opaqueElements, prelude };
})
