// Optional Overture data for Maps, org.moqui.maps.OvertureServices (plans/Maps-Overture.md, stages O1 and O2 of plans/Maps.md
// §4.3; docs/tools/Maps.md "Overture places and buildings", "Overture addresses"): what Maps › Configuration › Overture shows, the license
// acknowledgement, regions, an import started as a run of the OvertureImport ServiceJob (never in the request) and its run
// (read#OvertureTheme of the moqui-maps crate stages the rows or the archive; this checks and activates), activation,
// rollback and removal, the maps.overture readiness checks, and the Places search Browse's search asks after the postal
// codes and before the geocoder (searchPlaces: local rows only, no outside request).
//
// A region theme has an active run and the one before it (kept for rollback); the places of the active runs are searched,
// the address points of the active Addresses runs are the local geocoder's index (MapGeocoders.js, moqui-maps
// search#OvertureAddresses), and a Buildings run's archive is its region's layer. A run that fails, is cancelled or is superseded past the previous
// one has its rows and archive deleted. An import writes only moqui.maps.Overture* rows, its archive and its own layer:
// never a Mantle record, a Geo, a point or a record's location.
//
// The services are MAPS_ADMIN's, asked of the caller's own grant (OWN_GRANTS, as Geo data's); rows are written with
// authorization off as the services' own steps.
const THEME = "moqui.maps.OvertureTheme"
const REGION = "moqui.maps.OvertureRegion"
const REGION_THEME = "moqui.maps.OvertureRegionTheme"
const IMPORT = "moqui.maps.OvertureImport"
const PLACE = "moqui.maps.OverturePlace"
const ADDRESS = "moqui.maps.OvertureAddress"
const LAYER = "moqui.maps.MapLayer"
const PROFILE = "moqui.maps.MapProfile"
const PROFILE_LAYER = "moqui.maps.MapProfileLayer"
const S = "org.moqui.maps.OvertureServices."
const JOB = "OvertureImport"
const ARCHIVE_ROOT = "dbresource://moqui/maps/overture"
const IN_PROGRESS = ["OviPending", "OviRunning"]
const STATUS_TEXT = {
    OviPending: "Waiting to start",
    OviRunning: "Reading",
    OviReview: "Waiting for license review",
    OviActive: "Active",
    OviPrevious: "Previous (kept for rollback)",
    OviRolledBack: "Rolled back",
    OviSuperseded: "Superseded",
    OviFailed: "Failed",
    OviCancelled: "Cancelled",
    OviRemoved: "Removed",
    OviDiscarded: "Discarded"
}
// A run with fewer rows than this share of the active run's is refused unless forced (a release that lost most of a region
// is more likely incomplete than right)
const MIN_SHARE = 0.5
// What a source that names no license is held for review as
const UNKNOWN_LICENSE = "UNKNOWN"
// A run whose row has not changed for this long is taken as abandoned
const STALE_MS = 3600 * 1000
const DEFAULT_MAX_AREA = 2500
const MAX_RUNS = 50
const SEARCH_LIMIT = 20

const L = value => ec.l10n.localize(value)
const say = (value, values) => ec.resource.expand(L(value), "", values || {})
const text = value => (value == null ? "" : String(value).trim())
// a number of a parameter or a field: a BigDecimal parameter is a decimal, which says its number with toNumber()
const num = value =>
    value == null || value === "" ? null : typeof value?.toNumber === "function" ? value.toNumber() : Number(value)
const find = (entity, conditions) =>
    ec.entity
        .find(entity)
        .condition(conditions || {})
        .disableAuthz()
const one = (entity, conditions) => find(entity, conditions).one()
const rows = (entity, conditions) => Array.from(find(entity, conditions).list())
const stampText = value => (value ? ec.l10n.format(value, "yyyy-MM-dd HH:mm") : "")
const offline = () => text(ec.factory.getConfValue("maps@external-requests")) === "false"
const built = () => !!ec.factory.hasFeature("overture")
const asSystem = write => {
    const was = ec.artifactExecution.disableAuthz()
    try {
        return write()
    } finally {
        if (!was) ec.artifactExecution.enableAuthz()
    }
}
function refuse(code, message) {
    ec.message.addError(code + ": " + message)
    return {}
}
const licenseList = value => text(value).split(",").map(text).filter(Boolean)
const maxArea = () => {
    const value = Number(text(ec.factory.getConfValue("maps@overture-max-area")))
    return Number.isFinite(value) && value > 0 ? value : DEFAULT_MAX_AREA
}
// A box's area in km², near enough for a limit
function areaOf(region) {
    const [w, s, e, n] = [num(region.west), num(region.south), num(region.east), num(region.north)]
    const middle = (((s + n) / 2) * Math.PI) / 180
    return (e - w) * 111.32 * Math.cos(middle) * ((n - s) * 110.57)
}
function stale(run) {
    const at = run.lastProgressDate || run.startedDate
    return !!at && Date.now() - new Date(at).getTime() > STALE_MS
}
const userName = userId => {
    if (!userId) return ""
    const account = find("moqui.security.UserAccount", {userId}).useCache(true).one()
    return account ? text(account.userFullName) || text(account.username) : userId
}

