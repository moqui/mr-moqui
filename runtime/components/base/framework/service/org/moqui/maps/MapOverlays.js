// Users' overlays and saved places of org.moqui.maps.MapServices (plans/Maps.md §4.1, §4.2, §8; docs/tools/Maps.md
// "Overlays and places"): a geographic file (GeoJSON, TopoJSON, EsriJSON, GPX, KML, GML, WKT, WKB, delimited text, a
// zipped shapefile) added as a personal overlay, the overlay transition's access check and content, the overlays a user
// owns or reads through a share, the saved places file, and the editing of one's own overlay (editOverlay, saveEdit).
//
// A file is the user's: an upload goes to Maps/overlays in their personal drive (ensure#PersonalDrive) as it was sent, and
// a file picked from a drive stays where it is. Adding one reads it first (MapSourceServices.convert#Overlay, in Rust:
// at most maps@overlay-max-bytes within maps@import-max-bytes, at most maps@max-features features (5000 when that is 0),
// WGS84 or a declared EPSG:3857, each feature's geometry checked by itself, labels and descriptions plain text, no network
// link, icon or other reference followed, and at most GEOJSON_MAX_CHARS as GeoJSON) and refuses a file that cannot be
// drawn before anything is written; it records what it found on a moqui.maps.MapOverlay row, one a user for a file. A shared overlay is the file shared through ResourceShare in Files: no other access model. A
// reader the file is shared with lists the owner's row read-only (find#Overlays), and the overlay transition
// (get#OverlayContent) checks the reader's access to the file in its drive on every request, writing nothing, and answers
// it as sanitized GeoJSON, cached by the file's version (a file grown too large is remembered so, a 413). The drive
// access answers are kept for one call only (levelAt's memo).
//
// A file too large to draw whole (past GEOJSON_MAX_CHARS as GeoJSON, or past maps@max-features) is refused with
// canGeneralize; added again with generalize it is kept and simplified on the server (buildTiles, the MapOverlayTiles
// ServiceJob, as the user: MapSourceServices.generalize#Overlay in Rust cuts it into vector tiles for each zoom), and the
// archive is written beside the file in its drive as "name.tiles.pmtiles" through upload#Resource (the drive's write
// check, quota and versions). The row records it (tilesStatus, tilesPath, tilesRevision...), and the map draws it from
// LocalMap's tile transition (MapSources.js archiveTile), which checks the reader's access to the overlay's file on every
// request, as the overlay transition does. Adding the file again simplifies it again; cancelTiles stops a run.
//
// Saved places are one GeoJSON file, Maps/places.geojson of the personal drive, written through FileServices.save#Text
// (the drive's own write check and versions), and listed as an overlay of the user's (isPlaces). Sharing that file (or the
// Maps folder) shares them, read-only. Browse's search reads the places of the user and those shared with them. A save
// locks the user's places row first, so two saves at once are one after the other; the file is read only up to
// maps@overlay-max-bytes.
const OVERLAY = "moqui.maps.MapOverlay"
const DRIVE = "moqui.resource.ResourceDrive"
const OVERLAY_FOLDER = "Maps/overlays"
const PLACES_PATH = "Maps/places.geojson"
const OVERLAY_PATH = "/apps/maps/Browse/overlay"
const CACHE = "maps.overlay.geojson"
// the GeoJSON an overlay may be served as: what reading it may give (refused when added, a 413 when its file grew since),
// and so the most one entry of the cache holds (maps.overlay.geojson: 16 entries, at most 64 MB in all)
const GEOJSON_MAX_CHARS = 4 * 1024 * 1024
// what the cache keeps for a file too large to serve, so it is not read again on each request
const TOO_LARGE = "\u0000TOO_LARGE"
// the features an overlay may have when maps@max-features is 0 (record layers off) or not a whole number
const DEFAULT_FEATURES = 5000
// the most features a file simplified on the server may have (MapSourceServices.generalize#Overlay's limit)
const TILE_FEATURES = 250000
const TILES_JOB = "MapOverlayTiles"
const TILE_SUFFIX = ".tiles.pmtiles"
const TILES_BUSY = ["pending", "running"]
// a run older than the MapOverlayTiles job's transaction (an hour), with a margin, has ended whatever its row says
const TILES_STALE_MS = 90 * 60 * 1000
const millisOf = value => {
    if (value == null || value === "") return null
    if (typeof value === "number") return value
    if (typeof value.getTime === "function") return value.getTime()
    const parsed = /^\d+$/.test(String(value)) ? Number(value) : Date.parse(String(value))
    return Number.isFinite(parsed) ? parsed : null
}
const MAX_OVERLAYS = 100
const MAX_SHARED = 100
const MAX_PLACES = 1000

const L = value => ec.l10n.localize(value)
const say = (value, values) => ec.resource.expand(L(value), "", values || {})
const text = value => (value == null ? "" : String(value).trim())
const yes = value => value === true || value === "true" || value === "Y"
const refused = (reasonCode, reason, extra) => Object.assign({applied: false, reasonCode, reason}, extra || {})
const conf = selector => text(ec.factory.getConfValue(selector))
// maps@overlay-max-bytes (10 MiB by default), within maps@import-max-bytes
const maxBytes = () => {
    const overlay = Number(conf("maps@overlay-max-bytes")) > 0 ? Number(conf("maps@overlay-max-bytes")) : 10 * 1024 * 1024
    const imports = Number(conf("maps@import-max-bytes")) > 0 ? Number(conf("maps@import-max-bytes")) : overlay
    return Math.min(overlay, imports)
}
// maps@max-features, or DEFAULT_FEATURES when it is 0 or not a whole number: turning record layers off does not turn
// overlays off
const maxFeatures = () => {
    const found = Number(conf("maps@max-features"))
    return Number.isInteger(found) && found > 0 ? found : DEFAULT_FEATURES
}
const num = value => {
    const found = value == null || value === "" ? NaN : Number(value)
    return Number.isFinite(found) ? found : null
}
const find = (entity, conditions) =>
    ec.entity
        .find(entity)
        .condition(conditions || {})
        .disableAuthz()
