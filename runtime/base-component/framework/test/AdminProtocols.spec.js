// Protocol readiness and diagnostics, org.moqui.admin.ConfigurationServices (mr-boss plan/impl-admin.md P12; design-admin §9
// ProtocolsHome and ServerConfiguration, §15.3 "Protocol readiness"): a protocol this build leaves out, one the conf keeps
// off, one that did not start and one that runs read apart; SMTP reception is trusted only while its allowlist and its
// authserv-id hold, and a mismatch is never ready; a check that leaves the server runs only when confirmed, and a protocol
// that is off is sent nothing. The states are read from given facts (the server's own are a command-line run's here, which
// opens no listeners); nothing is connected to.
// Run: ./moq test framework --spec AdminProtocols
const C = "org.moqui.admin.ConfigurationServices."
const CHECK = "moqui.server.ConnectionCheck"
const users = {manage: "SPEC_PROTO_MANAGE", view: "SPEC_PROTO_VIEW", none: "SPEC_PROTO_NONE"}
const groups = {manage: "SPEC_PROTO_MANAGERS", view: "SPEC_PROTO_VIEWERS"}
const protocols = ec.resource.script("component://framework/service/org/moqui/admin/ConfigurationProtocols.js")

const make = (entityName, fields) => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
// As a user, with authorization on; inner: as a service that was authorized calls the server's own services (which no
// group may call directly), so only their own permission check answers.
function as(userId, action, inner = false) {
    ec.user.internalLoginUser(userId.toLowerCase().replace(/_/g, "."))
    if (!inner) ec.artifactExecution.enableAuthz()
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
// The facts of a server: its features, what its listeners did at startup and its conf.
const facts = ({features = [], entries = {}, properties = {}, serverStarted = true}) => ({
    features: {has: name => features.includes(name)},
    entries: new Map(Object.entries(entries)),
    serverStarted,
    packageName: "spec",
    prop: name => String(properties[name] ?? ""),
    conf: (attribute, propertyName) => String(properties[propertyName] ?? "")
})
const gateway = {
    smtp_listen_port: "2525",
    smtp_listen_host: "10.8.0.2",
    smtp_allowed_addresses: "10.8.0.1",
    smtp_authserv_id: "mx.example.com"
}
const listening = properties =>
    protocols.readinessOf(
        "smtpIn",
        facts({features: ["inbound-smtp"], entries: {SMTP: {state: "on", detail: "tcp 10.8.0.2:2525"}}, properties})
    )

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        for (const [key, userId] of Object.entries(users))
            make("moqui.security.UserAccount", {
                userId,
                username: userId.toLowerCase().replace(/_/g, "."),
                userFullName: "Spec protocols " + key
            })
        for (const [key, userGroupId] of Object.entries(groups))
            make("moqui.security.UserGroup", {userGroupId, description: "Spec protocols " + key})
        make("moqui.security.UserGroupPermission", {
            userGroupId: groups.manage,
            userPermissionId: "ADMIN_CONNECTION_MANAGE",
            fromDate: 0
        })
        make("moqui.security.UserGroupPermission", {
            userGroupId: groups.view,
            userPermissionId: "ADMIN_CONNECTION_VIEW",
            fromDate: 0
        })
        make("moqui.security.UserGroupMember", {userGroupId: groups.manage, userId: users.manage, fromDate: 0})
        make("moqui.security.UserGroupMember", {userGroupId: groups.view, userId: users.view, fromDate: 0})
        ec.artifactExecution.enableAuthz()
    },
    cleanupSpec() {
        ec.message.clearErrors()
        ec.artifactExecution.disableAuthz()
    }
})

