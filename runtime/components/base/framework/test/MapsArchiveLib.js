// PMTiles version 3 archives made in a spec (plans/Maps.md §5), for the map archive specs (MapsSource, MapsArchiveBackends,
// MapsLocalFixtures): an archive of given tiles with an uncompressed root directory and metadata, the tile IDs of the format
// (pmtiles' zxyToTileId, its Hilbert order), tiles of ASCII text a range read can be compared by, and a vector tile (MVT)
// of an "earth" polygon and a "water" polygon that the Protomaps styles draw. Loaded with
// ec.resource.script("component://framework/test/MapsArchiveLib.js").

// pmtiles 4.5.0's zxyToTileId and rotate, as the browser's reader computes a tile's ID
function rotate(n, x, y, rx, ry) {
    if (ry === 0) {
        if (rx !== 0) return [n - 1 - y, n - 1 - x]
        return [y, x]
    }
    return [x, y]
}
function zxyToTileId(z, x, y) {
    let acc = ((1 << z) * (1 << z) - 1) / 3
    let a = z - 1
    let tx = x
    let ty = y
    for (let s = 1 << a; s > 0; s >>= 1) {
        const rx = tx & s
        const ry = ty & s
        acc += ((3 * rx) ^ ry) * (1 << a)
        const turned = rotate(s, tx, ty, rx, ry)
        tx = turned[0]
        ty = turned[1]
        a--
    }
    return acc
}
// the tile x and y of a longitude and latitude at a zoom (web mercator)
function tileOf(z, lon, lat) {
    const n = 1 << z
    const x = Math.min(n - 1, Math.max(0, Math.floor(((lon + 180) / 360) * n)))
    const rad = (lat * Math.PI) / 180
    const y = Math.min(n - 1, Math.max(0, Math.floor(((1 - Math.log(Math.tan(rad) + 1 / Math.cos(rad)) / Math.PI) / 2) * n)))
    return {x, y}
}
// The tiles of a viewport (west, south, east, north) at a zoom: [{z, x, y, tileId}]
function viewportTiles(z, extent) {
    const [west, south, east, north] = extent
    const low = tileOf(z, west, north)
    const high = tileOf(z, east, south)
    const out = []
    for (let x = low.x; x <= high.x; x++) for (let y = low.y; y <= high.y; y++) out.push({z, x, y, tileId: zxyToTileId(z, x, y)})
    return out
}

const varints = values => {
    const out = []
    for (let value of values) {
        while (value >= 128) {
            out.push(value % 128 | 128)
            value = Math.floor(value / 128)
        }
        out.push(value)
    }
    return out
}
const ascii = text => Uint8Array.from(Array.from(text, c => c.charCodeAt(0) & 255))

// An archive of tiles [{tileId, bytes}] (any order; the same bytes object twice is one content), as
// {bytes, entries: [{tileId, offset, length}] (offset from the start of the file)}. options: tileType (1 mvt, 2 png),
// tileCompression (1 none, 2 gzip), minZoom, maxZoom, bounds [west, south, east, north], center [lon, lat, zoom],
// metadata (an object). Directories and metadata are not compressed.
function archive(tiles, options = {}) {
    const sorted = tiles.slice().sort((a, b) => a.tileId - b.tileId)
    const contents = []
    const placed = new Map()
    let offset = 0
    const rows = sorted.map(tile => {
        let at = placed.get(tile.bytes)
        if (at === undefined) {
            at = offset
            placed.set(tile.bytes, at)
            contents.push(tile.bytes)
            offset += tile.bytes.length
        }
        return {tileId: tile.tileId, offset: at, length: tile.bytes.length}
    })
    // the directory: count, tile ID deltas, run lengths, lengths, offsets + 1
    let last = 0
    const directory = varints(
        [rows.length]
            .concat(
                rows.map(row => {
                    const delta = row.tileId - last
                    last = row.tileId
                    return delta
                })
            )
            .concat(rows.map(() => 1))
            .concat(rows.map(row => row.length))
            .concat(rows.map(row => row.offset + 1))
    )
    if (127 + directory.length > 16384) throw new Error("the spec archive's root directory is past 16 KiB")
    const metadata = ascii(
        JSON.stringify(options.metadata || {name: "spec", attribution: "Spec data", vector_layers: [{id: "earth"}]})
    )
    const metadataOffset = 127 + directory.length
    const tileOffset = metadataOffset + metadata.length
    const bytes = new Uint8Array(tileOffset + offset)
    const view = new DataView(bytes.buffer)
    Array.from("PMTiles").forEach((c, i) => (bytes[i] = c.charCodeAt(0)))
    bytes[7] = 3
    const u64 = (at, value) => {
        view.setUint32(at, value % 4294967296, true)
        view.setUint32(at + 4, Math.floor(value / 4294967296), true)
    }
    ;[
        127,
        directory.length,
        metadataOffset,
        metadata.length,
        0,
        0,
        tileOffset,
        offset,
        rows.length,
        rows.length,
        contents.length
    ].forEach((value, i) => u64(8 + i * 8, value))
    const zooms = sorted.map(tile => tileIdZoom(tile.tileId))
    const [west, south, east, north] = options.bounds || [-180, -85, 180, 85]
    const center = options.center || [0, 0, 0]
    bytes[96] = 1
    bytes[97] = 1
    bytes[98] = options.tileCompression || 1
    bytes[99] = options.tileType || 1
    bytes[100] = options.minZoom ?? Math.min(...zooms)
    bytes[101] = options.maxZoom ?? Math.max(...zooms)
    const e7 = degrees => Math.round(degrees * 10000000)
    view.setInt32(102, e7(west), true)
    view.setInt32(106, e7(south), true)
    view.setInt32(110, e7(east), true)
    view.setInt32(114, e7(north), true)
    bytes[118] = center[2]
    view.setInt32(119, e7(center[0]), true)
    view.setInt32(123, e7(center[1]), true)
    bytes.set(directory, 127)
    bytes.set(metadata, metadataOffset)
    let at = tileOffset
    for (const content of contents) {
        bytes.set(content, at)
        at += content.length
    }
    return {bytes, entries: rows.map(row => ({tileId: row.tileId, offset: tileOffset + row.offset, length: row.length}))}
}
function tileIdZoom(tileId) {
    let z = 0
    let first = 0
    while (first + (1 << z) * (1 << z) <= tileId) {
        first += (1 << z) * (1 << z)
        z++
    }
    return z
}

