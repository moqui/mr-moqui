// E2EE by default (plans/Comms-Messaging.md §6.5; docs/interfaces/Chat-Keys.md "Encrypted by default"): get#EncryptionDefault
// answers one bit for a set of people, true only when the asker and each of them have a crypto device in use (not revoked,
// its identity keys uploaded, seen in the last chat_device_active_days); a user without one, with a revoked or keyless
// device, with one not seen in 90 days, or unknown counts as having none. The answer holds the bit and nothing of the
// devices; more than 50 people is refused, and past 600 people an hour a user is rateLimited. check#DevicesReady is the same
// count for create#ChatRoom's default. A person an app's chat-target provider or an external asker's reach refuses is
// checked through Chat's transition (basic-comms BasicCommsEncrypted). No crypto runs: keys are opaque strings.
// Run: ./moq test framework --spec ChatEncryptionDefault
const A = "CHATDEF_SPEC_A"
const B = "CHATDEF_SPEC_B"
const C = "CHATDEF_SPEC_C"
const D = "CHATDEF_SPEC_D"
const LIMITED = "CHATDEF_SPEC_LIMITED"
const USERS = [A, B, C, D, LIMITED]
const SERVICES = "org.moqui.chat.ChatKeyServices."

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
const username = (userId) => userId.toLowerCase().replace(/_/g, ".")
function as(userId, name, parameters) {
    ec.user.internalLoginUser(username(userId))
    let out = {}
    let thrown = ""
    try {
        out = sys(() => ec.service.sync().name(SERVICES + name).parameters(parameters || {}).call()) || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
function makeUser(userId) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() === 0) {
            const account = ec.entity.makeValue("moqui.security.UserAccount")
            account.setFields({userId, username: username(userId), userFullName: "Default " + userId.slice(-1), disabled: "N"}, true, null, null)
            account.create()
        }
    })
}
/** A device of userId as a browser makes it: identity keys uploaded (opaque here); its deviceId. */
function device(userId, tag, withKeys) {
    const made = as(userId, "create#ChatDevice", {displayName: "Default " + tag})
    if (withKeys !== false) {
        const mx = sys(() => ec.service.sync().name(SERVICES + "map#ChatKeyUserIds").parameter("userIdList", [userId]).call()).matrixIds[userId]
        as(userId, "upload#Keys", {deviceId: made.deviceId, device_keys: {user_id: mx, device_id: made.deviceId, algorithms: ["m.olm.v1.curve25519-aes-sha2"],
            keys: {["curve25519:" + made.deviceId]: "curve-" + tag, ["ed25519:" + made.deviceId]: "ed-" + tag},
            signatures: {[mx]: {["ed25519:" + made.deviceId]: "sig-" + tag}}}})
    }
    return made.deviceId
}
const ask = (userId, people) => as(userId, "get#EncryptionDefault", {userIdList: people})
const ready = (userId, people) => as(userId, "check#DevicesReady", {userIdList: people})

spec({
    setupSpec() {
        for (const u of USERS) makeUser(u)
        sys(() => {
            for (const entity of ["moqui.chat.ChatToDevice", "moqui.chat.ChatDeviceKey", "moqui.chat.ChatDevice"]) {
                ec.entity.find(entity).condition("userId", "in", USERS).deleteAll()
            }
        })
        // A, B and LIMITED have a device in use; C only one without keys; D one revoked, then one not seen for 91 days
        for (const u of [A, B, LIMITED]) device(u, u.slice(-1).toLowerCase())
        device(C, "c", false)
    }
})

feature("the bit is true only when the asker and every person have a device in use, and holds nothing else", {
    when() {
        const both = ask(A, [B])
        const self = ask(A, [])
        const keyless = ask(A, [B, C])
        const unknown = ask(A, [B, "CHATDEF_SPEC_NOBODY"])
        const revokedId = device(D, "d1")
        sys(() => {
            const row = ec.entity.find("moqui.chat.ChatDevice").condition({userId: D, deviceId: revokedId}).one()
            row.set("revokedDate", ec.user.nowTimestamp)
            row.update()
        })
        const revoked = ask(A, [D])
        const staleId = device(D, "d2")
        const staleBefore = ask(A, [D])
        sys(() => {
            const row = ec.entity.find("moqui.chat.ChatDevice").condition({userId: D, deviceId: staleId}).one()
            row.set("lastSeenDate", new Date(Date.now() - 91 * 86400000))
            row.update()
        })
        const stale = ask(A, [D])
        const internal = ready(A, [B])
        const internalKeyless = ready(A, [C])
        const tooMany = ask(A, Array.from({length: 51}, (_, i) => "CHATDEF_SPEC_MANY_" + i))
        return {both, self, keyless, unknown, revoked, staleBefore, stale, internal, internalKeyless, tooMany}
    },
    then({both, self, keyless, unknown, revoked, staleBefore, stale, internal, internalKeyless, tooMany}) {
        both.errors == "" && both.encrypted == true
        Object.keys(both).filter((key) => key !== "errors" && key !== "reasonCode" && both[key] != null).join(",") == "encrypted"
        self.encrypted == true
        keyless.encrypted == false
        unknown.encrypted == false
        revoked.encrypted == false
        staleBefore.encrypted == true
        stale.encrypted == false
        internal.ready == true
        internalKeyless.ready == false
        tooMany.encrypted == false && tooMany.reasonCode == "tooMany"
    }
})

feature("past 600 people an hour a user, the ask is rateLimited and false", {
    when() {
        const people = Array.from({length: 50}, (_, i) => "CHATDEF_SPEC_RATE_" + i)
        const answers = []
        for (let i = 0; i < 12; i++) answers.push(ask(LIMITED, people).reasonCode || "")
        const past = ask(LIMITED, [A])
        const other = ask(A, [B])
        return {answers, past, other}
    },
    then({answers, past, other}) {
        answers.every((code) => code === "")
        past.encrypted == false && past.reasonCode == "rateLimited"
        other.encrypted == true
    }
})
