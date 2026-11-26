// Map archives of org.moqui.maps.MapSourceServices (plans/Maps.md §4.1, §5; docs/tools/Maps.md "Archives"): registering a
// PMTiles archive as a layer's revision, the access check of the map-source transition (LocalMap.xml source), the
// same-origin URL the effective configuration hands a map (MapConfig.js), the archives Configuration lists and their
// readiness.
//
// A PMTiles layer (MapLayer, format MLF_PMTILES_VECTOR or MLF_PMTILES_RASTER) names a ResourceFacade location
// (resourceLocation). Registering it records the revision a map reads: on storage that keeps versions (git://,
// dbresource://) the current version (sourceVersionName), and in every case the content address of
// ec.resource.getContentAddress (an LFS oid, a content blob's or a small file's SHA-256: sourceRevision, sourceTag,
// sourceSize, sourceAddressKind); a file on disk has none, so its SHA-256 is read once, with its modification time
// (sourceModifiedDate). An archive in a git repository must be an LFS object (NOT_LFS): any other git object is read whole
// for every request. Storage not known to read by range is refused (STORAGE_NOT_RANGED), as is an archive whose header
// (read#ArchiveHeader, two small reads) a map cannot draw or whose tiles are not of the layer's format. The drive the
// location is under when it is registered is recorded (driveId, shown in Configuration); access is decided by the drive it
// is under at each request.
//
// The transition's URL names the layer and the revision. On every request (GET, HEAD, each range) the layer is resolved and
// the caller checked, writing nothing (no ResourceRecent): the layer enabled and registered, its viewPermission, its
// provider enabled when it has one, and the storage's own access: a level of at least view on the drive the location is
// under at its path (get#DriveAccess: a member, or a share of the file or a folder above it, so a revoked share or a
// removed member is refused at the next request), else for a git:// location view of its repository
// (its own AT_GIT_REPOSITORY grants, as the git server decides a clone). A location outside every drive and every repository (a dbresource:// path no drive holds, a
// file on the server's disk) has no access model of its own: every user who may see the layer reads it. A revision
// that is not the registered one is 409, so the map loads its configuration again rather than mix revisions; so is a file
// on disk changed since it was registered. The bytes go out by ec.web.sendPinnedResourceResponse with the registered tag
// as the strong ETag and Cache-Control private for resource-facade@pinned-max-age.
//
// What reaches a browser: the transition's path with the layer's ID and revision, never the location.

const LAYER = "moqui.maps.MapLayer"
const PROVIDER = "moqui.maps.MapProvider"
const DRIVE = "moqui.resource.ResourceDrive"
const PROFILE = "moqui.maps.MapProfile"
const CONFIGURATION_SCREEN = "component://tools/screen/Maps/Configuration.xml"
const SOURCE_PATH = "/apps/maps/LocalMap/source"
const PMTILES = ["MLF_PMTILES_VECTOR", "MLF_PMTILES_RASTER"]
// storage that reads a range of a file without reading the rest (plans/Maps.md §5, the backend table)
const RANGED_SCHEMES = ["file", "runtime", "component", "dbresource", "git"]
const MAX_ARCHIVES = 200
// An archive on the server's disk is registered only under the runtime's maps directory (runtime://maps/..., or a file: path
// in it), as an operator's Geo data file only under runtime://import/: no other file of the server becomes a map's
const DISK_ROOT = "runtime://maps/"

const L = value => ec.l10n.localize(value)
const say = (value, values) => ec.resource.expand(L(value), "", values || {})
const text = value => (value == null ? "" : String(value).trim())
const refusal = (reasonCode, reason, status) => ({ok: false, reasonCode, reason, status: status || 404})
const find = (entity, conditions) =>
    ec.entity
        .find(entity)
        .condition(conditions || {})
        .useCache(true)
        .disableAuthz()
const one = (entity, conditions) => find(entity, conditions).one()
// a location's scheme: file:/path and file:///path alike; a one-letter "scheme" (a Windows drive) is none
const schemeOf = location => {
    const match = /^([a-z][a-z0-9+.-]*):/i.exec(text(location))
    return match && match[1].length > 1 ? match[1].toLowerCase() : ""
}
const millis = value => {
    if (value == null || value === "") return null
    if (typeof value === "number") return value
    if (typeof value.getTime === "function") return value.getTime()
    if (/^\d+$/.test(String(value).trim())) return Number(String(value).trim())
    const parsed = Date.parse(String(value))
    return Number.isNaN(parsed) ? null : parsed
}
// a number field as a JavaScript number (a decimal field is a Decimal, which Number() refuses)
const num = value =>
    value == null || value === "" ? null : typeof value?.toNumber === "function" ? value.toNumber() : Number(value)
