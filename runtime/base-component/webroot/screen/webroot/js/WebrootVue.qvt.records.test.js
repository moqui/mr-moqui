/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// Tests of the record components of WebrootVue.qvt.js that need no browser: the HTML m-rich-text writes and reads (mentions,
// the empty value, the plain-text toggle, the mention search answer, typed links), the panel URL of m-chatter and
// m-record-files from the shell data, the initials and color of m-avatar, and the card JSON of m-record-card. What needs a
// browser (Tiptap itself, the toolbar, the @ list, a submit) is the Chromium smoke of Gate 3.
// Run: node --test runtime/base-component/webroot/screen/webroot/js/WebrootVue.qvt.records.test.js
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");

globalThis.moqui = require("./MoquiLib.js");
require("./WebrootVue.qvt.js");
const rich = moqui.richText;

test("a mention is a span with its id, escaped, and the mentions of a text are read back once each", () => {
    assert.equal(rich.mentionHtml("john.doe", "John Doe"), '<span data-mention-id="john.doe">@John Doe</span>');
    assert.equal(rich.mentionHtml('a"<b>', "A & B"), '<span data-mention-id="a&quot;&lt;b&gt;">@A &amp; B</span>');
    assert.equal(rich.mentionHtml("x"), '<span data-mention-id="x">@x</span>');
    const html = '<p>Hi <span data-mention-id="john.doe">@John Doe</span> and <span data-mention-id="a&amp;b">@A &amp; B</span>, ' +
        '<span class="other">not one</span> <span data-mention-id="john.doe">@John</span></p>';
    assert.deepEqual(rich.mentions(html), [{ id: "john.doe", label: "John Doe" }, { id: "a&b", label: "A & B" }]);
    assert.deepEqual(rich.mentions(""), []);
    assert.deepEqual(rich.mentions(null), []);
});

test("the empty document is the empty value; an image, a rule or a mention is not empty", () => {
    for (const empty of ["", null, undefined, "<p></p>", "<p><br></p>", "<p> &nbsp; </p><p></p>"]) {
        assert.equal(rich.isEmpty(empty), true, String(empty));
        assert.equal(rich.value(empty), "", String(empty));
    }
    for (const full of ["<p>x</p>", '<p><img src="/a.png"></p>', "<hr>", '<p><span data-mention-id="a">@A</span></p>',
        "<table><tr><td></td></tr></table>", '<p><span data-record-id="1">#SO</span></p>']) {
        assert.equal(rich.isEmpty(full), false, full);
        assert.equal(rich.value(full), full);
    }
});

test("a record link is a span with its entity and id, and the links of a text are read back once each", () => {
    assert.equal(rich.recordHtml("mantle.order.OrderHeader", "100050", "SO100050"),
        '<span data-record-entity="mantle.order.OrderHeader" data-record-id="100050">#SO100050</span>');
    assert.equal(rich.recordHtml('a"<b>', "1", "A & B"),
        '<span data-record-entity="a&quot;&lt;b&gt;" data-record-id="1">#A &amp; B</span>');
    const html = '<p>See <span data-record-entity="mantle.order.OrderHeader" data-record-id="100050">#SO100050</span> ' +
        'and <span data-record-id="100050">#again</span> <span data-mention-id="ann">@Ann</span></p>';
    assert.deepEqual(rich.records(html), [{ entity: "mantle.order.OrderHeader", id: "100050", label: "SO100050" }]);
    assert.deepEqual(rich.mentions(html), [{ id: "ann", label: "Ann" }]);
    assert.equal(rich.toPlain(html), "See #SO100050 and #again @Ann");
});

test("plain text of a table is its cells joined, with a dashed line under a header row", () => {
    const html = "<p>Before</p><table><thead><tr><th>Name</th><th>Qty</th></tr></thead><tbody>" +
        "<tr><td>Bolt</td><td>2</td></tr><tr><td>Nut</td><td colspan=\"2\">4</td></tr></tbody></table><p>After</p>";
    assert.equal(rich.toPlain(html), ["Before", "", "Name | Qty", "---- | ---", "Bolt | 2", "Nut | 4", "", "After"].join("\n"));
});

