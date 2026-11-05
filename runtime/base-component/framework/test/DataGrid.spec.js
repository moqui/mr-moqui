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
            call('create#moqui.security.UserAccount', { userId: 'GRID_PLAIN_USER', username: 'grid.plain.user',
                userFullName: 'Grid plain user', currentPassword: '16ac58bbfa332c1c55bd98b53e60720bfa90d394', passwordHashType: 'SHA' })
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
        const sql = { type: 'sql', sql: "SELECT enum_id, description, last_updated_stamp FROM enumeration WHERE enum_type_id = 'GridSpec'" }
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
        sqlColumns.keyColumns.join() == 'enum_id'
        sqlColumns.stampColumn == 'last_updated_stamp'
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
        saved.refused[0].message.includes('since you loaded it')
        a == 'Was GS_A'
        b == 'Was GS_B'
    },
})
feature('a row the entity-auto service refuses rolls back the rest; a clean save writes them all', {
    when() {
        const stampOf = (id = 'GS_A') => invoke(D + 'get#DataGridRows', { source, page: 1, size: 10 }).data.find(r => r.enumId == id)._stamp
        const refused = invoke(D + 'save#DataGridChanges', { entityName: E,
            updates: [{ pk: { enumId: 'GS_A' }, stamp: stampOf(), fields: { description: 'Now A' } }],
            creates: [{ fields: { enumId: 'GS_B', enumTypeId: 'GridSpec', description: 'Duplicate key' } }] })
        const afterRefused = description('GS_A')
        const clean = invoke(D + 'save#DataGridChanges', { entityName: E,
            updates: [{ pk: { enumId: 'GS_A' }, stamp: stampOf(), fields: { description: 'Now A' } }],
            creates: [{ fields: { enumId: 'GS_D', enumTypeId: 'GridSpec', description: 'New D' } }],
            deletes: [{ pk: { enumId: 'GS_C' }, stamp: stampOf('GS_C') }] })
        const unstamped = invoke(D + 'save#DataGridChanges', { entityName: E, deletes: [{ pk: { enumId: 'GS_B' } }] })
        return { refused, afterRefused, clean, unstamped, a: description('GS_A'), d: description('GS_D'), c: description('GS_C'),
            b: description('GS_B') }
    },
    then({ refused, afterRefused, clean, unstamped, a, d, c, b }) {
        unstamped.saved == false
        unstamped.refused[0].message.includes('stamp')
        b == 'Was GS_B'
        clean.createdKeys[0].enumId == 'GS_D'
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
        const lookup = invoke(D + 'get#DataGridLookup', { entityName: E, field: 'enumTypeId', term: 'grid spec' })
        const view = invoke(D + 'store#GridView', { description: 'Spec view', sourceJson: JSON.stringify({ source }) })
        const views = invoke(D + 'find#GridViews', {})
        const removed = invoke(D + 'delete#GridView', { viewId: view.viewId })
        return { exported, text, lookup, view, views, removed }
    },
    then({ exported, text, lookup, view, views, removed }) {
        exported.rowCount >= 3
        text.split('\n')[0].trim() == 'enumId,description'
        text.includes('GS_A,')
        lookup.options.some(o => o.value == 'GridSpec' && o.label.includes('Grid spec'))
        views.gridViews.some(v => v.viewId == view.viewId && v.ownerUserId == 'EX_JOHN_DOE')
        removed.error == ''
    },
})
feature('secrets stay out through a view, take no condition, and a self-join reads but does not save', {
    when() {
        const view = { type: 'entity', entityName: 'moqui.security.UserAccountAndGroup', conditions: { userId: 'EX_JOHN_DOE' } }
        const columns = invoke(D + 'get#DataGridColumns', { source: view })
        const rows = invoke(D + 'get#DataGridRows', { source: view, page: 1, size: 5 })
        const oracle = invoke(D + 'count#DataGridRows', { source: { type: 'entity', entityName: 'moqui.security.UserAccount',
            conditions: { currentPassword: '1', currentPassword_op: 'begins' } } })
        const sorted = invoke(D + 'get#DataGridRows', { source: view, page: 1, size: 5, sort: [{ field: 'passwordSalt', dir: 'asc' }] })
        const join = invoke(D + 'get#DataGridColumns', { source: { type: 'sql', sql: "SELECT a.enum_id, a.description, a.last_updated_stamp " +
            "FROM enumeration a JOIN enumeration b ON b.enum_id = a.enum_id WHERE a.enum_type_id = 'GridSpec'" } })
        return { columns, rows, oracle, sorted, join }
    },
    then({ columns, rows, oracle, sorted, join }) {
        columns.columns.length > 0
        !columns.columns.some(c => ['currentPassword', 'passwordSalt', 'resetPassword', 'passwordHashType'].includes(c.name))
        rows.data.length >= 1
        rows.data.every(r => r.currentPassword === undefined && r.passwordSalt === undefined && r.resetPassword === undefined)
        oracle.error.includes('takes no condition')
        sorted.error.includes('not a field the grid sorts or filters on')
        join.entityName == null
        join.readOnlyReason.includes('JOIN')
    },
})
feature('an export goes to the user\'s own place unless they are a database admin; reads run with authorization on', {
    when() {
        login('grid.plain.user')
        let own, other
        try {
            own = invoke(D + 'export#DataGrid', { source, location: 'runtime://tmp/grid-export/GRID_PLAIN_USER/spec/a.csv' })
            other = invoke(D + 'export#DataGrid', { source, location: 'runtime://tmp/grid-export/EX_JOHN_DOE/spec/a.csv' })
        } finally { login('john.doe') }
        // the service granted as a Tools screen's inherited authorization would let it run
        asAdmin(() => {
            call('create#moqui.security.ArtifactGroup', { artifactGroupId: 'GRID_SPEC_API', description: 'DataGrid spec' })
            call('create#moqui.security.ArtifactGroupMember', { artifactGroupId: 'GRID_SPEC_API',
                artifactName: D + 'get#DataGridRows', artifactTypeEnumId: 'AT_SERVICE', inheritAuthz: 'Y' })
            call('create#moqui.security.ArtifactAuthz', { artifactAuthzId: 'GRID_SPEC_ADMIN', userGroupId: 'ADMIN',
                artifactGroupId: 'GRID_SPEC_API', authzTypeEnumId: 'AUTHZT_ALLOW', authzActionEnumId: 'AUTHZA_ALL' })
        })
        // signed in again, so the new grant is read
        login('john.doe')
        let authorized
        try { authorized = call(D + 'get#DataGridRows', { source, page: 1, size: 10 }) || {} }
        catch (e) { authorized = { error: String(e.message || e) } }
        ec.message.clearErrors()
        return { own, other, authorized }
    },
    then({ own, other, authorized }) {
        own.error == ''
        own.rowCount >= 2
        other.error.includes('DATABASE_ADMIN')
        authorized.data.length >= 2
    },
})
feature('a data document gives every field it defines, its expression field read only and computed in the rows', {
    when() {
        asAdmin(() => {
            call('create#moqui.entity.document.DataDocument', { dataDocumentId: 'GridSpecDoc', documentName: 'Grid spec doc',
                primaryEntityName: E })
            call('create#moqui.entity.document.DataDocumentField', { dataDocumentId: 'GridSpecDoc', fieldSeqId: '01', fieldPath: 'enumId' })
            call('create#moqui.entity.document.DataDocumentField', { dataDocumentId: 'GridSpecDoc', fieldSeqId: '02', fieldPath: 'description' })
            call('create#moqui.entity.document.DataDocumentField', { dataDocumentId: 'GridSpecDoc', fieldSeqId: '03',
                fieldPath: "(enumId + ' / ' + description)", fieldNameAlias: 'enumLabel' })
        })
        const doc = { type: 'entity', entityName: 'DataDocument.GridSpecDoc', conditions: { enumId: 'GS_A' } }
        const columns = invoke(D + 'get#DataGridColumns', { source: doc })
        const rows = invoke(D + 'get#DataGridRows', { source: doc, page: 1, size: 5 })
        const sorted = invoke(D + 'get#DataGridRows', { source: doc, page: 1, size: 5, sort: [{ field: 'enumLabel', dir: 'asc' }] })
        const conditioned = invoke(D + 'get#DataGridRows', { source: Object.assign({}, doc, { conditions: { enumLabel: 'x' } }),
            page: 1, size: 5 })
        const exported = invoke(D + 'export#DataGrid', { source: doc, columns: ['enumId', 'enumLabel'], format: 'csv' })
        const text = exported.location ? ec.resource.getLocationText(exported.location) : ''
        // the definitions are not saved through the grid, where a Tools viewer could plant an expression
        const planted = invoke(D + 'save#DataGridChanges', { entityName: 'moqui.entity.document.DataDocumentField',
            updates: [{ pk: { dataDocumentId: 'GridSpecDoc', fieldSeqId: '03' }, fields: { fieldPath: '(1)' } }] })
        // an expression over an encrypted field or password material reads without it
        asAdmin(() => {
            call('create#moqui.monitor.ScrapeTarget', { targetId: 'GRID_SPEC_T', url: 'http://localhost:1/metrics',
                bearerToken: 'grid-spec-secret' })
            call('create#moqui.entity.document.DataDocument', { dataDocumentId: 'GridSpecTokenDoc', documentName: 'Grid spec token',
                primaryEntityName: 'moqui.monitor.ScrapeTarget' })
            call('create#moqui.entity.document.DataDocumentField', { dataDocumentId: 'GridSpecTokenDoc', fieldSeqId: '01', fieldPath: 'targetId' })
            call('create#moqui.entity.document.DataDocumentField', { dataDocumentId: 'GridSpecTokenDoc', fieldSeqId: '02',
                fieldPath: 'bearerToken', fieldNameAlias: 'renamedToken' })
            call('create#moqui.entity.document.DataDocumentField', { dataDocumentId: 'GridSpecTokenDoc', fieldSeqId: '03',
                fieldPath: "('t:' + renamedToken)", fieldNameAlias: 'tokenShown' })
            call('create#moqui.entity.document.DataDocument', { dataDocumentId: 'GridSpecUserDoc', documentName: 'Grid spec user',
                primaryEntityName: 'moqui.security.UserAccount' })
            call('create#moqui.entity.document.DataDocumentField', { dataDocumentId: 'GridSpecUserDoc', fieldSeqId: '01', fieldPath: 'userId' })
            call('create#moqui.entity.document.DataDocumentField', { dataDocumentId: 'GridSpecUserDoc', fieldSeqId: '02',
                fieldPath: 'currentPassword', fieldNameAlias: 'pw' })
            call('create#moqui.entity.document.DataDocumentField', { dataDocumentId: 'GridSpecUserDoc', fieldSeqId: '03',
                fieldPath: "('p:' + pw)", fieldNameAlias: 'pwShown' })
        })
        const token = invoke(D + 'get#DataGridRows', { source: { type: 'entity', entityName: 'DataDocument.GridSpecTokenDoc',
            conditions: { targetId: 'GRID_SPEC_T' } }, page: 1, size: 5 })
        const user = invoke(D + 'get#DataGridRows', { source: { type: 'entity', entityName: 'DataDocument.GridSpecUserDoc',
            conditions: { userId: 'GRID_PLAIN_USER' } }, page: 1, size: 5 })
        return { columns, rows, sorted, conditioned, exported, text, planted, token, user }
    },
    then({ columns, rows, sorted, conditioned, exported, text, planted, token, user }) {
        columns.columns.map(c => c.name).includes('description')
        columns.columns.find(c => c.name == 'enumLabel').editable == false
        columns.readOnlyReason != null
        rows.data.length == 1
        rows.data[0].enumLabel == 'GS_A / ' + rows.data[0].description
        sorted.error.includes('not a field the grid sorts or filters on')
        conditioned.error.includes('computed on read')
        exported.rowCount == 1
        text.includes('GS_A / ')
        planted.error.includes('not saved in the data grid')
        token.data.length == 1
        token.data[0].tokenShown == 't:null'
        !JSON.stringify(token.data).includes('grid-spec-secret')
        user.data.length == 1
        !JSON.stringify(user.data).includes('16ac58bbfa332c1c55bd98b53e60720bfa90d394')
    },
})
feature('a grid view of SQL is stored only by a user who may run SQL, and is marked so the screen shows it first', {
    when() {
        const sql = { type: 'sql', sql: "SELECT enum_id FROM enumeration WHERE enum_type_id = 'GridSpec'" }
        login('grid.plain.user')
        let planted, plantedNested, plainEntity
        try {
            planted = invoke(D + 'store#GridView', { description: 'Planted', isShared: true, sourceJson: JSON.stringify(sql) })
            plantedNested = invoke(D + 'store#GridView', { description: 'Planted', isShared: true,
                sourceJson: JSON.stringify({ source: sql }) })
            plainEntity = invoke(D + 'store#GridView', { description: 'Plain entity', isShared: true,
                sourceJson: JSON.stringify({ source }) })
        } finally { login('john.doe') }
        const own = invoke(D + 'store#GridView', { description: 'Spec SQL view', sourceJson: JSON.stringify({ source: sql }) })
        const views = invoke(D + 'find#GridViews', {}).gridViews
        for (const viewId of [own.viewId, plainEntity.viewId]) asAdmin(() =>
            ec.entity.find('moqui.database.GridView').condition({ viewId }).deleteAll())
        return { planted, plantedNested, plainEntity, own, views }
    },
    then({ planted, plantedNested, plainEntity, own, views }) {
        planted.error.includes('SQL_RUNNER_WEB')
        planted.viewId == null
        plantedNested.error.includes('SQL_RUNNER_WEB')
        plainEntity.error == ''
        own.error == ''
        views.find(v => v.viewId == own.viewId).sqlSource == true
        views.find(v => v.viewId == plainEntity.viewId).sqlSource == false
        views.find(v => v.viewId == plainEntity.viewId).ownerUserId == 'GRID_PLAIN_USER'
    },
})
