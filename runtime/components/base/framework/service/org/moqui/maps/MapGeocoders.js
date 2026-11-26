// Address and place search of org.moqui.maps.MapServices.find#Places (plans/Maps.md §6.1, §6.2, §7, §8; docs/tools/Maps.md
// "Address and place search"): a text to candidate places from one configured geocoder, and the geocoder's connection test.
//
// A geocoder is a MapProvider of a geocoder kind (MapProviderKind under MPK_GEOCODER: MPK_GEOCODER_CENSUS,
// MPK_GEOCODER_NOMINATIM, MPK_GEOCODER_PHOTON, MPK_GEOCODER_OVERTURE), whose adapter here builds its request and reads its
// answer. A new service is a new kind and an entry of ADAPTERS; the provider row holds only its endpoint, attribution and
// policy, never a service name or code.
//
// A local geocoder (MPK_GEOCODER_OVERTURE, plans/Maps-Overture.md §O2) is this server's: the address points of the active
// Overture Addresses runs, searched by search#OvertureAddresses (moqui-maps) with no outside request, so it works with
// outside requests off, has no URL, rate or day's count, and is available only while a region's addresses are active
// (and the server has the overture feature). Its test is a fixed query made from its own data. A text it does not match
// falls back to the local postal lookup (get#PostalLocation of the postal code read from the text: approximate) and to
// choosing the point on the map; no outside service is asked in its place.
//
// Policy, each on the server:
// - Off by default: nothing is sent while maps@external-requests is false (EXTERNAL_REQUESTS_OFF) or no geocoder is enabled
//   and has passed its test as it is configured now (NO_GEOCODER). Only an explicit search calls this; opening a map or
//   the local-map dialog never does, so no address leaves the server unless the user searched for it.
// - One provider is asked: the one named, or the enabled, tested geocoder first by sequenceNum. A failure is said
//   (PROVIDER_FAILED, naming the provider) and never sent on to another outside service (§8); the local results stay.
// - One GET with no credentials, following no redirect, ten seconds, reading at most MAX_BYTES of the body whether or
//   not it has a Content-Length (the RestClient's text(maxBytes)), to no internal address unless the provider is allowed
//   the private network (allowPrivateNetwork, as the map providers'); and only when its allowedOperations is empty or
//   names geocode or search (OPERATION_NOT_ALLOWED). The log names the provider and the kind of failure, never the text.
// - Its rate: requestsPerSecond, or 1 for Nominatim and Photon when empty, refused rather than queued (RATE_LIMITED). A
//   request claims its slot with the cache's putIfAbsent, so two at once cannot both pass; the cache is each server's, so a
//   cluster of n servers may send n times the rate. Its requestsPerDay, when set, is the most searches it is sent in a
//   calendar day (UTC), refused past it (DAILY_LIMIT): a search claims one by locking its MapProviderDayCount row in a
//   transaction of its own before anything is sent, so the servers of a cluster share the day's count and the limit holds
//   for them together; a cached answer, a refused search and the connection test claim none. The public Nominatim and
//   Photon demo servers are named by the test and readiness, since their policies allow an application very little.
// - This server keeps an answer only when its cachePolicy is shared (none, empty and private keep nothing here: private
//   is the user's browser alone), for cacheSeconds or, when empty, the answer's own Cache-Control max-age (at most a
//   day; nothing with no-store, no-cache or private), under a digest of the provider and the request, never the text.
// - Each candidate is a label (plain text), a WGS84 point, its match kind (address, street, postal, place) and precision
//   (GeoPointPrecision): the US Census geocoder interpolates along a street's address range (GPP_STREET_RANGE, never a
//   rooftop); Nominatim and Photon say an address point, a street, a postal code or a place. Several candidates are
//   ambiguous: choices, none chosen. A provider's confidence is never made a distance.
const PROVIDER = "moqui.maps.MapProvider"
const DAY_COUNT = "moqui.maps.MapProviderDayCount"
const ANSWERS = "maps.geocoder.answer"
const LAST = "maps.geocoder.last"
const MAX_BYTES = 1024 * 1024
const MAX_LIMIT = 20
const US_AREAS = ["USA", "PRI", "VIR", "GUM", "ASM", "MNP"]
const OVERTURE_FIND = "org.moqui.maps.OvertureServices.search#OvertureAddresses"
const OVERTURE_ADDRESS = "moqui.maps.OvertureAddress"
const PRECISION = {
    GPP_ADDRESS_MATCH: "Address match",
    GPP_STREET_RANGE: "Street-range interpolation (not a rooftop position)",
    GPP_POSTAL_AREA: "Postal-area representative point",
    GPP_PLACE_ESTIMATE: "Place estimate"
}

