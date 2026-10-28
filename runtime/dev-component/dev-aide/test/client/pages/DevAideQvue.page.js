/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// A .qvue template's elements as the server stamps them (the same lines and columns as stamp_qvue's test), and the
// Design view's edits of the template.
"use strict";

const pure = moqui.devAide.pure;

const TEXT = "<!-- a screen -->\n<template><div class=\"row\">\n  <q-btn label=\"a > b\" @click=\"go\"/>\n  " +
    "<!-- <span>not</span> -->{{ a < b }}<template v-slot:x><b>x</b></template>\n</div></template>\n<script>const x = '<p>';</script>";

test("the template's elements, with the lines and columns the server stamps", () => {
    const elements = pure.vueElements(TEXT);
    const stamped = elements.filter((element) => element.stamped).map((element) => element.name + "@" + element.line + ":" + element.column);
    assert.deepEqual(stamped, ["div@2:11", "q-btn@3:3", "b@4:58"]);
    const div = elements[0], btn = elements[1];
    assert.equal(btn.parent, 0);
    assert.equal(btn.selfClosing, true);
    assert.equal(TEXT.slice(div.start, div.end).endsWith("</div>"), true);
    assert.equal(pure.vueElementAt(elements, "component://a/A.qvue:3:3", "component://a/A.qvue"), 1);
    assert.equal(pure.vueElementAt(elements, "component://other/B.qvue:3:3", "component://a/A.qvue"), -1);
    assert.deepEqual(pure.vueElements("<script>x</script>"), []);
});

const BOARD = "<template>\n<div class=\"board\">\n    <q-btn label=\"One\"/>\n    <q-btn label=\"Two\"/>\n</div>\n</template>\n";

test("an element removed with its line, moved among its siblings, and its opening tag changed", () => {
    const elements = pure.vueElements(BOARD);
    const removed = pure.vueEdit(BOARD, elements, 1, "remove");
    assert.equal(removed.text, BOARD.replace("    <q-btn label=\"One\"/>\n", ""));
    const down = pure.vueEdit(BOARD, elements, 1, "down");
    assert.ok(down.text.indexOf("Two") < down.text.indexOf("One"));
    assert.equal(pure.vueEdit(BOARD, elements, 2, "down"), null);
    const tag = pure.vueEdit(BOARD, elements, 1, "tag", "<q-btn flat label=\"Uno\"/>");
    assert.ok(tag.text.includes("<q-btn flat label=\"Uno\"/>\n    <q-btn label=\"Two\"/>"));
});

test("a snippet inserted before, after or last inside, at the element's indent", () => {
    const elements = pure.vueElements(BOARD);
    const snippet = "<q-chip>New</q-chip>";
    assert.ok(pure.vueEdit(BOARD, elements, 2, "after", snippet).text.includes("<q-btn label=\"Two\"/>\n    <q-chip>New</q-chip>\n</div>"));
    assert.ok(pure.vueEdit(BOARD, elements, 1, "before", snippet).text.includes("<div class=\"board\">\n    <q-chip>New</q-chip>\n    <q-btn label=\"One\"/>"));
    const inside = pure.vueEdit(BOARD, elements, 0, "inside", snippet);
    assert.ok(inside.text.includes("<q-btn label=\"Two\"/>\n    <q-chip>New</q-chip>\n</div>"), inside.text);
    assert.equal(pure.vueElements(inside.text).find((element) => element.start === inside.select).name, "q-chip");
    assert.equal(pure.vueEdit(BOARD, elements, 1, "inside", snippet), null);
});
