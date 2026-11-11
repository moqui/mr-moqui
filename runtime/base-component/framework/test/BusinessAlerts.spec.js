// Owner-equivalent business alerts (crates/moqui-report/src/alert_delivery.rs; docs/tools/Monitoring.md "Business alerts"; BOSS
// impl-insights P8, INS-07). A rule over a dataset whose rows depend on the viewer is evaluated once, as its owner; it takes an
// audience of internal users and groups in place of contact points, each recipient is checked as check#ReportAudience checks a
// schedule's (at save and again at each notification) and told separately, one whose scope differs is skipped, the recipient
// cap holds, No Data and Error are never Normal, and nobody but the owner and an equivalent recipient reads a value: not the
// delivery rows, not the administrative list. A copy is paused until its owner activates it, and a rule whose owner left is
// paused and handed to a qualified owner. The dataset, its scope groups and the users are ReportDrill's and ReportAudience's
// (after it rda.next owns, rda.equal and rda.french read what rda.next reads, rda.other reads another scope and may not deliver
// to a group, RDA_OWNER is disabled). Run: ./moq test framework --spec BusinessAlerts
const R = "org.moqui.report.ReportServices."
const notes = ec.factory.getTool("NotificationCapture")

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
// records by region of the scoped dataset: north 2, south 1 and east 1 for the RDR_A scope
const rule = (id, extra, query) => ({
    definition: {
        id,
        type: "alert",
        title: "Sales by region",
        userGroupId: "RDR_READER",
        body: Object.assign(
            {
                query: Object.assign(
                    {
                        datasetId: "rdr-sales",
                        from: "now-1d",
                        thru: "now+1d",
                        aggregations: [{id: "region", type: "terms", params: {field: "relatedEnumId"}}]
                    },
                    query
                ),
                reduce: "last",
                condition: {type: "above", threshold: 1},
                intervalSeconds: 60,
                detailLink: "/apps/test/AlertDetail"
            },
            extra
        )
    }
})
const team = {recipientGroupIds: ["RDA_TEAM"], channels: ["notification"], groupWaitSeconds: 0}
let clock = Date.now()
// one evaluation of a rule as its reader, a few minutes after the one before
function evaluate(username, definitionId) {
    clock += 300000
    ec.user.setEffectiveTime(clock)
    try {
        return as(username, "evaluate#Alerts", {definitionId})
    } finally {
        ec.user.setEffectiveTime(null)
    }
}
const told = () =>
    Array.from(notes.getMessages("ReportAlert")).map(message => ({
        to: Array.from(message.userIds || []).join("+"),
        title: String(message.title),
        link: String(message.link),
        body: typeof message.message === "string" ? JSON.parse(message.message) : message.message
    }))
const states = definitionId =>
    Array.from(sys(() => ec.entity.find("moqui.report.AlertInstance").condition("definitionId", definitionId).list()))
        .map(row => String(row.state))
        .sort()
        .join(",")

spec({
    stepwise: true,
    dependsOn: ["ReportAudience"],
    setupSpec() {
        login("john.doe")
        notes.enable()
    },
    cleanupSpec() {
        notes.disable()
        notes.clear()
        ec.user.setEffectiveTime(null)
        ec.message.clearErrors()
        ec.artifactExecution.enableAuthz()
    }
})

