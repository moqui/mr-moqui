// Entity Sync Detail's Update stores the form's fields (plans/ui-review-system-ai-integrations.md AI-UI-005): its
// service-call is inside actions, which pass no parameters without an in-map, so the update failed on a missing
// entitySyncId. An empty password field keeps the stored password and an entered one replaces it (SEC-TLS-116). The
// transition is posted as its form posts it; no screen is rendered.
// Run: ./moq test tools --spec EntitySyncDetail
const SYNC = "moqui.entity.sync.EntitySync"
const entitySyncId = "SPEC_SYNC_DETAIL"
const server = "https://sync.example.com"

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try { return run() } finally { if (!wasDisabled) ec.artifactExecution.enableAuthz() }
}
const stored = () => sys(() => ec.entity.find(SYNC).condition({entitySyncId}).useCache(false).one())
// the Detail form's post, with the stored address and username so no secret moves
function update(fields) {
    const test = ec.screen.makeTest().rootScreen("component://tools/screen/System.xml").renderMode("html")
    const result = test.render("EntitySync/EntitySyncDetail/updateEntitySync", Object.assign({moquiFormName: "EntitySyncForm",
        entitySyncId, description: "Spec sync detail", recordThreshold: "", targetServerUrl: server, targetUsername: "sync",
        targetPath: "", forPull: "N"}, fields), "post")
    const errors = Array.from(result.errorMessages || []).join(" | ")
    ec.message.clearAll()
    return {status: result.status, errors}
}

spec({
    setupSpec() {
        login("john.doe")
        sys(() => {
            const row = ec.entity.makeValue(SYNC)
            row.setFields({entitySyncId, statusId: "EsNotStarted", syncSplitMillis: 1000, delayBufferMillis: 300000,
                targetServerUrl: server, targetUsername: "sync", targetPassword: "spec-sync-pass-one"}, true, null, null)
            row.create()
        })
    },
    cleanupSpec() {
        sys(() => ec.entity.find(SYNC).condition({entitySyncId}).deleteAll())
    },
})
feature("the Detail form's Update stores its fields and keeps the password unless one is entered", {
    when() {
        const kept = update({syncSplitMillis: "60000", delayBufferMillis: "5000", targetPassword: ""})
        const afterKept = stored()
        const changed = update({syncSplitMillis: "120000", delayBufferMillis: "5000", targetPassword: "spec-sync-pass-two"})
        const afterChanged = stored()
        return {kept, afterKept, changed, afterChanged}
    },
    then({kept, afterKept, changed, afterChanged}) {
        kept.errors == ""
        kept.status == 302
        afterKept.syncSplitMillis == 60000
        afterKept.delayBufferMillis == 5000
        afterKept.description == "Spec sync detail"
        afterKept.targetPassword == "spec-sync-pass-one"
        changed.errors == ""
        afterChanged.syncSplitMillis == 120000
        afterChanged.targetPassword == "spec-sync-pass-two"
    },
})
