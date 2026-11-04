/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// m-markdown in the page. moqui is /js/MoquiLib.js and /js/WebrootVue.qvt.js.
// marked is the UMD global from /libs/marked/marked.umd.min.js (Node required that same build).
// The resolver test replaces console.warn. The page records a real console.warn as a problem, so the stub stays.
"use strict";

const marked = globalThis.marked;
const md = moqui.markdown;

test("[[path]] and [[path|text]] are wiki: links outside fenced code and code spans", () => {
    assert.equal(md.expandWikiLinks("See [[Policies/Travel]] and [[Home|the start]]."),
        "See [Policies/Travel](wiki:Policies/Travel) and [the start](wiki:Home).");
    assert.equal(md.expandWikiLinks("a `[[not]]` b"), "a `[[not]]` b");
    assert.equal(md.expandWikiLinks("```\n[[in code]]\n```\n[[out]]"), "```\n[[in code]]\n```\n[out](wiki:out)");
    assert.equal(md.expandWikiLinks("[[Two Words/And more]]"), "[Two Words/And more](wiki:Two%20Words/And%20more)");
});

test("a heading's id is its words, made unique, after the prefix; the outline lists the headings", () => {
    assert.equal(md.slug("Hello, World!"), "hello-world");
    assert.equal(md.slug("Größe & Café"), "größe-café");
    assert.equal(md.slug("***"), "section");
    const out = md.render(marked, "# Intro\n\n## Setup *now*\n\n## Setup now\n\n### A &amp; B", {});
    assert.match(out.html, /<h1 id="md-intro">Intro<\/h1>/);
    assert.match(out.html, /<h2 id="md-setup-now">Setup <em>now<\/em><\/h2>/);
    assert.match(out.html, /<h2 id="md-setup-now-1">Setup now<\/h2>/);
    assert.deepEqual(out.outline, [
        { id: "md-intro", level: 1, text: "Intro" }, { id: "md-setup-now", level: 2, text: "Setup now" },
        { id: "md-setup-now-1", level: 2, text: "Setup now" }, { id: "md-a-b", level: 3, text: "A & B" }
    ]);
    const prefixed = md.render(marked, "## One", { headingPrefix: "k-" });
    assert.match(prefixed.html, /<h2 id="k-one">/);
});

test("a first heading that is the screen's title is left out", () => {
    const out = md.render(marked, "# Travel **Policy**\n\nText\n\n# Travel Policy", { stripTitle: "Travel Policy" });
    assert.equal(out.html.indexOf("<h1"), out.html.lastIndexOf("<h1"), out.html);
    assert.match(out.html, /^<p>Text<\/p>/);
    assert.equal(out.outline.length, 1);
    assert.match(md.render(marked, "Intro\n\n# Travel Policy", { stripTitle: "Travel Policy" }).html, /<h1/, "only a first heading");
});

test("links: external in a new tab with noopener, an anchor to its heading, a relative one from the base path, a path through the shell", () => {
    const out = md.render(marked, "[site](https://example.com \"Ex\") [mail](mailto:a@b.c) [top](#Intro) [doc](guide/Start?x=1) [up](../Other) [abs](/apps/x)",
        { basePath: "/apps/wiki/Space" });
    assert.match(out.html, /<a href="https:\/\/example.com" title="Ex" target="_blank" rel="noopener noreferrer">site<\/a>/);
    assert.match(out.html, /<a href="mailto:a@b.c">mail<\/a>/);
    assert.match(out.html, /<a href="#Intro" data-md-anchor="md-intro">top<\/a>/);
    assert.match(out.html, /<a href="\/apps\/wiki\/Space\/guide\/Start\?x=1" data-md-link>doc<\/a>/);
    assert.match(out.html, /<a href="\/apps\/wiki\/Other" data-md-link>up<\/a>/);
    assert.match(out.html, /<a href="\/apps\/x" data-md-link>abs<\/a>/);
    assert.equal(md.linkKind("javascript:alert(1)"), "external", "another scheme is never a shell path (DOMPurify drops it)");
    assert.equal(md.linkKind("//cdn.example.com/x"), "external");
    assert.equal(md.linkKind("wiki:Home"), "wiki");
    assert.equal(md.resolvePath("../../x", "/a"), "/x");
});