const sameSecond = (a, b) => a !== null && b !== null && Math.floor(a / 1000) === Math.floor(b / 1000)
const conf = selector => text(ec.factory.getConfValue(selector))
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
// the caller's own grants of Configuration, never one inherited from an app root a map is mounted under
const canConfigure = () => !!ec.artifactExecution.isPermittedOwn(CONFIGURATION_SCREEN, "AT_XML_SCREEN", "AUTHZA_VIEW")
const canChangeConfiguration = () => !!ec.artifactExecution.isPermittedOwn(CONFIGURATION_SCREEN, "AT_XML_SCREEN", "AUTHZA_UPDATE")
// A write of configuration no group is granted to write, after the caller was checked: authorization off, the caller kept
// for the audit log
function asCaller(write) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return write()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}

// A read of the resource facade as the framework: the caller was checked, and the storage's rows (DbResource, git) are no
// group's to read
const quietly = read => asCaller(read)

// The location a map reads: on storage that keeps versions, the registered version
const pinnedLocation = layer =>
    text(layer.sourceVersionName)
        ? text(layer.resourceLocation) + "#" + text(layer.sourceVersionName)
        : text(layer.resourceLocation)
const fileNameOf = location => {
    const base = text(location).split("#")[0].replace(/\/+$/, "")
    return base.slice(base.lastIndexOf("/") + 1) || "archive.pmtiles"
}

// ===== drives and repositories: the storage's own access =====
// The drive whose root is the longest one above location, or null: its ID, root and the location's path under it. Read on
// each request through the entity cache (a drive's root changes rarely; a write of ResourceDrive clears it).
function driveOf(location) {
    const base = text(location).split("#")[0]
    const scheme = schemeOf(base)
    if (!scheme) return null
    const candidates = []
    for (let at = base.lastIndexOf("/"); at > scheme.length; at = base.lastIndexOf("/", at - 1))
        candidates.push(base.slice(0, at), base.slice(0, at + 1))
    if (!candidates.length) return null
    const drives = Array.from(
        ec.entity
            .find(DRIVE)
            .condition("rootLocation", "in", candidates)
            .selectField("driveId,rootLocation")
            .useCache(true)
            .disableAuthz()
            .list()
    )
    if (!drives.length) return null
    const rootOf = drive => text(drive.rootLocation).replace(/\/+$/, "")
    drives.sort((a, b) => rootOf(b).length - rootOf(a).length)
    return {driveId: drives[0].driveId, root: rootOf(drives[0]), path: base.slice(rootOf(drives[0]).length).replace(/^\/+/, "")}
}
// The current user's level in a drive at a path (get#DriveAccess, with nothing written: its share query is by the drive's
// indexed ID): 0 when none
function driveLevel(drive) {
    // with no user signed in and authorization off, get#DriveAccess answers manage (a job's): never asked so
    if (!text(ec.user.userId) || !drive.path) return 0
    const access = probe("org.moqui.resource.FileServices.get#DriveAccess", {driveId: drive.driveId, path: drive.path})
    return access && access.drive ? Number(access.level) || 0 : 0
}
// A service call that reads, as the current user, with its own errors kept apart; null when it failed
function probe(name, parameters) {
    let out = null
    ec.message.pushErrors()
    try {
        out = ec.service.sync().name(name).parameters(parameters).disableAuthz().call()
        if (ec.message.hasError()) out = null
    } catch (err) {
        ec.logger.warn("Map archive access: " + name + " not answered: " + (err?.message ?? err))
        out = null
    } finally {
        ec.message.clearErrors()
        ec.message.popErrors()
    }
    return out
}
// Whether the current user may read the location by its storage's own access: the drive it is under, else a git
// repository's view; a location outside both is the installation's (true)
function storageAllows(location) {
    if (!text(ec.user.userId)) return false
    const drive = driveOf(location)
    if (drive) return driveLevel(drive) >= 1
    if (schemeOf(location) === "git") {
        // the repository's own grants, as the git server decides a clone (not what the frames below would inherit), asked
        // with authorization on whatever the caller's state
        const repositoryId = text(location).slice("git://".length).split("/")[0]
        if (!repositoryId) return false
        const wasDisabled = ec.artifactExecution.disableAuthz()
        ec.artifactExecution.enableAuthz()
        try {
            return !!ec.artifactExecution.isPermittedOwn(repositoryId, "AT_GIT_REPOSITORY", "view")
        } finally {
            if (wasDisabled) ec.artifactExecution.disableAuthz()
        }
    }
    return true
}

