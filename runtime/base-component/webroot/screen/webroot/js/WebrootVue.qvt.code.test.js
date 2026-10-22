/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// Tests of m-code's helpers (moqui.code in WebrootVue.qvt.js) on the vendored highlight.js, with no browser: the language of a file
// name, the marked lines, and highlighted HTML split into lines that each close and reopen their spans.
// Run: node --test runtime/base-component/webroot/screen/webroot/js/WebrootVue.qvt.code.test.js
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

globalThis.moqui = require("./MoquiLib.js");
require("./WebrootVue.qvt.js");
// the vendored bundle is an IIFE that sets the global hljs, as a script tag runs it
vm.runInThisContext(fs.readFileSync(path.join(__dirname, "../libs/highlightjs/highlight.min.js"), "utf8"));
const code = moqui.code;

test("the language of a file comes from its extension, else plain text", () => {
    assert.equal(code.languageOf("src/main/java/org/Example.java"), "java");
    assert.equal(code.languageOf("build.gradle"), "groovy");
    assert.equal(code.languageOf("App.kt"), "kotlin");
    assert.equal(code.languageOf("screen/Home.XML"), "xml");
    assert.equal(code.languageOf("lib.rs"), "rust");
    assert.equal(code.languageOf("deploy.sh"), "bash");
    assert.equal(code.languageOf("Dockerfile"), "bash");
    assert.equal(code.languageOf("notes"), "plaintext");
    assert.equal(code.languageOf(""), "plaintext");
    // every language a file name can name is in the vendored bundle
    for (const language of new Set(Object.values(code.extensions))) assert.ok(hljs.getLanguage(language), language);
});

test("marked lines are single lines and ranges", () => {
    assert.deepEqual([...code.parseLines("3,5-7, 10")], [3, 5, 6, 7, 10]);
    assert.deepEqual([...code.parseLines("9-8")], [8, 9]);
    assert.deepEqual([...code.parseLines("x, 2")], [2]);
    assert.equal(code.parseLines("").size, 0);
});

test("a token over several lines keeps its spans on each line", () => {
    const html = hljs.highlight("/* one\n   two */\nint x = 1;\n", { language: "java" }).value;
    const lines = code.splitLines(html);
    assert.equal(lines.length, 4);
    assert.match(lines[0], /^<span class="hljs-comment">\/\* one<\/span>$/);
    assert.match(lines[1], /^<span class="hljs-comment">   two \*\/<\/span>$/);
    assert.match(lines[2], /<span class="hljs-type">int<\/span>/);
    for (const line of lines) assert.equal(line.split("<span").length, line.split("</span>").length, line);
    assert.deepEqual(code.splitLines("a\n<span class=\"x\">b\nc</span>\nd"), ["a", "<span class=\"x\">b</span>", "<span class=\"x\">c</span>", "d"]);
});

test("plain lines are escaped and the file's last newline dropped", () => {
    assert.deepEqual(code.plainLines(code.trimEnd("<a>&\r\nb\n\n")), ["&lt;a&gt;&amp;", "b"]);
});
