// Offline map packages of org.moqui.maps.MapSourceServices (plans/Maps-Offline.md §2.1-§2.3; docs/tools/Maps.md "Offline
// packages"): a region at a zoom range, built on the server from what its owner may see when it is built, and downloaded
// as one zip file to the Android app or the desktop client.
//
// A package (moqui.maps.MapPackage) is its owner's alone: every service here acts on the signed-in user's own, and another
// user's is to them one that is not there. Its estimate is read from the archives' directories before anything is built
// (estimate#PackageExtract, in Rust), and a package past maps@package-max-bytes is refused there. The build is the
// MapPackageBuild ServiceJob, run as the owner (buildPackage): the basemap archives the owner's map profile offers them
// now, each cut to the box and zooms (build#PackageExtract: moqui-tools maps_extract.rs), their styles, sprites and fonts,
// the outline, and as the owner asked their places, the overlays they can read now that meet the box (a tiled overlay as
// its tiles' extract), the postal codes and the Overture places and addresses of the box, then a manifest and the zip
// (ec.resource.writeZip, every member stored as it is), at runtime://tmp/map-packages/{packageId}-{revision}-{buildKey}.zip. A
// member that cannot be read is logged and named in the manifest's failures, and the rest goes on; an archive registered
// again before or while it is cut fails the build (its recorded revision is checked when the build starts and when the
// cuts end, and the cut checks its tag when it starts and ends). Record layers (an app's records) and a provider's remote
// tiles are never packaged: a record's access is read at each request, and a provider's terms allow a limited offline
// download, not a mirror. The run's progress and its cancel are on the row; a run gone, ended or stalled (no word for an
// hour once it started) is ended by the next scheduler pass (a build asked for or ending, the hourly clean) or when its
// owner next updates, cancels or deletes it. Update builds the package again as a new revision, and the old file is
// removed once the new one is ready.
//
// The Offline screen's package transition sends the zip by ec.web.sendPinnedResourceResponse, its SHA-256 the strong tag,
// by range, checking the owner on every request (packageAccess); a revision that is no longer the package's is 409, and
// each download is recorded (MapPackageDownload) with the device's UserLoginKey when the request came signed in by one.
// The hourly clean (cleanPackages) removes the files of deleted packages and of packages not downloaded for
// maps@package-keep-days.
const PACKAGE = "moqui.maps.MapPackage"
const DOWNLOAD = "moqui.maps.MapPackageDownload"
const LAYER = "moqui.maps.MapLayer"
const S = "org.moqui.maps.MapSourceServices."
const BUILD_JOB = "MapPackageBuild"
const ROOT = "runtime://tmp/map-packages"
const PACKAGE_PATH = "/apps/maps/Offline/package"
const SCHEMA_VERSION = 1
const BUSY = ["MapPkgPending", "MapPkgBuilding"]
// a started run with no word for this long has stalled: its row is written when its progress percent changes (at most once
// a second), and when each member is done
const STALE_MS = 60 * 60 * 1000
// a run handed to the job that never started (the worker queue is in memory: a restart loses it, and its ServiceJobRun
// keeps no start and no end) is judged lost after this long since its ServiceJobRun row was written
const QUEUED_STALE_MS = 6 * 60 * 60 * 1000
const MAX_PACKAGES = 50
const MAX_ZOOM = 16
const DEFAULT_MAX_BYTES = 2147483648
const DEFAULT_USER_MAX_BYTES = 8589934592
const DEFAULT_CONCURRENT_BUILDS = 1
const DEFAULT_KEEP_DAYS = 30
// what a lookup row takes in its member, near enough, for the estimate
const POSTAL_ROW_BYTES = 200
const OVERTURE_ROW_BYTES = {PLACES: 400, ADDRESSES: 250}
// the rows of a lookup member at most: a region's postal codes, its Overture places and addresses
const MAX_POSTAL = 50000
const MAX_OVERTURE = 200000
// a download's row is touched at most this often, and the package's last download with it
const DOWNLOAD_EVERY_MS = 60 * 60 * 1000
// the browser libraries a style names (webroot's /libs), read where the webroot serves them from
const LIBS_PATH = "/libs/"
const LIBS_LOCATION = "component://webroot/screen/webroot/libs/"
const OUTLINE = "/libs/naturalearth/countries-110m.json"
const FONTS = "mapfonts/noto-sans"
const FONT_TEMPLATE = "fonts/{font-family}/{fontweight}{-fontstyle}.css"
const SPRITE_SUFFIXES = [".json", ".png", "@2x.json", "@2x.png"]

const L = value => ec.l10n.localize(value)
const say = (value, values) => ec.resource.expand(L(value), "", values || {})
const text = value => (value == null ? "" : String(value).trim())
const yes = value => value === true || value === "true" || value === "Y"
const refused = (reasonCode, reason, extra) => Object.assign({applied: false, reasonCode, reason}, extra || {})
const conf = selector => text(ec.factory.getConfValue(selector))
const num = value => {
    if (value == null || value === "") return null
    const found = typeof value?.toNumber === "function" ? value.toNumber() : Number(value)
    return Number.isFinite(found) ? found : null
}
const millisOf = value => {
    if (value == null || value === "") return null
    if (typeof value === "number") return value
    if (typeof value.getTime === "function") return value.getTime()
    const parsed = /^\d+$/.test(String(value)) ? Number(value) : Date.parse(String(value))
    return Number.isFinite(parsed) ? parsed : null
}
const find = (entity, conditions) =>
    ec.entity
        .find(entity)
        .condition(conditions || {})
        .disableAuthz()
const one = (entity, conditions) => find(entity, conditions).one()
const maxBytes = () => (Number(conf("maps@package-max-bytes")) > 0 ? Number(conf("maps@package-max-bytes")) : DEFAULT_MAX_BYTES)
const keepDays = () => (Number(conf("maps@package-keep-days")) > 0 ? Number(conf("maps@package-keep-days")) : DEFAULT_KEEP_DAYS)
const userMaxBytes = () =>
    Number(conf("maps@package-user-max-bytes")) > 0 ? Number(conf("maps@package-user-max-bytes")) : DEFAULT_USER_MAX_BYTES
const concurrentBuilds = () =>
    Number.isInteger(Number(conf("maps@package-concurrent-builds"))) && Number(conf("maps@package-concurrent-builds")) > 0
        ? Number(conf("maps@package-concurrent-builds"))
        : DEFAULT_CONCURRENT_BUILDS
// a build's own key, in its row while it is the package's build, its job's parameters and its files' names
const newBuildKey = () => (text(ec.user.randomString(20)).replace(/[^A-Za-z0-9]/g, "") + Date.now().toString(36)).slice(0, 24)
const bytesText = value => {
    const bytes = Number(value)
    if (!Number.isFinite(bytes) || bytes < 0) return ""
    const units = ["bytes", "KiB", "MiB", "GiB", "TiB"]
    let amount = bytes
    let unit = 0
    while (amount >= 1024 && unit < units.length - 1) {
        amount /= 1024
        unit++
    }
    return (unit === 0 ? String(bytes) : amount.toFixed(amount >= 10 ? 0 : 1)) + " " + units[unit]
}
// a member's name made of an ID: letters, digits, dot, dash and underscore
const safeName = value => text(value).replace(/[^A-Za-z0-9._-]/g, "_") || "member"

let sourcesScript = null
const sources = () =>
    (sourcesScript = sourcesScript || ec.resource.script("component://framework/service/org/moqui/maps/MapSources.js"))
let overlaysScript = null
const overlays = () =>
    (overlaysScript = overlaysScript || ec.resource.script("component://framework/service/org/moqui/maps/MapOverlays.js"))

// A call with its own errors kept apart: its result, or null when it failed (the first error in errorsOut). withAuthz
// false calls a service no group is granted (the Rust extract), after the checks here.
function probe(name, parameters, errorsOut, withAuthz) {
    let out = null
    ec.message.pushErrors()
    try {
        let call = ec.service.sync().name(name).parameters(parameters)
        if (!withAuthz) call = call.disableAuthz()
        out = call.call()
        if (ec.message.hasError()) {
            if (errorsOut) errorsOut.push(ec.message.getErrorsString())
            out = null
        }
    } catch (err) {
        ec.logger.warn("Map packages: " + name + " not answered: " + (err?.message ?? err))
        if (errorsOut) errorsOut.push(String(err?.message ?? err))
        out = null
    } finally {
        ec.message.clearErrors()
        ec.message.popErrors()
    }
    return out
}
// A read or write of the resource facade or of rows no group is granted, after the checks here
function quietly(body) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return body()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
const writeRow = (row, create) => quietly(() => (create ? row.create() : row.update()))

