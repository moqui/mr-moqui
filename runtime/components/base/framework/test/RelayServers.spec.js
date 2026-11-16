// The Service REST email/servers resource (tools moqui.rest.xml; org.moqui.mail.EmailServices find#RelayServers and
// get#RelayServer, SEC-MAIL-04): a mail server that a mail client account owns (ownsEmailServer Y) is neither listed nor
// answered, and no answer carries an encrypted field, only whether it is set.
// Run: ./moq test framework --spec RelayServers
const RELAY = "SPEC_RELAY_SHARED"
const OWNED = "SPEC_RELAY_OWNED"
const ACCOUNT = "SPEC_RELAY_ACCOUNT"
const S = "org.moqui.mail.EmailServices."

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
function call(name, parameters) {
    const out = sys(() => ec.service.sync().name(S + name).parameters(parameters || {}).call()) || {}
    const errors = ec.message.hasError() ? ec.message.errorsString : ""
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}

spec({
    setupSpec() {
        sys(() => {
            for (const emailServerId of [RELAY, OWNED]) {
                if (ec.entity.find("moqui.basic.email.EmailServer").condition("emailServerId", emailServerId).one()) continue
                const server = ec.entity.makeValue("moqui.basic.email.EmailServer")
                server.setFields({emailServerId, smtpHost: "smtp.example.com", mailUsername: "spec", mailPassword: "spec-mailbox-secret"}, true, null, null)
                server.create()
            }
            if (!ec.entity.find("moqui.basic.email.EmailAccount").condition("emailAccountId", ACCOUNT).one()) {
                const account = ec.entity.makeValue("moqui.basic.email.EmailAccount")
                account.setFields({emailAccountId: ACCOUNT, emailServerId: OWNED, ownsEmailServer: "Y", accountKindEnumId: "EakExternal",
                    statusId: "EasActive"}, true, null, null)
                account.create()
            }
        })
    },
})

feature("an account's own server is neither listed nor answered; a relay is, with no secret", {
    when() {
        const listed = call("find#RelayServers")
        return {listed, relay: call("get#RelayServer", {emailServerId: RELAY}), owned: call("get#RelayServer", {emailServerId: OWNED})}
    },
    then({listed, relay, owned}) {
        const ids = Array.from(listed.emailServerList).map((row) => row.emailServerId)
        ids.includes(RELAY)
        !ids.includes(OWNED)
        !JSON.stringify(listed.emailServerList).includes("spec-mailbox-secret")
        relay.emailServer.smtpHost == "smtp.example.com"
        relay.emailServer.mailPasswordIsSet == true
        relay.emailServer.mailPassword === undefined
        owned.errors.includes("not found")
        owned.emailServer == null
    },
})
