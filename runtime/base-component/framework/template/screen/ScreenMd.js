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

// The md render mode (not in Java): a form-list is a Markdown table, a header row of field titles, a delimiter row with
// each field's align, and a row for each row of the list, and a blank line after it; a form-single is ignored, and elements it does not name are
// no-ops. The form-list show-md-button fetches it to copy a list as Markdown. Cells are written as ScreenCsv.js writes
// them; a field that writes nothing in every case (a submit, an ignored or hidden-only widget) has no column, so every
// row has the header's cells.
(function (T, S) {
    "use strict";

    const { attr, has, jsString, child, childrenNamed, js, raw, stmt, noops } = T;
    const { nodeLit, fieldChain, widgetsSource, setCode, widgetless, expandExpr } = S;

    const MODE = "md";

    // $md escapes a value for a table cell: a backslash and a pipe are escaped, a line break is a <br>.
    // $sep starts a row or writes the pipe between its cells; $end closes the row.
    const prelude = `let $hasPrev = false;
const $md = (value) => String(value).replace(/\\\\/g, "\\\\\\\\").replace(/\\|/g, "\\\\|").replace(/\\r?\\n/g, "<br>");
const $sep = () => { sri.write($hasPrev ? " | " : "| "); $hasPrev = true; };
const $end = () => { sri.write($hasPrev ? " |\\n" : "|\\n"); $hasPrev = false; };
`;

    const md = (expression) => `sri.write($md(${expression}));\n`;

    // The title of a header: the header-field's title, else the default-field's, else the field name in words.
    const headerTitle = (sub) => {
        let chosen = sub;
        if (sub.name === "header-field") {
            const field = sub.parent;
            const dflt = child(field, "default-field");
            if (attr(sub, "title")) chosen = sub;
            else if (dflt && attr(dflt, "title")) chosen = dflt;
        }
        const text = attr(chosen, "title") ? expandExpr(attr(chosen, "title")) : jsString(S.titleText(chosen));
        return `ec.l10n.localize(${text})`;
    };

    const optionWidget = (expandKey) => stmt((node) => {
        const key = has(node, "no-current-selected-key") ?
            (expandKey ? S.expandNoL10nExpr(attr(node, "no-current-selected-key")) : jsString(attr(node, "no-current-selected-key"))) : '""';
        return js`
            {
                const $options = sri.getFieldOptions(${nodeLit(node, true)});
                let $current = sri.getFieldValueString(${nodeLit(node, false)});
                if (!$current) $current = ${key};
                const $text = $options.get($current);
                sri.write($md($text != null ? $text : $current));
            }
        `;
    });

    const linkMacro = stmt((node) => {
        const inField = node.parent && node.parent.name.endsWith("-field");
        const type = attr(node, "link-type");
        if (!inField || (type !== "anchor" && type !== "hidden-form-link")) return "";
        const condition = attr(node, "condition");
        const body = js`
            {
                let $linkText = ${S.linkTextExpr(node)};
                if ($linkText === "null") $linkText = "";
                sri.write($md($linkText));
            }
        `;
        return condition ? js`
            if (${raw(condition)}) {
                ${body}
            }
        ` : body;
    });

    // The sub-nodes of a field that its cell may come from.
    const subsOf = (field) => childrenNamed(field, "conditional-field").concat(childrenNamed(field, "default-field"));
    // Whether a field has a column: some sub-node of it writes a value.
    const hasColumn = (field) => subsOf(field).some((sub) => !widgetless(sub));

    // The code for one cell, which first writes the pipe before it; a sub-node that writes nothing leaves the cell empty.
    const cellCode = (field) => {
        const code = fieldChain(field, (sub) => setCode(sub) + (widgetless(sub) ? "" : widgetsSource(sub)));
        return "$sep();\n" + code;
    };

    const alignRule = (field) => {
        const align = attr(field, "align");
        return align === "right" ? "---:" : align === "center" ? ":---:" : "---";
    };

    const formListMacro = stmt((node) => {
        const fields = [].concat(...S.formListColumns(node)).filter(hasColumn);
        const headers = fields.map((field) => S.hideGuard(field, js`
            $sep();
            sri.write($md(${headerTitle(S.headerSub(field))}));
        `)).join("");
        const rules = fields.map((field) => S.hideGuard(field, js`
            $sep();
            sri.write(${jsString(alignRule(field))});
        `)).join("");
        const cells = fields.map((field) => S.hideGuard(field, cellCode(field))).join("");
        return js`
            {
                const $info = sri.openFormList(${jsString(attr(node, "name"))});
                $hasPrev = false;
                ${headers}
                $end();
                ${rules}
                $end();
                for (let $index = 0; $index < $info.count; $index++) {
                    sri.startFormListRow($info, $index);
                    try {
                        $hasPrev = false;
                        ${cells}
                        $end();
                    } finally {
                        sri.endFormListRow();
                    }
                }
                sri.safeCloseList($info);
                sri.write("\\n");
            }
        `;
    });

    const textWidget = stmt((node) => md(`sri.getFieldValueString(${nodeLit(node, false)})`));

    const macros = Object.assign({}, noops(S.allElements), S.anyMacros(MODE), {
        "screen": stmt((node, children) => children),
        "widgets": stmt((node, children) => children),
        "fail-widgets": stmt((node, children) => children),
        "dynamic-form": stmt((node, children) => children),
        "container": stmt((node, children) => children),
        "container-box": stmt((node) => ["box-body", "box-body-nopad"]
            .map((name) => { const kid = child(node, name); return kid ? kid.source : ""; }).join("")),
        "box-body": stmt((node, children) => children),
        "box-body-nopad": stmt((node, children) => children),
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
        "section-include": stmt((node) => `sri.renderSectionInclude(${S.bareLit(node)});`),
        "link": linkMacro,
        "form-list": formListMacro,
        "field": stmt((node, children) => children),
        "conditional-field": stmt((node, children) => children),
        "default-field": stmt((node, children) => children),
        "header-field": stmt((node, children) => children),

        "check": optionWidget(true),
        "drop-down": optionWidget(false),
        "radio": optionWidget(false),
        "date-time": stmt((node) => md(
            `sri.getFieldValueString(${S.fieldLit(node)}, ${jsString(attr(node, "default-value"))}, ${jsString(S.dateTimeFormat(node))}, ${jsString(attr(node, "type") || "date-time")})`)),
        "display": stmt((node) => md(S.displayValueExpr(node))),
        "display-entity": stmt((node) => md(`sri.getFieldEntityValue(${nodeLit(node, false)})`)),
        "text-area": textWidget,
        "text-line": textWidget,
        "text-find": textWidget
    });

    return { macros, opaque: S.opaqueElements, prelude };
})
