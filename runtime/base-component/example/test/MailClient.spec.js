// The framework's mail client (org.moqui.impl.EmailClientServices) on the real database: the SQL the message list
// writes (case-insensitive LIKE, NOT IN with nulls, the date cursor), filing and a delete forever under the foreign
// keys, a send that cannot connect waiting in the Outbox, a send that goes (through the mail capture,
// docs/Build-Test.md "Mail"), removing an account with its mail, and the sync engine's SQL (an import with its files as
// DbResource rows, rules, the queue of remote changes, the sync lock of an account whose server is not there). The
// access rules, the views and the sends in full are the Rust tests in
// crates/moqui-core/src/service/mail/client_tests.rs, and the sync against scripted servers in sync_tests.rs.

const SERVICES = "org.moqui.impl.EmailClientServices"
const base = Date.now() - 10 * 86400000
let accountId
let inboxId

const mail = (verb, params) => call(`${SERVICES}.${verb}`, params || {})
const threadIds = (out) => out.threadList.map((row) => row.threadId)
const user = (userId) => {
    if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() > 0) return
    const row = ec.entity.makeValue("moqui.security.UserAccount")
    row.setFields({ userId: userId, username: userId, emailAddress: `${userId}@example.com` }, true, null, null)
    row.create()
}
const message = (id, kind, offset, fields) => {
    const folder = kind ? ec.entity.find("moqui.basic.email.EmailFolder").condition("emailAccountId", accountId)
        .condition("folderKindEnumId", kind).one() : null
    const row = ec.entity.makeValue("moqui.basic.email.EmailMessage")
    row.setFields(Object.assign({ emailMessageId: id, emailAccountId: accountId, emailFolderId: folder ? folder.emailFolderId : null,
        statusId: "ES_RECEIVED", subject: `Subject ${id}`, body: `<p>Body of ${id}</p>`, snippet: `Body of ${id}`,
        fromAddress: `${id.toLowerCase()}@sender.example.com`, toAddresses: "team@example.com", messageDate: new Date(base + offset * 1000),
        isRead: "N" }, fields || {}), true, null, null)
    row.create()
}

// the search index has an account's messages now, rather than when the data feed's worker gets to them
const indexMail = (accountIds) => {
    if (!ec.search.isAvailable()) return false
    ec.search.ensureIndex({ documentType: "MoquiEmailMessage" })
    for (const id of accountIds) ec.search.indexDocuments(ec.entity.getDataDocuments("MoquiEmailMessage", { emailAccountId: id }))
    return true
}

spec({
    setup() { ec.artifactExecution.disableAuthz() },
    cleanup() { ec.artifactExecution.enableAuthz() },
})

// sign-in with a provider (docs/interfaces/Mail-OAuth.md): the seeded providers, not set up without a client, and the webroot
// callback page refusing a state its session did not start; the flow itself is oauth_tests.rs against a fake provider
feature("sign-in with a provider: the seeded providers and the callback page", {
    requires: ["imap"],
    when() {
        user("mailClientOwner")
        ec.user.internalLoginUser("mailClientOwner")
        const providers = mail("get#OAuthProviders").providerList.map((row) => `${row.oauthProviderEnumId}:${row.configured}`)
        const google = ec.entity.find("moqui.basic.email.EmailOAuthProvider").condition("oauthProviderEnumId", "EopGoogle").one()
        const render = screenTest("emailOAuth").render("callback", { state: "not-a-state-of-this-session", code: "x" }, "get")
        const page = render.output || ""
        return { providers, googleScope: google ? google.mailScope : null, status: render.status, page }
    },
    then({ providers, googleScope, status, page }) {
        providers.includes("EopGoogle:false")
        providers.includes("EopMicrosoft:false")
        googleScope == "https://mail.google.com/"
        status == 200
        page.includes("The sign-in did not finish")
        page.includes("\"errorCode\":\"invalidState\"")
        page.includes("window.opener.postMessage(m,o)")
    },
})

