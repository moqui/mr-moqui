// Drives and files (org.moqui.resource.FileServices) on the real database: uploads as versions with notes on the database
// tree and as one commit on a git drive in the database, the list's modified-by from the path index, a change set, trash
// and restore, a public link with a password through its token check, and the MoquiDriveFile search document. The access
// rules, shares, quotas, the records drive and previews in full are the Rust tests in
// crates/moqui-core/src/service/files_tests.rs.

const SERVICES = "org.moqui.resource.FileServices"
let folderDriveId
let gitDriveId

const files = (verb, params) => call(`${SERVICES}.${verb}`, params || {})
const user = (userId) => {
    if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() > 0) return
    const row = ec.entity.makeValue("moqui.security.UserAccount")
    row.setFields({ userId: userId, username: userId, userFullName: `${userId} Example` }, true, null, null)
    row.create()
}
const names = (out) => out.fileList.map((row) => row.name)

spec({
    requires: ["git"],
    setup() { ec.artifactExecution.disableAuthz() },
    cleanup() { ec.artifactExecution.enableAuthz() },
})

feature("uploads keep versions with their notes, as history rows and as one commit", {
    when() {
        for (const id of ["filesOwner", "filesEditor"]) user(id)
        ec.user.internalLoginUser("filesOwner")
        folderDriveId = files("create#Drive", { driveName: "Spec folder", storage: "folder" }).driveId
        gitDriveId = files("create#Drive", { driveName: "Spec git", storage: "gitDb" }).driveId
        for (const driveId of [folderDriveId, gitDriveId])
            files("add#DriveMember", { driveId: driveId, userId: "filesEditor", permissionEnumId: "RdpEdit" })
        files("upload#Resource", { driveId: folderDriveId, path: "docs", versionNote: "First",
            fileList: [uploadedFile("plan.md", "plan one\n"), uploadedFile("notes.txt", "notes\n")] })
        const commit = files("upload#Resource", { driveId: gitDriveId, path: "docs", versionNote: "First drafts",
            fileList: [uploadedFile("a.md", "a\n"), uploadedFile("b.md", "b\n"), uploadedFile("c.md", "c\n")] })
        ec.user.internalLoginUser("filesEditor")
        files("upload#Resource", { driveId: folderDriveId, path: "docs", versionNote: "Second", file: uploadedFile("plan.md", "plan two\n") })
        files("upload#Resource", { driveId: gitDriveId, path: "docs", versionNote: "Edited b", file: uploadedFile("b.md", "b two\n") })
        const changeSet = ec.resource.makeChangeSet(`git://DRIVE_${gitDriveId}/`).write("docs/d.md", "d\n").move("docs/c.md", "old/c.md").commit("Tidy")
        ec.user.internalLoginUser("filesOwner")
        return { commit, changeSet,
            folderVersions: files("get#Versions", { driveId: folderDriveId, path: "docs/plan.md" }).versionList,
            gitList: files("list#Folder", { driveId: gitDriveId, path: "docs" }).fileList,
            moved: files("get#Versions", { driveId: gitDriveId, path: "old/c.md" }).versionList,
            log: ec.resource.getChangeLog(`git://DRIVE_${gitDriveId}/docs`, 10),
            drives: files("get#UserDrives").driveList, sized: files("get#UserDrives", { includeUsage: true }).driveList,
            usage: files("get#DriveUsage", { driveId: folderDriveId }).usageList }
    },
    then({ commit, changeSet, folderVersions, gitList, moved, log, drives, sized, usage }) {
        commit.versionName.length == 40
        commit.uploadedList.map((row) => row.outcome) == ["created", "created", "created"]
        folderVersions.map((version) => version.note) == ["Second", "First"]
        folderVersions[0].userName == "filesEditor Example"
        gitList.map((row) => row.name) == ["a.md", "b.md", "d.md"]
        gitList.find((row) => row.name === "a.md").modifiedByUserId == "filesOwner"
        gitList.find((row) => row.name === "a.md").versionNote == "First drafts"
        gitList.find((row) => row.name === "b.md").modifiedByUserId == "filesEditor"
        changeSet.applied == ["docs/d.md", "old/c.md"]
        moved.map((version) => version.note) == ["Tidy", "First drafts"]
        log.map((entry) => entry.note) == ["Tidy", "Edited b", "First drafts"]
        drives[0].isPersonal == true
        drives.find((drive) => drive.driveId === folderDriveId).usedBytes == null
        sized.find((drive) => drive.driveId === folderDriveId).usedBytes == 15
        usage[0].usedBytes == 15
    },
})

