// The mail part of org.moqui.admin.OperationServices (mr-boss plan/design-admin.md §9 MailQueue, §14, §15.3 "Unknown provider
// outcome"; impl-admin P12): the send outcomes of the stored mail of the instance (moqui.basic.email.EmailMessage), for the
// person who reconciles transport failures without opening anyone's mailbox.
//
// What is read. An outgoing message is one that is Ready, Sent, Bounced or Cancelled; a draft, a scheduled message and received
// mail are never listed. A row answers the message's id, status, time, transport (its mail server, its template), the number of
// its recipients, the outcome of its last send and what may be done about it. Never its subject, body, headers, addresses or
// attachments: there is no parameter that returns them, and the error text is answered with every address struck out.
//
// The outcome is the message's sendOutcome, which the mail services keep with a failed send (send#EmailTemplate's and
// send#EmailMessage's deliveryOutcome):
// - retryable or permanent: the server did not take it, so nothing was sent and it may be sent again;
// - uncertain: the session was lost after it began, so it may have gone. It is never sent again from here until someone
//   settles it (looks at the relay's own log and says which it was);
// - none on a Ready message (a failure before this was kept, or a send that stopped part way): treated as uncertain.
//
// The settlement. A message's status and sendOutcome are what the transport knows, and no person sets them: a message is
// Sent only when a send of it succeeded. What a person concluded about an uncertain send is kept apart, on the message's
// sendSettlement (delivered or notDelivered: "reported", never known) with who, when and the reason, and in the audit log.
// A settled message is its own state here (settled), out of "Needs attention" and never counted or shown as Sent; no reader
// of statusId sees a delivery. Reported not delivered is the person's decision that it did not arrive, so the message may
// then be sent again as a failed one may: through the retry's claim, which clears the settlement (it was of the send
// before), and the transport's result of that send is then the message's state.
//
// Who acts. A message belongs to whoever sent it:
// - a mail account's message (emailAccountId) is its sender's, in the Outbox of the mail client, which sends it again or takes
//   it back; nothing here sends, settles or cancels it;
// - a message a registered owner claims (moqui.server.MailOutcomeOwner: a component whose own ledger holds the attempts, such as
//   Mantle's delivery ledger) is retried and reconciled by that owner's own service, with the owner's permission, or not at all;
//   while an owner cannot be asked, no message is retried here;
// - any other stored message (a template's, sent by a service that keeps no ledger) is the mail service's own: a retry is
//   send#EmailMessage of that same stored message, so its identity is kept and no second message is made.
// Reading needs ADMIN_OPERATION_VIEW; with ADMIN_CONNECTION_VIEW alone (the integration administrator, who keeps the relays)
// only what the instance itself sent is read, never a mail account's message. Acting needs ADMIN_OPERATION_MANAGE.
// A retry is claimed first, in a transaction of its own, at the revision read: a second request finds the claim, and a server
// that stops part way leaves the message uncertain, not retryable.
const lib = ec.resource.script("component://framework/service/org/moqui/admin/OperationLib.js")
const {find, one, rows, map, ms, iso, stamp, text, refused} = lib
const MESSAGE = "moqui.basic.email.EmailMessage"
const OWNER = "moqui.server.MailOutcomeOwner"
const SEND = "org.moqui.mail.EmailServices.send#EmailMessage"
const CLAIM = "org.moqui.admin.OperationServices.claim#MailOutcomeRetry"
const relayPermissions = ["ADMIN_CONNECTION_VIEW", "ADMIN_CONNECTION_MANAGE"]
const outgoing = ["ES_READY", "ES_SENT", "ES_BOUNCED", "ES_CANCELLED"]
const outcomes = ["retryable", "permanent", "uncertain"]
// what a person may report of an uncertain send (EmailMessage.sendSettlement), and the owner contract's word for each
const settlements = {
    delivered: {text: "Reported delivered", ownerAction: "sent"},
    notDelivered: {text: "Reported not delivered", ownerAction: "notSent"}
}
// a send under way longer than this stopped (as the mail client's fail#StoppedSend reads a claim)
const sendingMinutes = 15
// with no dates given, the list reads this far back
const defaultDays = 14
// Sign-in mail (a reset password, a code, a verification link) holds a credential of its moment and is asked for again by the
// person it is for: it is never sent again from here, only closed.
const signInTemplates = () =>
    ["PASSWORD_RESET", "SINGLE_USE_CODE", "ADDED_EMAIL_AUTHC_FACTOR", "EMAIL_AUTHC_FACTOR_SENT", "ACCOUNT_SIGN_IN"].concat(
        [text(System.getProperty("pass_verification_template"))].filter(Boolean)
    )