feature("an account with its folders, and the message list in threads with its cursor", {
    when() {
        for (const id of ["mailClientOwner", "mailClientStranger"]) user(id)
        ec.user.internalLoginUser("mailClientOwner")
        // no server listens on port 1, so a send fails at once
        accountId = mail("create#EmailAccount", { accountKindEnumId: "EakShared", emailAddress: "team@example.com", accountName: "Team",
            smtpHost: "127.0.0.1", smtpPort: "1", smtpStartTls: "N" }).emailAccountId
        inboxId = ec.entity.find("moqui.basic.email.EmailFolder").condition("emailAccountId", accountId)
            .condition("folderKindEnumId", "EfkInbox").one().emailFolderId
        message("MCM1", "EfkInbox", 1, { messageId: "<mcm1@sender.example.com>" })
        message("MCM2", "EfkInbox", 30, { rootEmailMessageId: "MCM1", parentEmailMessageId: "MCM1" })
        message("MCM3", "EfkInbox", 20, { isFlagged: "Y", subject: "Invoice 42" })
        message("MCM4", "EfkInbox", 40, { snoozedThruDate: new Date(Date.now() + 86400000) })
        message("MCM5", "EfkTrash", 50)
        message("MCM6", null, 60, { statusId: "ES_SCHEDULED", scheduledSendDate: new Date(Date.now() + 86400000), isRead: "Y" })
        indexMail([accountId])
        const inbox = mail("find#Messages")
        const page = mail("find#Messages", { pageSize: 1 })
        const next = mail("find#Messages", { pageSize: 1, beforeDate: page.nextBeforeDate })
        const accounts = mail("get#UserEmailAccounts").accountList
        return { inbox, page, next, accounts,
            search: mail("find#Messages", { searchText: "INVOICE", threadMode: false }),
            flagged: mail("find#Messages", { view: "flagged" }), all: mail("find#Messages", { view: "all" }),
            scheduled: mail("find#Messages", { view: "outbox" }), snoozed: mail("find#Messages", { view: "snoozed" }),
            unread: mail("count#UnreadEmail") }
    },
    then({ inbox, page, next, accounts, search, flagged, all, scheduled, snoozed, unread }) {
        threadIds(inbox) == ["MCM1", "MCM3"]
        inbox.threadList[0].messageCount == 2
        inbox.threadList[0].store == "email"
        page.hasMore == true
        threadIds(next) == ["MCM3"]
        accounts.length == 2
        accounts[0].accountKindEnumId == "EakInternal"
        accounts[1].folderList.length == 6
        accounts[1].unreadCount == 3
        search.threadList.map((row) => row.emailMessageId) == ["MCM3"]
        threadIds(flagged) == ["MCM3"]
        threadIds(all) == ["MCM6", "MCM4", "MCM1", "MCM3"]
        threadIds(scheduled) == ["MCM6"]
        threadIds(snoozed) == ["MCM4"]
        unread.unreadCount == 3
        unread.unreadThreadCount == 2
    },
})

feature("a stranger reaches nothing", {
    when() {
        ec.user.internalLoginUser("mailClientStranger")
        const access = mail("get#EmailAccountAccess", { emailAccountId: accountId })
        ec.service.sync().name(`${SERVICES}.get#Thread`).parameter("emailMessageId", "MCM1").call()
        const refused = ec.message.errorsString
        ec.message.clearErrors()
        return { access, refused }
    },
    then({ access, refused }) {
        access.level == 0
        refused.includes("not found")
    },
})

feature("filing, and a delete forever that keeps the rest of the thread", {
    when() {
        ec.user.internalLoginUser("mailClientOwner")
        mail("set#MessageFlags", { rootEmailMessageId: "MCM1", read: true })
        const folderId = mail("create#EmailFolder", { emailAccountId: accountId, folderName: "Clients" }).emailFolderId
        mail("move#Messages", { emailMessageId: "MCM3", emailFolderId: folderId })
        const labelId = mail("create#EmailLabel", { emailAccountId: accountId, labelName: "Follow up" }).emailLabelId
        mail("apply#Labels", { rootEmailMessageId: "MCM1", addLabelIdList: [labelId] })
        mail("archive#Messages", { rootEmailMessageId: "MCM1" })
        const archived = mail("find#Messages", { view: "archive" })
        mail("archive#Messages", { rootEmailMessageId: "MCM1", unarchive: true })
        mail("trash#Messages", { emailMessageId: "MCM1" })
        const deleted = mail("trash#Messages", { emailMessageId: "MCM1", deleteForever: true })
        const reply = ec.entity.find("moqui.basic.email.EmailMessage").condition("emailMessageId", "MCM2").one()
        return { archived, deleted, reply, gone: ec.entity.find("moqui.basic.email.EmailMessage").condition("emailMessageId", "MCM1").count(),
            labeled: mail("find#Messages", { emailLabelId: labelId }), unread: mail("count#UnreadEmail") }
    },
    then({ archived, deleted, reply, gone, labeled, unread }) {
        threadIds(archived) == ["MCM1"]
        deleted.deletedCount == 1
        gone == 0
        reply.rootEmailMessageId == null
        reply.parentEmailMessageId == null
        threadIds(labeled) == ["MCM2"]
        unread.unreadCount == 0
    },
})

