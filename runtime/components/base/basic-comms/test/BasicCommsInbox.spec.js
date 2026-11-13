// The BasicComms Inbox, Compose and Home over the user's mail alone (org.moqui.comms.InboxServices, ComposeServices and
// CommsHomeServices; plans/archive/Comms-Framework.md A3): a user with no party and a mail account of their own sees its Inbox, a
// thread, its details and its Home counter, acts on the thread, and writes, keeps and discards a reply; no internal
// messages or mailboxes are offered. run#Operation hands the operations to an app's inbox-source when the user may open
// one (IronStone's, when it is installed), and to these services otherwise. Mail renders the message-actions slot and hands a
// message to the apps' create-from providers (createRecord).
// Run: ./moq test basic-comms --spec BasicCommsInbox
const USER = "BASIC_COMMS_INBOX"
const CLIENT = "BASIC_COMMS_INBOX_CLIENT"
const ACCOUNT = "BASIC_COMMS_INBOX_ACCT"
const FOLDER = "BASIC_COMMS_INBOX_INBOX"
const MESSAGE = "BASIC_COMMS_INBOX_M1"
const bossInstalled = !!ec.resource.getLocationReference("component://mr-boss/screen/Comms.xml")?.getExists()

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
const inbox = (name, parameters) => attempt("org.moqui.comms.InboxServices." + name, parameters)
const compose = (name, parameters) => attempt("org.moqui.comms.ComposeServices." + name, parameters)
function store(entityName, fields) {
    sys(() => {
        const value = ec.entity.makeValue(entityName)
        value.setFields(fields, true, null, null)
        value.createOrUpdate()
    })
}
function makeUser(userId, username, groupId) {
    store("moqui.security.UserAccount", {
        userId,
        username,
        userFullName: "Inbox " + username,
        emailAddress: username + "@example.com",
        disabled: "N"
    })
    if (groupId)
        sys(() => {
            if (ec.entity.find("moqui.security.UserGroup").condition("userGroupId", groupId).count() == 0) return
            if (
                ec.entity
                    .find("moqui.security.UserGroupMember")
                    .condition("userGroupId", groupId)
                    .condition("userId", userId)
                    .count() > 0
            )
                return
            const member = ec.entity.makeValue("moqui.security.UserGroupMember")
            member.setFields({userGroupId: groupId, userId, fromDate: ec.user.nowTimestamp}, true, null, null)
            member.create()
        })
}

spec({
    stepwise: true,
    setupSpec() {
        makeUser(USER, "basic.comms.inbox", null)
        makeUser(CLIENT, "basic.comms.inbox.client", "BOSS_PROJECT_CLIENT")
        // a mail account delivered here (no server), its Inbox and one message in it
        store("moqui.basic.email.EmailAccount", {
            emailAccountId: ACCOUNT,
            ownerUserId: USER,
            accountKindEnumId: "EakHosted",
            emailAddress: "inbox.spec@example.com",
            accountName: "Spec mail",
            statusId: "EasActive"
        })
        store("moqui.basic.email.EmailFolder", {
            emailFolderId: FOLDER,
            emailAccountId: ACCOUNT,
            folderName: "Inbox",
            folderPath: "INBOX",
            folderKindEnumId: "EfkInbox",
            subscribed: "Y",
            sequenceNum: 1
        })
        store("moqui.basic.email.EmailMessage", {
            emailMessageId: MESSAGE,
            rootEmailMessageId: MESSAGE,
            emailAccountId: ACCOUNT,
            emailFolderId: FOLDER,
            statusId: "ES_RECEIVED",
            fromAddress: "pat@example.org",
            fromName: "Pat Sender",
            toAddresses: "inbox.spec@example.com",
            subject: "Lunch on Friday",
            body: "<p>Are you free for lunch on Friday?</p>",
            contentType: "text/html",
            snippet: "Are you free for lunch on Friday?",
            messageDate: ec.user.nowTimestamp,
            receivedDate: ec.user.nowTimestamp,
            isRead: "N",
            messageId: "<basic-comms-inbox-1@example.org>"
        })
    }
})

