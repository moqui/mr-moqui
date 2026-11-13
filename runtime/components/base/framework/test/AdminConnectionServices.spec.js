// Connections and mail relays, org.moqui.admin.ConfigurationServices (mr-boss plan/impl-admin.md P11; design-admin §8.1,
// Connections, ConnectionDetail, §9 MailRelays, §14 "Connection adapter"): a secret is never returned; a change of an
// endpoint without the secret clears it and disables the connection, nothing is sent while it is missing, and a passing
// test of the new endpoint enables it; a connection that may not use the private network is sent nothing at an internal
// address; an EmailServer a mail account owns is no relay on any path; only a kind's declared fields change. No request
// leaves the machine: a remote's address is answered by the HTTP fixtures, which also show that a test with a missing secret
// sent nothing and that a test sends no credentials; the others are refused before a socket (an internal address, no print
// client) or are of a kind whose test sends nothing (the mail sign-in provider).
// Run: ./moq test framework --spec AdminConnectionServices
const C = "org.moqui.admin.ConfigurationServices."
const REMOTE = "moqui.service.message.SystemMessageRemote"
const SERVER = "moqui.basic.email.EmailServer"
const OAUTH = "moqui.basic.email.EmailOAuthProvider"
const PRINTER = "moqui.basic.print.NetworkPrinter"
const CHECK = "moqui.server.ConnectionCheck"
const ids = {
    remote: "SPEC_CONN_REMOTE",
    inner: "SPEC_CONN_INNER",
    hook: "SPEC_CONN_HOOK",
    made: "SPEC_CONN_MADE",
    legacy: "SPEC_CONN_LEGACY",
    oauth: "EopSpecConn",
    relay: "SPEC_CONN_RELAY",
    owned: "SPEC_CONN_OWNED",
    newRelay: "SPEC_CONN_NEW",
    printer: "SPEC_CONN_PRT"
}
const users = {manage: "SPEC_CONN_MANAGE", view: "SPEC_CONN_VIEW", none: "SPEC_CONN_NONE"}
const groups = {manage: "SPEC_CONN_MANAGERS", view: "SPEC_CONN_VIEWERS"}
const secret = "spec-conn-secret-7q2x"
const shared = "spec-conn-shared-4m9d"

const http = ec.factory.getTool("HttpFixtures")
const find = (entityName, conditions = {}) => ec.entity.find(entityName).condition(conditions).disableAuthz()
const one = (entityName, conditions) => find(entityName, conditions).one()
const make = (entityName, fields) => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
function as(userId, action) {
    ec.user.internalLoginUser(userId.toLowerCase().replace(/_/g, "."))
    ec.artifactExecution.enableAuthz()
    try {
        return action()
    } finally {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
    }
}
function invoke(name, parameters = {}) {
    try {
        const result = ec.service.sync().name(name).parameters(parameters).call() || {}
        const error = ec.message.hasError() ? ec.message.getErrorsString() : ""
        ec.message.clearErrors()
        return Object.assign({}, result, {error})
    } catch (e) {
        ec.message.clearErrors()
        return {error: String(e.message || e)}
    }
}
const get = (adapterKey, connectionKey) => invoke(C + "get#Connection", {adapterKey, connectionKey}).connection
const store = (adapterKey, connectionKey, values, extra = {}) =>
    invoke(
        C + "store#Connection",
        Object.assign({adapterKey, connectionKey, values, expectedRevision: get(adapterKey, connectionKey)?.revision}, extra)
    )
const test = (adapterKey, connectionKey) => invoke(C + "test#Connection", {adapterKey, connectionKey})
const audit = (entityName, fieldName, key) =>
    Array.from(
        find("moqui.entity.EntityAuditLog", {
            changedEntityName: entityName,
            changedFieldName: fieldName,
            pkPrimaryValue: key
        }).list()
    )
const check = (adapterKey, connectionKey) => one(CHECK, {adapterKey, connectionKey})

