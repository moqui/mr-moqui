// Voicemail (org.moqui.impl.VoicemailServices and Voicemail.js; the V2 block of framework/entity/VoiceEntities.xml;
// plans/Comms-Framework.md §5.3, §5.4, V2): a call to a busy user goes straight to voicemail and one not answered after the
// box's rings; begin#Voicemail moves the call there (the user missed it); a stored message, its private audio (refused for
// another user with the same id), the PIN over the phone with its refusal and its limits per box and per source, the
// notices (email optional, attachments off by default), a failed transcription that keeps the audio and shows its state,
// its retry, and the trash's retention. No SIP engine runs here: fake SipNode and CallMediaNode rows stand in for one, and
// the spool file the engine would write is made here. Every refusal has its positive case with the same parameters.
// Run: ./moq test framework --spec Voicemail
const A = "VM_SPEC_A"
const B = "VM_SPEC_B"
const NODE = "vm-spec-node"
const NUMBER_A = "+15550170001"
const NUMBER_B = "+15550170002"
const CALLER = "+15557770123"
const BOX = "moqui.call.VoicemailBox"
const VOICEMAIL = "moqui.call.Voicemail"
const TRANSCRIPT = "moqui.call.CallTranscript"

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
    if (userId) ec.user.internalLoginUser(username(userId))
    let out = {}
    let thrown = ""
    try {
        out = sys(() => ec.service.sync().name(name.includes(".") ? name : "org.moqui.impl.VoicemailServices." + name)
            .parameters(parameters || {}).call()) || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
const engine = (name, parameters) => as(null, name, parameters)
function makeUser(userId) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() === 0) {
            const account = ec.entity.makeValue("moqui.security.UserAccount")
            account.setFields({userId, username: username(userId), userFullName: "Voicemail " + userId.slice(-1), disabled: "N"}, true, null, null)
            account.create()
        }
    })
}
function setStatus(userId, inCall) {
    sys(() => {
        const row = ec.entity.find("moqui.security.UserStatus").condition("userId", userId).one()
        if (row) {
            row.set("inCall", inCall)
            row.update()
        } else {
            const made = ec.entity.makeValue("moqui.security.UserStatus")
            made.setFields({userId, inCall}, true, null, null)
            made.create()
        }
    })
}
function create(entity, fields) {
    sys(() => {
        const value = ec.entity.makeValue(entity)
        value.setFields(fields, true, null, null)
        value.create()
    })
}
function clean() {
    sys(() => {
        for (const box of ec.entity.find(BOX).condition("userId", "in", [A, B]).list()) {
            for (const v of ec.entity.find(VOICEMAIL).condition("voicemailBoxId", box.voicemailBoxId).list()) {
                ec.entity.find(TRANSCRIPT).condition("voicemailId", v.voicemailId).deleteAll()
                if (v.audioLocation) {
                    const ref = ec.resource.getLocationReference(v.audioLocation)
                    if (ref.getExists()) ref.delete()
                }
                v.delete()
            }
            box.delete()
        }
        ec.entity.find("moqui.call.VoicemailPinFailure").deleteAll()
        for (const s of ec.entity.find("moqui.call.CallSession").condition("kindEnumId", "CskPhone").condition("statusId", "!=", "CssEnded").list()) {
            s.set("statusId", "CssEnded")
            s.update()
        }
        ec.entity.find("moqui.call.PhoneNumber").condition("phoneNumber", "in", [NUMBER_A, NUMBER_B]).deleteAll()
        ec.entity.find("moqui.call.SipNode").condition("instanceId", NODE).deleteAll()
        ec.entity.find("moqui.call.CallMediaNode").condition("instanceId", NODE).deleteAll()
        ec.entity.find("moqui.security.UserStatus").condition("userId", "in", [A, B]).deleteAll()
    })
}
function nodesUp() {
    sys(() => {
        ec.service.sync().name("org.moqui.impl.CallServices.register#MediaNode").parameters({instanceId: NODE, mediaAddress: "127.0.0.1:10000"}).call()
        ec.service.sync().name("org.moqui.impl.VoiceServices.register#SipNode").parameters({instanceId: NODE, sipAddress: "127.0.0.1:5060"}).call()
    })
}
const receive = (toNumber) => engine("org.moqui.impl.VoiceServices.receive#PhoneCall", {toNumber, fromNumber: CALLER,
    fromName: "Spec Caller", instanceId: NODE})
