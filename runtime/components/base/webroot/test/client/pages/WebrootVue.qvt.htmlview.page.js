/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// m-html-view's cleaning in the page. The walk uses this file's fake DOM, the same one the Node file built. hv is moqui.htmlView.
// The library starts the app only when #moqui-config, Vue and Quasar are all present.
"use strict";

const hv = moqui.htmlView;

// ---- a fake DOM: what process and collapse use ----

class FakeNode {
    constructor(type) { this.nodeType = type; this.parentNode = null; this.childNodes = []; }
    get nextSibling() {
        if (!this.parentNode) return null;
        const list = this.parentNode.childNodes;
        return list[list.indexOf(this) + 1] || null;
    }
    get textContent() { return this.nodeType === 3 ? this.data : this.childNodes.map((child) => child.textContent).join(""); }
    set textContent(value) {
        this.childNodes.forEach((child) => { child.parentNode = null; });
        this.childNodes = [];
        if (value) this.appendChild(new FakeText(value));
    }
    appendChild(child) {
        if (child.parentNode) child.parentNode.removeChild(child);
        child.parentNode = this;
        this.childNodes.push(child);
        return child;
    }
    insertBefore(child, ref) {
        if (child.parentNode) child.parentNode.removeChild(child);
        const at = this.childNodes.indexOf(ref);
        child.parentNode = this;
        this.childNodes.splice(at < 0 ? this.childNodes.length : at, 0, child);
        return child;
    }
    removeChild(child) {
        const at = this.childNodes.indexOf(child);
        if (at >= 0) this.childNodes.splice(at, 1);
        child.parentNode = null;
        return child;
    }
    replaceChild(child, old) { this.insertBefore(child, old); this.removeChild(old); return old; }
}
class FakeText extends FakeNode { constructor(data) { super(3); this.data = data; } }
class FakeElement extends FakeNode {
    constructor(tag, attrs) { super(1); this.tagName = tag.toUpperCase(); this.attrs = Object.assign({}, attrs); }
    getAttribute(name) { return Object.prototype.hasOwnProperty.call(this.attrs, name) ? this.attrs[name] : null; }
    setAttribute(name, value) { this.attrs[name] = String(value); }
    removeAttribute(name) { delete this.attrs[name]; }
}
const doc = { createElement: (tag) => new FakeElement(tag) };
const h = (tag, attrs, ...children) => {
    const el = new FakeElement(tag, attrs || {});
    for (const child of children) el.appendChild(typeof child === "string" ? new FakeText(child) : child);
    return el;
};
const html = (node) => {
    if (node.nodeType === 3) return node.data;
    const tag = node.tagName.toLowerCase();
    const attrs = Object.keys(node.attrs).map((key) => " " + key + '="' + node.attrs[key] + '"').join("");
    if (["img", "br", "hr"].includes(tag)) return "<" + tag + attrs + ">";
    return "<" + tag + attrs + ">" + node.childNodes.map(html).join("") + "</" + tag + ">";
};
const inner = (node) => node.childNodes.map(html).join("");

// ---- sources, links, CSS ----

test("an image is shown from data:, from the host's cid: map, or remote only when allowed", () => {
    assert.deepEqual(hv.imageSource("data:image/png;base64,AAAA"), { src: "data:image/png;base64,AAAA", remote: false, blocked: false });
    assert.deepEqual(hv.imageSource("data:text/html;base64,PHNjcmlwdD4="), { src: null, remote: false, blocked: false });
    const cidMap = { "logo@x": "data:image/png;base64,LOGO" };
    assert.equal(hv.imageSource("cid:logo@x", { cidMap }).src, "data:image/png;base64,LOGO");
    assert.equal(hv.imageSource("cid:<LOGO@X>", { cidMap }).src, "data:image/png;base64,LOGO");
    assert.deepEqual(hv.imageSource("cid:missing@x", { cidMap }), { src: null, remote: false, blocked: false });
    assert.deepEqual(hv.imageSource("https://track.example.com/p.gif"), { src: null, remote: true, blocked: true });
    assert.deepEqual(hv.imageSource("https://cdn.example.com/a.png", { allowRemote: true }), { src: "https://cdn.example.com/a.png", remote: true, blocked: false });
    assert.equal(hv.imageSource("//cdn.example.com/a.png", { allowRemote: true }).src, "https://cdn.example.com/a.png");
    assert.equal(hv.imageSource("//cdn.example.com/a.png").blocked, true);
    for (const dropped of ["/apps/secret.png", "a.png", "javascript:alert(1)", "file:///etc/passwd", ""]) {
        assert.deepEqual(hv.imageSource(dropped, { allowRemote: true }), { src: null, remote: false, blocked: false }, dropped);
    }
    assert.equal(hv.cidKey("<Part1.ABC@example.com>"), "part1.abc@example.com");
});