// ===== the access check: the transition's and the effective configuration's =====
// {ok, location, tag, size, fileName, url} or {ok: false, reasonCode, reason, status}. revision: the one the URL names
// (the transition's); null for the effective configuration, which hands out the registered one.
function check(layer, layerId, revision) {
    const missing = () => refusal("SOURCE_MISSING", say("the map archive ${layerId} is not registered", {layerId}), 404)
    if (!layer || layer.disabled === "Y") return missing()
    if (!PMTILES.includes(text(layer.formatEnumId)) || !text(layer.resourceLocation)) return missing()
    if (!text(layer.sourceRevision) || !text(layer.sourceTag) || layer.sourceSize == null)
        return refusal("SOURCE_NOT_REGISTERED", L("its archive has not been registered (Maps › Configuration › Archives)"), 404)
    // one the caller may not see is, to them, one that is not there
    if (text(layer.viewPermission) && !ec.user.hasPermission(layer.viewPermission)) return missing()
    if (text(layer.providerId)) {
        const provider = one(PROVIDER, {providerId: layer.providerId})
        if (!provider || provider.disabled === "Y") return refusal("PROVIDER_DISABLED", L("its provider is not enabled"), 404)
    }
    if (!text(ec.user.userId) || !storageAllows(layer.resourceLocation))
        return refusal("SOURCE_NOT_PERMITTED", L("its archive is not shared with you"), 403)
    if (revision !== null && text(revision) !== text(layer.sourceRevision))
        return refusal("REVISION_CHANGED", L("the archive has a newer registered revision: load the map again"), 409)
    const location = pinnedLocation(layer)
    if (text(layer.sourceAddressKind) === "file") {
        // a file on disk changed in place: its bytes are no longer those the tag names
        const stat = statOf(location)
        if (!stat || stat.size !== num(layer.sourceSize) || !sameSecond(stat.modified, millis(layer.sourceModifiedDate))) {
            ec.logger.warn("Map archive of layer " + layer.layerId + " changed since it was registered: register it again")
            return refusal(
                "SOURCE_CHANGED",
                L("its archive file changed since it was registered: an administrator registers it again"),
                409
            )
        }
    }
    return {
        ok: true,
        location,
        tag: text(layer.sourceTag),
        size: num(layer.sourceSize),
        fileName: fileNameOf(layer.resourceLocation),
        url:
            SOURCE_PATH +
            "?layerId=" +
            encodeURIComponent(layer.layerId) +
            "&revision=" +
            encodeURIComponent(layer.sourceRevision)
    }
}

// get#SourceAccess: for the transition, every request
function sourceAccess(p) {
    const layerId = text(p.layerId)
    const layer = layerId ? one(LAYER, {layerId}) : null
    const out = check(layer, layerId || "(none)", text(p.revision))
    return out.ok
        ? {ok: true, status: 200, location: out.location, tag: out.tag, size: out.size, fileName: out.fileName}
        : {ok: false, status: out.status, reasonCode: out.reasonCode, reason: out.reason}
}

// ===== get#ArchiveTile: the tile transition, every request (plans/Maps.md §5, a Z/X/Y endpoint) =====
// One tile of a registered archive, or of a user's overlay generalized into one on the server (overlay:ID, MapOverlays.js
// buildTiles), for a client with no PMTiles reader. The same check as the map-source transition, on every request: a layer
// by check() above; an overlay by its file's own access, the reader's in its drive at its path now (a revoked share or a
// removed member is refused at the next request), as the overlay transition decides it. Then the tile is read by range in
// Rust (read#ArchiveTile: the header and directories kept by the archive's tag), writing nothing.
const TILE_PATH = "/apps/maps/LocalMap/tile/"
const OVERLAY = "moqui.maps.MapOverlay"
const cleanPath = value =>
    String(value ?? "")
        .split("/")
        .filter(part => part && part !== ".")
        .join("/")
// The tile URL template of a layer or an overlay at a revision: {z}, {x} and {y} left for the client
const tileUrl = (layerId, revision) =>
    TILE_PATH + encodeURIComponent(layerId).replace(/%3A/g, ":") + "/" + encodeURIComponent(revision) + "/{z}/{x}/{y}"
