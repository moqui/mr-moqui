// What the delivery screens of Dashboards show and ask (screen/Dashboards/Schedules.xml, ScheduledReportDetail.xml and
// ReportDelivery.xml; docs/tools/Monitoring.md, "Report delivery"; plans/archive/Admin-Screens-Down.md, Track I): the viewer's
// report schedules and the reports delivered to them, one schedule with its recipients, their check and its runs, and one
// run with each recipient's outcome. Presentation only: every read and write is an operation of
// org.moqui.report.ReportServices called as the viewer, so the engine checks the operation's grant, the schedule's owner,
// each dataset and each recipient's scope, and no row keeps a report; no entity is read here.
//
// What is offered follows the viewer's own grant of each operation (offered below), so that nothing is asked that would
// only be refused: reading deliveries (find#ReportOccurrences), scheduling for oneself (list#Schedules, store#Schedule),
// and delivering to others (find#ReportRecipients' canDeliver, the engine's group delivery artifact).
//
// Another app shows the screens under its own root (BOSS Insights › Scheduled reports). Its policy (template/ReportMount.js)
// is read here, in the transitions too, and never from a request: whom a schedule's owner may propose and name
// (recipientPermissionId, through find#ReportRecipients), the EmailTemplate a schedule saved here is delivered with
// (reportEmailTemplateId) and the most recipients it is saved and checked with (recipientCap, which the app's delivery
// area adds). The definitions of the components the mount hides (reportHiddenComponents, as the dashboard list reads it)
// are not scheduled there. The root sets the maps, so these hold for every request the app's route takes.
const R = "org.moqui.report.ReportServices."
const mountLib = ec.resource.script("component://tools/template/ReportMount.js")
const refusalLength = 400
const receivedPageSize = 20
const formats = [
    {value: "pdf", label: "PDF"},
    {value: "xlsx", label: "Excel workbook"},
    {value: "csv", label: "CSV"},
    {value: "png", label: "Picture"}
]
// what a screen schedule renders as (org.moqui.impl.ScreenServices render#ScheduledScreen)
const screenFormats = [
    {value: "xlsx", label: "Excel workbook"},
    {value: "csv", label: "CSV"},
    {value: "xsl-fo", label: "PDF"}
]
const cronPresets = [
    {value: "0 0 6 ? * *", label: "Daily at 6 AM"},
    {value: "0 0 6 ? * MON-FRI", label: "Weekdays at 6 AM"},
    {value: "0 0 6 ? * MON", label: "Weekly on Monday at 6 AM"},
    {value: "0 0 6 1 * ?", label: "Monthly on the 1st at 6 AM"}
]
const deliveryStates = {
    RptDlvPending: ["Pending", "grey-7"],
    RptDlvSending: ["Sending", "info"],
    RptDlvSent: ["Sent", "positive"],
    RptDlvSkipped: ["Skipped", "grey-7"],
    RptDlvFailed: ["Failed", "negative"],
    RptDlvUncertain: ["Not known", "warning"]
}
const occurrenceStates = {
    RptOccRunning: ["Running", "info"],
    RptOccComplete: ["Complete", "positive"],
    RptOccIncomplete: ["Incomplete", "warning"],
    RptOccSkipped: ["Skipped", "grey-7"],
    RptOccFailed: ["Failed", "negative"]
}
const occurrenceReasons = {
    empty: "The report had no data, so nothing was sent",
    "owner-inactive": "Its owner is not active, so it was paused",
    "source-failure": "The report could not be rendered",
    "run-ended": "The run ended before it finished",
    skipped: "Nothing was due"
}
const pauseReasons = {
    "owner-disabled": "Paused until a new owner takes it",
    "transfer-pending": "Paused until its new owner resumes it"
}
const unread = "A report you do not read"

const text = value => String(value ?? "").trim()
const idList = value =>
    (value == null || value === "" ? [] : Array.from(typeof value === "string" ? value.split(",") : value))
        .map(text)
        .filter(Boolean)
const flag = value => value === true || value === "true" || value === "Y"
const dateText = millis => (millis == null || millis === "" ? "" : ec.l10n.format(new Date(Number(millis)), "yyyy-MM-dd HH:mm"))
function cronText(cron) {
    if (!cron) return ""
    try {
        return String(ec.service.getCronDescription(String(cron)))
    } catch (error) {
        return String(cron)
    }
}
const formatText = (list, value) => list.find(format => format.value === value)?.label || value || ""