test("links stay for http, https, mailto, tel and anchors; anything else is not a link", () => {
    for (const kept of ["https://example.com/a?b=1", "http://example.com", "mailto:ada@example.com", "tel:+15551234", "#section"]) {
        assert.equal(hv.linkHref(kept), kept);
    }
    assert.equal(hv.linkHref("//example.com/x"), "https://example.com/x");
    for (const dropped of ["javascript:alert(1)", " JAVASCRIPT:alert(1)", "data:text/html,<b>", "/apps/admin", "vbscript:x", "", null]) {
        assert.equal(hv.linkHref(dropped), null, String(dropped));
    }
});

test("the CSS of a message loses remote urls (counted), imports, expressions, escapes and fixed positions", () => {
    const blocked = hv.css("background: url('https://t.example.com/bg.png') no-repeat; color: red", false);
    assert.equal(blocked.css, "background: none no-repeat; color: red");
    assert.equal(blocked.blocked, 1);
    const allowed = hv.css("background-image:url(https://cdn.example.com/bg.png)", true);
    assert.equal(allowed.css, "background-image:url(https://cdn.example.com/bg.png)");
    assert.equal(allowed.blocked, 0);
    assert.equal(hv.css("background:url(data:image/png;base64,AAAA)", false).css, "background:url(data:image/png;base64,AAAA)");
    assert.equal(hv.css("background:url(/apps/x.png)", true).css, "background:none");
    assert.equal(hv.css("@import url(https://x.example.com/a.css); p { color: blue }", true).css, " p { color: blue }");
    assert.equal(hv.css("width: expression(alert(1))", false).css, "width: (alert(1))");
    assert.equal(hv.css("behavior: url(x.htc); -moz-binding: url(x.xml#y)", true).css, "; ");
    const escaped = hv.css("background: \\75 rl(https://t.example.com/x.png)", false);
    assert.equal(escaped.css, "background: none");
    assert.equal(escaped.blocked, 1);
    assert.equal(hv.css("div { position : fixed; top: 0 }", false).css, "div { position:static; top: 0 }");
});

test("a message with its own colors or backgrounds is designed; plain mail follows the theme", () => {
    for (const designed of ['<table bgcolor="#ffffff">', '<td style="background-color:#000">', '<div style="color: #333">',
        "<style>body { background: #eee }</style>", '<font color="red">x</font>', '<td background="https://x/b.png">']) {
        assert.equal(hv.isDesigned(designed), true, designed);
    }
    for (const plain of ['<div dir="ltr">Hi Ada,<br>see you.</div>',
        '<blockquote class="gmail_quote" style="margin:0px 0px 0px 0.8ex;border-left:1px solid rgb(204,204,204);padding-left:1ex">x</blockquote>', ""]) {
        assert.equal(hv.isDesigned(plain), false, plain);
    }
});

// ---- plain text ----

test("plain text is escaped, its addresses and URLs are links, and trailing punctuation stays text", () => {
    assert.equal(hv.autolink(hv.escape("See https://example.com/a?b=1&c=2, or www.example.org.")),
        'See <a href="https://example.com/a?b=1&amp;c=2" target="_blank" rel="noopener noreferrer">https://example.com/a?b=1&amp;c=2</a>, or ' +
        '<a href="https://www.example.org" target="_blank" rel="noopener noreferrer">www.example.org</a>.');
    assert.equal(hv.autolink(hv.escape('Mail "ada@example.com" (or "https://x.example.com/p")')),
        'Mail &quot;<a href="mailto:ada@example.com" target="_blank" rel="noopener noreferrer">ada@example.com</a>&quot; (or &quot;' +
        '<a href="https://x.example.com/p" target="_blank" rel="noopener noreferrer">https://x.example.com/p</a>&quot;)');
    assert.equal(hv.autolink(hv.escape("<script>alert(1)</script>")), "&lt;script&gt;alert(1)&lt;/script&gt;");
});