// ===== what a package asks =====
// {ok, fields} of a request (name, west, south, east, north, minZoom, maxZoom, profileId, basemapLayerIds, include*), or a
// refusal
function requestOf(p) {
    const box = ["west", "south", "east", "north"].map(key => num(p[key]))
    const [west, south, east, north] = box
    const lon = value => value !== null && value >= -180 && value <= 180
    const lat = value => value !== null && value >= -90 && value <= 90
    if (!lon(west) || !lon(east) || !lat(south) || !lat(north) || south >= north || west === east)
        return refused("BBOX_INVALID", L("Choose a region: west, south, east and north in degrees, south below north"))
    const minZoom = num(p.minZoom) ?? 0
    const maxZoom = num(p.maxZoom) ?? 14
    if (!Number.isInteger(minZoom) || !Number.isInteger(maxZoom) || minZoom < 0 || maxZoom > MAX_ZOOM || minZoom > maxZoom)
        return refused("ZOOMS_INVALID", say("Choose zooms from 0 to ${max}, the lowest not above the highest", {max: MAX_ZOOM}))
    const layerIds = (Array.isArray(p.basemapLayerIds) ? p.basemapLayerIds : text(p.basemapLayerIds).split(","))
        .map(text)
        .filter(id => id)
        .slice(0, 20)
    const flag = (key, otherwise) => (p[key] == null || p[key] === "" ? otherwise : yes(p[key]) ? "Y" : "N")
    return {
        ok: true,
        fields: {
            name: text(p.name).slice(0, 100) || L("Offline map"),
            west,
            south,
            east,
            north,
            minZoom,
            maxZoom,
            profileId: text(p.profileId) || null,
            basemapLayerIds: layerIds.length ? layerIds.join(",") : null,
            includePlaces: flag("includePlaces", "Y"),
            includeOverlays: flag("includeOverlays", "Y"),
            includePostal: flag("includePostal", "Y"),
            includeOverture: flag("includeOverture", "Y")
        }
    }
}
const boxOf = fields => ({west: num(fields.west), south: num(fields.south), east: num(fields.east), north: num(fields.north)})
// whether bounds [west, south, east, north] (an overlay's) meet the box, across the antimeridian as the box may be
function meets(bounds, box) {
    if (!Array.isArray(bounds) || bounds.some(value => num(value) === null)) return true
    const [w, s, e, n] = bounds.map(num)
    if (n < box.south || s > box.north) return false
    const spans = box.west > box.east ? [[box.west, 180], [-180, box.east]] : [[box.west, box.east]]
    return spans.some(([from, to]) => e >= from && w <= to)
}

// The current user's effective map configuration for the profile (get#EffectiveConfig, which decides what they may be
// offered), or null
function effectiveConfig(profileId) {
    const out = probe("org.moqui.maps.MapServices.get#EffectiveConfig", {profileId: profileId || null, recordLayers: false}, null, true)
    return out?.config || null
}

// The basemap archives the package cuts: the profile's basemap and those of its offered basemaps (the basemap override)
// asked for, each a registered PMTiles archive the user may read now; the archives shared by several layers (a light and
// a dark style of one extract) cut once. {archives: [{key, layerIds, entries, source}], omitted: [{id, kind, reason}]}
function basemapArchives(config, fields) {
    const offered = Array.from(config?.profile?.basemaps || [])
    const asked = text(fields.basemapLayerIds)
        .split(",")
        .map(text)
        .filter(id => id)
    const omitted = []
    let chosen
    if (asked.length) {
        chosen = []
        for (const id of asked) {
            const entry = offered.find(each => each.id === id)
            // a layer the user is not offered is, to them, not there
            if (!entry) omitted.push({id, kind: "basemap", reason: L("not offered to you")})
            else chosen.push(entry)
        }
    } else {
        const main = offered.find(each => each.id === config?.profile?.basemap) || offered[0]
        chosen = main ? [main] : []
    }
    const archives = []
    const add = entry => {
        if (entry.kind !== "pmtiles") {
            omitted.push({
                id: entry.id,
                kind: "basemap",
                reason:
                    entry.kind === "outline"
                        ? L("the outline is packaged as outline.json")
                        : L("not a map archive on this server: a provider's tiles are not packaged")
            })
            return
        }
        const source = sources().packageSource(entry.id)
        if (!source.ok) {
            omitted.push({id: entry.id, kind: "basemap", reason: text(source.reason) || L("not available")})
            return
        }
        const key = source.location + "|" + source.tag
        const known = archives.find(archive => archive.key === key)
        if (known) {
            known.layerIds.push(entry.id)
            known.entries.push(entry)
        } else archives.push({key, layerIds: [entry.id], entries: [entry], source})
    }
    chosen.forEach(add)
    // without a choice: the other offered layers of the same archive (the other style of the extract)
    if (!asked.length && archives.length)
        for (const entry of offered) {
            if (chosen.includes(entry) || entry.kind !== "pmtiles") continue
            const source = sources().packageSource(entry.id)
            if (source.ok && source.location + "|" + source.tag === archives[0].key) {
                archives[0].layerIds.push(entry.id)
                archives[0].entries.push(entry)
            }
        }
    return {archives, omitted}
}

// The user's overlays (their own and those they read now) that meet the box, and their places when asked
function overlaysOf(fields, box) {
    if (fields.includeOverlays !== "Y" && fields.includePlaces !== "Y") return []
    const found = Array.from(overlays().execute("findOverlays", {}).overlayList || [])
    return found.filter(entry => {
        // another user's places are theirs, even shared: never in a package
        if (entry.isPlaces) return entry.isOwn && fields.includePlaces === "Y"
        return fields.includeOverlays === "Y" && meets(entry.bounds, box)
    })
}

const extractParameters = (source, fields) => ({
    location: source.location,
    tag: source.tag,
    size: source.size,
    modifiedMillis: source.modifiedMillis ?? null,
    west: num(fields.west),
    south: num(fields.south),
    east: num(fields.east),
    north: num(fields.north),
    minZoom: num(fields.minZoom),
    maxZoom: num(fields.maxZoom)
})

// The reasons of an extract's refusal a user is shown as they are (they name no path of the server); any other is
// logged where it failed and shown in general words
const SHOWN_REASONS = ["NO_TILES", "NO_ZOOMS", "TOO_LARGE", "TOO_MANY_TILES", "BBOX_INVALID", "ZOOMS_INVALID"]
const reasonOf = out => (SHOWN_REASONS.includes(text(out?.reasonCode)) && text(out?.reason)) || L("the map archive could not be read")

// The lookup members' rows in the box near enough, from counts: {postal, overture: {PLACES, ADDRESSES}} with their bytes
function lookupBytes(fields, box) {
    const cf = ec.entity.conditionFactory
    const spans = box.west > box.east ? [[box.west, 180], [-180, box.east]] : [[box.west, box.east]]
    const inBox = () =>
        cf.makeCondition(
            [
                cf.makeCondition("latitude", "greater-equals", box.south),
                cf.makeCondition("latitude", "less-equals", box.north),
                spans
                    .map(([from, to]) =>
                        cf.makeCondition(
                            cf.makeCondition("longitude", "greater-equals", from),
                            "and",
                            cf.makeCondition("longitude", "less-equals", to)
                        )
                    )
                    .reduce((all, each) => (all ? cf.makeCondition(all, "or", each) : each), null)
            ],
            "and"
        )
    let bytes = 0
    try {
        if (fields.includePostal === "Y")
            bytes += Math.min(MAX_POSTAL, Number(ec.entity.find("moqui.basic.GeoPoint").condition(inBox()).disableAuthz().count())) * POSTAL_ROW_BYTES
        if (fields.includeOverture === "Y")
            for (const [themeId, entity] of [
                ["PLACES", "moqui.maps.OverturePlace"],
                ["ADDRESSES", "moqui.maps.OvertureAddress"]
            ]) {
                const imports = Array.from(find("moqui.maps.OvertureRegionTheme", {themeId}).list())
                    .map(row => text(row.activeImportId))
                    .filter(id => id)
                if (!imports.length) continue
                const rows = Number(
                    ec.entity.find(entity).condition("importId", "in", imports).condition(inBox()).disableAuthz().count()
                )
                bytes += Math.min(MAX_OVERTURE, rows) * OVERTURE_ROW_BYTES[themeId]
            }
    } catch (err) {
        ec.logger.warn("Map package estimate: the lookup rows could not be counted: " + (err?.message ?? err))
    }
    return bytes
}

// The bytes the user's other packages hold: their files, or a busy one's estimate
function userBytes(exceptPackageId) {
    let total = 0
    for (const row of Array.from(find(PACKAGE, {ownerUserId: ec.user.userId}).selectField("packageId,statusId,size,estimatedSize").list())) {
        if (exceptPackageId && String(row.packageId) === String(exceptPackageId)) continue
        total += BUSY.includes(text(row.statusId)) ? num(row.estimatedSize) || 0 : num(row.size) || 0
    }
    return total
}

