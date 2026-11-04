// What a client that keeps data offline relies on (plans/Mobile-Desktop.md MD5): the mail change cursor
// (EmailClientServices.get#MailChanges, the Inbox's mailChanges), requests made once by their clientRef
// (ClientSyncServices, the Inbox's state and Chat's edit and react), conflicts (save#Draft's baseStamp, a state target's
// seen) and files sent in chunks (uploadChunk, uploadStatus, then upload by uploadId), with the jobs that clean them up.
// Run: ./moq test basic-comms --spec BasicCommsSync
const root = "component://basic-comms/screen/BasicComms.xml"
const OWNER = "basic.comms.sync"
const OTHER = "basic.comms.sync.other"
const MESSAGE = "moqui.basic.email.EmailMessage"
const MAIL = "org.moqui.impl.EmailClientServices."
const SYNC = "org.moqui.impl.ClientSyncServices."
// with IronStone installed its inbox source answers the Inbox, and has no change cursor of its own yet
const bossInstalled = !!ec.resource.getLocationReference("component://mr-boss/screen/Comms.xml")?.getExists()
let accountId = null
let identity = null
let inboxId = null
let cursor = 0
const ids = {}

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
// as the screens call them, authorized by the app's screen; each service checks the user itself
function attempt(name, parameters) {
    let out = {}
    let thrown = ""
    try {
        out = sys(() => call(name, parameters || {})) || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
function request(path, parameters, method) {
    const str = ec.screen
        .makeTest()
        .rootScreen(root)
        .render(path, parameters || {}, method || null)
    const output = String(str.output || "")
    ec.message.clearErrors()
    let json = null
    try {
        json = JSON.parse(output)
    } catch (err) {
        json = null
    }
    return {status: str.status, errors: str.errorMessages.join(" | "), json: json || {}}
}
const op = (operation, parameters) =>
    attempt("org.moqui.comms.InboxServices.run#Operation", {operation, parameters: parameters || {}}).result || {}
const changes = (afterSeq, limit) => attempt(MAIL + "get#MailChanges", {afterSeq, limit: limit || 200})
const message = emailMessageId => sys(() => ec.entity.find(MESSAGE).condition("emailMessageId", emailMessageId).one())
const setFields = (emailMessageId, fields) =>
    sys(() => {
        const row = ec.entity.find(MESSAGE).condition("emailMessageId", emailMessageId).one()
        row.setFields(fields, true, null, null)
        row.update()
    })
function makeUser(username) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("username", username).count() > 0) return
        const user = ec.entity.makeValue("moqui.security.UserAccount")
        user.setFields(
            {userId: username.replace(/\./g, "_").toUpperCase(), username, userFullName: "Sync " + username, disabled: "N"},
            true,
            null,
            null
        )
        user.create()
    })
}
function makeMessage(subject, rootId) {
    return sys(() => {
        const row = ec.entity.makeValue(MESSAGE)
        row.setSequencedIdPrimary()
        row.setFields(
            {
                emailAccountId: accountId,
                emailFolderId: inboxId,
                subject,
                statusId: "ES_RECEIVED",
                isRead: "N",
                fromAddress: "pat@example.org",
                messageDate: ec.user.nowTimestamp,
                rootEmailMessageId: rootId || null,
                parentEmailMessageId: rootId || null
            },
            true,
            null,
            null
        )
        row.create()
        return row.emailMessageId
    })
}
const draftOf = (clientRef, subject, baseStamp) => ({
    fromValue: identity,
    clientRef,
    toJson: JSON.stringify([{value: "pat@example.org", label: "pat@example.org"}]),
    subject,
    body: "<p>" + subject + "</p>",
    baseStamp
})
const compose = (name, parameters) => attempt("org.moqui.comms.ComposeServices." + name, parameters)
const capture = ec.factory.getTool("MailCapture")
const threadOf = (page, threadId) => Array.from(page.changes || []).find(change => change.threadId === threadId) || null

