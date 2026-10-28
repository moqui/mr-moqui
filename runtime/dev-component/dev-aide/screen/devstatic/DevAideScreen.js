/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The screen view of the Dev AIDE (plans/design-dev-aide.md 11): m-dev-screen-view, a visual view of a screen or
 * screen-extend file's tab. The canvas is the real render of the screen in a same-origin frame (11.2): it renders the
 * tab's buffers, not the files (put#CanvasBuffer and the frame's buffer:<id> preview token, F14), each widget stamped
 * with the file and line that wrote it (moquiAide=source, F10), under an overlay that outlines and names the element
 * under the pointer and selects it on a click. An upstream screen opens in extension mode (D17): every edit is written
 * into the site component's screen-extend of it, a companion buffer made on its first save; with the upstream toggle
 * on, or on a site's own screen, the edits go to the screen file. What the selection offers is shaped by it: Add column
 * on a form-list and Add field on a form-single (F16's form merge with after=, or in place the field with its
 * field-ref in the layout or its column), Insert before, after or inside at a named anchor (widgets-extend), and for an
 * element with no name the nearest anchor to insert at instead (question 14: anchors are never generated). The
 * Extension points list names the base screen's anchors; the Outline is the editable file's widgets on the block tree;
 * Properties edits the selected element's attributes from the schema digest.
 *
 * The operation builders are moqui.devAide.screenOps, pure, which test/client/pages/DevAideScreen.page.js runs.
 */