// What the package would hold, from the archives' directories and counts of the lookup rows: {ok, estimate, revisions}
// or a refusal: NO_ARCHIVE (no registered basemap archive to cut), TOO_LARGE past maps@package-max-bytes, TOO_LARGE_FOR_USER
// past maps@package-user-max-bytes with the user's other packages (exceptPackageId, the one updated, left out)
function estimateOf(fields, exceptPackageId) {
    const config = effectiveConfig(fields.profileId)
    if (!config) return refused("CONFIG_UNAVAILABLE", L("Your map configuration could not be read"))
    const box = boxOf(fields)
    const {archives, omitted} = basemapArchives(config, fields)
    const layers = []
    const revisions = {}
    let bytes = 0
    let tileCount = 0
    const limit = maxBytes()
    const estimateOne = (id, label, source) => {
        const errors = []
        const out = probe(S + "estimate#PackageExtract", Object.assign(extractParameters(source, fields), {maxBytes: limit}), errors)
        if (!out || !out.valid) {
            if (text(out?.reasonCode) === "TOO_LARGE" || text(out?.reasonCode) === "TOO_MANY_TILES") return out
            if (errors.length) ec.logger.warn("Map package estimate: " + id + " not estimated: " + errors[0])
            omitted.push({id, kind: "archive", reason: reasonOf(out)})
            return null
        }
        layers.push({id, label, bytes: Number(out.bytes) || 0, tileCount: Number(out.tileCount) || 0,
            minZoom: Number(out.minZoom), maxZoom: Number(out.maxZoom)})
        revisions[id] = source.revision
        bytes += Number(out.bytes) || 0
        tileCount += Number(out.tileCount) || 0
        return null
    }
    for (const archive of archives) {
        const stop = estimateOne(archive.layerIds[0], archive.entries.map(entry => entry.label).join(", "), archive.source)
        if (stop) return refused(text(stop.reasonCode), reasonOf(stop))
        if (revisions[archive.layerIds[0]] !== undefined)
            for (const id of archive.layerIds.slice(1)) revisions[id] = archive.source.revision
    }
    // a package needs a basemap archive of this server: a provider's tiles are not packaged
    if (!layers.length)
        return refused(
            "NO_ARCHIVE",
            L("No map archive of this server can be cut for this region: an offline map needs a basemap registered under Maps › Configuration › Archives"),
            {estimate: {omitted}}
        )
    for (const entry of overlaysOf(fields, box)) {
        if (entry.tilesStatus === "ready") {
            const source = sources().packageOverlayArchive(entry.overlayId)
            if (!source.ok) {
                omitted.push({id: "overlay:" + entry.overlayId, kind: "overlay", reason: text(source.reason)})
                continue
            }
            const stop = estimateOne("overlay:" + entry.overlayId, entry.label, source)
            if (stop) return refused(text(stop.reasonCode), reasonOf(stop))
        } else bytes += Math.max(1024, (Number(entry.featureCount) || 0) * 200)
    }
    bytes += lookupBytes(fields, box)
    // record layers and a profile's other overlay layers are not packaged in this build
    for (const layer of Array.from(config.recordLayers || []))
        omitted.push({id: layer.id, kind: "record", reason: L("a record layer is read at each request, not packaged")})
    for (const layer of Array.from(config.layers || []))
        omitted.push({id: layer.id, kind: "layer", reason: L("a profile's overlay layer is not packaged in this build")})
    const estimate = {bytes, bytesText: bytesText(bytes), tileCount, layers, omitted, maxBytes: limit}
    if (bytes > limit)
        return refused(
            "TOO_LARGE",
            say("The package would be about ${size}, more than the ${limit} a package may be: choose a smaller region or fewer zooms", {
                size: bytesText(bytes),
                limit: bytesText(limit)
            }),
            {estimate}
        )
    const overUser = userCap(bytes, exceptPackageId, estimate)
    if (overUser) return overUser
    return {ok: true, estimate, revisions}
}
// TOO_LARGE_FOR_USER when the user's other packages and this one's bytes pass maps@package-user-max-bytes, else null
function userCap(bytes, exceptPackageId, estimate) {
    const others = userBytes(exceptPackageId)
    const userLimit = userMaxBytes()
    if (others + bytes <= userLimit) return null
    return refused(
        "TOO_LARGE_FOR_USER",
        say("Your offline maps would hold about ${size} in all, more than the ${limit} you may keep: delete one first", {
            size: bytesText(others + bytes),
            limit: bytesText(userLimit)
        }),
        {estimate}
    )
}

// ===== estimate#Package, create#Package, update#Package =====
function estimatePackage(p) {
    if (!text(ec.user.userId)) throw new Error(L("Sign in to make an offline map"))
    const request = requestOf(p)
    if (!request.ok) return request
    const found = estimateOf(request.fields)
    if (!found.ok) return found
    return {applied: true, reasonCode: null, reason: null, estimate: found.estimate}
}

// A package's run handed to a worker when this transaction commits, with its build's key; it runs as the package's owner.
// The job's rows are no group's to read: it is started as the framework, for the owner checked when it was asked for
function startBuild(row) {
    const jobRunId = quietly(() =>
        ec.service
            .job(BUILD_JOB)
            .parameters({packageId: String(row.packageId), buildKey: text(row.buildKey)})
            .run()
    )
    if (jobRunId) {
        row.set("jobRunId", String(jobRunId))
        writeRow(row, false)
        return true
    }
    ec.message.clearErrors()
    row.setAll({statusId: "MapPkgFailed", statusNote: L("The build did not start"), buildKey: null})
    writeRow(row, false)
    return false
}

// Start the waiting builds the installation has room for (maps@package-concurrent-builds). One scheduler at a time:
// it first locks the MapPackageBuild ServiceJob row (its mutex), then reads the busy packages locked; a run that has
// ended without saying so (its ServiceJobRun gone or ended, or no word for an hour) is ended first, its owner told, so
// a dead run never holds the queue; a package handed to the job (its jobRunId) counts, a pending one without waits its
// turn, oldest first. Run in a transaction holding no package lock of its own: from a build's end and the hourly clean
// in one of its own, and after the commit of a create, update, cancel or delete (scheduleAfterCommit).
function scheduleBuilds() {
    try {
        // the mutex; on SQLite, whose one writer at a time serializes the schedulers anyway, the lock is a no-op
        if (!find("moqui.service.job.ServiceJob", {jobName: BUILD_JOB}).forUpdate(true).one())
            ec.logger.error("Map packages: the " + BUILD_JOB + " ServiceJob is missing: the schedulers are not serialized")
        const locked = Array.from(
            ec.entity
                .find(PACKAGE)
                .condition("statusId", "in", BUSY)
                .orderBy("createdDate,packageId")
                .forUpdate(true)
                .disableAuthz()
                .list()
        )
        const busy = locked.filter(row => !(text(row.jobRunId) && endStale(row)))
        let running = busy.filter(row => text(row.jobRunId)).length
        const owners = new Map()
        const ownerLocked = userId => {
            if (!owners.has(userId)) owners.set(userId, accountLocked(one("moqui.security.UserAccount", {userId})))
            return owners.get(userId)
        }
        for (const row of busy) {
            if (running >= concurrentBuilds()) break
            if (text(row.jobRunId) || !text(row.buildKey)) continue
            // an owner locked out for a few minutes after failed logins: their build waits for a later pass
            if (ownerLocked(text(row.ownerUserId))) continue
            if (startBuild(row)) running++
        }
    } catch (err) {
        ec.logger.error("Map packages: the waiting builds were not started: " + (err?.message ?? err))
    }
}
// scheduleBuilds once this transaction commits (schedule#PackageBuilds, run async then): the caller's own row write is
// committed and its locks let go before the scheduler takes its mutex and the busy rows, so no two wait on each other
function scheduleAfterCommit() {
    try {
        // no group is granted the scheduler: registered as the framework, after the caller was checked
        quietly(() => ec.service.special().name(S + "schedule#PackageBuilds").registerOnCommit())
    } catch (err) {
        ec.logger.error("Map packages: the next build was not scheduled: " + (err?.message ?? err))
    }
}
function schedulePackageBuilds() {
    scheduleBuilds()
    return {}
}

// An account closed as the framework reads it (login.rs account_closed): disabled for good (no disabledDateTime) or past
// its terminateDate. A tarpit lock after failed logins (disabled with a disabledDateTime) is not: it ends after
// user-facade/login@disable-minutes, and its builds wait, never fail
const millisNow = () => Date.now()
function accountClosed(account) {
    if (!account) return true
    if (account.disabled === "Y" && millisOf(account.disabledDateTime) === null) return true
    const terminate = millisOf(account.terminateDate)
    return terminate !== null && millisNow() > terminate
}
// Locked for now as login.rs still_disabled reads it: disabled with a disabledDateTime within user-facade/login@disable-
// minutes, read as policy.rs disable_minutes reads it (the raw text a whole number, else 5; 0 or less, no lock window)
function accountLocked(account) {
    const at = account ? millisOf(account.disabledDateTime) : null
    if (!account || account.disabled !== "Y" || at === null) return false
    const raw = ec.factory.getConfValue("user-facade/login@disable-minutes")
    const minutes = typeof raw === "string" && /^[+-]?\d+$/.test(raw) ? Number(raw) : 5
    return at + minutes * 60000 > millisNow()
}

// The signed-in user's account row, locked: what serializes their creates and updates, so their counts hold
const lockUser = () => find("moqui.security.UserAccount", {userId: ec.user.userId}).forUpdate(true).one()
// A refusal when the user has a package pending or building (one at a time), but the one given; the rows read locked,
// so a stale one is ended only while its build is not ending it
function userBusy(exceptPackageId) {
    const rows = Array.from(
        ec.entity
            .find(PACKAGE)
            .condition("ownerUserId", ec.user.userId)
            .condition("statusId", "in", BUSY)
            .orderBy("createdDate,packageId")
            .forUpdate(true)
            .disableAuthz()
            .list()
    )
    const other = rows.find(row => String(row.packageId) !== String(exceptPackageId || "") && !endStale(row))
    return other
        ? refused("PACKAGE_BUSY", say("Your offline map ${name} is being built: wait for it to end, or cancel it", {name: text(other.name)}))
        : null
}