const states = {
    retryable: {text: "Not sent", next: "Nothing was sent. It can be sent again."},
    permanent: {text: "Refused", next: "The server refused it. It can be sent again once what it refused is put right."},
    bounced: {text: "Bounced", next: "It was returned after it was sent. It can be sent again."},
    uncertain: {
        text: "May have been sent",
        next: "The session was lost part way. Look in the mail server's own log before anything is sent again."
    },
    // told apart by its conclusion in described(): never "Sent"
    settled: {text: "Settled", next: ""},
    sending: {text: "Being sent", next: "A send is under way."},
    sent: {text: "Sent", next: ""},
    cancelled: {text: "Cancelled", next: ""}
}
const stateList = [
    {state: "attention", text: "Needs attention"},
    {state: "retryable", text: "Not sent"},
    {state: "permanent", text: "Refused"},
    {state: "uncertain", text: "May have been sent"},
    {state: "bounced", text: "Bounced"},
    {state: "settled", text: "Settled by a person"},
    {state: "sent", text: "Sent"},
    {state: "all", text: "All outgoing"}
]

// ===== reading =====
// The settlement a message carries, or "": what a person reported of its uncertain send.
const settlementOf = message => (Object.hasOwn(settlements, text(message.sendSettlement)) ? text(message.sendSettlement) : "")
// The state of a message's last send. Sent is the transport's alone (the status a successful send wrote); a settlement
// is read only over a send that is uncertain, as "settled", and never as sent.
function stateOf(message) {
    if (message.statusId === "ES_SENT") return "sent"
    if (message.statusId === "ES_CANCELLED") return "cancelled"
    const unknown = settlementOf(message) ? "settled" : "uncertain"
    // a bounced message whose retry was claimed and never answered may have gone again
    if (message.statusId === "ES_BOUNCED") return message.sendOutcome === "uncertain" ? unknown : "bounced"
    const claimed = ms(message.sendClaimDate)
    if (claimed != null && ms(ec.user.nowTimestamp) - claimed < sendingMinutes * 60000) return "sending"
    if (!text(message.sendError)) {
        // Ready with no result: just made (being sent), or a send that stopped; a settled one was last written by its
        // settlement, not by a send
        const touched = ms(message.lastUpdatedStamp)
        if (unknown === "uncertain" && touched != null && ms(ec.user.nowTimestamp) - touched < sendingMinutes * 60000)
            return "sending"
        return unknown
    }
    return outcomes.includes(message.sendOutcome) && message.sendOutcome !== "uncertain" ? message.sendOutcome : unknown
}
const recipientCount = message =>
    [message.toAddresses, message.ccAddresses, message.bccAddresses]
        .flatMap(list => text(list).split(/[,;]/))
        .filter(address => address.trim()).length
