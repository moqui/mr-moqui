// The protocols part of org.moqui.admin.ConfigurationServices (mr-boss plan/design-admin.md §9 ProtocolsHome and
// ServerConfiguration, §15.3 "Protocol readiness"; impl-admin P12): what each protocol of the server is doing, told apart, and
// the diagnostics that may be run on it.
//
// A protocol is a row of the table below, nothing else: what it is, the build feature it needs, the startup report's entry
// that says what its listener or integration did, and how the conf turns it on. Its state is one of:
// - absent: this build leaves it out (or no build has it yet); never shown as something that could work;
// - disabled: built, and the conf does not turn it on;
// - failing: turned on, and it did not start (the reason the server recorded), or its last check failed;
// - untrusted: running, and what it trusts does not hold (SMTP reception, below);
// - notStarted: turned on, in a process that opened no listeners (a command-line run), so nothing is known of it;
// - ready: running.
// The facts come from the server (get#ProtocolFacts: the build's features, the startup report's entries, a fixed list of
// nonsecret conf attributes) and from the conf's properties; nothing here is entered by an administrator, and the conf
// stays the deployment's (the settings a protocol runs with are shown read only by get#EffectiveSettings, each registered
// in a Protocols group).
//
// SMTP reception takes mail from the edge gateway alone and trusts the gateway's verdict, so it is trusted only while all
// of this holds: it listens; smtp_allowed_addresses names addresses, none of them a range of every address; an allowed
// address can reach the address it is bound to; and smtp_authserv_id is set, so a forged Authentication-Results field is not
// kept beside the gateway's. A listener that takes mail from the internet (smtp_internet) trusts no gateway: it is never
// reported as trusted reception, only as checking mail itself.
//
// test#Protocol runs one check of a protocol's list: its configuration (nothing is sent), its listener (one TCP connection
// from this server to the address the conf binds, closed at once) or its public address (the name the deployment gives its
// clients, looked up and connected to from here: something leaves the server, so it is run only when confirmed). The target
// is always the conf's own, by the server's fixed list (probe#ProtocolTarget); none is taken from the request.
const lib = ec.resource.script("component://framework/service/org/moqui/admin/ConfigurationLib.js")
const {one, rows, map, iso, text, refused} = lib
const CHECK = "moqui.server.ConnectionCheck"
const CHECK_KIND = "protocol"
const FACTS = "org.moqui.admin.ConfigurationServices.get#ProtocolFacts"
const PROBE = "org.moqui.admin.ConfigurationServices.probe#ProtocolTarget"
const viewPermissions = [
    "ADMIN_CONNECTION_VIEW",
    "ADMIN_CONNECTION_MANAGE",
    "ADMIN_SETTINGS_VIEW",
    "ADMIN_SETTINGS_MANAGE",
    "ADMIN_OPERATION_VIEW",
    "ADMIN_OPERATION_MANAGE"
]
const MANAGE = "ADMIN_CONNECTION_MANAGE"
const stateTexts = {
    absent: "Not in this build",
    disabled: "Off",
    failing: "Failing",
    untrusted: "Running, not trusted",
    notStarted: "Configured",
    ready: "Ready"
}

// ===== facts =====
const yes = value => ["true", "y", "yes", "1"].includes(text(value).toLowerCase())
// What the server knows, for one call: {features: Set, entries: Map name -> {state, detail}, serverStarted, conf, prop}.
// A server that does not answer (an older build) leaves the entries empty, and nothing is then said to run.
function serverFacts() {
    let out = {}
    try {
        ec.message.pushErrors()
        out = ec.service.sync().name(FACTS).call() || {}
        if (ec.message.hasError()) {
            ec.logger.error("The protocol facts could not be read: " + ec.message.getErrorsString())
            out = {}
        }
    } catch (error) {
        ec.logger.error("The protocol facts could not be read: " + error.message)
    } finally {
        ec.message.clearErrors()
        ec.message.popErrors()
    }
    const conf = map(out.confMap)
    return {
        features: {has: name => !!ec.factory.hasFeature(name)},
        entries: new Map(
            Array.from(out.entryList || []).map(row => [String(row.name), {state: String(row.state), detail: text(row.detail)}])
        ),
        serverStarted: out.serverStarted === true,
        packageName: text(out.packageName),
        prop: name => text(System.getProperty(name)),
        // a conf attribute as the server reads it, else the property the shipped conf sets it from
        conf: (attribute, propertyName) =>
            Object.hasOwn(conf, attribute) ? text(conf[attribute]) : text(System.getProperty(propertyName))
    }
}

