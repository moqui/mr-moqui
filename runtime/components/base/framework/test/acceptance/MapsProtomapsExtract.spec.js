// MapsProtomapsExtract (plans/archive/Maps.md §4.1, §5, §9 acceptance): a real Protomaps basemap, a small extract of a recent daily
// planet build (https://build.protomaps.com/<date>.pmtiles, the newest of the last two weeks that answers), read by HTTP
// ranges as pmtiles reads an archive: its header, its root directory and the leaf directories the extract's tiles are in
// (gzipped, unpacked with ec.resource.gunzip), and the tiles of a small box over central Florence from zoom 0 to 14,
// copied as they are (gzipped MVT) into a new version 3 archive (MapsArchiveLib.archive) of a few megabytes under the
// runtime's maps directory. The packaged local profile's two layers, LOCAL_PROTOMAPS_LIGHT and LOCAL_PROTOMAPS_DARK, are
// registered from that one location in one action (register#ArchiveLayer with alsoLayerIds), the profile enabled and drawn
// through the map-source transition; a few tiles' ranges are read through the transition (206, the registered strong
// ETag) and the same tiles decoded as MVT carry the Protomaps layers (earth, water, roads, buildings). Its sizes and
// times are logged; the extract is deleted afterwards and the layers and profile put back as they were.
// Run when asked: ./moq accept framework --spec MapsProtomapsExtract
const http = ec.factory.getTool("HttpFixtures")
const lib = ec.resource.script("component://framework/test/MapsArchiveLib.js")
const S = "org.moqui.maps.MapSourceServices."
const HOST = "https://build.protomaps.com/"
const LOCATION = "runtime://maps/protomaps-extract-spec/florence.pmtiles"
const SCRATCH = "runtime://tmp/protomaps-extract-spec"
// central Florence: west, south, east, north; and the zooms copied
const BOX = [11.245, 43.765, 11.265, 43.777]
const MAX_ZOOM = 14
const LAYERS = ["LOCAL_PROTOMAPS_LIGHT", "LOCAL_PROTOMAPS_DARK"]
const PROFILE = "LOCAL_PROTOMAPS"
const log = (label, value) => ec.logger.warn("MapsProtomapsExtract: " + label + " " + JSON.stringify(value))
const sys = run => {
    const was = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!was) ec.artifactExecution.enableAuthz()
    }
}