feature("a protocol that is not built, one kept off, one that did not start and one that runs read apart", {
    when() {
        const built = ["inbound-smtp", "ssh", "xmpp", "axum"]
        const server = facts({
            features: built,
            entries: {
                HTTP: {state: "on", detail: "tcp 0.0.0.0:8080"},
                SMTP: {state: "off", detail: "smtp_listen_port is not set"},
                SSH: {state: "failed", detail: "could not bind 127.0.0.1:22"},
                XMPP: {state: "skipped", detail: "port 5222 is in use"}
            }
        })
        const of = key => protocols.readinessOf(key, server)
        const read = as(users.view, () => invoke(C + "get#ProtocolReadiness"))
        context.out = {
            smtp: of("smtpIn"),
            rtc: of("rtc"),
            ssh: of("ssh"),
            xmpp: of("xmpp"),
            dav: of("dav"),
            wopi: of("wopi"),
            // turned on, in a process that opened no listeners: not known to run, never ready
            quiet: protocols.readinessOf("ssh", facts({features: built, properties: {ssh_port: "2222"}, serverStarted: false})),
            read,
            one: as(users.view, () => invoke(C + "get#ProtocolReadiness", {protocolKey: "smtpIn"})),
            none: as(users.none, () => invoke(C + "get#ProtocolReadiness")),
            facts: as(users.view, () => invoke(C + "get#ProtocolFacts"), true),
            factsNone: as(users.none, () => invoke(C + "get#ProtocolFacts"), true),
            factsDirect: as(users.view, () => invoke(C + "get#ProtocolFacts")),
            // the protocols' settings alone for the integration administrator, each read only
            settings: as(users.view, () => invoke(C + "get#EffectiveSettings"))
        }
    },
    then() {
        const out = context.out
        // off and absent are different answers, each with its reason
        out.smtp.state === "disabled"
        out.smtp.built === true
        out.smtp.enabled === false
        out.smtp.reason === "smtp_listen_port is not set"
        out.rtc.state === "absent"
        out.rtc.built === false
        out.rtc.checkList.length === 0
        out.wopi.state === "absent"
        out.ssh.state === "failing"
        out.ssh.reason.includes("could not bind")
        out.xmpp.state === "failing"
        out.dav.state === "ready"
        out.dav.listener === "tcp 0.0.0.0:8080"
        out.quiet.state === "notStarted"
        new Set([out.smtp.stateText, out.rtc.stateText, out.ssh.stateText, out.dav.stateText, out.quiet.stateText]).size === 5
        // the service: every protocol with one of the states, nothing ready in a run that opened no listener
        out.read.error === ""
        out.read.protocolList.length >= 12
        out.read.protocolList.every(row =>
            ["absent", "disabled", "failing", "untrusted", "notStarted", "ready"].includes(row.state)
        )
        out.read.protocolList.find(row => row.protocolKey === "wopi").state === "absent"
        out.read.protocolList.find(row => row.protocolKey === "speechLocal").state === "absent"
        out.read.protocolList.filter(row => row.buildFeature && row.protocolKey !== "speechLocal").every(row => row.built) ===
            ["smtp", "inbound-smtp", "imap", "axum", "ssh", "git", "xmpp", "rtc", "sip"].every(name =>
                ec.factory.hasFeature(name)
            )
        out.read.canTest === false
        out.one.protocolList.length === 1
        out.none.error.includes("ADMIN_CONNECTION_VIEW")
        // the server's own facts: a fixed list, no key among it, refused without a permission
        Object.keys(out.facts.confMap).includes("smtp-server/@allowed-addresses")
        Object.keys(out.facts.confMap).every(key => !/key|secret|pass|token/.test(key))
        out.factsNone.error.includes("is required")
        out.factsDirect.error !== ""
        out.factsDirect.confMap == null
        out.settings.settingList.length > 0
        out.settings.settingList.every(row => row.settingGroup.startsWith("Protocols: ") && row.editable === false)
        out.settings.settingList.some(row => row.propertyName === "smtp_allowed_addresses")
    }
})

