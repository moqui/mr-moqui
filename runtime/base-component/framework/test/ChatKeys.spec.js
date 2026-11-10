// Chat devices, keys and to-device messages (org.moqui.chat.ChatKeyServices and ChatKeys.js; framework/entity/ChatKeyEntities.xml;
// plans/Comms-Framework.md §4.5, C7 core; docs/interfaces/Chat-Keys.md): Matrix user IDs that map back; a device's keys
// uploaded with its identity keys fixed; another user refused with the very same parameters; key queries, claims and
// to-device sends reaching only users who share a room or a live call (and the user's own devices); a revoked device left out
// of everything; one-time keys never handed out twice by concurrent claims; the to-device queue's stream positions, cursor and
// acknowledgement, with a wake push that carries nothing of the message. No crypto runs here: the keys are opaque strings, as
// the server treats them (the browser's OlmMachine is checked by the client spec ChatKeysFlow).
// Run: ./moq test framework --spec ChatKeys
const A = "CHATKEY_SPEC_A"
const B = "CHATKEY_SPEC_B"
const C = "CHATKEY_SPEC_C"
const D = "CHATKEY_SPEC_D"
const SERVICES = "org.moqui.chat.ChatKeyServices."

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
function username(userId) {
    return userId.toLowerCase().replace(/_/g, ".")
}
function as(userId, name, parameters) {
    if (userId) ec.user.internalLoginUser(username(userId))
    let out = {}
    let thrown = ""
    try {
        out = sys(() => ec.service.sync().name(name.includes(".") ? name : SERVICES + name).parameters(parameters || {}).call()) || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
const PASSWORD = "Keys-spec-9!"
function makeUser(userId) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() === 0) {
            const account = ec.entity.makeValue("moqui.security.UserAccount")
            account.setFields({userId, username: username(userId), userFullName: "Keys " + userId.slice(-1), disabled: "N"}, true, null, null)
            account.create()
        }
        // a password, so a spec can sign in for real: a revoke of a device with keys needs a sign-in in the last five minutes
        const account = ec.entity.find("moqui.security.UserAccount").condition("userId", userId).one()
        account.setFields({currentPassword: ec.user.hashPassword(PASSWORD), passwordHashType: "argon2", passwordSalt: "", passwordBase64: "N",
            successiveFailedLogins: 0, requirePasswordChange: "N"}, true, null, null)
        account.update()
    })
}
/** as, signed in with the password just now (a recent sign-in, check#RecentAuth), not by an internal login. */
function asFresh(userId, name, parameters) {
    ec.user.loginUser(username(userId), PASSWORD)
    return as(null, name, parameters)
}
function waitFor(test, millis) {
    const deadline = Date.now() + millis
    while (Date.now() < deadline) {
        if (test()) return true
        const pause = Date.now() + 50
        while (Date.now() < pause) { /* look again shortly */ }
    }
    return test()
}
const mx = (userId) => as(null, "map#ChatKeyUserIds", {userIdList: [userId]}).matrixIds[userId]
/** The signed device keys a browser would upload (opaque strings here). */
function deviceKeys(userId, deviceId, tag) {
    return {user_id: mx(userId), device_id: deviceId, algorithms: ["m.olm.v1.curve25519-aes-sha2"],
        keys: {["curve25519:" + deviceId]: "curve-" + tag, ["ed25519:" + deviceId]: "ed-" + tag},
        signatures: {[mx(userId)]: {["ed25519:" + deviceId]: "sig-" + tag}}}
}
function otks(prefix, count) {
    const out = {}
    for (let i = 0; i < count; i++) out["signed_curve25519:" + prefix + i] = {key: "otk-" + prefix + i, signatures: {}}
    return out
}
/** A new device of userId with its keys and count one-time keys; its deviceId. */
function device(userId, count, tag) {
    const made = as(userId, "create#ChatDevice", {displayName: "Spec " + tag})
    as(userId, "upload#Keys", {deviceId: made.deviceId, device_keys: deviceKeys(userId, made.deviceId, tag), one_time_keys: otks(tag, count || 0),
        fallback_keys: {["signed_curve25519:fb" + tag]: {key: "fallback-" + tag, fallback: true, signatures: {}}}})
    return made.deviceId
}
const queried = (asUser, ofUser) => {
    const out = as(asUser, "query#Keys", {device_keys: {[mx(ofUser)]: []}})
    return Object.keys((out.device_keys || {})[mx(ofUser)] || {}).sort()
}
const toDeviceRows = (userId, deviceId) => sys(() => ec.entity.find("moqui.chat.ChatToDevice").condition({userId, deviceId}).count())
let room = null
const dev = {}