// ===== SMTP trust =====
// An entry of smtp_allowed_addresses: an address or a range (CIDR). {ok, every: a range of every address, loopback: it holds
// this machine's own address}.
function allowedNet(entry) {
    const found = /^([0-9a-fA-F:.]+)(?:\/(\d{1,3}))?$/.exec(entry)
    if (!found) return {ok: false}
    const [, address, bitsText] = found
    if (address.includes(":")) {
        if (!/^[0-9a-fA-F:]+$/.test(address) || address.split(":").length > 8) return {ok: false}
        const bits = bitsText == null ? 128 : Number(bitsText)
        if (bits > 128) return {ok: false}
        // ::1 itself, or a range from :: wide enough to hold it
        const holdsLoopback = /^[0:]*:0*1$/.test(address) || (/^[0:]+$/.test(address) && bits <= 127)
        return {ok: true, every: bits === 0, loopback: bits === 0 || holdsLoopback}
    }
    const octets = address.split(".").map(Number)
    if (octets.length !== 4 || octets.some(octet => !Number.isInteger(octet) || octet > 255)) return {ok: false}
    const bits = bitsText == null ? 32 : Number(bitsText)
    if (bits > 32) return {ok: false}
    const value = octets.reduce((sum, octet) => sum * 256 + octet, 0)
    const size = 2 ** (32 - bits)
    const start = Math.floor(value / size) * size
    const loopbackStart = 127 * 2 ** 24
    return {ok: true, every: bits === 0, loopback: start <= loopbackStart + 1 && start + size > loopbackStart + 1}
}
const loopbackHost = host => ["", "localhost", "::1", "[::1]"].includes(host) || /^127\./.test(host)
// Whether SMTP reception may be called trusted, from what it is configured with: {trusted, state (trusted, direct: it takes
// internet mail and trusts no gateway, mismatch, off), problems: each in words}. Trusted only with no problem at all.
function smtpTrust(configured) {
    const allowed = text(configured.allowedAddresses)
        .split(/[,\s]+/)
        .filter(Boolean)
    const problems = []
    if (configured.internet)
        return {
            trusted: false,
            state: "direct",
            problems: [],
            text: "It takes mail from the internet and checks it itself: no gateway's verdict is trusted"
        }
    const nets = allowed.map(allowedNet)
    if (!allowed.length) problems.push("smtp_allowed_addresses is empty: no gateway may deliver, and the listener does not start")
    else if (nets.some(net => !net.ok)) problems.push("smtp_allowed_addresses has an entry that is no address or range")
    else {
        if (nets.some(net => net.every))
            problems.push("smtp_allowed_addresses allows every address: anyone who connects could write the gateway's verdict")
        if (loopbackHost(text(configured.listenHost)) && !configured.proxyProtocol && !nets.some(net => net.loopback))
            problems.push(
                "The listener is bound to this machine only (smtp_listen_host) and no allowed address is on it: the gateway cannot connect"
            )
    }
    if (!text(configured.authservId) && !configured.edgeNode)
        problems.push(
            "smtp_authserv_id is empty: every Authentication-Results field is kept as it came, a forged one beside the gateway's"
        )
    if (problems.length) return {trusted: false, state: "mismatch", problems, text: problems[0]}
    if (!configured.listening)
        return {trusted: false, state: "off", problems: [], text: "The listener is not running, so nothing is received"}
    return {
        trusted: true,
        state: "trusted",
        problems: [],
        text: "Only the gateway's addresses may deliver, and only its Authentication-Results are kept"
    }
}
const smtpConfigured = (facts, listening) => ({
    listening,
    allowedAddresses: facts.conf("smtp-server/@allowed-addresses", "smtp_allowed_addresses"),
    authservId: facts.conf("smtp-server/@authserv-id", "smtp_authserv_id"),
    listenHost: facts.conf("smtp-server/@listen-host", "smtp_listen_host"),
    proxyProtocol: yes(facts.conf("smtp-server/@proxy-protocol", "")),
    internet: text(facts.conf("smtp-server/@internet", ""))
        ? yes(facts.conf("smtp-server/@internet", ""))
        : yes(facts.prop("smtp_internet")),
    edgeNode: facts.prop("node_role") === "edge"
})

