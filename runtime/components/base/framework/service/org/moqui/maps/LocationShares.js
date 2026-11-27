// Shared live location (plans/Location-Sharing.md §1, §3.2; LocationServices.xml): the owner's shares, the reads of their
// current audience, the reports and the hourly clean. Every function checks the caller itself: the owner for a change or
// a report, the current audience (a user whose entry is not revoked, a current member of a group whose entry is not
// revoked) for a read, and answers anyone else, an ADMIN included, as not found. No position goes to a log, an error
// message, an audited field or a cache: a refusal names its reason only.

const SHARE = "moqui.maps.LocationShare"
const AUDIENCE = "moqui.maps.LocationShareAudience"
const LATEST = "moqui.maps.LocationLatest"
const TRAIL = "moqui.maps.LocationTrailPoint"
const ACCOUNT = "moqui.security.UserAccount"
const GROUP_MEMBER = "moqui.security.UserGroupMember"
const HOUR = 3600000
const DAY = 86400000
const LIST_MAX = 200
const TRAIL_PAGE_MAX = 1000
// a device's clock a little ahead of the server's is not a position in the future
const FUTURE_SKEW_MILLIS = 60000
// about 1.1 km of latitude, and as near that of longitude as the latitude gives
const APPROXIMATE_DEGREES = 0.01
const APPROXIMATE_ACCURACY = 1000
const PRECISIONS = ["LspExact", "LspApproximate"]

const find = entity => ec.entity.find(entity).disableAuthz()
const blank = value => value == null || String(value).trim() === ""
const text = value => (blank(value) ? "" : String(value).trim())
const millis = value => (blank(value) ? null : ec.user.timestampMillis(value))
const nowMillis = () => millis(ec.user.nowTimestamp)
const yes = value => value === true || value === "Y" || value === "true"
const round6 = value => Math.round(value * 1e6) / 1e6

/** A whole-number setting of the location conf element; the fallback when it is not set or not a whole number. */
function setting(name, fallback) {
    const value = text(ec.factory.getConfValue("location@" + name))
    const number = Number(value)
    return value !== "" && Number.isInteger(number) && number >= 0 ? number : fallback
}

function refuse(reasonCode, message, statusCode) {
    ec.message.addError(ec.l10n.localize(message))
    return statusCode ? {reasonCode, statusCode} : {reasonCode}
}
const notFound = statusCode => refuse("SHARE_NOT_FOUND", "Location share not found", statusCode)

function shareOf(shareId, forUpdate) {
    if (blank(shareId)) return null
    return find(SHARE).condition("shareId", text(shareId)).forUpdate(!!forUpdate).one()
}

/** The end of a share: its thruDate, or when it was stopped. */
function endOf(share) {
    const thru = millis(share.thruDate)
    const revoked = millis(share.revokedDate)
    return revoked != null && revoked < thru ? revoked : thru
}
const isActive = (share, now) =>
    share != null && share.revokedDate == null && share.endedDate == null && millis(share.thruDate) > now

/** The caller's own share, else null: another person's share is the same as none. */
function ownShare(shareId, forUpdate) {
    const share = shareOf(shareId, forUpdate)
    return share && ec.user.userId && share.ownerUserId === ec.user.userId ? share : null
}

/** The current groups of a user: memberships in effect now. */
function currentGroupIds(userId, groupIds) {
    if (!userId || (groupIds && !groupIds.length)) return []
    let members = find(GROUP_MEMBER).condition("userId", userId).conditionDate("fromDate", "thruDate", null)
    if (groupIds) members = members.condition("userGroupId", "in", groupIds)
    return Array.from(new Set(Array.from(members.selectField("userGroupId").list()).map(row => row.userGroupId)))
}

const activeAudience = shareId => Array.from(find(AUDIENCE).condition("shareId", shareId).condition("revokedDate", null).list())

