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

// The qvt render mode when the screen is server-static (plans/QvtServerStatic.md): a Vue template that holds no request
// data. It is the transcription of the screen. An expression is not evaluated here: it is written into the template and
// the browser evaluates it against the scope chain, which is the screen package, the map of a form-single, the entry of a
// section-iterate and the row of a form-list. The generated units run once when the template is cached, with no actions,
// and the text they write is compiled by the server into a render function.
//
// What the template does with the scope chain:
//  - a bare name in an expression is a name of the chain (the compiler writes _ctx.name, and an m-scope or the row slot of
//    an m-static-form-list declares its own _ctx, so the innermost scope wins as the context stack does on the server);
//  - ec is the restricted browser host (js/README.md): localize, format, formatCurrency, formatCurrencyNoSymbol, a snapshot
//    of the user, and resource.expand, which takes the table of the expressions of its text so no eval is needed;
//  - $scope is the chain itself, for a name that is not an identifier, and $mq holds the few helpers the template calls.
//
// Differences from ScreenQvt.js (the server-merged module):
//  - nothing is expanded or evaluated: conditions are v-if, section-iterate is v-for, text is v-text of a localized call;
//  - a form starts from fields-initial expressions over the scope and a form-list rows come from /actions/{form};
//  - display-entity, entity-options, list-options and dynamic-options (without server-search) read keys the package
//    holds: name_display, and mqOpt_{form}_{field}_{sub}_{widget} for the options of a widget;
//  - a link that the user may not use is still written (the server refuses the request), and no icon comes from the theme;
//  - include-screen is written inline. A form-list writes the find dialog, saved finds, the exports, a multi form, the column picker,
//    row-selection, the first/second/last rows and aggregated sub-rows; their data comes with the form request.
(function (T, S) {
    "use strict";

    const { attr, jsString, child, childrenNamed, stmt, noops } = T;

    const MODE = "qvt";
    const lit = jsString;

    const childLit = (node) => {
        const parts = [`"name":${lit(node.name)}`, `"attrs":${JSON.stringify(node.attrs || {})}`];
        if (node.text) parts.push(`"text":${lit(node.text)}`);
        if (node.children.length) parts.push(`"children":[${node.children.map(childLit).join(",")}]`);
        return `{${parts.join(",")}}`;
    };
    const parentChain = (node) => {
        if (!node.parent) return "";
        const parts = [`"name":${lit(node.parent.name)}`, `"attrs":${JSON.stringify(node.parent.attrs || {})}`];
        const up = parentChain(node.parent);
        if (up) parts.push(up);
        return `"parent":{${parts.join(",")}}`;
    };
    // Whether the element is below a field, whose name and form the runtime reads through the chain of parents.
    const inField = (node) => {
        let up = node.parent;
        for (let depth = 0; up && depth < 6; depth++) {
            if (up.name === "field") return true;
            up = up.parent;
        }
        return false;
    };
    // The element with its subtree, and for a widget of a field the chain of parents (attributes only).
    const wlit = (node, withChildren) => {
        const parts = [`"name":${lit(node.name)}`, `"attrs":${JSON.stringify(node.attrs || {})}`];
        if (node.text) parts.push(`"text":${lit(node.text)}`);
        if (withChildren !== false && node.children.length) parts.push(`"children":[${node.children.map(childLit).join(",")}]`);
        const up = inField(node) ? parentChain(node) : "";
        if (up) parts.push(up);
        return `{${parts.join(",")}}`;
    };

    // The runtime. Its source is run once for each script engine, and every unit reads it as $s.
    const runtimeFunction = function () {
        "use strict";

        const ENT = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" };
        const e = (value) => {
            if (value === null || value === undefined) return "";
            const text = String(value);
            return /[&<>"]/.test(text) ? text.replace(/[&<>"]/g, (ch) => ENT[ch]) : text;
        };
        // Attributes: a key starting with ":" holds an expression, any other key holds text. null, undefined and false leave it out.
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
        const sq = (text) => "'" + String(text).replace(/[\\'\n\r\u2028\u2029]/g, (ch) => ({ "\\": "\\\\", "'": "\\'", "\n": "\\n", "\r": "\\r", "\u2028": "\\u2028", "\u2029": "\\u2029" })[ch]) + "'";
        const js_ = (value) => {
            if (value === null || value === undefined) return "null";
            if (typeof value === "string") return sq(value);
            if (typeof value === "number" || typeof value === "boolean") return String(value);
            if (Array.isArray(value)) return "[" + value.map(js_).join(",") + "]";
            return "{" + Object.keys(value).map((key) => sq(key) + ":" + js_(value[key])).join(",") + "}";
        };
        const quasarColor = (color) => (color === "success" ? "positive" : color === "danger" ? "negative" : color === "default" ? "" : color || "");
        const blank = (text) => text === null || text === undefined || text === "";
        const IDENT = /^[A-Za-z_$][A-Za-z0-9_$]*$/;
        const scopeRef = (name) => "$scope[" + sq(name) + "]";

        // ---- expressions ----

        // An expression the template will evaluate. The host parses it and reports what a browser cannot do. The text is
        // written in parentheses so it is one operand wherever it lands.
        const X = (expr, n, attrName) => {
            const text = String(expr).trim();
            sri.staticCheck(JSON.stringify([{ expr: text, element: n ? n.name : "", attr: attrName || "" }]));
            return "(" + text + ")";
        };

        // The ${...} parts of a text: [{text}, {expr}].
        const split = (text) => {
            const out = [];
            let literal = "";
            let i = 0;
            while (i < text.length) {
                if (text.charAt(i) === "$" && text.charAt(i + 1) === "{") {
                    let depth = 1;
                    let j = i + 2;
                    let quote = "";
                    while (j < text.length && depth > 0) {
                        const ch = text.charAt(j);
                        if (quote) {
                            if (ch === "\\") j++;
                            else if (ch === quote) quote = "";
                        } else if (ch === "'" || ch === '"' || ch === "`") quote = ch;
                        else if (ch === "{") depth++;
                        else if (ch === "}") depth--;
                        j++;
                    }
                    if (depth === 0) {
                        if (literal) out.push({ text: literal });
                        literal = "";
                        out.push({ expr: text.slice(i + 2, j - 1) });
                        i = j;
                        continue;
                    }
                }
                literal += text.charAt(i);
                i++;
            }
            if (literal) out.push({ text: literal });
            return out;
        };
        const isDynamic = (text) => text.indexOf("${") >= 0;

        // An expression of a text whose element has text-map: Java expands the text with the map's entries over the context, so a name or a path
        // that starts at a key of the map reads the map. Any other expression reads the scope. The map expression is evaluated on each read.
        const PATH = /^([A-Za-z_$][A-Za-z0-9_$]*)((?:\.[A-Za-z_$][A-Za-z0-9_$]*)*)$/;
        const textMapped = (key, n, attrName) => {
            const map = attrName === "text" && n && n.attrs ? n.attrs["text-map"] : "";
            const found = map ? PATH.exec(key) : null;
            if (!found) return X(key, n, attrName);
            const mapExpr = X(map, n, "text-map");
            const rest = found[2] ? found[2].split(".").slice(1).map((part) => "?." + part).join("") : "";
            return "($mq.inMap(" + mapExpr + "," + sq(found[1]) + ")?$mq.inMap(" + mapExpr + "," + sq(found[1]) + ",true)" + rest + ":" + X(key, n, attrName) + ")";
        };
        // A string expression for a text with ${} in it. localize says whether the pattern is localized first, as
        // ec.resource.expand does. The table maps each expression text to a function over the scope, so the browser never
        // compiles code, and a localized pattern can use the expressions the original has.
        const exp = (text, n, attrName, localize) => {
            if (!isDynamic(text)) return localize ? "ec.l10n.localize(" + sq(text) + ")" : sq(text);
            const seen = {};
            const entries = [];
            split(text).forEach((part) => {
                if (part.expr === undefined) return;
                const key = part.expr.trim();
                if (seen[key]) return;
                seen[key] = true;
                entries.push(sq(key) + ":()=>" + textMapped(key, n, attrName));
            });
            return "ec.resource." + (localize ? "expand" : "expandNoL10n") + "(" + sq(text) + ",{" + entries.join(",") + "})";
        };
        // Set an attribute from a text: plain when it holds no ${}, else bound.
        const textAttr = (map, key, text, n, localize) => {
            if (blank(text)) return;
            if (isDynamic(text)) map[":" + key] = exp(text, n, key, localize);
            else if (localize) map[":" + key] = exp(text, n, key, true);
            else map[key] = text;
        };
        // A text with no ${} stays as it is (a class, an id), a dynamic one is bound without localization.
        const plainAttr = (map, key, text, n) => {
            if (blank(text)) return;
            if (isDynamic(text)) map[":" + key] = exp(text, n, key, false);
            else map[key] = text;
        };
        // The value of a Moqui expression attribute, or null when it is empty.
        const expAttr = (n, key) => (at(n, key) ? X(at(n, key), n, key) : null);

        // The text of an element as a v-text bound to a localized expansion.
        const textOf = (n, key) => exp(at(n, key || "text"), n, key || "text", true);

        const note = (kind, n, detail) => sri.staticNote(kind, n ? n.name : "", detail || "");

        // ---- scope helpers in the template ----

        // The parameters of a link or lookup as an object expression: its parameter elements and the extra entries (expressions by name).
        const paramsOf = (n, extra) => {
            const params = [];
            kids(n, "parameter").forEach((p) => {
                const name = at(p, "name");
                let value;
                if (at(p, "value")) value = exp(at(p, "value"), p, "value", true);
                else value = at(p, "from") ? X(at(p, "from"), p, "from") : scopeRef(name);
                params.push(sq(name) + ":" + value);
            });
            Object.keys(extra || {}).forEach((key) => params.push(sq(key) + ":" + extra[key]));
            return "{" + params.join(",") + "}";
        };
        const mapOf = (n) => (at(n, "parameter-map") ? X(at(n, "parameter-map"), n, "parameter-map") : "null");
        // The parameters with the map of parameter-map, as one object.
        const allParams = (n, extra) => "$mq.merge(" + mapOf(n) + "," + paramsOf(n, extra) + ")";
        // A path with the query string the parameters make.
        const urlOf = (n, path, extra) => "$mq.url(" + sq(path) + "," + paramsOf(n, extra) + "," + mapOf(n) + ")";

        // ---- state of the form being written ----

        const st = {};
        const saved = [];
        const reset = () => {
            Object.assign(st, { formName: "", fieldForm: "", owner: "", inForm: false, inRow: false, listName: "", multi: false, accordion: "", accIndex: 0, accActive: "", selectedForm: false });
            saved.length = 0;
        };
        reset();
        const push = () => { saved.push(Object.assign({}, st)); };
        const pop = () => { const old = saved.pop(); if (old) Object.assign(st, old); };

        const fieldOf = (n) => n.parent.parent;
        const fname = (n) => at(fieldOf(n), "name");
        const formOf = (n) => fieldOf(n).parent;
        const rowSuffix = () => (st.inRow ? "+'_'+rowIndex" : "");
        // The id of a widget: static in a form-single, a bound id with the row number in a list.
        const idAttrs = (n, suffix) => {
            const base = (st.fieldForm || at(formOf(n), "name")) + "_" + fname(n) + (suffix || "");
            return st.inRow ? { ":id": sq(base) + "+'_'+rowIndex" } : { id: base };
        };
        const nameOf = (n) => fname(n);
        const owner = () => st.owner || null;
        const tipOf = (n, out) => { if (at(n.parent, "tooltip")) textAttr(out, "tooltip", at(n.parent, "tooltip"), n.parent, true); };
        const titleExpr = (sub) => {
            let holder = sub;
            if (sub.name === "header-field" && sub.parent) {
                const header = first(sub.parent, "header-field");
                const dflt = first(sub.parent, "default-field");
                if (header && at(header, "title")) holder = header;
                else if (dflt && at(dflt, "title")) holder = dflt;
            }
            if (at(holder, "title")) return exp(at(holder, "title"), holder, "title", true);
            const name = holder.parent ? at(holder.parent, "name") : "";
            return "ec.l10n.localize(" + sq(name.split(/(?=[A-Z])/).map((word) => (word.charAt(0).toUpperCase() + word.slice(1)).replace("Id", "ID")).join(" ")) + ")";
        };
        const labelAttr = (n, out) => { out[":label"] = titleExpr(n.parent); };

        // How a field binds. In a form the model is formProps.fields.name; outside one the value is given as model-value.
        const fieldRef = (name) => (IDENT.test(name) ? "formProps.fields." + name : "formProps.fields[" + sq(name) + "]");

        // The value of a field: its from expression, else the name in the scope chain (the map of the form, then the screen), else the default.
        const baseValue = (field, defaultValue) => {
            const from = at(field, "from") || at(field, "entry-name");
            const base = from ? X(from, field, "from") : scopeRef(at(field, "name"));
            if (defaultValue) return "$mq.dflt(" + base + "," + exp(defaultValue, field, "default-value", false) + ")";
            return base;
        };
        const bindAttrs = (n, field, value) => {
            const name = at(field, "name");
            return st.inForm ? { "v-model": fieldRef(name), ":fields": "formProps.fields" } : { ":model-value": value };
        };
        const bindOnly = (n, field, value) => {
            const name = at(field, "name");
            return st.inForm ? { "v-model": fieldRef(name) } : { ":model-value": value };
        };

        // Rules the field's validation hints give.
        // The element and its parents for a host call. Parents are linked back from their children here, so the host is given a copy without the cycle.
        const hostLit = (node) => {
            const out = { name: node.name, attrs: node.attrs || {} };
            if (node.parent) out.parent = hostLit(node.parent);
            return out;
        };
        const validation = (sub) => (at(sub, "validate-service") || at(sub, "validate-entity") ? sri.getFieldValidation(hostLit(sub)) : { classes: "" });
        const validate = (sub) => {
            const v = validation(sub);
            const has = (name) => v.classes.indexOf(name) >= 0;
            return { required: has("required"), email: has("email"), url: has("url"), number: has("number"), digits: has("digits"), v };
        };
        const ruleList = (n, check) => {
            const rules = [];
            if (at(n, "rules")) rules.push(at(n, "rules"));
            if (check.number) rules.push("value => !value || moqui.isStringNumber(value) || ec.l10n.localize('Please enter a valid number')");
            if (check.digits) rules.push("value => !value || /^\\d*$/.test(value) || ec.l10n.localize('Please enter only digits')");
            return rules.length ? "[" + rules.join(",") + "]" : null;
        };
        const quasarMask = (mask) => mask.replace(/9/g, "#").replace(/a/g, "S").replace(/\*/g, "N");

        // The options a widget takes from the package, by the key the host reserves for it, else the option elements it holds.
        const optionKey = (n) => {
            const sub = n.parent;
            const field = sub.parent;
            const subIndex = field.children.indexOf(sub);
            return "mqOpt_" + at(field.parent, "name") + "_" + at(field, "name") + "_" + subIndex + "_" + sub.children.indexOf(n);
        };
        const hasLookup = (n) => kids(n, "entity-options").length > 0 || kids(n, "list-options").length > 0 || (first(n, "dynamic-options") && at(first(n, "dynamic-options"), "server-search") !== "true");
        const optionsExpr = (n) => {
            const fixed = kids(n, "option").map((o) => {
                const key = at(o, "key");
                const label = o.attrs && o.attrs.text !== undefined ? exp(at(o, "text"), o, "text", true) : "ec.l10n.localize(" + (isDynamic(key) ? exp(key, o, "key", false) : sq(key)) + ")";
                return "{value:" + (isDynamic(key) ? exp(key, o, "key", false) : sq(key)) + ",label:" + label + "}";
            });
            if (hasLookup(n)) {
                const found = "$scope[" + sq(optionKey(n)) + "]||[]";
                return fixed.length ? "[" + fixed.join(",") + "].concat(" + found + ")" : "(" + found + ")";
            }
            return "[" + fixed.join(",") + "]";
        };

        // A remote lookup of a widget: the url of its transition and the parameters the page can give, with the fields it depends on.
        const remote = (holder, transition) => {
            const found = sri.staticUrl(transition, "transition");
            const deps = {};
            kids(holder, "depends-on").forEach((dep) => { deps[at(dep, "parameter") || at(dep, "field")] = at(dep, "field"); });
            return { url: found.path, params: allParams(holder), dependsOn: js_(deps) };
        };

        // ---- the values a field starts with ----

        // The entries of fields-initial that one widget gives: [name, expression]. They follow ScreenRenderImpl.addFormFieldValue.
        const entriesOf = (widget, field) => {
            const name = at(field, "name");
            const base = baseValue(field, at(widget, "default-value"));
            const kind = widget.name;
            const out = [];
            const plain = "$mq.str(" + baseValue(field, "") + ")";
            if (kind === "display") {
                if (!at(widget, "also-hidden") || at(widget, "also-hidden") === "true") out.push([name, plain]);
                out.push([name + "_display", displayText(widget, field)]);
            } else if (kind === "display-entity") {
                if (!at(widget, "also-hidden") || at(widget, "also-hidden") === "true") out.push([name, plain]);
                out.push([name + "_display", "$mq.str(" + scopeRef(name + "_display") + ")"]);
            } else if (kind === "drop-down") {
                const multiple = at(widget, "allow-multiple") === "true";
                const none = at(widget, "no-current-selected-key") ? exp(at(widget, "no-current-selected-key"), widget, "no-current-selected-key", false) : "''";
                out.push([name, "$mq.cur(" + baseValue(field, "") + "," + multiple + "," + none + ")"]);
                if (multiple) out.push([name + "_op", "'in'"]);
                if (at(widget, "show-not") === "true") out.push([name + "_not", "(" + scopeRef(name + "_not") + "||'N')"]);
            } else if (kind === "text-line" || kind === "text-area" || kind === "lookup" || kind === "hidden") {
                out.push([name, "$mq.str(" + base + ")"]);
            } else if (kind === "check") {
                out.push([name, "$mq.cur(" + baseValue(field, "") + ",true," + (at(widget, "no-current-selected-key") ? exp(at(widget, "no-current-selected-key"), widget, "no-current-selected-key", false) : "''") + ")"]);
            } else if (kind === "radio") {
                out.push([name, "$mq.cur(" + baseValue(field, "") + ",false," + (at(widget, "no-current-selected-key") ? exp(at(widget, "no-current-selected-key"), widget, "no-current-selected-key", false) : "''") + ")"]);
            } else if (kind === "date-time") {
                const format = at(widget, "format") || dateDefault(widget);
                out.push([name, "$mq.fmt(" + base + "," + sq(format) + "," + sq(at(widget, "type") || "date-time") + ")"]);
            } else if (kind === "date-find") {
                const format = dateDefault(widget);
                const type = sq(at(widget, "type") || "date-time");
                out.push([name + "_from", "$mq.fmt(" + scopeRef(name + "_from") + "||" + (at(widget, "default-value-from") ? exp(at(widget, "default-value-from"), widget, "default-value-from", false) : "''") + "," + sq(format) + "," + type + ")"]);
                out.push([name + "_thru", "$mq.fmt(" + scopeRef(name + "_thru") + "||" + (at(widget, "default-value-thru") ? exp(at(widget, "default-value-thru"), widget, "default-value-thru", false) : "''") + "," + sq(format) + "," + type + ")"]);
            } else if (kind === "date-period") {
                ["_poffset", "_period", "_pdate", "_from", "_thru"].forEach((suffix) => out.push([name + suffix, "$mq.nul(" + scopeRef(name + suffix) + ")"]));
            } else if (kind === "range-find") {
                out.push([name + "_from", "$mq.nul(" + scopeRef(name + "_from") + ")"]);
                out.push([name + "_thru", "$mq.nul(" + scopeRef(name + "_thru") + ")"]);
            } else if (kind === "text-find") {
                out.push([name, "$mq.str(" + base + ")"]);
                out.push([name + "_op", "(" + scopeRef(name + "_op") + "||" + sq(at(widget, "default-operator") || "contains") + ")"]);
                out.push([name + "_not", "(" + scopeRef(name + "_not") + "||'N')"]);
                const ignore = !at(widget, "ignore-case") || at(widget, "ignore-case") === "true";
                out.push([name + "_ic", "(" + scopeRef(name + "_ic") + "||" + sq(ignore ? "Y" : "N") + ")"]);
            } else if (kind === "link") {
                if (at(widget, "entity-name")) out.push([name + "_display", "$mq.str(" + scopeRef(name + "_display") + ")"]);
            } else if (kind === "file" || kind === "ignored" || kind === "password" || kind === "submit" || kind === "reset" || kind === "label" || kind === "image") {
                // nothing to start with
            } else out.push([name, plain]);
            return out;
        };

        // The fields-initial of the fields of a form, as an object expression. A field with conditional-field gives each key
        // the value of the branch that holds.
        const initialOf = (fields, branchKind, extraEntries) => {
            const keys = [];
            const byKey = {};
            const add = (key, condition, expression) => {
                if (!byKey[key]) { byKey[key] = []; keys.push(key); }
                byKey[key].push({ condition, expression });
            };
            fields.forEach((field) => {
                const subs = [];
                if (branchKind === "r") {
                    kids(field, "conditional-field").forEach((sub) => { if (at(sub, "condition")) subs.push({ sub, condition: at(sub, "condition") }); });
                    const dflt = first(field, "default-field");
                    if (dflt) subs.push({ sub: dflt, condition: null });
                } else {
                    const sub = first(field, branchKind);
                    if (sub) subs.push({ sub, condition: null });
                }
                subs.forEach(({ sub, condition }) => {
                    sub.children.filter((widget) => widget.name !== "set").forEach((widget) => {
                        entriesOf(widget, field).forEach(([key, expression]) => add(key, condition, expression));
                    });
                });
            });
            const parts = keys.map((key) => {
                let chain = "undefined";
                const branches = byKey[key];
                for (let i = branches.length - 1; i >= 0; i--) {
                    chain = branches[i].condition ? "(" + X(branches[i].condition, null, "condition") + "?" + branches[i].expression + ":" + chain + ")" : branches[i].expression;
                }
                return sq(key) + ":" + chain;
            });
            (extraEntries || []).forEach(([key, expression]) => parts.push(sq(key) + ":" + expression));
            return "{" + parts.join(",") + "}";
        };

        // ---- inputs ----

        const textLine = (n) => {
            const field = fieldOf(n);
            const name = nameOf(n);
            const out = {};
            Object.assign(out, idAttrs(n));
            if (at(n, "ac-transition")) {
                const info = remote(n, at(n, "ac-transition"));
                Object.assign(out, {
                    name, form: owner(), "allow-empty": true, "server-search": true, "server-min-length": at(n, "ac-min-length") || null,
                    "server-delay": at(n, "ac-delay") || null, combo: at(n, "ac-use-actual") === "true" ? true : null, "options-url": info.url,
                    ":options-parameters": info.params, ":depends-on": info.dependsOn, "depends-optional": at(n, "depends-optional") === "true" ? true : null
                });
                labelAttr(n, out);
                tipOf(n, out);
                const value = "$mq.str(" + baseValue(field, at(n, "default-value")) + ")";
                out[":options"] = value + "?[{value:" + value + ",label:" + (at(n, "ac-initial-text") ? exp(at(n, "ac-initial-text"), n, "ac-initial-text", true) : value) + "}]:[]";
                Object.assign(out, bindAttrs(n, field, value));
                w("<m-drop-down" + A(out) + "></m-drop-down>");
                return;
            }
            const check = validate(n.parent);
            const align = at(field, "align");
            Object.assign(out, {
                type: at(n, "input-type") || (check.email ? "email" : check.url ? "url" : "text"), name, size: at(n, "size") || null,
                style: at(n, "size") ? null : "width:100%;", maxlength: at(n, "maxlength") || null,
                class: cls(check.v.classes || null, align === "center" ? "text-center" : align === "right" ? "text-right" : null),
                required: check.required || null, ":rules": ruleList(n, check), form: owner()
            });
            labelAttr(n, out);
            if (at(n, "prefix")) textAttr(out, "prefix", at(n, "prefix"), n, true);
            if (at(n, "mask")) {
                out.mask = quasarMask(at(n, "mask"));
                out["fill-mask"] = "_";
                if (check.number) out["reverse-fill-mask"] = true;
            }
            if (at(n, "disabled")) out[":disable"] = X(at(n, "disabled"), n, "disabled");
            tipOf(n, out);
            if (at(n, "default-transition")) {
                const info = remote(n, at(n, "default-transition"));
                Object.assign(out, {
                    "default-url": info.url, "default-load-init": true, "depends-optional": at(n, "depends-optional") === "true" ? true : null,
                    ":depends-on": info.dependsOn, ":default-parameters": info.params
                });
            }
            Object.assign(out, bindAttrs(n, field, "$mq.str(" + baseValue(field, at(n, "default-value")) + ")"));
            w("<m-text-line" + A(out) + "></m-text-line>");
        };

        const textArea = (n) => {
            const field = fieldOf(n);
            const out = Object.assign({ name: nameOf(n) }, idAttrs(n), {
                rows: at(n, "rows") || "3", cols: at(n, "cols") || null, style: at(n, "cols") ? null : "width:100%;", readonly: at(n, "read-only") === "true",
                autogrow: at(n, "autogrow") === "true", maxlength: at(n, "maxlength") || null, form: owner()
            });
            labelAttr(n, out);
            if (at(n, "editor-type")) plainAttr(out, "editor-type", at(n, "editor-type"), n);
            tipOf(n, out);
            Object.assign(out, bindOnly(n, field, "$mq.str(" + baseValue(field, at(n, "default-value")) + ")"));
            w("<m-text-area" + A(out) + "></m-text-area>");
        };

        const password = (n) => {
            const check = validate(n.parent);
            const field = fieldOf(n);
            const out = Object.assign({ type: "password", name: nameOf(n), size: at(n, "size") || "25", maxlength: at(n, "maxlength") || null, required: check.required || null, form: owner() }, idAttrs(n));
            labelAttr(n, out);
            tipOf(n, out);
            Object.assign(out, bindOnly(n, field, "''"));
            w("<m-text-line" + A(out) + "></m-text-line>");
        };

        const file = (n) => {
            const field = fieldOf(n);
            const out = Object.assign({ name: nameOf(n), size: at(n, "size") || "30", multiple: at(n, "multiple") === "true", maxlength: at(n, "maxlength") || null, form: owner() }, idAttrs(n));
            labelAttr(n, out);
            if (at(n, "accept")) textAttr(out, "accept", at(n, "accept"), n, false);
            tipOf(n, out);
            Object.assign(out, bindOnly(n, field, "null"));
            w("<m-file" + A(out) + "></m-file>");
        };

        const hidden = (n) => {
            if (st.inForm) return;
            const field = fieldOf(n);
            w("<input" + A(Object.assign({ type: "hidden", name: nameOf(n), ":value": "$mq.str(" + baseValue(field, at(n, "default-value")) + ")", form: owner() }, idAttrs(n))) + ">\n");
        };

        const reset_ = (n) => {
            const out = Object.assign({ dense: true, outline: true, "no-caps": true, type: "reset", name: nameOf(n), form: owner() }, idAttrs(n));
            w("<q-btn" + A(out) + ">" + "<span v-text=\"" + e(titleExpr(n.parent)) + "\"></span></q-btn>");
        };

        const submit = (n) => {
            const out = Object.assign({
                dense: true, ":outline": st.inForm ? "!formProps.hasFieldsChanged" : null, outline: st.inForm ? null : true, "no-caps": true, type: "submit",
                name: nameOf(n), value: nameOf(n), color: quasarColor(at(n, "type") || "primary") || null, form: owner()
            }, idAttrs(n));
            if (at(n, "confirmation")) textAttr(out, "data-confirm", at(n, "confirmation"), n, true);
            const image = first(n, "image");
            const iconClass = at(n, "icon");
            let body = iconClass ? '<i class="' + e(iconClass) + (image ? "" : " q-icon on-left") + '"></i>' : "";
            if (image) {
                body += "<img" + A({ ":src": exp(at(image, "url"), image, "url", false), ":alt": exp(at(image, "alt") || "image", image, "alt", true), width: at(image, "width") || null, height: at(image, "height") || null }) + ">";
            } else body += "<span v-text=\"" + e(at(n, "text") ? exp(at(n, "text"), n, "text", true) : titleExpr(n.parent)) + "\"></span>";
            w("<q-btn" + A(out) + ">" + body + "</q-btn>");
        };

        // ---- dates ----

        const dateDefault = (n) => (at(n, "type") === "time" ? "HH:mm" : at(n, "type") === "date" ? "yyyy-MM-dd" : "yyyy-MM-dd HH:mm");
        const maskFormat = (format) => format.replace(/a/g, "A").replace(/d/g, "D").replace(/y/g, "Y");

        const dateTime = (n) => {
            const field = fieldOf(n);
            const format = at(n, "format") || dateDefault(n);
            const check = validate(n.parent);
            const out = Object.assign({
                name: nameOf(n), type: at(n, "type") || null, size: at(n, "size") || null, format: maskFormat(format), required: check.required || null,
                ":rules": at(n, "rules") ? "[" + at(n, "rules") + "]" : null, "auto-year": at(n, "auto-year") || "true", ":minute-step": at(n, "minute-stepping") || "5", form: owner()
            }, idAttrs(n));
            labelAttr(n, out);
            if (at(n, "disabled")) out[":disable"] = X(at(n, "disabled"), n, "disabled");
            tipOf(n, out);
            Object.assign(out, bindOnly(n, field, "$mq.fmt(" + baseValue(field, at(n, "default-value")) + "," + sq(format) + "," + sq(at(n, "type") || "date-time") + ")"));
            w("<m-date-time" + A(out) + "></m-date-time>");
        };

        const dateFind = (n) => {
            const field = fieldOf(n);
            const name = nameOf(n);
            const format = dateDefault(n);
            const one = (suffix, text, def) => {
                const out = Object.assign({ name: name + suffix, type: at(n, "type") || null, size: at(n, "size") || null, format: maskFormat(format), form: owner() }, idAttrs(n, suffix));
                out[":label"] = titleExpr(n.parent) + "+' '+ec.l10n.localize(" + sq(text) + ")";
                tipOf(n, out);
                const fallback = "$mq.fmt(" + scopeRef(name + suffix) + "||" + (def ? exp(def, n, "default-value" + suffix, false) : "''") + "," + sq(format) + "," + sq(at(n, "type") || "date-time") + ")";
                Object.assign(out, st.inForm ? { "v-model": fieldRef(name + suffix) } : { ":model-value": fallback });
                return '<span class="form-date-find"><m-date-time' + A(out) + "></m-date-time></span>";
            };
            w(one("_from", "From", at(n, "default-value-from")) + one("_thru", "Thru", at(n, "default-value-thru")));
        };

        const datePeriod = (n) => {
            const name = nameOf(n);
            const values = {};
            ["_poffset", "_period", "_pdate", "_from", "_thru"].forEach((suffix) => { values[name + suffix] = "$mq.nul(" + scopeRef(name + suffix) + ")"; });
            const out = Object.assign({ name, "allow-empty": at(n, "allow-empty") !== "false", "from-thru-type": at(n, "time") === "true" ? "date-time" : "date", form: owner() }, idAttrs(n));
            labelAttr(n, out);
            tipOf(n, out);
            out[":fields"] = st.inForm ? "formProps.fields" : "{" + Object.keys(values).map((key) => sq(key) + ":" + values[key]).join(",") + "}";
            w("<m-date-period" + A(out) + "></m-date-period>");
        };

        const rangeFind = (n) => {
            const name = nameOf(n);
            const out = Object.assign({ name, size: at(n, "size") || "10", maxlength: at(n, "maxlength") || null, form: owner() }, idAttrs(n));
            labelAttr(n, out);
            tipOf(n, out);
            out[":fields"] = st.inForm ? "formProps.fields" : "{" + sq(name + "_from") + ":" + scopeRef(name + "_from") + "," + sq(name + "_thru") + ":" + scopeRef(name + "_thru") + "}";
            w("<m-range-find" + A(out) + "></m-range-find>");
        };

        const textFind = (n) => {
            const name = nameOf(n);
            const out = Object.assign({ name, size: at(n, "size") || "30", maxlength: at(n, "maxlength") || null, "hide-options": at(n, "hide-options") || "false", form: owner() }, idAttrs(n));
            labelAttr(n, out);
            tipOf(n, out);
            const ignore = !at(n, "ignore-case") || at(n, "ignore-case") === "true";
            out[":fields"] = st.inForm ? "formProps.fields" : "{" + sq(name) + ":$mq.str(" + baseValue(fieldOf(n), "") + ")," + sq(name + "_op") + ":(" + scopeRef(name + "_op") + "||" + sq(at(n, "default-operator") || "contains") + ")," +
                sq(name + "_not") + ":(" + scopeRef(name + "_not") + "||'N')," + sq(name + "_ic") + ":(" + scopeRef(name + "_ic") + "||" + sq(ignore ? "Y" : "N") + ")}";
            w("<m-text-find" + A(out) + "></m-text-find>");
        };

        // ---- options widgets ----

        const dropDown = (n) => {
            const field = fieldOf(n);
            const name = nameOf(n);
            const multiple = at(n, "allow-multiple") === "true";
            const allowEmpty = at(n, "allow-empty") === "true";
            const dyn = first(n, "dynamic-options");
            const check = validate(n.parent);
            const out = Object.assign({
                name, form: owner(), size: at(n, "size") || null, class: cls(dyn ? "dynamic-options" : null, at(n, "style") || null, check.v.classes || null),
                multiple: multiple || null, "allow-empty": allowEmpty || null, combo: at(n, "combo-box") === "true" ? true : null,
                "required-manual-select": at(n, "required-manual-select") === "true" ? true : null, "submit-on-select": at(n, "submit-on-select") === "true" ? true : null
            }, idAttrs(n));
            labelAttr(n, out);
            tipOf(n, out);
            out[":options"] = optionsExpr(n);
            if (dyn) {
                const info = remote(dyn, at(dyn, "transition"));
                Object.assign(out, {
                    "options-url": info.url, "value-field": at(dyn, "value-field") || "value", "label-field": at(dyn, "label-field") || "label",
                    "depends-optional": at(dyn, "depends-optional") === "true" ? true : null, ":depends-on": info.dependsOn, ":options-parameters": info.params,
                    "server-search": at(dyn, "server-search") === "true" ? true : null, ":server-delay": at(dyn, "delay") || null, ":server-min-length": at(dyn, "min-length") || null,
                    "options-load-init": dyn.children.length > 1 || undefined
                });
                if (!out["options-load-init"]) delete out["options-load-init"];
            }
            const none = at(n, "no-current-selected-key") ? exp(at(n, "no-current-selected-key"), n, "no-current-selected-key", false) : "''";
            const current = "$mq.cur(" + baseValue(field, "") + "," + multiple + "," + none + ")";
            Object.assign(out, st.inForm ? { "v-model": fieldRef(name) } : { ":model-value": current });
            let body = "<m-drop-down" + A(out) + ">";
            if (at(n, "show-not") === "true") {
                body += "<template v-slot:after><m-check" + A(Object.assign({
                    name: name + "_not", "true-value": "Y", "false-value": "N", ":single-label": "ec.l10n.localize('Not')", form: owner()
                }, idAttrs(n, "_not"), st.inForm ? { "v-model": fieldRef(name + "_not") } : { ":model-value": "(" + scopeRef(name + "_not") + "||'N')" })) + "></m-check></template>";
            }
            w(body + "</m-drop-down>");
        };

        const checkOrRadio = (n, kind) => {
            const field = fieldOf(n);
            const name = nameOf(n);
            const tag = kind === "checkbox" ? "m-check" : "m-radio";
            const out = Object.assign({ name, form: owner(), class: kind === "checkbox" ? at(n, "container-style") || null : null, "no-wrapper": kind === "checkbox" && at(n, "no-wrapper") === "true" ? true : null }, idAttrs(n));
            labelAttr(n, out);
            tipOf(n, out);
            out[":options"] = optionsExpr(n);
            const none = at(n, "no-current-selected-key") ? exp(at(n, "no-current-selected-key"), n, "no-current-selected-key", false) : "''";
            const current = "$mq.cur(" + baseValue(field, "") + "," + (kind === "checkbox") + "," + none + ")";
            Object.assign(out, st.inForm ? { "v-model": fieldRef(name) } : { ":model-value": current });
            w("<" + tag + A(out) + "></" + tag + ">");
        };

        // ---- display ----

        const displayText = (n, field) => {
            const unit = at(n, "currency-unit-field");
            const hideSymbol = at(n, "currency-hide-symbol") === "true";
            const currency = (value) => "ec.l10n." + (hideSymbol ? "formatCurrencyNoSymbol" : "formatCurrency") + "(" + value + "," + X(unit, n, "currency-unit-field") + ")";
            if (at(n, "text")) {
                const text = exp(at(n, "text"), n, "text", true);
                return unit ? currency(text) : text;
            }
            const base = baseValue(field, "");
            if (unit) return currency(base);
            if (at(n, "format")) return "$mq.fmt(" + base + "," + sq(at(n, "format")) + ")";
            return "$mq.str(" + base + ")";
        };

        const displayWidget = (n) => {
            const field = fieldOf(n);
            const name = nameOf(n);
            const align = at(field, "align");
            const labelWrapper = field.parent && field.parent.name === "form-single";
            const dyn = !!at(n, "dynamic-transition");
            const out = Object.assign({
                name, "label-wrapper": labelWrapper || null,
                class: cls(at(n, "currency-unit-field") ? "currency" : null, align === "center" ? "text-center" : align === "right" ? "text-right" : null, at(n, "style") || null)
            }, st.inRow ? { ":id": sq((st.fieldForm || at(formOf(n), "name")) + "_" + name) + "+'_'+rowIndex+'_display'" } : { id: (st.fieldForm || at(formOf(n), "name")) + "_" + name + "_display" });
            if (labelWrapper) labelAttr(n, out);
            tipOf(n, out);
            if (dyn) {
                const info = remote(n, at(n, "dynamic-transition"));
                Object.assign(out, { "value-url": info.url, "depends-optional": at(n, "depends-optional") === "true" ? true : null, ":depends-on": info.dependsOn, ":value-parameters": info.params });
            }
            if (st.inForm) {
                Object.assign(out, { "v-model": fieldRef(name), ":display": fieldRef(name + "_display"), ":fields": "formProps.fields" });
                w("<m-display" + A(out) + "></m-display>");
                return;
            }
            const text = displayText(n, field);
            if (labelWrapper) {
                out[":display"] = text;
                w("<m-display" + A(out) + "></m-display>");
            } else {
                w("<m-display" + A(out) + '><span v-text="' + e(text) + '"></span></m-display>');
            }
            if ((!at(n, "also-hidden") || at(n, "also-hidden") === "true") && !dyn) {
                w("<input" + A({ type: "hidden", name, ":value": "$mq.str(" + baseValue(field, "") + ")", form: owner() }) + ">");
            }
        };

        const displayEntity = (n) => {
            const field = fieldOf(n);
            const name = nameOf(n);
            const align = at(field, "align");
            const formKind = field.parent ? field.parent.name : "";
            const out = Object.assign({
                name, "label-wrapper": formKind === "form-single" || null,
                class: cls(align === "center" ? "text-center" : align === "right" ? "text-right" : null, at(n, "style") || null)
            }, st.inRow ? { ":id": sq((st.fieldForm || at(formOf(n), "name")) + "_" + name) + "+'_'+rowIndex+'_display'" } : { id: (st.fieldForm || at(formOf(n), "name")) + "_" + name + "_display" });
            if (formKind === "form-single") labelAttr(n, out);
            tipOf(n, out);
            const text = "$mq.str(" + scopeRef(name + "_display") + ")";
            if (st.inForm) {
                Object.assign(out, { "v-model": fieldRef(name), ":display": fieldRef(name + "_display") });
                w("<m-display" + A(out) + "></m-display>");
                return;
            }
            if (formKind === "form-single") {
                out[":display"] = text;
                w("<m-display" + A(out) + "></m-display>");
            } else w("<m-display" + A(out) + '><span v-text="' + e(text) + '"></span></m-display>');
            if (!at(n, "also-hidden") || at(n, "also-hidden") === "true") {
                w("<input" + A({ type: "hidden", name, ":value": "$mq.str(" + baseValue(field, "") + ")", form: owner() }) + ">");
            }
        };

        // ---- links, images, labels ----

        const imageTag = (img) => {
            const type = at(img, "url-type") || "content";
            const src = type === "content" || type === "plain" ? exp(at(img, "url"), img, "url", false) : urlOf(img, sri.staticUrl(at(img, "url"), type).path);
            const out = { ":src": src, ":alt": exp(at(img, "alt") || "image", img, "alt", true), width: at(img, "width") || null, height: at(img, "height") || null };
            plainAttr(out, "id", at(img, "id"), img);
            plainAttr(out, "class", at(img, "style"), img);
            return "<img" + A(out) + ">";
        };
        const withCondition = (n, body) => {
            if (!at(n, "condition")) return body;
            return '<template v-if="' + e(X(at(n, "condition"), n, "condition")) + '">' + body + "</template>";
        };
        const image = (n) => {
            const tag = imageTag(n);
            w(withCondition(n, at(n, "hover") === "true" ? '<span class="hover-image-container">' + tag + tag.replace("<img", '<img class="hover-image"') + "</span>" : tag));
        };

        const labelWidget = (n) => {
            const type = at(n, "type") || "span";
            const out = { class: cls("text-inline", at(n, "style") || null), ":v-text": null };
            plainAttr(out, "id", at(n, "id"), n);
            if (at(n, "tooltip")) textAttr(out, "title", at(n, "tooltip"), n, true);
            delete out[":v-text"];
            out["v-text"] = textOf(n);
            if (at(n, "encode") === "false") note("encode-false", n, "label with encode=false writes text as html");
            w(withCondition(n, "<" + type + A(out) + "></" + type + ">\n"));
        };

        // The way a link is written: a screen link, a plain link, or a form that posts a transition.
        const link = (n, inField) => {
            const urlType = at(n, "url-type") || "transition";
            const found = urlType === "transition" || urlType === "screen" ? sri.staticUrl(at(n, "url"), urlType) : null;
            const img = first(n, "image");
            const linkType = at(n, "link-type");
            const anchor = linkType === "anchor" || linkType === "anchor-button" ||
                ((!linkType || linkType === "auto") && ((urlType && urlType !== "transition") || (found ? found.readOnly : true)));
            const style = at(n, "style") || null;
            const color = quasarColor(at(n, "btn-type") || "primary") || null;
            const iconClass = at(n, "icon");
            let label = "";
            if (at(n, "entity-name")) label = "$mq.str(" + scopeRef(fieldName(n) + "_display") + ")";
            else label = at(n, "text") ? exp(at(n, "text"), n, "text", true) : "''";
            const body = (iconClass ? '<i class="' + e(iconClass) + ' q-icon' + '"></i> ' : "") +
                (img ? imageTag(img) : '<span v-text="' + e(label) + '"></span>') +
                (at(n, "badge") ? ' <q-badge class="on-right" transparent><span v-text="' + e(exp(at(n, "badge"), n, "badge", true)) + '"></span></q-badge>' : "");
            const tooltip = at(n, "tooltip") ? '<q-tooltip><span v-text="' + e(exp(at(n, "tooltip"), n, "tooltip", true)) + '"></span></q-tooltip>' : "";
            const confirmation = at(n, "confirmation");
            const confirmAttr = {};
            if (confirmation) textAttr(confirmAttr, "data-confirm", confirmation, n, true);
            const noParam = at(n, "url-noparam") === "true";
            // An alias transition's screenPath is the screen it names. A form still posts found.path.
            const navPath = found && at(n, "expand-transition-url") !== "false" && found.screenPath ? found.screenPath : (found ? found.path : "");
            let out = "";
            if (anchor) {
                // A type=none or plain-response transition is not a screen URL, so the anchor is <a> and the client
                // does not setUrl it. An unresolved ${} target stays an m-link.
                const toScreen = !!(found && found.screenUrl);
                const href = found ? (noParam ? sq(navPath) : urlOf(n, navPath)) : exp(at(n, "url"), n, "url", false);
                const tag = toScreen ? "m-link" : "a";
                const asButton = linkType !== "anchor";
                const attrs = Object.assign({ ":href": href, target: at(n, "target-window") || null, "load-id": at(n, "dynamic-load-id") || null, class: asButton ? null : style }, confirmAttr);
                plainAttr(attrs, "id", at(n, "id"), n);
                out = "<" + tag + A(attrs) + ">" + (asButton ? "<q-btn" + A({ dense: true, outline: true, "no-caps": true, color, class: cls("m-link", style) }) + ">" + tooltip + body + "</q-btn>" : tooltip + body) + "</" + tag + ">";
            } else {
                const fieldsInitial = allParams(n);
                const isForm = found ? !found.isScreen : true;
                const tag = isForm ? "m-form" : "m-form-link";
                const hiddenLink = linkType === "hidden-form-link";
                const button = img ?
                    "<input" + A({ type: "image", ":src": exp(at(img, "url"), img, "url", false), ":alt": exp(at(img, "alt") || "image", img, "alt", true) }) + ">" :
                    "<q-btn" + A(Object.assign({ dense: true, "no-caps": true, type: "submit", flat: hiddenLink ? true : null, outline: hiddenLink ? null : true, color, class: cls("m-link", style) }, confirmAttr)) + ">" + tooltip + body + "</q-btn>";
                const attrs = { action: found ? found.path : at(n, "url"), ":fields-initial": fieldsInitial, "v-slot:default": "formProps", "read-only": isForm && found && found.readOnly ? true : null, target: at(n, "target-window") || null };
                if (isForm && found && found.method === "get") attrs.method = "GET";
                out = "<" + tag + A(attrs) + ">" + button + "</" + tag + ">";
            }
            const wrapper = inField && linkType === "anchor" && n.parent && n.parent.parent && n.parent.parent.parent && n.parent.parent.parent.name === "form-single";
            if (wrapper) out = "<m-field" + A({ ":label": titleExpr(n.parent) }) + ">" + out + "</m-field>";
            w(withCondition(n, out + (inField ? "" : "\n")));
        };
        const fieldName = (n) => (n.parent && n.parent.parent ? at(n.parent.parent, "name") : "");

        const editable = (n) => {
            const found = sri.staticUrl(at(n, "transition"), "transition");
            const load = first(n, "editable-load");
            const out = Object.assign({
                "label-type": at(n, "type") || "span", ":label-value": exp(at(n, "text"), n, "text", true), url: found.path, ":url-parameters": allParams(n),
                "parameter-name": at(n, "parameter-name") || "value", "widget-type": at(n, "widget-type") || "textarea", ":indicator": "ec.l10n.localize('Saving')",
                ":tooltip": "ec.l10n.localize('Click to edit')", ":cancel": "ec.l10n.localize('Cancel')", ":submit": "ec.l10n.localize('Save')"
            });
            plainAttr(out, "id", at(n, "id"), n);
            if (load) {
                const loadUrl = sri.staticUrl(at(load, "transition"), "transition");
                out["load-url"] = loadUrl.path;
                out[":load-parameters"] = allParams(load);
            }
            w("<m-editable" + A(out) + "></m-editable>");
        };

        // ---- containers ----

        const containerDialog = (n) => {
            const out = { color: quasarColor(at(n, "type") || "primary") || null, width: at(n, "width") || null, "button-class": at(n, "button-style") || null };
            plainAttr(out, "id", at(n, "id"), n);
            textAttr(out, "button-text", at(n, "button-text"), n, true);
            if (at(n, "title")) textAttr(out, "title", at(n, "title"), n, true);
            else if (at(n, "button-text")) textAttr(out, "title", at(n, "button-text"), n, true);
            const open = at(n, "condition") ? '<template v-if="' + e(X(at(n, "condition"), n, "condition")) + '">' : "";
            w(open + "<m-container-dialog" + A(out) + ">");
            return !!open;
        };

        const dynamicDialog = (n) => {
            const found = sri.staticUrl(at(n, "transition"), "transition");
            const out = { ":url": urlOf(n, found.screenPath || found.path), color: quasarColor(at(n, "type") || "primary") || null, width: at(n, "width") || null, "button-class": at(n, "button-style") || null };
            plainAttr(out, "id", at(n, "id"), n);
            textAttr(out, "button-text", at(n, "button-text"), n, true);
            if (at(n, "title")) textAttr(out, "title", at(n, "title"), n, true);
            else if (at(n, "button-text")) textAttr(out, "title", at(n, "button-text"), n, true);
            w(withCondition(n, "<m-dynamic-dialog" + A(out) + "></m-dynamic-dialog>"));
        };

        const dynamicContainer = (n) => {
            const found = sri.staticUrl(at(n, "transition"), "transition");
            const id = at(n, "id");
            const out = { ":url": urlOf(n, found.screenPath || found.path, { _dynamic_container_id: sq(id) }) };
            plainAttr(out, "id", id, n);
            w("<m-dynamic-container" + A(out) + "></m-dynamic-container>\n");
        };

        const tree = (n) => {
            let items;
            let found;
            if (at(n, "transition")) {
                found = sri.staticUrl(at(n, "transition"), "transition");
                items = found.path;
            } else {
                found = sri.staticUrl("actions", "transition");
                items = found.path + "/" + at(n, "name");
            }
            const out = { id: at(n, "name"), items, ":parameters": allParams(n) };
            plainAttr(out, "open-path", at(n, "open-path"), n);
            w("<m-tree-top" + A(out) + "></m-tree-top>\n");
        };

        const subscreensPanel = (n) => {
            const type = at(n, "type");
            if (type === "popup") { w("<m-subscreens-active></m-subscreens-active>"); return; }
            if (type === "stack" || type === "wizard") { w("<h1>LATER " + e(type) + " type subscreens-panel not yet supported.</h1>"); return; }
            w("<div" + A({ id: at(n, "id") ? at(n, "id") + "-tabpanel" : null }) + "><m-subscreens-tabs></m-subscreens-tabs><m-subscreens-active></m-subscreens-active></div>");
        };
        const subscreensTabs = (n) => {
            if (at(n, "type") === "popup") return;
            w("<m-subscreens-tabs></m-subscreens-tabs>");
        };

        // ---- conditions and loops ----

        const vIf = (n, expression, attrName) => '<template v-if="' + e(X(expression, n, attrName || "condition")) + '">';
        const vFor = (n) => {
            const entry = at(n, "entry");
            const key = at(n, "key");
            const list = X(at(n, "list"), n, "list");
            const idx = entry + "_index";
            const head = key ? "(" + entry + ", " + key + ", " + idx + ")" : "(" + entry + ", " + idx + ")";
            return '<template v-for="' + e(head + " in " + list) + '" :key="' + idx + '">';
        };

        // ---- forms ----

        // The form around the fields of a form-single: the scope of its map, then the m-form that holds the values.
        const formSingleOpen = (n) => {
            push();
            st.formName = at(n, "name");
            st.fieldForm = at(n, "name");
            st.owner = at(n, "owner-form");
            st.inForm = true;
            st.inRow = false;
            const skipStart = at(n, "skip-start") === "true" || !!st.owner;
            const found = sri.staticUrl(at(n, "transition"), "transition");
            const isForm = !found.isScreen;
            const tag = isForm ? "m-form" : "m-form-link";
            const map = at(n, "map") || "fieldValues";
            const fields = kids(n, "field");
            const initial = initialOf(fields, "r", [["moquiFormName", sq(at(n, "name"))]]);
            const attrs = {
                name: at(n, "name"), id: at(n, "name"), action: found.path, method: isForm && found.method === "get" ? "GET" : null, "read-only": isForm && found.readOnly ? true : null,
                "focus-field": at(n, "focus-field") || null, "exclude-empty-fields": at(n, "exclude-empty-fields") === "true" ? true : null,
                "submit-message": isForm && at(n, "background-message") ? at(n, "background-message") : null,
                "submit-reload-id": isForm ? at(n, "background-reload-id") || null : null, "submit-hide-id": isForm ? at(n, "background-hide-id") || null : null,
                ":fields-initial": initial, "v-slot:default": "formProps"
            };
            if (at(n, "body-parameters")) attrs[":body-parameter-names"] = js_(at(n, "body-parameters").split(",").map((item) => item.trim()).filter((item) => item));
            // a row action of a form-list with row-selection posts the selected rows: the checkbox set is the selection slot's mqSelSet
            if (st.selectedForm && isForm) attrs[":parent-checkbox-set"] = "mqSelSet";
            w('<m-scope :value="' + e(X(map, n, "map")) + '" :extra="' + e("$scope[" + sq("mqExtra_" + at(n, "name")) + "]") + '" v-slot="_ctx">');
            w(skipStart ? "" : '<div class="q-my-md"><' + tag + A(attrs) + ">");
            return { skipStart, tag };
        };
        const formSingleClose = (opened) => {
            w((opened.skipStart ? "" : "</" + opened.tag + "></div>") + "</m-scope>\n");
            pop();
        };

        const fieldBox = (when, containerStyle, big) => {
            let style = null;
            if (when) {
                const target = fieldRef(at(when, "field"));
                const value = at(when, "from") ? X(at(when, "from"), when, "from") : exp(at(when, "value"), when, "value", false);
                style = "$mq.shown(" + value + "," + target + ")";
            }
            const box = big ? "q-mx-sm q-my-auto big-row-item" : cls("q-ma-sm", containerStyle || null);
            const attrs = { class: box, ":style": style };
            if (containerStyle && isDynamic(containerStyle)) { delete attrs.class; attrs[":class"] = "'q-ma-sm '+" + exp(containerStyle, null, "container-style", false); }
            w("<div" + A(attrs) + ">");
        };
        const rowOpen = (kind, style) => w("<div" + A({ class: cls(kind, style || null) }) + ">");
        const colOpen = (n, plain) => {
            const hasLg = !!at(n, "lg");
            const hasMd = !!at(n, "md");
            const hasSm = !!at(n, "sm");
            const classes = [
                plain ? null : "q-px-xs", hasLg ? "col-lg-" + at(n, "lg") : null, hasMd ? "col-md-" + at(n, "md") : hasLg && !plain ? "col-md-12" : null,
                hasSm ? "col-sm-" + at(n, "sm") : (hasLg || hasMd) && !plain ? "col-sm-12" : null,
                at(n, "xs") ? "col-xs-" + at(n, "xs") : (hasLg || hasMd || hasSm) && !plain ? "col-xs-12" : null, at(n, "style") || null
            ];
            w("<div" + A({ class: cls(...classes) }) + ">");
        };
        const fieldGroup = (n) => {
            const title = at(n, "title");
            const heading = title ? "<h5><span v-text=\"" + e(exp(title, n, "title", true)) + "\"></span></h5>" : "";
            if (st.accordion) {
                st.accIndex++;
                w("<q-list bordered padding><q-expansion-item" + A({ "switch-toggle-side": true, group: st.accordion, ":label": title ? exp(title, n, "title", true) : "ec.l10n.localize('Fields')",
                    "default-opened": String(st.accIndex) === st.accActive ? true : null }) + '><div class="' + cls("panel-body", at(n, "style") || null) + '">');
            } else if (at(n, "box") === "true") w('<div class="panel panel-default"><div class="panel-heading"><h5 class="panel-title">' + (title ? "<span v-text=\"" + e(exp(title, n, "title", true)) + "\"></span>" : "") + '</h5></div><div class="' + cls("panel-body", at(n, "style") || null) + '">');
            else w('<div class="' + cls("form-single-field-group", at(n, "style") || null) + '">' + heading);
        };
        const fieldGroupClose = (n) => w(st.accordion ? "</div></q-expansion-item></q-list>\n" : at(n, "box") === "true" ? "</div></div>\n" : "</div>\n");

        // ---- form-list ----

        // The cells of one column header: the title and the links that sort by the field.
        const headerCell = (column, cfg) => {
            w('<div class="th text-left">');
            column.forEach((f) => headerFieldCell(f, cfg));
            w("</div>");
        };
        const headerFieldCell = (f, cfg) => {
            {
                const align = f.align && f.align !== "left" ? f.align : null;
                let out = '<div><div class="' + cls("form-title", f.headerStyle || null, align === "center" ? "text-center" : align === "right" ? "text-right" : null) + '">' +
                    (f.submit ? "" : '<span v-text="' + e(f.title) + '"></span>');
                if (f.orderBy) {
                    const insensitive = f.orderBy === "case-insensitive";
                    const ascName = (insensitive ? "^" : "") + f.name;
                    out += '<span class="form-order-by"><a href="#" @click.prevent="hs.sortBy(' + e(sq(ascName)) + ')"' + ' :class="{active: hs.orderBy(' + e(sq(ascName)) + ') === \'asc\'}"><i class="fa fa-caret-up"></i></a>' +
                        '<a href="#" @click.prevent="hs.sortBy(' + e(sq("-" + ascName)) + ')"' + ' :class="{active: hs.orderBy(' + e(sq("-" + ascName)) + ') === \'desc\'}"><i class="fa fa-caret-down"></i></a></span>';
                }
                out += "</div>";
                w(out);
                if (f.find && !cfg.headerDialog) {
                    w('<div class="' + cls("form-header-field", f.headerStyle || null) + '">');
                    f.find();
                    w("</div>");
                }
                w("</div>");
            }
        };

        const formListOpen = (n, cols, cfg) => {
            push();
            const name = at(n, "name");
            const skipForm = at(n, "skip-form") === "true";
            const found = sri.staticUrl(at(n, "transition"), "transition");
            st.formName = name;
            st.fieldForm = name;
            st.listName = at(n, "list");
            const multi = !skipForm && at(n, "multi") === "true";
            st.multi = multi;
            const attrs = {
                name, id: name, action: found.path, "list-name": at(n, "list"), "skip-form": skipForm || null,
                "skip-header": at(n, "skip-header") === "true" || null, class: at(n, "style") || null, multi: multi || null
            };
            // A form that posts every row: the component makes one m-form, and each row's values start it under name_rowIndex.
            const columnsMode = at(n, "select-columns") === "true";
            st.columnsMode = columnsMode;
            if (multi || columnsMode) attrs[":row-initial"] = "(_ctx) => (" + initialOf(kids(n, "field"), "r", []) + ")";
            if (columnsMode) {
                // The column picker: the component lays out the columns, the template's or the user's, from a slot for each field.
                attrs[":default-columns"] = js_(cols.map((fields) => fields.map((field) => at(field, "name"))));
                const listed = kids(n, "field").filter((field) => kids(field, "default-field").concat(kids(field, "conditional-field")).some((sub) => !kids(sub, "hidden").length && !kids(sub, "ignored").length));
                attrs[":column-fields"] = "[" + listed.map((field) => "{id:" + sq(at(field, "name")) + ",label:" + titleExpr(first(field, "header-field") || first(field, "default-field") || kids(field, "conditional-field")[0]) + "}").join(",") + "]";
                attrs["columns-action"] = sri.staticUrl("formSelectColumns", "transition").path;
                if (cfg.subFields && cfg.subFields.length) attrs[":sub-fields"] = js_(cfg.subFields);
                attrs["form-location"] = name;
                const headerFields = kids(n, "field").filter((f) => { const sub = first(f, "header-field"); return sub && sub.children.filter((x) => x.name !== "set").length; });
                if (headerFields.length && at(n, "header-dialog") !== "true") attrs[":header-initial"] = "(_ctx, hs) => (" + initialOf(headerFields, "header-field", [["orderByField", "hs.orderByField"]]) + ")";
            }
            if (cfg.selection) {
                attrs["selection-id-field"] = cfg.selection.idField;
                attrs["selection-parameter"] = cfg.selection.parameter || null;
                attrs[":selection-list-mode"] = cfg.selection.listMode ? "true" : "false";
            }
            const kinds = Object.keys(cfg.special || {});
            if (kinds.length) {
                attrs[":special-rows"] = "{" + kinds.map((kind) => {
                    const { tag, transition } = cfg.special[kind];
                    const action = transition ? sq(sri.staticUrl(transition, "transition").path) : "null";
                    return kind + ":{action:" + action + ",initial:(_ctx) => (" + initialOf(kids(n, "field"), tag, []) + ")}";
                }).join(",") + "}";
            }
            if (at(n, "saved-finds") === "true") {
                attrs["form-location"] = name;
                attrs["find-action"] = sri.staticUrl("formSaveFind", "transition").path;
            }
            w("<m-static-form-list" + A(attrs) + ">");
            return { found };
        };

        const rowFormOpen = (n, cfg, fields, opened) => {
            const name = at(n, "name");
            const skipForm = at(n, "skip-form") === "true";
            if (skipForm) { st.inForm = false; st.owner = ""; return false; }
            st.inForm = true;
            st.owner = "";
            // The rows of a multi form bind to the one m-form of the list; formProps in the row scope maps a name to name_rowIndex.
            if (st.multi) return false;
            const initial = initialOf(fields, "r", [["moquiFormName", sq(name)], ["pageIndex", "$mq.str(" + scopeRef("pageIndex") + "||0)"]]);
            w("<m-form" + A({ ":name": sq(name + "_") + "+rowIndex", ":id": sq(name + "_") + "+rowIndex", action: opened.found.path, ":fields-initial": initial, "v-slot:default": "formProps" }) + ">");
            return true;
        };

        const cellStyle = (style, align) => cls("form-group", style || null, align === "right" ? "text-right" : align === "center" ? "text-center" : null);

        // The widgets of the field in a cell: the branch that holds, as a chain of v-if templates.
        const cellBody = (field, emitSub) => {
            const hide = at(field, "hide");
            const conds = kids(field, "conditional-field");
            const dflt = first(field, "default-field");
            if (hide) w('<template v-if="!' + e(X(hide, field, "hide")) + '">');
            conds.forEach((sub, index) => {
                w("<template " + (index === 0 ? "v-if" : "v-else-if") + '="' + e(X(at(sub, "condition"), sub, "condition")) + '">');
                emitSub(sub);
                w("</template>");
            });
            if (dflt) {
                if (conds.length) w("<template v-else>");
                emitSub(dflt);
                if (conds.length) w("</template>");
            }
            if (hide) w("</template>");
        };

        const widgetFns = {
            "text-line": textLine, "lookup": textLine, "text-area": textArea, "password": password, "file": file, "hidden": hidden, "reset": reset_, "submit": submit,
            "date-time": dateTime, "date-find": dateFind, "date-period": datePeriod, "range-find": rangeFind, "text-find": textFind, "drop-down": dropDown,
            "check": (n) => checkOrRadio(n, "checkbox"), "radio": (n) => checkOrRadio(n, "radio"), "display": displayWidget, "display-entity": displayEntity,
            "image": image, "label": labelWidget, "link": (n) => link(n, true), "editable": editable
        };
        // The page buttons of a section-iterate with paginate, over the page keys of its list in the package (ScreenSection paginates the
        // list the same way); a page goes to the screen with pageIndex. The list must be a name, which the keys are named after.
        const sectionPaginate = (n) => {
            const list = at(n, "list");
            if (!IDENT.test(list)) {
                note("section-paginate", n, "paginate of section-iterate " + at(n, "name") + " needs a list name, which the page keys are named after");
                return "";
            }
            const key = (suffix) => "Number(" + scopeRef(list + suffix) + ")";
            const props = "{count:" + key("Count") + ",pageIndex:" + key("PageIndex") + ",pageSize:" + key("PageSize") + ",pageMaxIndex:" + key("PageMaxIndex") +
                ",pageRangeLow:" + key("PageRangeLow") + ",pageRangeHigh:" + key("PageRangeHigh") + "}";
            return "<m-form-paginate" + A({ "v-if": scopeRef(list + "Count"), ":paginate": props }) + "></m-form-paginate>";
        };

        // A button-menu, as Java's: a q-btn-dropdown whose label has the tooltip, the icon (the attribute, else the theme's icon of the
        // text), the image child or the text, and the badge; each child is a q-item, shown by its own condition.
        const buttonMenuOpen = (n, img) => {
            const text = at(n, "text");
            const icon = at(n, "icon") || (text && !isDynamic(text) ? sri.getThemeIconClass(text) || "" : "");
            if (!text && !img && !icon) return "";
            if (at(n, "encode") === "false") note("encode-false", n, "button-menu with encode=false writes text as html");
            let out = (at(n, "condition") ? '<template v-if="' + e(X(at(n, "condition"), n, "condition")) + '">' : "") +
                "<q-btn-dropdown" + A({ dense: true, outline: true, "no-caps": true, color: quasarColor(at(n, "btn-type") || "primary") || null, class: at(n, "style") || null }) + "><template v-slot:label>";
            if (at(n, "tooltip")) out += '<q-tooltip><span v-text="' + e(exp(at(n, "tooltip"), n, "tooltip", true)) + '"></span></q-tooltip>';
            if (icon) out += '<i class="' + e(icon + " q-icon" + (text ? " on-left" : "")) + '"></i>';
            out += img ? imageTag(img) : '<span v-text="' + e(textOf(n)) + '"></span>';
            if (at(n, "badge")) {
                const badge = exp(at(n, "badge"), n, "badge", false);
                out += '<q-badge class="on-right" transparent' + A({ "v-if": badge, "v-text": badge }) + "></q-badge>";
            }
            return out + "</template><q-list>";
        };
        const buttonMenuItem = (kid) => "<q-item" + (at(kid, "condition") ? ' v-if="' + e(X(at(kid, "condition"), kid, "condition")) + '"' : "") + "><q-item-section>";
        const buttonMenuClose = (n) => "</q-list></q-btn-dropdown>" + (at(n, "condition") ? "</template>" : "") + "\n";

        // An XML condition element as one expression, as XmlActions.js writes it for an if: and, or and not join their parts, an expression
        // is its text, and a compare is $mq.compare, the browser's copy of the server's compare. The value of a compare with ${} is expanded.
        const xmlCondExpr = (n) => {
            const parts = (kidsOf, joiner) => (kidsOf.length ? "(" + kidsOf.map(xmlCondExpr).join(joiner) + ")" : "true");
            switch (n.name) {
                case "and": return parts(n.children || [], " && ");
                case "or": return parts(n.children || [], " || ");
                case "not": return "!(" + ((n.children || []).length ? xmlCondExpr(n.children[0]) : "true") + ")";
                case "expression": return "(" + ((n.text || "").trim() || "true") + ")";
                case "compare": {
                    const value = n.attrs && n.attrs.value !== undefined && n.attrs.value !== null ? String(n.attrs.value) : null;
                    const valueExpr = value === null ? "null" : isDynamic(value) ? exp(value, n, "value", false) : sq(value);
                    return "$mq.compare(" + (at(n, "field") || "null") + "," + sq(at(n, "operator") || "equals") + "," + valueExpr + "," +
                        (at(n, "to-field") || "null") + "," + (at(n, "format") ? sq(at(n, "format")) : "null") + "," + sq(at(n, "type") || "Object") + ")";
                }
                default: throw new Error(n.name + " is not a condition element");
            }
        };
        // The condition element of a section or a section-iterate, or "" (with a note) when it does not compile.
        const xmlCondition = (element, owner) => {
            try {
                const kidsOf = element.children || [];
                return kidsOf.length ? kidsOf.map(xmlCondExpr).join(" && ") : "";
            } catch (err) {
                note("condition", owner, "section " + at(owner, "name") + ": " + err.message);
                return "";
            }
        };

        // The elements of a field's sub-node that are not field widgets: a container, a section, a loop. These are the same elements the
        // macros write for a screen, written here from the literal because a form is read whole.
        const sectionCondition = (n) => {
            if (at(n, "condition")) return at(n, "condition");
            const element = first(n, "condition");
            if (!element) return "";
            return xmlCondition(element, n);
        };
        const nodesOf = (n, name) => { const part = first(n, name); return part ? part.children : []; };
        const renderNode = (n) => {
            const name = n.name;
            if (widgetFns[name]) { widgetFns[name](n); return; }
            if (name === "set" || name === "visible-when" || name === "ignored") return;
            if (name === "container") {
                containerOpen(n);
                n.children.forEach(renderNode);
                w("</" + (at(n, "type") || "div") + ">\n");
            } else if (name === "section") {
                const condition = sectionCondition(n);
                if (condition === null) note("condition", n, "section " + at(n, "name") + " has an XML condition that cannot be a v-if");
                if (condition) w(vIf(n, condition));
                nodesOf(n, "widgets").forEach(renderNode);
                if (condition) {
                    w("</template>");
                    if (first(n, "fail-widgets")) { w("<template v-else>"); nodesOf(n, "fail-widgets").forEach(renderNode); w("</template>"); }
                }
            } else if (name === "section-iterate") {
                const condition = sectionCondition(n);
                if (at(n, "paginate") === "true") w(sectionPaginate(n));
                w(vFor(n));
                if (condition) w(vIf(n, condition));
                nodesOf(n, "widgets").forEach(renderNode);
                if (condition) {
                    if (first(n, "fail-widgets")) { w("</template><template v-else>"); nodesOf(n, "fail-widgets").forEach(renderNode); }
                    w("</template>");
                }
                w("</template>");
            } else if (name === "container-dialog") {
                const wrapped = containerDialog(n);
                n.children.forEach(renderNode);
                w(wrapped ? "</m-container-dialog></template>\n" : "</m-container-dialog>\n");
            } else if (name === "dynamic-container") dynamicContainer(n);
            else if (name === "dynamic-dialog") dynamicDialog(n);
            else if (name === "tree") tree(n);
            else if (name === "form-single") formSingle(n);
            else if (name === "render-mode" || name === "text") {
                const texts = name === "text" ? [n] : kids(n, "text");
                const matches = (text) => !at(text, "type") || at(text, "type") === "any" || at(text, "type").split(",").map((part) => part.trim()).includes("qvt");
                texts.filter(matches).forEach((text) => {
                    const body = text.text || "";
                    if (at(text, "location") || at(text, "template") === "true" || body.indexOf("${") >= 0) note("text", text, "a text with request data or a file is not written");
                    else w(body);
                });
            } else note("widget", n, "the element " + name + " in a field is not written by the server-static macros");
        };
        const renderWidget = renderNode;
        // The widgets of a field sub-node. A set element is run on the server when the package is made (its result is in the package).
        const subWidgets = (sub) => {
            if (kids(sub, "ignored").length) return;
            sub.children.filter((widget) => widget.name !== "set" && widget.name !== "visible-when").forEach(renderWidget);
        };

        const singleField = (field, big) => {
            if (at(field, "hide")) w('<template v-if="!' + e(X(at(field, "hide"), field, "hide")) + '">');
            const conds = kids(field, "conditional-field");
            const dflt = first(field, "default-field");
            const box = (sub) => {
                if (kids(sub, "ignored").length) return;
                if (kids(sub, "hidden").length) { subWidgets(sub); return; }
                fieldBox(first(sub, "visible-when"), at(sub, "container-style"), big);
                subWidgets(sub);
                w("</div>");
            };
            conds.forEach((sub, index) => {
                w("<template " + (index === 0 ? "v-if" : "v-else-if") + '="' + e(X(at(sub, "condition"), sub, "condition")) + '">');
                box(sub);
                w("</template>");
            });
            if (dflt) {
                if (conds.length) w("<template v-else>");
                box(dflt);
                if (conds.length) w("</template>");
            }
            if (at(field, "hide")) w("</template>");
        };

        // The literal of a form comes down from the form, so each element gets its parent back for the field and form a widget reads.
        const relink = (node, parent) => {
            if (parent) node.parent = parent;
            if (!node.children) node.children = [];
            node.children.forEach((kid) => relink(kid, node));
            return node;
        };

        const formSingle = (n) => {
            relink(n, null);
            const fields = {};
            kids(n, "field").forEach((field) => { fields[at(field, "name")] = field; });
            const layout = first(n, "field-layout");
            const opened = formSingleOpen(n);
            const layoutCode = (node, big) => {
                node.children.forEach((kid) => {
                    const name = kid.name;
                    if (name === "field-ref") {
                        const field = fields[at(kid, "name")];
                        if (!field) w("<div>Error: could not find field with name " + e(at(kid, "name")) + " referred to in a field-ref.name attribute.</div>\n");
                        else singleField(field, big);
                    } else if (name === "fields-not-referenced") {
                        const referenced = {};
                        const collect = (item) => item.children.forEach((k) => { if (k.name === "field-ref") referenced[at(k, "name")] = true; collect(k); });
                        collect(layout);
                        Object.keys(fields).filter((fieldName) => !referenced[fieldName]).forEach((fieldName) => singleField(fields[fieldName], big));
                    } else if (name === "field-row") {
                        w('<div class="row">');
                        kid.children.forEach((cell) => { w('<div class="col-sm-6">'); layoutCode({ children: [cell] }, false); w("</div>"); });
                        w("</div>\n");
                    } else if (name === "field-row-big") {
                        w('<div class="q-my-sm big-row"' + (at(kid, "justify") ? ' style="justify-content:' + e(at(kid, "justify")) + ';"' : "") + ">");
                        if (at(kid, "title")) w('<div class="q-mx-sm q-my-auto big-row-item"><span v-text="' + e(exp(at(kid, "title"), kid, "title", true)) + '"></span></div>');
                        layoutCode(kid, true);
                        w("</div>\n");
                    } else if (name === "field-group") {
                        fieldGroup(kid);
                        layoutCode(kid, big);
                        fieldGroupClose(kid);
                    } else if (name === "field-accordion") {
                        // Java's accordion: a q-list of groups, each a q-expansion-item of one group, the active one (1-based, "false" for none) open
                        const before = { accordion: st.accordion, accIndex: st.accIndex, accActive: st.accActive };
                        Object.assign(st, { accordion: at(kid, "id") || (st.fieldForm || at(n, "name")) + "_accordion", accIndex: 0, accActive: at(kid, "active") || "1" });
                        w("<q-list>");
                        layoutCode(kid, big);
                        w("</q-list>\n");
                        Object.assign(st, before);
                    } else if (name === "field-col-row") {
                        rowOpen("row", at(kid, "style"));
                        kids(kid, "field-col").forEach((col) => { colOpen(col, true); layoutCode(col, false); w("</div>"); });
                        w("</div>\n");
                    }
                });
            };
            if (layout) layoutCode(layout, false);
            else Object.keys(fields).forEach((fieldName) => singleField(fields[fieldName], false));
            formSingleClose(opened);
        };

        // A column: the fields it holds as {name, align, ...} with the pieces of the head and the cell.
        const columnOf = (n, fields) => fields.map((field) => {
            const header = first(field, "header-field") || first(field, "default-field") || kids(field, "conditional-field")[0];
            const headerField = first(field, "header-field");
            const orderBy = header && (at(header, "show-order-by") === "true" || at(header, "show-order-by") === "case-insensitive") ? at(header, "show-order-by") : "";
            const finds = headerField && headerField.children.filter((x) => x.name !== "set").length && !kids(headerField, "hidden").length && !kids(headerField, "ignored").length;
            return {
                name: at(field, "name"), align: at(field, "align"), headerStyle: headerField ? at(headerField, "container-style") : "", orderBy,
                submit: !!(header && kids(header, "submit").length), title: titleExpr(header), field,
                find: finds ? () => { fieldBoxHeader(headerField); } : null
            };
        });
        const fieldBoxHeader = (sub) => {
            push();
            st.inRow = false;
            subWidgets(sub);
            pop();
        };

        // The tools above a form-list, as the server-merged nav row has them: separate find options and saved finds dialogs,
        // and the csv, xlsx, text and print exports. The component gives nv: the finds, the current parameters, and the URLs.
        const loc = (text) => "ec.l10n.localize(" + sq(text) + ")";
        const navSlot = (n, cols, headerFields, flags) => {
            if (!(flags.headerDialog || flags.savedFinds || flags.csv || flags.xlsx || flags.text || flags.pdf || flags.columns)) return;
            const name = at(n, "name");
            w('<template v-slot:nav="nv">');
            if (flags.headerDialog) {
                w("<m-container-dialog" + A({ id: name + "_hdialog", ":title": loc("Find Options") }) + '><template v-slot:button><q-btn dense outline no-caps icon="search"' + A({ ":label": loc("Find Options") }) + "></q-btn></template>");
                findDialogForm(n, cols, headerFields);
                w("</m-container-dialog>");
            }
            if (flags.columns) {
                w("<m-container-dialog" + A({ id: name + "_SelColsDialog", ":title": loc("Column Fields") }) + '><template v-slot:button><q-btn dense outline no-caps icon="table_chart"' + A({ ":label": loc("Columns") }) + "></q-btn></template>");
                w("<m-form-column-config" + A({ id: name + "_SelColsForm", ":action": "nv.columnsAction", ":find-parameters": "nv.params", ":columns-initial": "nv.columnsTree", "form-location": name }) + "></m-form-column-config>");
                w("</m-container-dialog>");
            }
            if (flags.savedFinds) {
                w("<m-container-dialog" + A({ id: name + "_sfdialog", ":title": loc("Saved Finds") }) + '><template v-slot:button><q-btn dense outline no-caps icon="find_in_page"' + A({ ":label": loc("Saved Finds") }) + "></q-btn></template>");
                savedFindsBody(name);
                w("</m-container-dialog>");
            }
            if (flags.csv) w('<q-btn dense outline type="a"' + A({ ":href": "nv.exportUrl('csv')", ":label": loc("CSV") }) + "></q-btn>");
            if (flags.xlsx) w('<q-btn dense outline type="a"' + A({ ":href": "nv.exportUrl('xlsx')", ":label": loc("XLS") }) + "></q-btn>");
            if (flags.text) textExportDialog(name);
            if (flags.pdf) w('<q-btn dense outline type="a" target="_blank"' + A({ ":href": "nv.printUrl()", ":label": loc("Print") }) + "></q-btn>");
            w("</template>");
        };
        const savedFindsBody = (name) => {
            const button = (buttonName, text, extra) => '<q-btn dense flat no-caps type="submit"' + A(Object.assign({ name: buttonName, value: buttonName ? "Y" : null, ":label": loc(text) }, extra || {})) + "></q-btn>";
            w('<q-list dense class="q-my-sm" v-if="nv.finds.length">');
            w('<q-item v-for="mqFind in nv.finds" :key="mqFind.formListFindId" :active="mqFind.formListFindId === nv.activeFindId">');
            w('<q-item-section><m-link :href="nv.findUrl(mqFind.formListFindId)"><span v-text="(mqFind.description || mqFind.formListFindId) + (mqFind.formListFindId === nv.defaultFindId ? \' (\' + ' + e(loc("Default")) + ' + \')\' : \'\')"></span></m-link></q-item-section>');
            w('<q-item-section side><m-form class="form-inline" :action="nv.saveFindAction" :fields-initial="nv.findFields(mqFind.formListFindId)">');
            w(button(null, "Update to Current", { "v-if": "mqFind.isByUserId === 'true' && nv.hasParams" }));
            w(button("ClearDefault", "Clear Default", { "v-if": "mqFind.formListFindId === nv.defaultFindId" }));
            w(button("MakeDefault", "Make Default", { "v-else": true }));
            w(button("DeleteFind", "Delete", { "v-if": "mqFind.isByUserId === 'true'", color: "negative" }));
            w("</m-form></q-item-section></q-item></q-list>");
            w("<m-form" + A({ "v-if": "nv.hasParams", class: "form-inline", id: name + "_NewFind", ":action": "nv.saveFindAction", ":fields-initial": "nv.newFindFields()", "v-slot:default": "formProps" }) + '><div class="big-row"><div class="q-my-auto big-row-item"><m-text-line' +
                A({ "v-model": "formProps.fields._findDescription", name: "_findDescription", id: name + "_NewFind_description", ":label": loc("Description"), size: "30", required: true }) +
                '></m-text-line></div><div class="on-right q-my-auto big-row-item"><q-btn dense outline no-caps type="submit"' + A({ ":label": loc("Save New Find") }) + "></q-btn></div></div></m-form>");
            w('<div v-else style="margin:12px 0;"><strong' + A({ "v-text": loc("No find parameters (or default), choose some in Find Options to save a new find or update existing") }) + "></strong></div>");
        };
        // The header fields as a find form in the dialog, with clear and order-by, which goes to the screen with the fields as parameters.
        const findDialogForm = (n, cols, headerFields) => {
            const name = at(n, "name");
            const orderOptions = [];
            cols.flat().filter((f) => f.orderBy).forEach((f) => {
                const ci = f.orderBy === "case-insensitive" ? "^" : "";
                orderOptions.push("{value:" + sq(ci + f.name) + ",label:" + f.title + "+' '+" + loc("(Asc)") + "}", "{value:" + sq("-" + ci + f.name) + ",label:" + f.title + "+' '+" + loc("(Desc)") + "}");
            });
            push();
            st.owner = name + "_header";
            st.fieldForm = name + "_header";
            st.inForm = true;
            st.inRow = false;
            w("<m-form-link" + A({ name: name + "_header", id: name + "_header", ":action": "nv.findAction", ":fields-initial": initialOf(headerFields, "header-field", [["orderByField", "nv.orderByField"]]), "v-slot:default": "formProps" }) + ">");
            w('<div class="q-mx-sm"><q-btn dense outline no-caps name="clearParameters" @click.prevent="formProps.clearForm"' + A({ ":label": loc("Clear Parameters") }) + "></q-btn>");
            w("<m-drop-down" + A({ name: "orderByField", id: name + "_header_orderByField", ":label": loc("Order By"), multiple: true, "allow-empty": true, ":options": "[" + orderOptions.join(",") + "]", "v-model": "formProps.fields.orderByField" }) + "></m-drop-down></div>");
            cols.flat().forEach((f) => {
                if (!f.find) return;
                w('<div class="q-mx-sm q-my-sm"><div class="form-title"><span v-text="' + e(f.title) + '"></span></div>');
                f.find();
                w("</div>");
            });
            w("</m-form-link>");
            pop();
        };
        // The fixed-width text export: a plain form posted to the screen with renderMode text, the current parameters, and the sizes.
        const textExportDialog = (name) => {
            const row = (title, fieldName, value, size) => '<div class="q-my-sm"><m-text-line' + A({ name: fieldName, id: name + "_Text_" + fieldName, ":label": loc(title), "model-value": value, form: name + "_Text", size }) + "></m-text-line></div>";
            w("<m-container-dialog" + A({ id: name + "_TextDialog", ":button-text": loc("Text"), ":title": loc("Export Fixed-Width Plain Text") }) + ">");
            w("<form" + A({ id: name + "_Text", method: "post", ":action": "nv.screenPath" }) + '><input type="hidden" name="renderMode" value="text"><input type="hidden" name="pageNoLimit" value="true"><input type="hidden" name="lastStandalone" value="true">');
            w('<input v-for="(mqValue, mqKey) in nv.params" :key="mqKey" type="hidden" :name="mqKey" :value="mqValue">');
            w(row("Line Characters", "lineCharacters", "132", "4") + row("Page Lines", "pageLines", "88", "4") + row("Line Wrap?", "lineWrap", "true", "5") + row("Save to Filename", "saveFilename", name + ".txt", "40"));
            w('<q-btn dense outline no-caps type="submit"' + A({ ":label": loc("Export Text") }) + "></q-btn></form></m-container-dialog>");
        };

        // The cells of a form-list with the column picker: a header slot (hc_) and a row slot (rc_) for each field, which the component places in
        // the columns the package names, or the template's own. The header slot gets hs and the formProps of the header find form; the row slot
        // the row's scope, whose formProps is the row form the component makes.
        const columnSlots = (n, cols, headerFields, flags, cfg) => {
            const name = at(n, "name");
            const skipForm = at(n, "skip-form") === "true";
            const headerForm = headerFields.length && !flags.headerDialog;
            if (at(n, "skip-header") !== "true") {
                cols.flat().forEach((f) => {
                    w("<template" + A({ ["v-slot:hc_" + f.name]: "{hs, formProps}" }) + ">");
                    push();
                    st.owner = headerForm ? name + "_header" : "";
                    st.fieldForm = name + "_header";
                    st.inForm = !!headerForm;
                    st.inRow = false;
                    headerFieldCell(f, cfg);
                    pop();
                    w("</template>");
                });
            }
            Object.keys(cfg.special || {}).forEach((kind) => {
                const { tag, transition } = cfg.special[kind];
                kids(n, "field").forEach((field) => {
                    const sub = first(field, tag);
                    if (!sub || !sub.children.length || kids(sub, "hidden").length) return;
                    w("<template" + A({ ["v-slot:rc" + kind.charAt(0) + "_" + at(field, "name")]: "_ctx" }) + ">");
                    push();
                    Object.assign(st, { inRow: false, inForm: !!transition, fieldForm: name + "_" + kind, owner: "" });
                    specialCell(field, tag);
                    pop();
                    w("</template>");
                });
            });
            kids(n, "field").forEach((field) => {
                const fieldName = at(field, "name");
                w("<template" + A({ ["v-slot:rc_" + fieldName]: "_ctx" }) + ">");
                push();
                st.inRow = true;
                // a sub-list field is a cell of a sub-row, which is outside the row's form
                st.inForm = !skipForm && at(field, "aggregate") !== "sub-list";
                st.owner = "";
                cellBody(field, (sub) => {
                    if (kids(sub, "ignored").length) return;
                    if (kids(sub, "hidden").length) { subWidgets(sub); return; }
                    w("<div" + A({ class: cellStyle(at(sub, "container-style"), at(field, "align")) }) + ">");
                    subWidgets(sub);
                    w("</div>");
                });
                pop();
                w("</template>");
            });
        };

        // The checkbox of a row of a form-list with row-selection, bound to the checkbox set (mqSel in the row's scope), written when the row has
        // an id, as Java's.
        const selectionCell = (selection) => '<div class="td"><div class="q-my-auto"' + A({ "v-if": "$mq.str(" + scopeRef(selection.idField) + ")" }) +
            '><input type="checkbox" style="transform: scale(1.3)" :checked="mqSel.checkboxStates[rowIndex]" @click="mqSel.clickCheckbox($event, rowIndex)" @change="mqSel.checkboxStates[rowIndex] = $event.target.checked"></div></div>';
        // The actions of the selected rows, in the menu of the head: each action's form-single, in a dialog when it has one, posting the
        // selected ids through the checkbox set (Java's formListSelectedRowCard).
        const selectionSlot = (selection) => {
            w('<template v-slot:selection="mqSelSet"><q-card flat bordered><q-card-section horizontal class="q-pa-md">');
            kids(selection.node, "action").forEach((action) => {
                const dialog = first(action, "dialog");
                const form = first(action, "form-single");
                if (dialog) {
                    const out = { color: quasarColor(at(dialog, "button-type") || "primary") || null, width: at(dialog, "width") || null, "button-class": at(dialog, "button-style") || null };
                    textAttr(out, "button-text", at(dialog, "button-text"), dialog, true);
                    textAttr(out, "title", at(dialog, "title") || at(dialog, "button-text"), dialog, true);
                    w('<span class="q-my-md"><div class="q-ma-sm"><m-container-dialog' + A(out) + ">");
                }
                if (form) {
                    push();
                    st.selectedForm = true;
                    formSingle(form);
                    pop();
                }
                if (dialog) w("</m-container-dialog></div></span>");
            });
            w("</q-card-section></q-card></template>");
        };
        // A cell of a first, second or last row: the field's *-row-field widgets. One that holds a hidden widget writes nothing; its value is
        // in the row form's starting values.
        const specialCell = (field, tag) => {
            const sub = first(field, tag);
            if (!sub || !sub.children.length || kids(sub, "hidden").length) return;
            w("<div" + A({ class: cellStyle(at(sub, "container-style"), at(field, "align")) }) + ">");
            subWidgets(sub);
            w("</div>");
        };
        // The sub-rows of an aggregated list: the titles of the sub-list columns for the head, and the cells of one sub-row over its entry.
        const subSlots = (subLists) => {
            w("<template v-slot:subhead>");
            subLists.forEach((fields) => w('<div class="col">' + fields.map((field) => '<span v-text="' + e(titleExpr(first(field, "header-field") || first(field, "default-field") || kids(field, "conditional-field")[0])) + '"></span>').join(" ") + "</div>"));
            w("</template>");
            w('<template v-slot:subrow="_ctx">');
            push();
            st.inRow = true;
            st.inForm = false;
            subLists.forEach((fields) => {
                w('<div class="col">');
                fields.forEach((field) => cellBody(field, (sub) => {
                    if (kids(sub, "ignored").length || kids(sub, "hidden").length) return;
                    w("<div" + A({ class: cellStyle(at(sub, "container-style"), at(field, "align")) }) + ">");
                    subWidgets(sub);
                    w("</div>");
                }));
                w("</div>");
            });
            pop();
            w("</template>");
        };

        const formList = (n, columnFieldLists, allFields) => {
            const cfg = {};
            const name = at(n, "name");
            // the first, second and last rows: the fields' *-row-field widgets over the map-*-row map, in a form when transition-*-row names one
            cfg.special = {};
            ["first", "second", "last"].forEach((kind) => {
                const tag = kind + "-row-field";
                if (allFields.some((f) => { const sub = first(f, tag); return sub && sub.children.length; })) cfg.special[kind] = { tag, transition: at(n, "transition-" + kind + "-row") };
            });
            const selection = first(n, "row-selection");
            cfg.selection = selection ? { idField: at(selection, "id-field"), parameter: at(selection, "parameter"), listMode: at(selection, "list-mode") === "true", node: selection } : null;
            const flags = {
                headerDialog: at(n, "header-dialog") === "true", savedFinds: at(n, "saved-finds") === "true",
                csv: at(n, "show-csv-button") === "true", xlsx: at(n, "show-xlsx-button") === "true" && ec.screen.isRenderModeValid("xlsx"),
                text: at(n, "show-text-button") === "true", pdf: at(n, "show-pdf-button") === "true", columns: at(n, "select-columns") === "true"
            };
            cfg.headerDialog = flags.headerDialog;
            // An aggregated list's sub-list fields are not main columns: they are the columns of the sub-rows under each row (Java's subColInfo).
            const isSub = (field) => at(field, "aggregate") === "sub-list";
            const subLists = columnFieldLists.map((fields) => fields.filter(isSub)).filter((fields) => fields.length);
            columnFieldLists = columnFieldLists.map((fields) => fields.filter((field) => !isSub(field))).filter((fields) => fields.length);
            cfg.subFields = kids(n, "field").filter(isSub).map((field) => at(field, "name"));
            const opened = formListOpen(n, columnFieldLists, cfg);
            const cols = columnFieldLists.map((fields) => columnOf(n, fields));
            const skipHeader = at(n, "skip-header") === "true";
            const headerFields = allFields.filter((f) => { const sub = first(f, "header-field"); return sub && sub.children.filter((x) => x.name !== "set").length; });
            if (!skipHeader) navSlot(n, cols, headerFields, flags);
            if (flags.columns) {
                columnSlots(n, cols, headerFields, flags, cfg);
                if (cfg.selection) selectionSlot(cfg.selection);
                w("</m-static-form-list>\n");
                pop();
                return;
            }
            if (!skipHeader) {
                w('<template v-slot:header="hs">');
                if (headerFields.length && !flags.headerDialog) {
                    push();
                    st.owner = name + "_header";
                    st.fieldForm = name + "_header";
                    st.inForm = true;
                    w("<m-form-link" + A({ name: name + "_header", id: name + "_header", ":action": "hs.action", ":fields-initial": initialOf(headerFields, "header-field", [["orderByField", "hs.orderByField"]]), "v-slot:default": "formProps" }) + ">");
                    cols.forEach((col) => headerCell(col, cfg));
                    w("</m-form-link>");
                    pop();
                } else {
                    push();
                    st.inForm = false;
                    cols.forEach((col) => headerCell(col, cfg));
                    pop();
                }
                w("</template>");
            }
            w('<template v-slot:row="_ctx">');
            push();
            st.inRow = true;
            // the show-total row, which the component draws after the rows, is bold as the server-merged one is
            w('<div class="tr" :class="{\'text-weight-bold\': _moquiRowType === \'total\'}">');
            if (cfg.selection) w(selectionCell(cfg.selection));
            const formOpened = rowFormOpen(n, cfg, kids(n, "field"), opened);
            columnFieldLists.forEach((fields) => {
                w('<div class="td">');
                fields.forEach((field) => {
                    cellBody(field, (sub) => {
                        if (kids(sub, "ignored").length) return;
                        if (kids(sub, "hidden").length) { subWidgets(sub); return; }
                        w("<div" + A({ class: cellStyle(at(sub, "container-style"), at(field, "align")) }) + ">");
                        subWidgets(sub);
                        w("</div>");
                    });
                });
                w("</div>");
            });
            if (formOpened) w("</m-form>");
            w("</div>");
            // fields with only a hidden widget are not in a column but their values still start the form
            pop();
            w("</template>");
            Object.keys(cfg.special).forEach((kind) => {
                const { tag, transition } = cfg.special[kind];
                w('<template v-slot:row_' + kind + '="_ctx">');
                push();
                Object.assign(st, { inRow: false, inForm: !!transition, fieldForm: name + "_" + kind, owner: "" });
                w('<div class="tr ' + kind + '">' + (cfg.selection ? '<div class="td">&nbsp;</div>' : ""));
                columnFieldLists.forEach((fields) => { w('<div class="td">'); fields.forEach((field) => specialCell(field, tag)); w("</div>"); });
                w("</div>");
                pop();
                w("</template>");
            });
            if (subLists.length) subSlots(subLists);
            if (cfg.selection) selectionSlot(cfg.selection);
            w("</m-static-form-list>\n");
            pop();
        };

        const containerOpen = (n) => {
            const out = { class: at(n, "style") || null };
            if (isDynamic(out.class || "")) { out[":class"] = exp(out.class, n, "style", false); delete out.class; }
            plainAttr(out, "id", at(n, "id"), n);
            w("<" + (at(n, "type") || "div") + A(out) + ">");
        };
        const boxOpen = (n, header) => {
            const out = { type: at(n, "type") || "default", "initial-open": at(n, "initial") === "closed" ? "false" : null };
            plainAttr(out, "id", at(n, "id"), n);
            if (header && at(header, "title")) out[":title"] = exp(at(header, "title"), header, "title", true);
            w("<m-container-box" + A(out) + ">");
        };
        // A form-list with its columns given as field names.
        const formListWith = (n, columnNames) => {
            relink(n, null);
            const fields = kids(n, "field");
            const byName = {};
            fields.forEach((field) => { byName[at(field, "name")] = field; });
            formList(n, columnNames.map((column) => column.map((name) => byName[name]).filter((field) => field)), fields);
        };

        const h = {
            containerOpen, boxOpen, formListWith, paramsOf, allParams,
            reset, push, pop, e, A, cls, at, X, exp, st, sq, w, note, vIf, vFor, withCondition, urlOf, textOf,
            containerDialog, dynamicDialog, dynamicContainer, tree, subscreensPanel, subscreensTabs, image, label: labelWidget,
            link: (n, inField) => link(n, inField), editable, formSingle, formList, rowOpen, colOpen, fieldGroup, fieldGroupClose, quasarColor,
            split, js: js_, widgetFns, imageTag, xmlCondition, sectionPaginate, buttonMenuOpen, buttonMenuItem, buttonMenuClose
        };
        Object.defineProperty(globalThis, "$mqQvtS", { value: h, configurable: true });
    };

    const runtime = "(" + runtimeFunction.toString() + ")();";
    const prelude = "const $s = globalThis.$mqQvtS;\n$s.reset();\n";

    // ---- macros ----

    const call = (name, node, withChildren) => `$s.${name}(${wlit(node, withChildren)});\n`;
    const write = (text) => `sri.write(${lit(text)});\n`;

    // The condition of a section as code for the unit: the attribute, an expression element alone, or the condition element compiled at
    // render time by $s.xmlCondition (and, or, not, compare, expression). "" is no condition.
    const conditionCode = (node) => {
        if (attr(node, "condition")) return lit(attr(node, "condition"));
        const element = child(node, "condition");
        if (!element) return "";
        if (element.children.length === 1 && element.children[0].name === "expression" && !element.children[0].children.length) return lit((element.children[0].text || "").trim());
        return `$s.xmlCondition(${wlit(element)}, ${wlit(node, false)})`;
    };

    const widgetsMacro = stmt((node, children) => children);

    const sectionMacro = stmt((node) => {
        const widgets = child(node, "widgets");
        const fail = child(node, "fail-widgets");
        const condition = conditionCode(node);
        const body = widgets ? widgets.source || "" : "";
        if (!condition) return body;
        return `{\nconst $cond = ${condition};\nif ($cond) sri.write($s.vIf(${wlit(node, false)}, $cond));\n${body}if ($cond) {\nsri.write("</template>");\n` +
            (fail ? `sri.write("<template v-else>");\n${fail.source || ""}sri.write("</template>");\n` : "") + "}\n}\n";
    });

    const sectionIterateMacro = stmt((node) => {
        const widgets = child(node, "widgets");
        const fail = child(node, "fail-widgets");
        const condition = conditionCode(node);
        let code = "";
        if (attr(node, "paginate") === "true") code += `sri.write($s.sectionPaginate(${wlit(node, false)}));\n`;
        code += `sri.write($s.vFor(${wlit(node, false)}));\n`;
        if (condition) code += `const $cond = ${condition};\nif ($cond) sri.write($s.vIf(${wlit(node, false)}, $cond));\n`;
        code += widgets ? widgets.source || "" : "";
        if (condition) {
            code += `if ($cond) {\n`;
            if (fail) code += `sri.write("</template><template v-else>");\n${fail.source || ""}`;
            code += `sri.write("</template>");\n}\n`;
        }
        return "{\n" + code + `sri.write("</template>");\n}\n`;
    });

    const containerMacro = stmt((node, children) => `$s.containerOpen(${wlit(node, false)});\n${children}sri.write(${lit(`</${attr(node, "type") || "div"}>\n`)});\n`);

    const containerBoxMacro = stmt((node) => {
        const header = child(node, "box-header");
        const body = child(node, "box-body");
        const nopad = child(node, "box-body-nopad");
        const toolbar = child(node, "box-toolbar");
        return `$s.boxOpen(${wlit(node, false)}, ${header ? wlit(header, false) : "null"});\n` +
            (header ? `sri.write("<template v-slot:header>");\n${header.source || ""}sri.write("</template>");\n` : "") +
            (toolbar ? `sri.write("<template v-slot:toolbar>");\n${toolbar.source || ""}sri.write("</template>");\n` : "") +
            (body ? `sri.write(${lit(`<m-box-body${attr(body, "height") ? ` height="${attr(body, "height")}"` : ""}>`)});\n${body.source || ""}sri.write("</m-box-body>");\n` : "") +
            (nopad ? nopad.source || "" : "") + `sri.write("</m-container-box>\\n");\n`;
    });

    const containerRowMacro = stmt((node) => `$s.rowOpen("row", ${lit(attr(node, "style"))});\n` +
        childrenNamed(node, "row-col").map((col) => `$s.colOpen(${wlit(col, false)}, false);\n${col.source || ""}sri.write("</div>");\n`).join("") + `sri.write("</div>\\n");\n`);

    const containerPanelMacro = stmt((node) => {
        const part = (name) => { const kid = child(node, name); return kid ? kid.source || "" : ""; };
        const sized = (name, cls) => {
            const kid = child(node, name);
            return kid ? `sri.write(${lit(`<div class="${cls}" style="width:${attr(kid, "size") || "180"}px;">`)});\n${kid.source || ""}sri.write("</div>");\n` : "";
        };
        const centerClass = child(node, "panel-left") ? (child(node, "panel-right") ? "container-panel-center-both" : "container-panel-center-left") :
            (child(node, "panel-right") ? "container-panel-center-right" : "container-panel-center-only");
        return write(`<div${attr(node, "id") ? ` id="${attr(node, "id")}"` : ""} class="container-panel-outer">`) +
            (child(node, "panel-header") ? write('<div class="container-panel-header">') + part("panel-header") + write("</div>") : "") +
            write('<div class="container-panel-middle">') + sized("panel-left", "container-panel-left") + write(`<div class="${centerClass}">`) + part("panel-center") + write("</div>") +
            sized("panel-right", "container-panel-right") + write("</div>") +
            (child(node, "panel-footer") ? write('<div class="container-panel-footer">') + part("panel-footer") + write("</div>") : "") + write("</div>\n");
    });

    const containerDialogMacro = stmt((node, children) => `{\n    const $c = $s.containerDialog(${wlit(node, false)});\n${children}    sri.write($c ? "</m-container-dialog></template>\\n" : "</m-container-dialog>\\n");\n}\n`);

    const buttonMenuMacro = stmt((node) => {
        const image = child(node, "image");
        const items = node.children.filter((kid) => kid.name !== "image" && kid.name !== "parameter");
        return `{\nconst $bm = $s.buttonMenuOpen(${wlit(node, false)}, ${image ? wlit(image) : "null"});\nif ($bm) {\nsri.write($bm);\n` +
            items.map((kid) => `sri.write($s.buttonMenuItem(${wlit(kid, false)}));\n${kid.source || ""}sri.write("</q-item-section></q-item>");\n`).join("") +
            `sri.write($s.buttonMenuClose(${wlit(node, false)}));\n}\n}\n`;
    });

    const formSingleMacro = stmt((node) => `$s.formSingle(${wlit(node, true)});\n`);

    const formListMacro = stmt((node) => {
        const columns = S.formListColumns(node);
        // The columns are lists of field names, and the runtime finds the fields in the form it is given.
        return `$s.formListWith(${wlit(node, true)}, ${JSON.stringify(columns.map((column) => column.map((field) => attr(field, "name"))))});\n`;
    });

    const typeMatches = (type) => !type || type === "any" || type.split(",").map((name) => name.trim()).includes(MODE);

    const textMacro = stmt((node) => {
        if (!typeMatches(attr(node, "type"))) return "";
        const inline = node.text || "";
        let code = "";
        if (attr(node, "location")) code += `sri.staticNote("text", "text", ${lit("text at " + attr(node, "location") + " is not written")});\n`;
        if (inline.trim() !== "") {
            const template = attr(node, "template");
            if (template === "true" || (template !== "false" && (inline.indexOf("${") >= 0 || inline.indexOf("<#") >= 0))) {
                code += `sri.staticNote("text", "text", "an inline text template has request data");\n`;
            } else code += `sri.write(${lit(inline)});\n`;
        }
        return code;
    });

    const renderModeMacro = stmt((node) => {
        const texts = childrenNamed(node, "text");
        let chosen = null;
        texts.forEach((kid) => { if (!attr(kid, "type") || attr(kid, "type") === "any") chosen = kid; });
        texts.forEach((kid) => { if (attr(kid, "type") && typeMatches(attr(kid, "type"))) chosen = kid; });
        return chosen ? chosen.source || "" : "";
    });

    const linkMacro = stmt((node) => `$s.link(${wlit(node, true)}, ${!!(node.parent && node.parent.name.endsWith("-field"))});\n`);
    const widget = (name, withChildren) => stmt((node) => call(name, node, withChildren));
    const noteMacro = (kind, what) => stmt((node) => `sri.staticNote(${lit(kind)}, ${lit(node.name)}, ${lit(what + " " + (attr(node, "name") || attr(node, "location")) + " was not resolved")});\n`);

    const macros = Object.assign({}, noops(S.allElements), noops(["condition", "actions"]), {
        "screen": stmt((node, children) => children),
        "widgets": widgetsMacro,
        "fail-widgets": widgetsMacro,
        "dynamic-form": widgetsMacro,
        "subscreens-menu": widget("subscreensTabs", false),
        "subscreens-panel": widget("subscreensPanel", false),
        "subscreens-active": stmt(() => write("<m-subscreens-active></m-subscreens-active>")),
        "section": sectionMacro,
        "section-iterate": sectionIterateMacro,
        "section-include": noteMacro("section-include", "section-include"),
        "include-screen": noteMacro("include-screen", "include-screen"),
        "render-mode": renderModeMacro,
        "text": textMacro,
        "render-dynamic-form": noteMacro("dynamic-form", "form"),
        "form-list-user-columns": stmt((node, children) => children),

        "container": containerMacro,
        "container-box": containerBoxMacro,
        "box-header": widgetsMacro,
        "box-toolbar": widgetsMacro,
        "box-body": widgetsMacro,
        "box-body-nopad": widgetsMacro,
        "container-row": containerRowMacro,
        "row-col": widgetsMacro,
        "container-panel": containerPanelMacro,
        "panel-header": widgetsMacro,
        "panel-left": widgetsMacro,
        "panel-center": widgetsMacro,
        "panel-right": widgetsMacro,
        "panel-footer": widgetsMacro,
        "container-dialog": containerDialogMacro,
        "dynamic-container": widget("dynamicContainer", true),
        "dynamic-dialog": widget("dynamicDialog", true),
        "button-menu": buttonMenuMacro,
        "menu-item": stmt(() => ""),
        "tree": widget("tree", true),
        "link": linkMacro,
        "image": widget("image", true),
        "label": widget("label", false),
        "editable": widget("editable", true),
        "form-single": formSingleMacro,
        "form-list": formListMacro
    });

    // Forms, links and the like are read whole by the runtime, so the walker does not descend into them. A section and its
    // widgets are walked, so the template of a section is the code of its children.
    const opaque = S.opaqueElements
        .filter((name) => !["section", "section-iterate", "section-include", "button-menu", "row-selection", "action"].includes(name))
        .concat(["condition", "actions", "form-single", "form-list", "link", "image", "label", "editable", "dynamic-container", "dynamic-dialog", "row-selection"]);

    return { macros, opaque, prelude, runtime };
})
