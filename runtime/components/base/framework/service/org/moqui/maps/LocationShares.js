// Shared live location (plans/Location-Sharing.md §1, §3.2; LocationServices.xml): the owner's shares, the reads of
// their current audience, the reports and the hourly clean. Every function checks the caller itself: the owner for a
// change or a report, the current audience (a user whose entry is not revoked, a member of a group whose entry is not
// revoked whom the owner's adding of the group reached, while that membership row is current) for a read, and answers
// anyone else, an ADMIN included, as not found. No position goes to a log, an error message, an audited field or a
// cache: a refusal names its reason only.

const SHARE = "moqui.maps.LocationShare"
const AUDIENCE = "moqui.maps.LocationShareAudience"
const AUDIENCE_MEMBER = "moqui.maps.LocationShareAudienceMember"
const OWNER_LOCK = "moqui.maps.LocationShareOwner"
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
// a fix keeps the last cell reported until it is more than a quarter of a cell inside another one
const APPROXIMATE_KEEP_CELLS = 0.75
const MINUTE = 60000
// the hourly clean takes its shares in batches, each stage at most this long a run
const CLEAN_BATCH = 500
const CLEAN_STAGE_MILLIS = 2 * 60000
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

/** A share by its id; with ownerUserId only that owner's, so a lock is never taken on another person's row. */
function shareOf(shareId, forUpdate, ownerUserId) {
    if (blank(shareId)) return null
    let share = find(SHARE).condition("shareId", text(shareId))
    if (ownerUserId) share = share.condition("ownerUserId", ownerUserId)
    return share.forUpdate(!!forUpdate).one()
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
    const me = ec.user.userId
    if (!me) return null
    const share = shareOf(shareId, forUpdate, me)
    return share && share.ownerUserId === me ? share : null
}

/** The groups a user is a member of now. */
function currentGroupIds(userId, groupIds) {
    if (!userId || (groupIds && !groupIds.length)) return []
    let members = find(GROUP_MEMBER).condition("userId", userId).conditionDate("fromDate", "thruDate", null)
    if (groupIds) members = members.condition("userGroupId", "in", groupIds)
    return Array.from(new Set(Array.from(members.selectField("userGroupId").list()).map(row => row.userGroupId)))
}

const activeAudience = shareId => Array.from(find(AUDIENCE).condition("shareId", shareId).condition("revokedDate", null).list())

/**
 * The users a group entry reaches: those of its snapshot (LocationShareAudienceMember, the members the owner's adding
 * of the group reached, written only by create#Share and update#Share) while the membership row the snapshot names is
 * current. A membership row written by anyone else (an administrator, an app or household organizer, with whatever
 * dates or stamps) adds nobody, and a member taken out drops out at once, with one exception: a snapshot member's own
 * row, ended and reopened, or deleted and made again with the same fromDate (which System › Access lets an
 * administrator do), lets that same person back. With userId, that user only.
 */
function groupEntryUserIds(row, userId) {
    let snapshot = find(AUDIENCE_MEMBER).condition("shareId", row.shareId).condition("audienceSeqId", row.audienceSeqId)
    if (userId) snapshot = snapshot.condition("userId", userId)
    const held = new Set(Array.from(snapshot.selectField("userId,memberFromDate").list()).map(heldKey))
    if (!held.size) return []
    const current = find(GROUP_MEMBER)
        .condition("userGroupId", row.userGroupId)
        .condition(
            "userId",
            "in",
            Array.from(held).map(key => key.split("|")[0])
        )
        .conditionDate("fromDate", "thruDate", null)
        .selectField("userId,fromDate")
        .list()
    return Array.from(
        new Set(
            Array.from(current)
                .filter(member => held.has(memberKey(member)))
                .map(member => member.userId)
        )
    )
}
// a snapshot row and the membership row it names, by the user and the membership's fromDate (its key in the group)
const heldKey = row => row.userId + "|" + millis(row.memberFromDate)
const memberKey = row => row.userId + "|" + millis(row.fromDate)

