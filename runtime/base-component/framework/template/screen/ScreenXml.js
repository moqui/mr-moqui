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

// The xml render mode. This follows DefaultScreenMacros.xml.ftl: the screen is an element named for it, a
// form-list is an element of entries whose attributes are the displayed fields, a form-single is ignored,
// and elements it does not name (labels and section-include among them) are no-ops.
//
// Differences from the FreeMarker macros: a date-time is formatted with the pattern or the type's default.
(function (T, S) {
    "use strict";

    const { attr, has, jsString, child, js, raw, stmt, noops } = T;
    const { nodeLit, fieldChain, widgetsSource, setCode, widgetless, expandExpr } = S;

    const MODE = "xml";

    const prelude = `const $attr = (value) => StringUtilities.encodeForXmlAttribute(String(value));
`;

    const attribute = (expression) => `sri.write($attr(${expression}));\n`;

    const optionWidget = stmt((node) => {
        const key = has(node, "no-current-selected-key") ? jsString(attr(node, "no-current-selected-key")) : '""';
        return js`
            {
                const $options = sri.getFieldOptions(${nodeLit(node, true)});
                let $current = sri.getFieldValueString(${nodeLit(node, false)});
                if (!$current) $current = ${key};
                const $text = $options.get($current);
                sri.write($attr($text != null ? $text : $current));
            }
        `;
    });

    // The text of a display widget: the expanded text with a currency of two fraction digits, or the field value.
    const displayExpr = (node) => {
        const currency = attr(node, "currency-unit-field");
        const format = attr(node, "currency-hide-symbol") === "true" ? "formatCurrencyNoSymbol" : "formatCurrency";
        if (attr(node, "text")) {
            const text = expandExpr(attr(node, "text"));
            return currency ? `ec.l10n.${format}(${text}, ${raw(currency)}, 2)` : text;
        }
        if (currency) return `ec.l10n.${format}(sri.getFieldValue(${S.fieldLit(node)}, ""), ${raw(currency)}, 2)`;
        return `sri.getFieldValueString(${nodeLit(node, false)})`;
    };

    // One attribute of a list entry: the field name and the value its widgets write.
    const attributeCode = (field) => fieldChain(field, (sub) => {
        if (widgetless(sub)) return "";
        return js`
            ${setCode(sub)}
            sri.write(${jsString(" " + attr(field, "name") + '="')});
            ${widgetsSource(sub)}
            sri.write('"');
        `;
    });

    const formListMacro = stmt((node) => {
        const name = attr(node, "name");
        const fields = [].concat(...S.formListColumns(node)).map((field) => S.hideGuard(field, attributeCode(field))).join("");
        return js`
            {
                const $info = sri.openFormList(${jsString(name)});
                sri.write(${jsString("    <" + name + ">\n")});
                for (let $index = 0; $index < $info.count; $index++) {
                    sri.write(${jsString("        <" + name + "Entry")});
                    sri.startFormListRow($info, $index);
                    try {
                        ${fields}
                        sri.write("/>\\n");
                    } finally {
                        sri.endFormListRow();
                    }
                }
                sri.safeCloseList($info);
                sri.write(${jsString("    </" + name + ">\n")});
            }
        `;
    });

    const textWidget = stmt((node) => attribute(`sri.getFieldValueString(${nodeLit(node, false)})`));

    const reportWidget = stmt((node) => {
        const body = `sri.write(${S.reportRenderExpr(node, "xml")});\n`;
        return attr(node, "condition") ? `if (${attr(node, "condition")}) {\n${body}}\n` : body;
    });

    const macros = Object.assign({}, noops(S.allElements), S.anyMacros(MODE), {
        "report-panel": reportWidget,
        "report-dashboard": reportWidget,
        "report-embed": reportWidget,
        "screen": stmt((node, children) => children),
        "widgets": stmt((node, children) => {
            const root = node.parent && node.parent.name === "screen";
            return root ? js`
                sri.write("<" + sri.getActiveScreenDef().getScreenName() + ">\\n");
                ${children}
                sri.write("</" + sri.getActiveScreenDef().getScreenName() + ">\\n");
            ` : js`
                sri.write("\\n");
                ${children}
                sri.write("\\n");
            `;
        }),
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
        "container-dialog": stmt((node, children) => children),
        "link": stmt((node) => node.parent && node.parent.name.includes("-field") ?
            `sri.write(${expandExpr(attr(node, "text"))});` : ""),
        "form-list": formListMacro,
        "field": stmt((node, children) => children),
        "conditional-field": stmt((node, children) => children),
        "default-field": stmt((node, children) => children),
        "header-field": stmt((node, children) => children),

        "check": optionWidget,
        "drop-down": optionWidget,
        "radio": optionWidget,
        "date-time": stmt((node) => attribute(
            `sri.getFieldValueString(${S.fieldLit(node)}, ${jsString(attr(node, "default-value"))}, ${jsString(S.dateTimeFormat(node))}, ${jsString(attr(node, "type") || "date-time")})`)),
        "display": stmt((node) => attribute(displayExpr(node))),
        "display-entity": stmt((node) => attribute(`sri.getFieldEntityValue(${nodeLit(node, false)})`)),
        "submit": stmt((node) => attribute(`ec.l10n.localize(${jsString(S.titleText(node.parent))})`)),
        "text-area": textWidget,
        "text-line": textWidget,
        "text-find": textWidget
    });

    return { macros, opaque: S.opaqueElements, prelude };
})
