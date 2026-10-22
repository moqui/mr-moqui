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

// Helpers shared by the screen macro modules (ScreenText.js, ScreenCsv.js, ScreenXml.js, and the
// modes after them). This file is a function of the support object from TemplateSupport.js and
// returns the helper object S. A mode file is a function of (T, S) and returns
//   { macros, opaque, prelude }
// where macros has one function per element, opaque lists elements the walker does not descend into
// and prelude is the runtime code every generated unit starts with.
//
// A screen unit is one section's widgets (or fail-widgets). The generated code is a statement list
// that runs per request on the server runtime. It writes with sri.write and reads the request with
// sri, ec and bare names. Nothing here sees a request. Names the generated code declares start with $
// so they cannot hide a screen field.
(function (T) {
    "use strict";

    const { attr, has, jsString, child, childrenNamed, js, raw, stmt, noop } = T;

    // Every element a macro table must name, so an element that is missing is an error and an element
    // a mode does not render is a decision in that mode's file. The Java macros fall back to a no-op
    // for any element they do not define, so nothing is dropped by accident.
    const widgetElements = [
        "screen", "widgets", "fail-widgets", "subscreens-menu", "subscreens-active", "subscreens-panel",
        "section", "section-iterate", "section-include", "include-screen", "include-content", "render-mode", "text",
        "container", "container-box", "box-header", "box-toolbar", "box-body", "box-body-nopad",
        "container-row", "row-col", "container-panel", "panel-header", "panel-left", "panel-center",
        "panel-right", "panel-footer", "container-dialog", "dynamic-container", "dynamic-dialog", "button-menu",
        "tree", "tree-node", "tree-sub-node", "label", "link", "image", "parameter", "dialog",
        "render-dynamic-form", "dynamic-form", "form-list-user-columns", "context", "dashboard", "number-card",
        "chatter", "record-files", "container-split", "split-side", "split-main", "html-view", "status-bar", "markdown-view",
        "code-view"
    ];
    const formElements = [
        "form-single", "form-list", "row-actions", "field", "header-field", "first-row-field", "second-row-field",
        "last-row-field", "default-field", "conditional-field", "check", "date-find", "date-period", "date-time",
        "display", "display-entity", "drop-down", "dynamic-options", "editable", "file", "hidden", "ignored",
        "lookup", "password", "radio", "range-find", "reset", "submit", "text-area", "text-find", "text-line",
        "entity-options", "list-options", "option", "depends-on", "field-layout", "field-group", "field-accordion",
        "field-row", "field-row-big", "field-col-row", "field-col", "field-ref", "fields-not-referenced",
        "form-list-column", "columns", "column", "auto-fields-entity", "auto-fields-service", "auto-widget-entity",
        "auto-widget-service", "exclude", "entity-find", "set", "hidden-parameters", "row-selection",
        "visible-when", "editable-load", "action", "first-row", "second-row", "last-row", "entity-find-one", "view-board", "view-calendar", "view-tree"
    ];
    const allElements = widgetElements.concat(formElements);

    // Elements whose subtree a macro reads for itself, so the walker does not generate code for it.
    const opaqueElements = [
        "section", "section-iterate", "section-include", "entity-find", "row-actions", "entity-options",
        "list-options", "dynamic-options", "option", "depends-on", "set", "hidden-parameters", "field-layout",
        "columns", "form-list-column", "row-selection", "auto-fields-entity", "auto-fields-service",
        "auto-widget-entity", "auto-widget-service", "visible-when", "editable-load", "action", "tree",
        "dynamic-container", "dynamic-dialog", "button-menu", "include-content", "parameter"
    ];

    // The text of an html-view for the text and print modes, as an expression of (text, html): the text, else the HTML's
    // text (tags out, line breaks at blocks, the common entities read).
    const htmlViewTextFn = String.raw`((t, h) => { const v = t && t !== "null" ? String(t) : (h && h !== "null" ? String(h).replace(/<(style|script)[^>]*>[\s\S]*?<\/\1>/gi, " ").replace(/<br\s*\/?>/gi, "\n").replace(/<\/(p|div|li|tr|h[1-6]|blockquote)>/gi, "\n").replace(/<[^>]*>/g, "").replace(/&nbsp;/g, " ").replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, '"').replace(/&#39;/g, "'").replace(/&amp;/g, "&") : ""); return v.replace(/[ \t]+\n/g, "\n").replace(/\n{3,}/g, "\n\n").trim(); })`;
    // The text of a markdown-view for the html, text and print modes: the markdown as it is.
    const markdownTextExpr = (node) => `((t) => (t && t !== "null" ? String(t).trim() : ""))(ec.resource.expandNoL10n(${jsString(attr(node, "text") || "")}))`;
    const htmlViewTextExpr = (node) => `${htmlViewTextFn}(ec.resource.expandNoL10n(${jsString(attr(node, "text") || "")}), ec.resource.expandNoL10n(${jsString(attr(node, "html") || "")}))`;

    // The description of the current status of a status-bar for the text and print modes, as an expression: the StatusItem's
    // description, else the id; empty without a status.
    const statusTextExpr = (node) => `((id) => { const v = String(id === null || id === undefined ? "" : id).trim(); if (!v || v === "null") return ""; ` +
        `const item = ec.entity.find("moqui.basic.StatusItem").condition("statusId", v).useCache(true).disableAuthz().one(); ` +
        `return item && item.description ? String(item.description) : v; })(ec.resource.expandNoL10n(${jsString(attr(node, "status-id") || "")}))`;

    const expandExpr = (text) => `ec.resource.expand(${jsString(text)})`;
    const expandNoL10nExpr = (text) => `ec.resource.expandNoL10n(${jsString(text)})`;

    // The element as a JavaScript object literal: its name and attributes, text, children when asked and the chain of
    // parents with attributes only. The host reads these to find the field of a widget, a link's parameters
    // or the options under a drop-down.
    const nodeLit = (node, withChildren, withParent) => {
        const parts = [`"name":${jsString(node.name)}`, `"attrs":${JSON.stringify(node.attrs || {})}`];
        if (node.text) parts.push(`"text":${jsString(node.text)}`);
        if (withChildren && node.children.length) parts.push(`"children":[${node.children.map((kid) => nodeLit(kid, true, false)).join(",")}]`);
        if (node.parent && withParent !== false) parts.push(`"parent":${parentLit(node.parent)}`);
        return `{${parts.join(",")}}`;
    };
    const parentLit = (node) => {
        const parts = [`"name":${jsString(node.name)}`, `"attrs":${JSON.stringify(node.attrs || {})}`];
        if (node.parent) parts.push(`"parent":${parentLit(node.parent)}`);
        return `{${parts.join(",")}}`;
    };

    // A literal with no parent chain, for an element the host reads as a unit such as a section-include or set.
    const bareLit = (node) => {
        const parts = [`"name":${jsString(node.name)}`, `"attrs":${JSON.stringify(node.attrs || {})}`];
        if (node.text) parts.push(`"text":${jsString(node.text)}`);
        return `{${parts.join(",")}}`;
    };

    const write = (expression) => `sri.write(${expression});\n`;

    const indent = (text, spaces) => {
        const pad = " ".repeat(spaces == null ? 4 : spaces);
        return text.split("\n").map((line) => line === "" ? line : pad + line).join("\n");
    };

    // The widgets of a field sub-node (default-field and so on): the child elements' code.
    const widgetsSource = (sub) => sub.children.map((kid) => kid.source || "").join("");

    // Code that runs the set elements of a field sub-node before its widgets.
    const setCode = (sub) => childrenNamed(sub, "set").map((setNode) => `sri.setInContext(${bareLit(setNode)});\n`).join("");

    // Whether a field sub-node renders nothing in the text modes: it holds an ignored, hidden or
    // submit widget, or its field has hide="true".
    const widgetless = (sub) => ["ignored", "hidden", "submit"].some((name) => child(sub, name)) ||
        (sub.parent && attr(sub.parent, "hide") === "true");

    // The code of a field: a conditional-field whose condition holds, else the default-field.
    // emit(sub) returns the code for one sub-node.
    const fieldChain = (field, emit) => {
        const conds = childrenNamed(field, "conditional-field");
        const dflt = child(field, "default-field");
        const dfltCode = dflt ? emit(dflt) : "";
        if (!conds.length) return dfltCode;
        let code = "";
        conds.forEach((sub, index) => {
            const head = index === 0 ? "if" : "} else if";
            code += js`
                ${head} (${raw(attr(sub, "condition") || "false")}) {
                    ${emit(sub)}
            `;
        });
        if (dfltCode) {
            code += js`
                } else {
                    ${dfltCode}
            `;
        }
        return code + "}\n";
    };

    // Code for a list field that its hide attribute may remove: the condition is checked when the code runs.
    const hideGuard = (field, code) => {
        const hide = attr(field, "hide");
        if (!hide || !code) return code;
        return js`
            if (!(${raw(hide)})) {
                ${code}
            }
        `;
    };

    // The title of a field sub-node: its title attribute, else the field name split into capitalized words.
    // Static text, localized when the code runs.
    const titleText = (sub) => {
        if (attr(sub, "title")) return attr(sub, "title");
        const name = sub.parent ? attr(sub.parent, "name") : "";
        return name.split(/(?=[A-Z])/).map((word) => (word.charAt(0).toUpperCase() + word.slice(1)).replace("Id", "ID")).join(" ");
    };

    const localizedTitle = (sub) => `ec.l10n.localize(${jsString(titleText(sub))})`;

    // A literal for a field and its form, for sri.getFieldValueString and the other field methods.
    const fieldLit = (widget) => nodeLit(widget.parent.parent, false);

    // The statements that render a text element: a file or inline text, with boundary comments when the
    // screen facade asks for them. Inline text with template true is a trusted fancy template, compiled once
    // and cached by its text. A text file is rendered by the renderer registered for its extension.
    const renderTextCode = (node) => {
        const template = attr(node, "template");
        const label = template || "true";
        const encode = attr(node, "encode") === "true";
        const boundary = attr(node, "no-boundary-comment") !== "true";
        let code = "";
        if (attr(node, "location")) {
            const call = `sri.renderText($location, ${jsString(template)})`;
            const comment = (word) => `if (sri.doBoundaryComments()) sri.write(${jsString(`<!-- ${word} render-mode.text[@location=`)} + $location + ${jsString(`][@template=${label}] -->`)});`;
            const begin = boundary ? comment("BEGIN") : "";
            const end = boundary ? comment("END  ") : "";
            code += js`
                {
                    const $location = ${expandNoL10nExpr(attr(node, "location"))};
                    ${begin}
                    sri.write(${encode ? `sri.encodeHtml(${call})` : call});
                    ${end}
                }
            `;
        }
        const inline = node.text || "";
        if (inline.trim() !== "") {
            const marker = `render-mode.text[inline][@template=${label}]`;
            const isTemplate = !template || template === "true";
            const text = isTemplate ? `sri.renderInlineText(${jsString(inline)})` : jsString(inline);
            const value = encode && !isTemplate ? `sri.encodeHtml(${text})` : text;
            const begin = boundary ? `if (sri.doBoundaryComments()) sri.write(${jsString(`<!-- BEGIN ${marker} -->`)});` : "";
            const end = boundary ? `if (sri.doBoundaryComments()) sri.write(${jsString(`<!-- END   ${marker} -->`)});` : "";
            code += js`
                ${begin}
                sri.write(${value});
                ${end}
            `;
        }
        return code;
    };

    // The macros every mode shares: include-screen, render-mode, text and the subscreen and section calls.
    const anyMacros = (mode) => {
        // A text type list names the mode; xsl-fo is the older name of html-print.
        const typeMatches = (type) => type.split(",").map((name) => name.trim()).map((name) => name === "xsl-fo" ? "html-print" : name).includes(mode);
        return {
            "include-screen": stmt((node) =>
                `sri.renderIncludeScreen(${jsString(attr(node, "location"))}, ${jsString(attr(node, "share-scope"))});`),
            "render-mode": stmt((node) => {
                const texts = childrenNamed(node, "text");
                if (!texts.length) return "";
                let chosen = null;
                texts.forEach((kid) => { if (!attr(kid, "type") || attr(kid, "type") === "any") chosen = kid; });
                texts.forEach((kid) => { if (attr(kid, "type") && typeMatches(attr(kid, "type"))) chosen = kid; });
                return chosen ? renderTextCode(chosen) : "";
            }),
            "text": stmt((node) => {
                const type = attr(node, "type");
                return !type || typeMatches(type) ? renderTextCode(node) : "";
            }),
            // A dynamic form is assembled when it renders: the call runs it, and its code is made then with the
            // dynamic-form element around the assembled form.
            "render-dynamic-form": stmt((node) => `sri.renderDynamicForm(${jsString(attr(node, "name"))});`),
            // A form-list whose columns the user may choose: with a saved choice the render assembles it with those
            // columns, and the code inside is the form as defined.
            "form-list-user-columns": stmt((node, children) =>
                `if (!sri.renderUserColumnsForm(${jsString(attr(node, "name"))})) {\n${children}}\n`),
            "subscreens-menu": noop,
            "subscreens-active": stmt(() => "sri.renderSubscreen();"),
            "subscreens-panel": stmt(() => "sri.renderSubscreen();"),
            "section": stmt((node) => `sri.renderSection(${jsString(attr(node, "name"))});`),
            "section-iterate": stmt((node) => `sri.renderSection(${jsString(attr(node, "name"))});`),
            // An element no macro names, Java's @element: the walker logs it and renders this instead of failing, and the
            // element's children are not rendered. html and qvt show Java's placeholder, xml writes it as a comment, and the
            // other modes write nothing.
            "@element": stmt((node) => unknownElementCode(mode, node.name)),
        };
    };

    // The placeholder of an element no macro names, as the Java macros' @element writes it.
    const unknownElementCode = (mode, name) => {
        const text = "Doing nothing for element " + name + ", not yet implemented.";
        if (mode === "html" || mode === "qvt") return `sri.write(${jsString("<p>=== " + text + " ===</p>")});`;
        if (mode === "xml") return `sri.write(${jsString("<!-- " + text + " -->")});`;
        return "";
    };

    // The text of a display widget: the expanded text, or the field value, with currency when asked.
    // Shared by the text, csv and xml modes, which differ in how they present the result. The result is
    // the code of an expression.
    const displayValueExpr = (widget) => {
        const field = nodeLit(widget.parent.parent, false);
        const textMap = attr(widget, "text-map");
        const currency = attr(widget, "currency-unit-field");
        const hide = attr(widget, "currency-hide-symbol") === "true";
        const format = hide ? "formatCurrencyNoSymbol" : "formatCurrency";
        let base;
        if (attr(widget, "text")) {
            base = textMap ? `ec.resource.expand(${jsString(attr(widget, "text"))}, "", ${raw(textMap)})` : expandExpr(attr(widget, "text"));
        } else if (currency) {
            return `ec.l10n.${format}(sri.getFieldValue(${field}, ""), ${raw(currency)})`;
        } else {
            return `sri.getFieldValueString(${nodeLit(widget, false)})`;
        }
        if (currency) return `ec.l10n.${format}(${base}, ${raw(currency)})`;
        return base;
    };

    // Code for the text a link shows: an entity value, or the text with its text-map. A link whose
    // condition fails, or that is not an anchor in a field, shows nothing.
    const linkTextExpr = (node) => {
        if (attr(node, "entity-name")) return `sri.getFieldEntityValue(${nodeLit(node, false)})`;
        const textMap = attr(node, "text-map");
        if (textMap) return `ec.resource.expand(${jsString(attr(node, "text"))}, "", ${raw(textMap)})`;
        return expandExpr(attr(node, "text"));
    };

    // The default pattern of a date-time widget by type.
    const dateTimeFormat = (widget) => {
        if (attr(widget, "format")) return attr(widget, "format");
        if (attr(widget, "type") === "time") return "HH:mm";
        if (attr(widget, "type") === "date") return "yyyy-MM-dd";
        return "yyyy-MM-dd HH:mm";
    };

    // Whether a list field never renders: its default-field and each conditional-field hold only hidden or ignored widgets.
    const listFieldHiddenWidget = (field) => {
        const dflt = child(field, "default-field");
        if (dflt && !child(dflt, "hidden") && !child(dflt, "ignored")) return false;
        return !childrenNamed(field, "conditional-field").some((sub) => !child(sub, "hidden") && !child(sub, "ignored"));
    };

    // The columns of a form-list as arrays of field nodes: the columns element of the type (a _uiType such as mobile) when
    // one is asked and the form has it, else the untyped columns element, else the form-list-column elements, else one
    // column for each field that is not hidden.
    const formListColumns = (form, type) => {
        const fields = {};
        childrenNamed(form, "field").forEach((field) => { fields[attr(field, "name")] = field; });
        let columnNodes = null;
        const columns = (type ? childrenNamed(form, "columns").find((node) => attr(node, "type") === type) : null) ||
            childrenNamed(form, "columns").find((node) => !attr(node, "type"));
        if (columns) columnNodes = childrenNamed(columns, "column");
        if (!columnNodes) columnNodes = childrenNamed(form, "form-list-column");
        if (columnNodes.length) {
            const out = [];
            columnNodes.forEach((column) => {
                const list = [];
                childrenNamed(column, "field-ref").forEach((ref) => {
                    const field = fields[attr(ref, "name")];
                    if (!field) throw new Error(`field ${attr(ref, "name")} in form-list-column is not a field of form ${attr(form, "name")}`);
                    if (!listFieldHiddenWidget(field)) list.push(field);
                });
                if (list.length) out.push(list);
            });
            return out;
        }
        return childrenNamed(form, "field").filter((field) => !listFieldHiddenWidget(field)).map((field) => [field]);
    };

    // The sub-node that gives a list field its header: header-field, else default-field, else the first conditional-field.
    const headerSub = (field) => child(field, "header-field") || child(field, "default-field") || childrenNamed(field, "conditional-field")[0];

    // The code of a condition that turns a part of a form-list off for a render (row-selection condition, a view's move-condition):
    // true when it holds. A condition that fails to evaluate is logged naming the part and does not hold, so the part is left out.
    const heldCode = (expr, what) => `(() => {\ntry {\nreturn !!(${raw(expr)});\n} catch ($err) {\nec.logger.error(${jsString(`Leaving the ${what} out, its condition failed: `)} + $err);\nreturn false;\n}\n})()`;

    const noops = (names) => T.noops(names);

    return {
        allElements, opaqueElements, unknownElementCode, expandExpr, expandNoL10nExpr, htmlViewTextFn, htmlViewTextExpr, markdownTextExpr, statusTextExpr, nodeLit, bareLit, write, indent, widgetsSource,
        setCode, widgetless, fieldChain, hideGuard, titleText, localizedTitle, fieldLit, renderTextCode, anyMacros,
        displayValueExpr, linkTextExpr, dateTimeFormat, listFieldHiddenWidget, formListColumns, headerSub, heldCode, noops
    };
})