function createPackage(p) {
    if (!text(ec.user.userId)) throw new Error(L("Sign in to make an offline map"))
    const request = requestOf(p)
    if (!request.ok) return request
    // the estimate reads the archives' directories, slowly for a large region: before the user's row is locked
    const found = estimateOf(request.fields)
    if (!found.ok) return found
    // then under the lock the counts that must hold whatever the user's other requests did meanwhile
    lockUser()
    const busy = userBusy(null)
    if (busy) return afterStale(busy)
    const count = Number(find(PACKAGE, {ownerUserId: ec.user.userId}).count())
    if (count >= MAX_PACKAGES)
        return afterStale(refused("TOO_MANY_PACKAGES", say("You have ${count} offline maps: delete one first", {count})))
    const overUser = userCap(found.estimate.bytes, null, found.estimate)
    if (overUser) return afterStale(overUser)
    const row = ec.entity.makeValue(PACKAGE)
    row.setSequencedIdPrimary()
    row.setAll(
        Object.assign({}, request.fields, {
            ownerUserId: ec.user.userId,
            statusId: "MapPkgPending",
            progress: 0,
            cancelRequested: "N",
            estimatedSize: found.estimate.bytes,
            tileCount: found.estimate.tileCount,
            sourceRevisions: JSON.stringify(found.revisions),
            buildKey: newBuildKey(),
            createdDate: ec.user.nowTimestamp
        })
    )
    writeRow(row, true)
    scheduleAfterCommit()
    return {applied: true, reasonCode: null, reason: null, packageId: String(row.packageId), estimate: found.estimate}
}

// The user's own package (locked with forUpdate), or null
function ownPackage(packageId, forUpdate) {
    const id = text(packageId)
    const row = id ? find(PACKAGE, {packageId: id}).forUpdate(!!forUpdate).one() : null
    return row && text(ec.user.userId) && row.ownerUserId === ec.user.userId ? row : null
}
const notThere = () => refused("RECORD_UNAVAILABLE", L("The offline map is not available"))

// Whether a busy package's run has ended without saying so: its ServiceJobRun gone or ended; once started, no word from
// it for STALE_MS (its last progress, the package's progressDate, which only the build writes, else its start); still
// waiting for a worker (no startTime), lost QUEUED_STALE_MS after its ServiceJobRun row was written (made when it was
// handed to a worker, written next at its start). Neither clock is the package row's own stamp, which a download of an
// earlier revision or a cancel also moves. A pending package not yet handed to the job waits its turn: it has not
// ended; a building one with no run has.
function runEnded(row) {
    const jobRunId = text(row.jobRunId)
    if (!jobRunId) return text(row.statusId) !== "MapPkgPending"
    const run = one("moqui.service.job.ServiceJobRun", {jobRunId})
    if (!run || run.endTime) return true
    // queued and never started: lost after QUEUED_STALE_MS (a restart while it waited); a late start writes nothing,
    // its key gone
    if (millisOf(run.startTime) === null) {
        const queued = millisOf(run.lastUpdatedStamp)
        return queued !== null && Date.now() - queued > QUEUED_STALE_MS
    }
    const since = Math.max(millisOf(row.progressDate) ?? 0, millisOf(run.startTime) ?? 0)
    return since > 0 && Date.now() - since > STALE_MS
}
// A busy package whose run has ended is ended here: failed, or cancelled when a cancel was asked. Its build's key goes,
// so a run that was only slow stops at its next batch and writes nothing
function endStale(row) {
    if (!BUSY.includes(text(row.statusId)) || !runEnded(row)) return false
    const cancelled = row.cancelRequested === "Y"
    row.setAll({
        statusId: cancelled ? "MapPkgCancelled" : "MapPkgFailed",
        statusNote: cancelled ? L("Cancelled") : L("The build stopped without finishing"),
        cancelRequested: "N",
        buildKey: null,
        jobRunId: null
    })
    writeRow(row, false)
    notifyOwner(row, text(row.statusId), text(row.statusNote))
    staleEnded = true
    return true
}
// Whether this run ended a stale build: a turn of the queue freed, which a request that then refuses still gives on
let staleEnded = false
const afterStale = refusal => {
    if (staleEnded) scheduleAfterCommit()
    return refusal
}

function updatePackage(p) {
    if (!text(ec.user.userId)) throw new Error(L("Sign in to update an offline map"))
    const asked = ownPackage(p.packageId, false)
    if (!asked) return notThere()
    const busyRefusal = () => refused("PACKAGE_BUSY", L("The offline map is being built: wait for it to end, or cancel it"))
    if (BUSY.includes(text(asked.statusId)) && !runEnded(asked)) return busyRefusal()
    // the estimate first, unlocked; then under the user's and the package's locks the checks again
    const request = requestOf(asked.getMap())
    if (!request.ok) return request
    const found = estimateOf(request.fields, asked.packageId)
    if (!found.ok) return found
    lockUser()
    const row = ownPackage(p.packageId, true)
    if (!row) return notThere()
    if (BUSY.includes(text(row.statusId)) && !endStale(row)) return busyRefusal()
    const busy = userBusy(row.packageId)
    if (busy) return afterStale(busy)
    const overUser = userCap(found.estimate.bytes, row.packageId, found.estimate)
    if (overUser) return afterStale(overUser)
    row.setAll({
        statusId: "MapPkgPending",
        statusNote: null,
        progress: 0,
        cancelRequested: "N",
        estimatedSize: found.estimate.bytes,
        tileCount: found.estimate.tileCount,
        sourceRevisions: JSON.stringify(found.revisions),
        buildKey: newBuildKey(),
        jobRunId: null
    })
    writeRow(row, false)
    scheduleAfterCommit()
    return {applied: true, reasonCode: null, reason: null, packageId: String(row.packageId), estimate: found.estimate}
}

// ===== cancel#Package, delete#Package, find#Packages =====
function cancelPackage(p) {
    if (!text(ec.user.userId)) throw new Error(L("Sign in to cancel"))
    const row = ownPackage(p.packageId, true)
    if (!row || !BUSY.includes(text(row.statusId))) return refused("RECORD_UNAVAILABLE", L("The offline map is not being built"))
    row.set("cancelRequested", "Y")
    if (text(row.statusId) === "MapPkgPending" && !text(row.jobRunId)) {
        // waiting its turn: it ends now, and never starts
        row.setAll({statusId: "MapPkgCancelled", statusNote: L("Cancelled"), cancelRequested: "N", buildKey: null})
        writeRow(row, false)
    } else if (!endStale(row)) writeRow(row, false)
    // a run that has ended without saying so is ended now, its turn given to the next; a live one stops at its next batch
    scheduleAfterCommit()
    return {applied: true, reasonCode: null, reason: null}
}

// Remove a file or a folder and everything in it, under ROOT only; each that cannot be removed is logged
function removeTree(location) {
    if (!text(location).startsWith(ROOT + "/")) return
    quietly(() => {
        const ref = ec.resource.getLocationReference(location)
        if (!ref.getExists()) return
        if (ref.isDirectory()) for (const child of Array.from(ref.getDirectoryEntries() || [])) removeTree(child.location)
        try {
            ref.delete()
        } catch (err) {
            ec.logger.warn("Map package file " + location + " not removed: " + (err?.message ?? err))
        }
    })
}

function deletePackage(p) {
    if (!text(ec.user.userId)) throw new Error(L("Sign in to delete an offline map"))
    const row = ownPackage(p.packageId, true)
    if (!row) return notThere()
    if (BUSY.includes(text(row.statusId)) && !endStale(row))
        return refused("PACKAGE_BUSY", L("The offline map is being built: cancel it first"))
    removeTree(text(row.location))
    quietly(() => {
        ec.entity.find(DOWNLOAD).condition("packageId", row.packageId).deleteAll()
        row.delete()
    })
    // a stale build it ended frees its turn
    scheduleAfterCommit()
    return {applied: true, reasonCode: null, reason: null}
}

const STATUS_TEXT = {
    MapPkgPending: "Waiting to build",
    MapPkgBuilding: "Building",
    MapPkgReady: "Ready",
    MapPkgFailed: "Failed",
    MapPkgCancelled: "Cancelled",
    MapPkgExpired: "Removed after no download"
}
const dateText = value => (value == null ? "" : text(ec.l10n.format(value, "yyyy-MM-dd HH:mm")))
function packageEntry(row) {
    const statusId = text(row.statusId)
    const busy = BUSY.includes(statusId)
    const downloadable = !!(text(row.location) && text(row.sha256) && row.revision != null)
    return {
        packageId: String(row.packageId),
        name: text(row.name),
        bbox: [row.west, row.south, row.east, row.north].map(num),
        minZoom: num(row.minZoom),
        maxZoom: num(row.maxZoom),
        zoomsText: num(row.minZoom) + "–" + num(row.maxZoom),
        statusId,
        statusText:
            statusId === "MapPkgPending" && !text(row.jobRunId) ? L("Waiting for its turn") : L(STATUS_TEXT[statusId] || statusId),
        statusNote: text(row.statusNote) || null,
        progress: num(row.progress),
        revision: num(row.revision),
        size: num(row.size),
        sizeText: downloadable ? bytesText(row.size) : num(row.estimatedSize) ? "~" + bytesText(row.estimatedSize) : "",
        builtDate: millisOf(row.builtDate),
        builtText: dateText(row.builtDate),
        downloadUrl: downloadable
            ? PACKAGE_PATH + "?packageId=" + encodeURIComponent(row.packageId) + "&revision=" + encodeURIComponent(num(row.revision))
            : null,
        tag: downloadable ? "sha256-" + text(row.sha256) : null,
        busy,
        canUpdate: !busy,
        canCancel: busy,
        canDelete: !busy
    }
}
function findPackages() {
    if (!text(ec.user.userId)) throw new Error(L("Sign in to read your offline maps"))
    const rows = Array.from(find(PACKAGE, {ownerUserId: ec.user.userId}).orderBy("-createdDate").limit(MAX_PACKAGES).list())
    return {packageList: rows.map(packageEntry)}
}