feature("a business rule takes an audience, never a contact point, and is saved only when every recipient may be told", {
    when() {
        sys(() => {
            for (const name of [
                "get#AlertState",
                "find#AlertDeliveries",
                "evaluate#Alerts",
                "copy#Definition",
                "get#Definition",
                "find#OwnedAlertRules",
                "transfer#AlertRuleOwner"
            ])
                make("moqui.security.ArtifactGroupMember", {
                    artifactGroupId: "RDA_SENDERS",
                    artifactName: R + name,
                    artifactTypeEnumId: "AT_SERVICE"
                })
        })
        const save = (username, id, extra) => as(username, "save#Definition", rule(id, extra))
        const webhook = save("rda.next", "rba-rule", {contactPoints: [{type: "webhook", url: "https://example.test/hook"}]})
        const address = save("rda.next", "rba-rule", {audience: {toAddresses: "someone@example.test"}})
        const channel = save("rda.next", "rba-rule", {audience: {channels: ["webhook"]}})
        const narrower = save("rda.next", "rba-rule", {audience: {recipientUserIds: ["RDA_EQUAL", "RDA_OTHER"]}})
        const overCap = save("rda.next", "rba-rule", {audience: Object.assign({recipientCap: 1}, team)})
        const link = save("rda.next", "rba-rule", {detailLink: "https://example.test/x", audience: team})
        // one who saves rules but holds no group delivery grant: refused for others, saved for themselves with no audience named
        const noGrant = save("rda.other", "rba-own", {audience: {recipientUserIds: ["RDA_EQUAL"]}})
        const personal = save("rda.other", "rba-own", {})
        const stored = save("rda.next", "rba-rule", {audience: team})
        const checked = as("rda.next", "check#ReportAudience", {
            definitionId: "rba-rule",
            recipientUserIds: ["RDA_EQUAL", "RDA_OTHER"]
        })
        return {
            webhook: webhook.error || "",
            address: address.error || "",
            channel: channel.error || "",
            narrower: narrower.error || "",
            overCap: overCap.error || "",
            link: link.error || "",
            noGrant: noGrant.error || "",
            personal: personal.out?.definition?.body?.audience || {},
            personalError: personal.error || "",
            stored: stored.out?.definition?.body?.audience || {},
            storedError: stored.error || "",
            checked: Array.from(checked.out?.recipients || [])
                .map(recipient => recipient.userId + ":" + recipient.reasonCode)
                .join(","),
            rules: sys(() => ec.entity.find("moqui.report.Definition").condition("definitionId", "like", "rba-%").count())
        }
    },
    then({
        webhook,
        address,
        channel,
        narrower,
        overCap,
        link,
        noGrant,
        personal,
        personalError,
        stored,
        storedError,
        checked,
        rules
    }) {
        webhook.includes("BUSINESS_CONTACT_REFUSED")
        address.includes("BUSINESS_AUDIENCE_REFUSED")
        channel.includes("BUSINESS_CONTACT_REFUSED")
        narrower.includes("RECIPIENT_REFUSED")
        narrower.includes("scope-differs")
        overCap.includes("RECIPIENT_CAP")
        link.includes("detailLink")
        noGrant.includes("GROUP_DELIVERY_DENIED")
        personalError == ""
        // the server names the mode: a rule over business data with no audience named is its owner's alone
        personal.deliveryMode == "personal"
        Array.from(personal.channels).join(",") == "notification"
        storedError == ""
        stored.deliveryMode == "group"
        checked == "RDA_EQUAL:,RDA_OTHER:scope-differs"
        // only the two saves that passed left a rule
        rules == 2
    }
})