// ===== a run's own rows and archive =====
// A run's address points, a page at a time, each page in a transaction of its own: a run of up to a million rows is removed
// within the transaction timeout, and a failure leaves the rest for the next removal
const DELETE_PAGE = 2000
function deleteAddresses(importId) {
    for (let page = 0; page < 1000; page++) {
        const ids = Array.from(
            ec.entity
                .find(ADDRESS)
                .condition({importId})
                .selectField("addressId")
                .orderBy("addressId")
                .limit(DELETE_PAGE)
                .disableAuthz()
                .list()
        ).map(row => row.addressId)
        if (!ids.length) return
        ec.transaction.runRequireNew(null, "Removing an Overture run's address points", () =>
            asSystem(() => ec.entity.find(ADDRESS).condition("addressId", "in", ids).disableAuthz().deleteAll())
        )
        if (ids.length < DELETE_PAGE) return
    }
}
function deleteRunData(run) {
    asSystem(() => ec.entity.find(PLACE).condition({importId: run.importId}).disableAuthz().deleteAll())
    if (run.themeId === "ADDRESSES" || !run.themeId) deleteAddresses(run.importId)
    const location = text(run.archiveLocation)
    if (location) {
        try {
            const reference = ec.resource.getLocationReference(location)
            if (reference && reference.exists) asSystem(() => reference.delete())
        } catch (error) {
            ec.logger.warn("Overture: could not delete the archive " + location + ": " + (error.message || error))
        }
    }
}
function setRun(importId, fields) {
    asSystem(() => ec.entity.makeValue(IMPORT).setAll(Object.assign({importId}, fields)).update())
}
// End run importId as statusId (failed or cancelled), its staged data deleted
function endRun(run, statusId, reasonCode, reason) {
    deleteRunData(run)
    setRun(run.importId, {
        statusId,
        reasonCode: reasonCode || null,
        failureText: reason ? String(reason).slice(0, 4000) : null,
        finishedDate: ec.user.nowTimestamp,
        cancelRequested: null
    })
    return {importId: run.importId, statusId, releaseVersion: run.releaseVersion, reason: reason || null}
}

// ===== Configuration › Overture =====
function themeRow(theme) {
    return {
        themeId: theme.themeId,
        themeName: L(text(theme.themeName) || theme.themeId),
        description: L(text(theme.description)),
        licenseName: text(theme.licenseName),
        licenseUrl: text(theme.licenseUrl),
        attribution: text(theme.attribution),
        obligations: L(text(theme.obligations)),
        licenses: licenseList(theme.licenses).join(", "),
        acknowledged: !!theme.licenseAckUserId,
        acknowledgedText: theme.licenseAckUserId
            ? say("Acknowledged by ${user} on ${date}", {
                  user: userName(theme.licenseAckUserId),
                  date: stampText(theme.licenseAckDate)
              })
            : L("Not acknowledged: nothing of this theme is imported until an administrator acknowledges its license")
    }
}
function runRow(run, regions, themes) {
    return {
        importId: run.importId,
        regionId: run.regionId,
        regionName: regions[run.regionId] || run.regionId,
        themeId: run.themeId,
        themeName: themes[run.themeId] || run.themeId,
        releaseVersion: text(run.releaseVersion),
        schemaVersion: text(run.schemaVersion),
        statusId: run.statusId,
        statusText: L(STATUS_TEXT[run.statusId] || run.statusId),
        filesRead: num(run.filesRead),
        rowGroupsRead: num(run.rowGroupsRead),
        rowGroupsSkipped: num(run.rowGroupsSkipped),
        bytesRead: num(run.bytesRead),
        rowsKept: num(run.rowsKept),
        rowsRejected: num(run.rowsRejected),
        newLicenses: text(run.newLicenses),
        failureText: text(run.failureText),
        tileCount: num(run.tileCount),
        countryCodes: text(run.countryCodes),
        changesText: changesText(run.changesJson),
        userName: userName(run.userId),
        startedText: stampText(run.startedDate),
        finishedText: stampText(run.finishedDate),
        inProgress: IN_PROGRESS.includes(run.statusId),
        review: run.statusId === "OviReview"
    }
}
// What an Addresses run changed against the run active before it, in words (by address, never by ID)
function changesText(json) {
    let changes = null
    try {
        changes = json ? JSON.parse(String(json)) : null
    } catch (error) {
        changes = null
    }
    if (!changes || typeof changes !== "object") return ""
    return say(
        "Against run ${comparedWith}: ${added} added, ${removed} removed, ${moved} moved, ${unchanged} unchanged addresses",
        {
            comparedWith: changes.comparedWith || "",
            added: Number(changes.added) || 0,
            removed: Number(changes.removed) || 0,
            moved: Number(changes.moved) || 0,
            unchanged: Number(changes.unchanged) || 0
        }
    )
}
function findData() {
    const themes = rows(THEME).filter(theme => theme.disabled !== "Y")
    const themeNames = Object.fromEntries(themes.map(theme => [theme.themeId, L(text(theme.themeName) || theme.themeId)]))
    const regionRows = rows(REGION).sort((a, b) => text(a.regionName).localeCompare(text(b.regionName)))
    const regionNames = Object.fromEntries(regionRows.map(region => [region.regionId, text(region.regionName)]))
    const runs = Array.from(find(IMPORT).orderBy("-importId").limit(MAX_RUNS).list())
    const regions = regionRows.map(region => {
        const regionThemes = rows(REGION_THEME, {regionId: region.regionId}).map(row => {
            const active = row.activeImportId ? one(IMPORT, {importId: row.activeImportId}) : null
            const previous = row.previousImportId ? one(IMPORT, {importId: row.previousImportId}) : null
            const last = find(IMPORT, {regionId: region.regionId, themeId: row.themeId}).orderBy("-importId").limit(1).one()
            return {
                themeId: row.themeId,
                themeName: themeNames[row.themeId] || row.themeId,
                activeImportId: row.activeImportId || null,
                activeRelease: active ? text(active.releaseVersion) : "",
                activeRows: active ? num(active.rowsKept) : null,
                previousImportId: row.previousImportId || null,
                previousRelease: previous ? text(previous.releaseVersion) : "",
                layerId: row.layerId || null,
                last: last ? runRow(last, regionNames, themeNames) : null
            }
        })
        return {
            regionId: region.regionId,
            regionName: text(region.regionName),
            west: num(region.west),
            south: num(region.south),
            east: num(region.east),
            north: num(region.north),
            box: [region.west, region.south, region.east, region.north].map(value => num(value).toFixed(4)).join(", "),
            area: Math.round(areaOf(region) * 10) / 10,
            releaseVersion: text(region.releaseVersion),
            profileId: region.profileId || null,
            themes: regionThemes
        }
    })
    return {
        themes: themes.map(themeRow),
        regions,
        runs: runs.map(run => runRow(run, regionNames, themeNames)),
        built: built(),
        offline: offline(),
        maxArea: maxArea()
    }
}