// ===== build#Package: the MapPackageBuild job's run, as the owner =====
const writeText = (location, value) => quietly(() => ec.resource.getLocationReference(location).putText(value))
// {size, sha256} of the file at a location, or null when it cannot be read
function fileFacts(location) {
    try {
        return quietly(() => {
            const ref = ec.resource.getLocationReference(location)
            if (!ref.getExists()) return null
            return {size: Number(ref.getSize()), sha256: text(ec.resource.digest(location, "sha256"))}
        })
    } catch (err) {
        ec.logger.warn("Map package member " + location + " not read: " + (err?.message ?? err))
        return null
    }
}
const libsLocation = path => (text(path).startsWith(LIBS_PATH) ? LIBS_LOCATION + text(path).slice(LIBS_PATH.length) : null)

// The style of a basemap made package-relative (its sprite and the fonts' template), with its sprites and the fonts the
// package holds: added to members, a failure named
function addStyle(entry, build) {
    const location = libsLocation(entry.styleUrl)
    if (!location) {
        if (text(entry.styleUrl)) build.omitted.push({id: entry.id, kind: "style", reason: L("its style is not on this server")})
        return null
    }
    const flavor = safeName(text(entry.styleUrl).split("/").pop().replace(/\.json$/, ""))
    const path = "style/" + flavor + ".json"
    if (build.members.some(member => member.path === path)) return path
    let style
    try {
        style = JSON.parse(quietly(() => ec.resource.getLocationText(location)))
    } catch (err) {
        build.failures.push({path, error: L("the style could not be read")})
        ec.logger.error("Map package " + build.packageId + ": the style " + location + " could not be read: " + (err?.message ?? err))
        return null
    }
    const sprite = libsLocation(style.sprite)
    if (sprite) {
        const name = safeName(text(style.sprite).split("/").pop())
        for (const suffix of SPRITE_SUFFIXES) build.addFile("sprites/" + name + suffix, sprite + suffix)
        style.sprite = "sprites/" + name
    } else if (style.sprite) delete style.sprite
    style.metadata = Object.assign({}, style.metadata || {}, {"ol:webfonts": FONT_TEMPLATE})
    build.addFonts()
    build.addText(path, JSON.stringify(style))
    return path
}

// The postal codes whose representative point is in the box: code, display code, country, place names with their admin
// names, point and precision, with the data sets' licenses
function postalRows(box) {
    const cf = ec.entity.conditionFactory
    const spans = box.west > box.east ? [[box.west, 180], [-180, box.east]] : [[box.west, box.east]]
    const inLon = spans
        .map(([from, to]) =>
            cf.makeCondition(cf.makeCondition("longitude", "greater-equals", from), "and", cf.makeCondition("longitude", "less-equals", to))
        )
        .reduce((all, each) => (all ? cf.makeCondition(all, "or", each) : each), null)
    const points = Array.from(
        ec.entity
            .find("moqui.basic.GeoPoint")
            .condition("latitude", "greater-equals", box.south)
            .condition("latitude", "less-equals", box.north)
            .condition(inLon)
            .condition(cf.makeCondition("geoPointTypeEnumId", "not-equals", "GPT_RECORD_LOCATION", true))
            .selectField("geoPointId,latitude,longitude")
            .limit(MAX_POSTAL * 4)
            .disableAuthz()
            .list()
    )
    // the points of the box cut at their limit: some codes may be left out
    const pointsCut = points.length >= MAX_POSTAL * 4
    const pointOf = new Map(points.map(point => [point.geoPointId, point]))
    const best = new Map()
    const ids = Array.from(pointOf.keys())
    for (let at = 0; at < ids.length; at += 500) {
        const assocs = Array.from(
            ec.entity
                .find("moqui.basic.GeoPointAssoc")
                .condition("geoPointId", "in", ids.slice(at, at + 500))
                .condition("pointRoleEnumId", "GPR_REPRESENTATIVE")
                .condition(cf.makeCondition("precisionEnumId", "not-equals", "GPP_USER_CONFIRMED", true))
                .conditionDate("fromDate", "thruDate", null)
                .disableAuthz()
                .list()
        )
        const rank = assoc => (assoc.priority == null ? Number.MAX_SAFE_INTEGER : Number(assoc.priority))
        for (const assoc of assocs) {
            const known = best.get(assoc.geoId)
            if (!known || rank(assoc) < rank(known)) best.set(assoc.geoId, assoc)
        }
    }
    const geoIds = Array.from(best.keys())
    const rows = []
    const dataSets = new Set()
    for (let at = 0; at < geoIds.length && rows.length < MAX_POSTAL; at += 500) {
        const batch = geoIds.slice(at, at + 500)
        const codes = Array.from(
            ec.entity
                .find("moqui.basic.GeoPostalCode")
                .condition("geoId", "in", batch)
                .condition(cf.makeCondition("thruDate", "greater-than", ec.user.nowTimestamp, true))
                .disableAuthz()
                .list()
        )
        const places = Array.from(
            ec.entity
                .find("moqui.basic.GeoPostalCodePlace")
                .condition("geoId", "in", codes.map(code => code.geoId))
                .condition(cf.makeCondition("thruDate", "greater-than", ec.user.nowTimestamp, true))
                .orderBy("sequenceNum,placeName")
                .disableAuthz()
                .list()
        )
        for (const code of codes) {
            if (rows.length >= MAX_POSTAL) break
            const assoc = best.get(code.geoId)
            const point = pointOf.get(assoc.geoPointId)
            if (text(assoc.dataSetId)) dataSets.add(text(assoc.dataSetId))
            rows.push({
                code: text(code.normalizedCode),
                displayCode: text(code.displayCode) || text(code.normalizedCode),
                country: text(code.countryGeoId),
                places: places
                    .filter(place => place.geoId === code.geoId)
                    .slice(0, 20)
                    .map(place => ({name: text(place.placeName), adminName1: text(place.adminName1) || null, adminName2: text(place.adminName2) || null})),
                lon: num(point.longitude),
                lat: num(point.latitude),
                precision: text(assoc.precisionEnumId) || null
            })
        }
    }
    const licenses = Array.from(dataSets).map(dataSetId => {
        const set = one("moqui.maps.GeoDataSet", {dataSetId})
        return {dataSetId, license: text(set?.licenseName) || null, licenseUrl: text(set?.licenseUrl) || null, attribution: text(set?.attribution) || null}
    })
    return {rows, licenses, truncated: pointsCut || rows.length >= MAX_POSTAL}
}

// The rows of the active Overture imports of a theme whose point is in the box, with each import's licenses
function overtureRows(themeId, entity, fieldNames, box) {
    const regions = Array.from(find("moqui.maps.OvertureRegionTheme", {themeId}).list()).filter(row => text(row.activeImportId))
    const rows = []
    const imports = []
    const spans = box.west > box.east ? [[box.west, 180], [-180, box.east]] : [[box.west, box.east]]
    for (const region of regions) {
        const run = one("moqui.maps.OvertureImport", {importId: region.activeImportId})
        if (!run) continue
        let licenses = []
        try {
            licenses = JSON.parse(text(run.licensesJson) || "[]")
        } catch (err) {
            licenses = []
        }
        imports.push({importId: text(run.importId), regionId: text(region.regionId), releaseVersion: text(run.releaseVersion) || null, licenses})
        for (const [from, to] of spans) {
            if (rows.length >= MAX_OVERTURE) break
            const found = ec.entity
                .find(entity)
                .condition("importId", run.importId)
                .condition("latitude", "greater-equals", box.south)
                .condition("latitude", "less-equals", box.north)
                .condition("longitude", "greater-equals", from)
                .condition("longitude", "less-equals", to)
                .selectField(fieldNames.join(","))
                .limit(MAX_OVERTURE - rows.length)
                .disableAuthz()
                .list()
            for (const row of Array.from(found)) {
                const out = {}
                for (const name of fieldNames) out[name] = typeof row[name]?.toNumber === "function" ? row[name].toNumber() : row[name] ?? null
                rows.push(out)
            }
        }
    }
    return {rows, imports, truncated: rows.length >= MAX_OVERTURE}
}
const PLACE_FIELDS = ["gersId", "placeName", "basicCategory", "taxonomyPrimary", "latitude", "longitude", "confidence", "operatingStatus",
    "brandName", "addressText", "locality", "region", "postcode", "countryCode", "sourceDatasets", "sourceLicenses"]
const ADDRESS_FIELDS = ["gersId", "countryCode", "streetName", "streetNorm", "houseNumber", "numberNorm", "unitText", "unitNorm", "postcode",
    "postcodeNorm", "postalCity", "locality", "region", "levelsText", "latitude", "longitude", "positionKind", "sourceDatasets", "sourceLicenses"]

// The configuration a map draws, frozen for the package: the packaged basemaps with their package-relative archive and
// style, the outline, the user's overlays and places from the package; nothing of this server or any other host
function frozenConfig(config, build) {
    const frozen = JSON.parse(JSON.stringify(config || {}))
    for (const key of ["recordLayers", "features", "points", "record", "featuresTruncated", "fallbacks", "layerErrors"]) delete frozen[key]
    const profile = frozen.profile || {}
    profile.basemaps = build.basemaps
    if (!build.basemaps.some(entry => entry.id === profile.basemap)) profile.basemap = build.basemaps[0]?.id || null
    profile.allowedOrigins = []
    if (profile.notice) profile.notice = {text: text(profile.notice.text), href: "", linkText: ""}
    frozen.profile = profile
    frozen.layers = build.layers
    frozen.externalRequests = false
    // the map opens on the package's region
    delete frozen.center
    delete frozen.zoom
    frozen.extent = [build.box.west, build.box.south, build.box.east, build.box.north]
    return frozen
}

