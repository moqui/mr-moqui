/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

// The text render mode: fixed-width columns for a console or a text file. This follows
// DefaultScreenMacros.text.ftl. Elements it does not name are no-ops, as the FreeMarker fallback was.
//
// Differences from the FreeMarker macros, all in the cell width. FreeMarker kept the last assigned
// width in a global, so text after a container-box or a form-single was cut to nothing or to 80%
// of the line. Here the width goes back to the line width after each of them.
(function (T, S) {
    "use strict";

    const { attr, has, jsString, child, childrenNamed, js, raw, stmt, noop, noops } = T;
    const { nodeLit, fieldChain, widgetsSource, setCode, widgetless, localizedTitle, expandExpr } = S;

    const MODE = "text";

    // Runtime for every unit: the line state, $pad (paddedValue) and $formList. The state is declared
    // here and set by the widgets macro, so each section starts with its own.
    const prelude = `let $lineChars = 132, $pageLines = 0, $lineWrap = false;
let $cellWidth = 0, $cellLeftPad = false, $cellWrapLine = 0, $cellOverflow = false, $lineCount = 1;
const $pad = (text, characters, leftPad, wrapLine) => {
    if (characters === undefined) characters = $cellWidth > 0 ? $cellWidth : $lineChars;
    if (leftPad === undefined) leftPad = $cellLeftPad;
    if (wrapLine === undefined) wrapLine = $cellWrapLine;
    characters = Math.trunc(characters);
    if (characters <= 0) return "";
    text = String(text);
    const start = wrapLine * characters, end = (wrapLine + 1) * characters;
    if (text.length > end) $cellOverflow = true;
    let out = start >= text.length ? "" : text.substring(start, Math.min(end, text.length));
    if (out.length < characters) out = leftPad ? out.padStart(characters) : out.padEnd(characters);
    return out;
};
const $formList = (name, cols) => {
    const info = sri.openFormList(name);
    const visible = cols.map((col) => col.filter((f) => !f.hide || !f.hide())).filter((col) => col.length > 0);
    const widths = sri.formListColumnCharWidths(visible.map((col) => col.map((f) => [f.printWidth, f.printWidthType])), $lineChars);
    const rightAlign = (f) => f.align === "right" || f.align === "center";
    const headerRows = () => {
        for (let hdr = 0; hdr < 6; hdr++) {
            let more = false;
            visible.forEach((col, ci) => {
                const width = widths[ci];
                if (width <= 0) return;
                if (ci > 0) sri.write("|");
                if (hdr >= col.length) sri.write(" ".repeat(width));
                else sri.write($pad(col[hdr].header(), width, rightAlign(col[hdr]), 0));
                if (col.length > hdr + 1) more = true;
            });
            sri.write("\\n");
            $lineCount++;
            if (!more) break;
        }
        let rule = "";
        visible.forEach((col, ci) => {
            if (widths[ci] <= 0) return;
            if (ci > 0) rule += "+";
            rule += "-".repeat(widths[ci]);
        });
        sri.write(rule + "\\n");
        $lineCount++;
    };
    $lineCount = 1;
    headerRows();
    for (let index = 0; index < info.count; index++) {
        sri.startFormListRow(info, index);
        try {
            for (let fieldIndex = 0; fieldIndex < 6; fieldIndex++) {
                let more = false;
                for (let wrap = 0; wrap <= 10; wrap++) {
                    $cellOverflow = false;
                    $cellWrapLine = wrap;
                    visible.forEach((col, ci) => {
                        const width = widths[ci];
                        $cellWidth = width;
                        if (width <= 0) return;
                        if (ci > 0) sri.write("|");
                        if (fieldIndex >= col.length) sri.write(" ".repeat(width));
                        else {
                            $cellLeftPad = rightAlign(col[fieldIndex]);
                            col[fieldIndex].cell();
                        }
                        if (col.length > fieldIndex + 1) more = true;
                    });
                    sri.write("\\n");
                    if ($pageLines > 0 && $lineCount === $pageLines) {
                        $lineCount = 1;
                        headerRows();
                    } else $lineCount++;
                    if (!$cellOverflow || !$lineWrap) break;
                }
                if (!more) break;
            }
        } finally {
            sri.endFormListRow();
        }
    }
    sri.safeCloseList(info);
    $cellWidth = 0;
    $cellWrapLine = 0;
    $cellLeftPad = false;
    sri.write("\\n");
};
`;

    const pad = (expression) => `sri.write($pad(${expression}));\n`;

    const widgetsMacro = stmt((node, children) => js`
        $lineChars = Number(lineCharacters || "132");
        $pageLines = pageLines ? Number(pageLines) : 0;
        $lineWrap = lineWrap === "true";
        ${children}
    `);

    // A drop-down, radio or check shows the text of the option that is the current value.
    const optionWidget = (expandKey) => stmt((node) => {
        const key = has(node, "no-current-selected-key") ?
            (expandKey ? S.expandNoL10nExpr(attr(node, "no-current-selected-key")) : jsString(attr(node, "no-current-selected-key"))) : '""';
        return js`
            {
                const $options = sri.getFieldOptions(${nodeLit(node, true)});
                let $current = sri.getFieldValueString(${nodeLit(node, false)});
                if (!$current) $current = ${key};
                const $text = $options.get($current);
                sri.write($pad($text != null ? $text : $current));
            }
        `;
    });

    // A link in a field shows its text when it is an anchor or a hidden form link.
    const linkMacro = stmt((node) => {
        const inField = node.parent && node.parent.name.endsWith("-field");
        const type = attr(node, "link-type");
        if (!inField || (type !== "anchor" && type !== "hidden-form-link")) return "";
        const condition = attr(node, "condition");
        const text = S.linkTextExpr(node);
        const body = js`
            {
                let $linkText = ${text};
                if ($linkText === "null") $linkText = "";
                sri.write($pad($linkText));
            }
        `;
        return condition ? js`
            if (${raw(condition)}) {
                ${body}
            }
        ` : body;
    });

    // One cell of a form-list: the widgets of the first conditional-field that holds, else the default-field.
    const cellCode = (field) => fieldChain(field, (sub) => widgetless(sub) ? "" : setCode(sub) + widgetsSource(sub));

    const columnField = (field) => {
        const fieldHide = attr(field, "hide");
        const header = localizedTitle(S.headerSub(field));
        return js`
            {
                "name": ${jsString(attr(field, "name"))},
                "align": ${jsString(attr(field, "align"))},
                "printWidth": ${jsString(attr(field, "print-width"))},
                "printWidthType": ${jsString(attr(field, "print-width-type"))},
                ${fieldHide ? `"hide": () => (${raw(fieldHide)}),` : ""}
                "header": () => ${header},
                "cell": () => {
                    ${cellCode(field)}
                }
            }
        `.replace(/\n$/, "");
    };

    const formListMacro = stmt((node) => {
        const columns = S.formListColumns(node).map((column) =>
            "[" + column.map(columnField).join(",\n") + "]").join(",\n");
        return js`
            $formList(${jsString(attr(node, "name"))}, [
                ${columns}
            ]);
        `;
    });

    const formSingleMacro = stmt((node) => {
        const map = attr(node, "map") || "fieldValues";
        const fields = childrenNamed(node, "field").map((field) => {
            const code = fieldChain(field, (sub) => {
                if (widgetless(sub)) return "";
                return js`
                    ${setCode(sub)}
                    $cellWidth = $lineChars * 0.8;
                    sri.write($pad(${localizedTitle(sub)}, $lineChars * 0.2, true) + ": ");
                    ${widgetsSource(sub)}
                `;
            });
            return js`
                ${code}
                sri.write("\\n");
            `;
        }).join("");
        return js`
            sri.pushSingleFormMapContext(${raw(map)});
            try {
                ${fields}
            } finally {
                sri.popContext();
            }
            $cellWidth = 0;
            sri.write("\\n");
        `;
    });

    const textWidget = stmt((node) => pad(`sri.getFieldValueString(${nodeLit(node, false)})`));

    const macros = Object.assign({}, noops(S.allElements), S.anyMacros(MODE), {
        "screen": stmt((node, children) => children),
        "widgets": widgetsMacro,
        "fail-widgets": widgetsMacro,
        "dynamic-form": widgetsMacro,
        "container": stmt((node, children) => js`
            ${children}
            sri.write("\\n");
        `),
        "container-box": stmt((node) => {
            const header = child(node, "box-header");
            const body = child(node, "box-body");
            const nopad = child(node, "box-body-nopad");
            return js`
                $cellWidth = $lineChars;
                ${header ? header.source : ""}
                $cellWidth = 0;
                sri.write("\\n");
                ${body ? body.source : ""}
                ${nopad ? nopad.source : ""}
            `;
        }),
        "box-header": stmt((node, children) => children),
        "box-body": stmt((node, children) => children),
        "box-body-nopad": stmt((node, children) => children),
        "container-row": stmt((node, children) => children),
        "row-col": stmt((node, children) => children),
        "container-panel": stmt((node) => ["panel-header", "panel-left", "panel-center", "panel-right", "panel-footer"]
            .map((name) => { const kid = child(node, name); return kid ? kid.source : ""; }).join("")),
        "panel-header": stmt((node, children) => children),
        "panel-left": stmt((node, children) => children),
        "container-split": stmt((node) => ["split-side", "split-main"]
            .map((name) => { const kid = child(node, name); return kid ? kid.source : ""; }).join("")),
        "split-side": stmt((node, children) => children),
        "split-main": stmt((node, children) => children),
        "panel-center": stmt((node, children) => children),
        "panel-right": stmt((node, children) => children),
        "panel-footer": stmt((node, children) => children),
        "container-dialog": stmt((node) => `sri.write(${expandExpr(attr(node, "button-text"))} + " ");`),
        "section-include": stmt((node) => `sri.renderSectionInclude(${S.bareLit(node)});`),
        "link": linkMacro,
        "image": stmt((node) => pad(jsString(attr(node, "alt")))),
        "label": stmt((node) => pad(expandExpr(attr(node, "text")))),
        // a number-card as "Title: value (delta)", a dashboard as its children one after another
        "number-card": stmt((node) => {
            const delta = S.expandNoL10nExpr(attr(node, "delta") || "");
            const line = pad(`${expandExpr(attr(node, "title") || "")} + ": " + ${S.expandNoL10nExpr(attr(node, "value") || "")} + ` +
                `(String(${delta}).trim() ? " (" + ${delta} + ")" : "")`) + `sri.write("\\n");\n`;
            const condition = attr(node, "condition");
            return condition ? `if (${condition}) {\n${line}}\n` : line;
        }),
        "dashboard": stmt((node, children) => children),
        // a status-bar as "Status: In Progress", the current status's description
        "status-bar": stmt((node) => {
            const line = `{ const $v = ${S.statusTextExpr(node)}; if ($v) {\n${pad('ec.l10n.localize("Status") + ": " + $v')}sri.write("\\n");\n} }\n`;
            const condition = attr(node, "condition");
            return condition ? `if (${condition}) {\n${line}}\n` : line;
        }),
        // an html-view as its text: the text body, else the HTML's text
        "html-view": stmt((node) => {
            const line = `{ const $v = ${S.htmlViewTextExpr(node)}; if ($v) sri.write($v + "\\n"); }\n`;
            const condition = attr(node, "condition");
            return condition ? `if (${condition}) {\n${line}}\n` : line;
        }),

        // a code-view as its code
        "code-view": stmt((node) => {
            const line = `{ const $v = ((t) => (t && t !== "null" ? String(t).replace(/\\s+$/, "") : ""))(ec.resource.expandNoL10n(${jsString(attr(node, "text") || "")})); if ($v) sri.write($v + "\\n"); }\n`;
            const condition = attr(node, "condition");
            return condition ? `if (${condition}) {\n${line}}\n` : line;
        }),

        // a markdown-view as its markdown
        "markdown-view": stmt((node) => {
            const line = `{ const $v = ${S.markdownTextExpr(node)}; if ($v) sri.write($v + "\\n"); }\n`;
            const condition = attr(node, "condition");
            return condition ? `if (${condition}) {\n${line}}\n` : line;
        }),

        "form-single": formSingleMacro,
        "form-list": formListMacro,
        "field": stmt((node, children) => children),
        "conditional-field": stmt((node, children) => children),
        "default-field": stmt((node, children) => children),
        "header-field": stmt((node, children) => children),

        "check": optionWidget(true),
        "drop-down": optionWidget(false),
        "radio": optionWidget(false),
        "date-time": stmt((node) => pad(
            `sri.getFieldValueString(${S.fieldLit(node)}, ${jsString(attr(node, "default-value"))}, ${jsString(S.dateTimeFormat(node))}, ${jsString(attr(node, "type") || "date-time")})`)),
        "display": stmt((node) => pad(S.displayValueExpr(node))),
        "display-entity": stmt((node) => pad(`sri.getFieldEntityValue(${nodeLit(node, false)})`)),
        "submit": stmt((node) => pad(localizedTitle(node.parent))),
        "text-area": textWidget,
        "text-line": textWidget,
        "text-find": textWidget
    });

    return { macros, opaque: S.opaqueElements, prelude };
})
