// The system message part of org.moqui.admin.OperationServices (mr-boss plan/design-admin.md §8 SystemMessages and
// SystemMessageDetail, §8.1, §15.3 "Unknown provider outcome"; impl-admin P13): the transport outcomes of the instance's system
// messages (moqui.service.message.SystemMessage) for the person who recovers failures, and system messages as a kind of
// recovery target (OperationRecovery.js).
//
// What is read. A row answers a message's id, type, remote, direction, status and state, its times and failed attempts, its
// correlation ids (the envelope's and the remote's) and the class of its last error. Never its text: a payload is answered
// only by get#Message, only when the type's owner registered the permission that reads it (RecoveryDescriptor
// dataPermission), the reader holds it and, where the owner answers per message, the owner says this reader may see it. An
// error is answered as its class and one line with what a remote answered, every address, URL path and long token struck
// out; where an owner answers per message, the line and the correlation ids are shown only with the owner's word (the
// owner's scope, a store's or an organization's, is not known here).
//
// The state of a message, from its status and attempts:
// - waiting (Produced or Received, never failed), retrying (failed, and the scheduled send or consume tries it again),
//   failed (Error: nothing tries it again), done (Sent, Consumed, Confirmed), staged, rejected, cancelled;
// - inProgress: Sending or Consuming, begun within the time such a call may take;
// - uncertain: Sending or Consuming for longer than that. The server stopped, or the call never answered: the message may
//   have gone, or its consuming service may have applied it. It is never sent or consumed again from here; someone who
//   looked at the other side says which it was (reconcile), and only what is then reported not done may be retried;
// - settled: an uncertain one a person settled. A message's status is what the transport knows, and no person sets it
//   (the decision of Nov 10 for mail, impl-admin §4): the settlement is kept apart, in outcomeSettlement (done or notDone:
//   "reported", never known) with who, when and the reason, and in the audit log. The message stays Sending or Consuming,
//   is out of "Needs attention" and is never counted or shown as sent or consumed; no reader of statusId, status hook or
//   rule takes the settlement for a delivery. One reported not done may be retried: the retry's claim moves it to Error
//   and clears the settlement (it was of the attempt before), and the send's or consume's own result is its state.
//
// What is done. Nothing without a registration (OperationRecovery.js): a type whose owner registered no descriptor is read
// only. With one, the native actions are the framework's own services, called for the same stored message, so its identity
// is kept and no second message is made: retry (send#ProducedSystemMessage or consume#ReceivedSystemMessage, now, once),
// requeue (reset#SystemMessageInError: the scheduled job tries it again), cancel (cancel#SystemMessage, of a failed message
// only) and the reconciliation of an uncertain one (the settlement above: nothing is marked Sent or Consumed). Each is
// claimed first in a transaction of its own, the row locked at the revision read, so a second request finds it changed;
// the framework's send and consume then take the message themselves (claim#SystemMessage), so the scheduled job and a
// retry here never both send or consume it. A message of a remote that is disabled (a connection waiting for its test,
// ConfigurationServices.store#Connection) is not sent.
const lib = ec.resource.script("component://framework/service/org/moqui/admin/OperationLib.js")
const engine = ec.resource.script("component://framework/service/org/moqui/admin/OperationRecovery.js")
const {find, one, map, ms, iso, stamp, text, refused} = lib
const MESSAGE = "moqui.service.message.SystemMessage"
const TYPE = "moqui.service.message.SystemMessageType"
const REMOTE = "moqui.service.message.SystemMessageRemote"
const ERROR = "moqui.service.message.SystemMessageError"
const SM = "org.moqui.impl.SystemMessageServices."
const APPLY = "org.moqui.admin.OperationServices.apply#MessageRecovery"
// how long a send or a consume may be under way before its outcome is unknown (the consume service's own limit is 30 minutes)
const busyMinutes = {SmsgSending: 15, SmsgConsuming: 35}
// with no dates given, the list reads this far back
const defaultDays = 14
// a payload is shown up to this many characters
const payloadLimit = 20000