spec({
    setupSpec() {
        http.enable()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        for (const [key, userId] of Object.entries(users))
            make("moqui.security.UserAccount", {
                userId,
                username: userId.toLowerCase().replace(/_/g, "."),
                userFullName: "Spec connections " + key
            })
        for (const [key, userGroupId] of Object.entries(groups))
            make("moqui.security.UserGroup", {userGroupId, description: "Spec connections " + key})
        const grant = (userGroupId, permissions) =>
            permissions.forEach(userPermissionId =>
                make("moqui.security.UserGroupPermission", {userGroupId, userPermissionId, fromDate: 0})
            )
        grant(groups.manage, ["ADMIN_CONNECTION_VIEW", "ADMIN_CONNECTION_MANAGE"])
        grant(groups.view, ["ADMIN_CONNECTION_VIEW"])
        make("moqui.security.UserGroupMember", {userGroupId: groups.manage, userId: users.manage, fromDate: 0})
        make("moqui.security.UserGroupMember", {userGroupId: groups.view, userId: users.view, fromDate: 0})
        // a remote as an instance administrator left it: live, with its password and its shared secret
        make(REMOTE, {
            systemMessageRemoteId: ids.remote,
            description: "Spec remote",
            sendUrl: "https://one.example.com/api/${systemMessageTypeId}",
            username: "spec-user",
            password: secret,
            sharedSecret: shared,
            sendServiceName: "org.moqui.impl.SystemMessageServices.send#SystemMessageRest"
        })
        // a remote as Tools may have left it: credentials written into its address, which today's validation would refuse
        make(REMOTE, {
            systemMessageRemoteId: ids.legacy,
            description: "Spec legacy remote",
            sendUrl: "https://spec-in:url-pass-9z@Legacy.Example.com/in",
            username: "spec-user",
            password: secret
        })
        // a remote that may not use the private network, at an internal address
        make(REMOTE, {
            systemMessageRemoteId: ids.inner,
            description: "Spec inner remote",
            sendUrl: "https://127.0.0.1:9/hook",
            messageAuthEnumId: "SmatNone",
            allowPrivateNetwork: "N"
        })
        // a remote that verifies what it receives by its shared secret
        make(REMOTE, {
            systemMessageRemoteId: ids.hook,
            description: "Spec hook remote",
            messageAuthEnumId: "SmatHmacSha256",
            authHeaderName: "X-Spec",
            sharedSecret: shared
        })
        make("moqui.basic.Enumeration", {enumId: ids.oauth, enumTypeId: "EmailOAuthProvider", description: "Spec mail provider"})
        make(OAUTH, {
            oauthProviderEnumId: ids.oauth,
            clientId: "spec-client",
            clientSecret: secret,
            authorizeUrl: "https://mail.example.com/authorize",
            tokenUrl: "https://mail.example.com/token"
        })
        make(SERVER, {emailServerId: ids.relay, smtpHost: "127.0.0.1", smtpPort: "9", smtpSsl: "Y", allowPrivateNetwork: "N"})
        make(SERVER, {
            emailServerId: ids.owned,
            smtpHost: "mail.example.com",
            smtpPort: "465",
            smtpSsl: "Y",
            mailUsername: "owner",
            mailPassword: secret
        })
        make("moqui.basic.email.EmailAccount", {
            emailAccountId: ids.owned,
            emailServerId: ids.owned,
            ownsEmailServer: "Y",
            ownerUserId: users.none
        })
        make("moqui.basic.email.EmailTemplate", {
            emailTemplateId: ids.relay,
            description: "Spec relay template",
            emailServerId: ids.relay
        })
        // a kind whose service does not exist is left out, and takes nothing with it
        make("moqui.server.ConnectionAdapter", {
            adapterKey: "specMissing",
            title: "Spec missing",
            adapterGroup: "integration",
            adapterServiceName: "spec.Missing.run#Nothing"
        })
        ec.artifactExecution.enableAuthz()
    },
    cleanupSpec() {
        http.reset()
        ec.message.clearErrors()
        ec.artifactExecution.disableAuthz()
        const keys = Object.values(ids)
        ec.entity.find(CHECK).condition("connectionKey", "in", keys).deleteAll()
        ec.entity.find("moqui.server.ConnectionAdapter").condition("adapterKey", "specMissing").deleteAll()
        ec.entity.find("moqui.basic.email.EmailTemplate").condition("emailTemplateId", ids.relay).deleteAll()
        ec.entity.find("moqui.basic.email.EmailAccount").condition("emailAccountId", ids.owned).deleteAll()
        ec.entity.find(SERVER).condition("emailServerId", "in", keys).deleteAll()
        ec.entity.find("moqui.basic.print.PrintJob").condition("networkPrinterId", ids.printer).deleteAll()
        ec.entity.find(PRINTER).condition("networkPrinterId", ids.printer).deleteAll()
        ec.entity.find(OAUTH).condition("oauthProviderEnumId", ids.oauth).deleteAll()
        ec.entity.find("moqui.basic.Enumeration").condition("enumId", ids.oauth).deleteAll()
        ec.entity.find(REMOTE).condition("systemMessageRemoteId", "in", keys).deleteAll()
        ec.entity.find("moqui.security.UserGroupPermission").condition("userGroupId", "in", Object.values(groups)).deleteAll()
        ec.entity.find("moqui.security.UserGroupMember").condition("userId", "in", Object.values(users)).deleteAll()
        ec.entity.find("moqui.security.UserGroup").condition("userGroupId", "in", Object.values(groups)).deleteAll()
        ec.entity.find("moqui.security.UserAccount").condition("userId", "in", Object.values(users)).deleteAll()
        ec.artifactExecution.enableAuthz()
    }
})

