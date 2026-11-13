// CalcGrid in the Dashboards app (screen/Dashboards/CalcGrid.xml; docs/tools/Database.md, CalcGrid and access profiles;
// plans/IronCalc-DB.md Phase 2b): what the screen's transitions answer, over org.moqui.calc.CalcServices and, for a
// document's list, group and deletion, the report engine's definition services, each called as the viewer. Presentation
// only: a service decides every access. No entity is read here.
//
// The access profile a call is made under is the mount's (template/ReportMount.js, policy.calcProfile) when the mount
// names one, else the request's profileId, else none, which is the Tools app's own access. Naming a profile gives no
// right: the engine checks that the viewer holds it and reads only what it offers, with its entity filters.
const mountLib = ec.resource.script("component://tools/template/ReportMount.js")
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

// How the viewer may use CalcGrid here: the profiles to choose from (none when the mount names one), the Tools app's own
// access where there is no mount, whether they author, and the profile in use.
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
    return {
        errors: found.errors,
        userId: ec.user.userId,
        fixedProfile: fixed,
        tools,
        current,
        profiles,
        canAuthor: found.canAuthor === true,
        canShare: mount.policy.features.share === true
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
const cells = () =>
    ask(CALC + ".get#CalcCells", {
        profileId: profileId(),
        roomId: ec.context.get("roomId"),
        sheet: ec.context.get("sheet"),
        row: ec.context.get("row"),
        column: ec.context.get("column"),
        width: ec.context.get("width"),
        height: ec.context.get("height")
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
// Save the room's document; a group is named only where the mount's policy lets the viewer share.
function save() {
    const group = text(ec.context.get("userGroupId"))
    if (group && mountLib.state().policy.features.share !== true) return {errors: "Sharing is not offered to you here"}
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
// engine's table, bar or pie panel of the range, read by each viewer of the dashboard under their own current access.
// The engine's save holds the dashboard to its owner.
const VISUALIZATIONS = ["table", "bar", "pie", "stat"]
function addPanel() {
    const definitionId = text(ec.context.get("definitionId"))
    const dashboardId = text(ec.context.get("dashboardId"))
    const range = text(ec.context.get("range"))
    const visualization = text(ec.context.get("visualization")) || "table"
    if (!definitionId) return {errors: "Save the document first: a panel reads a saved document"}
    if (!dashboardId || !range) return {errors: "Choose a dashboard and a range"}
    if (!VISUALIZATIONS.includes(visualization)) return {errors: "A panel of a range is a table, a bar chart, a pie chart or a number"}
    const document = ask(REPORT + ".get#Definition", {definitionId})
    if (document.errors || !document.definition || document.definition.type !== TYPE) return {errors: document.errors || "Not a CalcGrid document"}
    const found = ask(REPORT + ".get#Definition", {definitionId: dashboardId})
    if (found.errors || !found.definition || found.definition.type !== "dashboard") return {errors: found.errors || "Choose a dashboard"}
    const dashboard = found.definition
    dashboard.body = dashboard.body || {}
    dashboard.body.panels = dashboard.body.panels || {}
    let panelId = "calc-" + definitionId.replace(/[^a-zA-Z0-9_.:-]/g, "-").slice(0, 60)
    for (let n = 2; dashboard.body.panels[panelId]; n++) panelId = panelId.replace(/(-\d+)?$/, "-" + n)
    dashboard.body.panels[panelId] = {
        title: text(ec.context.get("panelTitle")) || document.definition.title || "CalcGrid",
        visualization,
        queries: [{datasetId: "calcgrid", parameters: {definitionId, range}}]
    }
    const saved = ask(REPORT + ".save#Definition", {definition: dashboard, note: "Added a CalcGrid range"})
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

;({access, choose, entities, documents, dashboards, addPanel, open, cells, edit, content, refresh, save, exported, close, remove, shareGroups, profileId})
