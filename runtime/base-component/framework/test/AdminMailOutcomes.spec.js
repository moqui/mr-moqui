// The mail queue, org.moqui.admin.OperationServices (mr-boss plan/impl-admin.md P12; design-admin §9 MailQueue, §15.3 "Unknown
// provider outcome"): a failed send is listed with its recipient count, owner and next step and never its subject, body or
// an address; a message known not sent is sent again as the same stored message, once for its operation key; a message that
// may have been sent is reconciled and never sent from here; a mail account's message is its sender's; and while a
// registered owner cannot be asked, nothing is retried. Mail is captured (no connection), but for one server that has no
// address.
// Run: ./moq test framework --spec AdminMailOutcomes
const O = "org.moqui.admin.OperationServices."
const MESSAGE = "moqui.basic.email.EmailMessage"
const users = {manage: "SPEC_MQ_MANAGE", view: "SPEC_MQ_VIEW", relays: "SPEC_MQ_RELAYS", none: "SPEC_MQ_NONE"}
const groups = {manage: "SPEC_MQ_MANAGERS", view: "SPEC_MQ_VIEWERS", relays: "SPEC_MQ_RELAY_KEEPERS"}
const ids = {
    retry: "SPEC_MQ_RETRY",
    down: "SPEC_MQ_DOWN",
    uncertain: "SPEC_MQ_UNCERTAIN",
    lost: "SPEC_MQ_LOST",
    closed: "SPEC_MQ_CLOSED",
    account: "SPEC_MQ_ACCOUNT",
    held: "SPEC_MQ_HELD",
    sent: "SPEC_MQ_SENT",
    draft: "SPEC_MQ_DRAFT"
}
const subject = "Spec queue private subject"
const body = "Spec queue private body"
const capture = ec.factory.getTool("MailCapture")

const find = (entityName, conditions = {}) => ec.entity.find(entityName).condition(conditions).disableAuthz()
const one = (entityName, conditions) => find(entityName, conditions).one()
const make = (entityName, fields) => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
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
const message = (emailMessageId, fields = {}) =>
    make(
        MESSAGE,
        Object.assign(
            {
                emailMessageId,
                statusId: "ES_READY",
                emailServerId: "SPEC_MQ_SMTP",
                emailTemplateId: "SPEC_MQ_NOTICE",
                fromAddress: "notices@queue.example.test",
                toAddresses: "ada@queue.example.test, bob@queue.example.test",
                ccAddresses: "cy@queue.example.test",
                subject,
                body,
                bodyText: body,
                sendError: "550 5.1.1 <ada@queue.example.test>: user ada@queue.example.test unknown",
                sendOutcome: "retryable"
            },
            fields
        )
    )
const row = emailMessageId => invoke(O + "find#MailOutcomes", {emailMessageId, state: "all"}).outcomeList?.[0]
const revision = emailMessageId => row(emailMessageId)?.revision
const retry = (emailMessageId, extra = {}) =>
    invoke(O + "retry#MailOutcome", Object.assign({emailMessageId, expectedRevision: revision(emailMessageId)}, extra))
const reconcile = (emailMessageId, resolution, extra = {}) =>
    invoke(
        O + "reconcile#MailOutcome",
        Object.assign({emailMessageId, resolution, reason: "The relay's log", expectedRevision: revision(emailMessageId)}, extra)
    )
const stored = emailMessageId => one(MESSAGE, {emailMessageId}).getMap()
const count = () => Number(find(MESSAGE).condition("emailMessageId", "like", "SPEC_MQ_%").count())
const audit = (fieldName, key) =>
    Array.from(
        find("moqui.entity.EntityAuditLog", {changedEntityName: MESSAGE, changedFieldName: fieldName, pkPrimaryValue: key}).list()
    )