feature("SMTP reception is trusted only while its allowlist and authserv-id hold; a mismatch is never ready", {
    when() {
        const trust = configured =>
            protocols.smtpTrust(
                Object.assign({listening: true, listenHost: "10.8.0.2", authservId: "mx.example.com"}, configured)
            )
        context.out = {
            trusted: listening(gateway),
            noAuthserv: listening(Object.assign({}, gateway, {smtp_authserv_id: ""})),
            everyone: listening(Object.assign({}, gateway, {smtp_allowed_addresses: "10.8.0.1, 0.0.0.0/0"})),
            // bound to this machine, and no allowed address is on it
            unreachable: listening(Object.assign({}, gateway, {smtp_listen_host: "127.0.0.1"})),
            local: listening(Object.assign({}, gateway, {smtp_listen_host: "127.0.0.1", smtp_allowed_addresses: "127.0.0.0/8"})),
            badEntry: trust({allowedAddresses: "10.8.0.1,gateway"}),
            empty: trust({allowedAddresses: ""}),
            v6: trust({allowedAddresses: "fd00::/8", listenHost: "fd00::2"}),
            v6Every: trust({allowedAddresses: "::/0"}),
            internet: listening(Object.assign({}, gateway, {smtp_internet: "true", smtp_allowed_addresses: ""})),
            stopped: trust({allowedAddresses: "10.8.0.1", listening: false})
        }
    },
    then() {
        const out = context.out
        out.trusted.state === "ready"
        out.trusted.trusted === true
        out.trusted.trustState === "trusted"
        out.noAuthserv.state === "untrusted"
        out.noAuthserv.trusted === false
        out.noAuthserv.reason.includes("smtp_authserv_id")
        out.everyone.state === "untrusted"
        out.everyone.trusted === false
        out.unreachable.state === "untrusted"
        out.unreachable.trustProblems.length === 1
        out.local.state === "ready"
        out.local.trusted === true
        out.badEntry.trusted === false
        out.badEntry.state === "mismatch"
        out.empty.trusted === false
        out.v6.trusted === true
        out.v6Every.trusted === false
        // internet mail trusts no gateway: it runs, and is never called trusted reception
        out.internet.state === "ready"
        out.internet.trusted === false
        out.internet.trustState === "direct"
        out.stopped.trusted === false
    }
})

feature("a check that leaves the server runs only when confirmed, and a protocol that is off is sent nothing", {
    when() {
        const test = (protocolKey, checkKey, extra = {}) =>
            invoke(C + "test#Protocol", Object.assign({protocolKey, checkKey}, extra))
        const out = as(users.manage, () => ({
            unconfirmed: test("dav", "publicAddress"),
            off: test("smtpIn", "listener"),
            configuration: test("smtpIn", "configuration"),
            noCheck: test("mailClient", "listener"),
            absent: test("wopi", "configuration"),
            after: invoke(C + "get#ProtocolReadiness", {protocolKey: "smtpIn"})
        }))
        out.viewer = as(users.view, () => test("smtpIn", "configuration"))
        const probe = (userId, parameters) => as(userId, () => invoke(C + "probe#ProtocolTarget", parameters), true)
        out.probeUnconfirmed = probe(users.manage, {targetKey: "web.public"})
        out.probeUnknown = probe(users.manage, {targetKey: "example.com:25", confirmed: true})
        out.probeOff = probe(users.manage, {targetKey: "smtp.listener"})
        out.viewerProbe = probe(users.view, {targetKey: "smtp.listener"})
        out.kept = Array.from(ec.entity.find(CHECK).condition("adapterKey", "protocol").disableAuthz().list()).map(row =>
            row.getMap()
        )
        out.smtpOn = !!System.getProperty("smtp_listen_port")
        context.out = out
    },
    then() {
        const out = context.out
        out.unconfirmed.applied === false
        out.unconfirmed.reasonCode === "CONFIRM_REQUIRED"
        out.probeUnconfirmed.error.includes("only when it is confirmed")
        out.probeUnknown.error.includes("is not a protocol probe target")
        // the test conf sets no SMTP port: nothing to connect to, and nothing was tried
        out.smtpOn === false
        out.off.reasonCode === "NOT_ENABLED"
        out.probeOff.reached === false
        out.probeOff.failureKind === "notConfigured"
        out.configuration.applied === true
        out.configuration.passed === false
        out.configuration.resultText.startsWith("Off")
        out.noCheck.error.includes("has no check")
        out.absent.error.includes("has no check")
        // only a check that ran is kept, with who ran it
        out.kept.length === 1
        out.kept[0].connectionKey === "smtpIn.configuration"
        out.kept[0].lastCheckPassed === "N"
        out.kept[0].lastCheckByUserId === users.manage
        out.after.checkList.find(row => row.checkKey === "configuration").lastCheckPassed === false
        out.after.canTest === true
        out.viewer.error.includes("ADMIN_CONNECTION_MANAGE")
        out.viewerProbe.error.includes("ADMIN_CONNECTION_MANAGE")
    }
})
