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

// The qjs and qvue modes, which are companions of qvt: a screen answers them with the text of its own
// render-mode elements and nothing else. This follows DefaultScreenMacros.plain.ftl. Widgets, forms and
// containers write nothing; the subscreen and section calls are kept so text inside a section still shows.
// qjs is the JavaScript object of a component (its data, methods and a template when it has one) and qvue is
// a component file with template, script and style parts. The client reads both
// (docs/Qvt-Client.md, loadComponent).
//
// The module serves two modes, so the mode a unit is made for is read when a macro runs: a text type list
// matches the mode, and "any" or no type matches both.
(function (T, S) {
    "use strict";

    const { attr, jsString, childrenNamed, stmt, noops } = T;

    const mode = () => T.options.mode || "qjs";
    const typeMatches = (type) => type.split(",").map((name) => name.trim()).includes(mode());

    const textFor = (node) => {
        const type = attr(node, "type");
        return !type || type === "any" || typeMatches(type) ? S.renderTextCode(node) : "";
    };

    const macros = Object.assign({}, noops(S.allElements), {
        "screen": stmt((node, children) => children),
        "widgets": stmt((node, children) => children),
        "fail-widgets": stmt((node, children) => children),
        "include-screen": stmt((node) =>
            `sri.renderIncludeScreen(${jsString(attr(node, "location"))}, ${jsString(attr(node, "share-scope"))});`),
        "render-mode": stmt((node) => {
            const texts = childrenNamed(node, "text");
            let chosen = null;
            texts.forEach((kid) => { if (!attr(kid, "type") || attr(kid, "type") === "any") chosen = kid; });
            texts.forEach((kid) => { if (attr(kid, "type") && typeMatches(attr(kid, "type"))) chosen = kid; });
            return chosen ? S.renderTextCode(chosen) : "";
        }),
        "text": stmt((node) => textFor(node)),
        "subscreens-active": stmt(() => "sri.renderSubscreen();"),
        "subscreens-panel": stmt(() => "sri.renderSubscreen();"),
        "section": stmt((node) => `sri.renderSection(${jsString(attr(node, "name"))});`),
        "section-iterate": stmt((node) => `sri.renderSection(${jsString(attr(node, "name"))});`),
        "section-include": stmt((node) => `sri.renderSectionInclude(${S.bareLit(node)});`),
        "render-dynamic-form": stmt(() => ""),
        "form-list-user-columns": stmt(() => "")
    });

    return { macros, opaque: S.opaqueElements, prelude: "" };
})