const boxOf = (userId) => sys(() => ec.entity.find(BOX).condition("userId", userId).one())
const voicemailRow = (id) => sys(() => ec.entity.find(VOICEMAIL).condition("voicemailId", id).one())
const userLeg = (callSessionId, userId) => sys(() => {
    const p = ec.entity.find("moqui.call.CallParticipant").condition({callSessionId, userId}).one()
    return p ? ec.entity.find("moqui.call.CallLeg").condition("callParticipantId", p.callParticipantId).one() : null
})
const log = (callSessionId, userId) => sys(() => ec.entity.find("moqui.call.CallUserLog").condition({callSessionId, userId}).one())
// a message as the SIP engine hands it over: a spooled file, then store#Voicemail
let spooled = 0
function leave(voicemailBoxId, callSessionId) {
    const path = ec.factory.getRuntimePath() + "/tmp/voicemail/vm-spec-" + (++spooled) + ".ogg"
    ec.resource.getLocationReference("file:" + path).putText("OggS a voicemail spec's message")
    return engine("store#Voicemail", {voicemailBoxId, callSessionId: callSessionId || null, tempPath: path, durationMs: 4200,
        fromNumber: CALLER, fromName: "Spec Caller"})
}

let boxA = null
let first = null

spec({
    stepwise: true,
    setupSpec() {
        makeUser(A)
        makeUser(B)
        clean()
        create("moqui.call.PhoneNumber", {phoneNumber: NUMBER_A, routeTypeEnumId: "PnrUser", routeId: A})
        create("moqui.call.PhoneNumber", {phoneNumber: NUMBER_B, routeTypeEnumId: "PnrUser", routeId: B})
        nodesUp()
    },
    cleanupSpec() {
        clean()
    }
})

feature("a busy user's caller goes straight to voicemail; a free one rings for the box's rings", {
    when() {
        setStatus(A, "Y")
        const busy = receive(NUMBER_A)
        setStatus(A, "N")
        const free = receive(NUMBER_A)
        const before = as(A, "get#VoicemailSettings")
        const saved = as(A, "update#VoicemailSettings", {rings: 2})
        const tooMany = as(A, "update#VoicemailSettings", {rings: 11})
        const fewer = receive(NUMBER_A)
        return {busy, free, before, saved, tooMany, fewer, busyLeg: userLeg(busy.callSessionId, A)}
    },
    then({busy, free, before, saved, tooMany, fewer, busyLeg}) {
        busy.errors == ""
        busy.voicemail == "busy"
        busyLeg.statusId == "ClsOffered"
        free.voicemail == null
        free.voicemailSeconds == before.ringSeconds
        before.box.hasPin == false
        before.box.attachAudio == false
        before.box.attachTranscript == false
        saved.box.rings == 2
        tooMany.reasonCode == "badRings"
        fewer.voicemailSeconds == 12
    }
})