// ===== the protocols =====
// on(facts): {on, reason} whether the conf turns it on, and when it does not, the setting that would. entry: its line of the
// startup report (a listener or an integration); web: it is served by the web listener. feature null: no build has it.
const set = (facts, name) => !!facts.prop(name)
const protocols = [
    {
        protocolKey: "smtpOut",
        title: "Outgoing mail",
        description: "The mail the instance sends through its relays (SMTP)",
        feature: "smtp",
        ownerKey: "mailRelays",
        on: () => ({on: true})
    },
    {
        protocolKey: "smtpIn",
        title: "Mail reception",
        description: "The SMTP listener that takes mail for hosted and shared accounts from the edge gateway",
        feature: "inbound-smtp",
        entry: "SMTP",
        settingGroup: "Protocols: mail reception",
        on: facts => ({
            on: !!facts.conf("smtp-server/@listen-port", "smtp_listen_port"),
            reason: "smtp_listen_port is not set"
        }),
        checks: ["configuration", "listener"],
        listenerTarget: "smtp.listener"
    },
    {
        protocolKey: "mailClient",
        title: "Mail accounts",
        description: "Personal and shared mailboxes read over IMAP and POP3, with a provider's OAuth sign-in",
        feature: "imap",
        ownerKey: "mailAccounts",
        on: () => ({on: true})
    },
    {
        protocolKey: "dav",
        title: "Calendars, contacts and files",
        description: "CalDAV, CardDAV and WebDAV for desktop and phone apps, on the web address",
        feature: "axum",
        web: true,
        on: () => ({on: true}),
        checks: ["configuration", "publicAddress"],
        publicTarget: "web.public"
    },
    {
        protocolKey: "ssh",
        title: "SSH and SFTP",
        description: "The built-in SSH server: SFTP, the terminal client and git over SSH",
        feature: "ssh",
        entry: "SSH",
        settingGroup: "Protocols: SSH and Git",
        on: facts => ({
            on: facts.conf("ssh-server/@enabled", "") !== "false" && !!facts.conf("ssh-server/@port", "ssh_port"),
            reason: "ssh_port is not set, or ssh-server enabled is false"
        }),
        checks: ["configuration", "listener", "publicAddress"],
        listenerTarget: "ssh.listener",
        publicTarget: "ssh.public"
    },
    {
        protocolKey: "git",
        title: "Git hosting",
        description: "Repositories over HTTPS on the web address (and over SSH, above)",
        feature: "git",
        web: true,
        settingGroup: "Protocols: SSH and Git",
        on: () => ({on: true})
    },
    {
        protocolKey: "xmpp",
        title: "Chat apps",
        description: "XMPP for the users' own chat apps",
        feature: "xmpp",
        entry: "XMPP",
        settingGroup: "Protocols: chat",
        on: facts => ({on: set(facts, "xmpp_domain"), reason: "xmpp_domain is not set"}),
        checks: ["configuration", "listener", "publicAddress"],
        listenerTarget: "xmpp.listener",
        publicTarget: "xmpp.public"
    },
    {
        protocolKey: "rtc",
        title: "Call media",
        description: "The media of audio and video calls (WebRTC)",
        feature: "rtc",
        entry: "Call media",
        settingGroup: "Protocols: calls",
        on: facts => ({on: set(facts, "rtc_media_port"), reason: "rtc_media_port is not set"}),
        checks: ["configuration"]
    },
    {
        protocolKey: "turn",
        title: "Call relay",
        description: "TURN, for browsers that cannot reach the call media directly",
        feature: "rtc",
        entry: "TURN",
        settingGroup: "Protocols: calls",
        on: facts => ({
            on: set(facts, "turn_port") || set(facts, "turn_tls_port") || set(facts, "turn_external_urls"),
            reason: "turn_port, turn_tls_port and turn_external_urls are not set"
        }),
        checks: ["configuration", "listener", "publicAddress"],
        listenerTarget: "turn.listener",
        publicTarget: "turn.public"
    },
    {
        protocolKey: "sip",
        title: "Phone calls",
        description: "SIP trunks and desk phones; the phone system itself is set up in Voice",
        feature: "sip",
        entry: "SIP",
        settingGroup: "Protocols: phone",
        ownerKey: "voice",
        on: facts => ({
            on: set(facts, "sip_port") || set(facts, "sip_tls_port"),
            reason: "sip_port and sip_tls_port are not set"
        }),
        checks: ["configuration", "listener"],
        listenerTarget: "sip.listener"
    },
    {
        protocolKey: "recording",
        title: "Call recording",
        description: "A call's host may record it; every leg is told",
        feature: "rtc",
        entry: "Call recording",
        settingGroup: "Protocols: calls",
        on: facts => ({on: yes(facts.prop("call_recording_enabled")), reason: "call_recording_enabled is not true"})
    },
    {
        protocolKey: "captions",
        title: "Live captions",
        description: "Speech to text during a call, by the speech provider",
        feature: "rtc",
        entry: "Call captions",
        settingGroup: "Protocols: calls",
        on: facts => ({on: yes(facts.prop("call_captions_enabled")), reason: "call_captions_enabled is not true"})
    },
    {
        protocolKey: "speechLocal",
        title: "Local speech to text",
        description: "Captions without a speech provider",
        feature: "speech-local",
        on: () => ({on: true})
    },
    {
        protocolKey: "wopi",
        title: "Office documents",
        description: "Editing documents in an office suite over WOPI",
        feature: null,
        on: () => ({on: false})
    }
]
const checkTitles = {
    configuration: {
        title: "Configuration",
        expected: "The protocol is built, turned on and started, and what it trusts holds",
        external: false
    },
    listener: {
        title: "Listener",
        expected: "The address the server binds accepts a connection from this server",
        external: false
    },
    publicAddress: {
        title: "Public address",
        expected: "The name clients are given has an address, and its port accepts a connection from this server",
        external: true
    }
}