const one = (entity, conditions) => find(entity, conditions).one()
// A file's format by its extension, as moqui-lib geo/overlay's of_file_name reads it (.esri.json before .json; a shapefile
// is a .zip of it)
const FORMATS = [
    [/\.esri\.json$|\.esrijson$/, "MLF_ESRIJSON"],
    [/\.(geojson|json)$/, "MLF_GEOJSON"],
    [/\.topojson$/, "MLF_TOPOJSON"],
    [/\.gpx$/, "MLF_GPX"],
    [/\.kml$/, "MLF_KML"],
    [/\.gml$/, "MLF_GML"],
    [/\.wkt$/, "MLF_WKT"],
    [/\.wkb$/, "MLF_WKB"],
    [/\.(csv|tsv)$/, "MLF_CSV"],
    [/\.zip$/, "MLF_SHAPEFILE"]
]
const formatOf = name => {
    const lower = text(name).toLowerCase()
    const found = FORMATS.find(([pattern]) => pattern.test(lower))
    return found ? found[1] : null
}
const FORMATS_TEXT = "GeoJSON, TopoJSON, EsriJSON, GPX, KML, GML, WKT, WKB, CSV or a zipped shapefile"
// the file of an overlay edited on the map whose own file is not GeoJSON the map reads whole: written beside it
const EDITED_SUFFIX = ".edited.geojson"
// The version of a file a reader saw: its drive's version name, size and modification time (a drive on the server's disk
// keeps no version name); what an edit's save compares before it writes
const versionOf = change => (change ? [text(change.versionName), text(change.size), text(change.date)].join("|") : "")
const fileNameOf = path => text(path).split("/").pop()
// a label as one line of plain text
const labelText = (value, max) =>
    text(value)
        .replace(/[\u0000-\u001f\u007f​-‏‪-‮⁦-⁩]/g, " ")
        .replace(/\s+/g, " ")
        .slice(0, max || 100)
const cleanPath = value =>
    String(value ?? "")
        .split("/")
        .filter(part => part && part !== ".")
        .join("/")

// A call as the current user with its own errors kept apart: its result, or null when it failed. Authorization is off
// for the call itself: each service called checks the user (get#DriveAccess, save#Text, upload#Resource).
function probe(name, parameters, errorsOut) {
    let out = null
    ec.message.pushErrors()
    try {
        out = ec.service.sync().name(name).parameters(parameters).disableAuthz().call()
        if (ec.message.hasError()) {
            if (errorsOut) errorsOut.push(ec.message.getErrorsString())
            out = null
        }
    } catch (err) {
        ec.logger.warn("Maps overlays: " + name + " not answered: " + (err?.message ?? err))
        if (errorsOut) errorsOut.push(String(err?.message ?? err))
        out = null
    } finally {
        ec.message.clearErrors()
        ec.message.popErrors()
    }
    return out
}
// The current user's level in a drive at a path (0 none, 1 view, 2 edit, 3 manage), asked of get#DriveAccess with nothing
// written. memo (a Map made for the call, or none) keeps each answer for the call: the drive's own level first (an owner
// or a member reads every path of a drive that is not a records drive), and a path's only when that is none
function levelAt(driveId, path, memo) {
    const clean = cleanPath(path)
    if (!text(ec.user.userId) || !text(driveId) || !clean) return 0
    const known = memo || new Map()
    const driveKey = ec.user.userId + "|" + driveId
    if (!known.has(driveKey)) {
        const access = probe("org.moqui.resource.FileServices.get#DriveAccess", {driveId})
        const whole = access && access.drive && access.drive.driveKindEnumId !== "RdkRecords" ? Number(access.level) || 0 : 0
        known.set(driveKey, whole)
    }
    if (known.get(driveKey) >= 1) return known.get(driveKey)
    const pathKey = driveKey + "|" + clean
    if (!known.has(pathKey)) {
        const access = probe("org.moqui.resource.FileServices.get#DriveAccess", {driveId, path: clean})
        known.set(pathKey, access && access.drive ? Number(access.level) || 0 : 0)
    }
    return known.get(pathKey)
}
const driveRoot = driveId => {
    const drive = text(driveId) ? find(DRIVE, {driveId}).useCache(true).one() : null
    return drive ? text(drive.rootLocation).replace(/\/+$/, "") : null
}
const locationOf = row => {
    const root = driveRoot(row.driveId)
    return root ? root + "/" + cleanPath(row.path) : null
}
// A read of the resource facade as the framework: the caller was checked, and the storage's rows are no group's
function quietly(read) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return read()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
// convert#Overlay (Rust): {valid, reasonCode, reason, featureCount, rejectedCount, bounds, crs, transform, notes, geojson}
function convert(parameters) {
    const out = probe(
        "org.moqui.maps.MapSourceServices.convert#Overlay",
        Object.assign({maxBytes: maxBytes(), maxFeatures: maxFeatures()}, parameters)
    )
    return out || {valid: false, reasonCode: "NOT_READ", reason: L("The file could not be read")}
}
const ownerNames = new Map()
function ownerName(userId) {
    if (!ownerNames.has(userId)) {
        const account = find("moqui.security.UserAccount", {userId}).useCache(true).one()
        ownerNames.set(userId, account ? text(account.userFullName) || text(account.username) : userId)
    }
    return ownerNames.get(userId)
}
const personalDrive = () => text(probe("org.moqui.resource.FileServices.ensure#PersonalDrive", {})?.driveId) || null

// What a row records of a reading
function recordReading(row, reading) {
    const bounds = Array.isArray(reading.bounds) ? reading.bounds.map(num) : [null, null, null, null]
    row.set("featureCount", Number(reading.featureCount) || 0)
    row.set("rejectedCount", Number(reading.rejectedCount) || 0)
    row.set("minLongitude", bounds[0])
    row.set("minLatitude", bounds[1])
    row.set("maxLongitude", bounds[2])
    row.set("maxLatitude", bounds[3])
    row.set("sourceCrs", text(reading.crs) || "EPSG:4326")
    // the provenance of a converted CRS (P4c): what converting it to WGS84 did, empty for WGS84
    row.set("crsTransform", text(reading.transform).slice(0, 250) || null)
    row.set(
        "importNotes",
        Array.from(reading.notes || [])
            .map(text)
            .filter(note => note)
            .slice(0, 30)
            .join("\n") || null
    )
}
function writeRow(row, create) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        if (create) row.create()
        else row.update()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}