/** The users a share reaches now: its users whose entries are not revoked and the current members of its groups, but its owner. */
function currentUserIds(share) {
    const ids = new Set()
    const groups = []
    for (const row of activeAudience(share.shareId)) {
        if (row.audienceTypeEnumId === "LsaUser" && row.userId) ids.add(row.userId)
        if (row.audienceTypeEnumId === "LsaUserGroup" && row.userGroupId) groups.push(row.userGroupId)
    }
    if (groups.length) {
        const members = find(GROUP_MEMBER)
            .condition("userGroupId", "in", groups)
            .conditionDate("fromDate", "thruDate", null)
            .selectField("userId")
        for (const row of members.list()) if (row.userId) ids.add(row.userId)
    }
    ids.delete(share.ownerUserId)
    return Array.from(ids)
}

/** Whether the share's current audience holds the user, checked at each read. */
function inAudience(share, userId) {
    if (!userId || userId === share.ownerUserId) return false
    const rows = activeAudience(share.shareId)
    if (rows.some(row => row.audienceTypeEnumId === "LsaUser" && row.userId === userId)) return true
    const groups = rows.filter(row => row.audienceTypeEnumId === "LsaUserGroup" && row.userGroupId).map(row => row.userGroupId)
    return currentGroupIds(userId, groups).length > 0
}

/** A push on the share's topic to these users only: sent when the transaction commits, never saved or sent to a phone. */
function push(share, userIds, message) {
    const ids = Array.from(new Set(userIds.filter(id => id)))
    if (!ids.length) return
    ec.makeNotificationMessage()
        .topic("locationShare:" + share.shareId)
        .userIds(ids)
        .directOnly()
        .persistOnSend(false)
        .showAlert(false)
        .message(Object.assign({shareId: share.shareId}, message))
        .send()
}

function namesOf(userIds) {
    const names = {}
    if (!userIds.length) return names
    const accounts = find(ACCOUNT).condition("userId", "in", userIds).selectField("userId,userFullName,username,timeZone").list()
    for (const row of accounts)
        names[row.userId] = {name: row.userFullName || row.username || row.userId, timeZone: row.timeZone || null}
    return names
}

/** "15:30", or "Fri 09:00" when the end is not today, in the zone given. */
function untilWords(thru, now, timeZone) {
    const day = value => ec.l10n.format(new Date(value), "yyyy-MM-dd", null, timeZone)
    return ec.l10n.format(new Date(thru), day(thru) === day(now) ? "HH:mm" : "EEE HH:mm", null, timeZone)
}

/** Each user added is told who shares with them and until when, in their own time zone: no position. */
function notifyShared(share, userIds, now) {
    const ids = userIds.filter(id => id && id !== share.ownerUserId)
    if (!ids.length) return
    const people = namesOf(ids.concat([share.ownerUserId]))
    const ownerName = (people[share.ownerUserId] || {}).name || share.ownerUserId
    const byZone = {}
    for (const id of ids) {
        const zone = (people[id] || {}).timeZone || ""
        ;(byZone[zone] = byZone[zone] || []).push(id)
    }
    for (const zone of Object.keys(byZone)) {
        const untilText = untilWords(millis(share.thruDate), now, zone || null)
        ec.makeNotificationMessage()
            .topic("LocationShared")
            .userIds(byZone[zone])
            .directOnly()
            .message({shareId: share.shareId, ownerName, label: share.label || ownerName, untilText})
            .send()
    }
}

/** The end asked for, within location@max-share-hours of now. */
function checkUntil(value, now) {
    const until = millis(value)
    if (until == null || until <= now) return refuse("BAD_UNTIL", "A share ends after now")
    const hours = setting("max-share-hours", 24)
    if (until > now + hours * HOUR) return refuse("TOO_LONG", `A share ends within ${hours} hours; extend it later`)
    return {until}
}

/** Whether a user may be in a share's audience: an account that may sign in, and one the owner may message. */
function checkUsers(userIds, now) {
    if (!userIds.length) return {}
    const open = new Set()
    for (const row of find(ACCOUNT).condition("userId", "in", userIds).selectField("userId,disabled,terminateDate").list()) {
        const ended = row.terminateDate != null && millis(row.terminateDate) <= now
        if (row.disabled !== "Y" && !ended) open.add(row.userId)
    }
    // the chat directory's rule (SEC-COM-01): the users the owner may not reach, and those who blocked the owner, answer
    // as users that do not exist
    const refused = ec.service
        .sync()
        .name("org.moqui.chat.ChatServices.get#RefusedUsers")
        .parameters({userIdList: userIds, blocks: true})
        .call()
    if (ec.message.hasError()) return {reasonCode: "USER_NOT_FOUND"}
    for (const id of Array.from((refused && refused.refusedUserIdList) || [])) open.delete(String(id))
    if (userIds.some(id => !open.has(id))) return refuse("USER_NOT_FOUND", "A person to share with was not found")
    return {}
}