feature("trash and restore on the database tree and on git", {
    when() {
        ec.user.internalLoginUser("filesOwner")
        const out = {}
        for (const [key, driveId, path] of [["folder", folderDriveId, "docs"], ["git", gitDriveId, "docs/a.md"]]) {
            const trashed = files("trash#Resource", { driveId: driveId, path: path }).trashList
            out[key + "Gone"] = names(files("list#Folder", { driveId: driveId, path: path.includes("/") ? "docs" : "" }))
            files("restore#Resource", { trashId: trashed[0].trashId })
            out[key + "Back"] = names(files("list#Folder", { driveId: driveId, path: path.includes("/") ? "docs" : "" }))
        }
        out.folderVersions = files("get#Versions", { driveId: folderDriveId, path: "docs/plan.md" }).versionList.length
        return out
    },
    then({ folderGone, folderBack, gitGone, gitBack, folderVersions }) {
        folderGone == []
        folderBack == ["docs"]
        gitGone == ["b.md", "d.md"]
        gitBack == ["a.md", "b.md", "d.md"]
        folderVersions == 2
    },
})

feature("a link with a password: the token check, the grant, the listing, and a limit on its tries", {
    when() {
        ec.user.internalLoginUser("filesOwner")
        const link = files("share#Resource", { driveId: folderDriveId, path: "docs", isLink: "Y", password: "spec secret" })
        ec.user.logoutUser()
        const check = files("check#PublicShareToken", { token: link.linkToken })
        const bad = files("check#PublicShareToken", { token: "NoSuchTokenAtAll0000" })
        const opened = files("open#PublicShare", { token: link.linkToken, password: "spec secret" })
        const listed = files("list#PublicShare", { token: link.linkToken, grant: opened.grant })
        // SEC-FIL-05: ten tries of a link's password in ten minutes, the good one above among them; the next, even the right
        // one, is refused before the password is checked
        const tries = []
        for (const password of ["guess 1", "guess 2", "guess 3", "guess 4", "guess 5", "guess 6", "guess 7", "guess 8", "guess 9", "spec secret"]) {
            try {
                tries.push(files("open#PublicShare", { token: link.linkToken, password }).passwordOk === true ? "ok" : "no")
            } catch (error) {
                tries.push(String(error.message || error).includes("Too many tries") ? "limited" : "wrong")
            }
            ec.message.clearErrors()
        }
        ec.user.internalLoginUser("filesOwner")
        const links = files("get#Shares").linkList
        return { check, bad, opened, listed, links, tries }
    },
    then({ check, bad, opened, listed, links, tries }) {
        tries.join(",") == "wrong,wrong,wrong,wrong,wrong,wrong,wrong,wrong,wrong,limited"
        check.valid == true
        check.routeContext.needsPassword == true
        check.routeContext.sharerName == "filesOwner Example"
        bad.valid == false
        opened.passwordOk == true
        listed.fileList.map((row) => row.name) == ["notes.txt", "plan.md"]
        links[0].openedCount == 1
        links[0].hasPassword == true
    },
})