spec({
    stepwise: true,
    setupSpec() {
        makeUser(OWNER)
        makeUser(OTHER)
        ec.user.internalLoginUser(OWNER)
        accountId = sys(() =>
            call(MAIL + "create#EmailAccount", {
                accountKindEnumId: "EakHosted",
                emailAddress: "sync.owner@example.com",
                accountName: "Sync",
                smtpHost: "127.0.0.1",
                smtpPort: "1",
                smtpStartTls: "N"
            })
        ).emailAccountId
        inboxId = sys(() =>
            ec.entity
                .find("moqui.basic.email.EmailFolder")
                .condition("emailAccountId", accountId)
                .condition("folderKindEnumId", "EfkInbox")
                .one()
        ).emailFolderId
        const row = sys(() => ec.entity.find("moqui.basic.email.EmailIdentity").condition("emailAccountId", accountId).list())[0]
        identity = "identity:" + row.emailIdentityId
        // the chunks of these specs are a few bytes
        setProperty("client_upload_min_chunk_bytes", "1")
        capture.enable()
    },
    cleanupSpec() {
        ec.user.setEffectiveTime(null)
        capture.reset()
    }
})

feature("the first read starts the cursor; arrivals then page in the order they committed, one change per thread", {
    when() {
        ec.user.internalLoginUser(OWNER)
        const first = changes(null)
        // a user with no cursor gets no rows; this one has one now
        ids.first = makeMessage("First")
        ids.second = makeMessage("Second")
        ids.third = makeMessage("Third")
        ids.reply = makeMessage("Re: First", ids.first)
        const page1 = changes(first.lastSeq, 2)
        const page2 = changes(page1.lastSeq, 2)
        const screen = request("Inbox/Mail/mailChanges", {afterSeq: String(first.lastSeq), limit: "5"}, null)
        const none = request("Inbox/Mail/mailChanges", {afterSeq: String(page2.lastSeq)}, null)
        cursor = page2.lastSeq
        return {first, page1, page2, screen, none}
    },
    then({first, page1, page2, screen, none}) {
        first.errors == ""
        first.reset == true
        first.changes.length == 0
        page1.errors == ""
        page1.reset == false
        page1.more == true
        page1.changes.map(change => change.threadId).join(",") == [ids.second, ids.third].join(",")
        page1.changes[0].kind == "new"
        page1.changes[0].messages.length == 1
        page1.changes[0].messages[0].isRead == false
        page2.more == false
        page2.changes.length == 1
        page2.changes[0].threadId == ids.first
        page2.changes[0].messages.map(row => row.emailMessageId).join(",") == [ids.first, ids.reply].join(",")
        page2.lastSeq > page1.lastSeq
        screen.json.unsupported == undefined
        screen.json.changes.length == 3
        screen.json.reset == false
        // the stores the cursor does not cover: IronStone's internal messages, none of BasicComms'
        JSON.stringify(screen.json.uncoveredStores) == (bossInstalled ? '["internal"]' : "[]")
        none.json.changes.length == 0
        none.json.lastSeq == page2.lastSeq
    }
})

feature("the client config names what the app's inbox source supports offline, in server.features", {
    when() {
        ec.user.internalLoginUser(OWNER)
        const str = ec.screen.makeTest().rootScreen("component://webroot/screen/webroot.xml").render("qapps/clientConfig", {}, null)
        const errors = str.errorMessages.join(" | ")
        ec.message.clearErrors()
        let json = {}
        try {
            json = JSON.parse(String(str.output || ""))
        } catch (err) {
            json = {}
        }
        const features = (json.server && json.server.features) || {}
        const uncovered = Array.from(features.mailChangesUncovered || []).join(",")
        return {errors, json, features, uncovered}
    },
    then({errors, json, features, uncovered}) {
        errors == ""
        json.server.clientApi == 4
        typeof features == "object"
        features.internalSendRef == (bossInstalled ? true : undefined)
        uncovered == (bossInstalled ? "internal" : "")
    }
})