// ===== store#Overlay =====
// An upload (file) to Maps/overlays of the personal drive, or a file of a drive the user reads (driveId, path); read and
// refused before anything is written when it cannot be drawn.
function storeOverlay(p) {
    if (!text(ec.user.userId)) throw new Error(L("Sign in to add an overlay"))
    const userId = ec.user.userId
    const file = p.file
    let driveId = text(p.driveId)
    let path = cleanPath(p.path)
    let fileName = file ? text(file.fileName || file.name) : fileNameOf(path)
    const format = formatOf(fileName)
    if (!format)
        return refused(
            "FORMAT_UNKNOWN",
            /\.gpkg$/i.test(fileName)
                ? L("A GeoPackage is not read here: export its layer as GeoJSON or a zipped shapefile and add that")
                : say("An overlay is a ${formats} file", {formats: FORMATS_TEXT})
        )
    const limit = maxBytes()
    let reading
    if (file) {
        if (Number(file.size) > limit)
            return refused("TOO_LARGE", say("The file is larger than the ${limit} bytes an overlay may be", {limit}))
        reading = convert({bytes: file.getInputStream().readAllBytes(), format})
    } else {
        // a '..' or a '#' would reach another file
        const unsafe = String(p.path ?? "").includes("..") || path.includes("#")
        if (!driveId || !path || unsafe) return refused("NOT_FOUND", L("Choose a file to add"))
        if (levelAt(driveId, path) < 1) return refused("NOT_FOUND", L("File not found"))
        const root = driveRoot(driveId)
        if (!root) return refused("NOT_FOUND", L("File not found"))
        reading = quietly(() => convert({location: root + "/" + path, format}))
    }
    // too large to draw whole: offered simplifying on the server, and with generalize read again within its limits
    const tooMany = !reading.valid && text(reading.reasonCode) === "TOO_MANY_FEATURES"
    const tooLarge = reading.valid && String(reading.geojson || "").length > GEOJSON_MAX_CHARS
    const tiling = (tooMany || tooLarge) && yes(p.generalize)
    if (!reading.valid && !tooMany)
        return refused(text(reading.reasonCode) || "NOT_READ", text(reading.reason) || L("The file cannot be drawn"))
    if ((tooMany || tooLarge) && !tiling)
        return tooMany
            ? refused("TOO_MANY_FEATURES", text(reading.reason), {canGeneralize: true})
            : refused(
                  "TOO_LARGE_TO_DRAW",
                  say("The file has more than a map draws at once (${limit} characters as GeoJSON): simplify it or split it", {
                      limit: GEOJSON_MAX_CHARS
                  }),
                  {canGeneralize: true}
              )
    if (tiling) {
        const parameters = {summaryOnly: true, maxFeatures: TILE_FEATURES, format}
        if (file) parameters.bytes = file.getInputStream().readAllBytes()
        else parameters.location = driveRoot(driveId) + "/" + path
        reading = quietly(() => convert(parameters))
        if (!reading.valid)
            return refused(text(reading.reasonCode) || "NOT_READ", text(reading.reason) || L("The file cannot be drawn"))
        // the archive is written beside the file: a picked file the user may not write beside is refused here
        const folder = path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : ""
        if (!file && levelAt(driveId, folder || path) < 2)
            return refused("TILES_NOT_WRITABLE", L("The simplified copy is kept beside the file, and you may not add a file to its folder"))
    }
    if (file) {
        driveId = personalDrive()
        if (!driveId) return refused("NO_DRIVE", L("Your files are not available"))
        const errors = []
        const uploaded = probe(
            "org.moqui.resource.FileServices.upload#Resource",
            {
                driveId,
                path: OVERLAY_FOLDER,
                fileList: [file],
                conflictPolicy: "keepBoth",
                versionNote: L("Added as a map overlay")
            },
            errors
        )
        const written = Array.from(uploaded?.uploadedList || []).find(
            row => row.outcome !== "failed" && row.outcome !== "skipped"
        )
        if (!written) return refused("NOT_SAVED", errors[0] || L("The file was not saved to your files"))
        path = cleanPath(written.path)
        fileName = fileNameOf(path)
    }
    const existing = find(OVERLAY, {ownerUserId: userId, driveId, path}).forUpdate(true).one()
    // one run of the simplifying at a time for a file of the user's: two would write the same archive (a run that ended
    // without saying so is ended here first)
    if (existing && TILES_BUSY.includes(text(existing.tilesStatus)) && !endStale(existing))
        return refused("TILES_RUNNING", L("This file is being simplified: wait for it to end, or stop it under Layers"))
    if (!existing) {
        const count = Number(find(OVERLAY, {ownerUserId: userId}).count())
        if (count >= MAX_OVERLAYS)
            return refused("TOO_MANY_OVERLAYS", say("You have ${count} overlays: remove one first", {count}))
    }
    const row = existing || ec.entity.makeValue(OVERLAY)
    if (!existing) {
        row.setSequencedIdPrimary()
        row.setAll({ownerUserId: userId, driveId, path, createdDate: ec.user.nowTimestamp})
    }
    row.set("label", labelText(p.label) || labelText(fileName.replace(/\.[^.]*$/, "")) || fileName)
    row.set("formatEnumId", format)
    row.set("isPlaces", path === PLACES_PATH && driveId === personalDrive() ? "Y" : "N")
    recordReading(row, reading)
    // drawn whole, or simplified again: an earlier archive is not drawn until the new one is ready (its path is kept, so
    // the new archive replaces it)
    row.setAll({
        tilesStatus: tiling ? "pending" : null,
        tilesProgress: tiling ? 0 : null,
        tilesCancel: "N",
        tilesNote: null,
        tilesRevision: null,
        tilesVersionName: null,
        tilesTag: null,
        tilesSize: null,
        tilesMaxZoom: null,
        tilesModifiedDate: null,
        tilesJobRunId: null
    })
    writeRow(row, !existing)
    // handed to a worker when this transaction commits; it runs as the user. The job's rows are no group's to read: it is
    // started as the framework, for the caller checked above
    const started = () => {
        const wasDisabled = ec.artifactExecution.disableAuthz()
        try {
            return ec.service.job(TILES_JOB).parameters({overlayId: String(row.overlayId)}).run()
        } finally {
            if (!wasDisabled) ec.artifactExecution.enableAuthz()
        }
    }
    const jobRunId = tiling ? started() : null
    if (jobRunId) {
        row.set("tilesJobRunId", String(jobRunId))
        writeRow(row, false)
    } else if (tiling) {
        ec.message.clearErrors()
        row.setAll({tilesStatus: "failed", tilesNote: L("The simplifying job did not start")})
        writeRow(row, false)
    }
    return {
        applied: true,
        reasonCode: null,
        reason: null,
        overlayId: String(row.overlayId),
        featureCount: Number(reading.featureCount) || 0,
        rejectedCount: Number(reading.rejectedCount) || 0,
        notes: Array.from(reading.notes || []),
        generalizing: tiling
    }
}

