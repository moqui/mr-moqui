/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// Every Dev AIDE component's template compiles to a render function whose code parses. An expression that does not
// parse (a \' in a template literal is a bare ' in the template) fails the whole component when it first renders, which
// for a dialog may be long after the page loaded: a commit's Checks dialog and the ECA view's Try once failed so.
"use strict";

const compiler = globalThis.VueCompilerDOM;

test("each Dev AIDE component's template compiles, its expressions too", () => {
    const devAide = moqui.devAide;
    const all = Object.assign({}, moqui.lateComponents, devAide.components, devAide.gitComponents, devAide.browserComponents);
    const names = Object.keys(all).filter((name) => typeof all[name].template === "string");
    for (const name of ["m-dev-workspace", "m-dev-git-checks", "m-dev-eca-view", "m-dev-service-view", "m-dev-entity-view",
        "m-dev-screen-view", "m-dev-qvue-view", "m-dev-qvue-xml", "m-dev-artifact-browser", "m-dev-composer"]) {
        assert.ok(names.indexOf(name) >= 0, name + " among " + names.join(", "));
    }
    const failed = [];
    for (const name of names) {
        const errors = [];
        const made = compiler.compile(all[name].template, { mode: "function", onError: (err) => errors.push(err.message) });
        if (!errors.length) {
            try {
                new Function(made.code);
            } catch (err) {
                errors.push(String(err.message));
            }
        }
        if (errors.length) failed.push(name + ": " + errors.join("; "));
    }
    assert.deepEqual(failed, []);
});
