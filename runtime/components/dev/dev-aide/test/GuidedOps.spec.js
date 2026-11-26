// The terminal guided editors' operation builders (service/org/moqui/devaide/GuidedOps.js) make the operations the
// browser views' builders make: the cases of the browser page specs (test/client/pages/DevAideEntity.page.js,
// DevAideService.page.js and DevAideScreen.page.js) and the entity view's vectors (test/vectors/entity-view-vectors.json,
// whose operations the dev-aide's Rust test applies), run here on the server's script engine.
// Run: ./moq test dev-aide --spec GuidedOps
const guided = () => ec.resource.script("component://dev-aide/service/org/moqui/devaide/GuidedOps.js")
// equal as data, whatever the order of an object's keys (the vectors file has its keys sorted), as deepEqual compares
const canon = value =>
    Array.isArray(value)
        ? value.map(canon)
        : value && typeof value === "object"
          ? Object.fromEntries(
                Object.keys(value)
                    .sort()
                    .filter(key => value[key] !== undefined)
                    .map(key => [key, canon(value[key])])
            )
          : value
const same = (a, b) => JSON.stringify(canon(a)) === JSON.stringify(canon(b))
const el = (name, path, attrs, children, line, text) => ({
    name,
    path,
    attrs: attrs || [],
    children: children || [],
    line: line || 0,
    text
})

