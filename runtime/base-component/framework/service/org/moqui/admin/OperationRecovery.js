// The recovery part of org.moqui.admin.OperationServices (mr-boss plan/design-admin.md §8.1, §14 "Operational recovery", §15.3
// "Unknown provider outcome" and "Independent failure"; impl-admin P13): preview#Recovery and execute#Recovery, the one
// dispatch of a retry, a reconciliation or a cancellation of an operational record to what its owner registered for it.
//
// What may be recovered. An owner seeds a moqui.server.RecoveryDescriptor for a kind of record (targetKind) and what of that
// kind it covers (targetKey): without one, nothing is done here. For each action the descriptor names a mode: native (the
// kind's own framework action, which the owner thereby says is safe to repeat), owner (the owner's own service,
// recover#OwnedTarget, which checks the owner's permission and scope) or none. It may name the owner's permission an actor
// needs besides ADMIN_OPERATION_MANAGE and a service that answers per record what the owner would take now
// (find#RecoveryTargets). The registration adds no right, and no service or record name comes from a request.
//
// The kinds. A kind is a module beside this one that knows its records (kindModules below): how one is read with its state
// and revision, which actions it has, what never holds for an action whatever a descriptor says (its baseline: a record
// whose outcome is unknown is reconciled and never retried; one in progress is left alone), and its native actions. System
// messages are OperationMessages.js; job runs add theirs.
//
// A batch. Each record is on its own: read again, checked at the revision its caller read, claimed in a transaction of its
// own, then acted on apart from the others, so one that is refused or fails is named in its result and the rest go on. An
// operation key answers a repeat of the same batch with its first results and does nothing again.
const lib = ec.resource.script("component://framework/service/org/moqui/admin/OperationLib.js")
const {rows, map, text, refused} = lib
const DESCRIPTOR = "moqui.server.RecoveryDescriptor"
const kindModules = {SystemMessage: "component://framework/service/org/moqui/admin/OperationMessages.js"}
const modeFields = ["retryMode", "reconcileMode", "cancelMode"]
const modes = ["native", "owner"]
// the records of one preview or one batch
const maxTargets = 25

function kindOf(targetKind) {
    const name = text(targetKind)
    if (!Object.hasOwn(kindModules, name)) throw new Error("Unknown recovery target kind " + name)
    return ec.resource.script(kindModules[name]).recoveryKind
}
// A short, stable name for a longer text (FNV-1a, twice), for keys that must stay within a column.
function digest(value) {
    const once = seed => {
        let hash = seed
        for (const character of String(value)) {
            hash ^= character.codePointAt(0)
            hash = Math.imul(hash, 0x01000193) >>> 0
        }
        return hash.toString(16).padStart(8, "0")
    }
    return once(0x811c9dc5) + once(0x01000193)
}

// ===== descriptors =====
// The descriptors of a kind in use: each is checked when read, and one with no target, an unknown mode, an owner mode with
// no service or a service that is not defined is logged by name and left out, so its records are offered nothing.
function descriptorsOf(targetKind) {
    const found = []
    for (const row of rows(DESCRIPTOR, {targetKind})) {
        if (row.disabled === "Y") continue
        const descriptor = map(row)
        const named = [descriptor.describeService, descriptor.recoverService].map(text).filter(Boolean)
        const problem = !text(descriptor.targetKey)
            ? "names no target"
            : modeFields.some(field => text(descriptor[field]) && !modes.includes(text(descriptor[field])))
              ? "names a mode that is not native or owner"
              : modeFields.some(field => text(descriptor[field]) === "owner") && !text(descriptor.recoverService)
                ? "names an owner action and no service"
                : named.some(name => !ec.service.getServiceDefinition(name))
                  ? "names a service that is not defined"
                  : ""
        if (problem) {
            ec.logger.error("Recovery descriptor " + descriptor.descriptorKey + " " + problem + "; left out")
            continue
        }
        found.push(descriptor)
    }
    return found.sort(
        (a, b) => Number(a.sequenceNum ?? 9999) - Number(b.sequenceNum ?? 9999) || a.descriptorKey.localeCompare(b.descriptorKey)
    )
}
// The first descriptor that covers a record, by its target key and, where the descriptor names one, its direction.
const descriptorFor = (descriptors, target) =>
    descriptors.find(
        descriptor =>
            text(descriptor.targetKey) === target.targetKey &&
            (!text(descriptor.direction) || text(descriptor.direction) === target.direction)
    ) || null
const ownerName = descriptor => text(descriptor?.ownerApp) || text(descriptor?.title) || "Its owner"

