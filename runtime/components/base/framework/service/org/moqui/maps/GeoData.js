// The Geo data settings of org.moqui.maps.GeoDataServices (plans/Maps.md §6.4; docs/tools/Maps.md "Geo data"), around the
// importer (import#GeoData, cancel#GeoDataImport and remove#GeoData, in crates/moqui-tools/src/geo): what Maps ›
// Configuration › Geo data shows (each data set with its license, the countries imported with their edition, counts and
// last run, the run history and the refresh schedule), the license acknowledgement, an import started as a run of the
// GeoDataImport ServiceJob (never in the request), the refresh of every imported scope (the GeoDataRefresh ServiceJob, run
// now or on its schedule) and the maps.geodata readiness checks of org.moqui.maps.MapServices.
//
// A scope is a data set's source key (the sets sharing it: a mirror, a later year) and a country. Importing every GeoNames
// country at once is not offered: the US measurement of P2 (plans/Maps.md §6.3) leaves the international shape open.
// The services are MAPS_ADMIN's (the artifact group names GeoDataServices); the rows they write (a pending run, the job's
// schedule) are written with authorization off as the services' own steps.
const DATA_SET = "moqui.maps.GeoDataSet"
const IMPORT = "moqui.maps.GeoDataImport"
const IMPORT_JOB = "GeoDataImport"
const REFRESH_JOB = "GeoDataRefresh"
const IMPORT_ROOT = "dbresource://moqui/maps/import"
const IN_PROGRESS = ["GdiPending", "GdiDownloading", "GdiValidating", "GdiLoading"]
const STATUS_TEXT = {
    GdiPending: "Waiting to start",
    GdiDownloading: "Downloading",
    GdiValidating: "Checking the file",
    GdiLoading: "Loading",
    GdiCompleted: "Completed",
    GdiUnchanged: "Current (source unchanged)",
    GdiFailed: "Failed",
    GdiCancelled: "Cancelled"
}
// the refresh schedules offered: a Quartz expression each (moqui_lib::cron), at a quiet hour
const SCHEDULES = {weekly: "0 20 3 ? * SUN", monthly: "0 20 3 1 * ?"}
const MAX_RUNS = 50

const L = value => ec.l10n.localize(value)
const say = (value, values) => ec.resource.expand(L(value), "", values || {})
const text = value => (value == null ? "" : String(value).trim())
const find = (entity, conditions) =>
    ec.entity
        .find(entity)
        .condition(conditions || {})
        .disableAuthz()
const one = (entity, conditions) => find(entity, conditions).one()
const rows = (entity, conditions) => Array.from(find(entity, conditions).list())
const stampText = value => (value ? ec.l10n.format(value, "yyyy-MM-dd HH:mm") : "")
const offline = () => text(ec.factory.getConfValue("maps@external-requests")) === "false"
const asSystemWrite = write => {
    const was = ec.artifactExecution.disableAuthz()
    try {
        return write()
    } finally {
        if (!was) ec.artifactExecution.enableAuthz()
    }
}
function refuse(message) {
    ec.message.addError(message)
    return {}
}

// ===== scopes and their counts =====
// Every data set by ID, read once a call
let setCache = null
const allSets = () => (setCache = setCache || rows(DATA_SET).sort((a, b) => text(a.dataSetId).localeCompare(text(b.dataSetId))))
const setIdsOf = sourceKey =>
    allSets()
        .filter(set => set.sourceKey === sourceKey)
        .map(set => set.dataSetId)