feature(
    "an endpoint change without the secret clears it and disables the connection; nothing is sent until it is entered again, and a passing test enables it",
    {
        when() {
            const out = {}
            as(users.manage, () => {
                out.read = get("remote", ids.remote)
                out.listed = invoke(C + "find#Connections", {adapterGroup: "integration", text: "Spec remote"})
                // a name the kind does not declare is not changed, whatever is given; a secret alone leaves it enabled
                out.ignored = store("remote", ids.remote, {
                    sendServiceName: "spec.Evil#Service",
                    allowPrivateNetwork: "Y",
                    disabled: "Y"
                })
                // a form posts every setting back: what is given as it was read is no change, so no endpoint change either;
                // credentials written into an address are not read
                out.legacy = get("remote", ids.legacy)
                out.legacySave = store(
                    "remote",
                    ids.legacy,
                    Object.assign(Object.fromEntries(out.legacy.fieldList.map(row => [row.name, row.value])), {
                        description: "Spec legacy renamed"
                    })
                )
                out.expression = store("remote", ids.remote, {sendUrl: "https://one.example.com/${ec.user.userId}"})
                out.plain = store("remote", ids.remote, {sendUrl: "http://one.example.com/api"})
                out.stale = invoke(C + "store#Connection", {
                    adapterKey: "remote",
                    connectionKey: ids.remote,
                    values: {description: "Stale"},
                    expectedRevision: "0"
                })
                out.moved = store(
                    "remote",
                    ids.remote,
                    {sendUrl: "https://two.example.com/api"},
                    {reason: "Spec: the provider moved"}
                )
            })
            out.movedRow = one(REMOTE, {systemMessageRemoteId: ids.remote}).getMap()
            out.legacyRow = one(REMOTE, {systemMessageRemoteId: ids.legacy}).getMap()
            out.movedCheck = check("remote", ids.remote).getMap()
            as(users.manage, () => {
                out.afterMove = get("remote", ids.remote)
                out.unsent = test("remote", ids.remote)
                out.unsentRequests = http.count()
                out.enable = store("remote", ids.remote, {}, {enabled: true})
                out.entered = store("remote", ids.remote, {password: secret})
                // (the fixtures answer before the transport's own guard, so this one runs without them: refused before a socket)
                http.disable()
                out.inner = test("remote", ids.inner)
                http.enable()
                // a new remote is saved disabled, waiting for its test
                out.made = invoke(C + "store#Connection", {
                    adapterKey: "remote",
                    values: {
                        connectionId: ids.made,
                        description: "Spec made remote",
                        sendUrl: "https://three.example.com/in",
                        messageAuthEnumId: "SmatNone",
                        sendAuthEnumId: "SmatNone"
                    }
                })
                // the secret received messages are verified by, and how they are verified, are not set here
                out.hookSecret = store("remote", ids.hook, {sharedSecret: "spec-conn-forged-1x"})
                out.hookAuth = store("remote", ids.hook, {messageAuthEnumId: "SmatNone", description: "Spec hook renamed"})
                out.backup = store("remote", "EmbeddedDbBackup", {
                    sendUrl: "https://elsewhere.example.net/",
                    username: "spec",
                    password: secret
                })
            })
            out.enteredRow = one(REMOTE, {systemMessageRemoteId: ids.remote}).getMap()
            out.enteredCheck = check("remote", ids.remote).getMap()
            // the new address answers: one request with no credentials and no body, and the test enables the remote
            http.add({method: "HEAD", url: "https://two.example.com/api", status: 405})
            out.passed = as(users.manage, () => test("remote", ids.remote))
            out.sent = http.requests().map(request => ({
                method: request.method,
                url: request.url,
                headers: Object.keys(request.headers || {})
                    .join(",")
                    .toLowerCase(),
                body: String(request.body || "")
            }))
            out.passedRow = one(REMOTE, {systemMessageRemoteId: ids.remote}).getMap()
            out.madeRow = one(REMOTE, {systemMessageRemoteId: ids.made})?.getMap()
            out.hookRow = one(REMOTE, {systemMessageRemoteId: ids.hook}).getMap()
            out.view = as(users.view, () => ({
                read: get("remote", ids.remote),
                store: invoke(C + "store#Connection", {
                    adapterKey: "remote",
                    connectionKey: ids.remote,
                    values: {description: "No"}
                }),
                test: test("remote", ids.remote)
            }))
            out.none = as(users.none, () => invoke(C + "find#Connections", {}))
            context.out = out
        },
        then() {
            const out = context.out
            // a read gives whether a secret is set, never its value
            out.read.secretList.find(row => row.name === "password").secretSet === true
            !JSON.stringify([out.read, out.listed, out.afterMove]).includes(secret)
            !JSON.stringify([out.read, out.afterMove]).includes(shared)
            out.read.state === "untested"
            JSON.stringify(out.listed.connectionList.map(row => row.connectionKey)) === JSON.stringify([ids.remote])
            out.listed.connectionList[0].endpointSummary === "one.example.com"
            !out.listed.adapterList.map(row => row.adapterKey).includes("specMissing")
            !JSON.stringify(out.legacy).includes("url-pass-9z")
            out.legacy.fieldList.find(row => row.name === "sendUrl").value === "https://(credentials)@Legacy.Example.com/in"
            out.legacySave.applied === true && out.legacySave.secretCleared === false && out.legacySave.enabled === true
            out.legacyRow.description === "Spec legacy renamed" && out.legacyRow.password === secret
            out.legacyRow.sendUrl === "https://spec-in:url-pass-9z@Legacy.Example.com/in" && out.legacyRow.disabled == null
            // only declared fields change; a ${} of its own, a plain http address and a stale revision are refused
            out.ignored.applied === true
            out.expression.error.includes("${")
            out.plain.error.includes("https")
            out.stale.reasonCode === "STALE_REVISION"
            // the endpoint moved: every stored secret cleared, disabled, its test mark gone, the audit with no value
            out.moved.applied === true
            out.moved.secretCleared === true
            out.moved.enabled === false
            out.movedRow.sendUrl === "https://two.example.com/api"
            out.movedRow.password == null
            out.movedRow.sharedSecret == null
            out.movedRow.disabled === "Y"
            out.movedRow.sendServiceName === "org.moqui.impl.SystemMessageServices.send#SystemMessageRest"
            out.movedRow.allowPrivateNetwork == null
            out.movedCheck.pendingTest === "Y"
            out.movedCheck.secretCleared === "Y"
            audit(REMOTE, "password", ids.remote)
                .map(row => row.newValueText)
                .includes("(cleared: the endpoint changed)")
            !JSON.stringify(audit(REMOTE, "password", ids.remote).map(row => row.getMap())).includes(secret)
            audit(REMOTE, "sendUrl", ids.remote).some(row => row.changeReason === "Spec: the provider moved") === true
            // the disabling too, a first value the entity's own audit log does not write
            audit(REMOTE, "disabled", ids.remote).some(
                row => row.newValueText === "Y" && row.changeReason === "Spec: the provider moved"
            ) === true
            out.afterMove.state === "incomplete"
            // a test while the secret is missing sends nothing, and enabling is refused
            out.unsent.passed === false
            out.unsent.checkList.some(row => row.check === "Address") === false
            out.unsentRequests === 0
            out.enable.applied === false
            out.enable.reasonCode === "INCOMPLETE" || out.enable.reasonCode === "TEST_REQUIRED"
            // the secret entered again: kept, still disabled until a test passes
            out.entered.applied === true
            out.entered.testRequired === true
            out.enteredRow.password === secret
            out.enteredRow.disabled === "Y"
            out.enteredCheck.secretCleared === "N"
            // nothing goes to an internal address for a connection that may not use the private network
            out.inner.passed === false
            out.inner.checkList.find(row => row.check === "Address").text.includes("internal address")
            // a passing test of the new address: one HEAD with no credentials and no body, and the remote is enabled again
            out.passed.passed === true && out.passed.enabledByTest === true && out.passed.enabled === true
            out.sent.length === 1 && out.sent[0].method === "HEAD" && out.sent[0].url === "https://two.example.com/api"
            !out.sent[0].headers.includes("authorization") && out.sent[0].body === ""
            !JSON.stringify(out.passed).includes(secret)
            out.passedRow.password === secret && out.passedRow.disabled === "N"
            out.made.applied === true
            out.madeRow.disabled === "Y"
            out.madeRow.sendServiceName == null
            // how it receives is not the administrator's to choose: a new remote receives from an authorized user only
            out.madeRow.messageAuthEnumId === "SmatLogin" && out.madeRow.sendAuthEnumId === "SmatNone"
            out.hookSecret.reasonCode === "SECRET_NOT_DELEGATED"
            out.hookAuth.applied === true && out.hookRow.description === "Spec hook renamed"
            out.hookRow.messageAuthEnumId === "SmatHmacSha256" && out.hookRow.sharedSecret === shared
            // a remote of a transport of its own (the backup mirror's) is not changed here
            out.backup.reasonCode === "READ_ONLY"
            // a reader reads and changes nothing; without the permission nothing is read
            out.view.read.connectionKey === ids.remote
            out.view.store.error.includes("ADMIN_CONNECTION_MANAGE")
            out.view.test.error.includes("ADMIN_CONNECTION_MANAGE")
            out.none.error.includes("ADMIN_CONNECTION_VIEW")
        }
    }
)