// ===== the engine =====
// A refusal's own words: the first line of the engine's message, without the service's name and the code before it.
function reason(message) {
    const line = String(message ?? "")
        .split("\n")[0]
        .replace(/^service \S+ failed: /, "")
        .replace(/^Error: /, "")
        .replace(/^[A-Z][A-Z0-9_]+:\s*/, "")
        .trim()
    const words = line.charAt(0).toUpperCase() + line.slice(1)
    return words.length <= refusalLength ? words : words.slice(0, refusalLength) + "…"
}
// An engine operation as the viewer, in a transaction and an error frame of its own: {out, failure}, failure the
// refusal's words (the engine's own checks decide, and a screen shows why).
function ask(name, parameters) {
    ec.message.pushErrors()
    try {
        const out =
            ec.service
                .sync()
                .name(R + name)
                .parameters(parameters || {})
                .requireNewTransaction(true)
                .call() || {}
        if (!ec.message.hasError()) return {out, failure: null}
        return {out: null, failure: reason(ec.message.getErrorsString())}
    } catch (error) {
        return {out: null, failure: reason(error?.message ?? error)}
    } finally {
        ec.message.clearErrors()
        ec.message.popErrors()
    }
}
// Whether the viewer's own grant opens an operation, by the action its verb asks (the service facade's). A screen
// offers what this answers and asks nothing else, so a viewer who may only read their deliveries is not refused a list
// of schedules at every render; the operation still decides when it is called.
const verbActions = {
    create: "create",
    update: "update",
    store: "update",
    delete: "delete",
    view: "view",
    find: "view",
    get: "view",
    search: "view"
}
function offered(name) {
    const verb = name.split("#")[0]
    const action = Object.hasOwn(verbActions, verb) ? verbActions[verb] : "all"
    return ec.artifactExecution.isPermittedOwn(R + name, "AT_SERVICE", action) === true
}
// A transition's refusal, shown as its error; and its word of what was done.
function refuse(words) {
    ec.message.addError(words)
    return null
}
const done = words => ec.message.addMessage(words, "success")

// ===== recipients =====
// Who may be proposed and named here, as find#ReportRecipients answers the viewer under the mount's permission (the
// engine's own rule with no mount): whether they may deliver to others at all, a page of people, their own groups, and
// of the users and groups named which are accepted, with their names.
function recipients(policy, named) {
    const users = idList(named?.userIds)
    const groups = idList(named?.groupIds)
    const none = {
        canDeliver: false,
        users: [],
        groups: [],
        selectedUsers: [],
        refusedUserIds: users.filter(userId => userId !== ec.user.userId),
        refusedGroupIds: groups,
        failure: null
    }
    if (!offered("find#ReportRecipients")) return none
    const found = ask("find#ReportRecipients", {
        permissionId: policy.recipientPermissionId,
        term: named?.term || null,
        userIds: users,
        groupIds: groups
    })
    if (found.failure) {
        ec.logger.error("Report delivery: the recipients were not read: " + found.failure)
        return Object.assign(none, {failure: found.failure})
    }
    return {
        canDeliver: found.out.canDeliver === true,
        users: Array.from(found.out.users || []),
        groups: Array.from(found.out.groups || []),
        selectedUsers: Array.from(found.out.selectedUsers || []),
        refusedUserIds: Array.from(found.out.refusedUserIds || []),
        refusedGroupIds: Array.from(found.out.refusedGroupIds || []),
        failure: null
    }
}
const namesOf = found => ({
    users: Object.fromEntries(found.selectedUsers.map(user => [user.userId, user.name])),
    groups: Object.fromEntries(found.groups.map(group => [group.userGroupId, group.description]))
})
const groupOptions = found => found.groups.map(group => ({value: group.userGroupId, label: group.description}))
// Why the people and groups a form names may not be its recipients here, or null when they may: the viewer may deliver
// to others, each group is one of theirs and each person is among those find#ReportRecipients accepts. It says nothing
// of an account it refuses. A delivery to oneself alone asks nothing.
function unaccepted(policy, users, groups) {
    if (!groups.length && users.every(userId => userId === ec.user.userId)) return null
    const found = recipients(policy, {userIds: users, groupIds: groups})
    if (found.failure) return "The recipients could not be checked just now"
    if (!found.canDeliver) return "Delivering a report to other people needs the group delivery grant"
    if (found.refusedGroupIds.length) return "Deliver only to a group you belong to"
    if (found.refusedUserIds.length) return "Choose recipients among the people who may receive reports here"
    return null
}
// recipientChoices: the people a drop-down offers as the viewer types, with the selected ones outside the page.
function recipientChoices(p) {
    const found = recipients(mountLib.state().policy, {term: p.term, userIds: idList(p.selected)})
    const options = found.users.map(user => ({value: user.userId, label: user.name}))
    for (const user of found.selectedUsers)
        if (!options.some(option => option.value === user.userId)) options.push({value: user.userId, label: user.name})
    return options
}

