// Entity data export (screen/Tools/Data/Export.xml, EntityExport): its filter is a JSON object of field values, parsed
// and never evaluated, since an expression there ran as a server script with the caller's ec (plans/archive/Security-Review.md
// SEC-TLS-01). The transition is not read-only, so a GET with URL parameters does not run it: the client check of that
// is in framework/test/client/SecurityScreens.spec.js.
// Run: ./moq test tools --spec EntityExport
const marker = "runtime://tmp/EntityExportSpec.ran"
const ran = () => !!ec.resource.getLocationReference(marker)?.getExists()
const remove = () => {
    if (ran()) ec.resource.getLocationReference(marker).delete()
}
const exportOf = (filterMap) => ec.screen.makeTest().rootScreen("component://tools/screen/Tools.xml").render("Data/Export/EntityExport",
    { entityNames: "moqui.basic.Enumeration", filterMap, fileType: "JSON", output: "browser" }, "post")

spec({
    setupSpec() { login("john.doe") },
    cleanup() { remove() },
})

feature("a script filter is refused without running; a JSON filter selects the records", {
    when() {
        remove()
        const script = exportOf(`(function () { ec.resource.getLocationReference('${marker}').putText('ran'); return {enumTypeId: 'X'} })()`)
        const scriptRan = ran()
        ec.message.clearErrors()
        const json = exportOf('{"enumTypeId": "EmailType"}')
        ec.message.clearErrors()
        return { script, scriptRan, json }
    },
    then({ script, scriptRan, json }) {
        !scriptRan
        script.errorMessages.join(" ").includes("not a JSON object")
        json.errorMessages.length == 0
        json.output.includes("EmailType")
        !json.output.includes("\"enumTypeId\": \"StatusType\"") && !json.output.includes("\"enumTypeId\":\"StatusType\"")
    },
})

// plans/archive/Security-Review-Medium.md SEC-TLS-05: AutoEditMaster's export refuses the entities its update refuses (the
// parent screen's check is in its actions, which a transition does not run). plans/archive/Security-Review-Low.md SEC-TLS-07:
// a snapshot's baseFilename is one path segment, so the zip is never written outside runtime/db/snapshot.
feature("a record export refuses a restricted entity, and a snapshot name with a path is refused", {
    when() {
        const record = ec.screen.makeTest().rootScreen("component://tools/screen/Tools.xml").render(
            "Data/Browse/AutoEdit/AutoEditMaster/export",
            { aen: "moqui.security.UserLoginKey", loginKey: "none", fileType: "JSON", output: "browser" }, "post")
        ec.message.clearErrors()
        const escaped = "runtime://db/EntityExportSpecEscape.zip"
        // the job runs it as its user (Snapshots' exportSnapshot); the check is the service's own, so authz is off here
        ec.artifactExecution.disableAuthz()
        try {
            ec.service.sync().name("org.moqui.impl.EntityServices.export#EntityDataSnapshot")
                .parameters({ entitiesToInclude: ["moqui.basic.Uom"], baseFilename: "../EntityExportSpecEscape" }).call()
        } finally {
            ec.artifactExecution.enableAuthz()
        }
        const errors = ec.message.getErrorsString()
        ec.message.clearErrors()
        const written = !!ec.resource.getLocationReference(escaped)?.getExists()
        if (written) ec.resource.getLocationReference(escaped).delete()
        return { record, errors, written }
    },
    then({ record, errors, written }) {
        record.errorMessages.join(" ").includes("not available through this tool")
        errors.includes("one file name")
        !written
    },
})

// SEC-DAT-89: a snapshot import restores any record and runs its service elements, so it needs the sealed
// ADMIN_BACKUP_DOWNLOAD (an ADMIN without it is refused before anything is read); john.doe holds it
feature("a snapshot import needs ADMIN_BACKUP_DOWNLOAD", {
    when() {
        const groupId = "SPEC_SNAPSHOT_ADMINS"
        const was = ec.artifactExecution.disableAuthz()
        try {
            for (const [entity, fields] of [
                ["moqui.security.UserAccount", {userId: "SPEC_SNAP", username: "spec.snap", userFullName: "Spec snapshot"}],
                ["moqui.security.UserGroup", {userGroupId: groupId, description: "Spec snapshot"}],
                ["moqui.security.UserGroupMember", {userGroupId: groupId, userId: "SPEC_SNAP", fromDate: 0}],
            ]) {
                const row = ec.entity.makeValue(entity)
                row.setFields(fields, true, null, null)
                row.create()
            }
        } finally { if (!was) ec.artifactExecution.enableAuthz() }
        const importAs = (username) => {
            ec.user.internalLoginUser(username)
            try {
                ec.service.sync().name("org.moqui.impl.EntityServices.import#EntityDataSnapshot")
                    .parameters({zipFilename: "NoSuchSpecSnapshot.zip"}).disableAuthz().call()
                return ec.message.getErrorsString()
            } finally {
                ec.message.clearErrors()
                ec.user.internalLoginUser("john.doe")
            }
        }
        return {without: importAs("spec.snap"), holder: importAs("john.doe")}
    },
    then({without, holder}) {
        without.includes("needs ADMIN_BACKUP_DOWNLOAD")
        !holder.includes("needs ADMIN_BACKUP_DOWNLOAD")
    },
})

