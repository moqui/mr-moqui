/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The service writer and the ECA view of the Dev AIDE (plans/design-dev-aide.md 9, 10). m-dev-service-view is the
 * visual view of a service file's tab: one service at a time, its header and settings from the schema digest, its
 * parameters (nested, those auto-parameters imply marked), its implementation (inline actions in the logic composer, a
 * script service's file, an entity-auto's entity), its SECA rules with Add (into the site's secas file, D17), what uses
 * it and the last spec run's calls, and the Try panel (9.4): a form of the in-parameters, Run in sim or for real, the
 * rows the sim would write, Save as spec and Run spec, and a snippet runner. m-dev-composer is the logic composer
 * (9.2): the actions as blocks on the block tree, each a sentence with its main attributes as chips that edit one
 * attribute, the palette by the XSD's groups with snippets, the names known before each block marking an unknown one in
 * an expression, a service-call's in-parameters to add, a script's popup editor, and the generated JavaScript.
 * m-dev-eca-view is the view of an ECA file: its rules, a rule's attributes, its condition and its actions in the
 * composer, and Add, Remove and Move. Every edit is an operation of the lossless XML layer on a buffer.
 *
 * The pure parts are moqui.devAide.serviceOps, which test/client/pages/DevAideService.page.js runs.
 */
(function (root) {
    "use strict";

    const moqui = root.moqui;
    const devAide = moqui.devAide;
    const pure = devAide.pure;
    const ops = devAide.serviceOps = {};
    const attrOf = pure.attr;
    const children = (node, names) => ((node && node.children) || []).filter((child) => !names || names.indexOf(child.name) >= 0);
    const escape = (value) => String(value == null ? "" : value).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/"/g, "&quot;");
    const xml = (name, attrs, body) => {
        let out = "<" + name;
        for (const [key, value] of attrs || []) if (value !== undefined && value !== null && value !== "") out += " " + key + "=\"" + escape(value) + "\"";
        return body ? out + ">" + body + "</" + name + ">" : out + "/>";
    };
    ops.xml = xml;

    // ---- pure: the sentences ----

    // Each XML Actions element as a sentence: its words, with the attributes that are its chips (9.2). A chip of kind
    // expr holds an expression, entity an entity name, service a service name, name a name the block sets.
    const S = (parts) => parts;
    ops.SENTENCES = {
        "set": S(["Set", { attr: "field", kind: "name" }, "=", { attr: "from", kind: "expr", or: "value" }]),
        "entity-find-one": S(["Find one", { attr: "entity-name", kind: "entity" }, "→", { attr: "value-field", kind: "name" }]),
        "entity-find": S(["Find", { attr: "entity-name", kind: "entity" }, "→", { attr: "list", kind: "name" }]),
        "entity-find-count": S(["Count", { attr: "entity-name", kind: "entity" }, "→", { attr: "count-field", kind: "name" }]),
        "entity-find-related": S(["Find related", { attr: "relationship-name", kind: "text" }, "of", { attr: "value-field", kind: "expr" }, "→", { attr: "list", kind: "name" }]),
        "entity-find-related-one": S(["Find related one", { attr: "relationship-name", kind: "text" }, "of", { attr: "value-field", kind: "expr" }, "→", { attr: "to-value-field", kind: "name" }]),
        "econdition": S(["where", { attr: "field-name", kind: "text" }, { attr: "operator", kind: "text", fallback: "equals" }, { attr: "from", kind: "expr", or: "value" }]),
        "service-call": S(["Call", { attr: "name", kind: "service" }, "with", { attr: "in-map", kind: "expr", fallback: "its fields" }, "→", { attr: "out-map", kind: "name" }]),
        "if": S(["If", { attr: "condition", kind: "expr" }]),
        "else-if": S(["Else if", { attr: "condition", kind: "expr" }]),
        "else": S(["Else"]),
        "while": S(["While", { attr: "condition", kind: "expr" }]),
        "iterate": S(["For each", { attr: "entry", kind: "name" }, "in", { attr: "list", kind: "expr" }]),
        "return": S(["Return", { attr: "error", kind: "text", flag: "error" }, { attr: "message", kind: "text" }]),
        "message": S(["Message", { attr: "error", kind: "text", flag: "error" }, { text: true, kind: "text" }]),
        "log": S(["Log", { attr: "level", kind: "text", fallback: "info" }, { attr: "message", kind: "text" }]),
        "script": S(["Script", { text: true, kind: "code" }]),
        "check-errors": S(["Check errors"]),
        "break": S(["Break"]),
        "continue": S(["Continue"]),
        "assert": S(["Assert", { attr: "title", kind: "text" }]),
        "entity-make-value": S(["Make", { attr: "entity-name", kind: "entity" }, "→", { attr: "value-field", kind: "name" }]),
        "entity-create": S(["Create", { attr: "value-field", kind: "expr" }]),
        "entity-update": S(["Update", { attr: "value-field", kind: "expr" }]),
        "entity-delete": S(["Delete", { attr: "value-field", kind: "expr" }]),
        "entity-set": S(["Set fields of", { attr: "value-field", kind: "expr" }, "from", { attr: "map", kind: "expr", fallback: "the context" }]),
        "entity-sequenced-id-primary": S(["Sequenced id of", { attr: "value-field", kind: "expr" }]),
        "entity-delete-by-condition": S(["Delete", { attr: "entity-name", kind: "entity" }, "where its conditions"]),
        "filter-map-list": S(["Filter", { attr: "list", kind: "expr" }]),
        "order-map-list": S(["Order", { attr: "list", kind: "expr" }]),
        "field-map": S(["with", { attr: "field-name", kind: "text" }, "=", { attr: "from", kind: "expr", or: "value" }]),
        "order-by": S(["order by", { attr: "field-name", kind: "text" }]),
        "select-field": S(["select", { attr: "field-name", kind: "text" }])
    };
    // A block's sentence: { text, chips } for the block tree, each chip with the attribute it edits.
    ops.sentence = (node, known) => {
        const parts = ops.SENTENCES[node.name];
        if (!parts) {
            const chips = (node.attrs || []).slice(0, 2).map(([name, value]) => ({ text: name + "=" + value, attr: name, kind: "text", edit: true }));
            return { text: node.name, chips };
        }
        const words = [];
        const chips = [];
        for (const part of parts) {
            // the first word names the block; the later ones read between its chips
            if (typeof part === "string") { if (words.length) chips.push({ text: part, word: true }); words.push(part); continue; }
            if (part.flag) { if (attrOf(node, part.attr) === "true") chips.push({ text: part.flag, attr: part.attr, kind: "text", edit: true }); continue; }
            if (part.text) {
                const text = node.text || "";
                chips.push({ text: part.kind === "code" ? (text.split("\n").find((line) => line.trim()) || "(empty)").trim() : text || "…", textual: true, kind: part.kind, edit: true });
                continue;
            }
            let attr = part.attr;
            let value = attrOf(node, attr);
            if (value === undefined && part.or && attrOf(node, part.or) !== undefined) { attr = part.or; value = attrOf(node, part.or); }
            const chip = { text: value === undefined ? (part.fallback || "…") : (part.or && attr === part.or ? "\"" + value + "\"" : value), attr, kind: attr === part.or ? "text" : part.kind, edit: true, empty: value === undefined };
            if (chip.kind === "expr" && value !== undefined && known) {
                const unknown = ops.unknownNames(value, known);
                if (unknown.length) { chip.warn = true; chip.title = "Not set before this block: " + unknown.join(", "); }
            }
            chips.push(chip);
        }
        return { text: words[0] || node.name, chips, words };
    };

    // The globals an expression may name besides the context.
    const GLOBALS = new Set(["ec", "context", "true", "false", "null", "undefined", "Math", "JSON", "Date", "Number", "String", "Boolean", "Array",
        "Object", "parseInt", "parseFloat", "isNaN", "typeof", "instanceof", "new", "this", "in", "of", "let", "const", "var", "return", "if", "else",
        "function", "BigDecimal", "Timestamp", "nowTimestamp", "user", "l10n", "resource", "message"]);
    // The names an expression uses that are not known before its block: identifiers not after a dot, outside quotes.
    ops.unknownNames = (expression, known) => {
        const bare = String(expression || "").replace(/"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|`(?:[^`\\]|\\.)*`/g, "\"\"");
        const found = [];
        const pattern = /(^|[^.\w$])([A-Za-z_$][\w$]*)/g;
        let match;
        while ((match = pattern.exec(bare))) {
            const name = match[2];
            if (!GLOBALS.has(name) && !known.has(name) && found.indexOf(name) < 0) found.push(name);
        }
        return found;
    };
    // The names known before each block of an actions element, by its path: the in-parameters, and what the blocks
    // before it set (set's field, value-field, list, out-map, count-field, an iterate's entry inside it).
    ops.knownBefore = (actions, inParameters) => {
        const known = new Map();
        const names = new Set(inParameters || []);
        const SETS = ["field", "value-field", "list", "out-map", "count-field", "to-value-field", "entry", "key"];
        const visit = (node, inherited) => {
            const here = new Set(inherited);
            for (const child of node.children || []) {
                known.set(child.path.join("."), new Set(here));
                if (child.name === "iterate") {
                    const inside = new Set(here);
                    for (const name of ["entry", "key"]) { const value = attrOf(child, name); if (value) inside.add(value); }
                    visit(child, inside);
                } else visit(child, here);
                for (const name of SETS) {
                    const value = attrOf(child, name);
                    if (value && child.name !== "iterate" && /^[A-Za-z_$][\w$]*$/.test(value.split(".")[0])) here.add(value.split(".")[0]);
                }
            }
        };
        visit(actions || { children: [] }, names);
        return known;
    };
    // The palette (9.2), by the XSD's groups, each with what it inserts.
    ops.PALETTE = [
        { group: "Control", items: [
            ["If", "<if condition=\"true\"></if>"], ["For each", "<iterate list=\"list\" entry=\"item\"></iterate>"], ["While", "<while condition=\"false\"></while>"],
            ["Message", "<message>Done</message>"], ["Return", "<return/>"], ["Return error", "<return error=\"true\" message=\"Not found\"/>"],
            ["Check errors", "<check-errors/>"], ["Break", "<break/>"], ["Continue", "<continue/>"]] },
        { group: "Environment", items: [["Set", "<set field=\"name\" from=\"null\"/>"], ["Filter a list", "<filter-map-list list=\"list\"/>"],
            ["Order a list", "<order-map-list list=\"list\"><order-by field-name=\"name\"/></order-map-list>"]] },
        { group: "Calls", items: [["Call a service", "<service-call name=\"\" out-map=\"result\"/>"], ["Script", "<script><![CDATA[\n]]></script>"],
            ["Log", "<log level=\"info\" message=\"\"/>"]] },
        { group: "Entity find", items: [["Find one by its key", "<entity-find-one entity-name=\"\" value-field=\"value\"/>"],
            ["Find", "<entity-find entity-name=\"\" list=\"valueList\"><econdition field-name=\"\" from=\"\"/></entity-find>"],
            ["Count", "<entity-find-count entity-name=\"\" count-field=\"count\"/>"],
            ["Loop over a find", "<entity-find entity-name=\"\" list=\"valueList\"/><iterate list=\"valueList\" entry=\"value\"></iterate>"]] },
        { group: "Entity values", items: [["Create from the parameters", "<service-call name=\"create#\" in-map=\"context\" out-map=\"context\"/>"],
            ["Make a value", "<entity-make-value entity-name=\"\" value-field=\"value\"/>"], ["Create", "<entity-create value-field=\"value\"/>"],
            ["Update", "<entity-update value-field=\"value\"/>"], ["Delete", "<entity-delete value-field=\"value\"/>"],
            ["Set fields", "<entity-set value-field=\"value\"/>"]] }
    ];
    // The containers among the blocks: what holds blocks, and the children shown as blocks (a condition element is part
    // of its block's sentence).
    ops.CONTAINERS = ["actions", "if", "else-if", "else", "iterate", "while", "then", "entity-find", "service-call"];
    ops.blockChildren = (node) => children(node).filter((child) => child.name !== "condition" && child.name !== "description");
    // A chip's edit: its attribute set (removed when empty), or a textual block's text.
    ops.chipOp = (node, chip, value) => {
        if (chip.textual) return [{ op: "set-text", at: node.path, text: value }];
        if (value === "" || value == null) return attrOf(node, chip.attr) === undefined ? [] : [{ op: "remove-attribute", at: node.path, name: chip.attr }];
        return [{ op: "set-attribute", at: node.path, name: chip.attr, value }];
    };
    // An insert from the palette: after the selected block, else last in the actions.
    ops.insertOp = (actions, selected, snippet) => selected && selected !== actions
        ? [{ op: "insert", at: selected.path, position: "after", xml: snippet }]
        : [{ op: "insert", at: actions.path, position: "last", xml: snippet }];
    // A service-call's in-parameter added as a field-map.
    ops.fieldMapOp = (call, name) => [{ op: "insert", at: call.path, position: "last", xml: xml("field-map", [["field-name", name], ["from", name]]) }];

    // ---- pure: the service file ----

    // The service elements of a services file, each with its verb#noun.
    ops.servicesOf = (outline) => children(outline, ["service"]).map((node) => ({ node, name: (attrOf(node, "verb") || "") + (attrOf(node, "noun") ? "#" + attrOf(node, "noun") : "") }));
    ops.parameterXml = (parameter) => xml("parameter", [["name", parameter.name], ["type", parameter.type && parameter.type !== "String" ? parameter.type : ""],
        ["required", parameter.required ? "true" : ""], ["default-value", parameter.defaultValue]],
    parameter.description ? "<description>" + escape(parameter.description) + "</description>" : "");
    // A parameter into in-parameters or out-parameters, the element made when the service has none.
    ops.addParameter = (service, which, parameter) => {
        const holder = children(service, [which])[0];
        const text = ops.parameterXml(parameter);
        if (holder) return [{ op: "insert", at: holder.path, position: "last", xml: text }];
        const before = children(service, which === "in-parameters" ? ["out-parameters", "actions"] : ["actions"])[0];
        const made = xml(which, [], text);
        return before ? [{ op: "insert", at: before.path, position: "before", xml: made }] : [{ op: "insert", at: service.path, position: "last", xml: made }];
    };
    // A SECA rule on a service, into a secas file.
    ops.secaXml = (service, when) => xml("seca", [["service", service], ["when", when || "post-service"]], "<actions></actions>");
    ops.addRule = (outline, ruleXml) => [{ op: "insert", at: (outline && outline.path) || [], position: "last", xml: ruleXml }];
    // The in-parameters of a Try as a call's parameters: the values given, typed by the parameter's type.
    ops.tryParameters = (values, parameters) => {
        const out = {};
        for (const parameter of parameters || []) {
            const value = values[parameter.name];
            if (value === undefined || value === "") continue;
            out[parameter.name] = /Integer|Long|BigDecimal|Double|Number/.test(parameter.type || "") && value !== "" && !isNaN(Number(value)) ? Number(value)
                : parameter.type === "Boolean" ? value === true || value === "true" : value;
        }
        return out;
    };
    ops.SECAS_TEXT = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<secas xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" " +
        "xsi:noNamespaceSchemaLocation=\"http://moqui.org/xsd/service-eca-3.xsd\">\n</secas>\n";

    // ---- the composer ----

    const composer = {
        name: "mDevComposer",
        props: { ctx: Object, actions: Object, readOnly: Boolean, inParameters: Array, apply: Function, text: String },
        data() { return { selected: null, chipEdit: null, scriptEdit: null, generated: null, called: {}, paletteFor: null }; },
        computed: {
            known() { return ops.knownBefore(this.actions, this.inParameters); },
            nodes() { return this.actions ? ops.blockChildren(this.actions) : []; },
            selectedNode() { return this.selected ? this.nodeAt(this.selected) : null; },
            calledService() {
                const node = this.selectedNode;
                return node && node.name === "service-call" ? attrOf(node, "name") : "";
            }
        },
        watch: {
            calledService(name) {
                if (!name || this.called[name] !== undefined) return;
                this.called[name] = null;
                this.ctx.get("serviceView", { serviceName: name }).then((answer) => {
                    this.called[name] = ((answer.view && answer.view.service && answer.view.service.inParameters) || []).map((parameter) => parameter);
                }, () => { this.called[name] = []; });
            }
        },
        methods: {
            labelFn(node) {
                const known = this.known.get(node.path.join(".")) || new Set();
                return ops.sentence(node, known);
            },
            containerFn(node) { return ops.CONTAINERS.indexOf(node.name) >= 0; },
            childrenFn(node) { return ops.blockChildren(node); },
            select(node) { this.selected = node.path; },
            // the actions' outline is a part of the file's: find a node by its path in the file's outline
            nodeAt(path) {
                let node = this.actions;
                if (!node) return null;
                const base = node.path.length;
                for (const index of path.slice(base)) node = node && node.children ? node.children[index] : null;
                return node;
            },
            chip(event) {
                const { node, chip } = event;
                if (this.readOnly) return;
                if (chip.kind === "code") { this.scriptEdit = { node, text: node.text || "" }; return; }
                this.chipEdit = { node, chip, value: chip.textual ? node.text || "" : attrOf(node, chip.attr) || "", options: [] };
            },
            saveChip() {
                const edit = this.chipEdit;
                this.chipEdit = null;
                this.apply(ops.chipOp(edit.node, edit.chip, edit.value));
            },
            saveScript() {
                const edit = this.scriptEdit;
                this.scriptEdit = null;
                this.apply([{ op: "set-text", at: edit.node.path, text: edit.text }]);
            },
            searchNames(text, update) {
                const chip = this.chipEdit && this.chipEdit.chip;
                const kinds = chip && chip.kind === "service" ? "service" : "entity,view-entity";
                if (!text || text.length < 2) { update(() => {}); return; }
                this.ctx.get("artifacts", { text, kinds, limit: 20 }).then((answer) => {
                    update(() => { this.chipEdit.options = (answer.artifacts || []).map((artifact) => artifact.name); });
                }, () => update(() => {}));
            },
            insert(snippet) {
                const selected = this.paletteFor || this.selectedNode;
                this.paletteFor = null;
                this.apply(ops.insertOp(this.actions, selected, snippet));
            },
            openPalette(node) { this.paletteFor = node || null; this.$refs.palette && this.$refs.palette.show(); },
            onOp(list) { this.apply(list); },
            remove(node) { this.apply([{ op: "remove", at: node.path }]); },
            addFieldMap(name) { const node = this.selectedNode; if (node) this.apply(ops.fieldMapOp(node, name)); },
            showGenerated() {
                const text = this.text || "";
                const source = this.actions ? text.slice(this.actions.from16, this.actions.to16) : "";
                this.ctx.post("generatedJs", { xml: source }).then((answer) => {
                    this.generated = answer.error ? "// " + answer.error : answer.source || "";
                }, (error) => this.ctx.fail(error));
            }
        },
        template: `
<div class="m-dev-composer" id="DevComposer">
  <div class="row items-center q-gutter-x-sm q-mb-xs">
    <q-btn dense flat no-caps icon="add" label="Add" :disable="readOnly || !actions" @click="paletteFor = selectedNode" id="DevComposerAdd">
      <q-menu ref="palette"><q-list dense style="min-width: 16rem">
        <template v-for="group in palette" :key="group.group">
          <q-item-label header class="q-py-xs">{{group.group}}</q-item-label>
          <q-item v-for="item in group.items" :key="item[0]" clickable v-close-popup @click="insert(item[1])" :data-palette="item[0]"><q-item-section>{{item[0]}}</q-item-section></q-item>
        </template></q-list></q-menu></q-btn>
    <span class="text-caption m-muted">/ adds, Alt+Up and Down move, Delete removes</span>
    <q-space></q-space>
    <q-btn dense flat no-caps icon="sym_o_javascript" label="Generated" :disable="!actions" @click="showGenerated" id="DevComposerGenerated"></q-btn>
  </div>
  <div v-if="!actions" class="m-muted text-caption">No actions.</div>
  <m-dev-block-tree v-else :nodes="nodes" :parent-path="actions.path" :selected="selected" :read-only="readOnly" :label-fn="labelFn"
      :children-fn="childrenFn" :container-fn="containerFn" @select="select" @op="onOp" @chip="chip" @remove="remove" @insert="openPalette"></m-dev-block-tree>
  <div v-if="calledService && called[calledService] && called[calledService].length" class="q-mt-sm" id="DevCalledParameters">
    <div class="text-caption text-weight-medium">{{calledService}} takes</div>
    <q-chip v-for="parameter in called[calledService]" :key="parameter.name" dense square clickable :disable="readOnly" @click="addFieldMap(parameter.name)"
        :color="parameter.required === 'true' ? 'amber-3' : undefined" :text-color="parameter.required === 'true' ? 'black' : undefined">+ {{parameter.name}}<q-tooltip>{{parameter.type}}{{parameter.required === 'true' ? ', required' : ''}}</q-tooltip></q-chip>
  </div>
  <pre v-if="generated !== null" class="m-dev-mono m-dev-generated q-pa-sm q-mt-sm" id="DevGeneratedJs">{{generated}}</pre>
  <q-dialog :model-value="!!chipEdit" @update:model-value="v => { if (!v) chipEdit = null; }">
    <q-card v-if="chipEdit" style="width: 30rem; max-width: 95vw" id="DevChipEdit">
      <q-card-section class="text-subtitle2">{{chipEdit.node.name}} {{chipEdit.chip.textual ? 'text' : chipEdit.chip.attr}}</q-card-section>
      <q-card-section class="q-pt-none">
        <q-select v-if="chipEdit.chip.kind === 'entity' || chipEdit.chip.kind === 'service'" dense outlined use-input fill-input hide-selected
            v-model="chipEdit.value" :options="chipEdit.options" @filter="searchNames" @input-value="v => chipEdit.value = v" autofocus></q-select>
        <q-input v-else dense outlined v-model="chipEdit.value" autofocus @keyup.enter="saveChip" :class="chipEdit.chip.kind === 'expr' ? 'm-dev-mono' : ''" for="DevChipValue"></q-input>
      </q-card-section>
      <q-card-actions align="right"><q-btn flat no-caps label="Cancel" @click="chipEdit = null"></q-btn><q-btn unelevated no-caps color="primary" label="Set" @click="saveChip" id="DevChipSet"></q-btn></q-card-actions>
    </q-card>
  </q-dialog>
  <q-dialog :model-value="!!scriptEdit" @update:model-value="v => { if (!v) scriptEdit = null; }">
    <q-card v-if="scriptEdit" style="width: 46rem; max-width: 95vw">
      <q-card-section class="text-subtitle2">Script</q-card-section>
      <q-card-section class="q-pt-none"><m-code-editor v-model="scriptEdit.text" language="javascript" height="18rem"></m-code-editor></q-card-section>
      <q-card-actions align="right"><q-btn flat no-caps label="Cancel" @click="scriptEdit = null"></q-btn><q-btn unelevated no-caps color="primary" label="Set" @click="saveScript"></q-btn></q-card-actions>
    </q-card>
  </q-dialog>
</div>`
    };
    composer.computed.palette = () => ops.PALETTE;

    // ---- the service view ----

    const serviceView = {
        name: "mDevServiceView",
        props: { ctx: { type: Object, required: true }, tab: { type: Object, required: true }, outline: Object },
        data() {
            return { name: "", view: null, error: "", section: "implementation", values: {}, trying: false, result: null, real: false,
                spec: null, specRun: null, snippet: { language: "xml", body: "<actions>\n    <set field=\"result\" from=\"1 + 1\"/>\n</actions>", result: null },
                secas: null, adding: null };
        },
        computed: {
            services() { return ops.servicesOf(this.outline); },
            serviceNode() { const found = this.services.find((entry) => entry.name === this.shortName); return found ? found.node : null; },
            shortName() { return this.name.indexOf(".") >= 0 && this.name.indexOf("#") > 0 ? this.name.slice(this.name.lastIndexOf(".", this.name.indexOf("#")) + 1) : this.name; },
            actionsNode() { return this.serviceNode ? children(this.serviceNode, ["actions"])[0] || null : null; },
            service() { return this.view ? this.view.service : null; },
            inParameters() { return (this.service && this.service.inParameters) || []; },
            canEdit() { return this.ctx.canWrite; },
            storageKey() { return "moqui.devAide.try." + this.name; }
        },
        mounted() {
            this.pick();
            this.ctx.onSaved(() => this.load(this.name));
        },
        watch: { outline() { if (!this.name) this.pick(); } },
        methods: {
            prefixOf() {
                const path = String(this.tab.path || "").replace(/\\/g, "/");
                const at = path.indexOf("service/");
                return at >= 0 ? path.slice(at + 8).replace(/\.xml$/, "").replace(/\//g, ".") : "";
            },
            pick() {
                const asked = String(this.tab.artifact || "").replace(/^service:/, "");
                const first = this.services[0];
                const name = asked || (first ? this.prefixOf() + "." + first.name : "");
                if (name && name !== this.name) this.load(name);
            },
            load(name) {
                this.name = name;
                try { this.values = JSON.parse(root.localStorage.getItem(this.storageKey) || "{}"); } catch (err) { this.values = {}; }
                return this.ctx.get("serviceView", { serviceName: name }).then((answer) => { this.view = answer.view; this.error = ""; },
                    (error) => { this.error = pure.errorText(error); });
            },
            pickService(entry) { this.load(this.prefixOf() + "." + entry); },
            apply(list) { return this.ctx.apply(list).catch((error) => this.ctx.fail(error)); },
            setAttr(name, value) {
                const node = this.serviceNode;
                if (!node) return;
                this.apply(value === "" ? (attrOf(node, name) === undefined ? [] : [{ op: "remove-attribute", at: node.path, name }])
                    : [{ op: "set-attribute", at: node.path, name, value }]);
            },
            startParameter(which) { this.adding = { which, name: "", type: "String", required: false, defaultValue: "", description: "" }; },
            addParameter() {
                const add = this.adding;
                this.adding = null;
                if (add.name && this.serviceNode) this.apply(ops.addParameter(this.serviceNode, add.which, add));
            },
            removeParameter(which, name) {
                const holder = this.serviceNode ? children(this.serviceNode, [which])[0] : null;
                const node = holder ? children(holder, ["parameter"]).find((child) => attrOf(child, "name") === name) : null;
                if (node) this.apply([{ op: "remove", at: node.path }]);
            },
            // ---- SECA rules: Add writes into the site's secas file (D17) ----
            addSeca() {
                const site = this.view && this.view.site;
                if (!site) { this.ctx.note("Create a site component on Home first", "warning"); return; }
                const roots = this.ctx.roots || [];
                const rootItem = roots.find((item) => (item.components || []).some((component) => component.site && component.name === site));
                const component = rootItem && rootItem.components.find((item) => item.name === site);
                if (!component) return;
                const klass = site.split(/[-_]/).filter((part) => part).map((part) => part.charAt(0).toUpperCase() + part.slice(1)).join("");
                const path = (component.path ? component.path + "/" : "") + "service/" + klass + ".secas.xml";
                this.ctx.companion(rootItem.id, path, ops.SECAS_TEXT).then((buffer) => {
                    this.secas = buffer;
                    return this.ctx.apply(ops.addRule(buffer.outline, ops.secaXml(this.name, "post-service")), buffer);
                }).catch((error) => this.ctx.fail(error));
            },
            // ---- Try (9.4) ----
            remember() { try { root.localStorage.setItem(this.storageKey, JSON.stringify(this.values)); } catch (err) { /* no storage */ } },
            runTry(real) {
                if (real) {
                    this.$q.dialog({ title: "Run " + this.name + " for real?", message: "What it writes is kept.", cancel: true, ok: { label: "Run", color: "negative" } })
                        .onOk(() => this.doTry(true));
                } else this.doTry(false);
            },
            doTry(real) {
                this.remember();
                this.trying = true;
                const parameters = ops.tryParameters(this.values, this.inParameters);
                this.ctx.post("tryService", { serviceName: this.name, parameters: JSON.stringify(parameters), real, confirm: real }).then((answer) => {
                    this.trying = false;
                    this.result = Object.assign({ parameters }, answer.result || {});
                    // the last Try, which a rule's condition is tried against (10)
                    try { root.localStorage.setItem("moqui.devAide.tryResult." + this.name, JSON.stringify({ parameters, out: this.result.out || {} })); } catch (err) { /* no storage */ }
                }, (error) => { this.trying = false; this.ctx.fail(error); });
            },
            saveSpec() {
                const result = this.result;
                this.ctx.post("saveTrySpec", { serviceName: this.name, parameters: JSON.stringify(result.parameters || {}),
                    out: JSON.stringify(result.out || {}), rows: JSON.stringify(result.rows || []) }).then((answer) => {
                    this.spec = { rootId: answer.rootId, path: answer.path, name: answer.spec };
                    this.ctx.note("Saved as a feature of " + answer.path);
                }, (error) => this.ctx.fail(error));
            },
            runSpec() {
                const spec = this.spec;
                this.specRun = { running: true };
                this.ctx.post("runSpec", { rootId: spec.rootId, path: spec.path }).then((answer) => {
                    this.specRun = answer;
                    this.load(this.name);
                }, (error) => { this.specRun = null; this.ctx.fail(error); });
            },
            runSnippet() {
                this.snippet.result = { running: true };
                const parameters = ops.tryParameters(this.values, this.inParameters);
                this.ctx.post("trySnippet", { language: this.snippet.language, body: this.snippet.body, parameters: JSON.stringify(parameters) })
                    .then((answer) => { this.snippet.result = answer.result || {}; }, (error) => { this.snippet.result = null; this.ctx.fail(error); });
            },
            openScript() {
                const script = this.view && this.view.script;
                if (script && script.rootId) this.ctx.openArtifact({ rootId: script.rootId, relativePath: script.path, line: 1, name: script.path });
            },
            json(value) { return JSON.stringify(value, null, 2); }
        },
        template: `
<div class="m-dev-service-view q-pa-sm" id="DevServiceView">
  <div class="row items-center q-gutter-x-sm q-mb-xs">
    <q-select v-if="services.length > 1" dense outlined options-dense style="min-width: 16rem" :model-value="shortName"
        :options="services.map(s => s.name)" @update:model-value="pickService" id="DevServicePick"></q-select>
  </div>
  <div v-if="error" class="text-negative">{{error}}</div>
  <template v-if="view && service">
    <div class="row items-center q-gutter-x-sm" id="DevServiceHeader">
      <q-icon name="sym_o_functions" size="22px"></q-icon>
      <span class="text-h6">{{service.verb}}<span class="m-muted">#</span>{{service.noun}}</span>
      <q-chip dense square>{{service.kind}}</q-chip>
      <q-chip v-if="view.artifact" dense square :icon="view.artifact.upstream ? 'sym_o_lock' : 'sym_o_home'">{{view.artifact.component}}</q-chip>
      <span class="text-caption m-muted">used by {{view.usedByCount}}{{view.calls != null ? ' · ' + view.calls + ' calls in the last spec run' : ''}}</span>
    </div>
    <div class="row q-gutter-sm q-my-sm text-caption">
      <label>authenticate <select class="m-dev-cell" :disabled="!canEdit" :value="service.authenticate" @change="setAttr('authenticate', $event.target.value)">
        <option>true</option><option>false</option><option>anonymous-view</option><option>anonymous-all</option></select></label>
      <label>allow-remote <input type="checkbox" :disabled="!canEdit" :checked="serviceNode && pure.attr(serviceNode, 'allow-remote') === 'true'" @change="setAttr('allow-remote', $event.target.checked ? 'true' : '')"></label>
      <label>transaction <select class="m-dev-cell" :disabled="!canEdit" :value="serviceNode ? (pure.attr(serviceNode, 'transaction') || '') : ''" @change="setAttr('transaction', $event.target.value)">
        <option value="">use or begin</option><option>ignore</option><option>force-new</option><option>cache</option><option>force-cache</option></select></label>
      <label>timeout <input class="m-dev-cell" style="width: 4rem" :disabled="!canEdit" :value="serviceNode ? (pure.attr(serviceNode, 'transaction-timeout') || '') : ''" @change="setAttr('transaction-timeout', $event.target.value)"></label>
      <span v-if="service.implements.length">implements {{service.implements.join(', ')}}</span>
    </div>
    <q-tabs dense align="left" no-caps v-model="section" class="m-dev-tabs">
      <q-tab name="implementation" label="Implementation"></q-tab>
      <q-tab name="parameters" :label="'Parameters ' + inParameters.length + ' in, ' + service.outParameters.length + ' out'"></q-tab>
      <q-tab name="try" label="Try" id="DevTryTab"></q-tab>
      <q-tab name="rules" :label="'SECA rules ' + view.secas.length"></q-tab>
    </q-tabs>
    <q-separator class="q-mb-sm"></q-separator>
    <div v-show="section === 'implementation'">
      <template v-if="service.kind === 'inline'">
        <m-dev-composer :ctx="ctx" :actions="actionsNode" :read-only="!canEdit" :in-parameters="inParameters.map(p => p.name)" :apply="apply" :text="tab.text"></m-dev-composer>
      </template>
      <div v-else-if="service.kind === 'script'"><q-btn dense flat no-caps icon="sym_o_javascript" :label="'Open ' + service.location" @click="openScript"></q-btn></div>
      <div v-else-if="service.kind === 'entity-auto'" class="text-caption">Entity-auto: {{service.verb}} of {{service.noun}}.</div>
      <div v-else class="text-caption">{{service.kind}} at {{service.location}}{{service.method ? ' · ' + service.method : ''}}</div>
    </div>
    <div v-show="section === 'parameters'" id="DevServiceParameters">
      <div v-for="which in ['in', 'out']" :key="which" class="q-mb-md">
        <div class="row items-center"><span class="text-subtitle2">{{which === 'in' ? 'In' : 'Out'}}</span><q-space></q-space>
          <q-btn dense flat no-caps size="sm" icon="add" label="Add" :disable="!canEdit" @click="startParameter(which + '-parameters')"></q-btn></div>
        <div class="m-dev-grid-wrap"><table class="m-dev-grid"><thead><tr><th>Name</th><th>Type</th><th>Required</th><th>Default</th><th>Checks</th><th>Description</th><th></th></tr></thead>
          <tbody><template v-for="parameter in (which === 'in' ? inParameters : service.outParameters)" :key="parameter.name">
            <tr :class="{ 'm-muted': parameter.implied }"><td class="m-dev-mono">{{parameter.name}}<span v-if="parameter.implied" class="q-ml-xs text-caption">(auto)</span></td>
              <td>{{parameter.type}}</td><td>{{parameter.required === 'true' ? 'yes' : ''}}</td><td>{{parameter.default || ''}}</td>
              <td class="text-caption">{{parameter.checks.length || ''}}</td><td class="ellipsis" style="max-width: 18rem">{{parameter.description || ''}}</td>
              <td><q-btn v-if="!parameter.implied" dense flat round size="xs" icon="delete" :disable="!canEdit" @click="removeParameter(which + '-parameters', parameter.name)"></q-btn></td></tr>
            <tr v-for="child in parameter.children" :key="parameter.name + '.' + child.name"><td class="m-dev-mono q-pl-lg">{{child.name}}</td><td>{{child.type}}</td><td>{{child.required === 'true' ? 'yes' : ''}}</td><td></td><td></td><td>{{child.description || ''}}</td><td></td></tr>
          </template></tbody></table></div>
      </div>
    </div>
    <div v-show="section === 'try'" id="DevTryPanel">
      <div v-if="!view.sim" class="text-caption text-warning q-mb-xs">This server has no sim (it needs embedded Postgres): only Run for real.</div>
      <div class="row q-col-gutter-sm">
        <div v-for="parameter in inParameters" :key="parameter.name" class="col-12 col-sm-6 col-md-4">
          <q-input dense outlined v-model="values[parameter.name]" :label="parameter.name + (parameter.required === 'true' ? ' *' : '')" :hint="parameter.type" :for="'DevTry_' + parameter.name"></q-input>
        </div>
      </div>
      <div class="row q-gutter-x-sm q-mt-sm">
        <q-btn unelevated no-caps color="primary" icon="sym_o_science" label="Run in sim" :disable="!view.sim" :loading="trying" @click="runTry(false)" id="DevRunInSim"></q-btn>
        <q-btn flat no-caps color="negative" label="Run for real" :disable="!canEdit" @click="runTry(true)"></q-btn>
        <q-space></q-space>
        <q-btn v-if="result && result.sim && !result.failed" flat no-caps icon="sym_o_note_add" label="Save as spec" :disable="!canEdit" @click="saveSpec" id="DevSaveSpec"></q-btn>
        <q-btn v-if="spec" flat no-caps icon="sym_o_play_arrow" :label="'Run spec ' + spec.name" :loading="specRun && specRun.running" @click="runSpec" id="DevRunSpec"></q-btn>
      </div>
      <div v-if="result" class="q-mt-sm" id="DevTryResult">
        <div class="text-caption">{{result.sim ? 'In sim, nothing kept' : 'For real'}} · {{result.millis}} ms</div>
        <div v-if="result.failed" class="text-negative">{{result.failed}}</div>
        <div v-for="(text, i) in (result.messages || [])" :key="'m' + i" class="text-caption">{{text}}</div>
        <div v-for="(text, i) in (result.errors || [])" :key="'e' + i" class="text-caption text-negative">{{text}}</div>
        <pre v-if="result.out" class="m-dev-mono q-ma-none">{{json(result.out)}}</pre>
        <div v-if="(result.rows || []).length" class="q-mt-xs">
          <div class="text-caption text-weight-medium">Rows it would write{{result.moreRows ? ' (the first ones)' : ''}}</div>
          <div class="m-dev-grid-wrap"><table class="m-dev-grid" id="DevTryRows"><thead><tr><th>Entity</th><th>Action</th><th>Key</th><th>Fields</th></tr></thead>
            <tbody><tr v-for="(row, i) in result.rows" :key="i"><td>{{row.entity}}</td><td>{{row.action}}</td><td class="m-dev-mono">{{json(row.key)}}</td>
              <td class="text-caption">{{(row.fields || []).map(f => f.field + (f.before !== undefined ? ': ' + f.before + ' → ' : ': ') + f.after).join('; ')}}</td></tr></tbody></table></div>
        </div>
      </div>
      <q-expansion-item v-if="result && (result.log || []).length" dense dense-toggle :label="'Log ' + result.log.length + (result.moreLog ? ' (and ' + result.moreLog + ' more)' : '')"
          header-class="text-caption" id="DevTryLog"><pre class="m-dev-mono m-dev-generated q-pa-sm q-ma-none">{{result.log.join('\\n')}}</pre></q-expansion-item>
      <div v-if="specRun && !specRun.running" class="q-mt-sm" id="DevSpecRun">
        <div :class="specRun.passed ? 'text-positive' : 'text-negative'">{{specRun.passed ? 'The spec passed (run in sim)' : 'The spec failed'}}</div>
        <div v-if="specRun.setupFailure" class="text-caption text-negative">{{specRun.setupFailure}}</div>
        <div v-for="item in specRun.cases" :key="item.name" class="text-caption">{{item.outcome}} · {{item.name}}<span v-if="item.message" class="text-negative"> · {{item.message}}</span></div>
      </div>
      <q-expansion-item dense dense-toggle label="Snippet" class="q-mt-md" header-class="text-caption">
        <q-btn-toggle dense no-caps flat v-model="snippet.language" :options="[{ label: 'XML Actions', value: 'xml' }, { label: 'JavaScript', value: 'javascript' }]"></q-btn-toggle>
        <m-code-editor v-model="snippet.body" :language="snippet.language === 'xml' ? 'xml' : 'javascript'" height="8rem"></m-code-editor>
        <q-btn dense flat no-caps icon="sym_o_science" label="Run in sim" :disable="!view.sim || !canEdit" @click="runSnippet"></q-btn>
        <div v-if="snippet.result && !snippet.result.running">
          <div v-if="snippet.result.failed" class="text-negative text-caption">{{snippet.result.failed}}</div>
          <div v-for="(text, i) in (snippet.result.messages || [])" :key="i" class="text-caption">{{text}}</div>
          <div v-if="(snippet.result.rows || []).length" class="text-caption">{{snippet.result.rows.length}} rows it would write</div>
        </div>
      </q-expansion-item>
    </div>
    <div v-show="section === 'rules'" id="DevServiceRules">
      <q-list dense separator><q-item v-for="rule in view.secas" :key="rule.file + rule.line" clickable @click="ctx.openArtifact({ rootId: rule.open.rootId, relativePath: rule.open.path, line: rule.line, name: rule.file })">
        <q-item-section><q-item-label>{{rule.when}}{{rule.id ? ' · ' + rule.id : ''}}{{rule.replacedBy ? ' (replaced)' : ''}}</q-item-label>
          <q-item-label caption>{{rule.component}} · {{rule.condition || 'always'}} · {{rule.actions.join(', ')}}</q-item-label></q-item-section></q-item></q-list>
      <q-btn dense flat no-caps icon="add" label="Add a SECA rule in the site" :disable="!ctx.mayWrite" @click="addSeca" class="q-mt-xs" id="DevAddSeca"></q-btn>
      <div v-if="secas" class="text-caption m-muted">{{secas.path}}{{secas.text !== secas.saved ? ' (unsaved)' : ''}}</div>
    </div>
  </template>
  <q-dialog :model-value="!!adding" @update:model-value="v => { if (!v) adding = null; }">
    <q-card v-if="adding" style="width: 28rem"><q-card-section class="text-subtitle1">Add a parameter</q-card-section>
      <q-card-section class="q-pt-none q-gutter-y-sm"><q-input dense outlined v-model="adding.name" label="Name" autofocus></q-input>
        <q-select dense outlined v-model="adding.type" :options="['String', 'Integer', 'Long', 'BigDecimal', 'Boolean', 'Timestamp', 'Date', 'Map', 'List', 'Object']" label="Type"></q-select>
        <q-checkbox dense v-model="adding.required" label="Required"></q-checkbox><q-input dense outlined v-model="adding.defaultValue" label="Default value"></q-input>
        <q-input dense outlined v-model="adding.description" label="Description"></q-input></q-card-section>
      <q-card-actions align="right"><q-btn flat no-caps label="Cancel" @click="adding = null"></q-btn><q-btn unelevated no-caps color="primary" label="Add" @click="addParameter"></q-btn></q-card-actions></q-card>
  </q-dialog>
</div>`
    };
    serviceView.computed.pure = () => pure;

    // ---- the ECA view (10) ----

    const RULES = ["seca", "eeca", "emeca", "giteca"];
    const ecaView = {
        name: "mDevEcaView",
        props: { ctx: { type: Object, required: true }, tab: { type: Object, required: true }, outline: Object },
        data() { return { selected: null, schema: null, tryContext: "", conditionResult: null }; },
        computed: {
            rules() { return children(this.outline, RULES); },
            rule() { return this.selected ? pure.outlineAt(this.outline, this.selected) : this.rules[0] || null; },
            actions() { return this.rule ? children(this.rule, ["actions"])[0] || null : null; },
            conditionNode() { const condition = this.rule ? children(this.rule, ["condition"])[0] : null; return condition ? children(condition)[0] || null : null; },
            spec() { return this.rule && this.schema ? (this.schema.elements || []).find((element) => element.name === this.rule.name) || null : null; },
            canEdit() { return this.ctx.canWrite; }
        },
        mounted() {
            this.ctx.get("schema", { rootElement: this.outline ? this.outline.name : "secas" }).then((answer) => { this.schema = answer.digest || null; }, () => {});
            const line = Number(this.tab.line) || 0;
            const at = line ? this.rules.find((node) => node.line === line) : null;
            if (at) this.selected = at.path;
        },
        methods: {
            labelOf(rule) {
                const what = attrOf(rule, "service") || attrOf(rule, "entity") || attrOf(rule, "event") || "";
                const when = attrOf(rule, "when") || ["on-create", "on-update", "on-delete"].filter((name) => attrOf(rule, name) === "true").map((name) => name.slice(3)).join(", ");
                return rule.name + " " + what + (when ? " · " + when : "") + (attrOf(rule, "id") ? " · " + attrOf(rule, "id") : "");
            },
            apply(list) { return this.ctx.apply(list).catch((error) => this.ctx.fail(error)); },
            setAttr(name, value) {
                const node = this.rule;
                if (!node) return;
                this.apply(value === "" ? (attrOf(node, name) === undefined ? [] : [{ op: "remove-attribute", at: node.path, name }])
                    : [{ op: "set-attribute", at: node.path, name, value }]);
            },
            setCondition(text) {
                const rule = this.rule;
                const condition = children(rule, ["condition"])[0];
                const expression = this.conditionNode;
                if (expression && expression.name === "expression") { this.apply([{ op: "set-text", at: expression.path, text }]); return; }
                const made = "<condition><expression>" + escape(text) + "</expression></condition>";
                if (condition) this.apply([{ op: "replace", at: condition.path, xml: made }]);
                else this.apply([{ op: "insert", at: rule.path, position: "first", xml: made }]);
            },
            addRule() {
                const kind = this.outline.name === "eecas" ? "<eeca entity=\"\" on-create=\"true\"><actions></actions></eeca>"
                    : this.outline.name === "emecas" ? "<emeca><actions></actions></emeca>"
                        : this.outline.name === "gitecas" ? "<giteca event=\"push\"><actions></actions></giteca>" : ops.secaXml("", "post-service");
                this.apply(ops.addRule(this.outline, kind));
            },
            // the condition against a context: a SECA's service's last Try (its parameters and out), or any JSON object (a
            // record of an EECA's entity)
            defaultContext() {
                const service = this.rule ? attrOf(this.rule, "service") : "";
                let last = null;
                try { last = JSON.parse(root.localStorage.getItem("moqui.devAide.tryResult." + service) || "null"); } catch (err) { last = null; }
                return JSON.stringify(last ? Object.assign({}, last.parameters || {}, last.out || {}) : {}, null, 1);
            },
            tryCondition() {
                const node = this.conditionNode;
                const text = this.tab.text || "";
                const condition = !node ? "" : node.name === "expression" ? node.text || "" : text.slice(node.from16, node.to16);
                if (!this.tryContext) this.tryContext = this.defaultContext();
                this.conditionResult = { running: true };
                this.ctx.post("evalCondition", { condition, against: this.tryContext }).then((answer) => {
                    this.conditionResult = { holds: answer.holds, result: answer.result || {} };
                }, (error) => { this.conditionResult = null; this.ctx.fail(error); });
            },
            removeRule() { if (this.rule) { const path = this.rule.path; this.selected = null; this.apply([{ op: "remove", at: path }]); } },
            moveRule(delta) {
                const rule = this.rule;
                if (!rule) return;
                const index = this.rules.indexOf(rule);
                const other = this.rules[index + delta];
                if (other) this.apply([{ op: "move", at: rule.path, to: other.path, position: delta < 0 ? "before" : "after" }]).then(() => { this.selected = null; });
            }
        },
        template: `
<div class="m-dev-eca-view row no-wrap q-pa-sm" id="DevEcaView">
  <div class="m-dev-eca-rules q-pr-sm">
    <div class="row items-center"><span class="text-subtitle2">Rules {{rules.length}}</span><q-space></q-space>
      <q-btn dense flat round size="sm" icon="add" :disable="!canEdit" @click="addRule"><q-tooltip>Add a rule</q-tooltip></q-btn></div>
    <q-list dense><q-item v-for="node in rules" :key="node.path.join('.')" clickable :active="rule === node" @click="selected = node.path">
      <q-item-section><q-item-label class="ellipsis">{{labelOf(node)}}</q-item-label><q-item-label caption>line {{node.line}}</q-item-label></q-item-section></q-item></q-list>
    <div v-if="!rules.length" class="m-muted text-caption">No rules yet.</div>
  </div>
  <div class="col" v-if="rule">
    <div class="row items-center q-gutter-x-xs"><span class="text-subtitle2">{{labelOf(rule)}}</span><q-space></q-space>
      <q-btn dense flat round size="sm" icon="arrow_upward" :disable="!canEdit" @click="moveRule(-1)"></q-btn>
      <q-btn dense flat round size="sm" icon="arrow_downward" :disable="!canEdit" @click="moveRule(1)"></q-btn>
      <q-btn dense flat round size="sm" icon="delete" :disable="!canEdit" @click="removeRule"></q-btn></div>
    <div class="row q-col-gutter-sm q-mb-sm" v-if="spec">
      <div v-for="attribute in spec.attributes" :key="attribute.name" class="col-6 col-md-4">
        <select v-if="attribute.values && attribute.values.length" class="m-dev-cell m-dev-prop" :disabled="!canEdit" :value="pure.attr(rule, attribute.name) || ''" @change="setAttr(attribute.name, $event.target.value)">
          <option value=""></option><option v-for="value in attribute.values" :key="value" :value="value">{{value}}</option></select>
        <input v-else class="m-dev-cell m-dev-prop" :disabled="!canEdit" :value="pure.attr(rule, attribute.name) || ''" :placeholder="attribute.default || ''" @change="setAttr(attribute.name, $event.target.value)">
        <div class="text-caption m-muted" :title="attribute.doc || ''">{{attribute.name}}{{attribute.required ? ' *' : ''}}</div>
      </div>
    </div>
    <q-input dense outlined class="m-dev-mono q-mb-sm" label="Condition (an expression; empty is always)" :readonly="!canEdit"
        :model-value="conditionNode && conditionNode.name === 'expression' ? (conditionNode.text || '') : ''" debounce="800" @update:model-value="setCondition" for="DevEcaCondition"></q-input>
    <q-expansion-item dense dense-toggle label="Try the condition" header-class="text-caption" class="q-mb-sm" @show="tryContext = tryContext || defaultContext()" id="DevTryCondition">
      <q-input dense outlined type="textarea" autogrow v-model="tryContext" label="Against (JSON: the service's last Try, or a record)" class="m-dev-mono"></q-input>
      <q-btn dense flat no-caps icon="sym_o_science" label="Try in sim" @click="tryCondition" id="DevTryConditionRun"></q-btn>
      <span v-if="conditionResult && !conditionResult.running" :class="conditionResult.holds ? 'text-positive' : 'text-warning'" id="DevConditionResult">
        {{conditionResult.holds ? 'It holds: the rule runs' : 'It does not hold: the rule does not run'}}{{conditionResult.result.failed ? ' (' + conditionResult.result.failed + ')' : ''}}</span>
    </q-expansion-item>
    <m-dev-composer :ctx="ctx" :actions="actions" :read-only="!canEdit" :in-parameters="[]" :apply="apply" :text="tab.text"></m-dev-composer>
  </div>
</div>`
    };
    ecaView.computed.pure = () => pure;

    devAide.registerView({ name: "service", label: "Service", roots: ["services"], component: "m-dev-service-view",
        script: "/devstatic/DevAideService.js", ready: () => !!devAide.serviceViewRegistered });
    devAide.registerView({ name: "eca", label: "Rules", roots: ["secas", "eecas", "emecas", "gitecas"], component: "m-dev-eca-view",
        script: "/devstatic/DevAideService.js", ready: () => !!devAide.serviceViewRegistered });
    const register = () => {
        const qvt = moqui.qvt;
        for (const [name, component] of [["m-dev-composer", composer], ["m-dev-service-view", serviceView], ["m-dev-eca-view", ecaView]]) {
            if (qvt && qvt.registerLate) qvt.registerLate(name, component);
            devAide.components[name] = component;
        }
        devAide.serviceViewRegistered = true;
    };
    if (devAide.blocksRegistered) register();
    else if (moqui.loadScript) moqui.loadScript("/devstatic/DevAideBlocks.js", null, () => !!devAide.blocksRegistered).then(register, register);
    else register();
    if (root.document && !root.document.getElementById("m-dev-service-style")) {
        const style = root.document.createElement("style");
        style.id = "m-dev-service-style";
        style.textContent = ".m-dev-generated{max-height:22rem;overflow:auto;background:rgba(127,127,127,.08);border-radius:4px;white-space:pre}" +
            ".m-dev-eca-rules{width:280px;min-width:200px;border-right:1px solid var(--m-border,rgba(0,0,0,.12))}" +
            ".m-dev-grid-wrap{overflow:auto;max-width:100%}.m-dev-grid{border-collapse:collapse;font-size:12px}" +
            ".m-dev-grid th{text-align:left;font-weight:500;padding:2px 6px;white-space:nowrap}.m-dev-grid td{padding:1px 6px;border-top:1px solid var(--m-border,rgba(0,0,0,.08))}" +
            ".m-dev-cell{font:inherit;border:1px solid var(--m-border,rgba(0,0,0,.2));background:transparent;color:inherit;padding:1px 3px;border-radius:3px}.m-dev-prop{width:100%}" +
            "@media (max-width:1023px){.m-dev-eca-view.row{flex-direction:column}.m-dev-eca-rules{width:auto;border:none}}";
        root.document.head.appendChild(style);
    }
})(typeof window !== "undefined" ? window : globalThis);
