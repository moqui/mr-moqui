// A DataDocument field expression (a fieldPath with "(") is script that runs, as whoever reads the document, on the
// server, so writing one needs DATABASE_ADMIN (Security.eecas.xml DataDocumentFieldExpression,
// crates/moqui-core/src/service/security_checks.rs assert_document_expression). So does a document title or a link URL
// or label that is a template (it has "${"), and a link condition (DataDocumentTitleTemplate, DataDocumentLinkTemplate;
// plans/archive/Security-Review.md SEC-SCR-05). A plain field path or text needs nothing more, an update that leaves a template
// as it was passes, and a load with authorization off, as seed data loads, is allowed.
// Run: ./moq test framework --spec DocumentExpression
const adminId = "SPEC_DX_ADMIN"
const dbaId = "SPEC_DX_DBA"
const documentId = "SPEC_DX_DOC"
const make = (entityName, fields) => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
const fieldOf = (seqId) => ec.entity.find("moqui.entity.document.DataDocumentField").condition("dataDocumentId", documentId)
    .condition("fieldSeqId", seqId).disableAuthz().one()
// writes one field as the user, authorized as from the System app's DataDocument screens, and gives the error text
// (empty when it was written)
const writeAs = (username, seqId, fieldPath, alias) => {
    ec.user.internalLoginUser(username)
    let error = ""
    try {
        ec.artifactExecution.runAsUser("component://tools/screen/System.xml", () =>
            make("moqui.entity.document.DataDocumentField", { dataDocumentId: documentId, fieldSeqId: seqId, fieldPath, fieldNameAlias: alias }))
    } catch (e) {
        error = String(e)
    }
    error += ec.message.errorsString
    ec.message.clearErrors()
    ec.user.logoutUser()
    return error
}

spec({
    setupSpec() {
        ec.artifactExecution.disableAuthz()
        // an ADMIN who is not in ADMIN_ADV, so without DATABASE_ADMIN
        make("moqui.security.UserAccount", { userId: adminId, username: "spec.dx.admin", userFullName: "Spec Expression" })
        make("moqui.security.UserGroupMember", { userGroupId: "ADMIN", userId: adminId, fromDate: ec.user.nowTimestamp })
        // an ADMIN with DATABASE_ADMIN through a group of its own (granted with authorization off, as a seed load would)
        make("moqui.security.UserAccount", { userId: dbaId, username: "spec.dx.dba", userFullName: "Spec Expression DBA" })
        make("moqui.security.UserGroupMember", { userGroupId: "ADMIN", userId: dbaId, fromDate: ec.user.nowTimestamp })
        make("moqui.security.UserGroup", { userGroupId: "SPEC_DX_DBA_GROUP", description: "Spec expression DBA" })
        make("moqui.security.UserGroupPermission", { userGroupId: "SPEC_DX_DBA_GROUP", userPermissionId: "DATABASE_ADMIN", fromDate: ec.user.nowTimestamp })
        make("moqui.security.UserGroupMember", { userGroupId: "SPEC_DX_DBA_GROUP", userId: dbaId, fromDate: ec.user.nowTimestamp })
        make("moqui.entity.document.DataDocument", { dataDocumentId: documentId, documentName: "Spec expression", primaryEntityName: "moqui.basic.Enumeration",
            documentTitle: "Seeded ${enumId}" })
        make("moqui.entity.document.DataDocumentLink", { dataDocumentId: documentId, linkSeqId: "01", label: "Seeded ${enumId}", linkUrl: "/x?id=${enumId}" })
        // with authorization off, as seed data loads: an expression is written
        make("moqui.entity.document.DataDocumentField", { dataDocumentId: documentId, fieldSeqId: "01", fieldPath: "(enumId + '!')", fieldNameAlias: "seeded" })
        ec.artifactExecution.enableAuthz()
    },
    cleanupSpec() {
        ec.artifactExecution.disableAuthz()
        ec.entity.find("moqui.entity.document.DataDocumentField").condition("dataDocumentId", documentId).deleteAll()
        ec.entity.find("moqui.entity.document.DataDocumentLink").condition("dataDocumentId", documentId).deleteAll()
        ec.entity.find("moqui.entity.document.DataDocument").condition("dataDocumentId", documentId).deleteAll()
        for (const userId of [adminId, dbaId]) {
            ec.entity.find("moqui.security.UserGroupMember").condition("userId", userId).deleteAll()
            ec.entity.find("moqui.security.UserAccount").condition("userId", userId).deleteAll()
        }
        ec.entity.find("moqui.security.UserGroupPermission").condition("userGroupId", "SPEC_DX_DBA_GROUP").deleteAll()
        ec.entity.find("moqui.security.UserGroup").condition("userGroupId", "SPEC_DX_DBA_GROUP").deleteAll()
        ec.artifactExecution.enableAuthz()
    },
})