// ===== what may be scheduled =====
// The dashboards, the reports of dashboards and the CalcGrid documents the viewer reads (a playlist and any other kind
// of report definition renders no report), without those of the components the mount hides. A CalcGrid document is
// sent as its workbook (moqui-calc render#CalcDocument), so in Excel only.
const workbookFormats = formats.filter(format => format.value === "xlsx")
function schedulable(mount) {
    const hidden = mount.mounted ? Array.from(ec.context.get("reportHiddenComponents") || []) : []
    return Array.from(ask("list#Definitions", {}).out?.definitions || []).filter(
        definition =>
            !hidden.includes(definition.component) &&
            (definition.type === "dashboard" ||
                definition.type === "calcgrid" ||
                (definition.type === "report" &&
                    definition.body?.kind !== "playlist" &&
                    Array.from(definition.body?.dashboards || []).length > 0))
    )
}
const definitionOptions = definitions =>
    definitions
        .map(definition => ({
            value: definition.id,
            label:
                (definition.title || definition.id) +
                (definition.type === "report"
                    ? " (" + (text(definition.body?.kind) || "report") + ")"
                    : definition.type === "calcgrid"
                      ? " (CalcGrid)"
                      : "")
        }))
        .sort((a, b) => a.label.localeCompare(b.label))

// ===== schedules as the screens show them =====
function audienceText(row, names) {
    const users = Array.from(row.recipientUserIds || [])
    const groups = Array.from(row.recipientGroupIds || [])
    const viewer = ec.user.userId
    if (!groups.length && users.every(userId => userId === viewer)) return "You"
    const parts = []
    if (users.length === 1) parts.push(users[0] === viewer ? "You" : names.users[users[0]] || "1 person")
    else if (users.length) parts.push(users.length + " people")
    if (groups.length) parts.push(groups.map(groupId => names.groups[groupId] || groupId).join(", "))
    return parts.join(" and ")
}
const modeTexts = {group: "Group", personal: "Personal"}
function scheduleRow(row, last, names) {
    const lastState = last ? occurrenceStates[last.statusId] || [last.statusId, "grey-7"] : null
    return {
        screenScheduledId: row.screenScheduledId,
        kind: row.deliveryMode ? "business" : "owner",
        screenSchedule: false,
        title: row.emailSubject || row.definitionTitle || row.definitionId,
        definitionId: row.definitionId,
        definitionTitle: row.definitionTitle || unread,
        modeText: modeTexts[row.deliveryMode] || "Owner mode",
        audienceText: audienceText(row, names),
        cronText: cronText(row.cronExpression),
        formatText: formatText(formats, row.renderMode),
        paused: !!row.paused,
        stateText: row.paused ? pauseReasons[row.pausedReason] || "Paused" : "Active",
        stateColor: row.paused ? (row.pausedReason ? "warning" : "grey-7") : "positive",
        lastText: last
            ? dateText(last.occurrenceDate) + " · " + lastState[0]
            : row.lastRunTime
              ? dateText(row.lastRunTime)
              : "Not yet run",
        lastOccurrenceId: last?.occurrenceId || null,
        revision: row.revision
    }
}
// A screen schedule the viewer set up for themselves (find#ScreenSchedules): theirs alone, its timing changed here.
function screenRow(row) {
    const path = text(row.screenPath)
    const name = row.description || path.split("/").filter(Boolean).pop() || path
    const ended = row.thruDate != null && Number(row.thruDate) <= Date.now()
    const paused = row.paused === true || ended || !row.cronExpression
    return {
        screenScheduledId: row.screenScheduledId,
        kind: "screen",
        screenSchedule: true,
        title: row.emailSubject || name,
        definitionId: null,
        definitionTitle: name + " (saved list)",
        modeText: "Personal",
        audienceText: "You",
        cronText: cronText(row.cronExpression),
        formatText: formatText(screenFormats, row.renderMode),
        paused,
        stateText: paused ? "Paused" : "Active",
        stateColor: paused ? "grey-7" : "positive",
        lastText: "",
        lastOccurrenceId: null,
        // a group on a screen schedule is sent to only through a registered adapter (the framework's rule)
        groupNote:
            row.userGroupId && !row.groupAdmitted ? "Its group is not sent to: this list is not admitted for group delivery" : ""
    }
}
function deliveryRow(delivery) {
    const state = deliveryStates[delivery.statusId] || [delivery.statusId, "grey-7"]
    return {
        deliveryId: delivery.deliveryId,
        userId: delivery.userId,
        recipientName: delivery.userFullName || delivery.username || delivery.userId,
        statusId: delivery.statusId,
        stateText: state[0],
        stateColor: state[1],
        reasonCode: delivery.reasonCode || "",
        reasonText: delivery.reason || "",
        attemptText: dateText(delivery.attemptDate),
        attemptCount: delivery.attemptCount || 0,
        retryable: delivery.retryable === true,
        uncertain: delivery.uncertain === true,
        sent: delivery.statusId === "RptDlvSent"
    }
}
function occurrenceRow(occurrence) {
    const state = occurrenceStates[occurrence.statusId] || [occurrence.statusId, "grey-7"]
    return {
        occurrenceId: occurrence.occurrenceId,
        screenScheduledId: occurrence.screenScheduledId,
        dateText: dateText(occurrence.occurrenceDate),
        kindText: occurrence.occurrenceKind === "manual" ? "Sent now" : "Scheduled",
        stateText: state[0],
        stateColor: state[1],
        reasonText: occurrenceReasons[occurrence.reasonCode] || "",
        countsText:
            occurrence.sentCount +
            " sent" +
            (occurrence.skippedCount ? ", " + occurrence.skippedCount + " skipped" : "") +
            (occurrence.failedCount ? ", " + occurrence.failedCount + " failed" : "") +
            (occurrence.uncertainCount ? ", " + occurrence.uncertainCount + " not known" : "")
    }
}
function pageFields(out, field, total, pageIndex, pageSize) {
    out[field + "Count"] = total
    out[field + "PageIndex"] = pageIndex
    out[field + "PageSize"] = pageSize
    out[field + "PageMaxIndex"] = Math.max(0, Math.ceil(total / pageSize) - 1)
    out[field + "PageRangeLow"] = total ? pageIndex * pageSize + 1 : 0
    out[field + "PageRangeHigh"] = Math.min(total, (pageIndex + 1) * pageSize)
    return out
}

