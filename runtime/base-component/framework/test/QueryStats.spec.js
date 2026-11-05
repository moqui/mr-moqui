// Statement stats from the entity facade (plans/Tools-Database.md §7.1): on in the dev conf (entity-facade@query-stats).
const S = 'org.moqui.impl.BasicServices.get#GeoRegionsForDropDown'
function asAdmin(fn) { ec.artifactExecution.disableAuthz(); try { return fn() } finally { ec.artifactExecution.enableAuthz() } }
spec({ setupSpec() { login('john.doe') } })
feature('a find counts its statement under its entity and the service that ran it', {
    when() {
        ec.entity.clearQueryStats()
        asAdmin(() => call(S, { geoId: 'USA' }))
        const rows = Array.from(ec.entity.getQueryStatsList('-hitCount', 'GeoAssocAndToDetail', null))
        const row = rows.find(r => r.sql.includes('?') || r.sql.includes('$1')) || {}
        return { rows, row, artifacts: Object.keys(row.artifactCounts || {}) }
    },
    then({ rows, row, artifacts }) {
        rows.length >= 1
        row.entityName == 'moqui.basic.GeoAssocAndToDetail'
        row.hitCount >= 1
        row.errorCount == 0
        row.totalTime >= row.maxTime
        artifacts.some(a => a.includes('GeoRegionsForDropDown'))
    },
})
feature('clearing the stats forgets every statement', {
    when() {
        asAdmin(() => call(S, { geoId: 'USA' }))
        ec.entity.clearQueryStats()
        return { rows: Array.from(ec.entity.getQueryStatsList(null, 'GeoAssocAndToDetail', null)) }
    },
    then({ rows }) {
        rows.length == 0
    },
})