feature("a send that cannot connect waits in the Outbox, and goes back to Drafts", {
    when() {
        // a real connection, to port 1, where nothing listens: the capture is off for this send, whatever the run says
        const capture = ec.factory.getTool("MailCapture")
        capture.disable()
        try {
            const draftId = mail("save#Draft", { emailAccountId: accountId, parentEmailMessageId: "MCM2", toAddresses: "\"Sender\" <mcm2@sender.example.com>",
                body: "<p>Thanks</p>" }).emailMessageId
            const sent = mail("send#Draft", { emailMessageId: draftId })
            const outbox = mail("find#Messages", { view: "outbox", threadMode: false })
            mail("cancel#ScheduledSend", { emailMessageId: draftId })
            const draft = ec.entity.find("moqui.basic.email.EmailMessage").condition("emailMessageId", draftId).one()
            return { sent, outbox, draft, draftId }
        } finally {
            capture.reset()
        }
    },
    then({ sent, outbox, draft, draftId }) {
        sent.sent == false
        sent.statusId == "ES_READY"
        sent.sendError != null
        outbox.threadList.some((row) => row.emailMessageId == draftId && row.sendError != null)
        draft.statusId == "ES_DRAFT"
        draft.subject == "Re: Subject MCM2"
        draft.inReplyTo == null
    },
})

feature("a send goes: kept by the mail capture instead of connecting, and sent with its status", {
    when() {
        const capture = ec.factory.getTool("MailCapture")
        capture.enable()
        try {
            capture.clear()
            const draftId = mail("save#Draft", { emailAccountId: accountId, parentEmailMessageId: "MCM2", toAddresses: "\"Sender\" <mcm2@sender.example.com>",
                subject: "Re: the plan · v2", body: "<p>Agreed</p>" }).emailMessageId
            const sent = mail("send#Draft", { emailMessageId: draftId })
            const row = ec.entity.find("moqui.basic.email.EmailMessage").condition("emailMessageId", draftId).one()
            return { enabled: capture.isEnabled(), sent, row, captured: capture.getMessages() }
        } finally {
            capture.reset()
        }
    },
    then({ enabled, sent, row, captured }) {
        enabled == true
        sent.sent == true
        sent.statusId == "ES_SENT"
        row.statusId == "ES_SENT"
        row.sentDate != null
        captured.length == 1
        captured[0].subject == "Re: the plan · v2"
        captured[0].toList.length == 1
        captured[0].toList[0].includes("mcm2@sender.example.com")
        captured[0].body.includes("Agreed")
    },
})

feature("removing an account deletes its mail, folders, labels and server", {
    when() {
        const serverId = ec.entity.find("moqui.basic.email.EmailAccount").condition("emailAccountId", accountId).one().emailServerId
        const out = mail("delete#EmailAccount", { emailAccountId: accountId, deleteLocalCopies: true })
        return { out, serverId,
            messages: ec.entity.find("moqui.basic.email.EmailMessage").condition("emailAccountId", accountId).count(),
            folders: ec.entity.find("moqui.basic.email.EmailFolder").condition("emailAccountId", accountId).count(),
            account: ec.entity.find("moqui.basic.email.EmailAccount").condition("emailAccountId", accountId).count() }
    },
    then({ out, serverId, messages, folders, account }) {
        out.deletedMessageCount == 7
        out.keptMessageCount == 0
        messages == 0
        folders == 0
        account == 0
        ec.entity.find("moqui.basic.email.EmailServer").condition("emailServerId", serverId).count() == 0
    },
})

const MBOX = `From billing@example.com Mon Oct 19 10:00:00 2026
Message-ID: <mbox1@example.com>
From: billing@example.com
To: local@example.com
Subject: Invoice 7
Date: Mon, 19 Oct 2026 10:00:00 +0000
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="b1"

--b1
Content-Type: text/plain

Your invoice is attached.
--b1
Content-Type: text/plain; name="invoice.txt"
Content-Disposition: attachment; filename="invoice.txt"

Total 42
--b1--
From friend@example.com Mon Oct 19 11:00:00 2026
Message-ID: <mbox2@example.com>
From: friend@example.com
To: local@example.com
Subject: Lunch
Date: Mon, 19 Oct 2026 11:00:00 +0000
Status: O

Lunch on Friday?
`

