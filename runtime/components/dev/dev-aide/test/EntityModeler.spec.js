// The entity modeler's services on a dev instance (plans/design-dev-aide.md P5, 8): the entity view reads an entity as
// the server merged it, with each field's origin and the table against the fields; the changes the table check never
// makes are planned for the full tier and refused to the extend tier, which never sees a destructive change; a site's
// seed data loads. The view's own edits (a field through the site's extend-entity, a Picklist and its rows) are in the
// Chromium smoke the_entity_modeler_extends_an_upstream_entity_in_chromium.
// Run: ./moq test dev-aide --spec EntityModeler
const svc = (name, params) => ec.service.sync().name("org.moqui.devaide.EntityServices." + name).parameters(params || {}).call()
const errorsOf = (name, params) => {
    svc(name, params)
    const text = ec.message.getErrorsString()
    ec.message.clearErrors()
    return text
}

spec({
    setupSpec() {
        login("john.doe")
        ec.artifactExecution.disableAuthz()
        try {
            const make = (entity, fields) => { const row = ec.entity.makeValue(entity); row.setFields(fields, true, null, null); row.createOrUpdate() }
            make("moqui.security.UserAccount", { userId: "devExtender", username: "dev.extender", userFullName: "Dev Extender" })
            make("moqui.security.UserGroupMember", { userGroupId: "DEV_AIDE_EXTENDERS", userId: "devExtender", fromDate: new Date(Date.now() - 60000) })
        } finally {
            ec.artifactExecution.enableAuthz()
        }
    },
})

feature("the entity view reads an entity with its fields' origin and its table", {
    expect() {
        const view = svc("get#EntityView", { artifactId: "entity:moqui.example.Example" }).view
        !ec.message.hasError()
        view.entity.fullEntityName == "moqui.example.Example"
        view.entity.fields.some((field) => field.name == "exampleId" && field.isPk)
        view.fields.some((field) => field.name == "exampleId" && field.component == "example")
        view.table.exists
        view.table.missingColumns.length == 0
        view.fieldTypes.some((type) => type.type == "date" && type.sqlType)
        view.tier == "full"
    },
})

feature("a view entity has its members and aliases", {
    expect() {
        const view = svc("get#EntityView", { entityName: "moqui.basic.EnumerationAndType" }).view
        view.entity.isViewEntity
        view.entity.members.some((member) => member.entityAlias == "ENUM")
        view.entity.aliasAlls.length + view.entity.aliases.length > 0
    },
})

feature("the full tier plans a change the table check never makes", {
    expect() {
        const drop = svc("plan#EntityDdl", { entityName: "moqui.example.Example", action: "drop-column", columnName: "old_column" })
        const recreate = svc("plan#EntityDdl", { entityName: "moqui.example.Example", action: "recreate-table" })
        !ec.message.hasError()
        drop.statements[0].includes("DROP COLUMN")
        drop.tableName.toLowerCase() == "example"
        recreate.statements[0].startsWith("DROP TABLE")
        recreate.statements[1].startsWith("CREATE TABLE")
    },
})

feature("a run needs the table's name typed back", {
    expect() {
        const refused = errorsOf("run#EntityDdl", { entityName: "moqui.example.Example", action: "drop-column", columnName: "old_column", confirmTable: "wrong" })
        refused.includes("type the table's name")
    },
})

feature("the extend tier never sees a destructive change", {
    when() {
        const extender = ec.user.internalLoginUser("dev.extender")
        const plan = errorsOf("plan#EntityDdl", { entityName: "moqui.example.Example", action: "drop-column", columnName: "x" })
        const run = errorsOf("run#EntityDdl", { entityName: "moqui.example.Example", action: "recreate-table", confirmTable: "example" })
        const view = svc("get#EntityView", { artifactId: "entity:moqui.example.Example" }).view
        login("john.doe")
        return { extender, plan, run, view }
    },
    then({ extender, plan, run, view }) {
        extender
        plan.includes("full tier")
        run.includes("full tier")
        view.tier == "extend"
    },
})

feature("a view entity's Run shows its SQL", {
    expect() {
        const sql = svc("get#ViewSql", { entityName: "moqui.basic.EnumerationAndType" }).sql
        sql.startsWith("SELECT")
        sql.includes("ENUMERATION") || sql.includes("enumeration")
    },
})

feature("Export writes an entity's records as a data file's elements, and Check compares them with the database", {
    when() {
        login("john.doe")
        const exported = svc("export#Records", { entityName: "moqui.basic.EnumerationType", filter: JSON.stringify({ enumTypeId: "ComparisonOperator" }) })
        const roots = ec.service.sync().name("org.moqui.devaide.WorkspaceServices.get#Roots").call().roots
        const root = roots.find((item) => item.components.some((component) => component.name == "example"))
        const component = root.components.find((item) => item.name == "example")
        const path = (component.path ? component.path + "/" : "") + "data/ExampleTypeData.xml"
        const matching = svc("check#DataFile", { rootId: root.id, path, text: "<entity-facade-xml>" + exported.records + "</entity-facade-xml>" })
        const differing = svc("check#DataFile", { rootId: root.id, path,
            text: "<entity-facade-xml><moqui.basic.EnumerationType enumTypeId=\"ComparisonOperator\" description=\"Not this\"/></entity-facade-xml>" })
        return { exported, matching, differing }
    },
    then({ exported, matching, differing }) {
        exported.count == 1
        exported.records.startsWith("<moqui.basic.EnumerationType")
        matching.messages.length == 0
        differing.messages.some((text) => text.includes("description"))
    },
})
