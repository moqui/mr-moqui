// System messages, recovery and message types, org.moqui.admin.OperationServices and ConfigurationServices (mr-boss
// plan/impl-admin.md P13; design-admin §8, §14 "Operational recovery", §15.3 "Unknown provider outcome" and "Independent
// failure"): a message is listed with its state, the class of its error and no text, a remote's answer or a token; its payload
// is shown only with the permission its owner registered; nothing is recovered without a registration; a message whose
// outcome is unknown is offered reconciliation and never a retry, and its settlement is kept apart from its status, which no
// person sets (reported done: never sent; reported not done: sent again through the retry's claim); in a batch each message has its own result and a refused
// or failing one does not stop the others; an operation key answers the first results and sends nothing again; a disabled
// remote is sent nothing; and a message type is given only a handler registered for it. The HTTP fixtures answer every
// send: nothing leaves the machine.
// Run: ./moq test framework --spec AdminRecovery
const O = "org.moqui.admin.OperationServices."
const C = "org.moqui.admin.ConfigurationServices."
const MESSAGE = "moqui.service.message.SystemMessage"
const TYPE = "moqui.service.message.SystemMessageType"
const REST = "org.moqui.impl.SystemMessageServices.send#SystemMessageRest"
const FAILS = "org.moqui.impl.SystemMessageServices.send#SystemMessageJsonRpc"
const NOOP = "org.moqui.impl.BasicServices.noop"
const users = {manage: "SPEC_RC_MANAGE", view: "SPEC_RC_VIEW", integ: "SPEC_RC_INTEG", none: "SPEC_RC_NONE"}
const groups = {manage: "SPEC_RC_MANAGERS", view: "SPEC_RC_VIEWERS", integ: "SPEC_RC_INTEGRATORS"}
const types = {out: "SPEC_RC_OUT", incoming: "SPEC_RC_IN", plain: "SPEC_RC_PLAIN", held: "SPEC_RC_HELD", owned: "SPEC_RC_OWNED"}
const ids = {
    out: "SPEC_RC_M_OUT",
    again: "SPEC_RC_M_AGAIN",
    unknown: "SPEC_RC_M_UNKNOWN",
    gone: "SPEC_RC_M_GONE",
    busy: "SPEC_RC_M_BUSY",
    plain: "SPEC_RC_M_PLAIN",
    off: "SPEC_RC_M_OFF",
    incoming: "SPEC_RC_M_IN",
    held: "SPEC_RC_M_HELD",
    owned: "SPEC_RC_M_OWNED",
    cancel: "SPEC_RC_M_CANCEL",
    requeue: "SPEC_RC_M_REQUEUE",
    sent: "SPEC_RC_M_SENT"
}
const url = "https://rc.example.test/hook"
const payload = '{"order":"spec private payload 77","apiKey":"spec-rc-key-1"}'
const remoteAnswer = 'send error response (500): {"detail":"spec private remote body","token":"spec-rc-remote-token"}'

const http = ec.factory.getTool("HttpFixtures")
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
const has = (value, pattern) => pattern.test(String(value ?? ""))
const row = systemMessageId => invoke(O + "find#Messages", {reference: systemMessageId, state: "all"}).messageList?.[0]
const item = systemMessageId => ({targetId: systemMessageId, expectedRevision: row(systemMessageId)?.revision})
const recover = (action, systemMessageIds, extra = {}) =>
    invoke(
        O + "execute#Recovery",
        Object.assign(
            {targetKind: "SystemMessage", action, targetList: systemMessageIds.map(item), reason: "Spec recovery"},
            extra
        )
    )
const result = (out, systemMessageId) => Array.from(out.resultList || []).find(found => found.targetId === systemMessageId)
const stored = systemMessageId => one(MESSAGE, {systemMessageId}).getMap()
const audit = (entityName, fieldName, key) =>
    Array.from(
        find("moqui.entity.EntityAuditLog", {
            changedEntityName: entityName,
            changedFieldName: fieldName,
            pkPrimaryValue: key
        }).list()
    )
