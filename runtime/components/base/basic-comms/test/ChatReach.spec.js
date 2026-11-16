// Whom a Comms user reaches in chat (SEC-COM-01; org.moqui.comms.CommsServices search#Users, org.moqui.chat.ChatServices
// get#RefusedUsers, create#Room, add#Member): an app's chat-target provider keeps people out of the people search, of new
// direct rooms and of added members, answered as users that do not exist (the example gallery's provider refuses users
// named *.offlimits; A opens the gallery through a group of this spec's), and an external user (EXTERNAL_USER) is found
// by nobody and finds only the people of their own rooms. Skipped when the example component is not loaded.
// Run: ./moq test basic-comms --spec ChatReach
const root = "component://basic-comms/screen/BasicComms.xml"
const A = "CHAT_REACH_A"
const B = "CHAT_REACH_B"
const C = "CHAT_REACH_C"
const OFFLIMITS = "CHAT_REACH_OFFLIMITS"
const EXT = "CHAT_REACH_EXT"
const GALLERY_GROUP = "CHAT_REACH_GALLERY"
const EXTERNAL_GROUP = "CHAT_REACH_EXTERNAL"
const exampleInstalled = ec.resource.getLocationReference("component://example/screen/Gallery.xml").getExists()

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
function request(path, parameters, method) {
    const str = ec.screen.makeTest().rootScreen(root).render("Chat/" + path, parameters || {}, method || null)
    ec.message.clearErrors()
    let json = null
    try {
        json = JSON.parse(String(str.output || ""))
    } catch (err) {
        json = null
    }
    return {errors: str.errorMessages.join(" | "), json}
}
const username = (userId) => userId.toLowerCase().replace(/_/g, ".")
function group(groupId, make) {
    if (ec.entity.find("moqui.security.UserGroup").condition("userGroupId", groupId).one()) return
    const row = ec.entity.makeValue("moqui.security.UserGroup")
    row.setFields({userGroupId: groupId, description: "Chat reach spec"}, true, null, null)
    row.create()
    make()
}
function makeUser(userId, groups) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() == 0) {
            const account = ec.entity.makeValue("moqui.security.UserAccount")
            account.setFields({userId, username: username(userId), userFullName: "Chat reach " + userId.slice(11).toLowerCase(), disabled: "N"}, true, null, null)
            account.create()
        }
        for (const groupId of ["BASIC_COMMS_USERS"].concat(groups || [])) {
            if (ec.entity.find("moqui.security.UserGroupMember").condition({userGroupId: groupId, userId}).count() > 0) continue
            const member = ec.entity.makeValue("moqui.security.UserGroupMember")
            member.setFields({userGroupId: groupId, userId, fromDate: new Date(Date.now() - 60000)}, true, null, null)
            member.create()
        }
    })
}
const found = (out) => Array.from(out.json || []).map((user) => user.value).sort()

spec({
    stepwise: true,
    setupSpec() {
        if (!exampleInstalled) return
        sys(() => {
            group(GALLERY_GROUP, () => {
                const authz = ec.entity.makeValue("moqui.security.ArtifactAuthz")
                authz.setFields({artifactAuthzId: GALLERY_GROUP, userGroupId: GALLERY_GROUP, artifactGroupId: "EXAMPLE_GALLERY",
                    authzTypeEnumId: "AUTHZT_ALWAYS", authzActionEnumId: "AUTHZA_ALL"}, true, null, null)
                authz.create()
            })
            group(EXTERNAL_GROUP, () => {
                const grant = ec.entity.makeValue("moqui.security.UserGroupPermission")
                grant.setFields({userGroupId: EXTERNAL_GROUP, userPermissionId: "EXTERNAL_USER", fromDate: new Date(Date.now() - 60000)}, true, null, null)
                grant.create()
            })
        })
        makeUser(A, [GALLERY_GROUP])
        makeUser(B)
        makeUser(C)
        makeUser(OFFLIMITS)
        makeUser(EXT, [EXTERNAL_GROUP])
    },
})

feature("a user the app's chat-target provider refuses is not found, not messaged and not added; another user is", {
    when() {
        if (!exampleInstalled) return {skip: true}
        ec.user.internalLoginUser(username(A))
        const search = request("searchUsers", {term: "chat reach"})
        const directOff = request("createDirect", {userIds: OFFLIMITS}, "post")
        const directNobody = request("createDirect", {userIds: "CHAT_REACH_NO_SUCH_USER"}, "post")
        const room = request("createRoom", {roomTypeEnumId: "private", roomName: "Chat reach room"}, "post")
        const chatRoomId = room.json?.chatRoomId
        const addOff = request("addMembers", {chatRoomId, userIds: OFFLIMITS}, "post")
        const addB = request("addMembers", {chatRoomId, userIds: B}, "post")
        // the external user is in a room with B only
        const extRoom = request("createRoom", {roomTypeEnumId: "private", roomName: "Chat reach outside"}, "post").json?.chatRoomId
        sys(() => ec.service.sync().name("org.moqui.chat.ChatServices.add#Member").parameters({chatRoomId: extRoom, userIdList: [B, EXT]}).call())
        ec.message.clearErrors()
        ec.user.internalLoginUser(username(EXT))
        const extSearch = request("searchUsers", {term: "chat reach"})
        const extDirectC = request("createDirect", {userIds: C}, "post")
        const extDirectB = request("createDirect", {userIds: B}, "post")
        ec.user.logoutUser()
        return {search, directOff, directNobody, addOff, addB, extSearch, extDirectC, extDirectB}
    },
    then({skip, search, directOff, directNobody, addOff, addB, extSearch, extDirectC, extDirectB}) {
        // the search leaves out the refused user and the external one
        skip || found(search).join() == [B, C].join()
        skip || directOff.errors.includes("not found")
        skip || directOff.errors.replace(OFFLIMITS, "X") == directNobody.errors.replace("CHAT_REACH_NO_SUCH_USER", "X")
        skip || addOff.errors.includes("not found")
        skip || addB.errors == ""
        skip || addB.json.addedUserIdList.join() == B
        // an external user finds and messages only the people of their own rooms (A and B), never C
        skip || found(extSearch).join() == [A, B].join()
        skip || extDirectC.errors.includes("not found")
        skip || extDirectB.errors == ""
    },
})
