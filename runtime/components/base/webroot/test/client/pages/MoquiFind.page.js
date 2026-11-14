/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
"use strict";
const find = moqui.find;
const spans = (text) => find.sentences(text).map(([start, end]) => text.slice(start, end));

// A page like the one this find is for: the sentence wanted has the words "pump" and "warranty" far apart, so the
// browser's find of "pump and warranty" finds nothing.
const PAGE = `<main id="doc">
  <h1>Notes</h1>
  <p>The shop fits pumps, valves and hoses for farms.</p>
  <p>When a customer asks, I want to be sure that any part I fit in a <em>pump</em> myself stays as it is,
     and a part with a full service record needs less of a warranty check. This does not cover old stock.</p>
  <ul><li>A pump has a warranty of two years from the day it ships.</li><li>Code then picks the best span</li></ul>
  <script>const notText = "pump warranty";</script>
</main>`;
const WANTED = "When a customer asks";
const SECOND = "A pump has a warranty of two years from the day it ships.";

test("a text is cut into sentences at . ! ? before a space and at line breaks", () => {
    assert.deepEqual(spans("One. Two!  Three?\nFour 4.5 five"), ["One.", "Two!", "Three?", "Four 4.5 five"]);
    assert.deepEqual(find.sentences("a\nb", false).length, 1, "a line break that does not show is a space");
    assert.deepEqual(spans("  \n -- \n"), [], "a sentence has a letter or a digit");
    const long = ("word ".repeat(200)).trim();
    const cut = spans(long);
    assert.ok(cut.length >= 2 && cut.every((one) => one.length <= 600));
    assert.equal(cut.join(" "), long, "a long sentence is cut at spaces");
});

test("the word find has half or more of the words of three letters or more, at the start of a word", () => {
    const texts = ["A pump with a warranty.", "Pumps and their warranty cards", "Only a pump"];
    assert.deepEqual(find.wordMatches("pump and warranty", texts), [1, 0], "half the words or more, more first");
    assert.deepEqual(find.wordMatches("pump pump", texts), [0, 1, 2]);
    assert.deepEqual(find.wordMatches("a b", texts), []);
});

test("passages are the page's sentences, each a range over its exact text", () => {
    document.body.innerHTML = PAGE;
    const found = find.passages(document.getElementById("doc"));
    const texts = found.map((one) => one.text);
    assert.equal(texts[0], "Notes");
    assert.ok(texts.includes("This does not cover old stock."));
    assert.ok(texts.includes("Code then picks the best span"));
    assert.ok(!texts.some((text) => text.includes("notText")), "script text is not the page's");
    const wanted = found.find((one) => one.text.startsWith(WANTED));
    assert.ok(wanted, texts.join(" | "));
    assert.equal(wanted.range.toString().replace(/\s+/g, " "), wanted.text,
        "the range covers the sentence across the em");
    assert.ok(wanted.text.endsWith("less of a warranty check."));
    document.body.innerHTML = "<pre id='code'>let a = 1\nlet b = 2</pre>";
    assert.deepEqual(find.passages(document.getElementById("code")).map((one) => one.text), ["let a = 1", "let b = 2"],
        "the lines of a pre are sentences");
});

test("Mod+F is Ctrl+F, or Cmd+F on a Mac", () => {
    const key = (init) => new KeyboardEvent("keydown", Object.assign({ key: "f" }, init));
    assert.ok(find.isKey(key({ ctrlKey: true }), "Mod+F", false));
    assert.ok(!find.isKey(key({ ctrlKey: true }), "Mod+F", true));
    assert.ok(find.isKey(key({ metaKey: true }), "Mod+F", true));
    assert.ok(!find.isKey(key({ ctrlKey: true, shiftKey: true }), "Mod+F", false));
});

test("the bar shows the sentence the server ranks first and highlights its exact text", async () => {
    document.body.innerHTML = PAGE;
    const calls = [];
    const realFetch = window.fetch;
    // the server: the sentence with the meaning first, as the embedding model ranks it
    window.fetch = async (url, init) => {
        const body = JSON.parse(init.body);
        calls.push({ url, body, token: init.headers["X-CSRF-Token"] });
        const index = body.texts.findIndex((text) => text.startsWith(WANTED));
        const matches = body.query ? [{ index, score: 0.61 }, { index: body.texts.indexOf(SECOND), score: 0.45 }] : [];
        return new Response(JSON.stringify({ matches }), { status: 200, headers: { "Content-Type": "application/json" } });
    };
    try {
        const finder = find.attach({ root: "#doc", token: () => "t1" });
        document.dispatchEvent(new KeyboardEvent("keydown", { key: "f", ctrlKey: true, bubbles: true, cancelable: true }));
        const bar = document.querySelector(".moqui-find");
        assert.ok(bar, "Ctrl+F opens the bar");
        assert.equal(calls[0].body.query, "", "opening embeds the page's sentences");
        assert.equal(calls[0].url, "/find/rank");
        assert.equal(calls[0].token, "t1");
        const found = await finder.search("pump and warranty");
        assert.equal(found.length, 2);
        assert.ok(found[0].text.startsWith(WANTED));
        assert.equal(bar.querySelector(".count").textContent, "1/2");
        assert.equal(bar.querySelectorAll("li").length, 2);
        if (CSS.highlights) {
            const current = [...CSS.highlights.get("moqui-find-current")];
            assert.equal(current.length, 1);
            assert.ok(current[0].toString().includes("full service record"));
        }
        finder.detach();
        assert.equal(document.querySelector(".moqui-find"), null);
    } finally {
        window.fetch = realFetch;
    }
});

test("with no model the bar finds by words and says so", async () => {
    document.body.innerHTML = PAGE;
    const realFetch = window.fetch;
    window.fetch = async () => new Response(JSON.stringify({ errors: "No embedding model is configured" }), { status: 503 });
    try {
        const finder = find.attach({ root: "#doc" });
        finder.open();
        const found = await finder.search("service record");
        assert.equal(found.length, 1);
        assert.ok(found[0].text.startsWith(WANTED));
        const note = document.querySelector(".moqui-find .note");
        assert.ok(!note.hidden && note.textContent.includes("No embedding model"));
        finder.detach();
    } finally {
        window.fetch = realFetch;
    }
});