feature("an import, rules, the remote change queue and a sync whose server is not there, on the real database", {
    requires: ["imap"],
    when() {
        ec.user.internalLoginUser("mailClientOwner")
        const local = mail("create#EmailAccount", { emailAddress: "local@example.com", accountName: "Local" }).emailAccountId
        const inbox = ec.entity.find("moqui.basic.email.EmailFolder").condition("emailAccountId", local)
            .condition("folderKindEnumId", "EfkInbox").one().emailFolderId
        const imported = mail("import#EmailMessages", { emailFolderId: inbox, messageText: MBOX })
        const again = mail("import#EmailMessages", { emailFolderId: inbox, messageText: MBOX })
        const invoice = ec.entity.find("moqui.basic.email.EmailMessage").condition("messageId", "<mbox1@example.com>").one()
        const files = ec.entity.find("moqui.basic.email.EmailMessageAttachment").condition("emailMessageId", invoice.emailMessageId).list()
        const fileText = files.length ? ec.resource.getLocationReference(files[0].attachmentLocation).getText() : null
        const ruleId = mail("create#EmailRule", { ruleName: "Invoices", emailAccountId: local,
            conditionList: [{ field: "subject", operator: "contains", value: "INVOICE" }, { field: "hasAttachment", value: "true" }],
            actionList: [{ action: "addLabel", value: "Billing" }, { action: "flag" }] }).emailRuleId
        const ran = mail("run#EmailRules", { emailFolderId: inbox })
        const rules = mail("find#EmailRules")
        const labels = ec.entity.find("moqui.basic.email.EmailMessageLabel").condition("emailMessageId", invoice.emailMessageId).count()
        // an IMAP account whose server is not there: its changes wait in the queue, and its sync says why
        const remote = mail("create#EmailAccount", { emailAddress: "remote@example.com", storeHost: "127.0.0.1", storePort: "1",
            storeProtocol: "imap", mailPassword: "secret" }).emailAccountId
        const remoteInbox = ec.entity.find("moqui.basic.email.EmailFolder").condition("emailAccountId", remote)
            .condition("folderKindEnumId", "EfkInbox").one().emailFolderId
        const kept = ec.entity.makeValue("moqui.basic.email.EmailMessage")
        kept.setFields({ emailMessageId: "MCR1", emailAccountId: remote, emailFolderId: remoteInbox, remoteFolderId: remoteInbox,
            remoteUid: 7, statusId: "ES_RECEIVED", subject: "Remote", messageId: "<mcr1@example.com>", isRead: "N",
            messageDate: new Date() }, true, null, null)
        kept.create()
        mail("set#MessageFlags", { emailMessageId: "MCR1", read: true })
        const queued = ec.entity.find("moqui.basic.email.EmailRemoteChange").condition("emailAccountId", remote).list()
        // the push the change started after its commit may hold the account's sync lock for a moment
        let synced = null
        for (let attempt = 0; attempt < 1000 && (synced == null || synced.outcome == "busy"); attempt++)
            synced = mail("sync#EmailAccount", { emailAccountId: remote })
        const account = ec.entity.find("moqui.basic.email.EmailAccount").condition("emailAccountId", remote).one()
        const due = mail("sync#DueEmailAccounts")
        return { imported, again, files, fileText, ran, rules, ruleId, labels, queued, synced, account, due,
            flagged: ec.entity.find("moqui.basic.email.EmailMessage").condition("emailAccountId", local).condition("isFlagged", "Y").count(),
            stillQueued: ec.entity.find("moqui.basic.email.EmailRemoteChange").condition("emailAccountId", remote).count() }
    },
    then({ imported, again, files, fileText, ran, rules, ruleId, labels, queued, synced, account, due, flagged, stillQueued }) {
        imported.importedCount == 2
        again.importedCount == 0
        again.duplicateCount == 2
        files.length == 1
        files[0].fileName == "invoice.txt"
        fileText.includes("Total 42")
        ran.matchedCount == 1
        flagged == 1
        labels == 1
        rules.ruleList.some((rule) => rule.emailRuleId == ruleId && rule.matchedCount == 1)
        queued.length == 1
        queued[0].changeType == "flags"
        synced.outcome == "failed"
        synced.message.includes("could not connect")
        account.statusId == "EasError"
        account.syncLockedUntil == null
        due.failedCount >= 0
        stillQueued == 1
    },
})