/**
 * The snapshot of a group entry, written at the owner's act: the group's current members but the owner, each with the
 * membership row that makes them one (its fromDate), less every one the chat directory refuses the owner at this act
 * (ChatServices.get#RefusedUsers with blocks: the chat-target hooks, the external-caller rule and the people who
 * blocked the owner; one blocked later is applied at the next re-add, as for a direct user). It replaces an earlier
 * snapshot of the entry. The caller is the owner. A member reaches the share only while that same membership row is
 * current: one removed and put back by a new row is not, until the owner adds the group again. False when the
 * directory's check failed, its error left in ec.message.
 */
function snapshotGroup(share, row) {
    const since = {}
    for (const member of find(GROUP_MEMBER)
        .condition("userGroupId", row.userGroupId)
        .conditionDate("fromDate", "thruDate", null)
        .selectField("userId,fromDate")
        .orderBy("fromDate")
        .list())
        if (member.userId && member.userId !== share.ownerUserId && !(member.userId in since))
            since[member.userId] = member.fromDate
    const members = Object.keys(since)
    let refused = new Set()
    if (members.length) {
        const out = ec.service
            .sync()
            .name("org.moqui.chat.ChatServices.get#RefusedUsers")
            .parameters({userIdList: members, blocks: true, answerWithAuthzOff: true})
            .call()
        if (ec.message.hasError()) return false
        refused = new Set(Array.from((out && out.refusedUserIdList) || []).map(String))
    }
    find(AUDIENCE_MEMBER).condition("shareId", share.shareId).condition("audienceSeqId", row.audienceSeqId).deleteAll()
    for (const userId of members.filter(id => !refused.has(id)))
        ec.entity
            .makeValue(AUDIENCE_MEMBER)
            .setAll({shareId: share.shareId, audienceSeqId: row.audienceSeqId, userId, memberFromDate: since[userId]})
            .create()
    return true
}
const groupCheckFailed = () => ({reasonCode: "GROUP_CHECK_FAILED"})

/** The users a share reaches now: its users whose entries are not revoked and its groups' members as above, but its owner. */
function currentUserIds(share) {
    const ids = new Set()
    for (const row of activeAudience(share.shareId)) {
        if (row.audienceTypeEnumId === "LsaUser" && row.userId) ids.add(row.userId)
        if (row.audienceTypeEnumId === "LsaUserGroup" && row.userGroupId) for (const id of groupEntryUserIds(row)) ids.add(id)
    }
    ids.delete(share.ownerUserId)
    return Array.from(ids)
}

/** Whether the share's current audience holds the user, checked at each read. */
function inAudience(share, userId) {
    if (!userId || userId === share.ownerUserId) return false
    const rows = activeAudience(share.shareId)
    if (rows.some(row => row.audienceTypeEnumId === "LsaUser" && row.userId === userId)) return true
    return rows.some(
        row => row.audienceTypeEnumId === "LsaUserGroup" && row.userGroupId && groupEntryUserIds(row, userId).length > 0
    )
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
    // the chat directory's rule (SEC-COM-01): the users the owner may not reach, and those who blocked the owner,
    // answer as users that do not exist
    const refused = ec.service
        .sync()
        .name("org.moqui.chat.ChatServices.get#RefusedUsers")
        .parameters({userIdList: userIds, blocks: true, answerWithAuthzOff: true})
        .call()
    if (ec.message.hasError()) return {reasonCode: "USER_NOT_FOUND"}
    for (const id of Array.from((refused && refused.refusedUserIdList) || [])) open.delete(String(id))
    if (userIds.some(id => !open.has(id))) return refuse("USER_NOT_FOUND", "A person to share with was not found")
    return {}
}

