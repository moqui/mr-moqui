// What the playlist screens of Dashboards show and ask (screen/Dashboards/Playlists.xml, Playlist.xml and
// WallDisplay.xml; docs/tools/Monitoring.md, "Playlists and the wall display"; plans/Admin-Screens-Down.md, Track I). A
// playlist is a report definition of kind playlist: dashboards in an order, each shown for a number of seconds.
//
//     Playlists    the playlists the viewer reads, a new one, and its owner's sharing (ReportShare.js)
//     Playlist     one playlist played in the app, for whoever sits at it: the request's range, each dashboard also
//                  opened on its own
//     WallDisplay  one playlist on a page of its own, full screen and unattended (a wall display, which an app may
//                  run on a shared device as a display account): the mount's range and never the request's, only a
//                  dashboard of the stored playlist, and a failure shown as "not available" with its text logged
//
// Presentation only: every read and write is an operation of org.moqui.report.ReportServices called as the viewer, so
// a playlist or a dashboard the viewer may not read is left out and a panel over a dataset they may not read says so; no
// entity is read here. Under a mount (template/ReportMount.js) the default range is the mount's and the definitions of
// the components it hides (reportHiddenComponents) are neither listed nor put in a playlist.
const mountLib = ec.resource.script("component://tools/template/ReportMount.js")
const shareLib = ec.resource.script("component://tools/template/ReportShare.js")
const {ask, offered, refuse} = ec.resource.script("component://tools/template/ReportDelivery.js")
const R = "org.moqui.report.ReportServices."
const itemsMax = 50
const displayUnavailable = "This dashboard is not available to this display."
const panelUnavailable = "This panel is not available to this display."

const text = value => String(value ?? "").trim()
const idList = value =>
    (value == null || value === "" ? [] : Array.from(typeof value === "string" ? value.split(",") : value))
        .map(text)
        .filter(Boolean)
const isPlaylist = definition => definition?.type === "report" && definition.body?.kind === "playlist"
const secondsOf = definition => Math.min(3600, Math.max(5, Math.round(Number(definition.body?.seconds) || 30)))
const itemsOf = definition => Array.from(definition.body?.playlist || []).slice(0, itemsMax)
const hiddenComponents = mount => (mount.mounted ? Array.from(ec.context.get("reportHiddenComponents") || []) : [])

// ===== refusals =====
function fail(words) {
    const error = new Error(words)
    error.refusal = true
    throw error
}
function attempt(action) {
    try {
        return action()
    } catch (error) {
        if (error?.refusal) return refuse(error.message)
        throw error
    }
}