// {ok, location, tag, size} of an overlay's generalized archive for the current user, or the refusal
function overlayArchive(overlayId, revision) {
    const missing = refusal("OVERLAY_MISSING", L("The overlay is not available"), 404)
    const row = overlayId ? ec.entity.find(OVERLAY).condition({overlayId}).disableAuthz().one() : null
    if (!row || !text(ec.user.userId)) return missing
    if (text(row.tilesStatus) !== "ready" || !text(row.tilesRevision) || !text(row.tilesPath) || row.tilesSize == null)
        return missing
    const drive = text(row.driveId) ? one(DRIVE, {driveId: row.driveId}) : null
    const root = drive ? text(drive.rootLocation).replace(/\/+$/, "") : ""
    if (!root || !cleanPath(row.path)) return missing
    // the overlay's access: its file's, for the reader, at its path, now (the owner as anyone); a reader it is not shared
    // with is told nothing of it
    if (driveLevel({driveId: row.driveId, root, path: cleanPath(row.path)}) < 1) return missing
    if (text(revision) !== text(row.tilesRevision))
        return refusal("REVISION_CHANGED", L("The overlay was simplified again: load the map again"), 409)
    const location = root + "/" + cleanPath(row.tilesPath)
    // an archive in a drive on the server's disk has no content address: one whose size or modification time changed
    // is not the recorded one, as check() decides for a registered file
    if (row.tilesModifiedDate != null) {
        const stat = statOf(location)
        if (!stat || stat.size !== num(row.tilesSize) || !sameSecond(stat.modified, millis(row.tilesModifiedDate)))
            return refusal("SOURCE_CHANGED", L("The overlay's simplified copy changed: add the file again"), 409)
    }
    return {
        ok: true,
        location: text(row.tilesVersionName) ? location + "#" + text(row.tilesVersionName) : location,
        tag: text(row.tilesTag),
        size: num(row.tilesSize)
    }
}
function archiveTile(p) {
    const layerId = text(p.layerId)
    const revision = text(p.revision)
    let target
    if (layerId.startsWith("overlay:")) target = overlayArchive(layerId.slice("overlay:".length), revision)
    else target = check(layerId ? one(LAYER, {layerId}) : null, layerId || "(none)", revision)
    if (!target.ok) return {ok: false, status: target.status, reasonCode: target.reasonCode, reason: target.reason}
    // the caller was checked above; the archive's storage is read as the framework
    const read = quietly(() =>
        ec.service
            .sync()
            .name("org.moqui.maps.MapSourceServices.read#ArchiveTile")
            .parameters({
                location: target.location,
                tag: target.tag,
                size: target.size,
                z: text(p.z),
                x: text(p.x),
                y: text(p.y),
                acceptGzip: p.acceptGzip === true || p.acceptGzip === "true",
                ifNoneMatch: text(p.ifNoneMatch)
            })
            .disableAuthz()
            .call()
    )
    if (ec.message.hasError() || !read) return {ok: false, status: 404, reasonCode: "TILE_UNREAD", reason: L("The tile could not be read")}
    const status = Number(read.status) || 404
    return {
        ok: [200, 204, 304].includes(status),
        status,
        reasonCode: status === 409 ? "REVISION_CHANGED" : status === 404 ? "TILE_MISSING" : null,
        reason: text(read.reason) || null,
        bytes: read.bytes ?? null,
        contentType: text(read.contentType) || null,
        contentEncoding: text(read.contentEncoding) || null,
        etag: text(read.etag) || null,
        cacheControl: text(read.cacheControl) || null
    }
}

// For MapConfig.js: the URL a map reads a layer's archive at, or the refusal
function servedUrl(layer) {
    const out = check(layer, layer?.layerId || "(none)", null)
    return out.ok ? {ok: true, url: out.url} : refusal(out.reasonCode, out.reason, out.status)
}

