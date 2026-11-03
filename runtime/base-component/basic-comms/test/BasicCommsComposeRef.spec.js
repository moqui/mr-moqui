// Compose retried by the client's own reference (clientRef; plans/Mobile-Desktop.md MD4): a client that queued mail offline
// saves, uploads and sends it again after a lost answer, and the server keeps one draft, attaches a file once and sends once
// (org.moqui.comms.ComposeServices over EmailClientServices.save#Draft). A ref only ever finds the user's own messages. The
// same through run#Operation, which hands a user IronStone serves to its ComposeServices when mr-boss is installed.
// Run: ./moq test basic-comms --spec BasicCommsComposeRef
const OWNER = "basic.comms.ref"
const OTHER = "basic.comms.ref.other"
const MESSAGE = "moqui.basic.email.EmailMessage"
const capture = ec.factory.getTool("MailCapture")
let ownerIdentity = null
let otherIdentity = null
let johnIdentity = null
const bossInstalled = ec.resource.getLocationReference("component://mr-boss/screen/Comms.xml").getExists()

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
// as the screens call them, authorized by the app's screen
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
const compose = (name, parameters) => attempt("org.moqui.comms.ComposeServices." + name, parameters)
const chips = values => JSON.stringify(values.map(value => ({value, label: value})))
const rows = fields => sys(() => Array.from(ec.entity.find(MESSAGE).condition(fields).list()))
const attachments = emailMessageId =>
    sys(() =>
        Array.from(ec.entity.find("moqui.basic.email.EmailMessageAttachment").condition("emailMessageId", emailMessageId).list())
    )
function makeUser(username) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("username", username).count() > 0) return
        const user = ec.entity.makeValue("moqui.security.UserAccount")
        user.setFields(
            {
                userId: username.replace(/\./g, "_").toUpperCase(),
                username,
                userFullName: "Ref " + username,
                emailAddress: username + "@example.com",
                disabled: "N"
            },
            true,
            null,
            null
        )
        user.create()
    })
}
// a mail account of the current user's own with an SMTP host that the capture never connects to
function makeAccount(address) {
    const accountId = sys(() =>
        call("org.moqui.impl.EmailClientServices.create#EmailAccount", {
            accountKindEnumId: "EakHosted",
            emailAddress: address,
            accountName: "Ref " + address,
            smtpHost: "127.0.0.1",
            smtpPort: "1",
            smtpStartTls: "N"
        })
    ).emailAccountId
    const identity = sys(() => ec.entity.find("moqui.basic.email.EmailIdentity").condition("emailAccountId", accountId).list())[0]
    return "identity:" + identity.emailIdentityId
}
const draftOf = (fromValue, clientRef, subject) => ({
    fromValue,
    clientRef,
    toJson: chips(["pat@example.org"]),
    subject,
    body: "<p>" + subject + "</p>"
})

spec({
    stepwise: true,
    setupSpec() {
        makeUser(OWNER)
        makeUser(OTHER)
        ec.user.internalLoginUser(OWNER)
        ownerIdentity = makeAccount("ref.owner@example.com")
        ec.user.internalLoginUser(OTHER)
        otherIdentity = makeAccount("ref.other@example.com")
        login("john.doe")
        johnIdentity = makeAccount("ref.john@example.com")
        capture.enable()
    },
    cleanupSpec() {
        capture.reset()
    }
})

feature("a draft saved twice with one ref is one draft, updated by the second save", {
    when() {
        ec.user.internalLoginUser(OWNER)
        const first = compose("save#ComposeDraft", draftOf(ownerIdentity, "ref-draft-1", "First version"))
        const second = compose("save#ComposeDraft", draftOf(ownerIdentity, "ref-draft-1", "Second version"))
        const bad = compose("save#ComposeDraft", draftOf(ownerIdentity, "not a ref!", "Bad"))
        // the draft opened again: its ref, so a send queued offline is taken by the same one
        const reopened = compose("get#ComposeData", {emailMessageId: first.emailMessageId})
        return {first, second, bad, reopened, found: rows({clientRef: "ref-draft-1"})}
    },
    then({first, second, bad, reopened, found}) {
        reopened.errors == ""
        reopened.compose.clientRef == "ref-draft-1"
        first.errors == ""
        second.errors == ""
        first.emailMessageId != null
        second.emailMessageId == first.emailMessageId
        second.duplicate == false
        found.length == 1
        found[0].subject == "Second version"
        found[0].statusId == "ES_DRAFT"
        bad.errors.includes("Not a valid clientRef")
    }
})

