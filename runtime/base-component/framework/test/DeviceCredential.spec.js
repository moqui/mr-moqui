// The shared-device credential, org.moqui.admin.DeviceServices and crates/moqui-core/src/user/device.rs (mr-boss
// plan/impl-admin.md P18; design-crosscut CX-DEVICE; docs/security/Shared-Devices.md): registering needs
// ADMIN_ACCESS_MANAGE and a display account the registrar could assign; the registration code is returned once, stored
// only hashed and works once, for the browser that enters it; that browser's requests then open only the route group, as
// the display account, whatever else that account or a user signed in beside it is authorized for; and a revoked device's
// next request is refused. The cookie's flags, its daily replacement, the copy that stops a device, the session that never
// holds the display account and the sign-ins a display account never passes are cargo's (user::device::tests).
// Run: ./moq test framework --spec DeviceCredential
const S = "org.moqui.admin.DeviceServices."
const DEVICE = "moqui.security.DeviceCredential"
const AUDIT = "moqui.entity.EntityAuditLog"
const wallId = "SPEC_DEV_WALL"
const registrar = "spec.device.registrar"
let registered
let cookie

const make = (entityName, fields) => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
const rows = (entityName, conditions) => Array.from(ec.entity.find(entityName).condition(conditions).disableAuthz().list())
function as(username, action) {
    ec.user.internalLoginUser(username)
    ec.artifactExecution.enableAuthz()
    try {
        return action()
    } finally {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
    }
}
function invoke(name, parameters = {}) {
    try {
        const result = ec.service.sync().name(name).parameters(parameters).call() || {}
        const error = ec.message.hasError() ? ec.message.getErrorsString() : ""
        ec.message.clearErrors()
        return Object.assign({}, result, {error})
    } catch (e) {
        ec.message.clearErrors()
        return {error: String(e.message || e)}
    }
}
// a request of a browser: with the device cookie it is the device's, whoever is signed in on this thread
function request(path, parameters, method, deviceCookie) {
    let test = ec.screen.makeTest()
    if (deviceCookie) test = test.header("cookie", "moqui.device=" + deviceCookie)
    const out = test.render(path, parameters || {}, method || "get")
    ec.message.clearErrors()
    return {
        status: out.status,
        text: String(out.output || ""),
        errors: Array.from(out.errorMessages || []).join("\n"),
        setCookie: out.header("Set-Cookie")
    }
}
const refusedRoute = out => out.errors.includes("This device opens only SPEC_DEV_WALLS")

spec({
    stepwise: true,
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        make("moqui.security.UserAccount", {userId: wallId, username: "spec.device.wall", userFullName: "Spec wall", displayAccount: "Y"})
        make("moqui.security.UserAccount", {userId: "SPEC_DEV_PERSON", username: "spec.device.person", userFullName: "Spec person"})
        make("moqui.security.UserAccount", {userId: "SPEC_DEV_REGISTRAR", username: registrar, userFullName: "Spec registrar"})
        make("moqui.security.UserGroup", {userGroupId: "SPEC_DEV_REGISTRARS", description: "Spec device registrars"})
        for (const permission of ["ADMIN_ACCESS_VIEW", "ADMIN_ACCESS_MANAGE"])
            make("moqui.security.UserGroupPermission", {userGroupId: "SPEC_DEV_REGISTRARS", userPermissionId: permission, fromDate: 0})
        make("moqui.security.UserGroupMember", {userGroupId: "SPEC_DEV_REGISTRARS", userId: "SPEC_DEV_REGISTRAR", fromDate: 0})
        // a group that is no registered bundle: the registrar could not assign it
        make("moqui.security.UserGroup", {userGroupId: "SPEC_DEV_UNREGISTERED", description: "Spec unregistered"})
        // the route group: the app list alone, a screen every user may open (webroot's SCREEN_TREE grant), as is ScreenTree
        make("moqui.security.ArtifactGroup", {artifactGroupId: "SPEC_DEV_SCREENS", description: "Spec wall screens"})
        make("moqui.security.ArtifactGroupMember", {
            artifactGroupId: "SPEC_DEV_SCREENS",
            artifactTypeEnumId: "AT_XML_SCREEN",
            artifactName: "component://webroot/screen/webroot/apps/AppList.xml",
            inheritAuthz: "N",
            nameIsPattern: "N"
        })
        make("moqui.security.DeviceRouteGroup", {
            routeGroupId: "SPEC_DEV_WALLS",
            description: "Spec walls",
            artifactGroupId: "SPEC_DEV_SCREENS",
            homePath: "/apps/AppList",
            parameterName: "playlistId",
            parameterTitle: "Playlist",
            parameterRequired: "Y"
        })
    },
    cleanupSpec() {
        ec.artifactExecution.disableAuthz()
        ec.entity.find(DEVICE).condition("displayUserId", wallId).disableAuthz().deleteAll()
        ec.entity.find("moqui.security.DeviceRouteGroup").condition("routeGroupId", "SPEC_DEV_WALLS").disableAuthz().deleteAll()
        ec.entity.find("moqui.security.UserGroupMember").condition("userId", wallId).disableAuthz().deleteAll()
        ec.artifactExecution.enableAuthz()
    }
})

