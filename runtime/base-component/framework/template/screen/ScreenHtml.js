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

// The html render mode: terse semantic HTML and a few data attributes, bound by the one shared script
// /js/moqui-html.js. This follows DefaultScreenMacros.html.ftl without its Bootstrap classes, jQuery
// calls or per-widget scripts. The attributes the script reads are in
// runtime/base-component/webroot/screen/webroot/js/README.md.
//
// A macro here builds the code of its element. A widget calls a function of the runtime object, which is
// installed once for each script engine (the module's runtime property) and read by every unit as $h.
// The runtime holds the markup of each widget, so a form-list of many rows runs plain functions and
// the generated unit stays short. The elements a unit needs from the layout of its form (title, field
// chain, columns) are decided here, when the code is made.
//
// Differences from the FreeMarker macros:
//  - Option text, labels and display values are always encoded.
//  - The text-line ac-transition is a select that Tom Select fills from the transition, so the typed text
//    is not posted as name_ac.
//  - A form-list with sub-list fields (aggregate="sub-list") draws them in a table under each main row, and its
//    show-total row is the last row, with data-total. The saved finds and
//    column dialogs post to the implicit formSaveFind and formSelectColumns transitions, with the form's name as
//    formLocation (the server makes the full location of it).
//  - lookup is a text line. The Java macros had it commented out.
//  - A show-pdf-button is a link to the html-print version of the page.
(function (T, S) {
    "use strict";

    const { attr, has, jsString, child, childrenNamed, js, raw, stmt, noops } = T;
    const { bareLit, fieldChain, widgetsSource, setCode, titleText } = S;

    const MODE = "html";

    const lit = jsString;
    const dynamic = (text) => text.indexOf("${") >= 0;
    const noL10n = (text) => (dynamic(text) ? S.expandNoL10nExpr(text) : lit(text));
    const expandCode = (text) => (dynamic(text) ? S.expandExpr(text) : lit(text));

    // The element as the runtime and the host read it. A widget in a field carries the field and the name and map of
    // its form, which is all they use of the elements above it, so a form-list does not repeat its attributes in each widget.
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

    // The code that gives the localized title of a field: its title, else the words of its name. A
    // header-field takes the title of the default-field when it has none.
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

    // The runtime. It is source text: the body of this function is run once for each script engine. It
    // reads sri, ec and the context by bare name, so nothing here belongs to one request.
    const runtimeFunction = function () {
        "use strict";

        const ENT = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" };
        const e = (value) => {
            if (value === null || value === undefined) return "";
            const text = String(value);
            return /[&<>"']/.test(text) ? text.replace(/[&<>"']/g, (ch) => ENT[ch]) : text;
        };
        // Attributes from an object. A null, undefined or false value is left out and true is a bare attribute.
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
        const param = (key) => {
            const map = ec.web.parameters;
            const value = map ? map[key] : null;
            return value === null || value === undefined ? "" : String(value);
        };
        const token = () => ec.web.sessionToken || "";
        const json = (map) => {
            const out = {};
            for (const key of Object.keys(map)) if (map[key] !== null && map[key] !== undefined && map[key] !== "") out[key] = map[key];
            return Object.keys(out).length ? JSON.stringify(out) : null;
        };

        // Per-unit state of the form being written.
        const st = {};
        const reset = () => {
            Object.assign(st, {
                formName: "", fieldForm: "", owner: "", entry: "", multi: false, skipForm: false,
                labelCols: "", accordion: "", accIndex: 0, accActive: "", inRow: false, bigRow: false
            });
        };
        reset();

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
        const fidByName = (name) => (st.fieldForm || xn(st.formName)) + "_" + name + entry() + sec();
        const fnm = (n) => fname(n) + (st.multi && st.entry !== "" && /^\d+$/.test(st.entry) ? "_" + st.entry : "");
        const owner = () => st.owner || null;
        const tip = (n) => (at(n.parent, "tooltip") ? x(at(n.parent, "tooltip")) : null);
        const icon = (name) => (name ? '<i class="' + e(name) + '"></i> ' : "");

        // The libraries a page loads for widgets that need them. footer_scripts and footer_stylesheets
        // are sets the root screen makes; a render without them leaves the client to load the library.
        const LIBS = {
            "tom-select": { js: "/libs/tom-select/tom-select.complete.min.js", css: "/libs/tom-select/tom-select.css" },
            sortable: { js: "/libs/sortablejs/Sortable.min.js" }
        };
        const lib = (key) => {
            const item = LIBS[key];
            if (item.js && footer_scripts) footer_scripts.add(item.js);
            if (item.css && footer_stylesheets) footer_stylesheets.add(item.css);
        };

        // Remote options: the URL, its fixed parameters, and the fields it depends on as [field id, parameter].
        const remote = (holder, transition) => {
            const url = sri.makeUrlByType(transition, "transition", holder, "false");
            const params = Object.assign({}, url.getParameterMap());
            const deps = kids(holder, "depends-on").map((dep) => [at(dep, "field"), at(dep, "parameter") || at(dep, "field")]);
            deps.forEach((dep) => { delete params[dep[1]]; });
            return {
                "data-url": url.url,
                "data-params": json(params),
                "data-depends": deps.length ? JSON.stringify(deps.map((dep) => [fidByName(dep[0]), dep[1]])) : null,
                "data-optional": at(holder, "depends-optional") === "true" ? true : null
            };
        };

        const validation = (sub) => (at(sub, "validate-service") || at(sub, "validate-entity") ? sri.getFieldValidation(sub) : { classes: "" });
        const validate = (sub) => {
            const v = validation(sub);
            const has = (name) => v.classes.indexOf(name) >= 0;
            return { required: has("required"), email: has("email"), url: has("url"), number: has("number"), digits: has("digits"), v };
        };

        // ---- inputs ----

        const textLine = (n) => {
            const id = fid(n);
            const name = fnm(n);
            const value = sri.getFieldValueString(n);
            const check = validate(n.parent);
            const pattern = check.v.regexp ? { pattern: check.v.regexp, "data-msg-pattern": check.v.message || "Invalid format" } : {};
            const type = at(n, "input-type") || (check.email ? "email" : check.url ? "url" : "text");
            const disabled = cond(at(n, "disabled"));
            if (at(n, "ac-transition")) {
                lib("tom-select");
                const remoteAttrs = remote(n, at(n, "ac-transition"));
                const initial = at(n, "ac-initial-text") ? x(at(n, "ac-initial-text")) : value;
                w("<select" + A(Object.assign({
                    name, id, "data-ts": "ac", "data-search": true, "data-min": at(n, "ac-min-length") || null,
                    "data-delay": at(n, "ac-delay") || null, "data-use-actual": at(n, "ac-use-actual") === "true" ? true : null,
                    "data-label": at(n, "ac-initial-text") ? null : "load", disabled: disabled || null, required: check.required || null,
                    title: tip(n), form: owner()
                }, remoteAttrs)) + ">" + (value ? "<option" + A({ value, selected: true }) + ">" + e(initial) + "</option>" : "") + "</select>");
                if (at(n, "ac-show-value") === "true") w('<span class="ac-value" id="' + e(id) + '_value">' + e(initial) + "</span>");
                return;
            }
            const align = at(fieldOf(n), "align");
            const attrs = Object.assign({
                type, id, name, value, size: at(n, "size") || null, maxlength: at(n, "maxlength") || null,
                disabled: disabled || null, required: check.required || null, "data-align": align && align !== "left" ? align : null,
                "data-wide": at(n, "size") ? null : true, "data-mask": xn(at(n, "mask")) || null,
                inputmode: check.number ? "decimal" : check.digits ? "numeric" : null, title: tip(n), form: owner()
            }, pattern);
            if (at(n, "default-transition")) {
                const remoteAttrs = remote(n, at(n, "default-transition"));
                w("<input" + A(Object.assign(attrs, remoteAttrs, { "data-default": true })) + ">");
                return;
            }
            w("<input" + A(attrs) + ">");
        };

        const textArea = (n) => {
            w("<textarea" + A({
                name: fnm(n), id: fid(n), cols: at(n, "cols") || null, rows: at(n, "rows") || "3", "data-wide": at(n, "cols") ? null : true,
                readonly: at(n, "read-only") === "true", maxlength: at(n, "maxlength") || null, title: tip(n), form: owner()
            }) + ">" + e(sri.getFieldValueString(n)) + "</textarea>");
        };

        const password = (n) => {
            const check = validate(n.parent);
            w("<input" + A({
                type: "password", name: fnm(n), id: fid(n), size: at(n, "size") || "25", maxlength: at(n, "maxlength") || null,
                required: check.required || null, title: tip(n), form: owner()
            }) + ">");
        };

        const file = (n) => {
            w("<input" + A({
                type: "file", name: fnm(n), id: fid(n), multiple: at(n, "multiple") === "true", accept: at(n, "accept") || null,
                title: tip(n), form: owner()
            }) + ">");
        };

        const hidden = (n) => {
            const field = fieldOf(n);
            w("<input" + A({
                type: "hidden", name: fnm(n), id: fid(n), value: sri.getFieldValuePlainString(field, at(n, "default-value")), form: owner()
            }) + ">\n");
        };

        const reset_ = (n, title) => {
            w("<input" + A({ type: "reset", name: fnm(n), value: title, id: fid(n), title: tip(n), form: owner() }) + ">");
        };

        const submit = (n, title) => {
            const text = at(n, "text") ? x(at(n, "text")) : title;
            const confirmation = x(at(n, "confirmation"));
            const iconClass = at(n, "icon") || sri.getThemeIconClass(text) || "";
            const image = first(n, "image");
            let body = icon(iconClass);
            if (image) {
                const src = sri.makeUrlByType(at(image, "url"), at(image, "url-type") || "content", null, "true");
                body += "<img" + A({ src: src.url, alt: at(image, "alt") || title, width: at(image, "width") || null, height: at(image, "height") || null }) + ">";
            } else body += e(text);
            w("<button" + A({
                type: "submit", name: fnm(n), value: fnm(n), id: fid(n), "data-confirm": confirmation || null, title: tip(n),
                "data-btn": "primary", form: owner()
            }) + ">" + body + "</button>");
        };

        // ---- dates ----

        const dateDefault = (n) => (at(n, "type") === "time" ? "HH:mm" : at(n, "type") === "date" ? "yyyy-MM-dd" : "yyyy-MM-dd HH:mm");
        // The native input type for a widget, or null when its format is not the default one.
        const nativeType = (n) => {
            if (at(n, "format") && at(n, "format") !== dateDefault(n)) return null;
            return at(n, "type") === "time" ? "time" : at(n, "type") === "date" ? "date" : "datetime-local";
        };
        const nativeValue = (type, value) => (type === "datetime-local" && value ? value.replace(" ", "T") : value);
        const dateInput = (n, name, id, value, check) => {
            const kind = nativeType(n);
            return "<input" + A({
                type: kind || "text", name, id, value: nativeValue(kind, value), required: check && check.required ? true : null,
                step: kind === "datetime-local" || kind === "time" ? String(Number(at(n, "minute-stepping") || 5) * 60) : null,
                placeholder: kind ? null : at(n, "format"), "data-format": kind ? null : at(n, "format"), title: tip(n), form: owner()
            }) + ">";
        };

        const dateTime = (n) => {
            const format = at(n, "format") || dateDefault(n);
            const value = sri.getFieldValueString(fieldOf(n), at(n, "default-value"), format, at(n, "type") || "date-time");
            w(dateInput(n, fnm(n), fid(n), value, validate(n.parent)));
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
            const from = formatted(ec.context.get(name + "_from") || at(n, "default-value-from"), format);
            const thru = formatted(ec.context.get(name + "_thru") || at(n, "default-value-thru"), format);
            w('<span class="find date-find"><label><span>' + e(L("From")) + "</span> " + dateInput(n, name + "_from", id + "_from", from) +
                "</label> <label><span>" + e(L("Thru")) + "</span> " + dateInput(n, name + "_thru", id + "_thru", thru) + "</label></span>");
        };

        const datePeriod = (n) => {
            const name = fnm(n);
            const id = fid(n);
            const offset = String(ec.context.get(name + "_poffset") === null || ec.context.get(name + "_poffset") === undefined ? "" : ec.context.get(name + "_poffset"));
            const period = String(ec.context.get(name + "_period") || "").toLowerCase();
            const date = String(ec.context.get(name + "_pdate") || "");
            const empty = at(n, "allow-empty") !== "false" ? "<option value=\"\"></option>" : "";
            const option = (value, text, current) => "<option" + A({ value, selected: current === value ? true : null }) + ">" + e(text) + "</option>";
            const common = { title: tip(n), form: owner() };
            const offsets = [["0", L("This")], ["-1", L("Last")], ["1", L("Next")], ["-2", "-2"], ["2", "+2"], ["-3", "-3"], ["-4", "-4"], ["-6", "-6"], ["-12", "-12"]];
            const periods = [["day", L("Day")], ["7d", "7 " + L("Days")], ["30d", "30 " + L("Days")], ["week", L("Week")], ["weeks", L("Weeks")],
                ["month", L("Month")], ["months", L("Months")], ["quarter", L("Quarter")], ["year", L("Year")], ["7r", "+/-7d"], ["30r", "+/-30d"]];
            w('<span class="find date-period"><select' + A(Object.assign({ name: name + "_poffset", id: id + "_poffset" }, common)) + ">" + empty +
                offsets.map((item) => option(item[0], item[1], offset)).join("") + "</select><select" +
                A(Object.assign({ name: name + "_period", id: id + "_period" }, common)) + ">" + empty +
                periods.map((item) => option(item[0], item[1], period)).join("") + "</select><input" +
                A({ type: "date", name: name + "_pdate", id: id + "_pdate", value: date, form: owner() }) + "></span>");
        };

        const rangeFind = (n) => {
            const name = fnm(n);
            const id = fid(n);
            const common = { type: "text", size: at(n, "size") || "10", maxlength: at(n, "maxlength") || null, title: tip(n), form: owner() };
            const from = ec.context.get(name + "_from") || at(n, "default-value-from");
            const thru = ec.context.get(name + "_thru") || at(n, "default-value-thru");
            w('<span class="find range-find"><label><span>' + e(L("From")) + "</span> <input" + A(Object.assign({ name: name + "_from", id: id + "_from", value: from }, common)) +
                "></label> <label><span>" + e(L("Thru")) + "</span> <input" + A(Object.assign({ name: name + "_thru", id: id + "_thru", value: thru }, common)) + "></label></span>");
        };

        const textFind = (n) => {
            const name = fnm(n);
            const operator = at(n, "default-operator") || "contains";
            const hide = at(n, "hide-options");
            let out = '<span class="find text-find">';
            if (hide === "true" || hide === "operator") out += "<input" + A({ type: "hidden", name: name + "_op", value: operator, form: owner() }) + ">";
            else {
                out += '<label class="inline"><input' + A({ type: "checkbox", name: name + "_not", value: "Y", checked: param(name + "_not") === "Y" ? true : null, form: owner() }) + "> " + e(L("Not")) + "</label>";
                out += "<select" + A({ name: name + "_op", form: owner() }) + ">" + [["equals", "Equals"], ["like", "Like"], ["contains", "Contains"], ["begins", "Begins With"], ["empty", "Empty"]]
                    .map((item) => "<option" + A({ value: item[0], selected: operator === item[0] ? true : null }) + ">" + e(L(item[1])) + "</option>").join("") + "</select>";
            }
            out += "<input" + A({ type: "text", name, value: sri.getFieldValueString(n), size: at(n, "size") || "30", maxlength: at(n, "maxlength") || null, id: fid(n), title: tip(n), form: owner() }) + ">";
            const ignoreCase = param(name + "_ic") === "Y" || !at(n, "ignore-case") || at(n, "ignore-case") === "true";
            if (hide === "true" || hide === "ignore-case") out += "<input" + A({ type: "hidden", name: name + "_ic", value: "Y", form: owner() }) + ">";
            else out += '<label class="inline"><input' + A({ type: "checkbox", name: name + "_ic", value: "Y", checked: ignoreCase ? true : null, form: owner() }) + "> " + e(L("Ignore Case")) + "</label>";
            w(out + "</span>");
        };

        // ---- options ----

        const dropDown = (n) => {
            const id = fid(n);
            const name = fnm(n);
            const multiple = xn(at(n, "allow-multiple")) === "true";
            const dyn = first(n, "dynamic-options");
            const serverSearch = !!dyn && at(dyn, "server-search") === "true";
            const combo = at(n, "combo-box") === "true";
            const options = sri.getFieldOptions(n);
            let current = sri.getFieldValuePlainString(fieldOf(n), "");
            if (!current) current = xn(at(n, "no-current-selected-key"));
            if (current.indexOf("[") === 0) current = current.substring(1, current.length - 1).replace(/ /g, "");
            const currentList = current.split(",");
            if (multiple) current = "";
            else current = currentList[0];
            let description = options.get(current);
            const optionsHasCurrent = !!description;
            if (!optionsHasCurrent && at(n, "current-description")) description = x(at(n, "current-description"));
            else if (!optionsHasCurrent && dyn && current && !multiple) {
                description = sri.getFieldTransitionValue(at(dyn, "transition"), dyn, current, at(dyn, "label-field") || "label", false);
            }
            const check = validate(n.parent);
            const enhance = dyn || multiple || combo;
            if (enhance) lib("tom-select");
            let attrs = {
                name, id, class: xn(at(n, "style")) || null, multiple: multiple || null, size: at(n, "size") || null, required: check.required || null,
                title: tip(n), form: owner(), "data-ts": enhance ? (dyn ? "remote" : "select") : null, "data-combo": combo ? true : null,
                "data-search": serverSearch ? true : null
            };
            if (dyn) {
                attrs = Object.assign(attrs, remote(dyn, at(dyn, "transition")), {
                    "data-min": serverSearch ? at(dyn, "min-length") || "1" : null, "data-delay": at(dyn, "delay") || null,
                    "data-label": at(dyn, "label-field") || null, "data-value": at(dyn, "value-field") || null,
                    "data-empty": xn(at(n, "allow-empty")) === "true" ? true : null
                });
            }
            let out = "<select" + A(attrs) + ">";
            const option = (value, text, selected) => "<option" + A({ value, selected: selected ? true : null }) + ">" + e(text) + "</option>";
            if (!multiple) {
                if (current) {
                    if (at(n, "current") === "first-in-list") out += option(current, description || current, true) + option(current, "---", false);
                    else if (!optionsHasCurrent) out += option(current, description || current, true);
                }
                if (xn(at(n, "allow-empty")) === "true" || options.size === 0) out += '<option value=""></option>';
            }
            options.forEach((text, key) => {
                const selected = multiple && currentList.length ? currentList.indexOf(key) >= 0 : current !== "" && current === key;
                out += option(key, text, selected);
            });
            out += "</select>";
            if (xn(at(n, "show-not")) === "true") {
                out += '<label class="inline"><input' + A({ type: "checkbox", name: name + "_not", value: "Y", checked: param(name + "_not") === "Y" ? true : null, form: owner() }) + "> " + e(L("Not")) + "</label>";
            }
            if (multiple) out += "<input" + A({ type: "hidden", id: id + "_op", name: name + "_op", value: "in" }) + ">";
            w(out);
        };

        const checkOrRadio = (n, kind) => {
            const options = sri.getFieldOptions(n);
            let current = sri.getFieldValueString(n);
            if (!current) current = xn(at(n, "no-current-selected-key"));
            const id = fid(n);
            const name = fnm(n);
            const style = xn(at(n, "container-style"));
            const allChecked = xn(at(n, "all-checked"));
            let index = 0;
            options.forEach((text, key) => {
                const fullId = index > 0 ? id + "_" + index : id;
                index++;
                const checked = kind === "checkbox" ?
                    allChecked === "true" || (current !== "" && (current === key || current.indexOf(key) >= 0)) :
                    current !== "" && current === key;
                w("<span" + A({ id: fullId, class: kind === "checkbox" && style ? style : null }) + "><label><input" +
                    A({ type: kind, name, value: key, checked: checked || null, title: tip(n), form: owner() }) + ">" +
                    (text ? " <span>" + e(text) + "</span>" : "") + "</label></span>\n");
            });
        };

        // ---- display ----

        const currencyText = (value, unit, hideSymbol) =>
            hideSymbol ? ec.l10n.formatCurrencyNoSymbol(value, unit) : ec.l10n.formatCurrency(value, unit);

        const display = (n) => {
            const field = fieldOf(n);
            const id = fid(n);
            const alsoHidden = (!at(n, "also-hidden") || at(n, "also-hidden") === "true") && !st.skipForm;
            const dyn = !!at(n, "dynamic-transition");
            const unit = at(n, "currency-unit-field");
            const hideSymbol = at(n, "currency-hide-symbol") === "true";
            let value = "";
            if (at(n, "text")) {
                const map = at(n, "text-map") ? ec.resource.expression(at(n, "text-map"), "") : null;
                value = map ? ec.resource.expand(at(n, "text"), "", map) : ec.resource.expand(at(n, "text"), "");
                if (unit) value = currencyText(value, ec.resource.expression(unit, ""), hideSymbol);
            } else if (unit) {
                value = currencyText(sri.getFieldValue(field, ""), ec.resource.expression(unit, ""), hideSymbol);
            } else value = sri.getFieldValueString(n);
            const body = value ? (at(n, "encode") === "false" ? value : e(value).replace(/\n/g, "<br>")) : "";
            let out = "<span" + A({
                class: cls("display", unit ? "currency" : null, xn(at(n, "style"))), title: tip(n), id: dyn ? id + "_display" : null
            }) + ">" + body + "</span>";
            if (alsoHidden) {
                out += "<input" + A({ type: "hidden", id, name: fnm(n), value: dyn ? "" : sri.getFieldValuePlainString(field, ""), form: owner() }) + ">";
            }
            if (dyn) {
                const remoteAttrs = remote(n, at(n, "dynamic-transition"));
                out += "<span" + A(Object.assign({ hidden: true, "data-dynamic": id, "data-hidden": alsoHidden ? true : null }, remoteAttrs)) + "></span>";
            }
            w(out);
        };

        const displayEntity = (n) => {
            const value = sri.getFieldEntityValue(n) || "";
            const alsoHidden = (!at(n, "also-hidden") || at(n, "also-hidden") === "true") && !st.skipForm;
            const body = value ? (at(n, "encode") === "false" ? value : e(value).replace(/\n/g, "<br>")) : "";
            w("<span" + A({ class: cls("display", xn(at(n, "style"))) }) + ">" + body + "</span>" +
                (alsoHidden ? "<input" + A({ type: "hidden", id: fid(n), name: fnm(n), value: sri.getFieldValuePlainString(fieldOf(n), ""), form: owner() }) + ">" : ""));
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
            w(at(n, "hover") === "true" ? '<span class="hover-image">' + tag + tag.replace("<img", '<img class="hover"') + "</span>" : tag);
        };

        const label = (n) => {
            if (at(n, "condition") && !cond(at(n, "condition"))) return;
            const type = at(n, "type") || "span";
            const map = at(n, "text-map") ? ec.resource.expression(at(n, "text-map"), "") : null;
            let text = map ? ec.resource.expand(at(n, "text"), "", map) : ec.resource.expand(at(n, "text"), "");
            if (!(String(text).trim() || at(n, "condition"))) return;
            if (at(n, "encode") !== "false") {
                text = e(text);
                if (type !== "code" && type !== "pre") text = text.replace(/\n/g, "<br>");
            }
            w("<" + type + A({ id: nodeId(n) || null, class: xn(at(n, "style")) || null, title: at(n, "tooltip") ? x(at(n, "tooltip")) : null }) + ">" + text + "</" + type + ">\n");
        };

        // The link text and URL of a link element, or null when the link shows nothing.
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
            if (at(n, "encode") !== "false") text = e(text);
            let url = sri.makeUrlByType(at(n, "url"), at(n, "url-type") || "transition", n, at(n, "expand-transition-url") || "true");
            if (at(n, "pass-through-parameters") === "true") url = url.addParameters(sri.getScreenUrlInstance().getPassThroughParameterMap());
            return { text, url, img, anchor: sri.isAnchorLink(n, url) };
        };

        const linkContent = (n, text, img) => {
            let iconClass = at(n, "icon");
            if (!iconClass && at(n, "text")) iconClass = sri.getThemeIconClass(at(n, "text")) || "";
            iconClass = xn(iconClass);
            const badge = x(at(n, "badge"));
            return icon(iconClass) + (img ? imageTag(img) : text) + (badge ? ' <span class="badge">' + e(badge) + "</span>" : "");
        };

        const btnAttr = (n) => (at(n, "link-type") === "anchor" ? null : at(n, "link-type") === "hidden-form-link" ? "plain" : at(n, "btn-type") || "primary");

        // The element that follows the link: an anchor, or for a transition with actions a button for its form.
        const linkElement = (n, parts, formId) => {
            const body = linkContent(n, parts.text, parts.img);
            const tooltip = at(n, "tooltip") ? x(at(n, "tooltip")) : null;
            const confirmation = x(at(n, "confirmation"));
            const style = xn(at(n, "style")) || null;
            if (parts.url.getDisableLink()) {
                return "<a" + A({ href: "#", id: formId || null, class: cls("disabled", style), "data-btn": btnAttr(n), "aria-disabled": "true" }) + ">" + body + "</a>";
            }
            if (parts.anchor) {
                let href = at(n, "url-noparam") === "true" ? parts.url.url : parts.url.getUrlWithParams();
                const load = at(n, "dynamic-load-id");
                return "<a" + A({
                    href: load ? parts.url.getUrlWithParams() : href, id: formId || null, target: at(n, "target-window") || null,
                    "data-confirm": confirmation || null, class: style, "data-btn": btnAttr(n), title: tooltip, "data-load-into": load || null
                }) + ">" + body + "</a>";
            }
            if (!formId) return "";
            return "<button" + A({
                type: "submit", form: formId, id: formId + "_button", "data-btn": at(n, "btn-type") || "primary", class: style,
                "data-confirm": confirmation || null, title: tooltip
            }) + ">" + body + "</button>";
        };

        // The form a transition link posts. With a form id it holds only the hidden fields and a button elsewhere points to it.
        const linkForm = (n, parts, formId) => {
            if (parts.url.getDisableLink() || parts.anchor) return "";
            const params = parts.url.getParameterMap();
            let out = "<form" + A({ method: "post", action: parts.url.url, name: formId || null, id: formId || null, target: at(n, "target-window") || null }) + ">" +
                "<input" + A({ type: "hidden", name: "moquiSessionToken", value: token() }) + ">";
            Object.keys(params).forEach((key) => { out += "<input" + A({ type: "hidden", name: key, value: params[key] === null || params[key] === undefined ? "" : params[key] }) + ">"; });
            if (!formId) {
                const confirmation = x(at(n, "confirmation"));
                const img = first(n, "image");
                if (img) {
                    out += "<input" + A({ type: "image", src: sri.makeUrlByType(at(img, "url"), at(img, "url-type") || "content", null, "true").url, alt: at(img, "alt") || null, "data-confirm": confirmation || null }) + ">";
                } else {
                    out += "<button" + A({
                        type: "submit", "data-btn": btnAttr(n) || "plain", class: xn(at(n, "style")) || null, "data-confirm": confirmation || null,
                        title: at(n, "tooltip") ? x(at(n, "tooltip")) : null
                    }) + ">" + linkContent(n, parts.text, null) + "</button>";
                }
            }
            return out + "</form>";
        };

        const link = (n, inField) => {
            const parts = linkParts(n);
            if (!parts) {
                if (inField) w("&nbsp;");
                return;
            }
            if (inField) {
                const id = fid(n) + "_" + at(n, "url").replace(/\./g, "_");
                const form = linkForm(n, parts, id);
                if (form) sri.appendToAfterScreenWriter(form);
                w(linkElement(n, parts, id));
                return;
            }
            const id = nodeId(n);
            const form = linkForm(n, parts, id);
            w(form + linkElement(n, parts, id) + "\n");
        };

        const editable = (n) => {
            const url = sri.makeUrlByType(at(n, "transition"), "transition", n, "true");
            const text = ec.resource.expand(at(n, "text"), "");
            if (!String(text).trim()) return;
            const load = first(n, "editable-load");
            let loadUrl = null;
            if (load) loadUrl = sri.makeUrlByType(at(load, "transition"), "transition", load, "true");
            const type = at(n, "type") || "span";
            w("<" + type + A({
                id: nodeId(n) || null, class: "editable", "data-url": url.url, "data-params": json(url.getParameterMap()), "data-name": at(n, "parameter-name") || "value",
                "data-widget": at(n, "widget-type") || "textarea", "data-load-url": loadUrl ? loadUrl.url : null,
                "data-load-params": loadUrl ? json(loadUrl.getParameterMap()) : null, title: L("Click to edit")
            }) + ">" + (at(n, "encode") === "true" ? e(text).replace(/\n/g, "<br>") : text) + "</" + type + ">");
        };

        // ---- containers ----

        const dialogOpen = (n, id, text, extra) => {
            const iconClass = at(n, "icon") || "fa fa-share";
            return "<button" + A({
                type: "button", id: id + "-button", "data-dialog": id, "data-btn": xn(at(n, "type")) || "primary", class: xn(at(n, "button-style")) || null, title: text, "aria-label": text
            }) + ">" + icon(iconClass) + e(text) + "</button>";
        };
        const dialogShell = (n, id, text, bodyAttrs, body, extra) =>
            "<dialog" + A(Object.assign({ id, class: extra, style: "--w:" + (at(n, "width") || "760") + "px" }, bodyAttrs.dialog || {})) + "><header><h4>" + e(text) +
            '</h4><button type="button" class="close" data-close aria-label="Close">&times;</button></header><div' + A(Object.assign({ class: "body" }, bodyAttrs.body || {})) + ">" + body;

        const containerDialog = (n) => {
            if (at(n, "condition") && !cond(at(n, "condition"))) return false;
            const text = x(at(n, "button-text"));
            const id = nodeId(n);
            w(dialogOpen(n, id, text));
            w(dialogShell(n, id, text, { dialog: { "data-open": ec.context.get("_openDialog") === id ? true : null } }, "", "container-dialog") + "\n");
            return true;
        };

        const dynamicDialog = (n) => {
            if (at(n, "condition") && !cond(at(n, "condition"))) return;
            const text = x(at(n, "button-text"));
            const id = nodeId(n);
            const url = sri.makeUrlByType(at(n, "transition"), "transition", n, "true");
            w(dialogOpen(n, id, text));
            sri.appendToAfterScreenWriter(dialogShell(n, id, text, {
                dialog: { "data-load": url.getUrlWithParams(), "data-open": ec.context.get("_openDialog") === id ? true : null }
            }, '<span class="loading">' + e(L("Loading")) + "...</span>", "dynamic-dialog") + "</div></dialog>\n");
        };

        const dynamicContainer = (n) => {
            const id = nodeId(n);
            const url = sri.makeUrlByType(at(n, "transition"), "transition", n, "true").addParameter("_dynamic_container_id", id);
            w("<div" + A({ id, "data-load": url.getUrlWithParams() }) + '><span class="loading">' + e(L("Loading")) + "...</span></div>\n");
        };

        const tree = (n) => {
            let url;
            let items;
            if (at(n, "transition")) {
                url = sri.makeUrlByType(at(n, "transition"), "transition", n, "true");
                items = url.url;
            } else {
                url = sri.makeUrlByType("actions", "transition", n, "true");
                items = url.url + "/" + at(n, "name");
            }
            const params = Object.assign({}, url.getParameterMap());
            if (at(n, "open-path")) params.treeOpenPath = xn(at(n, "open-path"));
            w("<div" + A({ id: at(n, "name"), class: "tree", "data-url": items, "data-params": json(params) }) + "></div>\n");
        };

        const menuLink = (item) => {
            const title = ec.resource.expand(item.title, "");
            let img = '<i class="fa fa-link"></i> ';
            if (item.image) {
                if (item.imageType === "icon") img = '<i class="' + e(item.image) + '"></i> ';
                else if (item.imageType === "url-plain") img = '<img src="' + e(item.image) + '" alt="' + e(title) + '" width="18"> ';
                else img = '<img src="' + e(sri.buildUrl(item.image).url) + '" alt="' + e(title) + '" height="18"> ';
            }
            return { title, html: "<a" + A({ href: item.disabled ? "#" : item.url }) + ">" + img + e(title) + "</a>" };
        };

        const popupMenu = (n, moveTo) => {
            const menuId = at(n, "id") || "subscreensMenu";
            const title = ec.resource.expand(at(n, "title") || sri.getActiveScreenDef().getDefaultMenuName() || "Menu", "");
            let out = "<details" + A({ id: menuId, class: "menu", "data-move-to": at(n, "header-menus-id") || moveTo }) + "><summary>" + e(title) + " <i class=\"fa fa-chevron-right\"></i></summary><ul>";
            sri.getMenuItems().forEach((item) => {
                out += "<li" + A({ class: item.inPath ? "active" : null }) + ">" + menuLink(item).html + "</li>";
            });
            w(out + "</ul></details>\n");
        };

        const crumb = (n, menuId) => {
            const title = ec.resource.expand(at(n, "title") || sri.getActiveScreenDef().getDefaultMenuName() || "Menu", "");
            w("<a" + A({ id: menuId + "-crumb", class: "crumb", href: sri.buildUrl(".").url, "data-move-to": "navbar-menu-crumbs" }) + ">" + e(title) + ' <i class="fa fa-chevron-right"></i></a>\n');
        };

        const subscreensMenu = (n) => {
            if (ec.context.get("hideNav") === "true") return;
            const type = at(n, "type");
            if (type === "popup") return popupMenu(n, "header-menus");
            if (type === "popup-tree") return;
            if (sri.activeInCurrentMenu) {
                const items = sri.getMenuItems();
                let out = "<nav" + A({ id: at(n, "id") || null, class: "tabs" }) + "><ul>";
                items.forEach((item) => {
                    const title = ec.resource.expand(item.title, "");
                    out += "<li" + A({ class: item.disabled ? "disabled" : item.inPath ? "active" : null }) + ">" +
                        (item.disabled ? e(title) : '<a href="' + e(item.url) + '">' + e(L(item.title)) + "</a>") + "</li>";
                });
                w(out + "</ul></nav>\n");
            }
            crumb(n, at(n, "id") || "subscreensMenu");
        };

        const subscreensPanel = (n) => {
            const isDynamic = at(n, "dynamic") === "true" && !!at(n, "id");
            const hideNav = ec.context.get("hideNav") === "true";
            const menuId = at(n, "id") ? at(n, "id") + "-menu" : "subscreensPanelMenu";
            const type = at(n, "type");
            if (type === "popup") {
                if (!hideNav) popupMenu(n, "header-menus");
                sri.renderSubscreen();
                return;
            }
            if (type === "stack" || type === "wizard") {
                w("<p>" + e(type) + " type subscreens-panel is not supported.</p>");
                return;
            }
            const items = sri.getMenuItems();
            const displayMenu = sri.activeInCurrentMenu && !hideNav;
            w("<div" + A({ id: at(n, "id") ? at(n, "id") + "-tabpanel" : null, class: "tabpanel" }) + ">");
            if (items.length > 1 && displayMenu) {
                w("<nav" + A({ id: at(n, "id") ? menuId : null, class: "tabs", "data-dynamic": isDynamic ? at(n, "id") + "-active" : null }) + "><ul>");
                items.forEach((item) => {
                    const title = ec.resource.expand(item.title, "");
                    const url = isDynamic ? sri.buildUrl(item.name).addParameter("lastStandalone", "true").getUrlWithParams() : null;
                    w("<li" + A({ class: item.disabled ? "disabled" : item.inPath ? "active" : null }) + "><a" + A({ href: item.disabled ? "#" : item.url, "data-load-url": isDynamic ? url : null }) + ">" + e(title) + "</a></li>");
                });
                w("</ul></nav>");
            }
            if (!hideNav) crumb(n, menuId);
            w(isDynamic ? '<div id="' + e(at(n, "id")) + '-active">' : "");
            sri.renderSubscreen();
            w(isDynamic ? "</div>" : "");
            w("</div>\n");
        };

        // ---- forms ----

        const hiddenInput = (name, value, form) => "<input" + A({ type: "hidden", name, value, form: form || null }) + ">";

        const formOpen = (n, upload) => {
            st.formName = at(n, "name");
            // Read again for each form: the runtime outlives a render.
            st.validationErrors = undefined;
            st.owner = at(n, "owner-form");
            st.skipForm = false;
            const skipStart = at(n, "skip-start") === "true" || !!st.owner;
            const url = sri.makeUrlByType(at(n, "transition"), "transition", null, "true");
            const formId = xn(at(n, "name")) + sec();
            st.fieldForm = formId;
            let out = "";
            if (!skipStart) {
                out += "<form" + A({
                    name: formId, id: formId, method: "post", action: url.url, enctype: upload ? "multipart/form-data" : null,
                    "data-bg": at(n, "background-submit") === "true" ? true : null, "data-reload": at(n, "background-reload-id") || null,
                    "data-msg": at(n, "background-message") || null, "data-hide": at(n, "background-hide-id") || null,
                    "data-focus": at(n, "focus-field") || null
                }) + ">" + hiddenInput("moquiFormName", at(n, "name")) + hiddenInput("moquiSessionToken", token());
                const stamp = sri.getNamedValuePlain("lastUpdatedStamp", n);
                if (stamp) out += hiddenInput("lastUpdatedStamp", stamp);
            }
            if (at(n, "pass-through-parameters") === "true") {
                const params = sri.getScreenUrlInstance().getPassThroughParameterMap();
                Object.keys(params).forEach((key) => {
                    if (!kids(n, "field").some((field) => at(field, "name") === key)) out += hiddenInput(key, params[key] === null || params[key] === undefined ? "" : params[key]);
                });
            }
            out += "<fieldset" + A({ disabled: url.getDisableLink() ? true : null }) + ">";
            w(out);
        };

        const formClose = (n) => {
            w("</fieldset>" + (at(n, "skip-end") === "true" || at(n, "owner-form") ? "" : "</form>") + "\n");
            reset();
        };

        // The validation error the last submit left for a field of this form, as Java's form macros show it.
        const fieldError = (name) => {
            if (st.validationErrors === undefined) st.validationErrors = (ec.web && ec.web.savedValidationErrors) || [];
            const found = st.validationErrors.find((v) => v.field === name && (!v.form || v.form === st.formName));
            return found ? found.message : null;
        };
        // A form-single field: the label and a box for the widgets. Returns what to close.
        const fieldOpen = (name, title, isSubmit, containerStyle, big) => {
            st.fieldForm = st.fieldForm || xn(st.formName);
            const forId = st.fieldForm + "_" + name + entry() + sec();
            const style = cls(containerStyle || null);
            const lc = st.labelCols ? " style=\"--lc:" + e(st.labelCols) + '"' : "";
            const error = isSubmit ? null : fieldError(name);
            const invalid = error ? " data-invalid" : "";
            if (big) {
                w('<div class="big-item"><div class="field"' + invalid + lc + ">" + (title && !isSubmit ? '<label for="' + e(forId) + '">' + e(title) + "</label>" : "") + "<div" + A({ class: cls("widget", style) }) + ">");
                return { error };
            }
            if (isSubmit) {
                w('<div class="field" data-submit' + lc + '><span class="label"></span><div' + A({ class: cls("widget", style) }) + ">");
                return { error };
            }
            if (title) {
                w('<div class="field"' + invalid + lc + '><label for="' + e(forId) + '">' + e(title) + "</label><div" + A({ class: cls("widget", style) }) + ">");
                return { error };
            }
            if (error) w('<div class="field-error" role="alert">' + e(error) + "</div>");
            return 0;
        };
        const fieldClose = (opened, big) => {
            if (!opened) return;
            if (opened.error) w('<div class="field-error" role="alert">' + e(opened.error) + "</div>");
            w(big ? "</div></div></div>\n" : "</div></div>\n");
        };

        const rowOpen = (kind, style) => w("<div" + A({ class: cls(kind, xn(style) || null) }) + ">");
        const colOpen = (n) => w("<div" + A({
            class: cls("col", xn(at(n, "style")) || null), style: ["lg", "md", "sm", "xs"].filter((k) => at(n, k)).map((k) => "--" + k + ":" + at(n, k)).concat(at(n, "label-cols") ? ["--lc:" + at(n, "label-cols")] : []).join(";") || null
        }) + ">");

        const fieldGroup = (n) => {
            const title = L(at(n, "title") || "") || "";
            if (st.accordion) {
                st.accIndex++;
                const active = String(st.accIndex) === st.accActive;
                w("<details" + A({ class: "panel", name: st.accordion, open: active ? true : null }) + "><summary>" + e(title || "Fields") + '</summary><div class="body' + (at(n, "style") ? " " + e(at(n, "style")) : "") + '">');
            } else if (at(n, "box") === "true") {
                w('<section class="box" data-type="default"><header><h5>' + e(title || "Fields") + '</h5></header><div class="body' + (at(n, "style") ? " " + e(at(n, "style")) : "") + '">');
            } else {
                w('<div class="field-group' + (at(n, "style") ? " " + e(at(n, "style")) : "") + '">' + (title ? "<h5>" + e(title) + "</h5>" : ""));
            }
        };
        const fieldGroupClose = (n) => {
            w(st.accordion ? "</div></details>\n" : at(n, "box") === "true" ? "</div></section>\n" : "</div>\n");
        };

        // ---- form-list ----

        const pageUrl = (name, value) => sri.getScreenUrlInstance().cloneUrlInstance().addParameter(name, value).getUrlWithParams();

        const nav = (n, cols, cfg) => {
            const listName = at(n, "list");
            const formId = cfg.formId;
            const countValue = ec.context.get(listName + "Count");
            const maxIndex = ec.context.get(listName + "PageMaxIndex");
            const paginated = at(n, "paginate") !== "false" && countValue !== null && countValue !== undefined && countValue > 0 &&
                (!at(n, "paginate-always-show") || at(n, "paginate-always-show") === "true" || maxIndex > 0);
            const savedFinds = cfg.savedFinds;
            const selectColumns = cfg.selectColumns;
            if (!(cfg.headerDialog || savedFinds || selectColumns || paginated) || ec.context.get("hideNav") === "true") return;
            let out = '<tr class="nav-row"><th colspan="' + Math.max(cols.length, 1) + '"><nav>';
            if (cfg.headerDialog || savedFinds) {
                out += "<button" + A({ type: "button", "data-dialog": formId + "_hdialog", title: L("Find Options") }) + '><i class="fa fa-share"></i> ' + e(L("Find Options")) + "</button>";
            }
            if (selectColumns) {
                out += "<button" + A({ type: "button", "data-dialog": formId + "_SelColsDialog", title: L("Columns") }) + '><i class="fa fa-share"></i> ' + e(L("Columns")) + "</button>";
            }
            if (paginated) {
                const index = ec.context.get(listName + "PageIndex");
                const lowLink = Math.max(index - 3, 0);
                const highLink = Math.min(index + 3, maxIndex);
                out += '<ul class="pagination">';
                const arrow = (target, icon_, on) => (on ? '<li><a href="' + e(pageUrl("pageIndex", target)) + '"><i class="fa ' + icon_ + '"></i></a></li>' : '<li><span><i class="fa ' + icon_ + '"></i></span></li>');
                out += arrow(0, "fa-fast-backward", index > 0) + arrow(index - 1, "fa-backward", index > 0);
                for (let i = lowLink; i < index; i++) out += '<li><a href="' + e(pageUrl("pageIndex", i)) + '">' + (i + 1) + "</a></li>";
                const pageText = ec.resource.expand(L("PaginationTemplate"), "", { curPageIndex: index, curPageMaxIndex: maxIndex, listName });
                out += '<li class="current"><a href="' + e(sri.getScreenUrlInstance().getUrlWithParams()) + '">' + e(pageText) + "</a></li>";
                for (let i = index + 1; i <= highLink; i++) out += '<li><a href="' + e(pageUrl("pageIndex", i)) + '">' + (i + 1) + "</a></li>";
                out += arrow(index + 1, "fa-forward", index < maxIndex) + arrow(maxIndex, "fa-fast-forward", index < maxIndex) + "</ul>";
                if (maxIndex > 4) {
                    const go = sri.getScreenUrlInstance().cloneUrlInstance().removeParameter("pageIndex").removeParameter("moquiFormName").removeParameter("moquiSessionToken");
                    const goParams = go.getParameterMap();
                    out += "<form" + A({ id: formId + "_GoPage", method: "post", action: go.url, "data-gopage": maxIndex + 1 }) + ">" +
                        Object.keys(goParams).map((key) => hiddenInput(key, goParams[key])).join("") +
                        "<input" + A({ type: "number", name: "pageIndex", id: formId + "_GoPage_pageIndex", min: 1, max: maxIndex + 1, required: true, placeholder: L("Page #"), "aria-label": "Page Number" }) + ">" +
                        "<button type=\"submit\">" + e(L("Go##Page").replace(/##.*$/, "")) + "</button></form>";
                }
                if (at(n, "show-all-button") === "true" && countValue < 500) {
                    if (ec.context.get("pageNoLimit")) out += '<a data-btn="default" href="' + e(sri.getScreenUrlInstance().cloneUrlInstance().removeParameter("pageNoLimit").getUrlWithParams()) + '">' + e(L("Paginate")) + "</a>";
                    else out += '<a data-btn="default" href="' + e(pageUrl("pageNoLimit", "true")) + '">' + e(L("Show All")) + "</a>";
                }
            }
            if (at(n, "show-csv-button") === "true") {
                const csv = sri.getScreenUrlInstance().cloneUrlInstance().addParameter("renderMode", "csv").addParameter("pageNoLimit", "true").addParameter("lastStandalone", "true").addParameter("saveFilename", at(n, "name") + ".csv");
                out += '<a data-btn="default" href="' + e(csv.getUrlWithParams()) + '">' + e(L("CSV")) + "</a>";
            }
            if (at(n, "show-xlsx-button") === "true" && ec.screen.isRenderModeValid("xlsx")) {
                const xlsx = sri.getScreenUrlInstance().cloneUrlInstance().addParameter("renderMode", "xlsx").addParameter("pageNoLimit", "true").addParameter("lastStandalone", "true").addParameter("saveFilename", at(n, "name") + ".xlsx");
                out += '<a data-btn="default" href="' + e(xlsx.getUrlWithParams()) + '">' + e(L("XLS")) + "</a>";
            }
            if (at(n, "show-text-button") === "true") {
                out += "<button" + A({ type: "button", "data-dialog": formId + "_TextDialog", title: L("Text") }) + '><i class="fa fa-share"></i> ' + e(L("Text")) + "</button>";
            }
            if (at(n, "show-pdf-button") === "true") {
                const print = sri.getScreenUrlInstance().cloneUrlInstance().addParameter("renderMode", "html-print").addParameter("pageNoLimit", "true");
                out += '<a data-btn="default" target="_blank" href="' + e(print.getUrlWithParams()) + '">' + e(L("Print")) + "</a>";
            }
            w(out + "</nav></th></tr>\n");
        };

        const hiddenParams = (n) => sri.getFormHiddenParameters(n);

        const findDialog = (n, cols, cfg) => {
            const formId = cfg.formId;
            const savedFinds = cfg.savedFinds;
            const hiddenMap = hiddenParams(n);
            let out = '<dialog id="' + e(formId) + '_hdialog" style="--w:800px"><header><h4>' + e(L("Find Options")) + '</h4><button type="button" class="close" data-close aria-label="Close">&times;</button></header><div class="body">';
            w(out);
            if (savedFinds) {
                const current = sri.getScreenUrlInstance().cloneUrlInstance().removeParameter("pageIndex").removeParameter("moquiFormName").removeParameter("moquiSessionToken").removeParameter("formListFindId");
                const params = current.getParameterMap();
                const saveUrl = sri.buildUrl("formSaveFind").url;
                w("<h4>" + e(L("Saved Finds")) + "</h4>");
                // The user's saved finds: a link that applies one, and a form that updates it to the current parameters,
                // makes it the default of the screen or no longer, or deletes it.
                const finds = sri.getUserFormListFinds(at(n, "name"));
                if (finds.length) {
                    const activeId = ec.context.get("formListFindId") || "";
                    const defaultId = sri.getUserDefaultFormListFindId() || "";
                    const base = sri.getScreenUrlInstance().url.split("?")[0];
                    w('<ul class="saved-finds">');
                    finds.forEach((find) => {
                        const id = find.formListFind.formListFindId;
                        const label = (find.description || id) + (id === defaultId ? " (" + L("Default") + ")" : "");
                        w("<li" + A({ "data-active": id === activeId ? "" : null }) + "><a" + A({ href: base + "?formListFindId=" + encodeURIComponent(id) }) + ">" + e(label) + "</a>" +
                            "<form" + A({ method: "post", action: saveUrl }) + ">" + hiddenInput("moquiSessionToken", token()) + hiddenInput("formLocation", at(n, "name")) +
                            hiddenInput("formListFindId", id) + Object.keys(params).map((key) => hiddenInput(key, params[key])).join("") +
                            (find.isByUserId === "true" && Object.keys(params).length ? '<button type="submit" data-btn="default">' + e(L("Update to Current")) + "</button>" : "") +
                            (id === defaultId ? '<button type="submit" name="ClearDefault" value="Y" data-btn="default">' + e(L("Clear Default")) + "</button>"
                                : '<button type="submit" name="MakeDefault" value="Y" data-btn="default">' + e(L("Make Default")) + "</button>") +
                            (find.isByUserId === "true" ? '<button type="submit" name="DeleteFind" value="Y" data-btn="danger" data-confirm="' + e(L("Delete") + " " + label + "?") + '">' + e(L("Delete")) + "</button>" : "") +
                            "</form></li>");
                    });
                    w("</ul>");
                }
                if (Object.keys(params).length) {
                    w("<form" + A({ id: formId + "_NewFind", method: "post", action: saveUrl }) + ">" + hiddenInput("moquiSessionToken", token()) + hiddenInput("formLocation", at(n, "name")) +
                        Object.keys(params).map((key) => hiddenInput(key, params[key])).join("") +
                        "<input" + A({ type: "text", name: "_findDescription", size: 40, placeholder: L("Description"), "aria-label": L("Description") }) + "><button type=\"submit\" data-btn=\"primary\">" + e(L("Save New Find")) + "</button></form>");
                } else w("<p>" + e(L("No find parameters, choose some to save a new find or update existing")) + "</p>");
            }
            if (cfg.headerDialog) {
                w("<h4>" + e(L("Find Parameters")) + '</h4><form name="' + e(cfg.headerFormId) + '" id="' + e(cfg.headerFormId) + '" method="post" action="' + e(sri.getCurrentScreenUrl().url) + '">' +
                    hiddenInput("moquiSessionToken", token()) + (ec.context.get("formListFindId") ? hiddenInput("formListFindId", ec.context.get("formListFindId")) : "") +
                    Object.keys(hiddenMap).map((key) => hiddenInput(key, hiddenMap[key])).join("") + "<fieldset>");
                w('<div class="field"><span class="label"></span><div class="widget"><button type="button" data-clear="' + e(cfg.headerFormId) + '">' + e(L("Clear Parameters")) + "</button></div></div>");
                const orderable = cols.flat().filter((f) => f.orderBy);
                if (orderable.length) {
                    lib("tom-select");
                    w('<div class="field"><label for="' + e(cfg.headerFormId) + '_orderBySelect">' + e(L("Order By")) + '</label><div class="widget"><select multiple id="' + e(cfg.headerFormId) + '_orderBySelect" data-ts="select" data-orderby="' + e(cfg.headerFormId) + '_orderByField">' +
                        orderable.map((f) => {
                            const ci = f.orderBy === "case-insensitive" ? "^" : "";
                            return '<option value="' + e(ci + f.name) + '">' + e(f.title()) + " " + e(L("(Asc)")) + '</option><option value="' + e("-" + ci + f.name) + '">' + e(f.title()) + " " + e(L("(Desc)")) + "</option>";
                        }).join("") + "</select>" + hiddenInput("orderByField", ec.context.get("orderByField") || "").replace("<input", '<input id="' + e(cfg.headerFormId) + '_orderByField"') + "</div></div>");
                }
                st.owner = cfg.headerFormId;
                st.fieldForm = cfg.headerFormId;
                cols.flat().forEach((f) => { if (f.find) f.dialogField(); });
                st.owner = "";
                st.fieldForm = "";
                w("</fieldset></form>");
            }
            w("</div></dialog>\n");
        };

        const columnsDialog = (n, cols, allFields, cfg) => {
            lib("sortable");
            const formId = cfg.formId;
            const inColumns = new Set(cols.flat().map((f) => f.name));
            const notIn = allFields.filter((f) => !inColumns.has(f.name));
            const item = (f) => '<li id="' + e(f.name) + '"><div>' + e(f.title()) + "</div></li>";
            let out = '<dialog id="' + e(formId) + '_SelColsDialog" style="--w:600px"><header><h4>' + e(L("Column Fields")) + '</h4><button type="button" class="close" data-close aria-label="Close">&times;</button></header><div class="body"><p>' +
                e(L("Drag fields to the desired column or do not display")) + '</p><ul id="' + e(formId) + '_SelColsSortable" data-sortable-tree="' + e(formId) + '_SelColsForm_columnsTree">' +
                '<li id="hidden"><div>' + e(L("Do Not Display")) + "</div>" + (notIn.length ? "<ul>" + notIn.map(item).join("") + "</ul>" : "<ul></ul>") + "</li>";
            cols.forEach((col, index) => { out += '<li id="column_' + index + '"><div>' + e(L("Column")) + " " + (index + 1) + "</div><ul>" + col.map(item).join("") + "</ul></li>"; });
            for (let i = cols.length; i < cols.length + 3; i++) out += '<li id="column_' + i + '"><div>' + e(L("Column")) + " " + (i + 1) + "</div><ul></ul></li>";
            out += "</ul><form" + A({ id: formId + "_SelColsForm", method: "post", action: sri.buildUrl("formSelectColumns").url }) + ">" + hiddenInput("moquiSessionToken", token()) + hiddenInput("formLocation", at(n, "name")) +
                '<input type="hidden" id="' + e(formId) + '_SelColsForm_columnsTree" name="columnsTree" value="">' +
                '<button type="submit" name="SaveColumns" data-btn="primary">' + e(L("Save Columns")) + '</button> <button type="submit" name="ResetColumns" data-btn="primary">' + e(L("Reset to Default")) + "</button></form></div></dialog>\n";
            w(out);
        };

        const textDialog = (n, cfg) => {
            const formId = cfg.formId;
            const url = sri.getScreenUrlInstance();
            const params = url.getParameterMap();
            const row = (label, input) => '<div class="field"><label for="' + e(formId) + "_Text_" + input.name + '">' + e(L(label)) + '</label><div class="widget"><input' + A(Object.assign({ id: formId + "_Text_" + input.name }, input)) + "></div></div>";
            w('<dialog id="' + e(formId) + '_TextDialog" style="--w:600px"><header><h4>' + e(L("Export Fixed-Width Plain Text")) + '</h4><button type="button" class="close" data-close aria-label="Close">&times;</button></header><div class="body"><form' +
                A({ id: formId + "_Text", method: "post", action: url.url }) + ">" + hiddenInput("renderMode", "text") + hiddenInput("pageNoLimit", "true") + hiddenInput("lastStandalone", "true") +
                Object.keys(params).map((key) => hiddenInput(key, params[key])).join("") + "<fieldset>" +
                row("Line Characters", { type: "text", name: "lineCharacters", size: 4, value: "132" }) + row("Page Lines", { type: "text", name: "pageLines", size: 4, value: "88" }) +
                row("Line Wrap?", { type: "checkbox", name: "lineWrap", value: "true" }) + row("Save to Filename", { type: "text", name: "saveFilename", size: 40, value: at(n, "name") + ".txt" }) +
                '</fieldset><button type="submit" data-btn="default">' + e(L("Export Text")) + "</button></form></div></dialog>\n");
        };

        const th = (f, cfg) => {
            let out = "<th>";
            f.forEach((field) => {
                const sub = field.headerStyle;
                const align = field.align && field.align !== "left" ? field.align : null;
                out += "<div" + A({ class: cls("title", sub), "data-align": align }) + ">" + e(field.submit ? "" : field.title());
                if (field.orderBy) {
                    const insensitive = field.orderBy === "case-insensitive";
                    let current = ec.context.get("orderByField") || "";
                    if (current && current.indexOf(",") >= 0) {
                        const hit = current.split(",").filter((part) => part && part.indexOf(field.name) >= 0);
                        if (hit.length) current = hit[0];
                    }
                    const mentioned = current && current.indexOf(field.name) >= 0;
                    const asc = mentioned && current.indexOf("-") !== 0;
                    const desc = mentioned && current.indexOf("-") === 0;
                    let ascUrl = pageUrl("orderByField", (insensitive ? "^" : "") + field.name);
                    let descUrl = pageUrl("orderByField", "-" + (insensitive ? "^" : "") + field.name);
                    if (asc) ascUrl = descUrl;
                    if (desc) descUrl = ascUrl;
                    out += '<span class="order-by"><a' + A({ href: ascUrl, class: asc ? "active" : null }) + '><i class="fa fa-caret-up"></i></a><a' + A({ href: descUrl, class: desc ? "active" : null }) + '><i class="fa fa-caret-down"></i></a></span>';
                }
                out += "</div>";
                if (field.find && !cfg.headerDialog) {
                    w(out);
                    out = "";
                    w("<div" + A({ class: cls("find-field", sub), "data-align": align }) + ">");
                    field.find();
                    w("</div>");
                }
            });
            w(out + "</th>");
        };

        // Runs a field's hidden widgets: one chain per field.
        const hiddenRows = (fields) => fields.forEach((cell) => cell());

        const formList = (info, n, cols, cfg) => {
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
            const hidden = hiddenParams(n);
            const hiddenInputs = () => Object.keys(hidden).map((key) => hiddenInput(key, hidden[key])).join("");
            const orderField = ec.context.get("orderByField");
            const orderInput = () => (orderField ? hiddenInput("orderByField", orderField) : "");
            st.multi = multi;
            st.skipForm = skipForm;
            st.fieldForm = formId;

            // The row forms sit outside the table and the inputs of a row point to its form.
            if (!(multi || skipForm) && info.count > 0) {
                for (let i = 0; i < info.count; i++) {
                    sri.startFormListRow(info, i);
                    try {
                        st.entry = String(i);
                        w("<form" + A({ name: formId + "_" + i, id: formId + "_" + i, method: "post", action: listUrl.url }) + ">" + hiddenInput("moquiSessionToken", token()) + hiddenInputs());
                        hiddenRows(cfg.hiddenCells);
                        w("</form>\n");
                        st.entry = "";
                    } finally {
                        sri.endFormListRow();
                    }
                }
            }
            if (!skipStart) {
                if (needHeaderForm && !cfg.headerDialog) {
                    w("<form" + A({ name: cfg.headerFormId, id: cfg.headerFormId, method: "post", action: sri.getCurrentScreenUrl().url }) + ">" + hiddenInput("moquiSessionToken", token()) + orderInput() + hiddenInputs());
                    st.owner = cfg.headerFormId;
                    cfg.hiddenHeaderCells.forEach((cell) => cell());
                    st.owner = "";
                    w("</form>\n");
                }
                const rowForm = (suffix, has, transition, mapName, cells) => {
                    if (!has || !transition) return;
                    const url = sri.makeUrlByType(transition, "transition", null, "false");
                    sri.pushSingleFormMapContext(mapName || "");
                    try {
                        st.entry = suffix;
                        w("<form" + A({ name: formId + "_" + suffix, id: formId + "_" + suffix, method: "post", action: url.url }) + ">" + hiddenInput("moquiSessionToken", token()) + orderInput() + hiddenInputs());
                        cells.forEach((cell) => cell());
                        w("</form>\n");
                        st.entry = "";
                    } finally {
                        sri.popContext();
                    }
                };
                rowForm("first", cfg.hasFirst, at(n, "transition-first-row"), at(n, "map-first-row"), cfg.hiddenFirstCells);
                rowForm("second", cfg.hasSecond, at(n, "transition-second-row"), at(n, "map-second-row"), cfg.hiddenSecondCells);
                rowForm("last", cfg.hasLast, at(n, "transition-last-row"), at(n, "map-last-row"), cfg.hiddenLastCells);
                if (multi) {
                    w("<form" + A({ name: formId, id: formId, method: "post", action: listUrl.url }) + ">" + hiddenInput("moquiFormName", at(n, "name")) + hiddenInput("moquiSessionToken", token()) + hiddenInput("_isMulti", "true") + hiddenInputs());
                    for (let i = 0; i < info.count; i++) {
                        sri.startFormListRow(info, i);
                        try {
                            st.entry = String(i);
                            hiddenRows(cfg.hiddenCells);
                            st.entry = "";
                        } finally {
                            sri.endFormListRow();
                        }
                    }
                    w("</form>\n");
                }
                if (!skipHeader) {
                    if (cfg.headerDialog || cfg.savedFinds) findDialog(n, shown, cfg);
                    if (cfg.selectColumns) columnsDialog(n, shown, cfg.allFields, cfg);
                    if (at(n, "show-text-button") === "true") textDialog(n, cfg);
                }
                w('<div class="table-wrap"><table' + A({ class: cls("form-list", xn(at(n, "style")) || null), id: formId + "_table" }) + ">");
                if (!skipHeader) {
                    w("<thead>");
                    nav(n, visible, cfg);
                    if (cfg.headerDialog) {
                        // The active find parameters are shown as text with a button that clears them.
                        let summary = "";
                        visible.flat().forEach((f) => {
                            if (!f.summary) return;
                            const text = f.summary();
                            if (text) summary += '<span class="filter"><strong>' + e(f.title()) + ":</strong> " + e(text) + "</span> ";
                        });
                        if (summary) {
                            w('<tr><td colspan="' + Math.max(visible.length, 1) + '">' + summary + "<form" + A({ method: "post", action: sri.getCurrentScreenUrl().url }) + ">" + hiddenInputs() +
                                '<button type="submit" name="clearParameters" title="' + e(L("Clear")) + '"><i class="fa fa-remove"></i></button></form></td></tr>');
                        }
                    }
                    st.owner = cfg.headerFormId;
                    w("<tr>");
                    visible.forEach((col) => th(col, cfg));
                    w("</tr>");
                    if (subCols.length) {
                        w('<tr class="sub-header"><td colspan="' + Math.max(visible.length, 1) + '"><table class="form-list-sub"><thead><tr>');
                        subCols.forEach((col) => th(col, cfg));
                        w("</tr></thead></table></td></tr>");
                    }
                    w("</thead>\n");
                    st.owner = "";
                }
                w("<tbody>\n");
                st.owner = formId;
            }
            const cells = (kind, suffix, mapName) => {
                sri.pushSingleFormMapContext(mapName || "");
                try {
                    st.owner = formId + "_" + suffix;
                    st.entry = suffix;
                    w('<tr class="' + suffix + '">');
                    visible.forEach((col) => {
                        w("<td>");
                        col.forEach((f) => { if (f[kind]) f[kind](); });
                        w("</td>");
                    });
                    w("</tr>\n");
                    st.entry = "";
                    st.owner = formId;
                } finally {
                    sri.popContext();
                }
            };
            if (cfg.hasFirst) cells("first", "first", at(n, "map-first-row"));
            if (cfg.hasSecond) cells("second", "second", at(n, "map-second-row"));
            for (let i = 0; i < info.count; i++) {
                sri.startFormListRow(info, i);
                try {
                    st.entry = String(i);
                    if (!(multi || skipForm)) st.owner = formId + "_" + i;
                    w("<tr" + A({ "data-total": ec.context.get("_moquiRowType") === "total" ? "" : null }) + ">");
                    visible.forEach((col) => {
                        w("<td>");
                        col.forEach((f) => f.cell(multi ? 1 : 0));
                        w("</td>");
                    });
                    w("</tr>\n");
                    const subRows = subCols.length ? sri.formListSubRowCount(info, i) : 0;
                    if (subRows) {
                        w('<tr class="sub-rows"><td colspan="' + Math.max(visible.length, 1) + '"><table class="form-list-sub"><tbody>');
                        for (let j = 0; j < subRows; j++) {
                            sri.startFormListSubRow(info, i, j);
                            try {
                                st.entry = i + "_" + j;
                                w("<tr>");
                                subCols.forEach((col) => {
                                    w("<td>");
                                    col.forEach((f) => f.cell(multi ? 1 : 0));
                                    w("</td>");
                                });
                                w("</tr>");
                            } finally {
                                sri.endFormListSubRow();
                            }
                            st.entry = String(i);
                        }
                        w("</tbody></table></td></tr>\n");
                    }
                } finally {
                    sri.endFormListRow();
                }
            }
            st.entry = "";
            st.owner = skipStart ? "" : formId;
            sri.safeCloseList(info);
            if (cfg.hasLast) cells("last", "last", at(n, "map-last-row"));
            if (!skipEnd) {
                if (multi && info.count > 0) {
                    w('<tr><td colspan="' + Math.max(visible.length, 1) + '">');
                    st.owner = formId;
                    cols.forEach((col) => col.forEach((f) => f.cell(2)));
                    w("</td></tr>");
                }
                w("</tbody></table></div>\n");
            }
            st.owner = "";
            reset();
        };

        // A list cell for one field: a block with the container style and alignment of the field. The final row
        // of a multi form has none.
        const rowCellOpen = (style, align, final) => {
            if (!final) w("<div" + A({ class: style || null, "data-align": align && align !== "left" ? align : null }) + ">");
        };
        const rowCellClose = (final) => {
            if (!final) w("</div>");
        };

        // The text of a find widget for the line of active filters above a list.
        const valueText = (n) => {
            const type = n.name;
            const name = fnm(n);
            if (["display", "display-entity", "hidden", "ignored", "password", "reset", "submit", "text-area", "link", "label"].indexOf(type) >= 0) return "";
            if (type === "drop-down" || type === "check" || type === "radio") {
                let current = type === "drop-down" ? sri.getFieldValuePlainString(fieldOf(n), "") : sri.getFieldValueString(n);
                if (!current) return "";
                const options = sri.getFieldOptions(n);
                const text = current.split(",").map((part) => options.get(part) || part).join(", ");
                return text + (param(name + "_not") === "Y" ? " (" + L("Not") + ")" : "");
            }
            if (type === "date-period") {
                const period = String(ec.context.get(name + "_period") || "").toLowerCase();
                if (period) return period + " " + (ec.context.get(name + "_poffset") || "0") + " " + (ec.context.get(name + "_pdate") || "");
            }
            if (type === "date-find" || type === "date-period" || type === "range-find") {
                const format = type === "date-find" ? dateDefault(n) : "yyyy-MM-dd";
                const from = type === "range-find" ? ec.context.get(name + "_from") : formatted(ec.context.get(name + "_from"), format);
                const thru = type === "range-find" ? ec.context.get(name + "_thru") : formatted(ec.context.get(name + "_thru"), format);
                return (from ? L("From") + " " + from : "") + (thru ? " " + L("to") + " " + thru : "");
            }
            return sri.getFieldValueString(n);
        };

        const h = {
            reset, e, lib, fid, fnm,
            textLine, textArea, password, file, hidden, reset_, submit, dateTime, dateFind, datePeriod, rangeFind, textFind,
            dropDown, check: (n) => checkOrRadio(n, "checkbox"), radio: (n) => checkOrRadio(n, "radio"), display, displayEntity,
            image, label, link, editable, containerDialog, dynamicDialog, dynamicContainer, tree,
            subscreensMenu, subscreensPanel, formOpen, formClose, fieldOpen, fieldClose, rowOpen, colOpen, fieldGroup, fieldGroupClose,
            formList, st, x, xn, nodeId, cond, token, A, cls, at,
            valueText, rowCellOpen, rowCellClose,
            accordion: (id, active) => { st.accordion = id; st.accIndex = 0; st.accActive = active; },
            endAccordion: () => { st.accordion = ""; },
            labelCols: (value) => { st.labelCols = value || ""; }
        };
        Object.defineProperty(globalThis, "$mqHtml", { value: h, configurable: true });
    };

    const runtime = "(" + runtimeFunction.toString() + ")();";
    const prelude = "const $h = globalThis.$mqHtml;\n$h.reset();\n";

    // ---- macros ----

    const widgetCall = (name, node, withChildren) => `$h.${name}(${wlit(node, withChildren)});\n`;
    const write = (text) => `sri.write(${lit(text)});\n`;
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

    // A field sub-node of a form-single or the header of a form-list: its widgets, which are links or
    // other macros, after the set elements.
    const subWidgets = (sub) => widgetsSource(sub);

    // The code of one field of a form-single.
    const singleFieldCode = (field, bigRow) => {
        const code = fieldChain(field, (sub) => {
            if (child(sub, "ignored")) return "";
            if (child(sub, "hidden")) return subWidgets(sub);
            const isSubmit = !!child(sub, "submit");
            const style = attr(sub, "container-style");
            return js`
                {
                    const $title = ${titleCode(sub)};
                    const $open = $h.fieldOpen(${lit(attr(field, "name"))}, $title, ${isSubmit}, ${style ? noL10n(style) : '""'}, ${!!bigRow});
                    sri.pushContext();
                    try {
                        ${setCode(sub)}
                        ${subWidgets(sub)}
                    } finally {
                        sri.popContext();
                    }
                    $h.fieldClose($open, ${!!bigRow});
                }
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

    // The code for the children of a field-layout element. The fields are by name.
    const layoutCode = (node, fields, referenced, big) => node.children.map((kid) => {
        const name = kid.name;
        if (name === "field-ref") {
            const field = fields[attr(kid, "name")];
            if (!field) return write(`<div>Error: could not find field with name ${attr(kid, "name")} referred to in a field-ref.name attribute.</div>\n`);
            return singleFieldCode(field, big);
        }
        if (name === "fields-not-referenced") {
            return Object.keys(fields).filter((fieldName) => !referenced.has(fieldName)).map((fieldName) => singleFieldCode(fields[fieldName], big)).join("");
        }
        if (name === "field-row") {
            return js`
                $h.rowOpen("field-row", "");
                ${kid.children.map((cell) => write("<div>") + layoutCode({ children: [cell] }, fields, referenced, false) + write("</div>")).join("")}
                sri.write("</div>\\n");
            `;
        }
        if (name === "field-row-big") {
            const title = attr(kid, "title");
            return js`
                sri.write("<div class=\\"field big-row\\">");
                ${title ? `sri.write("<label>" + ec.resource.expand(${lit(title)}, "") + "</label>");` : ""}
                sri.write("<div class=\\"widget\\">");
                ${layoutCode(kid, fields, referenced, true)}
                sri.write("</div></div>\\n");
            `;
        }
        if (name === "field-group") {
            return js`
                $h.fieldGroup(${bareLit(kid)});
                ${layoutCode(kid, fields, referenced, big)}
                $h.fieldGroupClose(${bareLit(kid)});
            `;
        }
        if (name === "field-accordion") {
            const id = attr(kid, "id");
            return js`
                $h.accordion(${id ? lit(id) : `$h.st.fieldForm + "_accordion"`}, ${lit(attr(kid, "active") || "1")});
                sri.write("<div class=\\"accordion\\">");
                ${layoutCode(kid, fields, referenced, big)}
                sri.write("</div>\\n");
                $h.endAccordion();
            `;
        }
        if (name === "field-col-row") {
            return js`
                $h.rowOpen("row", ${lit(attr(kid, "style"))});
                ${childrenNamed(kid, "field-col").map((col) => js`
                    $h.colOpen(${bareLit(col)});
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

    const hasFile = (node) => node.name === "file" || node.children.some(hasFile);

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
                $h.formOpen(${bareLit(node)}, ${hasFile(node)});
                ${body}
                $h.formClose(${bareLit(node)});
            } finally {
                sri.popContext();
            }
            if (sri.doBoundaryComments()) sri.write(${lit(`<!-- END   form-single[@name=${attr(node, "name")}] -->\n`)});
        `;
    });

    // The code of a list cell for one field: the widgets of the first conditional-field that holds,
    // else the default-field. $m is 0 for a row, 1 for a row of a form that posts all rows, and 2 for the
    // final row, which holds the submit widgets of such a form.
    const cellCode = (field) => {
        const code = fieldChain(field, (sub) => {
            if (child(sub, "ignored")) return "";
            const hide = attr(field, "hide");
            const isSubmit = !!child(sub, "submit");
            const isHidden = !!child(sub, "hidden");
            const style = attr(sub, "container-style");
            const align = attr(field, "align");
            const inner = isHidden ? subWidgets(sub) : js`
                $h.rowCellOpen(${style ? noL10n(style) : '""'}, ${lit(align)}, $m === 2);
                sri.pushContext();
                try {
                    ${setCode(sub)}
                    ${subWidgets(sub)}
                } finally {
                    sri.popContext();
                }
                $h.rowCellClose($m === 2);
            `;
            const guard = isSubmit ? "$m !== 1" : "$m !== 2";
            return js`
                if (${guard}${hide ? ` && !ec.resource.condition(${lit(hide)}, "")` : ""}) {
                    ${inner}
                }
            `;
        });
        return code;
    };

    // The code of a header, first-row, second-row or last-row field. Hidden widgets are written by the form of the row.
    const sideCode = (field, tag, withHidden) => {
        const sub = child(field, tag);
        if (!sub || !sub.children.length) return null;
        if (!withHidden && child(sub, "hidden")) return null;
        if (child(sub, "ignored")) return null;
        const hide = attr(field, "hide");
        const body = child(sub, "hidden") ? subWidgets(sub) : js`
            sri.pushContext();
            try {
                ${setCode(sub)}
                ${subWidgets(sub)}
            } finally {
                sri.popContext();
            }
        `;
        return `() => {\n${hide ? `if (ec.resource.condition(${lit(hide)}, "")) return;\n` : ""}${body}}`;
    };

    const dialogFieldCode = (field) => {
        const sub = child(field, "header-field");
        if (!sub || !sub.children.length || child(sub, "ignored")) return null;
        const isHidden = !!child(sub, "hidden");
        const style = attr(sub, "container-style");
        const body = isHidden ? subWidgets(sub) : js`
            {
                const $title = ${titleCode(sub)};
                const $open = $h.fieldOpen(${lit(attr(field, "name"))}, $title, ${!!child(sub, "submit")}, ${style ? noL10n(style) : '""'}, false);
                sri.pushContext();
                try {
                    ${setCode(sub)}
                    ${subWidgets(sub)}
                } finally {
                    sri.popContext();
                }
                $h.fieldClose($open, false);
            }
        `;
        return `() => {\n${body}}`;
    };

    const columnField = (field, fieldIndex) => {
        const header = S.headerSub(field);
        const headerField = child(field, "header-field");
        const hide = attr(field, "hide");
        const orderBy = header && (attr(header, "show-order-by") === "true" || attr(header, "show-order-by") === "case-insensitive") ? attr(header, "show-order-by") : "";
        const find = sideCode(field, "header-field", false);
        const first = sideCode(field, "first-row-field", false);
        const second = sideCode(field, "second-row-field", false);
        const last = sideCode(field, "last-row-field", false);
        const summary = headerField && headerField.children.length && !child(headerField, "hidden") && !child(headerField, "ignored") ?
            `() => { let $v = ""; sri.pushContext(); try { ${setCode(headerField)} ${headerField.children.filter((w) => w.name !== "set" && w.name !== "link").map((w) => `$v += $h.valueText(${wlit(w, true)});`).join(" ")} } finally { sri.popContext(); } return $v.trim(); }` : "null";
        return js`
            {
                "name": ${lit(attr(field, "name"))},
                "align": ${lit(attr(field, "align"))},
                "headerStyle": ${noL10n(headerField ? attr(headerField, "container-style") : "")},
                "orderBy": ${lit(orderBy)},
                "submit": ${!!(header && child(header, "submit"))},
                "sub": ${attr(field, "aggregate") === "sub-list"},
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
        const hiddenOf = (tag) => fields.filter((field) => { const sub = child(field, tag); return sub && child(sub, "hidden"); });
        const hiddenCell = (field, tag) => {
            const sub = child(field, tag);
            return `() => {\n${subWidgets(sub)}}`;
        };
        const hiddenDefault = fields.filter(S.listFieldHiddenWidget).map((field) => `() => {\nconst $m = 0;\n${cellCode(field)}}`);
        const cfg = js`
            {
                "hasFirst": ${hasRow("first-row-field")},
                "hasSecond": ${hasRow("second-row-field")},
                "hasLast": ${hasRow("last-row-field")},
                "headerForm": ${fields.some((field) => { const sub = child(field, "header-field"); return sub && sub.children.length > 0; })},
                "hiddenCells": [
                    ${hiddenDefault.join(",\n")}
                ],
                "hiddenHeaderCells": [
                    ${hiddenOf("header-field").map((field) => hiddenCell(field, "header-field")).join(",\n")}
                ],
                "hiddenFirstCells": [
                    ${hiddenOf("first-row-field").map((field) => hiddenCell(field, "first-row-field")).join(",\n")}
                ],
                "hiddenSecondCells": [
                    ${hiddenOf("second-row-field").map((field) => hiddenCell(field, "second-row-field")).join(",\n")}
                ],
                "hiddenLastCells": [
                    ${hiddenOf("last-row-field").map((field) => hiddenCell(field, "last-row-field")).join(",\n")}
                ],
                "allFields": [
                    ${fields.filter((field) => !S.listFieldHiddenWidget(field)).map((field) => `{"name": ${lit(attr(field, "name"))}, "title": () => ${titleCode(S.headerSub(field))}}`).join(",\n")}
                ]
            }
        `.replace(/\n$/, "");
        const cols = columns.map((column) => "[" + column.map(columnField).join(",\n") + "]").join(",\n");
        return js`
            if (sri.doBoundaryComments()) sri.write(${lit(`<!-- BEGIN form-list[@name=${attr(node, "name")}] -->\n`)});
            {
                const $info = sri.openFormList(${lit(attr(node, "name"))});
                $h.formList($info, ${bareLit(node)}, [
                    ${cols}
                ], ${cfg});
            }
            if (sri.doBoundaryComments()) sri.write(${lit(`<!-- END   form-list[@name=${attr(node, "name")}] -->\n`)});
        `;
    });

    const widget = (name, withChildren) => stmt((node) => widgetCall(name, node, withChildren));

    const containerMacro = stmt((node, children) => {
        const type = attr(node, "type") || "div";
        const id = attr(node, "id");
        const style = attr(node, "style");
        return js`
            sri.write("<" + ${lit(type)} + $h.A({id: ${id ? "$h.nodeId(" + bareLit(node) + ")" : "null"}, class: ${style ? noL10n(style) : "null"}}) + ">");
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
        const height = body ? attr(body, "height") : "";
        return js`
            sri.write("<section" + $h.A({id: ${attr(node, "id") ? "$h.nodeId(" + bareLit(node) + ")" : "null"}, class: "box", "data-type": ${noL10n(attr(node, "type") || "default")}}) + "><header>");
            ${title ? `sri.write("<h5>" + $h.e(ec.resource.expand(${lit(title)}, "")) + "</h5>");` : ""}
            ${header ? header.source : ""}
            ${toolbar ? `sri.write("<div class=\\"toolbar\\">");\n${toolbar.source}sri.write("</div>");` : ""}
            sri.write("</header>");
            ${body ? `sri.write(${lit(`<div class="body"${height ? ` style="max-height:${height}px;overflow-y:auto"` : ""}>`)});\n${body.source}sri.write("</div>");` : ""}
            ${nopad ? `sri.write("<div class=\\"body-nopad\\">");\n${nopad.source}sri.write("</div>");` : ""}
            sri.write("</section>\\n");
        `;
    });

    const containerRowMacro = stmt((node) => js`
        $h.rowOpen("row", ${lit(attr(node, "style"))});
        ${childrenNamed(node, "row-col").map((col) => js`
            $h.colOpen(${bareLit(col)});
            ${col.source}
            sri.write("</div>");
        `).join("")}
        sri.write("</div>\\n");
    `);

    const containerPanelMacro = stmt((node) => {
        const part = (name) => { const kid = child(node, name); return kid ? kid.source : ""; };
        const sized = (name) => {
            const kid = child(node, name);
            return kid ? `sri.write(${lit(`<aside class="${name === "panel-left" ? "left" : "right"}" style="width:${attr(kid, "size") || "180"}px">`)});\n${kid.source}sri.write("</aside>");\n` : "";
        };
        const id = attr(node, "id");
        return js`
            sri.write("<div" + $h.A({id: ${id ? "$h.nodeId(" + bareLit(node) + ")" : "null"}, class: "panel-layout"}) + ">");
            ${child(node, "panel-header") ? `sri.write("<header>");\n${part("panel-header")}sri.write("</header>");` : ""}
            sri.write("<div class=\\"panel-middle\\">");
            ${sized("panel-left")}
            sri.write("<main>");
            ${part("panel-center")}
            sri.write("</main>");
            ${sized("panel-right")}
            sri.write("</div>");
            ${child(node, "panel-footer") ? `sri.write("<footer>");\n${part("panel-footer")}sri.write("</footer>");` : ""}
            sri.write("</div>\\n");
        `;
    });

    const containerDialogMacro = stmt((node, children) => condGuard("", js`
        if ($h.containerDialog(${bareLit(node)})) {
            ${children}
            sri.write("</div></dialog>\\n");
        }
    `));

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
                    if (${lit(attr(node, "encode"))} !== "false") $text = $h.e($text);
                    let $icon = ${lit(attr(node, "icon"))};
                    if (!$icon && $text) $icon = sri.getThemeIconClass($text) || "";
                    const $badge = ec.resource.expand(${lit(attr(node, "badge"))}, "");
                    sri.write("<details class=\\"menu\\"" + $h.A({id: ${attr(node, "id") ? lit(attr(node, "id")) : "null"}}) + "><summary" + $h.A({"data-btn": ${lit(attr(node, "btn-type") || "primary")}, class: ${attr(node, "style") ? noL10n(attr(node, "style")) : "null"}}) + ">" +
                        ($icon ? "<i class=\\"" + $h.e($icon) + "\\"></i> " : "") + $text + ($badge ? " <span class=\\"badge\\">" + $h.e($badge) + "</span>" : "") + "</summary><ul>");
                    ${linkItems.map((kid) => `sri.write("<li>");\n${kid.source}sri.write("</li>");\n`).join("")}
                    sri.write("</ul></details>\\n");
                }
            }
        `);
    });

    const linkMacro = stmt((node) => {
        const inField = node.parent && node.parent.name.endsWith("-field");
        return `$h.link(${wlit(node, true)}, ${inField});\n`;
    });

    const submitMacro = stmt((node) => `$h.submit(${wlit(node, true)}, ${titleCode(node.parent)});\n`);
    const resetMacro = stmt((node) => `$h.reset_(${wlit(node, false)}, ${titleCode(node.parent)});\n`);

    const tableOfFormNoops = noops(S.allElements);

    const macros = Object.assign({}, tableOfFormNoops, S.anyMacros(MODE), {
        "screen": stmt((node, children) => children),
        "widgets": widgetsMacro("widgets"),
        "fail-widgets": widgetsMacro("fail-widgets"),
        "dynamic-form": stmt((node, children) => children),
        "subscreens-menu": widget("subscreensMenu", false),
        "subscreens-panel": widget("subscreensPanel", false),
        "section-include": stmt((node) => `sri.renderSectionInclude(${S.bareLit(node)});`),

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

    // button-menu hands the macros of its items to its own macro, which writes them in list items.
    const opaque = S.opaqueElements.filter((name) => name !== "button-menu");

    return { macros, opaque, prelude, runtime };
})
