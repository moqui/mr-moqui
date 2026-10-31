/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The client of the html render mode. It has no dependencies and no build step. The macros in ScreenHtml.js write
 * elements and data attributes; this file binds them (README.md lists each attribute). Tom Select and SortableJS
 * are loaded the first time a page has an element that needs them.
 */
(function (root, factory) {
    const api = factory(root);
    if (typeof module !== "undefined" && module.exports) module.exports = api.pure;
    if (root.document) {
        root.moqui = api.client;
        if (root.document.readyState === "loading") root.document.addEventListener("DOMContentLoaded", api.client.start, { once: true });
        else api.client.start();
    }
})(typeof window !== "undefined" ? window : globalThis, function (root) {
    "use strict";

    // ---------- pure functions, tested under Node ----------

    // A datetime-local value with a T between date and time becomes the timestamp text the server reads.
    const normalizeDateTime = (value) => (/^\d{4}-\d\d-\d\d[T ]\d\d:\d\d/.test(value || "") ? value.replace("T", " ") : value);

    const excludeEmptyFields = (data) => {
        const empty = Array.from(data.entries()).filter(([, value]) => value === "" || value === null || value === undefined).map(([name]) => name);
        empty.forEach((name) => data.delete(name));
        return data;
    };

    const parseJson = (text, fallback) => {
        if (!text) return fallback;
        try {
            return JSON.parse(text);
        } catch (err) {
            return fallback;
        }
    };

    // Form-encoded text from an object. A list value repeats the name.
    const queryString = (params) => {
        const parts = [];
        Object.keys(params || {}).forEach((key) => {
            [].concat(params[key]).forEach((value) => {
                if (value !== null && value !== undefined) parts.push(encodeURIComponent(key) + "=" + encodeURIComponent(value));
            });
        });
        return parts.join("&");
    };

    // The options of a remote response: a list, or an object with an options list. Items read value and label by field name.
    const optionList = (data, valueField, labelField) => {
        const list = Array.isArray(data) ? data : data && Array.isArray(data.options) ? data.options : [];
        return list.map((item) => {
            const value = item[valueField || "value"];
            return { value: value === undefined || value === null || value === "" ? "\u00a0" : String(value), text: String(item[labelField || "label"] === undefined ? item.text || "" : item[labelField || "label"]) };
        });
    };

    // The parameters a field gets from the fields it depends on: [[field id, parameter name], ...] and a reader of values.
    // Returns null when a required value is missing, unless the dependency is optional.
    const dependsParams = (spec, read, optional) => {
        const out = {};
        let complete = true;
        (spec || []).forEach((pair) => {
            const value = read(pair[0]);
            if (value === "" || value === null || value === undefined) complete = false;
            else out[pair[1]] = value;
        });
        return complete || optional ? out : null;
    };

    // Order-by text from a Tom Select value: choosing a column in one direction drops it in the other.
    const orderByValue = (values) => {
        const seen = new Set();
        return [].concat(values || []).filter((value) => {
            const column = value.replace(/^[-^]+/, "");
            if (seen.has(column)) return false;
            seen.add(column);
            return true;
        }).join(",");
    };

    // The tree nodes of a response, as the renderer draws them: text, href, name, open state and children.
    const treeNodes = (data) => (Array.isArray(data) ? data : []).map((node) => ({
        id: node.id, text: node.text || "", href: node.a_attr && node.a_attr.href, name: node.li_attr && node.li_attr.treeNodeName,
        opened: !!(node.state && node.state.opened), children: Array.isArray(node.children) ? treeNodes(node.children) : node.children ? true : false
    }));

    // The options of an editable select from its editable-load answer: a list of {value, label} (key and text, or plain
    // values), or a map of key to label with the current key under "selected" (Java's jEditable).
    const editableOptions = (answer) => {
        const one = (item) => (item && typeof item === "object" ?
            { value: String(item.value ?? item.key ?? ""), label: String(item.label ?? item.text ?? item.value ?? item.key ?? "") } :
            { value: String(item), label: String(item) });
        if (Array.isArray(answer)) return { options: answer.map(one) };
        if (answer && typeof answer === "object") {
            if (Array.isArray(answer.options)) return { options: answer.options.map(one), selected: answer.selected };
            return { options: Object.keys(answer).filter((key) => key !== "selected").map((key) => ({ value: key, label: String(answer[key]) })), selected: answer.selected };
        }
        return { options: [] };
    };

    // The fields of a date pattern (Java's: yyyy-MM-dd HH:mm), each a number, and the text between them. Null when a field
    // is not a number (a month name, a weekday, AM/PM): the user types letters into those, so the letter keys stay text.
    const datePattern = (pattern) => {
        const parts = [];
        const re = /'([^']*)'|([A-Za-z])\2*|[^A-Za-z']+/g;
        let match;
        while ((match = re.exec(pattern || ""))) {
            const text = match[0];
            if (match[1] !== undefined) { parts.push({ text: match[1] }); continue; }
            if (!match[2]) { parts.push({ text }); continue; }
            const field = { y: "year", M: "month", d: "day", H: "hour", m: "minute", s: "second" }[match[2]];
            if (!field || (field === "month" && text.length > 2)) return null;
            parts.push({ field, width: text.length });
        }
        return parts.some((part) => part.field) ? parts : null;
    };
    const pad = (number, width) => String(number).padStart(width, "0");
    const parseDate = (parts, value) => {
        const digits = (part) => (part.field === "year" ? (part.width === 2 ? "\\d{2}" : "\\d{4}") : part.width === 1 ? "\\d{1,2}" : "\\d{2}");
        const source = parts.map((part) => (part.field ? "(" + digits(part) + ")" :
            part.text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"))).join("");
        const match = new RegExp("^" + source + "$").exec((value || "").trim());
        if (!match) return null;
        const now = new Date();
        const out = { year: now.getFullYear(), month: now.getMonth() + 1, day: now.getDate(), hour: 0, minute: 0, second: 0 };
        let index = 1;
        parts.forEach((part) => {
            if (!part.field) return;
            let number = Number(match[index++]);
            if (part.field === "year" && part.width === 2) number += 2000;
            out[part.field] = number;
        });
        return out;
    };
    const formatDate = (parts, date) => parts.map((part) => {
        if (!part.field) return part.text;
        const number = date[part.field];
        return part.field === "year" && part.width === 2 ? pad(number % 100, 2) : pad(number, part.width === 1 ? 1 : part.width);
    }).join("");
    // The value after a date key, or null when the key changes nothing. The keys: t today (now with a time), y, m, d and w a
    // year, month, day and week later, h, i and s an hour, `step` minutes and a second later, Shift with any of them earlier,
    // and ArrowUp and ArrowDown a day later and earlier. A key needs the field it changes, and a value, but t.
    const dateKey = (pattern, value, key, shift, step, now) => {
        const parts = datePattern(pattern);
        if (!parts) return null;
        const has = (field) => parts.some((part) => part.field === field);
        const lower = key.length === 1 ? key.toLowerCase() : key;
        const when = now || new Date();
        if (lower === "t") {
            return formatDate(parts, { year: when.getFullYear(), month: when.getMonth() + 1, day: when.getDate(), hour: when.getHours(),
                minute: when.getMinutes(), second: has("second") ? when.getSeconds() : 0 });
        }
        const sign = shift || key === "ArrowDown" ? -1 : 1;
        const change = { y: ["year", 1], m: ["month", 1], d: ["day", 1], w: ["day", 7], ArrowUp: ["day", 1], ArrowDown: ["day", 1],
            h: ["hour", 1], i: ["minute", Number(step) || 1], s: ["second", 1] }[lower];
        if (!change || !has(change[0])) return null;
        const date = parseDate(parts, value);
        if (!date) return null;
        let next;
        if (change[0] === "year" || change[0] === "month") {
            // the day stays, or is the month's last when the month is shorter (Jan 31 and a month is Feb 28)
            const months = date.year * 12 + date.month - 1 + sign * change[1] * (change[0] === "year" ? 12 : 1);
            const year = Math.floor(months / 12), month = months % 12;
            const last = new Date(year, month + 1, 0).getDate();
            next = new Date(year, month, Math.min(date.day, last), date.hour, date.minute, date.second);
        } else {
            next = new Date(date.year, date.month - 1, date.day, date.hour, date.minute, date.second);
            const unit = { day: "Date", hour: "Hours", minute: "Minutes", second: "Seconds" }[change[0]];
            next["set" + unit](next["get" + unit]() + sign * change[1]);
        }
        return formatDate(parts, { year: next.getFullYear(), month: next.getMonth() + 1, day: next.getDate(), hour: next.getHours(),
            minute: next.getMinutes(), second: next.getSeconds() });
    };

    // Quick entry in a drop-down search (a barcode scanner types fast and ends with ENTER or TAB): the key waits while options
    // load and until `quiet` ms have passed since the last key, so it acts on the options of what was typed.
    const quickKeyWaits = (loading, lastKeyTime, now, quiet) => !!loading || (lastKeyTime > 0 && now - lastKeyTime < (quiet || 500));

    // The value a select keeps when its options are replaced: the current values still offered, or, as a native single
    // select with no empty option, the first option.
    const keptValue = (current, options, multiple, empty) => {
        const keep = [].concat(current).filter((value) => options.some((item) => item.value === value));
        if (multiple) return keep;
        if (keep.length) return keep[0];
        return !empty && options.length ? options[0].value : "";
    };

    const pure = { normalizeDateTime, excludeEmptyFields, parseJson, queryString, optionList, dependsParams, orderByValue, treeNodes, editableOptions,
        datePattern, dateKey, quickKeyWaits, keptValue };

    // ---------- the browser client ----------

    const doc = root.document;
    if (!doc) return { pure, client: {} };

    const $ = (selector, scope) => (scope || doc).querySelector(selector);
    const $$ = (selector, scope) => Array.from((scope || doc).querySelectorAll(selector));
    const el = (tag, attrs, text) => {
        const node = doc.createElement(tag);
        Object.keys(attrs || {}).forEach((key) => node.setAttribute(key, attrs[key]));
        if (text) node.textContent = text;
        return node;
    };
    // Run init once for each element a selector finds under scope, and never again for the same element.
    const done = new WeakMap();
    const each = (scope, selector, init) => {
        const nodes = scope.matches && scope.matches(selector) ? [scope] : [];
        nodes.concat($$(selector, scope)).forEach((node) => {
            const ran = done.get(node) || new Set();
            if (ran.has(selector)) return;
            ran.add(selector);
            done.set(node, ran);
            init(node);
        });
    };

    const tokenMeta = () => $("meta[name=moquiSessionToken]");
    const sessionToken = () => (tokenMeta() ? tokenMeta().content : "");

    // fetch with the session token as a header, cookies, and the token kept current from the response.
    const request = (url, options) => {
        const settings = Object.assign({ credentials: "same-origin", headers: {} }, options);
        settings.headers = Object.assign({ "X-CSRF-Token": sessionToken(), "X-Requested-With": "fetch" }, settings.headers);
        return root.fetch(url, settings).then((response) => {
            const fresh = response.headers.get("moquiSessionToken");
            if (fresh && tokenMeta()) tokenMeta().content = fresh;
            return response;
        });
    };
    const post = (url, params) => request(url, {
        method: "POST", headers: { "Content-Type": "application/x-www-form-urlencoded" }, body: queryString(Object.assign({ moquiSessionToken: sessionToken() }, params))
    });
    const postJson = (url, params) => post(url, params).then((response) => (response.ok ? response.text() : Promise.reject(new Error(response.status + " " + response.statusText))))
        .then((text) => parseJson(text, text));

    // ---------- messages ----------

    const notify = (message, type) => {
        let box = $("#toasts");
        if (!box) { box = el("div", { id: "toasts", "aria-live": "polite" }); doc.body.appendChild(box); }
        const toast = el("div", { class: "alert", "data-type": type || "info", role: "alert" });
        toast.appendChild(el("button", { type: "button", class: "close", "aria-label": "Close", "data-dismiss": "" }, "\u00d7"));
        toast.appendChild(doc.createTextNode(message));
        box.appendChild(toast);
        root.setTimeout(() => toast.remove(), type === "danger" ? 12000 : 6000);
    };

    // Copy the text at a URL (a form-list's md render) to the clipboard. A ClipboardItem takes the pending text, so the copy keeps
    // the click's user activation while the request runs; without one the text is written when it arrives.
    const copyUrl = (button) => {
        const text = () => request(button.dataset.copyUrl).then((response) => (response.ok ? response.text() : Promise.reject(new Error(response.status + " " + response.statusText))));
        const clipboard = root.navigator.clipboard;
        if (!clipboard) { notify(button.dataset.copyFailed || "Copy failed", "danger"); return; }
        const copied = root.ClipboardItem && clipboard.write
            ? clipboard.write([new root.ClipboardItem({ "text/plain": text().then((body) => new Blob([body], { type: "text/plain" })) })])
            : text().then((body) => clipboard.writeText(body));
        copied.then(() => notify(button.dataset.copied || "Copied", "success"), (err) => notify((button.dataset.copyFailed || "Copy failed") + ": " + err.message, "danger"));
    };

    // The messages in a fetched page become toasts, since the page itself is not shown.
    const notifyFromHtml = (html) => {
        const found = new DOMParser().parseFromString(html, "text/html");
        $$(".alert", found).forEach((alert) => notify(alert.textContent.replace("\u00d7", "").trim(), alert.dataset.type));
    };

    // ---------- loading ----------

    const parseHtml = (html) => { const tpl = doc.createElement("template"); tpl.innerHTML = html; return tpl.content; };

    // Put fetched html into a container and bind what it holds.
    const fill = (container, html) => {
        container.replaceChildren(parseHtml(html));
        bind(container);
    };

    const load = (container, url) => {
        container.dataset.loadUrl = url || container.dataset.loadUrl || container.dataset.load;
        return request(container.dataset.loadUrl).then((response) => response.text().then((text) => {
            if (!response.ok) notify(response.status + " " + response.statusText, "danger");
            else fill(container, text);
        })).catch((err) => notify(String(err.message || err), "danger"));
    };

    // ---------- forms ----------

    const clearForm = (form) => {
        $$("input, textarea, select", form).forEach((field) => {
            if (field.type === "hidden" || field.type === "submit" || field.type === "button") return;
            if (field.type === "checkbox" || field.type === "radio") field.checked = false;
            else field.value = "";
            field.dispatchEvent(new Event("change", { bubbles: true }));
        });
    };

    // A background submit asks for JSON. The validation errors of this form's own fields are shown by the field, with the markup
    // the html form macros write after a redirect; the other messages and errors become toasts.
    const clearFieldErrors = (form) => {
        $$(".field-error[data-bg-error]", form).forEach((node) => node.remove());
        $$(".field[data-invalid][data-bg-error]", form).forEach((node) => { node.removeAttribute("data-invalid"); node.removeAttribute("data-bg-error"); });
    };
    const placeFieldErrors = (form, list) => {
        const placed = [];
        const formName = form.elements.moquiFormName ? form.elements.moquiFormName.value : null;
        (list || []).forEach((ve) => {
            if (!ve || !ve.field || (ve.form && formName && ve.form !== formName)) return;
            const input = form.elements[ve.field];
            const target = input && (input.length !== undefined && !input.tagName ? input[0] : input);
            const box = target && target.closest ? target.closest(".field") : null;
            if (!box) return;
            if (!box.hasAttribute("data-invalid")) box.setAttribute("data-invalid", "");
            box.setAttribute("data-bg-error", "");
            const widget = box.querySelector(".widget") || box;
            widget.appendChild(el("div", { class: "field-error", role: "alert", "data-bg-error": "" }, ve.message));
            // its line in errors is the message, or the message with its names as the server writes it (toStringPretty)
            placed.push(ve.message, ve.message + "(" + (ve.fieldPretty ? "for field " + ve.fieldPretty : "") +
                (ve.formPretty ? " on form " + ve.formPretty : "") + (ve.serviceNamePretty ? " of service " + ve.serviceNamePretty : ""));
        });
        return placed;
    };
    const notifyFromJson = (form, json) => {
        (json.messageInfos || []).forEach((info) => notify(info.message, info.type === "danger" || info.type === "error" ? "danger" : info.type));
        const placed = placeFieldErrors(form, json.validationErrors);
        const errors = Array.isArray(json.errors) ? json.errors : (json.errors ? String(json.errors).split("\n") : []);
        errors.filter((line) => line && placed.indexOf(line) === -1).forEach((line) => notify(line, "danger"));
        return errors.length > 0;
    };

    const submitBackground = (form, submitter) => {
        const data = new FormData(form, submitter);
        data.set("moquiSessionToken", sessionToken());
        clearFieldErrors(form);
        return request(form.action, { method: (form.method || "post").toUpperCase(), body: data, headers: { Accept: "application/json, text/html;q=0.9" } })
            .then((response) => response.text().then((text) => {
                const json = (response.headers.get("Content-Type") || "").indexOf("json") !== -1 ? parseJson(text, null) : null;
                let failed = !response.ok;
                if (json && typeof json === "object") failed = notifyFromJson(form, json) || failed;
                else notifyFromHtml(text);
                if (failed) { if (!json) notify(response.status + " " + response.statusText, "danger"); return; }
                if (form.dataset.msg) notify(form.dataset.msg, "success");
                if (form.dataset.reload) { const target = doc.getElementById(form.dataset.reload); if (target) load(target); }
                if (form.dataset.hide) { const dialog = doc.getElementById(form.dataset.hide); if (dialog && dialog.close) dialog.close(); }
            })).catch((err) => notify(String(err.message || err), "danger"));
    };

    const initForm = (form) => {
        // datetime-local values go to the server as "yyyy-MM-dd HH:mm" and not with a T
        form.addEventListener("formdata", (event) => {
            Array.from(form.elements).forEach((field) => {
                if (field.type === "datetime-local" && field.name && field.value) event.formData.set(field.name, normalizeDateTime(field.value));
            });
            if (form.dataset.excludeEmptyFields === "true") excludeEmptyFields(event.formData);
        });
        if (form.hasAttribute("data-bg")) {
            form.addEventListener("submit", (event) => {
                event.preventDefault();
                submitBackground(form, event.submitter);
            });
        }
        if (form.dataset.gopage) {
            form.addEventListener("submit", () => { const input = form.elements.pageIndex; input.value = Number(input.value) - 1; });
        }
        if (form.dataset.focus) {
            const target = form.querySelector('[name^="' + form.dataset.focus + '"]');
            if (target) target.focus();
        }
    };

    // A rich text-area in html mode: the div is the note, and the next input is what the form posts. Copy on each edit
    // and again as the form is submitted, so a save without a later input event still has the edit.
    const copyRichText = (node) => {
        const field = node.nextElementSibling;
        if (field && field.tagName === "INPUT") field.value = node.innerHTML;
    };
    const initRichText = (node) => {
        node.addEventListener("input", () => copyRichText(node));
        const form = node.closest("form");
        if (!form || form.dataset.richText) return;
        form.dataset.richText = "true";
        const copyAll = () => form.querySelectorAll("[data-rich-text]").forEach(copyRichText);
        form.addEventListener("formdata", copyAll);
        form.addEventListener("submit", copyAll);
    };

    // ---------- remote options, defaults and autocomplete ----------

    const readValue = (id) => { const field = doc.getElementById(id); return field ? field.value : ""; };

    const fetchOptions = (node, term) => {
        const params = dependsParams(parseJson(node.dataset.depends, []), readValue, node.hasAttribute("data-optional"));
        if (!params) return Promise.resolve(null);
        return postJson(node.dataset.url, Object.assign({}, parseJson(node.dataset.params, {}), params, { term: term || "" }))
            .then((data) => optionList(data, node.dataset.value, node.dataset.label === "load" ? "label" : node.dataset.label));
    };

    // The fields that others depend on change: run fn.
    const onDepends = (node, fn) => {
        parseJson(node.dataset.depends, []).forEach((pair) => {
            const source = doc.getElementById(pair[0]);
            if (source) source.addEventListener("change", fn);
        });
    };

    const setNativeOptions = (select, options, keep) => {
        const current = keep === undefined ? select.value : keep;
        select.replaceChildren();
        if (select.hasAttribute("data-empty")) select.appendChild(el("option", { value: "" }));
        options.forEach((item) => {
            const option = el("option", { value: item.value }, item.text);
            if (item.value === current) option.selected = true;
            select.appendChild(option);
        });
    };

    const libraries = {};
    // Load a script once; the promise settles when it has run.
    const loadScript = (src) => libraries[src] || (libraries[src] = new Promise((resolve, reject) => {
        const script = el("script", { src });
        script.onload = resolve;
        script.onerror = () => reject(new Error("could not load " + src));
        doc.head.appendChild(script);
    }));
    const loadStyle = (href) => { if (!$('link[href="' + href + '"]')) doc.head.appendChild(el("link", { rel: "stylesheet", href })); };
    const withTomSelect = () => {
        if (root.TomSelect) return Promise.resolve(root.TomSelect);
        loadStyle("/libs/tom-select/tom-select.css");
        return loadScript("/libs/tom-select/tom-select.complete.min.js").then(() => root.TomSelect);
    };
    const withSortable = () => (root.Sortable ? Promise.resolve(root.Sortable) : loadScript("/libs/sortablejs/Sortable.min.js").then(() => root.Sortable));

    // The fields a TAB goes through, in page order.
    const tabbable = () => $$("input, select, textarea, button, a[href], [tabindex]").filter((node) =>
        node.tabIndex >= 0 && !node.disabled && node.type !== "hidden" && node.getClientRects().length);
    const focusNext = (from, back) => {
        const list = tabbable();
        const next = list[list.indexOf(from) + (back ? -1 : 1)];
        if (next) next.focus();
        else from.blur();
    };
    // Quick entry in a Tom Select search: ENTER and TAB wait while options load and for 500 ms after the last key
    // (quickKeyWaits), so what a barcode scanner or a fast typist entered is matched first; then ENTER takes the highlighted
    // option and TAB takes it and moves to the next field (Shift+TAB the one before). Without a highlighted option ENTER does
    // nothing while the menu is open, and TAB just moves on.
    const quickEntry = (instance) => {
        const input = instance.control_input;
        let lastKey = 0;
        const replay = (key, shift) => {
            const event = new KeyboardEvent("keydown", { key, keyCode: key === "Enter" ? 13 : 9, shiftKey: shift, bubbles: true, cancelable: true });
            event.moquiReplay = true;
            input.dispatchEvent(event);
        };
        const whenSettled = (act) => {
            const started = Date.now();
            const check = () => {
                if (quickKeyWaits(instance.loading, lastKey, Date.now()) && Date.now() - started < 10000) root.setTimeout(check, 50);
                else act();
            };
            check();
        };
        // what was typed or pasted: a key down, or the text changing
        input.addEventListener("input", () => { lastKey = Date.now(); });
        input.addEventListener("keydown", (event) => {
            if (event.moquiReplay) return;
            if (event.key !== "Enter" && event.key !== "Tab") { lastKey = Date.now(); return; }
            const waits = quickKeyWaits(instance.loading, lastKey, Date.now());
            if (event.key === "Enter" && !waits) return;
            event.preventDefault();
            event.stopImmediatePropagation();
            const shift = event.shiftKey;
            whenSettled(() => {
                if (event.key === "Enter") { replay("Enter", shift); return; }
                if (instance.isOpen) { replay("Tab", shift); instance.close(); }
                focusNext(input, shift);
            });
        }, true);
    };

    // An inline (big-row) single select is as wide as a native select of its loaded options, plus 24px for the caret, so
    // its longest option is not cut to the 10em minimum; again after its options reload (`options`, as loaded, since a reload
    // through Tom Select's store leaves the native options as they were).
    const sizeInlineSelect = (select, options) => {
        if (select.multiple || select.hasAttribute("data-search") || !select.closest(".big-item, .big-row-item") || !select.tomselect) return;
        const measure = doc.createElement("select");
        for (const option of options || select.options) measure.appendChild(el("option", { value: option.value }, option.text));
        measure.setAttribute("aria-hidden", "true");
        measure.tabIndex = -1;
        measure.style.cssText = "position:absolute;left:-10000px;top:0;visibility:hidden;width:auto;min-width:0;max-width:none;height:auto;padding:0;border:1px solid;appearance:auto;";
        measure.style.font = root.getComputedStyle(select).font;
        doc.body.appendChild(measure);
        const width = measure.getBoundingClientRect().width;
        measure.remove();
        const wrapper = select.tomselect.wrapper;
        wrapper.style.minWidth = "60px";
        wrapper.style.width = Math.max(60, width + 24) + "px";
    };

    const initSelect = (select) => {
        const kind = select.dataset.ts;
        const search = select.hasAttribute("data-search");
        const multiple = select.multiple;
        let ready = Promise.resolve();
        if (select.dataset.url && !search) {
            // Options come from the transition now and whenever a field they depend on changes.
            const populate = () => fetchOptions(select).then((options) => {
                if (!options) return;
                const instance = select.tomselect;
                if (instance) {
                    // Through Tom Select's own store: a sync after the native options change keeps its old items, out of order
                    const current = instance.getValue();
                    // clearOptions keeps the selected items' options, so the value is cleared first (silently) and an
                    // option no longer offered is not left in the menu
                    instance.clear(true);
                    instance.clearOptions();
                    options.forEach((item, index) => {
                        const option = Object.assign({ $order: index }, item);
                        if (instance.options[item.value]) instance.updateOption(item.value, option);
                        else instance.addOption(option);
                    });
                    instance.setValue(keptValue(current, options, select.multiple, select.hasAttribute("data-empty")), true);
                    instance.refreshOptions(false);
                    sizeInlineSelect(select, options);
                } else {
                    setNativeOptions(select, options, select.value);
                }
                select.dispatchEvent(new Event("change"));
            });
            onDepends(select, populate);
            // Tom Select starts from the options the transition gives, as the native select it replaces
            ready = populate().catch(() => {});
        }
        ready.then(withTomSelect).then((TomSelect) => {
            const settings = {
                plugins: multiple ? ["remove_button"] : [], create: select.hasAttribute("data-combo") || select.hasAttribute("data-use-actual"),
                allowEmptyOption: select.hasAttribute("data-empty") || select.options.length === 0, maxOptions: null, closeAfterSelect: !multiple,
                // TAB takes the highlighted option, as ENTER does (quickEntry moves the focus on)
                selectOnTab: true,
                // Focus, such as a dialog focusing its first field, does not open the list over the fields below
                openOnFocus: false
            };
            if (search) {
                Object.assign(settings, {
                    valueField: "value", labelField: "text", searchField: ["text"], preload: false,
                    shouldLoad: (query) => query.length >= Number(select.dataset.min || 1), loadThrottle: Number(select.dataset.delay || 300),
                    load: (query, callback) => fetchOptions(select, query).then((options) => callback(options || [])).catch(() => callback())
                });
            }
            // In a table that scrolls sideways the drop-down goes on the body, so the table does not clip it (not in a dialog,
            // which is above the body); it follows the table when that scrolls.
            const table = select.closest(".table-wrap");
            if (table && !select.closest("dialog")) settings.dropdownParent = "body";
            const instance = new TomSelect(select, settings);
            quickEntry(instance);
            // a select with the style submit-on-close submits its form when its menu closes
            if (select.classList.contains("submit-on-close") && select.form) instance.on("dropdown_close", () => select.form.requestSubmit());
            if (settings.dropdownParent) table.addEventListener("scroll", () => { if (instance.isOpen) instance.positionDropdown(); }, { passive: true });
            sizeInlineSelect(select);
            // Tom Select's click only focuses, and with openOnFocus off the list would never open from the mouse: a click
            // opens the list and the next one closes it
            instance.onClick = () => {
                if (instance.activeItems.length) { instance.clearActiveItems(); instance.focus(); return; }
                if (instance.isOpen) { instance.close(); return; }
                instance.focus();
                instance.open();
            };
            const note = doc.getElementById(select.id + "_value");
            if (kind === "ac") {
                instance.on("change", (value) => { if (note) note.textContent = instance.options[value] ? instance.options[value].text : value; });
                // The text of the value the page started with comes from the transition when the page did not give it.
                if (select.dataset.label === "load" && select.value) {
                    fetchOptions(select, select.value).then((options) => {
                        const hit = (options || []).find((item) => item.value === select.value);
                        if (hit) { instance.updateOption(hit.value, { value: hit.value, text: hit.text }); if (note) note.textContent = hit.text; }
                    });
                }
            }
            if (select.dataset.orderby) {
                const target = doc.getElementById(select.dataset.orderby);
                instance.on("change", (values) => { target.value = orderByValue(values); });
                (target.value || "").split(",").filter(Boolean).forEach((value) => instance.addItem(value, true));
            }
        }).catch(() => { /* the native select stays */ });
    };

    const initDefault = (input) => {
        const fill = () => {
            const params = dependsParams(parseJson(input.dataset.depends, []), readValue, input.hasAttribute("data-optional"));
            if (!params) return;
            post(input.dataset.url, Object.assign({}, parseJson(input.dataset.params, {}), params)).then((r) => r.text()).then((text) => { input.value = text; });
        };
        onDepends(input, fill);
        fill();
    };

    // A display field whose text comes from a transition, with a hidden input for its value.
    const initDynamicDisplay = (marker) => {
        const id = marker.dataset.dynamic;
        const fill = () => {
            const params = dependsParams(parseJson(marker.dataset.depends, []), readValue, marker.hasAttribute("data-optional"));
            if (!params) return;
            post(marker.dataset.url, Object.assign({}, parseJson(marker.dataset.params, {}), params)).then((r) => r.text()).then((text) => {
                let label = text;
                let value = text;
                const data = parseJson(text, null);
                const item = Array.isArray(data) ? data[0] : data && Array.isArray(data.options) ? data.options[0] : data;
                if (item && typeof item === "object") { label = item.label || label; value = item.value || value; }
                const shown = doc.getElementById(id + "_display");
                if (shown) shown.textContent = label;
                if (marker.hasAttribute("data-hidden")) { const hidden = doc.getElementById(id); if (hidden) hidden.value = value; }
            });
        };
        onDepends(marker, fill);
        fill();
    };

    // ---------- tree ----------

    const drawTree = (host, nodes) => {
        const list = el("ul");
        nodes.forEach((node) => {
            const item = el("li", node.children === false ? { class: "leaf" } : {});
            item.append(el("span", { class: "toggle" }), node.href ? el("a", { href: node.href }, node.text) : el("span", {}, node.text));
            list.appendChild(item);
            // A node opens from its own children, or by asking the server for them the first time.
            const expand = () => {
                if (item.querySelector(":scope > ul")) { item.classList.toggle("open"); return Promise.resolve(); }
                const show = (children) => { item.appendChild(drawTree(host, children)); item.classList.add("open"); };
                if (Array.isArray(node.children)) { show(node.children); return Promise.resolve(); }
                return fetchTree(host, node).then(show);
            };
            if (node.children !== false) item.firstChild.addEventListener("click", expand);
            if (node.opened && node.children !== false) expand();
        });
        return list;
    };
    const fetchTree = (host, parent) => postJson(host.dataset.url, Object.assign({}, parseJson(host.dataset.params, {}), {
        treeNodeId: parent ? parent.id : "#", treeNodeName: parent && parent.name ? parent.name : ""
    })).then(treeNodes);
    const initTree = (host) => {
        fetchTree(host, null).then((nodes) => host.replaceChildren(drawTree(host, nodes))).catch((err) => notify(String(err.message || err), "danger"));
    };

    // ---------- click to edit ----------

    const initEditable = (node) => {
        node.addEventListener("click", () => {
            if (node.querySelector("form")) return;
            const originalHtml = node.innerHTML;
            const original = node.dataset.empty === "true" ? "" : node.textContent;
            const widget = node.dataset.widget;
            const restore = () => { node.innerHTML = originalHtml; };
            const render = (value, options) => {
                const form = el("form", { class: "editable-form" });
                let input;
                if (widget === "select") {
                    input = el("select");
                    (options || []).forEach((option) => input.append(el("option", { value: option.value }, option.label)));
                } else if (widget === "date" || widget === "date-time") {
                    input = el("input", { type: widget === "date" ? "date" : "datetime-local" });
                    value = widget === "date-time" ? String(value || "").replace(" ", "T") : value;
                } else input = widget === "text" || widget === "text-line" ? el("input", { type: "text" }) : el("textarea", { rows: "3" });
                input.value = value;
                form.append(input, el("button", { type: "submit", "data-btn": "primary" }, "Save"), el("button", { type: "button" }, "Cancel"));
                node.replaceChildren(form);
                input.focus();
                form.lastChild.addEventListener("click", (event) => { event.stopPropagation(); restore(); });
                form.addEventListener("submit", (event) => {
                    event.preventDefault();
                    const params = Object.assign({}, parseJson(node.dataset.params, {}), { parameterName: node.dataset.name });
                    const sent = widget === "date-time" ? input.value.replace("T", " ") : input.value;
                    params[node.dataset.name] = sent;
                    const shown = widget === "select" && input.selectedIndex >= 0 ? input.options[input.selectedIndex].text : sent;
                    node.textContent = "...";
                    post(node.dataset.url, params).then((response) => {
                        if (!response.ok) throw new Error(response.status + " " + response.statusText);
                        node.textContent = shown;
                        node.dataset.value = sent;
                        delete node.dataset.empty;
                    }).catch((err) => { restore(); notify(String(err.message || err), "danger"); });
                });
            };
            if (widget === "select") {
                const given = parseJson(node.dataset.options, []);
                const current = node.dataset.value !== undefined ? node.dataset.value : ((given.find((option) => option.label === original) || {}).value ?? original);
                if (node.dataset.loadUrl) {
                    post(node.dataset.loadUrl, Object.assign({}, parseJson(node.dataset.loadParams, {}), { currentValue: current })).then((r) => r.json())
                        .then((answer) => { const loaded = editableOptions(answer); render(loaded.selected !== undefined ? String(loaded.selected) : current, loaded.options); })
                        .catch((err) => notify(String(err.message || err), "danger"));
                } else render(current, given);
            } else if (node.dataset.loadUrl) post(node.dataset.loadUrl, Object.assign({}, parseJson(node.dataset.loadParams, {}), { currentValue: original })).then((r) => r.text()).then((text) => render(text));
            else render(original);
        });
    };

    // ---------- select columns ----------

    const initSortableTree = (list) => {
        const target = doc.getElementById(list.dataset.sortableTree);
        const read = (ul) => Array.from(ul.children).map((item) => ({ id: item.id, children: read(item.querySelector(":scope > ul") || el("ul")) }));
        const write = () => { target.value = JSON.stringify(read(list)); };
        withSortable().then((Sortable) => {
            [list].concat($$("ul", list)).forEach((group) => new Sortable(group, { group: "columns", animation: 120, onSort: write }));
            write();
        });
    };

    // ---------- dialogs, tabs, menus ----------

    const initDialog = (dialog) => {
        dialog.addEventListener("click", (event) => {
            if (event.target !== dialog) return;
            const rect = dialog.getBoundingClientRect();
            // A native dialog also receives clicks on its own padding. Only the
            // backdrop outside its border should dismiss it, as Java does.
            if (event.clientX < rect.left || event.clientX > rect.right ||
                event.clientY < rect.top || event.clientY > rect.bottom) dialog.close();
        });
        if (dialog.dataset.load) {
            dialog.addEventListener("close", () => {
                const body = $(".body", dialog);
                body.replaceChildren(el("span", { class: "loading" }, "Loading..."));
                delete dialog.dataset.loaded;
            });
        }
        if (dialog.hasAttribute("data-open")) openDialog(dialog);
    };
    const openDialog = (dialog) => {
        if (dialog.open) return;
        dialog.showModal();
        // The search input Tom Select adds opens its menu on focus, over the fields below it, so the first
        // focus goes to a field of the form instead.
        const focusField = (selector, scope) => {
            const focus = $$(selector, scope).find((node) => node.getClientRects().length && !node.closest(".ts-wrapper") && !node.matches(".ts-hidden-accessible"));
            if (focus) focus.focus();
        };
        if (dialog.dataset.load && !dialog.dataset.loaded) {
            dialog.dataset.loaded = "1";
            const body = $(".body", dialog);
            load(body, dialog.dataset.load).then(() => focusField("[autofocus], input:not([type=hidden]), select, textarea", body));
        } else {
            focusField(".default-focus, input:not([type=hidden]):not([type=checkbox])", dialog);
        }
    };

    const initTabsDynamic = (nav) => {
        nav.addEventListener("click", (event) => {
            const link = event.target.closest("a[data-load-url]");
            if (!link) return;
            event.preventDefault();
            $$("li", nav).forEach((item) => item.classList.remove("active"));
            link.parentNode.classList.add("active");
            load(doc.getElementById(nav.dataset.dynamic), link.dataset.loadUrl);
        });
    };

    // Login: the panels follow the tabs and the URL hash.
    const initHashTabs = (nav) => {
        const panels = $$(".tab-panel");
        const links = $$("a[href^='#']", nav);
        const initial = (panels.find((panel) => panel.dataset.initialTab) || { dataset: {} }).dataset.initialTab || "login";
        const show = () => {
            const name = (root.location.hash || "#" + initial).replace(/^#+/, "") || initial;
            const known = panels.some((panel) => panel.id === name) ? name : "login";
            panels.forEach((panel) => panel.classList.toggle("active", panel.id === known));
            links.forEach((link) => link.parentNode.classList.toggle("active", link.getAttribute("href") === "#" + known));
            $$(".initial-tab").forEach((input) => { input.value = known; });
            const focus = $("#" + known + " input:not([type=hidden]):not([disabled])");
            if (focus) focus.focus();
        };
        root.addEventListener("hashchange", show);
        show();
    };

    // Content that belongs in another place on the page, such as a menu for the navbar.
    const initMove = (node) => {
        const target = doc.getElementById(node.dataset.moveTo);
        if (!target) return;
        if (target.tagName === "UL" || target.tagName === "OL") {
            const item = el("li");
            item.appendChild(node);
            target.appendChild(item);
        } else target.appendChild(node);
    };

    const setTheme = (dark) => {
        doc.documentElement.dataset.theme = dark ? "dark" : "light";
    };

    // ---------- the whole page ----------

    function bind(scope) {
        scope = scope || doc;
        each(scope, "form", initForm);
        each(scope, "dialog", initDialog);
        each(scope, "[data-load]:not(dialog)", (node) => load(node));
        each(scope, "select[data-ts]", initSelect);
        each(scope, "input[data-default]", initDefault);
        each(scope, "[data-dynamic]:not(nav)", initDynamicDisplay);
        each(scope, ".tree", initTree);
        each(scope, ".editable", initEditable);
        each(scope, "[data-sortable-tree]", initSortableTree);
        each(scope, "nav.tabs[data-dynamic]", initTabsDynamic);
        each(scope, "nav[data-tab-hash]", initHashTabs);
        each(scope, "[data-move-to]", initMove);
        each(scope, "[data-rich-text]", initRichText);
        each(scope, "[data-nav-toggle]", (button) => button.addEventListener("click", () => button.closest(".navbar").classList.toggle("open")));
    }

    // One listener on the document handles clicks on every element the macros write.
    // The keys of a date, date-time or time field (dateKey). A native input keeps its own keys: the arrows step the part
    // under the caret and Backspace clears it, so it only gets the letter keys. A text field with a date pattern
    // (data-format) also gets ArrowUp and ArrowDown, with Ctrl too, for a day later or earlier.
    const onDateKey = (event) => {
        const input = event.target;
        if (!input || input.tagName !== "INPUT" || input.readOnly || input.disabled || event.altKey || event.metaKey) return;
        const seconds = /\d:\d\d:\d\d/.test(input.value) ? ":ss" : "";
        const native = { date: "yyyy-MM-dd", "datetime-local": "yyyy-MM-dd'T'HH:mm" + seconds, time: "HH:mm" + seconds }[input.type];
        const pattern = native || (input.type === "text" && input.dataset.format);
        if (!pattern) return;
        const arrow = event.key === "ArrowUp" || event.key === "ArrowDown";
        if (arrow ? native || event.shiftKey : event.key.length !== 1 || event.ctrlKey) return;
        const step = input.step ? Number(input.step) / 60 : Number(input.dataset.step || 5);
        const next = dateKey(pattern, input.value, event.key, event.shiftKey, step);
        if (next === null) return;
        event.preventDefault();
        input.value = next;
        input.dispatchEvent(new Event("input", { bubbles: true }));
        input.dispatchEvent(new Event("change", { bubbles: true }));
    };

    const delegate = () => {
        doc.addEventListener("keydown", onDateKey);
        doc.addEventListener("change", (event) => {
            const target = event.target;
            // a plain select with the style submit-on-close submits its form when it changes (a Tom Select one when its menu closes)
            if (target && target.tagName === "SELECT" && target.classList.contains("submit-on-close") && !target.classList.contains("tomselected") && target.form) {
                target.form.requestSubmit();
            }
            // the quick saved-find select goes to the find it names
            const select = target.closest && target.closest("select[data-saved-finds]");
            const option = select && select.selectedOptions[0];
            if (option && option.dataset.url) root.location.assign(option.dataset.url);
        });
        doc.addEventListener("click", (event) => {
            const target = event.target;
            if (!target.closest) return;
            const confirmed = target.closest("[data-confirm]");
            if (confirmed && !root.confirm(confirmed.dataset.confirm)) { event.preventDefault(); event.stopImmediatePropagation(); return; }
            const opener = target.closest("[data-dialog]");
            if (opener) { const dialog = doc.getElementById(opener.dataset.dialog); if (dialog) openDialog(dialog); return; }
            const copier = target.closest("[data-copy-url]");
            if (copier) { event.preventDefault(); copyUrl(copier); return; }
            const closer = target.closest("[data-close]");
            if (closer) { closer.closest("dialog").close(); return; }
            const dismiss = target.closest("[data-dismiss]");
            if (dismiss) { dismiss.closest(".alert").remove(); return; }
            const into = target.closest("a[data-load-into]");
            if (into) { event.preventDefault(); load(doc.getElementById(into.dataset.loadInto), into.href); return; }
            const clear = target.closest("[data-clear]");
            if (clear) { clearForm(doc.getElementById(clear.dataset.clear)); return; }
            const back = target.closest("[data-history-back]");
            if (back) { event.preventDefault(); root.history.back(); return; }
            const toggle = target.closest("[data-theme-toggle]");
            if (toggle) {
                const dark = !(doc.documentElement.dataset.theme === "dark" || (!doc.documentElement.dataset.theme && root.matchMedia("(prefers-color-scheme: dark)").matches));
                setTheme(dark);
                if (toggle.dataset.prefUrl) post(toggle.dataset.prefUrl, { preferenceKey: "OUTER_STYLE", preferenceValue: dark ? "bg-dark" : "bg-light" }).catch(() => {});
                return;
            }
            // a menu closes when the click is outside it
            $$("details.menu[open]").forEach((menu) => { if (!menu.contains(target)) menu.removeAttribute("open"); });
        });
        // A menu in a table that scrolls sideways opens fixed at its button, so the table does not clip it, and closes when
        // the page or the table scrolls under it.
        doc.addEventListener("toggle", (event) => {
            const menu = event.target;
            if (!menu.matches || !menu.matches("details.menu") || !menu.open || !menu.closest(".table-wrap")) return;
            const list = menu.querySelector(":scope > ul");
            const at = menu.querySelector(":scope > summary").getBoundingClientRect();
            if (list) Object.assign(list.style, { position: "fixed", top: at.bottom + 2 + "px", left: at.left + "px" });
        }, true);
        doc.addEventListener("scroll", (event) => {
            $$(".table-wrap details.menu[open]").forEach((menu) => { if (!menu.contains(event.target)) menu.removeAttribute("open"); });
        }, { capture: true, passive: true });
    };

    // A pushed notification with a title and showAlert true is a toast, as the qvt client shows it; its link is the toast's link.
    const showNotification = (data) => {
        if (!data || !data.title || data.showAlert !== true) return;
        notify(data.title, ["success", "warning", "danger"].indexOf(data.type) >= 0 ? data.type : "info");
        const toast = $("#toasts") && $("#toasts").lastElementChild;
        if (toast && data.link) {
            const text = toast.lastChild;
            const link = el("a", { href: data.link }, data.title);
            if (text && text.nodeType === 3) toast.replaceChild(link, text);
        }
    };

    // The notification socket (/notws by default) opens when the page names an endpoint, which the server does only for a signed in
    // user. It subscribes to all of the user's topics. A closed socket opens again after a growing delay, up to ten times in a row.
    const connectNotifications = () => {
        const meta = $("meta[name=moqui-notify]");
        if (!meta || !meta.content || !root.WebSocket) return;
        let delay = 2000;
        let tries = 0;
        const open = () => {
            const socket = new root.WebSocket((root.location.protocol === "https:" ? "wss://" : "ws://") + root.location.host + meta.content);
            root.moqui.notificationSocket = socket;
            socket.onopen = () => { delay = 2000; tries = 0; socket.send("subscribe:ALL"); };
            socket.onmessage = (event) => showNotification(parseJson(event.data, null));
            socket.onclose = () => {
                if (tries >= 10) return;
                tries++;
                root.setTimeout(open, delay);
                delay = Math.min(delay * 2, 60000);
            };
        };
        open();
    };

    // Inline screen templates call this helper for chart CSV exports.
    const downloadData = (data, filename, type) => {
        const url = root.URL.createObjectURL(new root.Blob([data], {type}));
        const link = doc.createElement("a");
        link.href = url;
        link.download = filename;
        doc.body.appendChild(link);
        link.click();
        root.setTimeout(() => {link.remove();root.URL.revokeObjectURL(url);},0);
    };

    // The page's header lists each validation error the last submit left; the line for one a form shows by its field goes,
    // so the line stays only for an error whose field is not on the page.
    const dropPlacedErrors = () => {
        $$(".alert[data-field]").forEach((alert) => {
            const shown = $$("form").some((form) => {
                const formName = form.elements.moquiFormName ? form.elements.moquiFormName.value : null;
                if (alert.dataset.form && formName && alert.dataset.form !== formName) return false;
                const input = form.elements[alert.dataset.field];
                const target = input && (input.length !== undefined && !input.tagName ? input[0] : input);
                return !!(target && target.closest && target.closest(".field[data-invalid]"));
            });
            if (shown) alert.remove();
        });
    };

    const start = () => {
        doc.documentElement.classList.add("js");
        dropPlacedErrors();
        delegate();
        bind(doc);
        connectNotifications();
    };

    return { pure, client: { bind, notify, load, request, post, start, downloadData } };
});