const countryName = geoId => {
    if (!geoId) return ""
    const geo = find("moqui.basic.Geo", {geoId}).useCache(true).one()
    return geo ? text(geo.geoName) || geoId : geoId
}
// A country Geo of a Geo ID or an alpha-2 or alpha-3 code
function countryOf(value) {
    const code = text(value).toUpperCase()
    if (!code) return null
    const byId = find("moqui.basic.Geo", {geoId: code}).useCache(true).one()
    if (byId && byId.geoTypeEnumId === "GEOT_COUNTRY") return byId
    const field = code.length === 2 ? "geoCodeAlpha2" : code.length === 3 ? "geoCodeAlpha3" : null
    return field
        ? Array.from(
              find("moqui.basic.Geo", {geoTypeEnumId: "GEOT_COUNTRY", [field]: code})
                  .useCache(true)
                  .list()
          )[0] || null
        : null
}
// A Geo ID prefix for LIKE, its underscores taken as themselves
const likePrefix = prefix => prefix.replace(/[\\%_]/g, character => "\\" + character) + "%"
// Sets whose rows are shared with other sources and with Mantle's own (countries, divisions): never removed, and counted by
// what is there rather than by owner
const SHARED_YIELDS = ["GDY_COUNTRIES", "GDY_ADMIN1", "GDY_ADMIN2"]
// The rows of a source key in date in a country (every country with none), by what the set yields: postal codes and
// points; the county links of the ZCTA-to-county file; the countries or the division Geos of a GeoNames gazetteer piece.
// hasRows says whether the scope has anything in use (a shared set always has).
function inDateCounts(set, countryGeoId) {
    const count = find => Number(find.count())
    if (set.yieldsEnumId === "GDY_POSTAL_COUNTY") {
        const links = count(
            find("moqui.basic.GeoAssoc")
                .condition("geoId", "like", likePrefix((countryGeoId || "USA") + "_C_"))
                .condition("toGeoId", "like", likePrefix((countryGeoId || "USA") + "_Z_"))
                .conditionDate("fromDate", "thruDate", null)
        )
        return {codes: 0, points: 0, links, hasRows: links > 0, text: say("${links} county links in use", {links})}
    }
    if (SHARED_YIELDS.includes(set.yieldsEnumId)) {
        const divisions =
            set.yieldsEnumId === "GDY_COUNTRIES"
                ? count(find("moqui.basic.Geo", {geoTypeEnumId: "GEOT_COUNTRY"}))
                : count(
                      find("moqui.basic.Geo").condition(
                          "geoId",
                          "like",
                          "%" + likePrefix(set.yieldsEnumId === "GDY_ADMIN1" ? "_A1_" : "_A2_")
                      )
                  )
        return {
            codes: 0,
            points: 0,
            divisions,
            hasRows: true,
            text:
                set.yieldsEnumId === "GDY_COUNTRIES"
                    ? say("${divisions} countries", {divisions})
                    : say("${divisions} divisions it made", {divisions})
        }
    }
    let codes = find("moqui.basic.GeoPostalCode", {sourceKey: set.sourceKey}).condition("thruDate", null)
    if (countryGeoId) codes = codes.condition("countryGeoId", countryGeoId)
    let points = find("moqui.basic.GeoPointAssoc")
        .condition("dataSetId", "in", setIdsOf(set.sourceKey))
        .condition("thruDate", null)
    if (countryGeoId) points = points.condition("geoId", "like", likePrefix(countryGeoId + "_"))
    const counts = {codes: count(codes), points: count(points)}
    return Object.assign(counts, {
        hasRows: counts.codes > 0 || counts.points > 0,
        text: say("${codes} postal codes, ${points} points in use", counts)
    })
}
// The scopes of a source key (its country, "" for none), each with its runs newest first
function scopesOf(sourceKey) {
    const byCountry = new Map()
    for (const run of runsOf(sourceKey)) {
        const key = run.countryGeoId || ""
        if (!byCountry.has(key)) byCountry.set(key, [])
        byCountry.get(key).push(run)
    }
    return byCountry
}
// The scopes in use: each source key and country that ever completed a run and still has rows (a Remove leaves none),
// with the set of its newest run, its newest run and its counts
function scopesInUse() {
    const out = []
    const seen = new Set()
    for (const set of allSets()) {
        if (seen.has(set.sourceKey)) continue
        seen.add(set.sourceKey)
        for (const [countryGeoId, runs] of scopesOf(set.sourceKey)) {
            if (!runs.some(run => run.statusId === "GdiCompleted")) continue
            const runSet = allSets().find(candidate => candidate.dataSetId === runs[0].dataSetId) || set
            const counts = inDateCounts(runSet, countryGeoId || null)
            if (!counts.hasRows) continue
            out.push({set: runSet, countryGeoId: countryGeoId || null, newest: runs[0], runs, counts})
        }
    }
    return out
}
// The runs of a source key, newest first, read once a call
const runCache = new Map()
const runsOf = sourceKey => {
    if (!runCache.has(sourceKey)) runCache.set(sourceKey, readRuns(sourceKey))
    return runCache.get(sourceKey)
}
const readRuns = sourceKey =>
    Array.from(
        find(IMPORT).condition("dataSetId", "in", setIdsOf(sourceKey)).orderBy("-fromDate,-geoDataImportId").limit(500).list()
    )
