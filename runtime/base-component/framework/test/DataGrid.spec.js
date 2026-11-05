// The data grid's services (plans/Tools-Database.md §9, §15 step 5): pages without a count, a SQL result's entity, and
// the save rules: a refused row rolls back the rest, a stale stamp is refused, a restricted entity is refused.
const D = 'org.moqui.impl.DatabaseServices.'
const E = 'moqui.basic.Enumeration'
const source = { type: 'entity', entityName: E, conditions: { enumTypeId: 'GridSpec' } }
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
const description = (enumId) => asAdmin(() => ec.entity.find(E).condition({ enumId }).useCache(false).one()?.description)
spec({
    setupSpec() {
        login('john.doe')
        asAdmin(() => {
            call('create#moqui.basic.EnumerationType', { enumTypeId: 'GridSpec', description: 'Grid spec' })
            for (const id of ['GS_A', 'GS_B', 'GS_C'])
                call('create#moqui.basic.Enumeration', { enumId: id, enumTypeId: 'GridSpec', description: 'Was ' + id })
        })
    },
})
feature('pages come without a count, and a SQL result of one table with its key saves to its entity', {
    when() {
        const columns = invoke(D + 'get#DataGridColumns', { source, columns: ['enumId', 'description'] })
        const first = invoke(D + 'get#DataGridRows', { source, page: 1, size: 2, sort: [{ field: 'enumId', dir: 'asc' }] })
        const second = invoke(D + 'get#DataGridRows', { source, page: 2, size: 2, sort: [{ field: 'enumId', dir: 'asc' }] })
        const filtered = invoke(D + 'get#DataGridRows', { source, page: 1, size: 10,
            filter: [{ field: 'description', type: 'like', value: 'was gs_b' }] })
        const sql = { type: 'sql', sql: "SELECT enum_id, description FROM enumeration WHERE enum_type_id = 'GridSpec'" }
        const sqlColumns = invoke(D + 'get#DataGridColumns', { source: sql })
        const sqlRows = invoke(D + 'get#DataGridRows', { source: sql, page: 1, size: 10, sort: [{ field: 'enum_id', dir: 'desc' }] })
        const count = invoke(D + 'count#DataGridRows', { source: sql })
        return { columns, first, second, filtered, sqlColumns, sqlRows, count }
    },
    then({ columns, first, second, filtered, sqlColumns, sqlRows, count }) {
        columns.entityName == E
        columns.columns.find(c => c.name == 'enumId').pk == true
        columns.columns.find(c => c.name == 'description').editor == 'input'
        first.last_page == 2
        first.data.length == 2
        first.data[0]._stamp != null
        second.last_page == 2
        second.last_row == 3
        filtered.data.map(r => r.enumId).join() == 'GS_B'
        sqlColumns.entityName == E
        sqlColumns.columns.find(c => c.name == 'enum_id').field == 'enumId'
        sqlRows.data[0].enum_id == 'GS_C'
        count.count == 3
    },
})
feature('a stale stamp is refused and nothing of the save is written', {
    when() {
        const rows = invoke(D + 'get#DataGridRows', { source, page: 1, size: 10 }).data
        const stamp = (id) => rows.find(r => r.enumId == id)._stamp
        const saved = invoke(D + 'save#DataGridChanges', { entityName: E, updates: [
            { pk: { enumId: 'GS_A' }, stamp: stamp('GS_A'), fields: { description: 'Now A' } },
            { pk: { enumId: 'GS_B' }, stamp: '2000-01-01T00:00:00Z', fields: { description: 'Now B' } }] })
        return { saved, a: description('GS_A'), b: description('GS_B') }
    },
    then({ saved, a, b }) {
        saved.saved == false
        saved.refused.length == 1
        saved.refused[0].conflict == true
        saved.refused[0].message.includes('changed by someone else since you loaded it')
        a == 'Was GS_A'
        b == 'Was GS_B'
    },
})
feature('a row the entity-auto service refuses rolls back the rest; a clean save writes them all', {
    when() {
        const stampOf = () => invoke(D + 'get#DataGridRows', { source, page: 1, size: 10 }).data.find(r => r.enumId == 'GS_A')._stamp
        const refused = invoke(D + 'save#DataGridChanges', { entityName: E,
            updates: [{ pk: { enumId: 'GS_A' }, stamp: stampOf(), fields: { description: 'Now A' } }],
            creates: [{ fields: { enumId: 'GS_B', enumTypeId: 'GridSpec', description: 'Duplicate key' } }] })
        const afterRefused = description('GS_A')
        const clean = invoke(D + 'save#DataGridChanges', { entityName: E,
            updates: [{ pk: { enumId: 'GS_A' }, stamp: stampOf(), fields: { description: 'Now A' } }],
            creates: [{ fields: { enumId: 'GS_D', enumTypeId: 'GridSpec', description: 'New D' } }],
            deletes: [{ pk: { enumId: 'GS_C' } }] })
        return { refused, afterRefused, clean, a: description('GS_A'), d: description('GS_D'), c: description('GS_C') }
    },
    then({ refused, afterRefused, clean, a, d, c }) {
        refused.saved == false
        refused.refused[0].kind == 'create'
        afterRefused == 'Was GS_A'
        clean.saved == true
        clean.updated == 1 && clean.created == 1 && clean.deleted == 1
        a == 'Now A'
        d == 'New D'
        c == null
    },
})
feature('a restricted entity is refused for reading and saving', {
    when() {
        const restricted = { type: 'entity', entityName: 'moqui.security.UserGroupMember' }
        return {
            rows: invoke(D + 'get#DataGridRows', { source: restricted, page: 1, size: 5 }),
            save: invoke(D + 'save#DataGridChanges', { entityName: 'moqui.security.UserGroupMember',
                creates: [{ fields: { userGroupId: 'ADMIN', userId: 'EX_JOHN_DOE', fromDate: '2001-01-01T00:00:00Z' } }] }),
        }
    },
    then({ rows, save }) {
        rows.error.includes('not available in the data grid')
        rows.data == null
        save.error.includes('not available in the data grid')
        save.saved == null
    },
})
feature('an export writes the whole source; a lookup lists the related entity; grid views are the owner\'s', {
    when() {
        const exported = invoke(D + 'export#DataGrid', { source, columns: ['enumId', 'description'], format: 'csv' })
        const text = exported.location ? ec.resource.getLocationText(exported.location) : ''
        const elsewhere = invoke(D + 'export#DataGrid', { source, location: 'component://framework/data/Grid.csv' })
        const lookup = invoke(D + 'get#DataGridLookup', { entityName: E, field: 'enumTypeId', term: 'grid spec' })
        const view = invoke(D + 'store#GridView', { description: 'Spec view', sourceJson: JSON.stringify({ source }) })
        const views = invoke(D + 'find#GridViews', {})
        const removed = invoke(D + 'delete#GridView', { viewId: view.viewId })
        return { exported, text, elsewhere, lookup, view, views, removed }
    },
    then({ exported, text, elsewhere, lookup, view, views, removed }) {
        exported.rowCount >= 3
        text.split('\n')[0].trim() == 'enumId,description'
        text.includes('GS_A,')
        elsewhere.error.includes('dbresource://')
        lookup.options.some(o => o.value == 'GridSpec' && o.label.includes('Grid spec'))
        views.gridViews.some(v => v.viewId == view.viewId && v.ownerUserId == 'EX_JOHN_DOE')
        removed.error == ''
    },
})