spec({
    setupSpec() {
        for (const u of [A, B, C, D]) makeUser(u)
        sys(() => {
            for (const entity of ["moqui.chat.ChatToDevice", "moqui.chat.ChatDeviceKey", "moqui.chat.ChatDevice"]) {
                ec.entity.find(entity).condition("userId", "in", [A, B, C, D]).deleteAll()
            }
            // C shares no room with anyone at the start, also after an earlier run that stopped half way
            for (const member of ec.entity.find("moqui.chat.ChatRoomMember").condition("userId", C).condition("leftDate", null).list()) {
                member.set("leftDate", ec.user.nowTimestamp)
                member.update()
            }
        })
        // A and B share a room; C shares nothing with anyone; D is only in a call with A (made in its feature)
        ec.user.internalLoginUser(username(A))
        room = sys(() => call("org.moqui.chat.ChatServices.create#Room", {roomTypeEnumId: "CrtPrivate", roomName: "Keys spec", memberUserIdList: [B]})).chatRoomId
    }
})

feature("Matrix user IDs map a userId and back; a device uploads its keys, which cannot change; one-time keys count", {
    when() {
        const ids = as(null, "map#ChatKeyUserIds", {userIdList: ["Ab_c.1", "EX_JOHN_DOE"], matrixIdList: ["@_ab__c.1:localhost", "@x:elsewhere", "@=zz:localhost"]})
        const made = as(A, "create#ChatDevice", {displayName: "A laptop", userAgent: "Spec/1"})
        dev.A1 = made.deviceId
        const first = as(A, "upload#Keys", {deviceId: dev.A1, device_keys: deviceKeys(A, dev.A1, "a1"), one_time_keys: otks("a1-", 5)})
        const again = as(A, "upload#Keys", {deviceId: dev.A1, device_keys: deviceKeys(A, dev.A1, "a1"), one_time_keys: otks("a1-", 5)})
        const changed = as(A, "upload#Keys", {deviceId: dev.A1, device_keys: deviceKeys(A, dev.A1, "other")})
        const wrongUser = as(A, "upload#Keys", {deviceId: dev.A1, device_keys: deviceKeys(B, dev.A1, "a1")})
        const sameIdOtherKey = as(A, "upload#Keys", {deviceId: dev.A1, one_time_keys: {"signed_curve25519:a1-0": {key: "different", signatures: {}}}})
        const mine = as(A, "find#ChatDevices", {})
        return {ids, made, first, again, changed, wrongUser, sameIdOtherKey, mine}
    },
    then({ids, made, first, again, changed, wrongUser, sameIdOtherKey, mine}) {
        ids.matrixIds["Ab_c.1"] == "@_ab__c.1:localhost"
        ids.matrixIds["EX_JOHN_DOE"] == "@_e_x___j_o_h_n___d_o_e:localhost"
        ids.userIds["@_ab__c.1:localhost"] == "Ab_c.1"
        ids.userIds["@x:elsewhere"] == null
        ids.userIds["@=zz:localhost"] == null
        made.errors == ""
        made.deviceId.length == 10
        made.user_id == mx(A)
        first.errors == ""
        first.one_time_key_counts.signed_curve25519 == 5
        again.errors == ""
        again.one_time_key_counts.signed_curve25519 == 5
        changed.reasonCode == "keysChanged"
        wrongUser.reasonCode == "badKeys"
        sameIdOtherKey.reasonCode == "keyExists"
        mine.deviceList.length == 1
        mine.deviceList[0].displayName == "A laptop"
        mine.deviceList[0].hasKeys == true
    }
})