spec({
    setupSpec() {
        capture.enable()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        for (const [key, userId] of Object.entries(users))
            make("moqui.security.UserAccount", {
                userId,
                username: userId.toLowerCase().replace(/_/g, "."),
                userFullName: "Spec mail queue " + key
            })
        for (const [key, userGroupId] of Object.entries(groups))
            make("moqui.security.UserGroup", {userGroupId, description: "Spec mail queue " + key})
        for (const userPermissionId of ["ADMIN_OPERATION_VIEW", "ADMIN_OPERATION_MANAGE"])
            make("moqui.security.UserGroupPermission", {userGroupId: groups.manage, userPermissionId, fromDate: 0})
        make("moqui.security.UserGroupPermission", {
            userGroupId: groups.view,
            userPermissionId: "ADMIN_OPERATION_VIEW",
            fromDate: 0
        })
        make("moqui.security.UserGroupMember", {userGroupId: groups.manage, userId: users.manage, fromDate: 0})
        make("moqui.security.UserGroupMember", {userGroupId: groups.view, userId: users.view, fromDate: 0})
        make("moqui.security.UserGroupPermission", {
            userGroupId: groups.relays,
            userPermissionId: "ADMIN_CONNECTION_VIEW",
            fromDate: 0
        })
        make("moqui.security.UserGroupMember", {userGroupId: groups.relays, userId: users.relays, fromDate: 0})
        make("moqui.basic.email.EmailServer", {
            emailServerId: "SPEC_MQ_SMTP",
            smtpHost: "127.0.0.1",
            smtpPort: "9",
            smtpStartTls: "N"
        })
        // a server with no address: a send to it is known not sent
        make("moqui.basic.email.EmailServer", {
            emailServerId: "SPEC_MQ_NOWHERE",
            smtpHost: "mail.nowhere.invalid",
            smtpPort: "25",
            smtpStartTls: "N"
        })
        make("moqui.basic.email.EmailTemplate", {
            emailTemplateId: "SPEC_MQ_NOTICE",
            description: "Spec queue notice",
            emailServerId: "SPEC_MQ_SMTP"
        })
        make("moqui.basic.email.EmailAccount", {
            emailAccountId: ids.account,
            emailServerId: "SPEC_MQ_SMTP",
            ownerUserId: users.none
        })
        message(ids.retry)
        message(ids.down, {emailServerId: "SPEC_MQ_NOWHERE"})
        message(ids.uncertain, {sendOutcome: "uncertain", sendError: "The SMTP session stopped before its end"})
        // a failure kept before the outcome was: not known to be unsent
        message(ids.lost, {sendOutcome: null})
        message(ids.closed, {sendOutcome: "permanent"})
        message(ids.account, {emailAccountId: ids.account, emailTemplateId: null})
        message(ids.held)
        message(ids.sent, {statusId: "ES_SENT", sendError: null, sendOutcome: null})
        message(ids.draft, {statusId: "ES_DRAFT", sendError: null, sendOutcome: null})
        ec.artifactExecution.enableAuthz()
    },
    cleanupSpec() {
        ec.message.clearErrors()
        ec.artifactExecution.disableAuthz()
        capture.reset()
    }
})