feature("one evaluation as the owner tells each equivalent recipient separately with the value, and skips a narrower one", {
    when() {
        // a member whose scope differs joins the team after the save
        sys(() =>
            make("moqui.security.UserGroupMember", {
                userGroupId: "RDA_TEAM",
                userId: "RDA_OTHER",
                fromDate: new Date(Date.now() - 60000)
            })
        )
        notes.clear()
        const run = evaluate("rda.next", "rba-rule")
        const messages = told()
        const owned = as("rda.next", "find#AlertDeliveries", {definitionId: "rba-rule"})
        const deliveries = Array.from(owned.out?.deliveries || [])
        const outcome = userId => deliveries.find(delivery => delivery.userId === userId) || {}
        const equal = as("rda.equal", "get#AlertState", {definitionId: "rba-rule"})
        const other = as("rda.other", "get#AlertState", {definitionId: "rba-rule"})
        const received = as("rda.equal", "find#AlertDeliveries", {received: true})
        const everyOwners = as("rda.equal", "find#AlertDeliveries", {administrative: true})
        const administrative = call(R + "find#AlertDeliveries", {administrative: true, definitionId: "rba-rule"})
        const north = Array.from(equal.out?.instances || []).find(instance => instance.state === "Firing") || {}
        return {
            runError: run.error || "",
            run: run.out || {},
            to: messages
                .map(message => message.to)
                .sort()
                .join(","),
            first: messages[0] || {body: {}},
            states: states("rba-rule"),
            other: outcome("RDA_OTHER"),
            disabled: outcome("RDA_OWNER"),
            sent: deliveries.filter(delivery => delivery.statusId === "RptDlvSent").length,
            deliveryText: JSON.stringify(deliveries),
            equal: equal.out || {},
            north,
            otherState: other.out || {},
            received: Array.from(received.out?.deliveries || [])
                .map(delivery => delivery.userId + ":" + delivery.alertState)
                .join(","),
            everyOwners: everyOwners.error || "",
            administrative: Array.from(administrative.deliveries || [])[0] || {},
            administrativeText: JSON.stringify(administrative.deliveries || [])
        }
    },
    then({
        runError,
        run,
        to,
        first,
        states,
        other,
        disabled,
        sent,
        deliveryText,
        equal,
        north,
        otherState,
        received,
        everyOwners,
        administrative,
        administrativeText
    }) {
        runError == ""
        run.evaluated == 1
        run.failed == 0
        // one state, three notifications, one user each
        states == "Firing,Normal,Normal"
        to == "RDA_EQUAL,RDA_FRENCH,RDA_NEXT"
        first.title.includes("Firing")
        first.link == "/apps/test/AlertDetail?definitionId=rba-rule"
        first.body.ruleId == "rba-rule"
        first.body.events[0].value == 2
        String(first.body.events[0].seriesId || "").length > 0
        sent == 3
        other.statusId == "RptDlvSkipped"
        other.reasonCode == "scope-differs"
        !/RDR_|RDA_GROUP|enumCode/.test(other.reason)
        disabled.reasonCode == "inactive"
        // evidence only: the state's name, no value and no series label
        !/"value"|north|labels|scopeHash/.test(deliveryText)
        equal.available == true
        equal.own == false
        north.value == 2
        north.labels.region == "north"
        // a reader of the rule who is not equivalent learns no state
        otherState.available == false
        otherState.reasonCode == "scope-differs"
        Array.from(otherState.instances).length == 0
        received == "RDA_EQUAL:Firing"
        everyOwners.includes("ADMIN_TRANSFER_VIEW")
        // an administrator's list is counters by rule: no title, series, recipient or value
        administrative.sentCount == 3
        administrative.skippedCount == 2
        !/Sales by region|north|RDA_EQUAL|"value"|title/.test(administrativeText)
    }
})

feature(
    "a source that cannot be read is Error and an empty result is No Data, never Normal, and a told state is not told twice",
    {
        when() {
            // the same state again within the repeat interval: nobody is told twice
            notes.clear()
            evaluate("rda.next", "rba-rule")
            const repeated = told().length
            // the dataset's fields changed after its index was built: no valid reading until it is rebuilt
            const admission = () => ec.entity.find("moqui.report.BusinessDataset").condition("datasetId", "rdr-sales").one()
            sys(() => {
                const row = admission()
                row.fieldsChangedDate = new Date(Date.now() + 3600000)
                row.update()
            })
            notes.clear()
            const outage = evaluate("rda.next", "rba-rule")
            const outageStates = states("rba-rule")
            const outageTitles = Array.from(new Set(told().map(message => message.title))).join("|")
            sys(() => {
                const row = admission()
                row.fieldsChangedDate = null
                row.update()
            })
            // a rule whose query finds nothing has no data, which is not health
            as(
                "rda.next",
                "save#Definition",
                rule("rba-empty", {audience: {groupWaitSeconds: 0}}, {queryString: "relatedEnumId:nowhere"})
            )
            notes.clear()
            const empty = evaluate("rda.next", "rba-empty")
            return {
                repeated,
                outageError: outage.error || "",
                outageStates,
                outageTitles,
                emptyError: empty.error || "",
                emptyStates: states("rba-empty"),
                emptyTitles: told()
                    .map(message => message.to + ":" + message.title)
                    .join("|")
            }
        },
        then({repeated, outageError, outageStates, outageTitles, emptyError, emptyStates, emptyTitles}) {
            repeated == 0
            outageError == ""
            outageStates == "Error,Error,Error"
            outageTitles.includes("Error")
            !outageTitles.includes("Normal")
            emptyError == ""
            emptyStates == "NoData"
            emptyTitles.startsWith("RDA_NEXT:No data")
        }
    }
)