feature("begin#Voicemail moves the call there: the user missed it and the box answers", {
    when() {
        setStatus(A, "Y")
        const call = receive(NUMBER_A)
        setStatus(A, "N")
        const begun = engine("begin#Voicemail", {callSessionId: call.callSessionId, callLegId: call.callLegId, reason: "busy"})
        const session = sys(() => ec.entity.find("moqui.call.CallSession").condition("callSessionId", call.callSessionId).one())
        // a call someone answered does not go: the user answers first, then the engine's timer fires
        const answered = receive(NUMBER_A)
        as(A, "org.moqui.impl.CallServices.answer#Call", {callSessionId: answered.callSessionId})
        const late = engine("begin#Voicemail", {callSessionId: answered.callSessionId, callLegId: answered.callLegId, reason: "noAnswer"})
        return {call, begun, session, leg: userLeg(call.callSessionId, A), log: log(call.callSessionId, A), late}
    },
    then({call, begun, session, leg, log, late}) {
        begun.errors == ""
        begun.mode == "deposit"
        begun.voicemailBoxId == boxOf(A).voicemailBoxId
        begun.fromNumber == CALLER
        begun.hasPin == false
        begun.maxSeconds == 120
        session.statusId == "CssActive"
        leg.statusId == "ClsTimedOut"
        log.outcomeEnumId == "CuoMissed"
        late.reasonCode == "answered"
    },
    cleanup() { boxA = boxOf(A).voicemailBoxId }
})

feature("a declined call keeps its user's decline and still goes to voicemail", {
    when() {
        const call = receive(NUMBER_A)
        const declined = as(A, "org.moqui.impl.CallServices.decline#Call", {callSessionId: call.callSessionId})
        const session = sys(() => ec.entity.find("moqui.call.CallSession").condition("callSessionId", call.callSessionId).one())
        const begun = engine("begin#Voicemail", {callSessionId: call.callSessionId, callLegId: call.callLegId, reason: "declined"})
        return {declined, status: session.statusId, begun, log: log(call.callSessionId, A)}
    },
    then({declined, status, begun, log}) {
        declined.errors == ""
        status != "CssEnded"
        begun.mode == "deposit"
        log.outcomeEnumId == "CuoDeclined"
    }
})

feature("a stored message is its owner's: the same voicemail is refused for another user", {
    when() {
        const stored = leave(boxA)
        first = stored.voicemailId
        const row = voicemailRow(first)
        const mine = as(A, "find#Voicemails")
        const audio = as(A, "get#VoicemailAudio", {voicemailId: first})
        const detail = as(A, "get#Voicemail", {voicemailId: first})
        const theirs = as(B, "find#Voicemails")
        const refused = {
            audio: as(B, "get#VoicemailAudio", {voicemailId: first}),
            detail: as(B, "get#Voicemail", {voicemailId: first}),
            mark: as(B, "mark#VoicemailHeard", {voicemailId: first}),
            remove: as(B, "delete#Voicemail", {voicemailId: first}),
            retry: as(B, "retry#VoicemailTranscript", {voicemailId: first}),
        }
        const after = voicemailRow(first)
        const missing = as(B, "get#VoicemailAudio", {voicemailId: "VM_SPEC_NONE"})
        return {stored, row, mine, audio, detail, theirs, refused, after, missing}
    },
    then({stored, row, mine, audio, detail, theirs, refused, after, missing}) {
        stored.errors == ""
        row.statusId == "VmsStored"
        row.audioLocation == "dbresource://moqui/call/voicemail/" + boxA + "/" + first + ".ogg"
        row.folder == "inbox"
        mine.voicemailList.some(v => v.voicemailId == first && !v.heard && v.fromNumber == CALLER)
        mine.unheardCount >= 1
        audio.location == row.audioLocation
        audio.contentType == "audio/ogg"
        detail.voicemail.voicemailId == first
        theirs.voicemailList.length == 0
        refused.audio.reasonCode == "notFound"
        refused.audio.location == null
        refused.detail.reasonCode == "notFound"
        refused.mark.reasonCode == "notFound"
        refused.remove.reasonCode == "notFound"
        refused.retry.reasonCode == "notFound"
        after.heardDate == null
        after.folder == "inbox"
        missing.reasonCode == refused.audio.reasonCode
        missing.errors == refused.audio.errors
    }
})

