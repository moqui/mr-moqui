// The evaluation tasks of the dev agent (plans/design-dev-aide.md P4, 13.1, 19): extend an upstream entity and its form
// from the site component, add a list column, find where a service is used, write a spec from a Try in the sim, and
// try a service as proposed text defines it (F14: an edit, a change set of the user's; another user's change set, a
// real run over proposed text and a reader's try refused).
// The example component stands in for the site component for the run (Settings' siteComponents), so the extensions land
// in a component that is loaded; the files the tasks write are put back after each task. Run with the recorded replies
// and no model, or with a model (a key from the environment; nightly, and before a phase closes):
//   moqui test dev-aide --agents --replay runtime/components/dev/dev-aide/test/agent/replay
//   moqui test dev-aide --agents --llm-profile <profile> --runs 3 --min-pass 0.8
// A task passes when its checks hold: the files proposed and written are the expected ones, and they check clean.

const SITE = "example"
const ENTITIES = "component://example/entity/ExampleEntities.xml"
const DETAIL_EXTEND = "component://example/screen-extend/tools/screen/System/Security/UserAccount/UserAccountDetail.xml"
const LIST_EXTEND = "component://example/screen-extend/tools/screen/System/Security/UserAccount/UserAccountList.xml"
const SPEC_FILE = "component://example/test/ExampleTry.spec.js"

const ref = (location) => ec.resource.getLocationReference(location)
const kept = {}
// keep a file's text (or that it was not there) to put it back after the task
const keep = (location) => { const file = ref(location); kept[location] = file.getExists() ? file.getText() : null }
const restore = (location) => {
    const file = ref(location)
    if (kept[location] === null || kept[location] === undefined) { if (file.getExists()) file.delete() }
    else file.putText(kept[location])
}
const setting = (name, value) => {
    const row = ec.entity.makeValue("moqui.dev.DevAideSetting")
    row.setFields({ settingName: name, settingValue: value }, true, null, null)
    row.createOrUpdate()
}
const make = (entityName, fields) => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.createOrUpdate()
}
const remove = (entityName, field, value) => ec.entity.find(entityName).condition(field, value).deleteAll()
const textOf = (location) => (ref(location).getExists() ? ref(location).getText() : "")

const EXAMPLE_SERVICES = "component://example/service/example/ExampleServices.xml"
const GREET = "example.ExampleServices.greet#WithTool"
const GREET_LINE = '<set field="text" from="tool.greet(name)"/>'
// a change set of `userId`'s that has greet#WithTool answer `text` (an expression)
const change = (approvalId, userId, text) => make("moqui.agent.AgentApproval", { approvalId, userId, agentId: "dev", targetKind: "file",
    targetName: "a greeting from a change set", argumentsJson: JSON.stringify({ summary: "a greeting from a change set", target: "in-place",
        files: [{ file: EXAMPLE_SERVICES, edits: [{ old: GREET_LINE, new: `<set field="text" from="${text}"/>` }] }] }) })
// the example's services called outside a screen need their permission themselves (as the spec task's entity)
const proposalSetup = () => {
    make("moqui.security.ArtifactGroup", { artifactGroupId: "DEV_EVAL_TRY", description: "Dev eval try" })
    make("moqui.security.ArtifactGroupMember", { artifactGroupId: "DEV_EVAL_TRY", artifactName: "example\\.ExampleServices\\..*",
        artifactTypeEnumId: "AT_SERVICE", nameIsPattern: "Y" })
    make("moqui.security.ArtifactAuthz", { artifactAuthzId: "DEV_EVAL_TRY", userGroupId: "ADMIN", artifactGroupId: "DEV_EVAL_TRY",
        authzTypeEnumId: "AUTHZT_ALLOW", authzActionEnumId: "AUTHZA_ALL" })
}
const proposalCleanup = () => {
    remove("moqui.security.ArtifactAuthz", "artifactAuthzId", "DEV_EVAL_TRY")
    remove("moqui.security.ArtifactGroupMember", "artifactGroupId", "DEV_EVAL_TRY")
    remove("moqui.security.ArtifactGroup", "artifactGroupId", "DEV_EVAL_TRY")
}