const L = value => ec.l10n.localize(value)
const say = (value, values) => ec.resource.expand(L(value), "", values || {})
const text = value => (value == null ? "" : String(value).trim())
const offline = () => text(ec.factory.getConfValue("maps@external-requests")) === "false"
const production = () => text(ec.factory.getConfValue("instance@purpose")) === "production"
const privateAllowed = row => row?.allowPrivateNetwork === "Y" || (row?.allowPrivateNetwork !== "N" && !production())
const num = value => {
    const found = value == null || value === "" ? NaN : Number(value)
    return Number.isFinite(found) ? found : null
}
const plain = (value, max) =>
    text(value)
        .replace(/<[A-Za-z\/!?][^>]*>/g, " ")
        .replace(/[\u0000-\u001f\u007f​-‏‪-‮⁦-⁩]/g, " ")
        .replace(/\s+/g, " ")
        .trim()
        .slice(0, max || 200)
const refusal = (reasonCode, reason, extra) => Object.assign({candidates: [], ambiguous: false, reasonCode, reason}, extra || {})
const endpointOf = provider => text(provider.endpointUrl).replace(/\/+$/, "")
const enc = value => encodeURIComponent(value)
const point = (lon, lat) => {
    const x = num(lon)
    const y = num(lat)
    return x !== null && y !== null && x >= -180 && x <= 180 && y >= -90 && y <= 90 ? [x, y] : null
}
const extentOf = (west, south, east, north) => {
    const values = [west, south, east, north].map(num)
    return values.every(value => value !== null) ? values : null
}
// a candidate as find#Places answers it
function candidate(label, position, matchKind, precisionEnumId, extent) {
    return {
        label: plain(label),
        lon: position[0],
        lat: position[1],
        matchKind,
        precisionEnumId,
        precisionText: L(PRECISION[precisionEnumId] || "Place estimate"),
        extent: extent || null
    }
}