// What the owners say of these records: Map targetId -> {answer} or {unanswered: true}, for the records whose descriptor
// names a describe service. One owner that fails is logged and leaves its own records unanswered; the others are asked.
// An answer is read as data: its texts are cut to a line and its flags are true only when exactly true.
function ownerAnswers(kind, targets, descriptors) {
    const answers = new Map()
    const byDescriptor = new Map()
    for (const target of targets) {
        const descriptor = descriptorFor(descriptors, target)
        if (!descriptor || !text(descriptor.describeService)) continue
        if (!byDescriptor.has(descriptor.descriptorKey)) byDescriptor.set(descriptor.descriptorKey, {descriptor, ids: []})
        byDescriptor.get(descriptor.descriptorKey).ids.push(target.targetId)
    }
    for (const {descriptor, ids} of byDescriptor.values()) {
        const asked = lib.callRegistered(text(descriptor.describeService), {targetKind: kind.targetKind, targetIdList: ids})
        if (asked.failure) {
            ec.logger.error("Recovery descriptor " + descriptor.descriptorKey + ": its owner did not answer: " + asked.failure)
            for (const id of ids) answers.set(id, {unanswered: true})
            continue
        }
        for (const answered of Array.from(asked.out.targetList || [])) {
            const answer = map(answered)
            const id = text(answer.targetId)
            if (!ids.includes(id) || answers.has(id)) continue
            answers.set(id, {
                answer: {
                    ownerRef: text(answer.ownerRef).slice(0, 80),
                    ownerStateText: kind.safeText(answer.ownerStateText).slice(0, 120),
                    reason: kind.safeText(answer.reason).slice(0, 200),
                    retryEligible: answer.retryEligible === true,
                    reconcileEligible: answer.reconcileEligible === true,
                    cancelEligible: answer.cancelEligible === true,
                    uncertain: answer.uncertain === true,
                    detailVisible: answer.detailVisible === true,
                    payloadVisible: answer.payloadVisible === true
                }
            })
        }
    }
    return answers
}

// ===== eligibility =====
// Whether an action may be taken on a record now: {eligible: true, mode} or {eligible: false, reasonCode, reason}. In order:
// a registration, the action offered by it, the owner's permission, the kind's baseline (with the owner's word that the
// outcome is unknown), then the owner's own answer for the record.
function evaluate(kind, target, action, descriptor, owner) {
    const no = (reasonCode, reason) => ({eligible: false, reasonCode, reason})
    const spec = kind.actions[action]
    if (!descriptor)
        return no(
            "NOT_REGISTERED",
            "No recovery is registered for this " + kind.noun + ": it is recovered by its owner, or by an instance administrator"
        )
    const mode = text(descriptor[spec.modeField])
    if (!mode || (spec.nativeOnly && mode !== "native"))
        return no("NOT_OFFERED", ownerName(descriptor) + " offers no " + spec.noun + " of it here")
    const permission = text(descriptor.managePermission)
    if (permission && !ec.user.hasPermission(permission))
        return no("OWNER_PERMISSION", "This also needs " + ownerName(descriptor) + "'s own permission (" + permission + ")")
    const uncertain = target.state === "uncertain" || owner?.answer?.uncertain === true
    const held = kind.baseline(Object.assign({}, target, {uncertain}), action, descriptor)
    if (held) return no(held.reasonCode, held.reason)
    if (text(descriptor.describeService)) {
        if (owner?.unanswered)
            return no("OWNER_UNAVAILABLE", ownerName(descriptor) + " could not be asked about it. Try again later.")
        if (!owner?.answer) return no("NOT_OWNED", ownerName(descriptor) + " does not answer for this one")
        if (owner.answer[spec.ownerField] !== true)
            return no(
                "OWNER_REFUSES",
                owner.answer.reason || owner.answer.ownerStateText || ownerName(descriptor) + " does not take this now"
            )
    }
    return {eligible: true, mode}
}
// The records of a list with what is registered for each and what may be done: Map targetId -> {descriptor, owner,
// uncertain, verdicts: {action: verdict}}. Shared by the previews and by a kind's own list and detail.
function assess(kind, targets) {
    const descriptors = descriptorsOf(kind.targetKind)
    const owners = ownerAnswers(kind, targets, descriptors)
    const assessed = new Map()
    for (const target of targets) {
        const descriptor = descriptorFor(descriptors, target)
        const owner = owners.get(target.targetId) || null
        const verdicts = Object.fromEntries(
            Object.keys(kind.actions).map(action => [action, evaluate(kind, target, action, descriptor, owner)])
        )
        assessed.set(target.targetId, {
            descriptor,
            owner,
            uncertain: target.state === "uncertain" || owner?.answer?.uncertain === true,
            verdicts
        })
    }
    return assessed
}
// One record's actions as a preview shows them: what each would do, whether it may be done now and, when not, why.
function actionList(kind, target, assessed, canManage) {
    return Object.entries(kind.actions).map(([action, spec]) => {
        const verdict = assessed.verdicts[action]
        return {
            action,
            label: spec.label,
            eligible: canManage && verdict.eligible,
            reasonCode: verdict.eligible ? (canManage ? null : "PERMISSION") : verdict.reasonCode,
            reason: verdict.eligible ? (canManage ? null : lib.MANAGE + " is required") : verdict.reason,
            mode: verdict.eligible ? verdict.mode : null,
            consequence: kind.consequence(target, action, assessed.descriptor),
            resolutionList: spec.resolutions ? spec.resolutions.map(resolution => kind.resolutionOf(target, resolution)) : []
        }
    })
}
// What a record's registration says, for a list or a detail (never a service name).
function registrationOf(assessed) {
    const descriptor = assessed.descriptor
    return {
        descriptorKey: descriptor?.descriptorKey || null,
        ownerApp: descriptor ? ownerName(descriptor) : null,
        ownerTitle: text(descriptor?.title) || null,
        ownerRoute: text(descriptor?.ownerRoute) || null,
        ownerPermission: text(descriptor?.managePermission) || null,
        idempotencyNote: text(descriptor?.idempotencyNote) || null,
        ownerRef: assessed.owner?.answer?.ownerRef || null,
        ownerStateText: assessed.owner?.answer?.ownerStateText || null,
        ownerUnanswered: assessed.owner?.unanswered === true
    }
}

