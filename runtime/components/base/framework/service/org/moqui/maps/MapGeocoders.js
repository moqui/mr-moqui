// Address and place search of org.moqui.maps.MapServices.find#Places (plans/Maps.md §6.1, §6.2, §7, §8; docs/tools/Maps.md
// "Address and place search"): a text to candidate places from one configured geocoder, and the geocoder's connection test.
//
// A geocoder is a MapProvider of a geocoder kind (MapProviderKind under MPK_GEOCODER: MPK_GEOCODER_CENSUS,
// MPK_GEOCODER_NOMINATIM, MPK_GEOCODER_PHOTON), whose adapter here builds its request and reads its answer. A new service
// is a new kind and an entry of ADAPTERS; the provider row holds only its endpoint, attribution and policy, never a
// service name or code.
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
//   cluster of n servers may send n times the rate. requestsPerDay is recorded, not enforced. The public Nominatim and
//   Photon demo servers are named by the test and readiness, since their policies allow an application very little.
// - This server keeps an answer only when its cachePolicy is shared (none, empty and private keep nothing here: private
//   is the user's browser alone), for cacheSeconds or, when empty, the answer's own Cache-Control max-age (at most a
//   day; nothing with no-store, no-cache or private), under a digest of the provider and the request, never the text.
// - Each candidate is a label (plain text), a WGS84 point, its match kind (address, street, postal, place) and precision
//   (GeoPointPrecision): the US Census geocoder interpolates along a street's address range (GPP_STREET_RANGE, never a
//   rooftop); Nominatim and Photon say an address point, a street, a postal code or a place. Several candidates are
//   ambiguous: choices, none chosen. A provider's confidence is never made a distance.
const PROVIDER = "moqui.maps.MapProvider"
const ANSWERS = "maps.geocoder.answer"
const LAST = "maps.geocoder.last"
const MAX_BYTES = 1024 * 1024
const MAX_LIMIT = 20
const US_AREAS = ["USA", "PRI", "VIR", "GUM", "ASM", "MNP"]
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
// Its requestsPerSecond, or 1 for Nominatim and Photon when empty (the OSM Foundation's policy for its public Nominatim,
// and a fair use of any shared server), none for the others
const rateOf = provider =>
    Number(provider.requestsPerSecond) > 0
        ? Number(provider.requestsPerSecond)
        : ["MPK_GEOCODER_NOMINATIM", "MPK_GEOCODER_PHOTON"].includes(text(provider.providerKindEnumId))
          ? 1
          : 0
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
// Whether a search may ask a geocoder now: outside requests on and one enabled, tested geocoder; {available, providerName}
function availability() {
    if (offline()) return {available: false}
    const provider = enabledGeocoders().find(row => tested(row))
    return provider
        ? {available: true, providerId: provider.providerId, providerName: providerName(provider)}
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

// ===== find#Places =====
function findPlaces(p) {
    const q = plain(p.query, 300)
    if (q.length < 2 || q.length > 200) return refusal("QUERY_INVALID", L("Search for a text of 2 to 200 characters"))
    if (offline())
        return refusal(
            "EXTERNAL_REQUESTS_OFF",
            L("Outside requests are turned off (maps@external-requests): no address or place search is asked")
        )
    const askedId = text(p.providerId)
    const provider = askedId
        ? enabledGeocoders().find(row => row.providerId === askedId && tested(row))
        : enabledGeocoders().find(row => tested(row))
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
    publicHost,
    publicHostText,
    availability
})