test("the reply history at the end of a text is collapsed with its attribution; quotes between replies stay", () => {
    const reply = ["Sounds good.", "", "On Mon, Oct 19, 2026 at 10:00 AM Ada <ada@example.com> wrote:", "> Shall we meet?", ">", "> Ada", ""].join("\n");
    assert.equal(hv.quoteStart(reply.split("\n")), 2);
    assert.equal(hv.textToHtml(reply),
        '<div class="m-hv-plain">Sounds good.</div><details class="m-hv-quote">' + hv.summary() +
        '<div class="m-hv-plain">On Mon, Oct 19, 2026 at 10:00 AM Ada &lt;' +
        '<a href="mailto:ada@example.com" target="_blank" rel="noopener noreferrer">ada@example.com</a>&gt; wrote:\n&gt; Shall we meet?\n&gt;\n&gt; Ada\n</div></details>');
    // an attribution on two lines (a long name wrapped)
    assert.equal(hv.quoteStart(["Yes", "On Mon, Oct 19, 2026 at 10:00 AM Ada Lovelace", "<ada@example.com> wrote:", "> Meet?"]), 1);
    const inline = ["> Can you come?", "Yes.", "> And bring the plan?", "Of course."].join("\n");
    assert.equal(hv.quoteStart(inline.split("\n")), -1);
    assert.equal(hv.textToHtml(inline).indexOf("<details"), -1);
    const outlook = ["Done.", "", "-----Original Message-----", "From: Bob", "Sent: Monday", "Do it"];
    assert.equal(hv.quoteStart(outlook), 2);
    assert.equal(hv.textToHtml(outlook.join("\n"), false), '<div class="m-hv-plain">' + hv.escape(outlook.join("\n")) + "</div>");
});

// ---- the walk over a cleaned body ----

test("images: remote ones are placeholders until allowed (pixels dropped), cid: from the map, sizes kept, none for nothing", () => {
    const body = () => h("body", {},
        h("p", {}, "Hi", h("img", { src: "https://cdn.example.com/logo.png", alt: "Logo", width: "120", height: "40" })),
        h("img", { src: "https://t.example.com/open.gif", width: "1", height: "1" }),
        h("img", { src: "cid:chart@x", alt: "Chart" }),
        h("img", { src: "cid:gone@x", alt: "Gone" }),
        h("img", { src: "/apps/secret.png" }));
    const blocked = body();
    const out = hv.process(blocked, doc, { cidMap: { "chart@x": "data:image/png;base64,CHART" } });
    assert.equal(out.blocked, 1);
    assert.equal(inner(blocked),
        '<p>Hi<span class="m-hv-blocked" title="Image hidden" style="width:120px;height:40px">Logo</span></p>' +
        '<img src="data:image/png;base64,CHART" alt="Chart">' +
        '<span class="m-hv-blocked" title="Image not available">Gone</span>');
    const allowed = body();
    assert.equal(hv.process(allowed, doc, { allowRemote: true, cidMap: {} }).blocked, 0);
    assert.equal(html(allowed.childNodes[0].childNodes[1]), '<img src="https://cdn.example.com/logo.png" alt="Logo" width="120" height="40">');
    assert.equal(html(allowed.childNodes[1]), '<img src="https://t.example.com/open.gif" width="1" height="1">');
});

test("links open in a new window without the opener; styles, backgrounds and stray targets are cleaned", () => {
    const body = h("body", {},
        h("a", { href: "https://example.com/x", target: "_self" }, "site"),
        h("a", { href: "javascript:alert(1)" }, "bad"),
        h("a", { href: "#top" }, "top"),
        h("table", { background: "https://t.example.com/bg.png", style: "background:url(https://t.example.com/b2.png);color:#333" },
            h("tr", {}, h("td", { target: "x" }, "cell"))));
    const out = hv.process(body, doc, {});
    assert.equal(out.blocked, 2);
    assert.equal(inner(body),
        '<a href="https://example.com/x" target="_blank" rel="noopener noreferrer">site</a><a>bad</a><a href="#top">top</a>' +
        '<table style="background:none;color:#333"><tr><td>cell</td></tr></table>');
});

