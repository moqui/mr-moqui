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

// The xlsx render mode, the port of moqui-poi's ScreenWidgetRenderExcel and FormListExcelRender. Each form-list is a
// sheet: a header row of field titles and a row for each list entry. The render sends the workbook the sri.xlsx*
// calls fill (crates/moqui-core/src/screen/xlsx.rs). Nothing is written as text.
//
// The walk is the Java one: sections (with their conditions and actions), container-box bodies and container-row
// columns, plus the containers the csv mode walks. A cell is the values of the field's widgets: one value keeps its
// type (number, date, text), several are joined as text. A display, display-entity, link or label shows its text, its
// entity description, or the field value, else its default-text; any other widget is the field value.
//
// Differences from Java: a conditional-field whose condition holds is used, as in the csv mode, where Java read only
// the default-field; and a hidden or ignored field is an empty cell, so later cells stay under their headers.
(function (T, S) {
    "use strict";

    const { attr, has, jsString, child, childrenNamed, js, raw, stmt, noops } = T;
    const { nodeLit, expandExpr } = S;

    const MODE = "xlsx";

    // The widgets FormListExcelRender gives no value, as DefaultScreenMacros.csv.ftl.
    const IGNORED = ["ignored", "hidden", "submit", "image", "date-find", "file", "password", "range-find", "reset"];
    const TEXT_WIDGETS = ["display", "display-entity", "link", "label"];

    // The title of a header: the header-field's title, else the default-field's, else the field name made pretty by
    // camelCaseToPretty, as FormListExcelRender (the other modes split the name on capitals, as their FTL does).
    const headerTitle = (sub) => {
        let chosen = sub;
        if (sub.name === "header-field") {
            const field = sub.parent;
            const dflt = child(field, "default-field");
            if (attr(sub, "title")) chosen = sub;
            else if (dflt && attr(dflt, "title")) chosen = dflt;
        }
        const name = chosen.parent ? attr(chosen.parent, "name") : "";
        const text = attr(chosen, "title") ? expandExpr(attr(chosen, "title")) : `StringUtilities.camelCaseToPretty(${jsString(name)})`;
        return `ec.l10n.localize(${text})`;
    };

    // The value of one widget, as FormListExcelRender.getFieldValue: an expression.
    const widgetValueExpr = (widget) => {
        if (IGNORED.includes(widget.name)) return "null";
        const field = nodeLit(widget.parent.parent, false);
        let value;
        if (TEXT_WIDGETS.includes(widget.name)) {
            const defaultText = attr(widget, "default-text");
            if (attr(widget, "entity-name")) {
                // getFieldEntityValue gives the default-text itself when there is no value.
                value = `sri.getFieldEntityValue(${nodeLit(widget, false)})`;
            } else {
                if (attr(widget, "text")) {
                    const textMap = attr(widget, "text-map");
                    value = textMap ? `ec.resource.expand(${jsString(attr(widget, "text"))}, "", ${raw(textMap)})` : expandExpr(attr(widget, "text"));
                } else {
                    value = `sri.getFieldValue(${field}, "")`;
                }
                if (defaultText) value = `((v) => v == null ? ${expandExpr(defaultText)} : v)(${value})`;
            }
        } else {
            value = `sri.getFieldValue(${field}, "")`;
        }
        const condition = attr(widget, "condition");
        return condition ? `((${raw(condition)}) ? ${value} : null)` : value;
    };

    // The code for one cell from a field sub-node (default-field or conditional-field).
    const subCellCode = (sub) => {
        const right = attr(sub.parent, "align") === "right";
        if (S.widgetless(sub)) return `sri.xlsxCell([], [], false, ${right});\n`;
        const widgets = sub.children.filter((kid) => kid.name !== "set");
        // A number shows as currency when its one widget names a currency field or a format with cents.
        const only = widgets.length === 1 ? widgets[0] : null;
        const currency = !!only && (!!attr(only, "currency-unit-field") || (attr(only, "format") || "").includes(".00"));
        const values = widgets.map(widgetValueExpr).join(", ");
        const formats = widgets.map((widget) => jsString(attr(widget, "format") || "")).join(", ");
        return S.setCode(sub) + `sri.xlsxCell([${values}], [${formats}], ${currency}, ${right});\n`;
    };

    const cellCode = (field) => {
        const code = S.fieldChain(field, subCellCode);
        // A field with only conditional-fields and none that holds is still a cell.
        if (!child(field, "default-field") && childrenNamed(field, "conditional-field").length) {
            return `{\nlet $cells = sri.xlsxCellCount();\n${code}if (sri.xlsxCellCount() === $cells) sri.xlsxCell([], [], false, false);\n}\n`;
        }
        return code;
    };

    const formListMacro = stmt((node) => {
        const columns = S.formListColumns(node);
        const fields = [].concat(...columns);
        const headers = fields.map((field) => S.hideGuard(field, `sri.xlsxHeader(${headerTitle(S.headerSub(field))});\n`)).join("");
        const cells = fields.map((field) => S.hideGuard(field, cellCode(field))).join("");
        return js`
            {
                const $info = sri.openFormList(${jsString(attr(node, "name"))});
                sri.xlsxSheet(${S.expandNoL10nExpr(attr(node, "name"))});
                ${headers}
                for (let $index = 0; $index < $info.count; $index++) {
                    sri.startFormListRow($info, $index);
                    try {
                        ${cells}
                        sri.xlsxEndRow();
                    } finally {
                        sri.endFormListRow();
                    }
                }
                sri.safeCloseList($info);
            }
        `;
    });

    const children = stmt((node, kids) => kids);
    const named = (...names) => stmt((node) => names.map((name) => childrenNamed(node, name).map((kid) => kid.source || "").join("")).join(""));

    const macros = Object.assign({}, noops(S.allElements), S.anyMacros(MODE), {
        "screen": children,
        "widgets": children,
        "fail-widgets": children,
        "dynamic-form": children,
        "container": children,
        "container-box": named("box-body", "box-body-nopad"),
        "box-body": children,
        "box-body-nopad": children,
        "container-row": children,
        "row-col": children,
        "container-panel": named("panel-header", "panel-left", "panel-center", "panel-right", "panel-footer"),
        "panel-header": children,
        "panel-left": children,
        "container-split": named("split-side", "split-main"),
        "split-side": children,
        "split-main": children,
        "panel-center": children,
        "panel-right": children,
        "panel-footer": children,
        "section-include": stmt((node) => `sri.renderSectionInclude(${S.bareLit(node)});`),
        "form-list": formListMacro
    });

    return { macros, opaque: S.opaqueElements, prelude: "" };
})
