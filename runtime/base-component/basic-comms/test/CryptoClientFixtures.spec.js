// The room the client spec CryptoFlow (test/client, moqui test-client basic-comms) needs: john.doe and calls.client.b
// (CallClientFixtures) in a private room, so each may reach the other's crypto devices (org.moqui.impl.ChatKeyServices).
// moqui test-client basic-comms --serve runs this spec on its server's scratch database; for another server:
//     moqui test basic-comms --spec CallClientFixtures,CryptoClientFixtures --db existing
// The room is made only when missing. Not demo data.
const ROOM_NAME = "Crypto client spec"

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}

spec({ dependsOn: ["CallClientFixtures"] })

feature("john.doe and calls.client.b share a private room", {
    when() {
        const userOf = (username) => sys(() => ec.entity.find("moqui.security.UserAccount").condition("username", username).one())
        const john = userOf("john.doe")
        const other = userOf("calls.client.b")
        const existing = sys(() => ec.entity.find("moqui.chat.ChatRoom").condition("roomName", ROOM_NAME).one())
        let chatRoomId = existing ? existing.chatRoomId : null
        if (!chatRoomId && john && other) {
            ec.user.internalLoginUser("john.doe")
            chatRoomId = sys(() => call("org.moqui.impl.ChatServices.create#Room", {roomTypeEnumId: "CrtPrivate", roomName: ROOM_NAME,
                memberUserIdList: [other.userId]})).chatRoomId
        }
        return {john, other, chatRoomId}
    },
    then({john, other, chatRoomId}) {
        john != null
        other != null
        chatRoomId != null
    }
})
