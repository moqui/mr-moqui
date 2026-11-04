// The BasicComms Inbox, Compose and Home over the user's mail alone (org.moqui.comms.InboxServices, ComposeServices and
// CommsHomeServices; plans/Comms-Framework.md A3): a user with no party and a mail account of their own sees its Inbox, a
// thread, its details and its Home counter, acts on the thread, and writes, keeps and discards a reply; no internal
// messages or mailboxes are offered. run#Operation hands the operations to an app's inbox-source when the user may open
// one (IronStone's, when it is installed), and to these services otherwise.
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