// ===== Schedules: the list =====
// Mine (the schedules that go to the viewer alone, the owner-mode ones and the screen schedules they set up for
// themselves among them), Shared administration (the group schedules they own, for one who may deliver to others) and
// Received (the reports delivered to them, whatever they may schedule), with what a new schedule may be made of.
function list(p) {
    const mount = mountLib.state()
    const canRead = offered("find#ReportOccurrences")
    const canSchedule = offered("list#Schedules") && offered("store#Schedule")
    const asked = ["mine", "shared", "received"].includes(p.view) ? p.view : ""
    const view = canSchedule && asked ? asked : canSchedule ? "mine" : "received"
    const out = {
        mounted: mount.mounted,
        view,
        schedules: [],
        received: [],
        canRead,
        canSchedule,
        canDistribute: false,
        definitionOptions: [],
        formatOptions: formats,
        cronOptions: cronPresets,
        groupOptions: [],
        failure: null
    }
    pageFields(out, "received", 0, 0, receivedPageSize)
    if (view === "received") {
        if (!canRead) return out
        const found = ask("find#ReportOccurrences", {
            received: true,
            pageIndex: Number(p.pageIndex) || 0,
            pageSize: receivedPageSize
        })
        if (found.failure) return Object.assign(out, {failure: found.failure})
        out.received = Array.from(found.out.occurrences || []).map(occurrence =>
            Object.assign(deliveryRow(occurrence.ownDelivery || {}), {
                occurrenceId: occurrence.occurrenceId,
                title: occurrence.title || unread,
                dateText: dateText(occurrence.occurrenceDate)
            })
        )
        // New schedule is offered beside the received reports too
        if (canSchedule) {
            const proposed = recipients(mount.policy, {})
            out.canDistribute = proposed.canDeliver
            out.groupOptions = groupOptions(proposed)
            out.definitionOptions = definitionOptions(schedulable(mount))
        }
        return pageFields(
            out,
            "received",
            Number(found.out.total) || 0,
            Number(found.out.pageIndex) || 0,
            Number(found.out.pageSize) || receivedPageSize
        )
    }
    const listed = ask("list#Schedules", {})
    if (listed.failure) return Object.assign(out, {failure: listed.failure})
    const owned = Array.from(listed.out.schedules || []).filter(row => (view === "shared") === (row.deliveryMode === "group"))
    const found = recipients(mount.policy, {
        userIds: Array.from(new Set(owned.flatMap(row => Array.from(row.recipientUserIds || [])))),
        groupIds: Array.from(new Set(owned.flatMap(row => Array.from(row.recipientGroupIds || []))))
    })
    out.canDistribute = found.canDeliver
    out.groupOptions = groupOptions(found)
    out.definitionOptions = definitionOptions(schedulable(mount))
    const last = {}
    if (canRead)
        for (const occurrence of Array.from(ask("find#ReportOccurrences", {pageSize: 100}).out?.occurrences || []))
            if (!last[occurrence.screenScheduledId]) last[occurrence.screenScheduledId] = occurrence
    const names = namesOf(found)
    out.schedules = owned.map(row => scheduleRow(row, last[row.screenScheduledId], names))
    if (view === "mine" && offered("find#ScreenSchedules"))
        out.schedules = out.schedules.concat(Array.from(ask("find#ScreenSchedules", {}).out?.schedules || []).map(screenRow))
    if (p.definitionId) out.schedules = out.schedules.filter(row => row.definitionId === p.definitionId)
    if (p.state === "active" || p.state === "paused")
        out.schedules = out.schedules.filter(row => row.paused === (p.state === "paused"))
    return out
}

