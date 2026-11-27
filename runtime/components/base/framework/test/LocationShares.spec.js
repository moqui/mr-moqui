// Shared live location (plans/Location-Sharing.md §3.2, LL1; org.moqui.maps.LocationServices, LocationShares.js), by the
// grants of data/MapsLocationData.xml with authorization on: a share is read and pushed to its current audience only, a
// group member taken out loses it at the next read, an ended share shows nothing and its latest position is cleaned, a
// trail is kept only when chosen and deleted or purged, only the owner reports, at most one a min-interval-seconds,
// an approximate share is rounded before it is stored or sent, an ADMIN reads nothing of another's, the audit rows hold
// no position, and a link's token is returned once with only its digest kept.
// Run: ./moq test framework --spec LocationShares
const S = "org.moqui.maps.LocationServices."
const A = "LOC_SHARE_A"
const B = "LOC_SHARE_B"
const C = "LOC_SHARE_C"
const GROUP = "LOC_SHARE_GROUP"
const OTHER_GROUP = "LOC_SHARE_OTHER"
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

spec({
    stepwise: true,
    setupSpec() {
        shared.now = Math.floor(Date.now() / 1000) * 1000
        at(shared.now)
        sys(() => {
            for (const userId of [A, B, C]) {
                if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() === 0)
                    ec.entity
                        .makeValue("moqui.security.UserAccount")
                        .setAll({userId, username: username(userId), userFullName: "Loc " + userId.slice(10), disabled: "N"})
                        .create()
            }
        })
        make("moqui.security.UserGroup", {userGroupId: GROUP, description: "Location shares spec"})
        make("moqui.security.UserGroup", {userGroupId: OTHER_GROUP, description: "Location shares spec, without A"})
        for (const userId of [A, B])
            make("moqui.security.UserGroupMember", {userGroupId: GROUP, userId, fromDate: new Date(shared.now - DAY)})
        make("moqui.security.UserGroupMember", {userGroupId: OTHER_GROUP, userId: C, fromDate: new Date(shared.now - DAY)})
        capture().enable()
        capture().clear()
    },
    cleanupSpec() {
        capture().disable()
        capture().clear()
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
                        .reasonCode
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
        }
    }
)

feature("a group's member reads it while in the group, and taken out of it reads nothing and gets no more pushes", {
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
        later(6000)
        const second = report(shareId, "45.6", "-122.6")
        return {
            refused,
            before,
            second,
            listed: sharedWith(B).filter(row => row.shareId === shareId).length,
            got: as(B, "get#Share", {shareId}).reasonCode,
            pushedToB: pushes(shareId).filter(one => one.event === "position" && one.userIds.includes(B)).length
        }
    },
    then({refused, before, second, listed, got, pushedToB}) {
        refused.notMine == "GROUP_NOT_ALLOWED"
        refused.everyone == "GROUP_NOT_ALLOWED"
        before.listed == 1
        before.pushedToB == 1
        second.statusCode == 200
        listed == 0
        got == "SHARE_NOT_FOUND"
        pushedToB == 1
    }
})

feature(
    "at most one report a min-interval-seconds, and an approximate share is rounded to about 1 km before it is stored or sent",
    {
        when() {
            const shareId = as(A, "create#Share", {
                audienceList: [toUser(B)],
                thruDate: new Date(shared.now + HOUR),
                precisionEnumId: "LspApproximate"
            }).shareId
            later(1000)
            const first = report(shareId, "40.745210", "-111.875", {accuracy: "8", heading: "180", speed: "3"})
            const stored = latestOf(shareId)
            later(1000)
            const soon = report(shareId, "40.7459", "-111.8755")
            later(5000)
            const near = report(shareId, "40.7459", "-111.8755")
            return {
                first,
                stored: {
                    latitude: stored.latitude,
                    longitude: stored.longitude,
                    accuracy: stored.accuracy,
                    heading: stored.heading,
                    speed: stored.speed
                },
                soon,
                near,
                again: latestOf(shareId),
                pushed: pushes(shareId).filter(one => one.event === "position"),
                seen: as(B, "get#Share", {shareId}).latest
            }
        },
        then({first, stored, soon, near, again, pushed, seen}) {
            first.statusCode == 200
            stored.latitude == 40.75
            stored.longitude != -111.875
            Math.abs(stored.longitude + 111.875) < 0.01
            stored.accuracy == 1000
            stored.heading == null
            stored.speed == null
            soon.statusCode == 429
            soon.reasonCode == "RATE_LIMITED"
            soon.retryAfterSeconds == 4
            near.statusCode == 200
            again.latitude == stored.latitude
            again.longitude == stored.longitude
            pushed.length == 2
            pushed.every(
                one =>
                    String(one.latitude) == String(stored.latitude) &&
                    String(one.longitude) == String(stored.longitude) &&
                    one.heading == null &&
                    one.approximate === true
            )
            seen.latitude == stored.latitude
            seen.longitude == stored.longitude
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
        cleaned.trailDeletedCount == 1
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
                audienceList: [toUser(B), {audienceTypeEnumId: "LsaUserGroup", userGroupId: GROUP}],
                thruDate: new Date(shared.now + HOUR)
            }).shareId
            later(6000)
            report(shareId, "35.6812", "139.7671")
            const extended = as(A, "update#Share", {shareId, thruDate: new Date(shared.now + 3 * HOUR)})
            const audience = rows("moqui.maps.LocationShareAudience", {shareId})
            const revoked = as(A, "revoke#Audience", {
                shareId,
                audienceSeqId: audience.find(row => row.userId === B).audienceSeqId
            })
            const afterRevoke = as(B, "get#Share", {shareId}).reasonCode
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
            userAdded[0].newValueText == B
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