agentSpec({
    agent: "dev",
    surface: "drawer",
    user: "john.doe",
    setup() {
        ec.artifactExecution.disableAuthz()
        setting("siteComponents", SITE)
    },
    cleanup() {
        ec.entity.find("moqui.dev.DevAideSetting").condition("settingName", "siteComponents").deleteAll()
        ec.artifactExecution.enableAuthz()
    },
    tasks: [
        {
            name: "add a field through the site and show it",
            start: "/apps/system/Security/UserAccount/UserAccountDetail?userId=EX_JOHN_DOE",
            say: "Add a Nickname field to user accounts and show it on this form after Full Name.",
            approvals: ["approve"],
            setup() { keep(ENTITIES); keep(DETAIL_EXTEND) },
            cleanup() { restore(ENTITIES); restore(DETAIL_EXTEND) },
            expect: {
                called: [{ tool: "skill", args: { name: /^moqui-dev/ } }, { tool: "artifact" }, { tool: "propose_edit" }],
                results: [{ tool: "propose_edit", result: /2 of 2 files written/ }],
                answer: /[Nn]ickname/,
            },
            verify(run) {
                const entities = textOf(ENTITIES)
                const extend = textOf(DETAIL_EXTEND)
                assert(entities.includes('<extend-entity entity-name="UserAccount" package="moqui.security">'), "no extend-entity in the site's entity file")
                assert(entities.includes('name="nickname"'), "no nickname field")
                assert(extend.includes('<form-single name="UserAccountForm">') && extend.includes('after="userFullName"'),
                    "the form field is not placed after Full Name")
                // the diff confined to the site component: the upstream files are as they were
                assert(textOf("component://tools/screen/System/Security/UserAccount/UserAccountDetail.xml").indexOf("nickname") < 0,
                    "the upstream screen was edited")
                assert(!run.calls.some((call) => call.tool == "propose_edit" && /"proposed":false/.test(call.result)),
                    "a proposal came back with problems")
            },
        },
        {
            name: "add a list column",
            start: "/apps/system/Security/UserAccount/UserAccountList",
            say: "Add a Currency column to this list after Time Zone.",
            approvals: ["approve"],
            setup() { keep(LIST_EXTEND) },
            cleanup() { restore(LIST_EXTEND) },
            expect: {
                called: [{ tool: "skill", args: { name: /^moqui-dev/ } }, { tool: "propose_edit" }],
                results: [{ tool: "propose_edit", result: /1 of 1 files written/ }],
                answer: /Currency/,
            },
            verify() {
                const extend = textOf(LIST_EXTEND)
                assert(extend.includes('<form-list name="UserAccountList">') && extend.includes('name="currencyUomId"') &&
                    extend.includes('after="timeZone"'), "the column is not added after Time Zone")
            },
        },
        {
            name: "find where a service is used",
            start: "/apps/dev/Artifacts/Search",
            say: "Where is the service org.moqui.impl.UserServices.create#UserAccount used?",
            expect: {
                called: [{ tool: "artifact", args: { action: "refs" } }],
                results: [{ tool: "artifact", result: /UserAccountList/ }],
                answer: /UserAccountList/,
                noWrite: true,
            },
        },
        {
            name: "write a spec from a try",
            start: "/apps/dev/Workspace",
            say: "Try creating an example named DEV_EVAL_EXAMPLE in the sim, then write a spec from it in the example " +
                "component's test directory.",
            approvals: ["approve"],
            setup() {
                keep(SPEC_FILE)
                // a service called outside a screen needs the entity's permission itself (as assist.agent.js's tool task)
                make("moqui.security.ArtifactGroup", { artifactGroupId: "DEV_EVAL_EXAMPLE", description: "Dev eval example" })
                make("moqui.security.ArtifactGroupMember", { artifactGroupId: "DEV_EVAL_EXAMPLE",
                    artifactName: "moqui.example.Example", artifactTypeEnumId: "AT_ENTITY", nameIsPattern: "N" })
                make("moqui.security.ArtifactAuthz", { artifactAuthzId: "DEV_EVAL_EXAMPLE", userGroupId: "ADMIN",
                    artifactGroupId: "DEV_EVAL_EXAMPLE", authzTypeEnumId: "AUTHZT_ALLOW", authzActionEnumId: "AUTHZA_ALL" })
            },
            cleanup() {
                restore(SPEC_FILE)
                remove("moqui.security.ArtifactAuthz", "artifactAuthzId", "DEV_EVAL_EXAMPLE")
                remove("moqui.security.ArtifactGroupMember", "artifactGroupId", "DEV_EVAL_EXAMPLE")
                remove("moqui.security.ArtifactGroup", "artifactGroupId", "DEV_EVAL_EXAMPLE")
            },
            expect: {
                called: [{ tool: "try_service", args: { service: "create#moqui.example.Example" } }, { tool: "propose_edit" }],
                results: [{ tool: "try_service", result: /"action":"create"/ }, { tool: "propose_edit", result: /1 of 1 files written/ }],
                answer: /spec/i,
            },
            verify() {
                const spec = textOf(SPEC_FILE)
                assert(spec.includes("create#moqui.example.Example") && spec.includes("DEV_EVAL_EXAMPLE"), "the spec does not call the service")
                // the sim kept nothing
                assert(ec.entity.find("moqui.example.Example").condition("exampleName", "DEV_EVAL_EXAMPLE").one() == null, "the sim's row is real")
            },
        },
        {
            name: "try a service as a proposal defines it",
            start: "/apps/dev/Workspace",
            say: "Try greet#WithTool for Ann in the sim as this edit would make it, and as my change set DEV_EVAL_MINE makes it. " +
                "Also try the change set DEV_EVAL_OTHER, and the edit for real.",
            setup() {
                proposalSetup()
                change("DEV_EVAL_MINE", "EX_JOHN_DOE", "'from my change set, ' + name")
                change("DEV_EVAL_OTHER", "EX_DEV_EVAL_OTHER", "'from another user, ' + name")
            },
            cleanup() {
                proposalCleanup()
                remove("moqui.agent.AgentApproval", "approvalId", "DEV_EVAL_MINE")
                remove("moqui.agent.AgentApproval", "approvalId", "DEV_EVAL_OTHER")
            },
            expect: {
                called: [{ tool: "try_service", args: { service: GREET } }],
                results: [{ tool: "try_service", result: /proposed for Ann/ }, { tool: "try_service", result: /from my change set, Ann/ }],
                noWrite: true,
                maxWrongCalls: 2,
            },
            verify(run) {
                const said = (call) => String(call.result || "") + String(call.error || "")
                const tries = run.calls.filter((call) => call.tool == "try_service")
                assert(tries.some((call) => /no_change_set/.test(said(call)) && !/from another user/.test(said(call))),
                    "another user's change set was not refused")
                assert(tries.some((call) => /runs in the sim only/.test(said(call))), "a real run over proposed text was not refused")
                // the proposals ran in the sim only: the file is as it was
                assert(textOf(EXAMPLE_SERVICES).includes(GREET_LINE), "the service file changed")
            },
        },
        {
            name: "a reader may not try proposed text",
            start: "/apps/dev/Workspace",
            user: "dev.eval.viewer",
            say: "Try greet#WithTool for Ann in the sim as this edit would make it.",
            setup() {
                proposalSetup()
                make("moqui.security.UserGroup", { userGroupId: "DEV_EVAL_VIEWERS", description: "Dev AIDE readers (eval)" })
                make("moqui.security.UserGroupPermission", { userGroupId: "DEV_EVAL_VIEWERS", userPermissionId: "DEV_AIDE_VIEW", fromDate: new Date(Date.now() - 60000) })
                make("moqui.security.UserGroupPermission", { userGroupId: "DEV_EVAL_VIEWERS", userPermissionId: "AgentUse", fromDate: new Date(Date.now() - 60000) })
                make("moqui.security.ArtifactAuthz", { artifactAuthzId: "DEV_EVAL_VIEWERS_APP", artifactGroupId: "DEV_AIDE_APP", userGroupId: "DEV_EVAL_VIEWERS",
                    authzTypeEnumId: "AUTHZT_ALWAYS", authzActionEnumId: "AUTHZA_ALL" })
                make("moqui.security.UserAccount", { userId: "DEV_EVAL_VIEWER", username: "dev.eval.viewer", userFullName: "Dev Eval Viewer" })
                make("moqui.security.UserGroupMember", { userGroupId: "DEV_EVAL_VIEWERS", userId: "DEV_EVAL_VIEWER", fromDate: new Date(Date.now() - 60000) })
            },
            cleanup() {
                proposalCleanup()
                remove("moqui.security.UserGroupMember", "userGroupId", "DEV_EVAL_VIEWERS")
                remove("moqui.security.ArtifactAuthz", "artifactAuthzId", "DEV_EVAL_VIEWERS_APP")
                remove("moqui.security.UserGroupPermission", "userGroupId", "DEV_EVAL_VIEWERS")
                remove("moqui.security.UserGroup", "userGroupId", "DEV_EVAL_VIEWERS")
                remove("moqui.security.UserAccount", "userId", "DEV_EVAL_VIEWER")
            },
            expect: { called: [{ tool: "try_service", args: { service: GREET } }], noWrite: true, maxWrongCalls: 1 },
            verify(run) {
                const said = (call) => String(call.result || "") + String(call.error || "")
                assert(run.calls.some((call) => call.tool == "try_service" && /extend tier/.test(said(call)) && !/proposed for Ann/.test(said(call))),
                    "a reader's try of proposed text was not refused")
            },
        },
    ],
})
