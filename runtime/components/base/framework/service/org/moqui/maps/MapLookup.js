// The local postal lookup of org.moqui.maps.MapServices.get#PostalLocation (plans/archive/Maps.md §6.1, §6.3): a country and a
// postal code to the code's representative point in the imported Geo data (GeoDataServices, docs/tools/Maps.md "Geo data"),
// with its source, edition and precision. It reads only public reference data and makes no outside request.
//
// - The country is a Geo ID or its ISO alpha-2 or alpha-3 code; the code is read by the importer's own rules
//   (normalize#PostalCode, moqui_lib::geo::postal), so it is found by the normalized form it was stored under: leading zeros
//   kept, a Canadian, British or Dutch code spaced as its country writes it, a US ZIP+4 read as its ZIP code.
// - The point is the code's active representative: its in-date GeoPointAssoc of role GPR_REPRESENTATIVE with the lowest
//   priority (the data set's: in the US the Census ZCTA point before GeoNames'), never averaged with another source's. A
//   user-confirmed point (GPP_USER_CONFIRMED) and a record's own location (GPT_RECORD_LOCATION) are never a postal
//   representative: they are a record's place, which its adapter answers.
// - A full code with no row falls back to its country's declared prefix (a forward sortation area, an outward code), a
//   lower precision said as such (GPP_PLACE_ESTIMATE and matchKind prefix); no other country's code is ever cut. A prefix
//   code (GPG_PREFIX), found so or asked for, is said to be only the first part of the code, with a wider extent.
// - The extent is a box around the point broad enough for a postal area (wider for a prefix), widened to the code's place
//   points nearby; it is not the area's boundary, and no accuracy in meters is made up. Where the active point's source
//   gave the area's land area (a Census ZCTA's ALAND, GeoPointAssoc.landArea) the box is a square of that area, within
//   limits; else a fixed size.
// - A code with several place names is ambiguous: its names are aliases of the one area, listed as choices, none chosen.
// - Puerto Rico, the US Virgin Islands, Guam, American Samoa and the Northern Mariana Islands have US ZIP codes, which
//   the US data sets (the Census ZCTAs) hold: a code of one of them not found under its own country is looked up as a US
//   code.
// Results are cached in maps.postal.lookup (MoquiDefaultConf.xml: bounded, a day) as data with no words of a language,
// keyed by country and normalized code, and put in the user's words when read; an import or a Remove clears it.
const CACHE = "maps.postal.lookup"
const NORMALIZE = "org.moqui.maps.GeoDataServices.normalize#PostalCode"
const APPROXIMATE = "Approximate location — postal area"
// half the height of the box around a point, in degrees of latitude: a full code's area, a prefix's district
const HALF_HEIGHT = {code: 0.08, prefix: 0.3}
// a box sized from a land area: half its side within these, in degrees of latitude (about 0.5 km to 55 km)
const AREA_HALF_HEIGHT = {min: 0.005, max: 0.5}
const METERS_PER_DEGREE = 111320
// a place point further than this from the representative point is not taken into the extent
const PLACE_REACH = 1.0
const MAX_PLACES = 50
// territories whose postal codes are US ZIP codes, in the US data sets
const US_ZIP_TERRITORIES = ["PRI", "VIR", "GUM", "ASM", "MNP"]
const PREFIX_NOTE = "only the first part of the code is known here"
const COUNTRIES_KEY = "_countries"

const L = value => ec.l10n.localize(value)
const text = value => (value == null ? "" : String(value).trim())
const find = (entity, conditions) =>
    ec.entity
        .find(entity)
        .condition(conditions || {})
        .disableAuthz()
const cached = entity => (entity, conditions) => find(entity, conditions).useCache(true)
const inDate = () => ec.entity.conditionFactory.makeCondition("thruDate", "greater-than", ec.user.nowTimestamp, true)

// The country Geo of a Geo ID or an alpha-2 or alpha-3 code; null when none
function countryOf(value) {
    const code = text(value).toUpperCase()
    if (!/^[A-Z][A-Z0-9_]{1,19}$/.test(code)) return null
    const byId = cached()("moqui.basic.Geo", {geoId: code}).one()
    if (byId && byId.geoTypeEnumId === "GEOT_COUNTRY") return byId
    const field = code.length === 2 ? "geoCodeAlpha2" : code.length === 3 ? "geoCodeAlpha3" : null
    if (!field) return null
    return cached()("moqui.basic.Geo", {geoTypeEnumId: "GEOT_COUNTRY", [field]: code}).list()[0] || null
}