// Every tile from zoom 0 to maxZoom as ASCII text naming it and the revision, of a length that differs by tile (and by
// revision, so a replaced archive's size changes): [{z, x, y, tileId, bytes, text}]
function asciiTiles(maxZoom, revision) {
    const out = []
    for (let z = 0; z <= maxZoom; z++)
        for (let x = 0; x < 1 << z; x++)
            for (let y = 0; y < 1 << z; y++) {
                const tileId = zxyToTileId(z, x, y)
                const unit = "z" + z + "x" + x + "y" + y + "-r" + revision + ";"
                const length = 600 + (tileId % 97) + revision
                const text = unit.repeat(Math.ceil(length / unit.length)).slice(0, length)
                out.push({z, x, y, tileId, text, bytes: ascii(text)})
            }
    return out
}

// A Mapbox Vector Tile (version 2, extent 4096) with a layer "earth" of one polygon over the whole tile and a layer "water"
// of one polygon over its lower right quarter, each with kind properties, as the Protomaps styles read them.
function mvtTile() {
    const bytesOf = list => list
    const key = (field, wire) => (field << 3) | wire
    const lengthDelimited = (field, inner) => [key(field, 2)].concat(varints([inner.length]), inner)
    const zigzag = n => (n << 1) ^ (n >> 31)
    const polygon = (x0, y0, x1, y1) =>
        varints([
            (1 & 7) | (1 << 3), // MoveTo 1
            zigzag(x0),
            zigzag(y0),
            (2 & 7) | (3 << 3), // LineTo 3
            zigzag(x1 - x0),
            zigzag(0),
            zigzag(0),
            zigzag(y1 - y0),
            zigzag(x0 - x1),
            zigzag(0),
            (7 & 7) | (1 << 3) // ClosePath
        ])
    const stringValue = text => lengthDelimited(1, Array.from(ascii(text)))
    const layer = (name, geometry, kind) => {
        const feature = [key(1, 0)]
            .concat(varints([1]))
            .concat(lengthDelimited(2, varints([0, 0]))) // tags: key 0, value 0
            .concat([key(3, 0)], varints([3])) // POLYGON
            .concat(lengthDelimited(4, geometry))
        const body = [key(15, 0)]
            .concat(varints([2])) // version
            .concat(lengthDelimited(1, Array.from(ascii(name))))
            .concat(lengthDelimited(2, feature))
            .concat(lengthDelimited(3, Array.from(ascii("kind"))))
            .concat(lengthDelimited(4, stringValue(kind)))
            .concat([key(5, 0)], varints([4096]))
        return lengthDelimited(3, body)
    }
    return Uint8Array.from(
        bytesOf(
            layer("earth", polygon(-64, -64, 4160, 4160), "earth").concat(
                layer("water", polygon(2048, 2048, 4160, 4160), "water")
            )
        )
    )
}

const put = (location, bytes) => ec.resource.getLocationReference(location).putBytes(bytes)

;({zxyToTileId, viewportTiles, archive, asciiTiles, mvtTile, put, ascii})