/** Whether the owner may share with these groups: groups the owner is in now, never every user or a group of external users. */
function checkGroups(groupIds) {
    if (!groupIds.length) return {}
    const mine = new Set(currentGroupIds(ec.user.userId, groupIds))
    const outside = Array.from(
        find("moqui.security.UserGroupPermission")
            .condition("userGroupId", "in", groupIds)
            .condition("userPermissionId", "EXTERNAL_USER")
            .conditionDate("fromDate", "thruDate", null)
            .selectField("userGroupId")
            .list()
    ).map(row => row.userGroupId)
    if (groupIds.some(id => id === "ALL_USERS" || !mine.has(id) || outside.includes(id)))
        return refuse("GROUP_NOT_ALLOWED", "Share with a group only when you are in it")
    return {}
}

/** Whether these bare JIDs are contacts of the owner's XMPP roster (the other people of the owner's direct rooms). */
function checkJids(jids) {
    if (!jids.length) return {}
    const domain = text(ec.factory.getConfValue("xmpp-server@domain")).toLowerCase()
    if (!domain) return refuse("XMPP_OFF", "This server has no XMPP")
    const roster = ec.service.sync().name("org.moqui.chat.XmppServices.get#Roster").call()
    if (ec.message.hasError()) return {reasonCode: "XMPP_NOT_CONTACT"}
    const contactIds = Array.from((roster && roster.userIdList) || [])
    const usernames = new Set(
        contactIds.length
            ? Array.from(find(ACCOUNT).condition("userId", "in", contactIds).selectField("username").list()).map(row =>
                  text(row.username).toLowerCase()
              )
            : []
    )
    for (const jid of jids) {
        const parts = /^([^@/\s]+)@([^@/\s]+)$/.exec(jid)
        if (!parts || parts[2] !== domain || !usernames.has(parts[1]))
            return refuse("XMPP_NOT_CONTACT", "Share over XMPP only with a contact of yours")
    }
    return {}
}

/** The audience entries asked for, checked by the rules of who an owner may share with. */
function checkAudience(list, now) {
    const me = ec.user.userId
    const entries = []
    const seen = new Set()
    let links = 0
    for (const raw of Array.from(list || [])) {
        const entry = raw || {}
        const type = text(entry.audienceTypeEnumId)
        let key = null
        if (type === "LsaUser") key = text(entry.userId)
        else if (type === "LsaUserGroup") key = text(entry.userGroupId)
        else if (type === "LsaXmpp") key = text(entry.jid).toLowerCase()
        else if (type === "LsaLink") key = "link" + links++
        else return refuse("BAD_AUDIENCE", "Share with a person, a group, an XMPP contact or a link")
        if (!key || (type === "LsaUser" && key === me))
            return refuse("BAD_AUDIENCE", "Share with a person, a group, an XMPP contact or a link")
        if (seen.has(type + ":" + key)) continue
        seen.add(type + ":" + key)
        entries.push({type, key})
    }
    if (links > 1) return refuse("ONE_LINK", "A share has one guest link")
    const of = type => entries.filter(entry => entry.type === type).map(entry => entry.key)
    for (const check of [
        () => checkUsers(of("LsaUser"), now),
        () => checkGroups(of("LsaUserGroup")),
        () => checkJids(of("LsaXmpp"))
    ]) {
        const out = check()
        if (out.reasonCode) return out
    }
    return {entries}
}

