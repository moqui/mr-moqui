/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The block tree of the Dev AIDE (plans/design-dev-aide.md 9.2, 11.3, D10): m-dev-block-tree draws the elements of an
 * XML outline as blocks, containers holding their children, and turns a gesture into an operation of the lossless XML
 * layer that its view applies to the buffer: a drag within or across containers (the vendored SortableJS, nested
 * groups) a move, Delete a remove, Alt+Up and Alt+Down a move past the sibling, + between blocks an insert. The tree holds
 * no state the text does not: after an operation its view reads the outline again and the tree draws it. The screen
 * builder's outline and the logic composer are its two uses.
 *
 * The pure parts are moqui.devAide.blocks, which test/client/pages/DevAideBlocks.page.js runs.
 */
(function (root) {
    "use strict";

    const moqui = root.moqui;
    const devAide = moqui.devAide;
    const blocks = devAide.blocks = devAide.blocks || {};

    // ---- pure ----

    const samePath = (a, b) => !!a && !!b && a.length === b.length && a.every((value, index) => value === b[index]);
    blocks.samePath = samePath;
    // Whether path `inner` is `outer` or inside it.
    blocks.within = (inner, outer) => !!inner && !!outer && inner.length >= outer.length && outer.every((value, index) => inner[index] === value);

    // The move operation of a drag: the block at `from` (an outline path) dropped into the children shown of `parent`
    // (a path) at `index` among them, the children listed as `shown` (their paths, in order, with the dragged one still
    // in its place when the drag stays in its container). Before the shown child that ends up after it, else last in
    // the parent; null for a drop into itself or one that changes nothing.
    blocks.dropOp = (from, parent, index, shown) => {
        if (blocks.within(parent, from)) return null;
        const others = (shown || []).filter((path) => !samePath(path, from));
        const sameParent = samePath(from.slice(0, -1), parent);
        if (sameParent) {
            const was = (shown || []).findIndex((path) => samePath(path, from));
            if (was === index) return null;
        }
        const next = others[index];
        if (next) return { op: "move", at: from, to: next, position: "before" };
        const lastOther = others[others.length - 1];
        if (lastOther) return { op: "move", at: from, to: lastOther, position: "after" };
        return { op: "move", at: from, to: parent, position: "last" };
    };
    // Alt+Up and Alt+Down: the block before its previous shown sibling, or after its next one.
    blocks.stepOp = (path, siblings, delta) => {
        const index = siblings.findIndex((sibling) => samePath(sibling, path));
        const other = siblings[index + delta];
        if (index < 0 || !other) return null;
        return { op: "move", at: path, to: other, position: delta < 0 ? "before" : "after" };
    };
    // The blocks in the order drawn, depth first, for the keyboard's Up and Down: each with its path and depth.
    blocks.flatten = (nodes, childrenOf, out, depth) => {
        out = out || [];
        for (const node of nodes || []) {
            out.push({ path: node.path, depth: depth || 0, node });
            blocks.flatten(childrenOf(node), childrenOf, out, (depth || 0) + 1);
        }
        return out;
    };

    // ---- the component ----

    let sortableLoading = null;
    blocks.loadSortable = () => {
        if (root.Sortable) return Promise.resolve(root.Sortable);
        if (!sortableLoading) sortableLoading = moqui.loadScript("/libs/sortablejs/Sortable.min.js", null, () => !!root.Sortable).then(() => root.Sortable);
        return sortableLoading;
    };

    const blockList = {
        name: "mDevBlockList",
        props: { nodes: Array, parentPath: Array, tree: Object, depth: Number },
        mounted() { this.attach(); },
        updated() { this.attach(); },
        beforeUnmount() { if (this.sortable) this.sortable.destroy(); },
        methods: {
            attach() {
                if (this.sortable || this.tree.readOnly || !this.$el || !this.$el.querySelectorAll) return;
                blocks.loadSortable().then((Sortable) => {
                    if (this.sortable || !this.$el) return;
                    this.sortable = Sortable.create(this.$el, {
                        group: { name: this.tree.group, pull: true, put: true },
                        handle: ".m-dev-block-handle", draggable: ".m-dev-block", animation: 120, fallbackOnBody: true,
                        swapThreshold: 0.6, emptyInsertThreshold: 12,
                        onEnd: (event) => this.dropped(event)
                    });
                }, () => {});
            },
            dropped(event) {
                const item = event.item;
                const from = JSON.parse(item.getAttribute("data-path"));
                const toParent = JSON.parse(event.to.getAttribute("data-parent"));
                const shown = JSON.parse(event.to.getAttribute("data-shown") || "[]");
                const sameList = event.from === event.to;
                // the tree is drawn from the outline: put the element back, then apply the move to the text
                if (sameList) {
                    const children = event.from.children;
                    event.from.insertBefore(item, children[event.oldIndex + (event.oldIndex > event.newIndex ? 1 : 0)] || null);
                } else {
                    event.from.insertBefore(item, event.from.children[event.oldIndex] || null);
                }
                const op = blocks.dropOp(from, toParent, event.newIndex, shown);
                if (op) this.tree.$emit("op", [op]);
            }
        },
        render() {
            const h = root.Vue.h;
            const shown = (this.nodes || []).map((node) => node.path);
            return h("div", { class: "m-dev-block-list", "data-parent": JSON.stringify(this.parentPath || []), "data-shown": JSON.stringify(shown) },
                (this.nodes || []).map((node) => h(blockItem, { key: node.path.join("."), node, tree: this.tree, depth: this.depth || 0 })));
        }
    };

    const blockItem = {
        name: "mDevBlock",
        props: { node: Object, tree: Object, depth: Number },
        computed: {
            label() { return this.tree.label(this.node) || { text: this.node.name }; },
            kids() { return this.tree.childrenOf(this.node); },
            container() { return this.tree.isContainer(this.node); },
            selected() { return samePath(this.tree.selected, this.node.path); }
        },
        render() {
            const h = root.Vue.h;
            const label = this.label;
            const head = h("div", {
                class: "m-dev-block-head row no-wrap items-center" + (this.selected ? " selected" : ""), tabindex: this.selected ? 0 : -1,
                "data-block": this.node.path.join("."), onClick: (event) => { event.stopPropagation(); this.tree.select(this.node); },
                onDblclick: (event) => { event.stopPropagation(); this.tree.$emit("open", this.node); }
            }, [
                this.tree.readOnly ? null : h("span", { class: "m-dev-block-handle", title: "Drag to move" }, "⋮⋮"),
                h("span", { class: "m-dev-block-name" }, label.text),
                (label.chips || []).map((chip, index) => chip.word ? h("span", { class: "m-dev-block-word", key: index }, chip.text) : h("span", { class: "m-dev-block-chip" + (chip.warn ? " warn" : ""), key: index,
                    title: chip.title || "", onClick: chip.edit ? (event) => { event.stopPropagation(); this.tree.$emit("chip", { node: this.node, chip }); } : undefined }, chip.text)),
                label.badge ? h("span", { class: "m-dev-block-badge " + (label.badgeClass || "") }, label.badge) : null
            ]);
            const body = this.container ? h(blockList, { nodes: this.kids, parentPath: this.node.path, tree: this.tree, depth: this.depth + 1 }) : null;
            return h("div", { class: "m-dev-block" + (this.container ? " container" : ""), "data-path": JSON.stringify(this.node.path) }, [head, body]);
        }
    };

    const blockTree = {
        name: "mDevBlockTree",
        props: {
            nodes: Array, parentPath: { type: Array, default: () => [] }, selected: Array, readOnly: Boolean,
            // (node) => { text, chips: [{ text, title, edit, warn }], badge, badgeClass }
            labelFn: Function,
            // (node) => the children drawn as its blocks; the default all of them
            childrenFn: Function,
            // (node) => whether it holds blocks (a drop target)
            containerFn: Function,
            group: { type: String, default: () => "m-dev-blocks-" + Math.random().toString(36).slice(2) }
        },
        emits: ["select", "op", "open", "chip", "remove", "insert"],
        computed: {
            flat() { return blocks.flatten(this.nodes, (node) => this.childrenOf(node)); }
        },
        methods: {
            label(node) { return this.labelFn ? this.labelFn(node) : { text: node.name }; },
            childrenOf(node) { return this.childrenFn ? this.childrenFn(node) || [] : (node.children || []); },
            isContainer(node) { return this.containerFn ? this.containerFn(node) : (node.children || []).length > 0; },
            select(node) { this.$emit("select", node); },
            onKey(event) {
                const flat = this.flat;
                const at = flat.findIndex((entry) => samePath(entry.path, this.selected));
                const key = event.key;
                if (key === "ArrowDown" || key === "ArrowUp") {
                    event.preventDefault();
                    if (event.altKey && !this.readOnly && at >= 0) {
                        const path = flat[at].path;
                        const parent = path.slice(0, -1);
                        const siblings = flat.filter((entry) => samePath(entry.path.slice(0, -1), parent)).map((entry) => entry.path);
                        const op = blocks.stepOp(path, siblings, key === "ArrowUp" ? -1 : 1);
                        if (op) this.$emit("op", [op]);
                        return;
                    }
                    const next = flat[Math.max(0, Math.min(flat.length - 1, at + (key === "ArrowDown" ? 1 : -1)))];
                    if (next) this.$emit("select", next.node);
                } else if ((key === "Delete" || key === "Backspace") && !this.readOnly && at >= 0) {
                    event.preventDefault();
                    this.$emit("remove", flat[at].node);
                } else if (key === "/" && !this.readOnly) {
                    event.preventDefault();
                    this.$emit("insert", at >= 0 ? flat[at].node : null);
                }
            }
        },
        render() {
            const h = root.Vue.h;
            return h("div", { class: "m-dev-block-tree", tabindex: 0, onKeydown: (event) => this.onKey(event) },
                [h(blockList, { nodes: this.nodes, parentPath: this.parentPath, tree: this, depth: 0 })]);
        }
    };
    blocks.components = { "m-dev-block-tree": blockTree };
    const qvt = moqui.qvt;
    if (qvt && qvt.registerLate) qvt.registerLate("m-dev-block-tree", blockTree);
    devAide.blocksRegistered = true;
    if (root.document && !root.document.getElementById("m-dev-blocks-style")) {
        const style = root.document.createElement("style");
        style.id = "m-dev-blocks-style";
        style.textContent = ".m-dev-block-tree{outline:none;font-size:12px}.m-dev-block-list{min-height:4px}" +
            ".m-dev-block-list .m-dev-block-list{margin-left:14px;border-left:1px dashed var(--m-border,rgba(0,0,0,.18));padding-left:4px}" +
            ".m-dev-block-head{padding:1px 4px;border-radius:4px;cursor:pointer;gap:4px;min-height:22px}" +
            ".m-dev-block-head:hover{background:rgba(127,127,127,.1)}.m-dev-block-head.selected{background:rgba(25,118,210,.16);outline:1px solid rgba(25,118,210,.5)}" +
            ".m-dev-block-handle{cursor:grab;opacity:.45;font-size:10px;letter-spacing:-2px}.m-dev-block-name{font-weight:500}" +
            ".m-dev-block-chip{padding:0 5px;border-radius:8px;background:rgba(127,127,127,.14);font-family:ui-monospace,Menlo,monospace;font-size:11px;max-width:22rem;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}" +
            ".m-dev-block-chip.warn{background:rgba(229,115,0,.22)}.m-dev-block-word{opacity:.7}.m-dev-block-badge{margin-left:auto;font-size:10px;opacity:.8}" +
            ".sortable-ghost>.m-dev-block-head{opacity:.4}";
        root.document.head.appendChild(style);
    }
})(typeof window !== "undefined" ? window : globalThis);