// ===== the adapters: a request's URL and its answer read =====
const ADAPTERS = {
    MPK_GEOCODER_CENSUS: {
        // US addresses only, one line; the benchmark is the current address ranges
        testQuery: "4600 Silver Hill Rd, Washington, DC 20233",
        covers: country => !country || US_AREAS.includes(country.geoId),
        url(provider, q) {
            const base = endpointOf(provider)
            const path = /\/onelineaddress$/.test(base) ? base : base + "/locations/onelineaddress"
            return path + "?address=" + enc(q) + "&benchmark=Public_AR_Current&format=json"
        },
        shapeOk: json => !!json && typeof json === "object" && !!json.result && Array.isArray(json.result.addressMatches),
        read(json) {
            return Array.from(json.result.addressMatches)
                .map(match => {
                    const position = point(match?.coordinates?.x, match?.coordinates?.y)
                    return position ? candidate(match.matchedAddress, position, "address", "GPP_STREET_RANGE") : null
                })
                .filter(found => found)
        }
    },
    MPK_GEOCODER_NOMINATIM: {
        testQuery: "Paris",
        covers: () => true,
        url(provider, q, options) {
            let url = endpointOf(provider) + "/search?format=jsonv2&addressdetails=0&limit=" + options.limit + "&q=" + enc(q)
            if (options.country?.alpha2) url += "&countrycodes=" + enc(options.country.alpha2.toLowerCase())
            // a bias, not a bound: west, north, east, south
            if (options.viewport) {
                const [west, south, east, north] = options.viewport
                url += "&viewbox=" + [west, north, east, south].join(",")
            }
            return url
        },
        shapeOk: json => Array.isArray(json),
        read(json) {
            return json
                .map(row => {
                    const position = point(row?.lon, row?.lat)
                    if (!position) return null
                    const box = Array.isArray(row.boundingbox) ? row.boundingbox : []
                    const extent = extentOf(box[2], box[0], box[3], box[1])
                    const type = text(row.addresstype || row.type)
                    const rank = Number(row.place_rank) || 0
                    if (rank >= 30 || ["house", "building"].includes(type) || row.category === "building")
                        return candidate(row.display_name, position, "address", "GPP_ADDRESS_MATCH", extent)
                    if (type === "postcode") return candidate(row.display_name, position, "postal", "GPP_POSTAL_AREA", extent)
                    if (rank >= 26 || row.category === "highway")
                        return candidate(row.display_name, position, "street", "GPP_PLACE_ESTIMATE", extent)
                    return candidate(row.display_name, position, "place", "GPP_PLACE_ESTIMATE", extent)
                })
                .filter(found => found)
        }
    },
    MPK_GEOCODER_OVERTURE: {
        // this server's index: no URL, no request
        local: true,
        covers: () => true
    },
    MPK_GEOCODER_PHOTON: {
        testQuery: "Paris",
        covers: () => true,
        url(provider, q, options) {
            let url = endpointOf(provider) + "/api?limit=" + options.limit + "&q=" + enc(q)
            if (options.viewport) {
                const [west, south, east, north] = options.viewport
                url += "&lat=" + (south + north) / 2 + "&lon=" + (west + east) / 2
            }
            return url
        },
        shapeOk: json => !!json && typeof json === "object" && Array.isArray(json.features),
        read(json, options) {
            const wanted = options.country?.alpha2 ? options.country.alpha2.toUpperCase() : ""
            return json.features
                .map(feature => {
                    const coordinates = feature?.geometry?.coordinates || []
                    const position = point(coordinates[0], coordinates[1])
                    const properties = feature?.properties || {}
                    if (!position) return null
                    if (wanted && text(properties.countrycode).toUpperCase() !== wanted) return null
                    const street = [properties.housenumber, properties.street]
                        .map(text)
                        .filter(part => part)
                        .join(" ")
                    const label = [
                        properties.name || street,
                        properties.city,
                        properties.state,
                        properties.postcode,
                        properties.country
                    ]
                        .map(text)
                        .filter((part, index, all) => part && all.indexOf(part) === index)
                        .join(", ")
                    const box = Array.isArray(properties.extent) ? properties.extent : []
                    const extent = extentOf(box[0], box[3], box[2], box[1])
                    const type = text(properties.type)
                    if (type === "house") return candidate(label, position, "address", "GPP_ADDRESS_MATCH", extent)
                    if (type === "postcode") return candidate(label, position, "postal", "GPP_POSTAL_AREA", extent)
                    if (type === "street") return candidate(label, position, "street", "GPP_PLACE_ESTIMATE", extent)
                    return candidate(label, position, "place", "GPP_PLACE_ESTIMATE", extent)
                })
                .filter(found => found)
        }
    }
}
// ===== the local geocoder: the active Overture Addresses runs =====
// The import IDs of the active Addresses runs
const activeAddressRuns = () =>
    Array.from(
        ec.entity
            .find("moqui.maps.OvertureRegionTheme")
            .condition({themeId: "ADDRESSES"})
            .condition("activeImportId", "is-not-null", null)
            .disableAuthz()
            .limit(100)
            .list()
    ).map(row => String(row.activeImportId))