/** The entries written: a link's token made here, returned once, and only its digest kept. */
function addAudience(share, entries, now) {
    let linkToken = null
    const userIds = []
    const groupIds = []
    for (const entry of entries) {
        const fields = {shareId: share.shareId, audienceTypeEnumId: entry.type, fromDate: new Date(now)}
        if (entry.type === "LsaUser") {
            fields.userId = entry.key
            userIds.push(entry.key)
        } else if (entry.type === "LsaUserGroup") {
            fields.userGroupId = entry.key
            groupIds.push(entry.key)
        } else if (entry.type === "LsaXmpp") {
            fields.jid = entry.key
        } else if (entry.type === "LsaLink") {
            linkToken = ec.user.randomString(32)
            fields.tokenDigest = ec.user.singleUseCodeHash(linkToken)
            fields.failedTries = 0
        }
        ec.entity.makeValue(AUDIENCE).setAll(fields).setSequencedIdSecondary().create()
    }
    if (groupIds.length) {
        const members = find(GROUP_MEMBER)
            .condition("userGroupId", "in", groupIds)
            .conditionDate("fromDate", "thruDate", null)
            .selectField("userId")
        for (const row of members.list()) if (row.userId) userIds.push(row.userId)
    }
    return {linkToken, userIds: Array.from(new Set(userIds))}
}

function checkTrail(p) {
    if (!yes(p.keepTrail)) return {keepTrail: "N", trailKeepDays: null}
    const maxDays = setting("trail-max-days", 30)
    const days = blank(p.trailKeepDays) ? maxDays : Number(p.trailKeepDays)
    if (!Number.isInteger(days) || days < 1 || days > maxDays)
        return refuse("BAD_TRAIL_DAYS", maxDays ? `A trail is kept from 1 to ${maxDays} days` : "This server keeps no trail")
    return {keepTrail: "Y", trailKeepDays: days}
}

/** create#Share: the caller's own share, its audience checked, each person told. */
function create(p) {
    const me = ec.user.userId
    const now = nowMillis()
    const until = checkUntil(p.thruDate, now)
    if (until.reasonCode) return until
    const precisionEnumId = text(p.precisionEnumId) || "LspExact"
    if (!PRECISIONS.includes(precisionEnumId)) return refuse("BAD_PRECISION", "A share is exact or approximate")
    const trail = checkTrail(p)
    if (trail.reasonCode) return trail
    if (!Array.from(p.audienceList || []).length) return refuse("AUDIENCE_EMPTY", "Choose who sees your location")
    const audience = checkAudience(p.audienceList, now)
    if (audience.reasonCode) return audience
    if (!audience.entries.length) return refuse("AUDIENCE_EMPTY", "Choose who sees your location")
    const ownerName = (namesOf([me])[me] || {}).name || me
    const share = ec.entity
        .makeValue(SHARE)
        .setAll({
            ownerUserId: me,
            label: text(p.label) || ownerName,
            purposeText: text(p.purposeText) || null,
            fromDate: new Date(now),
            thruDate: new Date(until.until),
            precisionEnumId,
            keepTrail: trail.keepTrail,
            trailKeepDays: trail.trailKeepDays
        })
        .setSequencedIdPrimary()
        .create()
    const added = addAudience(share, audience.entries, now)
    notifyShared(share, added.userIds, now)
    push(share, [me], {event: "started", thruDate: until.until})
    return {shareId: share.shareId, thruDate: until.until, linkToken: added.linkToken}
}

/** update#Share: an active share of the caller's own, extended, shortened, relabeled or given more audience. */
function update(p) {
    const now = nowMillis()
    const share = ownShare(p.shareId, true)
    if (!share) return notFound()
    if (!isActive(share, now)) return refuse("SHARE_ENDED", "This share has ended: start a new one")
    if (!blank(p.thruDate)) {
        const until = checkUntil(p.thruDate, now)
        if (until.reasonCode) return until
        share.thruDate = new Date(until.until)
    }
    if (p.label != null) share.label = text(p.label) || share.label
    if (p.purposeText != null) share.purposeText = text(p.purposeText) || null
    let added = {linkToken: null, userIds: []}
    if (Array.from(p.addAudienceList || []).length) {
        const audience = checkAudience(p.addAudienceList, now)
        if (audience.reasonCode) return audience
        const before = currentUserIds(share)
        const rows = activeAudience(share.shareId)
        const has = entry =>
            rows.some(
                row =>
                    row.audienceTypeEnumId === entry.type &&
                    ((entry.type === "LsaUser" && row.userId === entry.key) ||
                        (entry.type === "LsaUserGroup" && row.userGroupId === entry.key) ||
                        (entry.type === "LsaXmpp" && row.jid === entry.key) ||
                        entry.type === "LsaLink")
            )
        const fresh = audience.entries.filter(entry => !has(entry))
        if (audience.entries.some(entry => entry.type === "LsaLink") && rows.some(row => row.audienceTypeEnumId === "LsaLink"))
            return refuse("ONE_LINK", "A share has one guest link")
        added = addAudience(share, fresh, now)
        added.userIds = added.userIds.filter(id => !before.includes(id))
    }
    share.update()
    notifyShared(share, added.userIds, now)
    push(share, currentUserIds(share).concat([share.ownerUserId]), {event: "changed", thruDate: millis(share.thruDate)})
    return {thruDate: millis(share.thruDate), linkToken: added.linkToken}
}