feature("another user is refused a device's writes and its sync with the very same parameters its owner uses", {
    when() {
        const params = {deviceId: dev.A1, device_keys: deviceKeys(A, dev.A1, "a1")}
        const owner = as(A, "upload#Keys", params)
        const other = as(B, "upload#Keys", params)
        const otherSync = as(B, "sync#ToDevice", {deviceId: dev.A1, since: 0})
        const ownerSync = as(A, "sync#ToDevice", {deviceId: dev.A1, since: 0})
        const otherRename = as(B, "update#ChatDevice", {deviceId: dev.A1, displayName: "Taken"})
        const ownerRename = as(A, "update#ChatDevice", {deviceId: dev.A1, displayName: "A laptop"})
        const otherRevoke = as(B, "revoke#ChatDevice", {deviceId: dev.A1})
        const otherSend = as(B, "send#ToDevice", {deviceId: dev.A1, event_type: "m.room.encrypted", txn_id: "t-auth", messages: {}})
        const stillThere = as(A, "find#ChatDevices", {}).deviceList.length
        return {owner, other, otherSync, ownerSync, otherRename, ownerRename, otherRevoke, otherSend, stillThere}
    },
    then({owner, other, otherSync, ownerSync, otherRename, ownerRename, otherRevoke, otherSend, stillThere}) {
        owner.errors == ""
        other.reasonCode == "notFound"
        otherSync.reasonCode == "notFound"
        ownerSync.errors == ""
        otherRename.reasonCode == "notFound"
        ownerRename.errors == ""
        otherRevoke.reasonCode == "notFound"
        otherSend.reasonCode == "notFound"
        stillThere == 1
    }
})

feature("queries and claims reach only users who share a room or a live call, and the user's own devices", {
    when() {
        dev.B1 = device(B, 3, "b1")
        dev.C1 = device(C, 3, "c1")
        dev.D1 = device(D, 3, "d1")
        dev.A2 = device(A, 0, "a2")
        const aSeesB = queried(A, B)
        const aSeesC = queried(A, C)
        const cSeesA = queried(C, A)
        const aSeesOwn = queried(A, A)
        const claimC = as(A, "claim#Keys", {one_time_keys: {[mx(C)]: {[dev.C1]: "signed_curve25519"}}})
        const claimB = as(A, "claim#Keys", {one_time_keys: {[mx(B)]: {[dev.B1]: "signed_curve25519"}}})
        // a live call between A and D, then ended
        const aSeesDBefore = queried(A, D)
        const sessionId = sys(() => {
            const session = ec.entity.makeValue("moqui.call.CallSession")
            session.setSequencedIdPrimary()
            session.setFields({statusId: "CssActive", startedByUserId: A, startedDate: ec.user.nowTimestamp}, true, null, null)
            session.create()
            for (const u of [A, D]) {
                const p = ec.entity.makeValue("moqui.call.CallParticipant")
                p.setSequencedIdPrimary()
                p.setFields({callSessionId: session.callSessionId, userId: u, identityKindEnumId: "CikLocal", admissionStatusId: "CasAdmitted"}, true, null, null)
                p.create()
            }
            return session.callSessionId
        })
        const aSeesDInCall = queried(A, D)
        const dSeesAInCall = queried(D, A)
        sys(() => {
            const session = ec.entity.find("moqui.call.CallSession").condition("callSessionId", sessionId).one()
            session.set("statusId", "CssEnded")
            session.update()
        })
        const aSeesDAfter = queried(A, D)
        return {aSeesB, aSeesC, cSeesA, aSeesOwn, claimC, claimB, aSeesDBefore, aSeesDInCall, dSeesAInCall, aSeesDAfter}
    },
    then({aSeesB, aSeesC, cSeesA, aSeesOwn, claimC, claimB, aSeesDBefore, aSeesDInCall, dSeesAInCall, aSeesDAfter}) {
        aSeesB.join() == dev.B1
        aSeesC.length == 0
        cSeesA.length == 0
        aSeesOwn.join() == [dev.A1, dev.A2].sort().join()
        Object.keys(claimC.one_time_keys).length == 0
        Object.keys(claimB.one_time_keys[mx(B)][dev.B1])[0] == "signed_curve25519:b10"
        aSeesDBefore.length == 0
        aSeesDInCall.join() == dev.D1
        dSeesAInCall.join() == [dev.A1, dev.A2].sort().join()
        aSeesDAfter.length == 0
    }
})

