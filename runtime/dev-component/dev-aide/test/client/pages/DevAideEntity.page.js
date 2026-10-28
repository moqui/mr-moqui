/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// The entity view's operation builders in the page (moqui.devAide.entityOps from DevAideEntity.js). Each vector of
// test/vectors/entity-view-vectors.json has the outline the server's XML layer reads from its input; the builders make
// the vector's operations from it, and the dev-aide's Rust test applies those operations and checks the text, so the
// view's edits keep the file's comments and order.
"use strict";

const ops = moqui.devAide.entityOps;
const pure = moqui.devAide.pure;

test("the view's builders make the operations of the vectors", async () => {
    const response = await fetch("/files/runtime/dev-component/dev-aide/test/vectors/entity-view-vectors.json");
    const file = await response.json();
    const built = { picklist: ops.picklist(file.specs.picklist), status: ops.status(file.specs.status) };
    assert.ok(file.vectors.length >= 12);
    for (const vector of file.vectors) {
        const outline = vector.outline;
        const resolve = (arg) => {
            if (arg && typeof arg === "object" && !Array.isArray(arg)) {
                if (arg.$element) return pure.outlineAt(outline, arg.$element);
                if (arg.$path) return { path: arg.$path, children: [], attrs: [] };
                if (arg.$outline) return outline;
                if (arg.$picklist) return built.picklist[arg.$picklist];
                if (arg.$status) return built.status[arg.$status];
                if (arg.$row) return ops.recordTables(outline)[arg.$row[0]].rows[arg.$row[1] || 0];
                if (arg.$table !== undefined) return ops.recordTables(outline)[arg.$table];
            }
            return arg;
        };
        let list = [];
        for (const call of vector.calls) {
            const answer = ops[call.builder].apply(null, call.args.map(resolve));
            list = list.concat(call.take ? answer[call.take] : answer);
        }
        // as JSON: a builder may share a path between two operations, which the comparison would read as a cycle
        assert.deepEqual(JSON.parse(JSON.stringify(list)), vector.ops, vector.name);
    }
});

test("an entity's element is its definition or its extension", () => {
    const outline = { name: "entities", path: [], attrs: [], children: [
        { name: "entity", path: [0], attrs: [["entity-name", "Example"], ["package", "moqui.example"]], children: [] },
        { name: "extend-entity", path: [1], attrs: [["entity-name", "Party"], ["package", "mantle.party"]], children: [] }] };
    assert.equal(ops.findEntityElement(outline, "moqui.example.Example").node.path[0], 0);
    assert.equal(ops.findEntityElement(outline, "mantle.party.Party", ["extend-entity"]).kind, "extend-entity");
    assert.equal(ops.findEntityElement(outline, "moqui.example.Example", ["extend-entity"]), null);
    assert.deepEqual(ops.ensureExtend(outline, "mantle.party.Party").ops, []);
    assert.equal(ops.ensureExtend(outline, "moqui.basic.Enumeration").path[0], 2);
});

test("Add field names a field from words and types it by its kind", () => {
    assert.equal(ops.camel("Due date"), "dueDate");
    assert.equal(ops.camel(" ship-by  Date "), "shipByDate");
    assert.equal(ops.typeOfKind("Date-time"), "date-time");
    assert.equal(ops.typeOfKind("Yes/No"), "text-indicator");
    assert.equal(ops.kindOf({ name: "priorityEnumId", type: "id" }, []), "Picklist");
    assert.equal(ops.kindOf({ name: "statusId", type: "id" }, []), "Status");
    assert.equal(ops.kindOf({ name: "dueDate", type: "date" }, []), "Date");
    assert.equal(ops.kindOf({ name: "partyId", type: "id" }, [{ type: "one", keyMap: { partyId: "partyId" } }]), "Reference");
    assert.equal(ops.xml("a", [["x", "1 < 2 & \"3\""], ["y", ""], ["z", true]]), "<a x=\"1 &lt; 2 &amp; &quot;3&quot;\" z=\"true\"/>");
});

test("a data file's records are tables by entity", () => {
    const outline = { name: "entity-facade-xml", path: [], attrs: [["type", "seed"]], children: [
        { name: "moqui.basic.Enumeration", path: [0], attrs: [["enumId", "A"], ["enumTypeId", "T"]] },
        { name: "moqui.basic.Enumeration", path: [1], attrs: [["enumId", "B"], ["description", "Bee"]] },
        { name: "moqui.basic.EnumerationType", path: [2], attrs: [["enumTypeId", "T"]] }] };
    const tables = ops.recordTables(outline);
    assert.deepEqual(tables.map((table) => table.entity), ["moqui.basic.Enumeration", "moqui.basic.EnumerationType"]);
    assert.deepEqual(tables[0].columns, ["enumId", "enumTypeId", "description"]);
    assert.deepEqual(ops.setRecordValue(tables[0].rows[0], "description", ""), []);
    assert.deepEqual(ops.setRecordValue(tables[0].rows[1], "description", ""), [{ op: "remove-attribute", at: [1], name: "description" }]);
});

test("the fields shown are the server's with the buffer's element over them", () => {
    const server = [{ name: "widgetId", type: "id", isPk: true }, { name: "widgetName", type: "text-medium" }];
    const extend = { name: "extend-entity", path: [0], attrs: [], children: [
        { name: "field", path: [0, 0], attrs: [["name", "widgetName"], ["not-null", "true"]] },
        { name: "field", path: [0, 1], attrs: [["name", "dueDate"], ["type", "date"]] }] };
    const shown = ops.mergeFields(server, extend, false);
    assert.deepEqual(shown.map((field) => field.name), ["widgetId", "widgetName", "dueDate"]);
    assert.equal(shown[1].notNull, true);
    assert.equal(shown[1].type, "text-medium");
    assert.equal(shown[2].unsaved, true);
    // an override that names only not-null leaves the primary key as it is
    const pk = ops.mergeFields(server, { name: "extend-entity", path: [0], attrs: [], children: [
        { name: "field", path: [0, 0], attrs: [["name", "widgetId"], ["not-null", "true"]] }] }, false);
    assert.equal(pk[0].isPk, true);
    const entity = { name: "entity", path: [0], attrs: [], children: [
        { name: "field", path: [0, 0], attrs: [["name", "widgetName"], ["type", "text-long"]] },
        { name: "field", path: [0, 1], attrs: [["name", "widgetId"], ["type", "id"], ["is-pk", "true"]] }] };
    assert.deepEqual(ops.mergeFields(server, entity, true).map((field) => field.name + ":" + field.type), ["widgetName:text-long", "widgetId:id"]);
});
