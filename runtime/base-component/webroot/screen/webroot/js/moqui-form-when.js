/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * visible-when and require-when for the html render mode, and for any page that writes the same attributes (a PASS
 * page under CSP script-src 'self'): a self-hosted file with no dependencies, no inline script and no eval. The html
 * macros (ScreenHtml.js fieldOpen) write data-visible-when and data-require-when on a field's box as JSON:
 * {form, field, operator, values}. While the named field of the form meets the operator the box shows (visible-when)
 * or its inputs are required (require-when); a hidden box's inputs are never required, and a hidden field counts as
 * empty for the fields that depend on it. The server checks the same rules again (docs/ui/Db-Forms.md).
 */
(function (root, factory) {
    const api = factory(root);
    if (typeof module !== "undefined" && module.exports) module.exports = api.pure;
    root.moquiFormWhen = api;
    if (root.document) {
        if (root.document.readyState === "loading") root.document.addEventListener("DOMContentLoaded", api.start, { once: true });
        else api.start();
    }
})(typeof window !== "undefined" ? window : globalThis, function (root) {
    "use strict";

    // ---------- pure functions ----------

    // Whether a rule holds for a field's values (a list: a multiple select or checks have several). equals: any value is
    // one of the expected; not-equals: none is; empty, not-empty. The rule of xml-form-3.xsd WhenAttributes (DbForms.js
    // holds).
    const holds = (operator, expected, actual) => {
        const parts = (actual || []).map(String).filter((part) => part !== "");
        if (operator === "empty") return parts.length === 0;
        if (operator === "not-empty") return parts.length > 0;
        const wanted = (expected || []).map((part) => String(part).trim());
        const any = parts.some((part) => wanted.indexOf(part) >= 0);
        return operator === "not-equals" ? !any : any;
    };

    const parseRule = (text) => {
        if (!text) return null;
        try {
            const rule = JSON.parse(text);
            return rule && typeof rule.field === "string" ? rule : null;
        } catch (err) {
            return null;
        }
    };

    // ---------- the page ----------

    const doc = root.document;

    // The values of the field `name` of a form: the checked ones of radios and checks, the selected of a select.
    const valuesOf = (form, name) => {
        const found = form && form.elements ? form.elements.namedItem(name) : null;
        if (!found) return [];
        const list = typeof found.length === "number" && !found.tagName ? Array.from(found) : [found];
        const out = [];
        list.forEach((el) => {
            if (el.type === "checkbox" || el.type === "radio") {
                if (el.checked) out.push(el.value);
            } else if (el.tagName === "SELECT") {
                Array.from(el.selectedOptions || []).forEach((option) => out.push(option.value));
            } else {
                out.push(el.value);
            }
        });
        return out;
    };

    // The inputs of a box whose required flag a rule sets; a check stays as it is (each check would be required).
    const inputsOf = (box) => Array.from(box.querySelectorAll("input, select, textarea"))
        .filter((el) => ["hidden", "submit", "button", "reset", "image", "checkbox"].indexOf(el.type) < 0);

    let boxes = [];

    // The box of the field a rule names, when the page has one, to tell whether it is hidden.
    const boxOfField = (form, name) => {
        const found = form && form.elements ? form.elements.namedItem(name) : null;
        const el = found && typeof found.length === "number" && !found.tagName ? found[0] : found;
        return el && el.closest ? el.closest(".field") : null;
    };

    const ruleHolds = (rule) => {
        if (!rule) return true;
        const form = doc.getElementById(rule.form);
        const source = boxOfField(form, rule.field);
        const values = source && source.hidden ? [] : valuesOf(form, rule.field);
        return holds(rule.operator, rule.values, values);
    };

    // One pass over the boxes; true when a box changed, since a field shown or hidden can change the fields after it.
    const applyOnce = () => {
        let changed = false;
        boxes.forEach((entry) => {
            const shown = entry.visible ? ruleHolds(entry.visible) : true;
            if (entry.box.hidden !== !shown) {
                entry.box.hidden = !shown;
                changed = true;
            }
            const required = shown && (entry.require ? ruleHolds(entry.require) : false);
            inputsOf(entry.box).forEach((el) => {
                if (el.dataset.mqBaseRequired === undefined) el.dataset.mqBaseRequired = el.required ? "1" : "0";
                const want = shown && (el.dataset.mqBaseRequired === "1" || required);
                if (el.required !== want) el.required = want;
            });
            if (entry.require) entry.box.toggleAttribute("data-required", required);
        });
        return changed;
    };

    // The boxes not bound yet: those of the page, and of a dialog or container filled after it loaded. A box no longer
    // on the page (its dialog filled again) is let go.
    const scan = () => {
        boxes = boxes.filter((entry) => entry.box.isConnected);
        doc.querySelectorAll("[data-visible-when], [data-require-when]").forEach((box) => {
            if (box.dataset.mqWhenBound) return;
            box.dataset.mqWhenBound = "1";
            boxes.push({ box, visible: parseRule(box.dataset.visibleWhen), require: parseRule(box.dataset.requireWhen) });
        });
    };

    const apply = () => {
        if (!doc) return;
        scan();
        for (let pass = 0; pass <= boxes.length && applyOnce(); pass++);
    };

    // A field may be outside its form (its form attribute names it), so the changes are heard on the document.
    const start = () => {
        if (!doc || doc.documentElement.dataset.mqWhenListening) return;
        doc.documentElement.dataset.mqWhenListening = "1";
        doc.addEventListener("change", apply);
        doc.addEventListener("input", apply);
        apply();
    };

    return { pure: { holds, parseRule }, start, apply };
});