function readReleases() {
    if (!built()) return {error: ["OVERTURE_NOT_BUILT", L("This server is built without the overture feature")]}
    if (offline()) return {error: ["EXTERNAL_REQUESTS_OFF", L("Outside requests for maps are off (maps@external-requests)")]}
    const found = ec.service
        .sync()
        .name(S + "read#OvertureReleases")
        .disableAuthz()
        .call()
    if (ec.message.hasError()) {
        const errors = ec.message.getErrorsString()
        ec.message.clearErrors()
        return {error: ["CATALOG_UNREAD", errors]}
    }
    if (!found?.valid) return {error: [text(found?.reasonCode) || "CATALOG_UNREAD", text(found?.reason)]}
    const releases = Array.from(found.releases || []).map(release => ({version: text(release.version), latest: !!release.latest}))
    const latest = releases.find(release => release.latest) || releases[0]
    return {releases, latest: latest ? latest.version : null}
}
function findReleases() {
    const read = readReleases()
    if (read.error) return refuse(read.error[0], read.error[1])
    return read
}

function acknowledgeLicense(p) {
    const theme = one(THEME, {themeId: text(p.themeId)})
    if (!theme) return refuse("RECORD_UNAVAILABLE", say("There is no Overture theme ${themeId}", {themeId: text(p.themeId)}))
    const licenses = licenseList(theme.licenses)
    for (const license of Array.from(p.licenses || []).map(text)) {
        if (!/^[A-Za-z0-9][A-Za-z0-9.+-]{0,62}$/.test(license))
            return refuse("LICENSE_INVALID", say("${license} is not a license identifier", {license}))
        if (!licenses.includes(license)) licenses.push(license)
    }
    asSystem(() =>
        ec.entity
            .makeValue(THEME)
            .setAll({
                themeId: theme.themeId,
                licenses: licenses.join(","),
                licenseAckUserId: ec.user.userId,
                licenseAckDate: ec.user.nowTimestamp
            })
            .update()
    )
    return {}
}

function storeRegion(p) {
    const name = text(p.regionName)
    if (!name || name.length > 255) return refuse("NAME_INVALID", L("Give the region a name of up to 255 characters"))
    const region = {west: num(p.west), south: num(p.south), east: num(p.east), north: num(p.north)}
    const values = [region.west, region.south, region.east, region.north]
    if (
        values.some(value => value === null || !Number.isFinite(value)) ||
        region.west < -180 ||
        region.east > 180 ||
        region.south < -90 ||
        region.north > 90 ||
        region.west >= region.east ||
        region.south >= region.north
    )
        return refuse(
            "REGION_INVALID",
            L("The region is a box in degrees with west less than east (not across the antimeridian) and south less than north")
        )
    const area = areaOf(region)
    if (area > maxArea())
        return refuse(
            "REGION_TOO_LARGE",
            say("The region is about ${area} km², more than ${max} km² (maps@overture-max-area)", {
                area: Math.round(area),
                max: maxArea()
            })
        )
    const release = text(p.releaseVersion)
    if (release && !/^[0-9][0-9.-]{0,39}$/.test(release))
        return refuse("RELEASE_INVALID", say("${release} is not a release version", {release}))
    const profileId = text(p.profileId)
    if (profileId && !one(PROFILE, {profileId}))
        return refuse("RECORD_UNAVAILABLE", say("The map profile ${profileId} is not registered", {profileId}))
    const themeIds = Array.from(p.themeIds || [])
        .map(text)
        .filter(Boolean)
    for (const themeId of themeIds)
        if (!one(THEME, {themeId})) return refuse("RECORD_UNAVAILABLE", say("There is no Overture theme ${themeId}", {themeId}))
    let regionId = text(p.regionId)
    if (regionId && !one(REGION, {regionId}))
        return refuse("RECORD_UNAVAILABLE", say("There is no Overture region ${regionId}", {regionId}))
    asSystem(() => {
        const value = ec.entity
            .makeValue(REGION)
            .setAll(Object.assign({regionName: name, releaseVersion: release || null, profileId: profileId || null}, region))
        if (regionId) value.set("regionId", regionId).update()
        else {
            value.setSequencedIdPrimary()
            value.setAll({createdByUserId: ec.user.userId, createdDate: ec.user.nowTimestamp}).create()
            regionId = String(value.regionId)
        }
        for (const themeId of themeIds)
            if (!one(REGION_THEME, {regionId, themeId})) ec.entity.makeValue(REGION_THEME).setAll({regionId, themeId}).create()
    })
    return {regionId}
}