// ===== ScheduledReportDetail: one schedule =====
// The panels of a dashboard, or of each dashboard of a report, to show as the viewer's own preview (each a report-embed,
// rendered as the viewer); a section the viewer may not read is left out. A CalcGrid document is one section with no
// panel, its workbook: the preview is its file (previewFile).
function sections(definition) {
    if (definition.type === "calcgrid")
        return [{definitionId: definition.id, title: definition.title, panels: [], workbook: true}]
    const specs = definition.type === "report" ? Array.from(definition.body?.dashboards || []) : [{dashboardRef: definition.id}]
    return specs
        .map(spec => {
            const id = spec.dashboardRef || spec.definitionId
            const range = {from: spec.from || "now-7d", thru: spec.thru || "now"}
            const dashboard = ask("get#Definition", {definitionId: id}).out?.definition
            if (!dashboard || dashboard.type !== "dashboard") return null
            return Object.assign(range, {
                definitionId: id,
                title: dashboard.title,
                panels: Object.keys(dashboard.body?.panels || {})
                    .slice(0, 6)
                    .map(panelId => Object.assign({panelId, definitionId: id}, range))
            })
        })
        .filter(Boolean)
}
function audienceRows(checked) {
    return Array.from(checked.recipients || []).map(recipient => ({
        userId: recipient.userId,
        recipientName: recipient.userFullName || recipient.username || recipient.userId,
        viaText: Array.from(recipient.viaGroupIds || []).length ? "In a group" : "Named",
        eligible: recipient.eligible === true,
        blocking: recipient.blocking === true,
        resultText: recipient.eligible ? "May receive it" : recipient.reason,
        resultColor: recipient.eligible ? "positive" : recipient.blocking ? "negative" : "warning",
        reasonCode: recipient.reasonCode || ""
    }))
}
// One schedule of the viewer's: a report schedule with its recipients and their check, its runs and the viewer's own
// preview, or a screen schedule of theirs with its timing. Another's, and one that is not there, are not found.
function detail(screenScheduledId) {
    const mount = mountLib.state()
    const policy = mount.policy
    const none = {
        found: false,
        schedule: {},
        options: {formats, cronPresets, groups: []},
        recipients: [],
        audience: [],
        audienceNote: "",
        runs: [],
        sections: [],
        canDistribute: false
    }
    if (!screenScheduledId) return none
    const row = offered("list#Schedules") ? Array.from(ask("list#Schedules", {screenScheduledId}).out?.schedules || [])[0] : null
    if (!row) {
        const own = offered("find#ScreenSchedules")
            ? Array.from(ask("find#ScreenSchedules", {screenScheduledId}).out?.schedules || [])[0]
            : null
        if (!own) return none
        return Object.assign(none, {
            found: true,
            schedule: Object.assign(screenRow(own), {
                cronExpression: own.cronExpression,
                renderMode: own.renderMode || "csv",
                noResultsAbort: own.noResultsAbort ? "Y" : "N",
                fromDate: own.fromDate == null ? null : new Date(Number(own.fromDate)),
                thruDate: own.thruDate == null ? null : new Date(Number(own.thruDate)),
                emailSubject: own.emailSubject
            }),
            options: {formats: screenFormats, cronPresets, groups: []}
        })
    }
    const found = recipients(policy, {
        userIds: Array.from(row.recipientUserIds || []),
        groupIds: Array.from(row.recipientGroupIds || [])
    })
    const names = namesOf(found)
    const group = row.deliveryMode === "group"
    const schedule = Object.assign(scheduleRow(row, null, names), {
        cronExpression: row.cronExpression,
        renderMode: row.renderMode,
        noResultsAbort: row.noResultsAbort ? "Y" : "N",
        fromDate: row.fromDate == null ? null : new Date(Number(row.fromDate)),
        thruDate: row.thruDate == null ? null : new Date(Number(row.thruDate)),
        emailSubject: row.emailSubject,
        deliveryMode: row.deliveryMode,
        group,
        // an owner-mode schedule (create#Schedule): its timing and output change here, and the engine keeps its
        // recipients, template and location (storeTiming)
        ownerMode: !row.deliveryMode,
        recipientUserIds: group ? Array.from(row.recipientUserIds || []) : [],
        recipientGroupIds: group ? Array.from(row.recipientGroupIds || []) : [],
        expectedRevision: row.revision
    })
    const recipientRows = schedule.recipientUserIds
        .map(userId => ({kind: "Person", name: names.users[userId] || "A person who may no longer receive it here"}))
        .concat(schedule.recipientGroupIds.map(groupId => ({kind: "Group", name: names.groups[groupId] || groupId})))
    // each recipient's result, for one who may deliver to others: eligibility, never the other person's grants
    let audience = []
    let audienceNote = ""
    if (group && found.canDeliver && offered("check#ReportAudience")) {
        const checked = ask("check#ReportAudience", {
            definitionId: row.definitionId,
            recipientUserIds: schedule.recipientUserIds,
            recipientGroupIds: schedule.recipientGroupIds,
            recipientCap: row.recipientCap || policy.recipientCap || null
        })
        if (checked.failure) {
            ec.logger.error("Report delivery: audience of " + row.screenScheduledId + ": " + checked.failure)
            audienceNote = "The recipients could not be checked just now."
        } else {
            audience = audienceRows(checked.out)
            audienceNote =
                checked.out.eligibleCount +
                " of " +
                checked.out.recipientCount +
                " may receive it; at most " +
                checked.out.recipientCap +
                " recipients."
        }
    }
    const definition = ask("get#Definition", {definitionId: row.definitionId}).out?.definition
    return Object.assign(none, {
        found: true,
        schedule,
        options: {formats: definition?.type === "calcgrid" ? workbookFormats : formats, cronPresets, groups: groupOptions(found)},
        recipients: recipientRows,
        audience,
        audienceNote,
        runs: offered("find#ReportOccurrences")
            ? Array.from(
                  ask("find#ReportOccurrences", {screenScheduledId: row.screenScheduledId, pageSize: 20}).out?.occurrences || []
              ).map(occurrenceRow)
            : [],
        // the owner no longer reads the definition: the page says so, and there is nothing to preview
        sections: definition ? sections(definition) : [],
        canDistribute: found.canDeliver && !!row.deliveryMode
    })
}

