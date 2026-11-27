// The fixture of the live location walks (plans/Location-Sharing.md LL7; client/LocationShareWalk in a browser, and
// scripts/android-location-share.sh on the emulator, which serves this spec with `moqui test --serve`): three Maps users,
// loc.walk.ann (who shares), loc.walk.ben and loc.walk.cy, all members of the group Walk family (LOC_WALK_FAMILY) by a
// row of a fixed fromDate, so the walk's System › Access post can end Cy's. Cy's row is made again on each run (the walk
// ended it), and Ann's shares left active by an earlier run are stopped as she stops them, so each run starts with none.
// Run: ./moq accept tools --spec LocationShareWalk, which brings this spec in
const PASSWORD = "LocationWalk1!!"
const GROUP = "LOC_WALK_FAMILY"
// the walk's own membership rows: a fixed date, which the walk names when it ends Cy's
const SINCE = new Date(Date.UTC(2026, 0, 1))
const USERS = [
    ["LOC_WALK_ANN", "loc.walk.ann", "Ann Walker"],
    ["LOC_WALK_BEN", "loc.walk.ben", "Ben Walker"],
    ["LOC_WALK_CY", "loc.walk.cy", "Cy Walker"]
]
const find = entityName => ec.entity.find(entityName).disableAuthz().useCache(false)
const ensure = (entityName, key, fields) => {
    if (!find(entityName).condition(key).one())
        ec.entity
            .makeValue(entityName)
            .setAll(Object.assign({}, key, fields || {}))
            .create()
}
function ensureUser(userId, username, userFullName) {
    ensure("moqui.security.UserAccount", {userId}, {username, userFullName})
    const account = find("moqui.security.UserAccount").condition("userId", userId).one()
    account.setFields(
        {
            userFullName,
            currentPassword: ec.user.hashPassword(PASSWORD),
            passwordSalt: "",
            passwordHashType: "argon2",
            passwordBase64: "N",
            requirePasswordChange: "N",
            disabled: "N",
            successiveFailedLogins: 0
        },
        true,
        null,
        null
    )
    account.update()
}
// one current membership row of the user's in the group, of the fixed date: any other row of theirs there removed first
function member(userGroupId, userId, fromDate) {
    const rows = Array.from(find("moqui.security.UserGroupMember").condition({userGroupId, userId}).list())
    const current = rows.find(row => row.thruDate == null && (!fromDate || ec.user.timestampMillis(row.fromDate) === fromDate.getTime()))
    if (current && rows.length === 1) return
    find("moqui.security.UserGroupMember").condition({userGroupId, userId}).deleteAll()
    ec.entity
        .makeValue("moqui.security.UserGroupMember")
        .setAll({userGroupId, userId, fromDate: fromDate || ec.user.nowTimestamp})
        .create()
}

spec({
    setupSpec() {
        ec.artifactExecution.disableAuthz()
        for (const [userId, username, name] of USERS) ensureUser(userId, username, name)
        ensure("moqui.security.UserGroup", {userGroupId: GROUP}, {description: "Walk family"})
        for (const [userId] of USERS) {
            if (!find("moqui.security.UserGroupMember").condition({userGroupId: "MAPS_USERS", userId}).count())
                member("MAPS_USERS", userId, null)
            member(GROUP, userId, SINCE)
        }
        // Ann's shares an earlier run left on, stopped by her as Who sees my location stops them
        ec.user.internalLoginUser("loc.walk.ann")
        ec.artifactExecution.enableAuthz()
        try {
            const mine = ec.service.sync().name("org.moqui.maps.LocationServices.find#MyShares").call() || {}
            for (const share of Array.from(mine.shareList || []))
                ec.service.sync().name("org.moqui.maps.LocationServices.stop#Share").parameters({shareId: share.shareId}).call()
        } finally {
            ec.message.clearErrors()
            ec.user.internalLoginUser("john.doe")
            ec.artifactExecution.disableAuthz()
        }
    }
})

feature("the walk's users are Maps users in Walk family, and Ann has no share on", {
    when() {
        ec.user.internalLoginUser("loc.walk.ann")
        ec.artifactExecution.enableAuthz()
        try {
            const mine = ec.service.sync().name("org.moqui.maps.LocationServices.find#MyShares").call() || {}
            return {
                active: Array.from(mine.shareList || []).length,
                members: Number(find("moqui.security.UserGroupMember").condition({userGroupId: GROUP}).conditionDate("fromDate", "thruDate", null).count())
            }
        } finally {
            ec.message.clearErrors()
            ec.user.internalLoginUser("john.doe")
            ec.artifactExecution.disableAuthz()
        }
    },
    then({active, members}) {
        active == 0
        members == 3
    }
})
