// The table check of Tools › Schema (SEC-TLS-99): check#Tables runs DDL, so it needs DATABASE_ADMIN, as the planned
// jobs of the same screens do, and writes a DbaTableCheck audit row.
const CHECK = 'org.moqui.dev.DatabaseServices.check#Tables'
// A Tools screen's transition calls the service with the screen's inherited TOOLS_APP authorization; outside a screen
// the spec stands in for that with authorization disabled.
function asAdmin(fn) { ec.artifactExecution.disableAuthz(); try { return fn() } finally { ec.artifactExecution.enableAuthz() } }
const audits = userId => asAdmin(() => Array.from(ec.entity.find('moqui.database.DatabaseAudit')
    .condition({ actionEnumId: 'DbaTableCheck', userId }).list()))
spec({ setupSpec() { login('john.doe') } })
feature('a DATABASE_ADMIN holder checks a table and the check is audited', {
    when() {
        const before = audits('EX_JOHN_DOE').length
        const out = asAdmin(() => call(CHECK, { entityNames: ['moqui.basic.Enumeration'] })) || {}
        return { out, errors: ec.message.getErrorsString(), added: audits('EX_JOHN_DOE').length - before }
    },
    then({ out, errors, added }) {
        errors == ''
        Array.isArray(out.ddl)
        out.errors == 0
        added == 1
    },
})
feature('a user without DATABASE_ADMIN is refused the table check, even reached as a screen would reach it', {
    when() {
        asAdmin(() => call('create#moqui.security.UserAccount', { userId: 'TC_PLAIN_USER', username: 'tc.plain.user',
            userFullName: 'Table check plain user', currentPassword: '16ac58bbfa332c1c55bd98b53e60720bfa90d394',
            passwordHashType: 'SHA' }))
        login('tc.plain.user')
        try {
            let error = ''
            try { asAdmin(() => call(CHECK, { entityNames: ['moqui.basic.Enumeration'] })) } catch (e) { error = String(e.message || e) }
            error += ec.message.getErrorsString()
            return { error, added: audits('TC_PLAIN_USER').length }
        } finally { ec.message.clearErrors(); login('john.doe') }
    },
    then({ error, added }) {
        error.includes('DATABASE_ADMIN')
        added == 0
    },
})
