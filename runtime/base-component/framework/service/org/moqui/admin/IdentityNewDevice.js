// The new-device sign-in notice of org.moqui.admin.IdentityServices (mr-boss plan/design-admin.md §4 Sessions, §16, §19 Q6;
// impl-admin P10, F15): notify#NewDeviceSignIn, called once a successful sign-in's UserLoginHistory row is written (the EECA
// UserLoginHistoryNewDevice, after the sign-in itself has committed), tells the account's owner of a sign-in from a client
// the account was not seen from within the window.
//
// A client is a browser family and system with a network: the summary Sessions shows (never the user agent) and the
// address's network, an IPv4 /24 or an IPv6 /48, which is all of the address the notice shows. It is known when a visit of
// the account in the window had the same one: the visits its login history names (UserLoginHistory has no device of its
// own, F15) and the account's other visits. The window is the instance preference signin.newDeviceWindowDays (90 days,
// ConfigurationServices). An account with no earlier sign-in at all gets no notice: its first sign-in is its owner
// setting it up.
//
// The notice goes to the account alone, on the private topic AccountSignIn (kept in the notification list, and by email
// through the topic's template), with the time, the client, the network and the session's key; the topic's link opens
// where the owner signs that session out (an app sets it: the framework has no account screen of its own). A failure is
// logged and never fails the sign-in or its history row.
const identity = ec.resource.script("component://framework/service/org/moqui/admin/Identity.js")
const settings = ec.resource.script("component://framework/service/org/moqui/admin/ConfigurationSettings.js")
const VISIT = "moqui.server.Visit"
const HISTORY = "moqui.security.UserLoginHistory"
const TOPIC = "AccountSignIn"
const DAY = 86400000
// the most visits of the window compared: a sign-in checks its client against recent ones, not a whole history
const VISIT_LIMIT = 500
const text = value => String(value ?? "").trim()
const ms = value =>
    value == null || value === "" ? null : value instanceof Date ? value.getTime() : Number(value) || Date.parse(String(value))
const find = (entity, conditions = {}) => ec.entity.find(entity).condition(conditions).disableAuthz()

// The network of an address: the first three parts of an IPv4 address, the first three groups of an IPv6 one (zeros
// restored), as it is shown; "" for none.
function network(address) {
    const value = text(address).replace(/^\[|\]$/g, "")
    if (!value) return ""
    const v4 = /(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.\d{1,3}$/.exec(value)
    if (v4) return v4[1] + "." + v4[2] + "." + v4[3] + ".x"
    if (!value.includes(":")) return ""
    const sides = value.split("%")[0].split("::")
    const head = sides[0] ? sides[0].split(":") : []
    const tail = sides.length > 1 && sides[1] ? sides[1].split(":") : []
    const groups = sides.length > 1 ? head.concat(Array(Math.max(8 - head.length - tail.length, 0)).fill("0"), tail) : head
    return (
        groups
            .slice(0, 3)
            .map(group => (group || "0").toLowerCase().replace(/^0+(?=.)/, ""))
            .join(":") + "::x"
    )
}
// What tells one client from another: the summary of its user agent and its network.
const clientKey = visit => identity.clientSummary(visit.initialUserAgent) + "|" + network(visit.clientIpAddress)
function windowDays() {
    try {
        const days = Number(settings.value("signin.newDeviceWindowDays"))
        if (Number.isFinite(days) && days >= 1) return days
    } catch (error) {
        ec.logger.error("The new-device window could not be read, 90 days used: " + error.message)
    }
    return 90
}

// notify#NewDeviceSignIn: one notice for a sign-in from a client the account was not seen from in the window; none for a
// known client, a first sign-in, or a sign-in with no visit or client to tell.
function notifyNewDeviceSignIn(p) {
    const userId = text(p.userId)
    const visitId = text(p.visitId)
    const none = reasonCode => ({notified: false, reasonCode})
    if (!userId || !visitId) return none("NO_VISIT")
    const visit = find(VISIT, {visitId}).one()
    // the visit is the account's own: a sign-in on it, or its history row names it
    if (!visit || (visit.userId && visit.userId !== userId)) return none("NO_VISIT")
    if (!text(visit.initialUserAgent) && !text(visit.clientIpAddress)) return none("NO_CLIENT")
    const account = find("moqui.security.UserAccount", {userId}).one()
    if (!account) return none("NO_ACCOUNT")
    const at = ms(p.fromDate) ?? ms(ec.user.nowTimestamp)
    const since = new Date(at - windowDays() * DAY).toISOString()
    // the visits of the window: those the login history names, and the account's own
    const visitIds = new Set()
    const history = Array.from(
        find(HISTORY, {userId, successfulLogin: "Y"})
            .condition("fromDate", ">=", since)
            .orderBy("-fromDate")
            .limit(VISIT_LIMIT)
            .list()
    )
    for (const row of history) if (row.visitId && row.visitId !== visitId) visitIds.add(row.visitId)
    const earlier = Array.from(
        find(VISIT, {userId}).condition("fromDate", ">=", since).orderBy("-fromDate").limit(VISIT_LIMIT).list()
    )
    const known = new Set()
    for (const row of earlier) {
        if (row.visitId === visitId) continue
        visitIds.delete(row.visitId)
        known.add(clientKey(row))
    }
    if (visitIds.size)
        for (const row of Array.from(find(VISIT).condition("visitId", "in", Array.from(visitIds)).list()))
            known.add(clientKey(row))
    const key = clientKey(visit)
    if (known.has(key)) return none("KNOWN_CLIENT")
    if (!known.size) {
        // nothing in the window: a first sign-in ever is the owner setting the account up, not a new device
        const before =
            find(HISTORY, {userId, successfulLogin: "Y"}).condition("fromDate", "<", since).count() +
            find(VISIT, {userId}).condition("fromDate", "<", since).count()
        if (!before) return none("FIRST_SIGN_IN")
    }
    const clientSummary = identity.clientSummary(visit.initialUserAgent)
    const message = {
        userId,
        sessionKey: visitId,
        clientSummary,
        network: network(visit.clientIpAddress),
        place: [visit.clientIpCity, visit.clientIpCountryGeoId].filter(Boolean).join(", "),
        signInDate: new Date(at).toISOString(),
        // the time as the account's owner reads it; the sender is a worker with no user and no zone of its own
        signInText: ec.l10n.format(new Date(at), "yyyy-MM-dd HH:mm z")
    }
    ec.makeNotificationMessage().topic(TOPIC).userIds([userId]).message(message).record("moqui.server.Visit", visitId).send()
    return {notified: true, reasonCode: null, clientSummary, network: message.network}
}

const operations = {notifyNewDeviceSignIn}
;({
    network,
    execute: (op, parameters) => {
        if (!Object.hasOwn(operations, op)) throw new Error("Unknown new-device operation " + op)
        return operations[op](parameters)
    }
})
