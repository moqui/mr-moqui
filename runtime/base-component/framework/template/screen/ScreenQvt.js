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

// The qvt render mode, server-merged: the Vue 3 template of the target screen, with the request's values
// expanded into it and the fields of each form seeded through the fields-initial of its m-form. This follows
// DefaultScreenMacros.qvt.ftl for Vue 3.5 and Quasar 2.35. The page shell (WebrootVue.qvt.js) fetches the
// screen as <path>.qvt, wraps the text in a component, and compiles it in the browser.
//
// The elements the macros write are the m-* wrappers of js/WebrootVue.qvt.js, listed in js/README.md. A macro
// writes q-btn, q-tooltip and the like for buttons and layout, and never a raw q-input, q-select or q-date: the
// wrappers carry the model, the remote options, the depends-on fields and the change highlight. A wrapper
// binds with v-model (modelValue), and a widget outside a form gives its value as model-value.
//
// Differences from the FreeMarker macros:
//  - Text from the request is encoded and its opening braces are written as character references, so no data
//    is read as a Vue interpolation. The macros did that with v-pre.
//  - The wrappers get dense, outlined and stack-label themselves, and the change highlight from their form.
//  - A text-line with ac-transition is an m-drop-down with server-search over the same transition. The Java
//    macro wrote an error message.
//  - A confirmation is a data-confirm attribute the shell answers, not an inline onclick.
//  - The background-message of a form is text. The Java client evaluated it as a JavaScript string.
//  - An editor-type html or md text-area is an m-text-area (QEditor, or a textarea with a Marked preview).
//  - lookup is a text line. The Java macros had it commented out.
//  - The navigation bar sits above the table of a form-list and not in a table row.
//  - The mask of a text-line is converted from the Inputmask patterns (9, a, *) to Quasar mask tokens.
//  - In fields-initial a link does not replace the _display text of a display or display-entity before it in the same field.
(function (T, S) {
    "use strict";

    const { attr, has, jsString, child, childrenNamed, js, raw, stmt, noops } = T;
    const { bareLit, fieldChain, widgetsSource, setCode, titleText } = S;

    const MODE = "qvt";

    const lit = jsString;
    const dynamic = (text) => text.indexOf("${") >= 0;
    const noL10n = (text) => (dynamic(text) ? S.expandNoL10nExpr(text) : lit(text));
    const expandCode = (text) => (dynamic(text) ? S.expandExpr(text) : lit(text));

    const childLit = (node) => {
        const parts = [`"name":${lit(node.name)}`, `"attrs":${JSON.stringify(node.attrs || {})}`];
        if (node.text) parts.push(`"text":${lit(node.text)}`);
        if (node.children.length) parts.push(`"children":[${node.children.map(childLit).join(",")}]`);
        return `{${parts.join(",")}}`;
    };
    const pick = (attrs, names) => {
        const out = {};
        names.forEach((name) => { if (attrs && attrs[name] !== undefined) out[name] = attrs[name]; });
        return out;
    };
    const wlit = (node, withChildren) => {
        const parts = [`"name":${lit(node.name)}`, `"attrs":${JSON.stringify(node.attrs || {})}`];
        if (node.text) parts.push(`"text":${lit(node.text)}`);
        if (withChildren !== false && node.children.length) parts.push(`"children":[${node.children.map(childLit).join(",")}]`);
        const sub = node.parent;
        if (sub && sub.name.endsWith("-field") && sub.parent && sub.parent.name === "field") {
            const field = sub.parent;
            const form = field.parent;
            const formLit = form ? `,"parent":{"name":${lit(form.name)},"attrs":${JSON.stringify(pick(form.attrs, ["name", "map"]))}}` : "";
            parts.push(`"parent":{"name":${lit(sub.name)},"attrs":${JSON.stringify(sub.attrs || {})},"parent":{"name":"field","attrs":${JSON.stringify(field.attrs || {})}${formLit}}}`);
        }
        return `{${parts.join(",")}}`;
    };
    // The field of a sub-node, with its form, for the calls that read a field's value.
    const fieldLitOf = (field) => {
        const form = field.parent;
        const formLit = form ? `,"parent":{"name":${lit(form.name)},"attrs":${JSON.stringify(pick(form.attrs, ["name", "map"]))}}` : "";
        return `{"name":"field","attrs":${JSON.stringify(field.attrs || {})}${formLit}}`;
    };

    const titleCode = (sub) => {
        let holder = sub;
        if (sub.name === "header-field" && sub.parent) {
            const header = child(sub.parent, "header-field");
            const dflt = child(sub.parent, "default-field");
            if (header && attr(header, "title")) holder = header;
            else if (dflt && attr(dflt, "title")) holder = dflt;
        }
        const title = attr(holder, "title");
        if (title) return `ec.l10n.localize(${expandCode(title)})`;
        return `ec.l10n.localize(${lit(titleText(holder))})`;
    };

    // The runtime. Its source is run once for each script engine, and every unit reads it as $v.
    const runtimeFunction = function () {
        "use strict";

        const ENT = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" };
        // Attribute text, every attribute is double quoted. A bound attribute holds an expression and is encoded the same
        // way: the template parser decodes it.
        const e = (value) => {
            if (value === null || value === undefined) return "";
            const text = String(value);
            return /[&<>"]/.test(text) ? text.replace(/[&<>"]/g, (ch) => ENT[ch]) : text;
        };
        // Text between tags: an opening brace is a character reference so that two of them are not an interpolation.
        const t = (value) => e(value).replace(/\{/g, "&#123;");
        const A = (map) => {
            let out = "";
            for (const key of Object.keys(map)) {
                const value = map[key];
                if (value === null || value === undefined || value === false) continue;
                out += value === true ? " " + key : " " + key + '="' + e(value) + '"';
            }
            return out;
        };
        const cls = (...parts) => parts.filter((part) => part).join(" ") || null;
        const w = (text) => sri.write(text);
        const at = (n, key) => {
            const value = n && n.attrs ? n.attrs[key] : null;
            return value === null || value === undefined ? "" : String(value);
        };
        const kids = (n, name) => (n.children || []).filter((kid) => kid.name === name);
        const first = (n, name) => kids(n, name)[0] || null;
        const x = (text) => ec.resource.expand(text === null || text === undefined ? "" : String(text), "");
        const xn = (text) => (text && text.indexOf("${") >= 0 ? ec.resource.expandNoL10n(text, "") : text || "");
        const cond = (expr) => (expr ? !!ec.resource.condition(expr, "") : false);
        const L = (text) => ec.l10n.localize(text);
        const ctx = (name) => {
            const value = ec.context.get(name);
            return value === null || value === undefined ? "" : value;
        };
        const param = (key) => {
            const map = ec.web.parameters;
            const value = map ? map[key] : null;
            return value === null || value === undefined ? "" : String(value);
        };
        const blank = (value) => value === null || value === undefined || value === "";
        // A JavaScript string literal in single quotes, and a value as a JavaScript literal (JSON is also one, but its
        // double quotes would be character references in every attribute).
        const sq = (text) => "'" + String(text).replace(/[\\'\n\r\u2028\u2029]/g, (ch) => ({ "\\": "\\\\", "'": "\\'", "\n": "\\n", "\r": "\\r", "\u2028": "\\u2028", "\u2029": "\\u2029" })[ch]) + "'";
        const js_ = (value) => {
            if (value === null || value === undefined) return "null";
            if (typeof value === "string") return sq(value);
            if (typeof value === "number" || typeof value === "boolean") return String(value);
            if (Array.isArray(value)) return "[" + value.map(js_).join(",") + "]";
            return "{" + Object.keys(value).map((key) => sq(key) + ":" + js_(value[key])).join(",") + "}";
        };
        const arr = (list) => js_(list.map(String));
        const quasarColor = (color) => (color === "success" ? "positive" : color === "danger" ? "negative" : color === "default" ? "" : color || "");
        const icon = (name, text) => (name ? '<i class="' + e(name) + ' q-icon' + (text ? " on-left" : "") + '"></i> ' : "");
        const tooltipTag = (text) => (text ? "<q-tooltip>" + t(text) + "</q-tooltip>" : "");

        const OUTLINE_BG = ":bg-color";

        // Per-unit state of the form being written. A form inside a form (a row action in a list) saves and restores it.
        const st = {};
        const saved = [];
        const reset = () => {
            Object.assign(st, {
                formName: "", fieldForm: "", owner: "", entry: "", multi: false, skipForm: false, inForm: false,
                accordion: "", accIndex: 0, accActive: "", disabled: false, selectedForm: false
            });
            saved.length = 0;
        };
        reset();
        const push = () => { saved.push(Object.assign({}, st)); };
        const pop = () => { const old = saved.pop(); if (old) Object.assign(st, old); };

        const sec = () => {
            const value = ec.context.get("sectionEntryIndex");
            return value === null || value === undefined || value === "" ? "" : "_" + value;
        };
        const entry = () => (st.entry !== "" ? "_" + st.entry : "");
        const nodeId = (n) => {
            const id = xn(at(n, "id"));
            return id ? id + entry() + sec() : "";
        };
        const fieldOf = (n) => n.parent.parent;
        const fname = (n) => at(fieldOf(n), "name");
        const formName = (n) => at(fieldOf(n).parent, "name");
        const fid = (n) => (st.fieldForm || xn(formName(n))) + "_" + fname(n) + entry() + sec();
        const fnm = (n) => fname(n) + (st.multi && st.entry !== "" && /^\d+$/.test(st.entry) ? "_" + st.entry : "");
        const owner = () => st.owner || null;
        const tip = (n) => (at(n.parent, "tooltip") ? x(at(n.parent, "tooltip")) : null);
        const label = (n) => titleOf(n.parent);

        // The localized title of a field sub-node.
        const titleOf = (sub) => {
            let holder = sub;
            if (sub.name === "header-field" && sub.parent) {
                const header = first(sub.parent, "header-field");
                const dflt = first(sub.parent, "default-field");
                if (header && at(header, "title")) holder = header;
                else if (dflt && at(dflt, "title")) holder = dflt;
            }
            if (at(holder, "title")) return L(x(at(holder, "title")));
            const name = holder.parent ? at(holder.parent, "name") : "";
            return L(name.split(/(?=[A-Z])/).map((word) => (word.charAt(0).toUpperCase() + word.slice(1)).replace("Id", "ID")).join(" "));
        };

        // How a field binds. In a form the model is formProps.fields.name; outside one the value is given as model-value.
        const fieldRef = (name) => (/^[A-Za-z_$][A-Za-z0-9_$]*$/.test(name) ? "formProps.fields." + name : "formProps.fields[" + sq(name) + "]");
        const bind = (name, value) => (st.inForm ? { "v-model": fieldRef(name), ":fields": "formProps.fields" } : { "model-value": value });
        const bindOnly = (name, value) => (st.inForm ? { "v-model": fieldRef(name) } : { "model-value": value });

        const validation = (sub) => (at(sub, "validate-service") || at(sub, "validate-entity") ? sri.getFieldValidation(sub) : { classes: "" });
        const validate = (sub) => {
            const v = validation(sub);
            const has = (name) => v.classes.indexOf(name) >= 0;
            return { required: has("required"), email: has("email"), url: has("url"), number: has("number"), digits: has("digits"), v };
        };
        // The rules of a field as the template array of functions, the messages localized here.
        const rulesOf = (n, check) => {
            const rules = [];
            if (at(n, "rules")) rules.push(at(n, "rules"));
            // getFieldValidationJsRules; a required field is the widget's required prop.
            const sub = n.parent;
            const js = at(sub, "validate-service") || at(sub, "validate-entity") ? sri.getFieldValidationJsRules(sub) : [];
            js.forEach((rule) => { if (rule.expr !== "!!value") rules.push("value => (" + rule.expr + ") || " + sq(rule.message)); });
            return rules.length ? "[" + rules.join(",") + "]" : null;
        };
        // Inputmask patterns (9 digit, a letter, * any) to the tokens Quasar's mask uses.
        const quasarMask = (mask) => mask.replace(/9/g, "#").replace(/a/g, "S").replace(/\*/g, "N");

        // Remote options: the URL, its fixed parameters, and the fields it depends on as {parameter: field}.
        const remote = (holder, transition) => {
            const url = sri.makeUrlByType(transition, "transition", holder, "false");
            const params = Object.assign({}, url.getParameterMap());
            const deps = kids(holder, "depends-on");
            const dependsOn = {};
            deps.forEach((dep) => { dependsOn[at(dep, "parameter") || at(dep, "field")] = at(dep, "field"); delete params[at(dep, "parameter") || at(dep, "field")]; });
            const clean = {};
            Object.keys(params).forEach((key) => { if (params[key] !== null && params[key] !== undefined && params[key] !== "") clean[key] = params[key]; });
            return { url: url.url, params: clean, dependsOn, hasDepends: deps.length > 0 };
        };

        // ---- values of the fields of a form, for fields-initial ----

        const display = (n, field) => {
            const unit = at(n, "currency-unit-field");
            const hideSymbol = at(n, "currency-hide-symbol") === "true";
            const currency = (value) => (hideSymbol ? ec.l10n.formatCurrencyNoSymbol(value, ec.resource.expression(unit, "")) : ec.l10n.formatCurrency(value, ec.resource.expression(unit, "")));
            if (at(n, "text")) {
                const map = at(n, "text-map") ? ec.resource.expression(at(n, "text-map"), "") : null;
                const text = map ? ec.resource.expand(at(n, "text"), "", map) : ec.resource.expand(at(n, "text"), "");
                return unit ? currency(text) : text;
            }
            if (unit) return currency(sri.getFieldValue(field, ""));
            return sri.getFieldValueString(n);
        };

        // The widget's contribution to the values of its field: the named values of ScreenRenderImpl.addFormFieldValue.
        const addValue = (out, widget, field, name) => {
            let plain = sri.getFieldValuePlainString(field, "");
            if (blank(plain)) plain = xn(at(widget, "no-current-selected-key"));
            if (plain && plain.charAt(0) === "[") plain = plain.substring(1, plain.length - 1).replace(/ /g, "");
            const current = plain ? plain.split(",") : null;
            const kind = widget.name;
            if (kind === "display") {
                if (!at(widget, "also-hidden") || at(widget, "also-hidden") === "true") out[name] = plain;
                out[name + "_display"] = display(widget, field);
            } else if (kind === "drop-down") {
                if (xn(at(widget, "allow-multiple")) === "true") {
                    out[name] = current;
                    out[name + "_op"] = "in";
                } else out[name] = current && current.length ? current[0] : null;
                if (xn(at(widget, "show-not")) === "true") out[name + "_not"] = ctx(name + "_not") || "N";
            } else if (kind === "text-line" || kind === "text-area" || kind === "lookup") {
                out[name] = sri.getFieldValueString(widget);
            } else if (kind === "check") {
                if (xn(at(widget, "all-checked")) === "true") out[name] = Array.from(sri.getFieldOptions(widget).keys());
                else out[name] = current || [];
            } else if (kind === "date-find") {
                const type = at(widget, "type");
                const format = type === "date" ? "yyyy-MM-dd" : type === "time" ? "HH:mm" : "yyyy-MM-dd HH:mm";
                const fmt = (value) => { if (blank(value)) return ""; try { return ec.l10n.format(value, format); } catch (err) { return String(value); } };
                out[name + "_from"] = fmt(ctx(name + "_from") || at(widget, "default-value-from"));
                out[name + "_thru"] = fmt(ctx(name + "_thru") || at(widget, "default-value-thru"));
            } else if (kind === "date-period") {
                ["_poffset", "_period", "_pdate", "_from", "_thru"].forEach((suffix) => { out[name + suffix] = blank(ctx(name + suffix)) ? null : String(ctx(name + suffix)); });
            } else if (kind === "date-time") {
                const type = at(widget, "type");
                const format = at(widget, "format") || (type === "date" ? "yyyy-MM-dd" : type === "time" ? "HH:mm" : "yyyy-MM-dd HH:mm");
                out[name] = sri.getFieldValueString(field, at(widget, "default-value"), format, type || "date-time");
            } else if (kind === "display-entity") {
                if (!at(widget, "also-hidden") || at(widget, "also-hidden") === "true") out[name] = plain;
                out[name + "_display"] = sri.getFieldEntityValue(widget) || "";
            } else if (kind === "hidden") {
                out[name] = sri.getFieldValuePlainString(field, at(widget, "default-value"));
            } else if (kind === "file" || kind === "ignored" || kind === "password") {
                // nothing to seed
            } else if (kind === "radio") {
                out[name] = sri.getFieldValueString(field, at(widget, "no-current-selected-key"), "");
            } else if (kind === "range-find") {
                out[name + "_from"] = blank(ctx(name + "_from")) ? null : ctx(name + "_from");
                out[name + "_thru"] = blank(ctx(name + "_thru")) ? null : ctx(name + "_thru");
            } else if (kind === "text-find") {
                out[name] = sri.getFieldValueString(widget);
                out[name + "_op"] = ctx(name + "_op") || at(widget, "default-operator") || "contains";
                out[name + "_not"] = ctx(name + "_not") || "N";
                let ic = ctx(name + "_ic");
                if (blank(ic) && (!at(widget, "ignore-case") || at(widget, "ignore-case") === "true")) ic = "Y";
                out[name + "_ic"] = ic || "N";
            } else if (kind === "link") {
                if (blank(plain)) return;
                out[name] = plain;
                let text = null;
                if (at(widget, "entity-name")) {
                    try { text = sri.getFieldEntityValue(widget); } catch (err) { text = null; }
                } else if (at(widget, "text")) text = ec.resource.expand(at(widget, "text"), "");
                if (text !== null && text !== undefined) {
                    text = String(text).trim();
                    // A display or display-entity before the link in the same field keeps its text (Java let the link replace it, so
                    // `display-entity` + `link` showed the id twice).
                    if (text && text !== "null" && text.indexOf("null:") !== 0 && !(name + "_display" in out)) out[name + "_display"] = text;
                }
            } else if (kind !== "submit" && kind !== "reset" && kind !== "label" && kind !== "image") {
                out[name] = plain;
            }
        };

        // descs: [{name, field, subs: [{cond, sets, widgets}]}]. Row kind r picks the first conditional-field that holds
        // and else the default-field. The other kinds have one sub-node.
        const fieldValues = (out, descs) => {
            descs.forEach((desc) => {
                let active = null;
                for (const sub of desc.subs) {
                    if (sub.cond === null || sub.cond === undefined) { if (!active) active = sub; continue; }
                    try { if (ec.resource.condition(sub.cond, "")) { active = sub; break; } } catch (err) { /* a bad condition does not hold */ }
                }
                if (!active) return;
                const pushed = active.sets && active.sets.length;
                if (pushed) { sri.pushContext(); active.sets.forEach((set) => sri.setInContext(set)); }
                try {
                    active.widgets.forEach((widget) => addValue(out, widget, desc.field, desc.name));
                } finally {
                    if (pushed) sri.popContext();
                }
            });
            return out;
        };

        const hiddenParams = (n) => sri.getFormHiddenParameters(n);
        const orderList = () => { const text = ctx("orderByField"); return text ? String(text).split(",") : []; };

        const singleValues = (n, descs) => {
            const out = {};
            if (at(n, "pass-through-parameters") === "true") Object.assign(out, sri.getScreenUrlInstance().getPassThroughParameterMap());
            out.moquiFormName = at(n, "name");
            const stamp = sri.getNamedValuePlain("lastUpdatedStamp", n);
            if (stamp) out.lastUpdatedStamp = stamp;
            return fieldValues(out, descs);
        };

        const headerValues = (n, descs) => {
            const out = Object.assign({}, hiddenParams(n));
            fieldValues(out, descs);
            out.orderByField = orderList();
            if (ctx("formListFindId")) out.formListFindId = ctx("formListFindId");
            const size = ctx(at(n, "list") + "PageSize") || ctx("pageSize");
            if (size) out.pageSize = String(size);
            return out;
        };

        const rowValues = (n, descs, url) => {
            const out = {};
            Object.assign(out, url.getParameterMap(), hiddenParams(n));
            fieldValues(out, descs);
            out.moquiFormName = at(n, "name");
            out.pageIndex = String(ctx("pageIndex") || "0");
            if (ctx("orderByField")) out.orderByField = String(ctx("orderByField"));
            return out;
        };

        const initial = (values) => js_(values);

        // The validation errors a redirect saved for this form, which m-form shows by their fields. None writes no attribute.
        const savedErrors = (formName) => {
            const saved = (ec.web && ec.web.savedValidationErrors) || [];
            const mine = [];
            for (const ve of saved) if (ve && ve.field && (!ve.form || ve.form === formName)) mine.push(ve);
            return mine.length ? js_(mine) : null;
        };

        // ---- inputs ----

        const colorClass = (check) => null;

        const textLine = (n) => {
            const id = fid(n);
            const name = fnm(n);
            const value = sri.getFieldValueString(n);
            const check = validate(n.parent);
            if (at(n, "ac-transition")) {
                const remoteInfo = remote(n, at(n, "ac-transition"));
                const initialText = at(n, "ac-initial-text") ? x(at(n, "ac-initial-text"))
                    : (sri.getFieldTransitionValue(at(n, "ac-transition"), n, value, "label", false) || value);
                w("<m-drop-down" + A(Object.assign({
                    name, id, label: label(n), form: owner(), disable: st.disabled || null, "allow-empty": true, "server-search": true,
                    "server-min-length": at(n, "ac-min-length") || null, "server-delay": at(n, "ac-delay") || null, combo: at(n, "ac-use-actual") === "true" ? true : null,
                    "options-url": remoteInfo.url, ":options-parameters": js_(remoteInfo.params), ":depends-on": js_(remoteInfo.dependsOn),
                    "depends-optional": at(n, "depends-optional") === "true" ? true : null, tooltip: tip(n),
                    ":options": value ? js_([{ value, label: initialText || value }]) : "[]", "options-load-init": value ? true : null
                }, bind(name, value))) + "></m-drop-down>");
                return;
            }
            const align = at(fieldOf(n), "align");
            const mask = xn(at(n, "mask"));
            const disabled = st.disabled || cond(at(n, "disabled"));
            const attrs = {
                id, type: at(n, "input-type") || (check.email ? "email" : check.url ? "url" : "text"), name, label: label(n),
                prefix: at(n, "prefix") ? x(at(n, "prefix")) : null, size: at(n, "size") || null, style: at(n, "size") ? null : "width:100%;",
                maxlength: at(n, "maxlength") || null, disable: disabled || null,
                class: cls(check.v.classes || null, align === "center" ? "text-center" : align === "right" ? "text-right" : null),
                required: check.required || null, mask: mask ? quasarMask(mask) : null, "fill-mask": mask ? "_" : null,
                "reverse-fill-mask": mask && check.number ? true : null, ":rules": rulesOf(n, check), form: owner(), tooltip: tip(n)
            };
            if (at(n, "default-transition")) {
                const remoteInfo = remote(n, at(n, "default-transition"));
                Object.assign(attrs, {
                    "default-url": remoteInfo.url, "default-load-init": true, "depends-optional": at(n, "depends-optional") === "true" ? true : null,
                    ":depends-on": js_(remoteInfo.dependsOn), ":default-parameters": js_(remoteInfo.params)
                });
            }
            w("<m-text-line" + A(Object.assign(attrs, bind(name, value))) + "></m-text-line>");
        };

        const textArea = (n) => {
            const name = fnm(n);
            w("<m-text-area" + A(Object.assign({
                name, id: fid(n), label: label(n), "editor-type": x(at(n, "editor-type")) || null, rows: at(n, "rows") || "3",
                cols: at(n, "cols") || null, style: at(n, "cols") ? null : "width:100%;", readonly: at(n, "read-only") === "true",
                autogrow: at(n, "autogrow") === "true", maxlength: at(n, "maxlength") || null, disable: st.disabled || null, form: owner(), tooltip: tip(n)
            }, bindOnly(name, sri.getFieldValueString(n)))) + "></m-text-area>");
        };

        const password = (n) => {
            const check = validate(n.parent);
            const name = fnm(n);
            w("<m-text-line" + A(Object.assign({
                id: fid(n), type: "password", name, label: label(n), size: at(n, "size") || "25", maxlength: at(n, "maxlength") || null,
                required: check.required || null, disable: st.disabled || null, form: owner(), tooltip: tip(n)
            }, bindOnly(name, ""))) + "></m-text-line>");
        };

        const file = (n) => {
            const name = fnm(n);
            w("<m-file" + A(Object.assign({
                name, id: fid(n), label: label(n), size: at(n, "size") || "30", multiple: at(n, "multiple") === "true",
                accept: at(n, "accept") ? x(at(n, "accept")) : null, maxlength: at(n, "maxlength") || null, disable: st.disabled || null,
                form: owner(), tooltip: tip(n)
            }, bindOnly(name, null))) + "></m-file>");
        };

        const hidden = (n) => {
            if (st.inForm) return;
            w("<input" + A({
                type: "hidden", name: fnm(n), id: fid(n), value: sri.getFieldValuePlainString(fieldOf(n), at(n, "default-value")), form: owner()
            }) + ">\n");
        };

        const reset_ = (n) => {
            w("<q-btn" + A({
                dense: true, outline: true, "no-caps": true, type: "reset", name: fnm(n), id: fid(n), form: owner()
            }) + ">" + tooltipTag(tip(n)) + t(label(n)) + "</q-btn>");
        };

        const submit = (n) => {
            const title = label(n);
            const text = at(n, "text") ? x(at(n, "text")) : title;
            const confirmation = x(at(n, "confirmation"));
            const iconClass = at(n, "icon") || sri.getThemeIconClass(text) || "";
            const image = first(n, "image");
            let body = tooltipTag(tip(n)) + (iconClass ? '<i class="' + e(iconClass) + (image ? "" : " q-icon on-left") + '"></i>' : "");
            if (image) {
                const src = sri.makeUrlByType(at(image, "url"), at(image, "url-type") || "content", null, "true");
                body += "<img" + A({ src: src.url, alt: at(image, "alt") || title, width: at(image, "width") || null, height: at(image, "height") || null }) + ">";
            } else body += t(text);
            w("<q-btn" + A({
                dense: true, ":outline": st.inForm ? "!formProps.hasFieldsChanged" : null, outline: st.inForm ? null : true, "no-caps": true, type: "submit",
                name: fnm(n), value: fnm(n), id: fid(n), color: quasarColor(at(n, "type") || "primary") || null, disabled: st.disabled || null,
                "data-confirm": confirmation || null, form: owner()
            }) + ">" + body + "</q-btn>");
        };

        // ---- dates ----

        const dateDefault = (n) => (at(n, "type") === "time" ? "HH:mm" : at(n, "type") === "date" ? "yyyy-MM-dd" : "yyyy-MM-dd HH:mm");
        // Java date patterns to the tokens of the Quasar date mask: a is AM/PM, d is the day and y the year.
        const maskFormat = (format) => format.replace(/a/g, "A").replace(/d/g, "D").replace(/y/g, "Y");

        const dateTime = (n) => {
            const format = at(n, "format") || dateDefault(n);
            const name = fnm(n);
            const check = validate(n.parent);
            const value = sri.getFieldValueString(fieldOf(n), at(n, "default-value"), format, at(n, "type") || "date-time");
            w("<m-date-time" + A(Object.assign({
                id: fid(n), name, type: at(n, "type") || null, size: at(n, "size") || null, label: label(n), disable: st.disabled || null,
                format: maskFormat(format), required: check.required || null, ":rules": at(n, "rules") ? "[" + at(n, "rules") + "]" : null,
                "auto-year": at(n, "auto-year") || "true", ":minute-step": at(n, "minute-stepping") || "5", form: owner(), tooltip: tip(n)
            }, bindOnly(name, value))) + "></m-date-time>");
        };

        const formatted = (value, format) => {
            if (value === null || value === undefined || value === "") return "";
            try {
                return ec.l10n.format(value, format);
            } catch (err) {
                return String(value);
            }
        };

        const dateFind = (n) => {
            const name = fnm(n);
            const id = fid(n);
            const format = dateDefault(n);
            const title = label(n);
            const one = (suffix, text, def) => '<span class="form-date-find"><m-date-time' + A(Object.assign({
                id: id + suffix, name: name + suffix, type: at(n, "type") || null, size: at(n, "size") || null, label: title + " " + L(text),
                format: maskFormat(format), form: owner(), tooltip: tip(n)
            }, bindOnly(name + suffix, formatted(ctx(name + suffix) || def, format)))) + "></m-date-time></span>";
            w(one("_from", "From", at(n, "default-value-from")) + one("_thru", "Thru", at(n, "default-value-thru")));
        };

        const datePeriod = (n) => {
            const name = fnm(n);
            const values = {};
            ["_poffset", "_period", "_pdate", "_from", "_thru"].forEach((suffix) => {
                const value = ctx(name + suffix);
                values[name + suffix] = suffix === "_period" ? String(value).toLowerCase() : String(value);
            });
            w("<m-date-period" + A({
                name, id: fid(n), "allow-empty": at(n, "allow-empty") !== "false", "from-thru-type": at(n, "time") === "true" ? "date-time" : "date",
                label: label(n), ":fields": st.inForm ? "formProps.fields" : js_(values), form: owner(), tooltip: tip(n)
            }) + "></m-date-period>");
        };

        const rangeFind = (n) => {
            const name = fnm(n);
            w("<m-range-find" + A({
                name, id: fid(n), label: label(n), size: at(n, "size") || "10", maxlength: at(n, "maxlength") || null, form: owner(), tooltip: tip(n),
                ":fields": st.inForm ? "formProps.fields" : js_({ [name + "_from"]: ctx(name + "_from") || at(n, "default-value-from"), [name + "_thru"]: ctx(name + "_thru") || at(n, "default-value-thru") })
            }) + "></m-range-find>");
        };

        const textFind = (n) => {
            const name = fnm(n);
            const hide = at(n, "hide-options") || "false";
            const ic = param(name + "_ic") === "Y" || !at(n, "ignore-case") || at(n, "ignore-case") === "true";
            w("<m-text-find" + A({
                name, id: fid(n), label: label(n), size: at(n, "size") || "30", maxlength: at(n, "maxlength") || null, "hide-options": hide, form: owner(), tooltip: tip(n),
                ":fields": st.inForm ? "formProps.fields" : js_({
                    [name]: sri.getFieldValueString(n), [name + "_op"]: param(name + "_op") || at(n, "default-operator") || "contains",
                    [name + "_not"]: param(name + "_not") === "Y" ? "Y" : "N", [name + "_ic"]: ic ? "Y" : "N"
                })
            }) + "></m-text-find>");
        };

        // ---- options ----

        const optionList = (options, current, multiple) => {
            const out = [];
            options.forEach((text, key) => out.push({ value: key, label: text }));
            return out;
        };

        const dropDown = (n) => {
            const id = fid(n);
            const name = fnm(n);
            const multiple = xn(at(n, "allow-multiple")) === "true";
            const allowEmpty = xn(at(n, "allow-empty")) === "true";
            const dyn = first(n, "dynamic-options");
            const options = sri.getFieldOptions(n);
            let current = sri.getFieldValuePlainString(fieldOf(n), "");
            if (!current) current = xn(at(n, "no-current-selected-key"));
            if (current.indexOf("[") === 0) current = current.substring(1, current.length - 1).replace(/ /g, "");
            const currentList = current ? current.split(",") : [];
            if (multiple) current = "";
            else current = currentList[0] || "";
            if (!multiple && !allowEmpty && !current && options.size > 0) current = options.keys().next().value;
            let description = options.get(current);
            const optionsHasCurrent = !!description;
            if (!optionsHasCurrent && at(n, "current-description")) description = x(at(n, "current-description"));
            else if (!optionsHasCurrent && dyn && current && !multiple) {
                description = sri.getFieldTransitionValue(at(dyn, "transition"), dyn, current, at(dyn, "label-field") || "label", false);
            }
            const check = validate(n.parent);
            const list = optionList(options);
            if (current && !multiple && !optionsHasCurrent) list.unshift({ value: current, label: description || current });
            const attrs = {
                name, id, label: label(n), disable: st.disabled || null, form: owner(), size: at(n, "size") || null,
                class: cls(dyn ? "dynamic-options" : null, xn(at(n, "style")) || null, check.v.classes || null),
                multiple: multiple || null, "allow-empty": allowEmpty || null, combo: at(n, "combo-box") === "true" ? true : null,
                "required-manual-select": at(n, "required-manual-select") === "true" ? true : null,
                "submit-on-select": at(n, "submit-on-select") === "true" ? true : null, tooltip: tip(n), ":options": js_(list)
            };
            if (dyn) {
                const remoteInfo = remote(dyn, at(dyn, "transition"));
                Object.assign(attrs, {
                    "options-url": remoteInfo.url, "value-field": at(dyn, "value-field") || "value", "label-field": at(dyn, "label-field") || "label",
                    "depends-optional": at(dyn, "depends-optional") === "true" ? true : null, ":depends-on": js_(remoteInfo.dependsOn),
                    ":options-parameters": js_(remoteInfo.params), "server-search": at(dyn, "server-search") === "true" ? true : null,
                    ":server-delay": at(dyn, "delay") || null, ":server-min-length": at(dyn, "min-length") || null,
                    "options-load-init": (dyn.children || []).length > 1 || undefined
                });
                if (!attrs["options-load-init"]) delete attrs["options-load-init"];
                // the current value of a remote list shows its label when the page is written
                if (current && !multiple && !optionsHasCurrent && !description) {
                    try {
                        const text = sri.getFieldTransitionValue ? sri.getFieldTransitionValue(at(dyn, "transition"), dyn, current, at(dyn, "label-field") || "label", true) : "";
                        if (text) list[0].label = text;
                        attrs[":options"] = js_(list);
                    } catch (err) { /* the value is its own label */ }
                }
            }
            const model = multiple ? bind(name, currentList.filter((item) => item)) : bind(name, current);
            if (!st.inForm && multiple) { delete model["model-value"]; model[":model-value"] = js_(currentList.filter((item) => item)); }
            let out = "<m-drop-down" + A(Object.assign(attrs, model)) + ">";
            if (xn(at(n, "show-not")) === "true") {
                out += "<template v-slot:after><m-check" + A(Object.assign({
                    name: name + "_not", id: id + "_not", "true-value": "Y", "false-value": "N", "single-label": L("Not"), form: owner()
                }, st.inForm ? { "v-model": fieldRef(name + "_not") } : { "model-value": param(name + "_not") === "Y" ? "Y" : "N" })) + "></m-check></template>";
            }
            w(out + "</m-drop-down>");
        };

        const checkOrRadio = (n, kind) => {
            const options = sri.getFieldOptions(n);
            let current = sri.getFieldValueString(n);
            if (!current) current = xn(at(n, "no-current-selected-key"));
            const name = fnm(n);
            const list = optionList(options);
            const allChecked = xn(at(n, "all-checked"));
            const tag = kind === "checkbox" ? "m-check" : "m-radio";
            let value;
            if (kind === "checkbox") {
                value = allChecked === "true" ? list.map((item) => item.value) : current ? current.split(",").filter((item) => item && list.some((o) => o.value === item)) : [];
            } else value = current;
            const attrs = {
                name, id: fid(n), label: label(n), ":options": js_(list), disable: st.disabled || null, form: owner(), tooltip: tip(n),
                class: kind === "checkbox" ? xn(at(n, "container-style")) || null : null, "no-wrapper": kind === "checkbox" && at(n, "no-wrapper") === "true" ? true : null
            };
            const model = st.inForm ? { "v-model": fieldRef(name) } : kind === "checkbox" ? { ":model-value": js_(value) } : { "model-value": value };
            w("<" + tag + A(Object.assign(attrs, model)) + "></" + tag + ">");
        };

        // ---- display ----

        const displayWidget = (n) => {
            const field = fieldOf(n);
            const id = fid(n);
            const name = fnm(n);
            const alsoHidden = (!at(n, "also-hidden") || at(n, "also-hidden") === "true") && !st.skipForm;
            const dyn = !!at(n, "dynamic-transition");
            const align = at(field, "align");
            const labelWrapper = field.parent && field.parent.name === "form-single";
            let value = display(n, field);
            const attrs = {
                name, id: id + "_display", label: label(n), "label-wrapper": labelWrapper || null, tooltip: tip(n),
                class: cls(sri.getFieldValueClass(field), at(n, "currency-unit-field") ? "currency" : null, align === "center" ? "text-center" : align === "right" ? "text-right" : null, xn(at(n, "style")) || null)
            };
            if (dyn) {
                const remoteInfo = remote(n, at(n, "dynamic-transition"));
                Object.assign(attrs, {
                    "value-url": remoteInfo.url, "depends-optional": at(n, "depends-optional") === "true" ? true : null,
                    ":depends-on": js_(remoteInfo.dependsOn), ":value-parameters": js_(remoteInfo.params)
                });
            }
            let out;
            if (st.inForm) {
                Object.assign(attrs, { "v-model": fieldRef(name), ":display": fieldRef(name + "_display"), ":fields": "formProps.fields" });
                out = "<m-display" + A(attrs) + "></m-display>";
            } else {
                if (labelWrapper && value) attrs.display = at(n, "encode") === "false" ? value : value;
                const text = !labelWrapper && value ? (at(n, "encode") === "false" ? value : t(value).replace(/\n/g, "<br>")) : "";
                out = "<m-display" + A(attrs) + ">" + (text ? "<template v-slot:default>" + text + "</template>" : "") + "</m-display>";
                if (alsoHidden) out += "<input" + A({ type: "hidden", id, name, value: dyn ? "" : sri.getFieldValuePlainString(field, ""), form: owner() }) + ">";
            }
            w(out);
        };

        const displayEntity = (n) => {
            const field = fieldOf(n);
            const id = fid(n);
            const name = fnm(n);
            const alsoHidden = (!at(n, "also-hidden") || at(n, "also-hidden") === "true") && !st.skipForm;
            const align = at(field, "align");
            const formKind = field.parent ? field.parent.name : "";
            const attrs = {
                name, id: id + "_display", label: formKind === "form-single" ? label(n) : null, "label-wrapper": formKind === "form-single" || null, tooltip: tip(n),
                class: cls(align === "center" ? "text-center" : align === "right" ? "text-right" : null, xn(at(n, "style")) || null)
            };
            if (st.inForm) {
                Object.assign(attrs, { "v-model": fieldRef(name), ":display": fieldRef(name + "_display") });
                w("<m-display" + A(attrs) + "></m-display>");
                return;
            }
            const value = sri.getFieldEntityValue(n) || "";
            if (formKind === "form-single") attrs.display = value;
            const text = formKind !== "form-single" && value ? (at(n, "encode") === "false" ? value : t(value).replace(/\n/g, "<br>")) : "";
            w("<m-display" + A(attrs) + ">" + (text ? "<template v-slot:default>" + text + "</template>" : "") + "</m-display>");
            if (alsoHidden) w("<input" + A({ type: "hidden", id, name, value: sri.getFieldValuePlainString(field, ""), form: owner() }) + ">");
        };

        // ---- links, images, labels ----

        const imageTag = (img) => {
            const src = sri.makeUrlByType(at(img, "url"), at(img, "url-type") || "content", img, "true").getUrlWithParams();
            return "<img" + A({
                src, alt: x(at(img, "alt") || "image"), id: at(img, "id") || null, width: at(img, "width") || null,
                height: at(img, "height") || null, class: xn(at(img, "style")) || null
            }) + ">";
        };

        const image = (n) => {
            if (at(n, "condition") && !cond(at(n, "condition"))) return;
            const tag = imageTag(n);
            w(at(n, "hover") === "true" ? '<span class="hover-image-container">' + tag + tag.replace("<img", '<img class="hover-image"') + "</span>" : tag);
        };

        const labelWidget = (n) => {
            if (at(n, "condition") && !cond(at(n, "condition"))) return;
            const type = at(n, "type") || "span";
            const map = at(n, "text-map") ? ec.resource.expression(at(n, "text-map"), "") : null;
            let text = map ? ec.resource.expand(at(n, "text"), "", map) : ec.resource.expand(at(n, "text"), "");
            if (!(String(text).trim() || at(n, "condition"))) return;
            if (at(n, "encode") !== "false") {
                text = t(text);
                if (type !== "code" && type !== "pre") text = text.replace(/\n/g, "<br>");
            }
            w("<" + type + A({
                id: nodeId(n) || null, class: cls("text-inline", xn(at(n, "style")) || null), title: at(n, "tooltip") ? x(at(n, "tooltip")) : null
            }) + ">" + text + "</" + type + ">\n");
        };

        const linkParts = (n) => {
            if (at(n, "condition") && !cond(at(n, "condition"))) return null;
            let text;
            if (at(n, "entity-name")) text = sri.getFieldEntityValue(n);
            else {
                const map = at(n, "text-map") ? ec.resource.expression(at(n, "text-map"), "") : null;
                text = map ? ec.resource.expand(at(n, "text"), "", map) : ec.resource.expand(at(n, "text"), "");
            }
            if (text === "null" || text === null || text === undefined) text = "";
            const img = first(n, "image");
            if (!(text || img || at(n, "icon"))) return null;
            if (at(n, "encode") !== "false") text = t(text);
            let url = sri.makeUrlByType(at(n, "url"), at(n, "url-type") || "transition", n, at(n, "expand-transition-url") || "true");
            if (at(n, "pass-through-parameters") === "true") url = url.addParameters(sri.getScreenUrlInstance().getPassThroughParameterMap());
            return { text, url, img, anchor: sri.isAnchorLink(n, url) };
        };

        const linkBody = (n, text, img, plainIcon) => {
            let iconClass = at(n, "icon");
            if (!iconClass && at(n, "text")) iconClass = sri.getThemeIconClass(at(n, "text")) || "";
            iconClass = xn(iconClass);
            const badge = x(at(n, "badge"));
            const tooltip = at(n, "tooltip") ? tooltipTag(x(at(n, "tooltip"))) : "";
            const hasText = !!String(text).trim();
            return tooltip + (iconClass ? plainIcon ? '<i class="' + e(iconClass) + '"></i>' : icon(iconClass, hasText) : "") +
                (img ? imageTag(img) : text) + (badge ? ' <q-badge class="on-right" transparent>' + t(badge) + "</q-badge>" : "");
        };

        const linkElement = (n, parts, formId) => {
            const style = xn(at(n, "style")) || null;
            const linkType = at(n, "link-type");
            const color = quasarColor(at(n, "btn-type") || "primary") || null;
            if (parts.url.getDisableLink()) {
                return "<span><q-btn" + A({
                    dense: true, "no-caps": true, disabled: true, outline: linkType !== "anchor" && linkType !== "hidden-form-link" ? true : null,
                    flat: linkType === "anchor" || linkType === "hidden-form-link" ? true : null, class: cls("m-link", style), id: formId || null,
                    label: parts.text || null
                }) + ">" + (at(n, "icon") ? '<i class="' + e(xn(at(n, "icon"))) + '"></i>' : "") + "</q-btn></span>";
            }
            const confirmation = x(at(n, "confirmation"));
            if (parts.anchor) {
                const screen = parts.url.isScreenUrl();
                const noParam = at(n, "url-noparam") === "true";
                const href = screen ? (noParam ? parts.url.path : parts.url.pathWithParams) : (noParam ? parts.url.url : parts.url.getUrlWithParams());
                const tag = screen ? "m-link" : "a";
                const inner = linkBody(n, parts.text, parts.img, false) + "";
                const asButton = linkType !== "anchor";
                return "<" + tag + A({
                    href, id: formId || null, target: at(n, "target-window") || null, "load-id": at(n, "dynamic-load-id") || null,
                    "data-confirm": confirmation || null, class: asButton ? null : style
                }) + ">" + (asButton ? "<q-btn" + A({ dense: true, outline: true, "no-caps": true, color, class: cls("m-link", style) }) + ">" + inner + "</q-btn>" : inner) + "</" + tag + ">";
            }
            if (!formId) return "";
            return "<q-btn" + A({
                dense: true, outline: true, "no-caps": true, type: "submit", form: formId, id: formId + "_button", color, class: style || null,
                "data-confirm": confirmation || null
            }) + ">" + linkBody(n, parts.text, parts.img, false) + "</q-btn>";
        };

        const hiddenInput = (name, value, form) => "<input" + A({ type: "hidden", name, value, form: form || null }) + ">";

        // The form a transition link posts: an m-form for a transition, an m-form-link for a screen. With a form id it
        // holds only the hidden fields and a button elsewhere points to it.
        const linkForm = (n, parts, formId) => {
            if (parts.url.getDisableLink() || parts.anchor) return "";
            const isForm = parts.url.isTransition();
            const tag = isForm ? "m-form" : "m-form-link";
            const params = parts.url.getParameterMap();
            let out = "<" + tag + A({
                action: parts.url.path, name: formId || null, id: formId || null,
                method: isForm && parts.url.getTransitionMethod() === "get" ? "GET" : null,
                "read-only": isForm && parts.url.isReadOnly() ? true : null, target: at(n, "target-window") || null
            }) + ">";
            Object.keys(params).forEach((key) => { out += hiddenInput(key, params[key] === null || params[key] === undefined ? "" : params[key]); });
            if (!formId) {
                const confirmation = x(at(n, "confirmation"));
                const img = first(n, "image");
                if (img) {
                    out += "<input" + A({ type: "image", src: sri.makeUrlByType(at(img, "url"), at(img, "url-type") || "content", null, "true").url, alt: at(img, "alt") || null, "data-confirm": confirmation || null }) + ">";
                } else {
                    const linkType = at(n, "link-type");
                    out += "<q-btn" + A({
                        dense: true, "no-caps": true, type: "submit", flat: linkType === "hidden-form-link" ? true : null, outline: linkType === "hidden-form-link" ? null : true,
                        color: quasarColor(at(n, "btn-type") || "primary") || null, class: cls("m-link", xn(at(n, "style")) || null), "data-confirm": confirmation || null
                    }) + ">" + linkBody(n, parts.text, null, false) + "</q-btn>";
                }
            }
            return out + "</" + tag + ">";
        };

        const link = (n, inField) => {
            const parts = linkParts(n);
            if (!parts) {
                if (inField) w("&nbsp;");
                return;
            }
            const labelWrapper = at(n, "link-type") === "anchor" && inField && n.parent && n.parent.parent && n.parent.parent.parent && n.parent.parent.parent.name === "form-single";
            const open = labelWrapper ? "<m-field" + A({ label: label(n) }) + ">" : "";
            const close = labelWrapper ? "</m-field>" : "";
            if (inField) {
                const id = fid(n) + "_" + at(n, "url").replace(/\./g, "_");
                const form = linkForm(n, parts, id);
                if (form) sri.appendToAfterScreenWriter(form);
                w(open + linkElement(n, parts, id) + close);
                return;
            }
            const id = nodeId(n);
            w(linkForm(n, parts, id) + open + linkElement(n, parts, id) + close + "\n");
        };

        const editable = (n) => {
            const url = sri.makeUrlByType(at(n, "transition"), "transition", n, "true");
            const text = ec.resource.expand(at(n, "text"), "");
            if (!String(text).trim()) return;
            const load = first(n, "editable-load");
            let loadUrl = null;
            if (load) loadUrl = sri.makeUrlByType(at(load, "transition"), "transition", load, "true");
            w("<m-editable" + A({
                id: nodeId(n), "label-type": at(n, "type") || "span", "label-value": at(n, "encode") === "true" ? e(text).replace(/\n/g, "<br>") : text,
                url: url.url, ":url-parameters": js_(url.getParameterMap()), "parameter-name": at(n, "parameter-name") || "value",
                "widget-type": at(n, "widget-type") || "textarea", indicator: L("Saving"), tooltip: L("Click to edit"), cancel: L("Cancel"), submit: L("Save"),
                "load-url": loadUrl ? loadUrl.url : null, ":load-parameters": loadUrl ? js_(loadUrl.getParameterMap()) : null
            }) + "></m-editable>");
        };

        // ---- containers ----

        const containerDialog = (n) => {
            if (at(n, "condition") && !cond(at(n, "condition"))) return false;
            const buttonText = x(at(n, "button-text"));
            const id = nodeId(n);
            w("<m-container-dialog" + A({
                id, color: quasarColor(xn(at(n, "type") || "primary")) || null, width: at(n, "width") || null, "button-text": buttonText,
                "button-class": xn(at(n, "button-style")) || null, title: x(at(n, "title")) || buttonText, "open-dialog": ctx("_openDialog") === id ? true : null
            }) + ">");
            return true;
        };

        const dynamicDialog = (n) => {
            if (at(n, "condition") && !cond(at(n, "condition"))) return;
            const buttonText = x(at(n, "button-text"));
            const id = nodeId(n);
            const url = sri.makeUrlByType(at(n, "transition"), "transition", n, "true");
            const color = quasarColor(xn(at(n, "type") || "primary")) || null;
            if (url.getDisableLink()) {
                w("<q-btn" + A({ disabled: true, dense: true, outline: true, "no-caps": true, icon: "open_in_new", label: buttonText, color, class: xn(at(n, "button-style")) || null }) + "></q-btn>");
                return;
            }
            w("<m-dynamic-dialog" + A({
                id, url: url.getUrlWithParams(), color, width: at(n, "width") || null, "button-text": buttonText, "button-class": xn(at(n, "button-style")) || null,
                title: x(at(n, "title")) || buttonText, "open-dialog": ctx("_openDialog") === id ? true : null
            }) + "></m-dynamic-dialog>");
        };

        const dynamicContainer = (n) => {
            const id = nodeId(n);
            const url = sri.makeUrlByType(at(n, "transition"), "transition", n, "true").addParameter("_dynamic_container_id", id);
            w("<m-dynamic-container" + A({ id, url: url.getUrlWithParams() }) + "></m-dynamic-container>\n");
        };

        const tree = (n) => {
            let url;
            let items;
            if (at(n, "transition")) {
                url = sri.makeUrlByType(at(n, "transition"), "transition", n, "true");
                items = url.path;
            } else {
                url = sri.makeUrlByType("actions", "transition", n, "true");
                items = url.path + "/" + at(n, "name");
            }
            w("<m-tree-top" + A({
                id: at(n, "name"), items, "open-path": xn(at(n, "open-path")), ":parameters": js_(Object.assign({}, url.getParameterMap()))
            }) + "></m-tree-top>\n");
        };

        const subscreensTabs = (n) => {
            if (ctx("hideNav") === "true") return;
            if (at(n, "type") === "popup") return;
            w("<m-subscreens-tabs></m-subscreens-tabs>");
        };

        const subscreensPanel = (n) => {
            const type = at(n, "type");
            if (type === "popup") { w("<m-subscreens-active></m-subscreens-active>"); return; }
            if (type === "stack" || type === "wizard") { w("<h1>LATER " + e(type) + " type subscreens-panel not yet supported.</h1>"); return; }
            w("<div" + A({ id: at(n, "id") ? at(n, "id") + "-tabpanel" : null }) + ">" + (ctx("hideNav") === "true" ? "" : "<m-subscreens-tabs></m-subscreens-tabs>") + "<m-subscreens-active></m-subscreens-active></div>");
        };

        // ---- forms ----

        const paginateProps = (listName) => "{count:" + Number(ctx(listName + "Count") || 0) + ",pageIndex:" + Number(ctx(listName + "PageIndex") || 0) +
            ",pageSize:" + Number(ctx(listName + "PageSize") || 0) + ",pageMaxIndex:" + Number(ctx(listName + "PageMaxIndex") || 0) +
            ",pageRangeLow:" + Number(ctx(listName + "PageRangeLow") || 0) + ",pageRangeHigh:" + Number(ctx(listName + "PageRangeHigh") || 0) + "}";

        // section-iterate and section-include with paginate: the page keys of the list and the paginate control.
        const sectionPaginate = (listName) => {
            const list = ec.context.get(listName);
            const size = list && list.length !== undefined ? list.length : list && list.size ? list.size() : 0;
            const target = { pageIndex: ctx("pageIndex"), pageSize: ctx("pageSize") };
            if (!blank(ctx(listName + "Count"))) {
                ["Count", "PageIndex", "PageSize", "PageMaxIndex", "PageRangeLow", "PageRangeHigh"].forEach((suffix) => { target[listName + suffix] = ctx(listName + suffix); });
            } else CollectionUtilities.paginateParameters(size, listName, target);
            const count = Number(target[listName + "Count"] || 0);
            const props = "{count:" + count + ",pageIndex:" + Number(target[listName + "PageIndex"] || 0) + ",pageSize:" + Number(target[listName + "PageSize"] || 0) +
                ",pageMaxIndex:" + Number(target[listName + "PageMaxIndex"] || 0) + ",pageRangeLow:" + Number(target[listName + "PageRangeLow"] || 0) +
                ",pageRangeHigh:" + Number(target[listName + "PageRangeHigh"] || 0) + "}";
            w("<m-form-paginate" + A({ ":paginate": props }) + "></m-form-paginate>");
        };

        const visibleStyle = (when, fieldNameFn) => {
            if (!when) return null;
            let value = "";
            if (at(when, "from")) value = ec.resource.expression(at(when, "from"), "");
            else value = ec.resource.expand(at(when, "value"), "");
            if (blank(value)) return null;
            if (typeof value === "string" && value.indexOf(",") >= 0) value = value.split(",");
            const target = fieldRef(at(when, "field"));
            if (Array.isArray(value) || (value && typeof value === "object" && value.length !== undefined && typeof value !== "string")) {
                return "{display:(" + arr(Array.from(value)) + ".includes(" + target + ")?'':'none')}";
            }
            return "{display:(" + sq(value) + "==" + target + "?'':'none')}";
        };

        const formOpen = (n, formAttrs) => {
            push();
            st.formName = at(n, "name");
            st.owner = at(n, "owner-form");
            st.skipForm = false;
            const ownerForm = !!st.owner;
            const skipStart = at(n, "skip-start") === "true" || ownerForm;
            const url = sri.makeUrlByType(at(n, "transition"), "transition", null, "true");
            const formId = xn(at(n, "name")) + sec();
            st.fieldForm = formId;
            st.inForm = true;
            st.disabled = url.getDisableLink();
            const isForm = !url.isScreenUrl() || url.isTransition();
            const tag = isForm ? "m-form" : "m-form-link";
            const bodyParameters = at(n, "body-parameters") ? at(n, "body-parameters").split(",").map((item) => item.trim()).filter((item) => item) : null;
            const attrs = {
                name: formId, id: formId, action: url.path,
                method: isForm && url.getTransitionMethod() === "get" ? "GET" : null, "read-only": isForm && url.isReadOnly() ? true : null,
                "focus-field": at(n, "focus-field") || null, ":body-parameter-names": bodyParameters ? arr(bodyParameters) : null,
                "submit-message": isForm && at(n, "background-message") ? at(n, "background-message") : null,
                "submit-reload-id": isForm ? at(n, "background-reload-id") || null : null,
                "submit-hide-id": isForm ? at(n, "background-hide-id") || null : null,
                "exclude-empty-fields": at(n, "exclude-empty-fields") === "true" ? true : null,
                ":parent-checkbox-set": st.selectedForm ? "formProps" : null,
                "v-slot:default": "formProps", ":fields-initial": initial(singleValues(n, formAttrs)),
                ":validation-errors-initial": isForm ? savedErrors(at(n, "name")) : null
            };
            w((skipStart ? "" : '<div class="q-my-md"><' + tag + A(attrs) + ">"));
            return { skipStart, tag };
        };

        const formClose = (n, opened) => {
            if (!opened.skipStart) w("</" + opened.tag + "></div>\n");
            pop();
        };

        const fieldBox = (when, containerStyle, big) => {
            const style = visibleStyle(when);
            const box = big ? "q-mx-sm q-my-auto big-row-item" : cls("q-ma-sm", containerStyle || null);
            w("<div" + A({ class: box, ":style": style }) + ">");
        };

        const rowOpen = (kind, style) => w("<div" + A({ class: cls(kind, xn(style) || null) }) + ">");
        const colOpen = (n, plain) => {
            const hasLg = !!at(n, "lg");
            const hasMd = !!at(n, "md");
            const hasSm = !!at(n, "sm");
            const classes = [
                plain ? null : "q-px-xs", hasLg ? "col-lg-" + at(n, "lg") : null, hasMd ? "col-md-" + at(n, "md") : hasLg && !plain ? "col-md-12" : null,
                hasSm ? "col-sm-" + at(n, "sm") : (hasLg || hasMd) && !plain ? "col-sm-12" : null,
                at(n, "xs") ? "col-xs-" + at(n, "xs") : (hasLg || hasMd || hasSm) && !plain ? "col-xs-12" : null, xn(at(n, "style")) || null
            ];
            w("<div" + A({ class: cls(...classes) }) + ">");
        };

        const fieldGroup = (n) => {
            const title = L(at(n, "title") || "") || "";
            if (st.accordion) {
                st.accIndex++;
                const active = String(st.accIndex) === st.accActive;
                w("<q-list bordered padding><q-expansion-item" + A({ "switch-toggle-side": true, group: st.accordion, label: title || "Fields", "default-opened": active ? true : null }) +
                    '><div class="' + cls("panel-body", at(n, "style") || null) + '">');
            } else if (at(n, "box") === "true") {
                w('<div class="panel panel-default"><div class="panel-heading"><h5 class="panel-title">' + t(title || "Fields") + '</h5></div><div class="' + cls("panel-body", at(n, "style") || null) + '">');
            } else {
                w('<div class="' + cls("form-single-field-group", at(n, "style") || null) + '">' + (title ? "<h5>" + t(title) + "</h5>" : ""));
            }
        };
        const fieldGroupClose = (n) => {
            w(st.accordion ? "</div></q-expansion-item></q-list>\n" : at(n, "box") === "true" ? "</div></div>\n" : "</div>\n");
        };

        // ---- form-list ----

        const pageUrl = (name, value) => sri.getScreenUrlInstance().cloneUrlInstance().addParameter(name, value).getUrlWithParams();
        const pathUrl = (name, value) => sri.getScreenUrlInstance().cloneUrlInstance().addParameter(name, value).pathWithParams;
        const pageSizes = [10, 20, 50, 100, 200, 500];

        const pageSizeMenu = (listName) => {
            let out = '<span class="on-left q-my-auto"><q-btn-dropdown dense outline no-caps label="' + e(ctx(listName + "PageSize")) + '"><q-list dense>';
            pageSizes.forEach((size) => { out += '<q-item clickable v-close-popup><q-item-section><m-link href="' + e(pathUrl("pageSize", size)) + '">' + size + "</m-link></q-item-section></q-item>"; });
            return out + "</q-list></q-btn-dropdown></span>";
        };

        const isPaginated = (n) => {
            const listName = at(n, "list");
            const count = ctx(listName + "Count");
            const maxIndex = ctx(listName + "PageMaxIndex");
            return at(n, "paginate") !== "false" && count !== "" && count > 0 &&
                (!at(n, "paginate-always-show") || at(n, "paginate-always-show") === "true" || maxIndex > 0);
        };

        const nav = (n, cols, cfg) => {
            const listName = at(n, "list");
            const formId = cfg.formId;
            const paginated = isPaginated(n);
            if (!(cfg.headerDialog || cfg.savedFinds || cfg.selectColumns || paginated) || ctx("hideNav") === "true") return;
            let out = '<div class="form-list-nav-row row items-center"><div class="col-xs-12 col-sm-6"><div class="row items-center">';
            const flush = () => { if (out) { w(out); out = ""; } };
            if (cfg.headerDialog) {
                out += '<m-container-dialog id="' + e(formId) + '_hdialog" title="' + e(L("Find Options")) + '"><template v-slot:button><q-btn dense outline no-caps icon="search" label="' + e(L("Find Options")) + '"></q-btn></template>';
                flush();
                findDialogBody(n, cols, cfg);
                w("</m-container-dialog>");
            }
            if (cfg.selectColumns) {
                out += '<m-container-dialog id="' + e(formId) + '_SelColsDialog" title="' + e(L("Column Fields")) + '"><template v-slot:button><q-btn dense outline no-caps icon="table_chart" label="' + e(L("Columns")) + '"></q-btn></template>';
                flush();
                columnsDialogBody(n, cols, cfg);
                w("</m-container-dialog>");
            }
            if (cfg.savedFinds) {
                out += '<m-container-dialog id="' + e(formId) + '_sfdialog" title="' + e(L("Saved Finds")) + '"><template v-slot:button><q-btn dense outline no-caps icon="find_in_page" label="' + e(L("Saved Finds")) + '"></q-btn></template>';
                flush();
                savedFindsDialogBody(n, cfg);
                w("</m-container-dialog>");
            }
            if (at(n, "show-csv-button") === "true") {
                const csv = sri.getScreenUrlInstance().cloneUrlInstance().addParameter("renderMode", "csv").addParameter("pageNoLimit", "true").addParameter("lastStandalone", "true").addParameter("saveFilename", at(n, "name") + ".csv");
                out += '<q-btn dense outline type="a" href="' + e(csv.getUrlWithParams()) + '" label="' + e(L("CSV")) + '"></q-btn>';
            }
            if (at(n, "show-xlsx-button") === "true" && ec.screen.isRenderModeValid("xlsx")) {
                const xlsx = sri.getScreenUrlInstance().cloneUrlInstance().addParameter("renderMode", "xlsx").addParameter("pageNoLimit", "true").addParameter("lastStandalone", "true").addParameter("saveFilename", at(n, "name") + ".xlsx");
                out += '<q-btn dense outline type="a" href="' + e(xlsx.getUrlWithParams()) + '" label="' + e(L("XLS")) + '"></q-btn>';
            }
            if (at(n, "show-text-button") === "true") {
                flush();
                textDialog(n, cfg);
            }
            if (at(n, "show-pdf-button") === "true") {
                const print = sri.getScreenUrlInstance().cloneUrlInstance().addParameter("renderMode", "html-print").addParameter("pageNoLimit", "true");
                out += '<q-btn dense outline type="a" target="_blank" href="' + e(print.getUrlWithParams()) + '" label="' + e(L("Print")) + '"></q-btn>';
            }
            const count = ctx(listName + "Count");
            if ((count === "" ? 0 : count) === 0) {
                out += '<strong class="text-warning on-right q-my-auto">' + t(L(ec.context.get("_entityListNoSearchParms") ? "Find Options required to view results" : "No results found")) + "</strong>";
            }
            out += "</div></div>";
            if (paginated) {
                out += '<div class="col-xs-12 col-sm-6"><div class="row"><q-space></q-space>';
                if (at(n, "show-all-button") === "true" || at(n, "show-page-size") === "true") out += pageSizeMenu(listName);
                const maxIndex = Number(ctx(listName + "PageMaxIndex"));
                if (maxIndex > 4) out += '<m-form-go-page id-val="' + e(formId) + '" :max-index="' + maxIndex + '"></m-form-go-page>';
                out += '<m-form-paginate :paginate="' + e(paginateProps(listName)) + '"></m-form-paginate></div></div>';
            }
            w(out + "</div>\n");
            if (cfg.headerDialog) {
                const summaries = (cfg.allFields || []).map((field) => {
                    const value = field.summary ? field.summary() : "";
                    return value ? '<span style="white-space:nowrap;"><strong>' + t(field.title()) + ':</strong> <span class="text-positive">' + t(value) + '</span></span>' : "";
                }).filter(Boolean);
                w('<div class="form-list-filter-summary text-center q-pa-sm" style="font-weight:normal;">' + summaries.join(" "));
                if (summaries.length) {
                    w("<m-form-link" + A({ name: cfg.headerFormId + "_clr", id: cfg.headerFormId + "_clr", action: sri.getCurrentScreenUrl().path, ":fields-initial": initial(hiddenParams(n)) }) +
                        '><q-btn dense flat type="submit" icon="clear" color="negative"><q-tooltip>' + t(L("Reset to Default")) + '</q-tooltip></q-btn></m-form-link>');
                }
                w("</div>");
            }
        };

        // Saved finds have their own dialog, as in Java, separate from the search parameters.
        const savedFindsDialogBody = (n, cfg) => {
            const formId = cfg.formId;
            if (cfg.savedFinds) {
                const current = sri.getScreenUrlInstance().cloneUrlInstance().removeParameter("pageIndex").removeParameter("moquiFormName").removeParameter("moquiSessionToken").removeParameter("lastStandalone").removeParameter("formListFindId");
                const params = current.getParameterMap();
                const saveUrl = sri.buildUrl("formSaveFind").path;
                // The user's saved finds: a link that applies one, and buttons that update it to the current parameters,
                // make it the screen's default or no longer, or delete it.
                const finds = sri.getUserFormListFinds(at(n, "name"));
                if (finds.length) {
                    const activeId = ctx("formListFindId") || "";
                    const defaultId = sri.getUserDefaultFormListFindId() || "";
                    const base = sri.getScreenUrlInstance().path;
                    w('<q-list dense class="q-my-sm">');
                    finds.forEach((find) => {
                        const id = find.formListFind.formListFindId;
                        const label = (find.description || id) + (id === defaultId ? " (" + L("Default") + ")" : "");
                        const fields = Object.assign({ formLocation: at(n, "name"), formListFindId: id }, params);
                        const button = (name, text, color) => "<q-btn dense flat no-caps" + A({ type: "submit", name, value: name ? "Y" : null, color: color || null, label: text }) + "></q-btn>";
                        w("<q-item" + A({ active: id === activeId ? true : null }) + "><q-item-section><m-link" + A({ href: base + "?formListFindId=" + encodeURIComponent(id) }) + ">" + t(label) + "</m-link></q-item-section>" +
                            "<q-item-section side><m-form" + A({ class: "form-inline", action: saveUrl, ":fields-initial": js_(fields) }) + ">" +
                            (find.isByUserId === "true" && Object.keys(params).length ? button(null, L("Update to Current")) : "") +
                            (id === defaultId ? button("ClearDefault", L("Clear Default")) : button("MakeDefault", L("Make Default"))) +
                            (find.isByUserId === "true" ? button("DeleteFind", L("Delete"), "negative") : "") +
                            "</m-form></q-item-section></q-item>");
                    });
                    w("</q-list>");
                }
                if (Object.keys(params).length) {
                    w("<m-form" + A({
                        class: "form-inline", id: formId + "_NewFind", action: saveUrl, "v-slot:default": "formProps",
                        ":fields-initial": js_(Object.assign({ formLocation: at(n, "name"), _findDescription: "" }, params))
                    }) + '><div class="big-row"><div class="q-my-auto big-row-item"><m-text-line' + A({
                        "v-model": "formProps.fields._findDescription", name: "_findDescription", id: formId + "_NewFind_description", label: L("Description"), size: "30", required: true
                    }) + '></m-text-line></div><div class="on-right q-my-auto big-row-item"><q-btn dense outline no-caps type="submit" label="' + e(L("Save New Find")) + '"></q-btn></div></div></m-form>');
                } else w("<div style=\"margin:12px 0;\"><strong>" + t(L("No find parameters (or default), choose some in Find Options to save a new find or update existing")) + "</strong></div>");
            }
        };

        const findDialogBody = (n, cols, cfg) => {
            const headerId = cfg.headerFormId;
            if (!cfg.headerDialog) return;
            const orderable = cols.flat().filter((f) => f.orderBy);
            const orderOptions = [];
            orderable.forEach((f) => {
                const ci = f.orderBy === "case-insensitive" ? "^" : "";
                orderOptions.push({ value: ci + f.name, label: f.title() + " " + L("(Asc)") }, { value: "-" + ci + f.name, label: f.title() + " " + L("(Desc)") });
            });
            push();
            st.owner = headerId;
            st.fieldForm = headerId;
            st.inForm = true;
            w("<m-form-link" + A({
                name: headerId, id: headerId, action: sri.getCurrentScreenUrl().path, "v-slot:default": "formProps",
                ":fields-initial": initial(headerValues(n, cfg.valueDescs.h))
            }) + '><div class="q-mx-sm"><q-btn dense outline no-caps name="clearParameters" @click.prevent="formProps.clearForm" label="' + e(L("Clear Parameters")) + '"></q-btn>');
            w("<m-drop-down" + A({
                name: "orderByField", id: headerId + "_orderByField", label: L("Order By"), multiple: true, "allow-empty": true, ":options": js_(orderOptions), "v-model": "formProps.fields.orderByField"
            }) + "></m-drop-down></div>");
            sri.pushSingleFormMapContext("");
            try {
                cols.flat().forEach((f) => { if (f.dialogField) f.dialogField(); });
            } finally {
                sri.popContext();
            }
            w("</m-form-link>");
            pop();
        };

        const columnsDialogBody = (n, cols, cfg) => {
            const inColumns = new Set(cols.flat().map((f) => f.name));
            const notIn = cfg.allFields.filter((f) => !inColumns.has(f.name));
            const columns = [{ id: "hidden", label: L("Do Not Display"), children: notIn.map((f) => ({ id: f.name, label: f.title() })) }];
            cols.forEach((col, index) => columns.push({ id: "column_" + index, label: L("Column") + " " + (index + 1), children: col.map((f) => ({ id: f.name, label: f.title() })) }));
            const current = sri.getScreenUrlInstance().cloneUrlInstance().removeParameter("pageIndex").removeParameter("moquiFormName").removeParameter("moquiSessionToken").removeParameter("lastStandalone").removeParameter("formListFindId");
            w("<m-form-column-config" + A({
                id: cfg.formId + "_SelColsForm", action: sri.buildUrl("formSelectColumns").path, ":find-parameters": js_(current.getParameterMap()),
                ":columns-initial": js_(columns), "form-location": at(n, "name")
            }) + "></m-form-column-config>");
        };

        const textDialog = (n, cfg) => {
            const formId = cfg.formId;
            const url = sri.getScreenUrlInstance();
            const params = url.getParameterMap();
            const row = (title, name, value, extra) => '<div class="q-my-sm"><m-text-line' + A(Object.assign({ name, id: formId + "_Text_" + name, label: L(title), "model-value": value, form: formId + "_Text" }, extra || {})) + "></m-text-line></div>";
            w('<m-container-dialog id="' + e(formId) + '_TextDialog" button-text="' + e(L("Text")) + '" title="' + e(L("Export Fixed-Width Plain Text")) + '"><form' +
                A({ id: formId + "_Text", method: "post", action: url.url }) + ">" + hiddenInput("renderMode", "text") + hiddenInput("pageNoLimit", "true") + hiddenInput("lastStandalone", "true") +
                Object.keys(params).map((key) => hiddenInput(key, params[key])).join("") +
                row("Line Characters", "lineCharacters", "132", { size: "4" }) + row("Page Lines", "pageLines", "88", { size: "4" }) +
                row("Line Wrap?", "lineWrap", "true", { size: "5" }) + row("Save to Filename", "saveFilename", at(n, "name") + ".txt", { size: "40" }) +
                '<q-btn dense outline no-caps type="submit" label="' + e(L("Export Text")) + '"></q-btn></form></m-container-dialog>');
        };

        // The head of one column: for each field the title with its order-by links, and the find widgets of its header-field.
        const headerCell = (column, cfg) => {
            w('<div class="th text-left">');
            column.forEach((f) => {
                const align = f.align && f.align !== "left" ? f.align : null;
                let out = '<div><div class="' + cls("form-title", f.headerStyle || null, align === "center" ? "text-center" : align === "right" ? "text-right" : null) + '">' + t(f.submit ? "" : f.title());
                if (f.orderBy) {
                    const insensitive = f.orderBy === "case-insensitive";
                    let current = ctx("orderByField") || "";
                    if (current && String(current).indexOf(",") >= 0) {
                        const hit = String(current).split(",").filter((part) => part && part.indexOf(f.name) >= 0);
                        if (hit.length) current = hit[0];
                    }
                    current = String(current);
                    const mentioned = current && current.indexOf(f.name) >= 0;
                    const asc = mentioned && current.indexOf("-") !== 0;
                    const desc = mentioned && current.indexOf("-") === 0;
                    let ascUrl = pathUrl("orderByField", (insensitive ? "^" : "") + f.name);
                    let descUrl = pathUrl("orderByField", "-" + (insensitive ? "^" : "") + f.name);
                    if (asc) ascUrl = descUrl;
                    if (desc) descUrl = ascUrl;
                    out += '<span class="form-order-by"><m-link href="' + e(ascUrl) + '"' + (asc ? ' class="active"' : "") + '><i class="fa fa-caret-up"></i></m-link><m-link href="' + e(descUrl) + '"' + (desc ? ' class="active"' : "") +
                        '><i class="fa fa-caret-down"></i></m-link></span>';
                }
                out += "</div>";
                w(out);
                if (f.find && !cfg.headerDialog) {
                    w('<div class="' + cls("form-header-field", f.headerStyle || null) + '">');
                    f.find();
                    w("</div>");
                }
                w("</div>");
            });
            w("</div>");
        };

        const selectedRowCard = (cfg) => {
            if (!cfg.rowSelection) return;
            w('<q-card flat bordered><q-card-section horizontal class="q-pa-md">');
            cfg.rowSelection.actions.forEach((action) => {
                if (action.dialog) {
                    const d = action.dialog;
                    const buttonText = x(at(d, "button-text"));
                    w('<span class="q-my-md"><div class="q-ma-sm"><m-container-dialog' + A({
                        color: quasarColor(xn(at(d, "button-type") || "primary")) || null, width: at(d, "width") || null, title: x(at(d, "title")) || buttonText,
                        "button-text": buttonText, "button-class": xn(at(d, "button-style")) || null
                    }) + ">");
                }
                const wasSelected = st.selectedForm;
                st.selectedForm = true;
                action.render();
                st.selectedForm = wasSelected;
                if (action.dialog) w("</m-container-dialog></div></span>");
            });
            w("</q-card-section></q-card>");
        };

        const formList = (info, n, cols, cfg) => {
            push();
            const formId = xn(at(n, "name")) + sec();
            Object.assign(cfg, { formId, headerFormId: formId + "_header" });
            st.formName = at(n, "name");
            const skipStart = at(n, "skip-start") === "true";
            const skipEnd = at(n, "skip-end") === "true";
            const skipForm = at(n, "skip-form") === "true";
            const skipHeader = !skipStart && at(n, "skip-header") === "true";
            const needHeaderForm = !skipHeader && cfg.headerForm;
            cfg.headerDialog = needHeaderForm && at(n, "header-dialog") === "true";
            const multi = !skipForm && at(n, "multi") === "true";
            cfg.savedFinds = at(n, "saved-finds") === "true" && !sri.buildUrl("formSaveFind").getDisableLink();
            cfg.selectColumns = at(n, "select-columns") === "true" && !sri.buildUrl("formSelectColumns").getDisableLink();
            const shown = cols.map((col) => col.filter((f) => !f.hide || !f.hide())).filter((col) => col.length > 0);
            // The sub-list fields of an aggregated form-list have rows of their own under each main row.
            const visible = shown.map((col) => col.filter((f) => !f.sub)).filter((col) => col.length > 0);
            const subCols = shown.map((col) => col.filter((f) => f.sub)).filter((col) => col.length > 0);
            const listUrl = sri.makeUrlByType(at(n, "transition"), "transition", null, "false");
            const rowSel = cfg.rowSelection;
            const hidden = hiddenParams(n);
            const disabledList = listUrl.getDisableLink();
            st.multi = multi;
            st.skipForm = skipForm;
            st.fieldForm = formId;
            st.disabled = disabledList;
            let ids = [];
            if (rowSel) {
                for (let i = 0; i < info.count; i++) {
                    sri.startFormListRow(info, i);
                    try {
                        const id = ec.context.get(rowSel.idField);
                        ids.push(id === null || id === undefined ? "" : String(id));
                    } finally {
                        sri.endFormListRow();
                    }
                }
            }
            const checkboxAttrs = rowSel ? {
                "checkbox-parameter": rowSel.parameter || rowSel.idField, ":checkbox-list-mode": rowSel.listMode || "false", ":checkbox-values": arr(ids)
            } : {};

            if (!skipStart) {
                if (multi) {
                    w("<m-form" + A(Object.assign({
                        name: formId, id: formId, action: listUrl.path, "v-slot:default": "formProps",
                        ":fields-initial": initial(Object.assign({}, (() => {
                            const out = {};
                            Object.assign(out, listUrl.getParameterMap(), hidden);
                            for (let i = 0; i < info.count; i++) {
                                sri.startFormListRow(info, i);
                                try {
                                    st.entry = String(i);
                                    const row = fieldValues({}, cfg.valueDescs.r);
                                    Object.keys(row).forEach((key) => { out[key + "_" + i] = row[key]; });
                                } finally {
                                    st.entry = "";
                                    sri.endFormListRow();
                                }
                            }
                            out.moquiFormName = at(n, "name");
                            out.pageIndex = String(ctx("pageIndex") || "0");
                            if (ctx("orderByField")) out.orderByField = String(ctx("orderByField"));
                            out._isMulti = "true";
                            return out;
                        })()))
                    }, checkboxAttrs)) + ">");
                } else if (rowSel) {
                    w("<m-checkbox-set" + A(Object.assign({ ":checkbox-count": String(info.count), "v-slot:default": "formProps" }, checkboxAttrs)) + ">");
                }
                w('<div class="q-my-sm q-table__container q-table__card q-table--horizontal-separator q-table--dense q-table--flat" :class="{\'q-table--dark\':$q.dark.isActive, \'q-table__card--dark\':$q.dark.isActive, \'q-dark\':$q.dark.isActive}">');
                if (!skipHeader) nav(n, shown, cfg);
                w(' <div class="' + cls("table q-table", xn(at(n, "style")) || null) + '" id="' + e(formId) + '_table">');
                if (!skipHeader) {
                    w('<div class="thead"><div class="tr">');
                    if (rowSel) {
                        w('<div class="th"><span class="q-my-auto"><q-checkbox size="sm" v-model="formProps.checkboxAllState" @update:model-value="formProps.setCheckboxAllState"><q-tooltip>{{formProps.checkboxAllState ? ' + sq(L("Unselect All")) + " : " + sq(L("Select All")) +
                            '}}</q-tooltip></q-checkbox><q-btn dense flat icon="build" :color="formProps.checkboxStates && formProps.checkboxStates.includes(true) ? \'positive\' : \'\'"><q-tooltip>' + t(L("Row Actions")) + '</q-tooltip><q-menu anchor="top left" self="bottom left">');
                        selectedRowCard(cfg);
                        w("</q-menu></q-btn></span></div>");
                    }
                    if (needHeaderForm && !cfg.headerDialog) {
                        push();
                        st.owner = cfg.headerFormId;
                        st.fieldForm = cfg.headerFormId;
                        st.inForm = true;
                        w("<m-form-link" + A({
                            name: cfg.headerFormId, id: cfg.headerFormId, action: sri.getCurrentScreenUrl().path, "v-slot:default": "formProps",
                            ":fields-initial": initial(headerValues(n, cfg.valueDescs.h))
                        }) + ">");
                        visible.forEach((col) => headerCell(col, cfg));
                        w("</m-form-link>");
                        pop();
                    } else {
                        push();
                        st.inForm = false;
                        visible.forEach((col) => headerCell(col, cfg));
                        pop();
                    }
                    w("</div>");
                    // as Java: the titles of the sub-list columns once in the head, over the sub-rows of every row
                    if (subCols.length) {
                        w('<div class="tr sub-header">' + (rowSel ? '<div class="th">&nbsp;</div>' : "") + '<div class="th"><div class="q-pl-lg"><div class="row no-wrap q-col-gutter-sm text-weight-bold">');
                        subCols.forEach((col) => w('<div class="col">' + col.map((f) => t(f.title())).join(" ") + "</div>"));
                        w("</div></div></div></div>");
                    }
                    w("</div>\n");
                }
                w('<div class="tbody">\n');
                st.owner = formId;
            }

            const specialRow = (kind, suffix, mapName, transition, has) => {
                if (!has) return;
                const rowForm = !!transition;
                const rowUrl = rowForm ? sri.makeUrlByType(transition, "transition", null, "false") : null;
                sri.pushSingleFormMapContext(mapName || "");
                try {
                    push();
                    st.entry = suffix;
                    st.owner = formId + "_" + suffix;
                    st.inForm = rowForm;
                    if (rowForm) {
                        w("<m-form" + A({
                            name: formId + "_" + suffix, id: formId + "_" + suffix, action: rowUrl.path, "v-slot:default": "formProps",
                            ":fields-initial": initial(rowValues(n, cfg.valueDescs[suffix], rowUrl))
                        }) + ">");
                    }
                    w('<div class="tr ' + suffix + '">');
                    if (rowSel) w('<div class="td">&nbsp;</div>');
                    visible.forEach((col) => {
                        w('<div class="td">');
                        col.forEach((f) => { if (f[suffix]) f[suffix](); });
                        w("</div>");
                    });
                    w("</div>");
                    if (rowForm) w("</m-form>");
                    pop();
                } finally {
                    sri.popContext();
                }
            };
            specialRow("first", "first", at(n, "map-first-row"), at(n, "transition-first-row"), cfg.hasFirst);
            specialRow("second", "second", at(n, "map-second-row"), at(n, "transition-second-row"), cfg.hasSecond);

            for (let i = 0; i < info.count; i++) {
                sri.startFormListRow(info, i);
                try {
                    st.entry = String(i);
                    w('<div class="tr' + (ctx("_moquiRowType") === "total" ? " text-weight-bold" : "") + '">');
                    if (rowSel) {
                        w('<div class="td">' + (ids[i] ? '<div class="q-my-auto"><input style="transform:scale(1.3)" type="checkbox" v-model="formProps.checkboxStates[' + i + ']" @click="formProps.clickCheckbox($event, ' + i + ')"></div>' : "") + "</div>");
                    }
                    if (!(multi || skipForm)) {
                        st.owner = formId + "_" + i;
                        st.inForm = true;
                        w("<m-form" + A({
                            name: formId + "_" + i, id: formId + "_" + i, action: listUrl.path, "v-slot:default": "formProps",
                            ":fields-initial": initial(rowValues(n, cfg.valueDescs.r, listUrl))
                        }) + ">");
                    } else if (multi) {
                        st.owner = formId;
                        st.inForm = true;
                    } else st.inForm = false;
                    visible.forEach((col) => {
                        w('<div class="td">');
                        col.forEach((f) => f.cell(multi ? 1 : 0));
                        w("</div>");
                    });
                    if (!(multi || skipForm)) w("</m-form>");
                    w("</div>\n");
                    const subRows = subCols.length ? sri.formListSubRowCount(info, i) : 0;
                    if (subRows) {
                        const was = st.inForm;
                        st.inForm = false;
                        w('<div class="tr sub-rows">' + (rowSel ? '<div class="td">&nbsp;</div>' : "") + '<div class="td"><div class="q-pl-lg">');
                        for (let j = 0; j < subRows; j++) {
                            sri.startFormListSubRow(info, i, j);
                            try {
                                st.entry = i + "_" + j;
                                w('<div class="row no-wrap q-col-gutter-sm">');
                                subCols.forEach((col) => {
                                    w('<div class="col">');
                                    col.forEach((f) => f.cell(0));
                                    w("</div>");
                                });
                                w("</div>");
                            } finally {
                                sri.endFormListSubRow();
                            }
                            st.entry = String(i);
                        }
                        w("</div></div></div>\n");
                        st.inForm = was;
                    }
                } finally {
                    st.entry = "";
                    sri.endFormListRow();
                }
            }
            st.entry = "";
            st.owner = skipStart ? "" : formId;
            st.inForm = multi;
            sri.safeCloseList(info);
            specialRow("last", "last", at(n, "map-last-row"), at(n, "transition-last-row"), cfg.hasLast);

            if (!skipEnd) {
                if (multi && info.count > 0) {
                    w('<div class="tr"><div class="td">');
                    st.owner = formId;
                    st.inForm = true;
                    cols.forEach((col) => col.forEach((f) => f.cell(2)));
                    w("</div></div>");
                }
                if (isPaginated(n) && !skipHeader) {
                    const listName = at(n, "list");
                    w('<div class="tr form-list-nav-row"><div class="td"><div class="row"><q-space></q-space>');
                    if (at(n, "show-all-button") === "true" || at(n, "show-page-size") === "true") w(pageSizeMenu(listName));
                    const maxIndex = Number(ctx(listName + "PageMaxIndex"));
                    if (maxIndex > 4) w('<m-form-go-page id-val="' + e(formId) + '" :max-index="' + maxIndex + '"></m-form-go-page>');
                    w('<m-form-paginate :paginate="' + e(paginateProps(listName)) + '"></m-form-paginate></div></div></div>');
                }
                w("</div></div></div>\n");
                if (multi) w("</m-form>\n");
                else if (rowSel && !skipStart) w("</m-checkbox-set>\n");
            }
            pop();
        };

        // A list cell for one field. The final row of a multi form has none.
        const rowCellOpen = (style, align, final) => {
            if (!final) w("<div" + A({ class: cls("form-group", style || null, align === "right" ? "text-right" : align === "center" ? "text-center" : null) }) + ">");
        };
        const rowCellClose = (final) => {
            if (!final) w("</div>");
        };

        const valueText = (n) => {
            const type = n.name;
            const name = fnm(n);
            if (["display", "display-entity", "hidden", "ignored", "password", "reset", "submit", "text-area", "link", "label"].indexOf(type) >= 0) return "";
            if (type === "drop-down" || type === "check" || type === "radio") {
                const current = type === "drop-down" ? sri.getFieldValuePlainString(fieldOf(n), "") : sri.getFieldValueString(n);
                if (!current) return "";
                const options = sri.getFieldOptions(n);
                const text = current.split(",").map((part) => options.get(part) || part).join(", ");
                return text + (param(name + "_not") === "Y" ? " (" + L("Not") + ")" : "");
            }
            if (type === "date-period") {
                const period = String(ctx(name + "_period")).toLowerCase();
                if (period) return period + " " + (ctx(name + "_poffset") || "0") + " " + ctx(name + "_pdate");
            }
            if (type === "date-find" || type === "date-period" || type === "range-find") {
                const format = type === "date-find" ? dateDefault(n) : "yyyy-MM-dd";
                const from = type === "range-find" ? ctx(name + "_from") : formatted(ctx(name + "_from"), format);
                const thru = type === "range-find" ? ctx(name + "_thru") : formatted(ctx(name + "_thru"), format);
                return (from ? L("From") + " " + from : "") + (thru ? " " + L("to") + " " + thru : "");
            }
            return sri.getFieldValueString(n);
        };

        const h = {
            reset, push, pop, e, t, A, cls, at, x, xn, nodeId, cond, fid, fnm, fieldRef, st,
            textLine, textArea, password, file, hidden, reset_, submit, dateTime, dateFind, datePeriod, rangeFind, textFind,
            dropDown, check: (n) => checkOrRadio(n, "checkbox"), radio: (n) => checkOrRadio(n, "radio"), display: displayWidget, displayEntity,
            image, label: labelWidget, link, editable, containerDialog, dynamicDialog, dynamicContainer, tree,
            subscreensTabs, subscreensPanel, formOpen, formClose, fieldBox, rowOpen, colOpen, fieldGroup, fieldGroupClose,
            formList, rowCellOpen, rowCellClose, valueText, sectionPaginate, headerCell,
            accordion: (id, active) => { st.accordion = id; st.accIndex = 0; st.accActive = active; },
            endAccordion: () => { st.accordion = ""; },
            quasarColor, icon, tooltipTag, js: js_
        };
        Object.defineProperty(globalThis, "$mqQvt", { value: h, configurable: true });
    };

    const runtime = "(" + runtimeFunction.toString() + ")();";
    const prelude = "const $v = globalThis.$mqQvt;\n$v.reset();\n";

    // ---- macros ----

    const widgetCall = (name, node, withChildren) => `$v.${name}(${wlit(node, withChildren)});\n`;
    const condGuard = (condition, body) => (condition ? js`
        if (${raw(condition)}) {
            ${body}
        }
    ` : body);

    const widgetsMacro = (what) => stmt((node, children) => js`
        if (sri.doBoundaryComments()) sri.write("<!-- BEGIN screen[@location=" + sri.getActiveScreenDef().location + "].${what} -->\\n");
        ${children}
        if (sri.doBoundaryComments()) sri.write("<!-- END   screen[@location=" + sri.getActiveScreenDef().location + "].${what} -->\\n");
    `);

    const subWidgets = (sub) => widgetsSource(sub);

    // The descriptors of the values of a field, for the fields-initial of its form. kind r picks among the
    // conditional-field elements and the default-field, and the other kinds name the one sub-node.
    const valueDesc = (field, kind) => {
        const subs = [];
        const make = (sub, condition) => {
            const widgets = sub.children.filter((widget) => widget.name !== "set");
            const sets = sub.children.filter((widget) => widget.name === "set");
            subs.push(`{"cond": ${condition === null ? "null" : lit(condition)}, "sets": [${sets.map(bareLit).join(",")}], "widgets": [${widgets.map((widget) => wlit(widget, true)).join(",")}]}`);
        };
        if (kind === "r") {
            childrenNamed(field, "conditional-field").forEach((sub) => { if (attr(sub, "condition")) make(sub, attr(sub, "condition")); });
            const dflt = child(field, "default-field");
            if (dflt) make(dflt, null);
        } else {
            const tag = { h: "header-field", first: "first-row-field", second: "second-row-field", last: "last-row-field" }[kind];
            const sub = child(field, tag);
            if (sub) make(sub, null);
        }
        if (!subs.length) return null;
        return `{"name": ${lit(attr(field, "name"))}, "field": ${fieldLitOf(field)}, "subs": [${subs.join(",")}]}`;
    };
    const valueDescs = (fields, kind) => `[${fields.map((field) => valueDesc(field, kind)).filter((text) => text).join(",\n")}]`;

    // The code of one field of a form-single.
    const singleFieldCode = (field, bigRow) => {
        const code = fieldChain(field, (sub) => {
            if (child(sub, "ignored")) return "";
            if (child(sub, "hidden")) return subWidgets(sub);
            const style = attr(sub, "container-style");
            const when = child(sub, "visible-when");
            return js`
                $v.fieldBox(${when ? bareLit(when) : "null"}, ${style ? noL10n(style) : '""'}, ${!!bigRow});
                sri.pushContext();
                try {
                    ${setCode(sub)}
                    ${subWidgets(sub)}
                } finally {
                    sri.popContext();
                }
                sri.write("</div>");
            `;
        });
        const hide = attr(field, "hide");
        if (!hide || !code) return code;
        return js`
            if (!ec.resource.condition(${lit(hide)}, "")) {
                ${code}
            }
        `;
    };

    const layoutCode = (node, fields, referenced, big) => node.children.map((kid) => {
        const name = kid.name;
        if (name === "field-ref") {
            const field = fields[attr(kid, "name")];
            if (!field) return `sri.write(${lit(`<div>Error: could not find field with name ${attr(kid, "name")} referred to in a field-ref.name attribute.</div>\n`)});\n`;
            return singleFieldCode(field, big);
        }
        if (name === "fields-not-referenced") {
            return Object.keys(fields).filter((fieldName) => !referenced.has(fieldName)).map((fieldName) => singleFieldCode(fields[fieldName], big)).join("");
        }
        if (name === "field-row") {
            return js`
                sri.write("<div class=\\"row\\">");
                ${kid.children.map((cell) => `sri.write("<div class=\\"col-sm-6\\">");\n` + layoutCode({ children: [cell] }, fields, referenced, false) + `sri.write("</div>");\n`).join("")}
                sri.write("</div>\\n");
            `;
        }
        if (name === "field-row-big") {
            const title = attr(kid, "title");
            const justify = attr(kid, "justify");
            return js`
                sri.write("<div class=\\"q-my-sm big-row\\"" + ${justify ? lit(` style="justify-content:${justify};"`) : '""'} + ">");
                ${title ? `sri.write("<div class=\\"q-mx-sm q-my-auto big-row-item\\">" + $v.t(ec.resource.expand(${lit(title)}, "")) + "</div>");` : ""}
                ${layoutCode(kid, fields, referenced, true)}
                sri.write("</div>\\n");
            `;
        }
        if (name === "field-group") {
            return js`
                $v.fieldGroup(${bareLit(kid)});
                ${layoutCode(kid, fields, referenced, big)}
                $v.fieldGroupClose(${bareLit(kid)});
            `;
        }
        if (name === "field-accordion") {
            const id = attr(kid, "id");
            return js`
                $v.accordion(${id ? lit(id) : `$v.st.fieldForm + "_accordion"`}, ${lit(attr(kid, "active") || "1")});
                sri.write("<q-list>");
                ${layoutCode(kid, fields, referenced, big)}
                sri.write("</q-list>\\n");
                $v.endAccordion();
            `;
        }
        if (name === "field-col-row") {
            return js`
                $v.rowOpen("row", ${lit(attr(kid, "style"))});
                ${childrenNamed(kid, "field-col").map((col) => js`
                    $v.colOpen(${bareLit(col)}, true);
                    ${layoutCode(col, fields, referenced, false)}
                    sri.write("</div>");
                `).join("")}
                sri.write("</div>\\n");
            `;
        }
        return "";
    }).join("");

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
            layoutCode(layout, fields, collectRefs(layout, new Set()), false) :
            Object.keys(fields).map((name) => singleFieldCode(fields[name], false)).join("");
        return js`
            if (sri.doBoundaryComments()) sri.write(${lit(`<!-- BEGIN form-single[@name=${attr(node, "name")}] -->\n`)});
            sri.pushSingleFormMapContext(${raw(map)});
            try {
                {
                    const $form = $v.formOpen(${bareLit(node)}, ${valueDescs(childrenNamed(node, "field"), "r")});
                    ${body}
                    $v.formClose(${bareLit(node)}, $form);
                }
            } finally {
                sri.popContext();
            }
            if (sri.doBoundaryComments()) sri.write(${lit(`<!-- END   form-single[@name=${attr(node, "name")}] -->\n`)});
        `;
    });

    // A list cell for one field: the widgets of the first conditional-field that holds, else the default-field.
    // $m is 0 for a row, 1 for a row of a form that posts all rows, and 2 for the final row, which holds the
    // submit widgets of such a form.
    const cellCode = (field) => fieldChain(field, (sub) => {
        if (child(sub, "ignored")) return "";
        const hide = attr(field, "hide");
        const isSubmit = !!child(sub, "submit");
        const isHidden = !!child(sub, "hidden");
        const style = attr(sub, "container-style");
        const align = attr(field, "align");
        const inner = isHidden ? subWidgets(sub) : js`
            $v.rowCellOpen(${style ? noL10n(style) : '""'}, ${lit(align)}, $m === 2);
            sri.pushContext();
            try {
                ${setCode(sub)}
                ${subWidgets(sub)}
            } finally {
                sri.popContext();
            }
            $v.rowCellClose($m === 2);
        `;
        const guard = isSubmit ? "$m !== 1" : "$m !== 2";
        return js`
            if (${guard}${hide ? ` && !ec.resource.condition(${lit(hide)}, "")` : ""}) {
                ${inner}
            }
        `;
    });

    const sideCode = (field, tag) => {
        const sub = child(field, tag);
        if (!sub || !sub.children.length) return null;
        if (child(sub, "hidden") || child(sub, "ignored")) return null;
        const hide = attr(field, "hide");
        const body = js`
            $v.rowCellOpen("", ${lit(attr(field, "align"))}, false);
            sri.pushContext();
            try {
                ${setCode(sub)}
                ${subWidgets(sub)}
            } finally {
                sri.popContext();
            }
            $v.rowCellClose(false);
        `;
        return `() => {\n${hide ? `if (ec.resource.condition(${lit(hide)}, "")) return;\n` : ""}${body}}`;
    };

    const dialogFieldCode = (field) => {
        const sub = child(field, "header-field");
        if (!sub || !sub.children.length || child(sub, "ignored")) return null;
        const isHidden = !!child(sub, "hidden");
        const style = attr(sub, "container-style");
        const when = child(sub, "visible-when");
        const body = isHidden ? subWidgets(sub) : js`
            $v.fieldBox(${when ? bareLit(when) : "null"}, ${style ? noL10n(style) : '""'}, false);
            sri.pushContext();
            try {
                ${setCode(sub)}
                ${subWidgets(sub)}
            } finally {
                sri.popContext();
            }
            sri.write("</div>");
        `;
        return `() => {\n${body}}`;
    };

    const summaryCode = (field) => {
        const header = child(field, "header-field");
        if (!header || !header.children.length || child(header, "hidden") || child(header, "ignored")) return "null";
        const hide = attr(field, "hide");
        return `() => { ${hide ? `if (${hide}) return "";` : ""} let $t = ""; sri.pushContext(); try { ${setCode(header)} ${header.children.filter((w) => w.name !== "set" && w.name !== "link").map((w) => `$t += $v.valueText(${wlit(w, true)});`).join(" ")} } finally { sri.popContext(); } return $t.trim(); }`;
    };

    const columnField = (field) => {
        const header = S.headerSub(field);
        const headerField = child(field, "header-field");
        const hide = attr(field, "hide");
        const orderBy = header && (attr(header, "show-order-by") === "true" || attr(header, "show-order-by") === "case-insensitive") ? attr(header, "show-order-by") : "";
        const find = sideCode(field, "header-field");
        const first = sideCode(field, "first-row-field");
        const second = sideCode(field, "second-row-field");
        const last = sideCode(field, "last-row-field");
        const summary = summaryCode(field);
        return js`
            {
                "name": ${lit(attr(field, "name"))},
                "align": ${lit(attr(field, "align"))},
                "headerStyle": ${noL10n(headerField ? attr(headerField, "container-style") : "")},
                "orderBy": ${lit(orderBy)},
                "sub": ${attr(field, "aggregate") === "sub-list"},
                "submit": ${!!(header && child(header, "submit"))},
                ${hide ? `"hide": () => (${raw(hide)}),` : ""}
                "title": () => ${titleCode(header)},
                "find": ${find || "null"},
                "dialogField": ${dialogFieldCode(field) || "() => {}"},
                "summary": ${summary},
                ${first ? `"first": ${first},` : ""}
                ${second ? `"second": ${second},` : ""}
                ${last ? `"last": ${last},` : ""}
                "cell": ($m) => {
                    ${cellCode(field)}
                }
            }
        `.replace(/\n$/, "");
    };

    const formListMacro = stmt((node) => {
        const fields = childrenNamed(node, "field");
        const columns = S.formListColumns(node);
        const hasRow = (tag) => fields.some((field) => { const row = child(field, tag); return row && row.children.length > 0; });
        const selection = child(node, "row-selection");
        let rowSelection = "null";
        if (selection) {
            const actions = childrenNamed(selection, "action").map((action) => {
                const dialog = child(action, "dialog");
                const form = child(action, "form-single");
                return `{"dialog": ${dialog ? bareLit(dialog) : "null"}, "render": () => {\n${form ? form.source : ""}}}`;
            });
            rowSelection = `{"idField": ${lit(attr(selection, "id-field"))}, "parameter": ${lit(attr(selection, "parameter"))}, "listMode": ${lit(attr(selection, "list-mode"))}, "actions": [${actions.join(",\n")}]}`;
        }
        const cfg = js`
            {
                "hasFirst": ${hasRow("first-row-field")},
                "hasSecond": ${hasRow("second-row-field")},
                "hasLast": ${hasRow("last-row-field")},
                "headerForm": ${fields.some((field) => { const sub = child(field, "header-field"); return sub && sub.children.length > 0; })},
                "rowSelection": ${rowSelection},
                "valueDescs": {
                    "r": ${valueDescs(fields, "r")},
                    "h": ${valueDescs(fields, "h")},
                    "first": ${valueDescs(fields, "first")},
                    "second": ${valueDescs(fields, "second")},
                    "last": ${valueDescs(fields, "last")}
                },
                "allFields": [
                    ${fields.filter((field) => !S.listFieldHiddenWidget(field)).map((field) => `{"name": ${lit(attr(field, "name"))}, "title": () => ${titleCode(S.headerSub(field))}, "summary": ${summaryCode(field)}}`).join(",\n")}
                ]
            }
        `.replace(/\n$/, "");
        // A field with only a hidden widget has no column, and its value goes in the fields-initial of the row.
        const cols = columns.map((column) => "[" + column.map(columnField).join(",\n") + "]").join(",\n");
        return js`
            if (sri.doBoundaryComments()) sri.write(${lit(`<!-- BEGIN form-list[@name=${attr(node, "name")}] -->\n`)});
            {
                const $info = sri.openFormList(${lit(attr(node, "name"))});
                $v.formList($info, ${bareLit(node)}, [
                    ${cols}
                ], ${cfg});
            }
            if (sri.doBoundaryComments()) sri.write(${lit(`<!-- END   form-list[@name=${attr(node, "name")}] -->\n`)});
        `;
    });

    const widget = (name, withChildren) => stmt((node) => widgetCall(name, node, withChildren));

    const nodeIdCode = (node) => (attr(node, "id") ? `$v.nodeId(${bareLit(node)})` : "null");

    const containerMacro = stmt((node, children) => {
        const type = attr(node, "type") || "div";
        const style = attr(node, "style");
        return js`
            sri.write("<" + ${lit(type)} + $v.A({id: ${nodeIdCode(node)}, class: ${style ? noL10n(style) : "null"}}) + ">");
            ${children}
            sri.write(${lit(`</${type}>\n`)});
        `;
    });

    const containerBoxMacro = stmt((node) => {
        const header = child(node, "box-header");
        const body = child(node, "box-body");
        const nopad = child(node, "box-body-nopad");
        const toolbar = child(node, "box-toolbar");
        const title = header ? attr(header, "title") : "";
        return js`
            {
                const $type = ${noL10n(attr(node, "type") || "default")} || "default";
                sri.write("<m-container-box" + $v.A({id: ${nodeIdCode(node)}, type: $type, title: ${title ? `ec.resource.expand(${lit(title)}, "")` : "null"},
                    "initial-open": ${noL10n(attr(node, "initial") || "")} === "closed" ? "false" : null}) + ">");
            }
            ${header ? `sri.write("<template v-slot:header>");\n${header.source}sri.write("</template>");` : ""}
            ${toolbar ? `sri.write("<template v-slot:toolbar>");\n${toolbar.source}sri.write("</template>");` : ""}
            ${body ? `sri.write(${lit(`<m-box-body${attr(body, "height") ? ` height="${attr(body, "height")}"` : ""}>`)});\n${body.source}sri.write("</m-box-body>");` : ""}
            ${nopad ? nopad.source : ""}
            sri.write("</m-container-box>\\n");
        `;
    });

    const containerRowMacro = stmt((node) => js`
        $v.rowOpen("row", ${lit(attr(node, "style"))});
        ${childrenNamed(node, "row-col").map((col) => js`
            $v.colOpen(${bareLit(col)}, false);
            ${col.source}
            sri.write("</div>");
        `).join("")}
        sri.write("</div>\\n");
    `);

    const containerPanelMacro = stmt((node) => {
        const part = (name) => { const kid = child(node, name); return kid ? kid.source : ""; };
        const id = attr(node, "id");
        const sized = (name, cls) => {
            const kid = child(node, name);
            return kid ? `sri.write(${lit(`<div class="${cls}" style="width:${attr(kid, "size") || "180"}px;">`)});\n${kid.source}sri.write("</div>");\n` : "";
        };
        const centerClass = child(node, "panel-left") ? (child(node, "panel-right") ? "container-panel-center-both" : "container-panel-center-left") :
            (child(node, "panel-right") ? "container-panel-center-right" : "container-panel-center-only");
        return js`
            sri.write("<div" + $v.A({id: ${nodeIdCode(node)}, class: "container-panel-outer"}) + ">");
            ${child(node, "panel-header") ? `sri.write("<div class=\\"container-panel-header\\">");\n${part("panel-header")}sri.write("</div>");` : ""}
            sri.write("<div class=\\"container-panel-middle\\">");
            ${sized("panel-left", "container-panel-left")}
            sri.write("<div class=\\"${centerClass}\\">");
            ${part("panel-center")}
            sri.write("</div>");
            ${sized("panel-right", "container-panel-right")}
            sri.write("</div>");
            ${child(node, "panel-footer") ? `sri.write("<div class=\\"container-panel-footer\\">");\n${part("panel-footer")}sri.write("</div>");` : ""}
            sri.write("</div>\\n");
        `;
    });

    const containerDialogMacro = stmt((node, children) => js`
        if ($v.containerDialog(${bareLit(node)})) {
            ${children}
            sri.write("</m-container-dialog>\\n");
        }
    `);

    const buttonMenuMacro = stmt((node) => {
        const condition = attr(node, "condition");
        const image = child(node, "image");
        const linkItems = node.children.filter((kid) => kid.name !== "image" && kid.name !== "parameter");
        return condGuard(condition, js`
            {
                const $map = ${attr(node, "text-map") ? `ec.resource.expression(${lit(attr(node, "text-map"))}, "")` : "null"};
                let $text = $map ? ec.resource.expand(${lit(attr(node, "text"))}, "", $map) : ec.resource.expand(${lit(attr(node, "text"))}, "");
                if ($text === "null") $text = "";
                if ($text || ${!!image} || ${!!attr(node, "icon")}) {
                    if (${lit(attr(node, "encode"))} !== "false") $text = $v.t($text);
                    let $icon = ${lit(attr(node, "icon"))};
                    if (!$icon && $text) $icon = sri.getThemeIconClass($text) || "";
                    const $badge = ec.resource.expand(${lit(attr(node, "badge"))}, "");
                    sri.write("<q-btn-dropdown" + $v.A({dense: true, outline: true, "no-caps": true, color: $v.quasarColor(${lit(attr(node, "btn-type") || "primary")}) || null, class: ${attr(node, "style") ? noL10n(attr(node, "style")) : "null"}}) + "><template v-slot:label>" +
                        ${attr(node, "tooltip") ? `$v.tooltipTag(ec.resource.expand(${lit(attr(node, "tooltip"))}, "")) + ` : ""}
                        ($icon ? "<i class=\\"" + $v.e($icon) + " q-icon" + ($text ? " on-left" : "") + "\\"></i>" : ""));
                    // as Java: the image child in the place of the text
                    ${image ? image.source : "sri.write($text);"}
                    sri.write(($badge ? "<q-badge class=\\"on-right\\" transparent>" + $v.t($badge) + "</q-badge>" : "") + "</template><q-list>");
                    ${linkItems.map((kid) => condGuard(attr(kid, "condition"), `sri.write("<q-item><q-item-section>");\n${kid.source}sri.write("</q-item-section></q-item>");\n`)).join("")}
                    sri.write("</q-list></q-btn-dropdown>\\n");
                }
            }
        `);
    });

    const linkMacro = stmt((node) => {
        const inField = node.parent && node.parent.name.endsWith("-field");
        return `$v.link(${wlit(node, true)}, ${inField});\n`;
    });

    const submitMacro = stmt((node) => `$v.submit(${wlit(node, true)});\n`);
    const resetMacro = stmt((node) => `$v.reset_(${wlit(node, false)});\n`);

    const sectionIterateMacro = stmt((node) => {
        const name = attr(node, "name");
        const code = `sri.renderSection(${lit(name)});\n`;
        if (attr(node, "paginate") !== "true") return code;
        return `$v.sectionPaginate(${lit(attr(node, "list"))});\n${code}`;
    });

    const sectionIncludeMacro = stmt((node) => js`
        {
            const $included = sri.getSectionIncludedNode(${bareLit(node)});
            if ($included.attrs.paginate === "true") $v.sectionPaginate($included.attrs.list);
            sri.renderSectionInclude(${bareLit(node)});
        }
    `);

    const tableOfFormNoops = noops(S.allElements);

    const macros = Object.assign({}, tableOfFormNoops, S.anyMacros(MODE), {
        "screen": stmt((node, children) => children),
        "widgets": widgetsMacro("widgets"),
        "fail-widgets": widgetsMacro("fail-widgets"),
        "dynamic-form": stmt((node, children) => children),
        "subscreens-menu": widget("subscreensTabs", false),
        "subscreens-panel": widget("subscreensPanel", false),
        "subscreens-active": stmt(() => `sri.write("<m-subscreens-active></m-subscreens-active>");\n`),
        "section-iterate": sectionIterateMacro,
        "section-include": sectionIncludeMacro,

        "container": containerMacro,
        "container-box": containerBoxMacro,
        "box-header": stmt((node, children) => children),
        "box-toolbar": stmt((node, children) => children),
        "box-body": stmt((node, children) => children),
        "box-body-nopad": stmt((node, children) => children),
        "container-row": containerRowMacro,
        "row-col": stmt((node, children) => children),
        "container-panel": containerPanelMacro,
        "panel-header": stmt((node, children) => children),
        "panel-left": stmt((node, children) => children),
        "panel-center": stmt((node, children) => children),
        "panel-right": stmt((node, children) => children),
        "panel-footer": stmt((node, children) => children),
        "container-dialog": containerDialogMacro,
        "dynamic-container": widget("dynamicContainer"),
        "dynamic-dialog": widget("dynamicDialog"),
        "button-menu": buttonMenuMacro,
        "menu-item": stmt(() => ""),
        "tree": widget("tree"),
        "link": linkMacro,
        "image": widget("image", true),
        "label": widget("label", false),
        "editable": widget("editable"),

        "form-single": formSingleMacro,
        "form-list": formListMacro,
        "field": stmt((node, children) => children),
        "conditional-field": stmt((node, children) => children),
        "default-field": stmt((node, children) => children),
        "header-field": stmt((node, children) => children),
        "first-row-field": stmt((node, children) => children),
        "second-row-field": stmt((node, children) => children),
        "last-row-field": stmt((node, children) => children),
        "row-selection": stmt(() => ""),
        "action": stmt((node, children) => children),

        "check": widget("check"),
        "date-find": widget("dateFind", false),
        "date-period": widget("datePeriod", false),
        "date-time": widget("dateTime", false),
        "display": widget("display", true),
        "display-entity": widget("displayEntity", false),
        "drop-down": widget("dropDown"),
        "file": widget("file", false),
        "hidden": widget("hidden", false),
        "lookup": widget("textLine", false),
        "password": widget("password", false),
        "radio": widget("radio"),
        "range-find": widget("rangeFind", false),
        "reset": resetMacro,
        "submit": submitMacro,
        "text-area": widget("textArea", false),
        "text-find": widget("textFind", false),
        "text-line": widget("textLine", true)
    });

    // button-menu hands the macros of its items to its own macro. The row-selection actions are forms whose code
    // the form-list macro places inside its menu, so they are generated.
    const opaque = S.opaqueElements.filter((name) => !["button-menu", "row-selection", "action"].includes(name));

    return { macros, opaque, prelude, runtime };
})