const typeRevision = systemMessageTypeId => invoke(C + "get#MessageTypeUse", {systemMessageTypeId}).messageType?.revision
const storeType = (systemMessageTypeId, values) =>
    invoke(
        C + "store#MessageType",
        Object.assign({systemMessageTypeId, expectedRevision: typeRevision(systemMessageTypeId)}, values)
    )

spec({
    setupSpec() {
        http.enable()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        for (const [key, userId] of Object.entries(users))
            make("moqui.security.UserAccount", {
                userId,
                username: userId.toLowerCase().replace(/_/g, "."),
                userFullName: "Spec recovery " + key
            })
        for (const [key, userGroupId] of Object.entries(groups))
            make("moqui.security.UserGroup", {userGroupId, description: "Spec recovery " + key})
        make("moqui.security.UserPermission", {userPermissionId: "SPEC_RC_DATA", description: "Spec recovery payloads"})
        make("moqui.security.UserPermission", {userPermissionId: "SPEC_RC_OWNER", description: "Spec recovery owner"})
        const grant = (userGroupId, permissions) =>
            permissions.forEach(userPermissionId =>
                make("moqui.security.UserGroupPermission", {userGroupId, userPermissionId, fromDate: 0})
            )
        grant(groups.manage, ["ADMIN_OPERATION_VIEW", "ADMIN_OPERATION_MANAGE", "SPEC_RC_DATA"])
        grant(groups.view, ["ADMIN_OPERATION_VIEW"])
        grant(groups.integ, ["ADMIN_CONNECTION_VIEW", "ADMIN_CONNECTION_MANAGE"])
        for (const key of Object.keys(groups))
            make("moqui.security.UserGroupMember", {userGroupId: groups[key], userId: users[key], fromDate: 0})

        make(TYPE, {systemMessageTypeId: types.out, description: "Spec recovery outgoing", sendServiceName: REST})
        make(TYPE, {systemMessageTypeId: types.incoming, description: "Spec recovery incoming", consumeServiceName: FAILS})
        make(TYPE, {systemMessageTypeId: types.plain, description: "Spec recovery unregistered", sendServiceName: REST})
        make(TYPE, {systemMessageTypeId: types.held, description: "Spec recovery held", sendServiceName: REST})
        make(TYPE, {systemMessageTypeId: types.owned, description: "Spec recovery owned", sendServiceName: REST})
        const remote = (systemMessageRemoteId, fields = {}) =>
            make(
                "moqui.service.message.SystemMessageRemote",
                Object.assign(
                    {systemMessageRemoteId, description: "Spec recovery remote", sendUrl: url, sendAuthEnumId: "SmatNone"},
                    fields
                )
            )
        remote("SPEC_RC_REMOTE")
        remote("SPEC_RC_OFF", {disabled: "Y"})
        const descriptor = (descriptorKey, targetKey, fields) =>
            make(
                "moqui.server.RecoveryDescriptor",
                Object.assign(
                    {descriptorKey, targetKind: "SystemMessage", targetKey, ownerApp: "Spec owner", title: "Spec messages"},
                    fields
                )
            )
        descriptor("specRcOut", types.out, {
            direction: "out",
            retryMode: "native",
            reconcileMode: "native",
            cancelMode: "native",
            dataPermission: "SPEC_RC_DATA",
            idempotencyNote: "The receiver keeps one message for an id."
        })
        descriptor("specRcIn", types.incoming, {direction: "in", retryMode: "native"})
        // an owner that cannot be asked (its service fails), and an owner whose permission no one here holds
        descriptor("specRcHeld", types.held, {retryMode: "native", describeService: FAILS})
        descriptor("specRcOwned", types.owned, {retryMode: "native", cancelMode: "native", managePermission: "SPEC_RC_OWNER"})
        // a registration naming a service that is not there is left out
        descriptor("specRcBroken", types.plain, {retryMode: "owner", recoverService: "spec.Missing.recover#Message"})
        const handler = (handlerKey, systemMessageTypeId, serviceName, title) =>
            make("moqui.server.MessageHandler", {
                handlerKey,
                systemMessageTypeId,
                handlerRole: "consume",
                serviceName,
                title,
                ownerApp: "Spec owner"
            })
        handler("specRcConsume", types.incoming, NOOP, "Spec consumer")
        handler("specRcConsumeOld", types.incoming, FAILS, "Spec old consumer")
        handler("specRcOther", types.held, NOOP, "Spec other type's consumer")

        const hours = 3600000
        const message = (systemMessageId, fields = {}) => {
            make(
                MESSAGE,
                Object.assign(
                    {
                        systemMessageId,
                        systemMessageTypeId: types.out,
                        systemMessageRemoteId: "SPEC_RC_REMOTE",
                        statusId: "SmsgError",
                        isOutgoing: "Y",
                        initDate: Date.now() - 3 * hours,
                        lastAttemptDate: Date.now() - 2 * hours,
                        failCount: 3,
                        messageId: "ENV-" + systemMessageId,
                        messageText: payload
                    },
                    fields
                )
            )
            if ((fields.statusId || "SmsgError") === "SmsgError")
                make("moqui.service.message.SystemMessageError", {
                    systemMessageId,
                    errorDate: Date.now() - 2 * hours,
                    attemptedStatusId: fields.isOutgoing === "N" ? "SmsgConsumed" : "SmsgSent",
                    errorText: "System message " + systemMessageId + " " + remoteAnswer + "\nsecond line"
                })
        }
        message(ids.out)
        message(ids.again)
        // a send that never answered: it may have gone
        message(ids.unknown, {statusId: "SmsgSending", failCount: 0})
        message(ids.gone, {statusId: "SmsgSending", failCount: 0})
        message(ids.busy, {statusId: "SmsgSending", failCount: 0, lastAttemptDate: Date.now()})
        message(ids.plain, {systemMessageTypeId: types.plain})
        message(ids.off, {systemMessageRemoteId: "SPEC_RC_OFF"})
        message(ids.incoming, {systemMessageTypeId: types.incoming, isOutgoing: "N"})
        message(ids.held, {systemMessageTypeId: types.held})
        message(ids.owned, {systemMessageTypeId: types.owned})
        message(ids.cancel)
        message(ids.requeue)
        message(ids.sent, {statusId: "SmsgSent", failCount: 0})
        ec.artifactExecution.enableAuthz()
    },
    cleanupSpec() {
        ec.message.clearErrors()
        ec.artifactExecution.disableAuthz()
        http.reset()
    }
})

