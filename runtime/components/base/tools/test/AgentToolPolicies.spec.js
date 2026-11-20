// SEC-AI-87: Tools › Policies stores no rule without a tool pattern: a row with none decides no tool and is left out of
// the agent's policy, so the form and its transition refuse it.
const one = (p) => ec.entity.find('moqui.agent.AgentToolPolicy').condition(p).disableAuthz().one()
spec({
    cleanup() {
        ec.artifactExecution.disableAuthz()
        ec.entity.find('moqui.agent.AgentToolPolicy').condition({agentId: 'assist', sequenceNum: 9901}).deleteAll()
        ec.message.clearErrors()
    },
})
feature("a policy rule is stored only with a tool pattern", {
    when() {
        login('john.doe')
        const store = (toolPattern) => {
            const render = screenTest('apps').render('system/Llm/Tools/Policies/storePolicy',
                {agentId: 'assist', sequenceNum: '9901', toolPattern, approvalModeEnumId: 'AgamNever'}, 'post')
            const errors = Array.from(render.errorMessages || [])
            ec.message.clearErrors()
            return errors
        }
        const blank = store('  ')
        const none = one({agentId: 'assist', sequenceNum: 9901})
        const named = store('test_never')
        return {blank, none, named, row: one({agentId: 'assist', sequenceNum: 9901})}
    },
    then({blank, none, named, row}) {
        blank.some(error => error.includes('needs a tool pattern'))
        none == null
        named.length == 0
        row.toolPattern == 'test_never'
    },
})
