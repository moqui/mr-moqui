// Offline map packages (plans/Maps-Offline.md §2, OF1; org.moqui.maps.MapSourceServices, MapPackages.js, tools
// screen/Maps/Offline.xml package): a package is cut from a small fixture archive (MapsArchiveLib's tiles) and built by
// the MapPackageBuild job from what its owner may see, a basemap layer they may not see left out; its zip is served to
// its owner by range with its SHA-256 as the strong ETag and refused another user, and a stale revision is 409; its
// manifest lists every member with its size and SHA-256 and names no URL of this server; a package past
// maps@package-max-bytes is refused at its estimate; a cancel ends a build cancelled, and an archive registered again
// since the build was asked fails it; the Offline page renders once in qvt and once in the terminal (its own template);
// the hourly clean removes an expired package's file and a deleted one's. The cut's bounds, the antimeridian and the writer are moqui-tools' and moqui-lib's
// Rust tests.
// Run: ./moq test framework --spec MapsPackages
const S = "org.moqui.maps.MapSourceServices."
const OWNER = "MAPS_PKG_OWNER"
const OTHER = "MAPS_PKG_OTHER"
const VISIBLE = "MAPS_PKG_VISIBLE"
const HIDDEN = "MAPS_PKG_HIDDEN"
const PROFILE = "MAPS_PKG_PROFILE"
const SECRET = "MAPS_PKG_SPEC_SECRET"
const FAKE_RUN = "MAPS_PKG_FAKE_RUN"
const DIR = "runtime://maps/maps-packages-spec"
const VISIBLE_LOCATION = DIR + "/visible.pmtiles"
const HIDDEN_LOCATION = DIR + "/hidden.pmtiles"
const UNZIPPED = "runtime://tmp/maps-packages-spec"
const ROOT = "runtime://tmp/map-packages"
const usernameOf = userId => userId.toLowerCase().replace(/_/g, ".")
// a region of the north-east quarter, zooms 0 to 3
const REGION = {west: 10, south: 10, east: 80, north: 60, minZoom: 0, maxZoom: 3}

const lib = ec.resource.script("component://framework/test/MapsArchiveLib.js")
// every tile of zooms 0 to 4 as ASCII text naming it, an uncompressed archive
const archive = revision =>
    lib.archive(
        lib.asciiTiles(4, revision).map(tile => ({tileId: tile.tileId, bytes: tile.bytes})),
        {minZoom: 0, maxZoom: 4, metadata: {name: "spec", attribution: "Spec data", vector_layers: [{id: "earth"}], spec: {revision}}}
    ).bytes
const make = (entity, fields) => ec.entity.makeValue(entity).setAll(fields).createOrUpdate()
const packageRow = packageId =>
    ec.entity.find("moqui.maps.MapPackage").condition("packageId", packageId || "_NA_").disableAuthz().useCache(false).one()