// ===== the transitions of the schedule screens =====
// What every save of a report schedule here carries beside the form's values: the mount's template and recipient cap
// (Tools' own with no mount: the engine's), read from the policy and never from the request.
function save(policy, request) {
    if (policy.reportEmailTemplateId) request.emailTemplateId = policy.reportEmailTemplateId
    if (policy.recipientCap) request.recipientCap = policy.recipientCap
    return ask("store#Schedule", request)
}
const ownSchedule = screenScheduledId =>
    screenScheduledId ? Array.from(ask("list#Schedules", {screenScheduledId}).out?.schedules || [])[0] || null : null
const revisionOf = value => (value == null || value === "" ? null : Number(value))
// createSchedule: a new schedule of a report the list offers, for the viewer alone or, for one who may deliver to others,
// for the people and groups the form names, each accepted first. Answers the new schedule's id, or null with the refusal.
function create(p) {
    const mount = mountLib.state()
    const definitionId = text(p.newDefinitionId)
    if (!schedulable(mount).some(definition => definition.id === definitionId))
        return refuse("Choose a dashboard or report you may read")
    const others = p.audienceKind === "group"
    const users = others ? idList(p.recipientUserIds) : []
    const groups = others ? idList(p.recipientGroupIds) : []
    const refusal = unaccepted(mount.policy, users, groups)
    if (refusal) return refuse(refusal)
    const workbook = schedulable(mount).some(definition => definition.id === definitionId && definition.type === "calcgrid")
    const stored = save(mount.policy, {
        definitionId,
        cronExpression: text(p.cronSelected) || null,
        renderMode: workbook ? "xlsx" : p.renderMode || null,
        noResultsAbort: "Y",
        emailSubject: text(p.emailSubject) || null,
        recipientUserIds: users,
        recipientGroupIds: groups
    })
    if (stored.failure) return refuse(stored.failure)
    done("Schedule saved")
    return stored.out.screenScheduledId
}
// storeSchedule: the timing, format, active interval, subject and skipping of a schedule of the viewer's, a report
// schedule through store#Schedule (its recipients kept) and a screen schedule of theirs through store#ScreenSchedule.
// An owner-mode schedule keeps its own template, recipients and location: the request carries no recipient list, so the
// engine changes its timing and output alone, and the mount's template and cap are not put on it. The form posts every
// field, so one left empty is cleared.
function storeTiming(p) {
    const request = {
        screenScheduledId: p.screenScheduledId,
        cronExpression: text(p.cronSelected) || text(p.cronExpression) || null,
        renderMode: p.renderMode || null,
        noResultsAbort: p.noResultsAbort || null,
        fromDate: p.fromDate || null,
        thruDate: p.thruDate || null,
        emailSubject: text(p.emailSubject) || null,
        clearFields: ["emailSubject", "fromDate", "thruDate"].filter(name => p[name] == null || p[name] === "")
    }
    const row = ownSchedule(p.screenScheduledId)
    if (row) request.expectedRevision = revisionOf(p.expectedRevision)
    const stored = !row
        ? ask("store#ScreenSchedule", request)
        : row.deliveryMode
          ? save(mountLib.state().policy, request)
          : ask("store#Schedule", request)
    if (stored.failure) return refuse(stored.failure)
    return done("Schedule saved")
}
// storeAudience: the people and groups of a report schedule of the viewer's, each accepted first; none is the viewer alone.
function storeAudience(p) {
    const policy = mountLib.state().policy
    const users = idList(p.recipientUserIds)
    const groups = idList(p.recipientGroupIds)
    const refusal = unaccepted(policy, users, groups)
    if (refusal) return refuse(refusal)
    const stored = save(policy, {
        screenScheduledId: p.screenScheduledId,
        recipientUserIds: users,
        recipientGroupIds: groups,
        expectedRevision: revisionOf(p.expectedRevision)
    })
    if (stored.failure) return refuse(stored.failure)
    return done(
        stored.out.deliveryMode === "group"
            ? "Recipients saved: " + stored.out.recipientCount + " checked"
            : "The schedule now goes to you alone"
    )
}
// pauseSchedules: one schedule or several paused or resumed, each with its own result; a resume the engine refuses (a
// recipient whose scope differs, a schedule waiting for its new owner's check) says why and the others go on.
function pause(p) {
    const paused = !(p.paused === false || p.paused === "false" || p.paused === "N")
    const report = offered("list#Schedules")
        ? Array.from(ask("list#Schedules", {}).out?.schedules || []).map(row => row.screenScheduledId)
        : []
    let applied = 0
    for (const screenScheduledId of idList(p.screenScheduledIds)) {
        const asked = report.includes(screenScheduledId)
            ? ask("pause#Schedule", {screenScheduledId, paused})
            : ask("store#ScreenSchedule", {screenScheduledId, paused})
        if (asked.failure) ec.message.addMessage(asked.failure, "danger")
        else applied++
    }
    if (applied) done(applied + (applied === 1 ? " schedule " : " schedules ") + (paused ? "paused" : "resumed"))
}
// sendNow: a new occurrence of its own; each recipient is checked as at any run.
function send(p) {
    const sent = ask("send#Report", {screenScheduledId: p.screenScheduledId})
    if (sent.failure) return refuse(sent.failure)
    if (sent.out.skipped === true)
        return ec.message.addMessage("Nothing was sent: " + (text(sent.out.reason) || "not due"), "warning")
    return done("Sent: each recipient has their own message")
}
const encode = value => WebUtilities.encodeHtml(String(value ?? ""))
const page = (title, body) =>
    '<!doctype html><html><head><meta charset="utf-8"><title>' +
    encode(title) +
    "</title></head><body><h1>" +
    encode(title) +
    "</h1>" +
    body +
    "</body></html>"