feature("a state change by clientRef happens once: the retry answers duplicate and changes nothing", {
    when() {
        ec.user.internalLoginUser(OWNER)
        const target = JSON.stringify([{store: "email", threadId: ids.second}])
        const first = request("Inbox/Mail/state", {targetsJson: target, action: "read", clientRef: "sync-read-1"}, "post")
        // made unread elsewhere since: a second effect would read it again
        setFields(ids.second, {isRead: "N"})
        const retry = request("Inbox/Mail/state", {targetsJson: target, action: "read", clientRef: "sync-read-1"}, "post")
        const refs = sys(() =>
            ec.entity.find("moqui.basic.client.ClientRequestRef").condition("clientRef", "sync-read-1").list()
        ).length
        const page = changes(cursor)
        cursor = page.lastSeq
        return {first, retry, refs, page, isRead: message(ids.second).isRead}
    },
    then({first, retry, refs, page, isRead}) {
        first.errors == ""
        first.json.updatedCount == 1
        first.json.duplicate == false
        retry.json.duplicate == true
        retry.json.updatedCount == 1
        isRead == "N"
        refs == 1
        page.changes.length == 1
        threadOf(page, ids.second).kind == "state"
        threadOf(page, ids.second).messages[0].isRead == false
    }
})

feature("a queued action whose seen state changed on the server is not done, and says so with the state now", {
    when() {
        ec.user.internalLoginUser(OWNER)
        // read on another device since the client saw it unread
        setFields(ids.third, {isRead: "Y"})
        // the framework's: an app's inbox source checks seen through check#SeenState too
        const out = attempt("org.moqui.comms.InboxServices.set#ThreadState", {
            action: "unread",
            targetList: [
                {store: "email", threadId: ids.third, seen: {isRead: false}},
                {store: "email", threadId: ids.first, seen: {isRead: false}}
            ]
        })
        return {out, third: message(ids.third).isRead}
    },
    then({out, third}) {
        out.updatedCount == 1
        out.failedList.length == 1
        out.failedList[0].key == "email:" + ids.third
        out.failedList[0].reason == "conflict"
        out.failedList[0].current.isRead == true
        third == "Y"
    }
})

feature("deleteForever leaves a tombstone: the thread is deleted with its message removed", {
    when() {
        ec.user.internalLoginUser(OWNER)
        const trashed = op("set#ThreadState", {action: "trash", targetList: [{store: "email", threadId: ids.third}]})
        const moved = changes(cursor)
        const deleted = op("set#ThreadState", {action: "deleteForever", targetList: [{store: "email", threadId: ids.third}]})
        const page = changes(moved.lastSeq)
        cursor = page.lastSeq
        return {trashed, moved, deleted, page, gone: message(ids.third)}
    },
    then({trashed, moved, deleted, page, gone}) {
        trashed.updatedCount == 1
        threadOf(moved, ids.third).kind == "move"
        threadOf(moved, ids.third).messages[0].emailFolderId != inboxId
        deleted.updatedCount == 1
        gone == null
        threadOf(page, ids.third).kind == "delete"
        threadOf(page, ids.third).deleted == true
        threadOf(page, ids.third).removedMessageIds.includes(ids.third)
    }
})