feature("a message is listed with its state and the class of its error, and its payload only with its owner's permission", {
    when() {
        const out = (context.out = {})
        out.view = as(users.view, () => ({
            attention: invoke(O + "find#Messages", {systemMessageRemoteId: "SPEC_RC_REMOTE"}),
            unknown: invoke(O + "find#Messages", {state: "uncertain", systemMessageTypeId: types.out}),
            done: invoke(O + "find#Messages", {state: "done", systemMessageTypeId: types.out}),
            incoming: invoke(O + "find#Messages", {direction: "in", systemMessageTypeId: types.incoming}),
            byEnvelope: invoke(O + "find#Messages", {reference: "ENV-" + ids.out, state: "all"}),
            detail: invoke(O + "get#Message", {systemMessageId: ids.out, includePayload: true}),
            preview: invoke(O + "preview#Recovery", {
                targetKind: "SystemMessage",
                targetId: [ids.out, ids.unknown, "SPEC_RC_NONE"].join(",")
            }),
            act: recover("retry", [ids.out])
        }))
        out.manage = as(users.manage, () => ({
            list: invoke(O + "find#Messages", {systemMessageRemoteId: "SPEC_RC_REMOTE"}),
            detail: invoke(O + "get#Message", {systemMessageId: ids.out, includePayload: true}),
            unasked: invoke(O + "get#Message", {systemMessageId: ids.out}),
            plain: invoke(O + "get#Message", {systemMessageId: ids.plain, includePayload: true}),
            preview: invoke(O + "preview#Recovery", {
                targetKind: "SystemMessage",
                targetIdList: [ids.out, ids.unknown, ids.busy, ids.plain, ids.held, ids.owned, ids.sent]
            }),
            kind: invoke(O + "preview#Recovery", {targetKind: "constructor", targetId: ids.out})
        }))
        out.none = as(users.none, () => ({
            list: invoke(O + "find#Messages"),
            detail: invoke(O + "get#Message", {systemMessageId: ids.out}),
            preview: invoke(O + "preview#Recovery", {targetKind: "SystemMessage", targetId: ids.out}),
            step: invoke(O + "apply#MessageRecovery", {systemMessageId: ids.out, expectedRevision: "0", step: "cancel"})
        }))
        out.sends = http.count()
    },
    then() {
        const out = context.out
        const by = (list, systemMessageId) => list.messageList.find(found => found.systemMessageId === systemMessageId)
        const action = (preview, systemMessageId, name) =>
            preview.previewList.find(found => found.targetId === systemMessageId).actionList.find(found => found.action === name)
        const listed = out.view.attention.messageList.map(found => found.systemMessageId)
        out.view.attention.error === ""
        ;[ids.out, ids.again, ids.unknown, ids.plain, ids.held, ids.owned].every(id => listed.includes(id))
        // a message being sent now, a sent one and another remote's are not what needs attention at this remote
        !listed.includes(ids.busy) && !listed.includes(ids.sent) && !listed.includes(ids.off)
        by(out.view.attention, ids.out).state === "failed"
        by(out.view.attention, ids.out).direction === "out"
        by(out.view.attention, ids.out).failCount === 3
        by(out.view.attention, ids.out).errorClass === "HTTP 500: the remote failed"
        by(out.view.attention, ids.out).errorText.endsWith("send error response (500)")
        by(out.view.attention, ids.out).ownerApp === "Spec owner"
        by(out.view.attention, ids.out).messageId === "ENV-" + ids.out
        // a reader is offered no action
        by(out.view.attention, ids.out).retryEligible === false
        by(out.view.attention, ids.unknown).state === "uncertain"
        by(out.view.attention, ids.unknown).stateText === "May have been sent"
        out.view.unknown.messageList
            .map(found => found.systemMessageId)
            .sort()
            .join() === [ids.gone, ids.unknown].join()
        out.view.done.messageList.map(found => found.systemMessageId).join() === ids.sent
        out.view.incoming.messageList.map(found => found.systemMessageId).join() === ids.incoming
        out.view.byEnvelope.messageList.map(found => found.systemMessageId).join() === ids.out
        // no text of a message, no answer of a remote and no token in any list, detail or preview a reader gets
        !/private payload|spec-rc-key|private remote body|spec-rc-remote-token|second line/.test(JSON.stringify(out.view))
        out.view.detail.payloadVisible === false
        out.view.detail.payloadText == null
        out.view.detail.payloadHiddenReason.includes("SPEC_RC_DATA")
        out.view.detail.attemptList.length === 1
        out.view.detail.attemptList[0].errorClass === "HTTP 500: the remote failed"
        out.view.detail.message.payloadLength === payload.length
        action(out.view.preview, ids.out, "retry").eligible === false
        action(out.view.preview, ids.out, "retry").reasonCode === "PERMISSION"
        out.view.preview.previewList[2].found === false
        has(out.view.act.error, /ADMIN_OPERATION_MANAGE is required/)

        // the holder of the permission the owner registered reads the payload, a field named like a credential struck out
        out.manage.detail.payloadVisible === true
        out.manage.detail.payloadText.includes("spec private payload 77")
        !out.manage.detail.payloadText.includes("spec-rc-key-1")
        // and only when it is asked for
        out.manage.unasked.payloadText == null
        // a type no owner registered shows no payload to anyone
        out.manage.plain.payloadVisible === false
        by(out.manage.list, ids.out).retryEligible === true
        by(out.manage.list, ids.out).cancelEligible === true
        by(out.manage.list, ids.out).reconcileEligible === false
        action(out.manage.preview, ids.out, "retry").eligible === true
        action(out.manage.preview, ids.out, "retry").mode === "native"
        action(out.manage.preview, ids.out, "retry").consequence.includes("The receiver keeps one message for an id.")
        // an unknown outcome is offered reconciliation, never a retry, a requeue or a cancellation
        action(out.manage.preview, ids.unknown, "reconcile").eligible === true
        action(out.manage.preview, ids.unknown, "reconcile").resolutionList.length === 2
        ;["retry", "requeue", "cancel"].every(
            name => action(out.manage.preview, ids.unknown, name).reasonCode === "RECONCILE_REQUIRED"
        )
        action(out.manage.preview, ids.busy, "retry").reasonCode === "IN_PROGRESS"
        action(out.manage.preview, ids.busy, "reconcile").reasonCode === "NOT_UNCERTAIN"
        // nothing without a registration, and a registration naming a missing service is none
        ;["retry", "requeue", "reconcile", "cancel"].every(
            name => action(out.manage.preview, ids.plain, name).reasonCode === "NOT_REGISTERED"
        )
        by(out.manage.list, ids.plain).nextStep.includes("No recovery is registered")
        action(out.manage.preview, ids.held, "retry").reasonCode === "OWNER_UNAVAILABLE"
        // a message its owner does not answer for to this reader: its transport row, with no remote, reference or line
        by(out.manage.list, ids.held).errorClass === "HTTP 500: the remote failed"
        by(out.manage.list, ids.held).remoteText === ""
        by(out.manage.list, ids.held).systemMessageRemoteId == null
        by(out.manage.list, ids.held).messageId == null
        by(out.manage.list, ids.held).errorText === ""
        action(out.manage.preview, ids.owned, "retry").reasonCode === "OWNER_PERMISSION"
        action(out.manage.preview, ids.owned, "reconcile").reasonCode === "NOT_OFFERED"
        action(out.manage.preview, ids.sent, "retry").reasonCode === "NOT_FAILED"
        has(out.manage.kind.error, /Unknown recovery target kind/)

        has(out.none.list.error, /ADMIN_OPERATION_VIEW is required/)
        has(out.none.detail.error, /ADMIN_OPERATION_VIEW is required/)
        has(out.none.preview.error, /ADMIN_OPERATION_VIEW is required/)
        out.none.step.error !== ""
        stored(ids.out).statusId === "SmsgError"
        // nothing was sent by reading
        out.sends === 0
    }
})