const fileLink = (file, words) =>
    '<p><a download="' +
    encode(file.filename || "report") +
    '" href="data:' +
    encode(file.contentType) +
    ";base64," +
    encode(file.contentBase64) +
    '">' +
    encode(words) +
    "</a></p>"
// previewFile: the viewer's own render of a dashboard or report they read, as a small page with its download link.
function previewPage(p) {
    const mode = formats.some(format => format.value === p.renderMode) ? p.renderMode : "pdf"
    const preview = ask("preview#Report", {definitionId: p.definitionId, renderMode: mode})
    const body = preview.failure
        ? "<p>" + encode(preview.failure) + "</p>"
        : preview.out.empty === true
          ? "<p>The report has no data for its period.</p>"
          : fileLink(preview.out, "Download the preview") + "<p>This is your own result, as you may read it now.</p>"
    return page("Report preview", body)
}
// ownReportFile: the viewer's own delivered report, rendered again as themselves by the engine, or why it is not released.
function ownReportPage(p) {
    const report = ask("get#RecipientReport", {deliveryId: p.deliveryId})
    const body = report.failure
        ? "<p>This delivery is not available.</p>"
        : !report.out.available
          ? "<p>Your access to this report's data changed since it was sent, so the sent report is not released. Open the report to see what you may read now.</p>"
          : fileLink(report.out, "Download " + (report.out.title || "the report")) +
            "<p>Rendered again for you on " +
            encode(dateText(report.out.generatedDate)) +
            ", with what you may read now. What was sent is not kept.</p>"
    return page("Your report", body)
}