// SEC-BOS-106: an editor of a drive, or the holder of an Edit share of a path, shares with people and with a group of their
// own, never by a public link, with every user, with a group they are not in or with a group of external users; the owner does
feature("an editor of a drive shares with a person or a group of their own, not by a link, with every user, another group or external users", {
    allowMessageErrors: true,
    when() {
        for (const id of ["filesReader", "filesSharee"]) user(id)
        for (const [userGroupId, external] of [["FILES_SPEC_MINE", false], ["FILES_SPEC_OTHER", false], ["FILES_SPEC_EXTERNAL", true]]) {
            if (ec.entity.find("moqui.security.UserGroup").condition("userGroupId", userGroupId).count()) continue
            call("create#moqui.security.UserGroup", { userGroupId, description: userGroupId })
            if (userGroupId !== "FILES_SPEC_OTHER") call("create#moqui.security.UserGroupMember", { userGroupId, userId: "filesEditor", fromDate: 0 })
            if (external) call("create#moqui.security.UserGroupPermission", { userGroupId, userPermissionId: "EXTERNAL_USER", fromDate: 0 })
        }
        const refusal = (params) => {
            try { files("share#Resource", Object.assign({ driveId: folderDriveId, path: "docs" }, params)) } catch (e) { /* in the messages */ }
            const error = ec.message.getErrorsString()
            ec.message.clearErrors()
            return error
        }
        ec.user.internalLoginUser("filesOwner")
        // an Edit share of the path, not a membership of the drive
        files("share#Resource", { driveId: folderDriveId, path: "docs", sharedWithUserId: "filesSharee", permissionEnumId: "RspEdit" })
        ec.user.internalLoginUser("filesSharee")
        const shareeLink = refusal({ isLink: "Y" })
        const shareeEveryone = refusal({ sharedWithUserGroupId: "ALL_USERS" })
        ec.user.internalLoginUser("filesEditor")
        const link = refusal({ isLink: "Y" })
        const everyone = refusal({ sharedWithUserGroupId: "ALL_USERS" })
        const other = refusal({ sharedWithUserGroupId: "FILES_SPEC_OTHER" })
        const external = refusal({ sharedWithUserGroupId: "FILES_SPEC_EXTERNAL" })
        const mine = files("share#Resource", { driveId: folderDriveId, path: "docs", sharedWithUserGroupId: "FILES_SPEC_MINE" })
        const person = files("share#Resource", { driveId: folderDriveId, path: "docs", sharedWithUserId: "filesReader" })
        ec.user.internalLoginUser("filesOwner")
        const ownerLink = files("share#Resource", { driveId: folderDriveId, path: "docs", isLink: "Y" })
        const ownerOther = files("share#Resource", { driveId: folderDriveId, path: "docs", sharedWithUserGroupId: "FILES_SPEC_OTHER" })
        return { shareeLink, shareeEveryone, link, everyone, other, external, mine, person, ownerLink, ownerOther }
    },
    then({ shareeLink, shareeEveryone, link, everyone, other, external, mine, person, ownerLink, ownerOther }) {
        shareeLink.includes("Only a manager of the drive makes a link")
        shareeEveryone.includes("Only a manager of the drive shares with every user")
        link.includes("Only a manager of the drive makes a link")
        everyone.includes("Only a manager of the drive shares with every user")
        other.includes("You share with a group only when you are in it")
        external.includes("Only a manager of the drive shares with a group of external users")
        mine.shareId != null
        person.shareId != null
        ownerLink.linkToken != null
        ownerOther.shareId != null
    },
})

feature("drive files are search documents, found by name", {
    when() {
        ec.user.internalLoginUser("filesOwner")
        files("upload#Resource", { driveId: folderDriveId, path: "reports", file: uploadedFile("Quarterly Budget.csv", "a,b\n") })
        const tagId = files("set#Tags", { driveId: folderDriveId, path: "reports/Quarterly Budget.csv", tagNameList: ["finance"] }).tagList[0].tagId
        const found = files("find#Resources", { text: "quarterly" })
        const tagged = files("find#Resources", { text: "finance" })
        // by the tag: on the index a term on the document's tagIds list
        const byTag = files("find#Resources", { tagId: tagId })
        ec.user.internalLoginUser("filesEditor")
        const asEditor = files("find#Resources", { text: "quarterly" })
        return { found, tagged, byTag, asEditor, available: ec.search.isAvailable() }
    },
    then({ found, tagged, byTag, asEditor, available }) {
        found.usedIndex == available
        found.resultList.map((row) => row.path) == ["reports/Quarterly Budget.csv"]
        found.resultList[0].fileType == "spreadsheet"
        tagged.resultList.length == (available ? 1 : 0)
        byTag.usedIndex == available
        byTag.resultList.map((row) => row.path) == ["reports/Quarterly Budget.csv"]
        asEditor.resultList.length == 1
    },
})

