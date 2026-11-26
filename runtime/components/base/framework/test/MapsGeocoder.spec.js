// Address and place search (plans/Maps.md §6.1, §6.2, §8; org.moqui.maps.MapServices.find#Places, MapGeocoders.js, the
// geocoder kinds of the mapProvider connection kind): nothing is sent without an enabled geocoder that passed its test,
// nor with outside requests off; the connection test sends a fixed public address, never a user's text; the US Census
// geocoder's answer, through the HTTP fixtures, is street-range interpolation with several matches ambiguous, and Browse's
// search lists it third with its source and precision; a failure is said and no other service is asked; its rate and
// allowed operations refuse a search with nothing more sent, and so does its requests a day once the day's are used; the reads of the local-map dialog (the effective
// configuration and the postal lookup) send nothing to the geocoder.
// Run: ./moq test framework --spec MapsGeocoder
const S = "org.moqui.maps.MapServices."
const C = "org.moqui.admin.ConfigurationServices."
const http = ec.factory.getTool("HttpFixtures")
const USER = "MAPS_GEO_USER"
const MANAGER = "MAPS_GEO_MANAGER"
const MANAGERS = "MAPS_GEO_MANAGERS"
const PROVIDER = "MAPS_GEO_CENSUS"
const ENDPOINT = "https://geocoder.spec.example/geocoder"
const SEARCH = ENDPOINT + "/locations/onelineaddress*"
const census = matches =>
    JSON.stringify({
        result: {
            input: {benchmark: {benchmarkName: "Public_AR_Current"}},
            addressMatches: matches.map(([x, y, matchedAddress]) => ({
                tigerLine: {side: "L", tigerLineId: "1"},
                coordinates: {x, y},
                addressComponents: {},
                matchedAddress
            }))
        }
    })
const SPRINGFIELDS = census([
    [-72.5898, 42.1015, "100 MAIN ST, SPRINGFIELD, MA, 01103"],
    [-89.6501, 39.8017, "100 MAIN ST, SPRINGFIELD, IL, 62701"]
])