// The files of a build: {packageId}-{revision}-{buildKey}, a folder while it is built and its zip
function buildFilesOf(row, buildKey) {
    const folder = ROOT + "/" + safeName(row.packageId) + "-" + ((num(row.revision) || 0) + 1) + "-" + safeName(buildKey)
    return {folder, zip: folder + ".zip"}
}

// A build that could not run, or that threw, ended failed: its files removed, and the package written only while it holds
// the build's key, its owner told; then the next waiting build has its turn
function failBuild(packageId, buildKey, note) {
    let current = null
    try {
        ec.transaction.runRequireNew(60, "Error writing a map package's outcome", () => {
            current = find(PACKAGE, {packageId}).forUpdate(true).one()
            if (!current || text(current.buildKey) !== buildKey) {
                current = null
                return
            }
            const files = buildFilesOf(current, buildKey)
            removeTree(files.zip)
            removeTree(files.folder)
            current.setAll({statusId: "MapPkgFailed", statusNote: note, cancelRequested: "N", buildKey: null, jobRunId: null})
            writeRow(current, false)
        })
        if (current) notifyOwner(current, "MapPkgFailed", note)
        ec.transaction.runRequireNew(60, "Error starting the next map package build", scheduleBuilds)
    } catch (err) {
        ec.logger.error("Map package " + packageId + ": its failed build was not ended: " + (err?.message ?? err))
    }
    return {name: text(current?.name) || null, statusId: current ? "MapPkgFailed" : null, reason: note}
}

// A build whose owner is locked out for now waits again, pending with no run, the scheduler passing it by until the lock
// ends; the next waiting build has its turn
function waitBuild(packageId, buildKey) {
    let current = null
    try {
        ec.transaction.runRequireNew(60, "Error writing a map package's state", () => {
            current = find(PACKAGE, {packageId}).forUpdate(true).one()
            if (!current || text(current.buildKey) !== buildKey) {
                current = null
                return
            }
            current.setAll({statusId: "MapPkgPending", jobRunId: null,
                statusNote: L("Waiting: its owner's account is locked for a few minutes after failed sign-ins")})
            writeRow(current, false)
        })
        ec.transaction.runRequireNew(60, "Error starting the next map package build", scheduleBuilds)
    } catch (err) {
        ec.logger.error("Map package " + packageId + ": its build was not put back to wait: " + (err?.message ?? err))
    }
    return {name: text(current?.name) || null, statusId: current ? "MapPkgPending" : null, reason: null}
}

// The run as the package's owner. A run is started by whichever request, build's end or clean found the installation
// room for it (scheduleBuilds), so it signs in as the owner, whose build key the row holds (their own create or update
// asked for this build), and reads and cuts only what the owner may see. A job's run has a context of its own, so it
// stays signed in as the owner to its end, and a framework notice of the job goes to the owner, not the starter; a call
// in a web request never changes who is signed in, and is refused for another user's package. A run whose key the
// package does not hold does nothing; one that cannot sign in as the owner (a disabled account) or that throws ends the
// package failed, so no run holds the queue; the run itself never ends in error.
function buildPackage(p) {
    const packageId = text(p.packageId)
    const row = packageId ? one(PACKAGE, {packageId}) : null
    const buildKey = text(p.buildKey)
    if (!row || !buildKey || text(row.buildKey) !== buildKey)
        return {name: text(row?.name) || null, statusId: null, reason: L("The offline map is built by another run")}
    if (text(ec.user.userId) !== text(row.ownerUserId)) {
        if (ec.web?.request) return {name: text(row.name), statusId: null, reason: L("The offline map is not available")}
        const account = one("moqui.security.UserAccount", {userId: row.ownerUserId})
        let signedIn = false
        try {
            signedIn = !!account && !!quietly(() => ec.user.internalLoginUser(text(account.username)))
        } catch (err) {
            signedIn = false
        }
        ec.message.clearErrors()
        if (!signedIn || text(ec.user.userId) !== text(row.ownerUserId)) {
            // locked for a few minutes after failed logins (someone else's, it may be): the build waits its turn again
            if (!accountClosed(account) && account.requirePasswordChange !== "Y" && accountLocked(account))
                return waitBuild(packageId, buildKey)
            ec.logger.error("Map package " + packageId + ": its owner could not be signed in for its build")
            return failBuild(packageId, buildKey, L("The offline map's owner could not be signed in for its build"))
        }
    }
    try {
        return buildAsOwner(p)
    } catch (err) {
        ec.logger.error("Map package " + packageId + ": its build failed: " + (err?.stack ?? err?.message ?? err))
        ec.message.clearErrors()
        return failBuild(packageId, buildKey, L("The build failed"))
    }
}

// Tell the owner how their build ended: only them (directOnly), the package's name and outcome
function notifyOwner(row, statusId, note) {
    try {
        const outcome =
            statusId === "MapPkgReady" ? L("ready to download") : statusId === "MapPkgCancelled" ? L("cancelled") : L("not built")
        ec.makeNotificationMessage()
            .topic(BUILD_JOB)
            .userId(row.ownerUserId)
            .directOnly(true)
            .message({packageId: String(row.packageId), name: text(row.name), statusId, outcome, reason: note || null})
            .title("Offline map ${name}: ${outcome}")
            .link("/qapps/maps/Offline")
            .type(statusId === "MapPkgReady" ? "success" : statusId === "MapPkgCancelled" ? "info" : "warning")
            .send()
    } catch (err) {
        ec.logger.warn("Map package " + row.packageId + ": its owner was not notified: " + (err?.message ?? err))
    }
}