test("plain text of the editor's HTML keeps paragraphs, lists, quotes, links, code and mentions", () => {
    const html = "<h2>Plan</h2><p>Hello <strong>world</strong>,&nbsp;see <a href=\"https://example.com/x\">the page</a> and " +
        "<a href=\"https://example.com/y\">https://example.com/y</a>.</p>" +
        "<ul><li><p>one</p></li><li><p>two</p><ul><li><p>two a</p></li></ul></li></ul>" +
        "<ol start=\"3\"><li><p>third</p></li><li><p>fourth</p></li></ol>" +
        "<blockquote><p>quoted &amp; kept</p></blockquote>" +
        "<pre><code>let x = 1;\n  let y = 2;</code></pre>" +
        "<p>line<br>break <span data-mention-id=\"john.doe\">@John Doe</span></p><hr><p><img src=\"/a.png\" alt=\"a chart\"></p>";
    assert.equal(rich.toPlain(html), [
        "Plan",
        "",
        "Hello world, see the page (https://example.com/x) and https://example.com/y.",
        "",
        "- one",
        "- two",
        "  - two a",
        "",
        "3. third",
        "4. fourth",
        "",
        "> quoted & kept",
        "",
        "let x = 1;",
        "  let y = 2;",
        "",
        "line",
        "break @John Doe",
        "",
        "---",
        "",
        "a chart"
    ].join("\n"));
    assert.equal(rich.toPlain(""), "");
    assert.equal(rich.toPlain("<p></p>"), "");
    assert.equal(rich.toPlain("<p>a &lt;b&gt; &#39;c&#39; &#x263A;</p>"), "a <b> 'c' ☺");
    assert.equal(rich.toPlain('<p><a href="mailto:ann@example.com">ann@example.com</a></p>'), "ann@example.com");
});

test("HTML of plain text is a paragraph for each run of lines, escaped, and goes back to the same text", () => {
    assert.equal(rich.fromPlain(""), "");
    assert.equal(rich.fromPlain("  \n "), "");
    assert.equal(rich.fromPlain("one\ntwo\n\nthree <b> & 'x'"), "<p>one<br>two</p><p>three &lt;b&gt; &amp; &#39;x&#39;</p>");
    assert.equal(rich.fromPlain("a\r\n\r\n\r\nb"), "<p>a</p><p>b</p>");
    const text = "Dear Ann,\n\nThe order ships Monday.\nRegards";
    assert.equal(rich.toPlain(rich.fromPlain(text)), text);
});

test("plain text that was not edited comes back as the original HTML", () => {
    const original = "<p>Hello <strong>world</strong> <span data-mention-id=\"john.doe\">@John Doe</span></p>" +
        "<p>See <span data-record-entity=\"mantle.order.OrderHeader\" data-record-id=\"100050\">#SO100050</span></p>" +
        "<table><thead><tr><th>Name</th><th>Qty</th></tr></thead><tbody><tr><td>Bolt</td><td>2</td></tr></tbody></table>" +
        "<p><img src=\"/a.png\" alt=\"a chart\"></p>";
    assert.equal(rich.fromPlainKeeping(rich.toPlain(original), original), original);
    assert.equal(rich.fromPlainKeeping(rich.toPlain(original).replace(/\n/g, "\r\n"), original), original);
});

test("an edit puts a mention, a record link and a table back, and leaves the rest as plain paragraphs", () => {
    const original = "<p>Hello <strong>world</strong> <span data-mention-id=\"john.doe\">@John Doe</span></p>" +
        "<p>See <span data-record-entity=\"mantle.order.OrderHeader\" data-record-id=\"100050\">#SO100050</span> " +
        "and <span data-record-id=\"100050\">#again</span></p>" +
        "<table><thead><tr><th>Name</th><th>Qty</th></tr></thead><tbody><tr><td>Bolt</td><td>2</td></tr><tr><td>Nut</td><td>4</td></tr></tbody></table>" +
        "<p><img src=\"/a.png\" alt=\"a chart\"></p>";
    const plain = rich.toPlain(original).replace("Hello world", "Hello there");
    const kept = rich.fromPlainKeeping(plain, original);
    assert.ok(kept.includes("Hello there"));
    assert.ok(kept.includes('<span data-mention-id="john.doe">@John Doe</span>'));
    assert.ok(kept.includes('<span data-record-entity="mantle.order.OrderHeader" data-record-id="100050">#SO100050</span>'));
    assert.ok(kept.includes('<span data-record-id="100050">#again</span>'));
    assert.ok(kept.includes("<th>Name</th>"));
    assert.ok(kept.includes("<td>Bolt</td>"));
    assert.ok(!kept.includes("<strong>"));
    assert.ok(!kept.includes("<img"));
    assert.equal(rich.fromPlainKeeping(plain.replace("@John Doe", "Jane"), original).includes("data-mention-id"), false);
});

