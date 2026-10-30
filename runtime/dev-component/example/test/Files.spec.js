// Drives and files (org.moqui.impl.FileServices) on the real database: uploads as versions with notes on the database
// tree and as one commit on a git drive in the database, the list's modified-by from the path index, a change set, trash
// and restore, a public link with a password through its token check, and the MoquiDriveFile search document. The access
// rules, shares, quotas, the records drive and previews in full are the Rust tests in
// crates/moqui-core/src/service/files_tests.rs.

const SERVICES = "org.moqui.impl.FileServices"
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

feature("a link with a password: the token check, the grant, the listing", {
    when() {
        ec.user.internalLoginUser("filesOwner")
        const link = files("share#Resource", { driveId: folderDriveId, path: "docs", isLink: "Y", password: "spec secret" })
        ec.user.logoutUser()
        const check = files("check#PublicShareToken", { token: link.linkToken })
        const bad = files("check#PublicShareToken", { token: "NoSuchTokenAtAll0000" })
        const opened = files("open#PublicShare", { token: link.linkToken, password: "spec secret" })
        const listed = files("list#PublicShare", { token: link.linkToken, grant: opened.grant })
        ec.user.internalLoginUser("filesOwner")
        const links = files("get#Shares").linkList
        return { check, bad, opened, listed, links }
    },
    then({ check, bad, opened, listed, links }) {
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