// The state of a protocol from the facts alone (no stored check): {built, enabled, state, reason, listener, source}.
function stateOf(definition, facts) {
    const answer = (state, reason, extra = {}) =>
        Object.assign({built: state !== "absent", enabled: false, state, reason, listener: "", source: "Configuration"}, extra)
    if (!definition.feature) return answer("absent", "No build has it yet", {source: "Build"})
    if (!facts.features.has(definition.feature))
        return answer("absent", "This build leaves it out (the " + definition.feature + " feature)", {source: "Build"})
    const entryName = definition.web
        ? ["HTTPS", "HTTP"].find(name => facts.entries.get(name)?.state === "on") || "HTTP"
        : definition.entry
    const entry = entryName ? facts.entries.get(entryName) : null
    if (entry) {
        const source = "Startup"
        if (entry.state === "notBuilt") return answer("absent", "This build leaves it out", {source: "Build"})
        if (entry.state === "on") return answer("ready", "", {enabled: true, listener: entry.detail, source})
        if (entry.state === "off") return answer("disabled", entry.detail || "Not turned on", {source})
        if (entry.state === "skipped")
            return answer("failing", "Not started: " + (entry.detail || "its port was busy"), {enabled: true, source})
        return answer("failing", "Did not start: " + (entry.detail || "see the server's log"), {enabled: true, source})
    }
    const configured = definition.on(facts)
    if (!configured.on) return answer("disabled", configured.reason || "Not turned on")
    if (!definition.entry && !definition.web) return answer("ready", "", {enabled: true})
    if (!facts.serverStarted)
        return answer("notStarted", "Turned on; this process opened no listeners, so it is not known to run", {enabled: true})
    return answer("failing", "Turned on, and the server did not report it at startup", {enabled: true})
}
// The last result of each check, by protocol and check key.
function storedChecks() {
    const found = new Map()
    for (const row of rows(CHECK, {adapterKey: CHECK_KIND})) found.set(String(row.connectionKey), map(row))
    return found
}
const checkKeyOf = (protocolKey, checkKey) => protocolKey + "." + checkKey
function checksOf(definition, facts, stored, canTest) {
    return (definition.checks || []).map(checkKey => {
        const last = stored.get(checkKeyOf(definition.protocolKey, checkKey))
        const title = checkTitles[checkKey]
        let result = {}
        try {
            result = JSON.parse(last?.lastCheckText || "{}")
        } catch (error) {
            result = {}
        }
        return {
            protocolKey: definition.protocolKey,
            protocolTitle: definition.title,
            checkKey,
            title: title.title,
            expected: title.expected,
            external: title.external,
            target: text(result.target) || null,
            lastCheckDate: iso(last?.lastCheckDate),
            lastCheckPassed: last ? last.lastCheckPassed === "Y" : null,
            lastResultText: text(result.text) || null,
            canRun: canTest
        }
    })
}
// A protocol as the screens show it: its state with the stored checks and, for SMTP reception, its trust.
function described(definition, facts, stored, canTest) {
    const found = stateOf(definition, facts)
    const checkList = found.built ? checksOf(definition, facts, stored, canTest) : []
    let {state, reason} = found
    let trust = null
    if (definition.protocolKey === "smtpIn" && found.built && found.enabled) {
        trust = smtpTrust(smtpConfigured(facts, state === "ready"))
        // a mismatch is never ready: reception that runs without its trust is shown as such
        if (state === "ready" && trust.state === "mismatch") {
            state = "untrusted"
            reason = trust.text
        }
    }
    const failedCheck = checkList.find(check => check.lastCheckPassed === false)
    if (state === "ready" && failedCheck) {
        state = "failing"
        reason = "Its last check did not pass: " + failedCheck.title.toLowerCase()
    }
    return {
        protocolKey: definition.protocolKey,
        title: definition.title,
        description: definition.description,
        built: found.built,
        buildFeature: definition.feature,
        enabled: found.enabled,
        state,
        stateText: stateTexts[state],
        reason,
        listener: found.listener,
        source: found.source,
        settingGroup: definition.settingGroup || null,
        ownerKey: definition.ownerKey || null,
        trustState: trust ? trust.state : null,
        trusted: trust ? trust.trusted : null,
        trustText: trust ? trust.text : null,
        trustProblems: trust ? trust.problems : [],
        checkList
    }
}