test("a second chip with the same label is put back at the next copy", () => {
    const original = '<p><span data-mention-id="a">@Ann</span> and <span data-mention-id="b">@Ann</span></p>';
    const kept = rich.fromPlainKeeping(rich.toPlain(original) + "!", original);
    const first = kept.indexOf('data-mention-id="a"');
    const second = kept.indexOf('data-mention-id="b"');
    assert.ok(first >= 0 && second > first);
    assert.equal(rich.toPlain(original), "@Ann and @Ann");
});

test("the editor opens HTML as it is and text without markup as its paragraphs", () => {
    assert.equal(rich.toHtml("<p>x</p>"), "<p>x</p>");
    assert.equal(rich.toHtml("a < b\n\nc"), "<p>a &lt; b</p><p>c</p>");
    assert.equal(rich.toHtml(null), "");
    assert.equal(rich.toHtml("<!-- note --><p>y</p>"), "<!-- note --><p>y</p>");
});

test("a mention search answer is read as a list of id and label, each id once, at most ten", () => {
    assert.deepEqual(rich.mentionItems([{ id: "u1", label: "Ann" }, { value: "u2", label: "Bo" }, { id: "u1", label: "again" },
        { id: "", label: "none" }, "u3", { id: 4, name: "Four" }]), [
        { id: "u1", label: "Ann", image: null }, { id: "u2", label: "Bo", image: null }, { id: "u3", label: "u3", image: null },
        { id: "4", label: "Four", image: null }]);
    assert.deepEqual(rich.mentionItems({ options: [{ id: "a", title: "A", image: "/a.png" }] }), [{ id: "a", label: "A", image: "/a.png" }]);
    assert.deepEqual(rich.mentionItems(null), []);
    assert.equal(rich.mentionItems(Array.from({ length: 30 }, (x, i) => ({ id: "u" + i }))).length, 10);
    assert.equal(rich.mentionItems(Array.from({ length: 30 }, (x, i) => ({ id: "u" + i })), 3).length, 3);
    assert.deepEqual(rich.mentionItems({ optionList: [{ value: "mantle.order.OrderHeader|100050", label: "Order · SO100050",
        recordEntityName: "mantle.order.OrderHeader", recordId: "100050", id: "ignored" }] }),
        [{ id: "100050", label: "Order · SO100050", image: null, entity: "mantle.order.OrderHeader" }]);
    assert.deepEqual(rich.mentionItems([{ id: "100050", label: "SO100050", entity: "mantle.order.OrderHeader" }]),
        [{ id: "100050", label: "SO100050", image: null, entity: "mantle.order.OrderHeader" }]);
});

test("a typed link gets a scheme: https for a host, mailto for an address, as it is with one", () => {
    assert.equal(rich.linkHref("example.com/a"), "https://example.com/a");
    assert.equal(rich.linkHref(" ann@example.com "), "mailto:ann@example.com");
    assert.equal(rich.linkHref("http://x.org"), "http://x.org");
    assert.equal(rich.linkHref("/qapps/gallery"), "/qapps/gallery");
    assert.equal(rich.linkHref("tel:+15551234"), "tel:+15551234");
    assert.equal(rich.linkHref(""), "");
});

test("the light toolbar is bold, italic, link, list and code; the full one has the plain-text toggle last", () => {
    assert.deepEqual(rich.toolbar(true).map((button) => button.key), ["bold", "italic", "link", "bulletList", "code"]);
    const full = rich.toolbar(false, true).filter((button) => !button.separator).map((button) => button.key);
    assert.deepEqual(full, ["block", "bold", "italic", "strike", "bulletList", "orderedList", "link", "image", "table", "code", "codeBlock",
        "blockquote", "undo", "redo", "plain"]);
    assert.ok(!rich.toolbar(true).some((button) => button.key === "table"));
    assert.ok(!rich.toolbar(false, false).some((button) => button.key === "plain"));
});

