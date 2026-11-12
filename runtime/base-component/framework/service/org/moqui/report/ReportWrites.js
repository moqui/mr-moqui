// The writes of org.moqui.report.ReportServices that the report screens need beside the engine's own (docs/tools/Monitoring.md,
// "Report delivery"; plans/archive/Admin-Screens-Down.md, Track I), so that a mountable report screen writes no entity itself. Each
// operation checks what its caller may change and writes with authorization off only that. No dependency on Mantle or an app.
//
//     store#ScreenSchedule  the timing, format, active interval, subject and pause of a screen schedule the caller set up
//                           for themselves (find#ScreenSchedules, ReportReads.js); a report schedule is store#Schedule's
const SCHEDULE = "moqui.screen.ScreenScheduled"
// what a screen schedule renders as (org.moqui.impl.ScreenServices render#ScheduledScreen)
const FORMATS = ["csv", "xlsx", "xsl-fo", "html-print"]
const CLEARABLE = ["emailSubject", "fromDate", "thruDate"]
const SUBJECT_MAX = 255

const text = value => (value == null ? "" : String(value))
const sent = value => value != null && value !== ""
const idList = value =>
    (value == null || value === "" ? [] : Array.from(typeof value === "string" ? value.split(",") : value))
        .map(id => text(id).trim())
        .filter(Boolean)
const flag = value => value === true || value === "true" || value === "Y"

// store#ScreenSchedule: the caller's own screen schedule (userId the caller, no report definition), found again here
// whatever a screen showed; another user's, a report schedule and one that is not there are refused alike. Only its
// timing, format, skipping when empty, active interval, subject and pause change: never its screen, saved find, user,
// group, template or location. A value not sent is kept; clearFields names which of emailSubject, fromDate and thruDate
// to empty (a form posts an emptied field as no value). The subject is expanded when the schedule runs
// (render#ScheduledScreen, ec.resource.expand), so one set here is plain text: an expression is refused, as the expander
// would read it.
function storeScreenSchedule(p) {
    if (!ec.user.userId) throw new Error("A screen schedule is changed by a signed-in user")
    const screenScheduledId = text(p.screenScheduledId)
    const row = screenScheduledId
        ? ec.entity
              .find(SCHEDULE)
              .condition("screenScheduledId", screenScheduledId)
              .condition("userId", ec.user.userId)
              .condition("reportDefinitionId", "is-null", null)
              .disableAuthz()
              .one()
        : null
    if (!row) throw new Error("SCHEDULE_UNAVAILABLE: screen schedule " + screenScheduledId + " is not available")
    const clear = idList(p.clearFields).filter(name => CLEARABLE.includes(name))
    if (sent(p.cronExpression)) {
        const cron = text(p.cronExpression).trim()
        try {
            ec.service.nextCronExecution(cron)
        } catch (error) {
            throw new Error("VALUE_INVALID: the schedule is not a time pattern")
        }
        row.cronExpression = cron
    }
    if (sent(p.renderMode)) {
        if (!FORMATS.includes(text(p.renderMode))) throw new Error("VALUE_INVALID: choose a format of " + FORMATS.join(", "))
        row.renderMode = text(p.renderMode)
    }
    if (p.noResultsAbort === "Y" || p.noResultsAbort === "N") row.noResultsAbort = p.noResultsAbort
    if (clear.includes("emailSubject")) row.emailSubject = null
    else if (sent(p.emailSubject)) {
        const subject = text(p.emailSubject).trim()
        if (subject !== text(row.emailSubject) && (subject.includes("${") || subject.length > SUBJECT_MAX))
            throw new Error("VALUE_INVALID: a subject is plain text of at most " + SUBJECT_MAX + " characters, without ${ }")
        row.emailSubject = subject
    }
    for (const name of ["fromDate", "thruDate"]) {
        if (clear.includes(name)) row[name] = null
        else if (sent(p[name])) row[name] = p[name]
    }
    if (sent(p.paused)) row.paused = flag(p.paused) ? "Y" : "N"
    // the caller's own row, found above by its user: written with authorization off, as the user
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        row.update()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
    return {screenScheduledId, paused: row.paused === "Y"}
}

const operations = {storeScreenSchedule}
;({
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown report write " + op)
        return operations[op](parameters)
    }
})
