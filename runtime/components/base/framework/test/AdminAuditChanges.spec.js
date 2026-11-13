// Change evidence, org.moqui.admin.AuditServices (mr-boss plan/impl-admin.md P17; design-admin §12 AuditChanges): the audit
// log holds every audited value of every entity, so find#Changes answers only registered projections (never an entity or a
// field a request names), a secret is a marker whatever its registration says, a projection whose permission the caller
// lacks is not answered through its entity's every-other-field registration either, a registration that does not read is
// left out, a find is bounded, and reading needs ADMIN_AUDIT_VIEW. The owner-source path (a record that belongs to an
// organization) is tested where an owner is: mr-mantle test/party/IdentityAudit.
// Run: ./moq test framework --spec AdminAuditChanges
const A = "org.moqui.admin.AuditServices."
const LOG = "moqui.entity.EntityAuditLog"
const PROJECTION = "moqui.server.AuditProjection"
const SERVER = "moqui.basic.email.EmailServer"
const users = {view: "SPEC_AUD_VIEW", none: "SPEC_AUD_NONE"}
const GROUP = "SPEC_AUD_READERS"
const RELAY = "SPEC_AUD_RELAY"
const SECRET_OLD = "enc:c3BlYy1vbGQtc2VjcmV0"
const SECRET_NEW = "enc:c3BlYy1uZXctc2VjcmV0"

const find = (entityName, conditions = {}) => ec.entity.find(entityName).condition(conditions).disableAuthz()
const make = (entityName, fields) => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    if (entityName === LOG) row.setSequencedIdPrimary()
    row.create()
}
function as(userId, action) {
    ec.user.internalLoginUser(userId.toLowerCase().replace(/_/g, "."))
    ec.artifactExecution.enableAuthz()
    try {
        return action()
    } finally {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
    }
}
function invoke(name, parameters = {}) {
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
const has = (value, pattern) => pattern.test(String(value ?? ""))
const now = () => Number(ec.user.nowTimestamp)
let made = 0
// An audit row as the entity audit log or a service's auditRow writes one, a second apart so the order is known.
const audit = (changedEntityName, changedFieldName, fields = {}) =>
    make(
        LOG,
        Object.assign(
            {
                changedEntityName,
                changedFieldName,
                pkPrimaryValue: RELAY,
                changedDate: now() - 60000 + ++made * 1000,
                changedByUserId: users.none
            },
            fields
        )
    )
const changes = (parameters = {}) => invoke(A + "find#Changes", Object.assign({recordKey: RELAY}, parameters))
const rowOf = (out, fieldName) => Array.from(out.changeList || []).find(row => row.changedFieldName === fieldName)
const projection = (auditProjectionId, fields) =>
    make(PROJECTION, Object.assign({auditProjectionId, changedEntityName: SERVER, category: "connection"}, fields))

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        for (const [key, userId] of Object.entries(users))
            make("moqui.security.UserAccount", {
                userId,
                username: userId.toLowerCase().replace(/_/g, "."),
                userFullName: "Spec audit " + key
            })
        make("moqui.security.UserGroup", {userGroupId: GROUP, description: "Spec audit readers"})
        make("moqui.security.UserGroupMember", {userGroupId: GROUP, userId: users.view, fromDate: 0})
        make("moqui.security.UserGroupPermission", {userGroupId: GROUP, userPermissionId: "ADMIN_AUDIT_VIEW", fromDate: 0})
        // registrations a spec adds: one that says show for an encrypted field, one whose permission the reader lacks, and
        // three that do not read
        projection("SPEC_AUD_SECRET", {changedFieldName: "smtpPassword", valueMode: "show", description: "Spec relay password"})
        projection("SPEC_AUD_PERM", {
            changedFieldName: "webBaseUrl",
            valueMode: "show",
            viewPermission: "ADMIN_CONNECTION_VIEW",
            description: "Spec relay address"
        })
        // an event about a password is not one; a field named as one is, whatever its registration says
        projection("SPEC_AUD_EVENT", {changedFieldName: "passwordReset", valueMode: "show", description: "Spec reset sent"})
        projection("SPEC_AUD_NAMED", {changedFieldName: "resetPassword", valueMode: "show", description: "Spec reset password"})
        projection("SPEC_AUD_NOENT", {changedEntityName: "spec.audit.Missing", changedFieldName: "name", valueMode: "show"})
        projection("SPEC_AUD_NOSRC", {
            changedFieldName: "storePort",
            valueMode: "show",
            sourceService: "spec.audit.MissingServices.find#Changes"
        })
        projection("SPEC_AUD_NOCON", {changedFieldName: "storeProtocol", valueMode: "show", sourceService: A + "find#Changes"})
        // what the log holds of the relay, oldest first
        audit(SERVER, "disabled", {oldValueText: "N", newValueText: "Y", changeReason: "Spec\u0007 <b>repoint</b> ${1+1}"})
        audit(SERVER, "smtpPassword", {oldValueText: SECRET_OLD, newValueText: SECRET_NEW})
        audit(SERVER, "mailPassword", {oldValueText: "(set)", newValueText: "(replaced)", changeReason: "Rotated"})
        audit(SERVER, "smtpHost", {oldValueText: "old.spec.example", newValueText: "new.spec.example"})
        audit(SERVER, "webBaseUrl", {oldValueText: "https://old.spec.example", newValueText: "https://new.spec.example"})
        audit(SERVER, "storePort", {newValueText: "993"})
        audit(SERVER, "storeProtocol", {newValueText: "imaps"})
        // an entity nothing registers here, keyed like the relay
        audit("moqui.security.UserAccount", "emailAddress", {oldValueText: "old@spec.example", newValueText: "new@spec.example"})
        audit("moqui.basic.Enumeration", "description", {newValueText: "Spec unregistered"})
        audit(SERVER, "passwordReset", {newValueText: "Reset mailed"})
        audit(SERVER, "resetPassword", {newValueText: "spec-plain-reset-password"})
    },
    cleanupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        // the audit log is create-only: its rows stay, keyed by the spec's own relay and users
        find(PROJECTION).condition("auditProjectionId", "like", "SPEC_AUD_%").deleteAll()
        find("moqui.security.UserGroupPermission", {userGroupId: GROUP}).deleteAll()
        find("moqui.security.UserGroupMember", {userGroupId: GROUP}).deleteAll()
        find("moqui.security.UserGroup", {userGroupId: GROUP}).deleteAll()
        for (const userId of Object.values(users)) find("moqui.security.UserAccount", {userId}).deleteAll()
    }
})