function invoke(name, params = {}) {
    try {
        const result = ec.service.sync().name(name).parameters(params).call() || {}
        const error = ec.message.hasError() ? ec.message.getErrorsString() : ""
        ec.message.clearErrors()
        return Object.assign({}, result, {error})
    } catch (e) {
        ec.message.clearErrors()
        return {error: String(e.message || e)}
    }
}
// as the user with authorization on (john.doe, an administrator, when none)
function as(userId, action) {
    ec.user.internalLoginUser(userId ? usernameOf(userId) : "john.doe")
    ec.message.clearErrors()
    ec.artifactExecution.enableAuthz()
    try {
        return action()
    } finally {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
    }
}
// signed in as the user with authorization off: the job's run, which no group is granted
function asSignedIn(userId, action) {
    ec.user.internalLoginUser(usernameOf(userId))
    try {
        return action()
    } finally {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
    }
}
// the package transition as the user, with a Range header when given
function download(userId, packageId, revision, range) {
    return as(userId, () => {
        let test = ec.screen.makeTest().rootScreen("component://webroot/screen/webroot.xml")
        if (range) test = test.header("Range", range)
        const res = test.render("apps/maps/Offline/package", {packageId, revision: String(revision)}, "get")
        return {
            status: res.status,
            etag: res.header("ETag"),
            contentRange: res.header("Content-Range"),
            cacheControl: String(res.header("Cache-Control") || ""),
            length: String(res.output || "").length
        }
    })
}
// a package row of the owner's made here, as create#Package makes one, with no job started
function packageOf(packageId, fields) {
    make(
        "moqui.maps.MapPackage",
        Object.assign(
            {
                packageId,
                ownerUserId: OWNER,
                name: "Spec " + packageId,
                profileId: PROFILE,
                basemapLayerIds: VISIBLE,
                includePlaces: "N",
                includeOverlays: "N",
                includePostal: "N",
                includeOverture: "N",
                statusId: "MapPkgPending",
                progress: 0,
                cancelRequested: "N",
                createdDate: ec.user.nowTimestamp
            },
            REGION,
            fields
        )
    )
}
const removeTree = location => {
    const ref = ec.resource.getLocationReference(location)
    if (!ref.getExists()) return
    if (ref.isDirectory()) for (const child of Array.from(ref.getDirectoryEntries() || [])) removeTree(child.location)
    ref.delete()
}
// The members of a zip whose entries are all stored (as a package's are), each written under target as a device unpacks
// it, read from the central directory: their paths
function unzipStored(location, target) {
    const bytes = ec.resource.getLocationReference(location).openStream().readAllBytes()
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
    let end = bytes.length - 22
    while (end >= 0 && view.getUint32(end, true) !== 0x06054b50) end--
    if (end < 0) throw new Error(location + " is not a zip")
    const count = view.getUint16(end + 10, true)
    let at = view.getUint32(end + 16, true)
    const paths = []
    for (let index = 0; index < count; index++) {
        const nameLength = view.getUint16(at + 28, true)
        const name = String.fromCharCode(...bytes.subarray(at + 46, at + 46 + nameLength))
        if (view.getUint16(at + 10, true) !== 0) throw new Error(name + " is not stored")
        const size = view.getUint32(at + 20, true)
        const local = view.getUint32(at + 42, true)
        const dataAt = local + 30 + view.getUint16(local + 26, true) + view.getUint16(local + 28, true)
        lib.put(target + "/" + name, bytes.slice(dataAt, dataAt + size))
        paths.push(name)
        at += 46 + nameLength + view.getUint16(at + 30, true) + view.getUint16(at + 32, true)
    }
    return paths
}
const buildFiles = packageId => {
    const ref = ec.resource.getLocationReference(ROOT)
    if (!ref.getExists()) return []
    return Array.from(ref.getDirectoryEntries() || [])
        .map(child => String(child.fileName))
        .filter(name => name.startsWith(packageId + "-"))
}

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        for (const userId of [OWNER, OTHER]) {
            make("moqui.security.UserAccount", {userId, username: usernameOf(userId), userFullName: "Maps packages " + userId})
            make("moqui.security.UserGroupMember", {userGroupId: "MAPS_USERS", userId, fromDate: ec.user.nowTimestamp})
        }
        lib.put(VISIBLE_LOCATION, archive(1))
        lib.put(HIDDEN_LOCATION, archive(2))
        for (const [layerId, extra] of [
            [VISIBLE, {}],
            [HIDDEN, {viewPermission: SECRET}]
        ])
            make("moqui.maps.MapLayer", Object.assign({layerId, layerName: "Spec " + layerId, formatEnumId: "MLF_PMTILES_VECTOR",
                isBasemap: "Y", attribution: "Spec data", disabled: "Y"}, extra))
        // registered by an administrator: the revisions a package pins
        for (const [layerId, resourceLocation] of [
            [VISIBLE, VISIBLE_LOCATION],
            [HIDDEN, HIDDEN_LOCATION]
        ]) {
            const out = as(null, () => invoke(S + "register#ArchiveLayer", {layerId, resourceLocation}))
            if (!out.applied) throw new Error("MapsPackages setup: " + layerId + " not registered: " + (out.reason || out.error))
        }
        make("moqui.maps.MapProfile", {profileId: PROFILE, profileName: "Spec packages", basemapLayerId: VISIBLE, externalAccess: "N",
            allowedOverrides: "basemap", centerLongitude: 40, centerLatitude: 30, zoom: 3, disabled: "N"})
        ec.message.clearErrors()
    },
    cleanupSpec() {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        const rows = Array.from(ec.entity.find("moqui.maps.MapPackage").condition("ownerUserId", "in", [OWNER, OTHER]).list())
        for (const row of rows) {
            for (const name of buildFiles(String(row.packageId)))
                try {
                    removeTree(ROOT + "/" + name)
                } catch (e) {
                    ec.logger.warn("MapsPackages cleanup: " + name + " not deleted")
                }
            ec.entity.find("moqui.maps.MapPackageDownload").condition("packageId", row.packageId).deleteAll()
        }
        const del = (entity, field, values) => ec.entity.find(entity).condition(field, "in", values).deleteAll()
        del("moqui.maps.MapPackage", "ownerUserId", [OWNER, OTHER])
        del("moqui.service.job.ServiceJobRun", "jobRunId", [FAKE_RUN])
        del("moqui.maps.MapProfile", "profileId", [PROFILE])
        del("moqui.maps.MapLayer", "layerId", [VISIBLE, HIDDEN])
        for (const location of [VISIBLE_LOCATION, HIDDEN_LOCATION, UNZIPPED])
            try {
                removeTree(location)
            } catch (e) {
                ec.logger.warn("MapsPackages cleanup: " + location + " not deleted")
            }
        del("moqui.security.UserGroupMember", "userId", [OWNER, OTHER])
        del("moqui.security.UserAccount", "userId", [OWNER, OTHER])
        ec.message.clearErrors()
    }
})

