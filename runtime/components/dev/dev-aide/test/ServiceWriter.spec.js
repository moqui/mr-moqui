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
        // the Try runs as the user, with what Dev AIDE's own frame passes on at the extend tier (ADMIN, here) as a real
        // run has; a reader of the view tier keeps only their own grants
        ec.artifactExecution.disableAuthz()
        try {
            const make = (entity, fields) => { const row = ec.entity.makeValue(entity); row.setFields(fields, true, null, null); row.createOrUpdate() }
            make("moqui.security.UserGroup", { userGroupId: "SPEC_DEV_VIEWERS", description: "Dev AIDE readers (spec)" })
            make("moqui.security.UserGroupPermission", { userGroupId: "SPEC_DEV_VIEWERS", userPermissionId: "DEV_AIDE_VIEW", fromDate: new Date(Date.now() - 60000) })
            make("moqui.security.ArtifactAuthz", { artifactAuthzId: "SPEC_DEV_VIEWERS_APP", artifactGroupId: "DEV_AIDE_APP", userGroupId: "SPEC_DEV_VIEWERS",
                authzTypeEnumId: "AUTHZT_ALWAYS", authzActionEnumId: "AUTHZA_ALL" })
            make("moqui.security.UserAccount", { userId: "specDevViewer", username: "spec.dev.viewer", userFullName: "Spec Dev Viewer" })
            make("moqui.security.UserGroupMember", { userGroupId: "SPEC_DEV_VIEWERS", userId: "specDevViewer", fromDate: new Date(Date.now() - 60000) })
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

feature("a Try in sim at the view tier has only the user's own grants", {
    when() {
        ec.user.internalLoginUser("spec.dev.viewer")
        try {
            return svc("try#Service", { serviceName: "create#moqui.example.Example",
                parameters: JSON.stringify({ exampleName: "Tried by a reader", exampleTypeEnumId: "EXT_MADE_UP" }) }).result
        } finally {
            ec.message.clearAll()
            ec.user.internalLoginUser("john.doe")
        }
    },
    then(result) {
        result.sim
        result.failed.includes("not authorized")
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

feature("a rule's condition is tried against a context in the sim", {
    expect() {
        const holds = svc("eval#Condition", { condition: "amount > 3 && status == 'OPEN'", against: JSON.stringify({ amount: 5, status: "OPEN" }) })
        const not = svc("eval#Condition", { condition: "amount > 3", against: JSON.stringify({ amount: 1 }) })
        const element = svc("eval#Condition", { condition: "<compare field=\"amount\" operator=\"greater\" value=\"3\"/>", against: JSON.stringify({ amount: 4 }) })
        holds.holds
        !not.holds
        element.holds
    },
})

feature("a rule's condition is not tried at the view tier", {
    when() {
        ec.user.internalLoginUser("spec.dev.viewer")
        try {
            const answer = svc("eval#Condition", { condition: "amount > 3", against: JSON.stringify({ amount: 5 }) })
            return { holds: answer?.holds, refusal: String(ec.message.getErrorsString() || "") }
        } catch (error) {
            return { refusal: String(error) }
        } finally {
            ec.message.clearAll()
            ec.user.internalLoginUser("john.doe")
        }
    },
    then(result) {
        result.holds !== true
        result.refusal.includes("extend tier")
    },
})

feature("a Try's log lines come back with it", {
    expect() {
        const result = svc("try#Snippet", { language: "javascript", body: "ec.logger.info('the snippet wrote this line')" }).result
        result.log.some((line) => line.includes("the snippet wrote this line"))
    },
})

// F14 for services: a Try over an unsaved buffer of ExampleServices.xml that changes greet#WithTool and adds a service
// that calls it and creates a record
const EXAMPLE_SERVICES = "component://example/service/example/ExampleServices.xml"
const bufferedExample = () => {
    const view = svc("get#ServiceView", { serviceName: "example.ExampleServices.greet#WithTool" }).view
    const text = ec.resource.getLocationText(EXAMPLE_SERVICES)
    const buffered = text
        .replace("<set field=\"text\" from=\"tool.greet(name)\"/>", "<set field=\"text\" from=\"'from the buffer, ' + name\"/>")
        .replace("</services>", `    <service verb="make" noun="BufferedExample">
        <in-parameters><parameter name="name"/></in-parameters>
        <out-parameters><parameter name="text"/><parameter name="exampleId"/></out-parameters>
        <actions>
            <service-call name="example.ExampleServices.greet#WithTool" in-map="({ name: name })" out-map="greeted"/>
            <set field="text" from="greeted.text"/>
            <service-call name="create#moqui.example.Example" in-map="({ exampleName: text, exampleTypeEnumId: 'EXT_MADE_UP' })" out-map="made"/>
            <set field="exampleId" from="made.exampleId"/>
        </actions>
    </service>
</services>`)
    return { text, files: JSON.stringify([{ rootId: view.artifact.rootId, path: view.artifact.relativePath, text: buffered }]) }
}
const bufferedRows = () => {
    ec.artifactExecution.disableAuthz()
    try {
        return ec.entity.find("moqui.example.Example").condition("exampleName", "from the buffer, Ann").count()
    } finally {
        ec.artifactExecution.enableAuthz()
    }
}
const tryBuffered = (files, trace) => svc("try#Service", { serviceName: "example.ExampleServices.make#BufferedExample",
    parameters: JSON.stringify({ name: "Ann" }), files, trace }).result

feature("a Try runs a service as its unsaved buffer defines it, and nothing else sees the buffer", {
    when() {
        const { text, files } = bufferedExample()
        const result = tryBuffered(files, true)
        const view = svc("get#ServiceView", { serviceName: "example.ExampleServices.make#BufferedExample", files }).view
        ec.artifactExecution.disableAuthz()
        let normal
        try {
            normal = ec.service.sync().name("example.ExampleServices.greet#WithTool").parameters({ name: "Ann" }).call()
        } finally {
            ec.artifactExecution.enableAuthz()
        }
        return { result, view, normal, defined: ec.service.getServiceDefinition("example.ExampleServices.make#BufferedExample"),
            kept: bufferedRows(), unchanged: ec.resource.getLocationText(EXAMPLE_SERVICES) == text }
    },
    then({ result, view, normal, defined, kept, unchanged }) {
        !result.failed
        // the service the buffer defines anew, and the one it changes, which the new one calls in the same file
        result.out.text == "from the buffer, Ann"
        result.overlay.services.includes("example.ExampleServices.make#BufferedExample")
        result.rows.some((row) => row.entity == "moqui.example.Example" && row.action == "create")
        view.service.inParameters.map((parameter) => parameter.name).join(",") == "name"
        // a normal call has the file on the disk, the new service is not defined, and the sim kept nothing
        normal.text != "from the buffer, Ann"
        defined == null
        kept == 0
        unchanged
    },
})

feature("a Try's trace answers each element's count and values, and the element that failed", {
    when() {
        const { files } = bufferedExample()
        const failing = JSON.parse(files)
        failing[0].text = failing[0].text.replace("<set field=\"exampleId\" from=\"made.exampleId\"/>",
            "<set field=\"exampleId\" from=\"made.exampleId\"/><script>throw new Error('stopped in the buffer')</script>")
        const ran = tryBuffered(files, true)
        const failed = tryBuffered(JSON.stringify(failing), true)
        const untraced = tryBuffered(files, false)
        // the block of make#BufferedExample's actions: the one with a fourth element
        const block = (result) => result.trace.blocks.find((block) => String(block.path).endsWith("ExampleServices.xml") && block.elements["3"])
        return { ran: block(ran), failedBlock: block(failed), failedAt: failed.trace.failed, failure: failed.failed, untraced }
    },
    then({ ran, failedBlock, failedAt, failure, untraced }) {
        ran.elements["0"].count == 1
        ran.elements["1"].values.text == "from the buffer, Ann"
        ran.elements["3"].count == 1
        failedBlock.elements["4"].failed.includes("stopped in the buffer")
        failedAt.element == "4"
        failure.includes("stopped in the buffer")
        untraced.trace == null
    },
})

// what a viewer asks of a Try, as the view tier: its refusal, or its result
const asViewer = (parameters) => {
    ec.user.internalLoginUser("spec.dev.viewer")
    try {
        const answer = svc("try#Service", parameters)
        return { result: answer?.result, refusal: String(ec.message.getErrorsString() || "") }
    } catch (error) {
        return { refusal: String(error) }
    } finally {
        ec.message.clearAll()
        ec.user.internalLoginUser("john.doe")
    }
}

feature("a Try over unsaved text, or traced, needs the extend tier", {
    when() {
        const { files } = bufferedExample()
        const name = "example.ExampleServices.greet#WithTool"
        return { over: asViewer({ serviceName: name, parameters: "{}", files }), traced: asViewer({ serviceName: name, parameters: "{}", trace: true }) }
    },
    then({ over, traced }) {
        over.result == null
        over.refusal.includes("extend tier")
        traced.result == null
        traced.refusal.includes("extend tier")
    },
})

feature("a real run over unsaved text is refused", {
    when() {
        const { files } = bufferedExample()
        try {
            const answer = svc("try#Service", { serviceName: "example.ExampleServices.greet#WithTool", parameters: "{}", files, real: true, confirm: true })
            return { result: answer?.result, refusal: String(ec.message.getErrorsString() || "") }
        } catch (error) {
            return { refusal: String(error) }
        } finally {
            ec.message.clearAll()
        }
    },
    then({ result, refusal }) {
        result == null
        refusal.includes("runs in the sim only")
    },
})