const statusText = statusId => L(STATUS_TEXT[statusId] || statusId || "")
function runRow(run) {
    const user = run.userId ? find("moqui.security.UserAccount", {userId: run.userId}).useCache(true).one() : null
    return {
        geoDataImportId: run.geoDataImportId,
        dataSetId: run.dataSetId,
        countryGeoId: run.countryGeoId || null,
        countryName: countryName(run.countryGeoId) || L("Every country"),
        statusId: run.statusId,
        statusText: statusText(run.statusId),
        inProgress: IN_PROGRESS.includes(run.statusId),
        edition: text(run.edition) || null,
        rowsRead: Number(run.rowsRead || 0),
        rowsCreated: Number(run.rowsCreated || 0),
        rowsUpdated: Number(run.rowsUpdated || 0),
        rowsUnchanged: Number(run.rowsUnchanged || 0),
        rowsRetired: Number(run.rowsRetired || 0),
        rowsRejected: Number(run.rowsRejected || 0),
        countsText: say(
            "${read} read, ${created} new, ${updated} changed, ${unchanged} unchanged, ${retired} retired, ${rejected} rejected",
            {
                read: Number(run.rowsRead || 0),
                created: Number(run.rowsCreated || 0),
                updated: Number(run.rowsUpdated || 0),
                unchanged: Number(run.rowsUnchanged || 0),
                retired: Number(run.rowsRetired || 0),
                rejected: Number(run.rowsRejected || 0)
            }
        ),
        errorFileLocation: run.errorFileLocation || null,
        failureText: text(run.failureText) || null,
        userName: user ? text(user.userFullName) || text(user.username) : "",
        fromText: stampText(run.fromDate),
        thruText: stampText(run.thruDate),
        jobRunId: run.jobRunId || null
    }
}