/** A share's end acted on: its latest position deleted, the audience and the owner told. */
function endShare(share, now) {
    const userIds = currentUserIds(share)
    share.endedDate = new Date(now)
    share.update()
    find(LATEST).condition("shareId", share.shareId).deleteAll()
    push(share, userIds.concat([share.ownerUserId]), {event: "ended"})
}

/** stop#Share: the caller's own share ended now. */
function stop(p) {
    const now = nowMillis()
    const share = ownShare(p.shareId, true)
    if (!share) return notFound()
    if (share.endedDate != null) return {}
    if (share.revokedDate == null && millis(share.thruDate) > now) share.revokedDate = new Date(now)
    endShare(share, now)
    return {}
}

/** revoke#Audience: one entry of the caller's own share out of its audience; its users are told. */
function revoke(p) {
    const now = nowMillis()
    const share = ownShare(p.shareId, true)
    if (!share) return notFound()
    const row = find(AUDIENCE).condition("shareId", share.shareId).condition("audienceSeqId", text(p.audienceSeqId)).one()
    if (!row || row.revokedDate != null) return refuse("AUDIENCE_NOT_FOUND", "That person or link is not in this share")
    const before = currentUserIds(share)
    row.revokedDate = new Date(now)
    row.update()
    const after = new Set(currentUserIds(share))
    push(share, before.filter(id => !after.has(id)).concat([share.ownerUserId]), {event: "revoked"})
    return {}
}

const numberOf = value => (blank(value) ? null : Number(String(value).trim()))

/** A report's values checked: ranges, a time not in the future. A refusal names no value. */
function checkPosition(p, now) {
    const latitude = numberOf(p.latitude)
    const longitude = numberOf(p.longitude)
    if (!Number.isFinite(latitude) || !Number.isFinite(longitude) || Math.abs(latitude) > 90 || Math.abs(longitude) > 180)
        return refuse("BAD_POSITION", "A position needs a latitude and a longitude in range", 400)
    const accuracy = numberOf(p.accuracy)
    if (accuracy != null && (!Number.isFinite(accuracy) || accuracy < 0 || accuracy > 1e7))
        return refuse("BAD_ACCURACY", "A position's accuracy is in metres", 400)
    const heading = numberOf(p.heading)
    if (heading != null && (!Number.isFinite(heading) || heading < 0 || heading > 360))
        return refuse("BAD_HEADING", "A heading is in degrees from north", 400)
    const speed = numberOf(p.speed)
    if (speed != null && (!Number.isFinite(speed) || speed < 0 || speed > 1000))
        return refuse("BAD_SPEED", "A speed is in metres a second", 400)
    const observed = millis(p.observedDate) ?? now
    if (observed > now + FUTURE_SKEW_MILLIS) return refuse("BAD_DATE", "A position's time is not in the future", 400)
    return {position: {latitude, longitude, accuracy, heading: heading == null ? null : heading % 360, speed, observed}}
}

