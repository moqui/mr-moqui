// CalcGrid in the Dashboards app (screen/Dashboards/CalcGrid.xml; docs/tools/Database.md, CalcGrid and access profiles;
// plans/archive/IronCalc-DB.md Phase 2b): what the screen's transitions answer, over org.moqui.calc.CalcServices and,
// for a document's list, group and deletion, the report engine's definition services, each called as the viewer.
// Presentation only: a service decides every access. No entity is read here.
//
// The access profile a call is made under is the mount's (template/ReportMount.js, policy.calcProfile) when the mount
// names one, else the request's profileId, else none, which is the Tools app's own access. Naming a profile gives no
// right: the engine checks that the viewer holds it and reads only what it offers, with its entity filters.
const mountLib = ec.resource.script("component://tools/template/ReportMount.js")
// what the viewer's own grant of a report engine operation offers, as the delivery screens read it
const {offered} = ec.resource.script("component://tools/template/ReportDelivery.js")
const CALC = "org.moqui.calc.CalcServices"
const REPORT = "org.moqui.report.ReportServices"
const TYPE = "calcgrid"

const text = value => (value === null || value === undefined ? "" : String(value).trim())
function profileId() {
    const fixed = mountLib.state().policy.calcProfile
    return fixed || text(ec.context.get("profileId")) || null
}
// A service call as the viewer: its out-parameters with errors, the call's error text or null.
function ask(name, parameters) {
    let out = {}
    let errors = null
    try {
        out = ec.service.sync().name(name).parameters(parameters).call() || {}
    } catch (error) {
        errors = String(error.message || error)
    }
    if (ec.message.hasError()) errors = ec.message.errorsString
    ec.message.clearErrors()
    return Object.assign({}, out, {errors})
}
function parse(name, fallback) {
    const value = ec.context.get(name)
    if (value === null || value === undefined || value === "") return fallback
    try {
        return JSON.parse(String(value))
    } catch (error) {
        return fallback
    }
}

// The schedules screen a saved document is scheduled from (Tools' Schedules, or the mount's: the report link schedules,
// to which ?definitionId= preselects the document), when the viewer may schedule for themselves there (list#Schedules
// and store#Schedule, as the delivery screens offer it), else null. A link and not a dialog here, so that what the
// mount's schedules area adds to its policy (Insights' recipient cap) holds (plans/IronCalc-DB2.md CG1.2).
function schedulesPath(mount) {
    if (!mount.links.schedules || !(offered("list#Schedules") && offered("store#Schedule"))) return null
    // a screen the request's root does not reach is no link
    const path = String(sri.buildUrl(mount.links.schedules).path || "")
    return path && path !== "#" ? path : null
}