// ===== build#OverlayTiles: the MapOverlayTiles job's run, as the user who asked =====
function buildTiles(p) {
    const overlayId = text(p.overlayId)
    const row = overlayId ? one(OVERLAY, {overlayId}) : null
    if (!row || !text(ec.user.userId) || row.ownerUserId !== ec.user.userId)
        return {tilesStatus: null, reason: L("The overlay is not available")}
    const label = text(row.label)
    if (!TILES_BUSY.includes(text(row.tilesStatus))) return {label, tilesStatus: text(row.tilesStatus) || null}
    // The outcome on the row as it is now (generalize#Overlay wrote its progress, a cancel may have been asked), in a
    // transaction of its own, so a failure later in the job does not leave the row running
    const end = (tilesStatus, note, fields) => {
        ec.transaction.runRequireNew(60, "Error writing a map overlay's tiles", () => {
            const current = find(OVERLAY, {overlayId}).forUpdate(true).one()
            if (!current) return
            current.setAll(Object.assign({tilesStatus, tilesNote: note || null, tilesCancel: "N"}, fields || {}))
            writeRow(current, false)
        })
        if (tilesStatus !== "ready") ec.logger.warn("Map overlay " + overlayId + " not simplified: " + (note || tilesStatus))
        return {label, tilesStatus, reason: note || null}
    }
    if (row.tilesCancel === "Y") return end("cancelled", L("Cancelled"))
    const root = driveRoot(row.driveId)
    const path = cleanPath(row.path)
    if (!root || levelAt(row.driveId, path) < 1) return end("failed", L("The overlay's file is not readable"))
    const errors = []
    const made = quietly(() =>
        probe(
            "org.moqui.maps.MapSourceServices.generalize#Overlay",
            {location: root + "/" + path, format: row.formatEnumId, overlayId, maxBytes: maxBytes()},
            errors
        )
    )
    if (!made || !made.valid)
        return end(made?.cancelled ? "cancelled" : "failed", text(made?.reason) || errors[0] || L("The file could not be simplified"))
    // the overlay removed, or its run ended or cancelled meanwhile: nothing is written
    const now = find(OVERLAY, {overlayId}).one()
    if (!now || now.ownerUserId !== ec.user.userId || !TILES_BUSY.includes(text(now.tilesStatus)))
        return {label, tilesStatus: now ? text(now.tilesStatus) || null : null, reason: L("The overlay changed while it was simplified")}
    if (now.tilesCancel === "Y") return end("cancelled", L("Cancelled"))
    // beside the file: name.tiles.pmtiles, replacing only this overlay's own earlier archive; any other file of that name
    // (another reader's archive of a shared file, a file of the owner's) is kept, and this one gets "name (2)"
    const folder = path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : ""
    const name = fileNameOf(path).replace(/\.[^.]*$/, "") + TILE_SUFFIX
    const own = cleanPath(row.tilesPath) === (folder ? folder + "/" : "") + name
    const uploaded = probe(
        "org.moqui.resource.FileServices.upload#Resource",
        {
            driveId: row.driveId,
            path: folder,
            fileList: [made.bytes],
            relativePathList: [name],
            conflictPolicy: own ? "replace" : "keepBoth",
            versionNote: L("A map overlay simplified for drawing")
        },
        errors
    )
    const written = Array.from(uploaded?.uploadedList || []).find(entry => entry.outcome !== "failed" && entry.outcome !== "skipped")
    if (!written) return end("failed", errors[0] || L("The simplified copy could not be written beside the file"))
    const tilesPath = cleanPath(written.path)
    const location = root + "/" + tilesPath
    const change = quietly(() => ec.resource.getLastChange(location))
    const versionName = text(change?.versionName)
    const pinned = versionName ? location + "#" + versionName : location
    let address = quietly(() => ec.resource.getContentAddress(pinned))
    let modified = null
    // a git object outside LFS is read whole for every tile: such a drive keeps no archive a map reads
    if (address && text(address.kind) === "git-object")
        return end(
            "failed",
            L("The drive keeps the simplified copy in git outside Git LFS, which a map cannot read by range"),
            {tilesPath}
        )
    if (!address) {
        // a drive on the server's disk: its SHA-256 is its revision, and its size and modification time tell a change
        const ref = quietly(() => ec.resource.getLocationReference(location))
        const before = ref ? Number(quietly(() => ref.getLastModified())) : NaN
        const id = quietly(() => ec.resource.digest(location, "sha256"))
        const size = ref ? Number(quietly(() => ref.getSize())) : NaN
        const after = ref ? Number(quietly(() => ref.getLastModified())) : NaN
        address = id && before === after && Number.isFinite(size) ? {id, tag: "sha256-" + id, size} : null
        modified = Number.isFinite(after) ? after : null
    }
    if (!address || !text(address.id)) return end("failed", L("The simplified copy could not be read back"), {tilesPath})
    const stopped = text(made.stoppedReason)
    return end(
        "ready",
        say("${features} features simplified into ${tiles} tiles, zooms 0 to ${zoom}", {
            features: Number(made.featureCount) || 0,
            tiles: Number(made.tileCount) || 0,
            zoom: Number(made.maxZoom) || 0
        }) + (stopped ? " (" + stopped + ")" : ""),
        {
            tilesProgress: 100,
            tilesPath,
            tilesVersionName: versionName || null,
            tilesRevision: text(address.id),
            tilesTag: text(address.tag),
            tilesSize: Number(address.size),
            tilesModifiedDate: modified,
            tilesMaxZoom: Number(made.maxZoom) || 0,
            featureCount: Number(made.featureCount) || row.featureCount
        }
    )
}

// Whether the run making an overlay's tiles has ended without saying so: no run recorded, its ServiceJobRun gone (a
// run that succeeded may be removed) or ended, or started longer ago than the job's transaction may last
function tilesRunEnded(row) {
    const jobRunId = text(row.tilesJobRunId)
    if (!jobRunId) return true
    const run = ec.entity.find("moqui.service.job.ServiceJobRun").condition({jobRunId}).disableAuthz().one()
    if (!run || run.endTime) return true
    const since = millisOf(run.startTime) ?? millisOf(run.lastUpdatedStamp)
    return since !== null && Date.now() - since > TILES_STALE_MS
}
// A busy row whose run has ended is ended here: failed, or cancelled when a cancel was asked
function endStale(row) {
    if (!TILES_BUSY.includes(text(row.tilesStatus)) || !tilesRunEnded(row)) return false
    const cancelled = row.tilesCancel === "Y"
    row.setAll({
        tilesStatus: cancelled ? "cancelled" : "failed",
        tilesNote: cancelled ? L("Cancelled") : L("The simplifying stopped without finishing"),
        tilesCancel: "N"
    })
    writeRow(row, false)
    return true
}

// ===== cancel#OverlayTiles: the user's own run stops at its next zoom =====
function cancelTiles(p) {
    if (!text(ec.user.userId)) throw new Error(L("Sign in to cancel"))
    const overlayId = text(p.overlayId)
    const row = overlayId ? find(OVERLAY, {overlayId}).forUpdate(true).one() : null
    if (!row || row.ownerUserId !== ec.user.userId || !TILES_BUSY.includes(text(row.tilesStatus)))
        return refused("RECORD_UNAVAILABLE", L("The overlay is not being simplified"))
    row.set("tilesCancel", "Y")
    // a run that has ended without saying so is ended now; a live one stops at its next zoom or tiles
    if (!endStale(row)) writeRow(row, false)
    return {applied: true, reasonCode: null, reason: null}
}

