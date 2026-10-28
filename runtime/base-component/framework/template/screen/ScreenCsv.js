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

// The csv render mode. This follows DefaultScreenMacros.csv.ftl: a form-list is a header line of field
// titles and a line for each row, a form-single is ignored, and elements it does not name are no-ops.
//
// Differences from the FreeMarker macros: a date-time with no format is formatted with the type's default
// pattern, where FreeMarker failed on a date it could not treat as text. A label is not written: the FreeMarker
// macro wrote its text with no line break after it, so the header row of the list ran on from the label.
(function (T, S) {
    "use strict";

    const { attr, has, jsString, child, childrenNamed, js, raw, stmt, noops } = T;
    const { nodeLit, fieldChain, widgetsSource, setCode, widgetless, expandExpr } = S;

    const MODE = "csv";

    // $csv quotes a value with a comma, a quote or a line break and doubles its quotes.
    // $sep writes the comma between the values of a row.
    const prelude = `let $hasPrev = false;
const $csv = (value) => {
    value = String(value);
    return /[,"\\n]/.test(value) ? '"' + value.replace(/"/g, '""') + '"' : value;
};
const $sep = () => { if ($hasPrev) sri.write(","); else $hasPrev = true; };
`;

    const csv = (expression) => `sri.write($csv(${expression}));\n`;

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
                sri.write($csv($text != null ? $text : $current));
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
                sri.write($csv($linkText));
            }
        `;
        return condition ? js`
            if (${raw(condition)}) {
                ${body}
            }
        ` : body;
    });

    // The code for one cell, which first writes the comma that separates it from the cell before.
    const cellCode = (field) => fieldChain(field, (sub) =>
        widgetless(sub) ? "" : setCode(sub) + "$sep();\n" + widgetsSource(sub));

    const formListMacro = stmt((node) => {
        const columns = S.formListColumns(node);
        const fields = [].concat(...columns);
        const headers = fields.map((field) => S.hideGuard(field, js`
            $sep();
            sri.write($csv(${headerTitle(S.headerSub(field))}));
        `)).join("");
        const cells = fields.map((field) => S.hideGuard(field, cellCode(field))).join("");
        return js`
            {
                const $info = sri.openFormList(${jsString(attr(node, "name"))});
                $hasPrev = false;
                ${headers}
                sri.write("\\n");
                for (let $index = 0; $index < $info.count; $index++) {
                    sri.startFormListRow($info, $index);
                    try {
                        $hasPrev = false;
                        ${cells}
                        sri.write("\\n");
                    } finally {
                        sri.endFormListRow();
                    }
                }
                sri.safeCloseList($info);
            }
        `;
    });

    const textWidget = stmt((node) => csv(`sri.getFieldValueString(${nodeLit(node, false)})`));

    const reportWidget = stmt((node) => {
        const body = `sri.write(${S.reportRenderExpr(node, "csv")});\n`;
        return attr(node, "condition") ? `if (${attr(node, "condition")}) {\n${body}}\n` : body;
    });

    const macros = Object.assign({}, noops(S.allElements), S.anyMacros(MODE), {
        "report-panel": reportWidget,
        "report-dashboard": reportWidget,
        "report-embed": reportWidget,
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
        "date-time": stmt((node) => csv(
            `sri.getFieldValueString(${S.fieldLit(node)}, ${jsString(attr(node, "default-value"))}, ${jsString(S.dateTimeFormat(node))}, ${jsString(attr(node, "type") || "date-time")})`)),
        "display": stmt((node) => csv(S.displayValueExpr(node))),
        "display-entity": stmt((node) => csv(`sri.getFieldEntityValue(${nodeLit(node, false)})`)),
        "submit": stmt((node) => csv(headerTitle(node.parent))),
        "text-area": textWidget,
        "text-line": textWidget,
        "text-find": textWidget
    });

    return { macros, opaque: S.opaqueElements, prelude };
})