feature("the rail, a page of the Inbox and the thread of the user's mail, with no internal messages or mailboxes", {
    when() {
        ec.user.internalLoginUser("basic.comms.inbox")
        const data = inbox("get#InboxData")
        const rows = inbox("find#InboxRows", {view: "inbox"})
        const thread = inbox("get#ThreadView", {store: "email", threadId: MESSAGE})
        const internal = inbox("get#ThreadView", {store: "internal", threadId: "X"})
        const mailbox = inbox("find#InboxRows", {contactListId: "ANY"})
        const detail = inbox("get#MessageDetail", {emailMessageId: MESSAGE})
        return {data, rows, thread, internal, mailbox, detail}
    },
    then({data, rows, thread, internal, mailbox, detail}) {
        data.errors == ""
        data.accountList.map(account => account.emailAccountId).includes(ACCOUNT)
        data.accountList.every(account => !account.isInternal)
        data.mailboxList.length == 0
        rows.errors == ""
        rows.threadList.map(row => row.rowKey).includes("email:" + MESSAGE)
        rows.storeList.join(",") == "email"
        thread.errors == ""
        thread.thread.messageList[0].subject == "Lunch on Friday"
        thread.thread.messageList[0].from.address == "pat@example.org"
        internal.errors.includes("Unknown store")
        mailbox.errors.includes("Mailboxes are not available here")
        detail.errors == ""
        detail.detail.subject == "Lunch on Friday"
    }
})

feature("an action on the thread, and Home counts the account", {
    when() {
        ec.user.internalLoginUser("basic.comms.inbox")
        const flagged = inbox("set#ThreadState", {targetList: [{store: "email", threadId: MESSAGE}], action: "flag"})
        const read = inbox("set#ThreadState", {targetList: [{store: "email", threadId: MESSAGE}], action: "read"})
        const internalAction = inbox("set#ThreadState", {targetList: [{store: "internal", threadId: "X"}], action: "read"})
        const row = sys(() => ec.entity.find("moqui.basic.email.EmailMessage").condition("emailMessageId", MESSAGE).one())
        const home = attempt("org.moqui.comms.CommsHomeServices.get#CommsHome")
        return {flagged, read, internalAction, row, home}
    },
    then({flagged, read, internalAction, row, home}) {
        flagged.updatedCount == 1
        read.updatedCount == 1
        internalAction.failedList.length == 1
        row.isFlagged == "Y"
        row.isRead == "Y"
        home.hasMailAccount == true
        home.cardList.map(card => card.key).includes("account_" + ACCOUNT)
        home.cardList.every(card => card.key !== "internal")
        home.cardList.find(card => card.key === "account_" + ACCOUNT).link.includes("/Inbox?account=")
    }
})

feature("Needs reply keeps the mail a day old or more that is not answered and has no reply sent after it, on Home and as an Inbox filter", {
    when() {
        const daysAgo = (days) => new Date(Date.now() - days * 86400000)
        const message = (id, fields) => store("moqui.basic.email.EmailMessage", Object.assign({emailMessageId: id, rootEmailMessageId: id,
            emailAccountId: ACCOUNT, emailFolderId: FOLDER, statusId: "ES_RECEIVED", fromAddress: "lee@example.org", fromName: "Lee Asker",
            toAddresses: "inbox.spec@example.com", contentType: "text/plain", isRead: "Y", messageId: "<" + id + "@example.org>"}, fields))
        // waiting: three days old, not answered
        message("BCI_NR_WAIT", {subject: "Quote for the fence", body: "Can you send a quote?", messageDate: daysAgo(3), receivedDate: daysAgo(3)})
        // answered (the client marked it)
        message("BCI_NR_ANSWERED", {subject: "Answered already", body: "x", messageDate: daysAgo(3), receivedDate: daysAgo(3), isAnswered: "Y"})
        // a reply was sent in the thread after it
        message("BCI_NR_REPLIED", {subject: "Replied in thread", body: "x", messageDate: daysAgo(4), receivedDate: daysAgo(4)})
        message("BCI_NR_REPLY", {rootEmailMessageId: "BCI_NR_REPLIED", parentEmailMessageId: "BCI_NR_REPLIED", statusId: "ES_SENT",
            fromAddress: "inbox.spec@example.com", toAddresses: "lee@example.org", subject: "Re: Replied in thread", body: "y",
            messageDate: daysAgo(2), sentDate: daysAgo(2), emailFolderId: null})
        ec.user.internalLoginUser("basic.comms.inbox")
        const rows = inbox("find#InboxRows", {view: "inbox", needsReply: true, pageSize: 10})
        const home = attempt("org.moqui.comms.CommsHomeServices.get#CommsHome")
        const shown = sys(() => ec.screen.makeTest().rootScreen("component://basic-comms/screen/BasicComms.xml").renderMode("tui").render("Home", {}, null))
        return {rows, home, text: String(shown.output || "")}
    },
    then({rows, home, text}) {
        rows.errors == ""
        rows.threadList.map(row => row.rowKey).join() == "email:BCI_NR_WAIT"
        home.needsReplyList.map(row => row.rowKey).join() == "email:BCI_NR_WAIT"
        home.needsReplyList[0].who == "Lee Asker"
        home.needsReplyList[0].link.endsWith("/Inbox?thread=email%3ABCI_NR_WAIT")
        text.includes("Needs reply")
        text.includes("Quote for the fence")
        !text.includes("Lunch on Friday")
    }
})