// ===== reading the planet build by range =====
let source = null
let requests = 0
let downloaded = 0
function range(offset, length) {
    requests++
    const response = ec.service
        .rest()
        .method("GET")
        .uri(source)
        .addHeader("Range", "bytes=" + offset + "-" + (offset + length - 1))
        .timeout(60)
        .call()
    const status = Number(response.getStatusCode())
    if (status !== 206) throw new Error("the planet build answered HTTP " + status + " for a range")
    const bytes = response.bytes()
    downloaded += bytes.length
    return bytes
}
const u64 = (view, at) => view.getUint32(at, true) + view.getUint32(at + 4, true) * 4294967296
function headerOf(bytes) {
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
    const magic = String.fromCharCode(...bytes.slice(0, 7))
    if (magic !== "PMTiles" || bytes[7] !== 3) throw new Error("not a PMTiles version 3 archive")
    return {
        rootOffset: u64(view, 8),
        rootLength: u64(view, 16),
        metadataOffset: u64(view, 24),
        metadataLength: u64(view, 32),
        leafOffset: u64(view, 40),
        tileOffset: u64(view, 56),
        internalCompression: bytes[97],
        tileCompression: bytes[98],
        tileType: bytes[99],
        maxZoom: bytes[101]
    }
}
let scratch = 0
// gzip bytes unpacked through the resource facade (written, gunzipped to a second file, read back)
function gunzip(bytes) {
    const name = SCRATCH + "/part-" + scratch++
    ec.resource.getLocationReference(name + ".gz").putBytes(bytes)
    ec.resource.gunzip(name + ".gz", name, 64 * 1024 * 1024)
    return ec.resource.getLocationReference(name).openStream().readAllBytes()
}
const unpacked = (bytes, compression) => (compression === 2 ? gunzip(bytes) : bytes)
// a directory: [{tileId, offset, length, runLength}], its varints read by arithmetic (offsets pass 2^32)
function directoryOf(bytes) {
    let at = 0
    const next = () => {
        let value = 0
        let scale = 1
        for (;;) {
            const b = bytes[at++]
            value += (b & 127) * scale
            if (b < 128) return value
            scale *= 128
        }
    }
    const count = next()
    const entries = []
    let last = 0
    for (let i = 0; i < count; i++) {
        last += next()
        entries.push({tileId: last})
    }
    for (const entry of entries) entry.runLength = next()
    for (const entry of entries) entry.length = next()
    for (let i = 0; i < count; i++) {
        const value = next()
        entries[i].offset = value === 0 && i > 0 ? entries[i - 1].offset + entries[i - 1].length : value - 1
    }
    return entries
}
const leaves = new Map()
// the entry holding tileId: the last entry at or before it, a tile when its run covers it, a leaf directory when runLength 0
function findTile(header, entries, tileId, depth = 0) {
    let low = 0
    let high = entries.length - 1
    let found = null
    while (low <= high) {
        const mid = (low + high) >> 1
        if (entries[mid].tileId <= tileId) {
            found = entries[mid]
            low = mid + 1
        } else high = mid - 1
    }
    if (!found) return null
    if (found.runLength === 0) {
        if (depth > 3) return null
        const key = header.leafOffset + found.offset
        if (!leaves.has(key)) leaves.set(key, directoryOf(unpacked(range(key, found.length), header.internalCompression)))
        return findTile(header, leaves.get(key), tileId, depth + 1)
    }
    return tileId < found.tileId + found.runLength ? found : null
}

// ===== an MVT's layer names =====
function layerNames(bytes) {
    const names = []
    let at = 0
    const varint = () => {
        let value = 0
        let scale = 1
        for (;;) {
            const b = bytes[at++]
            value += (b & 127) * scale
            if (b < 128) return value
            scale *= 128
        }
    }
    while (at < bytes.length) {
        const key = varint()
        const wire = key & 7
        if (wire === 2) {
            const length = varint()
            if (key >> 3 === 3) {
                // a layer: its name is field 1
                let inner = at
                while (inner < at + length) {
                    const save = at
                    at = inner
                    const fieldKey = varint()
                    if ((fieldKey & 7) === 2) {
                        const fieldLength = varint()
                        if (fieldKey >> 3 === 1) names.push(String.fromCharCode(...bytes.slice(at, at + fieldLength)))
                        at += fieldLength
                    } else if ((fieldKey & 7) === 0) varint()
                    else if ((fieldKey & 7) === 5) at += 4
                    else at += 8
                    inner = at
                    at = save
                }
            }
            at += length
        } else if (wire === 0) varint()
        else if (wire === 5) at += 4
        else at += 8
    }
    return names
}

const two = value => String(value).padStart(2, "0")
const kept = {layers: {}, profile: null}
// what register#ArchiveLayer and store#ProfileSettings write, put back as they were (null where they were empty)
const LAYER_FIELDS = [
    "resourceLocation",
    "sourceRevision",
    "sourceTag",
    "sourceSize",
    "sourceVersionName",
    "sourceAddressKind",
    "sourceModifiedDate",
    "sourceRegisteredDate",
    "driveId",
    "minZoom",
    "maxZoom",
    "minLongitude",
    "minLatitude",
    "maxLongitude",
    "maxLatitude",
    "attribution",
    "sourceEdition",
    "disabled"
]
const PROFILE_FIELDS = ["disabled", "centerLongitude", "centerLatitude", "zoom"]
const keptOf = (row, fields) => (row ? Object.fromEntries(fields.map(field => [field, row[field] ?? null])) : null)
const out = {}

