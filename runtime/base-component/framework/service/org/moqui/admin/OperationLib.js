// What the parts of org.moqui.admin.OperationServices share (mr-boss plan/design-admin.md §8.1, §9, §10, §17.2; impl-admin P12
// and later): each part is a module of its own beside this one (OperationMail.js the mail queue; system messages and recovery,
// jobs and topics add theirs), and each service checks its own permission with ec.user.hasPermission (ADMIN_OPERATION_VIEW to
// read, ADMIN_OPERATION_MANAGE to act), never a group name. A refusal a person can act on is answered as a reason (applied
// false, reasonCode, reason), never written; a missing permission or a malformed request is an error. An action names the
// revision it read and may carry an operation key, so its repeat answers the first result. No dependency on Mantle or an app.
const lib = ec.resource.script("component://framework/service/org/moqui/admin/ConfigurationLib.js")
const {one, text} = lib
const VIEW = "ADMIN_OPERATION_VIEW"
const MANAGE = "ADMIN_OPERATION_MANAGE"
const RESULT = "moqui.server.OperationResult"

const requireView = () => lib.requireView(VIEW, MANAGE)
const requireManage = () => lib.requireManage(MANAGE)

// A reason is required for an action that settles something by a person's word.
function requiredReason(p) {
    const reason = lib.reasonText(p)
    if (!reason) throw new Error("Give a reason")
    return lib.plainText(reason, "The reason")
}
// An operation key as its caller gave it: letters, digits and . _ : -, at most 64; "" for none.
function operationKey(p) {
    const key = text(p.operationKey)
    if (key && !/^[A-Za-z0-9_.:-]{1,64}$/.test(key))
        throw new Error("An operation key is 1 to 64 letters, digits, dots, underscores, colons or hyphens")
    return key
}
// Run an action once for its operation key: a repeat by the same user for the same kind and target answers the first
// result (with repeated true) and does nothing; a key used for another operation is an error. Only an applied result is
// kept, so a refusal can be corrected and tried again under the same key. With no key the action runs. kept, when given,
// makes what is stored of a result (a batch keeps a short form of its results, which must fit the column).
function once(key, operationKind, targetKey, action, kept = result => result) {
    if (!key) return action()
    const fullKey = operationKind + ":" + key
    const earlier = one(RESULT, {operationKey: fullKey})
    if (earlier) {
        if (earlier.targetKey !== targetKey || earlier.userId !== ec.user.userId)
            throw new Error("The operation key " + key + " was used for another operation")
        return Object.assign(JSON.parse(earlier.resultText || "{}"), {repeated: true})
    }
    const result = action()
    if (result?.applied)
        lib.write(
            RESULT,
            {
                operationKey: fullKey,
                operationKind,
                targetKey,
                userId: ec.user.userId,
                operationDate: ec.user.nowTimestamp,
                resultText: JSON.stringify(kept(result))
            },
            true
        )
    return result
}
// Call a service another component registered (an owner's provider), apart from the caller's transaction and errors:
// {out} its answer, or {failure} why it has none (never thrown, so one owner that fails does not stop the others).
function callRegistered(serviceName, parameters) {
    let out = null
    let failure = ""
    ec.message.pushErrors()
    try {
        out = ec.service.sync().name(serviceName).parameters(parameters).requireNewTransaction(true).call() || {}
        if (ec.message.hasError()) failure = ec.message.getErrorsString()
    } catch (error) {
        failure = String(error?.message ?? error)
    } finally {
        ec.message.clearErrors()
        ec.message.popErrors()
    }
    return failure ? {failure} : {out}
}
// A page of a find: pageIndex from 0, pageSize 1 to 100 (50 when not given).
function pageOf(p) {
    const pageSize = Math.min(Math.max(Number(p.pageSize) || 50, 1), 100)
    const pageIndex = Math.max(Number(p.pageIndex) || 0, 0)
    return {pageIndex, pageSize}
}

;({
    ...lib,
    VIEW,
    MANAGE,
    RESULT,
    requireView,
    requireManage,
    requiredReason,
    operationKey,
    once,
    callRegistered,
    pageOf
})