// ===== an import =====
// What would refuse a run of theme for region now, or null
function preflight(region, theme) {
    if (!built())
        return [
            "OVERTURE_NOT_BUILT",
            L("This server is built without the overture feature: Overture data cannot be imported here")
        ]
    if (offline())
        return [
            "EXTERNAL_REQUESTS_OFF",
            L("Outside requests for maps are off (maps@external-requests): Overture data is not downloaded")
        ]
    if (!theme.licenseAckUserId)
        return [
            "LICENSE_NOT_ACKNOWLEDGED",
            say("The license of ${theme} has not been acknowledged by an administrator", {theme: L(text(theme.themeName))})
        ]
    if (theme.disabled === "Y") return ["THEME_DISABLED", say("The theme ${theme} is disabled", {theme: theme.themeId})]
    const area = areaOf(region)
    if (area > maxArea())
        return [
            "REGION_TOO_LARGE",
            say("The region is about ${area} km², more than ${max} km² (maps@overture-max-area)", {
                area: Math.round(area),
                max: maxArea()
            })
        ]
    return null
}
// A run of the region theme in progress, an abandoned one failed on the way
function runningOf(regionId, themeId) {
    for (const run of rows(IMPORT, {regionId, themeId}).filter(run => IN_PROGRESS.includes(run.statusId))) {
        if (stale(run)) {
            endRun(run, "OviFailed", "ABANDONED", L("The run stopped with no word for an hour: taken as abandoned"))
            continue
        }
        return run
    }
    return null
}
function startImport(p) {
    const region = one(REGION, {regionId: text(p.regionId)})
    const theme = one(THEME, {themeId: text(p.themeId)})
    if (!region || !theme) return refuse("RECORD_UNAVAILABLE", L("Choose a region and a theme of it"))
    const refused = preflight(region, theme)
    if (refused) return refuse(refused[0], refused[1])
    if (runningOf(region.regionId, theme.themeId))
        return refuse("RUN_IN_PROGRESS", L("A run of this region and theme is in progress: cancel it or wait for it"))
    let release = text(p.releaseVersion) || text(region.releaseVersion)
    if (!release) {
        const read = readReleases()
        if (read.error) return refuse(read.error[0], read.error[1])
        release = read.latest
        if (!release) return refuse("CATALOG_UNREAD", L("The catalog lists no release"))
    }
    if (!/^[0-9][0-9.-]{0,39}$/.test(release))
        return refuse("RELEASE_INVALID", say("${release} is not a release version", {release}))
    const run = asSystem(() => {
        if (!one(REGION_THEME, {regionId: region.regionId, themeId: theme.themeId}))
            ec.entity.makeValue(REGION_THEME).setAll({regionId: region.regionId, themeId: theme.themeId}).create()
        const value = ec.entity.makeValue(IMPORT).setAll({
            regionId: region.regionId,
            themeId: theme.themeId,
            statusId: "OviPending",
            releaseVersion: release,
            userId: ec.user.userId,
            startedDate: ec.user.nowTimestamp,
            forced: p.force === true || p.force === "true" ? "Y" : "N"
        })
        value.setSequencedIdPrimary()
        value.create()
        return value
    })
    const jobRunId = ec.service
        .job(JOB)
        .parameters({importId: run.importId, force: !!(p.force === true || p.force === "true")})
        .run()
    if (!jobRunId) {
        endRun(run, "OviFailed", "JOB_NOT_STARTED", L("The import job did not start"))
        return refuse("JOB_NOT_STARTED", L("The import job did not start"))
    }
    setRun(run.importId, {jobRunId})
    return {importId: run.importId, jobRunId}
}