function buildAsOwner(p) {
    const packageId = text(p.packageId)
    const row = packageId ? one(PACKAGE, {packageId}) : null
    if (!row || !text(ec.user.userId) || row.ownerUserId !== ec.user.userId)
        return {statusId: null, reason: L("The offline map is not available")}
    const name = text(row.name)
    // this run's build: the key it was started with is still the package's, or another run (an Update after this one was
    // ended as stale) holds it and this one writes nothing
    const buildKey = text(p.buildKey)
    if (!buildKey || text(row.buildKey) !== buildKey)
        return {name, statusId: text(row.statusId) || null, reason: L("The offline map is built by another run")}
    if (!BUSY.includes(text(row.statusId))) return {name, statusId: text(row.statusId) || null}
    const next = (num(row.revision) || 0) + 1
    // the build's own files, named by its key: two runs never share one
    const {folder, zip} = buildFilesOf(row, buildKey)
    const limit = maxBytes()
    // The outcome on the row as it is now (the extract wrote its progress, a cancel may have been asked), in a transaction
    // of its own, only while the row is still this build's; the build's files removed unless it is ready and written.
    // Then the next waiting build has its turn
    const end = (statusId, note, fields) => {
        if (statusId !== "MapPkgReady") removeTree(zip)
        removeTree(folder)
        let current = null
        ec.transaction.runRequireNew(60, "Error writing a map package's outcome", () => {
            current = find(PACKAGE, {packageId}).forUpdate(true).one()
            if (!current || text(current.buildKey) !== buildKey) {
                current = null
                return
            }
            current.setAll(
                Object.assign({statusId, statusNote: note || null, cancelRequested: "N", buildKey: null, jobRunId: null}, fields || {})
            )
            writeRow(current, false)
        })
        if (!current) removeTree(zip)
        if (statusId !== "MapPkgReady") ec.logger.warn("Map package " + packageId + " not built: " + (note || statusId))
        if (current) notifyOwner(row, statusId, note)
        ec.transaction.runRequireNew(60, "Error starting the next map package build", scheduleBuilds)
        return {name, statusId: current ? statusId : null, reason: current ? note || null : L("The offline map is built by another run")}
    }
    const fresh = () => find(PACKAGE, {packageId}).one()
    // a cancel asked, or the package no longer this build's
    const cancelled = () => {
        const now = fresh()
        return !now || now.cancelRequested === "Y" || text(now.buildKey) !== buildKey
    }
    if (row.cancelRequested === "Y") return end("MapPkgCancelled", L("Cancelled"))
    ec.transaction.runRequireNew(60, "Error writing a map package's state", () => {
        const current = find(PACKAGE, {packageId}).forUpdate(true).one()
        if (current && text(current.buildKey) === buildKey) {
            current.setAll({statusId: "MapPkgBuilding", progress: 0, statusNote: null, progressDate: ec.user.nowTimestamp})
            writeRow(current, false)
        }
    })
    const fields = row.getMap()
    const box = boxOf(fields)
    let recorded = {}
    try {
        recorded = JSON.parse(text(row.sourceRevisions) || "{}")
    } catch (err) {
        recorded = {}
    }
    // an archive registered again since the build was asked is not what was estimated: the owner updates it again
    const replaced = () =>
        Object.keys(recorded).find(id => {
            const now = id.startsWith("overlay:")
                ? text(one("moqui.maps.MapOverlay", {overlayId: id.slice("overlay:".length)})?.tilesRevision)
                : text(one(LAYER, {layerId: id})?.sourceRevision)
            return now !== text(recorded[id])
        })
    const changed = L("A map archive was registered again since the package was asked for: update it to build it again")
    if (replaced()) return end("MapPkgFailed", changed)
    const config = effectiveConfig(row.profileId)
    if (!config) return end("MapPkgFailed", L("Your map configuration could not be read"))
    const {archives, omitted} = basemapArchives(config, fields)
    // an archive asked for that is no longer what was registered (a file on disk changed in place) fails the build; one
    // the owner may no longer read is left out, as it would be from their map
    const cut = new Set(archives.flatMap(archive => archive.layerIds))
    for (const id of Object.keys(recorded).filter(id => !id.startsWith("overlay:") && !cut.has(id)))
        if (text(sources().packageSource(id).reasonCode) === "SOURCE_CHANGED") return end("MapPkgFailed", changed)
    const build = {
        packageId,
        box,
        members: [],
        failures: [],
        omitted,
        basemaps: [],
        layers: [],
        sources: [],
        fontsAdded: false,
        // a member read from a location as it is
        addFile(path, location) {
            if (this.members.some(member => member.path === path)) return true
            const facts = fileFacts(location)
            if (!facts) {
                this.failures.push({path, error: L("not read")})
                ec.logger.error("Map package " + packageId + ": " + path + " (" + location + ") could not be read")
                return false
            }
            this.members.push(Object.assign({path, location}, facts))
            return true
        },
        // a member written here as text
        addText(path, value) {
            const location = folder + "/" + path
            try {
                writeText(location, value)
            } catch (err) {
                this.failures.push({path, error: L("not written")})
                ec.logger.error("Map package " + packageId + ": " + path + " could not be written: " + (err?.message ?? err))
                return false
            }
            return this.addFile(path, location)
        },
        // the map label fonts: each face's CSS and the woff2 files it names
        addFonts() {
            if (this.fontsAdded) return
            this.fontsAdded = true
            const dir = LIBS_LOCATION + FONTS
            const faces = quietly(() => {
                const ref = ec.resource.getLocationReference(dir)
                return ref.getExists() ? Array.from(ref.getDirectoryEntries() || []).filter(child => /\.css$/.test(text(child.fileName))) : []
            })
            if (!faces.length) this.failures.push({path: "fonts/noto-sans/", error: L("the map fonts are not on this server")})
            for (const face of faces) {
                const css = quietly(() => text(ec.resource.getLocationText(face.location)))
                if (!this.addFile("fonts/noto-sans/" + face.fileName, face.location)) continue
                for (const match of css.matchAll(/url\(\.\/files\/([\w-]+\.woff2)\)/g))
                    this.addFile("fonts/noto-sans/files/" + match[1], dir + "/files/" + match[1])
            }
        }
    }
    // the archives take most of the time: their share of the progress by their estimated bytes
    const extractable = archives.map(archive => ({kind: "basemap", archive}))
    const own = overlaysOf(fields, box)
    for (const entry of own)
        if (entry.tilesStatus === "ready") extractable.push({kind: "overlay", entry, source: sources().packageOverlayArchive(entry.overlayId)})
    const share = extractable.length ? 80 / extractable.length : 0
    let progressAt = 0
    // the bytes the members so far take: each extract has what is left of maps@package-max-bytes
    const used = () => build.members.reduce((sum, member) => sum + (Number(member.size) || 0), 0)
    const tooLarge = () =>
        say("The offline map passed the ${limit} a package may be: choose a smaller region or fewer zooms", {limit: bytesText(limit)})
    const extract = (id, source, path) => {
        const errors = []
        const left = limit - used()
        if (left <= 0) return {stop: "TOO_LARGE", reason: tooLarge()}
        const out = probe(
            S + "build#PackageExtract",
            Object.assign(extractParameters(source, fields), {
                maxBytes: left,
                target: folder + "/" + path,
                packageId,
                buildKey,
                progressFrom: Math.round(progressAt),
                progressTo: Math.round(progressAt + share)
            }),
            errors
        )
        progressAt += share
        if (out && out.valid) {
            build.members.push({path, location: folder + "/" + path, size: Number(out.size), sha256: text(out.sha256)})
            return {ok: true, out}
        }
        const code = text(out?.reasonCode)
        if (code === "TOO_LARGE") return {stop: code, reason: tooLarge()}
        if (code === "CANCELLED" || code === "SOURCE_CHANGED") return {stop: code, reason: ""}
        build.failures.push({path, error: reasonOf(out)})
        ec.logger.error("Map package " + packageId + ": " + id + " could not be cut: " + (text(out?.reason) || errors[0] || ""))
        return {ok: false}
    }
    for (const item of extractable) {
        if (cancelled()) return end("MapPkgCancelled", L("Cancelled"))
        if (item.kind === "basemap") {
            const {archive} = item
            const path = "basemap/" + safeName(archive.layerIds[0]) + ".pmtiles"
            const made = extract(archive.layerIds[0], archive.source, path)
            if (made.stop === "CANCELLED") return end("MapPkgCancelled", L("Cancelled"))
            if (made.stop === "SOURCE_CHANGED") return end("MapPkgFailed", changed)
            if (made.stop) return end("MapPkgFailed", made.reason)
            if (!made.ok) continue
            build.sources.push({
                kind: "basemap",
                layerIds: archive.layerIds,
                member: path,
                revision: archive.source.revision,
                attribution: text(archive.entries[0].attribution) || null,
                // the notice of the data it was cut from (an OpenStreetMap extract is a produced work under the ODbL), as its archive says it
                sourceAttribution: text(made.out.attribution) || null,
                license: text(made.out.license) || null,
                minZoom: Number(made.out.minZoom),
                maxZoom: Number(made.out.maxZoom),
                tileCount: Number(made.out.tileCount)
            })
            for (const entry of archive.entries) {
                const basemap = {id: entry.id, label: entry.label, kind: "pmtiles", url: path, attribution: entry.attribution || "",
                    attributionUrls: Array.from(entry.attributionUrls || [])}
                if (entry.raster) basemap.raster = true
                const style = addStyle(entry, build)
                if (style) basemap.styleUrl = style
                basemap.minZoom = Number(made.out.minZoom)
                basemap.maxZoom = Number(made.out.maxZoom)
                build.basemaps.push(basemap)
            }
        } else {
            const {entry, source} = item
            const path = "overlays/" + safeName(entry.overlayId) + ".pmtiles"
            if (!source.ok) {
                build.omitted.push({id: "overlay:" + entry.overlayId, kind: "overlay", reason: text(source.reason)})
                continue
            }
            const made = extract("overlay:" + entry.overlayId, source, path)
            if (made.stop === "CANCELLED") return end("MapPkgCancelled", L("Cancelled"))
            if (made.stop === "SOURCE_CHANGED") return end("MapPkgFailed", changed)
            if (made.stop) return end("MapPkgFailed", made.reason)
            if (made.ok) build.layers.push({id: "overlay:" + entry.overlayId, label: entry.label, kind: "pmtiles", url: path, visible: true})
        }
    }
    if (cancelled()) return end("MapPkgCancelled", L("Cancelled"))
    // a package with no basemap cut is no offline map
    if (!build.basemaps.length) return end("MapPkgFailed", L("No basemap archive could be cut for the region"))
    // the overlays drawn whole and the places, as the overlay transition gives them to the owner now
    for (const entry of own) {
        if (entry.tilesStatus === "ready") continue
        const content = overlays().execute("overlayContent", {overlayId: entry.overlayId})
        const path = entry.isPlaces && entry.isOwn ? "places.geojson" : "overlays/" + safeName(entry.overlayId) + ".geojson"
        if (!content || !content.ok) {
            build.failures.push({path, error: L("the overlay could not be read")})
            ec.logger.warn("Map package " + packageId + ": " + path + " not read: " + text(content?.reason))
            continue
        }
        if (build.addText(path, String(content.geojson)))
            build.layers.push({id: entry.isPlaces && entry.isOwn ? "places" : "overlay:" + entry.overlayId, label: entry.label, kind: "geojson",
                url: path, visible: true})
    }
    // the outline, for the zooms below the extract
    if (build.addFile("outline.json", libsLocation(OUTLINE)))
        build.basemaps.push({id: "OUTLINE_NE110", label: L("Country outlines"), kind: "outline", url: "outline.json", attribution: "Natural Earth",
            attributionUrls: ["https://www.naturalearthdata.com/"]})
    if (fields.includePostal === "Y") {
        try {
            const postal = postalRows(box)
            if (build.addText("postal.json", JSON.stringify({rows: postal.rows, truncated: postal.truncated})))
                build.sources.push({kind: "postal", member: "postal.json", rows: postal.rows.length, truncated: postal.truncated, licenses: postal.licenses})
        } catch (err) {
            build.failures.push({path: "postal.json", error: L("not read")})
            ec.logger.error("Map package " + packageId + ": the postal codes could not be read: " + (err?.message ?? err))
        }
    }
    if (fields.includeOverture === "Y")
        for (const [themeId, entity, names, path] of [
            ["PLACES", "moqui.maps.OverturePlace", PLACE_FIELDS, "overture-places.json"],
            ["ADDRESSES", "moqui.maps.OvertureAddress", ADDRESS_FIELDS, "overture-addresses.json"]
        ]) {
            try {
                const found = overtureRows(themeId, entity, names, box)
                if (!found.imports.length) continue
                if (build.addText(path, JSON.stringify({rows: found.rows, truncated: found.truncated})))
                    build.sources.push({kind: "overture", theme: themeId, member: path, rows: found.rows.length, truncated: found.truncated,
                        imports: found.imports})
            } catch (err) {
                build.failures.push({path, error: L("not read")})
                ec.logger.error("Map package " + packageId + ": " + path + " could not be read: " + (err?.message ?? err))
            }
        }
    if (cancelled()) return end("MapPkgCancelled", L("Cancelled"))
    // the archives cut as they were asked for: one registered again while it was cut fails the build
    if (replaced()) return end("MapPkgFailed", changed)
    if (used() > limit) return end("MapPkgFailed", tooLarge())
    const builtDate = ec.user.nowTimestamp
    const manifest = {
        schemaVersion: SCHEMA_VERSION,
        package: {
            packageId,
            name,
            bbox: [box.west, box.south, box.east, box.north],
            minZoom: num(fields.minZoom),
            maxZoom: num(fields.maxZoom),
            revision: next,
            builtDate: new Date(millisOf(builtDate)).toISOString()
        },
        configuration: frozenConfig(config, build),
        sources: build.sources,
        members: build.members.map(member => ({path: member.path, size: member.size, sha256: member.sha256})),
        omitted: build.omitted,
        failures: build.failures,
        notice: L(
            "This package holds what its owner could read when it was built, and stays readable on a device until it is deleted there. Record layers and a provider's remote tiles are not packaged."
        )
    }
    // the build's word between its members and its zip: a long lookup member is no silence
    ec.transaction.runRequireNew(60, "Error writing a map package's progress", () => {
        const current = find(PACKAGE, {packageId}).forUpdate(true).one()
        if (current && text(current.buildKey) === buildKey) {
            current.set("progressDate", ec.user.nowTimestamp)
            writeRow(current, false)
        }
    })
    const manifestLocation = folder + "/manifest.json"
    try {
        writeText(manifestLocation, JSON.stringify(manifest))
    } catch (err) {
        return end("MapPkgFailed", L("The package's manifest could not be written"))
    }
    let zipped
    try {
        zipped = quietly(() =>
            ec.resource.writeZip(
                zip,
                [{path: "manifest.json", location: manifestLocation, stored: true}].concat(
                    build.members.map(member => ({path: member.path, location: member.location, stored: true}))
                )
            )
        )
    } catch (err) {
        ec.logger.error("Map package " + packageId + ": the zip could not be written: " + (err?.message ?? err))
        return end("MapPkgFailed", L("The package file could not be written"))
    }
    const zipFailures = Array.from(zipped?.failures || [])
    if (zipFailures.length) {
        ec.logger.error("Map package " + packageId + ": members left out of the zip: " + zipFailures.map(each => each.path).join(", "))
        return end("MapPkgFailed", L("The package file could not be written whole"))
    }
    const facts = fileFacts(zip)
    if (!facts) return end("MapPkgFailed", L("The package file could not be read back"))
    if (facts.size > limit) return end("MapPkgFailed", tooLarge())
    if (cancelled()) return end("MapPkgCancelled", L("Cancelled"))
    const previous = text(row.location)
    const done = end(
        "MapPkgReady",
        say("${members} files, ${size}", {members: build.members.length + 1, size: bytesText(facts.size)}) +
            (build.failures.length ? " · " + say("${count} left out", {count: build.failures.length}) : ""),
        {progress: 100, revision: next, location: zip, size: facts.size, sha256: facts.sha256, builtDate}
    )
    // the old revision's file goes once the new one is ready
    if (done.statusId && previous && previous !== zip && fresh()?.location === zip) removeTree(previous)
    return done
}