feature("a failed send is listed with its owner, its recipient count and no subject, body or address", {
    when() {
        const list = parameters => invoke(O + "find#MailOutcomes", Object.assign({emailServerId: "SPEC_MQ_SMTP"}, parameters))
        context.out = as(users.view, () => ({
            attention: list({}),
            uncertain: list({state: "uncertain"}),
            sent: list({state: "sent"}),
            all: invoke(O + "find#MailOutcomes", {state: "all", pageSize: 3, pageIndex: 0, emailTemplateId: "SPEC_MQ_NOTICE"}),
            account: invoke(O + "find#MailOutcomes", {binding: "account"}),
            draft: invoke(O + "find#MailOutcomes", {emailMessageId: ids.draft, state: "all"}),
            viewerRetry: retry(ids.retry)
        }))
        context.out.none = as(users.none, () => invoke(O + "find#MailOutcomes"))
        // the keeper of the relays reads the instance's own mail, never a mail account's, whatever is asked
        context.out.relays = as(users.relays, () => ({
            all: invoke(O + "find#MailOutcomes", {emailServerId: "SPEC_MQ_SMTP"}),
            account: invoke(O + "find#MailOutcomes", {binding: "account"})
        }))
    },
    then() {
        const out = context.out
        const by = (list, emailMessageId) => list.outcomeList.find(found => found.emailMessageId === emailMessageId)
        const listed = out.attention.outcomeList.map(found => found.emailMessageId)
        out.attention.error === ""
        ;[ids.retry, ids.uncertain, ids.lost, ids.closed, ids.account, ids.held].every(id => listed.includes(id))
        !listed.includes(ids.sent)
        !listed.includes(ids.draft)
        by(out.attention, ids.retry).state === "retryable"
        by(out.attention, ids.retry).recipientCount === 3
        by(out.attention, ids.retry).ownerKind === "mailService"
        by(out.attention, ids.retry).ownerTitle === "Spec queue notice"
        by(out.attention, ids.retry).emailServerId === "SPEC_MQ_SMTP"
        by(out.attention, ids.retry).errorText.includes("(address)")
        // a reader is offered no action
        by(out.attention, ids.retry).retryEligible === false
        by(out.attention, ids.uncertain).state === "uncertain"
        by(out.attention, ids.lost).state === "uncertain"
        by(out.attention, ids.closed).state === "permanent"
        by(out.attention, ids.account).ownerKind === "mailAccount"
        by(out.attention, ids.account).binding === "account"
        // no subject, body or address in any answer
        !/private subject|private body|queue\.example\.test/.test(JSON.stringify(out))
        out.uncertain.outcomeList
            .map(found => found.emailMessageId)
            .sort()
            .join() === [ids.lost, ids.uncertain].sort().join()
        out.sent.outcomeList.map(found => found.emailMessageId).join() === ids.sent
        out.all.outcomeList.length === 3
        out.all.outcomeCount === 7
        out.account.outcomeList.every(found => found.binding === "account")
        out.account.outcomeList.some(found => found.emailMessageId === ids.account)
        out.draft.outcomeList.length === 0
        out.viewerRetry.error.includes("ADMIN_OPERATION_MANAGE")
        out.none.error.includes("ADMIN_OPERATION_VIEW")
        out.relays.all.instanceOnly === true
        out.relays.all.outcomeList.some(found => found.emailMessageId === ids.retry)
        out.relays.all.outcomeList.concat(out.relays.account.outcomeList).every(found => found.binding === "instance")
    }
})