feature("a copy waits for its owner, and a leaving owner's rule is paused and handed to a qualified owner who resumes it", {
    when() {
        const body = id => JSON.parse(String(one("moqui.report.Definition", {definitionId: id}).bodyJson))
        const copied = as("rda.equal", "copy#Definition", {definitionId: "rba-rule", newDefinitionId: "rba-copy"})
        const lookup = call(R + "find#OwnedAlertRules", {userId: "RDA_NEXT"})
        const lookupRefused = as("rda.equal", "find#OwnedAlertRules", {userId: "RDA_NEXT"})
        // an owner who can no longer sign in: the evaluation tells nobody and pauses the rule
        sys(() => {
            const account = ec.entity.find("moqui.security.UserAccount").condition("userId", "RDA_OTHER").one()
            account.disabled = "Y"
            account.update()
        })
        notes.clear()
        clock += 300000
        ec.user.setEffectiveTime(clock)
        const pass = call(R + "evaluate#Alerts", {})
        ec.user.setEffectiveTime(null)
        const ownerGone = body("rba-own")
        sys(() => {
            const account = ec.entity.find("moqui.security.UserAccount").condition("userId", "RDA_OTHER").one()
            account.disabled = "N"
            account.update()
        })
        const paused = call(R + "pause#OwnedAlertRules", {userId: "RDA_NEXT", reason: "Left the company"})
        const transfer = (username, toUserId) => {
            ec.user.internalLoginUser(username)
            try {
                return {
                    out: call(R + "transfer#AlertRuleOwner", {
                        definitionId: "rba-rule",
                        fromUserId: "RDA_NEXT",
                        toUserId,
                        reason: "Offboarding"
                    })
                }
            } catch (error) {
                ec.message.clearErrors()
                return {error: String(error.message)}
            } finally {
                login("john.doe")
            }
        }
        // the drill spec's reader reads the dataset but may not save a rule
        const unqualified = transfer("john.doe", "RDR_READER_USER")
        const withoutPermission = as("rda.equal", "transfer#AlertRuleOwner", {
            definitionId: "rba-rule",
            fromUserId: "RDA_NEXT",
            toUserId: "RDA_EQUAL"
        })
        const transferred = transfer("john.doe", "RDA_EQUAL")
        const handed = body("rba-rule")
        const pending = [
            one("moqui.report.Definition", {definitionId: "rba-rule"}).ownerId,
            handed.paused,
            handed.pausedReason
        ].join(",")
        const resume = () => {
            const current = as("rda.equal", "get#Definition", {definitionId: "rba-rule"}).out?.definition
            if (!current) return {error: "not read"}
            current.body.paused = false
            return as("rda.equal", "save#Definition", {definition: current})
        }
        // the team still holds the member whose scope differs: the new owner's resume is refused until they are gone
        const resumeRefused = resume()
        sys(() =>
            ec.entity
                .find("moqui.security.UserGroupMember")
                .condition({userGroupId: "RDA_TEAM", userId: "RDA_OTHER"})
                .one()
                .delete()
        )
        const resumed = resume()
        const after = body("rba-rule")
        return {
            copyError: copied.error || "",
            copy: copied.out?.definition?.body || {},
            lookup: Array.from(lookup.resourceList || [])
                .map(resource => resource.resourceId + ":" + resource.resourceKind)
                .join(","),
            lookupRefused: lookupRefused.error || "",
            passFailed: pass.failed,
            passTold: told().filter(message => message.to === "RDA_OTHER").length,
            ownerGone: [ownerGone.paused, ownerGone.pausedReason].join(","),
            pausedCount: paused.pausedCount,
            unqualified: unqualified.error || "",
            withoutPermission: withoutPermission.error || "",
            transferError: transferred.error || "",
            transferred: transferred.out || {},
            pending,
            resumeRefused: resumeRefused.error || "",
            resumed: resumed.error || "",
            after: [after.paused, after.pausedReason || "", after.audience.deliveryMode].join(",")
        }
    },
    then({
        copyError,
        copy,
        lookup,
        lookupRefused,
        passFailed,
        passTold,
        ownerGone,
        pausedCount,
        unqualified,
        withoutPermission,
        transferError,
        transferred,
        pending,
        resumeRefused,
        resumed,
        after
    }) {
        copyError == ""
        copy.paused == true
        copy.pausedReason == "not-validated"
        lookup == "rba-empty:alert,rba-rule:alert"
        lookupRefused.includes("REPORT_OWNER_CHANGE")
        passFailed == 0
        passTold == 0
        ownerGone == "true,owner-disabled"
        pausedCount == 2
        unqualified.includes("NEW_OWNER_REFUSED")
        withoutPermission.includes("REPORT_OWNER_CHANGE")
        transferError == ""
        transferred.transferred == true
        pending == "RDA_EQUAL,true,transfer-pending"
        resumeRefused.includes("RECIPIENT_REFUSED")
        resumed == ""
        after == "false,,group"
    }
})
