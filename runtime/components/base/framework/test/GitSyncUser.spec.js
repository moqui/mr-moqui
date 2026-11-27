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
            // the repository the syncs read and write (SEC-TLS-108)
            make("moqui.security.ArtifactGroupMember", {artifactGroupId: "GSU_SYNCERS", artifactName: "git-sync-spec", artifactTypeEnumId: "AT_GIT_REPOSITORY"})
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

feature("a holder of ADMIN_LOGIN_AS may not name an account in a privileged group they are not in", {
    when() {
        sys(() => {
            const from = new Date(Date.now() - 60000)
            make("moqui.security.UserAccount", {userId: "GSU_LOGIN_AS", username: "gsu.loginas", userFullName: "Git sync login as", disabled: "N"})
            make("moqui.security.UserGroupMember", {userGroupId: "GSU_SYNCERS", userId: "GSU_LOGIN_AS", fromDate: from})
            make("moqui.security.UserGroup", {userGroupId: "GSU_LOGIN_AS_G", description: "Git sync spec login as"})
            make("moqui.security.UserGroupMember", {userGroupId: "GSU_LOGIN_AS_G", userId: "GSU_LOGIN_AS", fromDate: from})
            make("moqui.security.UserGroupPermission", {userGroupId: "GSU_LOGIN_AS_G", userPermissionId: "ADMIN_LOGIN_AS", fromDate: from})
        })
        // john.doe is in ADMIN, a privileged group the holder is not in (user::held_beyond)
        const admin = storeAs("gsu.loginas", {syncId: "GSU_AS_ADMIN", userId: "EX_JOHN_DOE"})
        const plain = storeAs("gsu.loginas", {syncId: "GSU_AS_SYNCER", userId: "GSU_SYNCER"})
        return {admin, plain}
    },
    then({admin, plain}) {
        admin.includes("privileged group ADMIN")
        plain == ""
    },
})
feature("a sync runs as another user only while its storer still holds ADMIN_LOGIN_AS", {
    when() {
        // stored by one who held the grant then and holds it no more: written as the framework
        sys(() => make("moqui.report.DefinitionGitSync", {syncId: "GSU_STALE", folder: "GitSyncSpec", repositoryId: "git-sync-spec",
            userId: "GSU_LOGIN_AS", storedByUserId: "GSU_SYNCER", enabled: "Y"}))
        let error = ""
        try { call("org.moqui.report.ReportServices.push#DefinitionGitSync", {syncId: "GSU_STALE"}) } catch (e) { error = String(e.message) }
        error += ec.message.getErrorsString()
        ec.message.clearErrors()
        return {error}
    },
    then({error}) {
        error.includes("no longer holds ADMIN_LOGIN_AS")
    },
})

// SEC-TLS-108: a sync reads and writes its repository through git:// resources, which check no access of their own, so
// storing one asks the caller's update of the repository, and a run asks its user's (update to push, view to pull)
feature("a sync of a repository its storer or its user may not write is refused", {
    when() {
        sys(() => make("moqui.resource.GitRepository", {repositoryId: "git-sync-closed"}))
        const stored = storeAs("gsu.syncer", {syncId: "GSU_CLOSED", userId: "GSU_SYNCER", repositoryId: "git-sync-closed"})
        sys(() => make("moqui.report.DefinitionGitSync", {syncId: "GSU_CLOSED_RUN", folder: "GitSyncSpec", repositoryId: "git-sync-closed",
            userId: "GSU_SYNCER", storedByUserId: "GSU_SYNCER", enabled: "Y"}))
        let pushed = ""
        try { call("org.moqui.report.ReportServices.push#DefinitionGitSync", {syncId: "GSU_CLOSED_RUN"}) } catch (e) { pushed = String(e.message) }
        pushed += ec.message.getErrorsString()
        ec.message.clearErrors()
        const row = sys(() => ec.entity.find("moqui.report.DefinitionGitSync").condition("syncId", "GSU_CLOSED").one())
        return {stored, pushed, row}
    },
    then({stored, pushed, row}) {
        stored.includes("git-sync-closed")
        row == null
        pushed.includes("git-sync-closed")
    },
})