test("a resolver decides a link: a new href, a missing page in the negative color, or the default", () => {
    const seen = [];
    const resolveLink = (href, info) => {
        seen.push(href + ":" + info.kind + (info.image ? ":image" : ""));
        if (href.startsWith("wiki:")) return { href: "/qapps/commons/Knowledge?pagePath=" + encodeURIComponent(href.slice(5)), missing: href === "wiki:Gone", title: "Page" };
        if (href === "pic.png") return "/files/pic.png";
        return null;
    };
    const out = md.render(marked, "[[Home]] [[Gone]] [x](https://x.org) ![a picture](pic.png)", { wikiLinks: true, resolveLink, basePath: "/b" });
    assert.match(out.html, /<a href="\/qapps\/commons\/Knowledge\?pagePath=Home" title="Page" data-md-link>Home<\/a>/);
    assert.match(out.html, /<a href="\/qapps\/commons\/Knowledge\?pagePath=Gone" title="Page" class="text-negative" data-md-link>Gone<\/a>/);
    assert.match(out.html, /<a href="https:\/\/x.org" target="_blank" rel="noopener noreferrer">x<\/a>/, "null keeps the default");
    assert.match(out.html, /<img src="\/files\/pic.png" alt="a picture">/);
    assert.deepEqual(seen, ["wiki:Home:wiki", "wiki:Gone:wiki", "https://x.org:external", "pic.png:relative:image"]);
    // a resolver that throws leaves the link as it is
    const warn = console.warn;
    const warned = [];
    console.warn = (...args) => warned.push(args.join(" "));
    let safe;
    try { safe = md.render(marked, "[a](b)", { resolveLink: () => { throw new Error("no"); }, basePath: "/p" }); } finally { console.warn = warn; }
    assert.equal(warned.length, 1);
    assert.match(safe.html, /<a href="\/p\/b" data-md-link>a<\/a>/);
});

test("a table is in a scrolling q-table container, code keeps its language, and raw HTML waits for DOMPurify", () => {
    const out = md.render(marked, "| a | b |\n|---|---|\n| 1 | 2 |\n\n```js\nlet x = 1 < 2\n```\n\n<script>alert(1)</script>", {});
    assert.match(out.html, /<div class="m-md-table q-table__container q-table--flat q-table--bordered q-table--dense"><table class="q-table">/);
    assert.match(out.html, /<\/table><\/div>/);
    assert.match(out.html, /<pre><code class="language-js">let x = 1 &lt; 2/);
    let options = null;
    const cleaned = md.sanitize({ sanitize: (html, given) => { options = given; return html.replace(/<script>.*<\/script>/, ""); } }, out.html);
    assert.ok(cleaned.indexOf("<script") < 0);
    assert.deepEqual(options.ADD_ATTR, ["target"]);
    assert.ok(options.FORBID_TAGS.includes("style") && options.FORBID_TAGS.includes("form"));
    assert.deepEqual(options.FORBID_ATTR, ["style"]);
    assert.deepEqual(md.render(marked, "", {}), { html: "", outline: [] });
});

test("the markdown component's template compiles and takes its props", () => {
    const comp = moqui.qvt.components["m-markdown"];
    assert.ok(comp);
    ["text", "basePath", "resolveLink", "headingPrefix", "stripTitle", "wikiLinks", "mermaid", "copyable"].forEach((name) => assert.ok(Object.keys(comp.props).includes(name), name));
    assert.deepEqual(comp.emits, ["outline", "rendered", "link"]);
});

test("copyable: the template has the copy button, and copy writes the text to the clipboard and says so", async () => {
    const comp = moqui.qvt.components["m-markdown"];
    assert.match(comp.template, /<q-btn v-if="copyable" class="m-markdown-copy"[^>]*:aria-label="copyLabel" @click.stop="copy">/);
    const written = [];
    const notified = [];
    const had = Object.getOwnPropertyDescriptor(navigator, "clipboard");
    Object.defineProperty(navigator, "clipboard", { configurable: true,
        value: { writeText: (text) => { written.push(text); return Promise.resolve(); } } });
    try {
        comp.methods.copy.call({ text: "# Doc\n\nText", $q: { notify: (opts) => notified.push(opts) } });
        await Promise.resolve();
        await Promise.resolve();
    } finally {
        if (had) Object.defineProperty(navigator, "clipboard", had); else delete navigator.clipboard;
    }
    assert.deepEqual(written, ["# Doc\n\nText"]);
    assert.equal(notified.length, 1);
    assert.equal(notified[0].message, "Copied");
});