feature("a device is registered by an access administrator for a display account they could assign; the code is shown once", {
    when() {
        const parameters = {deviceName: "Spec plant wall", routeGroupId: "SPEC_DEV_WALLS", displayUserId: wallId, parameterValue: "OPS"}
        const out = {
            denied: as("spec.device.person", () => invoke(S + "register#Device", parameters)),
            person: as(registrar, () => invoke(S + "register#Device", Object.assign({}, parameters, {displayUserId: "SPEC_DEV_PERSON"}))),
            noParameter: as(registrar, () => invoke(S + "register#Device", Object.assign({}, parameters, {parameterValue: ""})))
        }
        make("moqui.security.UserGroupMember", {userGroupId: "SPEC_DEV_UNREGISTERED", userId: wallId, fromDate: 0})
        out.group = as(registrar, () => invoke(S + "register#Device", parameters))
        ec.entity.find("moqui.security.UserGroupMember").condition("userId", wallId).disableAuthz().deleteAll()
        registered = as(registrar, () => invoke(S + "register#Device", Object.assign({operationKey: "spec-dev-1"}, parameters)))
        out.again = as(registrar, () => invoke(S + "register#Device", Object.assign({operationKey: "spec-dev-1"}, parameters)))
        out.listed = as(registrar, () => invoke(S + "find#Devices", {displayUserId: wallId}))
        return out
    },
    then({denied, person, noParameter, group, again, listed}) {
        denied.error.includes("ADMIN_ACCESS_MANAGE")
        person.applied === false && person.reasonCode == "DISPLAY_ACCOUNT"
        noParameter.applied === false && noParameter.reasonCode == "PARAMETER_REQUIRED"
        group.applied === false && group.reasonCode == "DISPLAY_GROUP"
        registered.error == "" && registered.applied === true
        ;/^[a-z2-9]{4}(-[a-z2-9]{4}){4}$/.test(registered.registrationCode)
        // a retry answers the first registration, with no code: it was shown once
        again.applied === true && again.repeated === true && again.deviceId == registered.deviceId && !again.registrationCode
        rows(DEVICE, {displayUserId: wallId}).length == 1
        // only its hash is kept, and nothing lists or records the code
        !JSON.stringify(rows(DEVICE, {deviceId: registered.deviceId}).map(row => row.getMap())).includes(registered.registrationCode)
        !JSON.stringify(listed).includes(registered.registrationCode)
        listed.deviceList.length == 1 && listed.deviceList[0].state == "pending" && listed.deviceList[0].displayName == "Spec wall"
        const evidence = rows(AUDIT, {changedEntityName: "moqui.security.UserAccount", pkPrimaryValue: wallId, changedFieldName: "deviceRegistered"})
        evidence.length == 1 && evidence[0].pkSecondaryValue == registered.deviceId
        !String(evidence[0].newValueText).includes(registered.registrationCode)
    }
})

