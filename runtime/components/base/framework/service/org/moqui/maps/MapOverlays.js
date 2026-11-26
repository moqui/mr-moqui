// Users' overlays and saved places of org.moqui.maps.MapServices (plans/Maps.md §4.1, §4.2, §8; docs/tools/Maps.md
// "Overlays and places"): a GeoJSON, GPX or KML file added as a personal overlay, the overlay transition's access check
// and content, the overlays a user owns or reads through a share, and the saved places file.
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
const MAX_OVERLAYS = 100
const MAX_SHARED = 100
const MAX_PLACES = 1000
const EXTENSIONS = /\.(geojson|json|gpx|kml)$/i

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
const formatOf = name => {
    const lower = text(name).toLowerCase()
    return /\.(geojson|json)$/.test(lower)
        ? "MLF_GEOJSON"
        : lower.endsWith(".gpx")
          ? "MLF_GPX"
          : lower.endsWith(".kml")
            ? "MLF_KML"
            : null
}
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
// convert#Overlay (Rust): {valid, reasonCode, reason, featureCount, rejectedCount, bounds, crs, notes, geojson}
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
    if (!format) return refused("FORMAT_UNKNOWN", L("An overlay is a GeoJSON, GPX or KML file (.geojson, .json, .gpx or .kml)"))
    const limit = maxBytes()
    let reading
    if (file) {
        if (Number(file.size) > limit)
            return refused("TOO_LARGE", say("The file is larger than the ${limit} bytes an overlay may be", {limit}))
        reading = convert({text: file.getString("UTF-8"), format})
    } else {
        // a '..' or a '#' would reach another file
        const unsafe = String(p.path ?? "").includes("..") || path.includes("#")
        if (!driveId || !path || unsafe) return refused("NOT_FOUND", L("Choose a file to add"))
        if (levelAt(driveId, path) < 1) return refused("NOT_FOUND", L("File not found"))
        const root = driveRoot(driveId)
        if (!root) return refused("NOT_FOUND", L("File not found"))
        reading = quietly(() => convert({location: root + "/" + path, format}))
    }
    if (!reading.valid)
        return refused(text(reading.reasonCode) || "NOT_READ", text(reading.reason) || L("The file cannot be drawn"))
    if (String(reading.geojson || "").length > GEOJSON_MAX_CHARS)
        return refused(
            "TOO_LARGE_TO_DRAW",
            say("The file has more than a map draws at once (${limit} characters as GeoJSON): simplify it or split it", {
                limit: GEOJSON_MAX_CHARS
            })
        )
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
    writeRow(row, !existing)
    return {
        applied: true,
        reasonCode: null,
        reason: null,
        overlayId: String(row.overlayId),
        featureCount: Number(reading.featureCount) || 0,
        rejectedCount: Number(reading.rejectedCount) || 0,
        notes: Array.from(reading.notes || [])
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
        formatEnumId: text(row.formatEnumId) || null,
        featureCount: Number(row.featureCount) || 0,
        rejectedCount: Number(row.rejectedCount) || 0,
        bounds: bounds.every(value => value !== null) ? bounds : null,
        notes: text(row.importNotes) ? text(row.importNotes).split("\n") : [],
        url: OVERLAY_PATH + "?overlayId=" + encodeURIComponent(row.overlayId)
    }
}
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

// ===== find#OverlayFiles: files the user could add =====
// The geographic files of the user's personal drive (its top folder, Maps and Maps/overlays) and their recent files in
// any drive they still read, at most 50
function findOverlayFiles() {
    if (!text(ec.user.userId)) throw new Error(L("Sign in to choose a file"))
    const memo = new Map()
    const out = []
    const seen = new Set()
    const add = (driveId, path, driveName) => {
        const key = driveId + "/" + path
        if (seen.has(key) || out.length >= 50 || !EXTENSIONS.test(path)) return
        seen.add(key)
        out.push({driveId, path, name: fileNameOf(path), driveName: driveName || ""})
    }
    const own = personalDrive()
    const root = own ? driveRoot(own) : null
    if (root)
        for (const folder of ["", "Maps", OVERLAY_FOLDER]) {
            let entries = []
            try {
                entries = Array.from(quietly(() => ec.resource.getChildChanges(folder ? root + "/" + folder : root)) || [])
            } catch (err) {
                entries = []
            }
            for (const entry of entries)
                if (entry.isFile) add(own, folder ? folder + "/" + entry.name : String(entry.name), L("My files"))
        }
    const recent = Array.from(
        find("moqui.resource.ResourceRecent", {userId: ec.user.userId}).orderBy("-lastModifiedDate").limit(200).list()
    )
    for (const row of recent) {
        if (out.length >= 50) break
        const path = cleanPath(row.path)
        if (!EXTENSIONS.test(path) || seen.has(row.driveId + "/" + path)) continue
        if (levelAt(row.driveId, path, memo) < 1) continue
        const drive = find(DRIVE, {driveId: row.driveId}).useCache(true).one()
        add(String(row.driveId), path, drive ? text(drive.driveName) : "")
    }
    return {fileList: out}
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

// Browse's search: the saved places of the user and those shared with them whose name has the text, at most limit
function searchPlaces(p) {
    const term = text(p.q).toLowerCase()
    if (!term || !text(ec.user.userId)) return {results: []}
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
            if (!label.toLowerCase().includes(term) || feature.geometry?.type !== "Point") continue
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
    deleteOverlay,
    findOverlays,
    overlayContent,
    findOverlayFiles,
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