// The search document on the real database (data/MailSearchData.xml): the free text of the search line is searched in the
// MoquiEmailMessage index of the search backend (Postgres here), every word somewhere in the message, the last one as a
// prefix, a "phrase" as it is and a -word out; the operators are conditions; and only the accounts the user reaches are
// searched, so another user's mail with the same words is never found. A message deleted forever leaves the index.
feature("the search line through the search index, only in the user's own accounts", {
    when() {
        ec.user.internalLoginUser("mailClientStranger")
        const strangerAccount = mail("create#EmailAccount", { emailAddress: "stranger@example.com", smtpHost: "127.0.0.1", smtpPort: "1" }).emailAccountId
        ec.user.internalLoginUser("mailClientOwner")
        const own = mail("create#EmailAccount", { emailAddress: "owner@example.com", smtpHost: "127.0.0.1", smtpPort: "1" }).emailAccountId
        const save = (id, account, fields) => {
            const folder = ec.entity.find("moqui.basic.email.EmailFolder").condition("emailAccountId", account)
                .condition("folderKindEnumId", "EfkInbox").one()
            const row = ec.entity.makeValue("moqui.basic.email.EmailMessage")
            row.setFields(Object.assign({ emailMessageId: id, emailAccountId: account, emailFolderId: folder.emailFolderId, statusId: "ES_RECEIVED",
                toAddresses: "owner@example.com", messageDate: new Date(), isRead: "N" }, fields), true, null, null)
            row.create()
        }
        save("MCS1", own, { subject: "Quarterly zeppelin forecast", bodyText: "The numbers for the airship fleet", fromAddress: "ada@math.example.com",
            fromName: "Ada Lovelace", messageDate: new Date(Date.now() - 3000) })
        save("MCS2", own, { subject: "Lunch", bodyText: "Zeppelins are late again", fromAddress: "bob@example.com", messageDate: new Date(Date.now() - 2000) })
        save("MCS3", own, { subject: "Hangar plans", bodyText: "Airship hangar", fromAddress: "carol@example.com", hasAttachment: "Y",
            messageDate: new Date(Date.now() - 1000) })
        save("MCS4", strangerAccount, { subject: "Zeppelin secrets", bodyText: "Zeppelin airship numbers", fromAddress: "spy@example.com" })
        const indexed = indexMail([own, strangerAccount])
        const find = (text) => mail("find#Messages", { view: "all", threadMode: false, searchText: text })
        const ids = (out) => out.threadList.map((row) => row.emailMessageId)
        const zeppelin = find("zeppelin")
        const results = { indexed, mode: zeppelin.searchMode, zeppelin: ids(zeppelin), prefix: ids(find("zepp")), both: ids(find("airship numbers")),
            phrase: ids(find('"airship fleet"')), excluded: ids(find("airship -hangar")), from: ids(find("from:ada airship")),
            attachment: ids(find("airship has:attachment")), byName: ids(find("lovelace")) }
        // a message deleted forever leaves the index
        mail("trash#Messages", { emailMessageId: "MCS2" })
        mail("trash#Messages", { emailMessageId: "MCS2", deleteForever: true })
        ec.search.processFeedPending()
        results.deletedDocument = ec.search.getDocument("MoquiEmailMessage", "MCS2")
        results.keptDocument = ec.search.getDocument("MoquiEmailMessage", "MCS1")
        // the stranger finds only theirs
        ec.user.internalLoginUser("mailClientStranger")
        results.stranger = ids(find("zeppelin"))
        ec.user.internalLoginUser("mailClientOwner")
        return results
    },
    then(results) {
        results.indexed == true
        results.mode == "index"
        results.zeppelin == ["MCS2", "MCS1"]
        results.prefix == ["MCS2", "MCS1"]
        results.both == ["MCS1"]
        results.phrase == ["MCS1"]
        results.excluded == ["MCS1"]
        results.from == ["MCS1"]
        results.attachment == ["MCS3"]
        results.byName == ["MCS1"]
        results.deletedDocument == null
        results.keptDocument != null
        results.stranger == ["MCS4"]
    },
})