// ===== find#GeoDataSets =====
function findGeoDataSets() {
    const sets = allSets()
    const dataSetList = []
    const sourceScopes = new Map()
    for (const set of sets) {
        if (!sourceScopes.has(set.sourceKey)) sourceScopes.set(set.sourceKey, scopesOf(set.sourceKey))
        const operatorFile = !text(set.urlTemplate)
        const ack = set.licenseAckUserId
            ? find("moqui.security.UserAccount", {userId: set.licenseAckUserId}).useCache(true).one()
            : null
        // the scopes this source key has run in, each with its newest run, its edition in use and its rows in date
        const byCountry = sourceScopes.get(set.sourceKey)
        const countries = []
        // a scope is listed under the set its last run was of, so a mirror and its original do not list it twice
        for (const [countryGeoId, scopeRuns] of byCountry) {
            if (scopeRuns[0].dataSetId !== set.dataSetId) continue
            const last = scopeRuns[0]
            const inUse = scopeRuns.find(run => run.statusId === "GdiCompleted") || null
            const counts = inDateCounts(set, countryGeoId || null)
            countries.push({
                countryGeoId: countryGeoId || null,
                countryName: countryName(countryGeoId) || L("Every country"),
                edition: inUse ? text(inUse.edition) || null : null,
                editionText: inUse ? text(inUse.edition) || L("No edition named") : L("None in use"),
                codes: counts.codes,
                points: counts.points,
                countsText: counts.text,
                lastRun: runRow(last),
                inProgress: IN_PROGRESS.includes(last.statusId),
                hasRows: counts.hasRows,
                // remove#GeoData refuses the shared rows of countries and divisions
                removable: counts.hasRows && !SHARED_YIELDS.includes(set.yieldsEnumId)
            })
        }
        countries.sort((a, b) => a.countryName.localeCompare(b.countryName))
        const importRefusal =
            set.disabled === "Y"
                ? L("This data set is disabled")
                : !set.licenseAckUserId
                  ? L("Acknowledge its license first")
                  : !operatorFile && offline()
                    ? L("Outside requests for maps are off (maps@external-requests), so nothing is downloaded")
                    : null
        dataSetList.push({
            dataSetId: set.dataSetId,
            sourceKey: set.sourceKey,
            publisher: text(set.publisher),
            description: text(set.description),
            licenseName: text(set.licenseName),
            licenseUrl: text(set.licenseUrl) || null,
            attribution: text(set.attribution),
            sourceUrl: text(set.sourceUrl) || null,
            sourceYear: text(set.sourceYear) || null,
            priority: set.priority == null ? null : Number(set.priority),
            coverageGeoId: set.coverageGeoId || null,
            coverageText: set.coverageGeoId
                ? countryName(set.coverageGeoId)
                : set.perCountry === "Y"
                  ? L("A country at a time")
                  : operatorFile
                    ? L("The country you name")
                    : L("Every country"),
            perCountry: set.perCountry === "Y",
            operatorFile,
            needsCountry: !set.coverageGeoId && (set.perCountry === "Y" || operatorFile),
            disabled: set.disabled === "Y",
            licenseAcknowledged: !!set.licenseAckUserId,
            licenseAckText: set.licenseAckUserId
                ? say("Acknowledged by ${name} on ${date}", {
                      name: ack ? text(ack.userFullName) || text(ack.username) : set.licenseAckUserId,
                      date: stampText(set.licenseAckDate)
                  })
                : L("Not acknowledged"),
            importable: !importRefusal,
            importRefusal,
            countries
        })
    }
    const runList = Array.from(find(IMPORT).orderBy("-fromDate,-geoDataImportId").limit(MAX_RUNS).list()).map(runRow)
    const job = one("moqui.service.job.ServiceJob", {jobName: REFRESH_JOB})
    const cron = job && job.paused !== "Y" ? text(job.cronExpression) : ""
    const scheduleKey = Object.keys(SCHEDULES).find(key => SCHEDULES[key] === cron) || (cron ? "other" : "none")
    const runAs = text(
        one("moqui.service.job.ServiceJobParameter", {jobName: REFRESH_JOB, parameterName: "runAsUserId"})?.parameterValue
    )
    const runAsUser = runAs ? find("moqui.security.UserAccount", {userId: runAs}).useCache(true).one() : null
    return {
        dataSetList,
        runList,
        schedule: {
            scheduleKey,
            cronExpression: cron || null,
            scheduleText: {none: L("Not scheduled"), weekly: L("Every week"), monthly: L("Every month"), other: cron}[
                scheduleKey
            ],
            runAsName: runAsUser ? text(runAsUser.userFullName) || text(runAsUser.username) : null
        },
        externalRequests: !offline(),
        importMaxBytes: text(ec.factory.getConfValue("maps@import-max-bytes")) || null
    }
}

// ===== acknowledge, start, refresh, schedule =====
function acknowledgeLicense(p) {
    const set = one(DATA_SET, {dataSetId: text(p.dataSetId)})
    if (!set) return refuse(say("There is no Geo data set ${id}", {id: text(p.dataSetId)}))
    if (!ec.user.userId) return refuse(L("Sign in to acknowledge a data set's license"))
    asSystemWrite(() => set.setAll({licenseAckUserId: ec.user.userId, licenseAckDate: ec.user.nowTimestamp}).update())
    return {licenseAckDate: set.licenseAckDate}
}

// An uploaded operator's file, kept where the importer reads an operator's file from
function keepUpload(file) {
    const max = Number(text(ec.factory.getConfValue("maps@import-max-bytes")) || 0)
    const size = Number(file.getSize())
    if (max > 0 && size > max)
        throw new Error(say("The file is ${size} bytes, more than maps@import-max-bytes (${max})", {size, max}))
    let name = String(file.getName() || "")
        .split(/[\\/]/)
        .pop()
        .replace(/[\u0000-\u001f%]/g, "")
        .trim()
    if (!name || name === "." || name === "..") name = "geo-data.csv"
    if (name.length > 120) name = name.slice(-120)
    const stamp = Date.now().toString(36) + Math.floor(Math.random() * 1e6).toString(36)
    const location = `${IMPORT_ROOT}/${ec.user.userId}/${stamp}/${name}`
    asSystemWrite(() => file.write(location))
    return location
}

