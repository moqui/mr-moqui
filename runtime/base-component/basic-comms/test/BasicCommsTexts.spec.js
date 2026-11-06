// BasicComms texts (plans/Comms-Framework.md §5.6, C4-SMS; docs/interfaces/SMS.md): Calls History's Text a number, in the
// browser and the terminal, for a user granted a number with messaging; its transition opens the number pair's external
// room (one room for the pair, the user in it) and refuses a user with no such number the same post; an international
// number is refused before any room. The provider itself (Telnyx) is the moqui-channels tests' fake: nothing here sends.
// Run: ./moq test basic-comms --spec BasicCommsTexts
const root = "component://basic-comms/screen/BasicComms.xml"
const TEXTER = "BASIC_COMMS_TEXTER"
const STRANGER = "BASIC_COMMS_TEXT_STRANGER"
const BRIDGE = "BASIC_COMMS_SMS"
const OURS = "+15550004000"
const THEIRS = "+15552224000"

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
function render(path, parameters, method, mode) {
    let test = ec.screen.makeTest().rootScreen(root)
    if (mode) test = test.renderMode(mode)
    const str = test.render(path, parameters || {}, method || null)
    const output = String(str.output || "")
    ec.message.clearErrors()
    return {status: str.status, output}
}
function makeUser(userId, username) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() == 0) {
            const account = ec.entity.makeValue("moqui.security.UserAccount")
            account.setFields({userId, username, userFullName: "Texts " + username, disabled: "N"}, true, null, null)
            account.create()
        }
        // BasicComms is opened by BASIC_COMMS_USERS, not ALL_USERS; IronStone's Comms, when installed, by BOSS_USERS
        for (const group of ["BASIC_COMMS_USERS", "BOSS_USERS"]) {
            if (ec.entity.find("moqui.security.UserGroup").condition("userGroupId", group).count() == 0) continue
            const found = ec.entity.find("moqui.security.UserGroupMember").condition("userGroupId", group)
            if (found.condition("userId", userId).count() > 0) continue
            const member = ec.entity.makeValue("moqui.security.UserGroupMember")
            member.setFields({userGroupId: group, userId, fromDate: ec.user.nowTimestamp}, true, null, null)
            member.create()
        }
    })
}
const mappings = (theirs) => sys(() => Array.from(ec.entity.find("moqui.chat.ChatRoomBridge").condition("chatBridgeId", BRIDGE)
    .condition("externalRoomId", OURS + "/" + theirs).list()))

spec({
    stepwise: true,
    setupSpec() {
        makeUser(TEXTER, "basic.comms.texter")
        makeUser(STRANGER, "basic.comms.text.stranger")
        sys(() => {
            const bridge = ec.entity.makeValue("moqui.chat.ChatBridge")
            bridge.setFields({chatBridgeId: BRIDGE, typeEnumId: "CbtSms", adapterName: "sms", name: "Spec texts", statusId: "CbsDisabled",
                configJson: JSON.stringify({apiKey: "spec", apiBaseUrl: "http://127.0.0.1:9"})}, true, null, null)
            bridge.createOrUpdate()
            const number = ec.entity.makeValue("moqui.call.PhoneNumber")
            number.setFields({phoneNumber: OURS, messagingBridgeId: BRIDGE, routeTypeEnumId: "PnrUser", routeId: TEXTER}, true, null, null)
            number.createOrUpdate()
            const grant = ec.entity.makeValue("moqui.call.PhoneNumberUser")
            grant.setFields({phoneNumber: OURS, userId: TEXTER, fromDate: new Date(Date.now() - 60000)}, true, null, null)
            grant.create()
        })
    },
    cleanupSpec() {
        sys(() => {
            for (const mapping of mappings(THEIRS)) {
                ec.entity.find("moqui.chat.ChatRoomMember").condition("chatRoomId", mapping.chatRoomId).deleteAll()
                ec.entity.find("moqui.chat.ChatRoomBridge").condition("chatRoomBridgeId", mapping.chatRoomBridgeId).deleteAll()
                ec.entity.find("moqui.chat.ChatRoom").condition("chatRoomId", mapping.chatRoomId).deleteAll()
            }
            ec.entity.find("moqui.call.PhoneNumberUser").condition("phoneNumber", OURS).deleteAll()
            ec.entity.find("moqui.call.PhoneNumber").condition("phoneNumber", OURS).deleteAll()
            ec.entity.find("moqui.chat.ChatBridge").condition("chatBridgeId", BRIDGE).deleteAll()
        })
    }
})

feature("Text a number is in Calls History, in the browser and the terminal, for a user with a number that texts", {
    when() {
        ec.user.internalLoginUser("basic.comms.texter")
        const page = render("Calls/History", {}, null, null)
        const tui = render("Calls/History", {}, null, "tui")
        ec.user.internalLoginUser("basic.comms.text.stranger")
        const strangerPage = render("Calls/History", {}, null, null)
        return {page, tui, strangerPage}
    },
    then({page, tui, strangerPage}) {
        page.status == 200
        page.output.includes("Text a number")
        page.output.includes(OURS)
        tui.output.includes("Text a number")
        !strangerPage.output.includes("Text a number")
    }
})

feature("Text a number opens the pair's room once, with the user in it; the same post from a stranger makes nothing", {
    when() {
        ec.user.internalLoginUser("basic.comms.text.stranger")
        render("Calls/History/textNumber", {textTo: THEIRS, textFrom: OURS}, "post", null)
        const afterStranger = mappings(THEIRS).length
        ec.user.internalLoginUser("basic.comms.texter")
        render("Calls/History/textNumber", {textTo: "(555) 222-4000", textFrom: OURS}, "post", null)
        render("Calls/History/textNumber", {textTo: THEIRS, textFrom: OURS}, "post", null)
        const made = mappings(THEIRS)
        const room = made.length ? sys(() => ec.entity.find("moqui.chat.ChatRoom").condition("chatRoomId", made[0].chatRoomId).one()) : null
        const members = made.length ? sys(() => Array.from(ec.entity.find("moqui.chat.ChatRoomMember").condition("chatRoomId", made[0].chatRoomId).list())
            .map((m) => m.userId)) : []
        const international = sys(() => ec.service.sync().name("org.moqui.impl.SmsServices.open#PairRoom")
            .parameters({toNumber: "+442071234000", fromNumber: OURS}).call()) || {}
        ec.message.clearErrors()
        return {afterStranger, made, room, members, international}
    },
    then({afterStranger, made, room, members, international}) {
        afterStranger == 0
        made.length == 1
        room.roomTypeEnumId == "CrtExternal"
        room.roomName == THEIRS
        members.includes(TEXTER)
        !members.includes(STRANGER)
        international.reasonCode == "internationalNotEnabled"
        mappings("+442071234000").length == 0
    }
})