feature("a reply starts from the account it came to, is kept as a draft and discarded; other kinds of recipients are refused", {
    when() {
        ec.user.internalLoginUser("basic.comms.inbox")
        const started = compose("get#ComposeData", {replyToEmailMessageId: MESSAGE})
        const fromValue = started.compose && started.compose.fromValue
        const saved = compose("save#ComposeDraft", {
            fromValue,
            toJson: JSON.stringify(started.compose.to),
            subject: started.compose.subject,
            body: "<p>Yes, see you then.</p>",
            quoteHtml: started.compose.quoteHtml,
            parentEmailMessageId: MESSAGE
        })
        const party = compose("resolve#Recipients", {chipList: [{value: "party:SOMEONE", label: "Someone"}], mode: "email"})
        const internal = compose("send#Compose", {fromValue: "internal", toJson: "[]", body: "<p>x</p>"})
        const discarded = compose("discard#ComposeDraft", {emailMessageId: saved.emailMessageId})
        const users = compose("search#Recipients", {term: "basic.comms.inbox.client"})
        return {started, saved, party, internal, discarded, users}
    },
    then({started, saved, party, internal, discarded, users}) {
        started.errors == ""
        started.compose.kind == "reply"
        started.compose.subject == "Re: Lunch on Friday"
        started.compose.to[0].value == '"Pat Sender" <pat@example.org>'
        started.compose.identityList.every(identity => !identity.isInternal)
        saved.errors == ""
        saved.emailMessageId != null
        party.errors.includes("is not an email address")
        internal.errors.includes("Internal messages are not available here")
        discarded.errors == ""
        users.optionList.some(option => option.value.includes("basic.comms.inbox.client@example.com"))
    }
})

feature("the operations go to the app's inbox-source when the user may open it, else to the framework's mail alone", {
    when() {
        // a user IronStone denies its Comms (a project client) gets the framework's answer
        ec.user.internalLoginUser("basic.comms.inbox.client")
        const client = attempt("org.moqui.comms.InboxServices.run#Operation", {operation: "get#InboxData", parameters: {}})
        const app = bossInstalled ? null : client
        login("john.doe")
        const john = attempt("org.moqui.comms.InboxServices.run#Operation", {operation: "get#InboxData", parameters: {}})
        const unknown = attempt("org.moqui.comms.InboxServices.run#Operation", {operation: "drop#Everything", parameters: {}})
        return {client, john, unknown, app}
    },
    then({client, john, unknown}) {
        client.errors == ""
        !client.source
        client.result.mailboxList.length == 0
        john.errors == ""
        john.source == (bossInstalled ? "boss.internal" : null)
        unknown.errors.includes("Unknown Comms operation")
    }
})

// The home-source and contact-card hooks (plans/archive/Comms-Framework.md §3.3) over the example gallery's providers: its Home box,
// one failing on purpose and named in failedParts, and its contact card for its own addresses; an address nobody knows
// is the address alone. Skipped when the example component is not loaded.
const exampleInstalled = ec.resource.getLocationReference("component://example/screen/Gallery.xml").getExists()
feature("Home adds the apps' home-source boxes and leaves out one that fails; the contact card is a provider's, else the address alone", {
    when() {
        if (!exampleInstalled) return {skip: true}
        login("john.doe")
        const sources = attempt("org.moqui.comms.CommsHomeServices.get#HomeSources")
        const known = inbox("get#ContactCard", {emailAddress: "pat@gallery.example", name: "Pat"})
        const unknown = inbox("get#ContactCard", {emailAddress: "nobody@unknown.example", name: "Nobody Known"})
        return {sources, known, unknown}
    },
    then({skip, sources, known, unknown}) {
        skip || sources.cardList.some(card => card.sourceId == "gallery.home" && card.title == "Gallery notes" && card.rowList[0].title == "Gallery home row")
        skip || sources.failedParts.includes("gallery.homeFailing")
        skip || !sources.cardList.some(card => card.sourceId == "gallery.homeFailing")
        skip || known.card.name == "Gallery contact"
        skip || known.card.url == "/qapps/gallery/Widgets"
        skip || unknown.card.emailAddress == "nobody@unknown.example"
        skip || unknown.card.name == "Nobody Known"
        skip || unknown.card.url == null
    }
})

