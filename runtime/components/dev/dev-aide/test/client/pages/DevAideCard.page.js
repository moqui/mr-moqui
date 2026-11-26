/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// The change set card from a patch, in the page (plans/design-dev-aide.md 13.3): m-assist-change-set of
// js/WebrootAssist.js, mounted with Vue and Quasar on the card the server makes of a patch. The card and its files are
// test/vectors/change-card-vectors.json, whose cards src/agent/propose_card_tests.rs holds to change_card: what it
// changes as artifacts, its files with their counts and side, and the new titles a business user may word, written
// into the change set's arguments.
"use strict";

const assist = moqui.assist;

const settle = async () => {
    for (let i = 0; i < 4; i++) { await Vue.nextTick(); await new Promise((resolve) => setTimeout(resolve, 0)); }
};

const vectors = async () => (await (await fetch("/files/component/dev-aide/test/vectors/change-card-vectors.json")).json()).vectors;

// the card as the approval carries it: the server's change set card, and the change set's arguments
const cardOf = (vector) => ({
    approvalId: "AP1", targetKind: "file", changeSet: vector.card,
    arguments: { files: vector.files.map((file) => (file.text ? { file: file.file, text: file.text } : { file: file.file, edits: file.edits })) },
});

const mount = async (card, pending) => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const emitted = [];
    const app = Vue.createApp({
        data() { return { card, pending }; },
        methods: { got(args) { emitted.push(args); } },
        template: '<m-assist-change-set :card="card" :pending="pending" @arguments="got"></m-assist-change-set>',
    });
    app.use(Quasar);
    app.component("m-assist-change-set", assist.components["m-assist-change-set"]);
    app.mount(host);
    await settle();
    return { host, emitted, unmount: () => { app.unmount(); host.remove(); } };
};

// what the first vector's card reads as, to the letter
const EXPECTED_ROWS = [
    "Entity Example: + field dueDate (date)",
    "Screen EditExample: + field Due Date in EditExample after exampleEndDate",
    "DueServices.xml: + service update#DueDate (inline)",
    "DueServices.xml: + parameter exampleId (in String, required=true)",
    "DueServices.xml: + parameter dueDate (in Date, required=false)",
    "Things.xml: +1 −1",
];
const EXPECTED_FILES = [
    "my-site/entity/SiteEntities.xml +1 −0",
    "ExampleApp/Example/EditExample.xml +1 −0",
    "service/mysite/DueServices.xml +5 −0 (new)",
    "screen/UpApp/Things.xml +1 −1",
];

const squash = (text) => text.replace(/\s+/g, " ").trim();
const texts = (host, selector) => Array.from(host.querySelectorAll(selector)).map((el) => squash(el.textContent));
// a file's line without its icon's ligature text
const fileLines = (host) => Array.from(host.querySelectorAll("[data-assist-change-file]")).map((el) => {
    const line = el.cloneNode(true);
    line.querySelectorAll(".q-icon").forEach((icon) => icon.remove());
    return squash(line.textContent);
});

test("a file approval's card body is the change set's", () => {
    assert.equal(assist.approvalRenderers.file, "m-assist-change-set");
});

test("the card reads a patch as artifacts, and its files with their counts and side", async () => {
    for (const vector of await vectors()) {
        const card = await mount(cardOf(vector), true);
        assert.deepEqual(texts(card.host, "[data-assist-change-row]"), vector.card.rows.map((row) => row.artifact + ": " + row.change), vector.name);
        const files = fileLines(card.host);
        assert.equal(files.length, vector.card.files.length, vector.name);
        vector.card.files.forEach((file, at) => {
            const short = file.file.split("/").slice(-3).join("/");
            assert.equal(files[at].indexOf(short) >= 0 && files[at].indexOf("+" + file.added) >= 0 && files[at].indexOf("−" + file.removed) >= 0, true, files[at]);
            assert.equal(files[at].indexOf("(new)") >= 0, file.new, files[at]);
        });
        // the side by its icon: a site file is the user's extension, an upstream one is locked
        const icons = texts(card.host, "[data-assist-change-file] .q-icon");
        assert.deepEqual(icons, vector.card.files.map((file) => (file.side === "site" ? "extension" : "lock")));
        card.unmount();
    }
});

test("the first vector's summary names each artifact it changes", async () => {
    const [vector] = await vectors();
    const card = await mount(cardOf(vector), true);
    const rows = texts(card.host, "[data-assist-change-row]");
    assert.deepEqual(rows, EXPECTED_ROWS);
    assert.deepEqual(fileLines(card.host), EXPECTED_FILES);
    card.unmount();
});

test("a new title is worded on the card, and the change set's arguments carry it", async () => {
    const [vector] = await vectors();
    const card = await mount(cardOf(vector), true);
    const values = vector.card.values;
    assert.deepEqual(values.map((value) => value.value), ["Due Date", "Find your things"]);
    // q-input gives its attributes to its native input
    const inputs = card.host.querySelectorAll("input[data-assist-change-value]");
    assert.equal(inputs.length, values.length);
    assert.equal(inputs[0].value, "Due Date");
    inputs[0].value = "Due by";
    inputs[0].dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    assert.equal(card.emitted.length > 0, true, "the edited arguments");
    const args = card.emitted[card.emitted.length - 1];
    const extend = args.files[values[0].file];
    assert.equal(extend.edits[0].new.indexOf('title="Due by"') >= 0, true, extend.edits[0].new);
    assert.equal(extend.edits[0].new.indexOf("Due Date"), -1);
    // the others as proposed, and the card's own arguments untouched
    assert.equal(args.files[values[1].file].edits[0].new, 'title="Find your things"');
    assert.equal(card.emitted.length, 1);
    assert.equal(vector.files[values[0].file].edits[0].new.indexOf('title="Due Date"') >= 0, true);
    card.unmount();
});

test("once decided the card keeps its summary, with no titles to word and no Preview", async () => {
    const [vector] = await vectors();
    // pending, on a page that is not the Workspace, it has both, so their absence below is the decision's
    const pending = await mount(cardOf(vector), true);
    assert.equal(location.pathname.indexOf("/dev/Workspace"), -1, location.pathname);
    assert.notEqual(pending.host.querySelector('[data-assist-action="preview-change-set"]'), null, "Preview while pending");
    assert.equal(pending.host.querySelectorAll("input[data-assist-change-value]").length, vector.card.values.length);
    pending.unmount();
    const card = await mount(cardOf(vector), false);
    assert.equal(card.host.querySelectorAll("[data-assist-change-row]").length, vector.card.rows.length);
    assert.equal(card.host.querySelectorAll("[data-assist-change-value]").length, 0);
    assert.equal(card.host.querySelector('[data-assist-action="preview-change-set"]'), null);
    card.unmount();
});
