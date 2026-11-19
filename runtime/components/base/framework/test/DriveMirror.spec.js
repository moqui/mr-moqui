// A mirror drive's URL is an administrator's (SEC-BOS-90): a member with manage on a mirror, who is no administrator,
// changes its name but not its URL, which the server fetches with the mirror's stored token; an administrator does, and a
// new URL clears the stored token unless a new one is given (the connections' endpoint-change rule) and may carry no
// user or password. The rows are written directly, so no fetch runs.
// Run: ./moq test framework --spec DriveMirror
const F = "org.moqui.resource.FileServices."
const MANAGER = "DRIVE_MIRROR_MGR"
const DRIVE = "SPEC_MIRROR"
const REPO = "DRIVE_SPEC_MIRROR"
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
function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
const repo = () => sys(() => ec.entity.find("moqui.resource.GitRepository").condition("repositoryId", REPO).one())
const store = (entityName, fields) => sys(() => ec.service.sync().name("store#" + entityName).parameters(fields).call())

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        sys(() => {
            if (ec.entity.find("moqui.security.UserAccount").condition("userId", MANAGER).count() === 0)
                ec.entity.makeValue("moqui.security.UserAccount").setAll({userId: MANAGER, username: "drive.mirror.mgr",
                    userFullName: "Mirror Manager", disabled: "N"}).create()
        })
        // the service as an app's screen grants it (Commons' DriveSettings, by inheritance): to a group of the manager
        store("moqui.security.UserGroup", {userGroupId: "SPEC_MIRROR_GRP", description: "Spec mirror managers"})
        store("moqui.security.UserGroupMember", {userGroupId: "SPEC_MIRROR_GRP", userId: MANAGER, fromDate: ec.user.nowTimestamp})
        store("moqui.security.UserGroupMember", {userGroupId: "SPEC_MIRROR_GRP", userId: "EX_JOHN_DOE", fromDate: ec.user.nowTimestamp})
        store("moqui.security.ArtifactGroup", {artifactGroupId: "SPEC_MIRROR_ART", description: "Spec mirror update"})
        store("moqui.security.ArtifactGroupMember", {artifactGroupId: "SPEC_MIRROR_ART", artifactName: F + "update#Drive",
            artifactTypeEnumId: "AT_SERVICE", nameIsPattern: "N", inheritAuthz: "Y"})
        store("moqui.security.ArtifactAuthz", {artifactAuthzId: "SPEC_MIRROR_AUTHZ", userGroupId: "SPEC_MIRROR_GRP", artifactGroupId: "SPEC_MIRROR_ART",
            authzTypeEnumId: "AUTHZT_ALWAYS", authzActionEnumId: "AUTHZA_ALL"})
        store("moqui.resource.GitRepository", {repositoryId: REPO, description: "Spec mirror", branch: "main", repositoryKind: "remote",
            remoteUrl: "https://git.example.com/team/docs.git", username: "bot", accessToken: "spec-secret-token", fetchIntervalMinutes: 60})
        store("moqui.resource.ResourceDrive", {driveId: DRIVE, driveName: "Spec mirror", driveKindEnumId: "RdkMirror",
            rootLocation: "git://" + REPO + "/", repositoryId: REPO, ownerUserId: "EX_JOHN_DOE", createdDate: ec.user.nowTimestamp})
        store("moqui.resource.ResourceDriveMember", {driveMemberId: "SPEC_MIRROR_MGR", driveId: DRIVE, userId: MANAGER,
            permissionEnumId: "RdpManage", fromDate: ec.user.nowTimestamp})
    },
    cleanupSpec() {
        ec.user.internalLoginUser("john.doe")
        sys(() => {
            ec.entity.find("moqui.resource.ResourceDriveMember").condition("driveId", DRIVE).deleteAll()
            ec.entity.find("moqui.resource.ResourceDrive").condition("driveId", DRIVE).deleteAll()
            ec.entity.find("moqui.resource.GitRepository").condition("repositoryId", REPO).deleteAll()
            ec.entity.find("moqui.security.ArtifactAuthz").condition("artifactAuthzId", "SPEC_MIRROR_AUTHZ").deleteAll()
            ec.entity.find("moqui.security.ArtifactGroupMember").condition("artifactGroupId", "SPEC_MIRROR_ART").deleteAll()
            ec.entity.find("moqui.security.ArtifactGroup").condition("artifactGroupId", "SPEC_MIRROR_ART").deleteAll()
            ec.entity.find("moqui.security.UserGroupMember").condition("userGroupId", "SPEC_MIRROR_GRP").deleteAll()
            ec.entity.find("moqui.security.UserGroup").condition("userGroupId", "SPEC_MIRROR_GRP").deleteAll()
            ec.entity.find("moqui.security.UserAccount").condition("userId", MANAGER).deleteAll()
        })
    },
})

feature("a mirror's manager renames it but does not repoint it; an administrator does, and the stored token goes unless given again", {
    when() {
        ec.user.internalLoginUser("drive.mirror.mgr")
        const repointed = invoke(F + "update#Drive", {driveId: DRIVE, remoteUrl: "https://attacker.example.net/x.git"})
        const renamed = invoke(F + "update#Drive", {driveId: DRIVE, driveName: "Spec mirror renamed"})
        const afterManager = repo()
        ec.user.internalLoginUser("john.doe")
        const withUser = invoke(F + "update#Drive", {driveId: DRIVE, remoteUrl: "https://bot:pw@git.example.org/docs.git"})
        const byAdmin = invoke(F + "update#Drive", {driveId: DRIVE, remoteUrl: "https://git.example.org/docs.git"})
        const afterAdmin = repo()
        return {repointed, renamed, afterManager, withUser, byAdmin, afterAdmin,
            drive: sys(() => ec.entity.find("moqui.resource.ResourceDrive").condition("driveId", DRIVE).one())}
    },
    then({repointed, renamed, afterManager, withUser, byAdmin, afterAdmin, drive}) {
        repointed.error.includes("Only an administrator changes the URL of a mirror")
        renamed.error == ""
        drive.driveName == "Spec mirror renamed"
        afterManager.remoteUrl == "https://git.example.com/team/docs.git"
        afterManager.accessToken == "spec-secret-token"
        withUser.error.includes("has no user or password in it")
        byAdmin.error == ""
        afterAdmin.remoteUrl == "https://git.example.org/docs.git"
        afterAdmin.accessToken == null
    },
})
