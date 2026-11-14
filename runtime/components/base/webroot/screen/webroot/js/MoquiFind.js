/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The semantic find of a page (docs/ui/Semantic-Find.md): Ctrl+F (Cmd+F on a Mac) opens a find bar that finds the
 * sentences of the page that mean what was typed, not only the ones with its words. The page's text is cut into
 * sentences, the server ranks them (POST /find/rank, org.moqui.search.SearchServices.rank#Texts, which embeds them with
 * the search facade's embedding model), and the bar highlights the exact text of each match and scrolls to it. With no
 * model the server answers 503 and the bar finds the sentences that have every word typed. Ctrl+F in the bar gives
 * the key to the browser's own find.
 *
 * An app turns it on: moqui.find.attach({root, url, keys, limit, token, ignore}) returns {open, close, search, detach}.
 * No library is needed; the functions that touch no document run under the spec harness (MoquiFind.spec.js).
 */
(function (root, factory) {
    "use strict";
    const node = typeof module === "object" && module.exports;
    const moqui = node ? {} : (root.moqui = root.moqui || {});
    factory(root, moqui);
    if (node) module.exports = moqui;
})(globalThis, function (root, moqui) {
    "use strict";

    // Elements whose text is not the page's: a sentence is never found in them.
    const SKIP = "script,style,noscript,template,textarea,input,select,svg,[hidden],[aria-hidden=true],.moqui-find";
    // Elements that end a sentence: their text is never joined to the text after them.
    const BLOCK = /^(ADDRESS|ARTICLE|ASIDE|BLOCKQUOTE|BR|DD|DETAILS|DIV|DL|DT|FIELDSET|FIGCAPTION|FIGURE|FOOTER|FORM|H[1-6]|HEADER|HR|LI|MAIN|NAV|OL|P|PRE|SECTION|SUMMARY|TABLE|TBODY|TD|TFOOT|TH|THEAD|TR|UL)$/;
    // Characters of one sentence the server ranks; a longer one is cut at a space.
    const MAX_SENTENCE = 600;
    // Sentences one search sends (the server's own limit is 5000).
    const MAX_SENTENCES = 4000;

    // The sentences of a text as [start, end) offsets, spaces around them left out: a sentence ends at . ! or ? before
    // a space, at a line break when lines is true (text shown as written, as in a pre), or at MAX_SENTENCE characters
    // (at the last space before that, when there is one).
    const sentences = (text, lines = true) => {
        const out = [];
        const push = (start, end) => {
            while (start < end && /\s/.test(text[start])) start++;
            while (end > start && /\s/.test(text[end - 1])) end--;
            while (end - start > MAX_SENTENCE) {
                let cut = text.lastIndexOf(" ", start + MAX_SENTENCE);
                if (cut <= start) cut = start + MAX_SENTENCE;
                out.push([start, cut]);
                start = cut;
                while (start < end && /\s/.test(text[start])) start++;
            }
            if (end > start && /[\p{L}\p{N}]/u.test(text.slice(start, end))) out.push([start, end]);
        };
        let start = 0;
        for (let at = 0; at < text.length; at++) {
            const ch = text[at];
            if (ch === "\n" && lines) { push(start, at); start = at + 1; }
            else if ((ch === "." || ch === "!" || ch === "?") && (at + 1 === text.length || /\s/.test(text[at + 1]))) {
                push(start, at + 1); start = at + 1;
            }
        }
        push(start, text.length);
        return out;
    };

    // The lower-case words of a text.
    const words = (text) => String(text || "").toLowerCase().split(/[^\p{L}\p{N}]+/u).filter((word) => word.length);

    // The indexes of the texts that have half or more of the query's words (a word of three letters or more, at the
    // start of a word of the text), the texts with more of them first, then in page order: the find with no model. Half,
    // so a word such as "and" in the query does not drop a text.
    const wordMatches = (query, texts) => {
        const wanted = words(query).filter((word, index, all) => word.length >= 3 && all.indexOf(word) === index);
        if (!wanted.length) return [];
        const least = Math.ceil(wanted.length / 2);
        const out = [];
        texts.forEach((text, index) => {
            const have = words(text);
            const found = wanted.filter((word) => have.some((one) => one.startsWith(word))).length;
            if (found >= least) out.push({ index, found });
        });
        return out.sort((a, b) => b.found - a.found || a.index - b.index).map((one) => one.index);
    };

    // The text of root as runs, one per block: each { text, nodes: [{node, start}], lines }, a node's text starting at
    // start of the run's text, lines true when its line breaks show (white-space pre). A BR or a block element starts a
    // new run.
    const runs = (rootElement, ignore) => {
        const doc = rootElement.ownerDocument;
        const view = doc && doc.defaultView;
        const out = [];
        const showsLines = (element) => !!view && /^(pre|break-spaces)/.test(view.getComputedStyle(element).whiteSpace);
        let lines = showsLines(rootElement);
        let current = { text: "", nodes: [], lines };
        const close = () => { if (current.text.trim()) out.push(current); current = { text: "", nodes: [], lines }; };
        const walk = (element) => {
            for (let child = element.firstChild; child; child = child.nextSibling) {
                if (child.nodeType === 3) {
                    current.nodes.push({ node: child, start: current.text.length });
                    current.text += child.nodeValue;
                } else if (child.nodeType === 1) {
                    if (child.matches(SKIP) || (ignore && child.matches(ignore))) continue;
                    const block = BLOCK.test(child.tagName);
                    const outer = lines;
                    if (block) { close(); lines = current.lines = showsLines(child); }
                    if (child.tagName !== "BR") walk(child);
                    if (block) { close(); lines = current.lines = outer; }
                }
            }
        };
        if (doc) walk(rootElement);
        close();
        return out;
    };

    // The DOM position of offset in a run: [node, offset in the node].
    const position = (run, offset) => {
        let at = run.nodes[0];
        for (const one of run.nodes) {
            if (one.start > offset) break;
            at = one;
        }
        return [at.node, Math.min(offset - at.start, at.node.nodeValue.length)];
    };

    // The page's sentences: each { text, range } (a Range over its exact text, the text as it shows).
    const passages = (rootElement, ignore) => {
        const out = [];
        for (const run of runs(rootElement, ignore)) {
            for (const [start, end] of sentences(run.text, run.lines)) {
                if (out.length >= MAX_SENTENCES) return out;
                const range = rootElement.ownerDocument.createRange();
                range.setStart(...position(run, start));
                range.setEnd(...position(run, end));
                // spaces as they show: a run of them is one
                const text = run.text.slice(start, end);
                out.push({ text: run.lines ? text : text.replace(/\s+/g, " "), range });
            }
        }
        return out;
    };

    // Whether a keydown event is the step of keys ("Mod+F": Ctrl+F, or Cmd+F on a Mac).
    const isKey = (event, keys, isMac) => {
        const parts = String(keys).split("+");
        const key = parts.pop().toLowerCase();
        const want = { ctrl: false, meta: false, alt: false, shift: false };
        for (const part of parts) {
            const name = part.toLowerCase();
            if (name === "mod") want[isMac ? "meta" : "ctrl"] = true;
            else if (name === "cmd") want.meta = true;
            else if (name in want) want[name] = true;
        }
        return String(event.key).toLowerCase() === key && event.ctrlKey === want.ctrl && event.metaKey === want.meta &&
            event.altKey === want.alt && event.shiftKey === want.shift;
    };

    const STYLE = `
.moqui-find{position:fixed;top:.75rem;right:1rem;z-index:2147483000;width:min(32rem,calc(100vw - 2rem));font:14px/1.4 system-ui,sans-serif;
  color:CanvasText;background:Canvas;border:1px solid color-mix(in srgb,CanvasText 25%,transparent);border-radius:.5rem;
  box-shadow:0 .5rem 1.5rem rgb(0 0 0/.25)}
.moqui-find form{display:flex;gap:.5rem;align-items:center;padding:.5rem}
.moqui-find input{flex:1;min-width:0;font:inherit;padding:.3rem .5rem;color:inherit;background:transparent;
  border:1px solid color-mix(in srgb,CanvasText 30%,transparent);border-radius:.3rem}
.moqui-find button{font:inherit;padding:.2rem .5rem;color:inherit;background:transparent;border:0;cursor:pointer}
.moqui-find .count{opacity:.7;white-space:nowrap}
.moqui-find ol{list-style:none;margin:0;padding:0 0 .25rem;max-height:40vh;overflow:auto}
.moqui-find li{padding:.3rem .75rem;cursor:pointer;border-top:1px solid color-mix(in srgb,CanvasText 10%,transparent);
  overflow:hidden;text-overflow:ellipsis;display:-webkit-box;-webkit-line-clamp:2;-webkit-box-orient:vertical}
.moqui-find li[aria-selected=true]{background:color-mix(in srgb,Highlight 25%,transparent)}
.moqui-find .note{padding:0 .75rem .5rem;opacity:.7}
::highlight(moqui-find){background:color-mix(in srgb,Highlight 25%,transparent)}
::highlight(moqui-find-current){background:Mark;color:MarkText}`;

    // Turn the find on for a page. options: root (an element or a selector; the body), url (/find/rank), keys (Mod+F),
    // limit (matches shown, 10), token (a function giving the CSRF token; moqui.sessionToken), ignore (a selector of
    // elements whose text is not searched), isMac. Returns {open(query), close(), search(query), detach()}.
    const attach = (options) => {
        const opts = Object.assign({ url: "/find/rank", keys: "Mod+F", limit: 10 }, options);
        const doc = root.document;
        const rootElement = () => (typeof opts.root === "string" ? doc.querySelector(opts.root) : opts.root) || doc.body;
        const token = () => (opts.token ? opts.token() : moqui.sessionToken) || "";
        const isMac = opts.isMac ?? /Mac|iPhone|iPad/.test(root.navigator ? root.navigator.platform : "");
        const highlights = root.CSS && root.CSS.highlights && root.Highlight ? root.CSS.highlights : null;

        let bar = null, input = null, count = null, list = null, note = null;
        let matches = [], current = -1, seq = 0, timer = 0;

        const post = (query, texts) => root.fetch(opts.url, {
            method: "POST", credentials: "same-origin",
            headers: { "Content-Type": "application/json", Accept: "application/json", "X-CSRF-Token": token() },
            body: JSON.stringify({ query, texts, limit: opts.limit }),
        });

        const paint = () => {
            if (!highlights) return;
            highlights.set("moqui-find", new root.Highlight(...matches.map((match) => match.range)));
            highlights.set("moqui-find-current", new root.Highlight(...(current >= 0 ? [matches[current].range] : [])));
        };
        const show = (at) => {
            if (!matches.length) { current = -1; paint(); count.textContent = input.value.trim() ? "0/0" : ""; return; }
            current = (at + matches.length) % matches.length;
            paint();
            count.textContent = `${current + 1}/${matches.length}`;
            list.querySelectorAll("li").forEach((li, index) => li.setAttribute("aria-selected", String(index === current)));
            const range = matches[current].range;
            const start = range.startContainer.nodeType === 1 ? range.startContainer : range.startContainer.parentElement;
            if (start) start.scrollIntoView({ block: "center", behavior: "smooth" });
            // without the highlight API the current match is the selection
            if (!highlights) { const selection = root.getSelection(); selection.removeAllRanges(); selection.addRange(range); }
        };
        const render = (found, why) => {
            matches = found;
            list.replaceChildren(...found.map((match, index) => {
                const li = doc.createElement("li");
                li.textContent = match.text;
                li.title = match.score == null ? "" : `score ${match.score.toFixed(2)}`;
                li.addEventListener("mousedown", (event) => { event.preventDefault(); show(index); });
                return li;
            }));
            note.textContent = why || "";
            note.hidden = !why;
            show(0);
        };

        const search = async (query) => {
            const mine = ++seq;
            const text = String(query || "").trim();
            if (!text) { render([]); return []; }
            const found = passages(rootElement(), opts.ignore);
            const texts = found.map((one) => one.text);
            let result, why = "";
            try {
                const response = await post(text, texts);
                const body = await response.json().catch(() => ({}));
                if (!response.ok) throw new Error(body.errors || body.message || `status ${response.status}`);
                result = (body.matches || []).map((match) => Object.assign({}, found[match.index], { score: match.score }))
                    .filter((match) => match.range);
            } catch (error) {
                why = `Finding by words: ${error.message || error}`;
                result = wordMatches(text, texts).slice(0, opts.limit).map((index) => found[index]);
            }
            if (mine === seq && bar) render(result, why);
            return result;
        };

        const close = () => {
            if (!bar) return;
            bar.remove(); bar = null; matches = []; current = -1; seq++;
            if (highlights) { highlights.delete("moqui-find"); highlights.delete("moqui-find-current"); }
        };
        const open = (query) => {
            if (!doc.getElementById("moqui-find-style")) {
                const style = doc.createElement("style"); style.id = "moqui-find-style"; style.textContent = STYLE; doc.head.appendChild(style);
            }
            if (!bar) {
                bar = doc.createElement("div"); bar.className = "moqui-find"; bar.setAttribute("role", "search");
                bar.innerHTML = '<form><input type="search" placeholder="Find by meaning" aria-label="Find by meaning">' +
                    '<span class="count"></span><button type="button" data-step="-1" aria-label="Previous">↑</button>' +
                    '<button type="button" data-step="1" aria-label="Next">↓</button><button type="button" data-close aria-label="Close">✕</button>' +
                    '</form><div class="note" hidden></div><ol role="listbox"></ol>';
                input = bar.querySelector("input"); count = bar.querySelector(".count"); list = bar.querySelector("ol"); note = bar.querySelector(".note");
                bar.querySelector("form").addEventListener("submit", (event) => event.preventDefault());
                bar.addEventListener("click", (event) => {
                    const button = event.target.closest("button");
                    if (!button) return;
                    if (button.hasAttribute("data-close")) close(); else show(current + Number(button.dataset.step));
                });
                input.addEventListener("input", () => { clearTimeout(timer); timer = setTimeout(() => { timer = 0; search(input.value); }, 300); });
                input.addEventListener("keydown", (event) => {
                    if (event.key === "Escape") { event.preventDefault(); close(); }
                    else if (event.key === "Enter") {
                        event.preventDefault();
                        if (timer) { clearTimeout(timer); timer = 0; search(input.value); } else show(current + (event.shiftKey ? -1 : 1));
                    }
                });
                doc.body.appendChild(bar);
                // the sentences' vectors are made now, so the first search waits only for its own
                post("", passages(rootElement(), opts.ignore).map((one) => one.text)).catch(() => {});
            }
            if (query != null) { input.value = query; search(query); }
            input.focus(); input.select();
        };

        const onKey = (event) => {
            if (!isKey(event, opts.keys, isMac)) return;
            // the key again in the bar is the browser's find
            if (bar && bar.contains(doc.activeElement)) return;
            event.preventDefault();
            const selected = String(root.getSelection ? root.getSelection() : "").trim();
            open(selected && selected.length < 200 ? selected : null);
        };
        doc.addEventListener("keydown", onKey, true);
        const detach = () => { close(); doc.removeEventListener("keydown", onKey, true); };
        return { open, close, search, detach };
    };

    moqui.find = { attach, sentences, words, wordMatches, passages, isKey };
});
