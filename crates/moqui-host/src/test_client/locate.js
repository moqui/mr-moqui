// The page side of moqui test-client's browser (browser.rs, plans/archive/ClientTestHarness.md 5.5), evaluated in each new
// document before the page's own scripts: window.__moquiTest with the fetches in flight, the error notifications seen,
// whether the Moqui client is busy, and the locators. A locator is a chain of steps, each applied to what the one before
// found (the document at first): { css }, { role, name, exact }, { text, exact }, { label, exact } or { nth } (negative
// from the end). A name or text is { s } (a substring ignoring case and spacing, or the whole text with exact) or
// { re, flags }.
(function () {
    if (window.__moquiTest) return;
    const T = { inflight: 0, errors: [] };
    Object.defineProperty(window, "__moquiTest", { value: T, configurable: true });

    // fetch and XMLHttpRequest in flight, as gate3_browser's FETCH_SPY counts them
    if (window.fetch) {
        const original = window.fetch.bind(window);
        window.fetch = function (input, init) {
            T.inflight++;
            let done = false;
            const end = () => { if (!done) { done = true; T.inflight--; } };
            try {
                return original(input, init).then((response) => { end(); return response; }, (err) => { end(); throw err; });
            } catch (err) {
                end();
                throw err;
            }
        };
    }
    if (window.XMLHttpRequest) {
        const send = XMLHttpRequest.prototype.send;
        XMLHttpRequest.prototype.send = function () {
            T.inflight++;
            let done = false;
            this.addEventListener("loadend", () => { if (!done) { done = true; T.inflight--; } });
            return send.apply(this, arguments);
        };
    }

    // the client's error notifications: Quasar's negative ones in the shell, the danger alerts of moqui-html
    const NOTICE = ".q-notification.bg-negative, .alert[data-type=danger], .alert[data-type=error]";
    const seen = new WeakSet();
    const notice = (el) => {
        if (seen.has(el)) return;
        seen.add(el);
        setTimeout(() => {
            const text = clean(el.innerText || el.textContent).replace(/^×\s*/, "").replace(/\s*close$/i, "");
            if (text) T.errors.push(text);
        }, 0);
    };
    const scan = (node) => {
        if (!node || node.nodeType !== 1) return;
        if (node.matches(NOTICE)) notice(node);
        node.querySelectorAll(NOTICE).forEach(notice);
    };
    new MutationObserver((records) => {
        for (const record of records) {
            if (record.type === "attributes") scan(record.target);
            else record.addedNodes.forEach(scan);
        }
    }).observe(document, { childList: true, subtree: true, attributes: true, attributeFilter: ["class"] });

    // What keeps the page from being settled, or "" when it is idle: the document loading, a load of the Moqui client,
    // a loading bar or inner loading, or a fetch in flight. A page that is not the Moqui client has only the last.
    T.busy = () => {
        if (document.readyState !== "complete") return "document";
        const root = window.moqui && window.moqui.webrootVue;
        if (root && (root.loading === true || root.loading > 0)) return "moqui";
        if (document.querySelector(".q-loading-bar--visible, .q-inner-loading")) return "loading";
        if (T.inflight > 0) return "fetch";
        return "";
    };
    T.takeErrors = () => T.errors.splice(0, T.errors.length);

    const clean = (text) => String(text == null ? "" : text).replace(/\s+/g, " ").trim();
    const matches = (matcher, text, exact) => {
        const value = clean(text);
        if (matcher.re != null) return new RegExp(matcher.re, matcher.flags || "").test(value);
        const want = clean(matcher.s);
        return exact ? value === want : value.toLowerCase().includes(want.toLowerCase());
    };
    const visible = (el) => {
        if (!el || !el.isConnected) return false;
        const style = getComputedStyle(el);
        if (style.visibility === "hidden" || style.visibility === "collapse") return false;
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
    };
    const hiddenFromTree = (el) => {
        for (let at = el; at && at.nodeType === 1; at = at.parentElement) {
            if (at.getAttribute("aria-hidden") === "true" || at.hidden) return true;
        }
        return !visible(el);
    };

    const INPUT_ROLES = { button: "button", submit: "button", reset: "button", image: "button", checkbox: "checkbox", radio: "radio",
        number: "spinbutton", range: "slider", search: "searchbox", text: "textbox", email: "textbox", tel: "textbox", url: "textbox",
        password: "textbox", "": "textbox" };
    const TAG_ROLES = { BUTTON: "button", TEXTAREA: "textbox", DIALOG: "dialog", H1: "heading", H2: "heading", H3: "heading",
        H4: "heading", H5: "heading", H6: "heading", UL: "list", OL: "list", LI: "listitem", TABLE: "table", TR: "row", TD: "cell",
        TH: "columnheader", NAV: "navigation", MAIN: "main", IMG: "img", OPTION: "option", PROGRESS: "progressbar" };
    // The role of an element: its role attribute, else the one its tag implies.
    const roleOf = (el) => {
        const explicit = clean(el.getAttribute("role")).split(" ")[0];
        if (explicit) return explicit;
        const tag = el.tagName;
        if (tag === "A" || tag === "AREA") return el.hasAttribute("href") ? "link" : "";
        if (tag === "INPUT") {
            const type = (el.getAttribute("type") || "").toLowerCase();
            return INPUT_ROLES[type] === undefined ? "" : INPUT_ROLES[type];
        }
        if (tag === "SELECT") return el.multiple || el.size > 1 ? "listbox" : "combobox";
        if (el.isContentEditable && el.getAttribute("contenteditable") !== null) return "textbox";
        return TAG_ROLES[tag] || "";
    };
    // The text of an element as its accessible name has it: what is not hidden from the tree, with a space around
    // what is not inline. CSS text-transform does not change it, as innerText would (Quasar buttons are upper case).
    const contentText = (el) => {
        let out = "";
        for (const node of el.childNodes) {
            if (node.nodeType === 3) out += node.data;
            else if (node.nodeType === 1) {
                if (node.getAttribute("aria-hidden") === "true" || /^(SCRIPT|STYLE|TEMPLATE|NOSCRIPT)$/.test(node.tagName)) continue;
                const style = getComputedStyle(node);
                if (style.display === "none" || style.visibility === "hidden") continue;
                const inline = style.display === "inline";
                const own = node.getAttribute("aria-label") || (node.tagName === "IMG" ? node.getAttribute("alt") : null);
                const text = own != null ? own : contentText(node);
                out += inline ? text : ` ${text} `;
            }
        }
        return clean(out);
    };
    const labelsText = (el) => Array.from(el.labels || []).map(contentText).join(" ");
    // The accessible name: aria-label, then aria-labelledby, then for a field its labels, placeholder or title, for an
    // input button its value, else the text.
    const nameOf = (el) => {
        const label = el.getAttribute("aria-label");
        if (label && clean(label)) return clean(label);
        const by = el.getAttribute("aria-labelledby");
        if (by) {
            const text = by.split(/\s+/).map((id) => document.getElementById(id)).filter(Boolean).map(contentText).join(" ");
            if (clean(text)) return clean(text);
        }
        const tag = el.tagName;
        if (tag === "INPUT" && /^(button|submit|reset)$/i.test(el.type)) return clean(el.value || (el.type === "submit" ? "Submit" : ""));
        if (tag === "INPUT" && el.type === "image") return clean(el.alt || el.value);
        if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") {
            return clean(labelsText(el) || el.getAttribute("placeholder") || el.getAttribute("title") || "");
        }
        if (tag === "IMG") return clean(el.getAttribute("alt") || el.getAttribute("title") || "");
        return clean(contentText(el) || el.getAttribute("title") || "");
    };

    // The elements one step finds within root, in document order.
    const within = (root, step) => {
        const all = (selector) => Array.from(root.querySelectorAll(selector));
        if (step.css != null) return all(step.css);
        if (step.role != null) {
            return all("*").filter((el) => roleOf(el) === step.role && !hiddenFromTree(el)
                && (step.name == null || matches(step.name, nameOf(el), step.exact)));
        }
        if (step.text != null) {
            const found = all("*").filter((el) => !/^(SCRIPT|STYLE|TEMPLATE|NOSCRIPT|HEAD|TITLE|HTML|BODY)$/.test(el.tagName)
                && matches(step.text, el.textContent, step.exact));
            // the deepest: an element whose child matches too is left for the child
            return found.filter((el) => !found.some((other) => other !== el && el.contains(other)) && visible(el));
        }
        if (step.label != null) {
            const out = [];
            for (const label of all("label")) {
                if (label.control && matches(step.label, contentText(label), step.exact)) out.push(label.control);
            }
            for (const el of all("[aria-label], [aria-labelledby]")) {
                if (matches(step.label, nameOf(el), step.exact)) out.push(el);
            }
            for (const field of all(".q-field")) {
                const label = field.querySelector(".q-field__label");
                const control = field.querySelector("input, textarea, select, [contenteditable]");
                if (label && control && matches(step.label, contentText(label), step.exact)) out.push(control);
            }
            return out;
        }
        return [];
    };
    // The elements a chain finds, in document order without repeats.
    T.locate = (chain) => {
        let found = [document];
        for (const step of chain) {
            if (step.nth != null) {
                const at = step.nth < 0 ? found.length + step.nth : step.nth;
                found = at >= 0 && at < found.length ? [found[at]] : [];
                continue;
            }
            const next = new Set();
            for (const root of found) within(root, step).forEach((el) => next.add(el));
            found = Array.from(next);
            if (found.length > 1) found.sort((a, b) => (a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING ? -1 : 1));
        }
        return found.filter((el) => el !== document);
    };
    const editable = (el) => {
        if (el.isContentEditable) return true;
        if (!/^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName)) return false;
        return !el.disabled && !el.readOnly && !/^(button|submit|reset|image|checkbox|radio|hidden|file)$/i.test(el.type || "");
    };
    const enabled = (el) => {
        for (let at = el; at && at.nodeType === 1; at = at.parentElement) {
            if (at.disabled === true || at.getAttribute("aria-disabled") === "true") return false;
        }
        return !el.classList.contains("disabled");
    };
    const box = (el) => {
        const rect = el.getBoundingClientRect();
        return { x: rect.left, y: rect.top, width: rect.width, height: rect.height };
    };
    // What an action needs of the first element a chain finds: whether it is there, visible, enabled and editable,
    // its box after it is scrolled into view, and whether a click at its middle reaches it.
    T.actionable = (chain, scroll) => {
        const all = T.locate(chain);
        const el = all[0];
        if (!el) return { count: 0 };
        const shown = visible(el);
        if (shown && scroll) el.scrollIntoView({ block: "center", inline: "center" });
        const b = box(el);
        let hit = null;
        if (shown) {
            const target = document.elementFromPoint(b.x + b.width / 2, b.y + b.height / 2);
            hit = !!target && (el.contains(target) || (target.tagName === "LABEL" && target.control === el)
                || (el.tagName === "LABEL" && el.control === target));
            if (!hit && target) hit = describe(target);
        }
        return { count: all.length, visible: shown, enabled: enabled(el), editable: editable(el), box: b, hit };
    };
    const describe = (el) => {
        const id = el.id ? `#${el.id}` : "";
        const cls = typeof el.className === "string" && el.className.trim() ? "." + el.className.trim().split(/\s+/).slice(0, 3).join(".") : "";
        return `<${el.tagName.toLowerCase()}${id}${cls}>`;
    };
    // A read of the first element (or every one, for count): text, value, attribute, visible, enabled, editable,
    // box or describe.
    T.read = (chain, what, arg) => {
        const all = T.locate(chain);
        if (what === "count") return all.length;
        const el = all[0];
        if (what === "visible") return !!el && visible(el);
        if (!el) return { missing: true };
        switch (what) {
            case "text": return el.innerText != null ? el.innerText : el.textContent;
            case "value": return el.value != null ? String(el.value) : null;
            case "attribute": return el.getAttribute(arg);
            case "enabled": return enabled(el);
            case "editable": return editable(el) && enabled(el);
            case "box": return visible(el) ? box(el) : null;
            case "describe": return describe(el);
            default: return null;
        }
    };
    // Get the first element ready to take typing: focused, its text selected.
    T.focusForFill = (chain) => {
        const el = T.locate(chain)[0];
        if (!el) return false;
        el.focus();
        if (el.isContentEditable) {
            const range = document.createRange();
            range.selectNodeContents(el);
            const selection = getSelection();
            selection.removeAllRanges();
            selection.addRange(range);
        } else if (typeof el.select === "function") {
            el.select();
        }
        return document.activeElement === el || el.contains(document.activeElement);
    };
    // An empty fill: the value cleared with the events a user's deleting sends.
    T.clear = (chain) => {
        const el = T.locate(chain)[0];
        if (!el) return false;
        if (el.isContentEditable) el.textContent = "";
        else el.value = "";
        el.dispatchEvent(new Event("input", { bubbles: true }));
        el.dispatchEvent(new Event("change", { bubbles: true }));
        return true;
    };
    T.focus = (chain) => {
        const el = T.locate(chain)[0];
        if (!el) return false;
        el.focus();
        return true;
    };
    T.element = (chain) => T.locate(chain)[0] || null;
    T.describe = describe;
})();