function runImport(p) {
    const run = one(IMPORT, {importId: text(p.importId)})
    if (!run) return refuse("RECORD_UNAVAILABLE", say("There is no Overture import ${importId}", {importId: text(p.importId)}))
    if (run.statusId !== "OviPending") return {importId: run.importId, statusId: run.statusId, releaseVersion: run.releaseVersion}
    const region = one(REGION, {regionId: run.regionId})
    const theme = one(THEME, {themeId: run.themeId})
    if (!region || !theme) return endRun(run, "OviFailed", "RECORD_UNAVAILABLE", L("Its region or theme is gone"))
    // the job runs as the administrator who started it: their own grant, asked again, as the rest of the preflight
    if (!ownGrant("start#OvertureImport", "all"))
        return endRun(
            run,
            "OviFailed",
            "PERMISSION_DENIED",
            L("Overture imports need your own Maps administration grant (MAPS_ADMIN)")
        )
    const refused = preflight(region, theme)
    if (refused) return endRun(run, "OviFailed", refused[0], refused[1])
    setRun(run.importId, {statusId: "OviRunning", lastProgressDate: ec.user.nowTimestamp})
    let read = null
    try {
        read = ec.service
            .sync()
            .name(S + "read#OvertureTheme")
            .parameters({importId: run.importId})
            .disableAuthz()
            .call()
    } catch (error) {
        read = {valid: false, reasonCode: "READ_FAILED", reason: String(error.message || error)}
    }
    if (ec.message.hasError()) {
        read = {valid: false, reasonCode: "READ_FAILED", reason: ec.message.getErrorsString()}
        ec.message.clearErrors()
    }
    const counts = {
        schemaVersion: text(read?.schemaVersion) || null,
        filesRead: num(read?.filesRead),
        rowGroupsRead: num(read?.rowGroupsRead),
        rowGroupsSkipped: num(read?.rowGroupsSkipped),
        bytesRead: num(read?.bytesRead),
        rowsRead: num(read?.rowsRead),
        rowsKept: num(read?.rowsKept),
        rowsRejected: num(read?.rowsRejected),
        licensesJson: JSON.stringify(Array.from(read?.licenses || [])),
        rejectionsJson: JSON.stringify(Array.from(read?.rejections || []).slice(0, 20)),
        countryCodes: text(read?.countryCodes).slice(0, 60) || null,
        changesJson: read?.changes ? JSON.stringify(read.changes) : null
    }
    setRun(run.importId, counts)
    const current = Object.assign({}, run, counts)
    if (!read?.valid)
        return endRun(
            current,
            read?.cancelled ? "OviCancelled" : "OviFailed",
            text(read?.reasonCode) || "READ_FAILED",
            text(read?.reason)
        )
    // Buildings: the archive, written through the resource facade where no drive holds it (the layer guards it)
    if (theme.themeId === "BUILDINGS") {
        const location = ARCHIVE_ROOT + "/" + region.regionId + "/buildings-" + run.importId + ".pmtiles"
        try {
            asSystem(() => ec.resource.putContent(location, read.bytes, "Overture buildings " + run.releaseVersion))
        } catch (error) {
            return endRun(current, "OviFailed", "ARCHIVE_NOT_WRITTEN", String(error.message || error))
        }
        const bounds = Array.from(read.bounds || [])
        const archive = {
            archiveLocation: location,
            archiveSize: num(read.size),
            tileCount: num(read.tileCount),
            maxZoom: num(read.maxZoom),
            minLongitude: num(bounds[0]),
            minLatitude: num(bounds[1]),
            maxLongitude: num(bounds[2]),
            maxLatitude: num(bounds[3])
        }
        setRun(run.importId, archive)
        Object.assign(current, archive)
    }
    // checked before anything in use changes
    const kept = Number(counts.rowsKept || 0)
    if (kept === 0) return endRun(current, "OviFailed", "NO_ROWS", L("The release has no row of this theme in the region"))
    const regionTheme = one(REGION_THEME, {regionId: run.regionId, themeId: run.themeId})
    const active = regionTheme?.activeImportId ? one(IMPORT, {importId: regionTheme.activeImportId}) : null
    const force = p.force === true || p.force === "true"
    if (active && !force && kept < MIN_SHARE * (num(active.rowsKept) || 0))
        return endRun(
            current,
            "OviFailed",
            "FEWER_ROWS",
            say("The release has ${kept} rows in the region where the active one has ${active}: refused unless forced", {
                kept,
                active: num(active.rowsKept) || 0
            })
        )
    // the run as it is now: one cancelled, or taken for abandoned and ended, while it read is not made active
    const now = one(IMPORT, {importId: run.importId})
    if (now?.statusId !== "OviRunning" || now?.cancelRequested === "Y") {
        if (now?.statusId === "OviRunning") return endRun(current, "OviCancelled", "CANCELLED", L("The import was cancelled"))
        deleteRunData(current)
        return {
            importId: run.importId,
            statusId: now?.statusId || "OviFailed",
            releaseVersion: run.releaseVersion,
            reason: now?.failureText || null
        }
    }
    const covered = licenseList(theme.licenses)
    // a source that names no license is of a license not known: the run waits for review, as for a new one
    const found = Array.from(read.licenses || []).map(entry => text(entry.license) || UNKNOWN_LICENSE)
    const newLicenses = Array.from(new Set(found.filter(license => !covered.includes(license))))
    if (newLicenses.length) {
        setRun(run.importId, {statusId: "OviReview", newLicenses: newLicenses.join(","), finishedDate: ec.user.nowTimestamp})
        return {
            importId: run.importId,
            statusId: "OviReview",
            releaseVersion: run.releaseVersion,
            rowsKept: kept,
            reason: say("Sources under licenses not yet acknowledged: ${licenses}", {licenses: newLicenses.join(", ")})
        }
    }
    const activated = activate(Object.assign(current, {finishedDate: ec.user.nowTimestamp}), force)
    if (activated.error) return endRun(current, "OviFailed", activated.error[0], activated.error[1])
    return {importId: run.importId, statusId: "OviActive", releaseVersion: run.releaseVersion, rowsKept: kept}
}

// Point the region's Buildings layer at archive location (registered as the revision maps read), made when not there,
// added to the region's profile; throws when the archive does not register
function pointLayer(region, theme, regionTheme, run) {
    const layerId = text(regionTheme.layerId) || ("OVT_BLD_" + region.regionId).slice(0, 40)
    asSystem(() => {
        if (!one(LAYER, {layerId}))
            ec.entity
                .makeValue(LAYER)
                .setAll({
                    layerId,
                    layerName: say("Overture buildings: ${region}", {region: text(region.regionName)}).slice(0, 255),
                    description: L("Building footprints of an Overture release for one region; not legal property boundaries"),
                    formatEnumId: "MLF_PMTILES_VECTOR",
                    attribution: text(theme.attribution),
                    disabled: "N"
                })
                .create()
        ec.entity
            .makeValue(LAYER)
            .setAll({
                layerId,
                resourceLocation: run.archiveLocation,
                sourceEdition: text(run.releaseVersion).slice(0, 60),
                attribution: text(theme.attribution)
            })
            .update()
    })
    const registered = ec.service
        .sync()
        .name("org.moqui.maps.MapSourceServices.register#ArchiveLayer")
        .parameters({layerId, resourceLocation: run.archiveLocation})
        .call()
    if (ec.message.hasError()) throw new Error(ec.message.getErrorsString())
    if (!registered?.applied) throw new Error(text(registered?.reason) || "the archive did not register")
    const profileId = text(region.profileId)
    if (profileId && one(PROFILE, {profileId}) && !one(PROFILE_LAYER, {profileId, layerId})) {
        const count = Number(ec.entity.find(PROFILE_LAYER).condition({profileId}).disableAuthz().count())
        asSystem(() =>
            ec.entity
                .makeValue(PROFILE_LAYER)
                .setAll({profileId, layerId, sequenceNum: (count + 1) * 10, defaultVisible: "Y"})
                .create()
        )
    }
    return layerId
}