// Whether a set read a country at a time has a file for `country` (its countryCodes; any country when it names none)
function countryAllowed(set, country) {
    const codes = text(set.countryCodes)
        .split(",")
        .map(code => code.trim())
        .filter(code => code)
    return !codes.length || codes.includes(text(country.geoCodeAlpha2))
}
// The countries Import offers for a set: those of its countryCodes, else every country, by name
function countryOptions(p) {
    const set = one(DATA_SET, {dataSetId: text(p.dataSetId)})
    const codes = set
        ? text(set.countryCodes)
              .split(",")
              .map(code => code.trim())
              .filter(code => code)
        : []
    let countries = find("moqui.basic.Geo", {geoTypeEnumId: "GEOT_COUNTRY"}).useCache(true)
    if (codes.length) countries = countries.condition("geoCodeAlpha2", "in", codes)
    return {
        countryOptions: Array.from(countries.orderBy("geoName").limit(400).list()).map(geo => ({
            geoId: geo.geoId,
            geoName: text(geo.geoName) || geo.geoId
        }))
    }
}

// A scope's run in progress, of the source key and country (or of either with none), if any
function runningRun(set, countryGeoId) {
    return runsOf(set.sourceKey).find(
        run => IN_PROGRESS.includes(run.statusId) && (!countryGeoId || !run.countryGeoId || run.countryGeoId === countryGeoId)
    )
}

function startImport(p) {
    const set = one(DATA_SET, {dataSetId: text(p.dataSetId)})
    if (!set) return refuse(say("There is no Geo data set ${id}", {id: text(p.dataSetId)}))
    if (set.disabled === "Y") return refuse(say("Geo data set ${id} is disabled", {id: set.dataSetId}))
    if (!set.licenseAckUserId)
        return refuse(
            say("Acknowledge the license of ${id} (${license}) before importing it", {
                id: set.dataSetId,
                license: text(set.licenseName)
            })
        )
    const operatorFile = !text(set.urlTemplate)
    if (!operatorFile && offline())
        return refuse(
            say("Geo data set ${id} is downloaded, and outside requests for maps are off (maps@external-requests)", {
                id: set.dataSetId
            })
        )
    let countryGeoId = set.coverageGeoId || null
    if (!countryGeoId && (set.perCountry === "Y" || operatorFile)) {
        const country = countryOf(p.countryGeoId)
        if (!country) return refuse(L("Choose the country to import"))
        if (!countryAllowed(set, country))
            return refuse(say("${id} has no file for ${country}", {id: set.dataSetId, country: text(country.geoName)}))
        countryGeoId = country.geoId
    }
    // refused before an upload is written
    const running = runningRun(set, countryGeoId)
    if (running)
        return refuse(
            say("An import of ${id} for ${country} is in progress (${run}); cancel it or wait for it to end", {
                id: set.dataSetId,
                country: countryName(countryGeoId) || L("every country"),
                run: running.geoDataImportId
            })
        )
    let location = null
    if (operatorFile) {
        try {
            location = p.file ? keepUpload(p.file) : text(p.location) || null
            // Refresh of an operator's file: the file its scope's last run read, found here, never sent by a browser
            if (!location && (p.again === true || p.again === "true")) {
                const last = runsOf(set.sourceKey).find(run => run.sourceLocation && run.countryGeoId === countryGeoId)
                location = last ? last.sourceLocation : null
            }
        } catch (error) {
            return refuse(String(error.message || error))
        }
        if (!location) return refuse(L("Choose the file to import"))
    }
    const force = p.force === true || p.force === "true" || p.force === "Y"
    const pending = asSystemWrite(() => {
        const row = ec.entity
            .makeValue(IMPORT)
            .setAll({
                dataSetId: set.dataSetId,
                countryGeoId,
                statusId: "GdiPending",
                sourceLocation: location,
                forced: force ? "Y" : "N",
                userId: ec.user.userId,
                fromDate: ec.user.nowTimestamp
            })
            .setSequencedIdPrimary()
        row.create()
        return row
    })
    const jobRunId = ec.service.job(IMPORT_JOB).parameters({geoDataImportId: pending.geoDataImportId, location, force}).run()
    if (!jobRunId) {
        asSystemWrite(() =>
            pending
                .setAll({statusId: "GdiFailed", failureText: L("The import job did not start"), thruDate: ec.user.nowTimestamp})
                .update()
        )
        return {geoDataImportId: pending.geoDataImportId}
    }
    asSystemWrite(() => pending.setAll({jobRunId}).update())
    return {geoDataImportId: pending.geoDataImportId, jobRunId}
}