feature("a cursor older than the log kept, past the last change, or not read for too long answers reset", {
    when() {
        ec.user.internalLoginUser(OWNER)
        makeMessage("Fourth")
        const current = changes(cursor)
        const past = changes(current.lastSeq + 5)
        // the log purged before now, its reader active
        sys(() => {
            const state = ec.entity.find("moqui.basic.email.EmailUserState").condition("userId", "BASIC_COMMS_SYNC").one()
            state.set("lastReadDate", Number(ec.user.nowTimestamp) + 86400000)
            // its catch-up window over, as it is long before a purge
            state.set("catchUpFromDate", null)
            state.set("catchUpThruDate", null)
            state.update()
        })
        const purged = sys(() =>
            call(MAIL + "clean#UserMailChanges", {userId: "BASIC_COMMS_SYNC", before: Number(ec.user.nowTimestamp) + 1000})
        )
        const old = changes(cursor)
        const atEnd = changes(current.lastSeq)
        // not read for longer than the log is kept: the cursor and its log go
        const dropped = sys(() =>
            call(MAIL + "clean#UserMailChanges", {
                userId: "BASIC_COMMS_SYNC",
                before: Number(ec.user.nowTimestamp) + 2 * 86400000
            })
        )
        // a dormant cursor notes nothing until it is read again
        const quiet = makeMessage("While dormant")
        const noted = sys(() =>
            ec.entity.find("moqui.basic.email.EmailUserChange").condition("userId", "BASIC_COMMS_SYNC").count()
        )
        const again = changes(current.lastSeq)
        // another device, asleep as long: its cursor is behind the numbers, never caught up by a new sequence
        const stale = changes(cursor)
        const after = changes(again.lastSeq)
        cursor = again.lastSeq
        return {current, past, purged, old, atEnd, dropped, noted, again, stale, after, quiet}
    },
    then({current, past, purged, old, atEnd, dropped, noted, again, stale, after, quiet}) {
        current.changes.length == 1
        current.reset == false
        past.reset == true
        purged.removedCount > 0
        old.reset == true
        old.lastSeq == current.lastSeq
        atEnd.reset == false
        atEnd.changes.length == 0
        dropped.removedCount == 0
        noted == 0
        again.reset == true
        again.lastSeq > current.lastSeq
        stale.reset == true
        after.reset == false
    }
})

feature("a caught-up cursor answers reset when its user is added to an account, and again when removed", {
    when() {
        ec.user.internalLoginUser(OTHER)
        const start = changes(null)
        const caughtUp = changes(start.lastSeq)
        ec.user.internalLoginUser(OWNER)
        sys(() =>
            call(MAIL + "add#EmailAccountUser", {
                emailAccountId: accountId,
                userId: "BASIC_COMMS_SYNC_OTHER",
                permissionEnumId: "EapRead"
            })
        )
        ec.user.internalLoginUser(OTHER)
        const added = changes(caughtUp.lastSeq)
        const next = changes(added.lastSeq)
        ec.user.internalLoginUser(OWNER)
        sys(() => call(MAIL + "remove#EmailAccountUser", {emailAccountId: accountId, userId: "BASIC_COMMS_SYNC_OTHER"}))
        ec.user.internalLoginUser(OTHER)
        const removed = changes(next.lastSeq)
        return {caughtUp, added, next, removed}
    },
    then({caughtUp, added, next, removed}) {
        caughtUp.reset == false
        added.reset == true
        added.lastSeq > caughtUp.lastSeq
        next.reset == false
        removed.reset == true
    }
})

feature("after a reset, a change a writer committed late without a note is caught up at the next read", {
    when() {
        ec.user.internalLoginUser(OWNER)
        sys(() => call(MAIL + "reset#MailChanges", {userIdList: ["BASIC_COMMS_SYNC"]}))
        const reset = changes(cursor)
        // the late writer: its change is in the row, its note missed (no cursor then)
        const late = makeMessage("Late writer")
        sys(() => ec.entity.find("moqui.basic.email.EmailUserChange").condition("rootEmailMessageId", late).deleteAll())
        const page = changes(reset.lastSeq)
        cursor = page.lastSeq
        return {reset, page, late}
    },
    then({reset, page, late}) {
        reset.reset == true
        page.reset == false
        threadOf(page, late) != null
        threadOf(page, late).messages.length == 1
    }
})