feature("a package is cut from what its owner may see, its manifest lists every member, and only its owner downloads it by range", {
    when() {
        const out = {}
        const asked = Object.assign({name: "Spec region", profileId: PROFILE, basemapLayerIds: [VISIBLE, HIDDEN], includePlaces: "N",
            includeOverlays: "N", includePostal: "Y", includeOverture: "N"}, REGION)
        out.estimate = as(OWNER, () => invoke(S + "estimate#Package", asked))
        // the MapPackageBuild job runs after the create commits
        out.created = as(OWNER, () => invoke(S + "create#Package", asked))
        const deadline = Date.now() + 10000
        while (Date.now() < deadline && ["MapPkgPending", "MapPkgBuilding"].includes(String(packageRow(out.created.packageId)?.statusId)))
            sleep(50)
        const row = packageRow(out.created.packageId) || {}
        out.row = {statusId: row.statusId, statusNote: row.statusNote, revision: row.revision, sha256: row.sha256, size: row.size,
            location: row.location}
        out.listed = (as(OWNER, () => invoke(S + "find#Packages").packageList) || []).find(entry => entry.packageId == out.created.packageId) || {}
        out.owner = download(OWNER, out.created.packageId, row.revision, "bytes=0-99")
        out.whole = download(OWNER, out.created.packageId, row.revision)
        out.other = download(OTHER, out.created.packageId, row.revision, "bytes=0-99")
        out.stale = download(OWNER, out.created.packageId, Number(row.revision) + 1, "bytes=0-99")
        out.otherUpdate = as(OTHER, () => invoke(S + "update#Package", {packageId: out.created.packageId}))
        out.otherDelete = as(OTHER, () => invoke(S + "delete#Package", {packageId: out.created.packageId}))
        ec.artifactExecution.disableAuthz()
        out.downloads = Array.from(
            ec.entity.find("moqui.maps.MapPackageDownload").condition("packageId", out.created.packageId || "_NA_").list()
        ).map(entry => ({userId: entry.userId, revision: entry.revision}))
        // the zip as a device unpacks it: each member against the manifest
        removeTree(UNZIPPED)
        out.zipPaths = row.location ? unzipStored(row.location, UNZIPPED) : []
        out.manifest = JSON.parse(ec.resource.getLocationText(UNZIPPED + "/manifest.json") || "{}")
        out.checked = Array.from(out.manifest.members || []).map(member => ({
            path: member.path,
            listed: member.sha256,
            size: member.size,
            actual: ec.resource.digest(UNZIPPED + "/" + member.path, "sha256"),
            actualSize: Number(ec.resource.getLocationReference(UNZIPPED + "/" + member.path).getSize())
        }))
        out.zipSha = row.location ? ec.resource.digest(row.location, "sha256") : null
        const extract = ec.resource.getLocationReference(UNZIPPED + "/basemap/" + VISIBLE + ".pmtiles")
        out.extractHead = extract.getExists() ? String.fromCharCode(...extract.openStream().readAllBytes().subarray(0, 7)) : ""
        // built again by its owner: a new revision, the first's file removed once it is ready and its URL then 409
        out.updated = as(OWNER, () => invoke(S + "update#Package", {packageId: out.created.packageId}))
        const again = Date.now() + 10000
        while (Date.now() < again && String(packageRow(out.created.packageId)?.revision) !== "2") sleep(50)
        ec.artifactExecution.disableAuthz()
        const second = packageRow(out.created.packageId) || {}
        out.second = {statusId: second.statusId, revision: second.revision, location: second.location}
        out.firstAfter = row.location ? ec.resource.getLocationReference(row.location).getExists() : null
        out.firstUrl = download(OWNER, out.created.packageId, 1, "bytes=0-99")
        // deleted by its owner: its file goes
        out.deleted = as(OWNER, () => invoke(S + "delete#Package", {packageId: out.created.packageId}))
        ec.artifactExecution.disableAuthz()
        out.fileAfter = second.location ? ec.resource.getLocationReference(second.location).getExists() : null
        out.rowAfter = !!packageRow(out.created.packageId)
        return out
    },
    then(out) {
        out.estimate.applied == true && out.estimate.estimate.layers.length == 1 && out.estimate.estimate.layers[0].id == VISIBLE
        out.estimate.estimate.tileCount > 0 && out.estimate.estimate.bytes > 0
        // the layer the owner may not see is left out, as one that is not there
        out.estimate.estimate.omitted.some(entry => entry.id == HIDDEN && entry.kind == "basemap")
        out.created.applied == true && out.created.started == true
        out.row.statusId == "MapPkgReady" && out.row.revision == 1 && out.row.sha256.length == 64
        out.row.location == ROOT + "/" + out.created.packageId + "-1.zip"
        out.zipSha == out.row.sha256
        out.listed.downloadUrl == "/apps/maps/Offline/package?packageId=" + out.created.packageId + "&revision=1"
        out.listed.tag == "sha256-" + out.row.sha256
        // its owner reads a range with the strong tag; another user is told nothing of it; another revision is 409
        out.owner.status == 206 && out.owner.etag == '"sha256-' + out.row.sha256 + '"'
        out.owner.contentRange == "bytes 0-99/" + out.row.size
        out.owner.cacheControl.includes("private")
        out.whole.status == 200
        out.other.status == 404 && out.other.etag == null
        out.stale.status == 409
        out.otherUpdate.reasonCode == "RECORD_UNAVAILABLE" && out.otherDelete.reasonCode == "RECORD_UNAVAILABLE"
        out.downloads.length == 1 && out.downloads[0].userId == OWNER && out.downloads[0].revision == 1
        // the zip holds the manifest and every member it lists, each of its size and SHA-256
        out.checked.length > 0 && out.checked.every(member => member.listed == member.actual && member.size == member.actualSize)
        out.zipPaths.length == out.checked.length + 1 && out.zipPaths.includes("manifest.json")
        out.checked.some(member => member.path == "basemap/" + VISIBLE + ".pmtiles") && out.extractHead == "PMTiles"
        out.checked.some(member => member.path == "postal.json")
        !out.zipPaths.some(path => path.includes(HIDDEN))
        out.manifest.schemaVersion == 1 && out.manifest.package.revision == 1 && out.manifest.package.maxZoom == 3
        out.manifest.sources.some(source => source.kind == "basemap" && source.layerIds.includes(VISIBLE) && source.maxZoom == 3)
        out.manifest.omitted.some(entry => entry.id == HIDDEN)
        // the frozen configuration names the package's own paths only
        out.manifest.configuration.profile.basemaps[0].url == "basemap/" + VISIBLE + ".pmtiles"
        out.manifest.configuration.profile.allowedOrigins.length == 0 && out.manifest.configuration.externalRequests == false
        out.manifest.configuration.profile.basemaps.concat(out.manifest.configuration.layers)
            .every(entry => !/^(\/|[a-z]+:)/i.test(entry.url || "") && !/^(\/|[a-z]+:)/i.test(entry.styleUrl || ""))
        !JSON.stringify(out.manifest).includes("/apps/maps")
        out.updated.applied == true && out.second.statusId == "MapPkgReady" && out.second.revision == 2
        out.second.location == ROOT + "/" + out.created.packageId + "-2.zip" && out.firstAfter == false
        out.firstUrl.status == 409
        out.deleted.applied == true && out.fileAfter == false && out.rowAfter == false
    }
})