/**
 * Whether the owner may share with these groups: groups the owner is in now, never every user or a group of external users,
 * and none whose current membership rows number more than four times location@max-audience, refused before its snapshot
 * so that a huge group costs no inserts the cap would roll back (plans/Location-Sharing.md §3.2; checkReach, after the
 * writes, stays the cap that counts).
 */
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
    const max = setting("max-audience", 50)
    for (const userGroupId of groupIds) {
        const members = find(GROUP_MEMBER)
            .condition("userGroupId", userGroupId)
            .conditionDate("fromDate", "thruDate", null)
            .count()
        if (members > max * 4) return refuse("AUDIENCE_TOO_LARGE", `A share is seen by at most ${max} people and links`)
    }
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
    for (const entry of entries) {
        const fields = {shareId: share.shareId, audienceTypeEnumId: entry.type, fromDate: new Date(now)}
        if (entry.type === "LsaUser") {
            fields.userId = entry.key
        } else if (entry.type === "LsaUserGroup") {
            fields.userGroupId = entry.key
        } else if (entry.type === "LsaXmpp") {
            fields.jid = entry.key
        } else if (entry.type === "LsaLink") {
            linkToken = ec.user.randomString(32)
            fields.tokenDigest = ec.user.singleUseCodeHash(linkToken)
            fields.failedTries = 0
        }
        const row = ec.entity.makeValue(AUDIENCE).setAll(fields).setSequencedIdSecondary().create()
        if (entry.type === "LsaUserGroup" && !snapshotGroup(share, row)) return groupCheckFailed()
    }
    return {linkToken}
}

/**
 * Whether the share's audience, as written (its groups by their snapshots), stays within location@max-audience: the
 * people it reaches, its XMPP contacts and its link. Checked after the writes, which a refusal rolls back.
 */
function checkReach(share) {
    const max = setting("max-audience", 50)
    const others = activeAudience(share.shareId).filter(row => ["LsaXmpp", "LsaLink"].includes(row.audienceTypeEnumId)).length
    if (currentUserIds(share).length + others > max)
        return refuse("AUDIENCE_TOO_LARGE", `A share is seen by at most ${max} people and links`)
    return {}
}

/**
 * The owner's lock row (LocationShareOwner, which no foreign key names, so no audience insert's key check waits on it
 * on any database), made on first use in its own transaction, then locked for the rest of the caller's. It fails closed
 * (plans/Location-Sharing.md §3.2, the LL1 review): the first look is a plain read, a missing row is made in its own
 * transaction, where only another request's making it at the same moment (the row there after the failure: a duplicate
 * key) is passed over and any other failure is logged; false when the row cannot then be locked, which the caller refuses.
 */
function lockOwner(userId) {
    const lockRow = () => find(OWNER_LOCK).condition("ownerUserId", userId)
    if (!lockRow().one()) {
        let failure = null
        ec.message.pushErrors()
        try {
            ec.transaction.runRequireNew(10, "Could not make a location share lock row", () => {
                if (!lockRow().one())
                    ec.entity.makeValue(OWNER_LOCK).setAll({ownerUserId: userId, createdDate: ec.user.nowTimestamp}).create()
            })
            if (ec.message.hasError()) failure = ec.message.getErrorsString()
        } catch (err) {
            failure = String(err && err.message ? err.message : err)
        } finally {
            ec.message.clearErrors()
            ec.message.popErrors()
        }
        if (failure != null && !lockRow().one())
            ec.logger.error(`Could not make the location share lock row of user ${userId}: ${failure}`)
    }
    return lockRow().forUpdate(true).one() != null
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
    const maxShares = setting("max-active-shares", 20)
    // counted under the owner's lock row, so two creates at once cannot both pass the cap
    if (!lockOwner(me)) return refuse("SHARE_LOCK_FAILED", "Your location could not be shared now: try again")
    const active = find(SHARE)
        .condition("ownerUserId", me)
        .condition("revokedDate", null)
        .condition("endedDate", null)
        .condition("thruDate", "greater", new Date(now))
        .count()
    if (active >= maxShares)
        return refuse("TOO_MANY_SHARES", `You share your location at most ${maxShares} times at once: stop one first`)
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
    if (added.reasonCode) return added
    const reach = checkReach(share)
    if (reach.reasonCode) return reach
    notifyShared(share, currentUserIds(share), now)
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
    let added = {linkToken: null}
    const before = currentUserIds(share)
    if (Array.from(p.addAudienceList || []).length) {
        const audience = checkAudience(p.addAudienceList, now)
        if (audience.reasonCode) return audience
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
        // a group added again is stamped now: its members who joined since the owner last added it are let in, and told
        for (const row of rows)
            if (
                row.audienceTypeEnumId === "LsaUserGroup" &&
                audience.entries.some(entry => entry.type === "LsaUserGroup" && entry.key === row.userGroupId)
            ) {
                row.fromDate = new Date(now)
                row.update()
                if (!snapshotGroup(share, row)) return groupCheckFailed()
            }
        added = addAudience(share, fresh, now)
        if (added.reasonCode) return added
        const reach = checkReach(share)
        if (reach.reasonCode) return reach
    }
    share.update()
    const reached = currentUserIds(share)
    const fresh = reached.filter(id => !before.includes(id))
    // a re-add that leaves someone out (who now blocks the owner, or has left the group) tells them, as revoke#Audience
    // does
    push(
        share,
        before.filter(id => !reached.includes(id)),
        {event: "revoked"}
    )
    notifyShared(share, fresh, now)
    const people = namesOf(fresh)
    const addedUserList = fresh.map(userId => ({userId, userName: (people[userId] || {}).name || userId}))
    const thruDate = millis(share.thruDate)
    push(share, reached, {event: "changed", thruDate})
    // the owner's devices are told whom the change lets in
    push(share, [share.ownerUserId], {event: "changed", thruDate, addedUserList})
    return {thruDate, linkToken: added.linkToken, addedUserList}
}