// Whether a region's addresses are active here, and this server can read them
const overtureReady = () => !!ec.factory.hasFeature("overture") && activeAddressRuns().length > 0
// A service of this server, its errors kept apart from the caller's: {out} or {failure}
function callLocal(name, parameters) {
    ec.message.pushErrors()
    try {
        const out = ec.service.sync().name(name).parameters(parameters).disableAuthz().call() || {}
        if (ec.message.hasError()) return {failure: ec.message.getErrorsString()}
        return {out}
    } catch (error) {
        return {failure: String(error?.message ?? error)}
    } finally {
        ec.message.clearErrors()
        ec.message.popErrors()
    }
}
const listOf = value => (value == null ? null : Array.from(value).map(num))
// A candidate of the local geocoder: its precision said as plans/Maps.md §6.1 asks, never exact and never a rooftop
function overtureCandidate(found) {
    const position = point(found.lon, found.lat)
    if (!position) return null
    const extent = listOf(found.extent)
    const tier = text(found.tier)
    const differs = [
        found.postcodeAgrees === false ? L("postal code") : null,
        found.localityAgrees === false ? L("locality") : null
    ].filter(Boolean)
    let precisionText
    if (tier === "address" && differs.length)
        precisionText = say("Place estimate: the number and street are found, but not with the ${parts} asked for", {
            parts: differs.join(" " + L("and") + " ")
        })
    else if (tier === "address")
        precisionText = L(
            "Address point (Overture): the source does not say whether it is an entrance, a building or parcel centre, or a position on the road"
        )
    else if (tier === "numberbase")
        precisionText = say("Place estimate: number ${found} of the street, not ${asked}", {
            found: text(found.number),
            asked: text(found.numberAsked)
        })
    else
        precisionText = text(found.numberAsked)
            ? say("Place estimate: the street (no address point with number ${asked})", {asked: text(found.numberAsked)})
            : L("Place estimate: the street")
    if (tier === "address" && found.unitAgrees === false)
        precisionText += " · " + L("the unit asked for is not among its points: the address is shown, no unit")
    if (Number(found.points) > 1 && tier === "address")
        precisionText +=
            " · " +
            (Number(found.units) > 0
                ? say("${count} points of this address, ${units} of them units", {
                      count: Number(found.points),
                      units: Number(found.units)
                  })
                : say("${count} points of this address (its entrances or a number given twice)", {count: Number(found.points)}))
    const licenses = text(found.sourceLicenses)
    const sources = text(found.sourceDatasets) + (licenses ? " (" + licenses + ")" : "")
    return Object.assign(
        candidate(
            found.label,
            position,
            text(found.matchKind) || "address",
            text(found.precisionEnumId) || "GPP_PLACE_ESTIMATE",
            extent && extent.length === 4 && extent.every(value => value !== null) ? extent : null
        ),
        {
            precisionText,
            positionKind: tier === "address" ? "unstated" : null,
            sourceText: plain(["Overture " + text(found.release), sources].filter(part => text(part)).join(": ")),
            gersId: text(found.gersId) || null,
            release: text(found.release) || null,
            postcodeAgrees: found.postcodeAgrees ?? null,
            localityAgrees: found.localityAgrees ?? null,
            unitAgrees: found.unitAgrees ?? null
        }
    )
}
// The postal lookup of the code read from a text the index did not match: approximate, a code's places as choices
function postalFallback(postcode, countryGeoId) {
    if (!text(postcode) || !text(countryGeoId)) return []
    const found = callLocal("org.moqui.maps.MapServices.get#PostalLocation", {countryGeoId, postalCode: postcode})
    if (found.failure) return []
    return Array.from(found.out.matches || [])
        .map(match => {
            const position = point(match.lon, match.lat)
            if (!position) return null
            const names = Array.from(match.placeNames || [])
                .map(place => text(place.name))
                .filter(Boolean)
            const extent = listOf(match.extent)
            return Object.assign(
                candidate(
                    text(match.displayCode) + (names.length ? " — " + names.slice(0, 4).join(", ") : ""),
                    position,
                    "postal",
                    text(match.precisionEnumId) || "GPP_POSTAL_AREA",
                    extent && extent.length === 4 ? extent : null
                ),
                {precisionText: text(match.precisionText) || L(PRECISION.GPP_POSTAL_AREA), sourceText: text(match.sourceText)}
            )
        })
        .filter(found => found)
}
// A user's local searches a second: a light throttle (each server by itself, the count not atomic), as each search sends a
// few indexed queries (at most 24)
const LOCAL_PER_SECOND = 20
function localThrottled() {
    const cache = ec.cache.getCache(LAST)
    const key = "local:" + (ec.user.userId || "-") + ":" + Math.floor(Date.now() / 1000)
    const count = Number(cache.get(key)) || 0
    if (count >= LOCAL_PER_SECOND) return true
    cache.put(key, count + 1)
    return false
}
function overtureSearch(provider, q, options, extra) {
    if (localThrottled())
        return refusal(
            "RATE_LIMITED",
            say("${name} takes at most ${count} searches a second from one user: search again in a moment", {
                name: extra.providerName,
                count: LOCAL_PER_SECOND
            }),
            extra
        )
    const found = callLocal(OVERTURE_FIND, {
        query: q,
        country: options.country?.alpha2 || null,
        viewport: options.viewport,
        limit: options.limit
    })
    if (found.failure) {
        // the log names the provider and the failure, never the text
        ec.logger.warn("Geocoder " + provider.providerId + " (local) failed a search: " + found.failure.slice(0, 200))
        return refusal(
            "PROVIDER_FAILED",
            say("The address search ${name} is not available on this server: the local results are shown", {
                name: extra.providerName
            }),
            extra
        )
    }
    const out = found.out
    const candidates = Array.from(out.candidates || [])
        .map(overtureCandidate)
        .filter(found => found && found.label)
        .slice(0, options.limit)
    if (candidates.length) return Object.assign({candidates, ambiguous: !!out.ambiguous, reasonCode: null, reason: null}, extra)
    // missing: the postal area of the code the text gave, approximate (§6.1), else the point chosen on the map
    const parsed = out.parsed || {}
    const countries = Array.from(out.countries || []).map(text)
    const countryGeoId = options.country?.geoId || (countries.length === 1 ? countries[0] : null)
    const postal = postalFallback(text(parsed.postcode), countryGeoId)
    if (postal.length)
        return Object.assign(
            {
                candidates: postal.slice(0, options.limit),
                ambiguous: postal.length > 1,
                reasonCode: "NO_ADDRESS_MATCH",
                reason: L(
                    "No address point matches: the postal area is shown (approximate); choose the point on the map for a better one"
                )
            },
            extra
        )
    return Object.assign(
        {
            candidates: [],
            ambiguous: false,
            reasonCode: "NO_MATCH",
            reason: L("No address point matches: choose the point on the map, or search its postal code")
        },
        extra
    )
}
// The test of the local geocoder: a fixed query made from a row of its own data (never a user's text), which must come back
// as an address match within 30 m of that row's point
function overtureTest() {
    if (!ec.factory.hasFeature("overture"))
        return [{check: "Search", ok: false, text: L("This server is built without the overture feature")}]
    const active = activeAddressRuns()
    if (!active.length)
        return [
            {
                check: "Search",
                ok: false,
                text: L(
                    "No region's Overture addresses are active: import a region's addresses first (Maps › Configuration › Overture)"
                )
            }
        ]
    const row = ec.entity
        .find(OVERTURE_ADDRESS)
        .condition("importId", "in", active)
        .condition("houseNumber", "is-not-null", null)
        .condition("streetName", "is-not-null", null)
        .orderBy("addressId")
        .limit(1)
        .disableAuthz()
        .one()
    if (!row)
        return [{check: "Search", ok: false, text: L("The active Overture addresses have no point with a street and a number")}]
    const street = text(row.houseNumber) + " " + text(row.streetName) + (text(row.unitText) ? " #" + text(row.unitText) : "")
    const place = [text(row.postcode), text(row.locality)].filter(Boolean).join(" ")
    const found = callLocal(OVERTURE_FIND, {
        query: place ? street + ", " + place : street,
        country: text(row.countryCode),
        limit: 5
    })
    if (found.failure) return [{check: "Search", ok: false, text: L("The local address index did not answer")}]
    const [lon, lat] = [Number(row.longitude), Number(row.latitude)]
    const metres = found =>
        Math.hypot((Number(found.lon) - lon) * 111320 * Math.cos((lat * Math.PI) / 180), (Number(found.lat) - lat) * 110570)
    const matched = Array.from(found.out.candidates || []).some(
        found => found.precisionEnumId === "GPP_ADDRESS_MATCH" && metres(found) <= 30
    )
    return [
        matched
            ? {
                  check: "Search",
                  ok: true,
                  text: say(
                      "The local address index of ${count} active regions answered a fixed query made from its own data with an address match; nothing was sent",
                      {count: active.length}
                  )
              }
            : {check: "Search", ok: false, text: L("The local address index did not find an address of its own data")}
    ]
}

