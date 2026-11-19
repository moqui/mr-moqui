// A definition git sync's user (crates/moqui-report/src/git_sync.rs, SEC-TLS-109): a push and a pull sign in as the
// sync's user, so store#DefinitionGitSync takes another user's id only from a holder of ADMIN_LOGIN_AS, and never an
// account holding access the caller lacks (Identity.js heldBeyond). Run: ./moq test framework --spec GitSyncUser
const STORE = "org.moqui.report.ReportServices.store#DefinitionGitSync"
function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try { return run() } finally { if (!wasDisabled) ec.artifactExecution.enableAuthz() }
}
function make(entityName, fields) {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
// a store as a user, with authorization on: its error, or ""
function storeAs(username, parameters) {
    ec.user.internalLoginUser(username)
    ec.artifactExecution.enableAuthz()
    try {
        call(STORE, Object.assign({folder: "GitSyncSpec", repositoryId: "git-sync-spec"}, parameters))
        return ec.message.getErrorsString()
    } catch (error) {
        return String(error.message)
    } finally {
        ec.message.clearErrors()
        login("john.doe")
    }
}
spec({ setupSpec() { login("john.doe") } })
feature("a sync runs as another user only for a holder of ADMIN_LOGIN_AS", {
    when() {
        sys(() => {
            const from = new Date(Date.now() - 60000)
            make("moqui.resource.GitRepository", {repositoryId: "git-sync-spec"})
            make("moqui.security.UserAccount", {userId: "GSU_SYNCER", username: "gsu.syncer", userFullName: "Git sync spec", disabled: "N"})
            make("moqui.security.UserGroup", {userGroupId: "GSU_SYNCERS", description: "Git sync spec"})
            make("moqui.security.UserGroupMember", {userGroupId: "GSU_SYNCERS", userId: "GSU_SYNCER", fromDate: from})
            make("moqui.security.ArtifactGroup", {artifactGroupId: "GSU_SYNCERS", description: "Git sync spec"})
            make("moqui.security.ArtifactGroupMember", {artifactGroupId: "GSU_SYNCERS", artifactName: "org.moqui.report.gitSync", artifactTypeEnumId: "AT_SERVICE"})
            make("moqui.security.ArtifactGroupMember", {artifactGroupId: "GSU_SYNCERS", artifactName: STORE, artifactTypeEnumId: "AT_SERVICE"})
            make("moqui.security.ArtifactAuthz", {artifactAuthzId: "GSU_SYNCERS", artifactGroupId: "GSU_SYNCERS", userGroupId: "GSU_SYNCERS",
                authzTypeEnumId: "AUTHZT_ALLOW", authzActionEnumId: "AUTHZA_ALL"})
        })
        const other = storeAs("gsu.syncer", {syncId: "GSU_OTHER", userId: "EX_JOHN_DOE"})
        const own = storeAs("gsu.syncer", {syncId: "GSU_OWN", userId: "GSU_SYNCER"})
        const stored = Array.from(sys(() => ec.entity.find("moqui.report.DefinitionGitSync").condition("syncId", "in", ["GSU_OTHER", "GSU_OWN"]).list()))
            .map(row => row.syncId + ":" + row.userId).sort().join(",")
        return {other, own, stored}
    },
    then({other, own, stored}) {
        other.includes("ADMIN_LOGIN_AS")
        own == ""
        stored == "GSU_OWN:GSU_SYNCER"
    },
})