// ===== get#OverlayTilesStatus: Browse's light poll of the user's own runs =====
function tilesStatus(p) {
    if (!text(ec.user.userId)) throw new Error(L("Sign in to read your overlays"))
    const ids = (Array.isArray(p.overlayIds) ? p.overlayIds : text(p.overlayIds).split(","))
        .map(text)
        .filter(id => id)
        .slice(0, MAX_OVERLAYS)
    if (!ids.length) return {statusList: []}
    const rows = Array.from(
        ec.entity.find(OVERLAY).condition("overlayId", "in", ids).condition("ownerUserId", ec.user.userId).disableAuthz().list()
    )
    return {
        statusList: rows.map(row => ({
            overlayId: String(row.overlayId),
            tilesStatus: text(row.tilesStatus) || null,
            tilesProgress: row.tilesProgress == null ? null : Number(row.tilesProgress)
        }))
    }
}

// ===== delete#Overlay: the row only, the file stays =====
function deleteOverlay(p) {
    if (!text(ec.user.userId)) throw new Error(L("Sign in to remove an overlay"))
    const overlayId = text(p.overlayId)
    const row = overlayId ? one(OVERLAY, {overlayId}) : null
    if (!row || row.ownerUserId !== ec.user.userId)
        return refused("RECORD_UNAVAILABLE", L("The overlay is not available"), {overlayId})
    find(OVERLAY, {overlayId}).deleteAll()
    return {applied: true, reasonCode: null, reason: null, overlayId}
}

// ===== get#OverlayEdit and store#OverlayEdit: editing one's own overlay on the map =====
// Only the owner edits an overlay, and only one drawn whole (not simplified into tiles). The map edits what it draws, the
// sanitized GeoJSON, and a save writes that, read again by the converter: as a new version of the overlay's own file only
// when nothing of the file would change but the edit (a GeoJSON file that is what the converter makes of it, compared
// as JSON, the user's saved places or an edited copy made here), through FileServices.save#Text (the drive's write check,
// quota and versions); else as "name.edited.geojson" beside it (FileServices.upload#Resource, which never replaces a file
// there), which the overlay draws from then on, the original kept as it was (sourcePath) with what reading it found
// (sourceCrs, importNotes). A save names the version of the file it started from and is refused when the file has
// changed since (STALE_VERSION); a write to the file by other means between that check and the save is not told apart,
// and the drive keeps both versions. A reader of the original through a share of that file alone does not read the
// edited copy: a share of the folder reaches both. Saved places are edited the same way, points only.

// Whether two JSON values are the same, the order of an object's keys aside
function sameJson(left, right) {
    if (left === right) return true
    if (typeof left !== "object" || typeof right !== "object" || left === null || right === null) return false
    if (Array.isArray(left) !== Array.isArray(right)) return false
    const leftKeys = Object.keys(left)
    if (leftKeys.length !== Object.keys(right).length) return false
    return leftKeys.every(key => Object.hasOwn(right, key) && sameJson(left[key], right[key]))
}
// Whether the file at location is what the converter made of it (reading): a GeoJSON file of only what a map keeps, so a
// save over it loses nothing
function roundTrips(row, location, reading) {
    if (text(row.formatEnumId) !== "MLF_GEOJSON") return false
    try {
        const raw = quietly(() => ec.resource.getLocationReference(location).getText())
        return raw != null && sameJson(JSON.parse(String(raw)), JSON.parse(String(reading.geojson)))
    } catch (err) {
        return false
    }
}
// Where a save writes, from the file as it is now: {path, inPlace}
function editTarget(row, location, reading) {
    const path = cleanPath(row.path)
    if (row.isPlaces === "Y" || text(row.sourcePath) || roundTrips(row, location, reading)) return {path, inPlace: true}
    const folder = path.includes("/") ? path.slice(0, path.lastIndexOf("/") + 1) : ""
    return {path: folder + fileNameOf(path).replace(/\.[^.]*$/, "") + EDITED_SUFFIX, inPlace: false}
}
// The user's level in a whole drive (a file at its root is written beside in the drive itself)
function driveLevel(driveId) {
    const access = probe("org.moqui.resource.FileServices.get#DriveAccess", {driveId})
    return access && access.drive && access.drive.driveKindEnumId !== "RdkRecords" ? Number(access.level) || 0 : 0
}
const geojsonMaxFeatures = row => (row.isPlaces === "Y" ? Math.min(MAX_PLACES, maxFeatures()) : maxFeatures())
// The overlay a user may edit now, read as it is: {row, location, version, reading, target} or a refusal
function editable(overlayId, forUpdate) {
    if (!text(ec.user.userId)) throw new Error(L("Sign in to edit an overlay"))
    const row = overlayId ? find(OVERLAY, {overlayId}).forUpdate(!!forUpdate).one() : null
    if (!row || row.ownerUserId !== ec.user.userId) return refused("RECORD_UNAVAILABLE", L("The overlay is not available"))
    if (text(row.tilesStatus))
        return refused("NOT_EDITABLE", L("This overlay is simplified on the server for drawing: edit its file, then add it again"))
    const location = locationOf(row)
    const change = location ? quietly(() => ec.resource.getLastChange(location)) : null
    if (!change || change.isFile === false) return refused("FILE_GONE", L("The overlay's file is gone"))
    if (Number(change.size) > maxBytes()) return refused("TOO_LARGE", L("The overlay's file is larger than an overlay may be"))
    const reading = quietly(() => convert({location, format: row.formatEnumId, maxFeatures: geojsonMaxFeatures(row)}))
    if (!reading.valid)
        return refused(text(reading.reasonCode) || "NOT_READ", text(reading.reason) || L("The overlay's file cannot be drawn"))
    if (String(reading.geojson || "").length > GEOJSON_MAX_CHARS)
        return refused("TOO_LARGE_TO_DRAW", L("The overlay's file has more than a map draws at once"))
    const target = editTarget(row, location, reading)
    const folder = target.path.includes("/") ? target.path.slice(0, target.path.lastIndexOf("/")) : ""
    const level = target.inPlace ? levelAt(row.driveId, target.path) : folder ? levelAt(row.driveId, folder) : driveLevel(row.driveId)
    if (level < 2) return refused("NOT_WRITABLE", L("You may not change this overlay's file or add a file beside it"))
    return {row, location, version: versionOf(change), reading, target}
}

function editOverlay(p) {
    const found = editable(text(p.overlayId), false)
    if (!found.row) return found
    const {row, version, reading, target} = found
    return {
        applied: true,
        reasonCode: null,
        reason: null,
        overlayId: String(row.overlayId),
        geojson: String(reading.geojson),
        version,
        isPlaces: row.isPlaces === "Y",
        editPath: target.path,
        inPlace: target.inPlace
    }
}

// A free name beside the file for its edited copy: "name.edited.geojson", else "name.edited (2).geojson" and on, a name
// with no file and no overlay of the owner's
function besidePath(row, wanted) {
    const root = driveRoot(row.driveId)
    const stem = wanted.slice(0, wanted.length - ".geojson".length)
    for (let n = 1; n < 100; n++) {
        const path = n === 1 ? wanted : stem + " (" + n + ").geojson"
        const there = quietly(() => ec.resource.getLastChange(root + "/" + path))
        const rowThere = find(OVERLAY, {ownerUserId: row.ownerUserId, driveId: row.driveId, path}).one()
        if (!there && !rowThere) return path
    }
    return null
}

