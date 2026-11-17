// The Service REST email/servers resource (tools moqui.rest.xml; org.moqui.mail.EmailServices find#RelayServers and
// get#RelayServer, SEC-MAIL-04): a mail server that a mail client account owns (ownsEmailServer Y) is neither listed nor
// answered, and no answer carries an encrypted field, only whether it is set.
// PATCH email/servers/{id} (update#RelayServer) and its poll (poll#EmailServer) refuse such a server the same way.
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

// update#RelayServer (PATCH email/servers/{id}) and poll#EmailServer (POST email/servers/{id}/poll): an account's own
// server is answered as one that does not exist and left as it was; a relay is changed, with nothing secret answered
feature("an account's own server is neither changed nor polled by id; a relay is changed", {
    when() {
        ec.user.internalLoginUser("john.doe")
        const isAdmin = ec.user.isInGroup("ADMIN")
        const before = sys(() => ec.entity.find("moqui.basic.email.EmailServer").condition("emailServerId", OWNED).useCache(false).one().storeHost)
        const ownedUpdate = call("update#RelayServer", {emailServerId: OWNED, storeHost: "127.0.0.1", storePort: "9143"})
        const ownedPoll = call("poll#EmailServer", {emailServerId: OWNED})
        const relayUpdate = call("update#RelayServer", {emailServerId: RELAY, storeHost: "imap.example.net"})
        const read = (id) => sys(() => ec.entity.find("moqui.basic.email.EmailServer").condition("emailServerId", id).useCache(false).one())
        return {isAdmin, before, ownedUpdate, ownedPoll, relayUpdate, owned: read(OWNED), relay: read(RELAY)}
    },
    then({isAdmin, before, ownedUpdate, ownedPoll, relayUpdate, owned, relay}) {
        isAdmin
        ownedUpdate.errors.includes("not found")
        owned.storeHost == before
        owned.storePort == null
        /No EmailServer found/.test(ownedPoll.errors)
        relayUpdate.errors == ""
        relay.storeHost == "imap.example.net"
        relay.smtpHost == "smtp.example.com"
        !JSON.stringify(relayUpdate).includes("spec-mailbox-secret")
    },
})

// PATCH email/servers/{id} keeps Admin's endpoint-change rule: a new host clears the stored password not given again,
// saves the relay disabled and waiting for a test; a change that is not of the endpoint leaves it as it was
feature("a relay repointed by PATCH is disabled until a test, and its password is not kept for the new host", {
    when() {
        ec.user.internalLoginUser("john.doe")
        const read = () => sys(() => ec.entity.find("moqui.basic.email.EmailServer").condition("emailServerId", RELAY).useCache(false).one())
        const check = () => sys(() => ec.entity.find("moqui.server.ConnectionCheck").condition({adapterKey: "relay", connectionKey: RELAY})
            .useCache(false).one())
        sys(() => {
            const row = read()
            row.setFields({disabled: "N", mailPassword: "spec-mailbox-secret", smtpHost: "smtp.example.com"}, true, null, null)
            row.update()
        })
        const plain = call("update#RelayServer", {emailServerId: RELAY, allowedToDomains: "example.com"})
        const afterPlain = read()
        const asksEnabled = call("update#RelayServer", {emailServerId: RELAY, smtpHost: "smtp.example.org", disabled: "N"})
        const afterAsk = read()
        const repointed = call("update#RelayServer", {emailServerId: RELAY, smtpHost: "smtp.example.org"})
        return {plain, afterPlain, asksEnabled, afterAsk, repointed, after: read(), check: check()}
    },
    then({plain, afterPlain, asksEnabled, afterAsk, repointed, after, check}) {
        plain.errors == "" && afterPlain.disabled != "Y" && afterPlain.mailPassword != null
        asksEnabled.errors.includes("test it as it is now") && afterAsk.smtpHost == "smtp.example.com"
        repointed.errors == ""
        after.smtpHost == "smtp.example.org"
        after.disabled == "Y"
        after.mailPassword == null
        check.pendingTest == "Y" && check.secretCleared == "Y" && check.testedConfigHash == null
    },
})