feature("a seen check that finds nothing keeps the request's clientRef, and a message newer than the one seen is a conflict", {
    when() {
        ec.user.internalLoginUser(OWNER)
        const gone = JSON.stringify([{store: "email", threadId: "NO_SUCH_THREAD", seen: {isRead: false}}])
        const first = request("Inbox/Mail/state", {targetsJson: gone, action: "read", clientRef: "sync-gone-1"}, "post")
        const retry = request("Inbox/Mail/state", {targetsJson: gone, action: "read", clientRef: "sync-gone-1"}, "post")
        // a reply came after the client saw the thread: its read does not take the reply unseen
        const reply = makeMessage("Re: Second", ids.second)
        const newer = attempt("org.moqui.comms.InboxServices.set#ThreadState", {
            action: "read",
            targetList: [{store: "email", threadId: ids.second, seen: {latestMessageId: ids.second}}]
        })
        // labels compare only the ones the action changes
        const labels = attempt(MAIL + "check#SeenState", {
            rootEmailMessageId: ids.second,
            seen: {labelIds: ["OTHER_LABEL"]},
            onlyLabelIds: ["L1"]
        })
        const unknown = attempt(MAIL + "check#SeenState", {rootEmailMessageId: "NO_SUCH_THREAD", seen: {isRead: true}})
        return {first, retry, newer, labels, unknown, reply}
    },
    then({first, retry, newer, labels, unknown, reply}) {
        first.errors == ""
        first.json.failedList.length == 1
        retry.json.duplicate == true
        newer.updatedCount == 0
        newer.failedList[0].reason == "conflict"
        newer.failedList[0].current.latestMessageId == reply
        labels.conflict == false
        unknown.errors == ""
        unknown.found == false
    }
})

feature("a draft saved elsewhere since the client's copy is kept: the save answers conflict with the server's draft", {
    when() {
        ec.user.internalLoginUser(OWNER)
        const start = Number(ec.user.nowTimestamp)
        ec.user.setEffectiveTime(start)
        const first = compose("save#ComposeDraft", draftOf("sync-draft-1", "First draft"))
        ec.user.setEffectiveTime(start + 1000)
        const elsewhere = compose("save#ComposeDraft", draftOf("sync-draft-1", "Saved elsewhere", first.draftStamp))
        ec.user.setEffectiveTime(start + 2000)
        const stale = compose("save#ComposeDraft", draftOf("sync-draft-1", "The stale copy", first.draftStamp))
        // a file added keeps the stamp, so the copy that added it still saves
        ids.draft = first.emailMessageId
        const upload = compose("upload#ComposeAttachment", {emailMessageId: ids.draft, contentFile: uploadedFile("a.txt", "A")})
        ec.user.setEffectiveTime(start + 3000)
        const after = compose("save#ComposeDraft", draftOf("sync-draft-1", "After the file", elsewhere.draftStamp))
        const data = compose("get#ComposeData", {emailMessageId: ids.draft})
        // its answer lost: the same save sent again, with the stamp it had, is not a conflict with itself
        ec.user.setEffectiveTime(start + 4000)
        const retried = compose("save#ComposeDraft", draftOf("sync-draft-1", "After the file", elsewhere.draftStamp))
        // by its saveRef too, after a later edit from here changed the content it brings
        ec.user.setEffectiveTime(start + 5000)
        const own = compose(
            "save#ComposeDraft",
            Object.assign(draftOf("sync-draft-1", "Own edit", retried.draftStamp), {saveRef: "sync-save-1"})
        )
        ec.user.setEffectiveTime(start + 6000)
        const ownAgain = compose(
            "save#ComposeDraft",
            Object.assign(draftOf("sync-draft-1", "Own edit again", retried.draftStamp), {saveRef: "sync-save-1"})
        )
        // another save with a stale stamp, its content not the draft's: a conflict
        const notOwn = compose(
            "save#ComposeDraft",
            Object.assign(draftOf("sync-draft-1", "From elsewhere", retried.draftStamp), {saveRef: "sync-save-2"})
        )
        // a send after its save's answer was lost: the draft holds what the send brings, so it goes
        ec.user.setEffectiveTime(start + 7000)
        const second = compose("save#ComposeDraft", draftOf("sync-draft-2", "Sent after a lost save"))
        ec.user.setEffectiveTime(start + 8000)
        const sent = compose(
            "send#Compose",
            Object.assign(draftOf("sync-draft-2", "Sent after a lost save", second.draftStamp - 500), {holdSeconds: 0})
        )
        ec.user.setEffectiveTime(null)
        return {
            first,
            elsewhere,
            stale,
            upload,
            after,
            data,
            retried,
            own,
            ownAgain,
            notOwn,
            sent,
            subject: message(ids.draft).subject
        }
    },
    then({first, elsewhere, stale, upload, after, data, retried, own, ownAgain, notOwn, sent, subject}) {
        sent.errors == ""
        sent.conflict == false
        sent.sent == true
        retried.conflict == false
        own.conflict == false
        ownAgain.conflict == false
        notOwn.conflict == true
        subject == "Own edit again"
        first.errors == ""
        first.draftStamp != null
        elsewhere.errors == ""
        elsewhere.conflict == false
        elsewhere.draftStamp != first.draftStamp
        stale.errors == ""
        stale.conflict == true
        stale.emailMessageId == first.emailMessageId
        stale.compose.subject == "Saved elsewhere"
        upload.errors == ""
        after.errors == ""
        after.conflict == false
        data.compose.draftStamp == after.draftStamp
    }
})