// ===== registering =====
const LIMITS = () => ({
    maxUpload: Number(conf("webapp-list/webapp[webroot]/web-protection/requests@max-upload")) || null,
    lfsMaxObject: Number(conf("git-server@lfs-max-object-size")) || null,
    gitMaxObject: Number(conf("git-server@max-object-size")) || null,
    pinnedMaxAge: conf("resource-facade@pinned-max-age") || "86400"
})
// What the size of an archive meets: the upload limit, an LFS object's, a drive's quota. Notes in words, none a refusal (the
// archive is already stored; the next replacement is what they bound).
function limitNotes(location, size, driveId) {
    const notes = []
    const limits = LIMITS()
    if (limits.maxUpload && size > limits.maxUpload)
        notes.push(
            say(
                "It is larger than an upload may be (${limit}, max-upload): a replacement is placed on the server, not uploaded",
                {
                    limit: bytesText(limits.maxUpload)
                }
            )
        )
    if (schemeOf(location) === "git" && limits.lfsMaxObject && size > limits.lfsMaxObject * 0.8)
        notes.push(
            say("It is near or over the largest Git LFS object (${limit}, git-server@lfs-max-object-size)", {
                limit: bytesText(limits.lfsMaxObject)
            })
        )
    if (driveId) {
        const drive = one(DRIVE, {driveId})
        // the quota only: the drive's use is its Files page's, not walked here on each view
        if (drive && drive.quotaBytes != null)
            notes.push(
                say("Its drive's quota is ${quota}: a replacement needs room for both revisions until the old one is collected", {
                    quota: bytesText(drive.quotaBytes)
                })
            )
    }
    return notes
}
const plainAttribution = html =>
    String(html || "")
        .replace(/<[^>]*>/g, " ")
        .replace(/&copy;/g, "©")
        .replace(/&amp;/g, "&")
        .replace(/\s+/g, " ")
        .trim()

// Whether a location on the server's disk is under the maps directory (component files never are)
function onDiskAllowed(location, scheme) {
    if (/(^|[\/])\.\.([\/]|$)/.test(location)) return false
    if (scheme === "runtime") return location.startsWith(DISK_ROOT)
    if (scheme === "file") {
        const root = String(ec.factory.getRuntimePath() || "").replace(/\/+$/, "") + "/maps/"
        return root.length > 6 && location.replace(/^file:(\/\/)?/i, "").startsWith(root)
    }
    return scheme !== "component"
}
// A file's size and modification time now, or null
const statOf = location =>
    quietly(() => {
        const ref = ec.resource.getLocationReference(location)
        return ref && ref.getExists() ? {size: Number(ref.getSize()), modified: millis(ref.getLastModified())} : null
    })

