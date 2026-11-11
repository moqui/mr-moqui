// The connection part of org.moqui.admin.ConfigurationServices (mr-boss plan/design-admin.md §8.1, Connections,
// ConnectionDetail, §9 MailRelays, MailRelayDetail, §14 "Connection adapter"; impl-admin P11): one list, detail, save and
// test over the native configuration records, each kind through its registered adapter (moqui.server.ConnectionAdapter,
// seed data, never a name an administrator enters). The framework's kinds are defined here: remote
// (SystemMessageRemote), relay (an EmailServer no EmailAccount owns), mailOAuth (EmailOAuthProvider) and printer
// (NetworkPrinter). A component's kinds (Mantle's gateways) are its own service, built with the same engine (lib.engine).
//
// What every kind holds to:
// - reading needs ADMIN_CONNECTION_VIEW (or MANAGE), a change or a test ADMIN_CONNECTION_MANAGE; an adapter adds its own
//   domain checks and never fewer;
// - a secret is write-only: never returned (secretSet only), never logged, never in a test's text; its replacement or
//   clearing is an audit row with no value;
// - the endpoint-change rule (design §8.1): a change of a field a stored secret is sent to or stands for (a host, port,
//   URL, user or kind of sign-in) needs the secrets entered again in the same save, else the stored ones are cleared;
//   either way the connection is saved disabled and its test mark cleared, and a passing test of it as it is then
//   enables it again. While a cleared secret has not been entered again a test sends nothing;
// - on a production instance a connection started or repointed here may not use the private network (the native
//   record's allowPrivateNetwork N, which the sender itself enforces at connect time and this service never sets
//   back): the address is the integration administrator's to choose, and the server would reach it from inside the
//   network. An instance administrator allows one on the instance's own network (Tools);
// - a test answers a fixed vocabulary (a host, a status code, a kind of failure), never a response body or a remote
//   server's own text, within seconds;
// - only the fields a kind declares are changed, each validated; an executable name (a send service, a gateway's
//   service names) is never one of them, and a stored text that the server expands is refused a ${}.
//
// A refusal a person can act on is answered (applied false, reasonCode, reason), never written; a missing permission or a
// malformed request is an error. No dependency on Mantle or an app.
const lib = ec.resource.script("component://framework/service/org/moqui/admin/ConfigurationLib.js")
const {find, one, rows, map, iso, stamp, text, refused, plainText, urlText, write} = lib

const ADAPTER = "moqui.server.ConnectionAdapter"
const PROVIDER = "moqui.server.ConnectionConsumerProvider"
const CHECK = "moqui.server.ConnectionCheck"
const ACCOUNT = "moqui.security.UserAccount"
const VIEW = "ADMIN_CONNECTION_VIEW"
const MANAGE = "ADMIN_CONNECTION_MANAGE"
const has = permission => ec.user.hasPermission(permission)
const yes = value => value === true || value === "true" || value === "Y"
const no = value => value === false || value === "false" || value === "N"
const production = () => text(System.getProperty("instance_purpose")) === "production"
const ownNames = value => {
    const found = value == null ? {} : map(value)
    return Object.fromEntries(Object.keys(found).map(name => [name, found[name]]))
}

