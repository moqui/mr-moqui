// Shared live location (plans/Location-Sharing.md §3.2, LL1; org.moqui.maps.LocationServices, LocationShares.js), by
// the grants of data/MapsLocationData.xml with authorization on: a share is read and pushed to its current audience
// only, a group member taken out loses it at the next read, and a membership added or reopened after the owner added
// the group (with a caller's dates and stamp, backdated, an ended row reopened, an ended row and a new one, a member
// removed and added again) gives nothing until the owner adds it again, which names and tells only the people it lets
// in, never one who blocked the owner; a blocked user and an audience or a share count past its limit are
// refused, on update too; an ended share shows nothing and its latest position is cleaned, a trail is kept only when
// chosen and deleted or purged, only the owner reports, at most one a min-interval-seconds, a device clock ahead and a
// time before the share told apart, an approximate share's cell (one fixed grid, the same for two shares, kept through
// jitter at a line, its accuracy 1000 m or the device's rounded up, its time to the minute) made before it is stored or
// sent, an ADMIN reads nothing of another's, the audit rows hold no position, and a link's token is returned once with
// only its digest kept; the owner's lock row is made on first use, and a group of more than four times the audience cap
// is refused before its snapshot is written (the LL1 review); and the effective configuration's locationShares (LL2, Maps
// › Browse) hold the owner's own share and the audience's, nobody else's; a share started on a device (LL3, §3.4) takes
// that device's reports only, another device or a browser is refused as OTHER_DEVICE, Send from here and Send from this
// device move it, the app's reportClientLocation and stopClientLocation take it, and the device key's renewal keeps it. It
// ends its memberships and stops its shares in cleanup, so it runs again on --db existing.
// Run: ./moq test framework --spec LocationShares
const S = "org.moqui.maps.LocationServices."
const A = "LOC_SHARE_A"
const B = "LOC_SHARE_B"
const C = "LOC_SHARE_C"
// D: a member of the group from before the share; F: an ended member whose row is reopened; G: an ended member given a
// new row
const D = "LOC_SHARE_D"
const F = "LOC_SHARE_F"
const G = "LOC_SHARE_G"
// H: added after the share with a caller's fromDate and lastUpdatedStamp of a day before; I: a member who blocked A
const H = "LOC_SHARE_H"
const I = "LOC_SHARE_I"
// J: in no group, for the audit feature's revoked entry
const J = "LOC_SHARE_J"
const USERS = [A, B, C, D, F, G, H, I, J]
const GROUP = "LOC_SHARE_GROUP"
const OTHER_GROUP = "LOC_SHARE_OTHER"
// a group of five current members, for the cap checked before a group's snapshot
const BIG_GROUP = "LOC_SHARE_BIG"
const MINUTE = 60000
const HOUR = 3600000
const DAY = 86400000
const shared = {}

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
const username = userId => (userId === "EX_JOHN_DOE" ? "john.doe" : userId.toLowerCase().replace(/_/g, "."))
// a call as the user, with authorization on: its results, and its errors as text
function as(userId, verbNoun, parameters) {
    ec.user.internalLoginUser(username(userId))
    ec.artifactExecution.enableAuthz()
    let out = {}
    let thrown = ""
    try {
        out =
            ec.service
                .sync()
                .name(S + verbNoun)
                .parameters(parameters || {})
                .call() || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
const make = (entity, fields) => sys(() => ec.entity.makeValue(entity).setAll(fields).createOrUpdate())
const rows = (entity, fields) => sys(() => Array.from(ec.entity.find(entity).condition(fields).list()))
const latestOf = shareId => sys(() => ec.entity.find("moqui.maps.LocationLatest").condition("shareId", shareId).one())
const trailCount = shareId => sys(() => ec.entity.find("moqui.maps.LocationTrailPoint").condition("shareId", shareId).count())
// time moves forward only: each step past the rate limit's interval
const at = millis => ec.user.setEffectiveTime(millis)
const later = millis => {
    shared.now += millis
    at(shared.now)
    return shared.now
}
const toUser = userId => ({audienceTypeEnumId: "LsaUser", userId})
const toGroup = userGroupId => ({audienceTypeEnumId: "LsaUserGroup", userGroupId})
const CELL = 0.01
const report = (shareId, latitude, longitude, extra) =>
    as(A, "report#Position", Object.assign({shareId, latitude, longitude, observedDate: new Date(shared.now)}, extra || {}))
const capture = () => ec.factory.getTool("NotificationCapture")
const pushes = shareId =>
    capture()
        .getMessages("locationShare:" + shareId)
        .map(row =>
            Object.assign(
                {userIds: Array.from(row.userIds || [])},
                typeof row.message === "string" ? JSON.parse(row.message) : row.message
            )
        )
const sharedWith = userId => Array.from(as(userId, "find#SharedWithMe").shareList || [])
// the run with blocker having blocked A in chat (ChatUserBlock), then the block taken off
function blockedBy(blocker, run) {
    make("moqui.chat.ChatUserBlock", {userId: blocker, blockedUserId: A, blockedDate: new Date(shared.now)})
    try {
        return run()
    } finally {
        sys(() => ec.entity.find("moqui.chat.ChatUserBlock").condition({userId: blocker, blockedUserId: A}).deleteAll())
    }
}

spec({
    stepwise: true,
    setupSpec() {
        shared.now = Math.floor(Date.now() / 1000) * 1000
        at(shared.now)
        sys(() => {
            for (const userId of USERS) {
                if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() === 0)
                    ec.entity
                        .makeValue("moqui.security.UserAccount")
                        .setAll({userId, username: username(userId), userFullName: "Loc " + userId.slice(10), disabled: "N"})
                        .create()
            }
        })
        make("moqui.security.UserGroup", {userGroupId: GROUP, description: "Location shares spec"})
        make("moqui.security.UserGroup", {userGroupId: OTHER_GROUP, description: "Location shares spec, without A"})
        for (const userId of [A, B, D, I])
            make("moqui.security.UserGroupMember", {userGroupId: GROUP, userId, fromDate: new Date(shared.now - DAY)})
        make("moqui.chat.ChatUserBlock", {userId: I, blockedUserId: A, blockedDate: new Date(shared.now)})
        for (const userId of [F, G])
            make("moqui.security.UserGroupMember", {
                userGroupId: GROUP,
                userId,
                fromDate: new Date(shared.now - 2 * DAY),
                thruDate: new Date(shared.now - DAY)
            })
        make("moqui.security.UserGroupMember", {userGroupId: OTHER_GROUP, userId: C, fromDate: new Date(shared.now - DAY)})
        capture().enable()
        capture().clear()
    },
    cleanupSpec() {
        capture().disable()
        capture().clear()
        // the spec's shares stopped and its memberships ended, by the clock, so a run on --db existing starts clean
        for (const share of Array.from(as(A, "find#MyShares").shareList || [])) as(A, "stop#Share", {shareId: share.shareId})
        sys(() =>
            ec.entity
                .find("moqui.security.UserGroupMember")
                .condition("userGroupId", "in", [GROUP, OTHER_GROUP, BIG_GROUP])
                .condition("userId", "in", USERS)
                .condition("thruDate", null)
                .updateAll({thruDate: new Date(Date.now() - 1000)})
        )
        sys(() =>
            ec.entity.find("moqui.chat.ChatUserBlock").condition("userId", "in", [D, I]).condition("blockedUserId", A).deleteAll()
        )
        ec.user.logoutUser()
    }
})

feature(
    "a share is read by its audience alone and pushed to it alone; another person, an ADMIN too, reads nothing and may not report",
    {
        when() {
            const made = as(A, "create#Share", {
                audienceList: [toUser(B)],
                thruDate: new Date(shared.now + 2 * HOUR),
                label: "Alex"
            })
            shared.first = made.shareId
            const reported = report(made.shareId, "40.745210", "-111.875", {accuracy: "12", heading: "90", speed: "1.5"})
            const byB = as(B, "report#Position", {shareId: made.shareId, latitude: "1", longitude: "2"})
            const told = capture()
                .getMessages("LocationShared")
                .filter(row => (typeof row.message === "string" ? JSON.parse(row.message) : row.message).shareId === made.shareId)
            return {
                made,
                reported,
                byB,
                told,
                pushed: pushes(made.shareId).filter(one => one.event === "position"),
                seen: sharedWith(B).find(row => row.shareId === made.shareId),
                byC: sharedWith(C).filter(row => row.shareId === made.shareId).length,
                getC: as(C, "get#Share", {shareId: made.shareId}).reasonCode,
                getAdmin: as("EX_JOHN_DOE", "get#Share", {shareId: made.shareId}).reasonCode,
                byAdmin: sharedWith("EX_JOHN_DOE").filter(row => row.shareId === made.shareId).length,
                stopAdmin: as("EX_JOHN_DOE", "stop#Share", {shareId: made.shareId}).reasonCode,
                generic: WebUtilities.isRestrictedGenericEntity("moqui.maps.LocationLatest"),
                latest: latestOf(made.shareId),
                refused: {
                    self: as(A, "create#Share", {audienceList: [toUser(A)], thruDate: new Date(shared.now + HOUR)}).reasonCode,
                    nobody: as(A, "create#Share", {
                        audienceList: [toUser("LOC_SHARE_NOBODY")],
                        thruDate: new Date(shared.now + HOUR)
                    }).reasonCode,
                    tooLong: as(A, "create#Share", {audienceList: [toUser(B)], thruDate: new Date(shared.now + 25 * HOUR)})
                        .reasonCode,
                    blocked: blockedBy(
                        C,
                        () => as(A, "create#Share", {audienceList: [toUser(C)], thruDate: new Date(shared.now + HOUR)}).reasonCode
                    ),
                    tooLarge: withConf(
                        "location@max-audience",
                        1,
                        () =>
                            as(A, "create#Share", {audienceList: [toUser(B), toUser(C)], thruDate: new Date(shared.now + HOUR)})
                                .reasonCode
                    ),
                    tooMany: withConf(
                        "location@max-active-shares",
                        1,
                        () => as(A, "create#Share", {audienceList: [toUser(B)], thruDate: new Date(shared.now + HOUR)}).reasonCode
                    )
                }
            }
        },
        then({made, reported, byB, told, pushed, seen, byC, getC, getAdmin, byAdmin, stopAdmin, generic, latest, refused}) {
            made.errors == ""
            reported.statusCode == 200
            told.length == 1
            told[0].userIds.join() == B
            String(told[0].title).startsWith("Loc A shares their location with you until ")
            pushed.length == 1
            pushed[0].userIds.join() == B
            pushed[0].latitude == 40.74521
            seen.ownerName == "Loc A"
            seen.latest.latitude == 40.74521
            seen.latest.accuracy == 12
            byC == 0
            getC == "SHARE_NOT_FOUND"
            getAdmin == "SHARE_NOT_FOUND"
            byAdmin == 0
            stopAdmin == "SHARE_NOT_FOUND"
            generic == true
            byB.statusCode == 404
            byB.reasonCode == "SHARE_NOT_FOUND"
            latest.latitude == 40.74521
            refused.self == "BAD_AUDIENCE"
            refused.nobody == "USER_NOT_FOUND"
            refused.tooLong == "TOO_LONG"
            refused.blocked == "USER_NOT_FOUND"
            refused.tooLarge == "AUDIENCE_TOO_LARGE"
            refused.tooMany == "TOO_MANY_SHARES"
        }
    }
)

feature(
    "a group's member reads it while in the group and taken out of it reads nothing; a membership added or reopened after the share gets nothing until the owner adds the group again, which names whom it lets in",
    {
        when() {
            const refused = {
                notMine: as(A, "create#Share", {
                    audienceList: [{audienceTypeEnumId: "LsaUserGroup", userGroupId: OTHER_GROUP}],
                    thruDate: new Date(shared.now + HOUR)
                }).reasonCode,
                everyone: as(A, "create#Share", {
                    audienceList: [{audienceTypeEnumId: "LsaUserGroup", userGroupId: "ALL_USERS"}],
                    thruDate: new Date(shared.now + HOUR)
                }).reasonCode
            }
            const shareId = as(A, "create#Share", {
                audienceList: [{audienceTypeEnumId: "LsaUserGroup", userGroupId: GROUP}],
                thruDate: new Date(shared.now + HOUR)
            }).shareId
            later(6000)
            report(shareId, "45.5", "-122.6")
            const before = {
                listed: sharedWith(B).filter(row => row.shareId === shareId).length,
                pushedToB: pushes(shareId).filter(one => one.event === "position" && one.userIds.includes(B)).length
            }
            sys(() =>
                ec.entity
                    .find("moqui.security.UserGroupMember")
                    .condition({userGroupId: GROUP, userId: B})
                    .updateAll({thruDate: new Date(shared.now - 1000)})
            )
            // after A added the group: C added with a start backdated before the share, F's ended row reopened, and G's
            // ended row left with a new current row; none of them is let in
            later(1000)
            make("moqui.security.UserGroupMember", {userGroupId: GROUP, userId: C, fromDate: new Date(shared.now - DAY)})
            sys(() =>
                ec.entity
                    .find("moqui.security.UserGroupMember")
                    .condition({userGroupId: GROUP, userId: F})
                    .updateAll({thruDate: null})
            )
            make("moqui.security.UserGroupMember", {userGroupId: GROUP, userId: G, fromDate: new Date(shared.now - HOUR)})
            // H with the dates and stamp a System › Access post may give: a start and a lastUpdatedStamp a day before
            // the share
            make("moqui.security.UserGroupMember", {
                userGroupId: GROUP,
                userId: H,
                fromDate: new Date(shared.now - DAY),
                lastUpdatedStamp: new Date(shared.now - DAY)
            })
            // B, removed above, is put back by a new row: still nothing until the owner adds the group again
            later(1000)
            make("moqui.security.UserGroupMember", {userGroupId: GROUP, userId: B, fromDate: new Date(shared.now - 500)})
            later(6000)
            const second = report(shareId, "45.6", "-122.6")
            const removed = {
                listed: sharedWith(B).filter(row => row.shareId === shareId).length,
                got: as(B, "get#Share", {shareId}).reasonCode,
                pushed: pushes(shareId).filter(one => one.event === "position" && one.userIds.includes(B)).length
            }
            const late = [C, F, G, H, I].map(userId => ({
                listed: sharedWith(userId).filter(row => row.shareId === shareId).length,
                got: as(userId, "get#Share", {shareId}).reasonCode,
                pushed: pushes(shareId).filter(one => one.userIds.includes(userId)).length
            }))
            // the audience cap holds on an update and on a group added again
            const capped = {
                user: withConf(
                    "location@max-audience",
                    1,
                    () => as(A, "update#Share", {shareId, addAudienceList: [toUser(B)]}).reasonCode
                ),
                group: withConf(
                    "location@max-audience",
                    2,
                    () => as(A, "update#Share", {shareId, addAudienceList: [toGroup(GROUP)]}).reasonCode
                )
            }
            // the owner adds the group again: B, C, F, G and H are let in, named to the owner and told; D, reached
            // until now but blocking A since, is left out and told; I, who blocked A from the start, is never reached
            // D blocks A first: the re-add leaves D out and tells D so
            later(1000)
            make("moqui.chat.ChatUserBlock", {userId: D, blockedUserId: A, blockedDate: new Date(shared.now)})
            const readded = as(A, "update#Share", {shareId, addAudienceList: [toGroup(GROUP)]})
            sys(() => ec.entity.find("moqui.chat.ChatUserBlock").condition({userId: D, blockedUserId: A}).deleteAll())
            later(6000)
            report(shareId, "45.7", "-122.6")
            const told = userId =>
                capture()
                    .getMessages("LocationShared")
                    .filter(
                        row =>
                            row.userIds.includes(userId) &&
                            (typeof row.message === "string" ? JSON.parse(row.message) : row.message).shareId === shareId
                    ).length
            const restamp = sys(() =>
                Array.from(
                    ec.entity
                        .find("moqui.entity.EntityAuditLog")
                        .condition({
                            pkPrimaryValue: shareId,
                            changedEntityName: "moqui.maps.LocationShareAudience",
                            changedFieldName: "fromDate"
                        })
                        .list()
                )
            )
            return {
                refused,
                before,
                second,
                late,
                capped,
                readded,
                addedIds: Array.from(readded.addedUserList || [])
                    .map(row => row.userId)
                    .sort()
                    .join(),
                addedNames: Array.from(readded.addedUserList || [])
                    .map(row => row.userName)
                    .sort()
                    .join(),
                ownerPush: pushes(shareId).filter(one => one.event === "changed" && one.userIds.join() === A && one.addedUserList)
                    .length,
                removed,
                toldNew: [C, F, G, H].map(told),
                toldB: told(B),
                toldD: told(D),
                toldI: told(I),
                readsNew: [B, C, F, G, H].map(userId => sharedWith(userId).filter(row => row.shareId === shareId).length),
                readsI: sharedWith(I).filter(row => row.shareId === shareId).length,
                droppedD: pushes(shareId).filter(one => one.event === "revoked" && one.userIds.join() === D).length,
                readsD: sharedWith(D).filter(row => row.shareId === shareId).length,
                pushedToC: pushes(shareId).filter(one => one.event === "position" && one.userIds.includes(C)).length,
                restamps: restamp.filter(row => row.oldValueText != null).length
            }
        },
        then({
            refused,
            before,
            second,
            late,
            capped,
            readded,
            addedIds,
            addedNames,
            ownerPush,
            removed,
            toldNew,
            toldB,
            toldD,
            toldI,
            readsNew,
            readsI,
            droppedD,
            readsD,
            pushedToC,
            restamps
        }) {
            refused.notMine == "GROUP_NOT_ALLOWED"
            refused.everyone == "GROUP_NOT_ALLOWED"
            before.listed == 1
            before.pushedToB == 1
            second.statusCode == 200
            removed.listed == 0
            removed.got == "SHARE_NOT_FOUND"
            removed.pushed == 1
            late.every(one => one.listed === 0 && one.got === "SHARE_NOT_FOUND" && one.pushed === 0)
            capped.user == "AUDIENCE_TOO_LARGE"
            capped.group == "AUDIENCE_TOO_LARGE"
            readded.errors == ""
            addedIds == [B, C, F, G, H].sort().join()
            addedNames == "Loc B,Loc C,Loc F,Loc G,Loc H"
            ownerPush == 1
            toldNew.every(count => count === 1)
            toldB == 2
            toldD == 1
            toldI == 0
            readsNew.every(count => count === 1)
            readsI == 0
            droppedD == 1
            readsD == 0
            pushedToC == 1
            restamps == 1
        }
    }
)

feature("at most one report a min-interval-seconds; a device clock ahead and a time before the share are told apart", {
    when() {
        const shareId = as(A, "create#Share", {audienceList: [toUser(B)], thruDate: new Date(shared.now + HOUR)}).shareId
        later(1000)
        const first = report(shareId, "40.7", "-111.8")
        later(1000)
        const soon = report(shareId, "40.7", "-111.8")
        later(5000)
        const after = report(shareId, "40.7", "-111.8")
        later(5000)
        const ahead = report(shareId, "40.7", "-111.8", {observedDate: new Date(shared.now + 2 * MINUTE)})
        const before = report(shareId, "40.7", "-111.8", {observedDate: new Date(shared.now - HOUR)})
        return {first, soon, after, ahead, before}
    },
    then({first, soon, after, ahead, before}) {
        first.statusCode == 200
        soon.statusCode == 429
        soon.reasonCode == "RATE_LIMITED"
        soon.retryAfterSeconds == 4
        after.statusCode == 200
        ahead.statusCode == 409
        ahead.reasonCode == "CLOCK_AHEAD"
        ahead.errors == ""
        before.statusCode == 400
        before.reasonCode == "BEFORE_SHARE"
    }
})

feature(
    "an approximate share is its cell of about 1 km on one fixed grid, kept through jitter at a cell line for the person across their shares, its time to the minute, before it is stored or sent",
    {
        when() {
            const shareId = as(A, "create#Share", {
                audienceList: [toUser(B)],
                thruDate: new Date(shared.now + HOUR),
                precisionEnumId: "LspApproximate"
            }).shareId
            later(1000)
            const observed = shared.now
            const first = report(shareId, "40.745210", "-111.875", {accuracy: "8", heading: "180", speed: "3"})
            const stored = latestOf(shareId)
            const cell = {latitude: Number(String(stored.latitude)), longitude: Number(String(stored.longitude))}
            later(1000)
            const soon = report(shareId, "40.745210", "-111.875")
            // jitter just past the cell's lines on either side keeps the cell; a fix well inside the next one moves it
            const jitter = []
            for (const rows of [0.6, -0.6, 0.6]) {
                later(MINUTE)
                report(shareId, String(cell.latitude + rows * CELL), String(cell.longitude))
                jitter.push(Number(String(latestOf(shareId).latitude)))
            }
            // a second approximate share of A, started while the first holds its cell, reports a fix just past the
            // line: the same cell, the person's and not the share's, and the device's accuracy rounded up to the next
            // 1000
            const otherId = as(A, "create#Share", {
                audienceList: [toUser(B)],
                thruDate: new Date(shared.now + HOUR),
                precisionEnumId: "LspApproximate"
            }).shareId
            later(1000)
            report(otherId, String(cell.latitude + 0.6 * CELL), String(cell.longitude), {accuracy: "1234.57"})
            const other = latestOf(otherId)
            later(MINUTE)
            report(shareId, String(cell.latitude + CELL), String(cell.longitude))
            const moved = Number(String(latestOf(shareId).latitude))
            const pushed = pushes(shareId).filter(one => one.event === "position")
            return {
                other: {
                    latitude: Number(String(other.latitude)),
                    longitude: Number(String(other.longitude)),
                    accuracy: other.accuracy
                },
                first,
                observed,
                stored: {
                    accuracy: stored.accuracy,
                    heading: stored.heading,
                    speed: stored.speed,
                    observedDate: ec.user.timestampMillis(stored.observedDate)
                },
                cell,
                soon,
                jitter,
                moved,
                pushed
            }
        },
        then({other, first, observed, stored, cell, soon, jitter, moved, pushed}) {
            first.statusCode == 200
            Math.abs(cell.latitude - 40.74521) <= CELL
            Math.abs(cell.longitude + 111.875) <= 2 * CELL
            cell.latitude != 40.74521
            stored.accuracy == 1000
            stored.heading == null
            stored.speed == null
            stored.observedDate == Math.floor(observed / MINUTE) * MINUTE
            soon.statusCode == 429
            soon.retryAfterSeconds == 59
            jitter.every(latitude => latitude === cell.latitude)
            Math.abs(moved - cell.latitude - CELL) < 1e-6
            other.latitude === cell.latitude
            other.longitude === cell.longitude
            other.accuracy == 2000
            pushed.length == 5
            pushed.every(
                one =>
                    one.heading == null &&
                    one.speed == null &&
                    one.approximate === true &&
                    Number(one.observedDate) % MINUTE === 0
            )
        }
    }
)

feature("an ended share shows nothing, refuses reports with 410, and the clean deletes its latest position", {
    when() {
        const shareId = as(A, "create#Share", {audienceList: [toUser(B)], thruDate: new Date(shared.now + HOUR)}).shareId
        later(6000)
        report(shareId, "51.5", "-0.12")
        later(HOUR)
        const kept = latestOf(shareId) != null
        const listed = sharedWith(B).filter(row => row.shareId === shareId).length
        const got = as(B, "get#Share", {shareId}).reasonCode
        const late = report(shareId, "51.5", "-0.12")
        const cleaned = sys(() => call(S + "clean#LocationShares", {}))
        const share = rows("moqui.maps.LocationShare", {shareId})[0]
        return {
            kept,
            listed,
            got,
            late,
            cleaned,
            latest: latestOf(shareId),
            ended: share.endedDate != null,
            pushed: pushes(shareId).filter(one => one.event === "ended")
        }
    },
    then({kept, listed, got, late, cleaned, latest, ended, pushed}) {
        kept == true
        listed == 0
        got == "SHARE_NOT_FOUND"
        late.statusCode == 410
        late.reasonCode == "SHARE_ENDED"
        cleaned.endedCount >= 1
        latest == null
        ended == true
        pushed.length == 1
        pushed[0].userIds.includes(B)
    }
})

feature("a trail is kept only when chosen, deleted by its owner, and purged past its days after the share ends", {
    when() {
        const tooMany = as(A, "create#Share", {
            audienceList: [toUser(B)],
            thruDate: new Date(shared.now + HOUR),
            keepTrail: true,
            trailKeepDays: 31
        }).reasonCode
        const shareId = as(A, "create#Share", {
            audienceList: [toUser(B)],
            thruDate: new Date(shared.now + HOUR),
            keepTrail: true,
            trailKeepDays: 1
        }).shareId
        later(6000)
        report(shareId, "48.85", "2.35")
        later(6000)
        report(shareId, "48.86", "2.35")
        const page = as(B, "get#Share", {shareId, trailPageSize: 1, trailPageIndex: 1})
        const byB = as(B, "delete#Trail", {shareId}).reasonCode
        const deleted = as(A, "delete#Trail", {shareId}).deletedCount
        later(6000)
        report(shareId, "48.87", "2.35")
        const kept = trailCount(shareId)
        const stopped = as(A, "stop#Share", {shareId})
        const latestGone = latestOf(shareId) == null
        later(6000)
        const afterStop = report(shareId, "48.88", "2.35").statusCode
        later(DAY - MINUTE)
        call(S + "clean#LocationShares", {})
        const withinDays = trailCount(shareId)
        later(2 * MINUTE)
        const cleaned = sys(() => call(S + "clean#LocationShares", {}))
        return {
            tooMany,
            noTrail: trailCount(shared.first),
            page,
            byB,
            deleted,
            kept,
            stopped,
            latestGone,
            afterStop,
            withinDays,
            cleaned,
            purged: trailCount(shareId),
            owner: as(A, "get#Share", {shareId})
        }
    },
    then({tooMany, noTrail, page, byB, deleted, kept, stopped, latestGone, afterStop, withinDays, cleaned, purged, owner}) {
        tooMany == "BAD_TRAIL_DAYS"
        noTrail == 0
        page.trailCount == 2
        page.trailPageMaxIndex == 1
        page.trailList.length == 1
        page.trailList[0].latitude == 48.86
        byB == "SHARE_NOT_FOUND"
        deleted == 2
        kept == 1
        stopped.errors == ""
        latestGone == true
        afterStop == 410
        withinDays == 1
        cleaned.trailDeletedCount >= 1
        purged == 0
        owner.latest == null
        owner.share.active == false
    }
})

feature(
    "start, extension, stop and a revoked audience are in the audit log with who and when, and no audit row holds a position",
    {
        when() {
            const shareId = as(A, "create#Share", {
                audienceList: [toUser(J), {audienceTypeEnumId: "LsaUserGroup", userGroupId: GROUP}],
                thruDate: new Date(shared.now + HOUR)
            }).shareId
            later(6000)
            report(shareId, "35.6812", "139.7671")
            const extended = as(A, "update#Share", {shareId, thruDate: new Date(shared.now + 3 * HOUR)})
            const audience = rows("moqui.maps.LocationShareAudience", {shareId})
            const revoked = as(A, "revoke#Audience", {
                shareId,
                audienceSeqId: audience.find(row => row.userId === J).audienceSeqId
            })
            const afterRevoke = as(J, "get#Share", {shareId}).reasonCode
            as(A, "stop#Share", {shareId})
            const audit = sys(() =>
                Array.from(ec.entity.find("moqui.entity.EntityAuditLog").condition("pkPrimaryValue", shareId).list())
            )
            const all = sys(() =>
                Array.from(
                    ec.entity
                        .find("moqui.entity.EntityAuditLog")
                        .condition("changedEntityName", "like", "moqui.maps.Location%")
                        .list()
                )
            )
            const field = (entity, name) =>
                audit.filter(row => row.changedEntityName === "moqui.maps." + entity && row.changedFieldName === name)
            return {
                extended,
                revoked,
                afterRevoke,
                thru: field("LocationShare", "thruDate"),
                stop: field("LocationShare", "revokedDate"),
                userAdded: field("LocationShareAudience", "userId"),
                audienceRevoked: field("LocationShareAudience", "revokedDate"),
                positionEntities: all.filter(row => /Latest|Trail/.test(row.changedEntityName)).length,
                positionFields: all.filter(row => /latitude|longitude|accuracy|heading|speed/i.test(row.changedFieldName)).length,
                positionValues: all.filter(row =>
                    /35\.68|139\.76|40\.74|111\.87/.test(String(row.oldValueText || "") + String(row.newValueText || ""))
                ).length
            }
        },
        then({
            extended,
            revoked,
            afterRevoke,
            thru,
            stop,
            userAdded,
            audienceRevoked,
            positionEntities,
            positionFields,
            positionValues
        }) {
            extended.errors == ""
            revoked.errors == ""
            afterRevoke == "SHARE_NOT_FOUND"
            thru.length == 2
            thru.every(row => row.changedByUserId == A && row.changedDate != null)
            stop.length == 1
            userAdded.length == 1
            userAdded[0].newValueText == J
            audienceRevoked.length == 1
            positionEntities == 0
            positionFields == 0
            positionValues == 0
        }
    }
)

feature("a guest link's token is returned once and only its digest is kept; a share has one link", {
    when() {
        const made = as(A, "create#Share", {
            audienceList: [toUser(B), {audienceTypeEnumId: "LsaLink"}],
            thruDate: new Date(shared.now + HOUR)
        })
        const link = rows("moqui.maps.LocationShareAudience", {shareId: made.shareId, audienceTypeEnumId: "LsaLink"})[0]
        const mine = Array.from(as(A, "find#MyShares").shareList || []).find(row => row.shareId === made.shareId)
        const answers = JSON.stringify([
            mine,
            as(A, "get#Share", {shareId: made.shareId}),
            as(B, "get#Share", {shareId: made.shareId})
        ])
        return {
            made,
            link,
            mine,
            leaked: answers.includes(made.linkToken) || answers.includes(link.tokenDigest),
            twoLinks: as(A, "create#Share", {
                audienceList: [{audienceTypeEnumId: "LsaLink"}, {audienceTypeEnumId: "LsaLink"}],
                thruDate: new Date(shared.now + HOUR)
            }).reasonCode,
            secondLink: as(A, "update#Share", {shareId: made.shareId, addAudienceList: [{audienceTypeEnumId: "LsaLink"}]})
                .reasonCode
        }
    },
    then({made, link, mine, leaked, twoLinks, secondLink}) {
        made.errors == ""
        made.linkToken.length == 32
        link.tokenDigest == ec.user.singleUseCodeHash(made.linkToken)
        link.tokenDigest != made.linkToken
        mine.audienceList.find(row => row.audienceTypeEnumId == "LsaLink").hasLink == true
        leaked == false
        twoLinks == "ONE_LINK"
        secondLink == "ONE_LINK"
    }
})

feature(
    "the owner's lock row is made on first use; a group of more than four times max-audience members is refused before its snapshot",
    {
        when() {
            sys(() => ec.entity.find("moqui.maps.LocationShareOwner").condition("ownerUserId", A).deleteAll())
            const first = as(A, "create#Share", {audienceList: [toUser(B)], thruDate: new Date(shared.now + HOUR)})
            const lockRows = rows("moqui.maps.LocationShareOwner", {ownerUserId: A}).length
            make("moqui.security.UserGroup", {userGroupId: BIG_GROUP, description: "Location shares spec, five members"})
            for (const userId of [A, B, C, I, J])
                make("moqui.security.UserGroupMember", {userGroupId: BIG_GROUP, userId, fromDate: new Date(shared.now - DAY)})
            // five membership rows, but a snapshot of B alone: C and J block A here, and I blocks A from the start, so
            // the cap after the writes passes at a max-audience of 1 and only the check before the snapshot refuses it
            const share = max =>
                withConf("location@max-audience", max, () =>
                    blockedBy(C, () =>
                        blockedBy(J, () =>
                            as(A, "create#Share", {audienceList: [toGroup(BIG_GROUP)], thruDate: new Date(shared.now + HOUR)})
                        )
                    )
                )
            const refused = share(1)
            const allowed = share(2)
            return {
                first,
                lockRows,
                refused,
                allowed,
                snapshot: rows("moqui.maps.LocationShareAudienceMember", {shareId: allowed.shareId || "-"}).map(row => row.userId)
            }
        },
        then({first, lockRows, refused, allowed, snapshot}) {
            first.errors == ""
            first.shareId != null
            lockRows == 1
            refused.reasonCode == "AUDIENCE_TOO_LARGE"
            refused.shareId == null
            allowed.errors == ""
            snapshot.join() == B
        }
    }
)

feature(
    "the effective configuration's locationShares: the owner's own with its audience, the audience's with the latest and the trail, nobody else's",
    {
        when() {
            later(6000)
            const shareId = as(A, "create#Share", {
                audienceList: [toUser(B)],
                thruDate: new Date(shared.now + HOUR),
                label: "Alex live",
                keepTrail: true
            }).shareId
            report(shareId, "47.6", "-122.3", {accuracy: "9"})
            later(6000)
            report(shareId, "47.61", "-122.31", {accuracy: "7"})
            // MapConfig.js liveShares as each user, with authorization on (Browse's getConfig and locationShares)
            const live = userId => {
                ec.user.internalLoginUser(username(userId))
                ec.artifactExecution.enableAuthz()
                try {
                    const all = Array.from(
                        ec.resource.script("component://framework/service/org/moqui/maps/MapConfig.js").execute("liveShares", {})
                            .locationShares || []
                    )
                    return all.find(row => row.shareId === shareId) || null
                } finally {
                    ec.message.clearErrors()
                }
            }
            const config = asked =>
                sys(() => {
                    ec.user.internalLoginUser(username(B))
                    return ec.service
                        .sync()
                        .name("org.moqui.maps.MapServices.get#EffectiveConfig")
                        .parameters({locationShares: asked})
                        .call().config
                })
            const out = {owner: live(A), viewer: live(B), other: live(C), admin: live("EX_JOHN_DOE")}
            out.carried = (config(true).locationShares || []).some(row => row.shareId === shareId)
            out.notAsked = config(false).locationShares === undefined
            // the app bar's chip (get#ShellIndicator, Maps' shell-indicator provider): the owner's, the count of their own
            // shares; none for a viewer who shares nothing, who is still told which pushes to read it again on
            out.indicator = as(A, "get#ShellIndicator")
            out.noIndicator = as(J, "get#ShellIndicator")
            as(A, "stop#Share", {shareId})
            out.afterStop = live(B)
            return out
        },
        then({owner, viewer, other, admin, carried, notAsked, afterStop, indicator, noIndicator}) {
            indicator.indicators.length == 1
            indicator.indicators[0].key == "locationSharing"
            indicator.indicators[0].label.startsWith("Sharing location")
            indicator.indicators[0].url == "/qapps/maps/Browse?panel=live"
            indicator.indicators[0].refreshAt > shared.now
            indicator.refreshOn[0].topicPrefix == "locationShare:"
            noIndicator.indicators.length == 0
            noIndicator.refreshOn.length == 1
            owner.role == "owner"
            owner.label == "Alex live"
            owner.audience.map(entry => entry.name).join() == "Loc B"
            owner.reportIntervalSeconds == 5
            owner.trailCount == 2
            owner.latest.latitude == 47.61
            viewer.role == "viewer"
            viewer.ownerName == "Loc A"
            viewer.latest.latitude == 47.61
            viewer.latest.accuracy == 7
            viewer.audience == null
            JSON.stringify(viewer.trail) ==
                JSON.stringify([
                    [-122.3, 47.6],
                    [-122.31, 47.61]
                ])
            other == null
            admin == null
            carried == true
            notAsked == true
            afterStop == null
        }
    }
)

// A share's device (LL3, plans/Location-Sharing.md §3.4): a device key of A's as the REST login's createLoginKey with
// deviceName makes it, the key and its stored hash
function deviceKey(deviceName) {
    const ids = () =>
        sys(() =>
            Array.from(
                ec.entity
                    .find("moqui.security.UserLoginKey")
                    .condition("userId", A)
                    .list()
            ).map(row => row.loginKey)
        )
    ec.user.internalLoginUser(username(A))
    const before = ids()
    const key = ec.user.getLoginKey()
    const hash = ids().find(found => !before.includes(found))
    sys(() => {
        const row = ec.entity.find("moqui.security.UserLoginKey").condition("loginKey", hash).one()
        row.deviceName = deviceName
        row.update()
    })
    return {key, hash}
}
// a call as A signed in by a device key (none: a browser), with authorization on: its results and errors
function asDevice(device, verbNoun, parameters) {
    ec.user.internalLoginUser(username(A))
    if (device && !ec.user.loginUserKey(device.key)) return {errors: "the device key did not sign in"}
    const kind = ec.user.loginKeyKind
    ec.artifactExecution.enableAuthz()
    let out = {}
    let thrown = ""
    try {
        out =
            ec.service
                .sync()
                .name(S + verbNoun)
                .parameters(parameters || {})
                .call() || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors, kind}, out)
}
const reportFrom = (device, shareId, latitude) => {
    later(6000)
    return asDevice(device, "report#Position", {shareId, latitude, longitude: "-111.9", observedDate: new Date(shared.now)})
}
// a POST to the app's /qapps transition as the app sends it, signed in by the device key (as its login_key header signs
// a request in; the header is given too, which renewClientDevice reads), or by none (this context's login ended alone: a
// logout of the account would end its keys' sign-ins too)
function postApp(transition, parameters, key) {
    ec.user.logoutLocal()
    if (key) {
        ec.user.internalLoginUser(username(A))
        if (!ec.user.loginUserKey(key)) return {status: 401, json: null}
    }
    let test = ec.screen.makeTest().rootScreen("component://webroot/screen/webroot.xml")
    if (key) test = test.header("login_key", key)
    const str = test.render("qapps/" + transition, parameters || {}, "post")
    ec.message.clearErrors()
    let json = null
    try {
        json = JSON.parse(String(str.output || ""))
    } catch (err) {
        json = null
    }
    return {status: str.status, json}
}
const deviceOf = shareId => sys(() => ec.entity.find("moqui.maps.LocationShare").condition("shareId", shareId).one().userLoginKeyId)