// Half the height of a full code's box, in degrees of latitude, from its land area in square meters: half the side of a
// square of that area, within AREA_HALF_HEIGHT; null with no area
function areaHalfHeight(landArea) {
    const area = Number(landArea)
    if (!Number.isFinite(area) || area <= 0) return null
    const half = Math.sqrt(area) / 2 / METERS_PER_DEGREE
    return Math.min(Math.max(half, AREA_HALF_HEIGHT.min), AREA_HALF_HEIGHT.max)
}

// [west, south, east, north] around a point, widened to the place points within reach; a full code's box sized from its
// land area where there is one
function extentAround(lon, lat, kind, places, landArea) {
    const halfHeight = (kind === "code" && areaHalfHeight(landArea)) || HALF_HEIGHT[kind] || HALF_HEIGHT.code
    const halfWidth = Math.min(halfHeight / Math.max(Math.cos((lat * Math.PI) / 180), 0.2), 5)
    let [west, south, east, north] = [lon - halfWidth, lat - halfHeight, lon + halfWidth, lat + halfHeight]
    for (const place of places) {
        if (place.lon == null || Math.abs(place.lon - lon) > PLACE_REACH || Math.abs(place.lat - lat) > PLACE_REACH) continue
        west = Math.min(west, place.lon - halfWidth / 4)
        east = Math.max(east, place.lon + halfWidth / 4)
        south = Math.min(south, place.lat - halfHeight / 4)
        north = Math.max(north, place.lat + halfHeight / 4)
    }
    const round = value => Math.round(value * 1e5) / 1e5
    return [Math.max(west, -180), Math.max(south, -90), Math.min(east, 180), Math.min(north, 90)].map(round)
}

// The code's active representative point: the in-date representative association of lowest priority whose point is a
// Geo reference point, never a user-confirmed one or a record's location
function representative(geoId) {
    // a row with no priority is last, whatever the database's order of nulls
    const rank = assoc => (assoc.priority == null ? Number.MAX_SAFE_INTEGER : Number(assoc.priority))
    const assocs = Array.from(
        find("moqui.basic.GeoPointAssoc", {geoId, pointRoleEnumId: "GPR_REPRESENTATIVE"})
            .conditionDate("fromDate", "thruDate", null)
            .condition("precisionEnumId", "not-equals", "GPP_USER_CONFIRMED")
            .limit(50)
            .list()
    ).sort((a, b) => rank(a) - rank(b) || text(a.geoPointId).localeCompare(text(b.geoPointId)))
    for (const assoc of assocs) {
        const point = find("moqui.basic.GeoPoint", {geoPointId: assoc.geoPointId}).one()
        if (!point || point.geoPointTypeEnumId === "GPT_RECORD_LOCATION") continue
        const lat = Number(point.latitude),
            lon = Number(point.longitude)
        if (!Number.isFinite(lat) || !Number.isFinite(lon)) continue
        return {assoc, lon, lat}
    }
    return null
}

// The code's in-date place names, in order, each with its own point where the row has one (never a record's location)
function placesOf(geoId) {
    const rows = Array.from(
        find("moqui.basic.GeoPostalCodePlace", {geoId})
            .condition(inDate())
            .orderBy("sequenceNum,placeName")
            .limit(MAX_PLACES)
            .list()
    )
    const seen = new Set()
    const out = []
    for (const row of rows) {
        const key = text(row.placeName).toLowerCase() + "|" + text(row.adminName1).toLowerCase()
        if (!text(row.placeName) || seen.has(key)) continue
        seen.add(key)
        const place = {
            name: text(row.placeName),
            adminName1: text(row.adminName1) || null,
            adminName2: text(row.adminName2) || null,
            sourceKey: row.sourceKey
        }
        if (row.geoPointId) {
            const point = find("moqui.basic.GeoPoint", {geoPointId: row.geoPointId}).one()
            if (point && point.geoPointTypeEnumId !== "GPT_RECORD_LOCATION" && Number.isFinite(Number(point.latitude))) {
                place.lon = Number(point.longitude)
                place.lat = Number(point.latitude)
            }
        }
        out.push(place)
    }
    return out
}