feature(
    "an ineligible message is refused, an unknown outcome is reconciled and not retried, and a failed item does not stop the batch",
    {
        when() {
            const out = (context.out = {})
            // the first send is taken, the second fails again
            http.add({method: "POST", url, status: 200, contentType: "application/json", body: "{}", times: 1})
            http.add({method: "POST", url, status: 503, body: "spec private remote body two"})
            as(users.manage, () => {
                const stale = {targetId: ids.requeue, expectedRevision: "1"}
                out.batch = invoke(O + "execute#Recovery", {
                    targetKind: "SystemMessage",
                    action: "retry",
                    reason: "Spec recovery",
                    operationKey: "spec-rc-batch-1",
                    targetList: [
                        item(ids.out),
                        item(ids.plain),
                        item(ids.unknown),
                        item(ids.again),
                        stale,
                        item(ids.off),
                        item(ids.held),
                        item(ids.owned)
                    ]
                })
                out.sendsAfterBatch = http.count()
                // the same batch again under its key: the first results, nothing sent
                out.repeat = invoke(O + "execute#Recovery", {
                    targetKind: "SystemMessage",
                    action: "retry",
                    reason: "Spec recovery",
                    operationKey: "spec-rc-batch-1",
                    targetList: [ids.out, ids.plain, ids.unknown, ids.again, ids.requeue, ids.off, ids.held, ids.owned].map(
                        targetId => ({
                            targetId,
                            expectedRevision: "0"
                        })
                    )
                })
                out.sendsAfterRepeat = http.count()
                out.otherUse = recover("retry", [ids.cancel], {operationKey: "spec-rc-batch-1"})
                out.noReason = recover("retry", [ids.cancel], {reason: " "})
                out.badAction = recover("constructor", [ids.cancel])

                // the unknown ones: settled by a person's word, kept apart from the status, which stays the transport's
                out.unknownRetry = recover("retry", [ids.unknown])
                out.noResolution = recover("reconcile", [ids.unknown])
                out.notSent = recover("reconcile", [ids.unknown], {resolution: "notDone"})
                out.afterNotSent = row(ids.unknown)
                out.storedNotSent = stored(ids.unknown)
                out.settleAgain = recover("reconcile", [ids.unknown], {resolution: "done"})
                out.gone = recover("reconcile", [ids.gone], {resolution: "done"})
                out.afterGone = row(ids.gone)
                out.goneRetry = recover("retry", [ids.gone])
                const listed = state =>
                    invoke(O + "find#Messages", {state, systemMessageTypeId: types.out})
                        .messageList.map(found => found.systemMessageId)
                        .sort()
                        .join()
                out.lists = {settled: listed("settled"), uncertain: listed("uncertain"), done: listed("done")}
                out.inAttention = invoke(O + "find#Messages", {systemMessageTypeId: types.out}).messageList.some(found =>
                    [ids.unknown, ids.gone].includes(found.systemMessageId)
                )
                out.sendsAfterSettle = http.count()
                // the one reported not sent is sent again through the retry's claim: its settlement cleared, and the
                // send's own result (it fails here) its state
                out.notSentRetry = recover("retry", [ids.unknown])
                out.sendsAfterNotSentRetry = http.count()

                out.cancelled = recover("cancel", [ids.cancel, ids.busy])
                out.requeued = recover("requeue", [ids.requeue])
                // an incoming message: its consuming service fails again, then succeeds once its registered handler is chosen
                out.consumeFails = recover("retry", [ids.incoming])
            })
            out.handler = as(users.integ, () => storeType(types.incoming, {consumeHandlerKey: "specRcConsume"}))
            as(users.manage, () => {
                out.consumed = recover("retry", [ids.incoming])
            })
            out.sendsAtEnd = http.count()
            // the framework's own claim of a message for a send: of two senders that read the same status only one takes it
            // (the scheduled job and an operator's retry)
            out.requeuedRow = stored(ids.requeue)
            const claim = () =>
                invoke("org.moqui.impl.SystemMessageServices.claim#SystemMessage", {
                    systemMessageId: ids.requeue,
                    fromStatusId: "SmsgProduced",
                    toStatusId: "SmsgSending"
                }).claimed
            out.claims = [claim(), claim()]
        },
        then() {
            const out = context.out
            out.batch.error === ""
            out.batch.applied === true
            out.batch.appliedCount === 1
            out.batch.refusedCount === 7
            out.batch.resultList.length === 8
            result(out.batch, ids.out).applied === true
            result(out.batch, ids.out).state === "done"
            stored(ids.out).statusId === "SmsgSent"
            // the same stored message: no second one was made
            Number(find(MESSAGE).condition("systemMessageId", "like", "SPEC_RC_M_%").count()) === Object.keys(ids).length
            result(out.batch, ids.plain).reasonCode === "NOT_REGISTERED"
            result(out.batch, ids.unknown).reasonCode === "RECONCILE_REQUIRED"
            // it failed again: named, with the class of the failure and nothing of what the remote answered
            result(out.batch, ids.again).reasonCode === "RETRY_FAILED"
            result(out.batch, ids.again).errorClass === "HTTP 503: the remote failed"
            !JSON.stringify(out.batch).includes("private remote body")
            stored(ids.again).statusId === "SmsgError"
            Number(stored(ids.again).failCount) === 4
            result(out.batch, ids.requeue).reasonCode === "STALE_REVISION"
            result(out.batch, ids.off).reasonCode === "REMOTE_DISABLED"
            result(out.batch, ids.held).reasonCode === "OWNER_UNAVAILABLE"
            result(out.batch, ids.owned).reasonCode === "OWNER_PERMISSION"
            // two sends in all: the one taken and the one that failed
            out.sendsAfterBatch === 2
            http.requests()[0].body === payload
            audit(MESSAGE, "statusId", ids.out).some(found => found.changeReason === "Retry now: Spec recovery")
            audit(MESSAGE, "statusId", ids.again).some(found => found.changeReason === "Not applied. Retry now: Spec recovery")

            out.repeat.repeated === true
            out.repeat.appliedCount === 1
            result(out.repeat, ids.again).reasonCode === "RETRY_FAILED"
            out.sendsAfterRepeat === 2
            has(out.otherUse.error, /was used for another operation/)
            has(out.noReason.error, /Give a reason/)
            has(out.badAction.error, /Unknown recovery action/)

            out.unknownRetry.applied === false
            out.unknownRetry.reasonCode === "RECONCILE_REQUIRED"
            has(out.noResolution.error, /A resolution is one of/)
            out.notSent.applied === true
            // the settlement is beside the message: its status is as the transport left it
            out.storedNotSent.statusId === "SmsgSending"
            out.storedNotSent.outcomeSettlement === "notDone"
            out.storedNotSent.outcomeSettledByUserId === users.manage
            out.storedNotSent.outcomeSettledReason === "Spec recovery"
            out.afterNotSent.state === "settled"
            out.afterNotSent.stateText === "Settled: reported not sent"
            out.afterNotSent.settlement === "notDone"
            out.afterNotSent.settledReason === "Spec recovery"
            out.afterNotSent.retryEligible === true
            out.afterNotSent.reconcileEligible === false
            result(out.settleAgain, ids.unknown).reasonCode === "ALREADY_SETTLED"
            // reported sent: not marked Sent, no status row, never listed as done, never sent again
            out.gone.applied === true
            stored(ids.gone).statusId === "SmsgSending"
            stored(ids.gone).processedDate == null
            stored(ids.gone).outcomeSettlement === "done"
            out.afterGone.state === "settled"
            out.afterGone.stateText === "Settled: reported sent"
            result(out.goneRetry, ids.gone).reasonCode === "SETTLED"
            !audit(MESSAGE, "statusId", ids.gone).some(found => found.newValueText === "SmsgSent")
            audit(MESSAGE, "statusId", ids.gone).some(found => found.changeReason === "Settle (reported done): Spec recovery")
            out.lists.settled === [ids.gone, ids.unknown].join()
            out.lists.uncertain === ""
            out.lists.done === [ids.out, ids.sent].sort().join()
            out.inAttention === false
            out.sendsAfterSettle === 2
            // sent again as itself: the claim cleared the settlement, and the send that failed is its state
            result(out.notSentRetry, ids.unknown).reasonCode === "RETRY_FAILED"
            out.sendsAfterNotSentRetry === 3
            stored(ids.unknown).statusId === "SmsgError"
            stored(ids.unknown).outcomeSettlement == null
            Number(stored(ids.unknown).failCount) === 1

            result(out.cancelled, ids.cancel).applied === true
            stored(ids.cancel).statusId === "SmsgCancelled"
            result(out.cancelled, ids.busy).reasonCode === "IN_PROGRESS"
            stored(ids.busy).statusId === "SmsgSending"
            out.requeued.applied === true
            out.requeuedRow.statusId === "SmsgProduced"
            Number(out.requeuedRow.failCount) === 0
            out.claims.join() === "true,false"
            stored(ids.requeue).statusId === "SmsgSending"

            result(out.consumeFails, ids.incoming).reasonCode === "RETRY_FAILED"
            out.handler.applied === true
            out.consumed.applied === true
            stored(ids.incoming).statusId === "SmsgConsumed"
            out.sendsAtEnd === 3
        }
    }
)

