// BasicComms and chat bridges (plans/Comms-Framework.md §4.3, C1): a room mapped to a network that is not connected (no
// adapter of it in this build) shows the bridge and "not connected" in the room list, the room and its terminal twin; an
// external member from that network has its badge and its message its network; a message of the room shows that it waits to
// be sent, and System, Chat Bridges lists the bridge with its last error and the delivery that failed, with Send again, for
// an administrator only. The delivery itself and its failures are crates/moqui-core/src/service/chat_bridge_tests.rs.
// Run: ./moq test basic-comms --spec BasicCommsBridges
const root = "component://basic-comms/screen/BasicComms.xml"
const systemRoot = "component://tools/screen/System.xml"
const BRIDGES = "org.moqui.chat.ChatBridgeServices"
const USER = "BASIC_COMMS_BRIDGES"

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
// a service as a screen calls it (the screens' artifact authorization covers it): the service's own checks decide
function call(name, parameters) {
    const out = sys(() => ec.service.sync().name(name).parameters(parameters || {}).call()) || {}
    const errors = ec.message.getErrorsString()
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
function render(rootLocation, path, mode, parameters) {
    let test = ec.screen.makeTest().rootScreen(rootLocation).renderMode(mode)
    if (mode === "qvt") test = test.withStaticData()
    const str = test.render(path, Object.assign({lastStandalone: "-2"}, parameters || {}), null)
    const output = String(str.output || "")
    ec.message.clearErrors()
    return {status: str.status, errors: str.errorMessages.join(" | "), output, broken: /Error rendering|Exception in render/.test(output)}
}
function json(path, parameters) {
    const str = ec.screen.makeTest().rootScreen(root).render("Chat/" + path, parameters || {}, null)
    ec.message.clearErrors()
    try {
        return JSON.parse(String(str.output || ""))
    } catch (err) {
        return null
    }
}

feature("a bridged room that is not connected shows its bridge, its external member and its waiting message, in the browser and the terminal", {
    when() {
        sys(() => {
            if (!ec.entity.find("moqui.security.UserAccount").condition("userId", USER).count()) {
                const account = ec.entity.makeValue("moqui.security.UserAccount")
                account.setFields({userId: USER, username: "basic.comms.bridges", userFullName: "Bridge Neighbor"}, true, null, null)
                account.create()
            }
        })
        ec.user.internalLoginUser("john.doe")
        // a bridge with no adapter in this build, a room mapped to it, and a person on its network
        const bridge = call(BRIDGES + ".store#Bridge", {name: "Spec SMS line", typeEnumId: "CbtSms", adapterName: "spec-missing",
            inboundToken: "spec-token"}).chatBridgeId
        const room = call("org.moqui.chat.ChatServices.create#Room", {roomTypeEnumId: "private", roomName: "Spec bridged room",
            memberUserIdList: [USER]}).chatRoomId
        const mapped = call(BRIDGES + ".store#RoomBridge", {chatRoomId: room, chatBridgeId: bridge, externalRoomId: "+15550142"})
        const again = call(BRIDGES + ".store#RoomBridge", {chatRoomId: room, chatBridgeId: bridge, externalRoomId: "+15550142"})
        const ingested = (call(BRIDGES + ".ingest#Event", {chatBridgeId: bridge, token: "spec-token", transactionId: "spec-1",
            itemList: [{itemId: "m1", type: "message", externalRoomId: "+15550142", externalEventId: "sms-1", externalUserId: "+15550199",
                displayName: "Riley Outside", text: "Is the order ready?"}]}))
        const wrongToken = (call(BRIDGES + ".ingest#Event", {chatBridgeId: bridge, token: "not-it", transactionId: "spec-2",
            itemList: [{itemId: "m2", type: "message", externalRoomId: "+15550142", externalEventId: "sms-2", externalUserId: "+15550199",
                text: "again"}]}))
        const sent = call("org.moqui.chat.ChatServices.send#Message", {chatRoomId: room, body: "<p>Yes, today</p>"})
        // the worker's round: the bridge does not start (no adapter), so it is not connected and the message waits
        call(BRIDGES + ".run#BridgeDeliveries", {})
        const timeline = call("org.moqui.chat.ChatServices.get#Timeline", {chatRoomId: room})
        const roomOut = call("org.moqui.chat.ChatServices.get#Room", {chatRoomId: room})
        const rooms = call("org.moqui.chat.ChatServices.find#Rooms", {})
        const tui = render(root, "ChatTui", "tui", {chatRoomId: room})
        const tuiRooms = render(root, "ChatTui", "tui", {})
        const view = json("roomView", {chatRoomId: room})
        // a delivery that failed, for the admin list: the mapping ended, so the worker fails the waiting one
        sys(() => {
            ec.entity.find("moqui.chat.ChatBridgeDelivery").condition("chatRoomId", room).updateAll({statusEnumId: "CdsFailed",
                reasonCode: "spec", lastErrorText: "Spec refusal", lastAttemptDate: ec.user.nowTimestamp})
        })
        const tuiFailed = render(root, "ChatTui", "tui", {chatRoomId: room})
        const adminQvt = render(systemRoot, "ChatBridges", "qvt", {})
        const adminTui = render(systemRoot, "ChatBridges", "tui", {})
        const delivery = sys(() => ec.entity.find("moqui.chat.ChatBridgeDelivery").condition("chatRoomId", room).one())
        ec.user.internalLoginUser("basic.comms.bridges")
        const strangerList = call(BRIDGES + ".find#Bridges", {})
        const memberRetry = call(BRIDGES + ".retry#Delivery", {chatBridgeDeliveryId: delivery.chatBridgeDeliveryId})
        const strangerStore = call(BRIDGES + ".store#Bridge", {chatBridgeId: bridge, disabled: "Y"})
        ec.user.internalLoginUser("john.doe")
        const ownerRetry = call(BRIDGES + ".retry#Delivery", {chatBridgeDeliveryId: delivery.chatBridgeDeliveryId})
        return {mapped, again, ingested, wrongToken, sent, timeline, roomOut, rooms, room, tui, tuiRooms, view, tuiFailed, adminQvt, adminTui,
            strangerList, memberRetry, strangerStore, ownerRetry}
    },
    then({mapped, again, ingested, wrongToken, sent, timeline, roomOut, rooms, room, tui, tuiRooms, view, tuiFailed, adminQvt, adminTui,
            strangerList, memberRetry, strangerStore, ownerRetry}) {
        mapped.errors == ""
        again.reasonCode == "mappingExists"
        ingested.errors == ""
        ingested.outcomeList[0].outcome == "accepted"
        wrongToken.reasonCode == "badToken"
        sent.errors == ""
        roomOut.room.bridged === true
        roomOut.room.notConnected === true
        roomOut.memberList.some(member => member.external && member.name == "Riley Outside" && member.networkName == "SMS")
        timeline.eventList.some(event => event.senderName == "Riley Outside" && event.senderExternal && event.senderExternal.networkName == "SMS")
        timeline.eventList.some(event => event.eventSeq == sent.eventSeq && event.delivery && event.delivery.state == "pending")
        rooms.roomList.some(item => item.chatRoomId == room && item.bridged && item.notConnected && item.bridgeNetworkNames.includes("SMS"))
        view.room.bridged === true
        !tui.broken
        tui.output.includes("Messages here also go to SMS")
        tui.output.includes("Not connected")
        tui.output.includes("Riley Outside (SMS)")
        tui.output.includes("Sending")
        tuiRooms.output.includes("[SMS, not connected]")
        !tuiFailed.broken
        tuiFailed.output.includes("Not delivered: Spec refusal")
        tuiFailed.output.includes("Send again")
        adminQvt.status == 200
        !adminQvt.broken
        adminQvt.output.includes("Spec SMS line")
        adminQvt.output.includes("No chat bridge adapter spec-missing")
        adminQvt.output.includes("Spec refusal")
        !adminTui.broken
        adminTui.output.includes("Spec SMS line")
        adminTui.output.includes("Send again")
        strangerList.errors.includes("Chat bridges are for administrators")
        memberRetry.reasonCode == "notAllowed"
        strangerStore.errors.includes("Chat bridges are for administrators")
        ownerRetry.errors == ""
    },
})
