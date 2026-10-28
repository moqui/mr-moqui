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
 * The Artifact Browser canvas (plans/design-dev-aide.md 7.3, D9, P9): the current artifact in the center, what uses it
 * in a card to its left and what it uses in a card to its right (above and below in the vertical orientation), each
 * card a section per kind; expanding a row adds a card in the next column out with that row's own users or uses, joined
 * to the row by one connector. Absolutely placed Quasar cards on m-pan-zoom with one SVG of connectors beneath them; the
 * layout is a pure function (columns by generation, cards in their source rows' order, each at its row's height and
 * pushed past the card above it, in one sweep) over the sizes measured after render. The data is the index's
 * (get#ArtifactDetail and get#Refs, the Workspace's transitions); Refs is the same as lists, for the keyboard and the
 * terminal.
 */
(function (root) {
    "use strict";
    const moqui = root.moqui = root.moqui || {};
    const devAide = moqui.devAide = moqui.devAide || {};
    const pure = devAide.pure = devAide.pure || {};

    // ---- pure ----

    // The kinds of the framework's own components, hidden by default when the center is not one of theirs.
    pure.frameworkComponents = ["framework", "webroot", "tools"];
    // The rows a section shows before "+ n more".
    pure.sectionRows = 8;

    // The layout of the cards and their connectors are DevAide.js's (pure.browserLayout, pure.connectorPath), which the
    // entity view's member diagram shares.

    // The references of an artifact as a card's sections: by the other side's kind, each with its rows (the other side's
    // id, short and full name, how it refers, its observed count, whether only observed), filtered as the browser is.
    pure.browserSections = (refs, filter) => {
        const f = Object.assign({ hideFramework: false, kinds: null, observed: true, staticLinks: true }, filter || {});
        const sections = {};
        for (const entry of refs || []) {
            const other = entry.other || {};
            const kind = other.kind || entry.toKind || "other";
            if (f.kinds && f.kinds.length && f.kinds.indexOf(kind) < 0) continue;
            if (f.hideFramework && pure.frameworkComponents.indexOf(other.component) >= 0) continue;
            if (entry.observedOnly ? !f.observed : !f.staticLinks) continue;
            const id = other.id || entry.toId || (kind + ":" + entry.toName);
            const section = sections[kind] = sections[kind] || { kind, rows: [] };
            const have = section.rows.find((row) => row.id === id);
            if (have) { have.calls += entry.calls || 0; if (have.how.indexOf(entry.refType) < 0) have.how += ", " + entry.refType; continue; }
            section.rows.push({ id, kind, shortName: other.shortName || entry.toName || id, name: other.name || entry.toName || id,
                component: other.component || "", how: entry.refType || "", calls: entry.calls || 0, observedOnly: !!entry.observedOnly,
                resolved: !!other.id, url: entry.url || other.url || "" });
        }
        return Object.keys(sections).sort().map((kind) => sections[kind]);
    };

    // Whether `id` is on the path from the center to `card` (a cycle, not expanded again).
    pure.onPath = (cards, card, id) => {
        for (let at = card; at; at = at.parentId ? cards.find((other) => other.id === at.parentId) : null) if (at.artifactId === id) return true;
        return false;
    };

    // ---- the component ----

    const kindIcon = { entity: "sym_o_table", "view-entity": "sym_o_view_column", service: "sym_o_settings", screen: "sym_o_web",
        transition: "sym_o_arrow_forward", form: "sym_o_list_alt", seca: "sym_o_bolt", eeca: "sym_o_bolt", rest: "sym_o_api",
        job: "sym_o_schedule", document: "sym_o_description", script: "sym_o_code", field: "sym_o_label" };

    const browser = {
        name: "mDevArtifactBrowser",
        props: { base: { type: String, required: true }, artifactId: String },
        data() {
            return {
                cards: [], sizes: {}, trail: [], at: -1, vertical: !!(root.innerWidth && root.innerWidth < 1024), hover: null,
                filter: { hideFramework: true, kinds: [], observed: true, staticLinks: true }, open: {}, error: ""
            };
        },
        computed: {
            center() { return this.cards.find((card) => card.generation === 0) || null; },
            places() { return pure.browserLayout(this.cards, this.sizes, { vertical: this.vertical, columnWidth: 280, gap: this.vertical ? 40 : 80 }); },
            allKinds() {
                const kinds = new Set();
                for (const card of this.cards) for (const entry of card.refs || []) kinds.add((entry.other && entry.other.kind) || entry.toKind);
                return Array.from(kinds).filter(Boolean).sort();
            },
            connectors() {
                const out = [];
                for (const card of this.cards) {
                    if (!card.parentId) continue;
                    const parent = this.cards.find((other) => other.id === card.parentId);
                    const from = this.places[card.parentId], to = this.places[card.id];
                    if (!parent || !from || !to) continue;
                    const anchor = (parent.anchors || {})[card.rowId] || 24;
                    out.push({ id: card.id, d: pure.connectorPath(from, anchor, to, card.generation < 0, this.vertical), dashed: card.observedOnly });
                }
                return out;
            }
        },
        watch: {
            artifactId(id) { if (id) this.go(id); },
            vertical() { this.$nextTick(() => this.measure()); },
            filter: { deep: true, handler() { this.$nextTick(() => this.measure()); } }
        },
        mounted() {
            this.observer = root.ResizeObserver ? new root.ResizeObserver(() => this.measure()) : null;
            if (this.artifactId) this.go(this.artifactId);
        },
        beforeUnmount() { if (this.observer) this.observer.disconnect(); },
        methods: {
            get(name, params) { return moqui.request(this.base + "/" + name, { params }).then((result) => result.json || {}); },
            icon(kind) { return kindIcon[kind] || "sym_o_category"; },
            // a new center, kept on the trail for Back and Forward
            go(id, fromTrail) {
                if (!fromTrail) { this.trail = this.trail.slice(0, this.at + 1).concat([id]); this.at = this.trail.length - 1; }
                this.error = ""; this.cards = []; this.sizes = {}; this.open = {};
                this.get("artifactDetail", { artifactId: id }).then((detail) => {
                    if (!detail.artifact) { this.error = "No artifact " + id + " in the index"; return; }
                    const center = { id: "c", artifactId: detail.artifact.id, artifact: detail.artifact, generation: 0, parentId: null };
                    // the framework's own artifacts show when the center is one of them
                    this.filter.hideFramework = pure.frameworkComponents.indexOf(detail.artifact.component) < 0;
                    this.cards = [center];
                    this.addSide(center, "in", -1);
                    this.addSide(center, "out", 1);
                    this.$nextTick(() => this.fitSoon());
                }, (error) => { this.error = (devAide.pure.errorText || String)(error); });
            },
            back() { if (this.at > 0) { this.at--; this.go(this.trail[this.at], true); } },
            forward() { if (this.at < this.trail.length - 1) { this.at++; this.go(this.trail[this.at], true); } },
            // the card of one side of `parent`: its users (in, before it) or its uses (out, after it)
            addSide(parent, direction, step, row) {
                const card = { id: parent.id + (row ? "/" + row.id : "") + ":" + direction, artifactId: row ? row.id : parent.artifactId,
                    direction, generation: parent.generation + step, parentId: parent.id, rowId: row ? row.id : null, order: row ? row.index : 0,
                    anchor: 0, refs: null, loading: true, observedOnly: row ? row.observedOnly : false, title: row ? row.shortName : (direction === "in" ? "used by" : "uses") };
                this.cards.push(card);
                // the reactive card, whose changes render
                const live = this.cards[this.cards.length - 1];
                this.get("refs", { artifactId: card.artifactId, direction }).then((answer) => {
                    live.refs = answer.refs || []; live.loading = false;
                    this.$nextTick(() => this.measure());
                }, (error) => { live.loading = false; live.error = (devAide.pure.errorText || String)(error); });
            },
            sections(card) { return pure.browserSections(card.refs, this.filter); },
            shown(card, section) {
                const key = card.id + "|" + section.kind;
                return this.open[key] ? section.rows : section.rows.slice(0, pure.sectionRows);
            },
            more(card, section) { this.open[card.id + "|" + section.kind] = true; this.$nextTick(() => this.measure()); },
            childOf(card, row) { return this.cards.find((other) => other.parentId === card.id && other.rowId === row.id) || null; },
            cycle(card, row) { return pure.onPath(this.cards, card, row.id); },
            // expand a row into the next column out, or collapse what grew from it
            toggle(card, row, index) {
                const child = this.childOf(card, row);
                if (child) { this.removeFrom(child); this.$nextTick(() => this.measure()); return; }
                if (!row.resolved || this.cycle(card, row)) return;
                this.addSide(card, card.direction, card.direction === "in" ? -1 : 1, Object.assign({ index }, row));
            },
            removeFrom(card) {
                for (const child of this.cards.filter((other) => other.parentId === card.id)) this.removeFrom(child);
                this.cards = this.cards.filter((other) => other.id !== card.id);
            },
            // the cards' sizes and their rows' offsets, after render
            measure() {
                const sizes = {};
                for (const card of this.cards) {
                    const el = this.$refs["card-" + card.id];
                    const node = Array.isArray(el) ? el[0] : el;
                    if (!node || !node.$el && !node.getBoundingClientRect) continue;
                    const dom = node.$el || node;
                    if (this.observer && !dom.__mObserved) { this.observer.observe(dom); dom.__mObserved = true; }
                    sizes[card.id] = { width: dom.offsetWidth, height: dom.offsetHeight };
                    const anchors = {};
                    for (const rowEl of dom.querySelectorAll("[data-row]")) {
                        anchors[rowEl.getAttribute("data-row")] = this.vertical ? rowEl.offsetLeft + rowEl.offsetWidth / 2 : rowEl.offsetTop + rowEl.offsetHeight / 2;
                    }
                    card.anchors = anchors;
                    const parent = this.cards.find((other) => other.id === card.parentId);
                    if (parent && card.rowId && parent.anchors) card.anchor = parent.anchors[card.rowId] || 0;
                }
                const changed = Object.keys(sizes).some((id) => !this.sizes[id] || this.sizes[id].width !== sizes[id].width || this.sizes[id].height !== sizes[id].height);
                if (changed) this.sizes = sizes;
            },
            fitSoon() { setTimeout(() => { this.measure(); this.$nextTick(() => this.$refs.layer && this.$refs.layer.fit()); }, 150); },
            fit() { if (this.$refs.layer) this.$refs.layer.fit(); },
            centerView() {
                const place = this.places.c;
                if (place && this.$refs.layer) this.$refs.layer.centerOn({ x: place.x + place.width / 2, y: place.y + place.height / 2 });
            },
            style(card) {
                const place = this.places[card.id];
                return place ? { left: place.x + "px", top: place.y + "px", width: card.generation === 0 ? "260px" : "280px" }
                    : { left: "0px", top: "0px", width: "280px", visibility: "hidden" };
            },
            centerLinks(artifact) {
                const id = encodeURIComponent(artifact.id);
                const links = [];
                if (artifact.url) links.push({ label: "Open in Text", href: artifact.url });
                if (artifact.kind === "service") links.push({ label: "Try", href: "/qapps/dev/Services/Edit?artifact=" + id });
                if (artifact.kind === "entity" || artifact.kind === "view-entity") links.push({ label: "Data", href: "/qapps/dev/Entities/Edit?artifact=" + id });
                if (artifact.kind === "screen" && artifact.rootId) links.push({ label: "Preview", href: "/qapps/dev/Screens/Design?rootId=" + encodeURIComponent(artifact.rootId) +
                    "&path=" + encodeURIComponent(artifact.relativePath || "") });
                links.push({ label: "List view", href: "/qapps/dev/Artifacts/Refs?artifactId=" + id });
                return links;
            },
            copyName(artifact) { if (root.navigator && root.navigator.clipboard) root.navigator.clipboard.writeText(artifact.name); }
        },
        template: `
<div class="m-dev-artifact-browser column no-wrap" id="DevArtifactBrowser">
  <div class="row items-center no-wrap q-gutter-x-sm q-px-sm q-py-xs">
    <q-btn dense flat round icon="arrow_back" :disable="at <= 0" @click="back"><q-tooltip>Back</q-tooltip></q-btn>
    <q-btn dense flat round icon="arrow_forward" :disable="at >= trail.length - 1" @click="forward"><q-tooltip>Forward</q-tooltip></q-btn>
    <span class="text-subtitle2 ellipsis" v-if="center">{{center.artifact.kind}} {{center.artifact.shortName}}</span>
    <q-space></q-space>
    <q-select dense outlined multiple emit-value map-options options-dense v-model="filter.kinds" :options="allKinds" label="Kinds" style="min-width: 9rem"></q-select>
    <q-checkbox dense v-model="filter.hideFramework" label="Hide framework"></q-checkbox>
    <q-checkbox dense v-model="filter.staticLinks" label="Static"></q-checkbox>
    <q-checkbox dense v-model="filter.observed" label="Observed"></q-checkbox>
    <q-btn-toggle dense flat no-caps v-model="vertical" :options="[{ label: 'Across', value: false }, { label: 'Down', value: true }]" id="DevBrowserOrientation"></q-btn-toggle>
    <q-btn dense flat no-caps label="Fit" @click="fit"></q-btn>
    <q-btn dense flat no-caps label="Center" @click="centerView"></q-btn>
  </div>
  <div v-if="error" class="q-pa-md text-negative">{{error}}</div>
  <m-pan-zoom v-else ref="layer" class="col" height="100%">
    <template v-slot:under>
      <svg width="1" height="1"><path v-for="line in connectors" :key="line.id" :d="line.d" fill="none" stroke="currentColor" stroke-opacity=".45"
          stroke-width="1.5" :stroke-dasharray="line.dashed ? '5 4' : null" :data-connector="line.id"></path></svg>
    </template>
    <q-card v-for="card in cards" :key="card.id" :ref="'card-' + card.id" flat bordered class="m-dev-browser-card absolute" :style="style(card)"
        :class="{ 'm-dev-browser-center': card.generation === 0 }" :data-card="card.id" :data-generation="card.generation">
      <template v-if="card.generation === 0">
        <q-card-section class="q-pa-sm">
          <div class="text-caption m-muted"><q-icon :name="icon(card.artifact.kind)" size="xs"></q-icon> {{card.artifact.kind}}</div>
          <div class="text-subtitle2" style="word-break: break-all">{{card.artifact.shortName}}</div>
          <div class="text-caption" style="word-break: break-all">{{card.artifact.name}}</div>
          <div class="text-caption m-muted">{{card.artifact.component}}{{card.artifact.upstream === false ? ' · site' : ''}} · {{(card.artifact.relativePath || card.artifact.path || '').split('/').pop()}}:{{card.artifact.line}}</div>
          <div class="text-caption q-mt-xs" v-if="card.artifact.summary">{{card.artifact.summary}}</div>
        </q-card-section>
        <q-card-actions class="q-pa-xs"><q-btn v-for="link in centerLinks(card.artifact)" :key="link.label" dense flat no-caps size="sm" :label="link.label" :href="link.href" type="a"></q-btn>
          <q-btn dense flat no-caps size="sm" label="Copy name" @click="copyName(card.artifact)"></q-btn></q-card-actions>
      </template>
      <template v-else>
        <div class="text-caption text-weight-medium q-px-sm q-pt-xs ellipsis">{{card.direction === 'in' ? 'used by' : 'uses'}}{{card.rowId ? ' · ' + card.title : ''}}</div>
        <q-linear-progress v-if="card.loading" indeterminate></q-linear-progress>
        <div v-else-if="card.error" class="text-caption text-negative q-pa-sm">{{card.error}}</div>
        <div v-else-if="!sections(card).length" class="text-caption m-muted q-pa-sm">Nothing</div>
        <div v-for="section in sections(card)" :key="section.kind" class="q-pb-xs">
          <div class="text-caption m-muted q-px-sm row no-wrap"><span class="text-capitalize">{{section.kind}}</span><q-space></q-space>{{section.rows.length}}</div>
          <div v-for="(row, index) in shown(card, section)" :key="row.id" class="row items-center no-wrap q-px-xs m-dev-browser-row" :data-row="row.id"
              :class="{ 'm-dev-browser-hover': hover === row.id }" @mouseenter="hover = row.id" @mouseleave="hover = null">
            <q-btn v-if="card.direction === 'in'" dense flat round size="xs" :icon="childOf(card, row) ? 'chevron_right' : 'chevron_left'"
                :disable="!row.resolved || cycle(card, row)" @click="toggle(card, row, index)" :data-expand="row.id"><q-tooltip>{{cycle(card, row) ? 'A cycle: on the path already' : 'Expand'}}</q-tooltip></q-btn>
            <q-icon :name="icon(row.kind)" size="xs" class="q-mx-xs"></q-icon>
            <a class="ellipsis col cursor-pointer" :title="row.name + ' · ' + row.how" @click="row.resolved && go(row.id)">{{row.shortName}}</a>
            <span v-if="row.calls" class="text-caption m-muted q-ml-xs">{{row.calls}}</span>
            <q-icon v-if="row.observedOnly" name="sym_o_visibility" size="xs" class="q-ml-xs"><q-tooltip>Only observed</q-tooltip></q-icon>
            <q-btn v-if="card.direction === 'out'" dense flat round size="xs" :icon="childOf(card, row) ? 'chevron_left' : 'chevron_right'"
                :disable="!row.resolved || cycle(card, row)" @click="toggle(card, row, index)" :data-expand="row.id"><q-tooltip>{{cycle(card, row) ? 'A cycle: on the path already' : 'Expand'}}</q-tooltip></q-btn>
          </div>
          <div v-if="section.rows.length > shown(card, section).length" class="text-caption q-px-sm cursor-pointer text-primary" @click="more(card, section)">+ {{section.rows.length - shown(card, section).length}} more</div>
        </div>
      </template>
    </q-card>
  </m-pan-zoom>
</div>`
    };

    devAide.browserComponents = { "m-dev-artifact-browser": browser };
    const qvt = moqui.qvt;
    if (qvt && qvt.registerLate) Object.keys(devAide.browserComponents).forEach((name) => qvt.registerLate(name, devAide.browserComponents[name]));
    if (root.document && !root.document.getElementById("m-dev-browser-style")) {
        const style = root.document.createElement("style");
        style.id = "m-dev-browser-style";
        style.textContent = ".m-dev-artifact-browser{height:calc(100vh - 140px);min-height:24rem}.m-dev-browser-card{font-size:12px}" +
            ".m-dev-browser-center{border-width:2px;border-color:var(--q-primary)}.m-dev-browser-row{min-height:22px}" +
            ".m-dev-browser-hover{background:rgba(255,213,79,.25)}";
        root.document.head.appendChild(style);
    }
    devAide.browserRegistered = true;
})(typeof window !== "undefined" ? window : globalThis);