spec({
    timeout: 900,
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        http.allowNetwork([HOST])
        // the packaged local profile and its layers (framework data/MapsSetupData.xml): the features say when they are absent
        sys(() => {
            for (const layerId of LAYERS)
                kept.layers[layerId] = keptOf(
                    ec.entity.find("moqui.maps.MapLayer").condition({layerId}).useCache(false).one(),
                    LAYER_FIELDS
                )
            kept.profile = keptOf(
                ec.entity.find("moqui.maps.MapProfile").condition({profileId: PROFILE}).useCache(false).one(),
                PROFILE_FIELDS
            )
        })
    },
    cleanupSpec() {
        ec.message.clearErrors()
        http.reset()
        sys(() => {
            for (const layerId of LAYERS)
                if (kept.layers[layerId])
                    ec.entity.makeValue("moqui.maps.MapLayer").setAll(Object.assign({layerId}, kept.layers[layerId])).update()
            if (kept.profile)
                ec.entity
                    .makeValue("moqui.maps.MapProfile")
                    .setAll(Object.assign({profileId: PROFILE}, kept.profile))
                    .update()
            ec.resource.getLocationReference(LOCATION).delete()
            ec.resource.getLocationReference("runtime://maps/protomaps-extract-spec").delete()
            for (let i = 0; i < scratch; i++)
                for (const name of [SCRATCH + "/part-" + i, SCRATCH + "/part-" + i + ".gz"])
                    ec.resource.getLocationReference(name).delete()
            ec.resource.getLocationReference(SCRATCH).delete()
        })
    }
})

feature("a Florence extract of a recent Protomaps build, copied by ranges into a new archive", {
    when() {
        const started = Date.now()
        // the newest daily build of the last two weeks
        const today = new Date()
        let first = null
        for (let back = 0; back < 14 && !first; back++) {
            const day = new Date(today.getTime() - back * 86400000)
            source = HOST + day.getUTCFullYear() + two(day.getUTCMonth() + 1) + two(day.getUTCDate()) + ".pmtiles"
            try {
                first = range(0, 16384)
            } catch (error) {
                first = null
            }
        }
        if (!first) throw new Error("no Protomaps build of the last two weeks answered")
        const header = headerOf(first)
        const root = directoryOf(
            unpacked(first.slice(header.rootOffset, header.rootOffset + header.rootLength), header.internalCompression)
        )
        // its metadata, read as Latin-1 (only its ASCII parts are kept)
        const metadataBytes = unpacked(range(header.metadataOffset, header.metadataLength), header.internalCompression)
        let metadataText = ""
        for (let at = 0; at < metadataBytes.length; at += 4096)
            metadataText += String.fromCharCode(...metadataBytes.slice(at, at + 4096))
        const metadata = JSON.parse(metadataText)
        const tiles = []
        for (let z = 0; z <= MAX_ZOOM; z++)
            for (const tile of lib.viewportTiles(z, BOX)) {
                const entry = findTile(header, root, tile.tileId)
                if (entry)
                    tiles.push({
                        z,
                        x: tile.x,
                        y: tile.y,
                        tileId: tile.tileId,
                        bytes: range(header.tileOffset + entry.offset, entry.length)
                    })
            }
        // ASCII metadata: the vector layers' ids, the attribution and the build's version
        const ascii = value => String(value ?? "").replace(/[^\x20-\x7e]/g, "")
        const built = lib.archive(tiles, {
            tileType: header.tileType,
            tileCompression: header.tileCompression,
            minZoom: 0,
            maxZoom: MAX_ZOOM,
            bounds: BOX,
            center: [(BOX[0] + BOX[2]) / 2, (BOX[1] + BOX[3]) / 2, 13],
            metadata: {
                name: "Florence extract",
                attribution: ascii(metadata.attribution) || "Protomaps (c) OpenStreetMap",
                version: ascii(metadata.version),
                type: "baselayer",
                vector_layers: Array.from(metadata.vector_layers || []).map(layer => ({id: ascii(layer.id), fields: {}}))
            }
        })
        sys(() => lib.put(LOCATION, built.bytes))
        Object.assign(out, {source, header, tiles, built})
        log("extract", {
            source,
            tiles: tiles.length,
            requests,
            downloadedBytes: downloaded,
            archiveBytes: built.bytes.length,
            ms: Date.now() - started
        })
        return {header, tiles, size: built.bytes.length}
    },
    then({header, tiles, size}) {
        header.tileType == 1
        header.maxZoom >= MAX_ZOOM
        tiles.length > 20
        tiles.some(tile => tile.z == MAX_ZOOM)
        size < 10 * 1024 * 1024
    }
})