feature("a DataDocument field expression needs DATABASE_ADMIN; a plain path and a seed load do not", {
    expect() {
        // (in a function: each statement at the top of expect is a condition)
        const run = () => ({
            refused: writeAs("spec.dx.admin", "02", "(enumId + ' / ' + description)", "label"),
            plain: writeAs("spec.dx.admin", "03", "description", null),
            allowed: writeAs("spec.dx.dba", "04", "(enumId + ' / ' + description)", "label2"),
        })
        const out = run()
        out.refused.includes("DATABASE_ADMIN")
        fieldOf("02") == null
        out.plain === ""
        fieldOf("03") != null
        out.allowed === ""
        fieldOf("04") != null
        fieldOf("01") != null
    },
})

// a write of the document or one of its links as the user, authorized as from the System app, and the error text
const changeAs = (username, change) => {
    ec.user.internalLoginUser(username)
    let error = ""
    try {
        ec.artifactExecution.runAsUser("component://tools/screen/System.xml", change)
    } catch (e) {
        error = String(e)
    }
    error += ec.message.errorsString
    ec.message.clearErrors()
    ec.user.logoutUser()
    return error
}
const documentRow = () => ec.entity.find("moqui.entity.document.DataDocument").condition("dataDocumentId", documentId).disableAuthz().one()
const linkOf = (seqId) => ec.entity.find("moqui.entity.document.DataDocumentLink").condition("dataDocumentId", documentId)
    .condition("linkSeqId", seqId).disableAuthz().one()
const setDocument = (fields) => () => {
    const row = ec.entity.find("moqui.entity.document.DataDocument").condition("dataDocumentId", documentId).one()
    row.setFields(fields, true, null, null)
    row.update()
}
const addLink = (fields) => () => make("moqui.entity.document.DataDocumentLink", Object.assign({ dataDocumentId: documentId }, fields))

feature("a DataDocument title or link template, and a link condition, need DATABASE_ADMIN; plain text and other fields do not", {
    expect() {
        const run = () => ({
            // the seeded template title stays while another field changes
            otherField: changeAs("spec.dx.admin", setDocument({ documentName: "Spec expression renamed" })),
            title: changeAs("spec.dx.admin", setDocument({ documentTitle: "Hit ${ec.factory}" })),
            titleAfter: String(documentRow().documentTitle),
            plainTitle: changeAs("spec.dx.admin", setDocument({ documentTitle: "Plain title" })),
            dbaTitle: changeAs("spec.dx.dba", setDocument({ documentTitle: "Hit ${enumId}" })),
            linkUrl: changeAs("spec.dx.admin", addLink({ linkSeqId: "02", label: "Open", linkUrl: "/x?id=${enumId}" })),
            linkLabel: changeAs("spec.dx.admin", addLink({ linkSeqId: "03", label: "${enumId}", linkUrl: "/x" })),
            linkCondition: changeAs("spec.dx.admin", addLink({ linkSeqId: "04", label: "Open", linkUrl: "/x", linkCondition: "true" })),
            plainLink: changeAs("spec.dx.admin", addLink({ linkSeqId: "05", label: "Open", linkUrl: "/x" })),
            dbaLink: changeAs("spec.dx.dba", addLink({ linkSeqId: "06", label: "${enumId}", linkUrl: "/x?id=${enumId}", linkCondition: "true" })),
        })
        const out = run()
        out.otherField === ""
        out.title.includes("DATABASE_ADMIN")
        out.titleAfter == "Seeded ${enumId}"
        out.plainTitle === ""
        out.dbaTitle === ""
        String(documentRow().documentTitle) == "Hit ${enumId}"
        out.linkUrl.includes("DATABASE_ADMIN") && linkOf("02") == null
        out.linkLabel.includes("DATABASE_ADMIN") && linkOf("03") == null
        out.linkCondition.includes("DATABASE_ADMIN") && linkOf("04") == null
        out.plainLink === "" && linkOf("05") != null
        out.dbaLink === "" && linkOf("06") != null
        linkOf("01") != null
    },
})