test("a mermaid fence is a pre for the diagram, its text escaped; mermaid false and the other languages stay code blocks", () => {
    const text = "```mermaid\nflowchart LR\n  a[\"A <b>\"] --> b\n```\n\n```js\nx\n```";
    const out = md.render(marked, text, {});
    assert.match(out.html, /<pre class="m-md-mermaid">flowchart LR\n {2}a\[&quot;A &lt;b&gt;&quot;\] --&gt; b<\/pre>/);
    assert.match(out.html, /<pre><code class="language-js">x/);
    assert.match(md.render(marked, text, { mermaid: false }).html, /<pre><code class="language-mermaid">flowchart LR/);
    assert.match(md.render(marked, "```mermaid extra\ngraph TD\n```", {}).html, /<pre class="m-md-mermaid">graph TD/);
    assert.ok(md.render(marked, "```mermaidish\nx\n```", {}).html.indexOf("m-md-mermaid") < 0);
});

test("moqui.mermaid draws a diagram from a pre's text, with strict security", async () => {
    const node = document.createElement("pre");
    node.textContent = "flowchart LR\n  a[\"One\"] --> b[\"Two\"]";
    document.body.appendChild(node);
    try {
        await moqui.mermaid.run([node], { theme: "default" });
        assert.ok(node.querySelector("svg"), "an svg in the pre");
        assert.equal(globalThis.mermaid.mermaidAPI.getConfig().securityLevel, "strict");
    } finally { node.remove(); }
});

test("a drawn diagram's size is its viewBox, a click is a fraction of it, and it gets one zoom button", () => {
    assert.ok(moqui.qvt.components["m-diagram-zoom"]);
    assert.deepEqual(Object.keys(moqui.qvt.components["m-diagram-zoom"].props), ["modelValue", "source", "at", "title"]);
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("viewBox", "-8 -8 1200.5 340");
    assert.deepEqual(moqui.mermaid.size(svg), { width: 1200.5, height: 340 });
    const pre = document.createElement("pre");
    pre.style.cssText = "position:fixed;left:0;top:0;margin:0;padding:0";
    svg.setAttribute("width", "400");
    svg.setAttribute("height", "100");
    svg.style.display = "block";
    pre.appendChild(svg);
    document.body.appendChild(pre);
    try {
        const at = moqui.mermaid.pointIn(svg, { clientX: 100, clientY: 75 });
        assert.deepEqual(at, { fx: 0.25, fy: 0.75 });
        assert.equal(moqui.mermaid.pointIn(svg, { clientX: 500, clientY: 10 }), null);
        svg.removeAttribute("viewBox");
        assert.deepEqual(moqui.mermaid.size(svg), { width: 400, height: 100 });
        const button = moqui.mermaid.zoomButton(pre);
        assert.ok(button && button.classList.contains("m-md-mermaid-zoom"));
        assert.equal(button.getAttribute("aria-label"), "Zoom diagram");
        assert.equal(moqui.mermaid.zoomButton(pre), null, "only one");
        assert.equal(pre.querySelectorAll(".m-md-mermaid-zoom").length, 1);
    } finally { pre.remove(); }
});

// The goldens of the Autodoc diagrams (crates/moqui-devkit/src/autodoc/testdata/mermaid), parsed by this Mermaid: what
// the generator writes is a diagram the docs can draw.
const AUTODOC_DIAGRAMS = ["usage.mmd", "usage-empty.mmd", "er.mmd", "class.mmd", "flow.mmd"];

test("every Autodoc diagram golden parses with the vendored Mermaid", async () => {
    await moqui.mermaid.load();
    let rejected = false;
    try { await globalThis.mermaid.parse("flowchart LR\n  a[\"x\" --> "); } catch (err) { rejected = true; }
    assert.ok(rejected, "a broken diagram does not parse");
    for (const name of AUTODOC_DIAGRAMS) {
        const response = await fetch("/files/crates/moqui-devkit/src/autodoc/testdata/mermaid/" + name);
        assert.ok(response.ok, name + " is served");
        const text = await response.text();
        let problem = "";
        try { await globalThis.mermaid.parse(text); } catch (err) { problem = name + ": " + (err && err.message || err); }
        assert.equal(problem, "");
    }
    // every diagram of the fixture project's generated docs
    const tree = await (await fetch("/files/crates/moqui-devkit/src/autodoc/testdata/mermaid/tree.json")).json();
    assert.ok(tree.length > 10, "the tree's diagrams are served");
    const problems = [];
    for (const diagram of tree) {
        try { await globalThis.mermaid.parse(diagram.text); } catch (err) { problems.push(diagram.doc + ": " + (err && err.message || err)); }
    }
    assert.deepEqual(problems, []);
});