// The error of a send as a reader of the queue may see it: one line, every address and bracketed name struck out.
const safeError = value =>
    text(value)
        .replace(/[\u0000-\u001f\u007f]+/g, " ")
        .replace(/<[^<>\s]*>/g, "(address)")
        .replace(/[^\s<>()"',;:]+@[^\s<>()"',;:]+/g, "(address)")
        .slice(0, 200)

// The registered owners in use, or why not: each is checked when read, logged by name and left out. A row left out for a
// service that is missing counts as an owner that cannot be asked (unanswered).
function registeredOwners() {
    const owners = []
    const unanswered = []
    for (const row of rows(OWNER)) {
        if (row.disabled === "Y") continue
        const owner = map(row)
        const missing = [owner.ownerService, owner.recoverService].filter(
            name => text(name) && !ec.service.getServiceDefinition(text(name))
        )
        if (!text(owner.ownerService) || missing.length) {
            ec.logger.error("Mail outcome owner " + owner.ownerKey + " names a service that is not defined; left out")
            unanswered.push(owner.ownerKey)
            continue
        }
        owners.push(owner)
    }
    owners.sort((a, b) => Number(a.sequenceNum ?? 9999) - Number(b.sequenceNum ?? 9999) || a.ownerKey.localeCompare(b.ownerKey))
    return {owners, unanswered}
}
// Which of these messages a registered owner claims: {claims: Map emailMessageId -> claim, unanswered: owner keys that could not
// be asked}. The first owner to claim a message has it. An owner's answer is read as data: its texts are cut to a line.
function ownerClaims(emailMessageIds) {
    const {owners, unanswered} = registeredOwners()
    const claims = new Map()
    if (!emailMessageIds.length) return {claims, unanswered}
    for (const owner of owners) {
        const asked = lib.callRegistered(owner.ownerService, {emailMessageIdList: emailMessageIds})
        if (asked.failure) {
            ec.logger.error("Mail outcome owner " + owner.ownerKey + " did not answer: " + asked.failure)
            unanswered.push(owner.ownerKey)
            continue
        }
        for (const answered of Array.from(asked.out.ownerList || [])) {
            const claim = map(answered)
            const id = text(claim.emailMessageId)
            if (!emailMessageIds.includes(id) || claims.has(id)) continue
            claims.set(id, {
                ownerKey: owner.ownerKey,
                ownerTitle: owner.title || owner.ownerKey,
                recoverService: text(owner.recoverService) || null,
                ownerRef: text(claim.ownerRef).slice(0, 80),
                ownerStateText: text(claim.ownerStateText).slice(0, 120),
                retryEligible: claim.retryEligible === true,
                reconcileEligible: claim.reconcileEligible === true,
                selfRetrying: claim.selfRetrying === true
            })
        }
    }
    return {claims, unanswered}
}
// A message as the queue shows it, with its owner and what may be done here.
function described(message, claim, unanswered, canManage, templates, userNames = new Map()) {
    const state = stateOf(message)
    const account = !!text(message.emailAccountId)
    const settlement = state === "settled" ? settlementOf(message) : ""
    // known not sent, or reported not delivered by the person who settled it: either may be sent again
    const failed = ["retryable", "permanent", "bounced"].includes(state) || settlement === "notDelivered"
    const template = message.emailTemplateId ? templates.get(message.emailTemplateId) : null
    let ownerKind = "mailService"
    let ownerTitle = template ? template.description || template.emailTemplateId : "Mail service"
    let nextStep =
        settlement === "notDelivered"
            ? "Someone reported that it did not arrive. It can be sent again."
            : settlement
              ? "Someone reported that it arrived. Nothing more is done here."
              : states[state].next
    let retryEligible = failed
    let reconcileEligible = state === "uncertain"
    const signIn = !account && !claim && signInTemplates().includes(text(message.emailTemplateId))
    if (signIn) {
        retryEligible = false
        if (failed)
            nextStep = "Sign-in mail is not sent again: the person asks for a new one." + (settlement ? "" : " It can be closed.")
    }
    if (account) {
        ownerKind = "mailAccount"
        ownerTitle = "Mail account"
        retryEligible = false
        reconcileEligible = false
        if (failed || state === "uncertain") nextStep = "It waits in its sender's Outbox, who sends it again or takes it back."
    } else if (claim) {
        ownerKind = "registered"
        ownerTitle = claim.ownerTitle
        retryEligible = failed && claim.retryEligible && !!claim.recoverService
        reconcileEligible = state === "uncertain" && claim.reconcileEligible && !!claim.recoverService
        if (failed && !retryEligible)
            nextStep = claim.selfRetrying
                ? "Its owner tries it again on its own."
                : settlement
                  ? "Settled with its owner, which keeps its attempts: nothing is sent from here."
                  : "Its owner keeps its attempts: nothing is sent from here."
        if (state === "uncertain" && !reconcileEligible) nextStep = "Its owner reconciles it."
    } else if (unanswered.length && (failed || state === "uncertain")) {
        // an owner that cannot be asked may hold this message: nothing is sent or settled here until it answers
        retryEligible = false
        reconcileEligible = false
        nextStep = "An owner of mail could not be asked whether this message is its own. Try again later."
    }
    return {
        emailMessageId: message.emailMessageId,
        statusId: message.statusId,
        state,
        stateText: settlement ? "Settled: " + settlements[settlement].text.toLowerCase() : states[state].text,
        // the settlement, apart from the outcome: what a person reported, who and when; null on every other state
        settlement: settlement || null,
        settlementText: settlement ? settlements[settlement].text : null,
        settledByUserId: settlement ? message.sendSettledByUserId || null : null,
        settledByName: settlement ? userNames.get(message.sendSettledByUserId) || message.sendSettledByUserId || null : null,
        settledDate: settlement ? iso(message.sendSettledDate) : null,
        settledReason: settlement ? text(message.sendSettledReason) || null : null,
        outcome:
            state === "sent" || state === "cancelled"
                ? null
                : outcomes.includes(message.sendOutcome)
                  ? message.sendOutcome
                  : null,
        outcomeDate: iso(message.lastUpdatedStamp),
        sentDate: iso(message.sentDate),
        recipientCount: recipientCount(message),
        // a mail account's error is its sender's to read (it can name an attachment or quote the message): the state says enough
        errorText: state === "sent" || account ? "" : safeError(message.sendError),
        signIn,
        emailServerId: message.emailServerId || null,
        emailTemplateId: message.emailTemplateId || null,
        emailTypeEnumId: message.emailTypeEnumId || null,
        binding: account ? "account" : "instance",
        emailAccountId: message.emailAccountId || null,
        ownerKind,
        ownerKey: claim && !account ? claim.ownerKey : null,
        ownerTitle,
        ownerRef: claim && !account ? claim.ownerRef : null,
        ownerStateText: claim && !account ? claim.ownerStateText : null,
        retryEligible: canManage && retryEligible,
        reconcileEligible: canManage && reconcileEligible,
        // a failed or uncertain message of the mail service's own may be closed, so it is not offered again; a settled one
        // is already out of the way
        abandonEligible:
            canManage && !account && !claim && !unanswered.length && !settlement && (failed || state === "uncertain"),
        // an owner that cannot be asked may hold it: nothing of the mail service's own is acted on until it answers
        ownerUnavailable: !account && !claim && unanswered.length > 0,
        nextStep,
        revision: stamp(message)
    }
}
// The names of the people who settled a page of messages, read in one find.
function settlerNames(messages) {
    const ids = Array.from(new Set(messages.map(message => message.sendSettledByUserId).filter(Boolean)))
    if (!ids.length) return new Map()
    return new Map(
        Array.from(
            find("moqui.security.UserAccount").condition("userId", "in", ids).selectField("userId,username,userFullName").list()
        ).map(row => [row.userId, text(row.userFullName) || text(row.username) || row.userId])
    )
}
// The templates of a page of messages, read in one find.
function templatesOf(messages) {
    const ids = Array.from(new Set(messages.map(message => message.emailTemplateId).filter(Boolean)))
    if (!ids.length) return new Map()
    return new Map(
        Array.from(
            find("moqui.basic.email.EmailTemplate")
                .condition("emailTemplateId", "in", ids)
                .selectField("emailTemplateId,description")
                .list()
        ).map(row => [row.emailTemplateId, map(row)])
    )
}
// The messages of a state, as a find: the conditions are in the query, before the page.
function outcomeFind(p) {
    const factory = ec.entity.conditionFactory
    const state = text(p.state) || "attention"
    if (!stateList.some(option => option.state === state)) throw new Error("Unknown mail outcome state " + state)
    let found = find(MESSAGE)
    const noOutcome = () =>
        factory.makeCondition(
            [factory.makeCondition("sendOutcome", "equals", "uncertain"), factory.makeCondition("sendOutcome", "equals", null)],
            "or"
        )
    // a settlement counts only over a send that is uncertain (stateOf): a message known not sent, or bounced since, is
    // listed as that
    const unsettled = () =>
        factory.makeCondition(
            [
                factory.makeCondition("sendSettlement", "equals", null),
                factory.makeCondition("sendOutcome", "in", ["retryable", "permanent"]),
                factory.makeCondition(
                    [
                        factory.makeCondition("statusId", "equals", "ES_BOUNCED"),
                        factory.makeCondition("sendOutcome", "equals", null)
                    ],
                    "and"
                )
            ],
            "or"
        )
    const settled = () =>
        factory.makeCondition(
            [
                factory.makeCondition([factory.makeCondition("statusId", "equals", "ES_READY"), noOutcome()], "and"),
                factory.makeCondition(
                    [
                        factory.makeCondition("statusId", "equals", "ES_BOUNCED"),
                        factory.makeCondition("sendOutcome", "equals", "uncertain")
                    ],
                    "and"
                )
            ],
            "or"
        )
    if (state === "attention") found = found.condition("statusId", "in", ["ES_READY", "ES_BOUNCED"]).condition(unsettled())
    else if (state === "all") found = found.condition("statusId", "in", outgoing)
    // Sent is the status a successful send wrote, and nothing else
    else if (state === "sent") found = found.condition("statusId", "ES_SENT")
    else if (state === "bounced") found = found.condition("statusId", "ES_BOUNCED").condition(unsettled())
    else if (state === "uncertain")
        found = found.condition("statusId", "ES_READY").condition(noOutcome()).condition("sendSettlement", "equals", null)
    else if (state === "settled") found = found.condition("sendSettlement", "in", Object.keys(settlements)).condition(settled())
    else found = found.condition("statusId", "ES_READY").condition("sendOutcome", state)
    if (text(p.emailMessageId)) found = found.condition("emailMessageId", text(p.emailMessageId))
    if (text(p.emailServerId)) found = found.condition("emailServerId", text(p.emailServerId))
    if (text(p.emailTemplateId)) found = found.condition("emailTemplateId", text(p.emailTemplateId))
    if (p.binding === "account") found = found.condition("emailAccountId", "not-equals", null)
    else if (p.binding === "instance") found = found.condition("emailAccountId", "equals", null)
    else if (text(p.binding)) throw new Error("A binding is account or instance")
    const from = ms(p.fromDate)
    const thru = ms(p.thruDate)
    if ((p.fromDate && (from == null || Number.isNaN(from))) || (p.thruDate && (thru == null || Number.isNaN(thru))))
        throw new Error("A date is not a date")
    // one message asked for by id is found whatever its age
    const since = from ?? (thru == null && !text(p.emailMessageId) ? ms(ec.user.nowTimestamp) - defaultDays * 86400000 : null)
    if (since != null) found = found.condition("lastUpdatedStamp", "greater-equals", iso(since))
    if (thru != null) found = found.condition("lastUpdatedStamp", "less", iso(thru))
    return found
}

// find#MailOutcomes: a page of send outcomes, newest first, each with its owner and what may be done here.
function findMailOutcomes(given) {
    lib.signedIn()
    const canManage = ec.user.hasPermission(lib.MANAGE)
    // the integration administrator, who keeps the relays, reads what the instance sent through them and no mail account's
    const operator = canManage || ec.user.hasPermission(lib.VIEW)
    if (!operator && !relayPermissions.some(permission => ec.user.hasPermission(permission)))
        throw new Error(lib.VIEW + " is required")
    const p = operator ? given : Object.assign({}, given, {binding: "instance"})
    const {pageIndex, pageSize} = lib.pageOf(p)
    const outcomeCount = Number(outcomeFind(p).count())
    const messages = Array.from(
        outcomeFind(p)
            .orderBy("-lastUpdatedStamp,emailMessageId")
            .offset(pageIndex * pageSize)
            .limit(pageSize)
            .list()
    ).map(map)
    // only a message someone may still act on is an owner's question
    const open = messages.filter(message => !message.emailAccountId && !["sent", "cancelled"].includes(stateOf(message)))
    const {claims, unanswered} = ownerClaims(open.map(message => message.emailMessageId))
    const templates = templatesOf(messages)
    const userNames = settlerNames(messages)
    return {
        outcomeList: messages.map(message =>
            described(message, claims.get(message.emailMessageId), unanswered, canManage, templates, userNames)
        ),
        outcomeCount,
        pageIndex,
        pageSize,
        canManage,
        instanceOnly: !operator,
        stateList,
        unansweredOwnerList: unanswered,
        defaultDays
    }
}

// ===== acting =====
// One outgoing message for an action, read without a lock (the mail service writes its result in a transaction of its own),
// at the revision the caller read: {message, row} or {answer} a refusal.
function actionable(p) {
    const emailMessageId = text(p.emailMessageId)
    const message = emailMessageId ? one(MESSAGE, {emailMessageId}) : null
    if (!message || !outgoing.includes(message.statusId)) throw new Error("No outgoing message " + emailMessageId)
    const revision = stamp(message)
    if (text(p.expectedRevision) !== revision)
        return {answer: refused("STALE_REVISION", "The message changed since it was read; read it again and repeat", {revision})}
    const {claims, unanswered} = ownerClaims(message.emailAccountId ? [] : [emailMessageId])
    const current = map(message)
    return {
        message: current,
        row: described(current, claims.get(emailMessageId), unanswered, true, templatesOf([current])),
        claim: claims.get(emailMessageId)
    }
}
// Why a message is not for this action, by its owner: a refusal, or null when it is the mail service's own.
function ownerRefusal(row, revision) {
    if (row.ownerKind === "mailAccount")
        return refused("OWNER_ONLY", "This message is in a mail account's Outbox: its sender sends it again or takes it back", {
            revision
        })
    if (row.ownerUnavailable && ["retryable", "permanent", "bounced", "uncertain", "settled"].includes(row.state))
        return refused("OWNER_UNAVAILABLE", row.nextStep, {revision})
    return null
}
const answerOf = emailMessageId => {
    const after = map(one(MESSAGE, {emailMessageId}))
    return {state: stateOf(after), revision: stamp(after)}
}
// An owner's own action on its message, by its registered service: its answer, read as data.
function ownerAction(claim, emailMessageId, action, reason, key) {
    const asked = lib.callRegistered(claim.recoverService, {
        emailMessageId,
        ownerRef: claim.ownerRef,
        action,
        reason,
        operationKey: key || null
    })
    if (asked.failure) return refused("OWNER_REFUSED", claim.ownerTitle + " did not take it: " + safeError(asked.failure))
    if (asked.out.applied !== true)
        return refused(
            text(asked.out.reasonCode) || "OWNER_REFUSED",
            safeError(asked.out.reason) || claim.ownerTitle + " did not take it"
        )
    return {applied: true, ownerStateText: text(asked.out.ownerStateText).slice(0, 120)}
}

// retry#MailOutcome: send again, through its owner, a message that is known not sent or that a person settled as not
// delivered; an uncertain one that nobody settled, and one reported delivered, are never sent.
function retryMailOutcome(p) {
    lib.requireManage()
    const key = lib.operationKey(p)
    const emailMessageId = text(p.emailMessageId)
    return lib.once(key, "mailRetry", emailMessageId, () => {
        const found = actionable(p)
        if (found.answer) return found.answer
        const {row, claim} = found
        const revision = row.revision
        if (row.state === "sent") return refused("NOT_FAILED", "The message was sent", {revision})
        if (row.state === "cancelled") return refused("NOT_FAILED", "The message was cancelled", {revision})
        if (row.state === "sending") return refused("IN_PROGRESS", "A send of this message is under way", {revision})
        if (row.state === "uncertain")
            return refused(
                "RECONCILE_REQUIRED",
                "The message may have been sent: it is reconciled, not sent again. " +
                    (row.reconcileEligible ? states.uncertain.next : row.nextStep),
                {revision}
            )
        if (row.settlement === "delivered")
            return refused("SETTLED", "The message was settled as reported delivered: it is not sent again", {revision})
        const owned = ownerRefusal(row, revision)
        if (owned) return owned
        if (row.signIn) return refused("SIGN_IN_MAIL", row.nextStep, {revision})
        if (row.ownerKind === "registered") {
            if (!row.retryEligible) return refused("OWNER_RETRIES", row.nextStep, {revision})
            const done = ownerAction(claim, emailMessageId, "retry", lib.reasonText(p), key)
            return Object.assign(done, answerOf(emailMessageId))
        }
        // the mail service's own: claimed first, at the revision read, then the same stored message is sent
        const claimed = lib.callRegistered(CLAIM, {emailMessageId, expectedRevision: revision})
        if (claimed.failure) throw new Error(claimed.failure)
        if (claimed.out.claimed !== true)
            return refused(
                "STALE_REVISION",
                "The message changed since it was read; read it again and repeat",
                answerOf(emailMessageId)
            )
        const sent = lib.callRegistered(SEND, {emailMessageId})
        const failure = sent.failure || text(sent.out?.sendError) || (text(sent.out?.messageId) ? "" : "The message was not sent")
        // the claim is over either way; a failure the mail service kept (its outcome) stands, any other stays uncertain
        lib.callRegistered(CLAIM, {emailMessageId, release: true})
        const after = answerOf(emailMessageId)
        if (failure) return refused("SEND_FAILED", "Not sent: " + safeError(failure), after)
        return Object.assign({applied: true, sent: true}, after)
    })
}
// The retry's claim, in a transaction of its own: the message locked, still at the revision read and still known not sent
// (or settled as not delivered); it is then marked uncertain with the claim's time, so a second request and a server that
// stops part way both find it held, and a settlement is cleared: it was of the send before, and this send's result is
// the transport's to give. With release, the claim's time is cleared (the mail service has written the result, or nothing).
function claimMailOutcomeRetry(p) {
    if (!ec.artifactExecution.authzDisabled() && !ec.user.hasPermission(lib.MANAGE)) throw new Error(lib.MANAGE + " is required")
    const emailMessageId = text(p.emailMessageId)
    const message = find(MESSAGE, {emailMessageId}).forUpdate(true).one()
    if (!message) return {claimed: false}
    if (p.release === true || p.release === "true") {
        if (message.sendClaimDate != null && !message.emailAccountId) {
            message.set("sendClaimDate", null)
            // the claim's own text does not stay on a message that went
            if (message.statusId === "ES_SENT") message.set("sendError", null)
            message.update()
        }
        return {claimed: false}
    }
    const current = map(message)
    if (stamp(current) !== text(p.expectedRevision) || current.emailAccountId) return {claimed: false}
    const state = stateOf(current)
    const settledNotDelivered = state === "settled" && settlementOf(current) === "notDelivered"
    if (!["retryable", "permanent", "bounced"].includes(state) && !settledNotDelivered) return {claimed: false}
    message.setFields(
        {
            sendClaimDate: ec.user.nowTimestamp,
            sendOutcome: "uncertain",
            sendError: "A retry of this message began and its result was not recorded",
            sendSettlement: null,
            sendSettledByUserId: null,
            sendSettledDate: null,
            sendSettledReason: null
        },
        true,
        null,
        null
    )
    message.update()
    return {claimed: true}
}

// reconcile#MailOutcome: what a person concluded about a message that may have been sent, kept apart from the message's
// sent state: delivered or notDelivered is recorded as its settlement (who, when, the reason; the status and sendOutcome
// stay what the transport left, so nothing reads it as Sent), and abandon closes a failed or uncertain message (Cancelled:
// it is not sent again). Nothing is sent. One settlement for a send: a second is refused, or answered by its operation key.
function reconcileMailOutcome(p) {
    lib.requireManage()
    const key = lib.operationKey(p)
    const emailMessageId = text(p.emailMessageId)
    const resolution = text(p.resolution)
    if (resolution !== "abandon" && !Object.hasOwn(settlements, resolution))
        throw new Error("A resolution is delivered, notDelivered or abandon")
    const reason = lib.requiredReason(p)
    return lib.once(key, "mailReconcile", emailMessageId, () => {
        const found = actionable(p)
        if (found.answer) return found.answer
        const {row, claim} = found
        const revision = row.revision
        if (["sent", "cancelled"].includes(row.state))
            return refused("NOT_FAILED", "The message is " + row.stateText.toLowerCase(), {revision})
        if (row.state === "sending") return refused("IN_PROGRESS", "A send of this message is under way", {revision})
        if (row.state === "settled")
            return refused("ALREADY_SETTLED", "The message is settled (" + row.settlementText.toLowerCase() + ")", {revision})
        const owned = ownerRefusal(row, revision)
        if (owned) return owned
        if (row.ownerKind === "registered") {
            if (resolution === "abandon" || !row.reconcileEligible) return refused("OWNER_RECONCILES", row.nextStep, {revision})
            const done = ownerAction(claim, emailMessageId, settlements[resolution].ownerAction, reason, key)
            if (!done.applied) return Object.assign(done, {revision})
        } else if (resolution !== "abandon" && row.state !== "uncertain")
            return refused("NOT_UNCERTAIN", "Only a message that may have been sent is settled; this one was not sent", {
                revision
            })
        // the message's own record: the settlement beside what the transport left, or closed; each with its audit row
        const locked = find(MESSAGE, {emailMessageId}).forUpdate(true).one()
        if (stamp(locked) !== revision)
            return refused("STALE_REVISION", "The message changed since it was read; read it again and repeat", {
                revision: stamp(locked)
            })
        const fields =
            resolution === "abandon"
                ? {statusId: "ES_CANCELLED", sendClaimDate: null}
                : {
                      sendSettlement: resolution,
                      sendSettledByUserId: ec.user.userId,
                      sendSettledDate: ec.user.nowTimestamp,
                      sendSettledReason: reason
                  }
        lib.auditReason(reason, [resolution === "abandon" ? "statusId" : "sendSettlement"])
        locked.setFields(fields, true, null, null)
        locked.update()
        const after = map(one(MESSAGE, {emailMessageId}))
        const answer = {applied: true, resolution, state: stateOf(after), revision: stamp(after)}
        if (resolution !== "abandon")
            Object.assign(answer, {
                settlement: resolution,
                settlementText: settlements[resolution].text,
                settledByUserId: after.sendSettledByUserId,
                settledDate: iso(after.sendSettledDate)
            })
        return answer
    })
}

const operations = {findMailOutcomes, retryMailOutcome, claimMailOutcomeRetry, reconcileMailOutcome}
;({
    stateOf,
    safeError,
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown mail operation " + op)
        return operations[op](parameters)
    }
})