(function (root) {
    "use strict";

    const moqui = root.moqui;
    const devAide = moqui.devAide;
    const pure = devAide.pure;
    const ops = devAide.screenOps = {};
    const attrOf = pure.attr;
    const children = (node, names) => ((node && node.children) || []).filter((child) => !names || names.indexOf(child.name) >= 0);
    const escape = (value) => String(value == null ? "" : value).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/"/g, "&quot;");
    const xml = (name, attrs, body) => {
        let out = "<" + name;
        for (const [key, value] of attrs || []) if (value !== undefined && value !== null && value !== "") out += " " + key + "=\"" + escape(value) + "\"";
        return body ? out + ">" + body + "</" + name + ">" : out + "/>";
    };
    ops.xml = xml;

    // ---- pure: the screen's anchors ----

    // The elements a screen-extend can name (F16, D17): widgets-extend finds an element by name or id.
    ops.ANCHORABLE = ["container", "container-box", "container-row", "row-col", "container-panel", "container-split", "container-dialog",
        "section", "section-iterate", "form-single", "form-list", "tree", "dynamic-container", "button-menu", "label", "link", "image"];
    // The containers an insert goes inside (widgets-extend where="prepend|append").
    ops.CONTAINERS = ["container", "container-box", "row-col", "section", "section-iterate", "container-panel", "dynamic-container"];
    ops.anchorName = (node) => attrOf(node, "name") || attrOf(node, "id") || "";
    ops.widgetsOf = (outline) => children(outline, ["widgets"])[0] || null;

    // The nearest element an insert at `path` can anchor to, for an element with no name: a named sibling before it
    // (insert after that), else one after it (insert before that), else the nearest named ancestor (append inside it).
    ops.nearestAnchor = (outline, path) => {
        for (let depth = path.length; depth > 0; depth--) {
            const parentPath = path.slice(0, depth - 1);
            const parent = pure.outlineAt(outline, parentPath);
            const index = path[depth - 1];
            const siblings = (parent && parent.children) || [];
            for (let at = index - 1; at >= 0; at--) {
                if (ops.ANCHORABLE.indexOf(siblings[at].name) >= 0 && ops.anchorName(siblings[at])) return { name: ops.anchorName(siblings[at]), where: "after", path: siblings[at].path };
            }
            for (let at = index + 1; at < siblings.length; at++) {
                if (ops.ANCHORABLE.indexOf(siblings[at].name) >= 0 && ops.anchorName(siblings[at])) return { name: ops.anchorName(siblings[at]), where: "before", path: siblings[at].path };
            }
            if (parent && ops.CONTAINERS.indexOf(parent.name) >= 0 && ops.anchorName(parent)) return { name: ops.anchorName(parent), where: "append", path: parent.path };
        }
        return null;
    };
    // The base screen's extension points (11.2): its named containers, sections and forms, its transitions, and each
    // container with no name with the nearest anchor to use instead.
    ops.extensionPoints = (outline) => {
        const points = [];
        const widgets = ops.widgetsOf(outline);
        if (widgets) {
            pure.walkOutline(widgets, (node) => {
                if (node === widgets || ops.ANCHORABLE.indexOf(node.name) < 0) return;
                const name = ops.anchorName(node);
                const container = ops.CONTAINERS.indexOf(node.name) >= 0 || node.name === "container-row";
                if (!name && !container && !/^form-|^section/.test(node.name)) return;
                points.push({ kind: node.name, name, path: node.path, line: node.line, named: !!name,
                    nearest: name ? null : ops.nearestAnchor(outline, node.path) });
            });
        }
        for (const transition of children(outline, ["transition"])) {
            points.push({ kind: "transition", name: attrOf(transition, "name") || "", path: transition.path, line: transition.line, named: true, nearest: null });
        }
        return points;
    };
    // The element written at a line (and column) of a file's outline.
    ops.nodeAtLine = (outline, line, column) => {
        let found = null;
        pure.walkOutline(outline, (node) => {
            if (node.line === line && (!column || node.column === column || !found)) found = node;
        });
        return found;
    };
    // The form an element is in (itself when it is one).
    ops.formOf = (outline, path) => {
        for (let depth = path.length; depth >= 0; depth--) {
            const node = pure.outlineAt(outline, path.slice(0, depth));
            if (node && (node.name === "form-single" || node.name === "form-list")) return node;
        }
        return null;
    };
    // The field an element is in (itself when it is one).
    ops.fieldOf = (outline, path) => {
        for (let depth = path.length; depth >= 0; depth--) {
            const node = pure.outlineAt(outline, path.slice(0, depth));
            if (node && node.name === "field") return node;
            if (node && (node.name === "form-single" || node.name === "form-list")) return null;
        }
        return null;
    };

    // ---- pure: the operations ----

    ops.EXTEND_TEXT = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<screen-extend xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" " +
        "xsi:noNamespaceSchemaLocation=\"http://moqui.org/xsd/xml-screen-3.xsd\">\n</screen-extend>\n";
    // A widget of a field: display, text-line, a drop-down of enumerations, a check or a date-time.
    ops.fieldWidget = (widget) => ({
        "text-line": "<text-line/>", "text-area": "<text-area/>", "date-time": "<date-time type=\"date\"/>", check: "<check><option key=\"Y\"/></check>",
        "drop-down": "<drop-down allow-empty=\"true\"/>", link: "<link url=\".\" text=\"${" + "}\"/>"
    })[widget] || "<display/>";
    // A field as a form merge writes it: its name, its place (after=, F16) and its default-field with a title.
    ops.fieldXml = (field) => xml("field", [["name", field.name], ["after", field.after]],
        xml("default-field", [["title", field.title]], field.widgetXml || ops.fieldWidget(field.widget)));
    // A field into a form of the site's screen-extend: the form merge of that name (made when missing) gets the field,
    // placed by after= in the form's layout or columns (F16).
    ops.extendFormField = (extOutline, formKind, formName, field) => {
        const form = children(extOutline, [formKind]).find((node) => attrOf(node, "name") === formName);
        const fieldXml = ops.fieldXml(field);
        if (form) return [{ op: "insert", at: form.path, position: "last", xml: fieldXml }];
        return [{ op: "insert", at: extOutline.path || [], position: "last", xml: xml(formKind, [["name", formName]], fieldXml) }];
    };
    // Widgets at an anchor of the base screen through the site's screen-extend: into its widgets-extend of that name and
    // place when it has one, else a new one.
    ops.extendWidgets = (extOutline, anchor, where, widgetXml) => {
        const existing = children(extOutline, ["widgets-extend"]).find((node) => attrOf(node, "name") === anchor && (attrOf(node, "where") || "") === where);
        if (existing) {
            return [{ op: "insert", at: existing.path, position: "last", xml: widgetXml }];
        }
        return [{ op: "insert", at: extOutline.path || [], position: "last", xml: xml("widgets-extend", [["name", anchor], ["where", where]], widgetXml) }];
    };
    // A field into a form of the screen file itself: the field after the field `afterName` (else after the last), and
    // its field-ref after that field's in the field-layout, or a form-list-column of its own after that field's column.
    ops.insertFormField = (form, field, afterName) => {
        const fields = children(form, ["field"]);
        const anchor = (afterName && fields.find((node) => attrOf(node, "name") === afterName)) || fields[fields.length - 1];
        const fieldXml = ops.fieldXml({ name: field.name, title: field.title, widget: field.widget, widgetXml: field.widgetXml });
        const list = [anchor ? { op: "insert", at: anchor.path, position: "after", xml: fieldXml } : { op: "insert", at: form.path, position: "first", xml: fieldXml }];
        const layout = children(form, ["field-layout"])[0];
        const findRef = (node, name) => {
            let found = null;
            pure.walkOutline(node, (child) => { if (!found && child.name === "field-ref" && attrOf(child, "name") === name) found = child; });
            return found;
        };
        const refXml = xml("field-ref", [["name", field.name]]);
        if (layout) {
            const ref = afterName ? findRef(layout, afterName) : null;
            list.push(ref ? { op: "insert", at: ref.path, position: "after", xml: refXml } : { op: "insert", at: layout.path, position: "last", xml: refXml });
        }
        const columns = children(form, ["form-list-column"]);
        if (columns.length) {
            const column = afterName ? columns.find((node) => findRef(node, afterName)) : null;
            const columnXml = xml("form-list-column", [], refXml);
            list.push({ op: "insert", at: (column || columns[columns.length - 1]).path, position: "after", xml: columnXml });
        }
        return list;
    };
    // An element of the base screen copied into the site's screen-extend, which replaces it there: a section (a section
    // of the same name replaces the base's) or a transition (merged by name). `text` is the base file's text, `node` its
    // outline element; the copy's own indentation is taken off its first line.
    ops.copyInto = (extOutline, text, node) => {
        const copied = String(text || "").slice(node.from16, node.to16);
        return copied ? [{ op: "insert", at: extOutline.path || [], position: "last", xml: copied }] : [];
    };
    // Actions run before or after the screen's own (actions-extend when="before|after").
    ops.actionsExtend = (extOutline, when) => [{ op: "insert", at: extOutline.path || [], position: "last",
        xml: xml("actions-extend", [["type", "actions"], ["when", when]], "<log level=\"info\" message=\"" + when + " the screen's actions\"/>") }];
    // The find of a form-list's list in the screen's actions, and whether it takes the list's search inputs (Add filter).
    ops.listFindOf = (outline, listName) => {
        let found = null;
        if (!listName) return null;
        pure.walkOutline(outline, (node) => {
            if (!found && (node.name === "entity-find" || node.name === "entity-find-count") && attrOf(node, "list") === listName) found = node;
        });
        return found;
    };
    ops.hasSearchInputs = (find) => !!(find && children(find, ["search-form-inputs"]).length);
    // Add filter: the column's field with a find widget in its header; the form merge of the site, or the field itself.
    ops.filterXml = (fieldName) => xml("field", [["name", fieldName]], "<header-field show-order-by=\"true\"><text-find/></header-field>");
    ops.filterOps = (inPlace, outline, form, fieldName) => {
        if (!inPlace) {
            const merge = children(outline, ["form-list"]).find((node) => attrOf(node, "name") === attrOf(form, "name"));
            return merge ? [{ op: "insert", at: merge.path, position: "last", xml: ops.filterXml(fieldName) }]
                : [{ op: "insert", at: outline.path || [], position: "last", xml: xml("form-list", [["name", attrOf(form, "name")]], ops.filterXml(fieldName)) }];
        }
        const field = children(form, ["field"]).find((node) => attrOf(node, "name") === fieldName);
        if (!field) return [{ op: "insert", at: form.path, position: "last", xml: xml("field", [["name", fieldName]],
            "<header-field show-order-by=\"true\"><text-find/></header-field><default-field><display/></default-field>") }];
        if (children(field, ["header-field"]).length) return [];
        return [{ op: "insert", at: field.path, position: "first", xml: "<header-field show-order-by=\"true\"><text-find/></header-field>" }];
    };
    // Add section (11.2): a box, or a list of a related entity's records keyed by a field of the page.
    ops.sectionXml = (spec) => {
        const name = spec.name || "NewSection";
        if (spec.kind === "related") {
            const list = (spec.entity.split(".").pop() || "related").replace(/^./, (ch) => ch.toLowerCase()) + "List";
            const find = xml("entity-find", [["entity-name", spec.entity], ["list", list]],
                xml("econdition", [["field-name", spec.keyField], ["from", spec.keyFrom || spec.keyField]]));
            const formList = xml("form-list", [["name", name + "List"], ["list", list], ["skip-form", "true"]],
                xml("auto-fields-entity", [["entity-name", spec.entity], ["field-type", "display"]]));
            return xml("section", [["name", name]], "<actions>" + find + "</actions><widgets>" +
                xml("container-box", [["name", name + "Box"]], xml("box-header", [["title", spec.title || name]]) + "<box-body>" + formList + "</box-body>") + "</widgets>");
        }
        return xml("container-box", [["name", name]], xml("box-header", [["title", spec.title || name]]) + "<box-body></box-body>");
    };
    // Add button (11.2): a transition that calls a service, and a link to it.
    ops.transitionXml = (spec) => xml("transition", [["name", spec.transition]], xml("service-call", [["name", spec.service], ["in-map", "true"]]) +
        "<default-response url=\".\"/>");
    ops.linkXml = (spec) => xml("link", [["url", spec.transition], ["text", spec.text], ["btn-type", "primary"]]);
    // the transition's place in a screen file: before its actions and widgets
    ops.transitionOp = (outline, transitionXml) => {
        const next = children(outline, ["subscreens", "pre-actions", "always-actions", "actions", "widgets"])[0];
        return next ? { op: "insert", at: next.path, position: "before", xml: transitionXml } : { op: "insert", at: outline.path || [], position: "last", xml: transitionXml };
    };
    // Add button as one list of operations: the transition, then its link at the place (in place the link's path moves
    // by one when the transition goes in before the element it is under).
    ops.buttonOps = (extension, outline, spec, insertAt, node) => {
        const transition = ops.transitionXml(spec);
        if (extension) return [{ op: "insert", at: outline.path || [], position: "last", xml: transition }].concat(insertAt(ops.linkXml(spec)));
        const first = ops.transitionOp(outline, transition);
        const path = node.path.slice();
        if (first.position === "before" && first.at.length === 1 && path[0] >= first.at[0]) path[0]++;
        return [first].concat(insertAt(ops.linkXml(spec)).map((op) => Object.assign({}, op, { at: path })));
    };
    // Add row action: a list column whose link calls a transition with the row's key.
    ops.rowActionXml = (spec) => xml("field", [["name", spec.name || "rowAction"]], xml("default-field", [["title", ""]],
        xml("link", [["url", spec.transition], ["text", spec.text], ["link-type", "anchor"], ["parameter-map", spec.keyField ? "{" + spec.keyField + ": " + spec.keyField + "}" : ""]])));
    // The text a double click edits: a label's text, a link's text, a box header's title.
    ops.textAttribute = (node) => node && ({ label: "text", link: "text", "box-header": "title", "default-field": "title", "container-box": null })[node.name] || null;
    // The widgets Insert offers (11.2), each as it is written.
    ops.SNIPPETS = [
        { key: "label", label: "Label", xml: "<label text=\"New label\" type=\"p\"/>" },
        { key: "link", label: "Link", xml: "<link url=\".\" text=\"New link\" btn-type=\"default\"/>" },
        { key: "box", label: "Box with header and body", xml: "<container-box name=\"NewBox\"><box-header title=\"New box\"/><box-body></box-body></container-box>" },
        { key: "container", label: "Container", xml: "<container name=\"NewContainer\"></container>" },
        { key: "row", label: "Row of two columns", xml: "<container-row name=\"NewRow\"><row-col md=\"6\"></row-col><row-col md=\"6\"></row-col></container-row>" },
        { key: "section", label: "Section with a condition", xml: "<section name=\"NewSection\" condition=\"true\"><widgets></widgets></section>" },
        { key: "dialog", label: "Dynamic dialog", xml: "<dynamic-dialog id=\"NewDialog\" button-text=\"Open\" transition=\".\"/>" },
        { key: "menu", label: "Button menu", xml: "<button-menu text=\"More\"></button-menu>" },
        { key: "status", label: "Status bar", xml: "<status-bar/>" }
    ];
    // An element's label for the Outline: its name with its naming attributes.
    ops.labelOf = (node) => {
        const chips = [];
        for (const key of ["name", "id", "text", "title", "location", "url", "list", "where"]) {
            const value = attrOf(node, key);
            if (value) chips.push({ text: key === "name" || key === "id" ? value : key + "=" + value, title: key });
            if (chips.length >= 2) break;
        }
        return { text: node.name, chips };
    };

    // ---- the view ----

    const WIDTHS = [{ label: "390", value: 390 }, { label: "768", value: 768 }, { label: "1280", value: 1280 }, { label: "Full", value: 0 }];
    const newId = () => "c" + Math.random().toString(36).slice(2, 10) + Date.now().toString(36);

    const screenView = {
        name: "mDevScreenView",
        props: { ctx: { type: Object, required: true }, tab: { type: Object, required: true }, outline: Object },
        data() {
            return {
                view: null, error: "", mode: "design", width: 0, dark: false, query: "", bufferId: newId(), frameSrc: "", version: 0,
                base: null, ext: null, selection: null, hover: null, panel: "points", stamps: {}, adding: null, inserting: null,
                schema: null, rendering: false, frameError: "", selectedBox: null, intent: null, actionsKey: "", composerReady: !!devAide.serviceViewRegistered
            };
        },
        computed: {
            tabIsBase() { return !!(this.view && this.view.base && this.view.base.rootId === this.ctx.rootId && this.view.base.path === this.tab.path); },
            baseOutline() { return this.tabIsBase ? this.tab.outline : (this.base ? this.base.outline : null); },
            // the buffer edits go to: the screen file in place, else the site's screen-extend
            edit() { return this.view && this.view.inPlace ? (this.tabIsBase ? this.tab : this.base) : (this.isExtendTab ? this.tab : this.ext); },
            editOutline() { const buffer = this.edit; return buffer ? (buffer === this.tab ? this.tab.outline : buffer.outline) : null; },
            isExtendTab() { return !!(this.tab.outline && this.tab.outline.name === "screen-extend"); },
            extensionMode() { return !!(this.view && !this.view.inPlace); },
            canEdit() { return this.ctx.mayWrite && !!this.edit && !(this.edit !== this.tab && this.edit.readOnly) && !(this.edit === this.tab && this.tab.readOnly); },
            points() { return this.baseOutline ? ops.extensionPoints(this.baseOutline) : []; },
            outlineNodes() {
                const outline = this.editOutline;
                if (!outline) return [];
                if (this.extensionMode) return outline.children || [];
                const widgets = ops.widgetsOf(outline);
                return widgets ? widgets.children || [] : [];
            },
            // the actions the composer may edit (9.2): the screen's actions, always-actions and pre-actions and each
            // transition's, or in extension mode each actions-extend and each transition of the screen-extend
            actionHolders() {
                const outline = this.editOutline;
                if (!outline) return [];
                const out = [];
                for (const node of outline.children || []) {
                    if (["actions", "always-actions", "pre-actions"].indexOf(node.name) >= 0) out.push({ key: node.path.join("."), label: node.name, node });
                    if (node.name === "actions-extend") out.push({ key: node.path.join("."), label: "actions " + (attrOf(node, "when") || "after"), node });
                    if (node.name === "transition") {
                        const actions = children(node, ["actions"])[0];
                        if (actions) out.push({ key: actions.path.join("."), label: "transition " + attrOf(node, "name"), node: actions });
                    }
                }
                return out;
            },
            selectedActions() {
                const found = this.actionHolders.find((holder) => holder.key === this.actionsKey) || this.actionHolders[0];
                return found ? found.node : null;
            },
            screenParameters() { return children(this.baseOutline, ["parameter"]).map((node) => attrOf(node, "name")).filter((name) => name); },
            frameStyle() { return this.width ? { width: this.width + "px" } : { width: "100%" }; },
            selectedPath() { return this.selection && this.selection.which === "edit" && this.selection.node ? this.selection.node.path : null; },
            // what the selection offers (11.2): the form intents, the inserts at an anchor or at the nearest one
            intents() {
                const selection = this.selection;
                if (!selection || !selection.node) return null;
                const outline = selection.which === "base" ? this.baseOutline : this.editOutline;
                const node = selection.node;
                const form = ops.formOf(outline, node.path);
                const field = ops.fieldOf(outline, node.path);
                const anchorable = ops.ANCHORABLE.indexOf(node.name) >= 0;
                const name = ops.anchorName(node);
                return {
                    form: form ? { kind: form.name, name: attrOf(form, "name"), node: form } : null,
                    field: field ? attrOf(field, "name") : "",
                    anchor: anchorable && name ? { name, container: ops.CONTAINERS.indexOf(node.name) >= 0 } : null,
                    nearest: anchorable && !name ? ops.nearestAnchor(outline, node.path) : null,
                    unnamed: anchorable && !name
                };
            },
            elementSpec() {
                const node = this.selection && this.selection.which === "edit" ? this.selection.node : null;
                const digest = this.schema;
                return node && digest ? (digest.elements || []).find((spec) => spec.name === node.name) || null : null;
            }
        },
        watch: {
            width() { this.$nextTick(() => this.drawHover()); },
            dark() { this.applyDark(); }
        },
        mounted() {
            this.load();
            this.ctx.onSaved(() => this.render());
            this.onScroll = () => this.drawHover();
            root.addEventListener("resize", this.onScroll);
        },
        beforeUnmount() { clearTimeout(this.renderTimer); root.removeEventListener("resize", this.onScroll); },
        methods: {
            load() {
                const params = this.tab.page ? { page: this.tab.page } : { rootId: this.ctx.rootId, path: this.tab.path };
                this.ctx.get("screenView", params).then((answer) => {
                    this.view = answer.view;
                    this.query = this.view.query || "";
                    const tasks = [];
                    if (!this.tabIsBase && this.view.base) tasks.push(this.ctx.companion(this.view.base.rootId, this.view.base.path, "").then((buffer) => { this.base = buffer; }));
                    if (this.extensionMode && !this.isExtendTab && this.view.extension) {
                        tasks.push(this.ctx.companion(this.view.extension.rootId, this.view.extension.path, ops.EXTEND_TEXT).then((buffer) => { this.ext = buffer; }));
                    }
                    return Promise.all(tasks);
                }).then(() => {
                    const root = this.editOutline ? this.editOutline.name : "screen";
                    this.ctx.get("schema", { rootElement: root }).then((answer) => { this.schema = answer.digest || null; }, () => {});
                    this.render();
                }, (error) => { this.error = pure.errorText(error); });
            },
            buffers() { return [this.tab].concat(this.tab.companions || []); },
            // the frame renders the buffers: the dirty ones go to the canvas buffer first
            render() {
                if (!this.view || !this.view.urlPath) { this.frameError = this.view ? "This screen is not mounted under the apps, so it has no page to show." : ""; return; }
                this.rendering = true;
                const files = this.buffers().filter((buffer) => buffer.text !== buffer.saved || buffer.missing)
                    .map((buffer) => ({ rootId: buffer === this.tab ? this.ctx.rootId : buffer.rootId, path: buffer.path, text: buffer.text }));
                this.ctx.post("canvasBuffer", { bufferId: this.bufferId, files: JSON.stringify(files) }).then(() => {
                    const frame = this.$refs.frame;
                    this.scrollY = frame && frame.contentWindow ? frame.contentWindow.scrollY : 0;
                    const query = this.query ? this.query + "&" : "";
                    const aide = this.mode === "design" ? "&moquiAide=source" : "";
                    this.frameSrc = this.view.urlPath + "?" + query + "moquiPreview=buffer:" + this.bufferId + aide + "&v=" + (++this.version);
                }, (error) => { this.rendering = false; this.ctx.fail(error); });
            },
            scheduleRender() { clearTimeout(this.renderTimer); this.renderTimer = setTimeout(() => this.render(), 250); },
            setMode(mode) { this.mode = mode; this.hover = null; this.render(); },
            frameLoaded() {
                this.rendering = false;
                const frame = this.$refs.frame;
                const doc = frame && frame.contentDocument;
                if (!doc) return;
                const win = frame.contentWindow;
                if (this.scrollY) setTimeout(() => win.scrollTo(0, this.scrollY), 300);
                win.addEventListener("scroll", this.onScroll, { passive: true });
                win.addEventListener("resize", this.onScroll);
                this.applyDark();
                if (this.mode !== "design") return;
                const stamped = (target) => target && target.closest ? target.closest("[data-m-src]") : null;
                doc.addEventListener("mouseover", (event) => { this.hoverEl = stamped(event.target); this.drawHover(); }, true);
                doc.addEventListener("mouseleave", () => { this.hoverEl = null; this.drawHover(); }, true);
                const block = (event) => { const el = stamped(event.target); event.preventDefault(); event.stopPropagation(); if (el && event.type === "click") this.selectElement(el); };
                doc.addEventListener("click", block, true);
                doc.addEventListener("dblclick", (event) => {
                    event.preventDefault(); event.stopPropagation();
                    const node = this.selection && this.selection.node;
                    if (node) this.editText(node);
                }, true);
                doc.addEventListener("submit", (event) => { event.preventDefault(); event.stopPropagation(); }, true);
                // reselect what was selected, by its source
                if (this.selection && this.selection.stamp) {
                    setTimeout(() => {
                        const el = doc.querySelector("[data-m-src=\"" + this.selection.stamp.replace(/"/g, "\\\"") + "\"]");
                        this.selectedEl = el;
                        this.drawHover();
                    }, 600);
                }
            },
            applyDark() {
                const frame = this.$refs.frame;
                const win = frame && frame.contentWindow;
                try { if (win && win.Quasar && win.Quasar.Dark) win.Quasar.Dark.set(this.dark); } catch (err) { /* not loaded yet */ }
            },
            boxOf(el) {
                const frame = this.$refs.frame;
                if (!el || !frame || !el.isConnected) return null;
                const rect = el.getBoundingClientRect();
                return { left: frame.offsetLeft + rect.left, top: frame.offsetTop + rect.top, width: rect.width, height: rect.height };
            },
            drawHover() {
                const el = this.hoverEl;
                this.hover = el ? Object.assign({ label: this.describe(el) }, this.boxOf(el)) : null;
                this.selectedBox = this.boxOf(this.selectedEl);
            },
            describe(el) {
                const stamp = el.getAttribute("data-m-src") || "";
                const known = this.stamps[stamp];
                if (known && known.loc && known.loc.dbform) return "field from DbForm " + known.loc.dbform + (known.loc.groups.length ? " · group " + known.loc.groups.join(", ") : "");
                const node = known && known.node;
                const name = node ? node.name + (ops.anchorName(node) ? " " + ops.anchorName(node) : "") : el.tagName.toLowerCase();
                return name + (known && known.loc ? " · " + (known.loc.site ? "site" : "upstream") : "");
            },
            resolve(stamp) {
                if (this.stamps[stamp] && this.stamps[stamp].loc) return Promise.resolve(this.stamps[stamp]);
                return this.ctx.post("stamps", { stamps: JSON.stringify([stamp]) }).then((answer) => {
                    const loc = (answer.stamps || {})[stamp] || null;
                    this.stamps[stamp] = { loc };
                    return this.stamps[stamp];
                });
            },
            // which buffer a stamp's file is: the base screen, the editable one, or another file (a template)
            whichOf(loc) {
                if (!loc || !loc.rootId) return "other";
                const same = (buffer, rootId) => buffer && buffer.path === loc.path && rootId === loc.rootId;
                if (this.edit && same(this.edit, this.edit === this.tab ? this.ctx.rootId : this.edit.rootId)) return "edit";
                if (this.view.base && this.view.base.rootId === loc.rootId && this.view.base.path === loc.path) return "base";
                return "other";
            },
            selectElement(el) {
                const stamp = el.getAttribute("data-m-src");
                this.selectedEl = el;
                this.drawHover();
                this.resolve(stamp).then((known) => {
                    const which = this.whichOf(known.loc);
                    const outline = which === "edit" ? this.editOutline : which === "base" ? this.baseOutline : null;
                    const node = outline && known.loc ? ops.nodeAtLine(outline, known.loc.line, known.loc.column) : null;
                    known.node = node;
                    this.selection = { stamp, loc: known.loc, which: which === "base" && this.edit && !this.extensionMode ? "edit" : which, node };
                    this.drawHover();
                }, (error) => this.ctx.fail(error));
            },
            selectNode(node) {
                this.selection = { stamp: "", loc: null, which: "edit", node };
                const frame = this.$refs.frame;
                const doc = frame && frame.contentDocument;
                const loc = node && doc ? [...doc.querySelectorAll("[data-m-src]")].find((el) => {
                    const parts = (el.getAttribute("data-m-src") || "").split(":");
                    return Number(parts[parts.length - 2]) === node.line;
                }) : null;
                this.selectedEl = loc || null;
                this.drawHover();
                if (loc) loc.scrollIntoView({ block: "nearest" });
            },
            selectPoint(point) {
                const frame = this.$refs.frame;
                const doc = frame && frame.contentDocument;
                const el = doc ? [...doc.querySelectorAll("[data-m-src]")].find((candidate) => {
                    const parts = (candidate.getAttribute("data-m-src") || "").split(":");
                    return Number(parts[parts.length - 2]) === point.line;
                }) : null;
                if (el && point.kind !== "transition") { this.selectElement(el); el.scrollIntoView({ block: "nearest" }); }
                else this.selection = { stamp: "", loc: null, which: this.extensionMode ? "base" : "edit", node: pure.outlineAt(this.baseOutline, point.path) };
            },
            apply(list) {
                if (!list || !list.length) return Promise.resolve();
                const buffer = this.edit;
                return this.ctx.apply(list, buffer === this.tab ? null : buffer).then(() => this.scheduleRender(), (error) => this.ctx.fail(error));
            },
            // ---- intents ----
            startField() {
                const intents = this.intents;
                const form = intents.form;
                const fields = children(form.node, ["field"]).map((node) => attrOf(node, "name"));
                this.adding = { kind: form.kind, form: form.name, formNode: form.node, name: "", title: "", widget: form.kind === "form-list" ? "display" : "text-line",
                    after: intents.field || fields[fields.length - 1] || "", fields };
            },
            addField() {
                const add = this.adding;
                if (!add.name) return;
                this.adding = null;
                const field = { name: add.name, title: add.title, widget: add.widget, after: add.after };
                if (this.extensionMode) this.apply(ops.extendFormField(this.editOutline, add.kind, add.form, field));
                else {
                    // in place: the form in the screen file, read again from the editable outline by its name
                    let target = null;
                    pure.walkOutline(this.editOutline, (node) => { if (!target && node.name === add.kind && attrOf(node, "name") === add.form) target = node; });
                    if (!target) { this.ctx.note("The form " + add.form + " is not in this file", "warning"); return; }
                    this.apply(ops.insertFormField(target, { name: add.name, title: add.title, widget: add.widget }, add.after));
                }
            },
            startInsert(where, anchor) {
                this.inserting = { where, anchor: anchor || (this.intents && this.intents.anchor ? this.intents.anchor.name : ""), snippet: "label", text: "" };
            },
            insert() {
                const ins = this.inserting;
                const snippet = ops.SNIPPETS.find((item) => item.key === ins.snippet);
                const body = ins.text || (snippet ? snippet.xml : "");
                this.inserting = null;
                if (!body) return;
                if (this.extensionMode) { this.apply(ops.extendWidgets(this.editOutline, ins.anchor, ins.where, body)); return; }
                const node = this.selection && this.selection.node;
                if (!node) return;
                const position = { before: "before", after: "after", prepend: "first", append: "last" }[ins.where];
                this.apply([{ op: "insert", at: node.path, position, xml: body }]);
            },
            // Replace section, Override transition: the base element copied into the screen-extend, to change there
            copyBase() {
                const node = this.selection && this.selection.node;
                const text = this.tabIsBase ? this.tab.text : (this.base ? this.base.text : "");
                if (node) this.apply(ops.copyInto(this.editOutline, text, node));
            },
            // the logic composer is the service writer's: its script loads on first use
            loadComposer() {
                if (this.composerReady) return;
                moqui.loadScript("/devstatic/DevAideService.js", null, () => !!devAide.serviceViewRegistered)
                    .then(() => { this.composerReady = true; }, (error) => this.ctx.fail(error));
            },
            actionsExtendOps(when) { return ops.actionsExtend(this.editOutline, when); },
            // ---- the intents beyond a field: a filter, a section, a button, a row action ----
            startIntent(kind) {
                const intents = this.intents;
                const form = intents && intents.form;
                const base = { kind, name: "", title: "", text: "", transition: "", service: "", entity: "", keyField: "", fieldName: intents ? intents.field : "", where: "after", sectionKind: "box" };
                if (kind === "filter") {
                    const find = ops.listFindOf(this.baseOutline, form ? attrOf(form.node, "list") : "");
                    base.why = !find ? "The list's find is not in this screen's actions, so its filters cannot be told apart."
                        : !ops.hasSearchInputs(find) ? "The list's find has no search-form-inputs, so a header filter would not narrow it: add search-form-inputs to its find first." : "";
                }
                this.intent = base;
            },
            doIntent() {
                const intent = this.intent;
                this.intent = null;
                const intents = this.intents;
                const node = this.selection && this.selection.node;
                const anchor = intents && (intents.anchor ? intents.anchor.name : intents.nearest ? intents.nearest.name : "");
                const where = intents && intents.anchor ? intent.where : intents && intents.nearest ? intents.nearest.where : intent.where;
                const outline = this.editOutline;
                const insertAt = (body) => this.extensionMode ? ops.extendWidgets(outline, anchor, where, body)
                    : [{ op: "insert", at: node.path, position: { before: "before", after: "after", prepend: "first", append: "last" }[where] || "after", xml: body }];
                if (intent.kind === "filter") {
                    const form = intents.form;
                    const target = this.extensionMode ? form.node : this.findInEdit(form.kind, form.name);
                    this.apply(ops.filterOps(!this.extensionMode, outline, target || form.node, intent.fieldName));
                } else if (intent.kind === "section") {
                    this.apply(insertAt(ops.sectionXml(Object.assign({}, intent, { kind: intent.sectionKind }))));
                } else if (intent.kind === "button") {
                    this.apply(ops.buttonOps(this.extensionMode, outline, intent, insertAt, node));
                } else if (intent.kind === "rowAction") {
                    const form = intents.form;
                    const field = ops.rowActionXml(intent);
                    if (this.extensionMode) {
                        const merge = children(outline, ["form-list"]).find((item) => attrOf(item, "name") === form.name);
                        this.apply(merge ? [{ op: "insert", at: merge.path, position: "last", xml: field }]
                            : [{ op: "insert", at: outline.path || [], position: "last", xml: xml("form-list", [["name", form.name]], field) }]);
                    } else {
                        const target = this.findInEdit(form.kind, form.name);
                        if (target) this.apply([{ op: "insert", at: target.path, position: "last", xml: field }]);
                    }
                }
            },
            findInEdit(kind, name) {
                let found = null;
                pure.walkOutline(this.editOutline, (item) => { if (!found && item.name === kind && attrOf(item, "name") === name) found = item; });
                return found;
            },
            // a double click edits a label's or link's text, a box's title, in place (the screen's own file)
            editText(node) {
                const attribute = ops.textAttribute(node);
                if (!attribute || this.extensionMode || !this.canEdit) {
                    if (attribute && this.extensionMode) this.ctx.note("An upstream screen's text is changed in its own file: with \"Allow editing upstream components\" on", "warning");
                    return;
                }
                this.$q.dialog({ title: node.name + " " + attribute, prompt: { model: attrOf(node, attribute) || "", type: "text" }, cancel: true })
                    .onOk((value) => this.apply([{ op: "set-attribute", at: node.path, name: attribute, value }]));
            },
            // Add an anchor here (question 14): the element named in the base screen's own file, for a user who may edit
            // upstream; never generated
            addAnchor() {
                const node = this.selection && this.selection.node;
                const buffer = this.tabIsBase ? this.tab : this.base;
                if (!node || !buffer) return;
                this.$q.dialog({ title: "Name this " + node.name, message: "The name is written into " + this.view.base.path + ", the base screen.",
                    prompt: { model: "", type: "text" }, cancel: true }).onOk((value) => {
                    if (!value) return;
                    const attribute = node.name === "container-row" || node.name === "row-col" || node.name === "container" ? "id" : "name";
                    this.ctx.apply([{ op: "set-attribute", at: node.path, name: attribute, value }], buffer === this.tab ? null : buffer)
                        .then(() => this.scheduleRender(), (error) => this.ctx.fail(error));
                });
            },
            removeSelected() {
                const node = this.selection && this.selection.node;
                if (!node || this.selection.which !== "edit") return;
                this.selection = null;
                this.apply([{ op: "remove", at: node.path }]);
            },
            setAttribute(name, value) {
                const node = this.selection && this.selection.node;
                if (!node) return;
                this.apply(value === "" ? (attrOf(node, name) === undefined ? [] : [{ op: "remove-attribute", at: node.path, name }])
                    : [{ op: "set-attribute", at: node.path, name, value }]);
            },
            outlineOp(list) { this.apply(list); },
            outlineRemove(node) { this.apply([{ op: "remove", at: node.path }]); },
            openText() {
                const node = this.selection && this.selection.node;
                this.ctx.showText();
                if (node && this.selection.which === "edit" && this.edit === this.tab) this.tab.line = node.line;
            }
        },
        template: `
<div class="m-dev-screen-view column no-wrap" id="DevScreenView">
  <div class="row items-center q-gutter-x-sm q-px-sm q-py-xs m-dev-screen-bar">
    <q-btn-toggle dense no-caps unelevated toggle-color="primary" :model-value="mode" @update:model-value="setMode"
        :options="[{ label: 'Design', value: 'design' }, { label: 'Preview', value: 'preview' }]" id="DevScreenMode"></q-btn-toggle>
    <q-btn-toggle dense no-caps flat v-model="width" :options="widths"></q-btn-toggle>
    <q-toggle dense v-model="dark" label="Dark"></q-toggle>
    <q-input dense outlined v-model="query" placeholder="Parameters: a=1&amp;b=2" class="col" @keyup.enter="render" id="DevScreenQuery"></q-input>
    <q-btn dense flat round icon="sym_o_refresh" :loading="rendering" @click="render"><q-tooltip>Render again</q-tooltip></q-btn>
  </div>
  <div class="text-caption q-px-sm" id="DevScreenTarget" v-if="view">
    <template v-if="extensionMode && edit">Extension mode: the edits go to {{view.site}}'s {{edit.path}}{{edit.missing ? ' (made on Save)' : ''}}</template>
    <template v-else-if="extensionMode">Upstream and read only: create a site component on Home to extend it</template>
    <template v-else>Editing {{edit ? edit.path : ''}}</template>
    <span class="m-muted"> · {{view.urlPath}}</span>
  </div>
  <div v-if="error" class="text-negative q-pa-sm">{{error}}</div>
  <div class="col row no-wrap m-dev-screen-body">
    <div class="m-dev-screen-side column no-wrap">
      <q-btn-toggle dense no-caps flat spread v-model="panel" @update:model-value="v => v === 'actions' && loadComposer()"
          :options="[{ label: 'Extension points', value: 'points' }, { label: 'Outline', value: 'outline' }, { label: 'Actions', value: 'actions' }]" id="DevScreenPanel"></q-btn-toggle>
      <div v-show="panel === 'actions'" class="col scroll q-pa-xs" id="DevScreenActions">
        <q-select dense outlined options-dense emit-value map-options v-model="actionsKey" :options="actionHolders.map(a => ({ label: a.label, value: a.key }))"
            label="Actions of" class="q-mb-xs"></q-select>
        <div v-if="!actionHolders.length" class="text-caption m-muted">{{extensionMode ? 'Add Actions before or after on the right first.' : 'No actions yet.'}}</div>
        <m-dev-composer v-if="composerReady && selectedActions" :ctx="ctx" :actions="selectedActions" :read-only="!canEdit" :in-parameters="screenParameters"
            :apply="apply" :text="edit === tab ? tab.text : (edit ? edit.text : '')"></m-dev-composer>
      </div>
      <div v-show="panel === 'points'" class="col scroll q-pa-xs" id="DevExtensionPoints">
        <div v-for="point in points" :key="point.path.join('.') + point.kind" class="m-dev-point" :class="{ unnamed: !point.named }" @click="selectPoint(point)">
          <span class="m-dev-mono">{{point.kind}}</span> <b>{{point.name}}</b>
          <div v-if="!point.named" class="text-caption m-dev-unnamed">cannot be anchored{{point.nearest ? '; nearest anchor is ' + point.nearest.name : ''}}</div>
        </div>
        <div v-if="!points.length" class="m-muted text-caption">No anchors.</div>
      </div>
      <div v-show="panel === 'outline'" class="col scroll q-pa-xs" id="DevScreenOutline">
        <m-dev-block-tree :nodes="outlineNodes" :selected="selectedPath" :read-only="!canEdit" :label-fn="labelOf"
            :container-fn="containerOf" @select="selectNode" @op="outlineOp" @remove="outlineRemove"></m-dev-block-tree>
      </div>
    </div>
    <div class="col m-dev-canvas" ref="canvas">
      <div v-if="frameError" class="q-pa-md m-muted">{{frameError}}</div>
      <iframe v-if="frameSrc" ref="frame" :src="frameSrc" :style="frameStyle" class="m-dev-frame" @load="frameLoaded" id="DevCanvasFrame"></iframe>
      <div v-if="hover && mode === 'design'" class="m-dev-hover" :style="{ left: hover.left + 'px', top: hover.top + 'px', width: hover.width + 'px', height: hover.height + 'px' }">
        <span class="m-dev-hover-label">{{hover.label}}</span></div>
      <div v-if="selectedBox && mode === 'design'" class="m-dev-selected" :style="{ left: selectedBox.left + 'px', top: selectedBox.top + 'px', width: selectedBox.width + 'px', height: selectedBox.height + 'px' }"></div>
    </div>
    <div class="m-dev-screen-panel scroll q-pa-sm" id="DevSelection">
      <template v-if="selection && selection.node">
        <div class="text-subtitle2">{{selection.node.name}} <span class="m-muted">{{selection.node.attrs.find(a => a[0] === 'name' || a[0] === 'id') ? (selection.node.attrs.find(a => a[0] === 'name' || a[0] === 'id') || [])[1] : ''}}</span></div>
        <div class="text-caption m-muted q-mb-sm">
          <q-icon :name="selection.loc && selection.loc.site ? 'sym_o_extension' : 'sym_o_lock'" size="14px"></q-icon>
          {{selection.loc ? selection.loc.path + ':' + selection.loc.line : ''}}
        </div>
        <div class="column q-gutter-y-xs" v-if="intents">
          <q-btn v-if="intents.form && intents.form.kind === 'form-list'" dense unelevated no-caps color="primary" icon="sym_o_view_column" label="Add column"
              :disable="!canEdit" @click="startField" id="DevAddColumn"></q-btn>
          <q-btn v-if="intents.form && intents.form.kind === 'form-single'" dense unelevated no-caps color="primary" icon="sym_o_add_box" label="Add field"
              :disable="!canEdit" @click="startField" id="DevAddFormField"></q-btn>
          <template v-if="intents.form && intents.form.kind === 'form-list'">
            <q-btn dense flat no-caps icon="sym_o_filter_alt" label="Add filter" :disable="!canEdit" @click="startIntent('filter')" id="DevAddFilter"></q-btn>
            <q-btn dense flat no-caps icon="sym_o_ads_click" label="Add row action" :disable="!canEdit" @click="startIntent('rowAction')"></q-btn>
          </template>
          <template v-if="intents.anchor || intents.nearest || !extensionMode">
            <q-btn dense flat no-caps icon="sym_o_view_agenda" label="Add section" :disable="!canEdit" @click="startIntent('section')" id="DevAddSection"></q-btn>
            <q-btn dense flat no-caps icon="sym_o_smart_button" label="Add button" :disable="!canEdit" @click="startIntent('button')" id="DevAddButton"></q-btn>
          </template>
          <q-btn v-if="intents.unnamed && extensionMode && ctx.tier === 'full' && ctx.upstreamEdits" dense flat no-caps icon="sym_o_anchor" label="Add an anchor here"
              @click="addAnchor" id="DevAddAnchor"></q-btn>
          <template v-if="intents.anchor">
            <q-btn dense flat no-caps icon="sym_o_vertical_align_top" label="Insert before" :disable="!canEdit" @click="startInsert('before')"></q-btn>
            <q-btn dense flat no-caps icon="sym_o_vertical_align_bottom" label="Insert after" :disable="!canEdit" @click="startInsert('after')" id="DevInsertAfter"></q-btn>
            <q-btn v-if="intents.anchor.container" dense flat no-caps icon="sym_o_add" label="Insert inside" :disable="!canEdit" @click="startInsert('append')"></q-btn>
          </template>
          <div v-if="intents.unnamed && extensionMode" class="text-caption m-dev-unnamed" id="DevNearestAnchor">This {{selection.node.name}} has no name, so it cannot be anchored{{intents.nearest ? '; nearest anchor is ' + intents.nearest.name : ''}}.
            <q-btn v-if="intents.nearest" dense flat no-caps size="sm" :label="'Insert ' + intents.nearest.where + ' ' + intents.nearest.name" :disable="!canEdit"
                @click="startInsert(intents.nearest.where, intents.nearest.name)"></q-btn>
            <div v-if="ctx.tier === 'full' && !ctx.upstreamEdits" class="m-muted">With "Allow editing upstream components" on you may name it in the base screen.</div></div>
          <template v-if="!extensionMode && selection.which === 'edit'">
            <q-btn dense flat no-caps icon="sym_o_vertical_align_top" label="Insert before" :disable="!canEdit" v-if="!intents.anchor" @click="startInsert('before')"></q-btn>
            <q-btn dense flat no-caps icon="sym_o_vertical_align_bottom" label="Insert after" :disable="!canEdit" v-if="!intents.anchor" @click="startInsert('after')"></q-btn>
            <q-btn dense flat no-caps icon="sym_o_delete" label="Remove" :disable="!canEdit" @click="removeSelected"></q-btn>
          </template>
          <template v-if="extensionMode">
            <q-btn v-if="/^section/.test(selection.node.name) && intents.anchor" dense flat no-caps icon="sym_o_swap_horiz" label="Replace section"
                :disable="!canEdit" @click="copyBase" id="DevReplaceSection"></q-btn>
            <q-btn v-if="selection.node.name === 'transition'" dense flat no-caps icon="sym_o_alt_route" label="Override transition"
                :disable="!canEdit" @click="copyBase" id="DevOverrideTransition"></q-btn>
          </template>
          <q-btn dense flat no-caps icon="sym_o_code" label="Open in Text" @click="openText"></q-btn>
        </div>
        <template v-if="elementSpec && canEdit">
          <div class="text-subtitle2 q-mt-md">Properties</div>
          <div v-for="spec in elementSpec.attributes" :key="spec.name" class="q-mb-xs">
            <select v-if="spec.values && spec.values.length" class="m-dev-cell m-dev-prop" :value="attrValue(spec.name)" @change="setAttribute(spec.name, $event.target.value)">
              <option value=""></option><option v-for="value in spec.values" :key="value" :value="value">{{value}}</option></select>
            <input v-else class="m-dev-cell m-dev-prop" :value="attrValue(spec.name)" :placeholder="spec.default || ''" @change="setAttribute(spec.name, $event.target.value)">
            <div class="text-caption m-muted" :title="spec.doc || ''">{{spec.name}}{{spec.required ? ' *' : ''}}</div>
          </div>
        </template>
      </template>
      <div v-else-if="selection && selection.loc && selection.loc.dbform" class="text-caption" id="DevDbFormField">A field from DbForm
        <b>{{selection.loc.dbform}}</b>{{selection.loc.groups.length ? ' for ' + selection.loc.groups.join(', ') : ''}}: a configuration in the database, changed
        in System's form settings, not a file.</div>
      <div v-else class="m-muted text-caption">Click an element on the canvas, an extension point, or a block of the Outline.</div>
      <div v-if="extensionMode" class="q-mt-md">
        <div class="text-caption text-weight-medium">The screen's actions</div>
        <q-btn dense flat no-caps size="sm" label="Actions before" :disable="!canEdit" @click="apply(actionsExtendOps('before'))"></q-btn>
        <q-btn dense flat no-caps size="sm" label="Actions after" :disable="!canEdit" @click="apply(actionsExtendOps('after'))"></q-btn>
      </div>
    </div>
  </div>
  <q-dialog :model-value="!!adding" @update:model-value="v => { if (!v) adding = null; }">
    <q-card v-if="adding" style="width: 30rem; max-width: 95vw" id="DevAddScreenField">
      <q-card-section class="text-subtitle1">{{adding.kind === 'form-list' ? 'Add a column to ' : 'Add a field to '}}{{adding.form}}</q-card-section>
      <q-card-section class="q-pt-none q-gutter-y-sm">
        <q-input dense outlined v-model="adding.name" label="Field name" autofocus for="DevScreenFieldName"></q-input>
        <q-input dense outlined v-model="adding.title" label="Title" for="DevScreenFieldTitle"></q-input>
        <q-select dense outlined v-model="adding.widget" :options="['display', 'text-line', 'text-area', 'date-time', 'drop-down', 'check', 'link']" label="Widget"></q-select>
        <q-select dense outlined clearable v-model="adding.after" :options="adding.fields" label="After"></q-select>
        <div class="text-caption m-muted" v-if="extensionMode">Written to the site's screen-extend as a {{adding.kind}} merge; after= places it in the layout or its column.</div>
      </q-card-section>
      <q-card-actions align="right"><q-btn flat no-caps label="Cancel" @click="adding = null"></q-btn>
        <q-btn unelevated no-caps color="primary" label="Add" @click="addField" id="DevScreenFieldAdd"></q-btn></q-card-actions>
    </q-card>
  </q-dialog>
  <q-dialog :model-value="!!intent" @update:model-value="v => { if (!v) intent = null; }">
    <q-card v-if="intent" style="width: 32rem; max-width: 95vw" id="DevIntentDialog">
      <q-card-section class="text-subtitle1">{{ { filter: 'Add a filter', section: 'Add a section', button: 'Add a button', rowAction: 'Add a row action' }[intent.kind] }}</q-card-section>
      <q-card-section class="q-pt-none q-gutter-y-sm">
        <template v-if="intent.kind === 'filter'">
          <div v-if="intent.why" class="text-warning text-caption" id="DevFilterWhy">{{intent.why}}</div>
          <q-input dense outlined v-model="intent.fieldName" label="The column's field" for="DevFilterField"></q-input>
        </template>
        <template v-if="intent.kind === 'section'">
          <q-btn-toggle dense no-caps v-model="intent.sectionKind" :options="[{ label: 'A box', value: 'box' }, { label: 'A related list', value: 'related' }]"
></q-btn-toggle>
          <q-input dense outlined v-model="intent.name" label="Name (its anchor)"></q-input>
          <q-input dense outlined v-model="intent.title" label="Title"></q-input>
          <template v-if="intent.sectionKind === 'related'">
            <q-input dense outlined v-model="intent.entity" label="The related entity"></q-input>
            <q-input dense outlined v-model="intent.keyField" label="Its field that holds this page's key"></q-input>
          </template>
        </template>
        <template v-if="intent.kind === 'button' || intent.kind === 'rowAction'">
          <q-input dense outlined v-model="intent.text" label="Its text"></q-input>
          <q-input dense outlined v-model="intent.transition" label="The transition (its name)"></q-input>
          <q-input v-if="intent.kind === 'button'" dense outlined v-model="intent.service" label="The service it calls"></q-input>
          <q-input v-if="intent.kind === 'rowAction'" dense outlined v-model="intent.keyField" label="The row's key field it passes"></q-input>
        </template>
      </q-card-section>
      <q-card-actions align="right"><q-btn flat no-caps label="Cancel" @click="intent = null"></q-btn>
        <q-btn unelevated no-caps color="primary" label="Add" :disable="intent.kind === 'filter' && !!intent.why" @click="doIntent" id="DevIntentDo"></q-btn></q-card-actions>
    </q-card>
  </q-dialog>
  <q-dialog :model-value="!!inserting" @update:model-value="v => { if (!v) inserting = null; }">
    <q-card v-if="inserting" style="width: 32rem; max-width: 95vw" id="DevInsertDialog">
      <q-card-section class="text-subtitle1">Insert {{inserting.where}} {{inserting.anchor}}</q-card-section>
      <q-card-section class="q-pt-none q-gutter-y-sm">
        <q-select dense outlined emit-value map-options v-model="inserting.snippet" :options="snippets.map(s => ({ label: s.label, value: s.key }))" label="Widget"></q-select>
        <q-input dense outlined type="textarea" autogrow v-model="inserting.text" label="Or the XML" class="m-dev-mono"></q-input>
      </q-card-section>
      <q-card-actions align="right"><q-btn flat no-caps label="Cancel" @click="inserting = null"></q-btn>
        <q-btn unelevated no-caps color="primary" label="Insert" @click="insert" id="DevInsertDo"></q-btn></q-card-actions>
    </q-card>
  </q-dialog>
</div>`
    };
    screenView.computed.widths = () => WIDTHS;
    screenView.computed.snippets = () => ops.SNIPPETS;
    screenView.methods.labelOf = (node) => ops.labelOf(node);
    screenView.methods.containerOf = (node) => ["widgets", "container", "container-box", "box-body", "box-header", "box-toolbar", "box-body-nopad", "container-row",
        "row-col", "section", "section-iterate", "container-panel", "panel-header", "panel-left", "panel-center", "panel-right", "panel-footer",
        "container-split", "split-side", "split-main", "container-dialog", "widgets-extend", "fail-widgets"].indexOf(node.name) >= 0;
    screenView.methods.attrValue = function (name) {
        const node = this.selection && this.selection.node;
        return (node && attrOf(node, name)) || "";
    };

    devAide.registerView({ name: "screen", label: "Screen", roots: ["screen", "screen-extend"], component: "m-dev-screen-view",
        script: "/devstatic/DevAideScreen.js", ready: () => !!devAide.screenViewRegistered });
    const register = () => {
        const qvt = moqui.qvt;
        if (qvt && qvt.registerLate) qvt.registerLate("m-dev-screen-view", screenView);
        devAide.components["m-dev-screen-view"] = screenView;
        devAide.screenViewRegistered = true;
    };
    // the Outline is the block tree's
    if (devAide.blocksRegistered) register();
    else if (moqui.loadScript) moqui.loadScript("/devstatic/DevAideBlocks.js", null, () => !!devAide.blocksRegistered).then(register, register);
    else register();
    if (root.document && !root.document.getElementById("m-dev-screen-style")) {
        const style = root.document.createElement("style");
        style.id = "m-dev-screen-style";
        style.textContent = ".m-dev-screen-view{min-height:0}.m-dev-screen-body{min-height:0}" +
            ".m-dev-screen-side{width:240px;min-width:180px;border-right:1px solid var(--m-border,rgba(0,0,0,.12))}" +
            ".m-dev-screen-panel{width:260px;min-width:200px;border-left:1px solid var(--m-border,rgba(0,0,0,.12))}" +
            ".m-dev-canvas{position:relative;overflow:auto;background:repeating-conic-gradient(rgba(127,127,127,.06) 0 25%,transparent 0 50%) 50%/16px 16px}" +
            ".m-dev-frame{display:block;height:100%;min-height:32rem;border:0;margin:0 auto;background:#fff}" +
            ".m-dev-hover{position:absolute;pointer-events:none;outline:2px dashed #1976d2;z-index:2}" +
            ".m-dev-hover-label{position:absolute;left:0;top:-18px;background:#1976d2;color:#fff;font-size:11px;padding:0 4px;border-radius:2px;white-space:nowrap}" +
            ".m-dev-selected{position:absolute;pointer-events:none;outline:2px solid #e2a400;z-index:1}" +
            ".m-dev-point{padding:2px 4px;border-radius:4px;cursor:pointer;font-size:12px}.m-dev-point:hover{background:rgba(127,127,127,.1)}" +
            ".m-dev-point.unnamed{opacity:.75}.m-dev-unnamed{color:#b26a00}.m-dev-prop{width:100%}" +
            "@media (max-width:1023px){.m-dev-screen-body.row{flex-direction:column}.m-dev-screen-side,.m-dev-screen-panel{width:auto;border:none}}";
        root.document.head.appendChild(style);
    }
})(typeof window !== "undefined" ? window : globalThis);
