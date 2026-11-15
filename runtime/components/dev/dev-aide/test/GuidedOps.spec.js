// The terminal guided editors' operation builders (service/org/moqui/devaide/GuidedOps.js) make the operations the
// browser views' builders make: the cases of the browser page specs (test/client/pages/DevAideEntity.page.js,
// DevAideService.page.js and DevAideScreen.page.js) and the entity view's vectors (test/vectors/entity-view-vectors.json,
// whose operations the dev-aide's Rust test applies), run here on the server's script engine.
// Run: ./moq test dev-aide --spec GuidedOps
const guided = () => ec.resource.script("component://dev-aide/service/org/moqui/devaide/GuidedOps.js")
// equal as data, whatever the order of an object's keys (the vectors file has its keys sorted), as deepEqual compares
const canon = (value) => Array.isArray(value) ? value.map(canon) : value && typeof value === "object"
    ? Object.fromEntries(Object.keys(value).sort().filter((key) => value[key] !== undefined).map((key) => [key, canon(value[key])])) : value
const same = (a, b) => JSON.stringify(canon(a)) === JSON.stringify(canon(b))
const el = (name, path, attrs, children, line, text) => ({name, path, attrs: attrs || [], children: children || [], line: line || 0, text})

feature("the entity builders make the operations of the vectors", {
    when() {
        const {entity: ops, pure} = guided()
        const file = JSON.parse(ec.resource.getLocationText("component://dev-aide/test/vectors/entity-view-vectors.json", false))
        const built = {picklist: ops.picklist(file.specs.picklist), status: ops.status(file.specs.status)}
        const differing = []
        for (const vector of file.vectors) {
            const outline = vector.outline
            const resolve = (arg) => {
                if (arg && typeof arg === "object" && !Array.isArray(arg)) {
                    if (arg.$element) return pure.outlineAt(outline, arg.$element)
                    if (arg.$path) return {path: arg.$path, children: [], attrs: []}
                    if (arg.$outline) return outline
                    if (arg.$picklist) return built.picklist[arg.$picklist]
                    if (arg.$status) return built.status[arg.$status]
                    if (arg.$row) return ops.recordTables(outline)[arg.$row[0]].rows[arg.$row[1] || 0]
                    if (arg.$table !== undefined) return ops.recordTables(outline)[arg.$table]
                }
                return arg
            }
            let list = []
            for (const call of vector.calls) {
                const answer = ops[call.builder].apply(null, call.args.map(resolve))
                list = list.concat(call.take ? answer[call.take] : answer)
            }
            if (!same(list, vector.ops)) differing.push(vector.name)
        }
        return {count: file.vectors.length, differing}
    },
    then({count, differing}) {
        count >= 12
        differing.join("; ") == ""
    }
})