// A match of postal code row `code` (found as asked, kind code, or as the declared prefix of a full code, kind prefix): its
// point, source, precision, extent and place names, with no words of a language (localized() adds them); null when it has
// no point
function matchOf(code, kind) {
    const found = representative(code.geoId)
    if (!found) return null
    const {assoc, lon, lat} = found
    const set = assoc.dataSetId ? cached()("moqui.maps.GeoDataSet", {dataSetId: assoc.dataSetId}).one() : null
    const source = set && set.dataSourceId ? cached()("moqui.basic.DataSource", {dataSourceId: set.dataSourceId}).one() : null
    const places = placesOf(code.geoId)
    const prefix = code.granularityEnumId === "GPG_PREFIX"
    // a full code's area found only by its prefix is a place estimate, whatever the prefix's own point
    const precisionEnumId = kind === "prefix" ? "GPP_PLACE_ESTIMATE" : text(assoc.precisionEnumId) || "GPP_POSTAL_AREA"
    const edition = text(assoc.edition)
    const sourceName = text(source?.description) || text(set?.publisher) || text(assoc.dataSetId)
    return {
        geoId: code.geoId,
        countryGeoId: code.countryGeoId,
        normalizedCode: code.normalizedCode,
        displayCode: text(code.displayCode) || code.normalizedCode,
        granularityEnumId: code.granularityEnumId,
        matchKind: kind,
        lon,
        lat,
        extent: extentAround(lon, lat, prefix ? "prefix" : "code", places, assoc.landArea),
        precisionEnumId,
        approximate: true,
        prefixOnly: prefix,
        dataSetId: text(assoc.dataSetId) || null,
        sourceKey: text(set?.sourceKey) || null,
        publisher: text(set?.publisher) || null,
        edition: edition || null,
        attribution: text(set?.attribution) || null,
        licenseName: text(set?.licenseName) || null,
        sourceText: [sourceName, edition].filter(part => part).join(", "),
        placeNames: places
    }
}

// The in-date postal code rows of a country, code and granularity
function codeRows(countryGeoId, normalizedCode, granularityEnumId) {
    return Array.from(
        find("moqui.basic.GeoPostalCode", {countryGeoId, normalizedCode, granularityEnumId}).condition(inDate()).limit(5).list()
    )
}

// A cached match in the user's words
const localized = match =>
    Object.assign({}, match, {
        precisionText: match.prefixOnly ? L(APPROXIMATE) + " (" + L(PREFIX_NOTE) + ")" : L(APPROXIMATE)
    })

function lookup(country, read) {
    let kind = "code"
    let rows = codeRows(country.geoId, read.normalizedCode, read.granularityEnumId)
    if (!rows.length && read.prefixCode) {
        kind = "prefix"
        rows = codeRows(country.geoId, read.prefixCode, "GPG_PREFIX")
    }
    const matches = rows.map(row => matchOf(row, kind)).filter(match => match)
    return matches
}

// The code read by `country`'s rules, and its matches, cached; read.normalizedCode is empty when the text is no code of it
function lookupIn(country, postalCode) {
    // the importer's own rules: a pure function, called with authorization off as this service's own step
    const read =
        ec.service
            .sync()
            .name(NORMALIZE)
            .disableAuthz()
            .parameters({
                country: country.geoCodeAlpha2 || country.geoCodeAlpha3 || country.geoId,
                postalCode: String(postalCode).slice(0, 40)
            })
            .call() || {}
    if (!read.normalizedCode) return {read, matches: []}
    const key = country.geoId + "|" + read.normalizedCode + "|" + read.granularityEnumId
    const cache = ec.cache.getCache(CACHE)
    let matches = null
    const hit = cache.get(key)
    if (hit) {
        try {
            matches = JSON.parse(String(hit))
        } catch (error) {
            matches = null
        }
    }
    if (!Array.isArray(matches)) {
        matches = lookup(country, read)
        cache.put(key, JSON.stringify(matches))
    }
    return {read, matches: matches.map(localized)}
}

