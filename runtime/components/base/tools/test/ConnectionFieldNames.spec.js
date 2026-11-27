// The connection editor names each configuration control by its setting, not by the column's "Value"
// (plans/ui-review-system-ai-integrations.md AI-UI-003; tools/template/ConnectionParts.xml, shared by
// Integrations/ConnectionDetail and Protocols/MailRelayDetail), and New Remote Agent and New MCP Server offer the
// private-network choice a loopback or LAN address needs (AI-UI-001). Each screen renders once, in qvt.
// Run: ./moq test tools --spec ConnectionFieldNames
const systemRoot = "component://tools/screen/System.xml"
const remoteId = "SPEC_FIELD_NAMES"

function render(path, parameters) {
    const str = ec.screen.makeTest().rootScreen(systemRoot).renderMode("qvt")
        .render(path, Object.assign({lastStandalone: "true"}, parameters || {}), null)
    const output = String(str.output || "")
    ec.message.clearErrors()
    return {status: str.status, errors: str.errorMessages.join(" | "), output}
}
// the controls of a form, by the label each one renders with
const labels = (output) => Array.from(output.matchAll(/\blabel="([^"]*)"/g), match => match[1])

spec({
    setupSpec() {
        login("john.doe")
        const wasDisabled = ec.artifactExecution.disableAuthz()
        try {
            const remote = ec.entity.makeValue("moqui.service.message.SystemMessageRemote")
            remote.setFields({systemMessageRemoteId: remoteId, description: "Spec field names"}, true, null, null)
            remote.createOrUpdate()
        } finally {
            if (!wasDisabled) ec.artifactExecution.enableAuthz()
        }
    },
    cleanupSpec() {
        ec.message.clearErrors()
    }
})

feature("each configuration control of a connection is named by its setting", {
    expect() {
        const page = render("Integrations/ConnectionDetail", {adapterKey: "remote", connectionKey: remoteId})
        assert.equal(page.status, 200, page.errors)
        const start = page.output.indexOf("NativeConnection")
        assert.ok(start >= 0, "the editable configuration form must render: " + page.output.slice(0, 2000))
        // the editor's controls, up to the read-only facts form after it
        const named = labels(page.output.slice(start, page.output.indexOf("ConnectionFacts", start)))
        assert.ok(named.includes("Reason"), "the reason control is named Reason: " + JSON.stringify(named))
        assert.ok(!named.includes("Value"), "no control is named only Value: " + JSON.stringify(named))
    }
})

feature("New Remote Agent offers the private-network choice", {
    expect() {
        const page = render("Llm/Remote/AgentList")
        assert.equal(page.status, 200, page.errors)
        const dialog = page.output.slice(page.output.indexOf("CreateAgent"))
        assert.ok(/name="allowPrivateNetwork"/.test(dialog), "the create form has allowPrivateNetwork: " + dialog.slice(0, 1500))
    }
})