feature("the PIN: weak ones refused, a wrong one refused over the phone and the right one opens the new messages", {
    when() {
        const weak = as(A, "set#VoicemailPin", {pin: "1234"})
        const short = as(A, "set#VoicemailPin", {pin: "12"})
        const set = as(A, "set#VoicemailPin", {pin: "4719"})
        const hash = boxOf(A).pinHash
        const wrong = engine("check#VoicemailPin", {voicemailBoxId: boxA, pin: "4718", fromNumber: CALLER})
        const right = engine("check#VoicemailPin", {voicemailBoxId: boxA, pin: "4719", fromNumber: CALLER})
        // the access number: the box by its number
        const byNumber = engine("check#VoicemailPin", {boxNumber: NUMBER_A.slice(2), pin: "4719", fromNumber: CALLER})
        const noBox = engine("check#VoicemailPin", {boxNumber: "5550179999", pin: "4719", fromNumber: CALLER})
        const noPin = engine("check#VoicemailPin", {boxNumber: NUMBER_B.slice(2), pin: "4719", fromNumber: CALLER})
        const wrongByNumber = engine("check#VoicemailPin", {boxNumber: NUMBER_A.slice(2), pin: "4718", fromNumber: CALLER})
        return {weak, short, set, hash, wrong, right, byNumber, noBox, noPin, wrongByNumber}
    },
    then({weak, short, set, hash, wrong, right, byNumber, noBox, noPin, wrongByNumber}) {
        weak.reasonCode == "weakPin"
        short.reasonCode == "badPin"
        set.errors == ""
        set.box.hasPin == true
        hash.startsWith("$argon2")
        !hash.includes("4719")
        wrong.errors == ""
        wrong.accepted == false
        wrong.blocked == false
        wrong.messageList == null
        right.accepted == true
        right.voicemailBoxId == boxA
        right.messageList.some(m => m.voicemailId == first)
        byNumber.accepted == true
        noBox.accepted == false
        noPin.accepted == false
        // a box number with no box, or with no PIN, is refused as a wrong PIN for a real one is: nothing tells them apart
        JSON.stringify(noBox) == JSON.stringify(wrongByNumber)
        JSON.stringify(noPin) == JSON.stringify(wrongByNumber)
    }
})

feature("failed PINs are limited per box and per source, and a block refuses the right PIN too", {
    when() {
        sys(() => ec.entity.find("moqui.call.VoicemailPinFailure").deleteAll())
        // setting the PIN clears the box's count of the last feature's wrong tries
        as(A, "set#VoicemailPin", {pin: "4719"})
        const tries = []
        for (let i = 0; i < 5; i++) tries.push(engine("check#VoicemailPin", {voicemailBoxId: boxA, pin: "1111", fromNumber: "+1555000000" + i}))
        const blockedRight = engine("check#VoicemailPin", {voicemailBoxId: boxA, pin: "4719", fromNumber: "+15550009999"})
        // a source that guesses across boxes, the boxes not there among them, is blocked on its own
        const source = []
        for (let i = 0; i < 10; i++) source.push(engine("check#VoicemailPin", {boxNumber: "55501800" + String(i).padStart(2, "0"), pin: "2580", fromNumber: "+15559990000"}))
        const sourceRow = sys(() => ec.entity.find("moqui.call.VoicemailPinFailure").condition("failureKey", "source:+15559990000").one())
        // the box's owner sets a new PIN, which clears the box's block
        as(A, "set#VoicemailPin", {pin: "4720"})
        const afterReset = engine("check#VoicemailPin", {voicemailBoxId: boxA, pin: "4720", fromNumber: "+15550009998"})
        const fromBlockedSource = engine("check#VoicemailPin", {voicemailBoxId: boxA, pin: "4720", fromNumber: "+15559990000"})
        return {tries, blockedRight, source, sourceRow, afterReset, fromBlockedSource}
    },
    then({tries, blockedRight, source, sourceRow, afterReset, fromBlockedSource}) {
        tries.slice(0, 4).every(t => !t.accepted && !t.blocked)
        tries[4].blocked == true
        blockedRight.accepted == false
        blockedRight.blocked == true
        source.slice(0, 9).every(t => !t.accepted && !t.blocked)
        source[9].blocked == true
        sourceRow.blockedUntilDate != null
        afterReset.accepted == true
        fromBlockedSource.accepted == false
        fromBlockedSource.blocked == true
    }
})