function getPostalLocation(p) {
    const answer = (reasonCode, reason, extra) =>
        Object.assign({matches: [], ambiguous: false, reasonCode, reason: reason ? L(reason) : null}, extra || {})
    const country = countryOf(p.countryGeoId)
    if (!country) return answer("COUNTRY_UNKNOWN", "Choose the postal code's country")
    if (!text(p.postalCode)) return answer("CODE_INVALID", "Enter a postal code")
    let {read, matches} = lookupIn(country, p.postalCode)
    // a US territory's ZIP code is in the US data sets
    if (!matches.length && US_ZIP_TERRITORIES.includes(country.geoId)) {
        const us = countryOf("USA")
        const asUs = us ? lookupIn(us, p.postalCode) : {matches: []}
        if (asUs.matches.length) ({read, matches} = asUs)
    }
    if (!read.normalizedCode)
        return answer("CODE_INVALID", read.reason || "This is not a postal code of that country", {countryGeoId: country.geoId})
    const base = {
        countryGeoId: country.geoId,
        normalizedCode: read.normalizedCode,
        displayCode: read.displayCode,
        zipPlus4: !!read.zipPlus4
    }
    if (!matches.length) return answer("NO_MATCH", "No imported Geo data has this postal code", base)
    const ambiguous = matches.length > 1 || matches.some(match => (match.placeNames || []).length > 1)
    return Object.assign({matches, ambiguous, reasonCode: null, reason: null}, base)
}

