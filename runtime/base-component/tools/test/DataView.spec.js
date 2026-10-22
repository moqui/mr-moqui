// The Data View tools (screen/Tools/DataView): Edit DB View renders the master entity's fields and saves the checked ones
// as aliases, with a member and key map for a field of a related entity.
// Run: ./moq test tools
const viewName = "SpecDbView"
const tools = () => ec.screen.makeTest().rootScreen("component://tools/screen/Tools.xml")
const dbViewEntities = ["DbViewEntityAlias", "DbViewEntityKeyMap", "DbViewEntityMember", "DbViewEntity"]

spec({
    setupSpec() { login("john.doe") },
    setup() {
        ec.artifactExecution.disableAuthz()
        const make = (entityName, fields) => {
            const row = ec.entity.makeValue(entityName)
            row.setFields(fields, true, null, null)
            row.create()
        }
        make("moqui.entity.view.DbViewEntity", { dbViewEntityName: viewName, packageName: "spec" })
        make("moqui.entity.view.DbViewEntityMember", { dbViewEntityName: viewName, entityAlias: "MASTER", entityName: "moqui.basic.Enumeration" })
    },
    cleanup() {
        for (const name of dbViewEntities) ec.entity.find(`moqui.entity.view.${name}`).condition("dbViewEntityName", viewName).deleteAll()
        ec.artifactExecution.enableAuthz()
    },
})

feature("Edit DB View saves the checked fields of the master and of a related entity", {
    expect() {
        const page = tools().render("DataView/EditDbView", { dbViewEntityName: viewName }, null)
        const saved = tools().render("DataView/EditDbView/setMasterFields", {
            dbViewEntityName_0: viewName, field_0: "moqui.basic.Enumeration.enumId", functionName_0: "",
            dbViewEntityName_1: viewName, field_1: "moqui.basic.Enumeration.description", functionName_1: "upper",
            dbViewEntityName_2: viewName, field_2: "moqui.basic.EnumerationType.description", functionName_2: "",
        }, "post")
        const find = (name) => ec.entity.find(`moqui.entity.view.${name}`).condition("dbViewEntityName", viewName).useCache(false)
        const aliases = find("DbViewEntityAlias").orderBy("fieldAlias").list()
        const members = find("DbViewEntityMember").orderBy("entityAlias").list()
        const keyMaps = find("DbViewEntityKeyMap").list()
        page.status == 200
        page.errorMessages.length == 0
        page.output.includes("dbViewEntityName_0")
        saved.errorMessages.length == 0
        aliases.map((alias) => `${alias.fieldAlias}:${alias.entityAlias}:${alias.fieldName}:${alias.functionName || ""}`).join(",") ==
            "description:MASTER:description:upper,description2:MEMBER1:description:,enumId:MASTER:enumId:"
        members.map((member) => `${member.entityAlias}:${member.entityName}`).join(",") ==
            "MASTER:moqui.basic.Enumeration,MEMBER1:moqui.basic.EnumerationType"
        keyMaps.length == 1 && keyMaps[0].fieldName == "enumTypeId" && keyMaps[0].entityAlias == "MEMBER1"
    },
})