// export#Drive of a folder drive by its manager, with authorization on: a named zip goes under runtime/drive-export, and a
// location with a scheme, an absolute path or '..' is refused (G10; the git drives' targetPath is export#GitRepository's)
feature("a folder drive's named export goes under runtime/drive-export; any other location is refused", {
    when() {
        const GROUP = "FILES_SPEC_EXPORT"
        const make = (entityName, fields) => {
            const {fromDate, ...keys} = fields
            if (ec.entity.find(entityName).condition(keys).count() > 0) return
            const row = ec.entity.makeValue(entityName)
            row.setFields(fields, true, null, null)
            row.create()
        }
        // the drive services as a screen grants them
        make("moqui.security.UserGroup", { userGroupId: GROUP, description: "Files spec export" })
        make("moqui.security.ArtifactGroup", { artifactGroupId: GROUP, description: "Files spec export" })
        make("moqui.security.ArtifactGroupMember", { artifactGroupId: GROUP, artifactTypeEnumId: "AT_SERVICE", inheritAuthz: "Y",
            nameIsPattern: "Y", artifactName: "org\\.moqui\\.resource\\.FileServices\\..*" })
        make("moqui.security.ArtifactAuthz", { artifactAuthzId: GROUP, userGroupId: GROUP, artifactGroupId: GROUP,
            authzTypeEnumId: "AUTHZT_ALWAYS", authzActionEnumId: "AUTHZA_ALL" })
        make("moqui.security.UserGroupMember", { userGroupId: GROUP, userId: "filesOwner", fromDate: 0 })
        ec.user.internalLoginUser("filesOwner")
        const exported = (targetLocation) => {
            ec.artifactExecution.enableAuthz()
            try {
                const out = ec.service.sync().name(`${SERVICES}.export#Drive`).parameters({ driveId: folderDriveId, targetLocation }).call() || {}
                return { location: out.exportLocation ?? null, errors: [...ec.message.errors].join(" | ") }
            } catch (err) {
                return { location: null, errors: String(err.message || err) }
            } finally {
                ec.message.clearErrors()
                ec.artifactExecution.disableAuthz()
            }
        }
        const named = exported("spec/files-export.zip")
        const zipThere = named.location ? ec.resource.getLocationReference(named.location).exists : false
        if (named.location) ec.resource.getLocationReference(named.location).delete()
        const outcome = {
            named, zipThere,
            absolute: exported("/tmp/files-spec-export.zip"),
            scheme: exported("component://example/files-export.zip"),
            up: exported("spec/../../conf/files-export.zip"),
        }
        ec.entity.find("moqui.security.UserGroupMember").condition("userGroupId", GROUP).deleteAll()
        ec.entity.find("moqui.security.ArtifactAuthz").condition("artifactAuthzId", GROUP).deleteAll()
        ec.entity.find("moqui.security.ArtifactGroupMember").condition("artifactGroupId", GROUP).deleteAll()
        ec.entity.find("moqui.security.ArtifactGroup").condition("artifactGroupId", GROUP).deleteAll()
        ec.entity.find("moqui.security.UserGroup").condition("userGroupId", GROUP).deleteAll()
        return outcome
    },
    then({ named, zipThere, absolute, scheme, up }) {
        named.errors == ""
        named.location == "runtime://drive-export/spec/files-export.zip"
        zipThere
        absolute.location == null
        absolute.errors.includes("runtime/drive-export")
        scheme.errors.includes("runtime/drive-export")
        up.errors.includes("runtime/drive-export")
    },
})

