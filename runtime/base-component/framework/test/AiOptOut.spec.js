// The per-field AI opt-out (design-people PE-21; docs/ai/Assist.md "AI opt-out"): an opted-out field's text reaches no
// model tool. An agent's render (what Assist's screen and data tools and agent-apps read) shows a placeholder for an
// ai-opt-out form field, hidden value, list column and section, and for an ai-opt-out screen, with no find field for the
// column, while the person's own render shows them; the browser's render marks them for the Assist client (no text actions
// in m-rich-text, nothing of them in the page state); and Update with AI refuses an ai-opt-out entity field before any
// model is asked. The data tool's refusal of a find or sort by such a column and its entity reads are cargo's
// (moqui-llm tools::core_tests), the client's handling of the marks the client spec's (WebrootAssist).
// Run: ./moq test framework --spec AiOptOut
const ROOT = "component://framework/test/screen/AiOptOut.xml"
const WHOLE = "component://framework/test/screen/AiOptOutScreen.xml"
const SECRETS = ["Confidential note A", "Confidential ref B", "Confidential reason C", "Confidential statement D"]
const PLACEHOLDER = "Not shared with AI"

function render(root, mode, agent) {
    let test = ec.screen.makeTest().rootScreen(root).renderMode(mode)
    if (agent) test = test.agent()
    const out = test.render("", {lastStandalone: "true"}, null)
    ec.message.clearErrors()
    return {status: out.status, text: String(out.output || ""), errors: Array.from(out.errorMessages || []).join("\n")}
}
// the nodes of a tui document, wherever they are
function nodes(value, out = []) {
    if (Array.isArray(value)) value.forEach((item) => nodes(item, out))
    else if (value && typeof value === "object") {
        if (value.t) out.push(value)
        Object.values(value).forEach((item) => nodes(item, out))
    }
    return out
}
function invoke(name, parameters) {
    try {
        const result = ec.service.sync().name(name).parameters(parameters).call() || {}
        const error = ec.message.hasError() ? ec.message.getErrorsString() : ""
        ec.message.clearErrors()
        return Object.assign({}, result, {error})
    } catch (e) {
        ec.message.clearErrors()
        return {error: String(e.message || e)}
    }
}

spec({
    setupSpec() { ec.user.internalLoginUser("john.doe") },
    cleanup() { ec.artifactExecution.enableAuthz() }
})

feature("an agent's render shows a placeholder for each opted-out field, hidden value, column and section, the person's shows them", {
    when() {
        const person = render(ROOT, "tui", false)
        const agent = render(ROOT, "tui", true)
        const all = nodes(JSON.parse(agent.text))
        return {person, agent, all, list: all.find((node) => node.t === "formList" && node.name === "CaseList"),
            note: all.find((node) => node.t === "field" && node.name === "note")}
    },
    then({person, agent, all, list, note}) {
        person.status == 200 && person.errors == ""
        agent.status == 200 && agent.errors == ""
        SECRETS.every((text) => person.text.includes(text))
        SECRETS.filter((text) => agent.text.includes(text)).length == 0
        agent.text.includes("Visible subject") && agent.text.includes("Visible title")
        note.w.kind == "display" && note.w.text == PLACEHOLDER
        // the list's opted-out column: its cell is the placeholder, it has no find field, and the list names it
        JSON.stringify(list.rows[0].cells).includes(PLACEHOLDER)
        list.headerFields.length == 0
        list.aiOff.join() == "reason"
        all.some((node) => node.t === "section" && node.name === "Statement" && JSON.stringify(node.c).includes(PLACEHOLDER))
    }
})

feature("an opted-out screen shows an agent its placeholder in place of its widgets", {
    when() { return {person: render(WHOLE, "tui", false), agent: render(WHOLE, "tui", true)} },
    then({person, agent}) {
        person.text.includes("Confidential screen E")
        !agent.text.includes("Confidential screen E")
        agent.text.includes(PLACEHOLDER)
    }
})

feature("the browser's render marks the opted-out field, column, section and screen for the Assist client", {
    when() { return {page: render(ROOT, "qvt", false), whole: render(WHOLE, "qvt", false)} },
    then({page, whole}) {
        page.status == 200 && page.errors == ""
        page.text.includes('data-ai-opt-out="field" data-ai-field="note"')
        page.text.includes('data-ai-opt-out="field" data-ai-field="reason"')
        page.text.includes('data-ai-opt-out="section"')
        // the person still reads the text in the browser
        page.text.includes("Confidential note A")
        whole.text.includes('data-ai-opt-out="screen"')
    }
})

feature("Update with AI refuses an opted-out entity field before any model is asked", {
    when() {
        // the service needs AgentUse, which the refusal comes before
        ec.artifactExecution.disableAuthz()
        return {
            off: invoke("org.moqui.agent.AiTaskServices.propose#AiFieldValue", {entityName: "moqui.test.TestEntity", recordId: "AI_OPT_OUT_NONE", fieldName: "testLong"}),
            on: invoke("org.moqui.agent.AiTaskServices.propose#AiFieldValue", {entityName: "moqui.test.TestEntity", recordId: "AI_OPT_OUT_NONE", fieldName: "testMedium"})
        }
    },
    then({off, on}) {
        off.error.includes("testLong of moqui.test.TestEntity is opted out of AI")
        off.proposedValue == null
        // a field that is not opted out goes on to the record, which is not there
        !on.error.includes("opted out") && on.error.includes("no moqui.test.TestEntity record")
    }
})