feature("concurrent claims never hand out the same one-time key, and one user takes at most 10 a minute of a device: the fallback key answers past it", {
    when() {
        const target = device(B, 20, "conc")
        // 8 threads, each claiming 4 times in its own transactions: 32 claims by one user of 20 one-time keys, 10 a minute
        const results = parallel(8, (n, args) => {
            ec.user.internalLoginUser(args.user)
            const wasDisabled = ec.artifactExecution.disableAuthz()
            const got = []
            try {
                for (let i = 0; i < 4; i++) {
                    const out = ec.service.sync().name("org.moqui.chat.ChatKeyServices.claim#Keys").requireNewTransaction(true)
                        .parameters({one_time_keys: {[args.mxB]: {[args.device]: "signed_curve25519"}}}).call() || {}
                    const keys = ((out.one_time_keys || {})[args.mxB] || {})[args.device] || {}
                    got.push(Object.keys(keys)[0] || "")
                }
            } finally {
                if (!wasDisabled) ec.artifactExecution.enableAuthz()
            }
            return got
        }, {user: username(A), mxB: mx(B), device: target}, {transaction: false})
        const claimed = [].concat(...Array.from(results).map((list) => Array.from(list)))
        const oneTime = claimed.filter((id) => id.indexOf(":fb") < 0 && id)
        const fallback = claimed.filter((id) => id.indexOf(":fb") >= 0)
        // another claimer is not held by the first one's limit
        const other = Object.keys(((as(B, "claim#Keys", {one_time_keys: {[mx(B)]: {[target]: "signed_curve25519"}}}).one_time_keys || {})[mx(B)] || {})[target] || {})[0] || ""
        const left = as(B, "sync#ToDevice", {deviceId: target, since: 0})
        return {claimed, oneTime, fallback, other, left}
    },
    then({claimed, oneTime, fallback, other, left}) {
        claimed.length == 32
        oneTime.length == 10
        new Set(oneTime).size == 10
        fallback.length == 22
        other != "" && other.indexOf(":fb") < 0 && !oneTime.includes(other)
        left.device_one_time_keys_count.signed_curve25519 == 9
        left.device_unused_fallback_key_types.length == 0
    }
})

feature("to-device messages queue per device in order, a sync acknowledges up to its cursor, a repeated txn is not queued again, and the wake push carries no content", {
    when() {
        const capture = ec.factory.getTool("NotificationCapture")
        capture.enable()
        try {
            capture.clear()
            const send = (txn, body) => as(A, "send#ToDevice", {deviceId: dev.A1, event_type: "m.room.encrypted", txn_id: txn,
                messages: {[mx(B)]: {[dev.B1]: {algorithm: "m.olm.v1.curve25519-aes-sha2", ciphertext: {x: {type: 0, body}}}}}})
            const first = send("t1", "one")
            const repeat = send("t1", "one")
            send("t2", "two")
            send("t3", "three")
            // to C (no shared room or call): nothing is queued
            const toC = as(A, "send#ToDevice", {deviceId: dev.A1, event_type: "m.room.encrypted", txn_id: "t-c",
                messages: {[mx(C)]: {"*": {ciphertext: "x"}}}})
            const page1 = as(B, "sync#ToDevice", {deviceId: dev.B1, since: 0, limit: 2})
            const rowsAfter1 = toDeviceRows(B, dev.B1)
            const page2 = as(B, "sync#ToDevice", {deviceId: dev.B1, since: Number(page1.next_batch), limit: 2})
            const rowsAfter2 = toDeviceRows(B, dev.B1)
            const page3 = as(B, "sync#ToDevice", {deviceId: dev.B1, since: Number(page2.next_batch)})
            const rowsAfter3 = toDeviceRows(B, dev.B1)
            // a cursor past the last position queued answers the last one as next_batch, so a message queued later
            // still arrives on the next sync from it
            const ahead = as(B, "sync#ToDevice", {deviceId: dev.B1, since: Number(page3.next_batch) + 50})
            const late = send("t4", "late")
            const afterAhead = as(B, "sync#ToDevice", {deviceId: dev.B1, since: Number(ahead.next_batch)})
            const woken = waitFor(() => capture.getMessages("chatUser:" + B).length > 0, 5000)
            const pushes = capture.getMessages("chatUser:" + B).map((row) => typeof row.message === "string" ? row.message : JSON.stringify(row.message))
            return {first, repeat, toC, page1, rowsAfter1, page2, rowsAfter2, page3, rowsAfter3, ahead, late, afterAhead, woken, pushes}
        } finally {
            capture.disable()
            capture.clear()
        }
    },
    then({first, repeat, toC, page1, rowsAfter1, page2, rowsAfter2, page3, rowsAfter3, ahead, late, afterAhead, woken, pushes}) {
        first.queued == 1
        repeat.repeated == true
        toC.queued == 0
        page1.to_device.events.length == 2
        page1.has_more == true
        page1.to_device.events[0].content.ciphertext.x.body == "one"
        page1.to_device.events[1].content.ciphertext.x.body == "two"
        page1.to_device.events[0].sender == mx(A)
        page1.to_device.events[0].type == "m.room.encrypted"
        rowsAfter1 == 3
        page2.to_device.events.length == 1
        page2.to_device.events[0].content.ciphertext.x.body == "three"
        page2.has_more == false
        rowsAfter2 == 1
        page3.to_device.events.length == 0
        page3.next_batch == page2.next_batch
        rowsAfter3 == 0
        ahead.next_batch == page3.next_batch
        late.queued == 1
        afterAhead.to_device.events.length == 1
        afterAhead.to_device.events[0].content.ciphertext.x.body == "late"
        woken == true
        pushes.every((text) => text.indexOf("toDevice") >= 0 && text.indexOf("one") < 0 && text.indexOf("ciphertext") < 0)
    }
})

