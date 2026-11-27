// The endpoint-change rule (ConfigurationConnections.js, mr-boss plan/design-admin.md §8.1) for a record that keeps a secret
// beside the address it is sent to, on every write path (plans/archive/Security-Review-Low.md SEC-TLS-116): the Security.eecas.xml
// rules *SecretMoves of LlmEndpoint (apiKey), EntitySync (targetPassword), SystemMessageRemote (password, the shared
// secrets, privateKey), ScrapeTarget (bearerToken), EmailServer (the passwords and the OAuth refresh token),
// AgentMcpServer and AgentRemoteA2a (authJson).
//
// moved(entityValue, originalValue, rule), run before an update. rule: {endpoints: the fields the secrets are sent to or
// with (a URL, a host, a port, a user, an auth header), secrets: the secret fields, admin: a change is ADMIN's only,
// origin: a URL counts as changed only when its scheme, host or port does, as the store service of the record decides,
// passThrough: an address that is no network host (the edge node's "edge:"), to or from which a change keeps the secrets}.
// When an endpoint field given in the write differs from the stored row, the change is refused outside ADMIN (admin), and
// each stored secret is cleared unless the write's caller gives one with it, with a message saying so. The same secret
// entered again with the new address is given (entityValue.isFieldGiven: the caller set it); the stored one an update
// carries over is not, though the rule's value holds it: an update of an entity with an audit-logged field fills in the
// whole stored row, and a value read and written back holds what it read. Only a
// seed or install load (an entity data load that is not a request's: UserSecurityChecks.seedLoadRunning) answers for
// itself; a Tools load or an entity sync, which restrict sensitive entities, and a service with authorization disabled
// are held to the rule too.
function moved(entityValue, originalValue, rule) {
    if (originalValue == null || UserSecurityChecks.seedLoadRunning()) return
    const plain = value => (value == null ? "" : String(value))
    const originOf = text => {
        const match = /^([a-z][a-z0-9+.-]*):\/\/([^/?#]*)/i.exec(text.trim())
        return match ? (match[1] + "://" + match[2]).toLowerCase() : text.trim()
    }
    const passThrough = rule.passThrough || []
    const changed = rule.endpoints.filter(name => {
        if (!entityValue.isFieldSet(name)) return false
        const before = plain(originalValue.get(name))
        const after = plain(entityValue.get(name))
        if (passThrough.includes(before) || passThrough.includes(after)) return false
        return rule.origin ? originOf(before) !== originOf(after) : before !== after
    })
    if (!changed.length) return
    if (rule.admin && !ec.user.isInGroup("ADMIN"))
        throw new Error("Changing " + changed.join(" or ") + " sends the stored secret to another address: an administrator (ADMIN) changes it")
    const cleared = []
    for (const name of rule.secrets) {
        const stored = plain(originalValue.get(name))
        const given = entityValue.isFieldGiven(name) ? plain(entityValue.get(name)) : ""
        if (stored !== "" && given === "") {
            entityValue.set(name, null)
            cleared.push(name)
        }
    }
    if (cleared.length)
        ec.message.addMessage(ec.l10n.localize("The stored secret was cleared because the address changed; enter it again") +
            " (" + cleared.join(", ") + ")", "warning")
}

;({moved})