test("the reply history at the end of a body is collapsed: Gmail, Apple Mail and a last blockquote; Outlook from its marker on", () => {
    const gmail = h("body", {},
        h("div", { dir: "ltr" }, "Sounds good."),
        h("br", {}),
        h("div", { class: "gmail_quote" }, h("div", { class: "gmail_attr" }, "On Mon, Ada wrote:"),
            h("blockquote", { class: "gmail_quote" }, "Shall we meet?", h("blockquote", {}, "older"))),
        h("div", {}, h("br", {})),
        h("img", { src: "x", width: "1", height: "1" }));
    assert.equal(hv.collapse(gmail, doc), 1);
    assert.equal(inner(gmail),
        '<div dir="ltr">Sounds good.</div><br><details class="m-hv-quote"><summary class="m-hv-more" title="Show trimmed content"' +
        ' aria-label="Show trimmed content">•••</summary><div class="gmail_quote"><div class="gmail_attr">On Mon, Ada wrote:</div>' +
        '<blockquote class="gmail_quote">Shall we meet?<blockquote>older</blockquote></blockquote></div></details><div><br></div>' +
        '<img src="x" width="1" height="1">');

    // a quote with a reply after it stays open
    const between = h("body", {}, h("blockquote", { type: "cite" }, "Can you come?"), h("p", {}, "Yes."));
    assert.equal(hv.collapse(between, doc), 0);
    assert.equal(inner(between), '<blockquote type="cite">Can you come?</blockquote><p>Yes.</p>');

    // Outlook: the separator, the From block and everything after it
    const outlook = h("body", {},
        h("p", {}, "Done."),
        h("div", {}, h("hr", {}), h("div", { id: "divRplyFwdMsg" }, "From: Bob"), h("div", {}, "Do it")));
    assert.equal(hv.collapse(outlook, doc), 1);
    assert.equal(inner(outlook.childNodes[1]),
        '<details class="m-hv-quote"><summary class="m-hv-more" title="Show trimmed content" aria-label="Show trimmed content">' +
        '•••</summary><hr><div id="divRplyFwdMsg">From: Bob</div><div>Do it</div></details>');

    const original = h("body", {}, h("p", {}, "Ok"), h("div", {}, "-----Original Message-----"), h("p", {}, "From: Bob"));
    assert.equal(hv.collapse(original, doc), 1);
    assert.equal(original.childNodes.length, 2);
    assert.equal(original.childNodes[1].tagName, "DETAILS");

    // process collapses unless told not to
    const kept = h("body", {}, h("p", {}, "Hi"), h("blockquote", {}, "old"));
    assert.equal(hv.process(kept, doc, { collapseQuotes: false }).collapsed, 0);
    assert.equal(inner(kept), "<p>Hi</p><blockquote>old</blockquote>");
    assert.equal(hv.process(kept, doc, {}).collapsed, 1);
});

// ---- the frame's document ----

test("the frame's policy allows no script and images only from data: unless remote images are allowed", () => {
    assert.equal(hv.csp(false), "default-src 'none'; img-src data:; style-src 'unsafe-inline'; font-src data:; base-uri 'none'; form-action 'none'; frame-src 'none'");
    assert.equal(hv.csp(true).indexOf("img-src data: https: http:") > 0, true);
    assert.equal(/script-src/.test(hv.csp(true)), false);
    const page = hv.srcdoc({ content: "<p>Hi</p>", css: "p { color: red } </style><script>x()</script>", dark: false });
    assert.ok(page.startsWith("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src &#39;none&#39;; img-src data:;"), page);
    assert.ok(page.includes('<meta name="referrer" content="no-referrer">'));
    assert.ok(page.includes("<style>p { color: red } ><script>x()</script></style>"), page);
    assert.ok(page.endsWith('<body><div id="m-hv-root"><p>Hi</p></div></body></html>'));
    assert.ok(page.includes("color-scheme: light"));
    // dark: a plain message follows the theme, a designed one is a light card
    assert.ok(hv.srcdoc({ content: "", dark: true }).includes("color-scheme: dark"));
    const card = hv.srcdoc({ content: "", dark: true, designed: true });
    assert.ok(card.includes("color-scheme: light") && card.includes("background: #ffffff") && card.includes("padding: 12px"), card);
});

test("the style blocks of a message are taken out and cleaned; the cleaner forbids what reaches the network or runs", () => {
    const split = hv.splitStyles('<html><head><style type="text/css">.a { background: url(https://t.example.com/x.png) }</style></head>' +
        "<body><style>@import url(https://x.example.com/y.css); .b { color: blue }</style><p class=a>Hi</p></body></html>", false);
    assert.equal(split.css, ".a { background: none }\n .b { color: blue }");
    assert.equal(split.blocked, 1);
    assert.equal(split.html, "<html><head></head><body><p class=a>Hi</p></body></html>");
    for (const tag of ["script", "style", "form", "input", "iframe", "object", "embed", "link", "meta", "base", "video", "audio"]) {
        assert.ok(hv.purifyOptions.FORBID_TAGS.includes(tag), tag);
    }
    for (const attr of ["srcset", "ping", "poster", "formaction", "srcdoc"]) assert.ok(hv.purifyOptions.FORBID_ATTR.includes(attr), attr);
    assert.deepEqual(hv.purifyOptions.USE_PROFILES, { html: true });
});