// ===== services =====
function requireView() {
    lib.signedIn()
    if (!viewPermissions.some(permission => ec.user.hasPermission(permission)))
        throw new Error(viewPermissions[0] + " is required")
}
// get#ProtocolReadiness: every protocol with its state, or one.
function getProtocolReadiness(p) {
    requireView()
    const canTest = ec.user.hasPermission(MANAGE)
    const facts = serverFacts()
    const stored = storedChecks()
    const protocolList = protocols
        .filter(definition => !text(p.protocolKey) || definition.protocolKey === text(p.protocolKey))
        .map(definition => described(definition, facts, stored, canTest))
    if (text(p.protocolKey) && !protocolList.length) throw new Error("Unknown protocol " + p.protocolKey)
    return {
        protocolList,
        checkList: protocolList.flatMap(protocol => protocol.checkList),
        serverStarted: facts.serverStarted,
        packageName: facts.packageName,
        checkedDate: iso(ec.user.nowTimestamp),
        canTest
    }
}
const failureTexts = {
    notConfigured: "the configuration names no host or port for it",
    notFound: "the name has no address",
    refused: "the connection was refused",
    timedOut: "no answer in time",
    unreachable: "it could not be reached"
}
// One check, run now: {passed, target, text}; texts name a host, a port and a kind of failure, nothing else.
function runCheck(definition, checkKey, facts, confirmed) {
    if (checkKey === "configuration") {
        const found = described(definition, facts, new Map(), false)
        const passed = found.state === "ready"
        return {
            passed,
            target: null,
            text: passed ? "Ready" + (found.listener ? ": " + found.listener : "") : found.stateText + ": " + found.reason
        }
    }
    const targetKey = checkKey === "listener" ? definition.listenerTarget : definition.publicTarget
    let out = {}
    let failure = ""
    ec.message.pushErrors()
    try {
        out = ec.service.sync().name(PROBE).parameters({targetKey, confirmed}).call() || {}
        if (ec.message.hasError()) failure = ec.message.getErrorsString()
    } catch (error) {
        failure = String(error?.message ?? error)
    } finally {
        ec.message.clearErrors()
        ec.message.popErrors()
    }
    if (failure) {
        ec.logger.error("The protocol probe " + targetKey + " did not run: " + failure)
        return {passed: false, target: null, text: "The check could not be run on this server"}
    }
    const target = text(out.target) || null
    if (out.reached === true)
        return {
            passed: true,
            target,
            text:
                target +
                " accepted a connection" +
                (out.internalAddress === true ? "; the name answers with an address inside the network" : "")
        }
    return {
        passed: false,
        target,
        text: (target || "The target") + ": " + (failureTexts[out.failureKind] || failureTexts.unreachable)
    }
}
// test#Protocol: one check of a protocol, its result kept as the check's last.
function testProtocol(p) {
    lib.requireManage(MANAGE)
    const definition = protocols.find(candidate => candidate.protocolKey === text(p.protocolKey))
    if (!definition) throw new Error("Unknown protocol " + text(p.protocolKey))
    const checkKey = text(p.checkKey)
    if (!(definition.checks || []).includes(checkKey)) throw new Error(definition.title + " has no check " + checkKey)
    const facts = serverFacts()
    const found = stateOf(definition, facts)
    const title = checkTitles[checkKey]
    if (!found.built) return refused("NOT_BUILT", definition.title + " is not in this build: there is nothing to check")
    // nothing leaves the server without the person's word
    const confirmed = p.confirmed === true || p.confirmed === "true"
    if (title.external && !confirmed)
        return refused(
            "CONFIRM_REQUIRED",
            "This check looks up the public name and connects to it from this server. Confirm it to run it."
        )
    if (checkKey !== "configuration" && !found.enabled)
        return refused("NOT_ENABLED", definition.title + " is off (" + found.reason + "): nothing was sent")
    const result = runCheck(definition, checkKey, facts, confirmed)
    const connectionKey = checkKeyOf(definition.protocolKey, checkKey)
    lib.write(
        CHECK,
        {
            adapterKey: CHECK_KIND,
            connectionKey,
            lastCheckDate: ec.user.nowTimestamp,
            lastCheckPassed: result.passed ? "Y" : "N",
            lastCheckByUserId: ec.user.userId,
            lastCheckText: JSON.stringify({target: result.target, text: result.text})
        },
        !one(CHECK, {adapterKey: CHECK_KIND, connectionKey})
    )
    return {
        applied: true,
        passed: result.passed,
        protocolKey: definition.protocolKey,
        checkKey,
        target: result.target,
        resultText: result.text,
        checkedDate: iso(ec.user.nowTimestamp),
        external: title.external
    }
}

const operations = {getProtocolReadiness, testProtocol}
;({
    smtpTrust,
    // a protocol's readiness from given facts, with no stored check: what get#ProtocolReadiness answers for it
    readinessOf: (protocolKey, facts) =>
        described(
            protocols.find(definition => definition.protocolKey === protocolKey),
            facts,
            new Map(),
            false
        ),
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown configuration protocols operation " + op)
        return operations[op](parameters)
    }
})