feature("notices are optional and attachments are off by default", {
    when() {
        const none = engine("send#VoicemailNotice", {voicemailId: first})
        const noEmailAttach = as(A, "update#VoicemailSettings", {attachAudio: true})
        const plain = as(A, "update#VoicemailSettings", {notifyEmail: "vm.spec@example.com"})
        const sent = engine("send#VoicemailNotice", {voicemailId: first})
        const again = engine("send#VoicemailNotice", {voicemailId: first})
        const email = sys(() => ec.entity.find("moqui.basic.email.EmailMessage").condition("emailMessageId", sent.emailMessageId).one())
        const plainAttachments = sys(() => ec.entity.find("moqui.basic.email.EmailMessageAttachment").condition("emailMessageId", sent.emailMessageId).count())
        const withAudio = as(A, "update#VoicemailSettings", {notifyEmail: "vm.spec@example.com", attachAudio: true})
        // the next message's notice, sent here rather than after its commit
        sys(() => { const b = ec.entity.find(BOX).condition("voicemailBoxId", boxA).one(); b.set("notifyEmail", null); b.update() })
        const second = leave(boxA)
        sys(() => { const b = ec.entity.find(BOX).condition("voicemailBoxId", boxA).one(); b.set("notifyEmail", "vm.spec@example.com"); b.update() })
        const audioSent = engine("send#VoicemailNotice", {voicemailId: second.voicemailId})
        const attachments = sys(() => Array.from(ec.entity.find("moqui.basic.email.EmailMessageAttachment").condition("emailMessageId", audioSent.emailMessageId).list()))
        as(A, "update#VoicemailSettings", {notifyEmail: ""})
        return {none, noEmailAttach, plain, sent, again, email, plainAttachments, withAudio, second, attachments}
    },
    then({none, noEmailAttach, plain, sent, again, email, plainAttachments, withAudio, second, attachments}) {
        none.sent == false
        noEmailAttach.box.attachAudio == false
        plain.warning == null
        plain.box.attachAudio == false
        plain.box.attachTranscript == false
        sent.sent == true
        again.sent == false
        email.toAddresses == "vm.spec@example.com"
        email.subject.includes("Spec Caller")
        email.body.includes("/Calls/Voicemail?voicemailId=" + first)
        plainAttachments == 0
        withAudio.warning != null
        withAudio.box.attachAudio == true
        attachments.length == 1
        attachments[0].attachmentLocation == voicemailRow(second.voicemailId).audioLocation
        attachments[0].contentType == "audio/ogg"
    }
})

feature("a failed transcription keeps the audio and shows its state, and its owner retries it", {
    when() {
        const off = as(A, "retry#VoicemailTranscript", {voicemailId: first})
        return withProperty("voicemail_transcribe", "true", () => {
            // no speech provider answers here: the transcription fails
            const ran = engine("transcribe#Voicemail", {voicemailId: first})
            const detail = as(A, "get#Voicemail", {voicemailId: first})
            const row = voicemailRow(first)
            const listed = as(A, "find#Voicemails")
            const retried = as(A, "retry#VoicemailTranscript", {voicemailId: first})
            const pending = sys(() => Array.from(ec.entity.find(TRANSCRIPT).condition("voicemailId", first).list()))
            const twice = as(A, "retry#VoicemailTranscript", {voicemailId: first})
            return {off, ran, detail, row, listed, retried, pending, twice}
        })
    },
    then({off, ran, detail, row, listed, retried, pending, twice}) {
        off.reasonCode == "transcriptionOff"
        ran.status == "failed"
        detail.voicemail.transcriptStatus == "failed"
        detail.voicemail.transcriptError != null
        detail.voicemail.canRetryTranscript == true
        detail.voicemail.hasAudio == true
        row.audioLocation != null
        row.statusId == "VmsStored"
        listed.voicemailList.find(v => v.voicemailId == first).transcriptStatus == "failed"
        retried.errors == ""
        retried.voicemail.transcriptStatus == "pending"
        pending.length == 1
        twice.reasonCode == "notFailed"
    }
})