feature("an upload by ref attaches to that draft, and its retry does not attach the file again", {
    when() {
        ec.user.internalLoginUser(OWNER)
        const draftId = rows({clientRef: "ref-draft-1"})[0].emailMessageId
        const first = compose("upload#ComposeAttachment", {
            clientRef: "ref-draft-1",
            contentFile: uploadedFile("notes.txt", "Meeting notes")
        })
        const retry = compose("upload#ComposeAttachment", {
            clientRef: "ref-draft-1",
            contentFile: uploadedFile("notes.txt", "Meeting notes")
        })
        const unknown = compose("upload#ComposeAttachment", {clientRef: "ref-none", contentFile: uploadedFile("x.txt", "x")})
        return {draftId, first, retry, unknown, files: attachments(draftId)}
    },
    then({draftId, first, retry, unknown, files}) {
        first.errors == ""
        first.attachmentList.map(file => file.name).join(",") == "notes.txt"
        retry.errors == ""
        retry.attachmentList.length == 1
        files.length == 1
        files[0].attachmentLocation.includes("/" + draftId + "/")
        unknown.errors.includes("No draft has this clientRef")
    }
})

feature("a send retried with its ref sends once; the retry answers duplicate with the message's state", {
    when() {
        ec.user.internalLoginUser(OWNER)
        capture.clear()
        const first = compose(
            "send#Compose",
            Object.assign(draftOf(ownerIdentity, "ref-draft-1", "Second version"), {holdSeconds: 0})
        )
        const retry = compose(
            "send#Compose",
            Object.assign(draftOf(ownerIdentity, "ref-draft-1", "Changed on retry"), {holdSeconds: 0})
        )
        const saveAfter = compose("save#ComposeDraft", draftOf(ownerIdentity, "ref-draft-1", "Saved after the send"))
        return {first, retry, saveAfter, captured: capture.count(), found: rows({clientRef: "ref-draft-1"})}
    },
    then({first, retry, saveAfter, captured, found}) {
        first.errors == ""
        first.sent == true
        first.duplicate == false
        retry.errors == ""
        retry.duplicate == true
        retry.emailMessageId == first.emailMessageId
        retry.statusId == "ES_SENT"
        retry.sent == true
        saveAfter.errors == ""
        saveAfter.duplicate == true
        saveAfter.emailMessageId == first.emailMessageId
        captured == 1
        found.length == 1
        found[0].subject == "Second version"
    }
})

feature("a send held for undo answers its retry with the hold, and after the undo the ref sends the draft again", {
    when() {
        ec.user.internalLoginUser(OWNER)
        capture.clear()
        const held = compose("send#Compose", Object.assign(draftOf(ownerIdentity, "ref-held-1", "Held"), {holdSeconds: 300}))
        const retry = compose("send#Compose", Object.assign(draftOf(ownerIdentity, "ref-held-1", "Held"), {holdSeconds: 300}))
        const undone = compose("cancel#ComposeSend", {emailMessageId: held.emailMessageId})
        const again = compose(
            "send#Compose",
            Object.assign(draftOf(ownerIdentity, "ref-held-1", "Held, then sent"), {holdSeconds: 0})
        )
        return {held, retry, undone, again, captured: capture.count(), found: rows({clientRef: "ref-held-1"})}
    },
    then({held, retry, undone, again, captured, found}) {
        held.errors == ""
        held.statusId == "ES_SCHEDULED"
        retry.duplicate == true
        retry.statusId == "ES_SCHEDULED"
        retry.sent == false
        retry.scheduledSendDate == held.scheduledSendDate
        undone.statusId == "ES_DRAFT"
        again.errors == ""
        again.duplicate == false
        again.emailMessageId == held.emailMessageId
        again.statusId == "ES_SENT"
        captured == 1
        found.length == 1
    }
})

feature("another user's ref finds nothing of the owner's: a save makes their own draft, an upload and a send do not reach it", {
    when() {
        ec.user.internalLoginUser(OWNER)
        const owned = compose("save#ComposeDraft", draftOf(ownerIdentity, "ref-shared-name", "The owner's"))
        ec.user.internalLoginUser(OTHER)
        const upload = compose("upload#ComposeAttachment", {
            clientRef: "ref-shared-name",
            contentFile: uploadedFile("other.txt", "Other")
        })
        const saved = compose("save#ComposeDraft", draftOf(otherIdentity, "ref-shared-name", "The other's"))
        // the owner's sent message by its ref: the other user's send is a new message of their own
        const sent = compose(
            "send#Compose",
            Object.assign(draftOf(otherIdentity, "ref-draft-1", "Not a retry"), {holdSeconds: 0})
        )
        return {
            owned,
            upload,
            saved,
            sent,
            ownerFiles: attachments(owned.emailMessageId),
            ownerDraft: rows({emailMessageId: owned.emailMessageId})[0],
            ownerSent: rows({clientRef: "ref-draft-1", fromUserId: "BASIC_COMMS_REF"})
        }
    },
    then({owned, upload, saved, sent, ownerFiles, ownerDraft, ownerSent}) {
        owned.errors == ""
        upload.errors.includes("No draft has this clientRef")
        ownerFiles.length == 0
        saved.errors == ""
        saved.duplicate == false
        saved.emailMessageId != owned.emailMessageId
        ownerDraft.subject == "The owner's"
        sent.errors == ""
        sent.duplicate == false
        sent.emailMessageId != ownerSent[0].emailMessageId
        ownerSent.length == 1
    }
})