feature("only registered projections are answered, and a secret is never a value", {
    when() {
        const out = {}
        out.stranger = as(users.none, () => changes())
        as(users.view, () => {
            out.all = changes()
            out.byProjection = changes({auditProjectionId: "EmailServerDisabled"})
            out.byCategory = changes({category: "connection"})
            out.otherCategory = changes({category: "access"})
            out.byActor = changes({changedByUserId: users.view})
            // a request never names an entity or a field, an unregistered or an unreadable projection, or an inherited name
            out.unregistered = changes({auditProjectionId: "moqui.security.UserAccount"})
            out.unreadable = changes({auditProjectionId: "SPEC_AUD_PERM"})
            out.inherited = changes({category: "constructor"})
            out.entityNamed = changes({changedEntityName: "moqui.security.UserAccount", changedFieldName: "emailAddress"})
            out.tooDeep = changes({pageIndex: 20, pageSize: 100})
            out.exported = changes({forExport: true})
            out.onePage = changes({pageSize: 2})
            out.source = invoke(A + "call#ChangeSource", {sourceService: "spec.audit.MissingServices.find#Changes"})
        })
        out.text = JSON.stringify(out.all)
        return out
    },
    then(out) {
        has(out.stranger.error, /ADMIN_AUDIT_VIEW is required/)
        out.all.error === ""
        // the relay's registered changes and nothing else: not the account's, not the enumeration's, not the field whose
        // permission the reader lacks (its entity's every-other-field registration does not answer it either), and not the
        // two whose registration names no usable source
        out.all.changeListCount === 6
        Array.from(out.all.changeList)
            .map(row => row.changedFieldName)
            .join() === "resetPassword,passwordReset,smtpHost,mailPassword,smtpPassword,disabled"
        !has(out.text, /emailAddress|spec\.example|Spec unregistered|storePort|storeProtocol|webBaseUrl/)
        // an encrypted field is a marker though its registration says show; the stored values are nowhere in the answer
        rowOf(out.all, "smtpPassword").masked === true
        rowOf(out.all, "smtpPassword").marker === "changed"
        rowOf(out.all, "smtpPassword").before === null && rowOf(out.all, "smtpPassword").after === null
        !out.text.includes(SECRET_OLD) && !out.text.includes(SECRET_NEW)
        // so is a field whose name reads as a secret; an event about one is shown
        rowOf(out.all, "resetPassword").masked === true && !out.text.includes("spec-plain-reset-password")
        rowOf(out.all, "passwordReset").masked === false && rowOf(out.all, "passwordReset").after === "Reset mailed"
        // a service's own marker of a secret's change is shown as it is; any other field of the relay is named, not shown
        rowOf(out.all, "mailPassword").after === "(replaced)" && rowOf(out.all, "mailPassword").before === "(set)"
        rowOf(out.all, "smtpHost").masked === true && rowOf(out.all, "smtpHost").after === null
        rowOf(out.all, "smtpHost").description === "Mail relay: smtpHost"
        // a registered value is shown, with who, when and the reason as plain text without control characters
        rowOf(out.all, "disabled").masked === false
        rowOf(out.all, "disabled").before === "N" && rowOf(out.all, "disabled").after === "Y"
        rowOf(out.all, "disabled").reason === "Spec  <b>repoint</b> ${1+1}"
        rowOf(out.all, "disabled").actorName === "spec.aud.none (Spec audit none)"
        rowOf(out.all, "disabled").recordText === "Relay: " + RELAY
        rowOf(out.all, "disabled").linkKey === "adminMailRelay" && rowOf(out.all, "disabled").linkId === RELAY
        has(rowOf(out.all, "disabled").changedDateText, /^\d{4}-\d\d-\d\d \d\d:\d\d:\d\d$/)
        // narrowed by a registered projection, a category and an actor
        out.byProjection.changeListCount === 1 && out.byProjection.changeList[0].changedFieldName === "disabled"
        out.byCategory.changeListCount === 6
        out.otherCategory.changeListCount === 0 && out.otherCategory.error === ""
        out.byActor.changeListCount === 0
        has(out.unregistered.error, /Unknown audit projection/)
        has(out.unreadable.error, /Unknown audit projection/)
        has(out.inherited.error, /Unknown audit category/)
        // an entity and a field a request names are not parameters: the same answer as none
        out.entityNamed.changeListCount === 6
        // the options name only what the reader may read, and never a registration that does not read
        Array.from(out.all.projectionList).some(row => row.auditProjectionId === "SPEC_AUD_SECRET")
        !Array.from(out.all.projectionList).some(row => /SPEC_AUD_(PERM|NOENT|NOSRC|NOCON)/.test(row.auditProjectionId))
        // bounds: a page, a deeper page than is read refused with a notice, an export of one bounded page
        out.onePage.changeList.length === 2 && out.onePage.changeListCount === 6 && out.onePage.changeListPageMaxIndex === 2
        out.onePage.changeListPageRangeLow === 1 && out.onePage.changeListPageRangeHigh === 2
        out.tooDeep.changeList.length === 0 && has(out.tooDeep.notice, /newest 2000 changes/)
        out.exported.changeList.length === 6 && out.exported.truncated === false && out.exported.exportRows === 1000
        // a source is called only when a registration names it
        has(out.source.error, /Unknown audit source/)
    }
})

feature("an administrative action on an account leaves a row with the actor, the session and the reason", {
    when() {
        const revoked = invoke("org.moqui.admin.IdentityServices.revoke#AccountAccess", {
            userId: users.none,
            kindList: ["loginKeys", "appPasswords"],
            reason: "Spec device lost"
        })
        const rows = Array.from(
            find(LOG, {
                changedEntityName: "moqui.security.UserAccount",
                changedFieldName: "access",
                pkPrimaryValue: users.none
            }).list()
        ).map(row => row.getMap())
        return {revoked, rows, sameVisit: (rows[0]?.changedInVisitId || null) === (ec.user.visitId || null)}
    },
    then(out) {
        out.revoked.error === ""
        out.rows.length === 1
        out.rows[0].changedByUserId === ec.user.userId
        out.rows[0].changeReason === "Spec device lost"
        out.rows[0].newValueText === "loginKeys: 0, appPasswords: 0"
        out.sameVisit === true
    }
})
