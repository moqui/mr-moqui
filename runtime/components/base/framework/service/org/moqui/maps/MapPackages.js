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
// (ec.resource.writeZip, every member stored as it is), at runtime://tmp/map-packages/{packageId}-{revision}.zip. A
// member that cannot be read is logged and named in the manifest's failures, and the rest goes on; an archive registered
// again before or while it is cut fails the build (its recorded revision is checked when the build starts and when the
// cuts end, and the cut checks its tag when it starts and ends). Record layers (an app's records) and a provider's remote
// tiles are never packaged: a record's access is read at each request, and a provider's terms allow a limited offline
// download, not a mirror. The run's progress and its cancel are on the row; a run that stopped with no word for an hour
// is ended when its owner next updates, cancels or deletes it. Update builds the package again as a new revision, and
// the old file is removed once the new one is ready.
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
// a run with no word for this long has stopped (its row's progress is written at least once a minute while it cuts)
const STALE_MS = 60 * 60 * 1000
const MAX_PACKAGES = 50
const MAX_ZOOM = 16
const DEFAULT_MAX_BYTES = 2147483648
const DEFAULT_KEEP_DAYS = 30
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
        if (entry.isPlaces && entry.isOwn) return fields.includePlaces === "Y"
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

// What the package would hold, from the archives' directories: {ok, bytes, tileCount, layers, omitted, revisions} or a
// refusal (TOO_LARGE past maps@package-max-bytes, with the estimate)
function estimateOf(fields) {
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
            omitted.push({id, kind: "archive", reason: text(out?.reason) || errors[0] || L("not read")})
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
        if (stop) return refused(text(stop.reasonCode), text(stop.reason))
        for (const id of archive.layerIds.slice(1)) revisions[id] = archive.source.revision
    }
    for (const entry of overlaysOf(fields, box)) {
        if (entry.tilesStatus === "ready") {
            const source = sources().packageOverlayArchive(entry.overlayId)
            if (!source.ok) {
                omitted.push({id: "overlay:" + entry.overlayId, kind: "overlay", reason: text(source.reason)})
                continue
            }
            const stop = estimateOne("overlay:" + entry.overlayId, entry.label, source)
            if (stop) return refused(text(stop.reasonCode), text(stop.reason))
        } else bytes += Math.max(1024, (Number(entry.featureCount) || 0) * 200)
    }
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
    return {ok: true, estimate, revisions}
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

// The package's run handed to a worker when this transaction commits; it runs as the user. The job's rows are no group's
// to read: it is started as the framework, for the owner checked here
function startBuild(row) {
    const jobRunId = quietly(() => ec.service.job(BUILD_JOB).parameters({packageId: String(row.packageId)}).run())
    if (jobRunId) {
        row.set("jobRunId", String(jobRunId))
        writeRow(row, false)
        return true
    }
    ec.message.clearErrors()
    row.setAll({statusId: "MapPkgFailed", statusNote: L("The build did not start")})
    writeRow(row, false)
    return false
}

function createPackage(p) {
    if (!text(ec.user.userId)) throw new Error(L("Sign in to make an offline map"))
    const request = requestOf(p)
    if (!request.ok) return request
    const count = Number(find(PACKAGE, {ownerUserId: ec.user.userId}).count())
    if (count >= MAX_PACKAGES)
        return refused("TOO_MANY_PACKAGES", say("You have ${count} offline maps: delete one first", {count}))
    const found = estimateOf(request.fields)
    if (!found.ok) return found
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
            createdDate: ec.user.nowTimestamp
        })
    )
    writeRow(row, true)
    const started = startBuild(row)
    return {applied: true, reasonCode: null, reason: null, packageId: String(row.packageId), started, estimate: found.estimate}
}

// The user's own package, or null
function ownPackage(packageId) {
    const id = text(packageId)
    const row = id ? one(PACKAGE, {packageId: id}) : null
    return row && text(ec.user.userId) && row.ownerUserId === ec.user.userId ? row : null
}
const notThere = () => refused("RECORD_UNAVAILABLE", L("The offline map is not available"))