feature(
    "a kind whose test sends nothing is enabled by it the same way, and a connection an administrator disabled stays disabled",
    {
        when() {
            const out = {}
            as(users.manage, () => {
                out.moved = store("mailOAuth", ids.oauth, {clientId: "spec-client-two"})
                out.unsent = test("mailOAuth", ids.oauth)
                out.entered = store("mailOAuth", ids.oauth, {clientSecret: secret + "-two"})
                out.passed = test("mailOAuth", ids.oauth)
                out.ready = get("mailOAuth", ids.oauth)
                out.off = store("mailOAuth", ids.oauth, {}, {enabled: false})
                out.offTest = test("mailOAuth", ids.oauth)
                // repointed while an administrator has it disabled: the test of the new client does not enable it
                out.offMoved = store("mailOAuth", ids.oauth, {clientId: "spec-client-three", clientSecret: secret + "-two"})
                out.offMovedTest = test("mailOAuth", ids.oauth)
                out.on = store("mailOAuth", ids.oauth, {}, {enabled: true})
            })
            out.row = one(OAUTH, {oauthProviderEnumId: ids.oauth}).getMap()
            context.out = out
        },
        then() {
            const out = context.out
            out.moved.secretCleared === true
            out.moved.enabled === false
            out.unsent.passed === false
            out.entered.testRequired === true
            out.passed.passed === true
            out.passed.enabledByTest === true
            out.ready.state === "ready"
            out.ready.tested === true
            out.ready.checkList.every(row => row.state === "ok") === true
            !JSON.stringify([out.passed, out.ready]).includes(secret)
            // disabled by an administrator: a passing test leaves it so, and enabling it takes that test
            out.off.enabled === false
            out.offTest.passed === true
            out.offTest.enabled === false
            out.offMoved.applied === true && out.offMoved.testRequired === false
            out.offMovedTest.passed === true && out.offMovedTest.enabledByTest === false && out.offMovedTest.enabled === false
            out.on.enabled === true
            out.row.disabled === "N"
            out.row.clientSecret === secret + "-two"
        }
    }
)