// register#ArchiveLayer
function register(p) {
    const refused = (reasonCode, reason, extra) => Object.assign({applied: false, reasonCode, reason}, extra || {})
    if (!canConfigure() || !canChangeConfiguration())
        return refused("PERMISSION_DENIED", L("Changing the maps configuration is not available to you"))
    const layerId = text(p.layerId)
    const layer = layerId ? ec.entity.find(LAYER).condition({layerId}).disableAuthz().one() : null
    if (!layer) return refused("RECORD_UNAVAILABLE", say("The map layer ${layerId} is not registered", {layerId}))
    if (!PMTILES.includes(text(layer.formatEnumId)))
        return refused("FORMAT_NOT_ARCHIVE", L("Only a PMTiles layer (vector or raster) is registered as an archive"))
    const location = text(p.resourceLocation) || text(layer.resourceLocation)
    if (!location) return refused("LOCATION_MISSING", L("Give the archive's location"))
    if (location.includes("#") || location.includes("?"))
        return refused("LOCATION_INVALID", L("The location is a file's, without # or ?: the revision is recorded here"))
    const scheme = schemeOf(location)
    if (!scheme)
        return refused("LOCATION_INVALID", L("The location is a resource location, such as dbresource://, git:// or file:"))
    if (!RANGED_SCHEMES.includes(scheme))
        return refused(
            "STORAGE_NOT_RANGED",
            say(
                "Storage ${scheme}:// is not known to read a part of a file without the rest, so every tile request would read the whole archive: keep it on disk, in a drive (dbresource://) or as a Git LFS object",
                {scheme}
            )
        )
    if (!onDiskAllowed(location, scheme))
        return refused(
            "DISK_ROOT_ONLY",
            say("An archive on the server's disk is placed under ${root} (the runtime's maps directory) and registered there", {
                root: DISK_ROOT
            })
        )
    // the other layers drawn from the same archive (the light and dark styles of one extract): those asked for, which must
    // be PMTiles layers of the same format, and every other one of that format that already names the location
    const format = text(layer.formatEnumId)
    const asked = Array.from(p.alsoLayerIds || [])
        .map(text)
        .filter(id => id && id !== layerId)
    const others = []
    for (const id of asked) {
        const row = ec.entity.find(LAYER).condition({layerId: id}).disableAuthz().one()
        if (!row) return refused("RECORD_UNAVAILABLE", say("The map layer ${layerId} is not registered", {layerId: id}))
        if (text(row.formatEnumId) !== format)
            return refused(
                "FORMAT_MISMATCH",
                say("The layer ${layerId} is not a PMTiles layer of the same format, so it cannot draw the same archive", {
                    layerId: id
                })
            )
        if (!others.some(other => other.layerId === row.layerId)) others.push(row)
    }
    for (const row of Array.from(
        ec.entity.find(LAYER).condition({resourceLocation: location, formatEnumId: format}).disableAuthz().limit(50).list()
    ))
        if (row.layerId !== layerId && !others.some(other => other.layerId === row.layerId)) others.push(row)
    const change = quietly(() => ec.resource.getLastChange(location))
    if (!change) return refused("SOURCE_GONE", say("There is no file at ${location}", {location}))
    if (!change.isFile) return refused("LOCATION_INVALID", say("${location} is a folder, not a file", {location}))
    const versionName = text(change.versionName)
    const pinned = versionName ? location + "#" + versionName : location
    let address = quietly(() => ec.resource.getContentAddress(pinned))
    let kind = address ? text(address.kind) : "file"
    let modified = null
    if (scheme === "git" && kind !== "git-lfs")
        return refused(
            "NOT_LFS",
            say(
                "The archive at ${location} is a git object, not a Git LFS object: a map archive in a git repository must be kept in LFS (its .gitattributes: *.pmtiles filter=lfs), since any other git object is read whole for every request",
                {location}
            )
        )
    const header = ec.service
        .sync()
        .name("org.moqui.maps.MapSourceServices.read#ArchiveHeader")
        .parameters({location: pinned})
        .disableAuthz()
        .call()
    if (ec.message.hasError()) return refused("HEADER_UNREAD", ec.message.getErrorsString())
    const problems = Array.from(header?.problems || []).map(text)
    if (!header?.found) return refused("SOURCE_GONE", problems[0] || say("There is no file at ${location}", {location}))
    if (problems.length)
        return refused("ARCHIVE_INVALID", say("A map cannot draw this archive: ${problems}", {problems: problems.join("; ")}), {
            problems
        })
    const head = header.header || {}
    const vector = text(layer.formatEnumId) === "MLF_PMTILES_VECTOR"
    if (vector ? !head.vector : !head.raster)
        return refused(
            "TILE_TYPE_MISMATCH",
            say("The archive's tiles are ${tileType}, and the layer is a PMTiles ${format} layer", {
                tileType: text(head.tileType),
                format: vector ? L("vector") : L("raster")
            })
        )
    if (!address) {
        // a file on disk, its header good: its SHA-256, read once now, is its tag, and its size and modification time,
        // the same before and after the read, tell a change in place later
        const before = statOf(location)
        const id = quietly(() => ec.resource.digest(location, "sha256"))
        const after = statOf(location)
        if (!before || !after || before.size !== after.size || before.modified !== after.modified)
            return refused("SOURCE_CHANGED", L("The archive changed while it was read: register it again"))
        address = {kind: "file", id, size: after.size, tag: "sha256-" + id}
        modified = after.modified
        kind = "file"
    }
    if (Number(header.size) !== Number(address.size))
        return refused("SOURCE_CHANGED", L("The archive changed while it was read: register it again"))
    const drive = driveOf(location)
    const metadata = header.metadata || {}
    const fields = {
        resourceLocation: location,
        sourceRevision: text(address.id),
        sourceTag: text(address.tag),
        sourceSize: num(address.size),
        sourceVersionName: versionName || null,
        sourceAddressKind: kind,
        sourceModifiedDate: modified === null ? null : new Date(modified),
        sourceRegisteredDate: ec.user.nowTimestamp,
        driveId: drive ? drive.driveId : null,
        minZoom: Number(head.minZoom),
        maxZoom: Number(head.maxZoom),
        minLongitude: Number(head.minLongitude),
        minLatitude: Number(head.minLatitude),
        maxLongitude: Number(head.maxLongitude),
        maxLatitude: Number(head.maxLatitude),
        disabled: "N"
    }
    // each layer's own: the archive's attribution and version where the layer has none
    // the layer chosen and those asked for are enabled; one that only names the location keeps its state
    const fieldsOf = row => {
        const own = Object.assign({layerId: row.layerId}, fields)
        if (row.layerId !== layerId && !asked.includes(String(row.layerId))) delete own.disabled
        if (!text(row.attribution) && text(metadata.attribution)) own.attribution = plainAttribution(metadata.attribution)
        if (!text(row.sourceEdition) && text(metadata.version)) own.sourceEdition = text(metadata.version).slice(0, 60)
        return own
    }
    const layerIds = [layerId].concat(others.map(row => String(row.layerId)))
    const previous = text(layer.sourceRevision)
    asCaller(() => {
        for (const row of [layer].concat(others)) ec.entity.makeValue(LAYER).setAll(fieldsOf(row)).update()
        // a profile drawn from it that still starts at the seed's whole-world view starts at the archive's center
        const centered = Array.from(ec.entity.find(PROFILE).condition("basemapLayerId", "in", layerIds).disableAuthz().list())
        for (const profile of centered) {
            const seedView = num(profile.centerLongitude) === 0 && num(profile.centerLatitude) === 20 && num(profile.zoom) === 2
            if (!seedView || profile.minLongitude != null) continue
            ec.entity
                .makeValue(PROFILE)
                .setAll({
                    profileId: profile.profileId,
                    centerLongitude: Number(head.centerLongitude),
                    centerLatitude: Number(head.centerLatitude),
                    zoom: Math.min(Math.max(Number(head.centerZoom) || Number(head.minZoom) || 0, 0), Number(head.maxZoom) || 14)
                })
                .update()
        }
    })
    const notes = limitNotes(location, Number(address.size), fields.driveId)
    if (!drive && scheme !== "git")
        notes.unshift(
            L(
                "It is outside every drive, so nothing but the layer guards it: every user who may see the layer reads the archive."
            )
        )
    const name = [layer]
        .concat(others)
        .map(row => (text(row.layerName) ? L(row.layerName) : row.layerId))
        .join(", ")
    return {
        applied: true,
        layerId,
        layerIds,
        revision: fields.sourceRevision,
        replaced: !!previous && previous !== fields.sourceRevision,
        size: fields.sourceSize,
        addressKind: kind,
        driveId: fields.driveId,
        header: head,
        metadata,
        notes,
        reason:
            previous && previous !== fields.sourceRevision
                ? say("${name}: the new revision is registered; a map that was drawing the old one loads again", {name})
                : say(
                      others.length
                          ? "${name}: their archive is registered (${size}, zoom ${min} to ${max})"
                          : "${name}: its archive is registered (${size}, zoom ${min} to ${max})",
                      {
                          name,
                          size: bytesText(fields.sourceSize),
                          min: fields.minZoom,
                          max: fields.maxZoom
                      }
                  )
    }
}