feature("a package past maps@package-max-bytes is refused at its estimate, and nothing is made", {
    when() {
        const asked = Object.assign({name: "Too large", profileId: PROFILE, basemapLayerIds: [VISIBLE]}, REGION)
        const before = Number(ec.entity.find("moqui.maps.MapPackage").condition("ownerUserId", OWNER).count())
        return withConf("maps@package-max-bytes", "1000", () => ({
            estimate: as(OWNER, () => invoke(S + "estimate#Package", asked)),
            created: as(OWNER, () => invoke(S + "create#Package", asked)),
            made: Number(ec.entity.find("moqui.maps.MapPackage").condition("ownerUserId", OWNER).disableAuthz().count()) - before
        }))
    },
    then({estimate, created, made}) {
        estimate.applied == false && estimate.reasonCode == "TOO_LARGE"
        created.applied == false && created.reasonCode == "TOO_LARGE"
        made == 0
    }
})

feature("a cancel ends a build cancelled, and an archive registered again since it was asked fails it", {
    when() {
        const out = {}
        ec.artifactExecution.disableAuthz()
        const revision = String(ec.entity.find("moqui.maps.MapLayer").condition("layerId", VISIBLE).one().sourceRevision)
        // a build under way (its job run not ended): a cancel is asked, and the run reads it at its start
        make("moqui.service.job.ServiceJobRun", {jobRunId: FAKE_RUN, jobName: "MapPackageBuild", startTime: ec.user.nowTimestamp})
        packageOf("MAPS_PKG_CANCEL", {statusId: "MapPkgBuilding", jobRunId: FAKE_RUN, sourceRevisions: JSON.stringify({[VISIBLE]: revision})})
        out.otherCancel = as(OTHER, () => invoke(S + "cancel#Package", {packageId: "MAPS_PKG_CANCEL"}))
        out.cancel = as(OWNER, () => invoke(S + "cancel#Package", {packageId: "MAPS_PKG_CANCEL"}))
        out.asked = packageRow("MAPS_PKG_CANCEL").cancelRequested
        out.busyDelete = as(OWNER, () => invoke(S + "delete#Package", {packageId: "MAPS_PKG_CANCEL"}))
        out.run = asSignedIn(OWNER, () => invoke(S + "build#Package", {packageId: "MAPS_PKG_CANCEL"}))
        out.cancelled = packageRow("MAPS_PKG_CANCEL").statusId
        out.cancelledFiles = buildFiles("MAPS_PKG_CANCEL")
        // asked for when the archive had another revision: it was registered again since
        packageOf("MAPS_PKG_REPLACED", {sourceRevisions: JSON.stringify({[VISIBLE]: "an-earlier-revision"})})
        out.replacedRun = asSignedIn(OWNER, () => invoke(S + "build#Package", {packageId: "MAPS_PKG_REPLACED"}))
        const replaced = packageRow("MAPS_PKG_REPLACED")
        out.replaced = {statusId: replaced.statusId, statusNote: String(replaced.statusNote || ""), location: replaced.location}
        out.replacedFiles = buildFiles("MAPS_PKG_REPLACED")
        // the Offline page lists them: in the browser with New offline map, in the terminal (its own template) with
        // Update and Delete and no build
        const page = mode =>
            as(OWNER, () => {
                const res = ec.screen.makeTest().rootScreen("component://webroot/screen/webroot.xml").renderMode(mode).render("apps/maps/Offline", mode === "qvt" ? {lastStandalone: "true"} : {}, null)
                return {output: String(res.output || ""), errors: Array.from(res.errorMessages || []).join(" | ")}
            })
        out.qvt = page("qvt")
        out.tui = page("tui")
        return out
    },
    then(out) {
        out.otherCancel.reasonCode == "RECORD_UNAVAILABLE"
        out.cancel.applied == true && out.asked == "Y"
        out.busyDelete.reasonCode == "PACKAGE_BUSY"
        out.run.statusId == "MapPkgCancelled" && out.cancelled == "MapPkgCancelled" && out.cancelledFiles.length == 0
        out.replacedRun.statusId == "MapPkgFailed" && out.replaced.statusId == "MapPkgFailed"
        out.replaced.statusNote.includes("registered again") && !out.replaced.location && out.replacedFiles.length == 0
        out.qvt.errors == "" && out.qvt.output.includes("Spec MAPS_PKG_CANCEL") && out.qvt.output.includes("NewPackage")
        out.tui.errors == "" && out.tui.output.includes("OfflinePackages") && !out.tui.output.includes("NewPackage")
        out.tui.output.includes("OfflineUpdateMAPS_PKG_CANCEL") && out.tui.output.includes("OfflineDeleteMAPS_PKG_REPLACED")
    }
})