feature("an entity's element, Add field's naming and kinds, and a data file's tables as the browser makes them", {
    when() {
        const {entity: ops} = guided()
        const outline = {name: "entities", path: [], attrs: [], children: [
            {name: "entity", path: [0], attrs: [["entity-name", "Example"], ["package", "moqui.example"]], children: []},
            {name: "extend-entity", path: [1], attrs: [["entity-name", "Party"], ["package", "mantle.party"]], children: []}]}
        const data = {name: "entity-facade-xml", path: [], attrs: [["type", "seed"]], children: [
            {name: "moqui.basic.Enumeration", path: [0], attrs: [["enumId", "A"], ["enumTypeId", "T"]]},
            {name: "moqui.basic.Enumeration", path: [1], attrs: [["enumId", "B"], ["description", "Bee"]]},
            {name: "moqui.basic.EnumerationType", path: [2], attrs: [["enumTypeId", "T"]]}]}
        const tables = ops.recordTables(data)
        const server = [{name: "widgetId", type: "id", isPk: true}, {name: "widgetName", type: "text-medium"}]
        const extend = {name: "extend-entity", path: [0], attrs: [], children: [
            {name: "field", path: [0, 0], attrs: [["name", "widgetName"], ["not-null", "true"]]},
            {name: "field", path: [0, 1], attrs: [["name", "dueDate"], ["type", "date"]]}]}
        const shown = ops.mergeFields(server, extend, false)
        const inEntity = {name: "entity", path: [0], attrs: [], children: [
            {name: "field", path: [0, 0], attrs: [["name", "widgetName"], ["type", "text-long"]]},
            {name: "field", path: [0, 1], attrs: [["name", "widgetId"], ["type", "id"], ["is-pk", "true"]]}]}
        return {ops, outline, tables, shown, inEntity, server}
    },
    then({ops, outline, tables, shown, inEntity, server}) {
        ops.findEntityElement(outline, "moqui.example.Example").node.path[0] == 0
        ops.findEntityElement(outline, "mantle.party.Party", ["extend-entity"]).kind == "extend-entity"
        ops.findEntityElement(outline, "moqui.example.Example", ["extend-entity"]) == null
        same(ops.ensureExtend(outline, "mantle.party.Party").ops, [])
        ops.ensureExtend(outline, "moqui.basic.Enumeration").path[0] == 2
        ops.camel("Due date") == "dueDate"
        ops.camel(" ship-by  Date ") == "shipByDate"
        ops.typeOfKind("Date-time") == "date-time"
        ops.typeOfKind("Yes/No") == "text-indicator"
        ops.kindOf({name: "priorityEnumId", type: "id"}, []) == "Picklist"
        ops.kindOf({name: "statusId", type: "id"}, []) == "Status"
        ops.kindOf({name: "dueDate", type: "date"}, []) == "Date"
        ops.kindOf({name: "partyId", type: "id"}, [{type: "one", keyMap: {partyId: "partyId"}}]) == "Reference"
        ops.xml("a", [["x", "1 < 2 & \"3\""], ["y", ""], ["z", true]]) == "<a x=\"1 &lt; 2 &amp; &quot;3&quot;\" z=\"true\"/>"
        same(tables.map((table) => table.entity), ["moqui.basic.Enumeration", "moqui.basic.EnumerationType"])
        same(tables[0].columns, ["enumId", "enumTypeId", "description"])
        same(ops.setRecordValue(tables[0].rows[0], "description", ""), [])
        same(ops.setRecordValue(tables[0].rows[1], "description", ""), [{op: "remove-attribute", at: [1], name: "description"}])
        same(shown.map((field) => field.name), ["widgetId", "widgetName", "dueDate"])
        shown[1].notNull === true
        shown[1].type == "text-medium"
        shown[2].unsaved === true
        same(ops.mergeFields(server, inEntity, true).map((field) => field.name + ":" + field.type), ["widgetName:text-long", "widgetId:id"])
        // the dialog's names, as the browser's nameFromWords makes them
        same(ops.fieldNaming("moqui.example.Example", "Picklist", "Priority"), {name: "priorityEnumId", type: "id", enumTypeId: "ExamplePriority", statusTypeId: ""})
        ops.fieldNaming("moqui.example.Example", "Reference", "owner party").name == "ownerPartyId"
        same(ops.fieldNaming("moqui.example.Example", "Status", ""), {name: "statusId", type: "id", enumTypeId: "", statusTypeId: "Example"})
        ops.fieldNaming("moqui.example.Example", "Date-time", "Shipped at").type == "date-time"
        same(ops.valuesOf("High, Low", "ExamplePriority"), [{id: "ExamplePriorityHigh", description: "High"}, {id: "ExamplePriorityLow", description: "Low"}])
    }
})

feature("a parameter, a SECA rule and a Try's parameters as the service writer writes them", {
    when() {
        const {service: ops} = guided()
        const service = el("service", [0], [["verb", "get"], ["noun", "Thing"]], [el("actions", [0, 0])])
        const withIn = el("service", [0], [], [el("in-parameters", [0, 0], [], [el("parameter", [0, 0, 0], [["name", "thingId"], ["required", "true"]])])])
        return {ops, service, withIn}
    },
    then({ops, service, withIn}) {
        same(ops.addParameter(service, "in-parameters", {name: "thingId", required: true}), [{op: "insert", at: [0, 0], position: "before",
            xml: "<in-parameters><parameter name=\"thingId\" required=\"true\"/></in-parameters>"}])
        ops.secaXml("create#Thing", "post-service") == "<seca service=\"create#Thing\" when=\"post-service\"><actions></actions></seca>"
        same(ops.tryParameters({a: "5", b: "x", c: "", d: "true"},
            [{name: "a", type: "Integer"}, {name: "b", type: "String"}, {name: "c", type: "String"}, {name: "d", type: "Boolean"}]), {a: 5, b: "x", d: true})
        same(ops.servicesOf(el("services", [], [], [service])).map((entry) => entry.name), ["get#Thing"])
        // the terminal's own: Remove and Edit of a parameter, and a rule with its condition and call
        same(ops.removeParameter(withIn, "in-parameters", "thingId"), [{op: "remove", at: [0, 0, 0]}])
        same(ops.editParameter(withIn, "in-parameters", "thingId", {type: "Long", required: false, description: "The thing"}), [
            {op: "set-attribute", at: [0, 0, 0], name: "type", value: "Long"}, {op: "remove-attribute", at: [0, 0, 0], name: "required"},
            {op: "insert", at: [0, 0, 0], position: "last", xml: "<description>The thing</description>"}])
        ops.secaXml("create#Thing", "post-service", "thingId != null", "my.Svc.note#Thing") ==
            "<seca service=\"create#Thing\" when=\"post-service\"><condition><expression>thingId != null</expression></condition><actions><service-call name=\"my.Svc.note#Thing\" in-map=\"context\"/></actions></seca>"
    }
})