// Whether a busy package's run has ended without saying so: no run recorded, its ServiceJobRun gone or ended, or no word
// from it (its row's last write) for STALE_MS
function runEnded(row) {
    const jobRunId = text(row.jobRunId)
    if (!jobRunId) return true
    const run = one("moqui.service.job.ServiceJobRun", {jobRunId})
    if (!run || run.endTime) return true
    const since = millisOf(row.lastUpdatedStamp) ?? millisOf(run.startTime)
    return since !== null && Date.now() - since > STALE_MS
}
// A busy package whose run has ended is ended here: failed, or cancelled when a cancel was asked
function endStale(row) {
    if (!BUSY.includes(text(row.statusId)) || !runEnded(row)) return false
    const cancelled = row.cancelRequested === "Y"
    row.setAll({
        statusId: cancelled ? "MapPkgCancelled" : "MapPkgFailed",
        statusNote: cancelled ? L("Cancelled") : L("The build stopped without finishing"),
        cancelRequested: "N"
    })
    writeRow(row, false)
    return true
}

function updatePackage(p) {
    if (!text(ec.user.userId)) throw new Error(L("Sign in to update an offline map"))
    const row = ownPackage(p.packageId)
    if (!row) return notThere()
    if (BUSY.includes(text(row.statusId)) && !endStale(row))
        return refused("PACKAGE_BUSY", L("The offline map is being built: wait for it to end, or cancel it"))
    const request = requestOf(row.getMap())
    if (!request.ok) return request
    const found = estimateOf(request.fields)
    if (!found.ok) return found
    row.setAll({
        statusId: "MapPkgPending",
        statusNote: null,
        progress: 0,
        cancelRequested: "N",
        estimatedSize: found.estimate.bytes,
        tileCount: found.estimate.tileCount,
        sourceRevisions: JSON.stringify(found.revisions),
        jobRunId: null
    })
    writeRow(row, false)
    const started = startBuild(row)
    return {applied: true, reasonCode: null, reason: null, packageId: String(row.packageId), started, estimate: found.estimate}
}