// Make run active for its region theme in one transaction, then remove what is older than the previous run. Answers
// {error: [code, reason]} when it could not.
function activate(run, force) {
    const region = one(REGION, {regionId: run.regionId})
    const theme = one(THEME, {themeId: run.themeId})
    let superseded = []
    try {
        ec.transaction.runRequireNew(null, "Could not activate the Overture import", () => {
            const regionTheme = find(REGION_THEME, {regionId: run.regionId, themeId: run.themeId}).forUpdate(true).one()
            if (!regionTheme) throw new Error("the region no longer imports this theme")
            const fields = {regionId: run.regionId, themeId: run.themeId}
            if (theme.themeId === "BUILDINGS") fields.layerId = pointLayer(region, theme, regionTheme, run)
            const active = text(regionTheme.activeImportId)
            const previous = text(regionTheme.previousImportId)
            if (previous && previous !== run.importId) superseded.push(previous)
            // other runs of the region theme that wait for review are of releases passed over now
            for (const waiting of rows(IMPORT, {regionId: run.regionId, themeId: run.themeId, statusId: "OviReview"}))
                if (waiting.importId !== run.importId) superseded.push(waiting.importId)
            if (active && active !== run.importId)
                asSystem(() => ec.entity.makeValue(IMPORT).setAll({importId: active, statusId: "OviPrevious"}).update())
            fields.previousImportId = active && active !== run.importId ? active : null
            fields.activeImportId = run.importId
            asSystem(() => {
                ec.entity.makeValue(REGION_THEME).setAll(fields).update()
                ec.entity
                    .makeValue(IMPORT)
                    .setAll({
                        importId: run.importId,
                        statusId: "OviActive",
                        activatedDate: ec.user.nowTimestamp,
                        finishedDate: run.finishedDate || ec.user.nowTimestamp,
                        forced: force ? "Y" : "N",
                        newLicenses: null
                    })
                    .update()
                for (const importId of superseded)
                    ec.entity.makeValue(IMPORT).setAll({importId, statusId: "OviSuperseded"}).update()
            })
        })
    } catch (error) {
        superseded = []
        return {error: ["ACTIVATION_FAILED", String(error.message || error)]}
    }
    // the runs older than the previous one: their rows and archives, each failure logged and the rest gone on with
    for (const importId of superseded) {
        try {
            const old = one(IMPORT, {importId})
            if (old) deleteRunData(old)
        } catch (error) {
            ec.logger.error("Overture: could not remove the data of superseded run " + importId + ": " + (error.message || error))
        }
    }
    return {}
}

function activateImport(p) {
    const run = one(IMPORT, {importId: text(p.importId)})
    if (!run) return refuse("RECORD_UNAVAILABLE", say("There is no Overture import ${importId}", {importId: text(p.importId)}))
    if (run.statusId !== "OviReview")
        return refuse("NOT_IN_REVIEW", L("Only a run that waits for license review is activated here"))
    const theme = one(THEME, {themeId: run.themeId})
    const covered = licenseList(theme?.licenses)
    const missing = licenseList(run.newLicenses).filter(license => !covered.includes(license))
    if (missing.length)
        return refuse(
            "LICENSE_NOT_ACKNOWLEDGED",
            say("Acknowledge the licenses ${licenses} of ${theme} first", {
                licenses: missing.join(", "),
                theme: L(text(theme?.themeName))
            })
        )
    const regionTheme = one(REGION_THEME, {regionId: run.regionId, themeId: run.themeId})
    const active = regionTheme?.activeImportId ? one(IMPORT, {importId: regionTheme.activeImportId}) : null
    if (active && runOrder(run) < runOrder(active))
        return refuse(
            "OLDER_THAN_ACTIVE",
            say("Run ${importId} started before the active one (${active}): discard it, or import again", {
                importId: run.importId,
                active: active.importId
            })
        )
    const activated = activate(run, p.force === true || p.force === "true" || run.forced === "Y")
    if (activated.error) return refuse(activated.error[0], activated.error[1])
    return {}
}

// The order runs were made in: their sequenced IDs
const runOrder = run => {
    const order = Number(run.importId)
    return Number.isFinite(order) ? order : 0
}

// Discard a run that waits for review: its staged rows and archive are deleted, and the active release stays
function discardImport(p) {
    const run = one(IMPORT, {importId: text(p.importId)})
    if (!run || run.statusId !== "OviReview")
        return refuse("NOT_IN_REVIEW", L("Only a run that waits for license review is discarded"))
    deleteRunData(run)
    setRun(run.importId, {statusId: "OviDiscarded", finishedDate: ec.user.nowTimestamp})
    return {}
}

function cancelImport(p) {
    const run = one(IMPORT, {importId: text(p.importId)})
    if (!run || !IN_PROGRESS.includes(run.statusId)) return refuse("NOT_IN_PROGRESS", L("That run is not in progress"))
    if (run.statusId === "OviPending" || stale(run)) {
        endRun(
            run,
            run.statusId === "OviPending" ? "OviCancelled" : "OviFailed",
            "CANCELLED",
            L("Cancelled before it read anything")
        )
        return {}
    }
    setRun(run.importId, {cancelRequested: "Y"})
    return {}
}

