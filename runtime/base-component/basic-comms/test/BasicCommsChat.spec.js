// BasicComms Chat (screen/BasicComms/Chat.xml and Chat.qvue, org.moqui.comms.ChatScreenServices; plans/Comms-Framework.md
// A2): a user with no party and john.doe make a room and talk in it through the framework's screens; the records of rooms
// and messages come only from the record-lookup, record-link and create-from providers of the apps a user may open
// (IronStone's, when it is installed), and its message actions (ext-action, hook create-from) reach only a user who has them,
// through either mount; a user without them is refused the same post that works for john.doe.
// Run: ./moq test basic-comms --spec BasicCommsChat
const root = "component://basic-comms/screen/BasicComms.xml"
const webroot = "component://webroot/screen/webroot.xml"
const USER = "BASIC_COMMS_CHAT"
const CLIENT = "BASIC_COMMS_CHAT_CLIENT"
const bossInstalled = ec.resource.getLocationReference("component://mr-boss/screen/Comms.xml").getExists()
let roomId = null
let firstSeq = null

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
function request(rootLocation, path, parameters, method) {
    const str = ec.screen
        .makeTest()
        .rootScreen(rootLocation)
        .render(path, parameters || {}, method || null)
    const output = String(str.output || "")
    ec.message.clearErrors()
    let json = null
    try {
        json = JSON.parse(output)
    } catch (err) {
        json = null
    }
    return {status: str.status, errors: str.errorMessages.join(" | "), json, output}
}
const post = (path, parameters) => request(root, "Chat/" + path, parameters, "post")
const get = path => request(root, "Chat/" + path, {}, null)
function makeUser(userId, username, groupId) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() == 0) {
            const account = ec.entity.makeValue("moqui.security.UserAccount")
            account.setFields({userId, username, userFullName: "Chat " + username, disabled: "N"}, true, null, null)
            account.create()
        }
        if (groupId && ec.entity.find("moqui.security.UserGroup").condition("userGroupId", groupId).count() > 0) {
            const member = ec.entity.makeValue("moqui.security.UserGroupMember")
            member.setFields({userGroupId: groupId, userId, fromDate: ec.user.nowTimestamp}, true, null, null)
            if (
                ec.entity
                    .find("moqui.security.UserGroupMember")
                    .condition("userGroupId", groupId)
                    .condition("userId", userId)
                    .count() == 0
            )
                member.create()
        }
    })
}
const actionNames = items => Array.from((items && items.actions) || []).map(action => action.name)

spec({
    stepwise: true,
    setupSpec() {
        makeUser(USER, "basic.comms.chat", null)
        makeUser(CLIENT, "basic.comms.chat.client", "BOSS_PROJECT_CLIENT")
    }
})

feature("a user with no party makes a room with john.doe and they talk in it", {
    when() {
        ec.user.internalLoginUser("basic.comms.chat")
        const made = post("createRoom", {roomTypeEnumId: "private", roomName: "Basic chat", memberUserIds: "EX_JOHN_DOE"})
        roomId = made.json && made.json.chatRoomId
        const sent = post("send", {chatRoomId: roomId, body: "<p>Hello from the framework</p>", clientRef: "bc-1"})
        firstSeq = sent.json && sent.json.eventSeq
        login("john.doe")
        const rail = get("chatData")
        const view = request(root, "Chat/roomView", {chatRoomId: roomId}, null)
        return {made, sent, rail, view}
    },
    then({made, sent, rail, view}) {
        made.errors == ""
        roomId != null
        sent.errors == ""
        firstSeq > 0
        Array.from(rail.json.roomList).some(room => room.chatRoomId === roomId && room.url.endsWith("/Chat?chatRoomId=" + roomId))
        view.status == 200
        JSON.stringify(view.json).includes("Hello from the framework")
    }
})

feature("john.doe gets the app's message actions and record providers through either mount", {
    when() {
        login("john.doe")
        if (!bossInstalled) return {skip: true}
        const own = request(webroot, "extItems/apps/BasicComms/Chat", {}, null).json
        const app = request(webroot, "extItems/apps/Comms/Chat", {}, null).json
        const prefs = get("chatData").json.prefs
        const made = post("createFrom", {chatRoomId: roomId, eventSeq: firstSeq, kind: "task"})
        return {own, app, prefs, made}
    },
    then({skip, own, app, prefs, made}) {
        skip || actionNames(own).includes("boss.createTask")
        skip || actionNames(app).includes("boss.createTask")
        skip || JSON.stringify(own.actions[0].parameters).includes('"value":"task"')
        skip || prefs.recordLink == true
        skip || made.errors == ""
        skip || made.json.recordId != null
    }
})

feature("a user no app answers the record hooks for gets no record actions, and the same post is refused", {
    when() {
        if (!bossInstalled) return {skip: true}
        // a member of the room, whom IronStone denies its Comms (a project client)
        login("john.doe")
        post("addMembers", {chatRoomId: roomId, userIds: CLIENT})
        ec.user.internalLoginUser("basic.comms.chat.client")
        const own = request(webroot, "extItems/apps/BasicComms/Chat", {}, null).json
        const prefs = get("chatData").json.prefs
        const made = post("createFrom", {chatRoomId: roomId, eventSeq: firstSeq, kind: "task"})
        const filed = post("fileOnRecord", {chatRoomId: roomId, eventSeq: firstSeq, recordEntityName: "x", recordId: "y"})
        login("john.doe")
        return {own, prefs, made, filed}
    },
    then({skip, own, prefs, made, filed}) {
        skip || !actionNames(own).includes("boss.createTask")
        skip || prefs.recordLink == false
        skip || made.errors.includes("Nothing here makes records from messages")
        skip || filed.errors.includes("Nothing here files messages on records")
    }
})