// ===== Browse's search (tools screen/Maps/Browse.xml search) =====
// The countries with postal codes in use (in-date GeoPostalCode rows), cached with the lookups (an import clears them)
function postalCountries() {
    const cache = ec.cache.getCache(CACHE)
    try {
        const hit = cache.get(COUNTRIES_KEY)
        if (hit) return JSON.parse(String(hit))
    } catch (error) {
        // read again below
    }
    const rows = Array.from(
        find("moqui.basic.GeoPostalCode").condition(inDate()).selectField("countryGeoId").distinct(true).limit(300).list()
    )
    const out = []
    for (const row of rows) {
        if (!row.countryGeoId || out.some(country => country.geoId === row.countryGeoId)) continue
        const geo = cached()("moqui.basic.Geo", {geoId: row.countryGeoId}).one()
        out.push({geoId: row.countryGeoId, name: text(geo?.geoName) || row.countryGeoId, alpha2: text(geo?.geoCodeAlpha2)})
    }
    out.sort((a, b) => a.name.localeCompare(b.name))
    cache.put(COUNTRIES_KEY, JSON.stringify(out))
    return out
}
// The user's country: their locale's, else the installation's
function homeCountry() {
    const localeCountry = locale => countryOf(text(locale).split(/[_-]/)[1] || "")
    return localeCountry(ec.user.locale) || localeCountry(ec.factory.getConfValue("instance@default-locale"))
}
// get#PostalLocation as the caller, in a transaction of its own and with its errors kept apart: a lookup that fails leaves
// the search's other sources
function probeLookup(countryGeoId, postalCode) {
    ec.message.pushErrors()
    try {
        const found =
            ec.service
                .sync()
                .name("org.moqui.maps.MapServices.get#PostalLocation")
                .parameters({countryGeoId, postalCode})
                .requireNewTransaction(true)
                .call() || {}
        if (!ec.message.hasError()) return found
        ec.logger.error("The postal lookup of a search failed: " + ec.message.getErrorsString())
    } catch (error) {
        ec.logger.error("The postal lookup of a search failed: " + (error?.message ?? error))
    } finally {
        ec.message.clearErrors()
        ec.message.popErrors()
    }
    return {failed: true, reason: L("The postal lookup is not available")}
}
// A search's results of one lookup: the code, and for a code with several places each place with its own point
function resultsOf(found, countryName) {
    const out = []
    const suffix = countryName ? " · " + countryName : ""
    for (const match of Array.from(found.matches || [])) {
        const names = (match.placeNames || []).map(place => place.name)
        out.push({
            kind: "postal",
            id: "postal:" + match.geoId,
            countryGeoId: match.countryGeoId,
            label:
                match.displayCode +
                (names.length ? " — " + names.slice(0, 4).join(", ") + (names.length > 4 ? ", …" : "") : "") +
                suffix,
            sourceText: match.sourceText,
            precisionEnumId: match.precisionEnumId,
            precisionText: match.precisionText,
            lon: match.lon,
            lat: match.lat,
            extent: match.extent
        })
        if (names.length > 1)
            for (const place of match.placeNames.filter(place => place.lon != null).slice(0, 10))
                out.push({
                    kind: "postalPlace",
                    id: "postal:" + match.geoId + ":" + place.name,
                    countryGeoId: match.countryGeoId,
                    label:
                        place.name + (place.adminName1 ? ", " + place.adminName1 : "") + " (" + match.displayCode + ")" + suffix,
                    sourceText: match.sourceText,
                    precisionEnumId: "GPP_PLACE_ESTIMATE",
                    precisionText: L(APPROXIMATE),
                    lon: place.lon,
                    lat: place.lat,
                    extent: extentAround(place.lon, place.lat, "code", [])
                })
    }
    return out
}
// The postal results of Browse's search. The country is the one chosen; else one named in the text ("US 02134",
// "02134, US"), taken only when it has postal data and its rules read the rest as a code (so "MA 02134" is not Morocco
// while only US data is imported, and its code is 02134 all the same); else inferred: the user's country first, then every other country with postal data
// whose rules read the text as a code, the results of more than one country listed as choices, each labeled with its
// country. A text that does not look like a code (no digit, other characters) is no postal search. Changes nothing.
function searchPostal(p) {
    const countries = postalCountries()
    const nameOf = geoId => (countries.find(country => country.geoId === geoId) || {}).name || geoId
    const looksLikeCode = value => /\d/.test(value) && /^[A-Za-z0-9 -]{2,16}$/.test(value)
    const none = (countryGeoId, reason) => ({results: [], countries, countryGeoId, ambiguous: false, reason: reason || null})
    let rest = text(p.q)
    let country = text(p.country) ? countryOf(p.country) : null
    if (!country) {
        const leading = /^([A-Za-z]{2,3})[\s:,]+(.+)$/.exec(rest)
        const trailing = /^(.+?)\s*,\s*([A-Za-z]{2,3})$/.exec(rest)
        const tokens = [leading ? [leading[1], leading[2]] : null, trailing ? [trailing[2], trailing[1]] : null].filter(
            pair => pair
        )
        for (const [token, remainder] of tokens) {
            if (!looksLikeCode(text(remainder))) continue
            // the token is not part of the code ("MA 02134": a state, or a country), whether or not it is the country
            rest = text(remainder)
            const named = countryOf(token)
            if (!named || !countries.some(one => one.geoId === named.geoId)) break
            if (probeLookup(named.geoId, rest).reasonCode === "CODE_INVALID") break
            country = named
            break
        }
    }
    if (!looksLikeCode(rest)) return none(country ? country.geoId : null)
    if (country) {
        const found = probeLookup(country.geoId, rest)
        const results = resultsOf(found, "")
        return {
            results,
            countries,
            countryGeoId: country.geoId,
            ambiguous: !!found.ambiguous,
            reason: results.length ? null : found.reason || null
        }
    }
    // inferred: the user's country, then each other country with postal data whose rules read the text as a code
    const home = homeCountry()
    const order = (home ? [home.geoId] : []).concat(
        countries.map(one => one.geoId).filter(geoId => !home || geoId !== home.geoId)
    )
    const answered = []
    for (const geoId of order) {
        const found = probeLookup(geoId, rest)
        if (found.reasonCode === "CODE_INVALID" || found.reasonCode === "COUNTRY_UNKNOWN") continue
        answered.push({geoId, found})
    }
    const withMatches = answered.filter(answer => (answer.found.matches || []).length)
    if (!withMatches.length) {
        const first = answered[0]
        return none(
            first ? first.geoId : home ? home.geoId : null,
            first ? first.found.reason : L("No imported Geo data has this postal code")
        )
    }
    const several = withMatches.length > 1
    const results = withMatches.flatMap(answer => resultsOf(answer.found, several ? nameOf(answer.geoId) : "")).slice(0, 40)
    return {
        results,
        countries,
        countryGeoId: withMatches[0].geoId,
        ambiguous: several || withMatches.some(answer => !!answer.found.ambiguous),
        reason: several ? L("This code is a postal code of more than one country: choose one, or choose the country") : null
    }
}

const operations = {getPostalLocation, searchPostal}
;({
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown map lookup operation " + op)
        return operations[op](parameters || {})
    },
    APPROXIMATE
})