// the screen of DevAideScreen.page.js: widgets with a named box holding a list, an unnamed container, a named one, a form with a layout
const screenOutline = () => el("screen", [], [], [
    el("transition", [0], [["name", "save"]]),
    el("widgets", [1], [], [
        el("container-box", [1, 0], [["name", "WidgetBox"]], [el("form-list", [1, 0, 0], [["name", "WidgetList"]], [
            el("field", [1, 0, 0, 0], [["name", "widgetId"]]), el("field", [1, 0, 0, 1], [["name", "widgetName"]], [], 9)])], 4),
        el("container", [1, 1], [], [el("label", [1, 1, 0], [["text", "x"]])], 12),
        el("container", [1, 2], [["name", "Notes"]], [], 14),
        el("form-single", [1, 3], [["name", "WidgetForm"]], [
            el("field", [1, 3, 0], [["name", "a"]]), el("field", [1, 3, 1], [["name", "b"]]),
            el("field-layout", [1, 3, 2], [], [el("field-row", [1, 3, 2, 0], [], [el("field-ref", [1, 3, 2, 0, 0], [["name", "a"]]),
                el("field-ref", [1, 3, 2, 0, 1], [["name", "b"]])])])])
    ])
])

feature("the screen builder's anchors and intents as the browser makes them", {
    when() {
        const {screen: ops, pure} = guided()
        const screen = screenOutline()
        const points = ops.extensionPoints(screen)
        const empty = el("screen-extend", [], [], [])
        const has = el("screen-extend", [], [], [el("form-list", [0], [["name", "WidgetList"]])])
        const field = {name: "dueNote", title: "Due note", widget: "display", after: "widgetName"}
        const placed = ops.insertFormField(ops.formOf(screen, [1, 3]), {name: "c", title: "C", widget: "text-line"}, "a")
        const columns = el("form-list", [0], [["name", "L"]], [el("field", [0, 0], [["name", "a"]]),
            el("form-list-column", [0, 1], [], [el("field-ref", [0, 1, 0], [["name", "a"]])])])
        const text = "<screen>\n    <transition name=\"save\"><actions/></transition>\n</screen>\n"
        const copied = {name: "transition", path: [0], attrs: [["name", "save"]], from16: text.indexOf("<transition"), to16: text.indexOf("</screen>") - 1}
        const actions = el("actions", [0], [], [el("entity-find", [0, 0], [["entity-name", "W"], ["list", "wList"]], [el("search-form-inputs", [0, 0, 0])])])
        const listScreen = el("screen", [], [], [actions, el("widgets", [1], [], [el("form-list", [1, 0], [["name", "L"], ["list", "wList"]], [el("field", [1, 0, 0], [["name", "a"]])])])])
        const form = listScreen.children[1].children[0]
        const button = ops.buttonOps(false, listScreen, {transition: "approve", service: "a.B.approve#C", text: "Approve"},
            () => [{op: "insert", at: [1, 0], position: "after", xml: "<link/>"}], el("label", [1, 0], [["text", "x"]]))
        return {ops, pure, screen, points, empty, has, field, placed, columns, text, copied, listScreen, form, button}
    },
    then({ops, pure, screen, points, empty, has, field, placed, columns, text, copied, listScreen, form, button}) {
        same(points.filter((point) => point.named).map((point) => point.kind + " " + point.name),
            ["container-box WidgetBox", "form-list WidgetList", "container Notes", "form-single WidgetForm", "transition save"])
        same(points.filter((point) => !point.named).map((point) => [point.kind, point.nearest.name, point.nearest.where]), [["container", "WidgetBox", "after"]])
        ops.nearestAnchor(screen, [1, 1, 0]).name == "WidgetBox"
        ops.nodeAtLine(screen, 14).path.join(".") == "1.2"
        ops.formOf(screen, [1, 0, 0, 1]).path.join(".") == "1.0.0"
        pure.attrOf(ops.fieldOf(screen, [1, 0, 0, 1]), "name") == "widgetName"
        same(ops.extendFormField(empty, "form-list", "WidgetList", field), [{op: "insert", at: [], position: "last",
            xml: "<form-list name=\"WidgetList\"><field name=\"dueNote\" after=\"widgetName\"><default-field title=\"Due note\"><display/></default-field></field></form-list>"}])
        ops.extendFormField(has, "form-list", "WidgetList", field)[0].at.join(".") == "0"
        same(ops.extendWidgets(empty, "Notes", "after", "<label text=\"x\"/>"), [{op: "insert", at: [], position: "last",
            xml: "<widgets-extend name=\"Notes\" where=\"after\"><label text=\"x\"/></widgets-extend>"}])
        same(placed.map((op) => [op.at.join("."), op.position]), [["1.3.0", "after"], ["1.3.2.0.0", "after"]])
        placed[1].xml == "<field-ref name=\"c\"/>"
        same(ops.insertFormField(columns, {name: "b"}, "a")[1], {op: "insert", at: [0, 1], position: "after", xml: "<form-list-column><field-ref name=\"b\"/></form-list-column>"})
        same(ops.copyInto(empty, text, copied), [{op: "insert", at: [], position: "last", xml: "<transition name=\"save\"><actions/></transition>"}])
        ops.actionsExtend(empty, "before")[0].xml == "<actions-extend type=\"actions\" when=\"before\"><log level=\"info\" message=\"before the screen's actions\"/></actions-extend>"
        ops.hasSearchInputs(ops.listFindOf(listScreen, "wList"))
        ops.listFindOf(listScreen, "other") == null
        same(ops.filterOps(true, listScreen, form, "a"), [{op: "insert", at: [1, 0, 0], position: "first", xml: "<header-field show-order-by=\"true\"><text-find/></header-field>"}])
        same(ops.filterOps(false, empty, form, "a"), [{op: "insert", at: [], position: "last",
            xml: "<form-list name=\"L\"><field name=\"a\"><header-field show-order-by=\"true\"><text-find/></header-field></field></form-list>"}])
        ops.sectionXml({kind: "box", name: "Notes", title: "Notes"}) == "<container-box name=\"Notes\"><box-header title=\"Notes\"/><box-body></box-body></container-box>"
        ops.sectionXml({kind: "related", name: "Lines", title: "Lines", entity: "mantle.order.OrderItem", keyField: "orderId"})
            .startsWith("<section name=\"Lines\"><actions><entity-find entity-name=\"mantle.order.OrderItem\" list=\"orderItemList\"><econdition field-name=\"orderId\" from=\"orderId\"/>")
        same(button.map((op) => [op.at.join("."), op.position]), [["0", "before"], ["2.0", "after"]])
        button[0].xml == "<transition name=\"approve\"><service-call name=\"a.B.approve#C\" in-map=\"true\"/><default-response url=\".\"/></transition>"
        ops.rowActionXml({name: "approveRow", transition: "approve", text: "Approve", keyField: "orderId"}) ==
            "<field name=\"approveRow\"><default-field><link url=\"approve\" text=\"Approve\" link-type=\"anchor\" parameter-map=\"{orderId: orderId}\"/></default-field></field>"
        ops.textAttribute(el("label", [0])) == "text"
        ops.textAttribute(el("form-list", [0])) == null
        // the terminal's own: a move as Alt+Up and Alt+Down make it, and the outline as tree rows with its anchors marked
        same(ops.stepOp(screen, [1, 1], -1), [{op: "move", at: [1, 1], to: [1, 0], position: "before"}])
        same(ops.stepOp(screen, [1, 3], 1), [])
        ops.outlineRows(screen).find((row) => row.key == "1.0").label == "⚓ container-box WidgetBox"
        ops.outlineRows(screen).find((row) => row.key == "1.0.0.1").parent == "1.0.0"
    }
})
