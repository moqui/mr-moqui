/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// Loaded before the Dev AIDE's scripts in DevAide.spec.js: a qvt that keeps each component a script registers
// (qvt.registerLate), for DevAideTemplates.page.js to compile.
"use strict";

(function (root) {
    const moqui = root.moqui || (root.moqui = {});
    const late = (moqui.lateComponents = {});
    moqui.qvt = { registerLate: (name, def) => { late[name] = def; return def; } };
})(window);