test("a record panel loads the theme's subscreen of the current app, with the record, as the screen renders", () => {
    const shellData = {
        shell: { chatterScreen: "Chatter", recordFilesScreen: "RecordFiles" },
        apps: [{ name: "sales", recordScreens: { chatter: { url: "/qapps/sales/Chatter", renderModes: ["vue", "qvue"] },
            recordFiles: { url: "/qapps/sales/RecordFiles", renderModes: ["all"] } } },
        { name: "tools", recordScreens: { chatter: null, recordFiles: null } }]
    };
    const record = { recordEntity: "mantle.order.OrderHeader", recordId: "SO 1&2", defaultTab: "log", hide: "" };
    assert.deepEqual(moqui.recordPanelUrl(shellData, "sales", "chatter", record, "/apps", "/qapps"),
        { url: "/apps/sales/Chatter.qvue?recordEntity=mantle.order.OrderHeader&recordId=SO%201%262&appRoot=sales&defaultTab=log" });
    assert.deepEqual(moqui.recordPanelUrl(shellData, "sales", "recordFiles", { recordEntity: "E", recordId: "1", hide: "share" }, "/apps", "/qapps"),
        { url: "/apps/sales/RecordFiles?recordEntity=E&recordId=1&appRoot=sales&hide=share" });
    // the reasons it shows nothing, to log once; no record is no reason (the data may still be coming)
    assert.deepEqual(moqui.recordPanelUrl(null, "sales", "chatter", record), { url: null, wait: true });
    assert.equal(moqui.recordPanelUrl({ shell: {}, apps: shellData.apps }, "sales", "chatter", record).reason, "the theme names no chatter screen");
    assert.equal(moqui.recordPanelUrl(shellData, "", "chatter", record).reason, "there is no current app root");
    assert.equal(moqui.recordPanelUrl(shellData, "tools", "chatter", record).reason, "app tools has no Chatter screen the user may open");
    assert.equal(moqui.recordPanelUrl(shellData, "other", "chatter", record).reason, "app other has no Chatter screen the user may open");
    assert.deepEqual(moqui.recordPanelUrl(shellData, "sales", "chatter", { recordEntity: "E", recordId: "" }), { url: null });
});

test("an avatar's initials are the first and last word's first letters, and its color is the same for the same name", () => {
    assert.equal(moqui.avatar.initials("Ada Lovelace"), "AL");
    assert.equal(moqui.avatar.initials("ada  king  lovelace"), "AL");
    assert.equal(moqui.avatar.initials("ada.lovelace@example.com"), "AL");
    assert.equal(moqui.avatar.initials("john_doe"), "JD");
    assert.equal(moqui.avatar.initials("Ümit"), "Ü");
    assert.equal(moqui.avatar.initials("(Acme) Co."), "AC");
    assert.equal(moqui.avatar.initials(""), "");
    assert.equal(moqui.avatar.initials(null), "");
    assert.equal(moqui.avatar.color("Ada Lovelace"), moqui.avatar.color("Ada Lovelace"));
    assert.match(moqui.avatar.color("x"), /^[a-z-]+$/);
    const template = "/qapps/Commons/Settings/Account/userPhoto?userId=${userId}";
    assert.equal(moqui.avatar.imageUrl("john.doe", template), "/qapps/Commons/Settings/Account/userPhoto?userId=john.doe");
    assert.equal(moqui.avatar.imageUrl("a b&c", template), "/qapps/Commons/Settings/Account/userPhoto?userId=a%20b%26c");
    assert.equal(moqui.avatar.imageUrl("john.doe", ""), "");
    assert.equal(moqui.avatar.imageUrl("john.doe", "/photo"), "");
    assert.equal(moqui.avatar.imageUrl("", template), "");
    assert.equal(moqui.avatar.imageUrl(null, template), "");
});

test("a record card is read from the card JSON: title, type, status, two fields with a value, the link", () => {
    const card = moqui.recordCard.normalize({ title: "Acme order", typeLabel: "Sales order", icon: "sym_o_receipt_long", id: "SO10023",
        url: "/qapps/sales/Order?orderId=SO10023", status: { label: "Approved", color: "positive" },
        fields: [{ label: "Total", value: "$1,200.00" }, { label: "Empty", value: "" }, { label: "Customer", value: "Acme" }, { label: "Third", value: "x" }] },
    "mantle.order.OrderHeader", "SO10023");
    assert.deepEqual(card, { entity: "mantle.order.OrderHeader", id: "SO10023", title: "Acme order", typeLabel: "Sales order",
        icon: "sym_o_receipt_long", status: { label: "Approved", color: "positive" },
        fields: [{ label: "Total", value: "$1,200.00" }, { label: "Customer", value: "Acme" }], url: "/qapps/sales/Order?orderId=SO10023" });
    // under card, a status as text, the title falling back to the id, the default icon
    assert.deepEqual(moqui.recordCard.normalize({ card: { status: "Open" } }, "E", "7"),
        { entity: "E", id: "7", title: "7", typeLabel: "", icon: "sym_o_description", status: { label: "Open", color: "primary" }, fields: [], url: null });
    assert.equal(moqui.recordCard.normalize("nope", "E", "7"), null);
});

