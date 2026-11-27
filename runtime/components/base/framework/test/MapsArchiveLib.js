// PMTiles version 3 archives made in a spec (plans/archive/Maps.md §5), and a small Cloud Optimized GeoTIFF (cog), for the map archive specs (MapsSource, MapsArchiveBackends,
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

// A Cloud Optimized GeoTIFF made in a spec (P4c, the COG layers of MapsSource): a little-endian classic TIFF of one uint8
// band, size by size pixels in tiles of 256 (uncompressed, each tile's bytes after the header), georeferenced in an EPSG
// CRS by a pixel scale and a tiepoint (its GeoKeys: a projected CRS by ProjectedCSTypeGeoKey), with GDAL's no-data. Its
// directory and values are in its first bytes, as a COG's are. options: {size, epsg, origin: [x, y], pixel, strips}
function cog(options = {}) {
    const size = options.size || 512
    const tile = 256
    const across = Math.ceil(size / tile)
    const tiles = across * across
    const epsg = options.epsg || 32633
    const [originX, originY] = options.origin || [500000, 6000000]
    const pixel = options.pixel || 10
    const entries = [
        [256, 4, [size]], // ImageWidth
        [257, 4, [size]], // ImageLength
        [258, 3, [8]], // BitsPerSample
        [259, 3, [1]], // Compression: none
        [262, 3, [1]], // Photometric: black is zero
        [277, 3, [1]], // SamplesPerPixel
        [284, 3, [1]], // PlanarConfiguration
        options.strips ? [273, 4, [0]] : [322, 3, [tile]], // StripOffsets, or TileWidth
        options.strips ? [278, 4, [size]] : [323, 3, [tile]], // RowsPerStrip, or TileLength
        [324, 4, new Array(tiles).fill(0)], // TileOffsets, set below
        [325, 4, new Array(tiles).fill(tile * tile)], // TileByteCounts
        [339, 3, [1]], // SampleFormat: unsigned
        [33550, 12, [pixel, pixel, 0]], // ModelPixelScale
        [33922, 12, [0, 0, 0, originX, originY, 0]], // ModelTiepoint
        [34735, 3, [1, 1, 0, 3, 1024, 0, 1, 1, 1025, 0, 1, 1, 3072, 0, 1, epsg]], // GeoKeyDirectory
        [42113, 2, "0"] // GDAL_NODATA
    ].sort((a, b) => a[0] - b[0])
    const size_of = {2: 1, 3: 2, 4: 4, 12: 8}
    const valueBytes = ([, type, values]) => (type === 2 ? values.length + 1 : values.length * size_of[type])
    const ifdStart = 8
    const tableEnd = ifdStart + 2 + entries.length * 12 + 4
    let extraAt = tableEnd
    const placed = entries.map(entry => {
        const length = valueBytes(entry)
        if (length <= 4) return {entry, offset: null}
        const offset = extraAt
        extraAt += length + (length % 2)
        return {entry, offset}
    })
    const dataStart = extraAt
    const total = dataStart + tiles * tile * tile
    const view = new DataView(new ArrayBuffer(total))
    const bytes = new Uint8Array(view.buffer)
    bytes[0] = 73
    bytes[1] = 73
    view.setUint16(2, 42, true)
    view.setUint32(4, ifdStart, true)
    view.setUint16(ifdStart, entries.length, true)
    const writeValues = (at, [, type, values]) => {
        if (type === 2) {
            for (let i = 0; i < values.length; i++) bytes[at + i] = values.charCodeAt(i)
            bytes[at + values.length] = 0
            return
        }
        values.forEach((value, index) => {
            if (type === 3) view.setUint16(at + index * 2, value, true)
            else if (type === 4) view.setUint32(at + index * 4, value, true)
            else view.setFloat64(at + index * 8, value, true)
        })
    }
    placed.forEach(({entry, offset}, index) => {
        const [tag, type, values] = entry
        if (tag === 324) entry[2] = values.map((unused, n) => dataStart + n * tile * tile)
        const at = ifdStart + 2 + index * 12
        view.setUint16(at, tag, true)
        view.setUint16(at + 2, type, true)
        view.setUint32(at + 4, type === 2 ? values.length + 1 : entry[2].length, true)
        if (offset === null) writeValues(at + 8, entry)
        else {
            view.setUint32(at + 8, offset, true)
            writeValues(offset, entry)
        }
    })
    view.setUint32(ifdStart + 2 + entries.length * 12, 0, true)
    // each pixel a value from its position: the tiles a range read can be compared by
    for (let i = dataStart; i < total; i++) bytes[i] = (i * 13) % 251
    return bytes
}

;({zxyToTileId, viewportTiles, archive, asciiTiles, mvtTile, put, ascii, cog})
