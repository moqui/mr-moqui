// BasicComms texts (plans/archive/Comms-Framework.md §5.6, C4-SMS; docs/interfaces/SMS.md): Calls History's Text a number, in the
// browser and the terminal, for a user granted a number with messaging; its transition opens the number pair's external
// room (one room for the pair, the user in it) and refuses a user with no such number the same post; an international
// number is refused before any room; several numbers open one group text's room, the same in any order (plans/Comms-Messaging.md
// §9), and the apps' call-context names a phone room's numbers (Chat's phoneNames). The provider itself (Telnyx) is the
// moqui-channels tests' fake: nothing here sends.
// Run: ./moq test basic-comms --spec BasicCommsTexts
const root = "component://basic-comms/screen/BasicComms.xml"
const TEXTER = "BASIC_COMMS_TEXTER"
const STRANGER = "BASIC_COMMS_TEXT_STRANGER"
const BRIDGE = "BASIC_COMMS_SMS"
const OURS = "+15550004000"
const THEIRS = "+15552224000"
const GROUP = "+15552224001,+15552224002"

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
            for (const mapping of mappings(THEIRS).concat(mappings(GROUP))) {
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
        const international = sys(() => ec.service.sync().name("org.moqui.call.SmsServices.open#PairRoom")
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

feature("Chat's pickers: a number the texter types is offered to text, their numbers listed, and textRoom opens the pair's room; a stranger gets none of it", {
    when() {
        ec.user.internalLoginUser("basic.comms.texter")
        const offered = JSON.parse(render("Chat/searchUsers", {term: "(555) 222-4000", numbers: "true"}).output || "[]")
        const plain = JSON.parse(render("Chat/searchUsers", {term: "(555) 222-4000"}).output || "[]")
        const numbers = JSON.parse(render("Chat/textNumbers", {}).output || "{}")
        const opened = JSON.parse(render("Chat/textRoom", {textTo: "(555) 222-4000", textFrom: OURS}, "post").output || "{}")
        ec.user.internalLoginUser("basic.comms.text.stranger")
        const strangerNumbers = JSON.parse(render("Chat/textNumbers", {}).output || "{}")
        const refused = render("Chat/textRoom", {textTo: THEIRS, textFrom: OURS}, "post")
        return {offered, plain, numbers, opened, strangerNumbers, refused, made: mappings(THEIRS)}
    },
    then({offered, plain, numbers, opened, strangerNumbers, refused, made}) {
        offered[0].value == "tel:(555) 222-4000"
        offered[0].label == "Text (555) 222-4000"
        !plain.some((option) => String(option.value).startsWith("tel:"))
        numbers.numberList.length == 1
        numbers.numberList[0].phoneNumber == OURS
        made.length == 1
        opened.chatRoomId == made[0].chatRoomId
        strangerNumbers.numberList.length == 0
        !String(refused.output).includes(made[0].chatRoomId)
    }
})

feature("A group text: several numbers open one room for the set, the same in any order; more than eight are refused", {
    when() {
        ec.user.internalLoginUser("basic.comms.texter")
        const opened = JSON.parse(render("Chat/textRoom", {textTo: "+15552224002, (555) 222-4001", textFrom: OURS}, "post").output || "{}")
        const again = JSON.parse(render("Chat/textRoom", {textTo: "+15552224001,+15552224002,+15552224001", textFrom: OURS}, "post").output || "{}")
        const many = Array.from({length: 9}, (_, n) => "+1555222410" + n).join(",")
        const tooMany = sys(() => ec.service.sync().name("org.moqui.call.SmsServices.open#PairRoom")
            .parameters({toNumber: many, fromNumber: OURS}).call()) || {}
        ec.message.clearErrors()
        const made = mappings(GROUP)
        const room = made.length ? sys(() => ec.entity.find("moqui.chat.ChatRoom").condition("chatRoomId", made[0].chatRoomId).one()) : null
        return {opened, again, tooMany, made, room}
    },
    then({opened, again, tooMany, made, room}) {
        made.length == 1
        opened.chatRoomId == made[0].chatRoomId
        again.chatRoomId == made[0].chatRoomId
        room.roomTypeEnumId == "CrtExternal"
        room.roomName == "+15552224001, +15552224002"
        tooMany.reasonCode == "tooManyNumbers"
    }
})

// the example gallery's call-context provider names +1555 0100 numbers; a user who cannot open the gallery gets no names
const exampleInstalled = ec.resource.getLocationReference("component://example/screen/Gallery.xml").getExists()
feature("A phone room's numbers are named by the apps' call-context, as the user; a number no app knows stays unnamed", {
    when() {
        if (!exampleInstalled) return {skip: true}
        ec.user.internalLoginUser("john.doe")
        const named = JSON.parse(render("Chat/phoneNames", {numbers: "+15550100777,+15559990000,not a number"}).output || "{}")
        ec.user.internalLoginUser("basic.comms.text.stranger")
        const stranger = JSON.parse(render("Chat/phoneNames", {numbers: "+15550100777"}).output || "{}")
        return {named, stranger}
    },
    then({skip, named, stranger}) {
        skip || named.nameMap["+15550100777"] == "Gallery caller 0777"
        skip || !("+15559990000" in named.nameMap)
        skip || Object.keys(named.nameMap).length == 1
        skip || !(stranger.nameMap && stranger.nameMap["+15550100777"])
    }
})