function rollbackTheme(p) {
    const regionId = text(p.regionId)
    const themeId = text(p.themeId)
    const regionTheme = one(REGION_THEME, {regionId, themeId})
    if (!regionTheme) return refuse("RECORD_UNAVAILABLE", L("That region does not import that theme"))
    if (runningOf(regionId, themeId)) return refuse("RUN_IN_PROGRESS", L("A run of this region and theme is in progress"))
    const previous = regionTheme.previousImportId ? one(IMPORT, {importId: regionTheme.previousImportId}) : null
    if (!previous || previous.statusId !== "OviPrevious")
        return refuse("NOTHING_TO_ROLL_BACK", L("There is no previous release kept for this region and theme"))
    const region = one(REGION, {regionId})
    const theme = one(THEME, {themeId})
    const rolledBack = text(regionTheme.activeImportId)
    try {
        ec.transaction.runRequireNew(null, "Could not roll back the Overture import", () => {
            const fields = {regionId, themeId, activeImportId: previous.importId, previousImportId: null}
            if (themeId === "BUILDINGS") fields.layerId = pointLayer(region, theme, regionTheme, previous)
            asSystem(() => {
                ec.entity.makeValue(REGION_THEME).setAll(fields).update()
                ec.entity
                    .makeValue(IMPORT)
                    .setAll({importId: previous.importId, statusId: "OviActive", activatedDate: ec.user.nowTimestamp})
                    .update()
                if (rolledBack) ec.entity.makeValue(IMPORT).setAll({importId: rolledBack, statusId: "OviRolledBack"}).update()
            })
        })
    } catch (error) {
        return refuse("ROLLBACK_FAILED", String(error.message || error))
    }
    if (rolledBack) {
        const old = one(IMPORT, {importId: rolledBack})
        if (old) deleteRunData(old)
    }
    return {}
}

function removeTheme(p) {
    const regionId = text(p.regionId)
    const region = one(REGION, {regionId})
    if (!region) return refuse("RECORD_UNAVAILABLE", say("There is no Overture region ${regionId}", {regionId}))
    const themeId = text(p.themeId)
    const regionThemes = rows(REGION_THEME, {regionId}).filter(row => !themeId || row.themeId === themeId)
    for (const row of regionThemes)
        if (runningOf(regionId, row.themeId))
            return refuse("RUN_IN_PROGRESS", L("A run of this region is in progress: cancel it first"))
    for (const row of regionThemes) {
        for (const run of rows(IMPORT, {regionId, themeId: row.themeId})) {
            deleteRunData(run)
            if (!["OviFailed", "OviCancelled", "OviRemoved"].includes(run.statusId))
                setRun(run.importId, {statusId: "OviRemoved"})
        }
        asSystem(() => {
            const layerId = text(row.layerId)
            if (layerId) {
                ec.entity.find(PROFILE_LAYER).condition({layerId}).disableAuthz().deleteAll()
                ec.entity.find(LAYER).condition({layerId}).disableAuthz().deleteAll()
            }
            ec.entity.makeValue(REGION_THEME).setAll({regionId, themeId: row.themeId}).delete()
        })
    }
    if (!themeId) asSystem(() => ec.entity.makeValue(REGION).setAll({regionId}).delete())
    return {}
}

// ===== readiness =====
const check = (checkKey, title, state, reasonCode, message, extra) =>
    Object.assign({checkKey, title, state, reasonCode, message, subject: "Instance"}, extra || {})
function readiness(p) {
    if (text(p.providerKey) !== "maps.overture") throw new Error("Unknown Overture readiness check " + text(p.providerKey))
    if (!ec.user.hasPermission("ADMIN_OVERVIEW_VIEW")) {
        const error = new Error("PERMISSION_DENIED: ADMIN_OVERVIEW_VIEW is required")
        error.reason = "PERMISSION_DENIED"
        throw error
    }
    const fix = {fixRoute: "mapsConfiguration"}
    const regionThemes = rows(REGION_THEME)
    if (!regionThemes.length)
        return {checks: [check("maps.overture.regions", "Overture data", "ready", null, L("No Overture region is imported"))]}
    const checks = []
    if (!built())
        checks.push(
            check(
                "maps.overture.built",
                "Overture data",
                "warning",
                "NOT_BUILT",
                L(
                    "Overture regions are set up, but this server is built without the overture feature: they cannot be imported or refreshed here"
                ),
                fix
            )
        )
    const regions = Object.fromEntries(rows(REGION).map(region => [region.regionId, text(region.regionName)]))
    const review = []
    const failed = []
    const active = []
    for (const row of regionThemes) {
        const last = find(IMPORT, {regionId: row.regionId, themeId: row.themeId}).orderBy("-importId").limit(1).one()
        const name = (regions[row.regionId] || row.regionId) + " (" + row.themeId + ")"
        if (last?.statusId === "OviReview") review.push(name)
        if (last?.statusId === "OviFailed") failed.push(name)
        const current = row.activeImportId ? one(IMPORT, {importId: row.activeImportId}) : null
        if (current) active.push(name + " " + text(current.releaseVersion))
    }
    checks.push(
        review.length
            ? check(
                  "maps.overture.review",
                  "Overture licenses",
                  "warning",
                  "REVIEW",
                  say("A run waits for license review: ${names}", {names: review.join(", ")}),
                  fix
              )
            : check("maps.overture.review", "Overture licenses", "ready", null, L("No Overture run waits for license review"))
    )
    checks.push(
        failed.length
            ? check(
                  "maps.overture.imports",
                  "Overture imports",
                  "failed",
                  "IMPORT_FAILED",
                  say("The last import did not complete for: ${names}", {names: failed.join(", ")}),
                  fix
              )
            : check(
                  "maps.overture.imports",
                  "Overture imports",
                  "ready",
                  null,
                  active.length ? say("Active: ${names}", {names: active.join(", ")}) : L("No Overture run is active yet")
              )
    )
    return {checks}
}