// export#Drive of a git drive needs only the view right on the drive (no delete right on its repository), with
// authorization on; a user who may not view it is refused, and a named targetPath still goes under runtime/git-export.
// The bare repository it writes (runtime/git-export/files-spec/, git-ignored) is written again by each run.
feature("a git drive's viewer exports it under runtime/git-export; one who may not view it is refused", {
    when() {
        const GROUP = "FILES_SPEC_GIT_EXPORT"
        const make = (entityName, fields) => {
            const {fromDate, ...keys} = fields
            if (ec.entity.find(entityName).condition(keys).count() > 0) return
            const row = ec.entity.makeValue(entityName)
            row.setFields(fields, true, null, null)
            row.create()
        }
        for (const id of ["filesViewer", "filesStranger"]) user(id)
        // the drive services as a screen grants them, to the viewer and to a user who is no member of the drive
        make("moqui.security.UserGroup", { userGroupId: GROUP, description: "Files spec git export" })
        make("moqui.security.ArtifactGroup", { artifactGroupId: GROUP, description: "Files spec git export" })
        make("moqui.security.ArtifactGroupMember", { artifactGroupId: GROUP, artifactTypeEnumId: "AT_SERVICE", inheritAuthz: "Y",
            nameIsPattern: "Y", artifactName: "org\\.moqui\\.resource\\.FileServices\\..*" })
        make("moqui.security.ArtifactAuthz", { artifactAuthzId: GROUP, userGroupId: GROUP, artifactGroupId: GROUP,
            authzTypeEnumId: "AUTHZT_ALWAYS", authzActionEnumId: "AUTHZA_ALL" })
        for (const id of ["filesViewer", "filesStranger"]) make("moqui.security.UserGroupMember", { userGroupId: GROUP, userId: id, fromDate: 0 })
        ec.user.internalLoginUser("filesOwner")
        files("add#DriveMember", { driveId: gitDriveId, userId: "filesViewer", permissionEnumId: "RdpView" })
        const exported = (userId, targetPath) => {
            ec.user.internalLoginUser(userId)
            ec.artifactExecution.enableAuthz()
            try {
                const out = ec.service.sync().name(`${SERVICES}.export#Drive`).parameters({ driveId: gitDriveId, targetPath }).call() || {}
                return { path: out.exportPath ?? null, count: out.fileCount ?? 0, errors: [...ec.message.errors].join(" | ") }
            } catch (err) {
                return { path: null, errors: String(err.message || err) }
            } finally {
                ec.message.clearErrors()
                ec.artifactExecution.disableAuthz()
            }
        }
        const outcome = {
            viewer: exported("filesViewer", "files-spec/viewer.git"),
            up: exported("filesViewer", "files-spec/../../conf"),
            stranger: exported("filesStranger", "files-spec/stranger.git"),
        }
        ec.user.internalLoginUser("filesOwner")
        ec.entity.find("moqui.security.UserGroupMember").condition("userGroupId", GROUP).deleteAll()
        ec.entity.find("moqui.security.ArtifactAuthz").condition("artifactAuthzId", GROUP).deleteAll()
        ec.entity.find("moqui.security.ArtifactGroupMember").condition("artifactGroupId", GROUP).deleteAll()
        ec.entity.find("moqui.security.ArtifactGroup").condition("artifactGroupId", GROUP).deleteAll()
        ec.entity.find("moqui.security.UserGroup").condition("userGroupId", GROUP).deleteAll()
        return outcome
    },
    then({ viewer, up, stranger }) {
        viewer.errors == ""
        viewer.path.endsWith("git-export/files-spec/viewer.git")
        up.path == null
        up.errors.includes("runtime/git-export")
        stranger.path == null
        stranger.errors.includes("Drive not found")
    },
})