// ===== ReportDelivery: one run =====
// An occurrence as get#ReportOccurrence answers the viewer: for its schedule's owner what ran and each recipient's
// outcome (administers), for a holder of the administrative read the same without the title and with its owner by name
// (oversees), for a recipient their own delivery alone. Missing and someone else's read alike.
function delivery(occurrenceId) {
    const none = {
        found: false,
        administers: false,
        oversees: false,
        whole: false,
        canRetry: false,
        occurrence: {},
        deliveries: [],
        ownDelivery: null
    }
    if (!occurrenceId || !offered("get#ReportOccurrence")) return none
    const got = ask("get#ReportOccurrence", {occurrenceId})
    if (got.failure || !got.out?.occurrence) return none
    const occurrence = got.out.occurrence
    const administers = got.out.administers === true
    // the engine answers the owner's id only with the whole run: to its owner and to the administrator
    const oversees = !administers && !!occurrence.ownerUserId
    const whole = administers || oversees
    const state = occurrenceStates[occurrence.statusId] || ["", "grey-7"]
    return {
        found: true,
        administers,
        oversees,
        whole,
        canRetry: administers && offered("retry#ReportRecipients"),
        occurrence: Object.assign(whole ? occurrenceRow(occurrence) : {}, {
            occurrenceId: occurrence.occurrenceId,
            screenScheduledId: administers ? occurrence.screenScheduledId : null,
            title: occurrence.title || (oversees ? "A report of " + (occurrence.ownerName || "another owner") : unread),
            dateText: dateText(occurrence.occurrenceDate),
            formatText: formatText(formats, occurrence.renderMode),
            stateText: state[0],
            stateColor: state[1],
            ownerName: oversees ? occurrence.ownerName || "" : "",
            versionText: whole
                ? "Schedule revision " +
                  (occurrence.scheduleRevision ?? "–") +
                  ", report version " +
                  (occurrence.definitionVersion ?? "–")
                : "",
            modeText: modeTexts[occurrence.deliveryMode] || (whole ? "Owner mode" : ""),
            rendersText:
                whole && occurrence.renderCount
                    ? occurrence.renderCount + (occurrence.renderCount === 1 ? " render" : " renders")
                    : "",
            revision: occurrence.revision
        }),
        deliveries: Array.from(got.out.deliveries || []).map(deliveryRow),
        ownDelivery: got.out.ownDelivery ? deliveryRow(got.out.ownDelivery) : null
    }
}
// retryRecipients: the failed deliveries chosen, each with its own outcome; a sent one is never replayed and an
// uncertain one waits to be reconciled (the engine's rules).
function retry(p) {
    const retried = ask("retry#ReportRecipients", {
        occurrenceId: p.occurrenceId,
        deliveryIds: idList(p.deliveryIds),
        expectedRevision: revisionOf(p.expectedRevision),
        reason: text(p.reason)
    })
    if (retried.failure) return refuse(retried.failure)
    for (const result of Array.from(retried.out.results || []))
        if (result.retried !== true && result.reason) ec.message.addMessage(result.reason, "warning")
    const sentCount = retried.out.sentCount || 0
    return ec.message.addMessage(
        sentCount + (sentCount === 1 ? " recipient" : " recipients") + " sent on retry",
        sentCount ? "success" : "warning"
    )
}
function reconcile(p) {
    const settled = ask("reconcile#ReportRecipient", {deliveryId: p.deliveryId, outcome: p.outcome, reason: text(p.reason)})
    if (settled.failure) return refuse(settled.failure)
    return done("Delivery settled")
}
// exportAudit: one run's delivery status as CSV for whom the page shows it: every recipient's result with its safe
// reason for the run's owner and the administrator, a recipient's own row alone. No report content, address or frame;
// the export is recorded (audit#Export). Null with the refusal when the run is not the viewer's to see.
function auditCsv(p) {
    const found = delivery(p.occurrenceId)
    if (!found.found) return refuse("This delivery is not available")
    const occurrence = found.occurrence
    // a recipient's own row: named as they are, since their own delivery carries their id alone
    const account = ec.user.userAccount
    const deliveries = found.whole
        ? found.deliveries
        : found.ownDelivery
          ? [Object.assign({}, found.ownDelivery, {recipientName: account?.userFullName || ec.user.username})]
          : []
    // a cell: quoted when it holds a separator, and a leading formula character made text (a spreadsheet would run it)
    const cell = value => {
        let cellText = String(value ?? "")
        if (/^[=+\-@\t\r]/.test(cellText)) cellText = "'" + cellText
        return /[",\n\r]/.test(cellText) ? '"' + cellText.replace(/"/g, '""') + '"' : cellText
    }
    const header = ["Report", "Run", "Kind", "Delivery", "Run state", "Recipient", "Result", "Why", "Attempted", "Attempts"]
    const lines = [header.map(cell).join(",")]
    for (const row of deliveries)
        lines.push(
            [
                occurrence.title,
                occurrence.dateText,
                occurrence.kindText || "",
                occurrence.modeText,
                found.whole ? occurrence.stateText : "",
                row.recipientName,
                row.stateText,
                row.reasonText,
                row.attemptText,
                row.attemptCount
            ]
                .map(cell)
                .join(",")
        )
    const recorded = ask("audit#Export", {kind: "delivery-audit", format: "csv", rowCount: deliveries.length})
    if (recorded.failure) return refuse(recorded.failure)
    return {
        content: lines.join("\r\n") + "\r\n",
        filename: "delivery-" + String(occurrence.occurrenceId || "run").replace(/[^A-Za-z0-9_-]/g, "") + ".csv",
        rowCount: deliveries.length
    }
}

// What the alert screens share with these (template/ReportAlerts.js): the engine call and its refusal, what a viewer's own
// grant offers, a transition's refusal and its word, who may be proposed and named as a recipient under the mount's
// policy (one place for a schedule's recipients and an alert rule's audience), and the words of a delivery.
;({
    ask,
    offered,
    refuse,
    done,
    recipients,
    namesOf,
    groupOptions,
    dateText,
    deliveryStates,
    list,
    detail,
    delivery,
    recipientChoices,
    create,
    storeTiming,
    storeAudience,
    pause,
    send,
    previewPage,
    ownReportPage,
    retry,
    reconcile,
    auditCsv
})