feature("a message known not sent is sent again as itself, once; one that may have been sent is reconciled, never sent", {
    when() {
        const out = {before: count()}
        capture.clear()
        as(users.manage, () => {
            out.offered = row(ids.retry)
            out.stale = invoke(O + "retry#MailOutcome", {emailMessageId: ids.retry, expectedRevision: "1"})
            out.uncertain = retry(ids.uncertain)
            out.lost = retry(ids.lost)
            out.account = retry(ids.account)
            out.accountSettle = reconcile(ids.account, "sent")
            out.refusedSends = capture.count()
            out.retried = retry(ids.retry, {operationKey: "spec-mq-1"})
            out.sends = capture.count()
            // the same request again: the first answer, and nothing sent
            out.repeated = invoke(O + "retry#MailOutcome", {
                emailMessageId: ids.retry,
                expectedRevision: "stale",
                operationKey: "spec-mq-1"
            })
            out.otherKey = invoke(O + "retry#MailOutcome", {
                emailMessageId: ids.closed,
                expectedRevision: "x",
                operationKey: "spec-mq-1"
            })
            out.again = retry(ids.retry)
            out.sendsAfter = capture.count()
            // a server that is not reached: the message stays known not sent
            capture.disable()
            out.down = retry(ids.down)
            capture.enable()
            out.downRow = row(ids.down)
            // reconciliation: by a person's word, with the reason; nothing is sent
            out.noReason = invoke(O + "reconcile#MailOutcome", {
                emailMessageId: ids.uncertain,
                resolution: "sent",
                expectedRevision: revision(ids.uncertain)
            })
            out.notUncertain = reconcile(ids.closed, "sent")
            out.wentOut = reconcile(ids.uncertain, "sent")
            out.notSent = reconcile(ids.lost, "notSent")
            out.lostRow = row(ids.lost)
            out.abandoned = reconcile(ids.closed, "abandon")
            out.sendsEnd = capture.count()
        })
        out.after = count()
        out.retriedRow = stored(ids.retry)
        out.uncertainRow = stored(ids.uncertain)
        out.closedRow = stored(ids.closed)
        out.accountRow = stored(ids.account)
        out.statusAudit = audit("statusId", ids.uncertain)
        out.outcomeAudit = audit("sendOutcome", ids.lost)
        context.out = out
    },
    then() {
        const out = context.out
        out.offered.retryEligible === true
        out.offered.reconcileEligible === false
        out.stale.reasonCode === "STALE_REVISION"
        out.uncertain.applied === false
        out.uncertain.reasonCode === "RECONCILE_REQUIRED"
        out.lost.reasonCode === "RECONCILE_REQUIRED"
        out.account.reasonCode === "OWNER_ONLY"
        out.accountSettle.reasonCode === "OWNER_ONLY"
        out.refusedSends === 0
        out.retried.applied === true
        out.retried.sent === true
        out.retried.state === "sent"
        out.sends === 1
        out.retriedRow.statusId === "ES_SENT"
        out.retriedRow.sendOutcome == null
        out.retriedRow.sendClaimDate == null
        out.repeated.applied === true
        out.repeated.repeated === true
        out.otherKey.error.includes("was used for another operation")
        out.again.reasonCode === "NOT_FAILED"
        out.sendsAfter === 1
        // no second message was made by any of it
        out.after === out.before
        out.down.applied === false
        out.down.reasonCode === "SEND_FAILED"
        out.downRow.state === "retryable"
        out.downRow.retryEligible === true
        out.noReason.error !== ""
        out.notUncertain.reasonCode === "NOT_UNCERTAIN"
        out.wentOut.applied === true
        out.wentOut.state === "sent"
        out.uncertainRow.statusId === "ES_SENT"
        out.statusAudit.some(found => found.changeReason === "The relay's log" && found.newValueText === "ES_SENT")
        out.notSent.applied === true
        out.lostRow.state === "retryable"
        out.lostRow.retryEligible === true
        out.outcomeAudit.length === 1
        out.outcomeAudit[0].changedByUserId === users.manage
        out.abandoned.applied === true
        out.closedRow.statusId === "ES_CANCELLED"
        out.accountRow.statusId === "ES_READY"
        out.sendsEnd === 1
    }
})

feature("while a registered owner cannot be asked, nothing is retried or settled", {
    when() {
        ec.artifactExecution.disableAuthz()
        make("moqui.server.MailOutcomeOwner", {
            ownerKey: "specMissing",
            title: "Spec ledger",
            ownerService: "spec.Missing.find#Owners"
        })
        ec.artifactExecution.enableAuthz()
        capture.clear()
        const out = as(users.manage, () => ({
            list: invoke(O + "find#MailOutcomes", {emailMessageId: ids.held}),
            retry: retry(ids.held),
            abandon: reconcile(ids.held, "abandon")
        }))
        out.sends = capture.count()
        ec.artifactExecution.disableAuthz()
        find("moqui.server.MailOutcomeOwner", {ownerKey: "specMissing"}).deleteAll()
        ec.artifactExecution.enableAuthz()
        out.answered = as(users.manage, () => row(ids.held))
        out.row = stored(ids.held)
        context.out = out
    },
    then() {
        const out = context.out
        out.list.unansweredOwnerList.join() === "specMissing"
        out.list.outcomeList[0].retryEligible === false
        out.list.outcomeList[0].abandonEligible === false
        out.retry.reasonCode === "OWNER_UNAVAILABLE"
        out.abandon.reasonCode === "OWNER_UNAVAILABLE"
        out.sends === 0
        out.row.statusId === "ES_READY"
        out.row.sendOutcome === "retryable"
        out.answered.retryEligible === true
    }
})
