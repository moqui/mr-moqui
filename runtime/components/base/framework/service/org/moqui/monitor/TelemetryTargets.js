// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// The scrape targets and synthetic checks of System › Monitor › Checks: store#ScrapeTarget and store#SyntheticCheck of
// org.moqui.monitor.TelemetryServices. A target or check reaches only public addresses unless its allowPrivateNetwork is
// Y (on an instance that is not production, also when it is empty), and only a holder of MONITOR_PRIVATE_NETWORK sets Y
// or changes the URL of a row that has it: the request is the server's own, so the row's author would otherwise read
// loopback and private services with it (SEC-OUT-03, SEC-TLS-13). A link-local or metadata address is refused always,
// by the framework's HTTP client.

const PERMISSION = 'MONITOR_PRIVATE_NETWORK'

const KINDS = {
    target: { entity: 'moqui.monitor.ScrapeTarget', key: 'targetId',
        fields: ['url', 'serviceName', 'intervalSeconds', 'timeoutSeconds', 'bearerToken', 'enabled'] },
    check: { entity: 'moqui.monitor.SyntheticCheck', key: 'checkId',
        fields: ['name', 'url', 'method', 'expectStatus', 'expectBodyContains', 'intervalSeconds', 'timeoutSeconds', 'enabled'] },
}

/** Create or update a target or check. Without MONITOR_PRIVATE_NETWORK, allowPrivateNetwork Y is refused, and so is a
 * new URL on a row that has it. */
function store(kind, p) {
    const def = KINDS[kind]
    const id = p[def.key]
    if (id == null || String(id).trim() === '') {
        ec.message.addError(def.key + ' is required')
        return {}
    }
    const existing = ec.entity.find(def.entity).condition(def.key, id).useCache(false).one()
    const flag = p.allowPrivateNetwork == null ? null : String(p.allowPrivateNetwork)
    const before = existing ? (existing.allowPrivateNetwork || '') : ''
    if (!ec.user.hasPermission(PERMISSION)) {
        const raising = flag === 'Y' && before !== 'Y'
        const moving = before === 'Y' && p.url != null && String(p.url) !== String(existing.url || '')
        if (raising || moving) {
            ec.message.addError(ec.l10n.localize(raising
                ? 'Reaching the private network needs the MONITOR_PRIVATE_NETWORK permission'
                : 'Only a holder of MONITOR_PRIVATE_NETWORK changes the URL of a row that may reach the private network'))
            return {}
        }
    }
    const fields = {}
    fields[def.key] = id
    for (const name of def.fields) if (p[name] != null) fields[name] = p[name]
    if (flag != null) fields.allowPrivateNetwork = flag === 'Y' ? 'Y' : flag === 'N' ? 'N' : null
    if (existing) {
        ec.service.sync().name('update#' + def.entity).parameters(fields).call()
    } else {
        if (fields.enabled == null) fields.enabled = 'Y'
        ec.service.sync().name('create#' + def.entity).parameters(fields).call()
    }
    const out = {}
    out[def.key] = id
    return out
}

function storeTarget(p) { return store('target', p) }
function storeCheck(p) { return store('check', p) }