// The scopes a refresh runs again: each in use (it ever completed a run and still has rows; a failed or cancelled last run
// does not drop it), but one with a run in progress or of a disabled set, with the set of its newest run and an
// operator's file where its last run read one
function refreshScopes() {
    return scopesInUse()
        .filter(scope => !IN_PROGRESS.includes(scope.newest.statusId) && scope.set.disabled !== "Y")
        .map(scope => ({
            dataSetId: scope.set.dataSetId,
            countryGeoId: scope.countryGeoId,
            location: text(scope.set.urlTemplate)
                ? null
                : (scope.runs.find(run => run.sourceLocation) || {}).sourceLocation || null
        }))
}

function requestRefresh(p) {
    const jobRunId = ec.service
        .job(REFRESH_JOB)
        .parameters(p.reason ? {reason: String(p.reason).slice(0, 255)} : {})
        .run()
    if (!jobRunId) return refuse(L("The Geo data refresh did not start"))
    return {jobRunId}
}

// The refresh job's run: each scope imported again in turn, one failing not stopping the rest
function runRefresh(p) {
    const scopes = refreshScopes()
    let completed = 0,
        unchanged = 0,
        failed = 0
    for (const scope of scopes) {
        ec.message.pushErrors()
        try {
            const out =
                ec.service
                    .sync()
                    .name("org.moqui.maps.GeoDataServices.import#GeoData")
                    .parameters(Object.assign({runAsUserId: text(p.runAsUserId) || null}, scope))
                    .call() || {}
            if (ec.message.hasError() || out.statusId === "GdiFailed" || !out.statusId) {
                failed++
                ec.logger.error(
                    "Geo data refresh of " +
                        scope.dataSetId +
                        " " +
                        (scope.countryGeoId || "") +
                        " did not complete: " +
                        (ec.message.getErrorsString() || out.failureText || "")
                )
            } else if (out.statusId === "GdiUnchanged") unchanged++
            else completed++
        } catch (error) {
            failed++
            ec.logger.error(
                "Geo data refresh of " +
                    scope.dataSetId +
                    " " +
                    (scope.countryGeoId || "") +
                    " failed: " +
                    (error?.message ?? error)
            )
        } finally {
            ec.message.clearErrors()
            ec.message.popErrors()
        }
    }
    const outcome = say("${completed} imported again, ${unchanged} unchanged at the source, ${failed} did not complete", {
        completed,
        unchanged,
        failed
    })
    return {scopeCount: scopes.length, completedCount: completed, unchangedCount: unchanged, failedCount: failed, outcome}
}

function scheduleRefresh(p) {
    const key = text(p.scheduleKey) || "none"
    if (key !== "none" && !SCHEDULES[key]) return refuse(L("Choose every week, every month or not scheduled"))
    if (key !== "none" && !ec.user.userId) return refuse(L("Sign in to schedule the Geo data refresh"))
    asSystemWrite(() => {
        const job = one("moqui.service.job.ServiceJob", {jobName: REFRESH_JOB})
        if (!job) throw new Error("The " + REFRESH_JOB + " ServiceJob is not there")
        if (key === "none") {
            job.setAll({cronExpression: null, paused: "Y"}).update()
            return
        }
        job.setAll({cronExpression: SCHEDULES[key], paused: "N"}).update()
        // a scheduled run has no user: it runs as the administrator who scheduled it, a user of the job who added
        // themselves (import#GeoData checks both), and notifies them
        ec.entity
            .makeValue("moqui.service.job.ServiceJobUser")
            .setAll({jobName: REFRESH_JOB, userId: ec.user.userId, receiveNotifications: "Y", addedByUserId: ec.user.userId})
            .createOrUpdate()
        ec.entity
            .makeValue("moqui.service.job.ServiceJobParameter")
            .setAll({jobName: REFRESH_JOB, parameterName: "runAsUserId", parameterValue: ec.user.userId})
            .createOrUpdate()
    })
    return {cronExpression: SCHEDULES[key] || null}
}

