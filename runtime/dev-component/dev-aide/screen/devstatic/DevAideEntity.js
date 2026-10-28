/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The entity view of the Dev AIDE (plans/design-dev-aide.md 8): m-dev-entity-view, a visual view of an entity file's
 * tab. One entity at a time, as the server merged it (get#EntityView): its header with the origin (the defining
 * component, each component whose extend-entity adds to it), the fields, the relationships, the incoming ones, the
 * indexes, the EECA rules, the table against the fields and the first rows; for a view entity its members, aliases,
 * alias-alls and order. Every edit is an operation of the lossless XML layer on a buffer: the tab's own when it defines
 * the entity (a site's entity, or an upstream one with "Allow editing upstream components" on) or holds the site's
 * extend-entity, else the site component's entity file (D17), opened as a companion buffer and made on its first save.
 * Add field asks for a kind first; Picklist and Status write the field, its relationship and the seed rows (the site's
 * seed data file, loaded after the save) as one Save. The seed data rows of a data file show as a table (12).
 *
 * The operation builders are moqui.devAide.entityOps, pure, which test/client/pages/DevAideEntity.page.js runs.
 */
(function (root) {
    "use strict";

    const moqui = root.moqui;
    const devAide = moqui.devAide;
    const pure = devAide.pure;
    const ops = devAide.entityOps = {};

    // ---- pure: XML text and operations ----

    ops.escape = (value) => String(value == null ? "" : value).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/"/g, "&quot;");
    ops.escapeText = (value) => String(value == null ? "" : value).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
    // An element as text: attributes in the order given, those empty or false left out; `body` is its content as XML.
    ops.xml = (name, attrs, body) => {
        let out = "<" + name;
        for (const [key, value] of attrs || []) {
            if (value === undefined || value === null || value === "" || value === false) continue;
            out += " " + key + "=\"" + ops.escape(value === true ? "true" : value) + "\"";
        }
        return body ? out + ">" + body + "</" + name + ">" : out + "/>";
    };
    const attrOf = pure.attr;
    const children = (node, names) => ((node && node.children) || []).filter((child) => !names || names.indexOf(child.name) >= 0);
    // The short name of an entity name (moqui.example.Example -> Example).
    ops.shortName = (name) => String(name || "").split(".").pop();

    // The entity, view-entity and extend-entity elements of an entities file.
    ops.entityElements = (outline) => children(outline, ["entity", "view-entity", "extend-entity"]).map((node) => ({
        node, kind: node.name, entityName: attrOf(node, "entity-name") || "", package: attrOf(node, "package") || ""
    }));
    // The element of `fullName` in an entities file: its definition, or an extend-entity of it.
    ops.findEntityElement = (outline, fullName, kinds) => {
        const short = ops.shortName(fullName);
        const pkg = String(fullName || "").indexOf(".") > 0 ? String(fullName).slice(0, String(fullName).lastIndexOf(".")) : "";
        return ops.entityElements(outline).find((entry) => (!kinds || kinds.indexOf(entry.kind) >= 0) &&
            (entry.entityName === fullName || (entry.entityName === short && (!entry.package || !pkg || entry.package === pkg)))) || null;
    };
    ops.childNamed = (node, name, key, value) => children(node, [name]).find((child) => attrOf(child, key || "name") === value) || null;
    const last = (list) => list.length ? list[list.length - 1] : null;

    // The operations that make the extend-entity of `fullName` in a site entities file when it has none: answers the
    // path the element has after them.
    ops.ensureExtend = (outline, fullName) => {
        const found = ops.findEntityElement(outline, fullName, ["extend-entity"]);
        if (found) return { path: found.node.path, ops: [] };
        const short = ops.shortName(fullName);
        const pkg = String(fullName).indexOf(".") > 0 ? String(fullName).slice(0, String(fullName).lastIndexOf(".")) : "";
        const count = children(outline).length;
        return {
            path: [count],
            ops: [{ op: "insert", at: outline.path || [], position: "last", xml: ops.xml("extend-entity", [["entity-name", short], ["package", pkg]]) }]
        };
    };

    // A field as XML from its form: name, type, primary key, not null, default, encrypt, audit log, localization,
    // create only, and the description as a child.
    ops.fieldXml = (field) => ops.xml("field", [["name", field.name], ["type", field.type], ["is-pk", !!field.isPk],
        ["not-null", !!field.notNull], ["default", field.defaultValue], ["encrypt", !!field.encrypt],
        ["enable-audit-log", field.enableAuditLog], ["enable-localization", !!field.enableLocalization], ["create-only", !!field.createOnly]],
    field.description ? "<description>" + ops.escapeText(field.description) + "</description>" : "");

    // Insert `xml` among an element's children of `names`: after the child `afterName` (by its name attribute), else
    // after the last of them, else before the first other child, else as the element's last child.
    const insertAmong = (element, names, xml, afterName) => {
        const among = children(element, names);
        const after = afterName ? among.find((child) => attrOf(child, "name") === afterName) : null;
        const anchor = after || last(among);
        if (anchor) return [{ op: "insert", at: anchor.path, position: "after", xml }];
        const first = children(element)[0];
        if (first) return [{ op: "insert", at: first.path, position: "before", xml }];
        return [{ op: "insert", at: element.path, position: "last", xml }];
    };
    ops.addField = (element, field, afterName) => insertAmong(element, ["field"], ops.fieldXml(field), afterName);
    // An attribute of a field: set on the field element when the element has it, else an override field of that name
    // with the attribute (an extend-entity's field merges into the entity's by name).
    ops.setFieldAttribute = (element, fieldName, name, value) => {
        const field = ops.childNamed(element, "field", "name", fieldName);
        if (field) {
            if (value === "" || value === false || value === null || value === undefined) {
                return attrOf(field, name) === undefined ? [] : [{ op: "remove-attribute", at: field.path, name }];
            }
            return [{ op: "set-attribute", at: field.path, name, value: value === true ? "true" : String(value) }];
        }
        if (value === "" || value === false || value == null) return [];
        return insertAmong(element, ["field"], ops.xml("field", [["name", fieldName], [name, value === true ? "true" : String(value)]]));
    };
    // A field's description: its description child's text, made when missing.
    ops.setFieldDescription = (element, fieldName, text) => {
        const field = ops.childNamed(element, "field", "name", fieldName);
        if (!field) return insertAmong(element, ["field"], ops.xml("field", [["name", fieldName]], "<description>" + ops.escapeText(text) + "</description>"));
        const description = children(field, ["description"])[0];
        if (description) return text ? [{ op: "set-text", at: description.path, text }] : [{ op: "remove", at: description.path }];
        return text ? [{ op: "insert", at: field.path, position: "last", xml: "<description>" + ops.escapeText(text) + "</description>" }] : [];
    };
    ops.removeField = (element, fieldName) => {
        const field = ops.childNamed(element, "field", "name", fieldName);
        return field ? [{ op: "remove", at: field.path }] : [];
    };
    // Move field `fieldName` before or after `otherName` (both of this element).
    ops.moveField = (element, fieldName, otherName, position) => {
        const field = ops.childNamed(element, "field", "name", fieldName);
        const other = ops.childNamed(element, "field", "name", otherName);
        if (!field || !other || field === other) return [];
        return [{ op: "move", at: field.path, to: other.path, position: position === "before" ? "before" : "after" }];
    };

    // A relationship as XML: type, related, title, short alias, fk name, and its key maps.
    ops.relationshipXml = (rel) => ops.xml("relationship", [["type", rel.type || "one"], ["title", rel.title], ["related", rel.related],
        ["short-alias", rel.shortAlias], ["fk-name", rel.fkName]],
    (rel.keyMaps || []).filter((key) => key.fieldName).map((key) => ops.xml("key-map", [["field-name", key.fieldName], ["related", key.related]])).join(""));
    // After the last relationship, else last in the element: so a field and its relationship made in one list of
    // operations land in that order.
    ops.insertRelationship = (element, xml) => {
        const rels = children(element, ["relationship"]);
        return rels.length ? [{ op: "insert", at: rels[rels.length - 1].path, position: "after", xml }]
            : [{ op: "insert", at: element.path, position: "last", xml }];
    };
    ops.addRelationship = (element, rel) => ops.insertRelationship(element, ops.relationshipXml(rel));
    ops.removeRelationship = (element, related, title) => {
        const found = children(element, ["relationship"]).find((child) => attrOf(child, "related") === related &&
            (attrOf(child, "title") || "") === (title || ""));
        return found ? [{ op: "remove", at: found.path }] : [];
    };
    ops.indexXml = (index) => ops.xml("index", [["name", index.name], ["unique", !!index.unique]],
        (index.fields || []).filter((name) => name).map((name) => ops.xml("index-field", [["name", name]])).join(""));
    ops.addIndex = (element, index) => insertAmong(element, ["index"], ops.indexXml(index));

    // A Picklist field (8.1): the ...EnumId field and its relationship to Enumeration titled by the type, and the seed
    // records: the EnumerationType and an Enumeration per value.
    ops.picklist = (spec) => {
        const typeId = spec.enumTypeId;
        const fieldName = spec.fieldName;
        const field = ops.fieldXml({ name: fieldName, type: "id", description: spec.description });
        const relationship = ops.relationshipXml({ type: "one", title: spec.title || typeId, related: "moqui.basic.Enumeration",
            keyMaps: [{ fieldName, related: "enumId" }] });
        const seed = [];
        if (!spec.existingType) seed.push(ops.xml("moqui.basic.EnumerationType", [["enumTypeId", typeId], ["description", spec.typeDescription || spec.title || typeId]]));
        (spec.values || []).forEach((value, index) => seed.push(ops.xml("moqui.basic.Enumeration", [["enumId", value.id], ["enumTypeId", typeId],
            ["sequenceNum", String((index + 1) * 10)], ["description", value.description || value.id]])));
        return { field, relationship, seed };
    };
    // A Status field (8.1): statusId and its relationship to StatusItem, and the seed records: the StatusType, a
    // StatusItem per status, the StatusFlow of the type and a transition from each status to the next.
    ops.status = (spec) => {
        const typeId = spec.statusTypeId;
        const fieldName = spec.fieldName || "statusId";
        const field = ops.fieldXml({ name: fieldName, type: "id", description: spec.description });
        const relationship = ops.relationshipXml({ type: "one", title: spec.title, related: "moqui.basic.StatusItem",
            keyMaps: fieldName === "statusId" ? [] : [{ fieldName, related: "statusId" }] });
        const items = spec.items || [];
        const seed = [ops.xml("moqui.basic.StatusType", [["statusTypeId", typeId], ["description", spec.typeDescription || typeId]])];
        items.forEach((item, index) => seed.push(ops.xml("moqui.basic.StatusItem", [["statusId", item.id], ["statusTypeId", typeId],
            ["sequenceNum", String((index + 1) * 10)], ["description", item.description || item.id]])));
        if (spec.flow !== false && items.length > 1) {
            seed.push(ops.xml("moqui.basic.StatusFlow", [["statusFlowId", typeId], ["statusTypeId", typeId], ["description", (spec.typeDescription || typeId) + " flow"]]));
            for (let index = 0; index + 1 < items.length; index++) {
                seed.push(ops.xml("moqui.basic.StatusFlowTransition", [["statusFlowId", typeId], ["statusId", items[index].id],
                    ["toStatusId", items[index + 1].id], ["transitionName", items[index + 1].description || items[index + 1].id]]));
            }
        }
        return { field, relationship, seed };
    };
    // The seed records as the last children of a data file's root.
    ops.insertRecords = (outline, records) => (records || []).map((xml) => ({ op: "insert", at: (outline && outline.path) || [], position: "last", xml }));

    // A data file's records as tables (12): each entity's records with the union of their attributes as columns.
    ops.recordTables = (outline) => {
        const tables = [];
        const byName = {};
        for (const node of children(outline)) {
            let table = byName[node.name];
            if (!table) { table = byName[node.name] = { entity: node.name, columns: [], rows: [] }; tables.push(table); }
            for (const [key] of node.attrs || []) if (table.columns.indexOf(key) < 0) table.columns.push(key);
            table.rows.push({ path: node.path, values: Object.fromEntries(node.attrs || []), nested: (node.children || []).length });
        }
        return tables;
    };
    // A record cell: set, or removed when empty.
    ops.setRecordValue = (row, name, value) => (value === "" || value == null)
        ? (row.values[name] === undefined ? [] : [{ op: "remove-attribute", at: row.path, name }])
        : [{ op: "set-attribute", at: row.path, name, value: String(value) }];
    ops.addRecord = (outline, table, values) => {
        const rows = table.rows;
        const xml = ops.xml(table.entity, table.columns.map((name) => [name, values ? values[name] : ""]));
        return rows.length ? [{ op: "insert", at: rows[rows.length - 1].path, position: "after", xml }]
            : [{ op: "insert", at: (outline && outline.path) || [], position: "last", xml }];
    };

    // A view entity's parts as XML (8.3).
    ops.memberXml = (member) => member.relationship
        ? ops.xml("member-relationship", [["entity-alias", member.entityAlias], ["join-from-alias", member.joinFromAlias],
            ["relationship", member.relationship], ["join-optional", !!member.joinOptional]])
        : ops.xml("member-entity", [["entity-alias", member.entityAlias], ["entity-name", member.entityName],
            ["join-from-alias", member.joinFromAlias], ["join-optional", !!member.joinOptional]],
        (member.keyMaps || []).filter((key) => key.fieldName).map((key) => ops.xml("key-map", [["field-name", key.fieldName], ["related", key.related]])).join(""));
    ops.addMember = (element, member) => insertAmong(element, ["member-entity", "member-relationship"], ops.memberXml(member));
    ops.aliasAllXml = (all) => ops.xml("alias-all", [["entity-alias", all.entityAlias], ["prefix", all.prefix]],
        (all.exclude || []).filter((name) => name).map((name) => ops.xml("exclude", [["field", name]])).join(""));
    ops.addAliasAll = (element, all) => insertAmong(element, ["alias-all"], ops.aliasAllXml(all));
    ops.aliasXml = (alias) => ops.xml("alias", [["name", alias.name], ["entity-alias", alias.entityAlias], ["field", alias.field], ["function", alias.function]]);
    ops.addAlias = (element, alias) => insertAmong(element, ["alias", "alias-all"], ops.aliasXml(alias));
    // A field element of an outline as the entity detail names a field's parts.
    // An attribute not written is undefined, so an override field leaves the merged field's value as it is.
    const flag = (node, name) => { const value = attrOf(node, name); return value === undefined ? undefined : value === "true"; };
    ops.fieldOf = (node) => ({
        name: attrOf(node, "name"), type: attrOf(node, "type"), isPk: flag(node, "is-pk"), notNull: flag(node, "not-null"),
        defaultStr: attrOf(node, "default"), encrypt: flag(node, "encrypt"), enableAuditLog: attrOf(node, "enable-audit-log"),
        enableLocalization: flag(node, "enable-localization"), createOnly: flag(node, "create-only")
    });
    // The fields the view shows: the server's merged definition with the buffer's element over it, so an edit shows
    // before Save. A definition in the buffer gives the order and the fields; an extend-entity's fields override the
    // merged ones by name, its new ones after them, marked unsaved until the server has them.
    ops.mergeFields = (serverFields, element, inPlace) => {
        const own = children(element, ["field"]).map(ops.fieldOf).filter((field) => field.name);
        const server = serverFields || [];
        const byName = {};
        for (const field of server) byName[field.name] = field;
        const merged = (field) => {
            const base = byName[field.name];
            if (!base) return Object.assign({}, field, { type: field.type || "", unsaved: true });
            const out = Object.assign({}, base);
            for (const key of Object.keys(field)) if (field[key] !== undefined && key !== "name") out[key] = field[key];
            if (field.type === undefined) out.type = base.type;
            return out;
        };
        if (inPlace && element && element.name !== "extend-entity") {
            const listed = own.map(merged);
            const names = new Set(own.map((field) => field.name));
            return listed.concat(server.filter((field) => !names.has(field.name)));
        }
        const overrides = {};
        for (const field of own) overrides[field.name] = field;
        const out = server.map((field) => overrides[field.name] ? merged(overrides[field.name]) : field);
        for (const field of own) if (!byName[field.name]) out.push(merged(field));
        return out;
    };
    // A field's kind from its type, for the Fields table's chip.
    ops.kindOf = (field, relationships) => {
        const type = String(field.type || "");
        if (field.name === "statusId" || (relationships || []).some((rel) => rel.relatedEntityName === "moqui.basic.StatusItem" &&
            (rel.keyMap ? Object.keys(rel.keyMap).indexOf(field.name) >= 0 : false))) return "Status";
        if (/EnumId$/.test(field.name)) return "Picklist";
        if (/^date/.test(type)) return "Date";
        if (/^number|^currency/.test(type)) return "Number";
        if (type === "text-indicator") return "Yes/No";
        if (/^id/.test(type) && (relationships || []).some((rel) => rel.type === "one" && rel.keyMap && Object.keys(rel.keyMap).indexOf(field.name) >= 0)) return "Reference";
        if (/^text/.test(type)) return "Text";
        return type;
    };
    // A field's type for a kind of Add field.
    ops.typeOfKind = (kind) => ({ Text: "text-medium", Number: "number-decimal", Date: "date", "Date-time": "date-time",
        "Yes/No": "text-indicator", Picklist: "id", Reference: "id", Status: "id" })[kind] || "text-medium";
    // A field name from words: "Due date" -> dueDate.
    ops.camel = (words) => String(words || "").trim().split(/[^A-Za-z0-9]+/).filter((part) => part)
        .map((part, index) => index === 0 ? part.charAt(0).toLowerCase() + part.slice(1) : part.charAt(0).toUpperCase() + part.slice(1)).join("");

    // A field's title as a form shows it with none given: its name in words (dueDate -> Due Date).
    ops.titleOf = (name) => String(name || "").replace(/([a-z0-9])([A-Z])/g, "$1 $2").replace(/^./, (ch) => ch.toUpperCase());
    // A label in another language: a LocalizedMessage record of the title (the component's L10n messages).
    ops.localizedXml = (original, locale, localized) => ops.xml("moqui.basic.LocalizedMessage", [["original", original], ["locale", locale], ["localized", localized]]);
    // A view entity's condition: an econdition into its entity-condition, made when it has none.
    ops.addCondition = (view, condition) => {
        const holder = children(view, ["entity-condition"])[0];
        const text = ops.xml("econdition", [["entity-alias", condition.entityAlias], ["field-name", condition.fieldName],
            ["operator", condition.operator && condition.operator !== "equals" ? condition.operator : ""], ["value", condition.value], ["to-field-name", condition.toFieldName]]);
        if (holder) {
            const first = children(holder, ["order-by"])[0];
            return [first ? { op: "insert", at: first.path, position: "before", xml: text } : { op: "insert", at: holder.path, position: "last", xml: text }];
        }
        return [{ op: "insert", at: view.path, position: "last", xml: ops.xml("entity-condition", [], text) }];
    };
    ops.addOrderBy = (view, fieldName) => {
        const holder = children(view, ["entity-condition"])[0];
        const text = ops.xml("order-by", [["field-name", fieldName]]);
        return holder ? [{ op: "insert", at: holder.path, position: "last", xml: text }]
            : [{ op: "insert", at: view.path, position: "last", xml: ops.xml("entity-condition", [], text) }];
    };

    const ENTITIES_TEXT = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<entities xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" " +
        "xsi:noNamespaceSchemaLocation=\"http://moqui.org/xsd/entity-definition-3.xsd\">\n</entities>\n";
    const SEED_TEXT = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<entity-facade-xml type=\"seed\">\n</entity-facade-xml>\n";
    ops.ENTITIES_TEXT = ENTITIES_TEXT;
    ops.SEED_TEXT = SEED_TEXT;

    // ---- the view ----

    const KINDS = ["Text", "Number", "Date", "Date-time", "Yes/No", "Picklist", "Reference", "Status", "Other"];

    const entityView = {
        name: "mDevEntityView",
        props: { ctx: { type: Object, required: true }, tab: { type: Object, required: true }, outline: Object },
        data() {
            return {
                entityName: "", view: null, loading: false, error: "", target: null, seed: null, section: "fields",
                adding: null, relAdding: null, indexAdding: null, memberAdding: null, aliasAdding: null, uses: null,
                ddl: null, showOnForm: null, entityOptions: [], filterFields: "", seedPending: false, schema: null, viewSql: "",
                dataCheck: null, exporting: null
            };
        },
        computed: {
            elements() { return ops.entityElements(this.outline); },
            isData() { return this.outline && this.outline.name === "entity-facade-xml"; },
            entity() { return this.view ? this.view.entity : null; },
            isView() { return !!(this.entity && this.entity.isViewEntity); },
            // the element edits go to, in the buffer they go to, from its outline as it is now
            buffer() { return this.target ? (this.target.inTab ? this.tab : this.target.buffer) : null; },
            element() {
                if (!this.target || !this.buffer) return null;
                const found = ops.findEntityElement(this.buffer.outline, this.view.entity.fullEntityName,
                    this.target.inPlace ? null : ["extend-entity"]);
                return found ? found.node : null;
            },
            // the tab's own file, or a companion the user may write; never below 1024 px or for the view tier
            canEdit() { return this.ctx.mayWrite && (this.ctx.canWrite || !!(this.target && !this.target.inTab && !(this.buffer && this.buffer.readOnly))); },
            originOf() {
                const map = {};
                for (const field of (this.view && this.view.fields) || []) map[field.name] = field;
                return map;
            },
            fields() {
                const list = ops.mergeFields((this.entity && this.entity.fields) || [], this.element, this.target && this.target.inPlace);
                const filter = this.filterFields.toLowerCase();
                return list.filter((field) => !filter || field.name.toLowerCase().indexOf(filter) >= 0);
            },
            relationships() { return ((this.entity && this.entity.relationships) || []).filter((rel) => !rel.isAutoReverse); },
            incoming() { return ((this.entity && this.entity.relationships) || []).filter((rel) => rel.isAutoReverse); },
            tableNote() {
                const table = this.view && this.view.table;
                if (!table || this.isView) return "";
                if (table.error) return table.error;
                if (!table.exists) return "No table yet: Save makes it.";
                if (!table.missingColumns.length && !table.extraColumns.length) return "The table matches the fields.";
                return "";
            },
            seedTables() { return this.isData ? ops.recordTables(this.outline) : (this.seed && this.seed.outline ? ops.recordTables(this.seed.outline) : []); },
            full() { return this.ctx.tier === "full"; },
            // the attributes the schema gives the element edits go to (entity, view-entity or extend-entity), but its name
            elementAttributes() {
                const element = this.element;
                const kind = element ? element.name : (this.target && !this.target.inPlace ? "extend-entity" : this.isView ? "view-entity" : "entity");
                const spec = this.schema ? (this.schema.elements || []).find((item) => item.name === kind) : null;
                return spec ? (spec.attributes || []).filter((attr) => ["entity-name", "package"].indexOf(attr.name) < 0) : [];
            },
            // a view entity's own definition in the buffer, whose condition may be edited
            viewElement() { return this.isView && this.element && this.element.name === "view-entity" ? this.element : null; },
            conditions() {
                const holder = this.viewElement ? children(this.viewElement, ["entity-condition"])[0] : null;
                return holder ? children(holder, ["econdition", "order-by", "date-filter", "econditions"]) : [];
            },
            dataEditUrl() { return this.entity ? "/qapps/tools/Entity/DataEdit/EntityDataFind?selectedEntity=" + encodeURIComponent(this.entity.fullEntityName) : ""; }
        },
        watch: {
            outline() { if (!this.entityName && !this.isData) this.pickEntity(); }
        },
        mounted() {
            this.pickEntity();
            this.ctx.onSaved((reports) => this.afterSave(reports));
            this.ctx.get("schema", { rootElement: this.isData ? "entity-facade-xml" : "entities" }).then((answer) => { this.schema = answer.digest || null; }, () => {});
        },
        methods: {
            pickEntity() {
                if (this.isData) return;
                const asked = String(this.tab.artifact || "").replace(/^(entity|view-entity):/, "");
                const first = this.elements[0];
                const name = asked || (first ? (first.package ? first.package + "." : "") + first.entityName : "");
                if (name && name !== this.entityName) this.load(name);
            },
            load(name) {
                this.entityName = name;
                this.loading = true; this.error = "";
                return this.ctx.get("entityView", { entityName: name }).then((answer) => {
                    this.loading = false;
                    this.view = answer.view || null;
                    return this.resolveTarget();
                }, (error) => { this.loading = false; this.error = pure.errorText(error); });
            },
            // Where the edits go: this tab when it holds the extend-entity or defines the entity and may be edited in
            // place, else the site's entity file as a companion buffer; the site's seed data file likewise.
            resolveTarget() {
                const view = this.view;
                if (!view) return;
                const here = ops.findEntityElement(this.tab.outline, view.entity.fullEntityName);
                const tabWritable = !this.tab.readOnly;
                if (here && here.kind === "extend-entity" && tabWritable) this.target = { inTab: true, inPlace: false };
                else if (here && here.kind !== "extend-entity" && view.inPlace && tabWritable) this.target = { inTab: true, inPlace: true };
                else if (view.target && !view.target.inPlace) {
                    return this.ctx.companion(view.target.rootId, view.target.path, ENTITIES_TEXT).then((buffer) => {
                        this.target = { inTab: false, inPlace: false, buffer, label: view.target.component + ": " + view.target.path };
                    }, (error) => { this.error = pure.errorText(error); });
                } else this.target = null;
            },
            seedBuffer() {
                if (this.seed) return Promise.resolve(this.seed);
                const file = this.view && this.view.seedFile;
                if (!file) return Promise.reject(new Error("No site component to write the seed data in: create one on Home"));
                return this.ctx.companion(file.rootId, file.path, SEED_TEXT).then((buffer) => { this.seed = buffer; return buffer; });
            },
            // Apply operations to the target element, made first when the site has no extend-entity of the entity yet.
            edit(build) {
                if (!this.target) { this.ctx.note("This entity is upstream and there is no site component to extend it in", "warning"); return Promise.resolve(); }
                const buffer = this.buffer;
                let path = this.element ? this.element.path : null;
                let first = [];
                if (!path) {
                    const made = ops.ensureExtend(buffer.outline || { path: [], children: [] }, this.view.entity.fullEntityName);
                    path = made.path; first = made.ops;
                }
                const element = this.element || { path, children: [], attrs: [] };
                const list = first.concat(build(element));
                if (!list.length) return Promise.resolve();
                return this.ctx.apply(list, this.target.inTab ? null : buffer).catch((error) => this.ctx.fail(error));
            },
            setAttr(field, name, value) { return this.edit((element) => ops.setFieldAttribute(element, field.name, name, value)); },
            setDescription(field, text) { return this.edit((element) => ops.setFieldDescription(element, field.name, text)); },
            descriptionOf(field) {
                const origin = this.originOf[field.name];
                const element = this.element;
                const node = element ? ops.childNamed(element, "field", "name", field.name) : null;
                const description = node ? children(node, ["description"])[0] : null;
                return description ? description.text || "" : (origin && origin.description) || "";
            },
            // ---- add a field ----
            startAdd(after) {
                this.adding = { kind: "Text", words: "", name: "", type: "text-medium", after: after || "", isPk: false, notNull: false,
                    description: "", related: "", relatedOptions: [], enumTypeId: "", values: "", statusTypeId: "", items: "", title: "", typeEdited: false,
                    locale: "", localized: "" };
            },
            kindChanged() { const add = this.adding; if (add.kind !== "Other") add.type = ops.typeOfKind(add.kind); this.nameFromWords(); },
            nameFromWords() {
                const add = this.adding;
                let name = ops.camel(add.words);
                if (add.kind === "Picklist" && name && !/EnumId$/.test(name)) name += "EnumId";
                if (add.kind === "Reference" && name && !/Id$/.test(name)) name += "Id";
                if (add.kind === "Status" && !name) name = "statusId";
                add.name = name;
                const pascal = name ? name.charAt(0).toUpperCase() + name.slice(1) : "";
                // the type follows the name until the user words it
                if (add.kind === "Picklist" && !add.typeEdited) add.enumTypeId = ops.shortName(this.entityName) + pascal.replace(/EnumId$/, "");
                if (add.kind === "Status" && !add.typeEdited) add.statusTypeId = ops.shortName(this.entityName) + (name && name !== "statusId" ? pascal.replace(/Id$/, "") : "");
            },
            searchEntities(text, update) {
                if (!text || text.length < 2) { update(() => {}); return; }
                this.ctx.get("artifacts", { text, kinds: "entity,view-entity", limit: 20 }).then((answer) => {
                    update(() => { this.entityOptions = (answer.artifacts || []).map((artifact) => artifact.name); });
                }, () => update(() => {}));
            },
            values(text, prefix) {
                return String(text || "").split(/[\n,]/).map((value) => value.trim()).filter((value) => value).map((description) => ({
                    id: prefix + ops.camel(description).replace(/^./, (ch) => ch.toUpperCase()), description
                }));
            },
            addField() {
                const add = this.adding;
                if (!add.name) { this.ctx.note("A field needs a name", "warning"); return; }
                const field = { name: add.name, type: add.type, isPk: add.isPk, notNull: add.notNull, description: add.description };
                const label = add.locale && add.localized ? ops.localizedXml(add.title || ops.titleOf(add.name), add.locale, add.localized) : "";
                const done = () => {
                    this.adding = null; this.offerForm(field.name);
                    // a label in another language: the component's L10n message, with the seed data
                    if (label) return this.seedBuffer().then((seed) => { this.seedPending = true; return this.ctx.apply(ops.insertRecords(seed.outline, [label]), seed); })
                        .catch((error) => this.ctx.fail(error));
                };
                if (add.kind === "Picklist" || add.kind === "Status") {
                    const spec = add.kind === "Picklist"
                        ? ops.picklist({ fieldName: add.name, enumTypeId: add.enumTypeId, title: add.title || add.enumTypeId.replace(ops.shortName(this.entityName), "") || add.enumTypeId,
                            description: add.description, values: this.values(add.values, add.enumTypeId.replace(/Type$/, "")) })
                        : ops.status({ fieldName: add.name, statusTypeId: add.statusTypeId, description: add.description,
                            items: this.values(add.items, add.statusTypeId) });
                    // one Save writes the three: the field and its relationship (one undo step), and the seed records
                    return this.seedBuffer().then((seed) => this.edit((element) => ops.addField(element, field, add.after)
                        .concat(ops.insertRelationship(element, spec.relationship)))
                        .then(() => { this.seedPending = true; return this.ctx.apply(ops.insertRecords(seed.outline, spec.seed), seed); }))
                        .then(done, (error) => this.ctx.fail(error));
                }
                const reference = add.kind === "Reference" && add.related
                    ? { type: "one", related: add.related, keyMaps: [{ fieldName: add.name, related: "" }] } : null;
                return this.edit((element) => ops.addField(element, field, add.after)
                    .concat(reference ? ops.addRelationship(element, reference) : [])).then(done);
            },
            // "Show it on a form": the screens the index finds using the entity (11.2 opens them in the builder)
            offerForm(fieldName) {
                this.ctx.get("refs", { artifactId: "entity:" + this.view.entity.fullEntityName, direction: "in", kinds: "screen,form" }).then((answer) => {
                    const seen = {};
                    const screens = (answer.refs || []).filter((ref) => ref.other && !seen[ref.other.id] && (seen[ref.other.id] = true)).slice(0, 12);
                    this.showOnForm = { fieldName, screens };
                }, () => { this.showOnForm = { fieldName, screens: [] }; });
            },
            openScreen(ref) {
                if (devAide.openInBuilder) devAide.openInBuilder(ref, this.ctx);
                else this.ctx.openArtifact(ref.other);
            },
            // ---- remove, move ----
            removeField(field) {
                const id = "field:" + this.view.entity.fullEntityName + "." + field.name;
                this.ctx.get("refs", { artifactId: id, direction: "in" }).then((answer) => {
                    const counts = {};
                    for (const ref of answer.refs || []) { const kind = ref.other ? ref.other.kind : ref.toKind; counts[kind] = (counts[kind] || 0) + 1; }
                    const words = Object.keys(counts).map((kind) => counts[kind] + " " + kind + (counts[kind] === 1 ? "" : "s"));
                    this.uses = { field, words, refs: answer.refs || [] };
                }, () => { this.uses = { field, words: [], refs: [] }; });
            },
            confirmRemove() {
                const field = this.uses.field;
                this.uses = null;
                this.edit((element) => ops.removeField(element, field.name));
            },
            ownField(field) { return !!(this.element && ops.childNamed(this.element, "field", "name", field.name) && !this.isOverride(field)); },
            isOverride(field) {
                const origin = this.originOf[field.name];
                return !!(origin && this.target && !this.target.inPlace && origin.component && this.view.target && origin.component !== this.view.target.component);
            },
            dragStart(field, event) { this.dragging = field.name; if (event.dataTransfer) event.dataTransfer.effectAllowed = "move"; },
            dropOn(field, event) {
                const from = this.dragging;
                this.dragging = null;
                if (!from || from === field.name) return;
                const rect = event.currentTarget.getBoundingClientRect();
                const position = event.clientY < rect.top + rect.height / 2 ? "before" : "after";
                this.edit((element) => ops.moveField(element, from, field.name, position));
            },
            // ---- relationships, indexes ----
            startRelationship() { this.relAdding = { type: "one", related: "", title: "", fieldName: "", relatedField: "" }; },
            addRelationship() {
                const rel = this.relAdding;
                if (!rel.related) return;
                this.relAdding = null;
                this.edit((element) => ops.addRelationship(element, { type: rel.type, related: rel.related, title: rel.title,
                    keyMaps: rel.fieldName ? [{ fieldName: rel.fieldName, related: rel.relatedField }] : [] }));
            },
            removeRelationship(rel) {
                this.edit((element) => ops.removeRelationship(element, rel.relatedEntityName, rel.title));
            },
            startIndex() { this.indexAdding = { name: "", unique: false, fields: [] }; },
            addIndex() {
                const index = this.indexAdding;
                if (!index.name || !index.fields.length) return;
                this.indexAdding = null;
                this.edit((element) => ops.addIndex(element, index));
            },
            // ---- view entities ----
            startMember() { this.memberAdding = { entityAlias: "", entityName: "", joinFromAlias: "", joinOptional: false, relationship: "", relationships: [] }; },
            memberJoinChanged() {
                const member = this.memberAdding;
                const from = ((this.entity && this.entity.members) || []).find((item) => item.entityAlias === member.joinFromAlias);
                member.relationships = [];
                if (!from || !from.entityName) return;
                this.ctx.get("entityView", { entityName: from.entityName }).then((answer) => {
                    const rels = (answer.view && answer.view.entity && answer.view.entity.relationships) || [];
                    member.relationships = rels.map((rel) => rel.title ? rel.title + "#" + rel.relatedEntityName : rel.relatedEntityName);
                }, () => {});
            },
            addMember() {
                const member = this.memberAdding;
                if (!member.entityAlias || !(member.entityName || member.relationship)) return;
                this.memberAdding = null;
                this.edit((element) => ops.addMember(element, member));
            },
            startAlias() { this.aliasAdding = { name: "", entityAlias: "", field: "", function: "", all: false, prefix: "", exclude: "" }; },
            addAlias() {
                const alias = this.aliasAdding;
                this.aliasAdding = null;
                if (alias.all) this.edit((element) => ops.addAliasAll(element, { entityAlias: alias.entityAlias, prefix: alias.prefix,
                    exclude: String(alias.exclude || "").split(",").map((name) => name.trim()) }));
                else if (alias.name) this.edit((element) => ops.addAlias(element, alias));
            },
            runView() {
                this.ctx.get("entityView", { entityName: this.entityName }).then((answer) => { this.view = answer.view || this.view; }, (error) => this.ctx.fail(error));
                this.ctx.get("viewSql", { entityName: this.entityName }).then((answer) => { this.viewSql = answer.sql || ""; }, (error) => { this.viewSql = pure.errorText(error); });
            },
            attrValue(name) { const element = this.element; return element ? pure.attr(element, name) || "" : ""; },
            setEntityAttr(name, value) {
                this.edit((element) => value === "" ? (pure.attr(element, name) === undefined ? [] : [{ op: "remove-attribute", at: element.path, name }])
                    : [{ op: "set-attribute", at: element.path, name, value }]);
            },
            setConditionAttr(node, name, value) {
                this.ctx.apply(value === "" ? (pure.attr(node, name) === undefined ? [] : [{ op: "remove-attribute", at: node.path, name }])
                    : [{ op: "set-attribute", at: node.path, name, value }]).catch((error) => this.ctx.fail(error));
            },
            addCondition() { if (this.viewElement) this.ctx.apply(ops.addCondition(this.viewElement, { fieldName: "", value: "" })).catch((error) => this.ctx.fail(error)); },
            addOrderBy() { if (this.viewElement) this.ctx.apply(ops.addOrderBy(this.viewElement, "")).catch((error) => this.ctx.fail(error)); },
            removeCondition(node) { this.ctx.apply([{ op: "remove", at: node.path }]).catch((error) => this.ctx.fail(error)); },
            // ---- a data file (12) ----
            checkData() {
                this.dataCheck = { running: true };
                this.ctx.post("checkData", { rootId: this.ctx.rootId, path: this.tab.path, text: this.tab.text }).then((answer) => {
                    this.dataCheck = { messages: answer.messages || [], differences: answer.differences || [], fields: answer.fieldsChecked };
                }, (error) => { this.dataCheck = null; this.ctx.fail(error); });
            },
            loadChanged() {
                this.ctx.post("loadChanged", { rootId: this.ctx.rootId, path: this.tab.path }).then((answer) => {
                    this.ctx.note(answer.changed ? "Loaded " + answer.records + " records of the " + answer.changed + " changed since the last commit" : "No record changed since the last commit");
                }, (error) => this.ctx.fail(error));
            },
            startExport() { this.exporting = { entity: "", filter: "" }; },
            exportRecords() {
                const exp = this.exporting;
                this.exporting = null;
                this.ctx.get("exportRecords", { entityName: exp.entity, filter: exp.filter }).then((answer) => {
                    if (!answer.records) { this.ctx.note("No " + exp.entity + " records", "warning"); return; }
                    return this.ctx.apply([{ op: "insert", at: (this.outline && this.outline.path) || [], position: "last", xml: answer.records }])
                        .then(() => this.ctx.note("Exported " + answer.count + " records into the file: Save keeps them"));
                }).catch((error) => this.ctx.fail(error));
            },
            // ---- the table: what the check never does, for the full tier ----
            planDdl(action, fieldName, columnName) {
                this.ctx.get("planDdl", { entityName: this.view.entity.fullEntityName, action, fieldName: fieldName || "", columnName: columnName || "" }).then((answer) => {
                    this.ddl = { action, fieldName, columnName, statements: answer.statements || [], table: answer.tableName, confirm: "", running: false };
                }, (error) => this.ctx.fail(error));
            },
            runDdl() {
                const ddl = this.ddl;
                ddl.running = true;
                this.ctx.post("runDdl", { entityName: this.view.entity.fullEntityName, action: ddl.action, fieldName: ddl.fieldName || "",
                    columnName: ddl.columnName || "", confirmTable: ddl.confirm }).then((answer) => {
                    this.ddl = null;
                    this.ctx.note("Ran " + (answer.ran || []).length + " statements on " + ddl.table, "positive");
                    this.load(this.entityName);
                }, (error) => { ddl.running = false; this.ctx.fail(error); });
            },
            // ---- seed rows ----
            applyRecords(buffer, list) {
                if (buffer !== this.tab) this.seedPending = true;
                return this.ctx.apply(list, buffer === this.tab ? null : buffer).catch((error) => this.ctx.fail(error));
            },
            setRecord(buffer, row, name, value) { this.applyRecords(buffer, ops.setRecordValue(row, name, value)); },
            addRecord(buffer, table) { this.applyRecords(buffer, ops.addRecord(buffer.outline, table, {})); },
            removeRecord(buffer, row) { this.applyRecords(buffer, [{ op: "remove", at: row.path }]); },
            // Load this file (12): a data file's records into the database
            loadThis() {
                this.ctx.post("loadSeed", { rootId: this.ctx.rootId, path: this.tab.path })
                    .then((answer) => this.ctx.note("Loaded " + answer.records + " records from " + answer.file), (error) => this.ctx.fail(error));
            },
            // after Save: a seed data file loads, and the view reads the entity again with what the save's reload did
            afterSave(reports) {
                const ddl = [];
                for (const report of reports || []) for (const file of (report.reload && report.reload.files) || []) for (const sql of file.ddl || []) ddl.push(sql);
                if (ddl.length) this.ctx.note("Table updated: " + ddl.length + " DDL statement" + (ddl.length === 1 ? "" : "s"), "positive");
                const seed = this.seedPending && this.seed && this.seed.saved === this.seed.text && this.seed.base ? this.seed : null;
                if (seed) this.seedPending = false;
                const loaded = seed
                    ? this.ctx.post("loadSeed", { rootId: seed.rootId, path: seed.path }).then((answer) => this.ctx.note("Seed data loaded: " + answer.records + " records"))
                    : Promise.resolve();
                return loaded.then(() => (this.entityName ? this.load(this.entityName) : null), (error) => this.ctx.fail(error));
            },
            openOrigin(origin) { if (origin && origin.rootId) this.ctx.openArtifact({ rootId: origin.rootId, relativePath: origin.path, line: origin.line, name: origin.path }); }
        },
        template: `
<div class="m-dev-entity-view q-pa-sm" id="DevEntityView">
  <div v-if="isData" id="DevSeedRows">
    <div class="row items-center q-mb-sm q-gutter-x-xs"><span class="text-subtitle2">Records</span><q-space></q-space>
      <q-btn dense flat no-caps icon="sym_o_fact_check" label="Check against the database" @click="checkData" id="DevCheckData"></q-btn>
      <q-btn dense flat no-caps icon="sym_o_difference" label="Load the changed records" :disable="!ctx.canWrite || tab.text !== tab.saved" @click="loadChanged" id="DevLoadChanged"></q-btn>
      <q-btn dense flat no-caps icon="sym_o_download" label="Export records…" :disable="!ctx.canWrite" @click="startExport" id="DevExportRecords"></q-btn>
      <q-btn dense flat no-caps icon="sym_o_upload" label="Load this file" :disable="!ctx.canWrite || tab.text !== tab.saved" @click="loadThis" id="DevLoadDataFile">
        <q-tooltip>Save first; loads its records into the database</q-tooltip></q-btn></div>
    <div v-for="table in seedTables" :key="table.entity" class="q-mb-md">
      <div class="row items-center no-wrap"><span class="text-weight-medium">{{table.entity}}</span><span class="m-muted q-ml-sm text-caption">{{table.rows.length}}</span>
        <q-space></q-space><q-btn dense flat no-caps size="sm" icon="add" label="Add row" :disable="!ctx.canWrite" @click="addRecord(tab, table)"></q-btn></div>
      <div class="m-dev-grid-wrap"><table class="m-dev-grid"><thead><tr><th v-for="column in table.columns" :key="column">{{column}}</th><th></th></tr></thead>
        <tbody><tr v-for="row in table.rows" :key="row.path.join('.')">
          <td v-for="column in table.columns" :key="column"><input class="m-dev-cell" :value="row.values[column] || ''" :readonly="!ctx.canWrite"
              @change="setRecord(tab, row, column, $event.target.value)"></td>
          <td><q-btn dense flat round size="xs" icon="delete" :disable="!ctx.canWrite" @click="removeRecord(tab, row)"></q-btn></td>
        </tr></tbody></table></div>
    </div>
    <div v-if="!seedTables.length" class="m-muted">No records yet.</div>
    <div v-if="dataCheck" class="q-mt-md" id="DevDataCheck">
      <div class="text-subtitle2">Against the database <q-spinner v-if="dataCheck.running" size="14px"></q-spinner></div>
      <template v-if="!dataCheck.running">
        <div v-if="!dataCheck.messages.length" class="text-positive text-caption">Every record matches ({{dataCheck.fields}} fields checked).</div>
        <div v-for="(text, i) in dataCheck.messages" :key="i" class="text-caption">{{text}}</div>
      </template>
    </div>
  </div>
  <template v-else>
    <div class="row items-center q-gutter-x-sm no-wrap q-mb-xs">
      <q-select v-if="elements.length > 1" dense outlined options-dense style="min-width: 14rem" :model-value="entityName"
          :options="elements.map(e => (e.package ? e.package + '.' : '') + e.entityName)" @update:model-value="load" id="DevEntityPick"></q-select>
      <q-linear-progress v-if="loading" indeterminate class="col"></q-linear-progress>
    </div>
    <div v-if="error" class="text-negative q-pa-sm" id="DevEntityError">{{error}}</div>
    <template v-if="view && entity">
      <div class="m-dev-entity-header q-mb-sm" id="DevEntityHeader">
        <div class="row items-center q-gutter-x-sm">
          <q-icon :name="isView ? 'sym_o_join_inner' : 'sym_o_table'" size="22px"></q-icon>
          <span class="text-h6">{{entity.entityName}}</span><span class="m-muted">{{entity.packageName}}</span>
          <q-chip v-if="entity.groupName" dense square>{{entity.groupName}}</q-chip>
          <q-chip v-if="view.artifact" dense square :icon="view.upstream ? 'sym_o_lock' : 'sym_o_home'" clickable @click="openOrigin({ rootId: view.artifact.rootId, path: view.artifact.relativePath, line: view.artifact.line })">{{view.artifact.component}}</q-chip>
          <q-chip v-for="(ext, i) in view.extensions" :key="i" dense square icon="sym_o_extension" :color="ext.site ? 'primary' : undefined" :text-color="ext.site ? 'white' : undefined"
              clickable @click="openOrigin(ext)">{{ext.component}}</q-chip>
        </div>
        <q-expansion-item v-if="elementAttributes.length" dense dense-toggle label="Attributes" header-class="text-caption q-px-none" id="DevEntityAttributes">
          <div class="row q-col-gutter-sm">
            <div v-for="attribute in elementAttributes" :key="attribute.name" class="col-6 col-md-3">
              <select v-if="attribute.values && attribute.values.length" class="m-dev-cell" style="width: 100%" :disabled="!canEdit || !target" :value="attrValue(attribute.name)"
                  @change="setEntityAttr(attribute.name, $event.target.value)"><option value=""></option><option v-for="value in attribute.values" :key="value" :value="value">{{value}}</option></select>
              <input v-else class="m-dev-cell" style="width: 100%" :disabled="!canEdit || !target" :value="attrValue(attribute.name)" :placeholder="attribute.default || ''"
                  @change="setEntityAttr(attribute.name, $event.target.value)">
              <div class="text-caption m-muted" :title="attribute.doc || ''">{{attribute.name}}</div>
            </div>
          </div>
        </q-expansion-item>
        <div class="text-caption q-mt-xs" id="DevEntityTarget">
          <template v-if="target && target.inTab && target.inPlace">Editing {{tab.path.split('/').pop()}}{{view.upstream ? ' in place: upstream editing is on' : ''}}</template>
          <template v-else-if="target && target.inTab">Extending in {{tab.path.split('/').pop()}}</template>
          <template v-else-if="target">Extending in {{view.site}}: {{target.buffer.path}}{{target.buffer.missing ? ' (made on Save)' : ''}}</template>
          <template v-else-if="view.upstream">Upstream and read only: create a site component on Home to extend it</template>
        </div>
      </div>
      <q-tabs dense align="left" no-caps v-model="section" class="m-dev-tabs">
        <q-tab name="fields" :label="(isView ? 'Fields ' : 'Fields ') + entity.fields.length"></q-tab>
        <q-tab v-if="isView" name="members" :label="'Members ' + (entity.members || []).length"></q-tab>
        <q-tab name="relationships" :label="'Relationships ' + relationships.length"></q-tab>
        <q-tab name="incoming" :label="'Incoming ' + incoming.length"></q-tab>
        <q-tab name="indexes" :label="'Indexes ' + (entity.indexes || []).length"></q-tab>
        <q-tab name="rules" :label="'EECA rules ' + view.eecas.length"></q-tab>
        <q-tab name="table" :label="isView ? 'Data' : 'Table'" @click="isView && runView()"></q-tab>
        <q-tab v-if="seed || (view.seedFile && view.seedFile.exists)" name="seed" label="Seed data"></q-tab>
      </q-tabs>
      <q-separator class="q-mb-sm"></q-separator>

      <div v-show="section === 'fields'" id="DevEntityFields">
        <div class="row items-center q-gutter-x-sm q-mb-xs">
          <q-input dense outlined v-model="filterFields" placeholder="Filter fields" style="width: 14rem"></q-input>
          <q-space></q-space>
          <q-btn v-if="!isView" dense unelevated no-caps color="primary" icon="add" label="Add field" :disable="!canEdit || !target" @click="startAdd('')" id="DevAddField"></q-btn>
          <span class="text-caption m-muted">Used by {{view.usedByCount}}</span>
        </div>
        <div class="m-dev-grid-wrap"><table class="m-dev-grid">
          <thead><tr><th></th><th>Name</th><th>Type</th><th>PK</th><th>Not null</th><th>Default</th><th>Encrypt</th><th>Audit</th><th>L10n</th><th>Create only</th><th>Description</th><th>Origin</th><th></th></tr></thead>
          <tbody>
            <tr v-for="field in fields" :key="field.name" :data-field="field.name" draggable="true" @dragstart="dragStart(field, $event)" @dragover.prevent @drop.prevent="dropOn(field, $event)">
              <td class="m-dev-drag"><q-icon name="drag_indicator" size="14px"></q-icon></td>
              <td class="m-dev-mono">{{field.name}} <q-chip dense size="sm" square class="q-ml-xs">{{kindOf(field)}}</q-chip>
                <q-chip v-if="field.unsaved" dense size="sm" square color="amber-3" text-color="black">unsaved</q-chip></td>
              <td><select class="m-dev-cell" :value="field.type" :disabled="!canEdit || isView" @change="setAttr(field, 'type', $event.target.value)">
                <option v-for="type in view.fieldTypes" :key="type.type" :value="type.type" :title="type.sqlType">{{type.type}}</option></select>
                <q-tooltip>{{(entity.sqlTypes || {})[field.name] || ''}}</q-tooltip></td>
              <td><input type="checkbox" :checked="field.isPk" :disabled="!canEdit || isView" @change="setAttr(field, 'is-pk', $event.target.checked)"></td>
              <td><input type="checkbox" :checked="field.notNull" :disabled="!canEdit || isView" @change="setAttr(field, 'not-null', $event.target.checked)"></td>
              <td><input class="m-dev-cell" :value="field.defaultStr || ''" :readonly="!canEdit || isView" @change="setAttr(field, 'default', $event.target.value)" style="width: 6rem"></td>
              <td><input type="checkbox" :checked="field.encrypt" :disabled="!canEdit || isView" @change="setAttr(field, 'encrypt', $event.target.checked)"></td>
              <td><select class="m-dev-cell" :value="field.enableAuditLog || 'false'" :disabled="!canEdit || isView" @change="setAttr(field, 'enable-audit-log', $event.target.value === 'false' ? '' : $event.target.value)">
                <option value="false">no</option><option value="true">yes</option><option value="update">update</option></select></td>
              <td><input type="checkbox" :checked="field.enableLocalization" :disabled="!canEdit || isView" @change="setAttr(field, 'enable-localization', $event.target.checked)"></td>
              <td><input type="checkbox" :checked="field.createOnly" :disabled="!canEdit || isView" @change="setAttr(field, 'create-only', $event.target.checked)"></td>
              <td><input class="m-dev-cell" :value="descriptionOf(field)" :readonly="!canEdit || isView" @change="setDescription(field, $event.target.value)" style="width: 14rem"></td>
              <td class="text-caption"><a v-if="originOf[field.name]" class="cursor-pointer" @click="openOrigin(originOf[field.name])">
                <q-icon :name="originOf[field.name].site ? 'sym_o_extension' : 'sym_o_lock'" size="14px"></q-icon> {{originOf[field.name].component}}</a></td>
              <td><q-btn v-if="!isView" dense flat round size="xs" icon="add" :disable="!canEdit" @click="startAdd(field.name)"><q-tooltip>Add a field after this one</q-tooltip></q-btn>
                <q-btn v-if="!isView" dense flat round size="xs" icon="delete" :disable="!canEdit || !ownField(field)" @click="removeField(field)"><q-tooltip>{{ownField(field) ? 'Remove' : 'Defined upstream: an extension cannot remove it'}}</q-tooltip></q-btn></td>
            </tr>
          </tbody></table></div>
        <div v-if="showOnForm" class="q-mt-sm q-pa-sm m-dev-note" id="DevShowOnForm">
          <div class="row items-center"><span class="text-weight-medium">Show {{showOnForm.fieldName}} on a form</span><q-space></q-space><q-btn dense flat round size="sm" icon="close" @click="showOnForm = null"></q-btn></div>
          <div class="text-caption m-muted">A form that lists its fields needs the field added; a form with auto-fields-entity shows it already.</div>
          <q-list dense><q-item v-for="ref in showOnForm.screens" :key="ref.other.id" clickable @click="openScreen(ref)">
            <q-item-section><q-item-label>{{ref.other.shortName}}</q-item-label><q-item-label caption>{{ref.other.component}} · {{ref.refType}}</q-item-label></q-item-section></q-item>
            <q-item v-if="!showOnForm.screens.length"><q-item-section class="m-muted">No screen uses the entity yet.</q-item-section></q-item></q-list>
        </div>
      </div>

      <div v-if="section === 'members'" id="DevEntityMembers">
        <div class="m-dev-grid-wrap"><table class="m-dev-grid"><thead><tr><th>Alias</th><th>Entity</th><th>Join from</th><th>Optional</th><th>Relationship</th><th>Key maps</th></tr></thead>
          <tbody><tr v-for="member in entity.members" :key="member.entityAlias"><td class="m-dev-mono">{{member.entityAlias}}</td><td>{{member.entityName}}</td>
            <td>{{member.joinFromAlias || ''}}</td><td>{{member.joinOptional ? 'yes' : ''}}</td><td>{{member.relationship || ''}}</td>
            <td class="text-caption">{{(member.keyMaps || []).map(k => k.fieldName + (k.related ? '=' + k.related : '')).join(', ')}}</td></tr></tbody></table></div>
        <div class="row q-gutter-x-sm q-mt-xs"><q-btn dense flat no-caps icon="add" label="Add member" :disable="!canEdit || !target" @click="startMember"></q-btn>
          <q-btn dense flat no-caps icon="add" label="Add alias" :disable="!canEdit || !target" @click="startAlias"></q-btn>
          <q-btn dense flat no-caps icon="sym_o_play_arrow" label="Run" @click="section = 'table'; runView()"></q-btn></div>
        <div class="text-subtitle2 q-mt-md">Aliases</div>
        <div class="text-caption" v-for="(all, i) in entity.aliasAlls" :key="'all' + i">alias-all {{all.entityAlias}}{{all.prefix ? ' prefix ' + all.prefix : ''}}{{all.exclude.length ? ' except ' + all.exclude.join(', ') : ''}}</div>
        <div class="text-caption m-dev-mono" v-for="alias in entity.aliases" :key="alias.name">{{alias.name}} = {{alias.function ? alias.function + '(' : ''}}{{alias.entityAlias}}.{{alias.field || alias.name}}{{alias.function ? ')' : ''}}</div>
        <div class="text-subtitle2 q-mt-md">Condition and order</div>
        <template v-if="viewElement">
          <div v-for="node in conditions" :key="node.path.join('.')" class="row items-center q-gutter-x-xs text-caption q-mb-xs">
            <span class="m-dev-mono" style="width: 6rem">{{node.name}}</span>
            <template v-if="node.name === 'econdition'">
              <input class="m-dev-cell" placeholder="alias" style="width: 5rem" :value="pure.attr(node, 'entity-alias') || ''" @change="setConditionAttr(node, 'entity-alias', $event.target.value)">
              <input class="m-dev-cell" placeholder="field" :value="pure.attr(node, 'field-name') || ''" @change="setConditionAttr(node, 'field-name', $event.target.value)">
              <select class="m-dev-cell" :value="pure.attr(node, 'operator') || 'equals'" @change="setConditionAttr(node, 'operator', $event.target.value === 'equals' ? '' : $event.target.value)">
                <option v-for="op in operators" :key="op">{{op}}</option></select>
              <input class="m-dev-cell" placeholder="value" :value="pure.attr(node, 'value') || ''" @change="setConditionAttr(node, 'value', $event.target.value)">
              <input class="m-dev-cell" placeholder="or to field" :value="pure.attr(node, 'to-field-name') || ''" @change="setConditionAttr(node, 'to-field-name', $event.target.value)">
            </template>
            <input v-else-if="node.name === 'order-by'" class="m-dev-cell" placeholder="field (-field descending)" :value="pure.attr(node, 'field-name') || ''" @change="setConditionAttr(node, 'field-name', $event.target.value)">
            <span v-else class="m-muted">edit in Text</span>
            <q-btn dense flat round size="xs" icon="delete" :disable="!canEdit" @click="removeCondition(node)"></q-btn>
          </div>
          <q-btn dense flat no-caps size="sm" icon="add" label="Condition" :disable="!canEdit" @click="addCondition" id="DevAddCondition"></q-btn>
          <q-btn dense flat no-caps size="sm" icon="add" label="Order by" :disable="!canEdit" @click="addOrderBy"></q-btn>
        </template>
        <div v-else class="text-caption m-muted">{{entity.hasCondition ? 'It has a condition, in its definition.' : 'No condition.'}} An extension cannot change a view's condition.</div>
      </div>

      <div v-if="section === 'relationships'" id="DevEntityRelationships">
        <q-list dense separator>
          <q-item v-for="rel in relationships" :key="rel.title + rel.relatedEntityName">
            <q-item-section><q-item-label>{{rel.type}} {{rel.title ? rel.title + ' ' : ''}}{{rel.relatedEntityName}}</q-item-label>
              <q-item-label caption>{{Object.keys(rel.keyMap || {}).map(k => k + ' = ' + rel.keyMap[k]).join(', ')}}</q-item-label></q-item-section>
            <q-item-section side><q-btn dense flat round size="xs" icon="sym_o_open_in_new" @click="ctx.openArtifactId('entity:' + rel.relatedEntityName, 'entity')"></q-btn></q-item-section>
            <q-item-section side><q-btn dense flat round size="xs" icon="delete" :disable="!canEdit" @click="removeRelationship(rel)"></q-btn></q-item-section>
          </q-item>
        </q-list>
        <q-btn dense flat no-caps icon="add" label="Add relationship" :disable="!canEdit || !target" @click="startRelationship" class="q-mt-xs"></q-btn>
      </div>
      <div v-if="section === 'incoming'" id="DevEntityIncoming">
        <q-list dense separator><q-item v-for="rel in incoming" :key="rel.title + rel.relatedEntityName" clickable @click="ctx.openArtifactId('entity:' + rel.relatedEntityName, 'entity')">
          <q-item-section><q-item-label>{{rel.relatedEntityName}}</q-item-label><q-item-label caption>{{rel.type}} {{rel.title || ''}} · {{Object.keys(rel.keyMap || {}).join(', ')}}</q-item-label></q-item-section></q-item></q-list>
        <div v-if="!incoming.length" class="m-muted">No entity points here.</div>
      </div>
      <div v-if="section === 'indexes'" id="DevEntityIndexes">
        <q-list dense separator><q-item v-for="index in entity.indexes" :key="index.name"><q-item-section><q-item-label>{{index.name}}{{index.unique ? ' (unique)' : ''}}</q-item-label>
          <q-item-label caption>{{index.fields.join(', ')}}</q-item-label></q-item-section></q-item></q-list>
        <q-btn dense flat no-caps icon="add" label="Add index" :disable="!canEdit || !target || isView" @click="startIndex" class="q-mt-xs"></q-btn>
        <div class="text-subtitle2 q-mt-md" v-if="(entity.masters || []).length">Master definitions</div>
        <div v-for="master in entity.masters" :key="master.name" class="text-caption">{{master.name}}: {{master.details.map(d => d.relationship).join(', ')}}</div>
      </div>
      <div v-if="section === 'rules'" id="DevEntityRules">
        <q-list dense separator><q-item v-for="rule in view.eecas" :key="rule.id" clickable @click="ctx.openArtifact(rule)">
          <q-item-section><q-item-label>{{rule.shortName}}</q-item-label><q-item-label caption>{{rule.component}} · {{rule.summary}}</q-item-label></q-item-section></q-item></q-list>
        <div v-if="!view.eecas.length" class="m-muted">No EECA rules on this entity.</div>
      </div>
      <div v-if="section === 'table'" id="DevEntityTable">
        <template v-if="!isView">
          <div class="q-mb-xs">{{tableNote}}</div>
          <div v-for="column in view.table.missingColumns" :key="'m' + column" class="text-caption">Missing column {{column}}: Save adds it.</div>
          <div v-for="column in view.table.extraColumns" :key="'x' + column" class="row items-center text-caption q-gutter-x-sm">
            <span>Column {{column}} has no field (a removed or renamed field keeps its column).</span>
            <template v-if="full"><q-btn dense flat no-caps size="sm" label="Drop it…" @click="planDdl('drop-column', '', column)"></q-btn>
              <q-btn-dropdown dense flat no-caps size="sm" label="Copy into a field…"><q-list dense>
                <q-item v-for="field in entity.fields" :key="field.name" clickable v-close-popup @click="planDdl('copy-column', field.name, column)"><q-item-section>{{field.name}}</q-item-section></q-item></q-list></q-btn-dropdown></template>
          </div>
          <div v-if="full && view.table.exists" class="q-mt-sm row q-gutter-x-sm">
            <q-btn-dropdown dense flat no-caps size="sm" label="Change a column's type…"><q-list dense>
              <q-item v-for="field in entity.fields" :key="field.name" clickable v-close-popup @click="planDdl('change-type', field.name, '')"><q-item-section>{{field.name}} → {{(entity.sqlTypes || {})[field.name]}}</q-item-section></q-item></q-list></q-btn-dropdown>
            <q-btn dense flat no-caps size="sm" color="negative" label="Drop and create the table…" @click="planDdl('recreate-table', '', '')"></q-btn>
          </div>
          <div class="m-dev-grid-wrap q-mt-sm" v-if="view.table.columns.length"><table class="m-dev-grid"><thead><tr><th>Column</th><th>SQL type</th></tr></thead>
            <tbody><tr v-for="column in view.table.columns" :key="column.name"><td class="m-dev-mono">{{column.name}}</td><td>{{column.sqlType}}</td></tr></tbody></table></div>
        </template>
        <pre v-if="isView && viewSql" class="m-dev-mono q-pa-sm m-dev-sql" id="DevViewSql">{{viewSql}}</pre>
        <div class="text-subtitle2 q-mt-md">Data <span class="m-muted text-caption" v-if="view.data.count !== undefined">{{view.data.count}} rows</span>
          <a class="text-caption q-ml-sm" :href="dataEditUrl" id="DevDataEdit">Open in Data Edit</a></div>
        <div v-if="view.data.error" class="text-caption text-negative">{{view.data.error}}</div>
        <div class="m-dev-grid-wrap" v-if="(view.data.rows || []).length"><table class="m-dev-grid"><thead><tr><th v-for="name in Object.keys(view.data.rows[0])" :key="name">{{name}}</th></tr></thead>
          <tbody><tr v-for="(row, i) in view.data.rows" :key="i"><td v-for="name in Object.keys(view.data.rows[0])" :key="name" class="ellipsis" style="max-width: 14rem">{{row[name]}}</td></tr></tbody></table></div>
      </div>
      <div v-if="section === 'seed' && seed" id="DevEntitySeed">
        <div class="text-caption m-muted q-mb-xs">{{seed.path}}{{seed.missing ? ' (made on Save)' : ''}}: loaded after Save.</div>
        <div v-for="table in seedTables" :key="table.entity" class="q-mb-md">
          <div class="row items-center no-wrap"><span class="text-weight-medium">{{table.entity}}</span><q-space></q-space>
            <q-btn dense flat no-caps size="sm" icon="add" label="Add row" @click="addRecord(seed, table)"></q-btn></div>
          <div class="m-dev-grid-wrap"><table class="m-dev-grid"><thead><tr><th v-for="column in table.columns" :key="column">{{column}}</th><th></th></tr></thead>
            <tbody><tr v-for="row in table.rows" :key="row.path.join('.')">
              <td v-for="column in table.columns" :key="column"><input class="m-dev-cell" :value="row.values[column] || ''" @change="setRecord(seed, row, column, $event.target.value)"></td>
              <td><q-btn dense flat round size="xs" icon="delete" @click="removeRecord(seed, row)"></q-btn></td></tr></tbody></table></div>
        </div>
      </div>
    </template>
  </template>

  <q-dialog :model-value="!!adding" @update:model-value="v => { if (!v) adding = null; }">
    <q-card v-if="adding" style="width: 34rem; max-width: 95vw" id="DevAddFieldDialog">
      <q-card-section class="text-subtitle1">Add a field{{adding.after ? ' after ' + adding.after : ''}}</q-card-section>
      <q-card-section class="q-pt-none q-gutter-y-sm">
        <q-btn-toggle dense no-caps unelevated toggle-color="primary" v-model="adding.kind" :options="kinds.map(k => ({ label: k, value: k }))"
            @update:model-value="kindChanged" class="m-dev-kinds" id="DevFieldKind"></q-btn-toggle>
        <q-input dense outlined v-model="adding.words" label="What it holds (Due date)" @update:model-value="nameFromWords" autofocus for="DevFieldWords"></q-input>
        <q-input dense outlined v-model="adding.name" label="Field name" for="DevFieldName"></q-input>
        <q-select v-if="adding.kind === 'Other'" dense outlined v-model="adding.type" :options="view.fieldTypes.map(t => t.type)" label="Type"></q-select>
        <q-select v-if="adding.kind === 'Reference'" dense outlined use-input fill-input hide-selected v-model="adding.related" :options="entityOptions"
            @filter="searchEntities" label="The entity it refers to"></q-select>
        <template v-if="adding.kind === 'Picklist'">
          <q-input dense outlined v-model="adding.enumTypeId" @update:model-value="adding.typeEdited = true" label="Enumeration type" hint="A new type is written to the seed data; an existing one is used as it is"></q-input>
          <q-input dense outlined type="textarea" autogrow v-model="adding.values" label="Values, one a line" for="DevPicklistValues"></q-input>
        </template>
        <template v-if="adding.kind === 'Status'">
          <q-input dense outlined v-model="adding.statusTypeId" @update:model-value="adding.typeEdited = true" label="Status type"></q-input>
          <q-input dense outlined type="textarea" autogrow v-model="adding.items" label="Statuses in their order, one a line" hint="A flow from each to the next is written too"></q-input>
        </template>
        <q-input dense outlined v-model="adding.description" label="Description"></q-input>
        <div class="row q-gutter-x-sm">
          <q-input class="col-3" dense outlined v-model="adding.locale" label="Locale" placeholder="es"></q-input>
          <q-input class="col" dense outlined v-model="adding.localized" :label="'Its label in that language (' + (adding.title || titleOf(adding.name)) + ')'"></q-input>
        </div>
        <div class="row q-gutter-x-md"><q-checkbox dense v-model="adding.isPk" label="Primary key"></q-checkbox><q-checkbox dense v-model="adding.notNull" label="Not null"></q-checkbox></div>
      </q-card-section>
      <q-card-actions align="right"><q-btn flat no-caps label="Cancel" @click="adding = null"></q-btn>
        <q-btn unelevated no-caps color="primary" label="Add" @click="addField" id="DevFieldAdd"></q-btn></q-card-actions>
    </q-card>
  </q-dialog>
  <q-dialog :model-value="!!uses" @update:model-value="v => { if (!v) uses = null; }">
    <q-card v-if="uses" style="width: 30rem"><q-card-section class="text-subtitle1">Remove {{uses.field.name}}?</q-card-section>
      <q-card-section class="q-pt-none">{{uses.words.length ? 'It is used in ' + uses.words.join(' and ') + '.' : 'Nothing the index knows uses it.'}}
        <div class="text-caption m-muted q-mt-xs">Its column stays in the table; the full tier can drop it from Table.</div></q-card-section>
      <q-card-actions align="right"><q-btn flat no-caps label="Keep it" @click="uses = null"></q-btn><q-btn unelevated no-caps color="negative" label="Remove" @click="confirmRemove"></q-btn></q-card-actions></q-card>
  </q-dialog>
  <q-dialog :model-value="!!relAdding" @update:model-value="v => { if (!v) relAdding = null; }">
    <q-card v-if="relAdding" style="width: 30rem"><q-card-section class="text-subtitle1">Add a relationship</q-card-section>
      <q-card-section class="q-pt-none q-gutter-y-sm">
        <q-btn-toggle dense no-caps v-model="relAdding.type" :options="['one', 'many', 'one-nofk'].map(t => ({ label: t, value: t }))"></q-btn-toggle>
        <q-select dense outlined use-input fill-input hide-selected v-model="relAdding.related" :options="entityOptions" @filter="searchEntities" label="Related entity"></q-select>
        <q-input dense outlined v-model="relAdding.title" label="Title"></q-input>
        <div class="row q-gutter-x-sm"><q-input class="col" dense outlined v-model="relAdding.fieldName" label="Field here"></q-input><q-input class="col" dense outlined v-model="relAdding.relatedField" label="Related field (its key when empty)"></q-input></div>
      </q-card-section>
      <q-card-actions align="right"><q-btn flat no-caps label="Cancel" @click="relAdding = null"></q-btn><q-btn unelevated no-caps color="primary" label="Add" @click="addRelationship"></q-btn></q-card-actions></q-card>
  </q-dialog>
  <q-dialog :model-value="!!indexAdding" @update:model-value="v => { if (!v) indexAdding = null; }">
    <q-card v-if="indexAdding" style="width: 26rem"><q-card-section class="text-subtitle1">Add an index</q-card-section>
      <q-card-section class="q-pt-none q-gutter-y-sm"><q-input dense outlined v-model="indexAdding.name" label="Name"></q-input>
        <q-select dense outlined multiple use-chips v-model="indexAdding.fields" :options="entity.fields.map(f => f.name)" label="Fields"></q-select>
        <q-checkbox dense v-model="indexAdding.unique" label="Unique"></q-checkbox></q-card-section>
      <q-card-actions align="right"><q-btn flat no-caps label="Cancel" @click="indexAdding = null"></q-btn><q-btn unelevated no-caps color="primary" label="Add" @click="addIndex"></q-btn></q-card-actions></q-card>
  </q-dialog>
  <q-dialog :model-value="!!memberAdding" @update:model-value="v => { if (!v) memberAdding = null; }">
    <q-card v-if="memberAdding" style="width: 30rem"><q-card-section class="text-subtitle1">Add a member</q-card-section>
      <q-card-section class="q-pt-none q-gutter-y-sm"><q-input dense outlined v-model="memberAdding.entityAlias" label="Alias"></q-input>
        <q-select dense outlined clearable v-model="memberAdding.joinFromAlias" :options="(entity.members || []).map(m => m.entityAlias)" label="Joined from" @update:model-value="memberJoinChanged"></q-select>
        <q-select v-if="memberAdding.relationships.length" dense outlined clearable v-model="memberAdding.relationship" :options="memberAdding.relationships" label="By its relationship (key maps implied)"></q-select>
        <q-select v-if="!memberAdding.relationship" dense outlined use-input fill-input hide-selected v-model="memberAdding.entityName" :options="entityOptions" @filter="searchEntities" label="Or any entity"></q-select>
        <q-checkbox dense v-model="memberAdding.joinOptional" label="Optional join"></q-checkbox></q-card-section>
      <q-card-actions align="right"><q-btn flat no-caps label="Cancel" @click="memberAdding = null"></q-btn><q-btn unelevated no-caps color="primary" label="Add" @click="addMember"></q-btn></q-card-actions></q-card>
  </q-dialog>
  <q-dialog :model-value="!!aliasAdding" @update:model-value="v => { if (!v) aliasAdding = null; }">
    <q-card v-if="aliasAdding" style="width: 30rem"><q-card-section class="text-subtitle1">Add an alias</q-card-section>
      <q-card-section class="q-pt-none q-gutter-y-sm"><q-checkbox dense v-model="aliasAdding.all" label="Every field of a member (alias-all)"></q-checkbox>
        <q-select dense outlined v-model="aliasAdding.entityAlias" :options="(entity.members || []).map(m => m.entityAlias)" label="Member"></q-select>
        <template v-if="aliasAdding.all"><q-input dense outlined v-model="aliasAdding.prefix" label="Prefix"></q-input><q-input dense outlined v-model="aliasAdding.exclude" label="Except (comma separated)"></q-input></template>
        <template v-else><q-input dense outlined v-model="aliasAdding.name" label="Name"></q-input><q-input dense outlined v-model="aliasAdding.field" label="Field (the name when empty)"></q-input>
          <q-select dense outlined clearable v-model="aliasAdding.function" :options="['min', 'max', 'sum', 'avg', 'count', 'count-distinct', 'upper', 'lower']" label="Function"></q-select></template></q-card-section>
      <q-card-actions align="right"><q-btn flat no-caps label="Cancel" @click="aliasAdding = null"></q-btn><q-btn unelevated no-caps color="primary" label="Add" @click="addAlias"></q-btn></q-card-actions></q-card>
  </q-dialog>
  <q-dialog :model-value="!!exporting" @update:model-value="v => { if (!v) exporting = null; }">
    <q-card v-if="exporting" style="width: 30rem"><q-card-section class="text-subtitle1">Export records into this file</q-card-section>
      <q-card-section class="q-pt-none q-gutter-y-sm">
        <q-select dense outlined use-input fill-input hide-selected v-model="exporting.entity" :options="entityOptions" @filter="searchEntities" @input-value="v => exporting.entity = v" label="Entity"></q-select>
        <q-input dense outlined v-model="exporting.filter" label="Only those with these values (JSON)" placeholder='{"enumTypeId": "ExamplePriority"}' class="m-dev-mono"></q-input>
      </q-card-section>
      <q-card-actions align="right"><q-btn flat no-caps label="Cancel" @click="exporting = null"></q-btn><q-btn unelevated no-caps color="primary" label="Export" @click="exportRecords" id="DevExportDo"></q-btn></q-card-actions></q-card>
  </q-dialog>
  <q-dialog :model-value="!!ddl" @update:model-value="v => { if (!v) ddl = null; }">
    <q-card v-if="ddl" style="width: 40rem; max-width: 95vw" id="DevDdlDialog"><q-card-section class="text-subtitle1 text-negative">This changes the dev database's table {{ddl.table}}</q-card-section>
      <q-card-section class="q-pt-none"><pre class="m-dev-mono q-ma-none" style="white-space: pre-wrap">{{ddl.statements.join(';\\n')}};</pre>
        <q-input dense outlined v-model="ddl.confirm" :label="'Type ' + ddl.table + ' to run it'" class="q-mt-sm" for="DevDdlConfirm"></q-input></q-card-section>
      <q-card-actions align="right"><q-btn flat no-caps label="Cancel" @click="ddl = null"></q-btn>
        <q-btn unelevated no-caps color="negative" label="Run" :loading="ddl.running" :disable="ddl.confirm.toLowerCase() !== String(ddl.table).toLowerCase()" @click="runDdl"></q-btn></q-card-actions></q-card>
  </q-dialog>
</div>`
    };
    entityView.computed.kinds = () => KINDS;
    entityView.methods.kindOf = function (field) { return ops.kindOf(field, this.entity && this.entity.relationships); };
    entityView.methods.titleOf = (name) => ops.titleOf(name);
    entityView.computed.pure = () => pure;
    entityView.computed.operators = () => ["equals", "not-equals", "less", "greater", "less-equals", "greater-equals", "in", "not-in", "between", "like", "not-like", "is-null", "is-not-null"];


    devAide.registerView({ name: "entity", label: "Entity", roots: ["entities", "entity-facade-xml"], component: "m-dev-entity-view",
        script: "/devstatic/DevAideEntity.js", ready: () => !!devAide.entityViewRegistered });
    const qvt = moqui.qvt;
    if (qvt && qvt.registerLate) qvt.registerLate("m-dev-entity-view", entityView);
    devAide.entityViewRegistered = true;
    devAide.components["m-dev-entity-view"] = entityView;
    if (root.document && !root.document.getElementById("m-dev-entity-style")) {
        const style = root.document.createElement("style");
        style.id = "m-dev-entity-style";
        style.textContent = ".m-dev-grid-wrap{overflow:auto;max-width:100%}.m-dev-grid{border-collapse:collapse;font-size:12px}" +
            ".m-dev-grid th{position:sticky;top:0;background:var(--m-surface-2,#f4f4f6);text-align:left;font-weight:500;padding:2px 6px;white-space:nowrap}" +
            ".body--dark .m-dev-grid th{background:#2a2a2e}.m-dev-grid td{padding:1px 6px;border-top:1px solid var(--m-border,rgba(0,0,0,.08));white-space:nowrap}" +
            ".m-dev-cell{font:inherit;border:1px solid transparent;background:transparent;color:inherit;padding:1px 3px;border-radius:3px}" +
            ".m-dev-cell:hover,.m-dev-cell:focus{border-color:var(--m-border,rgba(0,0,0,.2))}.m-dev-drag{cursor:grab;opacity:.5}" +
            ".m-dev-sql{white-space:pre-wrap;background:rgba(127,127,127,.08);border-radius:4px}.m-dev-note{border:1px solid var(--m-border,rgba(0,0,0,.12));border-radius:4px}.m-dev-kinds{flex-wrap:wrap}";
        root.document.head.appendChild(style);
    }
})(typeof window !== "undefined" ? window : globalThis);