// ===== get#PackageAccess: the package transition, every request =====
function packageAccess(p) {
    const missing = {ok: false, status: 404, reasonCode: "PACKAGE_MISSING", reason: L("The offline map is not available")}
    const row = ownPackage(p.packageId)
    if (!row || !text(row.location) || !text(row.sha256) || row.revision == null) return missing
    if (text(p.revision) !== String(num(row.revision)))
        return {ok: false, status: 409, reasonCode: "REVISION_CHANGED", reason: L("The offline map was built again: download it again")}
    const facts = quietly(() => {
        const ref = ec.resource.getLocationReference(text(row.location))
        return ref.getExists() ? Number(ref.getSize()) : null
    })
    if (facts === null || facts !== num(row.size)) return missing
    if (text(p.method).toUpperCase() !== "HEAD") recordDownload(row)
    return {
        ok: true,
        status: 200,
        location: text(row.location),
        tag: "sha256-" + text(row.sha256),
        size: num(row.size),
        fileName: safeName(text(row.name).replace(/\s+/g, "-")) + "-" + num(row.revision) + ".zip"
    }
}
// One row a package, revision and device, its last request touched at most once an hour (and the package's last
// download with it); a write that fails never takes the download with it
function recordDownload(row) {
    try {
        let loginKey = null
        let deviceName = null
        if (text(ec.user.loginKeyKind) === "device") {
            const devices = probe("org.moqui.impl.UserServices.get#UserDeviceKeys", {}, null, false)
            const current = Array.from(devices?.deviceList || []).find(device => device.current)
            if (current) {
                loginKey = text(current.loginKey) || null
                deviceName = text(current.deviceName) || null
            }
        }
        const now = ec.user.nowTimestamp
        const stale = value => millisOf(value) === null || Date.now() - millisOf(value) > DOWNLOAD_EVERY_MS
        ec.transaction.runRequireNew(30, "Error recording a map package download", () => {
            const conditions = {packageId: row.packageId, revision: num(row.revision), userId: ec.user.userId, userLoginKey: loginKey}
            const known = find(DOWNLOAD, conditions).one()
            if (!known) {
                const created = ec.entity.makeValue(DOWNLOAD)
                created.setSequencedIdPrimary()
                created.setAll(Object.assign({}, conditions, {deviceName, firstRequestDate: now, lastRequestDate: now}))
                writeRow(created, true)
            } else if (stale(known.lastRequestDate)) {
                known.set("lastRequestDate", now)
                writeRow(known, false)
            }
            if (stale(row.lastDownloadDate)) {
                const current = find(PACKAGE, {packageId: row.packageId}).one()
                if (current) {
                    current.set("lastDownloadDate", now)
                    writeRow(current, false)
                }
            }
        })
    } catch (err) {
        ec.logger.warn("Map package " + row.packageId + ": its download was not recorded: " + (err?.message ?? err))
    }
}

// ===== clean#MapPackages: the hourly clean =====
// The files of packages deleted, of builds left behind, and of packages not downloaded for maps@package-keep-days (from
// when they were built or last downloaded): each removed, a failure logged and the rest gone on with. removedCount: the
// files and folders removed.
function cleanPackages() {
    let removedCount = 0
    const cutoff = Date.now() - keepDays() * 86400000
    // the folder listed first, then the rows read: a build started between the two is in the rows, so keeps its files
    const entries = quietly(() => {
        const ref = ec.resource.getLocationReference(ROOT)
        return ref.getExists() && ref.isDirectory() ? Array.from(ref.getDirectoryEntries() || []) : []
    })
    const rows = Array.from(
        find(PACKAGE, {}).selectField("packageId,statusId,location,builtDate,lastDownloadDate,revision,buildKey").list()
    )
    const byId = new Map(rows.map(row => [String(row.packageId), row]))
    const named = new Set(rows.map(row => text(row.location)).filter(location => location))
    for (const row of rows) {
        if (BUSY.includes(text(row.statusId)) || !text(row.location)) continue
        const last = Math.max(millisOf(row.lastDownloadDate) ?? 0, millisOf(row.builtDate) ?? 0)
        if (!last || last >= cutoff) continue
        try {
            // under the row's lock, and only while it is still idle with that file: an Update meanwhile keeps it
            ec.transaction.runRequireNew(30, "Error writing an expired map package", () => {
                const current = find(PACKAGE, {packageId: row.packageId}).forUpdate(true).one()
                if (!current || BUSY.includes(text(current.statusId)) || text(current.location) !== text(row.location)) return
                removeTree(text(row.location))
                removedCount++
                current.setAll({location: null, size: null, sha256: null, statusId: "MapPkgExpired",
                    statusNote: say("Removed after ${days} days with no download: update it to build it again", {days: keepDays()})})
                writeRow(current, false)
            })
        } catch (err) {
            ec.logger.error("Map package " + row.packageId + " not expired: " + (err?.message ?? err))
        }
    }
    // what no package names: a deleted package's file, an old revision, a build that stopped (a busy package's own build
    // is left alone)
    for (const entry of entries) {
        // {packageId}-{revision}-{buildKey}, a folder or a zip
        const match = /^(.+)-(\d+)-([A-Za-z0-9]+)(\.zip)?$/.exec(text(entry.fileName))
        const row = match ? byId.get(match[1]) : null
        if (named.has(text(entry.location))) continue
        if (row && BUSY.includes(text(row.statusId)) && match[3] === text(row.buildKey)) continue
        try {
            removeTree(text(entry.location))
            removedCount++
        } catch (err) {
            ec.logger.error("Map package file " + entry.location + " not removed: " + (err?.message ?? err))
        }
    }
    // a waiting build whose turn was missed has it now
    ec.transaction.runRequireNew(60, "Error starting the next map package build", scheduleBuilds)
    return {removedCount}
}

const operations = {
    schedulePackageBuilds,
    estimatePackage,
    createPackage,
    updatePackage,
    cancelPackage,
    deletePackage,
    findPackages,
    buildPackage,
    packageAccess,
    cleanPackages
}
;({
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown map package operation " + op)
        return operations[op](parameters || {})
    }
})