feature("the hourly clean removes the file of a package not downloaded for maps@package-keep-days, and of a deleted one", {
    when() {
        ec.artifactExecution.disableAuthz()
        const day = 86400000
        const kept = ROOT + "/MAPS_PKG_KEPT-1.zip"
        const expired = ROOT + "/MAPS_PKG_OLD-1.zip"
        const orphan = ROOT + "/MAPS_PKG_GONE-3.zip"
        for (const location of [kept, expired, orphan]) lib.put(location, lib.ascii("PK spec"))
        packageOf("MAPS_PKG_KEPT", {statusId: "MapPkgReady", revision: 1, location: kept, size: 7, sha256: "0".repeat(64),
            builtDate: new Date(Date.now() - 40 * day), lastDownloadDate: new Date(Date.now() - 2 * day)})
        packageOf("MAPS_PKG_OLD", {statusId: "MapPkgReady", revision: 1, location: expired, size: 7, sha256: "0".repeat(64),
            builtDate: new Date(Date.now() - 31 * day)})
        const cleaned = invoke(S + "clean#MapPackages")
        const exists = location => ec.resource.getLocationReference(location).getExists()
        const old = packageRow("MAPS_PKG_OLD")
        return {cleaned, kept: exists(kept), expired: exists(expired), orphan: exists(orphan), oldStatus: old.statusId,
            oldLocation: old.location, keptStatus: packageRow("MAPS_PKG_KEPT").statusId}
    },
    then(out) {
        out.cleaned.removedCount >= 2
        out.kept && out.keptStatus == "MapPkgReady"
        !out.expired && out.oldStatus == "MapPkgExpired" && !out.oldLocation
        !out.orphan
    }
})