feature("the entity builders make the operations of the vectors", {
    when() {
        const {entity: ops, pure} = guided()
        const file = JSON.parse(ec.resource.getLocationText("component://dev-aide/test/vectors/entity-view-vectors.json", false))
        const built = {picklist: ops.picklist(file.specs.picklist), status: ops.status(file.specs.status)}
        const differing = []
        for (const vector of file.vectors) {
            const outline = vector.outline
            const resolve = arg => {
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
        const outline = {
            name: "entities",
            path: [],
            attrs: [],
            children: [
                {
                    name: "entity",
                    path: [0],
                    attrs: [
                        ["entity-name", "Example"],
                        ["package", "moqui.example"]
                    ],
                    children: []
                },
                {
                    name: "extend-entity",
                    path: [1],
                    attrs: [
                        ["entity-name", "Party"],
                        ["package", "mantle.party"]
                    ],
                    children: []
                }
            ]
        }
        const data = {
            name: "entity-facade-xml",
            path: [],
            attrs: [["type", "seed"]],
            children: [
                {
                    name: "moqui.basic.Enumeration",
                    path: [0],
                    attrs: [
                        ["enumId", "A"],
                        ["enumTypeId", "T"]
                    ]
                },
                {
                    name: "moqui.basic.Enumeration",
                    path: [1],
                    attrs: [
                        ["enumId", "B"],
                        ["description", "Bee"]
                    ]
                },
                {name: "moqui.basic.EnumerationType", path: [2], attrs: [["enumTypeId", "T"]]}
            ]
        }
        const tables = ops.recordTables(data)
        const server = [
            {name: "widgetId", type: "id", isPk: true},
            {name: "widgetName", type: "text-medium"}
        ]
        const extend = {
            name: "extend-entity",
            path: [0],
            attrs: [],
            children: [
                {
                    name: "field",
                    path: [0, 0],
                    attrs: [
                        ["name", "widgetName"],
                        ["not-null", "true"]
                    ]
                },
                {
                    name: "field",
                    path: [0, 1],
                    attrs: [
                        ["name", "dueDate"],
                        ["type", "date"]
                    ]
                }
            ]
        }
        const shown = ops.mergeFields(server, extend, false)
        const inEntity = {
            name: "entity",
            path: [0],
            attrs: [],
            children: [
                {
                    name: "field",
                    path: [0, 0],
                    attrs: [
                        ["name", "widgetName"],
                        ["type", "text-long"]
                    ]
                },
                {
                    name: "field",
                    path: [0, 1],
                    attrs: [
                        ["name", "widgetId"],
                        ["type", "id"],
                        ["is-pk", "true"]
                    ]
                }
            ]
        }
        return {ops, outline, data, tables, shown, inEntity, server}
    },
    then({ops, outline, data, tables, shown, inEntity, server}) {
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
        ops.xml("a", [
            ["x", '1 < 2 & "3"'],
            ["y", ""],
            ["z", true]
        ]) == '<a x="1 &lt; 2 &amp; &quot;3&quot;" z="true"/>'
        same(
            tables.map(table => table.entity),
            ["moqui.basic.Enumeration", "moqui.basic.EnumerationType"]
        )
        same(tables[0].columns, ["enumId", "enumTypeId", "description"])
        same(ops.setRecordValue(tables[0].rows[0], "description", ""), [])
        same(ops.setRecordValue(tables[0].rows[1], "description", ""), [{op: "remove-attribute", at: [1], name: "description"}])
        // the terminal's Edit of a record: only what changed, an empty value removed, one not posted kept
        same(ops.setRecordValues(tables[0].rows[1], {enumId: "B", enumTypeId: "T", description: ""}), [
            {op: "set-attribute", at: [1], name: "enumTypeId", value: "T"},
            {op: "remove-attribute", at: [1], name: "description"}
        ])
        same(ops.setRecordValues(tables[0].rows[0], {enumId: "A", description: null}), [])
        ops.recordAt(tables, "2").table.entity == "moqui.basic.EnumerationType"
        ops.recordAt(tables, "1").row.values.description == "Bee"
        ops.recordAt(tables, "3") == null
        same(ops.addRecord(data, tables[1], {enumTypeId: "U"}), [
            {op: "insert", at: [2], position: "after", xml: '<moqui.basic.EnumerationType enumTypeId="U"/>'}
        ])
        same(
            shown.map(field => field.name),
            ["widgetId", "widgetName", "dueDate"]
        )
        shown[1].notNull === true
        shown[1].type == "text-medium"
        shown[2].unsaved === true
        same(
            ops.mergeFields(server, inEntity, true).map(field => field.name + ":" + field.type),
            ["widgetName:text-long", "widgetId:id"]
        )
        // the dialog's names, as the browser's nameFromWords makes them
        same(ops.fieldNaming("moqui.example.Example", "Picklist", "Priority"), {
            name: "priorityEnumId",
            type: "id",
            enumTypeId: "ExamplePriority",
            statusTypeId: ""
        })
        ops.fieldNaming("moqui.example.Example", "Reference", "owner party").name == "ownerPartyId"
        same(ops.fieldNaming("moqui.example.Example", "Status", ""), {
            name: "statusId",
            type: "id",
            enumTypeId: "",
            statusTypeId: "Example"
        })
        ops.fieldNaming("moqui.example.Example", "Date-time", "Shipped at").type == "date-time"
        same(ops.valuesOf("High, Low", "ExamplePriority"), [
            {id: "ExamplePriorityHigh", description: "High"},
            {id: "ExamplePriorityLow", description: "Low"}
        ])
        // a Picklist's type is described by its words; a Status's with none by its id's words
        const extension = {path: [0], children: [], attrs: []}
        ops.addFieldSpec(extension, "moqui.example.Example", {kind: "Picklist", words: "contact channel", values: "Mail", extension: true})
            .seed.some(xml => xml.includes('enumTypeId="ExampleContactChannel"') && xml.includes('description="Contact channel"'))
        ops.addFieldSpec(extension, "moqui.example.Example", {kind: "Status", words: "", name: "reviewStatusId", statusTypeId: "ExampleReviewStatus", values: "Open, Done"})
            .seed[0].includes('description="Review Status"')
        // an upstream entity's extension takes no primary key; a related entity or values only where the kind takes them
        ops.addFieldSpec(extension, "moqui.example.Example", {kind: "Text", words: "code", isPk: true, extension: true}).refused.includes("primary key")
        !ops.addFieldSpec(extension, "moqui.example.Example", {kind: "Text", words: "code", isPk: true, extension: false}).refused
        ops.addFieldSpec(extension, "moqui.example.Example", {kind: "Text", words: "code", related: "moqui.example.ExampleItem"}).refused.includes("Reference")
        ops.addFieldSpec(extension, "moqui.example.Example", {kind: "Number", words: "size", values: "1, 2"}).refused.includes("Picklist")
    }
})

feature("a parameter, a SECA rule and a Try's parameters as the service writer writes them", {
    when() {
        const {service: ops} = guided()
        const service = el(
            "service",
            [0],
            [
                ["verb", "get"],
                ["noun", "Thing"]
            ],
            [el("actions", [0, 0])]
        )
        const withIn = el(
            "service",
            [0],
            [],
            [
                el(
                    "in-parameters",
                    [0, 0],
                    [],
                    [
                        el(
                            "parameter",
                            [0, 0, 0],
                            [
                                ["name", "thingId"],
                                ["required", "true"]
                            ]
                        )
                    ]
                )
            ]
        )
        // nested: order (a Map with a description and a validation) holding items (a List holding nothing yet), and an
        // empty Map
        const nested = el(
            "service",
            [0],
            [],
            [
                el(
                    "in-parameters",
                    [0, 0],
                    [],
                    [
                        el(
                            "parameter",
                            [0, 0, 0],
                            [
                                ["name", "order"],
                                ["type", "Map"]
                            ],
                            [
                                el("description", [0, 0, 0, 0], [], [], 0, "The order"),
                                el(
                                    "parameter",
                                    [0, 0, 0, 1],
                                    [
                                        ["name", "items"],
                                        ["type", "List"]
                                    ]
                                ),
                                el("matches", [0, 0, 0, 2], [["regexp", "x"]])
                            ]
                        ),
                        el(
                            "parameter",
                            [0, 0, 1],
                            [
                                ["name", "extra"],
                                ["type", "Map"]
                            ],
                            [el("description", [0, 0, 1, 0], [], [], 0, "More")]
                        ),
                        el("parameter", [0, 0, 2], [["name", "plain"]], [el("matches", [0, 0, 2, 0], [["regexp", "y"]])])
                    ]
                )
            ]
        )
        return {ops, service, withIn, nested}
    },
    then({ops, service, withIn, nested}) {
        same(ops.addParameter(service, "in-parameters", {name: "thingId", required: true}), [
            {
                op: "insert",
                at: [0, 0],
                position: "before",
                xml: '<in-parameters><parameter name="thingId" required="true"/></in-parameters>'
            }
        ])
        ops.secaXml("create#Thing", "post-service") ==
            '<seca service="create#Thing" when="post-service"><actions></actions></seca>'
        same(
            ops.tryParameters({a: "5", b: "x", c: "", d: "true"}, [
                {name: "a", type: "Integer"},
                {name: "b", type: "String"},
                {name: "c", type: "String"},
                {name: "d", type: "Boolean"}
            ]),
            {a: 5, b: "x", d: true}
        )
        same(
            ops.servicesOf(el("services", [], [], [service])).map(entry => entry.name),
            ["get#Thing"]
        )
        // Remove and Edit of a parameter (the browser view's too, DevAideService.page.js), and the terminal's own rule
        // with its condition and call
        same(ops.removeParameter(withIn, "in-parameters", "thingId"), [{op: "remove", at: [0, 0, 0]}])
        same(ops.editParameter(withIn, "in-parameters", "thingId", {type: "Long", required: false, description: "The thing"}), [
            {op: "set-attribute", at: [0, 0, 0], name: "type", value: "Long"},
            {op: "remove-attribute", at: [0, 0, 0], name: "required"},
            {op: "insert", at: [0, 0, 0], position: "last", xml: "<description>The thing</description>"}
        ])
        // nested parameters (as the browser view's, DevAideService.page.js): found by their path of names, added inside
        // a parent after its parameters and before its validations, removed and edited by path
        ops.parameterNode(nested, "in-parameters", "order.items").path.join(".") == "0.0.0.1"
        ops.parameterNode(nested, "in-parameters", ["order", "items"]).path.join(".") == "0.0.0.1"
        ops.parameterNode(nested, "in-parameters", "items") == null
        ops.parameterNode(nested, "in-parameters", "") == null
        same(ops.addParameter(nested, "in-parameters", {name: "quantity", type: "BigDecimal"}, "order"), [
            {op: "insert", at: [0, 0, 0, 1], position: "after", xml: '<parameter name="quantity" type="BigDecimal"/>'}
        ])
        same(ops.addParameter(nested, "in-parameters", {name: "sku"}, "order.items"), [
            {op: "insert", at: [0, 0, 0, 1], position: "last", xml: '<parameter name="sku"/>'}
        ])
        same(ops.addParameter(nested, "in-parameters", {name: "note"}, "extra"), [
            {op: "insert", at: [0, 0, 1], position: "last", xml: '<parameter name="note"/>'}
        ])
        same(ops.addParameter(nested, "in-parameters", {name: "inner"}, "plain"), [
            {op: "insert", at: [0, 0, 2, 0], position: "before", xml: '<parameter name="inner"/>'}
        ])
        same(ops.addParameter(nested, "in-parameters", {name: "x"}, "order.nothing"), [])
        same(ops.removeParameter(nested, "in-parameters", "order.items"), [{op: "remove", at: [0, 0, 0, 1]}])
        same(
            ops.editParameter(nested, "in-parameters", "order.items", {type: "List", required: true, description: "Its lines"}),
            [
                {op: "set-attribute", at: [0, 0, 0, 1], name: "type", value: "List"},
                {op: "set-attribute", at: [0, 0, 0, 1], name: "required", value: "true"},
                {op: "insert", at: [0, 0, 0, 1], position: "last", xml: "<description>Its lines</description>"}
            ]
        )
        // a description goes first in a parameter that holds others
        same(ops.editParameter(nested, "in-parameters", "plain", {description: "Plain"}).pop(), {
            op: "insert",
            at: [0, 0, 2],
            position: "first",
            xml: "<description>Plain</description>"
        })
        // the refusals the browser view makes too (DevAideService.page.js)
        ops.parameterRefusal(nested, "in-parameters", {mode: "add", parent: "order", name: "items"}).includes("already")
        ops.parameterRefusal(nested, "in-parameters", {mode: "add", parent: "order", name: "plain"}) == null
        ops.parameterRefusal(nested, "in-parameters", {mode: "edit", path: "order", type: "String"}).includes("holds parameters")
        ops.parameterRefusal(nested, "in-parameters", {mode: "edit", path: "extra", type: "String"}) == null
        ops.secaXml("create#Thing", "post-service", "thingId != null", "my.Svc.note#Thing") ==
            '<seca service="create#Thing" when="post-service"><condition><expression>thingId != null</expression></condition><actions><service-call name="my.Svc.note#Thing" in-map="context"/></actions></seca>'
    }
})

// the screen of DevAideScreen.page.js: widgets with a named box holding a list, an unnamed container, a named one, a form with a layout
const screenOutline = () =>
    el(
        "screen",
        [],
        [],
        [
            el("transition", [0], [["name", "save"]]),
            el(
                "widgets",
                [1],
                [],
                [
                    el(
                        "container-box",
                        [1, 0],
                        [["name", "WidgetBox"]],
                        [
                            el(
                                "form-list",
                                [1, 0, 0],
                                [["name", "WidgetList"]],
                                [
                                    el("field", [1, 0, 0, 0], [["name", "widgetId"]]),
                                    el("field", [1, 0, 0, 1], [["name", "widgetName"]], [], 9)
                                ]
                            )
                        ],
                        4
                    ),
                    el("container", [1, 1], [], [el("label", [1, 1, 0], [["text", "x"]])], 12),
                    el("container", [1, 2], [["name", "Notes"]], [], 14),
                    el(
                        "form-single",
                        [1, 3],
                        [["name", "WidgetForm"]],
                        [
                            el("field", [1, 3, 0], [["name", "a"]]),
                            el("field", [1, 3, 1], [["name", "b"]]),
                            el(
                                "field-layout",
                                [1, 3, 2],
                                [],
                                [
                                    el(
                                        "field-row",
                                        [1, 3, 2, 0],
                                        [],
                                        [
                                            el("field-ref", [1, 3, 2, 0, 0], [["name", "a"]]),
                                            el("field-ref", [1, 3, 2, 0, 1], [["name", "b"]])
                                        ]
                                    )
                                ]
                            )
                        ]
                    )
                ]
            )
        ]
    )

feature("the screen builder's anchors and intents as the browser makes them", {
    when() {
        const {screen: ops, pure} = guided()
        const screen = screenOutline()
        const points = ops.extensionPoints(screen)
        const empty = el("screen-extend", [], [], [])
        const has = el("screen-extend", [], [], [el("form-list", [0], [["name", "WidgetList"]])])
        const field = {name: "dueNote", title: "Due note", widget: "display", after: "widgetName"}
        const placed = ops.insertFormField(ops.formOf(screen, [1, 3]), {name: "c", title: "C", widget: "text-line"}, "a")
        const columns = el(
            "form-list",
            [0],
            [["name", "L"]],
            [
                el("field", [0, 0], [["name", "a"]]),
                el("form-list-column", [0, 1], [], [el("field-ref", [0, 1, 0], [["name", "a"]])])
            ]
        )
        const text = '<screen>\n    <transition name="save"><actions/></transition>\n</screen>\n'
        const copied = {
            name: "transition",
            path: [0],
            attrs: [["name", "save"]],
            from16: text.indexOf("<transition"),
            to16: text.indexOf("</screen>") - 1
        }
        const actions = el(
            "actions",
            [0],
            [],
            [
                el(
                    "entity-find",
                    [0, 0],
                    [
                        ["entity-name", "W"],
                        ["list", "wList"]
                    ],
                    [el("search-form-inputs", [0, 0, 0])]
                )
            ]
        )
        const listScreen = el(
            "screen",
            [],
            [],
            [
                actions,
                el(
                    "widgets",
                    [1],
                    [],
                    [
                        el(
                            "form-list",
                            [1, 0],
                            [
                                ["name", "L"],
                                ["list", "wList"]
                            ],
                            [el("field", [1, 0, 0], [["name", "a"]])]
                        )
                    ]
                )
            ]
        )
        const form = listScreen.children[1].children[0]
        const button = ops.buttonOps(
            false,
            listScreen,
            {transition: "approve", service: "a.B.approve#C", text: "Approve"},
            () => [{op: "insert", at: [1, 0], position: "after", xml: "<link/>"}],
            el("label", [1, 0], [["text", "x"]])
        )
        return {ops, pure, screen, points, empty, has, field, placed, columns, text, copied, listScreen, form, button}
    },
    then({ops, pure, screen, points, empty, has, field, placed, columns, text, copied, listScreen, form, button}) {
        same(
            points.filter(point => point.named).map(point => point.kind + " " + point.name),
            ["container-box WidgetBox", "form-list WidgetList", "container Notes", "form-single WidgetForm", "transition save"]
        )
        same(
            points.filter(point => !point.named).map(point => [point.kind, point.nearest.name, point.nearest.where]),
            [["container", "WidgetBox", "after"]]
        )
        ops.nearestAnchor(screen, [1, 1, 0]).name == "WidgetBox"
        ops.nodeAtLine(screen, 14).path.join(".") == "1.2"
        ops.formOf(screen, [1, 0, 0, 1]).path.join(".") == "1.0.0"
        pure.attrOf(ops.fieldOf(screen, [1, 0, 0, 1]), "name") == "widgetName"
        same(ops.extendFormField(empty, "form-list", "WidgetList", field), [
            {
                op: "insert",
                at: [],
                position: "last",
                xml: '<form-list name="WidgetList"><field name="dueNote" after="widgetName"><default-field title="Due note"><display/></default-field></field></form-list>'
            }
        ])
        ops.extendFormField(has, "form-list", "WidgetList", field)[0].at.join(".") == "0"
        same(ops.extendWidgets(empty, "Notes", "after", '<label text="x"/>'), [
            {
                op: "insert",
                at: [],
                position: "last",
                xml: '<widgets-extend name="Notes" where="after"><label text="x"/></widgets-extend>'
            }
        ])
        same(
            placed.map(op => [op.at.join("."), op.position]),
            [
                ["1.3.0", "after"],
                ["1.3.2.0.0", "after"]
            ]
        )
        placed[1].xml == '<field-ref name="c"/>'
        same(ops.insertFormField(columns, {name: "b"}, "a")[1], {
            op: "insert",
            at: [0, 1],
            position: "after",
            xml: '<form-list-column><field-ref name="b"/></form-list-column>'
        })
        same(ops.copyInto(empty, text, copied), [
            {op: "insert", at: [], position: "last", xml: '<transition name="save"><actions/></transition>'}
        ])
        ops.actionsExtend(empty, "before")[0].xml ==
            '<actions-extend type="actions" when="before"><log level="info" message="before the screen\'s actions"/></actions-extend>'
        ops.hasSearchInputs(ops.listFindOf(listScreen, "wList"))
        ops.listFindOf(listScreen, "other") == null
        same(ops.filterOps(true, listScreen, form, "a"), [
            {
                op: "insert",
                at: [1, 0, 0],
                position: "first",
                xml: '<header-field show-order-by="true"><text-find/></header-field>'
            }
        ])
        same(ops.filterOps(false, empty, form, "a"), [
            {
                op: "insert",
                at: [],
                position: "last",
                xml: '<form-list name="L"><field name="a"><header-field show-order-by="true"><text-find/></header-field></field></form-list>'
            }
        ])
        ops.sectionXml({kind: "box", name: "Notes", title: "Notes"}) ==
            '<container-box name="Notes"><box-header title="Notes"/><box-body></box-body></container-box>'
        ops.sectionXml({
            kind: "related",
            name: "Lines",
            title: "Lines",
            entity: "mantle.order.OrderItem",
            keyField: "orderId"
        }).startsWith(
            '<section name="Lines"><actions><entity-find entity-name="mantle.order.OrderItem" list="orderItemList"><econdition field-name="orderId" from="orderId"/>'
        )
        same(
            button.map(op => [op.at.join("."), op.position]),
            [
                ["0", "before"],
                ["2.0", "after"]
            ]
        )
        button[0].xml ==
            '<transition name="approve"><service-call name="a.B.approve#C" in-map="true"/><default-response url="."/></transition>'
        ops.rowActionXml({name: "approveRow", transition: "approve", text: "Approve", keyField: "orderId"}) ==
            '<field name="approveRow"><default-field><link url="approve" text="Approve" link-type="anchor" parameter-map="{orderId: orderId}"/></default-field></field>'
        ops.textAttribute(el("label", [0])) == "text"
        ops.textAttribute(el("form-list", [0])) == null
        // the terminal's own: a move as Alt+Up and Alt+Down make it, and the outline as tree rows with its anchors marked
        same(ops.stepOp(screen, [1, 1], -1), [{op: "move", at: [1, 1], to: [1, 0], position: "before"}])
        same(ops.stepOp(screen, [1, 3], 1), [])
        ops.outlineRows(screen).find(row => row.key == "1.0").label == "◆ container-box WidgetBox"
        ops.outlineRows(screen).find(row => row.key == "1.0.0.1").parent == "1.0.0"
    }
})