// How the viewer may use CalcGrid here: the profiles to choose from (none when the mount names one), the Tools app's own
// access where there is no mount, whether they author, whether and where they schedule a document, and the profile in
// use.
function access() {
    const mount = mountLib.state()
    const found = ask(CALC + ".get#CalcProfiles", {})
    const fixed = mount.policy.calcProfile
    const profiles = Array.from(found.profiles || []).filter(profile => !fixed || profile.profileId === fixed)
    const tools = !mount.mounted && found.tools === true
    // the access the viewer chose before (the user preference the engine reads for a panel that names none), while it
    // is still offered here: a profile's id, or "" for the Tools app's own
    const kept = text(ec.user.getPreference("calc.profile"))
    const current = fixed
        ? fixed
        : kept === "tools" && tools
          ? ""
          : profiles.some(profile => profile.profileId === kept)
            ? kept
            : tools
              ? ""
              : profiles.length
                ? profiles[0].profileId
                : null
    const schedules = schedulesPath(mount)
    return {
        errors: found.errors,
        userId: ec.user.userId,
        fixedProfile: fixed,
        tools,
        current,
        profiles,
        canAuthor: found.canAuthor === true,
        canShare: mount.policy.features.share === true,
        canSchedule: !!schedules,
        schedulesPath: schedules
    }
}
// Keep the access the viewer chose (a profile, or the Tools app's own) as their current one: what a dashboard panel and
// a scheduled report over a CalcGrid document read under.
const choose = () => ask(CALC + ".set#CalcProfile", {profileId: text(ec.context.get("profileId")) || "tools"})
// The entities and views a new document may be over, by a term.
const entities = () => ask(CALC + ".find#CalcEntities", {profileId: profileId(), term: text(ec.context.get("term"))})
// The documents the viewer reads: id, title, owner, group and version, never a body.
function documents() {
    const found = ask(REPORT + ".list#Definitions", {definitionType: TYPE})
    return {
        errors: found.errors,
        documents: Array.from(found.definitions || []).map(definition => ({
            definitionId: definition.id,
            title: definition.title || definition.id,
            ownerId: definition.ownerId,
            userGroupId: definition.userGroupId || null,
            version: definition.version
        }))
    }
}
// Open a saved document (definitionId) or a new one over an entity the viewer chose (entityName).
function open() {
    const entityName = text(ec.context.get("entityName"))
    return ask(CALC + ".open#CalcDocument", {
        profileId: profileId(),
        definitionId: text(ec.context.get("definitionId")) || null,
        source: entityName ? {type: "entity", entityName, conditions: parse("conditionsJson", {})} : null,
        title: text(ec.context.get("title")) || null
    })
}
// A new document from an uploaded workbook (the request's file) over an entity the viewer chose: the workbook its sheets
// and formulas, the entity its data sheet (plans/IronCalc-DB2.md CG4.4). The file is held to 32 MB before it is read; the
// engine checks the rest (an author only under a profile, the inflated size, the data sheet's names, the stored size).
const MAX_UPLOAD_BYTES = 32 * 1024 * 1024
function upload() {
    const file = ec.context.get("file")
    if (!file || typeof file.getSize !== "function") return {errors: ec.l10n.localize("Choose a workbook file")}
    if (Number(file.getSize()) > MAX_UPLOAD_BYTES) return {errors: ec.l10n.localize("A workbook to open has at most 32 MB")}
    const entityName = text(ec.context.get("entityName"))
    return ask(CALC + ".open#CalcDocument", {
        profileId: profileId(),
        workbookBytes: file.getInputStream().readAllBytes(),
        workbookName: String(file.getName() || ""),
        source: entityName ? {type: "entity", entityName, conditions: parse("conditionsJson", {})} : null,
        title: text(ec.context.get("title")) || null
    })
}
const cells = () =>
    ask(CALC + ".get#CalcCells", {
        profileId: profileId(),
        roomId: ec.context.get("roomId"),
        sheet: ec.context.get("sheet"),
        row: ec.context.get("row"),
        column: ec.context.get("column"),
        width: ec.context.get("width"),
        height: ec.context.get("height"),
        since: text(ec.context.get("since")) || null,
        client: text(ec.context.get("client")) || null,
        selection: parse("selectionJson", null)
    })
// Edits to the room's workbook (opsJson: a list of edits, as edit#CalcDocument takes them): the room's new state, with an
// edit's refusal as errors beside it.
function edit() {
    const ops = parse("opsJson", null)
    if (!Array.isArray(ops)) return {errors: "opsJson is not a list of edits"}
    return ask(CALC + ".edit#CalcDocument", {profileId: profileId(), roomId: ec.context.get("roomId"), ops})
}
// What was typed into a cell, for the formula bar and a cell's editor.
function content() {
    const found = ask(CALC + ".get#CalcCellContent", {
        profileId: profileId(),
        roomId: ec.context.get("roomId"),
        sheet: ec.context.get("sheet"),
        row: ec.context.get("row"),
        column: ec.context.get("column")
    })
    return {content: found.content || "", errors: found.errors}
}
const refresh = () => ask(CALC + ".refresh#CalcDocument", {profileId: profileId(), roomId: ec.context.get("roomId")})
// The room's state again, when the notification socket tells of another's edit.
const state = () => ask(CALC + ".get#CalcState", {profileId: profileId(), roomId: ec.context.get("roomId")})
// Discard everyone's edits since the document was saved (an author's): the room again, as open answers it.
const revert = () => ask(CALC + ".revert#CalcDocument", {profileId: profileId(), roomId: ec.context.get("roomId")})
// Save the room's document; a group is named only where the mount's policy lets the viewer share.
function save() {
    const group = text(ec.context.get("userGroupId"))
    if (group && mountLib.state().policy.features.share !== true) return {errors: ec.l10n.localize("Sharing is not offered to you here")}
    return ask(CALC + ".save#CalcDocument", {
        profileId: profileId(),
        roomId: ec.context.get("roomId"),
        title: text(ec.context.get("title")) || null,
        userGroupId: group || null
    })
}
const exported = () => ask(CALC + ".export#CalcDocument", {profileId: profileId(), roomId: ec.context.get("roomId")})
const close = () => ask(CALC + ".close#CalcDocument", {roomId: ec.context.get("roomId")})
// Delete a document: only a CalcGrid one, and the engine holds it to its owner.
function remove() {
    const definitionId = text(ec.context.get("definitionId"))
    const found = ask(REPORT + ".get#Definition", {definitionId})
    if (found.errors) return {errors: found.errors}
    if (!found.definition || found.definition.type !== TYPE) return {errors: "Not a CalcGrid document"}
    return {errors: ask(REPORT + ".delete#Definition", {definitionId}).errors}
}
// The dashboards a range may be added to: the viewer's own, by title.
function dashboards() {
    const found = ask(REPORT + ".list#Definitions", {definitionType: "dashboard"})
    return {
        errors: found.errors,
        dashboards: Array.from(found.definitions || [])
            .filter(definition => definition.ownerId === ec.user.userId && definition.readOnly !== true)
            .map(definition => ({value: definition.id, label: definition.title || definition.id}))
    }
}
// Add a range of a saved document to a dashboard of the viewer's own as a panel over the dataset calcgrid: the report
// engine's table, bar or pie panel of the range, read by each viewer of the dashboard under their own current access, or
// under the access profile chosen (panelProfileId, one the viewer holds), which the panel's query then names. The
// engine's save holds the dashboard to its owner. With a chart (its id) in place of a range, the document's chart as a
// panel drawn as the chart is (CHART_PANELS: a bar chart's bars across), read the same way (plans/IronCalc-DB2.md CG4.2).
const VISUALIZATIONS = ["table", "bar", "pie", "stat"]
const CHART_PANELS = {column: {visualization: "bar"}, bar: {visualization: "bar", horizontal: true}, line: {visualization: "line"},
    area: {visualization: "area"}, pie: {visualization: "pie"}}
