// The service writer and the ECA rules on a dev instance (plans/design-dev-aide.md P7, 9, 10): a service's view with its
// parameters nested and implied ones marked, Try in sim with the rows it would write and nothing kept, a snippet run in
// sim, the generated JavaScript of an actions block, and the ECA rules as the framework holds them. Save as spec, Run
// spec and a site SECA's save are in the host's integration test (crates/moqui-host/tests/it/devaide.rs), which has a
// site component.
// Run: ./moq test dev-aide --spec ServiceWriter
const svc = (name, params) => ec.service.sync().name("org.moqui.devaide.ServiceWriterServices." + name).parameters(params || {}).call()

spec({
    setupSpec() {
        login("john.doe")
        // the Try runs as the user: let ADMIN create examples
        ec.artifactExecution.disableAuthz()
        try {
            const make = (entity, fields) => { const row = ec.entity.makeValue(entity); row.setFields(fields, true, null, null); row.createOrUpdate() }
            make("moqui.security.ArtifactGroup", { artifactGroupId: "SPEC_DEV_TRY", description: "Examples to try" })
            make("moqui.security.ArtifactGroupMember", { artifactGroupId: "SPEC_DEV_TRY", artifactName: "moqui.example.Example", artifactTypeEnumId: "AT_ENTITY",
                nameIsPattern: "N", inheritAuthz: "Y" })
            make("moqui.security.ArtifactGroupMember", { artifactGroupId: "SPEC_DEV_TRY", artifactName: "create#moqui.example.Example", artifactTypeEnumId: "AT_SERVICE",
                nameIsPattern: "N", inheritAuthz: "Y" })
            make("moqui.security.ArtifactAuthz", { artifactAuthzId: "SPEC_DEV_TRY", userGroupId: "ADMIN", artifactGroupId: "SPEC_DEV_TRY",
                authzTypeEnumId: "AUTHZT_ALWAYS", authzActionEnumId: "AUTHZA_ALL" })
        } finally {
            ec.artifactExecution.enableAuthz()
        }
    },
})

feature("a service's view has its parameters, its file and how it is implemented", {
    expect() {
        const view = svc("get#ServiceView", { serviceName: "example.ExampleServices.greet#WithTool" }).view
        const entityAuto = svc("get#ServiceView", { serviceName: "create#moqui.example.Example" }).view
        !ec.message.hasError()
        view.service.kind == "inline"
        view.service.inParameters.map((parameter) => parameter.name).join(",") == "name,greeting"
        view.artifact.component == "example"
        entityAuto.service.kind == "entity-auto"
        entityAuto.service.inParameters.some((parameter) => parameter.name == "exampleName" && parameter.implied)
    },
})

feature("a Try in sim answers the rows it would write and keeps none", {
    when() {
        const count = () => {
            ec.artifactExecution.disableAuthz()
            try { return ec.entity.find("moqui.example.Example").condition("exampleName", "Tried in sim").count() } finally { ec.artifactExecution.enableAuthz() }
        }
        const before = count()
        const result = svc("try#Service", { serviceName: "create#moqui.example.Example",
            parameters: JSON.stringify({ exampleName: "Tried in sim", exampleTypeEnumId: "EXT_MADE_UP" }) }).result
        const after = count()
        return { before, result, after }
    },
    then({ before, result, after }) {
        result.sim
        !result.failed
        result.out.exampleId
        result.rows.some((row) => row.entity == "moqui.example.Example" && row.action == "create"
            && row.fields.some((field) => field.field == "exampleName" && field.after == "Tried in sim"))
        before == after
    },
})

feature("a snippet runs in sim and answers its result", {
    expect() {
        const result = svc("try#Snippet", { language: "javascript", body: "result = { sum: 1 + 2 }" }).result
        result.sim
        result.messages.some((text) => text == "result: {\"sum\":3}")
    },
})

feature("the generated JavaScript of an actions block names each element", {
    expect() {
        const generated = svc("get#GeneratedJs", { xml: "<set field=\"a\" from=\"1\"/><if condition=\"a\"><message>yes</message></if>" })
        !generated.error
        generated.source.includes("set")
        generated.source.includes("if")
    },
})

feature("the ECA rules list the active SECA rules with their files", {
    expect() {
        const rules = svc("list#EcaRules", { kind: "seca", target: "fetch#GitRepository" }).rules
        rules.length > 0
        rules[0].file.endsWith("Files.secas.xml")
        rules[0].line > 0
        rules[0].priority == 5
        rules[0].when == "post-service"
    },
})