// ===== what is registered: Configuration › Archives and readiness =====
// The state of one archive layer now: {state (ready, warning, failed), reasonCode, message}
function stateOf(layer) {
    const location = text(layer.resourceLocation)
    if (!location) return {state: "warning", reasonCode: "LOCATION_MISSING", message: L("No archive location is set")}
    if (!text(layer.sourceRevision))
        return {state: "warning", reasonCode: "NOT_REGISTERED", message: L("Its archive is not registered yet: register it")}
    const scheme = schemeOf(location)
    if (!RANGED_SCHEMES.includes(scheme))
        return {
            state: "failed",
            reasonCode: "STORAGE_NOT_RANGED",
            message: say("Storage ${scheme}:// does not read an archive by range", {scheme})
        }
    const pinned = pinnedLocation(layer)
    let address = null
    try {
        address = quietly(() => ec.resource.getContentAddress(pinned))
    } catch (err) {
        address = null
    }
    if (scheme === "git" && (!address || text(address.kind) !== "git-lfs"))
        return {
            state: "failed",
            reasonCode: "NOT_LFS",
            message: L(
                "The archive is in a git repository but not a Git LFS object: every request would read it whole. Keep it in LFS and register it again"
            )
        }
    if (address && text(address.tag) !== text(layer.sourceTag))
        return {
            state: "failed",
            reasonCode: "SOURCE_CHANGED",
            message: L("The registered revision now reads as other content: register it again")
        }
    const stat = quietly(() => {
        const ref = ec.resource.getLocationReference(pinned)
        return ref && ref.getExists() ? {size: Number(ref.getSize()), modified: millis(ref.getLastModified())} : null
    })
    if (!address && !stat)
        return {
            state: "failed",
            reasonCode: "SOURCE_GONE",
            message: L("The registered revision is no longer there: register the archive again")
        }
    if (text(layer.sourceAddressKind) === "file") {
        if (!stat || stat.size !== num(layer.sourceSize) || !sameSecond(stat.modified, millis(layer.sourceModifiedDate)))
            return {
                state: "failed",
                reasonCode: "SOURCE_CHANGED",
                message: L("The archive file changed since it was registered: register it again")
            }
    }
    if (text(layer.sourceVersionName)) {
        const change = quietly(() => ec.resource.getLastChange(location))
        if (change && text(change.versionName) && text(change.versionName) !== text(layer.sourceVersionName))
            return {
                state: "warning",
                reasonCode: "NEWER_REVISION",
                message: L("A newer revision of the archive is stored; maps draw the registered one until it is registered again")
            }
    }
    return {state: "ready", reasonCode: "", message: L("Registered and read by range")}
}