feature("the code works once, for the browser that enters it, which then opens only its route group as its display account", {
    when() {
        const out = {wrong: request("Device/redeem", {code: "aaaa-bbbb-cccc-dddd-eeee"}, "post")}
        out.redeemed = request("Device/redeem", {code: registered.registrationCode}, "post")
        cookie = String(out.redeemed.setCookie || "")
            .split(";")[0]
            .replace("moqui.device=", "")
        out.replayed = request("Device/redeem", {code: registered.registrationCode}, "post")
        // redeeming signs out whoever was signed in on the browser's session (this thread's, here)
        out.userAfterRedeem = ec.user.userId
        ec.user.internalLoginUser("john.doe")
        // the device's requests, beside an instance administrator signed in on this thread
        out.home = request("apps/AppList", {}, "get", cookie)
        out.page = request("Device", {}, "get", cookie)
        out.otherScreen = request("apps/ScreenTree", {}, "get", cookie)
        out.preferences = request("apps/getPreferences", {}, "get", cookie)
        out.login = request("Login", {}, "get", cookie)
        out.rest = request("rest/s1/moqui/basic/geos/USA", {}, "get", cookie)
        out.userAfter = ec.user.userId
        // and a page the device was refused, without the cookie
        out.administrator = request("Login", {}, "get")
        return out
    },
    then({wrong, redeemed, replayed, userAfterRedeem, home, page, otherScreen, preferences, login, rest, userAfter, administrator}) {
        !wrong.setCookie
        String(redeemed.setCookie).includes("moqui.device=" + registered.deviceId + ".")
        String(redeemed.setCookie).includes("HttpOnly") && String(redeemed.setCookie).includes("SameSite=Strict")
        cookie.length > 40
        !replayed.setCookie
        !userAfterRedeem
        const stored = rows(DEVICE, {deviceId: registered.deviceId})[0]
        stored.redeemedDate != null && !JSON.stringify(stored.getMap()).includes(cookie.split(".")[1])
        home.errors == "" && home.status == 200
        page.text.includes("registered as Spec plant wall")
        refusedRoute(otherScreen) && refusedRoute(preferences) && refusedRoute(login) && refusedRoute(rest)
        // the display account was the request's user, never this thread's or its session's
        userAfter == "EX_JOHN_DOE"
        administrator.errors == "" && [200, 302].includes(administrator.status)
        rows(DEVICE, {deviceId: registered.deviceId})[0].lastSeenDate != null
        rows(AUDIT, {pkPrimaryValue: wallId, changedFieldName: "deviceRedeemed"}).length == 1
        rows(AUDIT, {pkPrimaryValue: wallId, changedFieldName: "deviceRefused"}).length == 1
    }
})

feature("a revoked device's next request is refused", {
    when() {
        return {
            denied: as("spec.device.person", () => invoke(S + "revoke#Device", {deviceId: registered.deviceId})),
            revoked: as(registrar, () => invoke(S + "revoke#Device", {deviceId: registered.deviceId, reason: "Spec: taken down"})),
            again: as(registrar, () => invoke(S + "revoke#Device", {deviceId: registered.deviceId})),
            next: request("apps/AppList", {}, "get", cookie),
            listed: as(registrar, () => invoke(S + "find#Devices", {displayUserId: wallId, includeEnded: true}))
        }
    },
    then({denied, revoked, again, next, listed}) {
        denied.error.includes("ADMIN_ACCESS_MANAGE")
        revoked.error == "" && revoked.applied === true && revoked.repeated === false
        again.applied === true && again.repeated === true
        next.errors.includes("registration was revoked")
        listed.deviceList.length == 1 && listed.deviceList[0].state == "revoked"
        const evidence = rows(AUDIT, {pkPrimaryValue: wallId, changedFieldName: "deviceRevoked"})
        evidence.length == 1 && evidence[0].changeReason == "Spec: taken down" && evidence[0].changedByUserId == "SPEC_DEV_REGISTRAR"
    }
})
