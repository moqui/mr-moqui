// Owner-equivalent delivery of business reports (crates/moqui-report/src/delivery.rs; docs/tools/Monitoring.md "Report
// delivery"; BOSS impl-insights P7, INS-06). A schedule renders as its owner, so its result goes only to recipients whose scope
// fingerprint for the definition equals the owner's: check#ReportAudience and store#Schedule refuse one whose row filter
// differs, each run checks every recipient again and skips one who changed (scope-differs), each recipient gets a message of
// their own, one render serves a locale, the cap holds, a retry sends the failed recipient alone, and a disabled owner's
// schedule pauses until a qualified owner takes it and passes the same check. An owner-mode schedule (create#Schedule)
// changed with no recipient list keeps its group, location and template. A recipient reads their own report again, never
// another's. What the delivery screens read and write beside the engine's own (ReportReads.js, ReportWrites.js): recipients
// are proposed and accepted by the engine's rule or among a permission's holders, whatever a caller passes, and a screen
// schedule is read and changed by its own user alone. The group a definition is shared with is the saver's own or one
// whose share artifact their own grant holds, whatever saves it. The dataset, its scope groups (RDR_A, RDR_B by the ReportUserGroups
// policy) and the reading grants are ReportDrill's. Run: ./moq test framework --spec ReportAudience
const R = "org.moqui.report.ReportServices."
const capture = ec.factory.getTool("MailCapture")
// [userId, username, locale, scope group, may deliver to a group]. The ReportUserGroups policy scopes rows by the viewer's own
// groups, so its resolved filter names every group of theirs: the users of one scope here are in the same groups, the team
// among them.
const USERS = [
    ["RDA_OWNER", "rda.owner", null, "RDR_A", true],
    ["RDA_EQUAL", "rda.equal", null, "RDR_A", true],
    ["RDA_FRENCH", "rda.french", "fr", "RDR_A", true],
    ["RDA_OTHER", "rda.other", null, "RDR_B", false],
    ["RDA_NEXT", "rda.next", null, "RDR_A", true]
]
const address = username => username + "@example.test"

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
function make(entityName, fields) {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
// a call as a user, with authorization on: its result, or its error
function as(username, name, parameters) {
    ec.user.internalLoginUser(username)
    ec.artifactExecution.enableAuthz()
    try {
        return {out: call(R + name, parameters)}
    } catch (error) {
        ec.message.clearErrors()
        return {error: String(error.message)}
    } finally {
        login("john.doe")
    }
}
const one = (entityName, fields) => sys(() => ec.entity.find(entityName).condition(fields).useCache(false).one())
const schedule = extra =>
    Object.assign(
        {definitionId: "rda-board", cronExpression: "0 0 6 ? * MON", renderMode: "csv", emailTemplateId: "RDA_REPORT"},
        extra
    )
const toLists = () =>
    Array.from(capture.getMessages())
        .map(message => Array.from(message.toList).join("+"))
        .sort()
        .join(",")
const deliveryOf = (occurrence, userId) =>
    Array.from(occurrence.deliveries || []).find(delivery => delivery.userId === userId) || {}

spec({
    stepwise: true,
    dependsOn: ["ReportDrill"],
    setupSpec() {
        login("john.doe")
        capture.enable()
    },
    cleanupSpec() {
        capture.reset()
        ec.message.clearErrors()
        ec.artifactExecution.enableAuthz()
    }
})

feature("an owner, recipients in and out of the owner's scope, a team and a dashboard over the business dataset", {
    when() {
        sys(() => {
            const from = new Date(Date.now() - 60000)
            // RDA_WALL: a group no person is in (an app's wall displays), which only a holder of its share artifact names
            for (const userGroupId of ["RDA_SENDERS", "RDA_GROUP", "RDA_TEAM", "RDA_WALL"])
                make("moqui.security.UserGroup", {userGroupId, description: "Report audience spec " + userGroupId})
            for (const [userId, username, locale, scopeGroup, distributes] of USERS) {
                make("moqui.security.UserAccount", {
                    userId,
                    username,
                    userFullName: username,
                    emailAddress: address(username),
                    locale,
                    disabled: "N"
                })
                const groups = [scopeGroup, "RDR_READER", "RDA_SENDERS"].concat(distributes ? ["RDA_GROUP"] : [])
                for (const userGroupId of groups) make("moqui.security.UserGroupMember", {userGroupId, userId, fromDate: from})
            }
            for (const userId of ["RDA_OWNER", "RDA_EQUAL", "RDA_FRENCH", "RDA_NEXT"])
                make("moqui.security.UserGroupMember", {userGroupId: "RDA_TEAM", userId, fromDate: from})
            const grant = (artifactGroupId, names) => {
                make("moqui.security.ArtifactGroup", {artifactGroupId, description: "Report audience spec"})
                for (const artifactName of names)
                    make("moqui.security.ArtifactGroupMember", {artifactGroupId, artifactName, artifactTypeEnumId: "AT_SERVICE"})
                make("moqui.security.ArtifactAuthz", {
                    artifactAuthzId: artifactGroupId,
                    artifactGroupId,
                    userGroupId: artifactGroupId,
                    authzTypeEnumId: "AUTHZT_ALLOW",
                    authzActionEnumId: "AUTHZA_ALL"
                })
            }
            // scheduling is the operations; delivering to anyone else is the engine's own artifact
            grant(
                "RDA_SENDERS",
                [
                    "save#Definition",
                    "check#ReportAudience",
                    "store#Schedule",
                    "create#Schedule",
                    "list#Schedules",
                    "pause#Schedule",
                    "send#Report",
                    "find#ReportOccurrences",
                    "get#ReportOccurrence",
                    "get#RecipientReport",
                    "retry#ReportRecipients",
                    "reconcile#ReportRecipient",
                    "find#OwnedSchedules",
                    "transfer#DefinitionOwner",
                    "find#ReportRecipients",
                    "find#ShareGroups",
                    "find#ScreenSchedules",
                    "store#ScreenSchedule"
                ].map(name => R + name)
            )
            // and so is sharing a definition with a group the sharer is not in: the group's own share artifact
            grant("RDA_GROUP", ["org.moqui.report.delivery.group", "org.moqui.report.share.RDA_WALL"])
            // every reader of the dataset reads their own deliveries
            for (const name of ["find#ReportOccurrences", "get#ReportOccurrence", "get#RecipientReport"])
                make("moqui.security.ArtifactGroupMember", {
                    artifactGroupId: "RDR_READER",
                    artifactName: R + name,
                    artifactTypeEnumId: "AT_SERVICE"
                })
            make("moqui.basic.email.EmailServer", {emailServerId: "RDA_MAIL", smtpHost: "127.0.0.1", smtpPort: "25"})
            make("moqui.basic.email.EmailTemplate", {
                emailTemplateId: "RDA_REPORT",
                emailServerId: "RDA_MAIL",
                fromAddress: "reports@example.test",
                subject: "${title}",
                bodyScreenLocation: "component://framework/screen/DashboardReportEmail.xml",
                webappName: "webroot"
            })
        })
        // the owner's own dashboard over the scoped dataset, shared with the dataset's readers
        return as("rda.owner", "save#Definition", {
            definition: {
                id: "rda-board",
                type: "dashboard",
                title: "Audience board",
                userGroupId: "RDR_READER",
                body: {
                    panels: {
                        records: {
                            title: "Records",
                            visualization: "table",
                            queries: [
                                {
                                    datasetId: "rdr-sales",
                                    aggregations: [{id: "region", type: "terms", params: {field: "relatedEnumId"}}]
                                }
                            ]
                        }
                    },
                    layout: {type: "autoGrid", items: [{panelId: "records"}]}
                }
            }
        })
    },
    then({out, error}) {
        error == undefined
        out.definition.ownerId == "RDA_OWNER"
    }
})

feature("a recipient whose row filter differs is refused at save with a reason code, and the cap and the group grant hold", {
    when() {
        const audience = as("rda.owner", "check#ReportAudience", {
            definitionId: "rda-board",
            recipientUserIds: ["RDA_OTHER"],
            recipientGroupIds: ["RDA_TEAM"],
            recipientCap: 9999
        })
        const verdict = userId => Array.from(audience.out?.recipients || []).find(recipient => recipient.userId === userId) || {}
        // the same dashboard kept private: an equal recipient cannot open the report itself, which says so
        as("rda.owner", "save#Definition", {
            definition: {
                id: "rda-private",
                type: "dashboard",
                title: "Private board",
                body: {panels: {a: {panelRef: "rdr-panel"}}}
            }
        })
        const unshared = as("rda.owner", "check#ReportAudience", {definitionId: "rda-private", recipientUserIds: ["RDA_EQUAL"]})
        const refused = as("rda.owner", "store#Schedule", schedule({recipientUserIds: ["RDA_EQUAL", "RDA_OTHER"]}))
        const overCap = as("rda.owner", "store#Schedule", schedule({recipientGroupIds: ["RDA_TEAM"], recipientCap: 1}))
        // the same save by a user who schedules but holds no group delivery grant, then their own personal schedule
        const noGrant = as("rda.other", "store#Schedule", schedule({recipientGroupIds: ["RDA_TEAM"]}))
        const personal = as("rda.other", "store#Schedule", schedule({}))
        const stored = as("rda.owner", "store#Schedule", schedule({recipientGroupIds: ["RDA_TEAM"]}))
        const stale = as("rda.owner", "store#Schedule", {
            screenScheduledId: stored.out?.screenScheduledId,
            emailSubject: "Stale",
            expectedRevision: 7
        })
        return {
            audience: audience.out || {},
            audienceError: audience.error || "",
            other: verdict("RDA_OTHER"),
            french: verdict("RDA_FRENCH"),
            unshared: Array.from(unshared.out?.recipients || [])[0]?.reasonCode,
            refused: refused.error || "",
            overCap: overCap.error || "",
            noGrant: noGrant.error || "",
            personal: personal.out || {},
            stored: stored.out || {},
            storedError: stored.error || "",
            stale: stale.error || "",
            schedules: sys(() =>
                ec.entity.find("moqui.screen.ScreenScheduled").condition("reportDefinitionId", "rda-board").count()
            )
        }
    },
    then({
        audience,
        audienceError,
        other,
        french,
        unshared,
        refused,
        overCap,
        noGrant,
        personal,
        stored,
        storedError,
        stale,
        schedules
    }) {
        audienceError == ""
        audience.recipientCount == 5
        audience.eligibleCount == 4
        audience.deliveryMode == "group"
        // a cap asked above the instance ceiling is the ceiling
        audience.recipientCap == audience.recipientCeiling
        other.eligible == false
        other.blocking == true
        other.reasonCode == "scope-differs"
        // the reason is a sentence about access, never the other user's groups or filters
        !/RDR_|RDA_|enumCode/.test(other.reason)
        french.eligible == true
        unshared == "not-shared"
        Array.from(french.viaGroupIds).join(",") == "RDA_TEAM"
        refused.includes("RECIPIENT_REFUSED")
        refused.includes("scope-differs")
        overCap.includes("RECIPIENT_CAP")
        noGrant.includes("GROUP_DELIVERY_DENIED")
        personal.deliveryMode == "personal"
        storedError == ""
        stored.deliveryMode == "group"
        stored.recipientCount == 4
        stored.revision == 1
        stale.includes("STALE_REVISION")
        // only the two saves that passed left a schedule
        schedules == 2
    }
})

feature(
    "each recipient gets a message of their own from one render a locale, and one whose scope changed is skipped at the run",
    {
        when() {
            const scheduleId = String(one("moqui.screen.ScreenScheduled", {reportOwnerId: "RDA_OWNER"}).screenScheduledId)
            // a member whose scope differs joins the team after the save
            sys(() =>
                make("moqui.security.UserGroupMember", {
                    userGroupId: "RDA_TEAM",
                    userId: "RDA_OTHER",
                    fromDate: new Date(Date.now() - 60000)
                })
            )
            capture.clear()
            const sent = as("rda.owner", "send#Report", {screenScheduledId: scheduleId})
            const messages = Array.from(capture.getMessages())
            const found = as("rda.owner", "find#ReportOccurrences", {screenScheduledId: scheduleId})
            const occurrence = Array.from(found.out?.occurrences || [])[0] || {}
            const detail = as("rda.owner", "get#ReportOccurrence", {occurrenceId: occurrence.occurrenceId})
            const equalDelivery = deliveryOf(detail.out || {}, "RDA_EQUAL").deliveryId
            const frenchDelivery = deliveryOf(detail.out || {}, "RDA_FRENCH").deliveryId
            // a recipient: their own delivery and their own report, rendered again as themselves; never another's
            const received = as("rda.equal", "find#ReportOccurrences", {received: true})
            const recipientView = as("rda.equal", "get#ReportOccurrence", {occurrenceId: occurrence.occurrenceId})
            const ownReport = as("rda.equal", "get#RecipientReport", {deliveryId: equalDelivery})
            const anothers = as("rda.equal", "get#RecipientReport", {deliveryId: frenchDelivery})
            const everyOwners = as("rda.equal", "find#ReportOccurrences", {administrative: true})
            // a recipient who was skipped has no report to read
            const skippedReport = as("rda.other", "get#RecipientReport", {
                deliveryId: deliveryOf(detail.out || {}, "RDA_OTHER").deliveryId
            })
            const administrative = call(R + "find#ReportOccurrences", {administrative: true, screenScheduledId: scheduleId})
            return {
                sentError: sent.error || "",
                count: messages.length,
                toLists: toLists(),
                shared: messages.filter(message => Array.from(message.ccList).length || Array.from(message.bccList).length)
                    .length,
                attachments: Array.from(messages[0]?.attachmentNames || []).join(","),
                occurrence,
                other: deliveryOf(detail.out || {}, "RDA_OTHER"),
                ownerName: detail.out?.occurrence?.ownerName,
                detailKeys: Object.keys(detail.out?.occurrence || {})
                    .concat(Object.keys(deliveryOf(detail.out || {}, "RDA_EQUAL")))
                    .join(","),
                received: Array.from(received.out?.occurrences || [])
                    .map(row => row.ownDelivery?.deliveryId)
                    .join(","),
                equalDelivery,
                recipientView: recipientView.out || {},
                ownReport: ownReport.out || {},
                ownReportError: ownReport.error || "",
                anothers: anothers.error || "",
                everyOwners: everyOwners.error || "",
                skippedReport: skippedReport.error || "",
                administrative: Array.from(administrative.occurrences || [])[0] || {}
            }
        },
        then({
            sentError,
            count,
            toLists,
            shared,
            attachments,
            occurrence,
            other,
            ownerName,
            detailKeys,
            received,
            equalDelivery,
            recipientView,
            ownReport,
            ownReportError,
            anothers,
            everyOwners,
            skippedReport,
            administrative
        }) {
            sentError == ""
            // four messages, one address each, no shared address line
            count == 4
            toLists == "rda.equal@example.test,rda.french@example.test,rda.next@example.test,rda.owner@example.test"
            shared == 0
            attachments == "Records.csv"
            occurrence.statusId == "RptOccComplete"
            occurrence.sentCount == 4
            occurrence.skippedCount == 1
            occurrence.failedCount == 0
            // the owner's locale and the French recipient's: two renders for four recipients
            occurrence.renderCount == 2
            occurrence.deliveryMode == "group"
            other.statusId == "RptDlvSkipped"
            other.reasonCode == "scope-differs"
            // whose run it was, by name, for its owner; a recipient is told neither the id nor the name
            ownerName == "rda.owner"
            recipientView.occurrence.ownerUserId == undefined && recipientView.occurrence.ownerName == undefined
            // evidence only: no frame, file or message body
            !/frame|content|html|file|body|scopeHash/i.test(detailKeys)
            received == equalDelivery
            recipientView.administers == false
            Array.from(recipientView.deliveries).length == 0
            recipientView.ownDelivery.deliveryId == equalDelivery
            recipientView.occurrence.sentCount == undefined
            ownReportError == ""
            ownReport.available == true
            ownReport.rerendered == true
            ownReport.filename == "Records.csv"
            String(ownReport.contentBase64 || "").length > 0
            anothers.includes("not available")
            everyOwners.includes("ADMIN_TRANSFER_VIEW")
            skippedReport.includes("not available")
            // an administrator's list is status and counters, without the report's title
            administrative.sentCount == 4
            administrative.title == undefined
        }
    }
)

feature("a retry sends only the failed recipient, and an uncertain delivery waits to be reconciled", {
    when() {
        const scheduleId = String(one("moqui.screen.ScreenScheduled", {reportOwnerId: "RDA_OWNER"}).screenScheduledId)
        const occurrenceId = String(one("moqui.report.ReportOccurrence", {screenScheduledId: scheduleId}).occurrenceId)
        const row = userId => one("moqui.report.ReportRecipientDelivery", {occurrenceId, userId})
        const set = (userId, statusId, reasonCode) =>
            sys(() => {
                const delivery = row(userId)
                delivery.statusId = statusId
                delivery.reasonCode = reasonCode
                delivery.update()
            })
        // the transport's answers this spec cannot make: the French recipient's message failed, the equal one's is not known
        set("RDA_FRENCH", "RptDlvFailed", "transport-failure")
        set("RDA_EQUAL", "RptDlvUncertain", "uncertain")
        const revision = as("rda.owner", "get#ReportOccurrence", {occurrenceId}).out?.occurrence?.revision
        const deliveryIds = ["RDA_FRENCH", "RDA_EQUAL", "RDA_OWNER"].map(userId => String(row(userId).deliveryId))
        const stale = as("rda.owner", "retry#ReportRecipients", {occurrenceId, deliveryIds, expectedRevision: revision + 5})
        const notOwner = as("rda.equal", "retry#ReportRecipients", {occurrenceId, deliveryIds, expectedRevision: revision})
        capture.clear()
        const retried = as("rda.owner", "retry#ReportRecipients", {
            occurrenceId,
            deliveryIds,
            expectedRevision: revision,
            reason: "Mailbox was full"
        })
        const result = deliveryId => Array.from(retried.out?.results || []).find(entry => entry.deliveryId === deliveryId) || {}
        const sentTo = toLists()
        const reconciled = as("rda.owner", "reconcile#ReportRecipient", {
            deliveryId: deliveryIds[1],
            outcome: "sent",
            reason: "Confirmed by the recipient"
        })
        return {
            stale: stale.error || "",
            notOwner: notOwner.error || "",
            retriedError: retried.error || "",
            sentCount: retried.out?.sentCount,
            sentTo,
            french: result(deliveryIds[0]),
            uncertain: result(deliveryIds[1]),
            already: result(deliveryIds[2]),
            frenchAttempts: Number(row("RDA_FRENCH").attemptCount),
            reconciled: reconciled.out?.statusId,
            after: as("rda.owner", "get#ReportOccurrence", {occurrenceId}).out?.occurrence || {}
        }
    },
    then({stale, notOwner, retriedError, sentCount, sentTo, french, uncertain, already, frenchAttempts, reconciled, after}) {
        stale.includes("STALE_REVISION")
        notOwner.includes("not available")
        retriedError == ""
        // one message, to the failed recipient alone
        sentCount == 1
        sentTo == "rda.french@example.test"
        french.statusId == "RptDlvSent"
        frenchAttempts == 2
        uncertain.retried == false
        uncertain.reasonCode == "uncertain"
        already.reasonCode == "not-failed"
        reconciled == "RptDlvSent"
        after.statusId == "RptOccComplete"
        after.sentCount == 4
        after.uncertainCount == 0
    }
})

feature("an occurrence run again sends to nobody who was sent it, and leaves an outcome that is not known", {
    when() {
        const scheduleId = String(one("moqui.screen.ScreenScheduled", {reportOwnerId: "RDA_OWNER"}).screenScheduledId)
        // a run that ended before its last run time was kept: the schedule is due again for the same cron instant
        const due = () =>
            sys(() => {
                const lock = ec.entity.find("moqui.screen.ScreenScheduledLock").condition("screenScheduledId", scheduleId).one()
                lock.lastRunTime = null
                lock.update()
                return call(R + "run#ScheduledReport", {screenScheduledId: scheduleId})
            })
        due()
        const occurrenceId = String(
            one("moqui.report.ReportOccurrence", {screenScheduledId: scheduleId, occurrenceKind: "scheduled"}).occurrenceId
        )
        const row = userId => one("moqui.report.ReportRecipientDelivery", {occurrenceId, userId})
        const set = (userId, statusId, reasonCode) =>
            sys(() => {
                const delivery = row(userId)
                delivery.statusId = statusId
                delivery.reasonCode = reasonCode
                delivery.update()
            })
        set("RDA_FRENCH", "RptDlvFailed", "transport-failure")
        set("RDA_EQUAL", "RptDlvUncertain", "uncertain")
        capture.clear()
        due()
        return {
            sentTo: toLists(),
            occurrences: sys(() =>
                ec.entity
                    .find("moqui.report.ReportOccurrence")
                    .condition({screenScheduledId: scheduleId, occurrenceKind: "scheduled"})
                    .count()
            ),
            french: String(row("RDA_FRENCH").statusId),
            equal: String(row("RDA_EQUAL").statusId),
            ownerAttempts: Number(row("RDA_OWNER").attemptCount),
            statusId: String(one("moqui.report.ReportOccurrence", {occurrenceId}).statusId)
        }
    },
    then({sentTo, occurrences, french, equal, ownerAttempts, statusId}) {
        // the same occurrence, and one message: to the recipient whose delivery had failed
        occurrences == 1
        sentTo == "rda.french@example.test"
        french == "RptDlvSent"
        ownerAttempts == 1
        // not known is never sent again without a person's word
        equal == "RptDlvUncertain"
        statusId == "RptOccIncomplete"
    }
})

feature("a disabled owner's schedule pauses until a qualified owner takes it and its recipients pass the new owner's check", {
    when() {
        const scheduleId = String(one("moqui.screen.ScreenScheduled", {reportOwnerId: "RDA_OWNER"}).screenScheduledId)
        const owned = name =>
            Array.from(name.out?.resourceList || [])
                .map(resource => resource.resourceId)
                .join(",")
        const lookup = call(R + "find#OwnedSchedules", {userId: "RDA_OWNER"})
        const lookupRefused = as("rda.equal", "find#OwnedSchedules", {userId: "RDA_OWNER"})
        // the owner disabled, and the schedule due: the run finds its owner inactive, sends nothing and pauses it
        sys(() => {
            const account = ec.entity.find("moqui.security.UserAccount").condition("userId", "RDA_OWNER").one()
            account.disabled = "Y"
            account.update()
            const lock = ec.entity.find("moqui.screen.ScreenScheduledLock").condition("screenScheduledId", scheduleId).one()
            lock.lastRunTime = null
            lock.update()
            const row = ec.entity.find("moqui.screen.ScreenScheduled").condition("screenScheduledId", scheduleId).one()
            row.cronExpression = "0 * * * * ?"
            row.update()
        })
        capture.clear()
        const run = sys(() => call(R + "run#ScheduledReport", {screenScheduledId: scheduleId}))
        const atRun = one("moqui.screen.ScreenScheduled", {screenScheduledId: scheduleId})
        const runMessages = capture.count()
        // Admin's lifecycle: pause what the user owns (already paused here), then hand it on
        const paused = call(R + "pause#OwnedSchedules", {userId: "RDA_OWNER", reason: "Left the company"})
        const pausedOther = call(R + "pause#OwnedSchedules", {userId: "RDA_OTHER", reason: "On leave"})
        const equals = one("moqui.screen.ScreenScheduled", {reportOwnerId: "RDA_OTHER"})
        const transfer = toUserId => {
            try {
                return {
                    out: call(R + "transfer#DefinitionOwner", {
                        screenScheduledId: scheduleId,
                        fromUserId: "RDA_OWNER",
                        toUserId,
                        reason: "Offboarding"
                    })
                }
            } catch (error) {
                ec.message.clearErrors()
                return {error: String(error.message)}
            }
        }
        // the drill spec's reader reads the dashboard and its dataset but may not schedule
        const unqualified = transfer("RDR_READER_USER")
        const withoutPermission = as("rda.equal", "transfer#DefinitionOwner", {
            screenScheduledId: scheduleId,
            fromUserId: "RDA_OWNER",
            toUserId: "RDA_EQUAL"
        })
        const transferred = transfer("RDA_NEXT")
        const pending = one("moqui.screen.ScreenScheduled", {screenScheduledId: scheduleId})
        const sendPaused = as("rda.next", "send#Report", {screenScheduledId: scheduleId})
        // the team still holds the member whose scope differs: the new owner's resume is refused until they are gone
        const resumeRefused = as("rda.next", "pause#Schedule", {screenScheduledId: scheduleId, paused: false})
        sys(() => {
            const member = ec.entity
                .find("moqui.security.UserGroupMember")
                .condition({userGroupId: "RDA_TEAM", userId: "RDA_OTHER"})
                .one()
            member.delete()
        })
        const resumed = as("rda.next", "pause#Schedule", {screenScheduledId: scheduleId, paused: false})
        const after = one("moqui.screen.ScreenScheduled", {screenScheduledId: scheduleId})
        return {
            lookup: owned({out: lookup}),
            scheduleId,
            lookupRefused: lookupRefused.error || "",
            run,
            atRun: [atRun.paused, atRun.pausedReason].join(","),
            runMessages,
            pausedCount: paused.pausedCount,
            pausedOther: [pausedOther.pausedCount, equals.paused, equals.pausedReason].join(","),
            unqualified: unqualified.error || "",
            withoutPermission: withoutPermission.error || "",
            transferred: transferred.out || {},
            transferError: transferred.error || "",
            pending: [pending.reportOwnerId, pending.paused, pending.pausedReason].join(","),
            boardOwner: String(one("moqui.report.Definition", {definitionId: "rda-board"}).ownerId),
            sendPaused: sendPaused.error || "",
            resumeRefused: resumeRefused.error || "",
            resumed: resumed.error || "",
            after: [after.reportOwnerId, after.paused, after.pausedReason || ""].join(",")
        }
    },
    then({
        lookup,
        scheduleId,
        lookupRefused,
        run,
        atRun,
        runMessages,
        pausedCount,
        pausedOther,
        unqualified,
        withoutPermission,
        transferred,
        transferError,
        pending,
        boardOwner,
        sendPaused,
        resumeRefused,
        resumed,
        after
    }) {
        lookup == scheduleId
        lookupRefused.includes("REPORT_OWNER_CHANGE")
        run.skipped == true
        run.reason == "owner-inactive"
        atRun == "Y,owner-disabled"
        runMessages == 0
        // the run already paused it for its owner's sake
        pausedCount == 0
        pausedOther == "1,Y,owner-disabled"
        unqualified.includes("NEW_OWNER_REFUSED")
        withoutPermission.includes("REPORT_OWNER_CHANGE")
        transferError == ""
        transferred.transferred == true
        Array.from(transferred.definitionIds).join(",") == "rda-board"
        boardOwner == "RDA_NEXT"
        pending == "RDA_NEXT,Y,transfer-pending"
        sendPaused.includes("paused until its new owner resumes it")
        resumeRefused.includes("RECIPIENT_REFUSED")
        resumed == ""
        after == "RDA_NEXT,N,"
    }
})

feature(
    "a schedule over a business dataset made by create#Schedule is held to the same check at its run, and changed in its timing alone",
    {
        when() {
            // owner mode, as Tools writes it: another user named unchecked, whose scope differs from the owner's
            const created = as("rda.next", "create#Schedule", schedule({userId: "RDA_OTHER"}))
            capture.clear()
            const sent = as("rda.next", "send#Report", {screenScheduledId: created.out?.screenScheduledId})
            const found = as("rda.next", "find#ReportOccurrences", {screenScheduledId: created.out?.screenScheduledId})
            // an owner-mode schedule to a group and a location, its timing changed with no recipient list: the cron, format
            // and subject change, its group, location and template stay, and it stays in owner mode; another user is refused
            const grouped = as("rda.next", "create#Schedule", schedule({userGroupId: "RDA_TEAM"}))
            const groupedId = grouped.out?.screenScheduledId || "_NA_"
            sys(() => {
                const row = ec.entity
                    .find("moqui.screen.ScreenScheduled")
                    .condition("screenScheduledId", groupedId)
                    .useCache(false)
                    .one()
                row.saveToLocation = "dbresource://spec/reports"
                row.update()
            })
            const timing = {
                screenScheduledId: groupedId,
                cronExpression: "0 0 7 ? * TUE",
                renderMode: "xlsx",
                emailSubject: "Spec timing",
                emailTemplateId: "DASHBOARD_REPORT",
                expectedRevision: 0
            }
            const byOther = as("rda.equal", "store#Schedule", timing)
            const changed = as("rda.next", "store#Schedule", timing)
            return {
                createdError: created.error || "",
                sent,
                messages: capture.count(),
                occurrence: Array.from(found.out?.occurrences || [])[0] || {},
                byOther: byOther.error || "",
                changed,
                timed: one("moqui.screen.ScreenScheduled", {screenScheduledId: groupedId})
            }
        },
        then({createdError, sent, messages, occurrence, byOther, changed, timed}) {
            createdError == ""
            sent.error == undefined
            messages == 0
            occurrence.skippedCount == 1
            occurrence.sentCount == 0
            occurrence.deliveryMode == ""
            byOther.includes("report schedule not found for this owner")
            changed.error == undefined && changed.out.revision == 1
            timed.cronExpression == "0 0 7 ? * TUE" && timed.renderMode == "xlsx" && timed.emailSubject == "Spec timing"
            timed.userGroupId == "RDA_TEAM" &&
                timed.saveToLocation == "dbresource://spec/reports" &&
                timed.emailTemplateId == "RDA_REPORT"
            !timed.deliveryMode && !timed.userId && timed.versionNumber == 1
        }
    }
)

feature(
    "recipients are proposed and accepted by the engine's rule or among a permission's holders, and a screen schedule is its user's alone",
    {
        when() {
            sys(() => {
                const from = new Date(Date.now() - 60000)
                // a permission the team holds (the disabled first owner among them), and an account that holds it and is
                // not internal
                make("moqui.security.UserPermission", {userPermissionId: "RDA_RECEIVE", description: "Report audience spec"})
                make("moqui.security.UserGroup", {userGroupId: "RDA_EXTERNAL", description: "Report audience spec, external"})
                for (const [userGroupId, userPermissionId] of [
                    ["RDA_TEAM", "RDA_RECEIVE"],
                    ["RDA_EXTERNAL", "RDA_RECEIVE"],
                    ["RDA_EXTERNAL", "EXTERNAL_USER"]
                ])
                    make("moqui.security.UserGroupPermission", {userGroupId, userPermissionId, fromDate: from})
                make("moqui.security.UserAccount", {
                    userId: "RDA_GUEST",
                    username: "rda.guest",
                    userFullName: "rda.guest",
                    emailAddress: address("rda.guest"),
                    disabled: "N"
                })
                make("moqui.security.UserGroupMember", {userGroupId: "RDA_EXTERNAL", userId: "RDA_GUEST", fromDate: from})
                // a screen schedule of the equal recipient's own, naming a group, and one of another user's
                for (const [screenScheduledId, userId] of [
                    ["RDA_SCREEN", "RDA_EQUAL"],
                    ["RDA_SCREEN_OTHER", "RDA_OTHER"]
                ])
                    make("moqui.screen.ScreenScheduled", {
                        screenScheduledId,
                        screenPath: "/apps/rda/SpecList",
                        userId,
                        userGroupId: "RDA_TEAM",
                        cronExpression: "0 0 6 ? * MON",
                        renderMode: "csv",
                        emailSubject: "Weekly list",
                        paused: "Y"
                    })
            })
            const named = ["RDA_EQUAL", "RDA_OTHER", "RDA_OWNER", "RDA_GUEST", "RDA_NOBODY"]
            const idsOf = list =>
                Array.from(list || [])
                    .map(row => row.userId || row.userGroupId || row)
                    .sort()
                    .join(",")
            const recipients = (username, parameters) => as(username, "find#ReportRecipients", parameters)
            // the engine's rule: active internal accounts, found by name ignoring case, with the caller's own groups
            const any = recipients("rda.next", {term: "RDA.", userIds: named, groupIds: ["RDA_TEAM", "ADMIN"]})
            // an app's readers: only the holders of the permission named, whatever user is named
            const holders = recipients("rda.next", {permissionId: "RDA_RECEIVE", term: "rda.", userIds: named})
            const unheld = recipients("rda.next", {permissionId: "RDA_NOT_HELD", userIds: ["RDA_EQUAL", "RDA_NEXT"]})
            // a caller who schedules but may not deliver to others, and one who is not granted the read
            const noGrant = recipients("rda.other", {
                term: "rda.",
                userIds: ["RDA_EQUAL", "RDA_OTHER"],
                groupIds: ["RDA_SENDERS"]
            })
            const noRead = recipients("rdr.reader", {term: "rda."})
            // the cap holds for whoever is accepted: a team of four checked with a cap of two
            const overCap = as("rda.next", "store#Schedule", schedule({recipientGroupIds: ["RDA_TEAM"], recipientCap: 2}))
            const row = id => one("moqui.screen.ScreenScheduled", {screenScheduledId: id})
            const reportScheduleId = String(one("moqui.screen.ScreenScheduled", {reportOwnerId: "RDA_NEXT"}).screenScheduledId)
            const mine = as("rda.equal", "find#ScreenSchedules", {})
            const others = as("rda.equal", "find#ScreenSchedules", {screenScheduledId: "RDA_SCREEN_OTHER"})
            const change = {cronExpression: "0 0 7 ? * *", renderMode: "xlsx", paused: false, clearFields: ["emailSubject"]}
            const store = (username, screenScheduledId, values) =>
                as(username, "store#ScreenSchedule", Object.assign({screenScheduledId}, values || change)).error || ""
            const refused = {
                anothers: store("rda.equal", "RDA_SCREEN_OTHER"),
                report: store("rda.next", reportScheduleId),
                missing: store("rda.equal", "RDA_NOT_THERE"),
                cron: store("rda.equal", "RDA_SCREEN", {cronExpression: "every monday"}),
                format: store("rda.equal", "RDA_SCREEN", {renderMode: "exe"}),
                subject: store("rda.equal", "RDA_SCREEN", {emailSubject: "List ${ec.user.userId}"}),
                noChange: store("rdr.reader", "RDA_SCREEN")
            }
            const before = row("RDA_SCREEN")
            const untouched = [before.cronExpression, before.renderMode, before.paused, before.emailSubject].join("|")
            const stored = store("rda.equal", "RDA_SCREEN")
            const after = row("RDA_SCREEN")
            return {
                any: any.out || {},
                anyError: any.error || "",
                anyUsers: idsOf(any.out?.users),
                anySelected: idsOf(any.out?.selectedUsers),
                anyGroups: idsOf(any.out?.groups),
                holders: holders.out || {},
                holderUsers: idsOf(holders.out?.users),
                holderSelected: idsOf(holders.out?.selectedUsers),
                unheld: unheld.out || {},
                noGrant: noGrant.out || {},
                noGrantSelected: idsOf(noGrant.out?.selectedUsers),
                noRead: noRead.error || "",
                overCap: overCap.error || "",
                mine: Array.from(mine.out?.schedules || []),
                others: Array.from(others.out?.schedules || []).length,
                refused,
                untouched,
                stored,
                after: [after.cronExpression, after.renderMode, after.paused, after.emailSubject || ""].join("|"),
                kept: [after.screenPath, after.userId, after.userGroupId].join("|"),
                otherRow: String(row("RDA_SCREEN_OTHER").cronExpression)
            }
        },
        then({
            any,
            anyError,
            anyUsers,
            anySelected,
            anyGroups,
            holders,
            holderUsers,
            holderSelected,
            unheld,
            noGrant,
            noGrantSelected,
            noRead,
            overCap,
            mine,
            others,
            refused,
            untouched,
            stored,
            after,
            kept,
            otherRow
        }) {
            anyError == ""
            any.canDeliver == true
            // the disabled owner and the external account are neither proposed nor accepted, nor an id that names nobody
            anyUsers == "RDA_EQUAL,RDA_FRENCH,RDA_NEXT,RDA_OTHER"
            anySelected == "RDA_EQUAL,RDA_OTHER"
            Array.from(any.refusedUserIds).sort().join(",") == "RDA_GUEST,RDA_NOBODY,RDA_OWNER"
            // an account is a name: no address, group or grant of anyone's
            Object.keys(any.users[0]).sort().join(",") == "name,userId,username"
            // the caller's own groups, and a group that is not theirs refused
            anyGroups == "RDA_GROUP,RDA_SENDERS,RDA_TEAM,RDR_A,RDR_READER"
            Array.from(any.refusedGroupIds).join(",") == "ADMIN"
            // under a permission only its holders: the other scope's user is active and internal, and holds none
            holderUsers == "RDA_EQUAL,RDA_FRENCH,RDA_NEXT"
            holderSelected == "RDA_EQUAL"
            Array.from(holders.refusedUserIds).sort().join(",") == "RDA_GUEST,RDA_NOBODY,RDA_OTHER,RDA_OWNER"
            // a permission nobody holds accepts the caller alone
            Array.from(unheld.users).length == 0
            Array.from(unheld.refusedUserIds).join(",") == "RDA_EQUAL"
            // without the group delivery grant: no account and no group, everyone else refused, oneself accepted
            noGrant.canDeliver == false
            Array.from(noGrant.users).length == 0 && Array.from(noGrant.groups).length == 0
            noGrantSelected == "RDA_OTHER"
            Array.from(noGrant.refusedUserIds).join(",") == "RDA_EQUAL"
            Array.from(noGrant.refusedGroupIds).join(",") == "RDA_SENDERS"
            noRead.includes("not authorized")
            overCap.includes("RECIPIENT_CAP")
            // the caller's own screen schedule, with why its group is not sent to; never another user's
            mine.length == 1
            mine[0].screenScheduledId == "RDA_SCREEN"
            mine[0].paused == true && mine[0].userGroupId == "RDA_TEAM" && mine[0].groupAdmitted == false
            others == 0
            refused.anothers.includes("SCHEDULE_UNAVAILABLE")
            refused.report.includes("SCHEDULE_UNAVAILABLE")
            refused.missing.includes("SCHEDULE_UNAVAILABLE")
            refused.cron.includes("VALUE_INVALID")
            refused.format.includes("VALUE_INVALID")
            refused.subject.includes("VALUE_INVALID")
            refused.noChange.includes("not authorized")
            untouched == "0 0 6 ? * MON|csv|Y|Weekly list"
            // the same change by its own user: the timing, format, subject and pause, and nothing else of the row
            stored == ""
            after == "0 0 7 ? * *|xlsx|N|"
            kept == "/apps/rda/SpecList|RDA_EQUAL|RDA_TEAM"
            otherRow == "0 0 6 ? * MON"
        }
    }
)

feature("a definition is shared with a group of the saver's own, or one their own grant lets them name", {
    when() {
        const board = (id, userGroupId, title) => ({
            definition: {id, type: "dashboard", title: title || id, userGroupId, body: {panels: {}}},
            note: "Report audience spec"
        })
        const group = id => one("moqui.report.Definition", {definitionId: id})?.userGroupId || null
        const error = result => result.error || ""
        // rda.other saves and is in RDA_SENDERS, RDR_B and RDR_READER; not in the team, and holds no share artifact
        const own = error(as("rda.other", "save#Definition", board("rda-share-other", "RDA_SENDERS")))
        const refused = ["RDA_TEAM", "RDA_WALL", "ALL_USERS", "RDA_NO_SUCH"].map(userGroupId =>
            error(as("rda.other", "save#Definition", board("rda-share-other", userGroupId)))
        )
        const afterRefused = group("rda-share-other")
        const offered = as("rda.other", "find#ShareGroups", {groupIds: ["RDA_WALL", "RDA_TEAM"]}).out
        // a group the definition already has is kept by a later save, though its owner could not name it now
        sys(() => {
            ec.user.internalLoginUser("rda.other")
            try {
                call(R + "save#Definition", board("rda-share-kept", "RDA_TEAM"))
            } finally {
                login("john.doe")
            }
        })
        const kept = error(as("rda.other", "save#Definition", board("rda-share-kept", "RDA_TEAM", "Kept")))
        const moved = error(as("rda.other", "save#Definition", board("rda-share-kept", "RDA_WALL", "Moved")))
        const afterKept = one("moqui.report.Definition", {definitionId: "rda-share-kept"})
        // rda.next holds the wall group's share artifact (RDA_GROUP), and no other group's
        const granted = error(as("rda.next", "save#Definition", board("rda-share-next", "RDA_WALL")))
        const ungranted = error(as("rda.next", "save#Definition", board("rda-share-next", "RDR_B")))
        const afterGranted = group("rda-share-next")
        const named = as("rda.next", "find#ShareGroups", {groupIds: ["RDA_WALL", "RDR_B", "RDA_NO_SUCH"]}).out
        const priv = error(as("rda.next", "save#Definition", board("rda-share-next", null)))
        return {
            own,
            refused,
            afterRefused,
            offeredOwn: Array.from(offered.groups)
                .filter(row => row.own)
                .map(row => row.userGroupId)
                .sort()
                .join(","),
            offeredOther: Array.from(offered.groups).filter(row => !row.own).length,
            offeredRefused: Array.from(offered.refusedGroupIds).sort().join(","),
            kept,
            moved,
            keptGroup: afterKept.userGroupId,
            keptTitle: afterKept.title,
            granted,
            ungranted,
            afterGranted,
            namedWall: Array.from(named.groups)
                .filter(row => !row.own)
                .map(row => row.userGroupId + ":" + row.description)
                .join(","),
            namedRefused: Array.from(named.refusedGroupIds).sort().join(","),
            priv,
            afterPrivate: group("rda-share-next")
        }
    },
    then({
        own,
        refused,
        afterRefused,
        offeredOwn,
        offeredOther,
        offeredRefused,
        kept,
        moved,
        keptGroup,
        keptTitle,
        granted,
        ungranted,
        afterGranted,
        namedWall,
        namedRefused,
        priv,
        afterPrivate
    }) {
        own == ""
        // a group the saver is not in, one no person is in, everyone's implicit group and one that does not exist
        refused.every(message => message.includes("SHARE_GROUP_UNAVAILABLE"))
        afterRefused == "RDA_SENDERS"
        offeredOwn == "RDA_SENDERS,RDR_B,RDR_READER"
        offeredOther == 0
        offeredRefused == "RDA_TEAM,RDA_WALL"
        kept == ""
        moved.includes("SHARE_GROUP_UNAVAILABLE")
        keptGroup == "RDA_TEAM" && keptTitle == "Kept"
        // by the share artifact of that group alone
        granted == ""
        ungranted.includes("SHARE_GROUP_UNAVAILABLE")
        afterGranted == "RDA_WALL"
        namedWall == "RDA_WALL:Report audience spec RDA_WALL"
        namedRefused == "RDA_NO_SUCH,RDR_B"
        priv == ""
        afterPrivate == null
    }
})