feature("delete goes to the trash, the trash empties after voicemail_trash_days with its audio and transcripts", {
    when() {
        const location = voicemailRow(first).audioLocation
        const trashed = as(A, "delete#Voicemail", {voicemailId: first})
        const inbox = as(A, "find#Voicemails")
        const trash = as(A, "find#Voicemails", {folder: "trash"})
        const restored = as(A, "restore#Voicemail", {voicemailId: first})
        as(A, "delete#Voicemail", {voicemailId: first})
        // one trashed long ago, one trashed now
        const recent = leave(boxA).voicemailId
        as(A, "delete#Voicemail", {voicemailId: recent})
        sys(() => { const v = ec.entity.find(VOICEMAIL).condition("voicemailId", first).one(); v.set("trashedDate", new Date(Date.now() - 40 * 86400000)); v.update() })
        const kept = withProperty("voicemail_trash_days", "0", () => engine("expire#VoicemailTrash"))
        const expired = engine("expire#VoicemailTrash")
        const gone = voicemailRow(first)
        const transcripts = sys(() => ec.entity.find(TRANSCRIPT).condition("voicemailId", first).count())
        const fileGone = !sys(() => ec.resource.getLocationReference(location).getExists())
        const stays = voicemailRow(recent)
        const emptied = as(A, "empty#VoicemailTrash")
        return {trashed, inbox, trash, restored, kept, expired, gone, transcripts, fileGone, stays, emptied, recent}
    },
    then({trashed, inbox, trash, restored, kept, expired, gone, transcripts, fileGone, stays, emptied, recent}) {
        trashed.trashed == true
        !inbox.voicemailList.some(v => v.voicemailId == first)
        trash.voicemailList.some(v => v.voicemailId == first)
        restored.voicemail.folder == "inbox"
        kept.deletedCount == 0
        expired.deletedCount >= 1
        gone == null
        transcripts == 0
        fileGone
        stays != null
        stays.folder == "trash"
        emptied.deletedCount >= 1
        voicemailRow(recent) == null
    }
})

feature("a greeting upload takes WAV and Ogg Opus and refuses anything else", {
    when() {
        const bad = as(A, "store#VoicemailGreeting", {file: uploadedFile("greeting.txt", "not audio at all", "text/plain")})
        // a short WAV: kept as Ogg Opus where phone calls are built in, else as it is
        const header = "RIFF$\u0000\u0000\u0000WAVEfmt \u0010\u0000\u0000\u0000\u0001\u0000\u0001\u0000\u0080»\u0000\u0000\u0000w\u0001\u0000\u0002\u0000\u0010\u0000data\u0000\u0000\u0000\u0000"
        const wav = as(A, "store#VoicemailGreeting", {file: uploadedFile("greeting.wav", header, "audio/wav")})
        const box = boxOf(A)
        const got = as(A, "get#VoicemailGreeting")
        const other = as(B, "get#VoicemailGreeting")
        const removed = as(A, "delete#VoicemailGreeting")
        return {bad, wav, box, got, other, removed}
    },
    then({bad, wav, box, got, other, removed}) {
        bad.reasonCode == "badAudio"
        wav.reasonCode == "badAudio" || wav.box.hasGreeting == true
        !wav.converted || box.greetingContentType == "audio/ogg"
        wav.reasonCode == "badAudio" || got.location == box.greetingLocation
        other.reasonCode == "notFound"
        removed.box.hasGreeting == false
    }
})