feature("a device list change is in the next sync of the users who share a room, and not of others", {
    when() {
        const before = Date.now() - 1000
        const added = device(B, 1, "b3")
        const aSync = as(A, "sync#ToDevice", {deviceId: dev.A1, since: 0, deviceListSince: before})
        const cSync = as(C, "sync#ToDevice", {deviceId: dev.C1, since: 0, deviceListSince: before})
        asFresh(B, "revoke#ChatDevice", {deviceId: added})
        return {aSync, cSync}
    },
    then({aSync, cSync}) {
        aSync.device_lists.changed.includes(mx(B))
        !cSync.device_lists.changed.includes(mx(B))
        aSync.device_lists_next > 0
    }
})

feature("a revoked device gets no new keys or messages, drops out of queries, and its own sync is refused", {
    when() {
        const doomed = device(B, 3, "b2")
        const seenBefore = queried(A, B)
        const sentBefore = as(A, "send#ToDevice", {deviceId: dev.A1, event_type: "m.room.encrypted", txn_id: "t-r1",
            messages: {[mx(B)]: {[doomed]: {ciphertext: "before"}}}})
        const revoked = asFresh(B, "revoke#ChatDevice", {deviceId: doomed})
        const again = asFresh(B, "revoke#ChatDevice", {deviceId: doomed})
        const queueAfter = toDeviceRows(B, doomed)
        const seenAfter = queried(A, B)
        const sentAfter = as(A, "send#ToDevice", {deviceId: dev.A1, event_type: "m.room.encrypted", txn_id: "t-r2",
            messages: {[mx(B)]: {[doomed]: {ciphertext: "after"}, "*": {ciphertext: "all"}}}})
        const claim = as(A, "claim#Keys", {one_time_keys: {[mx(B)]: {[doomed]: "signed_curve25519"}}})
        const sync = as(B, "sync#ToDevice", {deviceId: doomed, since: 0})
        const upload = as(B, "upload#Keys", {deviceId: doomed, one_time_keys: otks("late", 1)})
        const list = as(B, "find#ChatDevices", {}).deviceList.map((d) => d.deviceId)
        const listAll = as(B, "find#ChatDevices", {includeRevoked: true}).deviceList.map((d) => d.deviceId)
        return {doomed, seenBefore, sentBefore, revoked, again, queueAfter, seenAfter, sentAfter, claim, sync, upload, list, listAll}
    },
    then({doomed, seenBefore, sentBefore, revoked, again, queueAfter, seenAfter, sentAfter, claim, sync, upload, list, listAll}) {
        seenBefore.includes(doomed)
        sentBefore.queued == 1
        revoked.errors == ""
        again.repeated == true
        queueAfter == 0
        !seenAfter.includes(doomed)
        seenAfter.length > 0
        sentAfter.queued == seenAfter.length
        Object.keys(claim.one_time_keys).length == 0
        sync.reasonCode == "revoked"
        upload.reasonCode == "revoked"
        !list.includes(doomed)
        listAll.includes(doomed)
    }
})

