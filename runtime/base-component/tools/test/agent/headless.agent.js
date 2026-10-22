// The headless evaluation tasks of the Assist agent (plans/LLM-A2A-Assist.md 9, "three headless A2A versions"): no one
// sees a page, so the agent reads and acts on screens and answers with the facts and links; a write waits for its
// approval in the app. They run on the a2a surface through the run engine directly (the A2A server is phase 13.10), so
// the spec lets the assist agent serve that surface for its run and puts its surfaces back after.
//   moqui test tools --agents --replay runtime/base-component/tools/test/agent/replay --spec headless

const daysAgo = (days) => new Date(Date.now() - days * 24 * 60 * 60 * 1000)
const make = (entityName, fields) => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.createOrUpdate()
}
const remove = (entityName, field, value) => ec.entity.find(entityName).condition(field, value).deleteAll()
const one = (entityName, field, value) => ec.entity.find(entityName).condition(field, value).one()
let surfaces = null

agentSpec({
    agent: "assist",
    surface: "a2a",
    user: "john.doe",
    setup() {
        ec.artifactExecution.disableAuthz()
        const agent = one("moqui.agent.AgentDefinition", "agentId", "assist")
        surfaces = agent.surfaces
        make("moqui.agent.AgentDefinition", { agentId: "assist", surfaces: `${surfaces},a2a` })
    },
    cleanup() {
        if (surfaces != null) make("moqui.agent.AgentDefinition", { agentId: "assist", surfaces })
        ec.artifactExecution.enableAuthz()
    },
    tasks: [
        {
            name: "headless answer from data",
            say: "How many visits with an initial request starting with /agent-eval-hl/ came in during the last 7 days?",
            setup() {
                ;[1, 2, 10].forEach((days, index) => make("moqui.server.Visit", { visitId: `AGENT_EVAL_H${index + 1}`,
                    fromDate: daysAgo(days), initialRequest: `/agent-eval-hl/page${index + 1}`, clientIpAddress: "10.0.0.11" }))
            },
            cleanup() { for (let n = 1; n <= 3; n++) remove("moqui.server.Visit", "visitId", `AGENT_EVAL_H${n}`) },
            expect: {
                called: [{ tool: "data", args: { path: /VisitList/ } }],
                notCalled: ["write_ui"],
                results: [{ tool: "data", result: /showing 2 of 2/ }],
                answer: /\b2\b/,
                noInvented: true,
                noWrite: true,
            },
        },
        {
            name: "headless report of a record with its link",
            say: "What is the description of the user group AGENT_EVAL_HL? Give me a link to it.",
            setup() { make("moqui.security.UserGroup", { userGroupId: "AGENT_EVAL_HL", description: "Agent eval headless group" }) },
            cleanup() { remove("moqui.security.UserGroup", "userGroupId", "AGENT_EVAL_HL") },
            expect: {
                called: [{ tool: "screen", args: { go: /UserGroupDetail\?userGroupId=AGENT_EVAL_HL/ } }],
                answer: [/Agent eval headless group/, /\]\(\/apps\/system\/Security\/UserGroup\/UserGroupDetail\?userGroupId=AGENT_EVAL_HL\)/],
                noInvented: true,
                noWrite: true,
            },
        },
        {
            name: "headless write approved in the app",
            say: "Change the description of the user group AGENT_EVAL_HLW to Agent evaluation group, headless.",
            approvals: ["approve"],
            setup() { make("moqui.security.UserGroup", { userGroupId: "AGENT_EVAL_HLW", description: "Agent eval headless write" }) },
            cleanup() { remove("moqui.security.UserGroup", "userGroupId", "AGENT_EVAL_HLW") },
            expect: {
                held: { transition: "updateUserGroup", status: "executed" },
                results: [{ tool: "screen", result: /The user approved updateUserGroup, and it was saved/ }],
                answer: /saved|changed|updated/i,
            },
            verify() {
                assert(one("moqui.security.UserGroup", "userGroupId", "AGENT_EVAL_HLW").description == "Agent evaluation group, headless",
                    "the approved write did not change the group")
            },
        },
    ],
})