const usernameOf = userId => userId.toLowerCase().replace(/_/g, ".")
const make = (entity, fields) => ec.entity.makeValue(entity).setAll(fields).createOrUpdate()
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
function as(userId, action) {
    ec.user.internalLoginUser(usernameOf(userId))
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
const search = (params = {}) =>
    as(USER, () => invoke(S + "find#Places", Object.assign({query: "100 Main St, Springfield"}, params)))

spec({
    setupSpec() {
        http.enable()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        const now = ec.user.nowTimestamp
        for (const userId of [USER, MANAGER])
            make("moqui.security.UserAccount", {userId, username: usernameOf(userId), userFullName: "Maps geocoder " + userId})
        make("moqui.security.UserGroupMember", {userGroupId: "MAPS_USERS", userId: USER, fromDate: now})
        make("moqui.security.UserGroup", {userGroupId: MANAGERS, description: "Maps geocoder managers"})
        for (const userPermissionId of ["ADMIN_CONNECTION_VIEW", "ADMIN_CONNECTION_MANAGE"])
            make("moqui.security.UserGroupPermission", {userGroupId: MANAGERS, userPermissionId, fromDate: now})
        make("moqui.security.UserGroupMember", {userGroupId: MANAGERS, userId: MANAGER, fromDate: now})
        // enabled, but not tested as it is configured: not asked yet
        make("moqui.maps.MapProvider", {
            providerId: PROVIDER,
            providerName: "Spec Census",
            providerKindEnumId: "MPK_GEOCODER_CENSUS",
            endpointUrl: ENDPOINT,
            attribution: "Spec Census Bureau",
            allowPrivateNetwork: "N",
            disabled: "N",
            sequenceNum: 1
        })
        ec.message.clearErrors()
    },
    cleanupSpec() {
        http.reset()
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        const del = (entity, field, values) => ec.entity.find(entity).condition(field, "in", values).deleteAll()
        del("moqui.server.ConnectionCheck", "connectionKey", [PROVIDER])
        del("moqui.maps.MapProviderDayCount", "providerId", [PROVIDER])
        del("moqui.maps.MapProvider", "providerId", [PROVIDER])
        del("moqui.security.UserGroupMember", "userId", [USER, MANAGER])
        del("moqui.security.UserGroupPermission", "userGroupId", [MANAGERS])
        del("moqui.security.UserGroup", "userGroupId", [MANAGERS])
        del("moqui.security.UserAccount", "userId", [USER, MANAGER])
    }
})

feature(
    "nothing is sent before the geocoder passes its test, whose search is a fixed public address, nor with outside requests off",
    {
        when() {
            http.clear()
            const out = {}
            out.untested = search()
            out.untestedRequests = http.count()
            http.add({
                method: "GET",
                url: SEARCH,
                status: 200,
                contentType: "application/json",
                body: census([[-76.92, 38.85, "4600 SILVER HILL RD, WASHINGTON, DC, 20233"]]),
                times: 1
            })
            out.tested = as(MANAGER, () => invoke(C + "test#Connection", {adapterKey: "mapProvider", connectionKey: PROVIDER}))
            out.testUrl = http.count() ? String(http.requests()[0].url) : ""
            out.offline = withConf("maps@external-requests", "false", () => search())
            out.requests = http.count()
            return out
        },
        then(out) {
            out.untested.reasonCode == "NO_GEOCODER"
            out.untested.candidates.length == 0
            out.untestedRequests == 0
            out.tested.passed == true
            out.testUrl.includes("address=4600%20Silver%20Hill")
            !out.testUrl.includes("Springfield")
            out.offline.reasonCode == "EXTERNAL_REQUESTS_OFF"
            out.requests == 1
        }
    }
)

feature("the Census answer is street-range interpolation, several matches ambiguous; a failure is said and nothing else asked", {
    when() {
        http.clear()
        const out = {}
        http.add({method: "GET", url: SEARCH, status: 200, contentType: "application/json", body: SPRINGFIELDS, times: 1})
        out.found = search({viewport: "-100,30,-60,50"})
        out.url = http.count() ? String(http.requests()[0].url) : ""
        // Browse's search: the geocoder third, its candidates with their source, precision and ambiguity
        http.add({method: "GET", url: SEARCH, status: 200, contentType: "application/json", body: SPRINGFIELDS, times: 1})
        out.browse = as(USER, () => {
            const res = ec.screen
                .makeTest()
                .rootScreen("component://webroot/screen/webroot.xml")
                .render("apps/maps/Browse/search", {q: "100 Main St, Springfield"}, "get")
            return JSON.parse(String(res.output || "{}"))
        })
        out.abroad = search({countryGeoId: "CAN"})
        out.beforeFailure = http.count()
        http.add({method: "GET", url: SEARCH, status: 503, contentType: "text/plain", body: "busy", times: 1})
        out.failed = search()
        out.afterFailure = http.count()
        // its policy, beside its URL: a rate of one a second refuses the second search at once, and allowed operations
        // without geocode or search refuse it with nothing sent
        const policy = fields => {
            const row = ec.entity.find("moqui.maps.MapProvider").condition("providerId", PROVIDER).disableAuthz().one()
            row.setAll(fields)
            row.update()
        }
        policy({requestsPerSecond: 1})
        http.add({method: "GET", url: SEARCH, status: 200, contentType: "application/json", body: SPRINGFIELDS, times: 1})
        const first = search({query: "1 First St"})
        out.rated = [first.reasonCode === "RATE_LIMITED" ? "limited" : "sent", search({query: "2 Second St"}).reasonCode]
        policy({requestsPerSecond: null, allowedOperations: "style,tiles"})
        out.notAllowed = search()
        policy({allowedOperations: null})
        out.afterPolicy = http.count()
        // its requests a day: today's count starts over, one is sent and the next refused with nothing sent
        ec.entity.find("moqui.maps.MapProviderDayCount").condition("providerId", PROVIDER).deleteAll()
        policy({requestsPerDay: 1})
        http.add({method: "GET", url: SEARCH, status: 200, contentType: "application/json", body: SPRINGFIELDS, times: 1})
        const daily = [search({query: "3 Third St"}), search({query: "4 Fourth St"})]
        out.daily = daily.map(each => each.reasonCode)
        out.dailyErrors = daily.map(each => each.error).join("")
        out.afterDaily = http.count()
        policy({requestsPerDay: null})
        return out
    },
    then(out) {
        out.found.candidates.length == 2
        out.found.ambiguous == true
        out.found.candidates.every(place => place.precisionEnumId == "GPP_STREET_RANGE" && place.matchKind == "address")
        out.found.candidates[0].label == "100 MAIN ST, SPRINGFIELD, MA, 01103"
        out.found.candidates[0].lon == -72.5898
        out.found.providerName == "Spec Census"
        out.found.attribution == "Spec Census Bureau"
        out.browse.sources.includes("geocoder")
        out.browse.geocoder.ambiguous == true
        out.browse.results.filter(result => result.kind == "geocoder").length == 2
        out.browse.results.find(result => result.kind == "geocoder").sourceText == "Spec Census · Spec Census Bureau"
        out.browse.results.find(result => result.kind == "geocoder").precisionEnumId == "GPP_STREET_RANGE"
        out.url.includes("address=100%20Main%20St%2C%20Springfield")
        out.abroad.reasonCode == "COUNTRY_NOT_COVERED"
        out.beforeFailure == 2
        out.failed.reasonCode == "PROVIDER_FAILED"
        out.failed.reason.includes("Spec Census")
        out.afterFailure == 3
        out.rated[1] == "RATE_LIMITED"
        out.notAllowed.reasonCode == "OPERATION_NOT_ALLOWED"
        out.afterPolicy - out.afterFailure <= 1
        out.daily[0] != "DAILY_LIMIT"
        out.daily[1] == "DAILY_LIMIT"
        out.dailyErrors == ""
        out.afterDaily - out.afterPolicy == 1
    }
})

feature("the local-map dialog's reads send nothing to the geocoder", {
    when() {
        http.clear()
        return as(USER, () => ({
            config: invoke(S + "get#EffectiveConfig", {recordLayers: false}),
            postal: invoke(S + "get#PostalLocation", {countryGeoId: "USA", postalCode: "02134"}),
            requests: http.count()
        }))
    },
    then(out) {
        out.config.error == ""
        out.postal.error == ""
        out.requests == 0
    }
})