feature("a member who left the room, and a forged sender, reach nothing: the sender is always the signed-in user", {
    when() {
        // C joins a room with A, so each reaches the other; then C leaves it
        ec.user.internalLoginUser(username(A))
        const shared = sys(() => call("org.moqui.chat.ChatServices.create#Room", {roomTypeEnumId: "CrtPrivate", roomName: "Keys spec left", memberUserIdList: [C]})).chatRoomId
        const aSeesCMember = queried(A, C)
        const toCMember = as(A, "send#ToDevice", {deviceId: dev.A1, event_type: "m.room.encrypted", txn_id: "t-left1",
            messages: {[mx(C)]: {[dev.C1]: {ciphertext: "member", sender: mx(B)}}}})
        const cSync = as(C, "sync#ToDevice", {deviceId: dev.C1, since: 0})
        sys(() => {
            const member = ec.entity.find("moqui.chat.ChatRoomMember").condition({chatRoomId: shared, userId: C}).one()
            member.set("leftDate", ec.user.nowTimestamp)
            member.update()
        })
        const aSeesCLeft = queried(A, C)
        const cSeesALeft = queried(C, A)
        const toCLeft = as(A, "send#ToDevice", {deviceId: dev.A1, event_type: "m.room.encrypted", txn_id: "t-left2",
            messages: {[mx(C)]: {[dev.C1]: {ciphertext: "left"}}}})
        const claimLeft = as(A, "claim#Keys", {one_time_keys: {[mx(C)]: {[dev.C1]: "signed_curve25519"}}})
        return {aSeesCMember, toCMember, cSync, aSeesCLeft, cSeesALeft, toCLeft, claimLeft}
    },
    then({aSeesCMember, toCMember, cSync, aSeesCLeft, cSeesALeft, toCLeft, claimLeft}) {
        aSeesCMember.join() == dev.C1
        toCMember.queued == 1
        cSync.to_device.events.length == 1
        // the sender is who signed in, whatever the content says
        cSync.to_device.events[0].sender == mx(A)
        aSeesCLeft.length == 0
        cSeesALeft.length == 0
        toCLeft.queued == 0
        Object.keys(claimLeft.one_time_keys).length == 0
    }
})

feature("retries of one to-device send at once queue one copy: the sender's row is locked before its transaction is looked up", {
    when() {
        const before = toDeviceRows(B, dev.B1)
        // 6 threads send the very same transaction at once, each in its own transaction
        const results = parallel(6, (n, args) => {
            ec.user.internalLoginUser(args.user)
            const wasDisabled = ec.artifactExecution.disableAuthz()
            try {
                const out = ec.service.sync().name("org.moqui.chat.ChatKeyServices.send#ToDevice").requireNewTransaction(true)
                    .parameters({deviceId: args.from, event_type: "m.room.encrypted", txn_id: "t-concurrent",
                        messages: {[args.mxB]: {[args.to]: {ciphertext: "once"}}}}).call() || {}
                return {queued: Number(out.queued || 0), repeated: out.repeated === true}
            } finally {
                if (!wasDisabled) ec.artifactExecution.enableAuthz()
            }
        }, {user: username(A), from: dev.A1, to: dev.B1, mxB: mx(B)}, {transaction: false})
        const answers = Array.from(results)
        return {added: toDeviceRows(B, dev.B1) - before, queued: answers.filter((a) => a.queued === 1).length,
            repeated: answers.filter((a) => a.repeated).length}
    },
    then({added, queued, repeated}) {
        added == 1
        queued == 1
        repeated == 5
    }
})