/** An approximate share's position: snapped to a grid of about 1 km, no heading or speed, an accuracy of at least 1 km. */
function approximate(position) {
    const latitude = Math.max(-90, Math.min(90, Math.round(position.latitude / APPROXIMATE_DEGREES) * APPROXIMATE_DEGREES))
    const cos = Math.cos((latitude * Math.PI) / 180)
    // the longitude cell keeps about the same width in km: wider in degrees toward a pole, one cell at it
    const step = cos > APPROXIMATE_DEGREES ? APPROXIMATE_DEGREES / cos : 360
    let longitude = Math.round(position.longitude / step) * step
    if (longitude > 180) longitude -= 360
    if (longitude < -180) longitude += 360
    return {
        latitude: round6(latitude),
        longitude: round6(Math.max(-180, Math.min(180, longitude))),
        accuracy: Math.max(position.accuracy ?? 0, APPROXIMATE_ACCURACY),
        heading: null,
        speed: null,
        observed: position.observed
    }
}

/** report#Position: from the owner, for an active share, at most one a min-interval-seconds; rounded here when approximate. */
function report(p) {
    const now = nowMillis()
    const share = ownShare(p.shareId, true)
    if (!share) return notFound(404)
    if (!isActive(share, now)) return {statusCode: 410, reasonCode: "SHARE_ENDED"}
    const latest = find(LATEST).condition("shareId", share.shareId).one()
    const intervalMillis = setting("min-interval-seconds", 5) * 1000
    const received = latest ? millis(latest.receivedDate) : null
    if (received != null && now - received < intervalMillis)
        return {
            statusCode: 429,
            reasonCode: "RATE_LIMITED",
            retryAfterSeconds: Math.max(1, Math.ceil((intervalMillis - (now - received)) / 1000))
        }
    const checked = checkPosition(p, now)
    if (checked.reasonCode) return checked
    if (latest && latest.observedDate != null && checked.position.observed <= millis(latest.observedDate))
        return {statusCode: 409, reasonCode: "STALE"}
    const approximated = share.precisionEnumId === "LspApproximate"
    const position = approximated ? approximate(checked.position) : checked.position
    const values = {
        latitude: position.latitude,
        longitude: position.longitude,
        accuracy: position.accuracy,
        heading: position.heading,
        speed: position.speed
    }
    ec.entity
        .makeValue(LATEST)
        .setAll(
            Object.assign(
                {shareId: share.shareId, observedDate: new Date(position.observed), receivedDate: new Date(now)},
                values
            )
        )
        .createOrUpdate()
    if (share.keepTrail === "Y")
        ec.entity
            .makeValue(TRAIL)
            .setAll(Object.assign({shareId: share.shareId, observedDate: new Date(position.observed)}, values))
            .create()
    push(
        share,
        currentUserIds(share),
        Object.assign({event: "position", observedDate: position.observed, approximate: approximated}, values)
    )
    return {statusCode: 200}
}

function positionOf(row, now) {
    if (!row) return null
    const observed = millis(row.observedDate)
    return {
        latitude: row.latitude,
        longitude: row.longitude,
        accuracy: row.accuracy,
        heading: row.heading,
        speed: row.speed,
        observedDate: observed,
        ageSeconds: observed == null ? null : Math.max(0, Math.round((now - observed) / 1000))
    }
}

function latestByShare(shareIds, now) {
    const out = {}
    if (!shareIds.length) return out
    for (const row of find(LATEST).condition("shareId", "in", shareIds).list()) out[row.shareId] = positionOf(row, now)
    return out
}

/** A share as its owner reads it: its audience with names, never a link's token or digest. */
function ownerView(share, now) {
    const rows = Array.from(find(AUDIENCE).condition("shareId", share.shareId).orderBy("audienceSeqId").list())
    const people = namesOf(rows.filter(row => row.userId).map(row => row.userId))
    const groupIds = rows.filter(row => row.userGroupId).map(row => row.userGroupId)
    const groups = {}
    if (groupIds.length)
        for (const row of find("moqui.security.UserGroup")
            .condition("userGroupId", "in", groupIds)
            .selectField("userGroupId,description")
            .list())
            groups[row.userGroupId] = row.description || row.userGroupId
    const audienceList = rows.map(row => ({
        audienceSeqId: row.audienceSeqId,
        audienceTypeEnumId: row.audienceTypeEnumId,
        userId: row.userId || null,
        userName: row.userId ? (people[row.userId] || {}).name || row.userId : null,
        userGroupId: row.userGroupId || null,
        groupName: row.userGroupId ? groups[row.userGroupId] || row.userGroupId : null,
        jid: row.jid || null,
        hasLink: row.audienceTypeEnumId === "LsaLink",
        lastViewedDate: millis(row.lastViewedDate),
        revoked: row.revokedDate != null
    }))
    return Object.assign(viewerView(share, now), {
        purposeText: share.purposeText || null,
        keepTrail: share.keepTrail === "Y",
        trailKeepDays: share.trailKeepDays,
        revokedDate: millis(share.revokedDate),
        endedDate: millis(share.endedDate),
        active: isActive(share, now),
        audienceList
    })
}