function saveEdit(p) {
    // the row locked, and the file read as it is now: its version and where a save goes
    const found = editable(text(p.overlayId), true)
    if (!found.row) return found
    const {row, version, target} = found
    if (version !== text(p.baseVersion))
        return refused("STALE_VERSION", L("The overlay's file changed after you started editing: open it again and redo your changes"))
    const given = String(p.geojson ?? "")
    if (given.length > maxBytes()) return refused("TOO_LARGE", say("The overlay is larger than the ${limit} bytes an overlay may be", {limit: maxBytes()}))
    let submitted = null
    try {
        submitted = JSON.parse(given)
    } catch (err) {
        submitted = null
    }
    if (!submitted || submitted.type !== "FeatureCollection" || !Array.isArray(submitted.features))
        return refused("NOT_FORMAT", L("The edit is not a GeoJSON FeatureCollection"))
    const isPlaces = row.isPlaces === "Y"
    let saved
    if (!submitted.features.length) {
        // places may all be removed, as delete#Place may remove the last; an overlay with nothing is removed instead
        if (!isPlaces) return refused("NO_FEATURES", L("An overlay needs a feature: remove the overlay from the map instead"))
        saved = {valid: true, geojson: JSON.stringify({type: "FeatureCollection", features: []}), featureCount: 0, rejectedCount: 0, notes: []}
    } else {
        // read as any overlay: the same bounds, checks and plain text; a geometry that is not sane refuses the save
        saved = convert({text: given, format: "MLF_GEOJSON", maxFeatures: geojsonMaxFeatures(row)})
        if (!saved.valid)
            return refused(text(saved.reasonCode) || "NOT_READ", text(saved.reason) || L("The edit cannot be drawn"), {notes: Array.from(saved.notes || [])})
        if (Number(saved.rejectedCount) > 0)
            return refused(
                "INVALID_GEOMETRY",
                say("${count} features have a geometry that is not valid: fix them before saving", {count: Number(saved.rejectedCount)}),
                {notes: Array.from(saved.notes || []).slice(0, 10)}
            )
        if (String(saved.geojson || "").length > GEOJSON_MAX_CHARS)
            return refused("TOO_LARGE_TO_DRAW", L("The overlay would have more than a map draws at once"))
        if (isPlaces && JSON.parse(saved.geojson).features.some(feature => feature?.geometry?.type !== "Point"))
            return refused("POINTS_ONLY", L("Your places are points only"))
    }
    const body = JSON.stringify(JSON.parse(saved.geojson), null, 1)
    const errors = []
    let path
    if (target.inPlace) {
        path = target.path
        const written = probe(
            "org.moqui.resource.FileServices.save#Text",
            {driveId: row.driveId, path, text: body, versionNote: L("Edited on the map")},
            errors
        )
        if (!written) return refused("NOT_SAVED", errors[0] || L("The overlay was not saved"))
    } else {
        // a new file beside the original: written only where no file is (keepBoth takes the next free name if one appeared)
        const wanted = besidePath(row, target.path)
        if (!wanted) return refused("NOT_SAVED", L("No free name for the edited copy beside the file"))
        const folder = wanted.includes("/") ? wanted.slice(0, wanted.lastIndexOf("/")) : ""
        const uploaded = probe(
            "org.moqui.resource.FileServices.upload#Resource",
            {
                driveId: row.driveId,
                path: folder,
                fileList: [body],
                relativePathList: [fileNameOf(wanted)],
                conflictPolicy: "keepBoth",
                versionNote: say("Edited on the map from ${file}", {file: fileNameOf(row.path)})
            },
            errors
        )
        const written = Array.from(uploaded?.uploadedList || []).find(entry => entry.outcome !== "failed" && entry.outcome !== "skipped")
        if (!written) return refused("NOT_SAVED", errors[0] || L("The overlay was not saved"))
        path = cleanPath(written.path)
    }
    const kept = {sourceCrs: row.sourceCrs, crsTransform: row.crsTransform, importNotes: row.importNotes}
    if (path !== cleanPath(row.path)) {
        row.set("sourcePath", cleanPath(row.path))
        row.set("path", path)
        row.set("formatEnumId", "MLF_GEOJSON")
    }
    // the file now holds the converter's output, nothing of it left out; an edited copy keeps what reading its original
    // found (its CRS and notes), the provenance of what it was made from
    recordReading(row, Object.assign({}, saved, {rejectedCount: 0, notes: []}))
    if (text(row.sourcePath)) row.setAll(kept)
    writeRow(row, false)
    const after = quietly(() => ec.resource.getLastChange(driveRoot(row.driveId) + "/" + path))
    return {
        applied: true,
        reasonCode: null,
        reason: null,
        overlayId: String(row.overlayId),
        version: versionOf(after),
        path,
        featureCount: Number(saved.featureCount) || 0
    }
}

// ===== find#Overlays =====
// The drives where another user's overlay may be readable to the current user: those with a share to them (the user,
// one of their groups, their party) or a membership of theirs
function candidateDrives() {
    const userId = ec.user.userId
    const groups = Array.from(ec.user.getUserGroupIdSet() || []).map(String)
    const account = one("moqui.security.UserAccount", {userId})
    const cf = ec.entity.conditionFactory
    const sharedWith = [cf.makeCondition("sharedWithUserId", "equals", userId)]
    if (groups.length) sharedWith.push(cf.makeCondition("sharedWithUserGroupId", "in", groups))
    if (text(account?.partyId)) sharedWith.push(cf.makeCondition("sharedWithPartyId", "equals", account.partyId))
    const drives = new Set()
    for (const share of ec.entity
        .find("moqui.resource.ResourceShare")
        .condition(cf.makeCondition(sharedWith, "or"))
        .condition("revokedDate", "is-null", null)
        .condition("declinedDate", "is-null", null)
        .selectField("driveId,isLink")
        .disableAuthz()
        .limit(500)
        .list())
        if (share.isLink !== "Y") drives.add(String(share.driveId))
    const memberOf = [cf.makeCondition("userId", "equals", userId)]
    if (groups.length) memberOf.push(cf.makeCondition("userGroupId", "in", groups))
    for (const member of ec.entity
        .find("moqui.resource.ResourceDriveMember")
        .condition(cf.makeCondition(memberOf, "or"))
        .conditionDate("fromDate", "thruDate", null)
        .selectField("driveId")
        .disableAuthz()
        .limit(500)
        .list())
        drives.add(String(member.driveId))
    return Array.from(drives)
}
function overlayEntry(row, isOwn) {
    const bounds = [row.minLongitude, row.minLatitude, row.maxLongitude, row.maxLatitude].map(num)
    return {
        overlayId: String(row.overlayId),
        layerId: "overlay:" + row.overlayId,
        label: text(row.label) || fileNameOf(row.path),
        isOwn,
        isPlaces: row.isPlaces === "Y",
        ownerName: isOwn ? "" : ownerName(row.ownerUserId),
        fileName: fileNameOf(row.path),
        path: isOwn ? cleanPath(row.path) : null,
        // the file an edit on the map was made from, kept beside the edited copy the overlay draws
        sourcePath: isOwn && text(row.sourcePath) ? cleanPath(row.sourcePath) : null,
        formatEnumId: text(row.formatEnumId) || null,
        featureCount: Number(row.featureCount) || 0,
        rejectedCount: Number(row.rejectedCount) || 0,
        bounds: bounds.every(value => value !== null) ? bounds : null,
        notes: text(row.importNotes) ? text(row.importNotes).split("\n") : [],
        url: OVERLAY_PATH + "?overlayId=" + encodeURIComponent(row.overlayId),
        // simplified into vector tiles on the server: drawn from LocalMap's tile transition once ready
        tilesStatus: text(row.tilesStatus) || null,
        tilesProgress: row.tilesProgress == null ? null : Number(row.tilesProgress),
        tilesNote: text(row.tilesNote) || null,
        tilesMaxZoom: text(row.tilesStatus) === "ready" ? Number(row.tilesMaxZoom) || 0 : null,
        tileUrl:
            text(row.tilesStatus) === "ready" && text(row.tilesRevision)
                ? sources().tileUrl("overlay:" + row.overlayId, text(row.tilesRevision))
                : null
    }
}
let sourcesScript = null
const sources = () =>
    (sourcesScript = sourcesScript || ec.resource.script("component://framework/service/org/moqui/maps/MapSources.js"))