// ===== Playlists =====
// The playlists the viewer reads, each with its dashboards' count, its seconds, whether it is the viewer's own and the
// group it is shared with (by name for one who may share and may name it, else only that it is shared); whether a new
// one is offered (the viewer's own grant of the engine's save, and under a mount an editor to order it in), the
// dashboards to put in one, and the sharing offered here (ReportShare.js).
function list() {
    const mount = mountLib.state()
    const hidden = hiddenComponents(mount)
    const shown = definition => !hidden.includes(definition.component)
    const found = ask("list#Definitions", {definitionType: "report"})
    const definitions = Array.from(found.out?.definitions || []).filter(definition => isPlaylist(definition) && shown(definition))
    const share = shareLib.listState(definitions.map(definition => definition.userGroupId))
    const canCreate = offered("save#Definition") && !!mount.links.definition
    const playlists = definitions.map(definition => {
        const owned = shareLib.owns(definition)
        const groupId = text(definition.userGroupId)
        const named = Object.hasOwn(share.names, groupId) ? share.names[groupId] : ""
        return {
            id: definition.id,
            title: definition.title || definition.id,
            count: itemsOf(definition).length,
            seconds: secondsOf(definition),
            owned,
            userGroupId: groupId,
            sharedWith: named,
            sharedText: !groupId ? "" : named ? "Shared with " + named : "Shared"
        }
    })
    const dashboards = canCreate ? ask("list#Definitions", {definitionType: "dashboard"}) : {out: {definitions: []}}
    return {
        failure: found.failure,
        playlists,
        canCreate,
        canEdit: !!mount.links.definition,
        mayShare: share.mayShare,
        shareGroups: share.groups,
        // the viewer's own playlists, for the share dialog's choice
        ownOptions: share.mayShare ? playlists.filter(row => row.owned).map(row => ({value: row.id, label: row.title})) : [],
        dashboardChoices: Array.from(dashboards.out?.definitions || [])
            .filter(shown)
            .map(definition => ({id: definition.id, title: definition.title || definition.id}))
    }
}
// createPlaylist: a playlist of one to fifty dashboards the viewer reads (under a mount none of a component it hides),
// each for five to 3600 seconds, saved as the viewer's own through the engine. Answers its id, or null with the refusal.
function create(p) {
    return attempt(() => {
        const mount = mountLib.state()
        const hidden = hiddenComponents(mount)
        const id = text(p.playlistId)
        if (!id) fail("Give the playlist an ID")
        const title = text(p.playlistTitle) || id
        const ids = idList(p.playlistDashboardIds)
        if (!ids.length || ids.length > itemsMax) fail("Choose between one and fifty dashboards")
        const seconds = parseInt(text(p.playlistSeconds) || "30", 10)
        if (!Number.isFinite(seconds) || seconds < 5 || seconds > 3600) fail("Choose a duration from five to 3600 seconds")
        for (const dashboardId of ids) {
            const entry = ask("get#Definition", {definitionId: dashboardId}).out?.definition
            if (!entry || entry.type !== "dashboard" || hidden.includes(entry.component))
                fail(dashboardId + " is not a dashboard you read")
        }
        const saved = ask("save#Definition", {
            definition: {
                id,
                type: "report",
                title,
                body: {kind: "playlist", seconds, playlist: ids.map(definitionId => ({definitionId}))}
            },
            note: "Created dashboard playlist"
        })
        if (saved.failure) fail(saved.failure)
        return saved.out.definition.id
    })
}

// ===== playing =====
// A playlist as the viewer reads it, or null: a definition they may not read and one that is no playlist alike.
function playlistOf(definitionId) {
    if (!text(definitionId)) return null
    const definition = ask("get#Definition", {definitionId: text(definitionId)}).out?.definition
    return isPlaylist(definition) ? definition : null
}
// Its dashboards as the viewer reads them, each with its index in the stored playlist (what the data transitions take);
// one the viewer may not read is left out and logged by name.
function entriesOf(playlist, what) {
    const entries = []
    for (const [index, item] of itemsOf(playlist).entries()) {
        const dashboard = ask("get#Definition", {definitionId: text(item?.definitionId)}).out?.definition
        if (dashboard?.type === "dashboard") entries.push({index, id: dashboard.id, title: dashboard.title || dashboard.id})
        else ec.logger.error(what + " " + playlist.id + ": dashboard " + text(item?.definitionId) + " unavailable")
    }
    return entries
}
// The dashboard at index of the stored playlist rendered as the viewer over a range: {id, output}, or null with why.
function dashboardAt(definitionId, index, range) {
    const playlist = playlistOf(definitionId)
    if (!playlist) return {failure: "Choose a dashboard playlist"}
    const items = itemsOf(playlist)
    const at = Number(index == null || index === "" ? 0 : index)
    const id = Number.isInteger(at) && at >= 0 && at < items.length ? text(items[at]?.definitionId) : ""
    if (!id) return {failure: "Choose a playlist item"}
    const dashboard = ask("get#Definition", {definitionId: id})
    if (dashboard.out?.definition?.type !== "dashboard")
        return {failure: dashboard.failure || "This playlist dashboard is not available"}
    const rendered = ask("render#Dashboard", {definitionId: id, from: range.from, thru: range.thru})
    if (!rendered.out?.dashboard) return {failure: rendered.failure || "This playlist dashboard is not available"}
    return {id, output: rendered.out.dashboard}
}
function framesOf(output) {
    const frames = {}
    for (const [panelId, panel] of Object.entries(output.panels || {})) frames[panelId] = panel.frames || []
    return frames
}