feature("the framework reads a message's invitation; with no invitation provider of an app the user may open it has no answers and its answer is refused", {
    when() {
        const location = "dbresource://email/spec/BCI_INVITE/invite.ics"
        sys(() => ec.resource.getLocationReference(location).putText(["BEGIN:VCALENDAR", "VERSION:2.0", "METHOD:REQUEST", "BEGIN:VEVENT",
            "UID:bci-invite@example.org", "DTSTART:20261201T150000Z", "DTEND:20261201T160000Z", "SUMMARY:Fence walk-through",
            "LOCATION:Back yard", "ORGANIZER;CN=Lee Asker:mailto:lee@example.org", "END:VEVENT", "END:VCALENDAR", ""].join("\r\n")))
        store("moqui.basic.email.EmailMessage", {emailMessageId: "BCI_INVITE", rootEmailMessageId: "BCI_INVITE", emailAccountId: ACCOUNT,
            emailFolderId: FOLDER, statusId: "ES_RECEIVED", fromAddress: "lee@example.org", fromName: "Lee Asker", toAddresses: "inbox.spec@example.com",
            subject: "Invitation: Fence walk-through", body: "Please come", contentType: "text/plain", isRead: "Y", messageId: "<bci-invite@example.org>",
            messageDate: ec.user.nowTimestamp, receivedDate: ec.user.nowTimestamp})
        store("moqui.basic.email.EmailMessageAttachment", {emailMessageAttachmentId: "BCI_INVITE_ICS", emailMessageId: "BCI_INVITE",
            attachmentLocation: location, fileName: "invite.ics", fileSize: 300, contentType: "text/calendar"})
        ec.user.internalLoginUser("basic.comms.inbox")
        const thread = inbox("get#ThreadView", {store: "email", threadId: "BCI_INVITE"})
        const answer = attempt("org.moqui.comms.InboxServices.run#Operation", {operation: "respond#Invitation",
            parameters: {emailMessageId: "BCI_INVITE", response: "accept"}})
        return {thread, answer}
    },
    then({thread, answer}) {
        thread.errors == ""
        thread.thread.messageList[0].invitation.summary == "Fence walk-through"
        thread.thread.messageList[0].invitation.location == "Back yard"
        thread.thread.messageList[0].invitation.method == "REQUEST"
        thread.thread.messageList[0].invitation.canRespond == false
        answer.errors.includes("not available here")
    }
})

// The message-actions slot (impl-sales P5, the user's call of Nov 13): Mail.qvue renders what apps add to a message in both of its More
// menus (the thread's, for its last message, and each message's), with the message's store and ids as the item; Mail's createRecord
// hands the message to the create-from providers of the apps the user may open, so a user no app answers that hook for is told so
// and nothing is made; and an app's ext-action on the slot is in Mail's ext items (IronStone's, when it is installed).
function render(path, parameters, method) {
    const result = ec.screen.makeTest().rootScreen("component://basic-comms/screen/BasicComms.xml").render(path, parameters || {}, method || null)
    let json = null
    try {
        json = JSON.parse(String(result.output || ""))
    } catch (error) {
        json = null
    }
    const errors = Array.from(result.errorMessages || []).join(" | ")
    ec.message.clearErrors()
    return {status: result.status, errors, json: json || {}}
}
feature("Mail renders the message-actions slot in a message's menus, and createRecord asks the apps' create-from providers", {
    when() {
        const source = ec.resource.getLocationText("component://basic-comms/screen/BasicComms/Inbox/Mail.qvue", false)
        const slots = source.match(/<m-ext-actions[^>]*slot="message-actions"[^>]*>/g) || []
        // a user who may open no app with a create-from provider: the framework's screens alone
        ec.user.internalLoginUser("basic.comms.inbox")
        const none = sys(() => render("Inbox/Mail/createRecord", {kind: "lead", emailMessageId: MESSAGE}, "post"))
        login("john.doe")
        const items = bossInstalled
            ? JSON.parse(String(ec.screen.makeTest().rootScreen("component://webroot/screen/webroot.xml").render("extItems/apps/BasicComms/Inbox/Mail", {}, null).output || "{}"))
            : null
        ec.message.clearErrors()
        return {slots, none, items}
    },
    then({slots, none, items}) {
        slots.length == 2
        slots.every(tag => tag.includes("emailMessageId") && tag.includes("communicationEventId") && tag.includes("store"))
        none.errors.includes("Nothing here makes records from messages")
        none.json.recordId == null
        items == null || items.actions.some(action => action.slot == "message-actions" && action.kind == "transition" && action.url.endsWith("/createRecord")
            && JSON.stringify(action.parameters).includes("emailMessageId"))
    }
})