// The record ids of a request: a list, or one comma separated text; unique, at most maxTargets.
function idList(value) {
    const given = Array.isArray(value) ? value : value?.toArray ? Array.from(value.toArray()) : text(value).split(",")
    const ids = Array.from(new Set(given.map(text).filter(Boolean)))
    if (!ids.length) throw new Error("Choose what to recover")
    if (ids.length > maxTargets) throw new Error("At most " + maxTargets + " at a time")
    return ids
}

// preview#Recovery: what may be done to each of these records now, and what it would do. Nothing is changed.
function previewRecovery(p) {
    lib.requireView()
    const kind = kindOf(p.targetKind)
    const ids = idList(p.targetIdList ?? p.targetId)
    const canManage = ec.user.hasPermission(lib.MANAGE)
    const targets = kind.read(ids)
    const assessed = assess(kind, Array.from(targets.values()))
    return {
        targetKind: kind.targetKind,
        canManage,
        previewList: ids.map(targetId => {
            const target = targets.get(targetId)
            if (!target) return {targetId, found: false, title: targetId, stateText: "Not found", actionList: []}
            const own = assessed.get(targetId)
            return Object.assign(
                {
                    targetId,
                    found: true,
                    title: target.title,
                    state: own.uncertain ? "uncertain" : target.state,
                    stateText: own.uncertain && target.state !== "uncertain" ? kind.uncertainText(target) : target.stateText,
                    revision: target.revision,
                    actionList: actionList(kind, target, own, canManage)
                },
                registrationOf(own)
            )
        })
    }
}

