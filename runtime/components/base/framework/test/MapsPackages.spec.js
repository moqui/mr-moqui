// Offline map packages (plans/Maps-Offline.md §2, OF1; org.moqui.maps.MapSourceServices, MapPackages.js, tools
// screen/Maps/Offline.xml package): a package is cut from a small fixture archive (MapsArchiveLib's tiles) and built by
// the MapPackageBuild job from what its owner may see, a basemap layer they may not see left out; its zip is served to
// its owner by range with its SHA-256 as the strong ETag and refused another user, and a stale revision is 409, and
// listed as JSON to its owner only (packageList, the apps' download); its manifest lists every member with its size and
// SHA-256 and names no URL of this server; a package past maps@package-max-bytes is refused at its estimate; one build
// at a time: a user's second refused while theirs is building, another user's waiting its turn and started when the
// build ends; a cancel ends a build cancelled, a run whose key the package does not hold writes nothing, and an archive
// registered again since the build was asked fails it; the Offline page renders once in qvt and once in the terminal
// (its own template); the hourly clean removes an expired package's file and a deleted one's; the owner's places and
// the overlays they read are packaged, another user's places and an unshared overlay never, a shared one until its
// share is revoked; a dead build and a disabled owner's build end failed and hold no turn, and a waiting build runs as
// its owner, who alone is told; a started run silent for an hour is ended by the build's own clocks, its package row's
// fresh stamp aside. The cut's bounds, the antimeridian and the writer are moqui-tools' and moqui-lib's Rust tests.
// Run: ./moq test framework --spec MapsPackages
const S = "org.moqui.maps.MapSourceServices."
const OWNER = "MAPS_PKG_OWNER"
const OTHER = "MAPS_PKG_OTHER"
const VISIBLE = "MAPS_PKG_VISIBLE"
const HIDDEN = "MAPS_PKG_HIDDEN"
const PROFILE = "MAPS_PKG_PROFILE"
const SECRET = "MAPS_PKG_SPEC_SECRET"
const FAKE_RUN = "MAPS_PKG_FAKE_RUN"
const DISABLED = "MAPS_PKG_DISABLED"
// locked out for a few minutes after failed sign-ins: a tarpit lock, not a closed account; and one locked ten minutes ago,
// past the default window of 5 and within a configured 30
const LOCKED = "MAPS_PKG_LOCKED"
const LOCKED_LONG = "MAPS_PKG_LOCKED_LONG"
const QUEUED_RUN = "MAPS_PKG_QUEUED_RUN"
// a started run silent for two hours
const STALLED_RUN = "MAPS_PKG_STALLED_RUN"
// a group whose members hold the hidden layer's permission: the other user's
const SECRET_GROUP = "MAPS_PKG_SECRET_GROUP"
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
// the packageList transition as the user, which the apps' download reads: its status and the packages it lists
function packageList(userId) {
    return as(userId, () => {
        const res = ec.screen.makeTest().rootScreen("component://webroot/screen/webroot.xml").render("apps/maps/Offline/packageList", {}, "get")
        let list = []
        try {
            list = JSON.parse(String(res.output || "{}")).packageList || []
        } catch (e) {
            list = []
        }
        return {status: res.status, list}
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
                buildKey: "SPECKEY",
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
        for (const userId of [OWNER, OTHER, DISABLED, LOCKED, LOCKED_LONG]) {
            make("moqui.security.UserAccount", {userId, username: usernameOf(userId), userFullName: "Maps packages " + userId,
                disabled: [DISABLED, LOCKED, LOCKED_LONG].includes(userId) ? "Y" : "N",
                disabledDateTime: userId === LOCKED ? ec.user.nowTimestamp : userId === LOCKED_LONG ? new Date(Date.now() - 600000) : null})
            make("moqui.security.UserGroupMember", {userGroupId: "MAPS_USERS", userId, fromDate: ec.user.nowTimestamp})
        }
        make("moqui.security.UserPermission", {userPermissionId: SECRET, description: "Maps packages spec"})
        make("moqui.security.UserGroup", {userGroupId: SECRET_GROUP, description: "Maps packages spec"})
        make("moqui.security.UserGroupPermission", {userGroupId: SECRET_GROUP, userPermissionId: SECRET, fromDate: ec.user.nowTimestamp})
        make("moqui.security.UserGroupMember", {userGroupId: SECRET_GROUP, userId: OTHER, fromDate: ec.user.nowTimestamp})
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
        const rows = Array.from(ec.entity.find("moqui.maps.MapPackage").condition("ownerUserId", "in", [OWNER, OTHER, DISABLED, LOCKED, LOCKED_LONG]).list())
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
        del("moqui.maps.MapPackage", "ownerUserId", [OWNER, OTHER, DISABLED, LOCKED, LOCKED_LONG])
        // the overlay feature's rows, should it stop part way
        del("moqui.resource.ResourceShare", "shareId", ["MAPS_PKG_SHARE_OVERLAY", "MAPS_PKG_SHARE_PLACES"])
        del("moqui.maps.MapOverlay", "ownerUserId", [OWNER, OTHER])
        del("moqui.resource.ResourceDrive", "driveId", ["MAPS_PKG_OWN_DRIVE", "MAPS_PKG_OTHER_DRIVE"])
        del("moqui.service.job.ServiceJobRun", "jobRunId", [FAKE_RUN, QUEUED_RUN, STALLED_RUN])
        del("moqui.maps.MapProfile", "profileId", [PROFILE])
        del("moqui.maps.MapLayer", "layerId", [VISIBLE, HIDDEN])
        for (const location of [VISIBLE_LOCATION, HIDDEN_LOCATION, UNZIPPED])
            try {
                removeTree(location)
            } catch (e) {
                ec.logger.warn("MapsPackages cleanup: " + location + " not deleted")
            }
        del("moqui.security.UserGroupMember", "userId", [OWNER, OTHER, DISABLED, LOCKED, LOCKED_LONG])
        del("moqui.security.UserGroupPermission", "userGroupId", [SECRET_GROUP])
        del("moqui.security.UserGroup", "userGroupId", [SECRET_GROUP])
        del("moqui.security.UserPermission", "userPermissionId", [SECRET])
        const notices = Array.from(ec.entity.find("moqui.security.user.NotificationMessageUser").condition("userId", "in", [OWNER, OTHER, DISABLED, LOCKED, LOCKED_LONG]).list())
            .map(row => row.notificationMessageId)
        del("moqui.security.user.NotificationMessageUser", "userId", [OWNER, OTHER, DISABLED, LOCKED, LOCKED_LONG])
        if (notices.length) del("moqui.security.user.NotificationMessage", "notificationMessageId", notices)
        del("moqui.security.UserAccount", "userId", [OWNER, OTHER, DISABLED, LOCKED, LOCKED_LONG])
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
        out.ownerList = packageList(OWNER)
        out.otherList = packageList(OTHER)
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
        out.created.applied == true
        out.row.statusId == "MapPkgReady" && out.row.revision == 1 && out.row.sha256.length == 64
        out.row.location.startsWith(ROOT + "/" + out.created.packageId + "-1-") && out.row.location.endsWith(".zip")
        out.zipSha == out.row.sha256
        out.listed.downloadUrl == "/apps/maps/Offline/package?packageId=" + out.created.packageId + "&revision=1"
        out.listed.tag == "sha256-" + out.row.sha256
        // the apps read the same list as JSON; another user's lists none of it
        out.ownerList.status == 200 && out.ownerList.list.some(entry => entry.packageId == out.created.packageId && entry.tag == out.listed.tag)
        out.otherList.status == 200 && !out.otherList.list.some(entry => entry.packageId == out.created.packageId)
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
        out.second.location.startsWith(ROOT + "/" + out.created.packageId + "-2-") && out.firstAfter == false
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

feature("one build at a time: a user's second refused, another user's waits its turn, a cancel ends a build, and a replaced archive fails one", {
    when() {
        const out = {}
        ec.artifactExecution.disableAuthz()
        const revision = String(ec.entity.find("moqui.maps.MapLayer").condition("layerId", VISIBLE).one().sourceRevision)
        // a build under way (its job run not ended): a cancel is asked, and the run reads it at its start
        make("moqui.service.job.ServiceJobRun", {jobRunId: FAKE_RUN, jobName: "MapPackageBuild", startTime: ec.user.nowTimestamp})
        packageOf("MAPS_PKG_CANCEL", {statusId: "MapPkgBuilding", jobRunId: FAKE_RUN, sourceRevisions: JSON.stringify({[VISIBLE]: revision})})
        const asked = Object.assign({name: "Spec waiting", profileId: PROFILE, basemapLayerIds: [VISIBLE], includePostal: "N"}, REGION)
        // the owner has one build under way: a second is refused; another user's waits, the installation's one build busy.
        // It asks the layer only that user may see, so its run, started by the owner's build's end, must be theirs
        out.ownerBusy = as(OWNER, () => invoke(S + "create#Package", asked))
        out.waiting = as(OTHER, () => invoke(S + "create#Package", Object.assign({}, asked, {basemapLayerIds: [VISIBLE, HIDDEN]})))
        const waitingRow = packageRow(out.waiting.packageId) || {}
        out.waitingRow = {statusId: waitingRow.statusId, jobRunId: waitingRow.jobRunId || null}
        out.waitingText =
            ((as(OTHER, () => invoke(S + "find#Packages").packageList) || []).find(entry => entry.packageId == out.waiting.packageId) || {})
                .statusText
        out.otherCancel = as(OTHER, () => invoke(S + "cancel#Package", {packageId: "MAPS_PKG_CANCEL"}))
        out.cancel = as(OWNER, () => invoke(S + "cancel#Package", {packageId: "MAPS_PKG_CANCEL"}))
        out.asked = packageRow("MAPS_PKG_CANCEL").cancelRequested
        out.busyDelete = as(OWNER, () => invoke(S + "delete#Package", {packageId: "MAPS_PKG_CANCEL"}))
        out.run = asSignedIn(OWNER, () => invoke(S + "build#Package", {packageId: "MAPS_PKG_CANCEL", buildKey: "SPECKEY"}))
        out.cancelled = packageRow("MAPS_PKG_CANCEL").statusId
        out.cancelledFiles = buildFiles("MAPS_PKG_CANCEL")
        // the build's end gave the waiting one its turn: wait for that run to end whole (its row is Ready before it tells
        // the owner and gives the next its turn)
        const waitedRunId = String(packageRow(out.waiting.packageId)?.jobRunId || "_NA_")
        const waitedRunEnded = () =>
            !!ec.entity.find("moqui.service.job.ServiceJobRun").condition("jobRunId", waitedRunId).disableAuthz().one()?.endTime
        const deadline = Date.now() + 10000
        while (Date.now() < deadline && !waitedRunEnded()) sleep(50)
        out.waitedStatus = packageRow(out.waiting.packageId)?.statusId
        ec.artifactExecution.disableAuthz()
        const waited = packageRow(out.waiting.packageId)
        removeTree(UNZIPPED)
        out.waitedPaths = waited?.location ? unzipStored(waited.location, UNZIPPED) : []
        // its owner was told it is ready; the user whose build's end started it was not
        const told = userId =>
            Array.from(ec.entity.find("moqui.security.user.NotificationMessageUser").condition("userId", userId).list())
                .map(row => ec.entity.find("moqui.security.user.NotificationMessage").condition("notificationMessageId", row.notificationMessageId).one())
                .filter(message => message && message.topic === "MapPackageBuild" && String(message.messageJson || "").includes(String(out.waiting.packageId)))
                .length
        out.toldOwner = told(OTHER)
        out.toldStarter = told(OWNER)
        // asked for when the archive had another revision: it was registered again since; a run with a key the package
        // does not hold writes nothing. Its jobRunId is the live fake run, so no scheduler starts it beside these calls
        packageOf("MAPS_PKG_REPLACED", {jobRunId: FAKE_RUN, sourceRevisions: JSON.stringify({[VISIBLE]: "an-earlier-revision"})})
        out.strayRun = asSignedIn(OWNER, () => invoke(S + "build#Package", {packageId: "MAPS_PKG_REPLACED", buildKey: "ANOTHERKEY"}))
        out.strayStatus = packageRow("MAPS_PKG_REPLACED").statusId
        out.replacedRun = asSignedIn(OWNER, () => invoke(S + "build#Package", {packageId: "MAPS_PKG_REPLACED", buildKey: "SPECKEY"}))
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
        out.ownerBusy.reasonCode == "PACKAGE_BUSY"
        out.waiting.applied == true && out.waitingRow.statusId == "MapPkgPending" && out.waitingRow.jobRunId == null
        out.waitingText == "Waiting for its turn"
        out.waitedStatus == "MapPkgReady"
        // cut as its owner: the layer only they may see is in it
        out.waitedPaths.includes("basemap/" + HIDDEN + ".pmtiles") && out.waitedPaths.includes("basemap/" + VISIBLE + ".pmtiles")
        out.toldOwner == 1 && out.toldStarter == 0
        out.strayRun.statusId == null && out.strayRun.reason.includes("another run") && out.strayStatus == "MapPkgPending"
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

feature("a package holds the owner's places and the overlays they read now, never another user's places or an unshared overlay", {
    when() {
        ec.artifactExecution.disableAuthz()
        const roots = {own: "dbresource://maps-packages-spec/own", other: "dbresource://maps-packages-spec/other"}
        for (const [driveId, ownerUserId, rootLocation] of [
            ["MAPS_PKG_OWN_DRIVE", OWNER, roots.own],
            ["MAPS_PKG_OTHER_DRIVE", OTHER, roots.other]
        ])
            make("moqui.resource.ResourceDrive", {driveId, driveName: "Maps packages " + driveId, driveKindEnumId: "RdkShared", rootLocation,
                ownerUserId, createdDate: ec.user.nowTimestamp})
        const point = label => JSON.stringify({type: "FeatureCollection",
            features: [{type: "Feature", properties: {label}, geometry: {type: "Point", coordinates: [30, 30]}}]})
        const overlays = [
            ["MAPS_PKG_P_OWN", OWNER, "MAPS_PKG_OWN_DRIVE", roots.own, "Maps/places.geojson", "Y"],
            ["MAPS_PKG_O_MINE", OWNER, "MAPS_PKG_OWN_DRIVE", roots.own, "Maps/overlays/mine.geojson", "N"],
            ["MAPS_PKG_O_PRIVATE", OTHER, "MAPS_PKG_OTHER_DRIVE", roots.other, "private.geojson", "N"],
            ["MAPS_PKG_O_SHARED", OTHER, "MAPS_PKG_OTHER_DRIVE", roots.other, "shared.geojson", "N"],
            ["MAPS_PKG_P_OTHER", OTHER, "MAPS_PKG_OTHER_DRIVE", roots.other, "Maps/places.geojson", "Y"]
        ]
        for (const [overlayId, ownerUserId, driveId, root, path, isPlaces] of overlays) {
            ec.resource.getLocationReference(root + "/" + path).putText(point(overlayId))
            make("moqui.maps.MapOverlay", {overlayId, ownerUserId, label: overlayId, driveId, path, formatEnumId: "MLF_GEOJSON", isPlaces,
                featureCount: 1, rejectedCount: 0, minLongitude: 30, minLatitude: 30, maxLongitude: 30, maxLatitude: 30,
                createdDate: ec.user.nowTimestamp})
        }
        // the other user shares one overlay and their places with the owner, not the private overlay
        for (const [shareId, path] of [
            ["MAPS_PKG_SHARE_OVERLAY", "shared.geojson"],
            ["MAPS_PKG_SHARE_PLACES", "Maps/places.geojson"]
        ])
            make("moqui.resource.ResourceShare", {shareId, driveId: "MAPS_PKG_OTHER_DRIVE", path, sharedWithUserId: OWNER,
                permissionEnumId: "RspView", createdByUserId: OTHER, createdDate: ec.user.nowTimestamp})
        const members = (packageId, revision) => {
            const deadline = Date.now() + 10000
            while (Date.now() < deadline && String(packageRow(packageId)?.revision) !== String(revision)) sleep(50)
            ec.artifactExecution.disableAuthz()
            const row = packageRow(packageId) || {}
            removeTree(UNZIPPED)
            const paths = row.location && String(row.revision) === String(revision) ? unzipStored(row.location, UNZIPPED) : []
            const manifest = paths.length ? JSON.parse(ec.resource.getLocationText(UNZIPPED + "/manifest.json") || "{}") : {}
            return {statusId: row.statusId, paths, layers: Array.from(manifest.configuration?.layers || []).map(layer => layer.id)}
        }
        const asked = Object.assign({name: "Spec overlays", profileId: PROFILE, basemapLayerIds: [VISIBLE], includePlaces: "Y",
            includeOverlays: "Y", includePostal: "N", includeOverture: "N"}, REGION)
        const created = as(OWNER, () => invoke(S + "create#Package", asked))
        const first = members(created.packageId, 1)
        // the share revoked: the next build leaves the overlay out
        ec.artifactExecution.disableAuthz()
        const share = ec.entity.find("moqui.resource.ResourceShare").condition("shareId", "MAPS_PKG_SHARE_OVERLAY").one()
        share.set("revokedDate", ec.user.nowTimestamp)
        share.update()
        const updated = as(OWNER, () => invoke(S + "update#Package", {packageId: created.packageId}))
        const second = members(created.packageId, 2)
        as(OWNER, () => invoke(S + "delete#Package", {packageId: created.packageId}))
        ec.artifactExecution.disableAuthz()
        ec.entity.find("moqui.resource.ResourceShare").condition("shareId", "in", ["MAPS_PKG_SHARE_OVERLAY", "MAPS_PKG_SHARE_PLACES"]).deleteAll()
        ec.entity.find("moqui.maps.MapOverlay").condition("overlayId", "in", overlays.map(row => row[0])).deleteAll()
        for (const [, , , root, path] of overlays) removeTree(root + "/" + path)
        ec.entity.find("moqui.resource.ResourceDrive").condition("driveId", "in", ["MAPS_PKG_OWN_DRIVE", "MAPS_PKG_OTHER_DRIVE"]).deleteAll()
        return {created, first, updated, second}
    },
    then({created, first, updated, second}) {
        created.applied == true && first.statusId == "MapPkgReady"
        first.paths.includes("places.geojson") && first.paths.includes("overlays/MAPS_PKG_O_MINE.geojson")
        first.paths.includes("overlays/MAPS_PKG_O_SHARED.geojson")
        !first.paths.some(path => path.includes("MAPS_PKG_O_PRIVATE") || path.includes("MAPS_PKG_P_OTHER"))
        first.layers.includes("places") && first.layers.includes("overlay:MAPS_PKG_O_SHARED") && !first.layers.includes("overlay:MAPS_PKG_P_OTHER")
        updated.applied == true && second.statusId == "MapPkgReady"
        second.paths.includes("overlays/MAPS_PKG_O_MINE.geojson") && !second.paths.includes("overlays/MAPS_PKG_O_SHARED.geojson")
        second.paths.includes("places.geojson") && !second.paths.some(path => path.includes("MAPS_PKG_P_OTHER"))
    }
})

feature("a dead build and a closed account's build end failed, a locked-out owner's waits, and the next build runs", {
    when() {
        ec.artifactExecution.disableAuthz()
        // a build left building by a run that is gone (a restart mid-build), and a disabled user's build waiting, older
        packageOf("MAPS_PKG_DEAD", {ownerUserId: OTHER, statusId: "MapPkgBuilding", jobRunId: "MAPS_PKG_GONE_RUN", buildKey: "DEADKEY"})
        packageOf("MAPS_PKG_DISABLED", {ownerUserId: DISABLED, createdDate: new Date(Date.now() - 60000)})
        packageOf("MAPS_PKG_LOCKED", {ownerUserId: LOCKED, createdDate: new Date(Date.now() - 120000)})
        const asked = Object.assign({name: "Spec after the dead", profileId: PROFILE, basemapLayerIds: [VISIBLE], includePostal: "N"}, REGION)
        const created = as(OWNER, () => invoke(S + "create#Package", asked))
        const deadline = Date.now() + 10000
        while (Date.now() < deadline && String(packageRow(created.packageId)?.statusId) !== "MapPkgReady") sleep(50)
        const dead = packageRow("MAPS_PKG_DEAD")
        const disabled = packageRow("MAPS_PKG_DISABLED")
        return {
            created,
            status: packageRow(created.packageId)?.statusId,
            dead: {statusId: dead.statusId, buildKey: dead.buildKey || null},
            disabled: {statusId: disabled.statusId, statusNote: String(disabled.statusNote || "")},
            locked: {statusId: packageRow("MAPS_PKG_LOCKED").statusId, jobRunId: packageRow("MAPS_PKG_LOCKED").jobRunId || null}
        }
    },
    then(out) {
        out.created.applied == true && out.status == "MapPkgReady"
        out.dead.statusId == "MapPkgFailed" && out.dead.buildKey == null
        out.disabled.statusId == "MapPkgFailed" && out.disabled.statusNote.includes("could not be signed in")
        // the locked owner's older build was passed by, still waiting
        out.locked.statusId == "MapPkgPending" && out.locked.jobRunId == null
    }
})

feature("a build of an owner locked out under a configured window waits, and a run lost while queued frees its turn", {
    when() {
        ec.artifactExecution.disableAuthz()
        // locked ten minutes ago: under user-facade/login@disable-minutes 30 the build itself, run by another user, cannot
        // sign the owner in and puts the package back to wait
        packageOf("MAPS_PKG_LOCKED_LONG", {ownerUserId: LOCKED_LONG, jobRunId: FAKE_RUN})
        const run = withConf("user-facade/login@disable-minutes", "30", () =>
            invoke(S + "build#Package", {packageId: "MAPS_PKG_LOCKED_LONG", buildKey: "SPECKEY"})
        )
        ec.artifactExecution.disableAuthz()
        const waiting = packageRow("MAPS_PKG_LOCKED_LONG")
        const locked = {statusId: waiting.statusId, jobRunId: waiting.jobRunId || null, statusNote: String(waiting.statusNote || ""),
            buildKey: waiting.buildKey || null, runStatus: run.statusId}
        // out of the queue for what follows (its lock is past the default window: a scheduler would start it)
        waiting.set("statusId", "MapPkgCancelled")
        waiting.update()
        // a run handed to the job a day ago and never started (a restart while it was queued), its package's row written
        // just now (a download of an earlier revision, a cancel): the run's own clock decides, the next scheduler ends it,
        // and the next build runs. The run row's stamp is set by SQL, as no entity write keeps one
        make("moqui.service.job.ServiceJobRun", {jobRunId: QUEUED_RUN, jobName: "MapPackageBuild"})
        packageOf("MAPS_PKG_QUEUED", {ownerUserId: OTHER, jobRunId: QUEUED_RUN, buildKey: "QUEUEDKEY"})
        const definition = ec.entity.getEntityDefinition("moqui.service.job.ServiceJobRun")
        const dayAgo = new Date(Date.now() - 24 * 3600000).toISOString().replace("T", " ").replace(/\.\d+Z$/, "")
        ec.entity.runSqlStatement(definition.groupName || "transactional", "UPDATE " + definition.tableName +
            " SET LAST_UPDATED_STAMP = '" + dayAgo + "' WHERE JOB_RUN_ID = '" + QUEUED_RUN + "'", 1)
        const queuedRun = ec.entity.find("moqui.service.job.ServiceJobRun").condition("jobRunId", QUEUED_RUN).one()
        const stamped = Number(queuedRun.lastUpdatedStamp?.getTime?.() ?? queuedRun.lastUpdatedStamp)
        const asked = Object.assign({name: "Spec after the queued", profileId: PROFILE, basemapLayerIds: [VISIBLE], includePostal: "N"}, REGION)
        const created = as(OWNER, () => invoke(S + "create#Package", asked))
        const deadline = Date.now() + 10000
        while (Date.now() < deadline && String(packageRow(created.packageId)?.statusId) !== "MapPkgReady") sleep(50)
        return {locked, stampedOld: Date.now() - stamped > 6 * 3600000, created, status: packageRow(created.packageId)?.statusId,
            queued: packageRow("MAPS_PKG_QUEUED").statusId}
    },
    then(out) {
        out.locked.runStatus == "MapPkgPending" && out.locked.statusId == "MapPkgPending" && out.locked.jobRunId == null
        out.locked.buildKey == "SPECKEY" && out.locked.statusNote.includes("locked")
        out.stampedOld == true
        out.created.applied == true && out.status == "MapPkgReady" && out.queued == "MapPkgFailed"
    }
})

feature("a started run silent for over an hour is ended by the scheduler, though its package row was just written", {
    when() {
        ec.artifactExecution.disableAuthz()
        // started two hours ago, its last word as long ago; the package row written just now (a download of an earlier
        // revision, a cancel asked): the build's own clocks decide
        const twoHoursAgo = new Date(Date.now() - 2 * 3600000)
        make("moqui.service.job.ServiceJobRun", {jobRunId: STALLED_RUN, jobName: "MapPackageBuild", startTime: twoHoursAgo})
        packageOf("MAPS_PKG_STALLED", {ownerUserId: OTHER, statusId: "MapPkgBuilding", jobRunId: STALLED_RUN, buildKey: "STALLEDKEY",
            progressDate: twoHoursAgo})
        const before = packageRow("MAPS_PKG_STALLED")
        const stamped = Number(before.lastUpdatedStamp?.getTime?.() ?? before.lastUpdatedStamp)
        const scheduled = invoke(S + "schedule#PackageBuilds")
        const after = packageRow("MAPS_PKG_STALLED")
        return {fresh: Date.now() - stamped < 60000, scheduled, statusId: after.statusId, buildKey: after.buildKey || null,
            jobRunId: after.jobRunId || null}
    },
    then(out) {
        out.fresh == true && out.scheduled.error == ""
        out.statusId == "MapPkgFailed" && out.buildKey == null && out.jobRunId == null
    }
})