feature("both local layers are registered from the one archive in one action and drawn through the map-source transition", {
    when() {
        if (!kept.profile || LAYERS.some(layerId => !kept.layers[layerId]))
            throw new Error(
                "The seed's LOCAL_PROTOMAPS profile and its two layers are not there (framework data/MapsSetupData.xml)"
            )
        const registered =
            ec.service
                .sync()
                .name(S + "register#ArchiveLayer")
                .parameters({layerId: LAYERS[0], resourceLocation: LOCATION, alsoLayerIds: [LAYERS[1]]})
                .call() || {}
        const errors = ec.message.getErrorsString()
        ec.message.clearErrors()
        const rows = sys(() =>
            LAYERS.map(layerId => ec.entity.find("moqui.maps.MapLayer").condition({layerId}).useCache(false).one())
        )
        const enabled = ec.service
            .sync()
            .name("org.moqui.maps.MapServices.store#ProfileSettings")
            .parameters({profileId: PROFILE, enabled: "Y"})
            .call()
        const config = ec.service
            .sync()
            .name("org.moqui.maps.MapServices.get#EffectiveConfig")
            .parameters({profileId: PROFILE, recordLayers: false})
            .call()?.config
        ec.message.clearErrors()
        // a few tiles of the highest zooms: their ranges through the transition, and their layers decoded
        const sample = out.tiles.filter(tile => tile.z >= 12).slice(0, 6)
        const reads = sample.map(tile => {
            const entry = out.built.entries.find(row => row.tileId === tile.tileId)
            const res = ec.screen
                .makeTest()
                .rootScreen("component://webroot/screen/webroot.xml")
                .header("Range", "bytes=" + entry.offset + "-" + (entry.offset + entry.length - 1))
                .render("apps/maps/LocalMap/source", {layerId: LAYERS[0], revision: rows[0].sourceRevision}, "get")
            return {
                z: tile.z,
                status: res.status,
                range: res.header("Content-Range"),
                expected: "bytes " + entry.offset + "-" + (entry.offset + entry.length - 1) + "/" + out.built.bytes.length,
                etag: res.header("ETag"),
                layers: layerNames(unpacked(tile.bytes, out.header.tileCompression))
            }
        })
        log(
            "reads",
            reads.map(read => ({z: read.z, status: read.status, layers: read.layers}))
        )
        return {registered, errors, rows, enabled, config, reads}
    },
    then({registered, errors, rows, enabled, config, reads}) {
        errors == ""
        registered.applied == true
        registered.layerIds.join() == LAYERS.join()
        rows.every(row => row.disabled == "N" && row.resourceLocation == LOCATION && row.sourceRevision == rows[0].sourceRevision)
        rows[0].maxZoom == MAX_ZOOM
        JSON.stringify(registered.metadata?.vectorLayers).includes('"earth"')
        enabled.applied == true
        config.profile.basemaps[0].kind == "pmtiles"
        config.profile.basemaps[0].url.startsWith("/apps/maps/LocalMap/source?")
        reads.length > 3
        reads.every(read => read.status == 206 && read.range == read.expected && read.etag == '"' + rows[0].sourceTag + '"')
        reads.every(read => read.layers.includes("earth"))
        reads.some(read => read.layers.includes("water"))
        reads.some(read => read.layers.includes("roads"))
        reads.some(read => read.layers.includes("buildings"))
    }
})
