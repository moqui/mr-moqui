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
        "render-dynamic-form", "dynamic-form", "form-list-user-columns", "context", "dashboard", "number-card", "nav-card",
        "chatter", "record-files", "container-split", "split-side", "split-main", "html-view", "status-bar", "markdown-view",
        "code-view", "report-panel", "report-dashboard", "report-embed"
    ];
    const formElements = [
        "form-single", "form-list", "row-actions", "field", "header-field", "first-row-field", "second-row-field",
        "last-row-field", "default-field", "conditional-field", "check", "date-find", "date-period", "date-time",
        "display", "display-entity", "drop-down", "dynamic-options", "editable", "file", "hidden", "ignored",
        "lookup", "password", "radio", "range-find", "reset", "submit", "text-area", "text-find", "text-line", "scan-input",
        "entity-options", "list-options", "option", "depends-on", "field-layout", "field-group", "field-accordion",
        "field-row", "field-row-big", "field-col-row", "field-col", "field-ref", "fields-not-referenced",
        "form-list-column", "columns", "column", "auto-fields-entity", "auto-fields-service", "auto-widget-entity",
        "auto-widget-service", "exclude", "entity-find", "set", "hidden-parameters", "row-selection",
        "visible-when", "require-when", "editable-load", "action", "first-row", "second-row", "last-row", "entity-find-one", "view-board", "view-calendar", "view-tree"
    ];
    const allElements = widgetElements.concat(formElements);

    // Elements whose subtree a macro reads for itself, so the walker does not generate code for it.
    const opaqueElements = [
        "section", "section-iterate", "section-include", "entity-find", "row-actions", "entity-options",
        "list-options", "dynamic-options", "option", "depends-on", "set", "hidden-parameters", "field-layout",
        "columns", "form-list-column", "row-selection", "auto-fields-entity", "auto-fields-service",
        "auto-widget-entity", "auto-widget-service", "visible-when", "require-when", "editable-load", "action", "tree",
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

    // Report attributes holding data are expressions, just like a form's list. The server computes and authorizes
    // frames before a widget renders; no renderer queries a dataset or executes transformations itself.
    // A report-embed: the saved dashboard or library panel definition-id, rendered for the viewer as report-dashboard props, or
    // null. Nothing renders without a signed-in viewer (an anonymous visitor of a public site sees nothing), and a definition
    // or query the viewer may not read logs a warning and renders nothing, so the rest of the screen is unaffected.
    const reportEmbedFn = String(function (ec, definitionId, panelId, from, thru, variables) {
        if (!definitionId || !ec.user.userId) return null;
        try {
            const rendered = ec.service.sync().name("org.moqui.report.ReportServices.render#Dashboard").throwOnError(true).parameters({definitionId,
                panelId: panelId || undefined, from: from || "now-7d", thru: thru || "now", variables: variables || {}, filters: []}).call().dashboard;
            const dashboard = rendered;
            const frames = {};
            for (const [id, panel] of Object.entries(dashboard.panels || {})) frames[id] = panel.frames || [];
            Object.assign(dashboard, {definitionId, embedded: true, absoluteTime: dashboard.range, requestedTime: {from: from || "now-7d", thru: thru || "now"},
                viewerId: ec.user.userId, screenUrl: "/apps/dashboards/Dashboard", panelId: panelId || null});
            return {dashboard, frames};
        } catch (error) {
            ec.logger.warn("report-embed " + definitionId + " is not shown: " + error);
            return null;
        }
    });
    const reportEmbedExpr = (node) => `(${reportEmbedFn})(ec,${expandNoL10nExpr(attr(node, "definition-id") || "")},${expandNoL10nExpr(attr(node, "panel-id") || "")},` +
        `${expandNoL10nExpr(attr(node, "from") || "")},${expandNoL10nExpr(attr(node, "thru") || "")},(${attr(node, "variables") || "{}"}))`;
    const reportPropsExpr = (node) => node.name === "report-embed" ? reportEmbedExpr(node) : node.name === "report-dashboard" ?
        `{dashboard:(${attr(node, "dashboard") || "{}"}),frames:(${attr(node, "frames") || "{}"})}` :
        `{frame:(${attr(node, "frame") || "{fields:[]}"}),panel:(${attr(node, "panel") || "{}"}),title:${expandExpr(attr(node, "title") || "")}}`;
    const reportDataFn = String(function (props, mode, l10n) {
        if (!props) return "";
        const escape = (value) => String(value == null ? "" : value).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
        const cell = (value) => value == null ? "—" : typeof value === "object" && typeof value.toNumber !== "function" ? JSON.stringify(value) : String(value);
        const csv = (value, field) => {
            let text = value == null ? "" : cell(value);
            const numeric = field && field.type === "number" && /^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?$/i.test(text);
            if (typeof value === "string" && !numeric && /^[=+@\-\t\r]/.test(text)) text = "'" + text;
            return '"' + text.replace(/"/g, '""') + '"';
        };
        // Only typed presentation values are formatted. Data exports below keep the original frame cells.
        const configured = (field, definition) => {
            const settings = definition && definition.fieldConfig || {};
            let result = Object.assign({}, settings.defaults || {}, field);
            for (const override of settings.overrides || []) {
                const match = override.match || {};
                let matches = match.name ? field.name === match.name : match.type ? field.type === match.type : false;
                if (match.regex && String(match.regex).length <= 256) {
                    try { matches = new RegExp(match.regex).test(field.name); } catch (_) { matches = false; }
                }
                if (matches) result = Object.assign(result, override.config || {});
            }
            return Object.assign(result, result.config || {});
        };
        const shown = (value, config, row) => {
            if (value == null) return config.noValue || "—";
            const raw = cell(value);
            const numeric = typeof value !== "boolean" && raw.trim() !== "" && Number.isFinite(Number(raw));
            for (const mapping of config.mappings || []) {
                if ((mapping.value !== undefined && String(mapping.value) === raw) ||
                        (numeric && mapping.from !== undefined && Number(raw) >= mapping.from && Number(raw) <= mapping.to)) return mapping.text;
            }
            try {
                if (config.type === "time" || config.kind === "time") {
                    // Passing a Date retains the instant across the script bridge; l10n uses the viewer's zone.
                    const date = new Date(/^[+-]?\d+$/.test(raw) ? Number(raw) : value);
                    return Number.isNaN(date.getTime()) ? raw : l10n.format(date, "yyyy-MM-dd HH:mm:ss.SSS z");
                }
                if (!numeric || !["number", "integer"].includes(config.type || config.kind)) return raw;
                const unit = Array.isArray(config.units) && config.units[row] || config.unit || "";
                let number = Number(raw), suffix = "";
                if (unit === "duration" || unit === "ns") { number /= 1e6; suffix = " ms"; }
                else if (unit === "ms" || unit === "s") suffix = " " + unit;
                else if (unit === "percent" || unit === "%") suffix = "%";
                else if (unit === "bytes") {
                    const units = [" B", " KiB", " MiB", " GiB", " TiB"];
                    let at = 0;
                    while (Math.abs(number) >= 1024 && at < units.length - 1) { number /= 1024; at++; }
                    suffix = units[at];
                }
                const supplied = Number(config.decimals);
                const decimals = config.decimals == null || !Number.isFinite(supplied)
                    ? (Number.isInteger(number) ? 0 : 2) : Math.min(12, Math.max(0, Math.trunc(supplied)));
                if (/^(currency:)?[A-Z]{3}$/.test(unit)) {
                    return l10n.formatCurrency(value, unit.replace(/^currency:/, ""), config.decimals == null ? 2 : decimals);
                }
                return l10n.format(number, "#,##0" + (decimals ? "." + "0".repeat(decimals) : "")) + suffix;
            } catch (_) { return raw; }
        };
        const panel = (frame, definition, title) => {
            const fields = frame && frame.fields || [];
            const length = fields.reduce((n, field) => Math.max(n, (field.values || []).length), 0);
            const rows = Array.from({length}, (_, at) => fields.map(field => (field.values || [])[at] ?? null));
            title = title || definition && definition.title || "Report";
            if (mode === "csv") return [fields.map(field => csv(field.name)).join(","), ...rows.map(row => row.map((value, at) => csv(value, fields[at])).join(","))].join("\r\n") + "\r\n";
            if (mode === "xml") return "<report title=\"" + escape(title) + "\">" + rows.map(row => "<row>" + row.map((value, at) => "<field name=\"" + escape(fields[at].name) + "\">" + escape(cell(value)) + "</field>").join("") + "</row>").join("") + "</report>";
            if (mode === "text") return title + "\n" + (length ? [fields.map(field => field.name), ...rows.map(row => row.map(cell))].map(row => row.join("\t")).join("\n") : "No data in this time range") + "\n";
            const displayFields = fields.map(field => configured(field, definition));
            return "<section class=\"report-panel\"><h3>" + escape(title) + "</h3>" + (length ? "<table><thead><tr>" + fields.map(field => "<th>" + escape(field.name) + "</th>").join("") + "</tr></thead><tbody>" + rows.map((row, index) => "<tr>" + row.map((value, at) => "<td>" + escape(shown(value, displayFields[at], index)) + "</td>").join("") + "</tr>").join("") + "</tbody></table>" : "<p>No data in this time range</p>") + "</section>";
        };
        if (!props.dashboard) return panel(props.frame, props.panel, props.title);
        const panels = props.dashboard.panels || {};
        const entries = Array.isArray(panels) ? panels.map((value, at) => [value.id || String(at), value]) : Object.entries(panels);
        return entries.map(([id, definition]) => panel((props.frames[id] || [])[0], definition)).join(mode === "csv" ? "\r\n" : "\n");
    });
    const reportRenderExpr = (node, mode) => `(${reportDataFn})(${reportPropsExpr(node)},${jsString(mode)},ec.l10n)`;

    // The literals of the unit being generated, while the host has asked for them (beginLits). Each distinct literal is
    // made once, when the unit is compiled, as $lits[i]: the host converts it once (sri.holdNodeLit) and it is frozen,
    // where an inline literal was made and converted again on every call and every form-list row.
    let lits = null;
    const hoist = (text) => {
        if (!lits) return text;
        let index = lits.index.get(text);
        if (index === undefined) {
            index = lits.list.length;
            lits.list.push(text);
            lits.index.set(text, index);
        }
        return `$lits[${index}]`;
    };
    const beginLits = () => { lits = { list: [], index: new Map() }; };
    const endLits = () => { const list = lits ? lits.list : []; lits = null; return list; };

    // The element as a JavaScript object literal: its name and attributes, text, children when asked and the chain of
    // parents with attributes only. The host reads these to find the field of a widget, a link's parameters
    // or the options under a drop-down.
    const nodeLit = (node, withChildren, withParent) => hoist(nodeLitText(node, withChildren, withParent));
    const nodeLitText = (node, withChildren, withParent) => {
        const parts = [`"name":${jsString(node.name)}`, `"attrs":${JSON.stringify(node.attrs || {})}`];
        if (node.text) parts.push(`"text":${jsString(node.text)}`);
        if (withChildren && node.children.length) parts.push(`"children":[${node.children.map((kid) => nodeLitText(kid, true, false)).join(",")}]`);
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
        return hoist(`{${parts.join(",")}}`);
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
    // A hidden widget beside a visible one (a link and the hidden value its row form posts) leaves the sub-node shown.
    const hasShownWidget = (sub) => sub.children.some((kid) => kid.name && !["hidden", "ignored", "submit", "set"].includes(kid.name));
    const widgetless = (sub) => ["ignored", "submit"].some((name) => child(sub, name)) ||
        (child(sub, "hidden") && !hasShownWidget(sub)) || (sub.parent && attr(sub.parent, "hide") === "true");

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

    // a title with ${} (a list header like "0 - ${periodDays} days") is expanded, as ScreenHtml does; expand localizes the
    // text before it expands it
    const localizedTitle = (sub) => {
        const text = titleText(sub);
        return text.includes("${") ? expandExpr(text) : `ec.l10n.localize(${jsString(text)})`;
    };

    // A literal for a field and its form, for sri.getFieldValueString and the other field methods.
    const fieldLit = (widget) => nodeLit(widget.parent.parent, false);

    // The statements that render a text element: a file or inline text, with boundary comments when the
    // screen facade asks for them. Inline text with template true is a trusted fancy template, compiled once
    // and cached by its text. A text file is rendered by the renderer registered for its extension. A template
    // writes as it renders (renderText, writeInlineText), so a screen it renders keeps its place in its text and
    // the page streams; one to encode is rendered for its value and encoded.
    const renderTextCode = (node) => {
        const template = attr(node, "template");
        const label = template || "true";
        const encode = attr(node, "encode") === "true";
        const boundary = attr(node, "no-boundary-comment") !== "true";
        let code = "";
        if (attr(node, "location")) {
            const call = `sri.${encode ? "renderTextValue" : "renderText"}($location, ${jsString(template)})`;
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
            const value = encode && !isTemplate ? `sri.encodeHtml(${jsString(inline)})` : jsString(inline);
            const written = isTemplate ? `sri.writeInlineText(${jsString(inline)});` : `sri.write(${value});`;
            const begin = boundary ? `if (sri.doBoundaryComments()) sri.write(${jsString(`<!-- BEGIN ${marker} -->`)});` : "";
            const end = boundary ? `if (sri.doBoundaryComments()) sri.write(${jsString(`<!-- END   ${marker} -->`)});` : "";
            code += js`
                ${begin}
                ${written}
                ${end}
            `;
        }
        return code;
    };

    // Self-contained so the static renderer can embed the same chooser in its isolated runtime.
    const renderModeHelpers = function () {
        const renderModeTypeMatches = (type, mode) => {
            const canonical = (name) => name === "xsl-fo" ? "html-print" : name;
            return String(type || "").split(",").map((name) => canonical(name.trim())).includes(canonical(mode));
        };
        // A print-only screen: its widgets are render-mode elements alone, as SimpleScreens' Facility/PrintBoxTypes. A screen
        // with other widgets keeps its html-print blocks (a print header, a page style) out of its html render.
        const printOnlyScreen = (node) => {
            const widgets = node.parent;
            return !!widgets && widgets.name === "widgets" && !!widgets.parent && widgets.parent.name === "screen"
                && (widgets.children || []).every((child) => child.name === "render-mode");
        };
        // An html-print text html can show: inline, or from a location that is an HTML template (not a FO one such as a
        // configured .xsl-fo.ftl, nor an expression whose file is not known here). xsl-fo texts are FO and never qualify.
        const HTML_LOCATION = /\.html?(\.|#|$)/i;
        const htmlPrintText = (text) => {
            const attrs = text.attrs || {};
            if (!String(attrs.type || "").split(",").some((name) => name.trim() === "html-print")) return false;
            const location = String(attrs.location || "");
            return !location || (!location.includes("${") && HTML_LOCATION.test(location));
        };
        // Last explicit match wins over the last fallback, regardless of their document order. An html render of a
        // print-only screen with neither takes the last html-print HTML text, so the screen opened in a browser shows its
        // content rather than a blank page (Java renders nothing there). Other modes do not: in qvt the print page's
        // styles would reach the whole app, and the browser apps open a print screen through /fop or renderMode=html-print.
        const chooseRenderModeText = (node, mode) => {
            let fallback = null;
            let matching = null;
            let print = null;
            (node.children || []).filter((text) => text.name === "text").forEach((text) => {
                const type = (text.attrs || {}).type;
                if (!type || type === "any") fallback = text;
                else if (renderModeTypeMatches(type, mode)) matching = text;
                else if (mode === "html" && htmlPrintText(text)) print = text;
            });
            return matching || fallback || (print && printOnlyScreen(node) ? print : null);
        };
        // Where the ${...} expression that starts at `from` ends: the index of its closing } outside brackets, strings, template literals,
        // comments and regular expressions, or -1. A / starts a regular expression where an operand may come (at the start, after a
        // punctuator other than ) and ], or after a keyword such as return; a word after . or ?. is a property name) and divides everywhere
        // else, as after a postfix ++ or --. As the host's scan_expression.
        const OPERAND_KEYWORDS = new Set(["return", "typeof", "instanceof", "in", "of", "new", "delete", "void", "throw", "case", "do", "else", "yield", "await"]);
        // The patterns are made once: a regular expression literal in a function is a new object at each evaluation, and
        // these are tested at each character.
        const WORD = /[A-Za-z0-9_$\u0080-\uffff]/, SPACE = /\s/, FLAG = /[A-Za-z]/;
        const regexEnd = (text, start) => {
            let j = start + 1;
            let inClass = false;
            for (; j < text.length; j++) {
                const ch = text.charAt(j);
                if (ch === "\n" || ch === "\r") return -1;
                if (ch === "\\") j++;
                else if (ch === "[") inClass = true;
                else if (ch === "]") inClass = false;
                else if (ch === "/" && !inClass) break;
            }
            if (j >= text.length) return -1;
            j++;
            while (j < text.length && FLAG.test(text.charAt(j))) j++;
            return j;
        };
        const exprEnd = (text, from) => {
            let depth = 0;
            let operandNext = true;
            let propertyNext = false;
            let regex = -1;
            let j = from;
            while (j < text.length) {
                const ch = text.charAt(j);
                const next = text.charAt(j + 1);
                const property = propertyNext;
                const space = SPACE.test(ch);
                if (!space) propertyNext = ch === ".";
                if (ch === "'" || ch === '"') {
                    for (j++; j < text.length && text.charAt(j) !== ch; j++) if (text.charAt(j) === "\\") j++;
                    operandNext = false;
                } else if (ch === "`") {
                    for (j++; j < text.length && text.charAt(j) !== "`"; j++) {
                        if (text.charAt(j) === "\\") j++;
                        else if (text.charAt(j) === "$" && text.charAt(j + 1) === "{") {
                            j = exprEnd(text, j + 2);
                            if (j < 0) return -1;
                        }
                    }
                    operandNext = false;
                } else if (ch === "/" && next === "/") {
                    const nl = text.indexOf("\n", j);
                    j = nl < 0 ? text.length : nl;
                    continue;
                } else if (ch === "/" && next === "*") {
                    const close = text.indexOf("*/", j + 2);
                    if (close < 0) return -1;
                    j = close + 2;
                    continue;
                } else if (ch === "/" && operandNext && (regex = regexEnd(text, j)) >= 0) {
                    j = regex;
                    operandNext = false;
                    continue;
                } else if (WORD.test(ch)) {
                    const start = j;
                    while (j < text.length && WORD.test(text.charAt(j))) j++;
                    operandNext = !property && OPERAND_KEYWORDS.has(text.slice(start, j));
                    continue;
                } else if (space) {
                    // whitespace leaves what may come next as it was
                } else if ((ch === "+" || ch === "-") && next === ch) {
                    // a postfix ++ or -- leaves an operand, a prefix one an operator: what may come next is as it was
                    j += 2;
                    continue;
                } else if (ch === "(" || ch === "[" || ch === "{") {
                    depth++;
                    operandNext = true;
                } else if (ch === ")" || ch === "]") {
                    depth = Math.max(0, depth - 1);
                    operandNext = false;
                } else if (ch === "}") {
                    if (depth === 0) return j;
                    depth--;
                    operandNext = true;
                } else operandNext = true;
                j++;
            }
            return -1;
        };

        return { renderModeTypeMatches, chooseRenderModeText, exprEnd };
    };
    const { renderModeTypeMatches, chooseRenderModeText, exprEnd } = renderModeHelpers();

    // The macros every mode shares: include-screen, render-mode, text and the subscreen and section calls.
    const anyMacros = (mode) => {
        // A text type list names the mode; xsl-fo is the older name of html-print.
        const typeMatches = (type) => renderModeTypeMatches(type, mode);
        return {
            "include-screen": stmt((node) =>
                `sri.renderIncludeScreen(${jsString(attr(node, "location"))}, ${jsString(attr(node, "share-scope"))});`),
            "render-mode": stmt((node) => {
                const chosen = chooseRenderModeText(node, mode);
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

    // Code for the text a label shows: its text expanded with the text-map, when it has one, over the context.
    const labelTextExpr = (node) => {
        const textMap = attr(node, "text-map");
        if (textMap) return `ec.resource.expand(${jsString(attr(node, "text") || "")}, "", ${raw(textMap)})`;
        return expandExpr(attr(node, "text") || "");
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
        // as widgetless: an ignored sub-node never renders, a hidden one only when no other widget is beside it
        const shown = (sub) => !child(sub, "ignored") && (!child(sub, "hidden") || hasShownWidget(sub));
        if (dflt && shown(dflt)) return false;
        return !childrenNamed(field, "conditional-field").some(shown);
    };

    // Whether a form-list's rows may be written once a row (a mode's bufferRows): each field's row parts hold only widgets
    // whose macros write through the mode's own writer and render nothing through the server (no section, include, dialog
    // or template), so nothing reaches the output in between.
    const BUFFERED_WIDGETS = new Set(["display", "display-entity", "link", "hidden", "ignored", "label", "image", "set"]);
    // Whether a list cell's field part needs a context of its own: Java pushes one for each cell so the part's set elements
    // stay in it, and a part with no set whose widgets put nothing in the context (as BUFFERED_WIDGETS's do) needs none.
    const cellNeedsContext = (sub) => (sub.children || []).some((widget) => widget.name === "set" || !BUFFERED_WIDGETS.has(widget.name));
    const bufferedListRows = (fields) => fields.every((field) => (field.children || [])
        .filter((sub) => sub.name === "default-field" || sub.name === "conditional-field")
        .every((sub) => (sub.children || []).every((widget) => BUFFERED_WIDGETS.has(widget.name))));

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

    // One entry of an orderByField list as OrderBy::parse (moqui-db sql/condition.rs) reads it: signs and the case prefix,
    // an optional direction and null placement, then UPPER/LOWER. The html and qvt runtimes mark the active sort with it,
    // and take it as source, so it uses nothing outside itself.
    const parseOrder = (value) => {
        let name = String(value).trimStart(), descending = false;
        while (name && "-+^ ".includes(name.charAt(0))) {
            if (name.charAt(0) === "-") descending = true;
            else if (name.charAt(0) === "+") descending = false;
            name = name.slice(1);
        }
        name = name.trimEnd().replace(/ NULLS (FIRST|LAST)$/i, "");
        const direction = / (ASC|DESC)$/i.exec(name);
        if (direction) {
            descending = direction[1].toUpperCase() === "DESC";
            name = name.slice(0, -direction[0].length);
        }
        if (name.includes("(")) {
            if (/^(UPPER|LOWER)\(/i.test(name)) name = name.slice(6);
            if (name.endsWith(")")) name = name.slice(0, -1);
        }
        name = name.trim();
        return name ? {name, descending} : null;
    };

    const noops = (names) => T.noops(names);

    return {
        beginLits, endLits, hoist, allElements, opaqueElements, unknownElementCode, expandExpr, expandNoL10nExpr, reportPropsExpr, reportRenderExpr, htmlViewTextFn, htmlViewTextExpr, markdownTextExpr, statusTextExpr, nodeLit, bareLit, write, indent, widgetsSource,
        setCode, widgetless, fieldChain, hideGuard, titleText, localizedTitle, fieldLit, renderTextCode, renderModeHelpers, renderModeTypeMatches, chooseRenderModeText, anyMacros,
        displayValueExpr, linkTextExpr, labelTextExpr, dateTimeFormat, listFieldHiddenWidget, bufferedListRows, cellNeedsContext, formListColumns, headerSub, heldCode, parseOrder, exprEnd, noops
    };
})