feature("a file sent in chunks goes on after a partial and a wrong offset, and the draft takes it by uploadId", {
    when() {
        ec.user.internalLoginUser(OWNER)
        const chunk = (offset, text) =>
            request(
                "Inbox/Compose/uploadChunk",
                {
                    uploadId: "sync-up-1",
                    offset: String(offset),
                    total: "10",
                    fileName: "hello.txt",
                    contentType: "text/plain",
                    chunk: uploadedFile("hello.txt", text)
                },
                "post"
            ).json
        const part = chunk(0, "Hello")
        const status = request("Inbox/Compose/uploadStatus", {uploadId: "sync-up-1"}, null).json
        const again = chunk(0, "Hello")
        const gap = chunk(7, "rld")
        const early = compose("upload#ComposeAttachment", {emailMessageId: ids.draft, uploadId: "sync-up-1"})
        const last = chunk(5, "World")
        const attached = compose("upload#ComposeAttachment", {emailMessageId: ids.draft, uploadId: "sync-up-1"})
        const file = sys(() =>
            ec.entity
                .find("moqui.basic.email.EmailMessageAttachment")
                .condition("emailMessageId", ids.draft)
                .condition("fileName", "hello.txt")
                .one()
        )
        const text = file ? sys(() => ec.resource.getLocationReference(file.attachmentLocation).getText()) : null
        const noted = changes(cursor)
        cursor = noted.lastSeq
        ec.user.internalLoginUser(OTHER)
        const other = attempt(SYNC + "take#Upload", {uploadId: "sync-up-1"})
        const otherStatus = attempt(SYNC + "get#UploadStatus", {uploadId: "sync-up-1"})
        return {part, status, again, gap, early, last, attached, text, other, otherStatus, noted}
    },
    then({part, status, again, gap, early, last, attached, text, other, otherStatus, noted}) {
        threadOf(noted, ids.draft).messages[0].hasAttachment == true
        part.accepted == true
        part.received == 5
        part.complete == false
        status.found == true
        status.received == 5
        again.accepted == false
        again.received == 5
        gap.accepted == false
        gap.received == 5
        early.errors.includes("not complete")
        last.accepted == true
        last.complete == true
        last.received == 10
        attached.errors == ""
        attached.attachmentList.some(file => file.name == "hello.txt")
        text == "HelloWorld"
        other.errors.includes("not found")
        otherStatus.found == false
    }
})

feature("uploads are limited: a small chunk other than the last, and more open uploads than allowed, are refused", {
    when() {
        ec.user.internalLoginUser(OWNER)
        const chunk = (uploadId, text, total) =>
            attempt(SYNC + "upload#Chunk", {uploadId, offset: 0, total, fileName: "x.txt", chunk: uploadedFile("x.txt", text)})
        const small = withProperty("client_upload_min_chunk_bytes", "4", () => chunk("sync-small-1", "abc", 10))
        const lastSmall = withProperty("client_upload_min_chunk_bytes", "4", () => chunk("sync-small-2", "abc", 3))
        const open = withProperty("client_upload_max_count", "1", () => [
            chunk("sync-open-1", "ab", 10),
            chunk("sync-open-2", "ab", 10)
        ])
        const big = withProperty("client_upload_max_mb", "1", () => chunk("sync-big-1", "ab", 2 * 1048576))
        return {small, lastSmall, open, big}
    },
    then({small, lastSmall, open, big}) {
        small.errors.includes("at least 4 bytes")
        lastSmall.errors == ""
        lastSmall.complete == true
        open[0].errors == ""
        open[1].errors.includes("At most 1 uploads")
        big.errors.includes("MB of uploads")
    }
})