feature("a message type is given only a handler registered for it, and only its registered transport options change", {
    when() {
        const out = (context.out = {})
        out.integ = as(users.integ, () => ({
            list: invoke(C + "find#MessageTypes", {term: "Spec recovery"}),
            use: invoke(C + "get#MessageTypeUse", {systemMessageTypeId: types.incoming}),
            unregistered: storeType(types.out, {description: "Changed"}),
            otherTypes: storeType(types.incoming, {consumeHandlerKey: "specRcOther"}),
            serviceName: storeType(types.incoming, {consumeHandlerKey: NOOP}),
            inherited: storeType(types.incoming, {consumeHandlerKey: "constructor"}),
            sendRole: storeType(types.incoming, {sendHandlerKey: "specRcConsume"}),
            expression: storeType(types.incoming, {sendPath: "out/${ec.user.userId}/file"}),
            receiveExpression: storeType(types.incoming, {receivePath: "in/${date}"}),
            parent: storeType(types.incoming, {receivePath: "in/../etc"}),
            pattern: storeType(types.incoming, {receiveFilePattern: "(unclosed"}),
            stale: invoke(C + "store#MessageType", {
                systemMessageTypeId: types.incoming,
                expectedRevision: "1",
                description: "Stale"
            }),
            saved: storeType(types.incoming, {
                description: "Spec recovery incoming files",
                contentType: "application/json",
                sendPath: "out/${date}/${systemMessageId}.json",
                receivePath: "in",
                receiveFilePattern: ".*\\.json",
                receiveResponseEnumId: "MsgRrMove",
                receiveMovePath: "done",
                consumeHandlerKey: "specRcConsumeOld",
                reason: "Spec transport"
            }),
            same: storeType(types.incoming, {description: "Spec recovery incoming files", consumeHandlerKey: "specRcConsumeOld"})
        }))
        out.manage = as(users.manage, () => ({
            list: invoke(C + "find#MessageTypes"),
            store: invoke(C + "store#MessageType", {
                systemMessageTypeId: types.incoming,
                expectedRevision: "1",
                description: "Operator"
            })
        }))
        out.type = one(TYPE, {systemMessageTypeId: types.incoming}).getMap()
    },
    then() {
        const out = context.out
        const listed = systemMessageTypeId =>
            out.integ.list.typeList.find(found => found.systemMessageTypeId === systemMessageTypeId)
        out.integ.list.error === ""
        out.integ.list.typeList.length === 5
        listed(types.incoming).configurable === true
        listed(types.incoming).canChange === true
        listed(types.incoming).direction === "in"
        listed(types.incoming).ownerText === "Spec owner"
        listed(types.incoming).recoveryText === "retry"
        // the handler chosen in the feature before, by its registered title
        listed(types.incoming).consumeHandlerText === "Spec consumer"
        listed(types.out).configurable === false
        listed(types.out).sendHandlerText === REST
        listed(types.out).recoveryText === "retry, settle, cancel"
        out.integ.use.handlerList.find(found => found.handlerRole === "consume").choiceList.length === 2
        out.integ.use.handlerList.find(found => found.handlerRole === "consume").currentHandlerKey === "specRcConsume"
        out.integ.use.handlerList.find(found => found.handlerRole === "send").changeable === false
        out.integ.use.handlerList.find(found => found.handlerRole === "receive").changeable === false
        out.integ.use.recoveryList[0].retryText === "By the message service"
        out.integ.use.countList.some(found => found.statusId === "SmsgConsumed" && found.messageCount === 1)

        out.integ.unregistered.reasonCode === "NOT_CONFIGURABLE"
        // another type's handler, a service name, an inherited name and another role's handler are no handler of this type
        ;[out.integ.otherTypes, out.integ.serviceName, out.integ.inherited, out.integ.sendRole].every(
            found => found.applied === false && found.reasonCode === "HANDLER_NOT_REGISTERED"
        )
        has(out.integ.expression.error, /may hold only the names/)
        has(out.integ.receiveExpression.error, /may not hold \$\{/)
        has(out.integ.parent.error, /\.\. segment/)
        has(out.integ.pattern.error, /not a regular expression/)
        out.integ.stale.reasonCode === "STALE_REVISION"
        out.integ.saved.applied === true
        out.integ.saved.changedFieldList.length === 8
        out.integ.same.applied === true
        out.integ.same.changed === false
        out.type.description === "Spec recovery incoming files"
        out.type.consumeServiceName === FAILS
        out.type.sendPath === "out/${date}/${systemMessageId}.json"
        out.type.receiveResponseEnumId === "MsgRrMove"
        // the receive service is never set
        out.type.receiveServiceName == null
        audit(TYPE, "consumeServiceName", types.incoming).some(
            found => found.oldValueText === NOOP && found.newValueText === FAILS && found.changeReason === "Spec transport"
        )
        one(TYPE, {systemMessageTypeId: types.out}).description === "Spec recovery outgoing"
        // the operations permissions read and change no message type
        has(out.manage.list.error, /ADMIN_CONNECTION_VIEW is required/)
        has(out.manage.store.error, /ADMIN_CONNECTION_MANAGE is required/)
    }
})