feature("a revoke of a device with keys needs a recent sign-in, refused with the same parameters until the user confirms who they are", {
    when() {
        const doomed = device(B, 1, "b4")
        const bare = as(B, "create#ChatDevice", {displayName: "No keys"}).deviceId
        // signed in by an internal login (no password, no code) after a logout: not a recent sign-in
        ec.user.logoutUser()
        const refused = as(B, "revoke#ChatDevice", {deviceId: doomed})
        const stillThere = as(B, "find#ChatDevices", {}).deviceList.map((d) => d.deviceId).includes(doomed)
        // a device without keys has nothing encrypted to it: no sign-in asked
        const bareRevoked = as(B, "revoke#ChatDevice", {deviceId: bare})
        const check = as(null, "org.moqui.sso.RecentAuthServices.check#RecentAuth", {})
        const wrong = as(null, "org.moqui.sso.RecentAuthServices.reauthenticate#User", {password: "not-it"})
        const confirmed = as(null, "org.moqui.sso.RecentAuthServices.reauthenticate#User", {password: PASSWORD})
        const checkAfter = as(null, "org.moqui.sso.RecentAuthServices.check#RecentAuth", {})
        const allowed = as(null, "revoke#ChatDevice", {deviceId: doomed})
        // a sign-in older than the age asked is not recent
        const pause = Date.now() + 1100
        while (Date.now() < pause) { /* let a second pass */ }
        const aged = as(null, "org.moqui.sso.RecentAuthServices.check#RecentAuth", {maxAgeSeconds: 1})
        // another user's internal login does not take B's recent sign-in
        const otherUser = as(A, "org.moqui.sso.RecentAuthServices.check#RecentAuth", {})
        return {refused, stillThere, bareRevoked, check, wrong, confirmed, checkAfter, allowed, aged, otherUser}
    },
    then({refused, stillThere, bareRevoked, check, wrong, confirmed, checkAfter, allowed, aged, otherUser}) {
        refused.reasonCode == "reauthRequired"
        stillThere
        bareRevoked.errors == ""
        check.recent == false
        check.method == "password"
        wrong.errors != ""
        wrong.reauthenticated == false
        confirmed.reauthenticated == true
        checkAfter.recent == true
        allowed.errors == ""
        aged.recent == false
        otherUser.recent == false
    }
})

feature("an account with a second factor confirms who it is with the password and then the code", {
    when() {
        // D gets single-use codes: a second factor
        const codes = sys(() => call("org.moqui.impl.UserServices.create#SingleUseAuthcCodes", {userId: D, numberOfCodes: 2})).singleUseCodes
        ec.user.logoutUser()
        ec.user.internalLoginUser(username(D))
        try {
            const check = as(null, "org.moqui.sso.RecentAuthServices.check#RecentAuth", {})
            const codeOnly = as(null, "org.moqui.sso.RecentAuthServices.reauthenticate#User", {code: codes[0]})
            const password = as(null, "org.moqui.sso.RecentAuthServices.reauthenticate#User", {password: PASSWORD})
            const wrongCode = as(null, "org.moqui.sso.RecentAuthServices.reauthenticate#User", {code: "00000000"})
            const afterWrong = as(null, "org.moqui.sso.RecentAuthServices.check#RecentAuth", {})
            // the password was checked a moment ago: the code may come alone
            const code = as(null, "org.moqui.sso.RecentAuthServices.reauthenticate#User", {code: codes[0]})
            const after = as(null, "org.moqui.sso.RecentAuthServices.check#RecentAuth", {})
            return {check, codeOnly, password, wrongCode, afterWrong, code, after}
        } finally {
            sys(() => {
                for (const factor of ec.entity.find("moqui.security.UserAuthcFactor").condition("userId", D).list()) {
                    factor.set("thruDate", ec.user.nowTimestamp)
                    factor.update()
                }
                const account = ec.entity.find("moqui.security.UserAccount").condition("userId", D).one()
                account.set("successiveFailedLogins", 0)
                account.update()
            })
        }
    },
    then({check, codeOnly, password, wrongCode, afterWrong, code, after}) {
        check.secondFactor == true
        check.recent == false
        codeOnly.errors != ""
        codeOnly.reauthenticated == false
        password.codeNeeded == true
        password.reauthenticated == false
        wrongCode.codeNeeded == true
        wrongCode.codeText.indexOf("not valid") >= 0
        afterWrong.recent == false
        code.reauthenticated == true
        after.recent == true
    }
})