feature(
    "a failed send is not sent again by its ref; a ref of another draft is refused; an upload after the send gives the files",
    {
        when() {
            ec.user.internalLoginUser(OWNER)
            capture.clear()
            const saved = compose("save#ComposeDraft", draftOf(ownerIdentity, "ref-failed-1", "Failed"))
            // as fail#StoppedSend leaves a send whose result is not known
            sys(() => {
                const row = ec.entity.find(MESSAGE).condition("emailMessageId", saved.emailMessageId).one()
                row.setFields({statusId: "ES_READY", sendError: "The send stopped"}, true, null, null)
                row.update()
            })
            const retry = compose(
                "send#Compose",
                Object.assign(draftOf(ownerIdentity, "ref-failed-1", "Failed"), {holdSeconds: 0})
            )
            const other = compose("save#ComposeDraft", draftOf(ownerIdentity, "ref-other-1", "Other"))
            const mixed = compose(
                "save#ComposeDraft",
                Object.assign(draftOf(ownerIdentity, "ref-other-2", "Mixed"), {emailMessageId: other.emailMessageId})
            )
            const sentId = rows({clientRef: "ref-draft-1", fromUserId: "BASIC_COMMS_REF"})[0].emailMessageId
            const byId = compose(
                "save#ComposeDraft",
                Object.assign(draftOf(ownerIdentity, "ref-draft-1", "By id"), {emailMessageId: sentId})
            )
            const upload = compose("upload#ComposeAttachment", {
                clientRef: "ref-draft-1",
                contentFile: uploadedFile("late.txt", "Late")
            })
            sys(() => {
                const row = ec.entity.find(MESSAGE).condition("emailMessageId", other.emailMessageId).one()
                row.setFields({statusId: "ES_CANCELLED"}, true, null, null)
                row.update()
            })
            const cancelled = compose(
                "send#Compose",
                Object.assign(draftOf(ownerIdentity, "ref-other-1", "Other"), {holdSeconds: 0})
            )
            return {retry, mixed, byId, upload, cancelled, captured: capture.count()}
        },
        then({retry, mixed, byId, upload, cancelled, captured}) {
            retry.errors == ""
            retry.duplicate == true
            retry.sent == false
            retry.statusId == "ES_READY"
            retry.sendError == "The send stopped"
            mixed.errors.includes("has another clientRef")
            byId.errors == ""
            byId.duplicate == true
            upload.errors == ""
            upload.attachmentList.map(file => file.name).join(",") == "notes.txt"
            cancelled.duplicate == true
            cancelled.sent == false
            cancelled.statusId == "ES_CANCELLED"
            captured == 0
        }
    }
)

feature(
    "through run#Operation (IronStone's ComposeServices when installed) a draft saved twice by ref is one, and a send twice sends once",
    {
        when() {
            login("john.doe")
            capture.clear()
            const operation = (name, parameters) =>
                attempt("org.moqui.comms.InboxServices.run#Operation", {operation: name, parameters})
            const first = operation("save#ComposeDraft", draftOf(johnIdentity, "ref-op-1", "Op first"))
            const second = operation("save#ComposeDraft", draftOf(johnIdentity, "ref-op-1", "Op second"))
            const sent = operation(
                "send#Compose",
                Object.assign(draftOf(johnIdentity, "ref-op-1", "Op second"), {holdSeconds: 0})
            )
            const retry = operation(
                "send#Compose",
                Object.assign(draftOf(johnIdentity, "ref-op-1", "Op retry"), {holdSeconds: 0})
            )
            return {first, second, sent, retry, captured: capture.count(), found: rows({clientRef: "ref-op-1"})}
        },
        then({first, second, sent, retry, captured, found}) {
            first.errors == ""
            first.source == (bossInstalled ? "boss.internal" : null)
            second.result.emailMessageId == first.result.emailMessageId
            second.result.duplicate == false
            sent.errors == ""
            sent.result.sent == true
            sent.result.duplicate == false
            retry.errors == ""
            retry.result.duplicate == true
            retry.result.emailMessageId == sent.result.emailMessageId
            retry.result.statusId == "ES_SENT"
            captured == 1
            found.length == 1
            found[0].subject == "Op second"
        }
    }
)
