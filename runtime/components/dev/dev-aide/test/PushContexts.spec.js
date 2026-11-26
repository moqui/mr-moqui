// A gitECA rule's condition against a push of the push log (ServiceWriterServices.get#PushContexts over
// GitServices.get#GitUpdateEventContexts, plans/design-dev-aide.md 10): the repositories a rule's repository-pattern
// takes, the push log of the one chosen, and the contexts the rule's event had from a change, as the events build them
// (crates/moqui-git events::context, whose Rust test replays real pushes), each marked whether the rule's ref-pattern
// takes its ref; then the condition tried against one in the sim. The change is a recorded push of a commit made by
// commit#GitFiles in a repository of the spec's own.
// Run: ./moq test dev-aide --spec PushContexts
const RUN = Date.now().toString(36).slice(-5).toUpperCase()
const REPO = "SPEC_DA_PUSH_" + RUN
const UPDATE = "SPEC_DA_PUSH_" + RUN
const svc = (name, parameters) => {
    const out = ec.service.sync().name("org.moqui.devaide.ServiceWriterServices." + name).parameters(parameters || {}).call() || {}
    const errors = ec.message.getErrorsString()
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
let commitId

spec({
    stepwise: true,
    requires: ["git"],
    setupSpec() {
        login("john.doe")
        ec.artifactExecution.disableAuthz()
        try {
            const make = (entity, fields) => {
                const row = ec.entity.makeValue(entity)
                row.setFields(fields, true, null, null)
                row.create()
            }
            make("moqui.resource.GitRepository", {repositoryId: REPO, repositoryKind: "db", description: "Dev AIDE push spec"})
            commitId = ec.service
                .sync()
                .name("org.moqui.git.GitServices.commit#GitFiles")
                .parameters({repositoryId: REPO, fileMap: {"a.txt": "one\n"}, message: "First commit\n\nTask: SPEC-1"})
                .call().commitId
            // the push of that commit, as the push log keeps it
            make("moqui.resource.GitUpdate", {updateId: UPDATE, repositoryId: REPO, source: "push", transport: "http",
                userId: ec.user.userId, updateDate: new Date(), priorTips: "", ecaStatus: "done"})
            make("moqui.resource.GitUpdateRef", {updateId: UPDATE, refName: "refs/heads/main", newObjectId: commitId,
                changeType: "create", result: "ok"})
        } finally {
            ec.artifactExecution.enableAuthz()
        }
    },
    cleanupSpec() {
        ec.artifactExecution.disableAuthz()
        try {
            ec.entity.find("moqui.resource.GitUpdateRef").condition("updateId", UPDATE).deleteAll()
            ec.entity.find("moqui.resource.GitUpdate").condition("updateId", UPDATE).deleteAll()
        } finally {
            ec.artifactExecution.enableAuthz()
        }
    }
})

feature("the repositories a rule's pattern takes, and the push log of the one chosen", {
    when() {
        const listed = svc("get#PushContexts", {repositoryPattern: "SPEC_DA_PUSH_.*"})
        const none = svc("get#PushContexts", {repositoryPattern: "NO_SUCH_.*"})
        const log = svc("get#PushContexts", {repositoryId: REPO})
        return {listed, none, log}
    },
    then({listed, none, log}) {
        listed.errors == ""
        listed.repositories.includes(REPO)
        none.repositories.length == 0
        log.updates.some(update => update.updateId == UPDATE && update.refList[0].shortName == "main")
    }
})

feature("a change's contexts for the rule's event, built as the events build them, marked by the rule's ref-pattern", {
    when() {
        const commit = svc("get#PushContexts", {repositoryId: REPO, updateId: UPDATE, event: "commit", refPattern: "refs/heads/main"})
        const refs = svc("get#PushContexts", {repositoryId: REPO, updateId: UPDATE, event: "ref-update", refPattern: "refs/tags/.*"})
        const unknown = svc("get#PushContexts", {repositoryId: REPO, updateId: UPDATE, event: "nothing"})
        return {commit, refs, unknown}
    },
    then({commit, refs, unknown}) {
        commit.errors == ""
        commit.contexts.length == 1
        commit.contexts[0].matches == true
        commit.contexts[0].context.commit.commitId == commitId
        commit.contexts[0].context.commit.summary == "First commit"
        commit.contexts[0].context.refName == "refs/heads/main"
        commit.contexts[0].context.updateId == UPDATE
        commit.contexts[0].context.source == "push"
        refs.contexts.length == 1
        refs.contexts[0].matches == false
        refs.contexts[0].context.changeType == "create"
        refs.contexts[0].context.isDefaultBranch == true
        unknown.errors.includes("no git event nothing")
    }
})

feature("the condition tried against that push in the sim", {
    when() {
        const contexts = svc("get#PushContexts", {repositoryId: REPO, updateId: UPDATE, event: "commit"}).contexts
        const against = JSON.stringify(contexts[0].context)
        const holds = svc("eval#Condition", {condition: "commit.summary.startsWith('First') && isDefaultBranch", against})
        const not = svc("eval#Condition", {condition: "refName == 'refs/heads/other'", against})
        return {holds, not}
    },
    then({holds, not}) {
        holds.holds == true
        not.holds == false
    }
})