// ===== Browse's search =====
// The places of the active Places runs whose name or category has the text, the names that start with it first, then those
// nearest the view's middle (extent: west, south, east, north); each with its category, source datasets and licenses and
// the theme's attribution. Local rows only: no outside request.
function searchPlaces(p) {
    const term = text(p.q)
        .toLowerCase()
        .replace(/[%_\\]/g, " ")
        .trim()
    const out = {results: [], attribution: null}
    if (term.length < 2) return out
    const activeIds = rows(REGION_THEME, {themeId: "PLACES"})
        .map(row => text(row.activeImportId))
        .filter(Boolean)
    if (!activeIds.length) return out
    const theme = one(THEME, {themeId: "PLACES"})
    const cf = ec.entity.conditionFactory
    // the view's box (west, south, east, north), when given and not across the antimeridian, bounds the search
    const extent = text(p.extent).split(",").map(Number)
    const box =
        extent.length === 4 && extent.every(Number.isFinite) && extent[0] < extent[2] && extent[1] < extent[3] ? extent : null
    const query = (condition, limit) => {
        let find = ec.entity.find(PLACE).condition("importId", "in", activeIds).condition(condition)
        if (box)
            find = find.condition("longitude", "between", [box[0], box[2]]).condition("latitude", "between", [box[1], box[3]])
        return Array.from(find.orderBy("nameSearch").limit(limit).disableAuthz().list())
    }
    // first the names that start with the text (the importId and nameSearch index), then those that have it anywhere or
    // whose category does, each query bounded, merged and ranked
    const prefixed = query(cf.makeCondition("nameSearch", "like", term + "%"), 50)
    const containing = query(
        cf.makeCondition(
            cf.makeCondition("nameSearch", "like", "%" + term + "%"),
            "or",
            cf.makeCondition("basicCategory", "like", "%" + term.replace(/\s+/g, "_") + "%")
        ),
        150
    )
    const seen = new Set()
    const found = prefixed.concat(containing).filter(row => !seen.has(row.placeId) && seen.add(row.placeId))
    const middle = box ? [(box[0] + box[2]) / 2, (box[1] + box[3]) / 2] : null
    const distance = row => (middle ? Math.hypot(num(row.longitude) - middle[0], num(row.latitude) - middle[1]) : 0)
    found.sort((a, b) => {
        const startsA = text(a.nameSearch).startsWith(term) ? 0 : 1
        const startsB = text(b.nameSearch).startsWith(term) ? 0 : 1
        return startsA - startsB || distance(a) - distance(b) || text(a.placeName).localeCompare(text(b.placeName))
    })
    const releases = {}
    for (const importId of activeIds) releases[importId] = text(one(IMPORT, {importId})?.releaseVersion)
    const limit = Math.min(Number(p.limit) || SEARCH_LIMIT, 50)
    out.results = found.slice(0, limit).map(row => {
        const category = text(row.basicCategory).replace(/_/g, " ")
        const sources = [text(row.sourceDatasets), text(row.sourceLicenses) ? "(" + text(row.sourceLicenses) + ")" : ""]
            .filter(Boolean)
            .join(" ")
        return {
            kind: "overture",
            id: "overture:" + row.placeId,
            label: text(row.placeName),
            lon: num(row.longitude),
            lat: num(row.latitude),
            category,
            sourceText: [category, text(row.locality), "Overture Maps" + (sources ? ": " + sources : "")]
                .filter(Boolean)
                .join(" · "),
            precisionText: L("The place's own point (Overture)"),
            gersId: text(row.gersId),
            release: releases[row.importId] || ""
        }
    })
    out.attribution = theme ? text(theme.attribution) : null
    return out
}

const operations = {
    findData,
    findReleases,
    acknowledgeLicense,
    storeRegion,
    startImport,
    runImport,
    cancelImport,
    activateImport,
    discardImport,
    rollbackTheme,
    removeTheme,
    readiness,
    searchPlaces
}
// The services an operation runs for, asked of the caller's own grants (isPermittedOwn), as Geo data's (plans/Maps.md §8):
// runImport asks its own of start#OvertureImport, readiness ADMIN_OVERVIEW_VIEW, and searchPlaces is Browse's (public
// reference data, local rows only).
// Whether the caller's own grants allow service S + name. With authorization turned off by the caller this answers yes;
// the run's read (read#OvertureTheme, in Rust) asks again with authorization required.
const ownGrant = (name, action) => !!ec.artifactExecution.isPermittedOwn(S + name, "AT_SERVICE", action)
const OWN_GRANTS = {
    findData: ["find#OvertureData", "view"],
    findReleases: ["find#OvertureReleases", "view"],
    acknowledgeLicense: ["acknowledge#OvertureLicense", "all"],
    storeRegion: ["store#OvertureRegion", "all"],
    startImport: ["start#OvertureImport", "all"],
    cancelImport: ["cancel#OvertureImport", "all"],
    activateImport: ["activate#OvertureImport", "all"],
    discardImport: ["discard#OvertureImport", "all"],
    rollbackTheme: ["rollback#OvertureTheme", "all"],
    removeTheme: ["remove#OvertureTheme", "all"]
}
;({
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown Overture operation " + op)
        const grant = OWN_GRANTS[op]
        if (grant && !ownGrant(grant[0], grant[1]))
            return refuse("PERMISSION_DENIED", L("Overture data needs your own Maps administration grant (MAPS_ADMIN)"))
        return operations[op](parameters || {})
    }
})