/** A share as its audience reads it. */
function viewerView(share, now) {
    const thru = millis(share.thruDate)
    return {
        shareId: share.shareId,
        ownerUserId: share.ownerUserId,
        label: share.label || null,
        purposeText: share.purposeText || null,
        fromDate: millis(share.fromDate),
        thruDate: thru,
        thruText: ec.l10n.format(new Date(thru), "yyyy-MM-dd HH:mm"),
        approximate: share.precisionEnumId === "LspApproximate",
        keepTrail: share.keepTrail === "Y"
    }
}

/** find#MyShares: the caller's own shares, the active ones or all of them. */
function findMine(p) {
    const now = nowMillis()
    let shares = find(SHARE).condition("ownerUserId", ec.user.userId)
    if (!yes(p.includeEnded))
        shares = shares
            .condition("revokedDate", null)
            .condition("endedDate", null)
            .condition("thruDate", "greater", new Date(now))
    const rows = Array.from(shares.orderBy("-fromDate").limit(LIST_MAX).list())
    const latest = latestByShare(
        rows.filter(share => isActive(share, now)).map(share => share.shareId),
        now
    )
    const shareList = rows.map(share =>
        Object.assign(ownerView(share, now), {
            latest: latest[share.shareId] || null,
            trailCount: share.keepTrail === "Y" ? find(TRAIL).condition("shareId", share.shareId).count() : 0
        })
    )
    return {shareList}
}

/** find#SharedWithMe: the active shares of others whose current audience holds the caller. */
function findShared() {
    const me = ec.user.userId
    const now = nowMillis()
    const ids = new Set()
    for (const row of find(AUDIENCE)
        .condition("audienceTypeEnumId", "LsaUser")
        .condition("userId", me)
        .condition("revokedDate", null)
        .selectField("shareId")
        .list())
        ids.add(row.shareId)
    const groups = currentGroupIds(me, null)
    if (groups.length) {
        const rows = find(AUDIENCE)
            .condition("audienceTypeEnumId", "LsaUserGroup")
            .condition("userGroupId", "in", groups)
            .condition("revokedDate", null)
            .selectField("shareId")
        for (const row of rows.list()) ids.add(row.shareId)
    }
    if (!ids.size) return {shareList: []}
    const shares = Array.from(
        find(SHARE)
            .condition("shareId", "in", Array.from(ids))
            .condition("ownerUserId", "not-equals", me)
            .condition("revokedDate", null)
            .condition("endedDate", null)
            .condition("thruDate", "greater", new Date(now))
            .orderBy("thruDate")
            .limit(LIST_MAX)
            .list()
    )
    const latest = latestByShare(
        shares.map(share => share.shareId),
        now
    )
    const owners = namesOf(Array.from(new Set(shares.map(share => share.ownerUserId))))
    const shareList = shares.map(share =>
        Object.assign(viewerView(share, now), {
            ownerName: (owners[share.ownerUserId] || {}).name || share.ownerUserId,
            latest: latest[share.shareId] || null
        })
    )
    return {shareList}
}