feature(
    "a share started on a device takes that device's reports only; another device or a browser is told so, Send from here and Send from this device move it, and a renewed key keeps it",
    {
        when() {
            const phone = deviceKey("Spec phone")
            const tablet = deviceKey("Spec tablet")
            later(6000)
            const made = asDevice(phone, "create#Share", {audienceList: [toUser(B)], thruDate: new Date(shared.now + HOUR)})
            const shareId = made.shareId
            const out = {made, boundTo: deviceOf(shareId), phone, tablet}
            out.fromPhone = reportFrom(phone, shareId, "40.71")
            out.fromBrowser = reportFrom(null, shareId, "40.72")
            out.fromTablet = reportFrom(tablet, shareId, "40.73")
            out.mineOnTablet = Array.from(asDevice(tablet, "find#MyShares").shareList || []).find(row => row.shareId === shareId)
            // the browser's Send from here: no device from now on, and the phone is refused
            out.toBrowser = asDevice(null, "update#Share", {shareId, sendFromHere: true})
            out.browserBound = deviceOf(shareId)
            out.phoneAfter = reportFrom(phone, shareId, "40.74")
            out.browserAfter = reportFrom(null, shareId, "40.75")
            // the tablet's Send from this device, then its report through the app's transition, and none without a user
            out.toTablet = asDevice(tablet, "update#Share", {shareId, sendFromHere: true})
            out.tabletBound = deviceOf(shareId)
            later(6000)
            out.appReport = postApp("reportClientLocation", {shareId, latitude: "40.76", longitude: "-111.9"}, tablet.key)
            out.noUser = postApp("reportClientLocation", {shareId, latitude: "40.77", longitude: "-111.9"}, null)
            out.mineOnPhone = Array.from(asDevice(phone, "find#MyShares").shareList || []).find(row => row.shareId === shareId)
            // the tablet's key renewed: the share goes with it, and the new key reports
            const renewed = postApp("renewClientDevice", {}, tablet.key)
            const renewedKey = renewed.json && renewed.json.loginKey
            out.renewed = renewed
            out.renewedBound = deviceOf(shareId)
            later(6000)
            out.renewedReport = postApp("reportClientLocation", {shareId, latitude: "40.78", longitude: "-111.9"}, renewedKey)
            // a share started in a browser has no device
            later(6000)
            const browserShare = asDevice(null, "create#Share", {audienceList: [toUser(B)], thruDate: new Date(shared.now + HOUR)}).shareId
            out.browserShareBound = deviceOf(browserShare)
            out.stopped = postApp("stopClientLocation", {shareId}, renewedKey)
            out.afterStop = reportFrom(phone, shareId, "40.79")
            as(A, "stop#Share", {shareId: browserShare})
            return out
        },
        then(out) {
            out.made.errors == ""
            out.made.kind == "device"
            out.boundTo == out.phone.hash
            out.fromPhone.statusCode == 200
            out.fromBrowser.statusCode == 409
            out.fromBrowser.reasonCode == "OTHER_DEVICE"
            out.fromBrowser.errors.includes("Another device sends")
            out.fromTablet.statusCode == 409
            out.fromTablet.reasonCode == "OTHER_DEVICE"
            out.mineOnTablet.sentFromDevice == true
            out.mineOnTablet.deviceName == "Spec phone"
            out.mineOnTablet.fromThisDevice == false
            JSON.stringify(out.mineOnTablet).includes(out.phone.hash) == false
            out.toBrowser.errors == ""
            out.browserBound == null
            out.phoneAfter.reasonCode == "OTHER_DEVICE"
            out.browserAfter.statusCode == 200
            out.toTablet.errors == ""
            out.tabletBound == out.tablet.hash
            out.appReport.status == 200
            out.appReport.json.statusCode == 200
            out.noUser.json == null || out.noUser.json.statusCode == null
            out.mineOnPhone.deviceName == "Spec tablet"
            out.mineOnPhone.fromThisDevice == false
            out.renewed.status == 200
            out.renewed.json.loginKey != null
            out.renewedBound != null
            out.renewedBound != out.tablet.hash
            out.renewedReport.json.statusCode == 200
            out.browserShareBound == null
            out.stopped.json.stopped == true
            out.afterStop.statusCode == 410
        }
    }
)