const stateList = [
    {state: "attention", text: "Needs attention"},
    {state: "failed", text: "Failed"},
    {state: "uncertain", text: "Outcome unknown"},
    {state: "settled", text: "Settled by a person"},
    {state: "waiting", text: "Waiting or in progress"},
    {state: "done", text: "Sent or consumed"},
    {state: "closed", text: "Cancelled or rejected"},
    {state: "all", text: "All"}
]
const now = () => ms(ec.user.nowTimestamp)
// what a person may report of an attempt whose outcome is unknown (SystemMessage.outcomeSettlement)
const settlements = ["done", "notDone"]
const settlementFields = ["outcomeSettlement", "outcomeSettledByUserId", "outcomeSettledDate", "outcomeSettledReason"]
// The settlement a message carries, or "": read only over a send or consume that never answered.
const settlementOf = message => (settlements.includes(text(message.outcomeSettlement)) ? text(message.outcomeSettlement) : "")
const settlementText = (message, settlement) =>
    "Reported " + (settlement === "done" ? "" : "not ") + (message.isOutgoing === "Y" ? "sent" : "applied")

// ===== reading =====
function stateOf(message) {
    switch (message.statusId) {
        case "SmsgCancelled":
            return "cancelled"
        case "SmsgRejected":
            return "rejected"
        case "SmsgSent":
        case "SmsgConsumed":
        case "SmsgConfirmed":
            return "done"
        case "SmsgStaged":
            return "staged"
        case "SmsgSending":
        case "SmsgConsuming": {
            // a settlement is of this attempt: the next send or consume clears it when it takes the message
            if (settlementOf(message)) return "settled"
            const began = ms(message.lastAttemptDate) ?? ms(message.lastUpdatedStamp)
            return began != null && now() - began < busyMinutes[message.statusId] * 60000 ? "inProgress" : "uncertain"
        }
        case "SmsgError":
            return "failed"
        case "SmsgProduced":
        case "SmsgReceived":
            return Number(message.failCount || 0) > 0 ? "retrying" : "waiting"
        default:
            return "other"
    }
}
const directionOf = message => (message.isOutgoing === "Y" ? "out" : "in")
const uncertainTextOf = direction => (direction === "out" ? "May have been sent" : "May have been applied")
function stateTextOf(message, state) {
    const out = directionOf(message) === "out"
    switch (state) {
        case "waiting":
            return out ? "Waiting to be sent" : "Waiting to be consumed"
        case "retrying":
            return "Failed, will be tried again"
        case "failed":
            return "Failed"
        case "inProgress":
            return out ? "Being sent" : "Being consumed"
        case "uncertain":
            return uncertainTextOf(directionOf(message))
        case "settled":
            return "Settled: " + settlementText(message, settlementOf(message)).toLowerCase()
        case "staged":
            return "Staged, not committed"
        case "rejected":
            return "Rejected"
        case "cancelled":
            return "Cancelled"
        default:
            return text(one("moqui.basic.StatusItem", {statusId: message.statusId})?.description) || text(message.statusId)
    }
}
// A text of an error, an owner or a remote as a reader of the queue may see it: one line, with what a remote answered, every
// address, URL path and query, credential and long token struck out.
function safeText(value) {
    return (
        text(value)
            .split(/\r?\n/)[0]
            .replace(/[\u0000-\u001f\u007f]+/g, " ")
            .replace(/^((Type|Reference)?Error:\s*)+/, "")
            // what the remote answered is the remote's: its status stays
            .replace(/(send error response \(\d{3}\)).*$/i, "$1")
            .replace(/\b([a-z][a-z0-9+.-]*:\/\/)[^\s\/@]*@/gi, "$1")
            .replace(/\b([a-z][a-z0-9+.-]*:\/\/[^\s\/?#"']+)[^\s"']*/gi, "$1")
            .replace(/[^\s<>()"',;:]+@[^\s<>()"',;:]+/g, "(address)")
            .replace(
                /\b(authorization|bearer|basic|password|passwd|secret|token|api[-_]?key|signature)\b(\s*[:=]?\s*)\S+/gi,
                "$1$2(hidden)"
            )
            .replace(/[A-Za-z0-9+\/_=-]{32,}/g, "(hidden)")
            .slice(0, 200)
    )
}
// The class of a failure, for telling a transport failure from a refusal without reading it.
function errorClassOf(value) {
    const found = text(value)
    if (!found) return ""
    const status = /send error response \((\d{3})\)/i.exec(found)
    if (status) return "HTTP " + status[1] + (status[1][0] === "5" ? ": the remote failed" : ": the remote refused it")
    if (/is disabled until a test/i.test(found)) return "Remote disabled"
    if (/timed? ?out/i.test(found)) return "Timeout"
    if (/sim mode|denied by|fence/i.test(found)) return "Not allowed to leave this instance"
    if (/connect|resolve|unreachable|certificate|handshake|\btls\b|\bdns\b/i.test(found)) return "Connection"
    if (/not authorized|requires login|permission|authz/i.test(found)) return "Authorization"
    if (/has no sendServiceName|has no consumeServiceName|has no type|not valid/i.test(found)) return "Configuration"
    return "Processing"
}
// A message as a recovery target (OperationRecovery.js).
function targetOf(message) {
    const state = stateOf(message)
    return {
        targetId: message.systemMessageId,
        targetKey: text(message.systemMessageTypeId),
        direction: directionOf(message),
        title: "Message " + message.systemMessageId,
        state,
        stateText: stateTextOf(message, state),
        statusId: message.statusId,
        // settled by a person: no longer unknown, whatever an owner says of it
        settled: state === "settled",
        settlement: state === "settled" ? settlementOf(message) : "",
        revision: stamp(message),
        row: message
    }
}
function read(ids) {
    const found = new Map()
    if (!ids.length) return found
    for (const row of Array.from(find(MESSAGE).condition("systemMessageId", "in", ids).list()))
        found.set(row.systemMessageId, targetOf(map(row)))
    return found
}
const remoteOf = message =>
    message.systemMessageRemoteId ? one(REMOTE, {systemMessageRemoteId: message.systemMessageRemoteId}) : null

// ===== the kind =====
const actions = {
    retry: {label: "Retry now", noun: "retry", modeField: "retryMode", ownerField: "retryEligible"},
    requeue: {label: "Queue again", noun: "requeue", modeField: "retryMode", ownerField: "retryEligible", nativeOnly: true},
    reconcile: {
        label: "Settle",
        noun: "reconciliation",
        modeField: "reconcileMode",
        ownerField: "reconcileEligible",
        resolutions: ["done", "notDone"]
    },
    cancel: {label: "Cancel", noun: "cancellation", modeField: "cancelMode", ownerField: "cancelEligible"}
}
// What never holds for an action on a message, whatever is registered: {reasonCode, reason}, or null.
function baseline(target, action, descriptor) {
    const out = target.direction === "out"
    const state = target.uncertain ? "uncertain" : target.state
    const hold = (reasonCode, reason) => ({reasonCode, reason})
    if (action === "reconcile")
        return state === "uncertain"
            ? null
            : state === "settled"
              ? hold("ALREADY_SETTLED", "It is settled already: " + target.stateText.replace(/^Settled: /, ""))
              : hold("NOT_UNCERTAIN", "Only a message whose outcome is unknown is settled; this one is: " + target.stateText)
    if (state === "uncertain")
        return hold(
            "RECONCILE_REQUIRED",
            (out ? "It may have been sent" : "It may have been applied") +
                ": it is settled first, by someone who looked at the other side, and never " +
                (out ? "sent" : "consumed") +
                " again from here"
        )
    if (state === "inProgress") return hold("IN_PROGRESS", "It is " + target.stateText.toLowerCase() + " now")
    // reported done by the person who settled it: never sent or consumed again
    if (state === "settled" && target.settlement === "done")
        return hold(
            "SETTLED",
            "It was settled as " + (out ? "reported sent: it is not sent again" : "reported applied: it is not consumed again")
        )
    if (action === "cancel" || action === "requeue")
        return target.statusId === "SmsgError"
            ? action === "requeue" && out && remoteOf(target.row)?.disabled === "Y"
                ? hold("REMOTE_DISABLED", "Its remote is disabled until a test of that connection passes")
                : null
            : hold("NOT_FAILED", "Only a failed message that nothing tries again is taken here; this one is: " + target.stateText)
    // retry
    const retryStates = text(descriptor?.retryStates)
        ? text(descriptor.retryStates)
              .split(",")
              .map(text)
              .filter(name => ["failed", "retrying"].includes(name))
        : ["failed", "retrying"]
    // one reported not done may be retried as a failed one may
    if (!retryStates.includes(state) && !(state === "settled" && target.settlement === "notDone"))
        return hold("NOT_FAILED", "Only a failed message is retried; this one is: " + target.stateText)
    if (out && remoteOf(target.row)?.disabled === "Y")
        return hold("REMOTE_DISABLED", "Its remote is disabled until a test of that connection passes")
    return null
}
// One step on a message in a transaction of its own (apply#MessageRecovery): whether it was taken.
const step = (target, name, reason = null) => {
    const taken = lib.callRegistered(APPLY, {
        systemMessageId: target.targetId,
        expectedRevision: target.revision,
        step: name,
        reason
    })
    if (taken.failure) throw new Error(taken.failure)
    return taken.out.taken === true
}
const stale = () => refused("STALE_REVISION", "It changed since it was read; read it again and repeat")
function latestError(systemMessageId) {
    return find(ERROR, {systemMessageId}).orderBy("-errorDate").limit(1).list()[0] || null
}
// The framework's own action on a message, for a descriptor that registered it as safe to repeat.
function nativeApply(target, action, given) {
    const out = target.direction === "out"
    if (action === "requeue") return step(target, "requeue") ? {applied: true} : stale()
    if (action === "cancel") return step(target, "cancel") ? {applied: true} : stale()
    if (action === "reconcile")
        return step(target, given.resolution, given.reason) ? {applied: true, resolution: given.resolution} : stale()
    // retry: claimed at the revision read, then the same stored message through the framework's own send or consume, once
    if (!step(target, "claim")) return stale()
    const called = out
        ? lib.callRegistered(SM + "send#ProducedSystemMessage", {systemMessageId: target.targetId, willRetry: false})
        : lib.callRegistered(SM + "consume#ReceivedSystemMessage", {
              systemMessageId: target.targetId,
              allowError: true,
              willRetry: false
          })
    const after = stateOf(map(one(MESSAGE, {systemMessageId: target.targetId})))
    if (!called.failure && !["failed", "retrying"].includes(after)) return {applied: true}
    // the framework's own claim found it taken (the scheduled job began it first): nothing was sent or consumed by this
    if (called.failure && after === "inProgress")
        return refused("IN_PROGRESS", "It is being " + (out ? "sent" : "consumed") + " by another request now")
    const error = called.failure || latestError(target.targetId)?.errorText
    return refused("RETRY_FAILED", "Not " + (out ? "sent" : "consumed") + ": " + (safeText(error) || "it failed again"), {
        errorClass: errorClassOf(error)
    })
}
function after(systemMessageId) {
    const message = map(one(MESSAGE, {systemMessageId}))
    const state = stateOf(message)
    return {state, stateText: stateTextOf(message, state), statusId: message.statusId, revision: stamp(message)}
}
function consequence(target, action, descriptor) {
    const out = target.direction === "out"
    const note = text(descriptor?.idempotencyNote)
    switch (action) {
        case "retry":
            return (
                (out
                    ? "The same message is sent to its remote again, now. No new message is made."
                    : "The same message is consumed again, now: its consuming service runs for it once more.") +
                (note ? " " + note : "")
            )
        case "requeue":
            return (
                "Its failed attempts are cleared and the scheduled " +
                (out ? "send" : "consume") +
                " tries it again, up to its retry limit." +
                (note ? " " + note : "")
            )
        case "reconcile":
            return (
                "Nothing is sent or consumed, and the message is not marked " +
                (out ? "sent" : "consumed") +
                ": what you found on the other side is recorded beside it, with your name and reason."
            )
        default:
            return "The message is closed and is not " + (out ? "sent" : "consumed") + ". It stays as evidence."
    }
}
const resolutionOf = (target, resolution) => {
    const out = target.direction === "out"
    return resolution === "done"
        ? {resolution, label: out ? "Reported sent (the remote has it)" : "Reported applied (what it does is done)"}
        : {
              resolution,
              label: out
                  ? "Reported not sent (it may then be sent again)"
                  : "Reported not applied (it may then be consumed again)"
          }
}
const recoveryKind = {
    targetKind: "SystemMessage",
    noun: "message",
    actions,
    read,
    baseline,
    nativeApply,
    claim: target => step(target, "claim"),
    // after an owner settled its own record: the same settlement beside the message, where its transport outcome is
    // unknown too (otherwise nothing is written: the owner's record is the owner's)
    ownerSettled: (systemMessageId, resolution, reason) => {
        const target = read([systemMessageId]).get(systemMessageId)
        if (target?.state === "uncertain" && settlements.includes(resolution)) step(target, resolution, reason)
    },
    after,
    consequence,
    resolutionOf,
    uncertainText: target => uncertainTextOf(target.direction),
    safeText,
    // what was asked of a message and why, beside the status rows the framework's services write
    audit: (target, afterwards, reasonText) =>
        lib.auditRow(MESSAGE, "statusId", target.targetId, null, target.statusId, afterwards.statusId, reasonText.slice(0, 255))
}

// apply#MessageRecovery: one step on a message, in a transaction of its own, the row locked, still at the revision read and
// still in the state the step is for. claim marks the attempt (so a second request at that revision finds it changed)
// and, for a message reported not done, moves it to Error with the settlement cleared, so the framework's send or consume
// takes it; requeue and cancel are the framework's own services; done and notDone record the settlement of a message
// whose outcome is unknown and leave its status as the transport left it.
function applyMessageRecovery(p) {
    if (!ec.artifactExecution.authzDisabled() && !ec.user.hasPermission(lib.MANAGE)) throw new Error(lib.MANAGE + " is required")
    const systemMessageId = text(p.systemMessageId)
    const name = text(p.step)
    const row = find(MESSAGE, {systemMessageId}).forUpdate(true).one()
    if (!row) return {taken: false}
    const message = map(row)
    if (stamp(message) !== text(p.expectedRevision)) return {taken: false}
    const state = stateOf(message)
    const call = (service, parameters) => {
        ec.service.sync().name(service).parameters(parameters).call()
        if (ec.message.hasError()) throw new Error(ec.message.getErrorsString())
    }
    switch (name) {
        case "claim":
            if (state === "settled" && settlementOf(message) === "notDone")
                // the attempt before is reported not done: this retry begins from Error, as a failed message's does
                row.setFields(
                    Object.assign(Object.fromEntries(settlementFields.map(field => [field, null])), {
                        statusId: "SmsgError",
                        lastAttemptDate: ec.user.nowTimestamp
                    }),
                    true,
                    null,
                    null
                )
            else if (["failed", "retrying"].includes(state)) row.set("lastAttemptDate", ec.user.nowTimestamp)
            else return {taken: false}
            row.update()
            return {taken: true}
        case "requeue":
            if (message.statusId !== "SmsgError") return {taken: false}
            call(SM + "reset#SystemMessageInError", {systemMessageId})
            return {taken: true}
        case "cancel":
            if (message.statusId !== "SmsgError") return {taken: false}
            call(SM + "cancel#SystemMessage", {systemMessageId})
            return {taken: true}
        case "done":
        case "notDone":
            if (state !== "uncertain") return {taken: false}
            // the settlement alone: the status, the processed date and the attempts stay as the transport left them
            row.setFields(
                {
                    outcomeSettlement: name,
                    outcomeSettledByUserId: ec.user.userId,
                    outcomeSettledDate: ec.user.nowTimestamp,
                    outcomeSettledReason: text(p.reason).slice(0, 255) || null
                },
                true,
                null,
                null
            )
            row.update()
            return {taken: true}
        default:
            throw new Error("Unknown recovery step " + name)
    }
}

// ===== the list =====
// The messages of a state, as a find: the conditions are in the query, before the page.
function messageFind(p) {
    const factory = ec.entity.conditionFactory
    const all = (...conditions) => factory.makeCondition(conditions, "and")
    const any = (...conditions) => factory.makeCondition(conditions, "or")
    const status = (...ids) => factory.makeCondition("statusId", "in", ids)
    const failedBefore = factory.makeCondition("failCount", "greater", 0)
    const neverFailed = any(factory.makeCondition("failCount", "equals", null), factory.makeCondition("failCount", "equals", 0))
    const unsettled = factory.makeCondition("outcomeSettlement", "equals", null)
    const stuck = (statusId, older) =>
        all(
            status(statusId),
            unsettled,
            factory.makeCondition(
                "lastAttemptDate",
                older ? "less" : "greater-equals",
                iso(now() - busyMinutes[statusId] * 60000)
            )
        )
    const failed = () => any(status("SmsgError"), all(status("SmsgProduced", "SmsgReceived"), failedBefore))
    const uncertain = () => any(stuck("SmsgSending", true), stuck("SmsgConsuming", true))
    const state = text(p.state) || "attention"
    if (!stateList.some(option => option.state === state)) throw new Error("Unknown message state " + state)
    let found = find(MESSAGE)
    if (state === "attention") found = found.condition(any(failed(), uncertain()))
    else if (state === "failed") found = found.condition(failed())
    else if (state === "uncertain") found = found.condition(uncertain())
    else if (state === "settled")
        found = found.condition(
            all(status("SmsgSending", "SmsgConsuming"), factory.makeCondition("outcomeSettlement", "in", settlements))
        )
    else if (state === "waiting")
        found = found.condition(
            any(
                all(status("SmsgProduced", "SmsgReceived"), neverFailed),
                status("SmsgStaged"),
                stuck("SmsgSending", false),
                stuck("SmsgConsuming", false)
            )
        )
    else if (state === "done") found = found.condition(status("SmsgSent", "SmsgConsumed", "SmsgConfirmed"))
    else if (state === "closed") found = found.condition(status("SmsgCancelled", "SmsgRejected"))
    if (text(p.systemMessageTypeId)) found = found.condition("systemMessageTypeId", text(p.systemMessageTypeId))
    if (text(p.systemMessageRemoteId)) found = found.condition("systemMessageRemoteId", text(p.systemMessageRemoteId))
    if (p.direction === "out") found = found.condition("isOutgoing", "Y")
    else if (p.direction === "in") found = found.condition("isOutgoing", "N")
    else if (text(p.direction)) throw new Error("A direction is in or out")
    // a reference is a whole id: the message's own, the envelope's or the remote's (no search inside a message)
    const reference = text(p.reference)
    if (reference)
        found = found.condition(
            any(
                factory.makeCondition("systemMessageId", "equals", reference),
                factory.makeCondition("messageId", "equals", reference),
                factory.makeCondition("remoteMessageId", "equals", reference)
            )
        )
    const from = ms(p.fromDate)
    const thru = ms(p.thruDate)
    if ((p.fromDate && (from == null || Number.isNaN(from))) || (p.thruDate && (thru == null || Number.isNaN(thru))))
        throw new Error("A date is not a date")
    // one message asked for by a reference is found whatever its age
    const since = from ?? (thru == null && !reference ? now() - defaultDays * 86400000 : null)
    if (since != null) found = found.condition("initDate", "greater-equals", iso(since))
    if (thru != null) found = found.condition("initDate", "less", iso(thru))
    return found
}
// The last error of each of these messages, in one find.
function latestErrors(ids) {
    const byMessage = new Map()
    if (!ids.length) return byMessage
    for (const row of Array.from(find(ERROR).condition("systemMessageId", "in", ids).orderBy("-errorDate").limit(2000).list()))
        if (!byMessage.has(row.systemMessageId)) byMessage.set(row.systemMessageId, map(row))
    return byMessage
}
const named = (entityName, keyField, ids) =>
    new Map(
        ids.length ? Array.from(find(entityName).condition(keyField, "in", ids).list()).map(row => [row[keyField], map(row)]) : []
    )
// Whether a reader is shown the line of a message's errors: always its class; the line only where no owner answers per
// message, or the owner says this reader may see it. The same holds for its references, its remote, who settled it and
// the reasons people gave: of a message its owner holds from this reader, only its transport row is answered.
const detailVisible = assessed =>
    !text(assessed.descriptor?.describeService) ? true : assessed.owner?.answer?.detailVisible === true
// A message as the list and the detail show it, with its owner and what may be done here.
function described(target, assessed, error, canManage, types, remotes, userNames = new Map()) {
    const message = target.row
    const state = assessed.uncertain ? "uncertain" : target.state
    const settlement = state === "settled" ? target.settlement : ""
    const verdicts = assessed.verdicts
    const can = action => canManage && verdicts[action].eligible
    const type = types.get(message.systemMessageTypeId)
    const open = ["failed", "retrying", "uncertain"].includes(state)
    // a message's own references, its remote and the line of its error are its owner's to show, where the owner answers
    // per message (a remote names the store or the partner it is of)
    const shown = detailVisible(assessed)
    const remote = shown ? remotes.get(message.systemMessageRemoteId) : null
    // why nothing is offered, where something would be expected
    const asked = state === "uncertain" ? verdicts.reconcile : verdicts.retry
    const nextStep = settlement
        ? settlement === "notDone" && verdicts.retry.eligible
            ? "Reported not " +
              (target.direction === "out" ? "sent: it can be sent again." : "applied: it can be consumed again.")
            : ""
        : !open
          ? ""
          : asked.eligible
            ? state === "uncertain"
                ? "Look at the other side, then settle it."
                : state === "retrying"
                  ? "It will be tried again on its own; it can be retried now."
                  : "It can be retried."
            : asked.reason
    return Object.assign(
        {
            systemMessageId: message.systemMessageId,
            systemMessageTypeId: message.systemMessageTypeId || null,
            typeText: text(type?.description) || text(message.systemMessageTypeId),
            systemMessageRemoteId: shown ? message.systemMessageRemoteId || null : null,
            remoteText: shown ? text(remote?.description) || text(message.systemMessageRemoteId) : "",
            remoteDisabled: remote?.disabled === "Y",
            direction: target.direction,
            directionText: target.direction === "out" ? "Sent from here" : "Received",
            statusId: message.statusId,
            state,
            stateText: assessed.uncertain && target.state !== "uncertain" ? uncertainTextOf(target.direction) : target.stateText,
            initDate: iso(message.initDate),
            processedDate: iso(message.processedDate),
            lastAttemptDate: iso(message.lastAttemptDate),
            failCount: Number(message.failCount || 0),
            messageId: shown ? text(message.messageId).slice(0, 80) || null : null,
            remoteMessageId: shown ? text(message.remoteMessageId).slice(0, 80) || null : null,
            parentMessageId: message.parentMessageId || null,
            ackMessageId: message.ackMessageId || null,
            docType: shown ? text(message.docType).slice(0, 60) || null : null,
            errorClass: open || state === "inProgress" ? errorClassOf(error?.errorText) : "",
            errorText: open && shown ? safeText(error?.errorText) : "",
            // the settlement, apart from the status: what a person reported, who, when and why; null on every other state
            settlement: settlement || null,
            settlementText: settlement ? settlementText(message, settlement) : null,
            settledByName:
                settlement && shown
                    ? userNames.get(message.outcomeSettledByUserId) || message.outcomeSettledByUserId || null
                    : null,
            settledDate: settlement ? iso(message.outcomeSettledDate) : null,
            settledReason: settlement && shown ? text(message.outcomeSettledReason) || null : null,
            retryEligible: can("retry"),
            requeueEligible: can("requeue"),
            reconcileEligible: can("reconcile"),
            cancelEligible: can("cancel"),
            nextStep,
            revision: target.revision
        },
        engine.registrationOf(assessed)
    )
}

// The names of the people who settled these messages, read in one find.
function settlerNames(targets) {
    const ids = Array.from(new Set(targets.map(target => target.row.outcomeSettledByUserId).filter(Boolean)))
    const names = new Map()
    for (const row of named("moqui.security.UserAccount", "userId", ids).values())
        names.set(row.userId, text(row.userFullName) || text(row.username) || row.userId)
    return names
}

// find#Messages: a page of system messages, newest first, each with its state, its owner and what may be done here.
function findMessages(p) {
    lib.requireView()
    const canManage = ec.user.hasPermission(lib.MANAGE)
    const {pageIndex, pageSize} = lib.pageOf(p)
    const messageCount = Number(messageFind(p).count())
    const targets = Array.from(
        messageFind(p)
            .orderBy("-initDate,-systemMessageId")
            .offset(pageIndex * pageSize)
            .limit(pageSize)
            .list()
    ).map(row => targetOf(map(row)))
    const assessed = engine.assess(recoveryKind, targets)
    const errors = latestErrors(
        targets.filter(target => !["done", "cancelled", "waiting"].includes(target.state)).map(target => target.targetId)
    )
    const ids = field => Array.from(new Set(targets.map(target => target.row[field]).filter(Boolean)))
    const types = named(TYPE, "systemMessageTypeId", ids("systemMessageTypeId"))
    const remotes = named(REMOTE, "systemMessageRemoteId", ids("systemMessageRemoteId"))
    const userNames = settlerNames(targets)
    return {
        messageList: targets.map(target =>
            described(target, assessed.get(target.targetId), errors.get(target.targetId), canManage, types, remotes, userNames)
        ),
        messageCount,
        pageIndex,
        pageSize,
        canManage,
        stateList,
        typeList: Array.from(find(TYPE).selectField("systemMessageTypeId,description").orderBy("description").list()).map(
            row => ({
                systemMessageTypeId: row.systemMessageTypeId,
                description: text(row.description) || row.systemMessageTypeId
            })
        ),
        defaultDays,
        maxTargets: engine.maxTargets
    }
}

// ===== one message =====
// Whether this reader is shown a message's payload, and when not, why: the owner's registered permission, held, and the
// owner's word where it answers per message.
function payloadAccess(assessed) {
    const descriptor = assessed.descriptor
    const owner = engine.ownerName(descriptor)
    if (!descriptor) return "No owner has registered who may read this message's content"
    const permission = text(descriptor.dataPermission)
    if (!permission) return owner + " shows what this message holds in its own screens only"
    if (!ec.user.hasPermission(permission)) return "Its content needs " + owner + "'s own permission (" + permission + ")"
    if (text(descriptor.describeService)) {
        if (assessed.owner?.unanswered) return owner + " could not be asked whether you may read it"
        if (assessed.owner?.answer?.payloadVisible !== true) return owner + " does not show this message's content to you here"
    }
    return ""
}
// A payload as shown: cut to the limit, with the values of fields named like a credential struck out.
const shownPayload = value =>
    text(value)
        .slice(0, payloadLimit)
        .replace(
            /((?:authorization|password|passwd|secret|token|api[-_]?key|signature)[\w-]*["']?\s*[:=]\s*)("[^"]*"|'[^']*'|[^\s,;&<]+)/gi,
            "$1(hidden)"
        )

// get#Message: one message's evidence: its row, its attempts, its status history, what is registered for it with each
// action's preview and, only for a reader its owner lets, its payload.
function getMessage(p) {
    lib.requireView()
    const canManage = ec.user.hasPermission(lib.MANAGE)
    const systemMessageId = text(p.systemMessageId)
    const target = read([systemMessageId]).get(systemMessageId)
    if (!target) throw new Error("No system message " + systemMessageId)
    const message = target.row
    const assessed = engine.assess(recoveryKind, [target]).get(systemMessageId)
    const lineVisible = detailVisible(assessed)
    const attempts = Array.from(find(ERROR, {systemMessageId}).orderBy("-errorDate").limit(20).list()).map(map)
    const statusText = statusId => text(one("moqui.basic.StatusItem", {statusId})?.description) || text(statusId)
    const types = named(TYPE, "systemMessageTypeId", [message.systemMessageTypeId].filter(Boolean))
    const remotes = named(REMOTE, "systemMessageRemoteId", [message.systemMessageRemoteId].filter(Boolean))
    const hidden = payloadAccess(assessed)
    const wanted = p.includePayload === true || p.includePayload === "true"
    const payloadText = wanted && !hidden ? shownPayload(message.messageText) : null
    return {
        message: Object.assign(described(target, assessed, attempts[0], canManage, types, remotes, settlerNames([target])), {
            contentType: text(types.get(message.systemMessageTypeId)?.contentType) || null,
            payloadLength: text(message.messageText).length,
            childCount: Number(find(MESSAGE, {parentMessageId: systemMessageId}).count())
        }),
        attemptList: attempts.map(row => ({
            errorDate: iso(row.errorDate),
            attemptedText:
                row.attemptedStatusId === "SmsgSent"
                    ? "Send"
                    : row.attemptedStatusId === "SmsgConsumed"
                      ? "Consume"
                      : statusText(row.attemptedStatusId),
            errorClass: errorClassOf(row.errorText),
            errorText: lineVisible ? safeText(row.errorText) : ""
        })),
        historyList: Array.from(
            find("moqui.entity.EntityAuditLog", {
                changedEntityName: MESSAGE,
                changedFieldName: "statusId",
                pkPrimaryValue: systemMessageId
            })
                .orderBy("-changedDate")
                .limit(30)
                .list()
        ).map(row => ({
            changedDate: iso(row.changedDate),
            fromText: statusText(row.oldValueText),
            toText: statusText(row.newValueText),
            // who acted and the reason they gave are shown as the error's line is: where its owner lets this reader
            username:
                lineVisible && row.changedByUserId
                    ? text(one("moqui.security.UserAccount", {userId: row.changedByUserId})?.username) || row.changedByUserId
                    : null,
            reason: lineVisible ? text(row.changeReason) || null : null
        })),
        actionList: engine.actionList(recoveryKind, target, assessed, canManage),
        canManage,
        payloadVisible: !hidden,
        payloadHiddenReason: hidden || null,
        payloadText,
        payloadTruncated: payloadText != null && text(message.messageText).length > payloadLimit
    }
}

const operations = {findMessages, getMessage, applyMessageRecovery}
;({
    recoveryKind,
    stateOf,
    safeText,
    errorClassOf,
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown message operation " + op)
        return operations[op](parameters)
    }
})