function addPanel() {
    const definitionId = text(ec.context.get("definitionId"))
    const dashboardId = text(ec.context.get("dashboardId"))
    const range = text(ec.context.get("range"))
    const chart = text(ec.context.get("chart"))
    const visualization = text(ec.context.get("visualization")) || "table"
    if (!definitionId) return {errors: ec.l10n.localize("Save the document first: a panel reads a saved document")}
    if (!dashboardId || (!range && !chart)) return {errors: ec.l10n.localize("Choose a dashboard and a range or a chart")}
    if (!chart && !VISUALIZATIONS.includes(visualization)) return {errors: ec.l10n.localize("A panel of a range is a table, a bar chart, a pie chart or a number")}
    // a profile the panel reads under is one the viewer holds; each viewer of the dashboard must hold it too
    const panelProfile = text(ec.context.get("panelProfileId"))
    if (panelProfile && !Array.from(ask(CALC + ".get#CalcProfiles", {}).profiles || []).some(profile => profile.profileId === panelProfile))
        return {errors: ec.l10n.localize("Choose an access profile you hold")}
    const document = ask(REPORT + ".get#Definition", {definitionId})
    if (document.errors || !document.definition || document.definition.type !== TYPE) return {errors: document.errors || "Not a CalcGrid document"}
    // the chart as the panel reads it, as the author: it is there, and its type and title
    const read = chart ? ask(CALC + ".get#CalcFrame", {definitionId, chart, profileId: panelProfile || null}) : null
    if (read && (read.errors || !read.frame)) return {errors: read.errors || ec.l10n.localize("The document has no such chart")}
    const found = ask(REPORT + ".get#Definition", {definitionId: dashboardId})
    if (found.errors || !found.definition || found.definition.type !== "dashboard") return {errors: found.errors || "Choose a dashboard"}
    const dashboard = found.definition
    dashboard.body = dashboard.body || {}
    dashboard.body.panels = dashboard.body.panels || {}
    let panelId = "calc-" + definitionId.replace(/[^a-zA-Z0-9_.:-]/g, "-").slice(0, 60)
    for (let n = 2; dashboard.body.panels[panelId]; n++) panelId = panelId.replace(/(-\d+)?$/, "-" + n)
    const source = chart ? {definitionId, chart} : {definitionId, range}
    dashboard.body.panels[panelId] = Object.assign(
        {title: text(ec.context.get("panelTitle")) || (read && text(read.chartTitle)) || document.definition.title || "CalcGrid"},
        read ? CHART_PANELS[text(read.chartType)] || CHART_PANELS.column : {visualization},
        {queries: [{datasetId: "calcgrid", parameters: Object.assign(source, panelProfile ? {profileId: panelProfile} : {})}]}
    )
    const saved = ask(REPORT + ".save#Definition", {definition: dashboard, note: chart ? "Added a CalcGrid chart" : "Added a CalcGrid range"})
    return {errors: saved.errors, dashboardId, panelId, link: mountLib.state().links.dashboard}
}
// The groups the viewer may share a document with: their own.
function shareGroups() {
    if (mountLib.state().policy.features.share !== true) return {errors: null, groups: []}
    const found = ask(REPORT + ".find#ShareGroups", {groupIds: []})
    return {
        errors: found.errors,
        groups: Array.from(found.groups || [])
            .filter(group => group.own)
            .map(group => ({value: group.userGroupId, label: group.description || group.userGroupId}))
    }
}

;({access, choose, entities, documents, dashboards, addPanel, open, upload, cells, edit, content, refresh, revert, state, save, exported, close, remove, shareGroups, profileId})