feature("chat edits and reactions by clientRef happen once; a file in chunks goes to a room", {
    when() {
        ec.user.internalLoginUser(OWNER)
        const room = request("Chat/createRoom", {roomTypeEnumId: "private", roomName: "Sync room"}, "post").json.chatRoomId
        const sent = request("Chat/send", {chatRoomId: room, body: "<p>Hello</p>", clientRef: "sync-chat-1"}, "post").json
        const react = () =>
            request(
                "Chat/react",
                {chatRoomId: room, eventSeq: String(sent.eventSeq), key: "👍", clientRef: "sync-react-1"},
                "post"
            ).json
        const reacted = react()
        const reactedAgain = react()
        const edit = () =>
            request(
                "Chat/edit",
                {chatRoomId: room, eventSeq: String(sent.eventSeq), body: "<p>Hello again</p>", clientRef: "sync-edit-1"},
                "post"
            ).json
        const edited = edit()
        const editedAgain = edit()
        const count = type =>
            sys(() =>
                ec.entity.find("moqui.chat.ChatEvent").condition("chatRoomId", room).condition("eventTypeEnumId", type).count()
            )
        request(
            "Chat/uploadChunk",
            {
                uploadId: "sync-up-2",
                offset: "0",
                total: "4",
                fileName: "note.txt",
                contentType: "text/plain",
                chunk: uploadedFile("note.txt", "Note")
            },
            "post"
        )
        const uploaded = request("Chat/upload", {chatRoomId: room, uploadId: "sync-up-2"}, "post").json
        return {
            reacted,
            reactedAgain,
            edited,
            editedAgain,
            reactions: count("CetReaction"),
            redactions: count("CetRedaction"),
            edits: count("CetEdit"),
            uploaded
        }
    },
    then({reacted, reactedAgain, edited, editedAgain, reactions, redactions, edits, uploaded}) {
        reacted.added == true
        reacted.duplicate == false
        reactedAgain.duplicate == true
        reactedAgain.added == true
        reactions == 1
        redactions == 0
        edited.editEventSeq != null
        editedAgain.duplicate == true
        editedAgain.editEventSeq == edited.editEventSeq
        edits == 1
        uploaded.uploadList.length == 1
        uploaded.uploadList[0].fileName == "note.txt"
        uploaded.uploadList[0].fileSize == 4
    }
})

feature("the jobs clean up: an upload left past its hours, and a clientRef past its days", {
    when() {
        ec.user.internalLoginUser(OWNER)
        const old = Number(ec.user.nowTimestamp) - 3 * 86400000
        sys(() => {
            const upload = ec.entity.find("moqui.basic.client.ClientUpload").condition("uploadId", "sync-up-1").one()
            upload.set("lastChunkDate", old)
            upload.update()
            const ref = ec.entity.find("moqui.basic.client.ClientRequestRef").condition("clientRef", "sync-read-1").one()
            ref.set("createdDate", old)
            ref.update()
        })
        const uploads = sys(() => call(SYNC + "clean#ClientUploads", {hours: 24}))
        const refs = sys(() => call(SYNC + "clean#ClientRequests", {days: 2}))
        const status = attempt(SYNC + "get#UploadStatus", {uploadId: "sync-up-1"})
        const kept = attempt(SYNC + "get#UploadStatus", {uploadId: "sync-up-2"})
        const left = sys(() =>
            ec.entity.find("moqui.basic.client.ClientRequestRef").condition("clientRef", "sync-read-1").count()
        )
        return {uploads, refs, status, kept, left}
    },
    then({uploads, refs, status, kept, left}) {
        uploads.removedCount == 1
        refs.removedCount >= 1
        status.found == false
        kept.found == true
        left == 0
    }
})