// ===== validation the kinds share =====
// A host name or an IPv4 address, nothing else (no scheme, port, path or credentials).
function hostText(value, what) {
    const found = plainText(value, what)
    if (!found) return ""
    if (!/^(?=.{1,253}$)[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?(\.[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?)*$/.test(found))
        throw new Error(what + " is a host name or an IPv4 address")
    return found.toLowerCase()
}
function portText(value, what) {
    const found = text(value)
    if (!found) return ""
    if (!/^\d{1,5}$/.test(found) || Number(found) < 1 || Number(found) > 65535)
        throw new Error(what + " is a port from 1 to 65535")
    return String(Number(found))
}
function numberText(value, what, min, max) {
    const found = text(value)
    if (!found) return ""
    if (!/^\d{1,9}$/.test(found) || Number(found) < min || Number(found) > max)
        throw new Error(what + " is a whole number from " + min + " to " + max)
    return String(Number(found))
}
function patternText(value, what, pattern, hint, maxLength = 255) {
    const found = plainText(value, what, maxLength)
    if (found && !pattern.test(found)) throw new Error(what + " " + hint)
    return found
}
const enumOptions = enumTypeId =>
    rows("moqui.basic.Enumeration", {enumTypeId})
        .sort(
            (a, b) =>
                (a.sequenceNum ?? 999) - (b.sequenceNum ?? 999) || String(a.description).localeCompare(String(b.description))
        )
        .map(row => ({value: row.enumId, label: row.description || row.enumId}))
// The host of a URL, for a summary and a test's text: never its path, query or credentials.
function hostOf(url) {
    const parts = /^[a-z][a-z0-9+.-]*:\/\/(?:[^\/?#@]*@)?(\[[^\]]+\]|[^\/?#:]+)/i.exec(text(url))
    return parts ? parts[1].toLowerCase() : ""
}
// Why a request did not get an answer, from the error's own words, as one of a fixed list: never the remote's text.
function failureKind(message) {
    const found = String(message || "").toLowerCase()
    if (found.includes("internal address")) return "internal"
    if (/sim mode|read fence|denied because|not allowed in|sends nothing to the network/.test(found)) return "blocked"
    if (/not installed|not available|not built|no print client/.test(found)) return "unavailable"
    if (/timed? ?out|deadline/.test(found)) return "timeout"
    if (/certificate|tls|ssl|handshake/.test(found)) return "tls"
    if (/refused/.test(found) && /connect/.test(found)) return "refused"
    if (/lookup|resolve|dns|nodename|name or service|no address|could not be looked up/.test(found)) return "lookup"
    if (/auth|credential|sign|53[45]/.test(found)) return "signin"
    return "other"
}
const failureTexts = {
    internal: "is an internal address: an instance administrator allows a connection on the instance's own network",
    blocked: "was not tried: outbound requests are not allowed in this session",
    unavailable: "cannot be tested: this build has no client for it",
    timeout: "did not answer in time",
    tls: "could not be reached over TLS (its certificate or protocol was not accepted)",
    refused: "refused the connection",
    lookup: "was not found (its name does not resolve)",
    signin: "refused the sign-in",
    other: "could not be reached"
}
const failureText = (place, message) => place + " " + failureTexts[failureKind(message)]
// One request with no credentials and no body, to learn that an address answers: {ok, text}. HEAD, ten seconds, the
// answer's status only (its body is never read into the result); with denyInternal no hop goes to an internal address.
function probe(url, denyInternal) {
    const host = hostOf(url) || "The address"
    try {
        let client = ec.service.rest().method("HEAD").uri(url).timeout(10)
        if (denyInternal) client = client.denyInternal()
        const status = Number(client.call().getStatusCode())
        return status > 0 && status < 500
            ? {ok: true, text: host + " answered (HTTP " + status + "); no credentials were sent"}
            : {ok: false, text: host + " answered with a server error (HTTP " + status + ")"}
    } catch (error) {
        return {ok: false, text: failureText(host, error?.message ?? error)}
    }
}

// ===== the engine: a kind's definition to its operations =====
// A definition: {adapterKey, parts: {name: {entity, keyField}} (main first; created in this order), statePart (the part
// with disabled and, unless privateNetwork is false, allowPrivateNetwork; null when the kind has no endpoint of its
// own), locate(connectionKey) -> {part: key}, fields: [{name, column, part, label, kind (text, url, host, port, number,
// choice, flag, display), endpoint, retest, options(), help, audited (the entity audit-logs its updates), read(parts) for a display
// value, validate(value) -> the value to store}], secrets: [{name, column, part, label, help, fixed (cleared by the rule,
// entered in Tools only)}], name(parts), providerName(parts), summary(parts), listKeys(p), hidden(parts) -> a refusal for a record that is
// not this kind's on any path, readOnly(parts) -> why it is not changed here, incomplete(parts) -> what it lacks,
// check(after, context) -> a refusal of the whole change, test(parts, context) -> [{check, ok, text}],
// newParts(values, p) -> {connectionKey, parts} for a new one, consumers(parts, connectionKey), extra(parts, detail)}.
function engine(def) {
    const partNames = Object.keys(def.parts)
    const partOf = entry => entry.part || "main"
    const columnOf = entry => entry.column || entry.name
    const valueOf = (parts, entry) => parts[partOf(entry)]?.[columnOf(entry)]
    const stored = (parts, entry) =>
        entry.kind === "flag" ? (valueOf(parts, entry) === "Y" ? "Y" : "N") : text(valueOf(parts, entry))
    const editable = def.fields.filter(entry => entry.kind !== "display")
    // a secret that is not entered here (cleared by the rule all the same): by its kind, or by the record as it is
    const fixedOf = (entry, parts) => (typeof entry.fixed === "function" ? !!entry.fixed(parts) : !!entry.fixed)
    const keysOf = connectionKey => (def.locate ? def.locate(connectionKey) : {main: connectionKey})

    function load(connectionKey) {
        const key = text(connectionKey)
        if (!key) return null
        const keys = keysOf(key) || {}
        const parts = {}
        for (const name of partNames) {
            const partKey = text(keys[name])
            const row = partKey ? one(def.parts[name].entity, {[def.parts[name].keyField]: partKey}) : null
            parts[name] = row ? map(row) : null
        }
        return parts.main ? parts : null
    }
    const checkOf = connectionKey => {
        const row = one(CHECK, {adapterKey: def.adapterKey, connectionKey})
        return row ? map(row) : null
    }
    // What a passing test stands for: the endpoint fields, what else needs a new test, and whether each secret is set.
    function fingerprint(parts) {
        const found = [def.adapterKey]
        for (const entry of editable.filter(entry => entry.endpoint || entry.retest))
            found.push(entry.name + "=" + stored(parts, entry))
        for (const entry of def.secrets) found.push(entry.name + (text(valueOf(parts, entry)) ? "+" : "-"))
        return ec.resource.digestText(found.join("\n"), "sha256")
    }
    const revisionOf = parts =>
        ec.resource.digestText(partNames.map(name => name + ":" + stamp(parts[name])).join("|"), "sha256").slice(0, 24)
    const stateRow = parts => (def.statePart ? parts[def.statePart] : null)
    const enabledOf = parts => stateRow(parts)?.disabled !== "Y"
    const stateTexts = {
        ready: "Enabled",
        untested: "Enabled, not tested as it is configured now",
        needsTest: "Disabled until a test passes",
        disabled: "Disabled",
        incomplete: "Not configured",
        readOnly: "Read only here"
    }
    function stateOf(parts, check) {
        if (def.readOnly?.(parts)) return "readOnly"
        if (def.incomplete?.(parts)) return "incomplete"
        const tested = !!check?.testedConfigHash && check.testedConfigHash === fingerprint(parts)
        // a connection with no endpoint of its own (a local gateway) has nothing to test
        if (enabledOf(parts)) return tested || !stateRow(parts) ? "ready" : "untested"
        return check?.pendingTest === "Y" ? "needsTest" : "disabled"
    }
    function described(connectionKey, parts, check) {
        const state = stateOf(parts, check)
        return {
            adapterKey: def.adapterKey,
            connectionKey,
            name: text(def.name?.(parts)) || connectionKey,
            providerName: text(def.providerName?.(parts)),
            endpointSummary: text(def.summary?.(parts)),
            enabled: enabledOf(parts) && state !== "incomplete",
            state,
            stateText: stateTexts[state],
            privateNetwork: !def.statePart || def.privateNetwork === false ? null : stateRow(parts)?.allowPrivateNetwork !== "N",
            lastCheckDate: iso(check?.lastCheckDate),
            lastCheckPassed: check?.lastCheckDate ? check.lastCheckPassed === "Y" : null
        }
    }
    function checkList(check) {
        try {
            return check?.lastCheckText ? Array.from(JSON.parse(check.lastCheckText)) : []
        } catch (error) {
            return []
        }
    }

    // ----- list and get -----
    function list(p) {
        const found = []
        for (const connectionKey of Array.from(def.listKeys(p) || []).slice(0, 500)) {
            const parts = load(connectionKey)
            if (!parts || def.hidden?.(parts)) continue
            const row = described(connectionKey, parts, checkOf(connectionKey))
            if (def.listExtra) Object.assign(row, def.listExtra(parts, connectionKey, p) || {})
            found.push(row)
        }
        return {connectionList: found}
    }
    function get(p) {
        const connectionKey = text(p.connectionKey)
        const parts = load(connectionKey)
        if (!parts || def.hidden?.(parts)) return {connection: null}
        const check = checkOf(connectionKey)
        const readOnlyReason = def.readOnly?.(parts) || null
        const tester = check?.testedByUserId ? one(ACCOUNT, {userId: check.testedByUserId}) : null
        const connection = Object.assign(described(connectionKey, parts, check), {
            // the fields and secrets of the parts this connection has (a gateway type's options, a gateway's remote)
            fieldList: def.fields
                .filter(entry => parts[partOf(entry)])
                .map(entry => ({
                    name: entry.name,
                    label: entry.label,
                    kind: entry.kind,
                    value: entry.read ? text(entry.read(parts)) : stored(parts, entry),
                    endpoint: !!entry.endpoint,
                    help: entry.help || "",
                    options: entry.kind === "choice" ? entry.options() : []
                })),
            // a secret is never returned: only whether one is set
            secretList: def.secrets
                .filter(entry => parts[partOf(entry)])
                .map(entry => ({
                    name: entry.name,
                    label: entry.label,
                    secretSet: !!text(valueOf(parts, entry)),
                    fixed: fixedOf(entry, parts),
                    help: entry.help || ""
                })),
            problem: def.incomplete?.(parts) || null,
            readOnlyReason,
            secretCleared: check?.secretCleared === "Y",
            tested: !!check?.testedConfigHash && check.testedConfigHash === fingerprint(parts),
            testedDate: iso(check?.testedDate),
            testedByName: tester ? tester.userFullName || tester.username : null,
            checkList: checkList(check),
            hasEndpoint: !!def.statePart && !!stateRow(parts),
            revision: revisionOf(parts)
        })
        if (def.extra) Object.assign(connection, def.extra(parts, connectionKey) || {})
        return {connection}
    }

    // ----- store -----
    function writeCheck(connectionKey, fields) {
        const key = {adapterKey: def.adapterKey, connectionKey}
        write(CHECK, Object.assign(key, fields), !one(CHECK, key))
    }
    function store(p) {
        lib.requireManage(MANAGE)
        const reason = lib.reasonText(p)
        const values = ownNames(p.values)
        const given = name => Object.hasOwn(values, name) && values[name] !== undefined && values[name] !== null
        let connectionKey = text(p.connectionKey)
        let parts = connectionKey ? load(connectionKey) : null
        const creating = !parts
        if (creating) {
            if (!def.newParts) return refused("NOT_CREATABLE", "A connection of this kind is not started here")
            const made = def.newParts(values, p)
            if (made.applied === false) return made
            connectionKey = made.connectionKey
            parts = made.parts
        } else {
            const hidden = def.hidden?.(parts)
            if (hidden) return hidden
            const readOnly = def.readOnly?.(parts)
            if (readOnly) return refused("READ_ONLY", readOnly, {connectionKey})
        }
        const revision = creating ? "" : revisionOf(parts)
        if (!creating && text(p.expectedRevision) !== revision)
            return refused(
                "STALE_REVISION",
                "The connection was changed since it was read; read it again and repeat the change",
                {
                    connectionKey,
                    revision
                }
            )
        // what the save changes: {part: {column: value}}, only what differs from the stored rows
        const changes = Object.fromEntries(partNames.map(name => [name, {}]))
        const changed = []
        for (const entry of editable) {
            if (!given(entry.name)) continue
            if (!parts[partOf(entry)])
                throw new Error(entry.label + " cannot be set: this connection has no " + partOf(entry) + " record")
            let value
            if (entry.kind === "flag") value = yes(values[entry.name]) ? "Y" : "N"
            else if (entry.validate) value = entry.validate(values[entry.name])
            else if (entry.kind === "url") value = urlText(values[entry.name], entry.label)
            else if (entry.kind === "host") value = hostText(values[entry.name], entry.label)
            else if (entry.kind === "port") value = portText(values[entry.name], entry.label)
            else if (entry.kind === "choice") {
                value = text(values[entry.name])
                if (value && !entry.options().some(option => option.value === value))
                    throw new Error(entry.label + " is not one of its options")
            } else value = plainText(values[entry.name], entry.label, entry.maxLength || 255)
            if (value !== stored(parts, entry)) {
                changes[partOf(entry)][columnOf(entry)] = value || null
                changed.push(entry)
            }
        }
        const secretWrites = []
        for (const entry of def.secrets) {
            if (!given(entry.name) || String(values[entry.name]) === "") continue
            if (fixedOf(entry, parts))
                return refused(
                    "SECRET_NOT_DELEGATED",
                    entry.label + " is not entered here: " + (entry.fixedReason || "an instance administrator sets it in Tools"),
                    {connectionKey}
                )
            const secret = String(values[entry.name])
            if (secret.length > 4000 || /[\u0000-\u0008\u000b\u000c\u000e-\u001f]/.test(secret))
                throw new Error(entry.label + " is too long or holds control characters")
            if (!parts[partOf(entry)])
                throw new Error(entry.label + " cannot be set: this connection has no " + partOf(entry) + " record")
            secretWrites.push({entry, value: secret})
        }

        // ----- the endpoint-change rule -----
        const endpointChanged = !creating && changed.some(entry => entry.endpoint)
        let secretCleared = false
        if (endpointChanged)
            for (const entry of def.secrets)
                if (!secretWrites.some(row => row.entry === entry) && text(valueOf(parts, entry))) {
                    secretWrites.push({entry, value: null})
                    secretCleared = true
                }
        for (const row of secretWrites) changes[partOf(row.entry)][columnOf(row.entry)] = row.value
        const after = Object.fromEntries(
            partNames.map(name => [name, parts[name] ? Object.assign({}, parts[name], changes[name]) : null])
        )
        const refusal = def.check?.(after, {creating, parts})
        if (refusal) return Object.assign({connectionKey, revision}, refusal)
        const check = creating ? null : checkOf(connectionKey)
        const hasState = !!def.statePart && !!parts[def.statePart]
        const wasEnabled = !creating && enabledOf(parts)
        let enabledAfter = hasState ? wasEnabled : true
        let pendingTest = check?.pendingTest === "Y"
        const retest = hasState && (creating || endpointChanged || changed.some(entry => entry.retest))
        const stateChanges = {}
        const checkChanges = {}
        if (retest) {
            if (yes(p.enabled))
                return refused(
                    "TEST_REQUIRED",
                    "The configuration changed: test the connection as it is now, and the test enables it",
                    {
                        connectionKey,
                        revision
                    }
                )
            enabledAfter = false
            pendingTest = true
            Object.assign(checkChanges, {testedConfigHash: null, testedDate: null, testedByUserId: null})
            // the address is the administrator's to choose: on a production instance the server reaches it on the
            // public network only, whoever made the record, until an instance administrator allows more
            if ((creating || endpointChanged) && def.privateNetwork !== false && production())
                stateChanges.allowPrivateNetwork = "N"
        } else if (hasState && yes(p.enabled) && !wasEnabled) {
            const problem = def.incomplete?.(after)
            if (problem) return refused("INCOMPLETE", problem, {connectionKey, revision})
            if (!check?.testedConfigHash || check.testedConfigHash !== fingerprint(after))
                return refused("TEST_REQUIRED", "Test the connection as it is configured now: a passing test enables it", {
                    connectionKey,
                    revision
                })
            enabledAfter = true
            pendingTest = false
        } else if (hasState && no(p.enabled) && wasEnabled) {
            enabledAfter = false
            pendingTest = false
        }
        if (hasState && (creating || enabledAfter !== wasEnabled)) stateChanges.disabled = enabledAfter ? "N" : "Y"
        if (hasState) Object.assign(changes[def.statePart], stateChanges)
        if (secretCleared) checkChanges.secretCleared = "Y"
        else if (secretWrites.some(row => row.value !== null) && check?.secretCleared === "Y") checkChanges.secretCleared = "N"
        if (hasState && pendingTest !== (check?.pendingTest === "Y")) checkChanges.pendingTest = pendingTest ? "Y" : "N"

        const nothing = !creating && partNames.every(name => !Object.keys(changes[name]).length)
        if (nothing)
            return {
                applied: true,
                connectionKey,
                revision,
                enabled: wasEnabled || !hasState,
                secretCleared: false,
                testRequired: false
            }

        // ----- write: each part, then the audit rows the entities do not write themselves -----
        lib.auditReason(reason, editable.map(columnOf).concat(def.secrets.map(columnOf), ["disabled", "allowPrivateNetwork"]))
        if (creating)
            for (const name of partNames) {
                const keyField = def.parts[name].keyField
                if (parts[name] && one(def.parts[name].entity, {[keyField]: parts[name][keyField]}))
                    return refused("KEY_IN_USE", "A record with the id " + parts[name][keyField] + " exists: choose another id")
            }
        for (const name of partNames) {
            if (!parts[name]) continue
            const keyField = def.parts[name].keyField
            const fields = Object.assign({[keyField]: parts[name][keyField]}, creating ? parts[name] : {}, changes[name])
            if (creating) write(def.parts[name].entity, fields, true)
            else if (Object.keys(changes[name]).length) write(def.parts[name].entity, fields, false)
        }
        for (const entry of changed) {
            // an audit-logged field's change of a value is the entity's own row; a value where there was none is not
            if (entry.audited && !creating && stored(parts, entry)) continue
            const part = def.parts[partOf(entry)]
            lib.auditRow(
                part.entity,
                columnOf(entry),
                parts[partOf(entry)][part.keyField],
                null,
                creating ? null : stored(parts, entry) || null,
                changes[partOf(entry)][columnOf(entry)],
                reason
            )
        }
        // a secret's replacement or clearing is recorded with no value
        for (const row of secretWrites) {
            const part = def.parts[partOf(row.entry)]
            lib.auditRow(
                part.entity,
                columnOf(row.entry),
                parts[partOf(row.entry)][part.keyField],
                null,
                !creating && text(valueOf(parts, row.entry)) ? "(set)" : null,
                row.value === null ? "(cleared: the endpoint changed)" : "(replaced)",
                reason
            )
        }
        if (Object.keys(checkChanges).length) writeCheck(connectionKey, checkChanges)
        def.afterStore?.(connectionKey, {creating, endpointChanged, secretCleared})
        const now = load(connectionKey)
        const nowCheck = checkOf(connectionKey)
        return {
            applied: true,
            reasonCode: null,
            reason:
                retest && !creating
                    ? (endpointChanged ? "The endpoint changed: " : "What the connection is tested by changed: ") +
                      (secretCleared ? "the stored secret was cleared, and " : "") +
                      "the connection is disabled until a test of it passes"
                    : null,
            connectionKey,
            revision: revisionOf(now),
            enabled: hasState ? enabledOf(now) : true,
            secretCleared,
            testRequired: hasState && !enabledOf(now) && nowCheck?.pendingTest === "Y"
        }
    }

    // ----- test -----
    // A check's text, safe to show and to keep: never a stored secret, one line, bounded.
    function safeText(message, parts) {
        let found = String(message || "")
        for (const entry of def.secrets) {
            const secret = String(valueOf(parts, entry) || "")
            if (secret.length >= 4) found = found.split(secret).join("(secret)")
        }
        return found.replace(/[\u0000-\u001f]+/g, " ").slice(0, 300)
    }
    function test(p) {
        lib.requireManage(MANAGE)
        const connectionKey = text(p.connectionKey)
        const parts = load(connectionKey)
        if (!parts) return refused("CONNECTION_UNAVAILABLE", "The connection is not available", {passed: false})
        const hidden = def.hidden?.(parts)
        if (hidden) return Object.assign({passed: false}, hidden)
        const readOnly = def.readOnly?.(parts)
        if (readOnly) return refused("READ_ONLY", readOnly, {passed: false, connectionKey})
        const check = checkOf(connectionKey)
        const found = []
        const add = (name, ok, okText, failedText) => {
            found.push({check: name, state: ok ? "ok" : "failed", text: safeText(ok ? okText : failedText, parts)})
            return ok
        }
        const problem = def.incomplete?.(parts)
        let go = add("Configuration", !problem, "The configuration is complete", problem)
        // nothing is sent anywhere while a secret the endpoint change cleared has not been entered again
        if (go && check?.secretCleared === "Y")
            go = add(
                "Credentials",
                false,
                "",
                "A stored secret was cleared when the endpoint changed: enter it again, then test. Nothing was sent"
            )
        if (go) {
            const state = stateRow(parts)
            const context = {
                denyInternal: !!state && state.allowPrivateNetwork === "N",
                testKind: text(p.testKind),
                connectionKey
            }
            let results
            try {
                results = Array.from(def.test ? def.test(parts, context) || [] : [])
            } catch (error) {
                results = [{check: "Connection", ok: false, text: failureText("The connection", error?.message ?? error)}]
            }
            for (const row of results) add(text(row.check) || "Connection", !!row.ok, row.text, row.text)
        }
        const passed = found.every(row => row.state === "ok")
        const fields = {
            lastCheckDate: ec.user.nowTimestamp,
            lastCheckPassed: passed ? "Y" : "N",
            lastCheckByUserId: ec.user.userId,
            lastCheckText: JSON.stringify(found).slice(0, 4000)
        }
        Object.assign(
            fields,
            passed
                ? {testedConfigHash: fingerprint(parts), testedDate: ec.user.nowTimestamp, testedByUserId: ec.user.userId}
                : {testedConfigHash: null, testedDate: null, testedByUserId: null}
        )
        // a passing test enables a connection the endpoint-change rule disabled (or a new one); one an administrator
        // disabled stays disabled
        let enabledByTest = false
        const state = stateRow(parts)
        if (passed && state && state.disabled === "Y" && check?.pendingTest === "Y") {
            const part = def.parts[def.statePart]
            lib.auditReason("A test of the connection passed", ["disabled"])
            write(part.entity, {[part.keyField]: state[part.keyField], disabled: "N"}, false)
            fields.pendingTest = "N"
            enabledByTest = true
        }
        writeCheck(connectionKey, fields)
        const now = load(connectionKey)
        return {
            applied: true,
            passed,
            checkList: found,
            enabled: enabledOf(now),
            enabledByTest,
            testedDate: passed ? iso(fields.testedDate) : null,
            connectionKey,
            revision: revisionOf(now)
        }
    }
    function consumers(p) {
        const connectionKey = text(p.connectionKey)
        const parts = load(connectionKey)
        if (!parts || def.hidden?.(parts)) return {consumerList: []}
        return {consumerList: Array.from(def.consumers?.(parts, connectionKey) || [])}
    }
    const operations = {list, get, store, test, consumers}
    return {
        run: (operation, p) => {
            if (!Object.hasOwn(operations, operation)) throw new Error("Unknown connection operation " + operation)
            return operations[operation](p || {})
        },
        load,
        fingerprint
    }
}

// ===== the framework's kinds =====
const REMOTE = "moqui.service.message.SystemMessageRemote"
const SERVER = "moqui.basic.email.EmailServer"
const MAIL_ACCOUNT = "moqui.basic.email.EmailAccount"
const OAUTH = "moqui.basic.email.EmailOAuthProvider"
const PRINTER = "moqui.basic.print.NetworkPrinter"
const allKeys = (entity, keyField) =>
    Array.from(find(entity).selectField(keyField).orderBy(keyField).limit(500).list()).map(row => String(row[keyField]))
const count = (entity, conditions) => Number(find(entity, conditions).count())

// The address a message is posted to: https (http only for this machine on an instance that is not production). The
// server expands it for each message, so it may hold the three names the sender gives and no other ${}.
const urlNames = ["${remoteMessageId}", "${systemMessageTypeId}", "${systemMessageRemoteId}"]
function sendUrlText(value, what) {
    const found = text(value)
    let bare = found
    for (const name of urlNames) bare = bare.split(name).join("x")
    urlText(bare, what)
    return found
}
// How a message sent is signed in or signed: sendAuthEnumId, else messageAuthEnumId (as send#SystemMessageRest reads them).
const authOf = remote => text(remote.sendAuthEnumId) || text(remote.messageAuthEnumId)
const REST_SEND = "org.moqui.impl.SystemMessageServices.send#SystemMessageRest"
// The remote's fields, for the remote kind and for a gateway whose connection it is (Mantle; gateway true): part names the
// part. How a remote verifies what it RECEIVES (messageAuthEnumId) is never one of them: whoever chose it could open the
// instance to messages of any type (/rest/sm), so it stays as its owner or an instance administrator set it, and a remote
// started here receives only from a signed-in user authorized to (SmatLogin). How it sends is sendAuthEnumId.
function remoteFields(part = "main", prefix = "", gateway = false) {
    const authOptions = () => enumOptions("SystemMessageAuthType").filter(option => option.value !== "SmatUrlToken")
    const fields = [
        {
            name: "sendUrl",
            part,
            label: prefix + "Address",
            kind: "url",
            endpoint: true,
            audited: true,
            validate: value => sendUrlText(value, "The address"),
            help: gateway ? "The provider's API address: an https URL" : "Where messages are posted: an https URL"
        },
        {name: "username", part, label: prefix + "User", kind: "text", endpoint: true, audited: true}
    ]
    if (!gateway)
        fields.push(
            {
                name: "sendAuthEnumId",
                part,
                label: prefix + "Sign-in when sending",
                kind: "choice",
                endpoint: true,
                options: authOptions,
                help: "How a message sent is signed in or signed; empty is as received messages are verified, else user and password"
            },
            {
                name: "authHeaderName",
                part,
                label: prefix + "Signature header",
                kind: "text",
                endpoint: true,
                validate: value => patternText(value, "The signature header", /^[A-Za-z0-9-]{1,80}$/, "is a header name")
            }
        )
    fields.push({
        name: "usageCode",
        part,
        label: prefix + "Usage",
        kind: "text",
        retest: true,
        validate: value => patternText(value, "The usage code", /^[A-Za-z0-9]{1,10}$/, "is letters or digits", 10),
        help: "Production or test, as the remote's standard codes it (P or T)"
    })
    const secrets = [
        {name: "password", part, label: prefix + "Password or token"},
        {
            name: "sharedSecret",
            part,
            label: prefix + (gateway ? "Webhook signature key" : "Shared secret"),
            help: gateway
                ? "Verifies what the provider sends here"
                : "Signs messages sent, when no other secret for sending is given",
            // /rest/sm takes a message of any type from whoever proves a remote's secret, so the secret a remote
            // verifies RECEIVED messages by is not a delegated administrator's to set
            fixed: parts => receivesBySecret(parts[part]),
            fixedReason: "received messages are verified by it, so an instance administrator sets it in Tools"
        }
    ]
    if (!gateway)
        secrets.push({
            name: "sendSharedSecret",
            part,
            label: prefix + "Shared secret for sending",
            help: "Only when it differs from the one received messages are verified by"
        })
    secrets.push({name: "privateKey", part, label: prefix + "Private key", fixed: true})
    return {fields, secrets}
}
const receivesBySecret = remote => /^(SmatHmac|SmatUrlToken)/.test(text(remote?.messageAuthEnumId))
function remoteIncomplete(remote) {
    if (!remote || !text(remote.sendUrl)) return null
    const auth = authOf(remote)
    if (auth === "SmatNone") return null
    if (auth === "SmatLogin" && !text(remote.username)) return "Give the user the remote signs in"
    if ((auth === "" || auth === "SmatLogin") && !text(remote.password))
        return "Give the password or token the remote signs in with"
    if (
        auth.startsWith("SmatHmac") &&
        (!text(remote.authHeaderName) || !(text(remote.sendSharedSecret) || text(remote.sharedSecret)))
    )
        return "Give the signature header and the shared secret messages are signed with"
    return null
}
// A remote that is changed here posts over http(s): another transport (SFTP, a local or custom one) is Tools'. own: the
// remote kind itself, where a remote's use is not known from a gateway that names it: one with a send service of its own,
// or of a message type that names a file path or a send service (the backup mirror's, an EDI partner's), reads its
// address and credentials its own way, and repointing it could send what it carries (a database backup) elsewhere.
function remoteReadOnly(remote, own = false) {
    const scheme = /^([a-z][a-z0-9+.-]*):/i.exec(text(remote?.sendUrl))
    if (text(remote?.sendUrl) && (!scheme || !["http", "https"].includes(scheme[1].toLowerCase())))
        return "This remote does not post over https (SFTP or a transport of its own): an instance administrator changes it in Tools"
    if (/^sftp:/i.test(text(remote?.receiveUrl)))
        return "This remote receives over SFTP: an instance administrator changes it in Tools"
    if (!own || !remote) return null
    const elsewhere = "it reads its address and credentials its own way, and an instance administrator changes it in Tools"
    if (text(remote.sendServiceName) && text(remote.sendServiceName) !== REST_SEND)
        return "This remote has a send service of its own: " + elsewhere
    const type = text(remote.systemMessageTypeId)
        ? one("moqui.service.message.SystemMessageType", {systemMessageTypeId: remote.systemMessageTypeId})
        : null
    if (
        type &&
        (text(type.sendPath) ||
            text(type.receivePath) ||
            (text(type.sendServiceName) && text(type.sendServiceName) !== REST_SEND))
    )
        return (
            "This remote is of a message type with a transport of its own (" +
            (type.description || type.systemMessageTypeId) +
            "): " +
            elsewhere
        )
    return null
}
function remoteTest(remote, context) {
    const url = text(remote?.sendUrl)
    if (!url) return [{check: "Address", ok: true, text: "No sending address: nothing is sent to this remote"}]
    // up to the first name the sender fills in
    const target = url.split("${")[0]
    return [Object.assign({check: "Address"}, probe(target, context.denyInternal))]
}
const remoteDefinition = (() => {
    const shared = remoteFields()
    return {
        adapterKey: "remote",
        parts: {main: {entity: REMOTE, keyField: "systemMessageRemoteId"}},
        statePart: "main",
        fields: [{name: "description", label: "Name", kind: "text"}].concat(shared.fields),
        secrets: shared.secrets,
        name: parts => parts.main.description,
        providerName: () => "Remote system",
        summary: parts => hostOf(parts.main.sendUrl),
        listKeys: () => allKeys(REMOTE, "systemMessageRemoteId"),
        readOnly: parts => remoteReadOnly(parts.main, true),
        incomplete: parts => remoteIncomplete(parts.main),
        test: (parts, context) => remoteTest(parts.main, context),
        newParts: values => {
            const systemMessageRemoteId = text(values.connectionId)
            if (!/^[A-Za-z][A-Za-z0-9_]{2,39}$/.test(systemMessageRemoteId))
                throw new Error("A remote's id is 3 to 40 letters, digits or underscores, starting with a letter")
            if (!plainText(values.description, "The name")) throw new Error("Give the remote a name")
            // what it receives needs a signed-in user authorized to receive system messages, never a secret set here
            return {connectionKey: systemMessageRemoteId, parts: {main: {systemMessageRemoteId, messageAuthEnumId: "SmatLogin"}}}
        },
        consumers: (parts, key) => {
            const waiting = count("moqui.service.message.SystemMessage", {
                systemMessageRemoteId: key,
                statusId: "SmsgProduced",
                isOutgoing: "Y"
            })
            const failed = count("moqui.service.message.SystemMessage", {systemMessageRemoteId: key, statusId: "SmsgError"})
            const found = [
                {
                    ownerName: "System messages",
                    recordType: "SystemMessage",
                    recordId: null,
                    recordName: waiting + " waiting to be sent, " + failed + " in error",
                    use: "Messages to and from this remote",
                    state: failed ? "attention" : "ok"
                }
            ]
            for (const row of rows(REMOTE, {preAuthMessageRemoteId: key}))
                found.push({
                    ownerName: "Remote systems",
                    recordType: "SystemMessageRemote",
                    recordId: row.systemMessageRemoteId,
                    recordName: row.description || row.systemMessageRemoteId,
                    use: "Signs in through this remote first",
                    state: "ok"
                })
            return found
        }
    }
})()

// A relay is an EmailServer no EmailAccount owns (design §9): the server of a mail account carries the account's password
// and token and is the account's, refused here on every path.
const accountOwned = emailServerId => count(MAIL_ACCOUNT, {emailServerId, ownsEmailServer: "Y"}) > 0
const smtpUserOf = server => text(server.smtpUsername) || text(server.mailUsername)
const relayDefinition = {
    adapterKey: "relay",
    parts: {main: {entity: SERVER, keyField: "emailServerId"}},
    statePart: "main",
    fields: [
        {name: "smtpHost", label: "SMTP host", kind: "host", endpoint: true, audited: true},
        {
            name: "smtpPort",
            label: "Port",
            kind: "port",
            endpoint: true,
            help: "465 for TLS from the start, 587 for STARTTLS; empty is 25"
        },
        {name: "smtpSsl", label: "TLS from the start", kind: "flag", endpoint: true},
        {name: "smtpStartTls", label: "STARTTLS", kind: "flag", endpoint: true},
        {
            name: "mailUsername",
            label: "User",
            kind: "text",
            endpoint: true,
            audited: true,
            help: "The user the relay signs in; empty for none"
        },
        {
            name: "smtpUsername",
            label: "Separate SMTP user",
            kind: "text",
            endpoint: true,
            audited: true,
            help: "Only when sending signs in as another user; with its own password below"
        },
        {
            name: "allowedToDomains",
            label: "Allowed recipient domains",
            kind: "text",
            audited: true,
            maxLength: 2000,
            validate: value =>
                patternText(
                    value,
                    "The allowed domains",
                    /^[A-Za-z0-9.-]+(\s*,\s*[A-Za-z0-9.-]+)*$/,
                    "are domain names separated by commas",
                    2000
                ),
            help: "Comma separated; empty allows every domain"
        },
        {
            name: "webBaseUrl",
            label: "Link base URL",
            kind: "url",
            audited: true,
            help: "The base of links in mail sent through this relay"
        },
        {
            name: "authTypeName",
            label: "Sign-in",
            kind: "display",
            read: parts => (parts.main.authTypeEnumId === "EatOAuth2" ? "OAuth 2.0" : "Password")
        }
    ],
    secrets: [
        {name: "mailPassword", label: "Password"},
        {name: "smtpPassword", label: "Separate SMTP password", help: "Only with a separate SMTP user"}
    ],
    name: parts => parts.main.emailServerId,
    providerName: () => "Mail relay",
    summary: parts =>
        text(parts.main.smtpHost)
            ? parts.main.smtpHost +
              (text(parts.main.smtpPort) ? ":" + parts.main.smtpPort : "") +
              (parts.main.smtpSsl === "Y" ? " · TLS" : parts.main.smtpStartTls === "Y" ? " · STARTTLS" : " · no TLS")
            : "",
    listKeys: () => allKeys(SERVER, "emailServerId"),
    hidden: parts =>
        accountOwned(parts.main.emailServerId)
            ? refused(
                  "ACCOUNT_OWNED",
                  "This mail server belongs to a mail account: it is changed in the account's own settings, not here"
              )
            : null,
    readOnly: parts =>
        text(parts.main.smtpHost).toLowerCase() === "edge:"
            ? "Mail goes out through the edge node: its setup changes this relay"
            : parts.main.authTypeEnumId === "EatOAuth2"
              ? "This server signs in with OAuth: it is set up where its account signs in"
              : null,
    incomplete: parts => {
        const server = parts.main
        if (!text(server.smtpHost)) return "Give the SMTP host"
        const password = text(server.smtpUsername)
            ? text(server.smtpPassword) || text(server.mailPassword)
            : text(server.mailPassword)
        return smtpUserOf(server) && !password ? "Give the password of the user the relay signs in" : null
    },
    check: after => {
        const server = after.main
        if ((text(server.mailPassword) || text(server.smtpPassword)) && server.smtpSsl !== "Y" && server.smtpStartTls !== "Y")
            return refused("INSECURE_TRANSPORT", "A relay that signs in needs TLS: choose TLS from the start or STARTTLS")
        return null
    },
    test: (parts, context) => {
        const place = parts.main.smtpHost + (text(parts.main.smtpPort) ? ":" + parts.main.smtpPort : "")
        const found = []
        ec.message.pushErrors()
        let result = {}
        let failure = ""
        try {
            result =
                ec.service
                    .sync()
                    .name("org.moqui.mail.EmailServices.test#EmailServer")
                    .parameters({emailServerId: parts.main.emailServerId, testSmtp: true, testStore: false})
                    .call() || {}
            if (ec.message.hasError()) failure = ec.message.getErrorsString()
        } catch (error) {
            failure = String(error?.message ?? error)
        } finally {
            ec.message.clearErrors()
            ec.message.popErrors()
        }
        const step = Array.from(result.stepList || []).find(row => row.step === "smtp")
        if (failure || !step) found.push({check: "Connection", ok: false, text: failureText(place, failure || "not available")})
        else if (step.ok)
            found.push({
                check: "Connection",
                ok: true,
                text: "Connected to " + place + (smtpUserOf(parts.main) ? " and signed in" : "")
            })
        else found.push({check: "Connection", ok: false, text: failureText(place, step.message)})
        // the named test message: one fixed message to the administrator's own address, only once the connection holds
        if (context.testKind === "send" && found.every(row => row.ok)) found.push(sendTestMessage(parts.main))
        return found
    },
    newParts: values => {
        const emailServerId = text(values.connectionId)
        if (!/^[A-Za-z][A-Za-z0-9_]{2,39}$/.test(emailServerId))
            throw new Error("A relay's id is 3 to 40 letters, digits or underscores, starting with a letter")
        return {connectionKey: emailServerId, parts: {main: {emailServerId, authTypeEnumId: "EatPassword"}}}
    },
    consumers: (parts, key) => {
        const found = Array.from(
            find("moqui.basic.email.EmailTemplate", {emailServerId: key}).orderBy("emailTemplateId").limit(100).list()
        ).map(row => ({
            ownerName: "Email templates",
            recordType: "EmailTemplate",
            recordId: row.emailTemplateId,
            recordName: row.description || row.emailTemplateId,
            use: "Sent through this relay",
            state: "ok"
        }))
        const accounts = count(MAIL_ACCOUNT, {emailServerId: key})
        if (accounts)
            found.push({
                ownerName: "Mail accounts",
                recordType: "EmailAccount",
                recordId: null,
                recordName: accounts + (accounts === 1 ? " account" : " accounts"),
                use: "Send through this relay without owning it",
                state: "ok"
            })
        return found
    }
}
// One fixed message from and to the administrator's own address through the relay: the relay must be enabled (a
// disabled server sends nothing), and the result is the delivery's kind, never the server's words.
function sendTestMessage(server) {
    const account = one(ACCOUNT, {userId: ec.user.userId})
    const address = text(account?.emailAddress)
    if (!/^[^\s@<>,;]+@[^\s@<>,;]+$/.test(address))
        return {check: "Message", ok: false, text: "Your account has no email address to send the test message to"}
    if (server.disabled === "Y")
        return {
            check: "Message",
            ok: false,
            text: "The relay is disabled: a test message is sent once the connection test has enabled it"
        }
    let failure = ""
    let sent = {}
    ec.message.pushErrors()
    try {
        const created =
            ec.service
                .sync()
                .name("create#moqui.basic.email.EmailMessage")
                // each in a transaction of its own: a message that is not sent is this test's answer, not its failure
                .requireNewTransaction(true)
                .parameters({
                    statusId: "ES_READY",
                    emailServerId: server.emailServerId,
                    fromAddress: /@/.test(text(server.mailUsername)) ? text(server.mailUsername) : address,
                    toAddresses: address,
                    subject: "Test message from the mail relay " + server.emailServerId,
                    bodyText:
                        "This message was sent from Admin to test the mail relay " +
                        server.emailServerId +
                        ". Nothing else is needed.",
                    fromUserId: ec.user.userId
                })
                .call() || {}
        sent =
            ec.service
                .sync()
                .name("org.moqui.mail.EmailServices.send#EmailMessage")
                .requireNewTransaction(true)
                .parameters({emailMessageId: created.emailMessageId})
                .call() || {}
        if (ec.message.hasError()) failure = ec.message.getErrorsString()
    } catch (error) {
        failure = String(error?.message ?? error)
    } finally {
        ec.message.clearErrors()
        ec.message.popErrors()
    }
    if (failure || text(sent.sendError))
        return {
            check: "Message",
            ok: false,
            text: failureText("The relay", failure || sent.sendError) + ": the test message was not sent"
        }
    return {check: "Message", ok: true, text: "A test message was sent to your own address"}
}

const oauthConfigured = provider => {
    const prefix = text(provider.propertyPrefix)
    return !!prefix && !!text(System.getProperty(prefix + "_client_id")) && !!text(System.getProperty(prefix + "_client_secret"))
}
const mailOAuthDefinition = {
    adapterKey: "mailOAuth",
    parts: {main: {entity: OAUTH, keyField: "oauthProviderEnumId"}},
    statePart: "main",
    // its endpoints are the provider's own (seed data, Tools'): nothing here names an address the server reaches
    privateNetwork: false,
    fields: [
        {
            name: "clientId",
            label: "Client ID",
            kind: "text",
            endpoint: true,
            help: "The OAuth client registered at the provider for this deployment"
        },
        {
            name: "tenant",
            label: "Tenant",
            kind: "text",
            endpoint: true,
            validate: value => patternText(value, "The tenant", /^[A-Za-z0-9.-]{1,80}$/, "is a name or an id", 80),
            help: "Microsoft: the directory, or common"
        },
        {name: "authorizeUrl", label: "Authorization endpoint", kind: "display"},
        {name: "tokenUrl", label: "Token endpoint", kind: "display"},
        {
            name: "clientSource",
            label: "Client in use",
            kind: "display",
            read: parts =>
                oauthConfigured(parts.main)
                    ? "The deployment's configuration (it wins over what is entered here)"
                    : "As entered here"
        }
    ],
    secrets: [{name: "clientSecret", label: "Client secret"}],
    name: parts =>
        one("moqui.basic.Enumeration", {enumId: parts.main.oauthProviderEnumId})?.description || parts.main.oauthProviderEnumId,
    providerName: () => "Mail sign-in provider",
    summary: parts => hostOf(parts.main.authorizeUrl),
    listKeys: () => allKeys(OAUTH, "oauthProviderEnumId"),
    incomplete: parts =>
        oauthConfigured(parts.main) || (text(parts.main.clientId) && text(parts.main.clientSecret))
            ? null
            : "Give the client ID and the client secret registered at the provider",
    // no request: a token is asked for only when a person signs a mail account in
    test: () => [
        {check: "Client", ok: true, text: "The client is configured; the provider proves it when a mail account signs in"}
    ],
    consumers: (parts, key) => {
        const servers = count(SERVER, {oauthProviderEnumId: key})
        return [
            {
                ownerName: "Mail accounts",
                recordType: "EmailAccount",
                recordId: null,
                recordName: servers + (servers === 1 ? " account" : " accounts"),
                use: "Sign in with this provider",
                state: "ok"
            }
        ]
    }
}

const printerDefinition = {
    adapterKey: "printer",
    parts: {main: {entity: PRINTER, keyField: "networkPrinterId"}},
    statePart: "main",
    fields: [
        {name: "description", label: "Name", kind: "text"},
        {name: "location", label: "Location", kind: "text"},
        {name: "serverHost", label: "Print server", kind: "host", endpoint: true},
        {name: "serverPort", label: "Port", kind: "port", endpoint: true, help: "Empty is 631"},
        {
            name: "printerName",
            label: "Printer on the server",
            kind: "text",
            endpoint: true,
            validate: value =>
                patternText(
                    value,
                    "The printer's name",
                    /^[A-Za-z0-9_.-]{1,127}$/,
                    "is letters, digits, dots, dashes and underscores",
                    127
                ),
            help: "Empty for the server's default printer"
        }
    ],
    secrets: [],
    name: parts => parts.main.description || parts.main.printerName || parts.main.networkPrinterId,
    providerName: () => "Network printer",
    summary: parts => text(parts.main.serverHost) + (text(parts.main.printerName) ? " · " + parts.main.printerName : ""),
    listKeys: () => allKeys(PRINTER, "networkPrinterId"),
    readOnly: parts =>
        /[:\/]/.test(text(parts.main.serverHost))
            ? "This printer's server names a scheme or a port of its own: an instance administrator changes it in Tools"
            : null,
    incomplete: parts => (text(parts.main.serverHost) ? null : "Give the print server"),
    // a fixed page through print#Document, sent also to a printer that waits for this test
    test: parts => {
        const place = parts.main.serverHost
        let failure = ""
        let result = {}
        ec.message.pushErrors()
        try {
            result =
                ec.service
                    .sync()
                    .name("org.moqui.impl.PrintServices.print#Document")
                    // in a transaction of its own: a page that is not taken is this test's answer, not its failure
                    .requireNewTransaction(true)
                    .parameters({
                        networkPrinterId: parts.main.networkPrinterId,
                        connectionTest: true,
                        storeDocument: false,
                        contentType: "text/plain",
                        jobName: "Connection test",
                        inputStream:
                            "Test page\n\nThis page was printed from Admin to test the printer " +
                            parts.main.networkPrinterId +
                            ".\n"
                    })
                    .call() || {}
            if (ec.message.hasError()) failure = ec.message.getErrorsString()
        } catch (error) {
            failure = String(error?.message ?? error)
        } finally {
            ec.message.clearErrors()
            ec.message.popErrors()
        }
        if (failure)
            return [
                {
                    check: "Test page",
                    ok: false,
                    text: /not found with name/i.test(failure)
                        ? place + " has no printer of that name"
                        : /Could not reach|internal address|not installed|sim session/i.test(failure)
                          ? failureText(place, failure)
                          : place + " did not take the test page"
                }
            ]
        return [
            {
                check: "Test page",
                ok: true,
                text: place + " took the test page" + (result.jobId != null ? " (job " + Number(result.jobId) + ")" : "")
            }
        ]
    },
    newParts: values => {
        if (!plainText(values.description, "The name")) throw new Error("Give the printer a name")
        let networkPrinterId = text(values.connectionId)
        if (!networkPrinterId) {
            const value = ec.entity.makeValue(PRINTER)
            value.setSequencedIdPrimary()
            networkPrinterId = String(value.networkPrinterId)
        }
        if (!/^[A-Za-z0-9][A-Za-z0-9_]{0,39}$/.test(networkPrinterId))
            throw new Error("A printer's id is up to 40 letters, digits or underscores")
        return {connectionKey: networkPrinterId, parts: {main: {networkPrinterId}}}
    },
    consumers: (parts, key) => {
        const failed = count("moqui.basic.print.PrintJob", {networkPrinterId: key, statusId: "PtjSendFailed"})
        const jobs = count("moqui.basic.print.PrintJob", {networkPrinterId: key})
        return [
            {
                ownerName: "Print jobs",
                recordType: "PrintJob",
                recordId: null,
                recordName: jobs + (jobs === 1 ? " job" : " jobs") + ", " + failed + " not sent",
                use: "Documents sent to this printer",
                state: failed ? "attention" : "ok"
            }
        ]
    }
}
const builtIn = {
    remote: engine(remoteDefinition),
    relay: engine(relayDefinition),
    mailOAuth: engine(mailOAuthDefinition),
    printer: engine(printerDefinition)
}

// ===== the registry and the services =====
// The registered kinds, in order: a row that names a service which does not exist is logged by name and left out, as is a
// row with no service whose key is not one of the framework's own.
function adapters() {
    const found = []
    for (const row of rows(ADAPTER).map(map)) {
        if (row.disabled === "Y") continue
        const serviceName = text(row.adapterServiceName)
        if (serviceName ? !ec.service.getServiceDefinition(serviceName) : !Object.hasOwn(builtIn, row.adapterKey)) {
            ec.logger.error(
                "Connection adapter " + row.adapterKey + " names a missing service " + (serviceName || "(none)") + ": left out"
            )
            continue
        }
        found.push({
            adapterKey: row.adapterKey,
            title: row.title || row.adapterKey,
            description: row.description || "",
            adapterGroup: row.adapterGroup || "integration",
            adapterServiceName: serviceName,
            creatable: row.creatable === "Y",
            organizationBinding: row.organizationBinding === "Y",
            sequenceNum: row.sequenceNum ?? 999
        })
    }
    return found.sort((a, b) => a.sequenceNum - b.sequenceNum || a.title.localeCompare(b.title))
}
function adapterOf(adapterKey) {
    const key = text(adapterKey)
    const adapter = adapters().find(row => row.adapterKey === key)
    if (!adapter) throw new Error("No connection kind " + (key || "(none)") + " is registered")
    return adapter
}
const shown = adapter => ({
    adapterKey: adapter.adapterKey,
    title: adapter.title,
    description: adapter.description,
    adapterGroup: adapter.adapterGroup,
    creatable: adapter.creatable,
    organizationBinding: adapter.organizationBinding
})
// One operation of a kind: the framework's own here, a component's through its service.
function run(adapter, operation, p) {
    const parameters = Object.assign({}, p, {adapterKey: adapter.adapterKey})
    if (!adapter.adapterServiceName) return builtIn[adapter.adapterKey].run(operation, parameters)
    const out =
        ec.service
            .sync()
            .name(adapter.adapterServiceName)
            .parameters({
                operation,
                adapterKey: adapter.adapterKey,
                connectionKey: parameters.connectionKey ?? null,
                values: parameters.values ?? null,
                expectedRevision: parameters.expectedRevision ?? null,
                enabled: parameters.enabled ?? null,
                reason: parameters.reason ?? null,
                testKind: parameters.testKind ?? null,
                organizationPartyId: parameters.organizationPartyId ?? null
            })
            .call() || {}
    return ownNames(out.result)
}
function requireRead() {
    lib.requireView(VIEW, MANAGE)
}

// find#Connections: the connections of every registered kind (or one, or one group), each with its name, kind, endpoint
// summary (a host, never a path or a credential), state and last test; narrowed by text, state and organization.
function findConnections(p) {
    requireRead()
    const all = adapters()
    const wanted = all
        .filter(adapter => !text(p.adapterKey) || adapter.adapterKey === text(p.adapterKey))
        .filter(adapter => !text(p.adapterGroup) || adapter.adapterGroup === text(p.adapterGroup))
        .filter(adapter => !text(p.organizationPartyId) || adapter.organizationBinding)
    const found = []
    // the provider types a kind is started as, which its list answers (a gateway is Authorize.Net, PayTrace or Shippo)
    const typeOptions = new Map()
    for (const adapter of wanted) {
        // one kind that fails does not take the list with it
        try {
            const listed = run(adapter, "list", {organizationPartyId: p.organizationPartyId})
            typeOptions.set(
                adapter.adapterKey,
                Array.from(listed.typeOptions || []).map(option => ({value: text(option.value), label: text(option.label)}))
            )
            for (const row of Array.from(listed.connectionList || []))
                found.push(Object.assign({}, ownNames(row), {adapterKey: adapter.adapterKey, adapterTitle: adapter.title}))
        } catch (error) {
            ec.message.clearErrors()
            ec.logger.error(
                "Connection adapter " + adapter.adapterKey + " could not list its connections: " + (error?.message ?? error)
            )
        }
    }
    const term = text(p.text).toLowerCase()
    const state = text(p.state)
    const filtered = found
        .filter(
            row =>
                !term ||
                [row.connectionKey, row.name, row.providerName, row.endpointSummary].some(value =>
                    String(value || "")
                        .toLowerCase()
                        .includes(term)
                )
        )
        .filter(
            row =>
                !state ||
                (state === "enabled" && row.enabled) ||
                (state === "disabled" && !row.enabled) ||
                (state === "attention" && ["needsTest", "incomplete", "untested"].includes(row.state))
        )
        .sort(
            (a, b) => String(a.adapterTitle).localeCompare(String(b.adapterTitle)) || String(a.name).localeCompare(String(b.name))
        )
    const paged = lib.page(filtered, p)
    return {
        connectionList: paged.rows,
        connectionListCount: paged.count,
        adapterList: all
            .filter(adapter => !text(p.adapterGroup) || adapter.adapterGroup === text(p.adapterGroup))
            .map(adapter => Object.assign(shown(adapter), {typeOptions: typeOptions.get(adapter.adapterKey) || []})),
        canManage: has(MANAGE)
    }
}
function getConnection(p) {
    requireRead()
    const adapter = adapterOf(p.adapterKey)
    const connection = run(adapter, "get", {connectionKey: p.connectionKey}).connection
    return {
        connection: connection ? Object.assign(ownNames(connection), {adapterTitle: adapter.title}) : null,
        adapter: shown(adapter),
        canManage: has(MANAGE)
    }
}
function storeConnection(p) {
    lib.requireManage(MANAGE)
    const adapter = adapterOf(p.adapterKey)
    if (!text(p.connectionKey) && !adapter.creatable)
        return refused("NOT_CREATABLE", "A " + adapter.title.toLowerCase() + " is not started here")
    return run(adapter, "store", {
        connectionKey: p.connectionKey,
        values: ownNames(p.values),
        expectedRevision: p.expectedRevision,
        enabled: p.enabled,
        reason: p.reason
    })
}
function testConnection(p) {
    lib.requireManage(MANAGE)
    return run(adapterOf(p.adapterKey), "test", {connectionKey: p.connectionKey, testKind: p.testKind})
}
// find#ConnectionConsumers: where a connection is used: the kind's own answer, then each registered provider's (the owner
// of records that name it). A provider that fails is logged and passed over.
function findConnectionConsumers(p) {
    requireRead()
    const adapter = adapterOf(p.adapterKey)
    const connectionKey = text(p.connectionKey)
    const found = Array.from(run(adapter, "consumers", {connectionKey}).consumerList || []).map(ownNames)
    for (const row of rows(PROVIDER, {adapterKey: adapter.adapterKey})
        .map(map)
        .sort((a, b) => (a.sequenceNum ?? 999) - (b.sequenceNum ?? 999))) {
        if (row.disabled === "Y") continue
        if (!ec.service.getServiceDefinition(text(row.serviceName))) {
            ec.logger.error(
                "Connection consumer provider " + row.providerKey + " names a missing service " + row.serviceName + ": left out"
            )
            continue
        }
        try {
            const out =
                ec.service.sync().name(row.serviceName).parameters({adapterKey: adapter.adapterKey, connectionKey}).call() || {}
            for (const consumer of Array.from(out.consumerList || [])) found.push(ownNames(consumer))
        } catch (error) {
            ec.message.clearErrors()
            ec.logger.error("Connection consumer provider " + row.providerKey + " failed: " + (error?.message ?? error))
        }
    }
    return {consumerList: found.slice(0, 500)}
}

const operations = {findConnections, getConnection, storeConnection, testConnection, findConnectionConsumers}
;({
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown connection operation " + op)
        return operations[op](parameters)
    },
    // for a component's adapter service (Mantle's gateways): the engine, the remote's fields and checks, and the
    // validators, so every kind holds to the same rules
    lib: {
        engine,
        remoteFields,
        remoteIncomplete,
        remoteReadOnly,
        remoteTest,
        probe,
        hostOf,
        failureText,
        hostText,
        portText,
        numberText,
        patternText,
        enumOptions,
        VIEW,
        MANAGE,
        REMOTE
    }
})