// The user's own overlays and, read-only, those of others whose file the user may read now. Changes nothing.
function findOverlays(p, memo) {
    if (!text(ec.user.userId)) throw new Error(L("Sign in to read your overlays"))
    const userId = ec.user.userId
    const known = memo || new Map()
    const placesOnly = yes(p.placesOnly)
    const ownConditions = placesOnly ? {ownerUserId: userId, isPlaces: "Y"} : {ownerUserId: userId}
    const own = Array.from(find(OVERLAY, ownConditions).orderBy("label").limit(MAX_OVERLAYS).list())
    const overlayList = own.map(row => overlayEntry(row, true))
    const seen = new Set(own.map(row => row.driveId + "/" + cleanPath(row.path)))
    const drives = candidateDrives()
    if (drives.length) {
        let othersFind = ec.entity.find(OVERLAY).condition("driveId", "in", drives).condition("ownerUserId", "not-equals", userId)
        if (placesOnly) othersFind = othersFind.condition("isPlaces", "equals", "Y")
        const others = Array.from(othersFind.orderBy("label").disableAuthz().limit(300).list())
        let shared = 0
        for (const row of others) {
            if (shared >= MAX_SHARED) break
            const key = row.driveId + "/" + cleanPath(row.path)
            if (seen.has(key)) continue
            // one row that fails to check never takes the list with it
            try {
                if (levelAt(row.driveId, row.path, known) < 1) continue
            } catch (err) {
                ec.logger.warn("Map overlay " + row.overlayId + " not checked: " + (err?.message ?? err))
                continue
            }
            seen.add(key)
            overlayList.push(overlayEntry(row, false))
            shared++
        }
    }
    return {overlayList}
}

// ===== get#OverlayContent: the overlay transition, every request =====
// {ok, status, geojson} or {ok: false, status, reasonCode, reason}: the overlay's file read for the current user, who
// must read it in its drive now (the owner as anyone else), writing nothing
function overlayContent(p, memo) {
    const overlayId = text(p.overlayId)
    const row = overlayId ? one(OVERLAY, {overlayId}) : null
    const missing = {ok: false, status: 404, reasonCode: "OVERLAY_MISSING", reason: L("The overlay is not available")}
    if (!row || !text(ec.user.userId)) return missing
    if (levelAt(row.driveId, row.path, memo) < 1) return missing
    const location = locationOf(row)
    if (!location) return missing
    const change = quietly(() => ec.resource.getLastChange(location))
    if (!change || change.isFile === false)
        return {ok: false, status: 404, reasonCode: "FILE_GONE", reason: L("The overlay's file is gone")}
    const key = [location, text(change.versionName), text(change.size), text(change.date)].join("|")
    const cache = ec.cache.getCache(CACHE)
    const tooLarge = {
        ok: false,
        status: 413,
        reasonCode: "TOO_LARGE_TO_DRAW",
        reason: L("The overlay's file has grown past what a map draws at once: simplify it or split it")
    }
    const hit = cache.get(key)
    if (hit) return String(hit) === TOO_LARGE ? tooLarge : {ok: true, status: 200, geojson: String(hit)}
    const reading = quietly(() => convert({location, format: row.formatEnumId}))
    if (!reading.valid)
        return {
            ok: false,
            status: 422,
            reasonCode: text(reading.reasonCode) || "NOT_READ",
            reason: text(reading.reason) || L("The overlay's file cannot be drawn")
        }
    const geojson = String(reading.geojson || "")
    // a file too large to serve is remembered as such, so it is not read again until it changes
    if (geojson.length > GEOJSON_MAX_CHARS) {
        cache.put(key, TOO_LARGE)
        return tooLarge
    }
    cache.put(key, geojson)
    return {ok: true, status: 200, geojson}
}

