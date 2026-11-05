// The Tools SQL services (plans/Tools-Database.md §8, §14): the run, history, saved queries and catalog as a Tools screen
// calls them, and their own permission check. The database rules (read only, the token, audit, cancel) are tested in
// crates/moqui-tools sql_tests.rs.
const D = 'org.moqui.impl.DatabaseServices.'
// A Tools screen's transition calls these with the screen's inherited TOOLS_APP authorization; outside a screen the
// spec stands in for that with authorization disabled.
function asAdmin(fn) { ec.artifactExecution.disableAuthz(); try { return fn() } finally { ec.artifactExecution.enableAuthz() } }
function invoke(name, parameters) {
    try {
        const result = asAdmin(() => call(name, parameters)) || {}
        const error = ec.message.getErrorsString()
        ec.message.clearErrors()
        return Object.assign({}, result, { error })
    } catch (e) { ec.message.clearErrors(); return { error: String(e.message || e) } }
}
spec({ setupSpec() { login('john.doe') } })
feature('a read-only run returns rows and the user sees it in the history; a write needs the confirmation', {
    when() {
        const read = invoke(D + 'run#Sql', { sqlText: "SELECT enum_id FROM enumeration WHERE enum_type_id = :type ORDER BY enum_id",
            parameters: { type: 'DatabaseAuditAction' } })
        const write = invoke(D + 'run#Sql', { sqlText: 'CREATE TEMP TABLE spec_note (a INT)', allowChanges: true })
        const history = invoke(D + 'find#SqlHistory', { limit: 5 })
        return { read, write, history }
    },
    then({ read, write, history }) {
        read.error == ''
        read.results[0].records.map(r => r[0]).includes('DbaSql')
        read.results[0].rerunnable == true
        write.error.includes('confirmation')
        history.history[0].statementText.includes('enumeration')
    },
})
feature('saved queries are the owner\'s to change; the catalog maps entities to their tables; the database\'s stats read', {
    when() {
        const saved = invoke(D + 'store#SavedQuery', { description: 'Spec query', sqlText: 'SELECT 1', isShared: false })
        const found = invoke(D + 'find#SavedQueries', {})
        const removed = invoke(D + 'delete#SavedQuery', { queryId: saved.queryId })
        const after = invoke(D + 'find#SavedQueries', {})
        const catalog = invoke(D + 'get#SqlCatalog', {})
        const stats = invoke(D + 'list#DatabaseStatements', { limit: 5 })
        return { saved, found, removed, after, catalog, stats }
    },
    then({ saved, found, removed, after, catalog, stats }) {
        saved.queryId != null
        found.savedQueries.some(q => q.queryId == saved.queryId && q.ownerUserId == 'EX_JOHN_DOE')
        removed.error == ''
        !after.savedQueries.some(q => q.queryId == saved.queryId)
        catalog.entities['moqui.basic.Enumeration'] == 'enumeration'
        catalog.tables['enumeration'].includes('enum_type_id')
        stats.available == true
        stats.statements.length >= 1
    },
})
feature('a user without SQL_RUNNER_WEB is refused the SQL services', {
    when() {
        asAdmin(() => call('create#moqui.security.UserAccount', { userId: 'SQL_PLAIN_USER', username: 'sql.plain.user',
            userFullName: 'SQL plain user', currentPassword: '16ac58bbfa332c1c55bd98b53e60720bfa90d394', passwordHashType: 'SHA' }))
        login('sql.plain.user')
        try {
            return { run: invoke(D + 'run#Sql', { sqlText: 'SELECT 1' }), saved: invoke(D + 'find#SavedQueries', {}) }
        } finally { login('john.doe') }
    },
    then({ run, saved }) {
        run.error.includes('SQL_RUNNER_WEB')
        run.results == null
        saved.error.includes('SQL_RUNNER_WEB')
    },
})