test("a status bar draws the stages and the transitions get#StatusBar answers", () => {
    const view = moqui.statusBarView({
        current: { statusId: "WeInProgress", label: "In Progress", color: "primary" },
        stages: [{ statusId: "WeInPlanning", label: "Planning", color: "grey-7", state: "done" },
            { statusId: "WeInProgress", label: "In Progress", color: "primary", state: "current" },
            { statusId: "WeComplete", label: "Complete", color: "positive", state: "todo" }, { label: "no id" }],
        transitions: [{ toStatusId: "WeComplete", label: "Complete", isNext: true }, { toStatusId: "WeOnHold", label: "Hold" },
            { toStatusId: "WeCancelled", label: "Cancel", isDanger: true }]
    }, { title: "Fix the roof" });
    assert.deepEqual(view.stages.map((stage) => stage.statusId + ":" + stage.state), ["WeInPlanning:done", "WeInProgress:current", "WeComplete:todo"]);
    assert.deepEqual(view.stages[0].chip, { icon: "sym_o_check", class: "q-ma-none m-tonal text-grey-8 m-status-done" });
    assert.deepEqual(view.stages[1].chip, { color: "primary", "text-color": "white", class: "q-ma-none text-weight-medium m-status-current" });
    assert.equal(view.stages[2].chip.outline, true);
    assert.deepEqual(view.buttons.map((item) => [item.toStatusId, item.button.color || item.button.class, item.confirm]), [
        ["WeComplete", "primary", "Complete: Fix the roof?"], ["WeOnHold", "m-btn-tonal text-primary", "Hold: Fix the roof?"],
        ["WeCancelled", "m-btn-tonal text-negative", "Cancel: Fix the roof?"]]);
    assert.equal(moqui.statusBarView({ transitions: [{ toStatusId: "A", label: "Go" }] }, { confirm: false }).buttons[0].confirm, null);
    assert.equal(moqui.statusBarView({ transitions: [{ toStatusId: "A" }] }, {}).buttons[0].confirm, "A?");
    assert.deepEqual(moqui.statusBarView(null), { current: null, stages: [], buttons: [] });
    const bar = moqui.qvt.components["m-status-bar"];
    assert.deepEqual(bar.methods.fieldsOf.call({ parameters: { workEffortId: "W1" }, parameterName: "statusId" }, { toStatusId: "WeComplete" }),
        { workEffortId: "W1", statusId: "WeComplete" });
});

test("an editable select reads its options from a list, a map with selected, or plain values", () => {
    assert.deepEqual(moqui.editableOptions([{ value: "A", label: "First" }, { key: "B", text: "Second" }, "C"]),
        { options: [{ value: "A", label: "First" }, { value: "B", label: "Second" }, { value: "C", label: "C" }], selected: undefined });
    assert.deepEqual(moqui.editableOptions({ E: "Letter E", F: "Letter F", selected: "F" }),
        { options: [{ value: "E", label: "Letter E" }, { value: "F", label: "Letter F" }], selected: "F" });
    assert.deepEqual(moqui.editableOptions({ options: [{ value: 1, label: "One" }], selected: 1 }), { options: [{ value: "1", label: "One" }], selected: "1" });
    assert.deepEqual(moqui.editableOptions(undefined), { options: [], selected: undefined });
    // the current key of a select: the value given, else the option whose label is the text
    const editable = moqui.qvt.components["m-editable"];
    const data = (props) => editable.data.call(Object.assign({ labelValue: "", options: null, value: undefined }, props));
    assert.equal(data({ labelValue: "Second", options: [{ value: "A", label: "First" }, { value: "B", label: "Second" }] }).current, "B");
    assert.equal(data({ labelValue: "Second", value: "X" }).current, "X");
    assert.equal(data({ labelValue: "a &amp; b" }).current, "a & b");
});

test("the helpers of a btn-type and row-col sizes that are expressions", () => {
    const mq = moqui.qvt.helpers;
    assert.deepEqual(mq.btnKind("primary", "m-link"), { color: "primary", class: "m-link" });
    assert.deepEqual(mq.btnKind("danger", "m-link"), { class: "m-btn-tonal text-negative m-link" });
    assert.deepEqual(mq.btnKind("", ""), { class: "m-btn-tonal text-primary" });
    assert.deepEqual(mq.btnKind(null, "x"), { class: "m-btn-tonal text-primary x" });
    assert.equal(mq.btnText("success"), "text-positive");
    assert.equal(mq.btnText(""), "text-primary");
    assert.equal(mq.cols("8", "8", "", "", false), "q-px-xs col-lg-8 col-md-8 col-sm-12 col-xs-12");
    assert.equal(mq.cols("", "2", "", "", false), "q-px-xs col-md-2 col-sm-12 col-xs-12");
    assert.equal(mq.cols(null, "", "", "", true), "");
});