// ===== cancel#Package, delete#Package, find#Packages =====
function cancelPackage(p) {
    if (!text(ec.user.userId)) throw new Error(L("Sign in to cancel"))
    const row = ownPackage(p.packageId)
    if (!row || !BUSY.includes(text(row.statusId))) return refused("RECORD_UNAVAILABLE", L("The offline map is not being built"))
    row.set("cancelRequested", "Y")
    // a run that has ended without saying so is ended now; a live one stops at its next batch of tiles
    if (!endStale(row)) writeRow(row, false)
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
    const row = ownPackage(p.packageId)
    if (!row) return notThere()
    if (BUSY.includes(text(row.statusId)) && !endStale(row))
        return refused("PACKAGE_BUSY", L("The offline map is being built: cancel it first"))
    removeTree(text(row.location))
    quietly(() => {
        ec.entity.find(DOWNLOAD).condition("packageId", row.packageId).deleteAll()
        row.delete()
    })
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
        statusText: L(STATUS_TEXT[statusId] || statusId),
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
    return {rows, licenses, truncated: rows.length >= MAX_POSTAL}
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

function buildPackage(p) {
    const packageId = text(p.packageId)
    const row = packageId ? one(PACKAGE, {packageId}) : null
    if (!row || !text(ec.user.userId) || row.ownerUserId !== ec.user.userId)
        return {statusId: null, reason: L("The offline map is not available")}
    const name = text(row.name)
    if (!BUSY.includes(text(row.statusId))) return {name, statusId: text(row.statusId) || null}
    const next = (num(row.revision) || 0) + 1
    const folder = ROOT + "/" + safeName(packageId) + "-" + next
    const zip = folder + ".zip"
    // The outcome on the row as it is now (the extract wrote its progress, a cancel may have been asked), in a transaction
    // of its own, so a failure later in the job does not leave it building; the build's files removed unless it is ready
    const end = (statusId, note, fields) => {
        if (statusId !== "MapPkgReady") removeTree(zip)
        removeTree(folder)
        let current = null
        ec.transaction.runRequireNew(60, "Error writing a map package's outcome", () => {
            current = find(PACKAGE, {packageId}).forUpdate(true).one()
            if (!current) return
            current.setAll(Object.assign({statusId, statusNote: note || null, cancelRequested: "N"}, fields || {}))
            writeRow(current, false)
        })
        if (!current) removeTree(zip)
        if (statusId !== "MapPkgReady") ec.logger.warn("Map package " + packageId + " not built: " + (note || statusId))
        return {name, statusId, reason: note || null}
    }
    const fresh = () => find(PACKAGE, {packageId}).one()
    const cancelled = () => fresh()?.cancelRequested === "Y"
    if (row.cancelRequested === "Y") return end("MapPkgCancelled", L("Cancelled"))
    ec.transaction.runRequireNew(60, "Error writing a map package's state", () => {
        const current = find(PACKAGE, {packageId}).forUpdate(true).one()
        if (current) {
            current.setAll({statusId: "MapPkgBuilding", progress: 0})
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
    const extract = (id, source, path) => {
        const errors = []
        const out = probe(
            S + "build#PackageExtract",
            Object.assign(extractParameters(source, fields), {
                maxBytes: maxBytes(),
                target: folder + "/" + path,
                packageId,
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
        if (code === "CANCELLED" || code === "SOURCE_CHANGED" || code === "TOO_LARGE") return {stop: code, reason: text(out?.reason)}
        build.failures.push({path, error: text(out?.reason) || errors[0] || L("not cut")})
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
    // the overlays drawn whole and the places, as the overlay transition gives them to the owner now
    for (const entry of own) {
        if (entry.tilesStatus === "ready") continue
        const content = overlays().execute("overlayContent", {overlayId: entry.overlayId})
        const path = entry.isPlaces && entry.isOwn ? "places.geojson" : "overlays/" + safeName(entry.overlayId) + ".geojson"
        if (!content || !content.ok) {
            build.failures.push({path, error: text(content?.reason) || L("not read")})
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
    if (cancelled()) return end("MapPkgCancelled", L("Cancelled"))
    const previous = text(row.location)
    const done = end(
        "MapPkgReady",
        say("${members} files, ${size}", {members: build.members.length + 1, size: bytesText(facts.size)}) +
            (build.failures.length ? " · " + say("${count} left out", {count: build.failures.length}) : ""),
        {progress: 100, revision: next, location: zip, size: facts.size, sha256: facts.sha256, builtDate}
    )
    // the old revision's file goes once the new one is ready
    if (previous && previous !== zip && fresh()?.location === zip) removeTree(previous)
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
    const rows = Array.from(find(PACKAGE, {}).selectField("packageId,statusId,location,builtDate,lastDownloadDate,revision").list())
    const byId = new Map(rows.map(row => [String(row.packageId), row]))
    for (const row of rows) {
        if (BUSY.includes(text(row.statusId)) || !text(row.location)) continue
        const last = Math.max(millisOf(row.lastDownloadDate) ?? 0, millisOf(row.builtDate) ?? 0)
        if (!last || last >= cutoff) continue
        try {
            removeTree(text(row.location))
            removedCount++
            ec.transaction.runRequireNew(30, "Error writing an expired map package", () => {
                const current = find(PACKAGE, {packageId: row.packageId}).forUpdate(true).one()
                if (!current || BUSY.includes(text(current.statusId))) return
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
    const entries = quietly(() => {
        const ref = ec.resource.getLocationReference(ROOT)
        return ref.getExists() && ref.isDirectory() ? Array.from(ref.getDirectoryEntries() || []) : []
    })
    for (const entry of entries) {
        const match = /^(.+)-(\d+)(\.zip)?$/.exec(text(entry.fileName))
        const row = match ? byId.get(match[1]) : null
        if (row && text(row.location) === text(entry.location)) continue
        if (row && BUSY.includes(text(row.statusId)) && Number(match[2]) > (num(row.revision) || 0)) continue
        try {
            removeTree(text(entry.location))
            removedCount++
        } catch (err) {
            ec.logger.error("Map package file " + entry.location + " not removed: " + (err?.message ?? err))
        }
    }
    return {removedCount}
}

const operations = {
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
