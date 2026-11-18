// Scheduled sends and message reminders (plans/Comms-Messaging.md §18.3 phase 15; ChatSchedule.js,
// ChatScheduleServices): a message the server holds and sends at its time as its sender, through send#Message, with the
// rules of the send time (a member, not archived, not blocked; a bridge's delivery); an encrypted room's, held by the
// sender's own device, which claims and reports it; reminders that keep no text and come back as an Activity row and a
// push. The jobs are called directly, on a clock pinned with ec.user.setEffectiveTime.
// Run: ./moq test framework --spec ChatSchedule
const A = "CHAT_SCHED_A"
const B = "CHAT_SCHED_B"
const C = "CHAT_SCHED_C"
// D has no grant of the chat services: a job sends as them with the chat rules, not a screen's grants
const D = "CHAT_SCHED_D"
const USERS = [A, B, C]
const CHAT = "org.moqui.chat.ChatServices."
const SCHED = "org.moqui.chat.ChatScheduleServices."
const GROUP = "CHAT_SCHED_SPEC"
const MINUTE = 60 * 1000
const T0 = Date.now()

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
const username = (userId) => userId.toLowerCase().replace(/_/g, ".")
// the clock of the calls: the spec's start, moved on by `later`
let clock = T0
function as(userId, name, parameters) {
    if (userId) ec.user.internalLoginUser(username(userId))
    ec.user.setEffectiveTime(clock)
    let out = {}
    let thrown = ""
    try {
        out = ec.service.sync().name(name).parameters(parameters || {}).call() || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
const job = (name, parameters) => as(null, SCHED + name, parameters)
const make = (entity, fields) => sys(() => ec.entity.makeValue(entity).setFields(fields, true, null, null).createOrUpdate())
const row = (entity, fields) => sys(() => ec.entity.find(entity).condition(fields).one())
const scheduled = (id) => row("moqui.chat.ChatScheduledMessage", {chatScheduledMessageId: id})
const messages = (userId, chatRoomId) => Array.from(as(userId, CHAT + "get#Timeline", {chatRoomId}).eventList || []).filter((event) => event.type === "message")
const room = (userId, fields) => as(userId, CHAT + "create#Room", Object.assign({roomTypeEnumId: "CrtPrivate"}, fields)).chatRoomId
const MEGOLM = {algorithm: "m.megolm.v1.aes-sha2", ciphertext: "c2VhbGVk", session_id: "spec-session", sender_key: "k", device_id: "DEVA1"}
const notices = (userId, topic) => sys(() => ec.entity.find("moqui.security.user.NotificationMessageUser").condition("userId", userId).list())
    .filter((user) => (row("moqui.security.user.NotificationMessage", {notificationMessageId: user.notificationMessageId}) || {}).topic === topic).length
const shared = {}

function makeUsers() {
    sys(() => {
        for (const userId of USERS.concat([D])) {
            if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() === 0) {
                ec.entity.makeValue("moqui.security.UserAccount").setFields({userId, username: username(userId), userFullName: "Sched " + userId.slice(11),
                    disabled: "N"}, true, null, null).create()
            }
        }
    })
    make("moqui.security.UserGroup", {userGroupId: GROUP, description: "Chat schedule spec"})
    make("moqui.security.ArtifactGroup", {artifactGroupId: GROUP, description: "Chat schedule spec"})
    make("moqui.security.ArtifactGroupMember", {artifactGroupId: GROUP, artifactName: "org\\.moqui\\.chat\\..*", artifactTypeEnumId: "AT_SERVICE",
        nameIsPattern: "Y", inheritAuthz: "Y"})
    make("moqui.security.ArtifactAuthz", {artifactAuthzId: GROUP, userGroupId: GROUP, artifactGroupId: GROUP, authzTypeEnumId: "AUTHZT_ALWAYS",
        authzActionEnumId: "AUTHZA_ALL"})
    for (const userId of USERS) make("moqui.security.UserGroupMember", {userGroupId: GROUP, userId, fromDate: new Date(T0 - 3600000)})
    // A's two encryption devices, and a push device: an app's distributor whose device key is good, at an endpoint that
    // takes nothing (the worker's sends fail at once; the rows stay)
    for (const deviceId of ["DEVA1", "DEVA2"]) make("moqui.chat.ChatDevice", {userId: A, deviceId, displayName: "Browser " + deviceId, createdDate: new Date(T0)})
    make("moqui.security.UserLoginKey", {loginKey: "CHAT_SCHED_DEVICE_KEY", userId: A, fromDate: new Date(T0 - 3600000)})
    make("moqui.security.user.WebPushSubscription", {subscriptionId: "CHAT_SCHED_PUSH", userId: A, pushProviderEnumId: "PushUnified",
        endpoint: "http://127.0.0.1:9/chat-sched", endpointHash: "chat-sched-spec", loginKey: "CHAT_SCHED_DEVICE_KEY", p256dhKey: "x", authSecret: "y"})
}
function cleanUp() {
    sys(() => {
        ec.entity.find("moqui.chat.ChatScheduledMessage").condition("senderUserId", "in", USERS.concat([D])).deleteAll()
        ec.entity.find("moqui.chat.ChatMessageReminder").condition("userId", "in", USERS).deleteAll()
        ec.entity.find("moqui.chat.PushOutbox").condition("userId", "in", USERS).deleteAll()
        ec.entity.find("moqui.chat.ChatUserBlock").condition("userId", "in", USERS).deleteAll()
    })
}

spec({
    stepwise: true,
    setupSpec() {
        makeUsers()
        cleanUp()
        shared.team = room(A, {roomName: "Sched team", memberUserIdList: [B]})
        shared.sealed = room(A, {roomName: "Sched sealed", memberUserIdList: [B], encrypted: true})
        as(B, CHAT + "send#Message", {chatRoomId: shared.team, body: "<p>first</p>"})
        shared.first = messages(A, shared.team)[0].eventSeq
    },
    cleanupSpec() {
        cleanUp()
        ec.user.setEffectiveTime(null)
        ec.user.logoutUser()
    }
})

feature("schedule#Message refuses what send#Message would, a time out of range, a body in an encrypted room and too many", {
    when() {
        const at = T0 + 10 * MINUTE
        const stranger = as(C, SCHED + "schedule#Message", {chatRoomId: shared.team, body: "<p>x</p>", sendDate: at})
        const past = as(A, SCHED + "schedule#Message", {chatRoomId: shared.team, body: "<p>x</p>", sendDate: T0 - MINUTE})
        const far = as(A, SCHED + "schedule#Message", {chatRoomId: shared.team, body: "<p>x</p>", sendDate: T0 + 400 * 86400000})
        const empty = as(A, SCHED + "schedule#Message", {chatRoomId: shared.team, body: "<p> </p>", sendDate: at})
        const sealedBody = as(A, SCHED + "schedule#Message", {chatRoomId: shared.sealed, body: "<p>plain</p>", sendDate: at, heldByDeviceId: "DEVA1"})
        const noDevice = as(A, SCHED + "schedule#Message", {chatRoomId: shared.sealed, sendDate: at, heldByDeviceId: "NO_SUCH_DEVICE"})
        const someoneElses = as(B, SCHED + "schedule#Message", {chatRoomId: shared.sealed, sendDate: at, heldByDeviceId: "DEVA1"})
        const archivedRoom = room(A, {roomName: "Sched archived"})
        sys(() => { const r = ec.entity.find("moqui.chat.ChatRoom").condition("chatRoomId", archivedRoom).one(); r.set("archivedDate", new Date(T0)); r.update() })
        const archived = as(A, SCHED + "schedule#Message", {chatRoomId: archivedRoom, body: "<p>x</p>", sendDate: at})
        // a hundred waiting: the next is refused
        sys(() => { for (let i = 0; i < 100; i++) ec.entity.makeValue("moqui.chat.ChatScheduledMessage").setFields({chatScheduledMessageId: "CSM_SPEC_" + i,
            chatRoomId: shared.team, senderUserId: A, statusEnumId: "CsmScheduled", sendDate: new Date(T0 + 86400000)}, true, null, null).create() })
        const tooMany = as(A, SCHED + "schedule#Message", {chatRoomId: shared.team, body: "<p>x</p>", sendDate: at})
        sys(() => ec.entity.find("moqui.chat.ChatScheduledMessage").condition("chatScheduledMessageId", "like", "CSM_SPEC_%").deleteAll())
        return {stranger, past, far, empty, sealedBody, noDevice, someoneElses, archived, tooMany}
    },
    then({stranger, past, far, empty, sealedBody, noDevice, someoneElses, archived, tooMany}) {
        stranger.reasonCode == "notFound"
        past.reasonCode == "badTime"
        far.reasonCode == "badTime"
        empty.reasonCode == "empty"
        sealedBody.reasonCode == "encryptedRoom"
        noDevice.reasonCode == "noDevice"
        someoneElses.reasonCode == "noDevice"
        archived.reasonCode == "archived"
        tooMany.reasonCode == "tooMany"
    }
})

feature("a message the server holds goes at its time as its sender, with its file and reply, once; cleanup leaves its file", {
    when() {
        const file = "CHAT_SCHED_FILE"
        make("moqui.chat.ChatAttachment", {chatAttachmentId: file, chatRoomId: shared.team, uploadedByUserId: A, fileName: "plan.txt", contentType: "text/plain",
            location: "dbresource://chat/spec/plan.txt", uploadedDate: new Date(T0 - 2 * 86400000)})
        const made = as(A, SCHED + "schedule#Message", {chatRoomId: shared.team, body: "<p>see you at 9</p>", replyToEventSeq: shared.first,
            chatAttachmentIdList: [file], sendDate: T0 + 2 * MINUTE})
        shared.id = made.chatScheduledMessageId
        // the daily cleanup of unsent files leaves a scheduled message's
        as(null, CHAT + "clean#PendingAttachments", {})
        const fileKept = row("moqui.chat.ChatAttachment", {chatAttachmentId: file}) != null
        // one of D's, as the screen made it
        as(A, CHAT + "add#Member", {chatRoomId: shared.team, userIdList: [D]})
        make("moqui.chat.ChatScheduledMessage", {chatScheduledMessageId: "CSM_SPEC_D", chatRoomId: shared.team, senderUserId: D, statusEnumId: "CsmScheduled",
            sendDate: new Date(T0 + 2 * MINUTE + 1000), body: "<p>from D</p>", bodyText: "from D", attemptCount: 0})
        const early = job("send#DueMessages")
        const before = messages(B, shared.team).length
        clock = T0 + 3 * MINUTE
        const due = job("send#DueMessages")
        const sent = messages(A, shared.team).filter((event) => event.senderUserId !== D)
        const fromD = messages(B, shared.team).filter((event) => event.senderUserId === D)
        const again = job("send#DueMessages")
        const after = scheduled(shared.id)
        return {made, fileKept, early, before, due, sent, fromD, again, after}
    },
    then({made, fileKept, early, before, due, sent, fromD, again, after}) {
        fromD.length == 1
        fromD[0].body.includes("from D")
        made.errors == ""
        made.scheduled.status == "scheduled"
        made.scheduled.files.length == 1
        fileKept
        early.sentCount == 0
        before == 1
        due.sentCount == 2
        sent.length == 2
        sent[1].senderUserId == A
        sent[1].body.includes("see you at 9")
        sent[1].replyToEventSeq == shared.first
        sent[1].attachments.length == 1
        sent[1].clientRef == "sched:" + shared.id
        again.sentCount == 0
        after.statusEnumId == "CsmSent"
        after.sentEventSeq == sent[1].eventSeq
    }
})

feature("edit and time before it goes; refused once sent; cancel removes its file; Send now", {
    when() {
        const one = as(A, SCHED + "schedule#Message", {chatRoomId: shared.team, body: "<p>draft</p>", sendDate: clock + 30 * MINUTE}).chatScheduledMessageId
        const edited = as(A, SCHED + "update#ScheduledMessage", {chatScheduledMessageId: one, body: "<p>edited</p>", sendDate: clock + 60 * MINUTE})
        const notMine = as(B, SCHED + "update#ScheduledMessage", {chatScheduledMessageId: one, body: "<p>mine</p>"})
        const now = as(A, SCHED + "send#ScheduledMessageNow", {chatScheduledMessageId: one})
        const last = messages(B, shared.team).slice(-1)[0]
        const late = as(A, SCHED + "update#ScheduledMessage", {chatScheduledMessageId: one, body: "<p>again</p>"})
        make("moqui.chat.ChatAttachment", {chatAttachmentId: "CHAT_SCHED_FILE2", chatRoomId: shared.team, uploadedByUserId: A, fileName: "b.txt",
            location: "dbresource://chat/spec/b.txt", uploadedDate: new Date(clock)})
        const two = as(A, SCHED + "schedule#Message", {chatRoomId: shared.team, chatAttachmentIdList: ["CHAT_SCHED_FILE2"], sendDate: clock + 30 * MINUTE})
        const cancelled = as(A, SCHED + "cancel#ScheduledMessage", {chatScheduledMessageId: two.chatScheduledMessageId})
        return {edited, notMine, now, last, late, cancelled, status: scheduled(two.chatScheduledMessageId).statusEnumId,
            file: row("moqui.chat.ChatAttachment", {chatAttachmentId: "CHAT_SCHED_FILE2"})}
    },
    then({edited, notMine, now, last, late, cancelled, status, file}) {
        edited.errors == ""
        edited.scheduled.bodyText == "edited"
        notMine.reasonCode == "notFound"
        now.errors == ""
        last.body.includes("edited")
        now.eventSeq == last.eventSeq
        late.reasonCode == "notScheduled"
        cancelled.errors == ""
        status == "CsmCancelled"
        file == null
    }
})

feature("at its time: a sender no longer in the room, an archived room or a conversation the sender blocked fail and tell them; a block of the sender does not stop it", {
    when() {
        const at = clock + 2 * MINUTE
        const left = room(A, {roomName: "Sched left", memberUserIdList: [B]})
        const archived = room(A, {roomName: "Sched archived later", memberUserIdList: [B]})
        const blocked = room(A, {roomName: "Sched blocked", memberUserIdList: [B]})
        const direct = as(A, CHAT + "create#Room", {roomTypeEnumId: "CrtDirect", memberUserIdList: [C], encrypted: false}).chatRoomId
        const ids = [left, archived, blocked, direct].map((chatRoomId) => as(A, SCHED + "schedule#Message", {chatRoomId, body: "<p>later</p>", sendDate: at}).chatScheduledMessageId)
        as(A, CHAT + "remove#Member", {chatRoomId: left, userId: A})
        sys(() => { const r = ec.entity.find("moqui.chat.ChatRoom").condition("chatRoomId", archived).one(); r.set("archivedDate", new Date(clock)); r.update() })
        sys(() => { const m = ec.entity.find("moqui.chat.ChatRoomMember").condition({chatRoomId: blocked, userId: A}).one(); m.set("filterEnumId", "CmfBlocked"); m.update() })
        as(C, "org.moqui.chat.ChatBlockServices.block#ChatUser", {blockedUserId: A})
        const noticesBefore = notices(A, "ChatScheduleFailed")
        clock = at + MINUTE
        const run = job("send#DueMessages")
        const rows = ids.map((id) => scheduled(id))
        const theirs = messages(C, direct)
        // the blocker's direct room with the sender is out of their sight (Spam & blocked), as for any message
        const filter = row("moqui.chat.ChatRoomMember", {chatRoomId: direct, userId: C}).filterEnumId
        return {run, rows, theirs, filter, told: notices(A, "ChatScheduleFailed") - noticesBefore}
    },
    then({run, rows, theirs, filter, told}) {
        run.sentCount == 1
        run.failedCount == 3
        rows[0].statusEnumId == "CsmFailed"
        rows[0].reasonCode == "notMember"
        rows[1].reasonCode == "archived"
        rows[2].reasonCode == "blocked"
        rows[3].statusEnumId == "CsmSent"
        theirs.length == 1
        filter == "CmfBlocked"
        told == 3
    }
})

feature("in a bridged room it goes out through the bridge's delivery at its time", {
    when() {
        const bridged = room(A, {roomName: "Sched bridged"})
        make("moqui.chat.ChatBridge", {chatBridgeId: "CHAT_SCHED_BRIDGE", name: "Spec bridge", adapterName: "none"})
        make("moqui.chat.ChatRoomBridge", {chatRoomBridgeId: "CHAT_SCHED_MAP", chatRoomId: bridged, chatBridgeId: "CHAT_SCHED_BRIDGE",
            externalRoomId: "+15550100", directionEnumId: "CrbOut", fromDate: new Date(T0)})
        const id = as(A, SCHED + "schedule#Message", {chatRoomId: bridged, body: "<p>text later</p>", sendDate: clock + 2 * MINUTE}).chatScheduledMessageId
        const before = sys(() => ec.entity.find("moqui.chat.ChatBridgeDelivery").condition("chatRoomId", bridged).count())
        clock += 3 * MINUTE
        job("send#DueMessages")
        const sent = scheduled(id)
        const after = sys(() => ec.entity.find("moqui.chat.ChatBridgeDelivery").condition({chatRoomId: bridged, eventSeq: sent.sentEventSeq}).count())
        sys(() => {
            ec.entity.find("moqui.chat.ChatBridgeDelivery").condition("chatRoomId", bridged).deleteAll()
            ec.entity.find("moqui.chat.ChatRoomBridge").condition("chatRoomBridgeId", "CHAT_SCHED_MAP").deleteAll()
        })
        return {before, after, status: sent.statusEnumId}
    },
    then({before, after, status}) {
        before == 0
        status == "CsmSent"
        after == 1
    }
})

feature("an encrypted room's message is held by its device: the job never sends it, only that device claims it, once, and reports it", {
    when() {
        const at = clock + 2 * MINUTE
        const made = as(A, SCHED + "schedule#Message", {chatRoomId: shared.sealed, sendDate: at, heldByDeviceId: "DEVA1"})
        const id = made.chatScheduledMessageId
        const early = as(A, SCHED + "claim#HeldMessage", {chatScheduledMessageId: id, deviceId: "DEVA1"})
        clock = at + 10 * MINUTE
        const run = job("send#DueMessages")
        const other = as(A, SCHED + "claim#HeldMessage", {chatScheduledMessageId: id, deviceId: "DEVA2"})
        const claim = as(A, SCHED + "claim#HeldMessage", {chatScheduledMessageId: id, deviceId: "DEVA1"})
        const twice = as(A, SCHED + "claim#HeldMessage", {chatScheduledMessageId: id, deviceId: "DEVA1"})
        const sent = as(A, CHAT + "send#Message", {chatRoomId: shared.sealed, encrypted: MEGOLM, clientRef: claim.clientRef})
        const wrong = as(A, SCHED + "complete#HeldMessage", {chatScheduledMessageId: id, deviceId: "DEVA1", eventSeq: sent.eventSeq + 5})
        const done = as(A, SCHED + "complete#HeldMessage", {chatScheduledMessageId: id, deviceId: "DEVA1", eventSeq: sent.eventSeq})
        const listed = Array.from(as(A, SCHED + "get#ChatSchedule").scheduledList || []).find((item) => item.chatScheduledMessageId === id)
        as(A, SCHED + "dismiss#ScheduledMessage", {chatScheduledMessageId: id})
        const dismissed = Array.from(as(A, SCHED + "get#ChatSchedule").scheduledList || []).some((item) => item.chatScheduledMessageId === id)
        // a report lost: the next claim finds the message and marks it sent
        const lost = as(A, SCHED + "schedule#Message", {chatRoomId: shared.sealed, sendDate: clock + 2 * MINUTE, heldByDeviceId: "DEVA1"}).chatScheduledMessageId
        clock += 3 * MINUTE
        const lostClaim = as(A, SCHED + "claim#HeldMessage", {chatScheduledMessageId: lost, deviceId: "DEVA1"})
        as(A, CHAT + "send#Message", {chatRoomId: shared.sealed, encrypted: MEGOLM, clientRef: lostClaim.clientRef})
        clock += 3 * MINUTE
        const reclaim = as(A, SCHED + "claim#HeldMessage", {chatScheduledMessageId: lost, deviceId: "DEVA1"})
        // its device revoked: the job fails it; a held one is cancelled from another device
        const gone = as(A, SCHED + "schedule#Message", {chatRoomId: shared.sealed, sendDate: clock + 2 * MINUTE, heldByDeviceId: "DEVA2"}).chatScheduledMessageId
        const elsewhere = as(A, SCHED + "schedule#Message", {chatRoomId: shared.sealed, sendDate: clock + 2 * MINUTE, heldByDeviceId: "DEVA1"}).chatScheduledMessageId
        const cancelled = as(A, SCHED + "cancel#ScheduledMessage", {chatScheduledMessageId: elsewhere})
        sys(() => { const d = ec.entity.find("moqui.chat.ChatDevice").condition({userId: A, deviceId: "DEVA2"}).one(); d.set("revokedDate", new Date(clock)); d.update() })
        const goneListed = Array.from(as(A, SCHED + "get#ChatSchedule").scheduledList || []).find((item) => item.chatScheduledMessageId === gone)
        clock += 3 * MINUTE
        job("send#DueMessages")
        return {made, early, run, other, claim, twice, wrong, done, listed, dismissed, reclaim, cancelled, goneListed, row: scheduled(id),
            lostRow: scheduled(lost), goneRow: scheduled(gone)}
    },
    then({made, early, run, other, claim, twice, wrong, done, listed, dismissed, reclaim, cancelled, goneListed, row, lostRow, goneRow}) {
        made.errors == ""
        made.scheduled.held
        made.scheduled.body == null
        made.scheduled.heldByDeviceName == "Browser DEVA1"
        early.claimed == false
        run.sentCount == 0
        other.reasonCode == "notHolder"
        claim.claimed == true
        twice.claimed == false
        wrong.reasonCode == "notFound"
        done.sent == true
        done.sentLate == true
        row.statusEnumId == "CsmSent"
        row.body == null
        listed.sentLate == true
        dismissed == false
        reclaim.sent == true
        lostRow.statusEnumId == "CsmSent"
        cancelled.errors == ""
        goneListed.deviceGone == true
        goneRow.statusEnumId == "CsmFailed"
        goneRow.reasonCode == "deviceGone"
    }
})

feature("reminders: refused for a room one cannot read, a past time or too many; one a message; listed without an encrypted room's text", {
    when() {
        const sealedSeq = as(B, CHAT + "send#Message", {chatRoomId: shared.sealed, encrypted: Object.assign({}, MEGOLM, {device_id: "DEVB"})}).eventSeq
        const stranger = as(C, SCHED + "set#Reminder", {chatRoomId: shared.team, eventSeq: shared.first, remindDate: clock + 20 * MINUTE})
        const past = as(A, SCHED + "set#Reminder", {chatRoomId: shared.team, eventSeq: shared.first, remindDate: clock - MINUTE})
        const plain = as(A, SCHED + "set#Reminder", {chatRoomId: shared.team, eventSeq: shared.first, remindDate: clock + 60 * MINUTE})
        const replaced = as(A, SCHED + "set#Reminder", {chatRoomId: shared.team, eventSeq: shared.first, remindDate: clock + 20 * MINUTE})
        const sealed = as(A, SCHED + "set#Reminder", {chatRoomId: shared.sealed, eventSeq: sealedSeq, remindDate: clock + 20 * MINUTE})
        shared.sealedSeq = sealedSeq
        const list = Array.from(as(A, SCHED + "get#ChatSchedule").reminderList || [])
        const reminderRows = sys(() => ec.entity.find("moqui.chat.ChatMessageReminder").condition({userId: A, statusEnumId: "CrmPending"}).count())
        sys(() => { for (let i = 0; i < 198; i++) ec.entity.makeValue("moqui.chat.ChatMessageReminder").setFields({chatMessageReminderId: "CMR_SPEC_" + i, userId: A,
            chatRoomId: shared.team, eventSeq: 999, statusEnumId: "CrmPending", remindDate: new Date(clock + 86400000)}, true, null, null).create() })
        const tooMany = as(A, SCHED + "set#Reminder", {chatRoomId: shared.sealed, eventSeq: shared.first, remindDate: clock + 20 * MINUTE})
        sys(() => ec.entity.find("moqui.chat.ChatMessageReminder").condition("chatMessageReminderId", "like", "CMR_SPEC_%").deleteAll())
        return {stranger, past, plain, replaced, sealed, list, reminderRows, tooMany}
    },
    then({stranger, past, plain, replaced, sealed, list, reminderRows, tooMany}) {
        stranger.reasonCode == "notFound"
        past.reasonCode == "badTime"
        plain.errors == ""
        replaced.chatMessageReminderId == plain.chatMessageReminderId
        sealed.errors == ""
        reminderRows == 2
        list.length == 2
        list[0].snippet == "first"
        list[0].senderName == "Sched B"
        list[1].encrypted == true
        list[1].snippet == null
        list[1].encryptedContent.ciphertext == MEGOLM.ciphertext
        tooMany.reasonCode == "tooMany"
    }
})

feature("a reminder at its time: an Activity row, a ChatReminder notification and a push to each device, the text only of a plain room; a message gone drops it; the daily cleanup", {
    when() {
        const outbox = () => sys(() => Array.from(ec.entity.find("moqui.chat.PushOutbox").condition({userId: A, kindEnumId: "PokReminder"}).orderBy("pushOutboxId").list()))
        const noticesBefore = notices(A, "ChatReminder")
        const early = job("send#DueReminders")
        clock += 21 * MINUTE
        const due = job("send#DueReminders")
        const again = job("send#DueReminders")
        const pushes = outbox().map((push) => JSON.parse(push.payloadJson))
        const activity = Array.from(as(A, CHAT + "find#ChatActivity", {kinds: ["reminder"]}).items || [])
        // the message removed before its reminder, and a room left
        const seq = as(B, CHAT + "send#Message", {chatRoomId: shared.team, body: "<p>soon gone</p>"}).eventSeq
        const goneId = as(A, SCHED + "set#Reminder", {chatRoomId: shared.team, eventSeq: seq, remindDate: clock + 2 * MINUTE}).chatMessageReminderId
        as(B, CHAT + "redact#Message", {chatRoomId: shared.team, eventSeq: seq})
        const leftRoom = room(B, {roomName: "Sched leave", memberUserIdList: [A]})
        const leftSeq = as(B, CHAT + "send#Message", {chatRoomId: leftRoom, body: "<p>bye</p>"}).eventSeq
        const leftId = as(A, SCHED + "set#Reminder", {chatRoomId: leftRoom, eventSeq: leftSeq, remindDate: clock + 2 * MINUTE}).chatMessageReminderId
        as(A, CHAT + "remove#Member", {chatRoomId: leftRoom, userId: A})
        clock += 3 * MINUTE
        const dropped = job("send#DueReminders")
        const gone = row("moqui.chat.ChatMessageReminder", {chatMessageReminderId: goneId})
        const left = row("moqui.chat.ChatMessageReminder", {chatMessageReminderId: leftId})
        // a week on, the daily cleanup takes what is done; what waits stays
        const waiting = as(A, SCHED + "schedule#Message", {chatRoomId: shared.team, body: "<p>next month</p>", sendDate: clock + 30 * 86400000}).chatScheduledMessageId
        clock += 8 * 86400000
        const cleaned = job("clean#ScheduledMessages")
        return {cleaned, kept: scheduled(waiting) != null, early, due, again, pushes, activity, dropped, told: notices(A, "ChatReminder") - noticesBefore, gone, left}
    },
    then({cleaned, kept, early, due, again, pushes, activity, dropped, told, gone, left}) {
        cleaned.removedCount > 5
        kept
        early.firedCount == 0
        due.firedCount == 2
        again.firedCount == 0
        told == 2
        pushes.length == 2
        pushes[0].kind == "notification"
        pushes[0].topic == "ChatReminder"
        pushes.some((push) => push.chatRoomId === shared.team && push.body === "first" && push.title === "Reminder: Sched B in Sched team")
        pushes.some((push) => push.chatRoomId === shared.sealed && push.encrypted && push.body === "A message you asked to be reminded of")
        pushes.every((push) => push.tag.startsWith("chat-reminder-") && push.link.includes("eventSeq="))
        activity.length == 2
        activity.every((item) => item.kind === "reminder")
        dropped.droppedCount == 2
        gone.reasonCode == "messageGone"
        left.reasonCode == "noAccess"
    }
})