// ===== saved places: Maps/places.geojson of the personal drive =====
// The places file as a FeatureCollection; null when it is not one, or is larger than an overlay may be (never read whole
// then): it is refused rather than overwritten
function readPlaces(driveId) {
    const root = driveRoot(driveId)
    if (!root) return null
    const location = root + "/" + PLACES_PATH
    const change = quietly(() => ec.resource.getLastChange(location))
    if (!change) return {type: "FeatureCollection", features: []}
    if (change.isFile === false || Number(change.size) > maxBytes()) return null
    const found = quietly(() => ec.resource.getLocationReference(location).getText())
    if (found == null) return {type: "FeatureCollection", features: []}
    try {
        const parsed = JSON.parse(String(found))
        if (parsed && parsed.type === "FeatureCollection" && Array.isArray(parsed.features)) return parsed
    } catch (err) {
        // a file the user broke by hand: refused rather than overwritten
    }
    return null
}
function writePlaces(driveId, collection, note) {
    const errors = []
    const saved = probe(
        "org.moqui.resource.FileServices.save#Text",
        {driveId, path: PLACES_PATH, text: JSON.stringify(collection, null, 1), versionNote: note},
        errors
    )
    return saved ? null : errors[0] || L("Your places were not saved")
}
// The places overlay row of the user, locked for the rest of the transaction (made first when there is none), so one
// save of their places waits for another rather than both reading the file and one write losing the other's place
function lockPlaces(driveId) {
    const conditions = {ownerUserId: ec.user.userId, driveId, path: PLACES_PATH}
    const existing = find(OVERLAY, conditions).forUpdate(true).one()
    if (existing) return existing
    const made = ec.entity.makeValue(OVERLAY)
    made.setSequencedIdPrimary()
    made.setAll(
        Object.assign({}, conditions, {
            createdDate: ec.user.nowTimestamp,
            label: L("My places"),
            formatEnumId: "MLF_GEOJSON",
            isPlaces: "Y",
            featureCount: 0,
            rejectedCount: 0
        })
    )
    writeRow(made, true)
    return find(OVERLAY, conditions).forUpdate(true).one()
}
// The places overlay row of the user, made or brought up to date with the file as written
function placesRow(driveId) {
    const userId = ec.user.userId
    const existing = find(OVERLAY, {ownerUserId: userId, driveId, path: PLACES_PATH}).forUpdate(true).one()
    const row = existing || ec.entity.makeValue(OVERLAY)
    if (!existing) {
        row.setSequencedIdPrimary()
        row.setAll({ownerUserId: userId, driveId, path: PLACES_PATH, createdDate: ec.user.nowTimestamp, label: L("My places")})
    }
    row.set("formatEnumId", "MLF_GEOJSON")
    row.set("isPlaces", "Y")
    const root = driveRoot(driveId)
    const reading = quietly(() => convert({location: root + "/" + PLACES_PATH, format: "MLF_GEOJSON", summaryOnly: true}))
    if (reading.valid) recordReading(row, reading)
    else row.setAll({featureCount: 0, rejectedCount: 0, importNotes: null})
    writeRow(row, !existing)
    return row
}
function storePlace(p) {
    if (!text(ec.user.userId)) throw new Error(L("Sign in to save a place"))
    const label = labelText(p.label, 200)
    if (!label) return refused("LABEL_MISSING", L("Give the place a name"))
    const lon = num(p.longitude)
    const lat = num(p.latitude)
    if (lon === null || lat === null || lon < -180 || lon > 180 || lat < -90 || lat > 90)
        return refused("POINT_INVALID", L("A place is a latitude from -90 to 90 and a longitude from -180 to 180"))
    const driveId = personalDrive()
    if (!driveId) return refused("NO_DRIVE", L("Your files are not available"))
    lockPlaces(driveId)
    const collection = readPlaces(driveId)
    if (!collection)
        return refused(
            "PLACES_UNREADABLE",
            say(
                "Your places file (${path}) is not a GeoJSON FeatureCollection of at most ${limit} bytes: fix or move it in Files",
                {
                    path: PLACES_PATH,
                    limit: maxBytes()
                }
            )
        )
    if (collection.features.length >= Math.min(MAX_PLACES, maxFeatures()))
        return refused(
            "TOO_MANY_PLACES",
            say("You have ${count} saved places: remove one first", {count: collection.features.length})
        )
    const used = new Set(collection.features.map(feature => text(feature?.id)))
    let placeId = "p" + Date.now().toString(36)
    for (let n = 2; used.has(placeId); n++) placeId = "p" + Date.now().toString(36) + "-" + n
    collection.features.push({
        type: "Feature",
        id: placeId,
        geometry: {type: "Point", coordinates: [Math.round(lon * 1e7) / 1e7, Math.round(lat * 1e7) / 1e7]},
        properties: {name: label}
    })
    const failed = writePlaces(driveId, collection, say("Saved the place ${label}", {label}))
    if (failed) return refused("NOT_SAVED", failed)
    const row = placesRow(driveId)
    return {applied: true, reasonCode: null, reason: null, placeId, overlayId: String(row.overlayId)}
}
function deletePlace(p) {
    if (!text(ec.user.userId)) throw new Error(L("Sign in to remove a place"))
    const placeId = text(p.placeId)
    const driveId = personalDrive()
    // its row locked as a save's is; with no row there are no places to remove
    const locked = driveId ? find(OVERLAY, {ownerUserId: ec.user.userId, driveId, path: PLACES_PATH}).forUpdate(true).one() : null
    const collection = locked ? readPlaces(driveId) : null
    const kept = collection ? collection.features.filter(feature => text(feature?.id) !== placeId) : null
    if (!collection || !placeId || kept.length === collection.features.length)
        return refused("RECORD_UNAVAILABLE", L("The place is not available"), {placeId})
    collection.features = kept
    const failed = writePlaces(driveId, collection, L("Removed a saved place"))
    if (failed) return refused("NOT_SAVED", failed)
    placesRow(driveId)
    return {applied: true, reasonCode: null, reason: null, placeId}
}

// Browse's search: the saved places of the user and those shared with them whose name has the text, at most limit; with
// all, every one of them (the terminal's list)
function searchPlaces(p) {
    const term = text(p.q).toLowerCase()
    const all = yes(p.all)
    if ((!term && !all) || !text(ec.user.userId)) return {results: []}
    const limit = Math.min(50, Number(p.limit) || 20)
    const results = []
    const memo = new Map()
    for (const entry of findOverlays({placesOnly: true}, memo).overlayList) {
        if (results.length >= limit) break
        const content = overlayContent({overlayId: entry.overlayId}, memo)
        if (!content.ok) continue
        let features = []
        try {
            features = JSON.parse(content.geojson).features || []
        } catch (err) {
            features = []
        }
        for (const feature of features) {
            if (results.length >= limit) break
            const label = text(feature?.properties?.label)
            if ((!all && !label.toLowerCase().includes(term)) || feature.geometry?.type !== "Point") continue
            const [lon, lat] = feature.geometry.coordinates
            results.push({
                kind: "place",
                id: "place:" + entry.overlayId + ":" + feature.id,
                label,
                lon,
                lat,
                overlayId: entry.overlayId,
                placeId: entry.isOwn ? String(feature.id) : null,
                isOwn: entry.isOwn,
                sourceText: entry.isOwn ? L("Your saved place") : L("Shared by") + " " + entry.ownerName,
                precisionEnumId: "GPP_USER_CONFIRMED"
            })
        }
    }
    return {results}
}

// A layer of a saved view or a Browse configuration naming an overlay ("overlay:ID"): whether the user may read it now
function overlayLayerReadable(layerId) {
    const overlayId = text(layerId).replace(/^overlay:/, "")
    const row = overlayId ? one(OVERLAY, {overlayId}) : null
    return !!row && levelAt(row.driveId, row.path) >= 1
}

const operations = {
    storeOverlay,
    editOverlay,
    saveEdit,
    buildTiles,
    cancelTiles,
    tilesStatus,
    deleteOverlay,
    findOverlays,
    overlayContent,
    storePlace,
    deletePlace,
    searchPlaces
}
;({
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown map overlay operation " + op)
        return operations[op](parameters || {})
    },
    // for saved views (MapConfig.js)
    overlayLayerReadable,
    PLACES_PATH
})