/** get#Share: for the owner, or the current audience of an active share; the trail in bounded pages. */
function get(p) {
    const now = nowMillis()
    const share = shareOf(p.shareId)
    const owner = share != null && share.ownerUserId === ec.user.userId
    if (!share || (!owner && !(isActive(share, now) && inAudience(share, ec.user.userId)))) return notFound()
    const view = owner ? ownerView(share, now) : viewerView(share, now)
    if (!owner) view.ownerName = (namesOf([share.ownerUserId])[share.ownerUserId] || {}).name || share.ownerUserId
    const latest = isActive(share, now) ? latestByShare([share.shareId], now)[share.shareId] || null : null
    const pageSize = Math.max(1, Math.min(TRAIL_PAGE_MAX, Number(p.trailPageSize) || 500))
    const pageIndex = Math.max(0, Number(p.trailPageIndex) || 0)
    let trailList = []
    let trailCount = 0
    if (share.keepTrail === "Y") {
        const trail = find(TRAIL).condition("shareId", share.shareId)
        trailCount = trail.count()
        trailList = Array.from(
            trail
                .orderBy("observedDate")
                .offset(pageIndex * pageSize)
                .limit(pageSize)
                .list()
        ).map(row => positionOf(row, now))
    }
    return {
        share: view,
        latest,
        trailList,
        trailCount,
        trailPageIndex: pageIndex,
        trailPageSize: pageSize,
        trailPageMaxIndex: Math.max(0, Math.ceil(trailCount / pageSize) - 1)
    }
}

/** delete#Trail: the caller's own share's trail, all of it, now. */
function deleteTrail(p) {
    const share = ownShare(p.shareId)
    if (!share) return notFound()
    return {deletedCount: find(TRAIL).condition("shareId", share.shareId).deleteAll()}
}

/** One share of the clean in its own transaction: a failure is logged by the share's id and the clean goes on. */
function eachShare(rows, counts, work) {
    for (const row of rows) {
        try {
            ec.transaction.runRequireNew(60, "Could not clean a location share", () => work(row.shareId))
        } catch (err) {
            counts.failedCount++
            ec.logger.error(`Could not clean location share ${row.shareId}: ${err && err.message ? err.message : err}`)
        }
    }
}

/** clean#LocationShares, hourly: the shares past their end ended, the trails past their days deleted, old shares removed. */
function clean() {
    const now = nowMillis()
    const counts = {endedCount: 0, trailDeletedCount: 0, removedCount: 0, failedCount: 0}
    const maxDays = setting("trail-max-days", 30)
    const due = find(SHARE)
        .condition("endedDate", null)
        .condition("thruDate", "less-equals", new Date(now))
        .selectField("shareId")
        .limit(1000)
        .list()
    eachShare(Array.from(due), counts, shareId => {
        const share = shareOf(shareId, true)
        if (!share || share.endedDate != null) return
        endShare(share, now)
        counts.endedCount++
    })
    const kept = find(SHARE)
        .condition("keepTrail", "Y")
        .condition("endedDate", "is-not-null", null)
        .selectField("shareId")
        .limit(5000)
        .list()
    eachShare(Array.from(kept), counts, shareId => {
        const share = shareOf(shareId, true)
        if (!share) return
        const days = Math.min(Number(share.trailKeepDays) || maxDays, maxDays)
        if (now <= endOf(share) + days * DAY) return
        counts.trailDeletedCount += find(TRAIL).condition("shareId", shareId).deleteAll()
    })
    const old = find(SHARE)
        .condition("endedDate", "less", new Date(now - maxDays * DAY))
        .selectField("shareId")
        .limit(1000)
        .list()
    eachShare(Array.from(old), counts, shareId => {
        if (find(TRAIL).condition("shareId", shareId).count() > 0) return
        find(LATEST).condition("shareId", shareId).deleteAll()
        find(AUDIENCE).condition("shareId", shareId).deleteAll()
        find(SHARE).condition("shareId", shareId).deleteAll()
        counts.removedCount++
    })
    if (counts.endedCount || counts.trailDeletedCount || counts.removedCount || counts.failedCount)
        ec.logger.info(
            `Location shares: ${counts.endedCount} ended, ${counts.trailDeletedCount} trail points deleted, ${counts.removedCount} removed, ${counts.failedCount} failed`
        )
    return counts
}

const operations = {create, update, stop, revoke, report, findMine, findShared, get, deleteTrail, clean}

function execute(op, parameters) {
    if (!operations[op]) throw new Error("Unknown location share operation " + op)
    return operations[op](parameters || {}) || {}
}

;({execute})
