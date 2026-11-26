/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The service writer and the ECA view of the Dev AIDE (plans/design-dev-aide.md 9, 10). m-dev-service-view is the
 * visual view of a service file's tab: one service at a time, its header and settings from the schema digest, its
 * parameters (nested, from the buffer, those auto-parameters imply marked; Add, Add child in a Map or a List, Edit and
 * Remove on each declared one by its path of names, with the terminal twin's builders), its implementation (inline
 * actions in the logic composer, a script service's file, an entity-auto's entity), its SECA rules with Add (into the
 * site's secas file, D17), what uses it and the last spec run's calls, and the Try panel (9.4): a form of the in-parameters, Run in sim or for real, the
 * rows the sim would write, Save as spec and Run spec, and a snippet runner. m-dev-composer is the logic composer
 * (9.2): the actions as blocks on the block tree, each a sentence with its main attributes as chips that edit one
 * attribute, the palette by the XSD's groups with snippets, the names known before each block marking an unknown one in
 * an expression, a service-call's in-parameters to add, a script's popup editor, and the generated JavaScript.
 * m-dev-eca-view is the view of an ECA file: its rules, a rule's attributes, its condition and its actions in the
 * composer, Add, Remove and Move, Try the condition against a context (for a gitECA rule, a push of the push log),
 * the contexts saved as named fixtures, and Save as spec and Run spec on a rule. A Try in sim runs over the Workspace's
 * unsaved service and script buffers (F14) and traces the actions, and the composer marks its blocks as run (with the
 * count and the values set on hover), not run, or failed. Every edit is an operation of the lossless XML layer on a
 * buffer.
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
            ["If", "<if condition=\"true\"></if>"], [moqui.l10n.localize("For each"), "<iterate list=\"list\" entry=\"item\"></iterate>"], ["While", "<while condition=\"false\"></while>"],
            ["Message", "<message>Done</message>"], ["Return", "<return/>"], [moqui.l10n.localize("Return error"), "<return error=\"true\" message=\"Not found\"/>"],
            [moqui.l10n.localize("Check errors"), "<check-errors/>"], ["Break", "<break/>"], ["Continue", "<continue/>"]] },
        { group: "Environment", items: [["Set", "<set field=\"name\" from=\"null\"/>"], [moqui.l10n.localize("Filter a list"), "<filter-map-list list=\"list\"/>"],
            [moqui.l10n.localize("Order a list"), "<order-map-list list=\"list\"><order-by field-name=\"name\"/></order-map-list>"]] },
        { group: "Calls", items: [[moqui.l10n.localize("Call a service"), "<service-call name=\"\" out-map=\"result\"/>"], ["Script", "<script><![CDATA[\n]]></script>"],
            ["Log", "<log level=\"info\" message=\"\"/>"]] },
        { group: moqui.l10n.localize("Entity find"), items: [[moqui.l10n.localize("Find one by its key"), "<entity-find-one entity-name=\"\" value-field=\"value\"/>"],
            ["Find", "<entity-find entity-name=\"\" list=\"valueList\"><econdition field-name=\"\" from=\"\"/></entity-find>"],
            ["Count", "<entity-find-count entity-name=\"\" count-field=\"count\"/>"],
            [moqui.l10n.localize("Loop over a find"), "<entity-find entity-name=\"\" list=\"valueList\"/><iterate list=\"valueList\" entry=\"value\"></iterate>"]] },
        { group: moqui.l10n.localize("Entity values"), items: [[moqui.l10n.localize("Create from the parameters"), "<service-call name=\"create#\" in-map=\"context\" out-map=\"context\"/>"],
            [moqui.l10n.localize("Make a value"), "<entity-make-value entity-name=\"\" value-field=\"value\"/>"], ["Create", "<entity-create value-field=\"value\"/>"],
            ["Update", "<entity-update value-field=\"value\"/>"], ["Delete", "<entity-delete value-field=\"value\"/>"],
            [moqui.l10n.localize("Set fields"), "<entity-set value-field=\"value\"/>"]] }
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
    // An attribute set, or removed when empty.
    const setAttributeOps = (node, name, value) => {
        if (!node) return [];
        if (value === "" || value === null || value === undefined || value === false) return attrOf(node, name) === undefined ? [] : [{ op: "remove-attribute", at: node.path, name }];
        return [{ op: "set-attribute", at: node.path, name, value: value === true ? "true" : String(value) }];
    };
    // A parameter of in-parameters or out-parameters by its name, or by its path of names (["order", "items"] or
    // "order.items"), each a parameter of the one before. The same builders as the terminal's twin (GuidedOps.js), so
    // the browser and the terminal make the same edits.
    ops.parameterNode = (service, which, name) => {
        const names = (Array.isArray(name) ? name : String(name == null ? "" : name).split(".")).filter((part) => part !== "");
        let node = names.length ? children(service, [which])[0] || null : null;
        for (const part of names) node = node ? children(node, ["parameter"]).find((child) => attrOf(child, "name") === part) || null : null;
        return node;
    };
    // A parameter added last in in-parameters or out-parameters (made when missing), or with `parent` (a name path, as
    // parameterNode takes it) last inside that parameter, a Map's or a List's: after its parameters, before its
    // validations, as the schema orders them. None when the parent is not there.
    ops.addParameter = (service, which, parameter, parent) => {
        const holder = children(service, [which])[0];
        const text = ops.parameterXml(parameter);
        if (parent && (!Array.isArray(parent) || parent.length)) {
            const node = ops.parameterNode(service, which, parent);
            if (!node) return [];
            const nested = children(node, ["parameter", "auto-parameters"]);
            if (nested.length) return [{ op: "insert", at: nested[nested.length - 1].path, position: "after", xml: text }];
            const validation = children(node).find((child) => child.name !== "description");
            return validation ? [{ op: "insert", at: validation.path, position: "before", xml: text }] : [{ op: "insert", at: node.path, position: "last", xml: text }];
        }
        if (holder) return [{ op: "insert", at: holder.path, position: "last", xml: text }];
        const before = children(service, which === "in-parameters" ? ["out-parameters", "actions"] : ["actions"])[0];
        const made = xml(which, [], text);
        return before ? [{ op: "insert", at: before.path, position: "before", xml: made }] : [{ op: "insert", at: service.path, position: "last", xml: made }];
    };
    ops.removeParameter = (service, which, name) => {
        const node = ops.parameterNode(service, which, name);
        return node ? [{ op: "remove", at: node.path }] : [];
    };
    // Edit a parameter: its type, required and default as attributes, its description as its child.
    ops.editParameter = (service, which, name, parameter) => {
        const node = ops.parameterNode(service, which, name);
        if (!node) return [];
        const list = setAttributeOps(node, "type", parameter.type && parameter.type !== "String" ? parameter.type : "")
            .concat(setAttributeOps(node, "required", parameter.required ? "true" : ""))
            .concat(setAttributeOps(node, "default-value", parameter.defaultValue));
        const description = children(node, ["description"])[0];
        if (description) list.push(parameter.description ? { op: "set-text", at: description.path, text: parameter.description } : { op: "remove", at: description.path });
        // first, before a Map's or List's parameters, as the schema orders them
        else if (parameter.description) list.push({ op: "insert", at: node.path, position: children(node).length ? "first" : "last", xml: "<description>" + escape(parameter.description) + "</description>" });
        return list;
    };
    // The parameter types that hold parameters of their own.
    ops.HOLDER_TYPES = ["Map", "List", "Set", "Collection", "Node"];
    // Why an Add or an Edit of a parameter is refused, or null: an Add of a name its parent (or the top) has already, an
    // Edit that makes a parameter holding others a type that holds none (remove them first). The terminal twin's too.
    ops.parameterRefusal = (service, which, change) => {
        const path = [change.parent, change.name].filter((part) => part).join(".");
        if (change.mode === "add") {
            return ops.parameterNode(service, which, path) ? "There is a parameter " + change.name + (change.parent ? " in " + change.parent : "") + " already" : null;
        }
        const node = ops.parameterNode(service, which, change.path || path);
        const holds = ops.HOLDER_TYPES.indexOf(String(change.type || "String").split(".").pop()) >= 0;
        return node && !holds && children(node, ["parameter", "auto-parameters"]).length
            ? (change.path || path) + " holds parameters: remove them first, or keep a type that holds them" : null;
    };
    // A service's parameters as the view's rows: the buffer's own (its service element's, children flattened with their
    // depth), each with the definition's checks by its path, then those the definition implies (auto-parameters, an
    // interface) after them; the definition's alone when the buffer has no such service. Each row has its parent's path
    // of names (parentPath, dotted, empty at the top), its own (path), and whether it may be edited (not implied) and
    // hold parameters (holder).
    ops.parameterRows = (service, which, defined) => {
        const rows = [];
        const byPath = new Map();
        const index = (list, parents) => {
            for (const parameter of list || []) {
                const path = parents.concat([parameter.name]);
                byPath.set(path.join("."), parameter);
                index(parameter.children, path);
            }
        };
        index(defined, []);
        const push = (parameter, parents, implied) => {
            const path = parents.concat([parameter.name]).join(".");
            const type = parameter.type || "String";
            const known = byPath.get(path);
            rows.push({ which, name: parameter.name, parentPath: parents.join("."), path, depth: parents.length, type,
                required: parameter.required === "true" || parameter.required === true, defaultValue: parameter.defaultValue || parameter.default || "",
                description: parameter.description || "", checks: known && known.checks ? known.checks.length : 0, implied,
                editable: !implied, holder: !implied && ops.HOLDER_TYPES.indexOf(type.split(".").pop()) >= 0 });
        };
        const holder = service ? children(service, [which])[0] : null;
        if (service) {
            const walk = (parent, parents) => {
                for (const child of children(parent, ["parameter"])) {
                    const name = attrOf(child, "name");
                    const description = children(child, ["description"])[0];
                    push({ name, type: attrOf(child, "type"), required: attrOf(child, "required"), defaultValue: attrOf(child, "default-value"),
                        description: description ? description.text : "" }, parents, false);
                    const inside = parents.concat([name]);
                    walk(child, inside);
                    // what the definition adds inside it (its auto-parameters): shown there, not to be edited
                    const declared = new Set(children(child, ["parameter"]).map((nested) => attrOf(nested, "name")));
                    const known = byPath.get(inside.join("."));
                    for (const nested of (known && known.children) || []) if (!declared.has(nested.name)) implied(nested, inside);
                }
            };
            const implied = (parameter, parents) => {
                push(parameter, parents, true);
                for (const child of parameter.children || []) implied(child, parents.concat([parameter.name]));
            };
            if (holder) walk(holder, []);
            const own = new Set(rows.filter((row) => !row.parentPath).map((row) => row.name));
            for (const parameter of defined || []) if (parameter.implied && !own.has(parameter.name)) implied(parameter, []);
            return rows;
        }
        const walk = (parameter, parents, implied) => {
            push(parameter, parents, implied);
            for (const child of parameter.children || []) walk(child, parents.concat([parameter.name]), implied);
        };
        for (const parameter of defined || []) walk(parameter, [], !!parameter.implied);
        return rows;
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
        "xsi:noNamespaceSchemaLocation=\"http://moqui.org/xsd/mr-service-eca.xsd\">\n</secas>\n";
    // What get#PushContexts takes of a gitECA rule: its patterns and event.
    ops.pushQuery = (rule) => ({ repositoryPattern: attrOf(rule, "repository-pattern") || "", refPattern: attrOf(rule, "ref-pattern") || "",
        refType: attrOf(rule, "ref-type") || "", defaultBranchOnly: attrOf(rule, "default-branch-only") === "true", source: attrOf(rule, "source") || "",
        event: attrOf(rule, "event") || "post-receive" });
    // A change of the push log (find#GitUpdates' row) in a line: when, who, its refs and how its rules ran.
    ops.updateLabel = (update) => {
        const refs = (update.refList || []).map((ref) => ref.shortName + (ref.result && ref.result !== "ok" ? " (" + ref.result + ")" : ""));
        const date = update.updateDate;
        const when = typeof date === "number" ? new Date(date).toLocaleString() : String(date || "").replace("T", " ").slice(0, 16);
        return [when, update.username || update.userId || update.source, refs.join(", ") || "no refs", update.ecaStatus].filter((part) => part).join(" · ");
    };

    // ---- pure: a Try over the unsaved buffers, and its trace on the composer's blocks (9.2, 9.4, F14) ----

    // The unsaved buffers a Try in sim runs over: the service files and the scripts, at most 20.
    ops.tryFiles = (buffers) => (buffers || []).filter((buffer) => {
        const path = String(buffer.path || "").replace(/\\/g, "/");
        return /\.js$/.test(path) || (/(^|\/)service\/.+\.xml$/.test(path) && !/\.(secas|emecas|gitecas|rest)\.xml$/.test(path));
    }).slice(0, 20).map((buffer) => ({ rootId: buffer.rootId, path: buffer.path, text: buffer.text }));
    // The block of a Try's trace that is the actions element at `line` of the file `path` of root `rootId`.
    ops.traceBlock = (trace, rootId, path, line) => ((trace && trace.blocks) || []).find((block) =>
        block.rootId === rootId && block.path === path && Number(block.line) === Number(line)) || null;
    // The elements a trace does not count, read by the statement they are in; and the branches of an if.
    const UNTRACED = new Set(["econdition", "econditions", "econdition-object", "date-filter", "having-econditions", "field-map", "order-by",
        "select-field", "search-form-inputs", "default-parameters", "limit-range", "limit-view", "use-iterator", "condition", "compare",
        "expression", "and", "or", "not", "description"]);
    const BRANCHES = new Set(["then", "else", "else-if"]);
    // Each element's mark under `actions` by its path below it ("1.0"): run with its count and the values it set,
    // failed with why, or not run. An element the trace does not count goes as the statement it is in; a branch is run
    // when something in it ran.
    ops.traceMarks = (actions, block) => {
        const marks = new Map();
        if (!actions || !block) return marks;
        const base = actions.path.length;
        const elements = block.elements || {};
        const visit = (node, around) => {
            const key = node.path.slice(base).join(".");
            const entry = elements[key];
            let mark = entry ? { state: entry.failed ? "failed" : "run", count: entry.count, values: entry.values || {}, failed: entry.failed || "" }
                : UNTRACED.has(node.name) ? { state: around === "not-run" ? "not-run" : "run" } : null;
            let inside = false;
            for (const kid of children(node)) {
                const state = visit(kid, mark ? mark.state : "not-run");
                if (state !== "not-run" && !UNTRACED.has(kid.name)) inside = true;
            }
            if (!mark) mark = { state: BRANCHES.has(node.name) && inside ? "run" : "not-run" };
            marks.set(key, mark);
            return mark.state;
        };
        for (const kid of children(actions)) visit(kid, "run");
        return marks;
    };
    // What a mark says on hover.
    ops.traceTitle = (mark) => {
        if (!mark) return "";
        if (mark.state === "not-run") return "Not run in the last Try";
        const lines = [mark.count ? "Ran " + mark.count + (mark.count === 1 ? " time" : " times") : "Ran"];
        for (const [name, value] of Object.entries(mark.values || {})) lines.push(name + " = " + value);
        if (mark.failed) lines.push("Failed: " + mark.failed);
        return lines.join("\n");
    };
    // A block's label with its mark: the run state as its class, the count or "failed" as its badge, the rest on hover.
    ops.markLabel = (label, mark) => {
        if (!mark) return label;
        label.headClass = "m-dev-trace-" + mark.state;
        label.title = ops.traceTitle(mark);
        if (mark.state === "failed") { label.badge = "failed"; label.badgeClass = "m-dev-trace-failed"; }
        else if (mark.count) { label.badge = "×" + mark.count; label.badgeClass = "m-dev-trace-run"; }
        return label;
    };
    // What Save as spec on a rule (10) sends of it: its kind, its service or entity, an EECA rule's operation (its first
    // event), and the condition the Try took.
    ops.ruleSpecQuery = (rule, condition) => ({
        kind: rule.name, target: attrOf(rule, "service") || attrOf(rule, "entity") || "",
        operation: ["create", "update", "delete"].find((event) => attrOf(rule, "on-" + event) === "true") || "",
        condition: condition || ""
    });

    // ---- the composer ----

    const composer = {
        name: "mDevComposer",
        props: { ctx: Object, actions: Object, readOnly: Boolean, inParameters: Array, apply: Function, text: String, trace: Object },
        data() { return { selected: null, chipEdit: null, scriptEdit: null, generated: null, called: {}, paletteFor: null }; },
        computed: {
            known() { return ops.knownBefore(this.actions, this.inParameters); },
            // the last Try's trace of these actions on their blocks
            marks() { return this.trace ? ops.traceMarks(this.actions, this.trace) : null; },
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
                const label = ops.sentence(node, known);
                return this.marks ? ops.markLabel(label, this.marks.get(node.path.slice(this.actions.path.length).join("."))) : label;
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
            // the rows of the Parameters tab, from the buffer (so an unsaved edit shows) with the definition's implied ones
            paramRows() { return { "in-parameters": this.parameterRows("in-parameters"), "out-parameters": this.parameterRows("out-parameters") }; },
            canEdit() { return this.ctx.canWrite; },
            storageKey() { return "moqui.devAide.try." + this.name; },
            // the last Try's trace of this service's actions, for the composer's marks
            composerTrace() {
                const result = this.result;
                if (!result || !result.trace || !this.actionsNode) return null;
                return ops.traceBlock(result.trace, this.ctx.rootId, this.tab.path, this.actionsNode.line);
            },
            failedAt() { return this.result && this.result.trace ? this.result.trace.failed : null; }
        },
        mounted() {
            this.pick();
            this.offSaved = this.ctx.onSaved(() => this.load(this.name));
        },
        beforeUnmount() { if (this.offSaved) this.offSaved(); },
        watch: { outline() { if (!this.name) this.pick(); } },
        methods: {
            t(text) { return moqui.l10n && moqui.l10n.localize ? moqui.l10n.localize(text) : text; },
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
            // the unsaved service and script buffers, which a Try in sim runs over and the view reads (F14)
            overFiles() { return ops.tryFiles(this.ctx.dirtyBuffers ? this.ctx.dirtyBuffers() : []); },
            load(name) {
                this.name = name;
                this.values = devAide.store.get(this.storageKey) || {};
                const files = this.overFiles();
                const asked = files.length ? this.ctx.post("serviceView", { serviceName: name, files: JSON.stringify(files) }) : this.ctx.get("serviceView", { serviceName: name });
                return asked.then((answer) => { this.view = answer.view; this.error = ""; }, (error) => { this.error = pure.errorText(error); });
            },
            pickService(entry) { this.load(this.prefixOf() + "." + entry); },
            apply(list) { return this.ctx.apply(list).catch((error) => this.ctx.fail(error)); },
            setAttr(name, value) {
                const node = this.serviceNode;
                if (!node) return;
                this.apply(value === "" ? (attrOf(node, name) === undefined ? [] : [{ op: "remove-attribute", at: node.path, name }])
                    : [{ op: "set-attribute", at: node.path, name, value }]);
            },
            // Add (at the top, or with `parent` a Map's or List's path inside it) and Edit, in one dialog
            startParameter(which, parent) {
                this.adding = { mode: "add", which, parent: parent || "", name: "", type: "String", required: false, defaultValue: "", description: "" };
            },
            startEdit(row) {
                this.adding = { mode: "edit", which: row.which, parent: row.parentPath, path: row.path, name: row.name, type: row.type,
                    required: row.required, defaultValue: row.defaultValue, description: row.description };
            },
            addParameter() {
                const add = this.adding;
                if (!this.serviceNode) { this.adding = null; return; }
                const refusal = ops.parameterRefusal(this.serviceNode, add.which, add);
                if (refusal) { this.ctx.note(refusal, "warning"); return; }
                if (add.mode === "edit") {
                    this.adding = null;
                    this.apply(ops.editParameter(this.serviceNode, add.which, add.path, add));
                    return;
                }
                if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(add.name || "")) { this.ctx.note(this.t("A parameter name is letters, digits and _"), "warning"); return; }
                this.adding = null;
                const list = ops.addParameter(this.serviceNode, add.which, add, add.parent);
                if (!list.length) { this.ctx.note("No parameter " + add.parent + " to add it in", "warning"); return; }
                this.apply(list);
            },
            removeParameter(row) {
                const list = ops.removeParameter(this.serviceNode, row.which, row.path);
                if (!list.length) return;
                const nested = this.parameterRows(row.which).some((other) => other.parentPath === row.path);
                if (!nested) { this.apply(list); return; }
                this.$q.dialog({ title: "Remove " + row.path + "?", message: this.t("The parameters in it are removed too."), cancel: true,
                    ok: { label: "Remove", color: "negative" } }).onOk(() => this.apply(list));
            },
            parameterRows(which) {
                const defined = which === "in-parameters" ? this.inParameters : (this.service && this.service.outParameters) || [];
                return ops.parameterRows(this.serviceNode, which, defined);
            },
            // ---- SECA rules: Add writes into the site's secas file (D17) ----
            addSeca() {
                const file = this.view && this.view.secasFile;
                if (!file) { this.ctx.note(moqui.l10n.localize("Create a site component on Home first"), "warning"); return; }
                this.ctx.companion(file.rootId, file.path, ops.SECAS_TEXT).then((buffer) => {
                    this.secas = buffer;
                    return this.ctx.apply(ops.addRule(buffer.outline, ops.secaXml(this.name, "post-service")), buffer);
                }).catch((error) => this.ctx.fail(error));
            },
            // ---- Try (9.4) ----
            remember() { devAide.store.set(this.storageKey, this.values); },
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
                const asked = { serviceName: this.name, parameters: JSON.stringify(parameters), real, confirm: real };
                // over the unsaved buffers, and traced: the extend tier's, as a snippet is
                if (!real && this.ctx.tier !== "view") Object.assign(asked, { files: JSON.stringify(this.overFiles()), trace: true });
                this.ctx.post("tryService", asked).then((answer) => {
                    this.trying = false;
                    this.result = Object.assign({ parameters }, answer.result || {});
                    // the last Try, which a rule's condition is tried against (10)
                    devAide.store.set("moqui.devAide.tryResult." + this.name, { parameters, out: this.result.out || {} });
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
    <m-dev-libraries :key="ctx.rootId + tab.path" :ctx="ctx" :root-id="ctx.rootId" :path="tab.path"></m-dev-libraries>
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
      <a class="text-caption" :href="'/qapps/tools/Service/ServiceDetail?serviceName=' + encodeURIComponent(name)" id="DevServiceTools">{{ t('Open in Tools') }}</a>
    </div>
    <q-tabs dense align="left" no-caps v-model="section" class="m-dev-tabs">
      <q-tab name="implementation" label="Implementation"></q-tab>
      <q-tab name="parameters" :label="'Parameters ' + inParameters.length + ' in, ' + service.outParameters.length + ' out'"></q-tab>
      <q-tab name="try" label="Try" id="DevTryTab"></q-tab>
      <q-tab name="rules" :label="'SECA rules ' + view.secas.length"></q-tab>
      <q-tab name="doc" label="Doc" id="DevServiceDocTab"></q-tab>
    </q-tabs>
    <q-separator class="q-mb-sm"></q-separator>
    <div v-show="section === 'implementation'">
      <template v-if="service.kind === 'inline'">
        <m-dev-composer :ctx="ctx" :actions="actionsNode" :read-only="!canEdit" :in-parameters="inParameters.map(p => p.name)" :apply="apply" :text="tab.text" :trace="composerTrace"></m-dev-composer>
      </template>
      <div v-else-if="service.kind === 'script'"><q-btn dense flat no-caps icon="sym_o_javascript" :label="'Open ' + service.location" @click="openScript"></q-btn></div>
      <div v-else-if="service.kind === 'entity-auto'" class="text-caption">Entity-auto: {{service.verb}} of {{service.noun}}.</div>
      <div v-else class="text-caption">{{service.kind}} at {{service.location}}{{service.method ? ' · ' + service.method : ''}}</div>
    </div>
    <div v-show="section === 'parameters'" id="DevServiceParameters">
      <div v-for="which in ['in', 'out']" :key="which" class="q-mb-md">
        <div class="row items-center"><span class="text-subtitle2">{{which === 'in' ? 'In' : 'Out'}}</span><q-space></q-space>
          <q-btn dense flat no-caps size="sm" icon="add" label="Add" :disable="!canEdit" @click="startParameter(which + '-parameters')"></q-btn></div>
        <div class="m-dev-grid-wrap"><table class="m-dev-grid" :id="'DevParameters_' + which"><thead><tr><th>Name</th><th>Type</th><th>Required</th><th>Default</th><th>Checks</th><th>Description</th><th></th></tr></thead>
          <tbody><tr v-for="row in paramRows[which + '-parameters']" :key="row.path" :class="{ 'm-muted': row.implied }" :data-path="row.path">
            <td class="m-dev-mono" :style="{ paddingLeft: (6 + row.depth * 16) + 'px' }">{{row.depth ? '└ ' : ''}}{{row.name}}<span v-if="row.implied" class="q-ml-xs text-caption">(auto)</span></td>
            <td>{{row.type}}</td><td>{{row.required ? 'yes' : ''}}</td><td>{{row.defaultValue}}</td>
            <td class="text-caption">{{row.checks || ''}}</td><td class="ellipsis" style="max-width: 18rem">{{row.description}}</td>
            <td class="text-no-wrap"><template v-if="row.editable">
              <q-btn v-if="row.holder" dense flat round size="xs" icon="sym_o_add_circle" :aria-label="'Add a parameter in ' + row.path" :disable="!canEdit" @click="startParameter(row.which, row.path)" class="m-dev-add-child"><q-tooltip>Add child</q-tooltip></q-btn>
              <q-btn dense flat round size="xs" icon="edit" :aria-label="'Edit ' + row.path" :disable="!canEdit" @click="startEdit(row)" class="m-dev-edit-parameter"><q-tooltip>Edit parameter</q-tooltip></q-btn>
              <q-btn dense flat round size="xs" icon="delete" :aria-label="'Remove ' + row.path" :disable="!canEdit" @click="removeParameter(row)" class="m-dev-remove-parameter"><q-tooltip>Remove parameter</q-tooltip></q-btn>
            </template></td></tr></tbody></table></div>
      </div>
    </div>
    <div v-show="section === 'try'" id="DevTryPanel">
      <div v-if="!view.sim" class="text-caption text-warning q-mb-xs">{{ t('This server has no sim (it needs embedded Postgres): only Run for real.') }}</div>
      <div class="row q-col-gutter-sm">
        <div v-for="parameter in inParameters" :key="parameter.name" class="col-12 col-sm-6 col-md-4">
          <q-input dense outlined v-model="values[parameter.name]" :label="parameter.name + (parameter.required === 'true' ? ' *' : '')" :hint="parameter.type" :for="'DevTry_' + parameter.name"></q-input>
        </div>
      </div>
      <div class="row q-gutter-x-sm q-mt-sm">
        <q-btn unelevated no-caps color="primary" icon="sym_o_science" :label="t('Run in sim')" :disable="!view.sim" :loading="trying" @click="runTry(false)" id="DevRunInSim"></q-btn>
        <q-btn flat no-caps color="negative" :label="t('Run for real')" :disable="!canEdit" @click="runTry(true)"></q-btn>
        <q-space></q-space>
        <q-btn v-if="result && result.sim && !result.failed" flat no-caps icon="sym_o_note_add" :label="t('Save as spec')" :disable="!canEdit" @click="saveSpec" id="DevSaveSpec"></q-btn>
        <q-btn v-if="spec" flat no-caps icon="sym_o_play_arrow" :label="'Run spec ' + spec.name" :loading="specRun && specRun.running" @click="runSpec" id="DevRunSpec"></q-btn>
      </div>
      <div v-if="result" class="q-mt-sm" id="DevTryResult">
        <div class="text-caption">{{result.sim ? 'In sim, nothing kept' : 'For real'}} · {{result.millis}} ms</div>
        <div v-if="result.overlay" class="text-caption m-muted" id="DevTryOverlay">{{ t('Ran as the unsaved text defines it:') }} {{(result.overlay.services || []).concat(result.overlay.scripts || []).join(', ')}}</div>
        <div v-if="result.failed" class="text-negative">{{result.failed}}</div>
        <div v-if="failedAt" class="text-caption text-negative" id="DevTryFailedAt">{{ t('Failed at') }} {{failedAt.path || failedAt.file || ''}}{{failedAt.line ? ':' + failedAt.line : ''}}
          ({{ t('block') }} {{failedAt.element}} {{ t('of its actions') }}): {{failedAt.message}}
          <q-btn dense flat no-caps size="sm" :label="t('Show in the composer')" @click="section = 'implementation'"></q-btn></div>
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
        <q-btn dense flat no-caps icon="sym_o_science" :label="t('Run in sim')" :disable="!view.sim || !canEdit" @click="runSnippet"></q-btn>
        <div v-if="snippet.result && !snippet.result.running">
          <div v-if="snippet.result.failed" class="text-negative text-caption">{{snippet.result.failed}}</div>
          <div v-for="(text, i) in (snippet.result.messages || [])" :key="i" class="text-caption">{{text}}</div>
          <div v-if="(snippet.result.rows || []).length" class="text-caption">{{snippet.result.rows.length}} rows it would write</div>
        </div>
      </q-expansion-item>
    </div>
    <div v-if="section === 'doc'" id="DevServiceDoc">
      <m-dev-artifact-doc :base="ctx.base" :artifact-id="'service:' + name" :stamp="view"></m-dev-artifact-doc>
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
    <q-card v-if="adding" style="width: 28rem" id="DevParameterDialog"><q-card-section class="text-subtitle1">{{adding.mode === 'edit' ? 'Edit ' + adding.path
        : adding.parent ? 'Add a parameter in ' + adding.parent : 'Add a parameter'}}</q-card-section>
      <q-card-section class="q-pt-none q-gutter-y-sm"><q-input v-if="adding.mode !== 'edit'" dense outlined v-model="adding.name" label="Name" autofocus></q-input>
        <q-select dense outlined v-model="adding.type" :options="['String', 'Integer', 'Long', 'BigDecimal', 'Boolean', 'Timestamp', 'Date', 'Map', 'List', 'Object']" label="Type"></q-select>
        <q-checkbox dense v-model="adding.required" label="Required"></q-checkbox><q-input dense outlined v-model="adding.defaultValue" label="Default value"></q-input>
        <q-input dense outlined v-model="adding.description" label="Description"></q-input></q-card-section>
      <q-card-actions align="right"><q-btn flat no-caps label="Cancel" @click="adding = null"></q-btn><q-btn unelevated no-caps color="primary" :label="adding.mode === 'edit' ? 'Set' : 'Add'" @click="addParameter" id="DevParameterSave"></q-btn></q-card-actions></q-card>
  </q-dialog>
</div>`
    };
    serviceView.computed.pure = () => pure;

    // ---- the ECA view (10) ----

    const RULES = ["seca", "eeca", "emeca", "giteca"];
    const ecaView = {
        name: "mDevEcaView",
        props: { ctx: { type: Object, required: true }, tab: { type: Object, required: true }, outline: Object },
        data() {
            return { selected: null, schema: null, tryContext: "", conditionResult: null, fixtures: [], fixtureName: "", ruleSpec: null, ruleSpecRun: null,
                ruleRun: null,
                push: { repositories: [], repositoryId: "", updates: [], updateId: "", contexts: [], loading: false, loaded: false } };
        },
        computed: {
            rules() { return children(this.outline, RULES); },
            gitRule() { return !!this.rule && this.rule.name === "giteca"; },
            rule() { return this.selected ? pure.outlineAt(this.outline, this.selected) : this.rules[0] || null; },
            actions() { return this.rule ? children(this.rule, ["actions"])[0] || null : null; },
            conditionNode() { const condition = this.rule ? children(this.rule, ["condition"])[0] : null; return condition ? children(condition)[0] || null : null; },
            spec() { return this.rule && this.schema ? (this.schema.elements || []).find((element) => element.name === this.rule.name) || null : null; },
            canEdit() { return this.ctx.canWrite; },
            // the last run of the rule's trigger, traced, on its actions' blocks
            ruleTrace() {
                const run = this.ruleRun && this.ruleRun.result;
                return run && run.trace && this.actions ? ops.traceBlock(run.trace, this.ctx.rootId, this.tab.path, this.actions.line) : null;
            }
        },
        watch: {
            // another rule: its own pushes, by its patterns and event
            selected() {
                this.push = { repositories: [], repositoryId: "", updates: [], updateId: "", contexts: [], loading: false, loaded: false };
                this.ruleSpec = null; this.ruleSpecRun = null; this.ruleRun = null;
            }
        },
        mounted() {
            this.ctx.schema(this.outline ? this.outline.name : "secas").then((digest) => { this.schema = digest; });
            const line = Number(this.tab.line) || 0;
            const at = line ? this.rules.find((node) => node.line === line) : null;
            if (at) this.selected = at.path;
            this.loadFixtures();
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
                        : this.outline.name === "gitecas" ? "<giteca event=\"post-receive\"><actions></actions></giteca>" : ops.secaXml("", "post-service");
                this.apply(ops.addRule(this.outline, kind));
            },
            // the condition against a context: a SECA's service's last Try (its parameters and out), for a gitECA rule a
            // push of the push log (loadPushes), or any JSON object (a record of an EECA's entity)
            defaultContext() {
                const service = this.rule ? attrOf(this.rule, "service") : "";
                const last = devAide.store.get("moqui.devAide.tryResult." + service);
                return JSON.stringify(last ? Object.assign({}, last.parameters || {}, last.out || {}) : {}, null, 1);
            },
            conditionText() {
                const node = this.conditionNode;
                const text = this.tab.text || "";
                return !node ? "" : node.name === "expression" ? node.text || "" : text.slice(node.from16, node.to16);
            },
            // ---- the contexts as named fixtures, and Save as spec on a rule (10) ----
            loadFixtures() {
                return this.ctx.get("ruleFixtures", { rootId: this.ctx.rootId, path: this.tab.path }).then((answer) => { this.fixtures = answer.fixtures || []; },
                    () => { this.fixtures = []; });
            },
            useFixture(name) {
                const fixture = this.fixtures.find((item) => item.name === name);
                if (!fixture || !fixture.context) return;
                this.fixtureName = fixture.name;
                this.tryContext = JSON.stringify(fixture.context, null, 1);
            },
            saveFixture() {
                this.ctx.post("ruleFixture", { rootId: this.ctx.rootId, path: this.tab.path, name: this.fixtureName, context: this.tryContext || "{}" }).then((answer) => {
                    this.ctx.note("Saved the fixture " + ((answer.fixture && answer.fixture.path) || this.fixtureName));
                    this.loadFixtures();
                }, (error) => this.ctx.fail(error));
            },
            saveRuleSpec() {
                if (pure.isDirty(this.tab)) { this.ctx.note(this.t("Save the rules file first: Save as spec runs the rules the server has"), "warning"); return; }
                const asked = Object.assign(ops.ruleSpecQuery(this.rule, this.conditionText()),
                    { rootId: this.ctx.rootId, path: this.tab.path, fixture: this.fixtureName, context: this.tryContext || "{}" });
                this.ruleSpecRun = null;
                this.ctx.post("ruleSpec", asked).then((answer) => {
                    this.ruleSpec = { rootId: answer.rootId, path: answer.path, name: answer.spec, note: answer.note || "" };
                    this.ctx.note("Saved as a feature of " + answer.path);
                    this.loadFixtures();
                }, (error) => this.ctx.fail(error));
            },
            // the rule's trigger in the sim with the context, traced: its messages and rows, and its actions marked
            runTrigger() {
                if (pure.isDirty(this.tab)) { this.ctx.note(this.t("Save the rules file first: the trigger runs the rules the server has"), "warning"); return; }
                this.ruleRun = { running: true };
                this.ctx.post("tryRule", Object.assign(ops.ruleSpecQuery(this.rule, this.conditionText()), { context: this.tryContext || "{}" }))
                    .then((answer) => { this.ruleRun = { result: answer.result || {}, note: answer.note || "" }; },
                        (error) => { this.ruleRun = null; this.ctx.fail(error); });
            },
            runRuleSpec() {
                const spec = this.ruleSpec;
                this.ruleSpecRun = { running: true };
                this.ctx.post("runSpec", { rootId: spec.rootId, path: spec.path }).then((answer) => { this.ruleSpecRun = answer; },
                    (error) => { this.ruleSpecRun = null; this.ctx.fail(error); });
            },
            tryCondition() {
                const condition = this.conditionText();
                if (!this.tryContext) this.tryContext = this.defaultContext();
                this.conditionResult = { running: true };
                this.ctx.post("evalCondition", { condition, against: this.tryContext }).then((answer) => {
                    this.conditionResult = { holds: answer.holds, result: answer.result || {} };
                }, (error) => { this.conditionResult = null; this.ctx.fail(error); });
            },
            t(text) { return moqui.l10n && moqui.l10n.localize ? moqui.l10n.localize(text) : text; },
            updateLabel(update) { return ops.updateLabel(update); },
            // a gitECA rule against a push of the push log (10): the repositories its pattern takes, a change, then the
            // contexts the rule's event had from it, one of which becomes the Try's context
            loadPushes(repositoryId, updateId) {
                const push = this.push;
                push.loading = true;
                const query = Object.assign(ops.pushQuery(this.rule), { repositoryId: repositoryId || "", updateId: updateId || "" });
                return this.ctx.get("pushContexts", query).then((answer) => {
                    push.loading = false; push.loaded = true;
                    push.repositories = answer.repositories || [];
                    push.repositoryId = answer.repositoryId || "";
                    push.updates = answer.updates || [];
                    push.updateId = updateId || "";
                    push.contexts = answer.contexts || [];
                    push.truncated = !!answer.truncated;
                    if (push.contexts.length) this.useContext(push.contexts.find((context) => context.matches) || push.contexts[0]);
                }, (error) => { push.loading = false; this.ctx.fail(error); });
            },
            useContext(context) { if (context) this.tryContext = JSON.stringify(context.context || {}, null, 1); },
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
      <q-btn dense flat round size="sm" icon="add" aria-label="Add a rule" :disable="!canEdit" @click="addRule"><q-tooltip>Add a rule</q-tooltip></q-btn></div>
    <q-list dense><q-item v-for="node in rules" :key="node.path.join('.')" clickable :active="rule === node" @click="selected = node.path">
      <q-item-section><q-item-label class="ellipsis">{{labelOf(node)}}</q-item-label><q-item-label caption>line {{node.line}}</q-item-label></q-item-section></q-item></q-list>
    <div v-if="!rules.length" class="m-muted text-caption">No rules yet.</div>
  </div>
  <div class="col" v-if="rule">
    <div class="row items-center q-gutter-x-xs"><span class="text-subtitle2">{{labelOf(rule)}}</span><q-space></q-space>
      <q-btn dense flat round size="sm" icon="arrow_upward" aria-label="Move rule up" :disable="!canEdit" @click="moveRule(-1)"><q-tooltip>Move up</q-tooltip></q-btn>
      <q-btn dense flat round size="sm" icon="arrow_downward" aria-label="Move rule down" :disable="!canEdit" @click="moveRule(1)"><q-tooltip>Move down</q-tooltip></q-btn>
      <q-btn dense flat round size="sm" icon="delete" aria-label="Remove rule" :disable="!canEdit" @click="removeRule"><q-tooltip>Remove rule</q-tooltip></q-btn></div>
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
      <div v-if="gitRule" class="q-mb-xs" id="DevTryPush">
        <div class="row items-center q-gutter-x-sm no-wrap">
          <q-btn v-if="!push.loaded" dense flat no-caps icon="sym_o_history" :label="t('Against a push')" :loading="push.loading" @click="loadPushes()" id="DevTryPushLoad"></q-btn>
          <template v-else>
            <q-select dense outlined options-dense style="min-width: 12rem" :model-value="push.repositoryId" :options="push.repositories" label="Repository"
                @update:model-value="id => loadPushes(id)" id="DevTryPushRepository"></q-select>
            <q-select dense outlined options-dense class="col" :model-value="push.updateId || null" :options="push.updates.map(u => ({ label: updateLabel(u), value: u.updateId }))"
                emit-value map-options label="A change of the push log" @update:model-value="id => loadPushes(push.repositoryId, id)" id="DevTryPushUpdate"></q-select>
            <q-spinner v-if="push.loading" size="1em"></q-spinner>
          </template>
        </div>
        <div v-if="push.loaded && !push.repositories.length" class="text-caption m-muted">{{ t('No hosted repository you may see matches the rule.') }}</div>
        <div v-if="push.contexts.length" class="row q-gutter-xs q-mt-xs" id="DevTryPushContexts">
          <q-chip v-for="(context, i) in push.contexts" :key="i" dense square clickable @click="useContext(context)" :outline="!context.matches"
              :title="context.matches ? '' : t('The rule does not run on this one (its source, ref-pattern, ref-type or default branch)')">{{context.label}}</q-chip>
        </div>
        <div v-if="push.truncated" class="text-caption m-muted" id="DevTryPushTruncated">{{ t('Some were left out: past the limits, or commits you do not read') }}</div>
        <div v-else-if="push.updateId && !push.loading && !push.contexts.length" class="text-caption m-muted">{{ t('That change has no context for this rule\\'s event.') }}</div>
      </div>
      <q-input dense outlined type="textarea" autogrow v-model="tryContext" :label="gitRule ? t('Against (JSON: a push, or any context)') : t('Against (JSON: the service\\'s last Try, or a record)')" class="m-dev-mono"></q-input>
      <q-btn dense flat no-caps icon="sym_o_science" label="Try in sim" @click="tryCondition" id="DevTryConditionRun"></q-btn>
      <span v-if="conditionResult && !conditionResult.running" :class="conditionResult.holds ? 'text-positive' : 'text-warning'" id="DevConditionResult">
        {{conditionResult.holds ? 'It holds: the rule runs' : 'It does not hold: the rule does not run'}}{{conditionResult.result.failed ? ' (' + conditionResult.result.failed + ')' : ''}}</span>
      <div class="row items-center q-gutter-x-sm q-mt-xs" id="DevRuleFixtures">
        <q-select dense outlined options-dense style="min-width: 10rem" :model-value="null" :options="fixtures.filter(f => f.context).map(f => f.name)"
            :label="t('A saved fixture')" @update:model-value="useFixture" id="DevRuleFixturePick"></q-select>
        <q-input dense outlined v-model="fixtureName" :label="t('Fixture name')" style="width: 11rem" for="DevRuleFixtureName"></q-input>
        <q-btn dense flat no-caps icon="sym_o_save" :label="t('Save as fixture')" :disable="!canEdit || !fixtureName" @click="saveFixture" id="DevRuleFixtureSave"></q-btn>
        <q-btn dense flat no-caps icon="sym_o_play_circle" :label="t('Run the trigger in sim')" :disable="!canEdit || rule.name === 'emeca'" :loading="ruleRun && ruleRun.running"
            @click="runTrigger" id="DevRuleRunTrigger"><q-tooltip>{{ t('Runs the rule\'s trigger with the context in the sim and marks its actions') }}</q-tooltip></q-btn>
        <q-btn dense flat no-caps icon="sym_o_note_add" :label="t('Save as spec')" :disable="!canEdit || !fixtureName || rule.name === 'emeca'" @click="saveRuleSpec" id="DevRuleSaveSpec">
          <q-tooltip>{{ t('Runs the rule\'s trigger with the fixture in the sim, and writes a feature that runs it again and checks what it did') }}</q-tooltip></q-btn>
        <q-btn v-if="ruleSpec" dense flat no-caps icon="sym_o_play_arrow" :label="'Run spec ' + ruleSpec.name" :loading="ruleSpecRun && ruleSpecRun.running" @click="runRuleSpec" id="DevRuleRunSpec"></q-btn>
      </div>
      <div v-if="ruleRun && ruleRun.result" class="text-caption q-mt-xs" id="DevRuleRun">
        <span v-if="ruleRun.result.failed" class="text-negative">{{ruleRun.result.failed}}</span>
        <span v-else>{{ t('In sim, nothing kept') }} · {{(ruleRun.result.messages || []).length}} {{ t('messages') }} · {{(ruleRun.result.rows || []).length}} {{ t('rows') }}</span>
        <div v-for="(text, i) in (ruleRun.result.messages || [])" :key="i">{{text}}</div>
        <div v-if="ruleRun.note" class="m-muted">{{ruleRun.note}}</div>
      </div>
      <div v-if="ruleSpec && ruleSpec.note" class="text-caption m-muted" id="DevRuleSpecNote">{{ruleSpec.note}}</div>
      <div v-if="ruleSpecRun && !ruleSpecRun.running" class="q-mt-xs" id="DevRuleSpecRun">
        <div :class="ruleSpecRun.passed ? 'text-positive' : 'text-negative'">{{ruleSpecRun.passed ? 'The spec passed (run in sim)' : 'The spec failed'}}</div>
        <div v-if="ruleSpecRun.setupFailure" class="text-caption text-negative">{{ruleSpecRun.setupFailure}}</div>
        <div v-for="item in ruleSpecRun.cases" :key="item.name" class="text-caption">{{item.outcome}} · {{item.name}}<span v-if="item.message" class="text-negative"> · {{item.message}}</span></div>
      </div>
    </q-expansion-item>
    <m-dev-composer :ctx="ctx" :actions="actions" :read-only="!canEdit" :in-parameters="[]" :apply="apply" :text="tab.text" :trace="ruleTrace"></m-dev-composer>
  </div>
</div>`
    };
    ecaView.computed.pure = () => pure;

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
