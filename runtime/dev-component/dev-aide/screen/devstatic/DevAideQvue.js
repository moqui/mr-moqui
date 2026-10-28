/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

/*
 * The Design view of a `.qvue` screen (plans/design-dev-aide.md 11.4, P9): the screen rendered from the tab's buffer
 * (F14: the canvas buffer stands at the `.qvue`'s location) in a frame of the page, its template's elements stamped
 * with their lines (F10, `stamp_qvue`), under an overlay that selects them; the selection's actions are text edits of
 * the template block (remove, move among its siblings, insert a snippet before, after or inside, edit its tag), so the
 * Text view and this one share one buffer and one undo history. The elements come from `pure.vueElements`, the same
 * scan as the server's stamps. The screen's XML is the `.qvue`'s sibling of the same name; its terminal form, when it
 * has one, is the `.tui.json` beside them.
 */
(function (root) {
    "use strict";
    const moqui = root.moqui;
    const devAide = moqui.devAide;
    const pure = devAide.pure;

    // ---- pure ----

    const VOID = ["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr"];

    // The elements of a `.qvue` text's template block, as the server's stamps see them (moqui_core::screen::stamp::
    // qvue_elements): each with its name, the offsets of its `<`, of the end of its name, of the end of its opening tag
    // and of its end (past its closing tag), its line and column (from 1), its parent's index (-1 at the top) and whether
    // it is stamped (an inner `template` or `slot` is not). Comments, quoted values and `{{ }}` are passed over.
    pure.vueElements = (text) => {
        const source = String(text || "");
        const open = source.indexOf("<template");
        if (open < 0) return [];
        const bodyStart = source.indexOf(">", open) + 1;
        if (!bodyStart) return [];
        let tail = source.length;
        for (const tag of ["<script", "<style"]) { const at = source.indexOf(tag, bodyStart); if (at >= 0 && at < tail) tail = at; }
        const close = source.lastIndexOf("</template>", tail);
        const bodyEnd = close >= bodyStart ? close : tail;
        const out = [];
        const stack = [];
        let at = bodyStart;
        const lineCol = (offset) => {
            const before = source.slice(0, offset);
            const line = before.split("\n").length;
            return { line, column: offset - (before.lastIndexOf("\n") + 1) + 1 };
        };
        while (at < bodyEnd) {
            if (source.startsWith("<!--", at)) { const end = source.indexOf("-->", at); at = end < 0 ? bodyEnd : end + 3; continue; }
            if (source.startsWith("{{", at)) { const end = source.indexOf("}}", at); at = end < 0 ? bodyEnd : end + 2; continue; }
            if (source[at] === "<" && source[at + 1] === "/") {
                const end = source.indexOf(">", at);
                const name = source.slice(at + 2, end < 0 ? bodyEnd : end).trim();
                for (let i = stack.length - 1; i >= 0; i--) {
                    if (out[stack[i]].name === name) { out[stack[i]].end = end + 1; stack.length = i; break; }
                }
                at = end < 0 ? bodyEnd : end + 1;
                continue;
            }
            if (source[at] === "<" && /[A-Za-z]/.test(source[at + 1] || "")) {
                let nameEnd = at + 1;
                while (nameEnd < bodyEnd && /[A-Za-z0-9\-:._]/.test(source[nameEnd])) nameEnd++;
                const name = source.slice(at + 1, nameEnd);
                let i = nameEnd, quote = null;
                while (i < bodyEnd) {
                    const ch = source[i];
                    if (quote) { if (ch === quote) quote = null; } else if (ch === "\"" || ch === "'") quote = ch; else if (ch === ">") break;
                    i++;
                }
                const openEnd = i + 1;
                const selfClosing = source[i - 1] === "/" || VOID.indexOf(name.toLowerCase()) >= 0;
                const element = Object.assign({ name, start: at, nameEnd, openEnd, end: selfClosing ? openEnd : -1, selfClosing,
                    parent: stack.length ? stack[stack.length - 1] : -1, stamped: name !== "template" && name !== "slot" }, lineCol(at));
                out.push(element);
                if (!selfClosing) stack.push(out.length - 1);
                at = openEnd;
                continue;
            }
            at++;
        }
        for (const element of out) if (element.end < 0) element.end = bodyEnd;
        return out;
    };
    // The element a stamp names (`<file>:<line>:<column>`), when the file is this one.
    pure.vueElementAt = (elements, stamp, file) => {
        const match = /^(.*):(\d+):(\d+)$/.exec(String(stamp || ""));
        if (!match || (file && match[1] !== file)) return -1;
        return elements.findIndex((element) => element.line === Number(match[2]) && element.column === Number(match[3]));
    };
    // The leading white space of the line an offset is on.
    const indentAt = (text, offset) => { const start = text.lastIndexOf("\n", offset - 1) + 1; return (/^[ \t]*/.exec(text.slice(start)) || [""])[0]; };
    // An edit of the template: `{ text, select }`, the new text and the offset of the element to select after it.
    pure.vueEdit = (text, elements, index, action, snippet) => {
        const element = elements[index];
        if (!element) return null;
        const indent = indentAt(text, element.start);
        const lineStart = text.lastIndexOf("\n", element.start - 1) + 1;
        const ownLine = text.slice(lineStart, element.start).trim() === "";
        const siblings = elements.map((other, i) => ({ other, i })).filter((entry) => entry.other.parent === element.parent);
        const at = siblings.findIndex((entry) => entry.i === index);
        if (action === "remove") {
            // its own lines go with it
            let from = element.start, to = element.end;
            if (ownLine) { from = lineStart; if (text[to] === "\n") to++; }
            return { text: text.slice(0, from) + text.slice(to), select: -1 };
        }
        if (action === "before") return { text: text.slice(0, element.start) + snippet + "\n" + indent + text.slice(element.start), select: element.start };
        if (action === "after") return { text: text.slice(0, element.end) + "\n" + indent + snippet + text.slice(element.end), select: element.end + 1 + indent.length };
        if (action === "inside") {
            // last inside it, on a line of its own one step in
            if (element.selfClosing) return null;
            const closeAt = text.lastIndexOf("</", element.end - 1);
            const closeLine = text.lastIndexOf("\n", closeAt - 1) + 1;
            const inner = indent + "    ";
            if (closeLine > element.start && text.slice(closeLine, closeAt).trim() === "") {
                return { text: text.slice(0, closeLine) + inner + snippet + "\n" + text.slice(closeLine), select: closeLine + inner.length };
            }
            return { text: text.slice(0, closeAt) + "\n" + inner + snippet + "\n" + indent + text.slice(closeAt), select: closeAt + 1 + inner.length };
        }
        if (action === "up" || action === "down") {
            const other = siblings[at + (action === "up" ? -1 : 1)];
            if (!other) return null;
            const [first, second] = action === "up" ? [other.other, element] : [element, other.other];
            const a = text.slice(first.start, first.end), b = text.slice(second.start, second.end), between = text.slice(first.end, second.start);
            const moved = text.slice(0, first.start) + b + between + a + text.slice(second.end);
            return { text: moved, select: action === "up" ? first.start : first.start + b.length + between.length };
        }
        if (action === "tag") {
            return { text: text.slice(0, element.start) + snippet + text.slice(element.openEnd), select: element.start };
        }
        return null;
    };

    // The snippets of the insert palette (11.4): Quasar components, the m-* widgets and layout.
    pure.VUE_SNIPPETS = [
        { group: "Layout", label: "Row", text: "<div class=\"row q-col-gutter-md\"></div>" },
        { group: "Layout", label: "Column", text: "<div class=\"col\"></div>" },
        { group: "Layout", label: "Card", text: "<q-card flat bordered><q-card-section></q-card-section></q-card>" },
        { group: "Layout", label: "Tabs", text: "<q-tabs dense v-model=\"tab\"><q-tab name=\"one\" label=\"One\"></q-tab></q-tabs>" },
        { group: "Quasar", label: "Button", text: "<q-btn unelevated no-caps color=\"primary\" label=\"Button\"></q-btn>" },
        { group: "Quasar", label: "Input", text: "<q-input dense outlined v-model=\"value\" label=\"Label\"></q-input>" },
        { group: "Quasar", label: "Select", text: "<q-select dense outlined v-model=\"value\" :options=\"[]\" label=\"Label\"></q-select>" },
        { group: "Quasar", label: "Table", text: "<q-table flat :rows=\"rows\" :columns=\"columns\" row-key=\"id\"></q-table>" },
        { group: "Quasar", label: "Chip", text: "<q-chip dense square>Chip</q-chip>" },
        { group: "Moqui", label: "Form", text: "<m-form :fields-initial=\"{}\"></m-form>" },
        { group: "Moqui", label: "Text line", text: "<m-text-line name=\"name\" label=\"Name\"></m-text-line>" },
        { group: "Moqui", label: "Drop-down", text: "<m-drop-down name=\"statusId\" label=\"Status\" :options=\"[]\"></m-drop-down>" },
        { group: "Moqui", label: "Date and time", text: "<m-date-time name=\"fromDate\" label=\"From\"></m-date-time>" },
        { group: "Moqui", label: "Display", text: "<m-display :value=\"value\"></m-display>" },
        { group: "Moqui", label: "Markdown", text: "<m-markdown :text=\"text\"></m-markdown>" },
        { group: "Moqui", label: "Chart", text: "<m-chart :config=\"{}\"></m-chart>" },
        { group: "Moqui", label: "Code editor", text: "<m-code-editor v-model=\"text\" language=\"xml\"></m-code-editor>" }
    ];

    // ---- the view ----

    const WIDTHS = [{ label: "390", value: 390 }, { label: "768", value: 768 }, { label: "1280", value: 1280 }, { label: "Full", value: 0 }];
    const newId = () => "q" + Math.random().toString(36).slice(2, 10) + Date.now().toString(36);

    const qvueView = {
        name: "mDevQvueView",
        props: { ctx: { type: Object, required: true }, tab: { type: Object, required: true } },
        data() {
            return { view: null, error: "", mode: "design", width: 0, bufferId: newId(), frameSrc: "", version: 0, selected: -1, hover: null,
                selectedBox: null, tui: null, rendering: false, frameError: "", tagText: "" };
        },
        computed: {
            elements() { return pure.vueElements(this.tab.text); },
            xmlPath() { return this.tab.path.replace(/\.qvue$/, ".xml"); },
            location() { return this.view && this.view.location ? this.view.location.replace(/\.xml$/, ".qvue") : ""; },
            canEdit() { return this.ctx.mayWrite && !this.tab.readOnly; },
            element() { return this.elements[this.selected] || null; },
            frameStyle() { return this.width ? { width: this.width + "px" } : { width: "100%" }; },
            tree() {
                const depth = (element) => { let d = 0; for (let p = element.parent; p >= 0; p = this.elements[p].parent) d++; return d; };
                return this.elements.map((element, index) => ({ index, name: element.name, depth: depth(element), line: element.line }));
            }
        },
        watch: {
            "tab.text"() { this.scheduleRender(); },
            element(element) { this.tagText = element ? this.tab.text.slice(element.start, element.openEnd) : ""; }
        },
        mounted() {
            this.load();
            this.ctx.onSaved(() => this.render());
            this.onScroll = () => this.draw();
            root.addEventListener("resize", this.onScroll);
        },
        beforeUnmount() { clearTimeout(this.renderTimer); root.removeEventListener("resize", this.onScroll); },
        methods: {
            load() {
                this.ctx.get("screenView", { rootId: this.ctx.rootId, path: this.xmlPath }).then((answer) => {
                    this.view = answer.view;
                    this.render();
                }, (error) => { this.error = "The screen of this .qvue (" + this.xmlPath + ") did not load: " + pure.errorText(error); });
                // the terminal form beside it, when the directory has one
                const tuiPath = this.tab.path.replace(/\.qvue$/, ".tui.json");
                const dir = tuiPath.split("/").slice(0, -1).join("/");
                this.ctx.get("list", { rootId: this.ctx.rootId, path: dir }).then((answer) => {
                    const here = (answer.entries || []).some((entry) => entry.path === tuiPath || entry.name === tuiPath.split("/").pop());
                    if (!here) { this.tui = { path: tuiPath, missing: true }; return; }
                    this.ctx.get("file", { rootId: this.ctx.rootId, path: tuiPath }).then((file) => { this.tui = { path: tuiPath, text: file.text || "" }; }, () => {});
                }, () => { this.tui = { path: tuiPath, missing: true }; });
            },
            render() {
                if (!this.view || !this.view.urlPath) { this.frameError = this.view ? "This screen is not mounted under the apps, so it has no page to show." : ""; return; }
                this.rendering = true;
                const files = this.tab.text !== this.tab.saved ? [{ rootId: this.ctx.rootId, path: this.tab.path, text: this.tab.text }] : [];
                this.ctx.post("canvasBuffer", { bufferId: this.bufferId, files: JSON.stringify(files) }).then(() => {
                    const aide = this.mode === "design" ? "&moquiAide=source" : "";
                    const query = this.view.query ? this.view.query + "&" : "";
                    this.frameSrc = this.view.urlPath + "?" + query + "moquiPreview=buffer:" + this.bufferId + aide + "&v=" + (++this.version);
                }, (error) => { this.rendering = false; this.ctx.fail(error); });
            },
            scheduleRender() { clearTimeout(this.renderTimer); this.renderTimer = setTimeout(() => this.render(), 300); },
            setMode(mode) { this.mode = mode; this.hover = null; this.render(); },
            frameLoaded() {
                this.rendering = false;
                const frame = this.$refs.frame;
                const doc = frame && frame.contentDocument;
                if (!doc || this.mode !== "design") return;
                frame.contentWindow.addEventListener("scroll", this.onScroll, { passive: true });
                const stamped = (target) => {
                    let el = target && target.closest ? target.closest("[data-m-src]") : null;
                    while (el && this.location && !String(el.getAttribute("data-m-src")).startsWith(this.location + ":")) el = el.parentElement && el.parentElement.closest("[data-m-src]");
                    return el;
                };
                doc.addEventListener("mouseover", (event) => { this.hoverEl = stamped(event.target); this.draw(); }, true);
                doc.addEventListener("click", (event) => {
                    event.preventDefault(); event.stopPropagation();
                    const el = stamped(event.target);
                    if (el) this.select(pure.vueElementAt(this.elements, el.getAttribute("data-m-src"), this.location));
                }, true);
                doc.addEventListener("submit", (event) => { event.preventDefault(); event.stopPropagation(); }, true);
                // the selection again, by its place in the new render
                setTimeout(() => this.draw(), 400);
            },
            elementInFrame(index) {
                const element = this.elements[index];
                const frame = this.$refs.frame;
                const doc = frame && frame.contentDocument;
                if (!element || !doc) return null;
                return doc.querySelector("[data-m-src=\"" + (this.location + ":" + element.line + ":" + element.column).replace(/"/g, "\\\"") + "\"]");
            },
            boxOf(el) {
                const frame = this.$refs.frame;
                if (!el || !frame || !el.isConnected) return null;
                const rect = el.getBoundingClientRect();
                return { left: frame.offsetLeft + rect.left, top: frame.offsetTop + rect.top, width: rect.width, height: rect.height };
            },
            draw() {
                const hover = this.hoverEl;
                const index = hover ? pure.vueElementAt(this.elements, hover.getAttribute("data-m-src"), this.location) : -1;
                this.hover = hover ? Object.assign({ label: (this.elements[index] || {}).name || hover.tagName.toLowerCase() }, this.boxOf(hover)) : null;
                this.selectedBox = this.boxOf(this.elementInFrame(this.selected));
            },
            select(index) { this.selected = index; this.$nextTick(() => this.draw()); },
            // an edit of the template, as a change of the tab's buffer
            act(action, snippet) {
                if (!this.canEdit) return;
                const edit = pure.vueEdit(this.tab.text, this.elements, this.selected, action, snippet);
                if (!edit) { this.ctx.note("That does not fit here", "warning"); return; }
                this.tab.text = edit.text;
                this.$nextTick(() => { this.selected = edit.select < 0 ? -1 : this.elements.findIndex((element) => element.start === edit.select); });
            },
            applyTag() { if (this.element && this.tagText && this.tagText !== this.tab.text.slice(this.element.start, this.element.openEnd)) this.act("tag", this.tagText); },
            openInText() { const element = this.element; this.ctx.showText(); if (element) this.ctx.openFile(this.tab.path, element.line); }
        },
        template: `
<div class="m-dev-qvue-view column no-wrap" id="DevQvueView">
  <div class="row items-center no-wrap q-gutter-x-sm q-px-sm q-py-xs">
    <q-btn-toggle dense flat no-caps :model-value="mode" @update:model-value="setMode" :options="[{ label: 'Design', value: 'design' }, { label: 'Preview', value: 'preview' }]"></q-btn-toggle>
    <q-btn-toggle dense flat no-caps v-model="width" :options="widths"></q-btn-toggle>
    <q-spinner v-if="rendering" size="1em"></q-spinner>
    <q-space></q-space>
    <span class="text-caption m-muted ellipsis">{{xmlPath.split('/').pop()}} · {{tab.path.split('/').pop()}}</span>
  </div>
  <div v-if="error" class="q-pa-md text-negative">{{error}}</div>
  <div v-else class="col row no-wrap">
    <div class="col relative-position scroll m-dev-canvas" @mouseleave="hover = null">
      <div v-if="frameError" class="q-pa-md m-muted">{{frameError}}</div>
      <iframe v-if="frameSrc" ref="frame" :src="frameSrc" :style="frameStyle" class="m-dev-frame" @load="frameLoaded" id="DevQvueFrame"></iframe>
      <div v-if="hover && mode === 'design'" class="m-dev-hover" :style="{ left: hover.left + 'px', top: hover.top + 'px', width: hover.width + 'px', height: hover.height + 'px' }">
        <span class="m-dev-hover-label">{{hover.label}}</span></div>
      <div v-if="selectedBox && mode === 'design'" class="m-dev-selected" :style="{ left: selectedBox.left + 'px', top: selectedBox.top + 'px', width: selectedBox.width + 'px', height: selectedBox.height + 'px' }"></div>
    </div>
    <div class="m-dev-side column no-wrap scroll q-pa-xs" style="width: 300px" id="DevQvuePanel">
      <template v-if="element">
        <div class="text-subtitle2">&lt;{{element.name}}&gt; <span class="text-caption m-muted">line {{element.line}}</span></div>
        <q-input dense outlined autogrow v-model="tagText" label="Opening tag" class="m-dev-mono" :disable="!canEdit" @keyup.ctrl.enter="applyTag" id="DevQvueTag"></q-input>
        <div class="row q-gutter-xs q-mt-xs">
          <q-btn dense flat no-caps size="sm" label="Apply tag" :disable="!canEdit" @click="applyTag"></q-btn>
          <q-btn dense flat no-caps size="sm" icon="arrow_upward" :disable="!canEdit" @click="act('up')"><q-tooltip>Move up</q-tooltip></q-btn>
          <q-btn dense flat no-caps size="sm" icon="arrow_downward" :disable="!canEdit" @click="act('down')"><q-tooltip>Move down</q-tooltip></q-btn>
          <q-btn dense flat no-caps size="sm" label="Remove" :disable="!canEdit" @click="act('remove')" id="DevQvueRemove"></q-btn>
          <q-btn dense flat no-caps size="sm" label="Open in Text" @click="openInText"></q-btn>
        </div>
        <div class="text-caption text-weight-medium q-mt-sm">Insert</div>
        <div v-for="group in ['Layout', 'Quasar', 'Moqui']" :key="group">
          <div class="text-caption m-muted">{{group}}</div>
          <div class="row q-gutter-xs">
            <q-btn-dropdown v-for="snippet in snippets.filter(s => s.group === group)" :key="snippet.label" dense flat no-caps size="sm" :label="snippet.label"
                :disable="!canEdit" :data-snippet="snippet.label">
              <q-list dense>
                <q-item clickable v-close-popup @click="act('before', snippet.text)"><q-item-section>Before</q-item-section></q-item>
                <q-item clickable v-close-popup @click="act('after', snippet.text)"><q-item-section>After</q-item-section></q-item>
                <q-item clickable v-close-popup :disable="element.selfClosing" @click="act('inside', snippet.text)"><q-item-section>Inside</q-item-section></q-item>
              </q-list>
            </q-btn-dropdown>
          </div>
        </div>
      </template>
      <div v-else class="text-caption m-muted">Select an element on the canvas or in the outline.</div>
      <div class="text-caption text-weight-medium q-mt-md">Outline</div>
      <q-list dense>
        <q-item v-for="row in tree" :key="row.index" clickable dense :active="row.index === selected" @click="select(row.index)" :data-element="row.name">
          <q-item-section :style="{ paddingLeft: (row.depth * 12) + 'px' }" class="m-dev-mono text-caption">{{row.name}}</q-item-section>
          <q-item-section side class="text-caption">{{row.line}}</q-item-section>
        </q-item>
      </q-list>
      <div class="text-caption text-weight-medium q-mt-md">Terminal form</div>
      <div v-if="tui && tui.missing" class="text-caption m-muted" id="DevQvueTuiMissing">No {{tui.path.split('/').pop()}}: the terminal shows a placeholder for this screen (moqui lint --screens lists it).</div>
      <pre v-else-if="tui" class="m-dev-mono text-caption" style="white-space: pre-wrap; max-height: 12rem; overflow: auto">{{tui.text}}</pre>
    </div>
  </div>
</div>`
    };
    qvueView.data = ((data) => function () { return Object.assign(data.call(this), { widths: WIDTHS, snippets: pure.VUE_SNIPPETS }); })(qvueView.data);

    const qvt = moqui.qvt;
    if (qvt && qvt.registerLate) qvt.registerLate("m-dev-qvue-view", qvueView);
    if (root.document && !root.document.getElementById("m-dev-qvue-style")) {
        const style = root.document.createElement("style");
        style.id = "m-dev-qvue-style";
        style.textContent = ".m-dev-qvue-view .m-dev-frame{border:0;min-height:100%;height:100%;display:block;margin:0 auto}" +
            ".m-dev-qvue-view .m-dev-canvas{background:rgba(127,127,127,.08)}" +
            ".m-dev-qvue-view .m-dev-hover{position:absolute;pointer-events:none;outline:1px dashed #1e88e5}" +
            ".m-dev-qvue-view .m-dev-hover-label{position:absolute;top:-16px;left:0;font-size:10px;background:#1e88e5;color:#fff;padding:0 4px;white-space:nowrap}" +
            ".m-dev-qvue-view .m-dev-selected{position:absolute;pointer-events:none;outline:2px solid #e2a400}";
        root.document.head.appendChild(style);
    }
    devAide.qvueViewRegistered = true;
})(typeof window !== "undefined" ? window : globalThis);
