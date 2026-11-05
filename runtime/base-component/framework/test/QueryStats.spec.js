// Statement stats from the entity facade (plans/archive/Tools-Database.md §7.1): on in the dev conf
// (entity-facade@query-stats).
const S = 'org.moqui.impl.BasicServices.get#GeoRegionsForDropDown'
// A Tools screen's transition calls the database services with the screen's inherited TOOLS_APP authorization;
// outside a screen the spec stands in for that with authorization disabled.
function asAdmin(fn) { ec.artifactExecution.disableAuthz(); try { return fn() } finally { ec.artifactExecution.enableAuthz() } }
spec({ setupSpec() { login('john.doe') } })
feature('a find counts its statement under its entity and the service that ran it', {
    when() {
        asAdmin(() => ec.entity.clearQueryStats())
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
feature('clearing the stats, with DATABASE_ADMIN, forgets every statement and writes an audit row', {
    when() {
        asAdmin(() => call(S, { geoId: 'USA' }))
        asAdmin(() => ec.entity.clearQueryStats())
        const audits = asAdmin(() => Array.from(ec.entity.find('moqui.database.DatabaseAudit')
            .condition({ actionEnumId: 'DbaStatsReset', userId: 'EX_JOHN_DOE' }).list()))
        return { rows: Array.from(ec.entity.getQueryStatsList(null, 'GeoAssocAndToDetail', null)), audits }
    },
    then({ rows, audits }) {
        rows.length == 0
        audits.length >= 1
    },
})
feature('a user the Tools app does not let in is refused the database services and the reset', {
    when() {
        asAdmin(() => call('create#moqui.security.UserAccount', { userId: 'QS_PLAIN_USER', username: 'qs.plain.user',
            userFullName: 'Query stats plain user', currentPassword: '16ac58bbfa332c1c55bd98b53e60720bfa90d394',
            passwordHashType: 'SHA' }))
        login('qs.plain.user')
        try {
            let capabilities = {}
            let capabilitiesError = ''
            try { capabilities = call('org.moqui.impl.DatabaseServices.get#Capabilities', {}) || {} } catch (e) {
                capabilitiesError = String(e.message || e)
            }
            ec.message.clearErrors()
            asAdmin(() => call(S, { geoId: 'USA' }))
            let resetError = ''
            try { ec.entity.clearQueryStats() } catch (e) { resetError = String(e.message || e) }
            resetError += ec.message.getErrorsString()
            ec.message.clearErrors()
            // reached as a screen would reach it, the service's own DATABASE_ADMIN check still refuses
            let adminError = ''
            try { asAdmin(() => ec.entity.clearQueryStats()) } catch (e) { adminError = String(e.message || e) }
            adminError += ec.message.getErrorsString()
            ec.message.clearErrors()
            return { capabilities, capabilitiesError, resetError, adminError,
                rows: Array.from(ec.entity.getQueryStatsList(null, 'GeoAssocAndToDetail', null)) }
        } finally { ec.message.clearErrors(); login('john.doe') }
    },
    then({ capabilities, capabilitiesError, resetError, adminError, rows }) {
        capabilities.features == null
        capabilitiesError.includes('not authorized')
        resetError.includes('not authorized')
        adminError.includes('DATABASE_ADMIN')
        rows.length >= 1
    },
})