// ===== readiness (org.moqui.maps.MapServices.check#GeoDataReadiness) =====
const check = (checkKey, title, state, reasonCode, message, extra) =>
    Object.assign({checkKey, title, state, reasonCode, message, subject: "Instance"}, extra || {})
function readiness(p) {
    if (text(p.providerKey) !== "maps.geodata") throw new Error("Unknown Geo data readiness check " + text(p.providerKey))
    if (!ec.user.hasPermission("ADMIN_OVERVIEW_VIEW")) {
        const error = new Error("PERMISSION_DENIED: ADMIN_OVERVIEW_VIEW is required")
        error.reason = "PERMISSION_DENIED"
        throw error
    }
    const fix = {fixRoute: "mapsConfiguration"}
    const checks = []
    const locale = text(ec.factory.getConfValue("instance@default-locale"))
    const country = countryOf(locale.split(/[_-]/)[1] || "")
    const name = country ? text(country.geoName) || country.geoId : ""
    let codes = 0
    if (country) {
        codes = Number(find("moqui.basic.GeoPostalCode", {countryGeoId: country.geoId}).condition("thruDate", null).count())
        checks.push(
            codes > 0
                ? check(
                      "maps.geodata.default",
                      "Postal lookup",
                      "ready",
                      null,
                      say("${count} postal codes of ${country} can be looked up", {count: codes, country: name})
                  )
                : check(
                      "maps.geodata.default",
                      "Postal lookup",
                      "warning",
                      "GEO_DATA_MISSING",
                      say(
                          "No postal code data of ${country} is imported: postal lookup and approximate places find nothing until a data set is imported under Maps › Configuration › Geo data",
                          {country: name}
                      ),
                      fix
                  )
        )
    }
    // the newest run of each scope in use: one that never completed, or was removed, is not a failure of data in use
    const failed = scopesInUse()
        .map(scope => scope.newest)
        .filter(run => run.statusId === "GdiFailed")
    checks.push(
        failed.length
            ? check(
                  "maps.geodata.imports",
                  "Geo data imports",
                  "failed",
                  "IMPORT_FAILED",
                  say("The last import did not complete for: ${scopes}", {
                      scopes: failed
                          .map(run => run.dataSetId + (run.countryGeoId ? " (" + countryName(run.countryGeoId) + ")" : ""))
                          .join(", ")
                  }),
                  fix
              )
            : check("maps.geodata.imports", "Geo data imports", "ready", null, L("No Geo data import's last run failed"))
    )
    if (country && codes === 0) {
        const covering = allSets().filter(
            set =>
                set.disabled !== "Y" &&
                set.yieldsEnumId === "GDY_POSTAL_POINTS" &&
                text(set.urlTemplate) &&
                (set.coverageGeoId === country.geoId || (!set.coverageGeoId && set.perCountry === "Y"))
        )
        if (covering.length && !covering.some(set => set.licenseAckUserId))
            checks.push(
                check(
                    "maps.geodata.license",
                    "Geo data licenses",
                    "warning",
                    "LICENSE_NOT_ACKNOWLEDGED",
                    say(
                        "No license of a postal data set for ${country} is acknowledged (${sets}): acknowledge one to import it",
                        {country: name, sets: covering.map(set => set.dataSetId).join(", ")}
                    ),
                    fix
                )
            )
    }
    return {checks}
}

const operations = {
    countryOptions,
    findGeoDataSets,
    acknowledgeLicense,
    startImport,
    requestRefresh,
    runRefresh,
    scheduleRefresh,
    readiness
}
;({
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown Geo data operation " + op)
        return operations[op](parameters || {})
    }
})
