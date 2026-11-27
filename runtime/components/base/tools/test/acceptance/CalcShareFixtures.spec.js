// The CalcGrid share walk's fixture (client/CalcGrid, "a share taken away ..."; DASH-UI-008), run on the walks' server
// by client/suite.js: calc.share.walk, a second Tools user (ADMIN) with a password; CALC_SHARE_WALK, a group of
// john.doe's and theirs that the walk's document is shared with; and walk-calc-share-board, a dashboard of
// calc.share.walk's that the walk adds a range panel to. Each row is made when it is missing.
// Run: ./moq accept tools --spec CalcGrid, which brings this spec in
const PASSWORD = "CalcShareWalk1!!"
const one = (entityName, fields) => ec.entity.find(entityName).condition(fields).disableAuthz().one()
const ensure = (entityName, key, fields) => {
    if (!one(entityName, key)) ec.entity.makeValue(entityName).setAll(Object.assign({}, key, fields || {})).create()
}
const member = (userGroupId, userId) => ensure("moqui.security.UserGroupMember", {userGroupId, userId}, {fromDate: ec.user.nowTimestamp})

spec({
    setupSpec() {
        ec.artifactExecution.disableAuthz()
        ensure("moqui.security.UserAccount", {userId: "CALC_SHARE_WALK"}, {username: "calc.share.walk", userFullName: "Calc share walk"})
        const account = ec.entity.find("moqui.security.UserAccount").condition("userId", "CALC_SHARE_WALK").one()
        account.setFields({currentPassword: ec.user.hashPassword(PASSWORD), passwordSalt: "", passwordHashType: "argon2", passwordBase64: "N",
            requirePasswordChange: "N", disabled: "N", successiveFailedLogins: 0}, true, null, null)
        account.update()
        ensure("moqui.security.UserGroup", {userGroupId: "CALC_SHARE_WALK"}, {description: "Calc share walk"})
        member("ADMIN", "CALC_SHARE_WALK")
        member("CALC_SHARE_WALK", "CALC_SHARE_WALK")
        member("CALC_SHARE_WALK", "EX_JOHN_DOE")
        ec.artifactExecution.enableAuthz()
        if (!one("moqui.report.Definition", {definitionId: "walk-calc-share-board"})) {
            ec.user.internalLoginUser("calc.share.walk")
            call("org.moqui.report.ReportServices.save#Definition",
                {definition: {id: "walk-calc-share-board", type: "dashboard", title: "Calc share walk board", body: {panels: {}}}})
            ec.user.logoutUser()
        }
    },
})

feature("the CalcGrid share walk's user and group are there", {
    expect() {
        one("moqui.security.UserAccount", {username: "calc.share.walk"}) != null
        one("moqui.security.UserGroupMember", {userGroupId: "CALC_SHARE_WALK", userId: "EX_JOHN_DOE"}) != null
        one("moqui.report.Definition", {definitionId: "walk-calc-share-board"}) != null
    },
})