/** A share's end acted on: its latest position deleted, the audience and the owner told. */
function endShare(share, now) {
    const userIds = currentUserIds(share)
    share.endedDate = new Date(now)
    if (share.keepTrail === "Y") {
        const maxDays = setting("trail-max-days", 30)
        const days = Math.min(Number(share.trailKeepDays) || maxDays, maxDays)
        share.trailPurgeDate = new Date(endOf(share) + days * DAY)
    }
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
function checkPosition(p, now, share) {
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
    // a device whose clock is ahead is told so, with no error, so that it can say why its positions are not taken
    if (observed > now + FUTURE_SKEW_MILLIS) return {statusCode: 409, reasonCode: "CLOCK_AHEAD"}
    if (observed < millis(share.fromDate)) return refuse("BEFORE_SHARE", "A position's time is after its share began", 400)
    return {position: {latitude, longitude, accuracy, heading: heading == null ? null : heading % 360, speed, observed}}
}

const wrapLongitude = longitude => ((((longitude + 180) % 360) + 360) % 360) - 180
// the longitude width of a cell at a latitude: about the same width in km, wider in degrees toward a pole, and one cell
// around the whole circle at a pole
function longitudeStep(latitude) {
    const cos = Math.cos((latitude * Math.PI) / 180)
    return cos > APPROXIMATE_DEGREES ? APPROXIMATE_DEGREES / cos : 360
}

/**
 * The centre of the cell that holds the point on the one fixed grid every share uses: so every share of a person snaps
 * one place to the same cell, and comparing shares narrows nothing.
 */
function cellOf(latitude, longitude) {
    const row = Math.floor(latitude / APPROXIMATE_DEGREES)
    const centreLatitude = Math.max(-90, Math.min(90, (row + 0.5) * APPROXIMATE_DEGREES))
    const step = longitudeStep(centreLatitude)
    const centreLongitude = step >= 360 ? 0 : wrapLongitude((Math.floor(longitude / step) + 0.5) * step)
    return {latitude: centreLatitude, longitude: centreLongitude}
}

/**
 * An approximate share's position (plans/Location-Sharing.md §1, "Approximate when chosen"): the centre of a cell of
 * about 1 km on the fixed grid; the last cell reported is kept until a fix is more than a quarter of a cell inside
 * another one, so jitter at a line does not flip cells (a change of cell still shows that the person crossed a line
 * 0.75 cell from the old centre, within what they move in one approximate-interval-seconds); the stored and answered
 * time to the minute (the push itself goes out when the report comes); no heading or speed; an accuracy of 1000 m, or
 * the device's rounded up to the next 1000.
 */
function approximate(position, latest) {
    let cell = cellOf(position.latitude, position.longitude)
    if (latest && latest.latitude != null && latest.longitude != null) {
        const lastLatitude = Number(latest.latitude)
        const lastLongitude = Number(latest.longitude)
        const rows = Math.abs(position.latitude - lastLatitude) / APPROXIMATE_DEGREES
        const step = longitudeStep(lastLatitude)
        const columns = step >= 360 ? 0 : Math.abs(wrapLongitude(position.longitude - lastLongitude)) / step
        if (rows <= APPROXIMATE_KEEP_CELLS && columns <= APPROXIMATE_KEEP_CELLS)
            cell = {latitude: lastLatitude, longitude: lastLongitude}
    }
    return {
        latitude: round6(cell.latitude),
        longitude: round6(cell.longitude),
        accuracy: Math.max(
            Math.ceil((position.accuracy ?? 0) / APPROXIMATE_ACCURACY) * APPROXIMATE_ACCURACY,
            APPROXIMATE_ACCURACY
        ),
        heading: null,
        speed: null,
        observed: Math.floor(position.observed / MINUTE) * MINUTE
    }
}

/**
 * The cell the owner's approximate shares last reported: the newest latest position of the owner's active approximate
 * shares, this one among them, so that every approximate share of a person keeps one cell (a new share does not snap to
 * the cell beside a line while another is still held).
 */
function lastCellOf(share, now) {
    const shareIds = Array.from(
        find(SHARE)
            .condition("ownerUserId", share.ownerUserId)
            .condition("precisionEnumId", "LspApproximate")
            .condition("revokedDate", null)
            .condition("endedDate", null)
            .condition("thruDate", "greater", new Date(now))
            .selectField("shareId")
            .list()
    ).map(row => row.shareId)
    if (!shareIds.length) return null
    return Array.from(find(LATEST).condition("shareId", "in", shareIds).orderBy("-receivedDate").limit(1).list())[0] || null
}

/** report#Position: from the owner, for an active share, at most one a min-interval-seconds; rounded here when approximate. */
function report(p) {
    const now = nowMillis()
    const share = ownShare(p.shareId, true)
    if (!share) return notFound(404)
    if (!isActive(share, now)) return {statusCode: 410, reasonCode: "SHARE_ENDED"}
    const latest = find(LATEST).condition("shareId", share.shareId).one()
    const approximated = share.precisionEnumId === "LspApproximate"
    const minimum = setting("min-interval-seconds", 5)
    // an approximate share takes a position less often, so its cell changes tell less of a path
    const intervalMillis = (approximated ? Math.max(minimum, setting("approximate-interval-seconds", 60)) : minimum) * 1000
    const received = latest ? millis(latest.receivedDate) : null
    if (received != null && now - received < intervalMillis)
        return {
            statusCode: 429,
            reasonCode: "RATE_LIMITED",
            retryAfterSeconds: Math.max(1, Math.ceil((intervalMillis - (now - received)) / 1000))
        }
    const checked = checkPosition(p, now, share)
    if (checked.reasonCode) return checked
    // an approximate share's stored time is its minute, so a later fix of the same minute replaces that minute's
    const latestObserved = latest && latest.observedDate != null ? millis(latest.observedDate) : null
    if (
        latestObserved != null &&
        (approximated ? checked.position.observed < latestObserved : checked.position.observed <= latestObserved)
    )
        return {statusCode: 409, reasonCode: "STALE"}
    const position = approximated ? approximate(checked.position, lastCellOf(share, now) || latest) : checked.position
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
            .createOrUpdate()
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
    // a group entry whose snapshot holds the caller by a membership row of the group that is still current
    const memberships = Array.from(
        find(GROUP_MEMBER)
            .condition("userId", me)
            .conditionDate("fromDate", "thruDate", null)
            .selectField("userGroupId,fromDate")
            .list()
    )
    const current = new Set(memberships.map(member => member.userGroupId + "|" + millis(member.fromDate)))
    const snapshots = Array.from(
        find(AUDIENCE_MEMBER).condition("userId", me).selectField("shareId,audienceSeqId,memberFromDate").list()
    )
    if (memberships.length && snapshots.length) {
        const held = {}
        for (const row of snapshots) held[row.shareId + "|" + row.audienceSeqId] = millis(row.memberFromDate)
        const rows = find(AUDIENCE)
            .condition("shareId", "in", Array.from(new Set(snapshots.map(row => row.shareId))))
            .condition("audienceTypeEnumId", "LsaUserGroup")
            .condition("revokedDate", null)
            .selectField("shareId,audienceSeqId,userGroupId")
        for (const row of rows.list())
            if (current.has(row.userGroupId + "|" + held[row.shareId + "|" + row.audienceSeqId])) ids.add(row.shareId)
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

/**
 * The rows of a stage of the clean, in batches by (orderField, shareId) from the oldest, each share in its own
 * transaction: a failure is logged by the share's id, the clean goes on, and the shares that failed are tried once more
 * after the rest. A stage ends at a short batch, or after CLEAN_STAGE_MILLIS of the clock.
 */
function eachShare(makeFind, orderField, counts, countName, work) {
    const started = Date.now()
    const failed = []
    // work answers how many it did; that is counted only once its transaction has committed
    const run = (shareId, last) => {
        try {
            counts[countName] += Number(
                ec.transaction.runRequireNew(60, "Could not clean a location share", () => work(shareId)) || 0
            )
            return true
        } catch (err) {
            if (last) {
                counts.failedCount++
                ec.logger.error(`Could not clean location share ${shareId}: ${err && err.message ? err.message : err}`)
            }
            return false
        }
    }
    const conditions = ec.entity.conditionFactory
    let after = null
    while (Date.now() - started < CLEAN_STAGE_MILLIS) {
        let batch = makeFind().selectField("shareId," + orderField)
        if (after != null)
            batch = batch.condition(
                conditions.makeCondition(
                    conditions.makeCondition(orderField, "greater", after.value),
                    "or",
                    conditions.makeCondition(
                        conditions.makeCondition(orderField, "equals", after.value),
                        "and",
                        conditions.makeCondition("shareId", "greater", after.shareId)
                    )
                )
            )
        const rows = Array.from(
            batch
                .orderBy(orderField + ",shareId")
                .limit(CLEAN_BATCH)
                .list()
        )
        for (const row of rows) if (!run(row.shareId, false)) failed.push(row.shareId)
        if (rows.length < CLEAN_BATCH) break
        const last = rows[rows.length - 1]
        after = {value: last[orderField], shareId: last.shareId}
    }
    for (const shareId of failed) run(shareId, true)
}

/** clean#LocationShares, hourly: the shares past their end ended, the trails past their days deleted, old shares removed. */
function clean() {
    const now = nowMillis()
    const counts = {endedCount: 0, trailDeletedCount: 0, removedCount: 0, failedCount: 0}
    const maxDays = setting("trail-max-days", 30)
    eachShare(
        () => find(SHARE).condition("endedDate", null).condition("thruDate", "less-equals", new Date(now)),
        "thruDate",
        counts,
        "endedCount",
        shareId => {
            const share = shareOf(shareId, true)
            if (!share || share.endedDate != null) return 0
            endShare(share, now)
            return 1
        }
    )
    eachShare(
        () => find(SHARE).condition("trailPurgeDate", "less-equals", new Date(now)),
        "trailPurgeDate",
        counts,
        "trailDeletedCount",
        shareId => {
            const share = shareOf(shareId, true)
            if (!share) return 0
            const deleted = find(TRAIL).condition("shareId", shareId).deleteAll()
            share.trailPurgeDate = null
            share.update()
            return deleted
        }
    )
    eachShare(
        () =>
            find(SHARE)
                .condition("trailPurgeDate", null)
                .condition("endedDate", "less", new Date(now - maxDays * DAY)),
        "endedDate",
        counts,
        "removedCount",
        shareId => {
            if (find(TRAIL).condition("shareId", shareId).count() > 0) return 0
            find(LATEST).condition("shareId", shareId).deleteAll()
            find(AUDIENCE_MEMBER).condition("shareId", shareId).deleteAll()
            find(AUDIENCE).condition("shareId", shareId).deleteAll()
            find(SHARE).condition("shareId", shareId).deleteAll()
            return 1
        }
    )
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