// Its requestsPerSecond, or 1 for Nominatim and Photon when empty (the OSM Foundation's policy for its public Nominatim,
// and a fair use of any shared server), none for the others
const rateOf = provider =>
    Number(provider.requestsPerSecond) > 0
        ? Number(provider.requestsPerSecond)
        : ["MPK_GEOCODER_NOMINATIM", "MPK_GEOCODER_PHOTON"].includes(text(provider.providerKindEnumId))
          ? 1
          : 0
// One of its requestsPerDay claimed for today (UTC), or false when the day's are used: the day's row locked in a transaction
// of its own, so two servers at once cannot both take the last one; a first request of the day that loses the race to
// create the row claims on the row the other made.
function claimDay(providerId, limit) {
    const now = new Date()
    const two = value => String(value).padStart(2, "0")
    const countDate = now.getUTCFullYear() + "-" + two(now.getUTCMonth() + 1) + "-" + two(now.getUTCDate())
    const claim = () => {
        const was = ec.artifactExecution.disableAuthz()
        try {
            const row = ec.entity.find(DAY_COUNT).condition({providerId, countDate}).forUpdate(true).one()
            if (!row) {
                ec.entity.makeValue(DAY_COUNT).setAll({providerId, countDate, requestCount: 1}).create()
                return true
            }
            const count = Number(row.requestCount) || 0
            if (count >= limit) return false
            row.set("requestCount", count + 1)
            row.update()
            return true
        } finally {
            if (!was) ec.artifactExecution.enableAuthz()
        }
    }
    // the first try's failure (the row another server made at once) is passed over, its words kept out of the caller's
    ec.message.pushErrors()
    let first = null
    try {
        first = !!ec.transaction.runRequireNew(10, "Counting a geocoder request", claim)
    } catch (error) {
        first = null
    } finally {
        ec.message.clearErrors()
        ec.message.popErrors()
    }
    return first !== null ? first : !!ec.transaction.runRequireNew(10, "Counting a geocoder request", claim)
}
// Its allowedOperations: empty, or naming geocode or search
const operationAllowed = provider => {
    const operations = text(provider.allowedOperations)
        .toLowerCase()
        .split(/[\s,]+/)
        .filter(each => each)
    return !operations.length || operations.includes("geocode") || operations.includes("search")
}
// The public demo servers whose policies forbid an application's traffic beyond a trickle (plans/Maps.md §6.2): said by
// the test and readiness, never refused, since an administrator may have an agreement
const PUBLIC_HOSTS = ["nominatim.openstreetmap.org", "photon.komoot.io"]
const publicHost = provider => {
    const host = (/^https?:\/\/([^\/:?#]+)/i.exec(text(provider.endpointUrl))?.[1] || "").toLowerCase()
    return PUBLIC_HOSTS.includes(host) ? host : null
}
const publicHostText = host =>
    say(
        "${host} is a public demo server: its policy allows an application at most about one search a second, no autocomplete and no bulk use, with no guarantee; run your own for regular use",
        {host}
    )
const isGeocoderKind = kind => text(kind) === "MPK_GEOCODER" || Object.hasOwn(ADAPTERS, text(kind))

// ===== the request =====
// One GET: {json} or {failure: words with no text of the answer}
function fetchJson(url, denyInternal) {
    try {
        let client = ec.service
            .rest()
            .method("GET")
            .uri(url)
            .acceptContentType("application/json")
            .timeout(10)
            .followRedirects(false)
        client = denyInternal ? client.denyInternal() : client.allowInternal()
        const response = client.send()
        const status = Number(response.getStatusCode())
        if (status >= 300 && status < 400)
            return {failure: L("redirected (a geocoder's URL is the one it answers at)"), kind: "redirect"}
        if (status < 200 || status > 299) return {failure: say("answered HTTP ${status}", {status}), kind: "status " + status}
        const length = Number(response.headerFirst("content-length"))
        if (Number.isFinite(length) && length > MAX_BYTES) return {failure: L("answered more than 1 MB"), kind: "too large"}
        // read at most MAX_BYTES of the kept body, whatever its Content-Length said or did not say
        let body
        try {
            body = response.text(MAX_BYTES)
        } catch (error) {
            return {failure: L("answered more than 1 MB"), kind: "too large"}
        }
        // how long its own headers let a shared cache keep it: max-age, unless no-store, no-cache or private
        const control = text(response.headerFirst("cache-control")).toLowerCase()
        const maxAge = /no-store|no-cache|private/.test(control) ? 0 : Number(/max-age=(\d+)/.exec(control)?.[1]) || 0
        try {
            return {json: JSON.parse(body), maxAge}
        } catch (error) {
            return {failure: L("answered something that is not JSON"), kind: "not JSON"}
        }
    } catch (error) {
        return {failure: L("could not be reached"), kind: "transport"}
    }
}
const providerName = provider => text(provider.providerName) || provider.providerId

// Whether a provider passed its last test as it is configured now (MapProviders.js tested), loaded when asked: the
// providers module loads this one for its test
function tested(provider) {
    return !!ec.resource.script("component://framework/service/org/moqui/maps/MapProviders.js").tested(provider)
}
function enabledGeocoders() {
    return Array.from(
        ec.entity
            .find(PROVIDER)
            .condition("providerKindEnumId", "in", Object.keys(ADAPTERS))
            .orderBy("sequenceNum,providerId")
            .disableAuthz()
            .limit(20)
            .list()
    ).filter(row => row.disabled !== "Y")
}
const isLocal = provider => !!ADAPTERS[text(provider?.providerKindEnumId)]?.local
// Whether a geocoder may be asked now: tested as it is configured; an outside one only with outside requests on, a local one
// only while its data is active
const usable = provider => tested(provider) && (isLocal(provider) ? overtureReady() : !offline())
// Whether a search may ask a geocoder now: an enabled, usable geocoder; {available, providerName, local}
function availability() {
    const provider = enabledGeocoders().find(row => usable(row))
    return provider
        ? {available: true, providerId: provider.providerId, providerName: providerName(provider), local: isLocal(provider)}
        : {available: false}
}
function countryOf(value) {
    const code = text(value).toUpperCase()
    if (!code) return null
    const cf = ec.entity.conditionFactory
    const geo = ec.entity
        .find("moqui.basic.Geo")
        .condition(
            cf.makeCondition(
                [
                    cf.makeCondition("geoId", "equals", code),
                    cf.makeCondition("geoCodeAlpha2", "equals", code),
                    cf.makeCondition("geoCodeAlpha3", "equals", code)
                ],
                "or"
            )
        )
        .condition("geoTypeEnumId", "equals", "GEOT_COUNTRY")
        .useCache(true)
        .disableAuthz()
        .one()
    return geo ? {geoId: String(geo.geoId), alpha2: text(geo.geoCodeAlpha2), alpha3: text(geo.geoCodeAlpha3)} : null
}
function viewportOf(value) {
    const parts = Array.isArray(value) ? value : text(value) ? text(value).split(",") : []
    const values = parts.map(num)
    if (values.length !== 4 || values.some(item => item === null)) return null
    const [west, south, east, north] = values
    return west >= -180 && east <= 180 && south >= -90 && north <= 90 && south <= north ? values : null
}

const localProvider = () => enabledGeocoders().find(row => isLocal(row) && usable(row)) || null
// The local geocoder's search of an address, as find#Places answers it, for a caller who may not call find#Places (a
// record's address in the local-map dialog, which its adapter let them read): no outside request; null when no local
// geocoder may be asked
function localSearch(query, countryGeoId, limit) {
    const provider = localProvider()
    if (!provider) return null
    const q = plain(query, 300)
    if (q.length < 2 || q.length > 200) return null
    const country = text(countryGeoId) ? countryOf(countryGeoId) : null
    const extra = {
        providerId: provider.providerId,
        providerName: providerName(provider),
        attribution: text(provider.attribution),
        termsUrl: text(provider.termsUrl) || null
    }
    return overtureSearch(
        provider,
        q,
        {limit: Math.max(1, Math.min(MAX_LIMIT, Number(limit) || 5)), country, viewport: null},
        extra
    )
}

// ===== find#Places =====
function findPlaces(p) {
    const q = plain(p.query, 300)
    if (q.length < 2 || q.length > 200) return refusal("QUERY_INVALID", L("Search for a text of 2 to 200 characters"))
    const askedId = text(p.providerId)
    const provider = askedId
        ? enabledGeocoders().find(row => row.providerId === askedId && usable(row))
        : enabledGeocoders().find(row => usable(row))
    if (!provider && offline())
        return refusal(
            "EXTERNAL_REQUESTS_OFF",
            L("Outside requests are turned off (maps@external-requests): no address or place search is asked")
        )
    if (!provider)
        return refusal("NO_GEOCODER", L("No address or place search is configured: an administrator enables a geocoder"))
    const adapter = ADAPTERS[text(provider.providerKindEnumId)]
    const name = providerName(provider)
    if (!operationAllowed(provider))
        return refusal(
            "OPERATION_NOT_ALLOWED",
            say("${name} is not configured for address search (its allowed operations)", {name})
        )
    const extra = {
        providerId: provider.providerId,
        providerName: name,
        attribution: text(provider.attribution),
        termsUrl: text(provider.termsUrl) || null
    }
    const limit = Math.max(1, Math.min(MAX_LIMIT, Number(p.limit) || 10))
    const country = text(p.countryGeoId) ? countryOf(p.countryGeoId) : null
    if (text(p.countryGeoId) && !country) return refusal("COUNTRY_UNKNOWN", L("The country is not known"), extra)
    if (!adapter.covers(country))
        return refusal("COUNTRY_NOT_COVERED", say("${name} searches only addresses of the United States", {name}), extra)
    const options = {limit, country, viewport: viewportOf(p.viewport)}
    // this server's own index: no request, no rate, no cache
    if (adapter.local) return overtureSearch(provider, q, options, extra)
    const url = adapter.url(provider, q, options)

    // what this server may keep (none by default; private is the browser's alone, so nothing here), by a digest of the
    // request, never the text, for cacheSeconds or, when empty, what the answer's own Cache-Control allows
    const shared = text(provider.cachePolicy).toLowerCase() === "shared"
    const key = shared ? ec.resource.digestText(provider.providerId + "\n" + url, "sha256") : null
    if (key) {
        const hit = ec.cache.getCache(ANSWERS).get(key)
        if (hit) {
            try {
                const kept = JSON.parse(String(hit))
                if (Date.now() < Number(kept.until))
                    return Object.assign(
                        {candidates: kept.candidates, ambiguous: kept.candidates.length > 1, cached: true},
                        extra
                    )
            } catch (error) {
                // a kept answer that does not read is asked again
            }
        }
    }
    // its rate, refused rather than queued: one request claims its slot of 1000/rate ms with putIfAbsent, so two at once
    // cannot both pass; the cache is this server's, so each server of a cluster keeps the rate by itself
    const perSecond = rateOf(provider)
    if (perSecond > 0) {
        const slotMs = Math.max(1, Math.floor(1000 / perSecond))
        const slot = Math.floor(Date.now() / slotMs)
        if (!ec.cache.getCache(LAST).putIfAbsent(provider.providerId + ":" + slot, Date.now()))
            return refusal(
                "RATE_LIMITED",
                say("${name} takes at most ${count} searches a second: search again in a moment", {name, count: perSecond}),
                extra
            )
    }
    // its requests a day, claimed last, so only a search that is sent counts
    const perDay = Number(provider.requestsPerDay) > 0 ? Number(provider.requestsPerDay) : 0
    if (perDay > 0 && !claimDay(provider.providerId, perDay))
        return refusal(
            "DAILY_LIMIT",
            say("${name} takes at most ${count} searches a day from this installation: search again tomorrow", {
                name,
                count: perDay
            }),
            extra
        )
    const answer = fetchJson(url, !privateAllowed(provider))
    if (answer.failure || !adapter.shapeOk(answer.json)) {
        ec.logger.warn("Geocoder " + provider.providerId + " failed a search: " + (answer.kind || "unexpected answer"))
        return refusal(
            "PROVIDER_FAILED",
            say("The address search ${name} ${failure}: the local results are shown", {
                name,
                failure: answer.failure || L("answered in a form it does not use")
            }),
            extra
        )
    }
    const candidates = adapter
        .read(answer.json, options)
        .filter(found => found.label)
        .slice(0, limit)
    const seconds = Number(provider.cacheSeconds) > 0 ? Number(provider.cacheSeconds) : answer.maxAge
    if (key && seconds > 0)
        ec.cache.getCache(ANSWERS).put(key, JSON.stringify({until: Date.now() + Math.min(seconds, 86400) * 1000, candidates}))
    return Object.assign(
        {
            candidates,
            ambiguous: candidates.length > 1,
            reasonCode: candidates.length ? null : "NO_MATCH",
            reason: candidates.length ? null : L("No place matches")
        },
        extra
    )
}

// ===== the connection test (MapProviders.js): a harmless fixed query =====
function test(provider, denyInternal) {
    const kind = text(provider.providerKindEnumId)
    const adapter = ADAPTERS[kind]
    if (!adapter) return [{check: "Search", ok: false, text: L("Choose the geocoder's service: its kind names it")}]
    if (adapter.local) return overtureTest()
    const answer = fetchJson(adapter.url(provider, adapter.testQuery, {limit: 1}), denyInternal)
    if (answer.failure) return [{check: "Search", ok: false, text: say("The geocoder ${failure}", {failure: answer.failure})}]
    if (!adapter.shapeOk(answer.json))
        return [{check: "Search", ok: false, text: L("The geocoder answered in a form its service does not use")}]
    const count = adapter.read(answer.json, {limit: 1}).length
    const host = publicHost(provider)
    return [
        ...(host ? [{check: "Policy", ok: true, text: publicHostText(host)}] : []),
        {
            check: "Search",
            ok: true,
            text: say(
                "The geocoder answered a fixed test search with ${count} candidates; no credentials and no user's text were sent",
                {count}
            )
        }
    ]
}

const operations = {findPlaces}
;({
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown geocoder operation " + op)
        return operations[op](parameters || {})
    },
    // for the providers (MapProviders.js) and Browse's search
    test,
    isGeocoderKind,
    isLocal,
    // for the providers' readiness: whether the local geocoder's data is active
    localReady: overtureReady,
    // for a record's address with no point (MapConfig.js): the enabled, tested local geocoder whose data is active, or null
    localProvider,
    // and its search, made here as the dialog's own step (the record's adapter has authorized it): find#Places' answer
    localSearch,
    publicHost,
    publicHostText,
    availability,
    // for the providers' proxy (MapProviders.js): its requestsPerDay
    claimDay
})