// ===== acting =====
// One record of a batch: read again, at the revision its caller read, eligible now, then the registered action. Its result
// never throws: a failure is this record's own.
function executeOne(kind, action, resolution, reason, key, item, descriptors) {
    const targetId = text(item.targetId)
    const result = fields => Object.assign({targetId}, fields)
    try {
        const target = kind.read([targetId]).get(targetId)
        if (!target) return result(refused("NOT_FOUND", "No such " + kind.noun))
        if (text(item.expectedRevision) !== target.revision)
            return result(
                refused("STALE_REVISION", "It changed since it was read; read it again and repeat", {
                    state: target.state,
                    stateText: target.stateText,
                    revision: target.revision
                })
            )
        const descriptor = descriptorFor(descriptors, target)
        const owner = ownerAnswers(kind, [target], descriptors).get(targetId) || null
        const verdict = evaluate(kind, target, action, descriptor, owner)
        if (!verdict.eligible)
            return result(
                refused(verdict.reasonCode, verdict.reason, {
                    state: target.state,
                    stateText: target.stateText,
                    revision: target.revision
                })
            )
        let done
        if (verdict.mode === "native") done = kind.nativeApply(target, action, {resolution, reason})
        else if (!kind.claim(target)) done = refused("STALE_REVISION", "It changed since it was read; read it again and repeat")
        else {
            // the owner's own action, keyed so the owner too answers a repeat with its first result
            const asked = lib.callRegistered(text(descriptor.recoverService), {
                targetKind: kind.targetKind,
                targetId,
                ownerRef: owner?.answer?.ownerRef || null,
                action,
                resolution: resolution || null,
                reason,
                operationKey:
                    "adm." + digest([key || ec.user.userId + ":" + target.revision, action, resolution, targetId].join(":"))
            })
            if (asked.failure)
                done = refused("OWNER_REFUSED", ownerName(descriptor) + " did not take it: " + kind.safeText(asked.failure))
            else if (asked.out.applied !== true)
                done = refused(
                    text(asked.out.reasonCode).slice(0, 40) || "OWNER_REFUSED",
                    kind.safeText(asked.out.reason) || ownerName(descriptor) + " did not take it"
                )
            else done = {applied: true, ownerStateText: kind.safeText(asked.out.ownerStateText).slice(0, 120)}
        }
        const after = kind.after(targetId)
        // what was asked for and by whom, with the reason, whatever came of it (the claim was made: something was tried)
        if (done.applied || done.reasonCode !== "STALE_REVISION")
            kind.audit(target, after, (done.applied ? "" : "Not applied. ") + kind.actions[action].label + ": " + reason)
        return result(Object.assign(done, after))
    } catch (error) {
        ec.logger.error("Recovery of " + kind.noun + " " + targetId + " failed: " + String(error?.message ?? error))
        return result(refused("FAILED", kind.safeText(String(error?.message ?? error)) || "It could not be recovered"))
    }
}
// The records of a batch with the revision each was read at: targetList ({targetId, expectedRevision}), or one targetId with
// its expectedRevision.
function itemList(p) {
    const given = p.targetList
        ? Array.from(p.targetList.toArray ? p.targetList.toArray() : p.targetList).map(map)
        : [{targetId: p.targetId, expectedRevision: p.expectedRevision}]
    const items = []
    for (const item of given) {
        const targetId = text(item.targetId)
        if (!targetId || items.some(existing => existing.targetId === targetId)) continue
        items.push({targetId, expectedRevision: text(item.expectedRevision)})
    }
    if (!items.length) throw new Error("Choose what to recover")
    if (items.length > maxTargets) throw new Error("At most " + maxTargets + " at a time")
    return items
}

// execute#Recovery: the registered action on each record of the batch, each on its own; a result per record.
function executeRecovery(p) {
    lib.requireManage()
    const kind = kindOf(p.targetKind)
    const action = text(p.action)
    if (!Object.hasOwn(kind.actions, action)) throw new Error("Unknown recovery action " + action)
    const spec = kind.actions[action]
    const resolution = text(p.resolution)
    if (spec.resolutions ? !spec.resolutions.includes(resolution) : !!resolution)
        throw new Error(
            spec.resolutions ? "A resolution is one of " + spec.resolutions.join(", ") : "This action takes no resolution"
        )
    const reason = lib.requiredReason(p)
    const items = itemList(p)
    const key = lib.operationKey(p)
    const batchKey =
        kind.targetKind +
        ":" +
        action +
        (resolution ? "." + resolution : "") +
        ":" +
        items.length +
        ":" +
        digest(
            items
                .map(item => item.targetId)
                .sort()
                .join(",")
        )
    const run = () => {
        const descriptors = descriptorsOf(kind.targetKind)
        const resultList = items.map(item => executeOne(kind, action, resolution, reason, key, item, descriptors))
        const appliedCount = resultList.filter(row => row.applied === true).length
        const refusedCount = resultList.length - appliedCount
        return {
            // a batch that did anything is kept for its key; one where nothing was done may be corrected and repeated
            applied: appliedCount > 0,
            reasonCode: appliedCount ? null : resultList.length === 1 ? resultList[0].reasonCode : "NONE_APPLIED",
            reason: appliedCount ? null : resultList.length === 1 ? resultList[0].reason : "None of them could be recovered",
            targetKind: kind.targetKind,
            action,
            appliedCount,
            refusedCount,
            resultList
        }
    }
    return lib.once(key, "recovery", batchKey, run, keptResult)
}
// What is kept of a batch's results for its operation key: each record's outcome, its reason cut short, or left out when
// the whole would not fit the column.
function keptResult(result) {
    const short = reasonLength =>
        Object.assign({}, result, {
            resultList: result.resultList.map(row => ({
                targetId: row.targetId,
                applied: row.applied === true,
                reasonCode: row.reasonCode || null,
                reason: reasonLength ? text(row.reason).slice(0, reasonLength) || null : null,
                stateText: reasonLength ? row.stateText || null : null
            }))
        })
    return [80, 30, 0].map(short).find(kept => JSON.stringify(kept).length < 3900) || short(0)
}

const operations = {previewRecovery, executeRecovery}
;({
    descriptorsOf,
    descriptorFor,
    ownerName,
    assess,
    actionList,
    registrationOf,
    digest,
    maxTargets,
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown recovery operation " + op)
        return operations[op](parameters)
    }
})