// find#ArchiveLayers
function findArchives() {
    if (!canConfigure())
        return {applied: false, reasonCode: "PERMISSION_DENIED", reason: L("Maps configuration is not available to you")}
    const limits = LIMITS()
    const layers = Array.from(
        ec.entity
            .find(LAYER)
            .condition("formatEnumId", "in", PMTILES)
            .orderBy("layerName")
            .limit(MAX_ARCHIVES)
            .disableAuthz()
            .list()
    )
    const profiles = Array.from(
        ec.entity
            .find(PROFILE)
            .condition("basemapLayerId", "in", layers.map(row => row.layerId).concat(["-"]))
            .disableAuthz()
            .list()
    )
    const archiveList = layers.map(layer => {
        const state = stateOf(layer)
        const using = profiles.filter(profile => profile.basemapLayerId === layer.layerId)
        return {
            layerId: layer.layerId,
            layerName: text(layer.layerName) ? L(layer.layerName) : layer.layerId,
            formatEnumId: text(layer.formatEnumId),
            resourceLocation: text(layer.resourceLocation) || null,
            revision: text(layer.sourceRevision) || null,
            versionName: text(layer.sourceVersionName) || null,
            addressKind: text(layer.sourceAddressKind) || null,
            size: num(layer.sourceSize),
            sizeText: layer.sourceSize == null ? "" : bytesText(layer.sourceSize),
            zoomText: layer.minZoom == null ? "" : layer.minZoom + "–" + layer.maxZoom,
            driveId: text(layer.driveId) || null,
            registeredDate: layer.sourceRegisteredDate ?? null,
            enabled: layer.disabled !== "Y",
            state: state.state,
            reasonCode: state.reasonCode,
            message: state.message,
            notes: text(layer.sourceRevision)
                ? limitNotes(text(layer.resourceLocation), num(layer.sourceSize), text(layer.driveId))
                : [],
            profiles: using.map(profile => ({
                profileId: profile.profileId,
                profileName: text(profile.profileName) || profile.profileId,
                enabled: profile.disabled !== "Y"
            }))
        }
    })
    return {
        applied: true,
        archiveList,
        limits: Object.assign({}, limits, {
            maxUploadText: limits.maxUpload ? bytesText(limits.maxUpload) : "",
            lfsMaxObjectText: limits.lfsMaxObject ? bytesText(limits.lfsMaxObject) : "",
            gitMaxObjectText: limits.gitMaxObject ? bytesText(limits.gitMaxObject) : ""
        })
    }
}

// check#ArchiveReadiness: the maps.archives provider, for ADMIN_OVERVIEW_VIEW
function readiness(p) {
    if (text(p.providerKey) !== "maps.archives") throw new Error("Unknown Maps readiness check " + text(p.providerKey))
    if (!ec.user.hasPermission("ADMIN_OVERVIEW_VIEW")) {
        const error = new Error("PERMISSION_DENIED: ADMIN_OVERVIEW_VIEW is required")
        error.reason = "PERMISSION_DENIED"
        throw error
    }
    const layers = Array.from(
        ec.entity
            .find(LAYER)
            .condition("formatEnumId", "in", PMTILES)
            .orderBy("layerName")
            .limit(MAX_ARCHIVES)
            .disableAuthz()
            .list()
    )
    const checks = layers
        .filter(layer => text(layer.resourceLocation) && (layer.disabled !== "Y" || text(layer.sourceRevision)))
        .map(layer => {
            const state = stateOf(layer)
            const name = text(layer.layerName) ? L(layer.layerName) : layer.layerId
            return {
                checkKey: "maps.archive." + layer.layerId,
                title: say("Map archive ${name}", {name}),
                subject: name,
                state: state.state,
                reasonCode: state.reasonCode || null,
                message: state.message,
                fixRoute: "mapsConfiguration"
            }
        })
    if (!checks.length)
        checks.push({
            checkKey: "maps.archive.none",
            title: L("Map archives"),
            state: "ready",
            reasonCode: "NO_ARCHIVES",
            message: L("No map archive is registered: maps draw from a provider or the country outlines")
        })
    return {checks}
}

const operations = {sourceAccess, archiveTile, register, findArchives, readiness}
;({
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown map source operation " + op)
        return operations[op](parameters || {})
    },
    // for the effective configuration (MapConfig.js)
    servedUrl,
    tileUrl,
    SOURCE_PATH
})