feature("an EmailServer a mail account owns is no relay on any path, and a relay or a printer is tested within its bounds", {
    when() {
        const out = {}
        as(users.manage, () => {
            out.relays = invoke(C + "find#Connections", {adapterGroup: "mail"})
            out.ownedRead = get("relay", ids.owned)
            out.ownedStore = invoke(C + "store#Connection", {
                adapterKey: "relay",
                connectionKey: ids.owned,
                values: {smtpHost: "evil.example.com"}
            })
            out.ownedTest = test("relay", ids.owned)
            out.ownedUse = invoke(C + "find#ConnectionConsumers", {adapterKey: "relay", connectionKey: ids.owned})
            // a relay that signs in needs TLS; one that may not use the private network connects to no internal address
            out.insecure = store("relay", ids.relay, {smtpSsl: "N", mailUsername: "relay", mailPassword: secret})
            out.inner = test("relay", ids.relay)
            out.consumers = invoke(C + "find#ConnectionConsumers", {adapterKey: "relay", connectionKey: ids.relay})
            out.newRelay = invoke(C + "store#Connection", {
                adapterKey: "relay",
                values: {
                    connectionId: ids.newRelay,
                    smtpHost: "smtp.example.com",
                    smtpPort: "587",
                    smtpStartTls: "Y",
                    mailUsername: "relay",
                    mailPassword: secret
                }
            })
            out.newRelayRead = get("relay", ids.newRelay)
            out.printer = invoke(C + "store#Connection", {
                adapterKey: "printer",
                values: {
                    connectionId: ids.printer,
                    description: "Spec printer",
                    serverHost: "print.example.com",
                    printerName: "gate"
                }
            })
            out.printerTest = test("printer", ids.printer)
            out.oauthNew = invoke(C + "store#Connection", {adapterKey: "mailOAuth", values: {clientId: "x"}})
        })
        out.ownedRow = one(SERVER, {emailServerId: ids.owned}).getMap()
        out.relayRow = one(SERVER, {emailServerId: ids.relay}).getMap()
        out.printerRow = one(PRINTER, {networkPrinterId: ids.printer}).getMap()
        context.out = out
    },
    then() {
        const out = context.out
        const listed = out.relays.connectionList.map(row => row.connectionKey)
        listed.includes("SYSTEM")
        listed.includes(ids.relay)
        !listed.includes(ids.owned)
        out.relays.connectionList.every(row => row.adapterKey === "relay") === true
        out.ownedRead == null
        out.ownedStore.reasonCode === "ACCOUNT_OWNED"
        out.ownedTest.reasonCode === "ACCOUNT_OWNED"
        out.ownedUse.consumerList.length === 0
        out.ownedRow.smtpHost === "mail.example.com"
        out.ownedRow.mailPassword === secret
        out.insecure.reasonCode === "INSECURE_TRANSPORT"
        out.relayRow.mailPassword == null
        out.inner.passed === false
        out.inner.checkList.find(row => row.check === "Connection").text.includes("internal address")
        out.consumers.consumerList.map(row => row.recordId).includes(ids.relay)
        // a new relay and a new printer are saved disabled; a secret is in no answer
        out.newRelay.applied === true
        out.newRelayRead.state === "needsTest"
        out.newRelayRead.secretList.find(row => row.name === "mailPassword").secretSet === true
        !JSON.stringify([out.newRelay, out.newRelayRead, out.relays]).includes(secret)
        out.printer.applied === true
        out.printerRow.disabled === "Y"
        // a test page that is not taken leaves the printer disabled, with a reason of the fixed kinds
        out.printerTest.passed === false
        out.printerTest.enabled === false
        out.oauthNew.reasonCode === "NOT_CREATABLE"
    }
})