// Playlist (in the app): the playlist's title, seconds and dashboards, over the request's range or the mount's.
function player(p) {
    const mount = mountLib.state()
    const range = {from: text(p.from) || mount.policy.range.from, thru: text(p.thru) || mount.policy.range.thru}
    const playlist = playlistOf(p.definitionId)
    if (!playlist) throw new Error("Choose a dashboard playlist")
    return {
        title: playlist.title || playlist.id,
        seconds: secondsOf(playlist),
        range,
        entries: entriesOf(playlist, "Playlist"),
        links: mount.links
    }
}
// Its data transition: {dashboard, frames} of the dashboard at index, as the viewer; a refusal is the answer.
function playerData(p) {
    const mount = mountLib.state()
    const range = {from: text(p.from) || mount.policy.range.from, thru: text(p.thru) || mount.policy.range.thru}
    const found = dashboardAt(p.definitionId, p.index, range)
    if (found.failure) throw new Error(found.failure)
    found.output.definitionId = found.id
    found.output.absoluteTime = found.output.range
    return {dashboard: found.output, frames: framesOf(found.output)}
}

// WallDisplay (unattended): the playlist's title, seconds and dashboards over the mount's range; a playlist the viewer
// may not read, or none, has no title and the page says it is not available. The request names the playlist only.
function wall(definitionId) {
    const range = mountLib.state().policy.range
    const playlist = playlistOf(definitionId)
    if (!playlist) {
        if (text(definitionId)) ec.logger.error("Wall display playlist " + text(definitionId) + " unavailable")
        return {title: null, seconds: 30, range, entries: []}
    }
    return {
        title: playlist.title || playlist.id,
        seconds: secondsOf(playlist),
        range,
        entries: entriesOf(playlist, "Wall display playlist")
    }
}
// Its data transition, with what an unattended browser may not choose taken away: the range is the mount's and never
// the request's, so a display cannot make every panel query years; only a dashboard of the stored playlist is rendered;
// a panel's failure is shown as "not available" with its text logged, never shown on a wall, with no query inspection;
// and any failure answers one fixed sentence.
function wallData(p) {
    let answer = null
    try {
        answer = wallAnswer(p)
    } catch (error) {
        ec.logger.warn("Wall display playlist " + text(p.definitionId) + " item " + text(p.index) + " failed: " + String(error))
    }
    if (!answer) throw new Error(displayUnavailable)
    return answer
}
function wallAnswer(p) {
    const range = mountLib.state().policy.range
    const found = dashboardAt(p.definitionId, p.index, range)
    if (found.failure) {
        ec.logger.warn(
            "Wall display playlist " + text(p.definitionId) + " item " + text(p.index) + " unavailable: " + found.failure
        )
        return null
    }
    const output = found.output
    const shown = (notices, panelId) =>
        Array.from(notices || [])
            .filter(notice => notice?.level === "error")
            .map(notice => {
                ec.logger.warn(
                    "Wall display playlist " +
                        text(p.definitionId) +
                        " dashboard " +
                        found.id +
                        (panelId ? " panel " + panelId : "") +
                        ": " +
                        String(notice.message || notice.code || "")
                )
                return {level: "error", complete: false, message: ec.l10n.localize(panelUnavailable)}
            })
    for (const [panelId, panel] of Object.entries(output.panels || {})) {
        panel.notices = shown(panel.notices, panelId)
        delete panel.inspection
        delete panel.requests
    }
    output.notices = shown(output.notices, null)
    output.definitionId = found.id
    output.absoluteTime = output.range
    return {dashboard: output, frames: framesOf(output)}
}

;({list, create, player, playerData, wall, wallData})
