// Rich messages (plans/Comms-Messaging.md §18.3 phase 13; framework/service/org/moqui/chat/ChatRich.js): a text with
// buttons and a card are kept as content.rich with their text form as the message's body (what every other client and
// bridge shows and sends); the limits refuse each by its reason; an encrypted room, a provider's template from a user
// and an edit are refused; a member taps a reply on another's message (its words and content.postback), not on their
// own, not a link and not in a bridged room; and a delivery's read receipt is past delivered, with its provider content
// named for deleting on the final one. The providers' forms are the moqui-channels tests' (tests/rcs.rs).
// Run: ./moq test basic-comms --spec BasicCommsRich
const SENDER = "BASIC_COMMS_RICH"
const READER = "BASIC_COMMS_RICH_READER"
const RICH = "org.moqui.chat.ChatRichServices"
const CHAT = "org.moqui.chat.ChatServices"
let roomId = null
let cardSeq = null

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
function makeUser(userId, username) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() == 0) {
            const account = ec.entity.makeValue("moqui.security.UserAccount")
            account.setFields({userId, username, userFullName: "Rich " + username, disabled: "N"}, true, null, null)
            account.create()
        }
        // BasicComms is opened by BASIC_COMMS_USERS, not ALL_USERS; IronStone's Comms, when installed, by BOSS_USERS
        for (const group of ["BASIC_COMMS_USERS", "BOSS_USERS"]) {
            if (ec.entity.find("moqui.security.UserGroup").condition("userGroupId", group).count() == 0) continue
            if (ec.entity.find("moqui.security.UserGroupMember").condition("userGroupId", group).condition("userId", userId).count() > 0) continue
            const member = ec.entity.makeValue("moqui.security.UserGroupMember")
            member.setFields({userGroupId: group, userId, fromDate: ec.user.nowTimestamp}, true, null, null)
            member.create()
        }
    })
}
// a post to a transition of Chat.xml, as the browser makes it: its JSON and its errors
function post(path, parameters) {
    const str = ec.screen.makeTest().rootScreen("component://basic-comms/screen/BasicComms.xml").render("Chat/" + path, parameters || {}, "post")
    ec.message.clearErrors()
    let json = null
    try {
        json = JSON.parse(String(str.output || ""))
    } catch (err) {
        json = null
    }
    return {out: json || {}, errors: str.errorMessages.join(" | ")}
}
// a service call as the current user: its out-parameters and its errors, cleared
function run(name, parameters) {
    let out = {}
    try {
        out = ec.service.sync().name(name).parameters(parameters).call() || {}
    } catch (err) {
        out = {thrown: String(err && err.message ? err.message : err)}
    }
    const errors = ec.message.errorsString || ""
    ec.message.clearErrors()
    return {out, errors}
}
const send = (rich, room) => post("sendRich", {chatRoomId: room || roomId, richJson: JSON.stringify(rich)})
const reply = (parameters) => post("replySuggestion", Object.assign({chatRoomId: roomId}, parameters))
const eventOf = (seq) => sys(() => ec.entity.find("moqui.chat.ChatEvent").condition("chatRoomId", roomId).condition("eventSeq", seq).one())
const contentOf = (seq) => JSON.parse(String(eventOf(seq).contentJson || "{}"))
const chips = (n) => Array.from({length: n}, (_, i) => ({type: "reply", text: "Option " + (i + 1)}))
const CARD = {kind: "card", cards: [{title: "Your order shipped", description: "Arrives Tuesday", suggestions: [
    {type: "reply", text: "Thanks", postback: "order:7:thanks"}, {type: "url", text: "Track", url: "https://example.test/track/7"},
    {type: "dial", text: "Call us", phone: "+1 (555) 010-9999"}]}]}

spec({
    stepwise: true,
    setupSpec() {
        makeUser(SENDER, "basic.comms.rich")
        makeUser(READER, "basic.comms.rich.reader")
        ec.user.internalLoginUser("basic.comms.rich")
        const made = sys(() => run(CHAT + ".create#Room", {roomTypeEnumId: "CrtPrivate", roomName: "Rich room", memberUserIdList: [READER]}))
        roomId = made.out.chatRoomId
    }
})

feature("a card and a text with buttons keep their text form as the message's body", {
    when() {
        ec.user.internalLoginUser("basic.comms.rich")
        const card = send(CARD)
        cardSeq = card.out.eventSeq
        const text = send({kind: "text", text: "Table for two?", suggestions: [{type: "reply", text: "Yes"}, {type: "reply", text: "No", postback: "no"}]})
        const timeline = sys(() => run(CHAT + ".get#Timeline", {chatRoomId: roomId})).out
        const listed = Array.from(timeline.eventList || []).find((event) => Number(event.eventSeq) === Number(cardSeq))
        const timelineRich = listed && listed.rich ? JSON.stringify(listed.rich) : ""
        const edit = sys(() => run(CHAT + ".edit#Message", {chatRoomId: roomId, eventSeq: cardSeq, body: "<p>changed</p>"}))
        return {card, text, timelineRich, edit}
    },
    then({card, text, timelineRich, edit}) {
        card.errors == ""
        cardSeq > 0
        card.out.event.rich.kind == "card"
        eventOf(cardSeq).bodyText == "Your order shipped\nArrives Tuesday\nTrack: https://example.test/track/7\nCall us: +15550109999\nReply: Thanks"
        // the call's number is kept in its E.164 form, the reply's postback as given
        contentOf(cardSeq).rich.cards[0].suggestions[2].phone == "+15550109999"
        contentOf(cardSeq).rich.cards[0].suggestions[0].postback == "order:7:thanks"
        text.errors == ""
        eventOf(text.out.eventSeq).bodyText == "Table for two?\n\nReply: Yes, No"
        // a reply with no postback answers with its words
        contentOf(text.out.eventSeq).rich.suggestions[0].postback == "Yes"
        timelineRich.includes('"title":"Your order shipped"')
        edit.errors.includes("A card is not edited")
    }
})

feature("the limits refuse a rich message, each by its reason", {
    when() {
        ec.user.internalLoginUser("basic.comms.rich")
        const reason = (rich) => send(rich).out.reasonCode
        return {
            kind: reason({kind: "poster"}),
            noWords: reason({kind: "text", text: " ", suggestions: chips(1)}),
            noChips: reason({kind: "text", text: "Hi", suggestions: []}),
            tooManyChips: reason({kind: "text", text: "Hi", suggestions: chips(11)}),
            longButton: reason({kind: "text", text: "Hi", suggestions: [{type: "reply", text: "x".repeat(21)}]}),
            script: reason({kind: "text", text: "Hi", suggestions: [{type: "url", text: "Go", url: "javascript:alert(1)"}]}),
            badPhone: reason({kind: "text", text: "Hi", suggestions: [{type: "dial", text: "Call", phone: "555"}]}),
            cardButtons: reason({kind: "card", cards: [{title: "T", suggestions: chips(5)}]}),
            oneCarousel: reason({kind: "carousel", cards: [{title: "A", suggestions: chips(1)}]}),
            noPicture: reason({kind: "carousel", cards: [{title: "A", suggestions: chips(1)}, {title: "B", suggestions: chips(1)}]}),
            missingFile: reason({kind: "card", cards: [{title: "T", attachmentSeq: 1}]}),
            tooLong: reason({kind: "card", cards: [{title: "T".repeat(200), description: "d".repeat(1500), suggestions: chips(2)}]}),
            template: reason(Object.assign({}, CARD, {providerContent: {twilio: {contentSid: "HX" + "0".repeat(32)}}})),
        }
    },
    then(reasons) {
        reasons.kind == "badRich"
        reasons.noWords == "badRich"
        reasons.noChips == "badSuggestion"
        reasons.tooManyChips == "badSuggestion"
        reasons.longButton == "badSuggestion"
        reasons.script == "badSuggestion"
        reasons.badPhone == "badSuggestion"
        reasons.cardButtons == "badSuggestion"
        reasons.oneCarousel == "badRich"
        reasons.noPicture == "badImage"
        reasons.missingFile == "badImage"
        reasons.tooLong == "tooLong"
        // a provider's template is an app's, never a user's
        reasons.template == "notAllowed"
    }
})

feature("an encrypted room has no cards", {
    when() {
        ec.user.internalLoginUser("basic.comms.rich")
        const sealed = sys(() => run(CHAT + ".create#Room", {roomTypeEnumId: "CrtPrivate", roomName: "Sealed rich room", memberUserIdList: [READER]})).out.chatRoomId
        sys(() => {
            const room = ec.entity.find("moqui.chat.ChatRoom").condition("chatRoomId", sealed).one()
            room.set("encrypted", "Y")
            room.update()
        })
        return {sent: send(CARD, sealed)}
    },
    then({sent}) {
        sent.out.reasonCode == "encryptedRoom"
        sent.out.eventSeq == null
    }
})

feature("a member taps a reply on another's message, not on their own, not a link and not in a bridged room", {
    when() {
        ec.user.internalLoginUser("basic.comms.rich.reader")
        const tapped = reply({eventSeq: cardSeq, index: 0, cardIndex: 0})
        const link = reply({eventSeq: cardSeq, index: 1, cardIndex: 0})
        ec.user.internalLoginUser("basic.comms.rich")
        const own = reply({eventSeq: cardSeq, index: 0, cardIndex: 0})
        // an inbound RCS reply's postback, noted by receive#Sms: no message it answers is named
        const noted = sys(() => run(RICH + ".note#SuggestionReply", {chatRoomId: roomId, eventSeq: tapped.out.eventSeq, postback: "  order:7:thanks  "}))
        sys(() => {
            if (ec.entity.find("moqui.chat.ChatBridge").condition("chatBridgeId", "RICH_SPEC_BRIDGE").count() == 0) {
                const bridge = ec.entity.makeValue("moqui.chat.ChatBridge")
                bridge.setFields({chatBridgeId: "RICH_SPEC_BRIDGE", typeEnumId: "CbtSms", name: "Rich spec", statusId: "CbsDisabled"}, true, null, null)
                bridge.create()
            }
            // the room's own mapping, made again on each run (its room is new)
            ec.entity.find("moqui.chat.ChatBridgeDelivery").condition("chatRoomBridgeId", "RICH_SPEC_MAP").deleteAll()
            ec.entity.find("moqui.chat.ChatRoomBridge").condition("chatRoomBridgeId", "RICH_SPEC_MAP").deleteAll()
            const mapping = ec.entity.makeValue("moqui.chat.ChatRoomBridge")
            mapping.setFields({chatRoomBridgeId: "RICH_SPEC_MAP", chatRoomId: roomId, chatBridgeId: "RICH_SPEC_BRIDGE", externalRoomId: "+15550001111/+15550002222",
                directionEnumId: "CrbBoth", fromDate: ec.user.nowTimestamp}, true, null, null)
            mapping.create()
        })
        ec.user.internalLoginUser("basic.comms.rich.reader")
        const bridged = reply({eventSeq: cardSeq, index: 0, cardIndex: 0})
        // the mapping ends: the room has no outbound bridge again
        sys(() => {
            const mapping = ec.entity.find("moqui.chat.ChatRoomBridge").condition("chatRoomBridgeId", "RICH_SPEC_MAP").one()
            mapping.set("thruDate", ec.user.nowTimestamp)
            mapping.update()
        })
        return {tapped, link, own, noted, bridged}
    },
    then({tapped, link, own, noted, bridged}) {
        tapped.errors == ""
        eventOf(tapped.out.eventSeq).bodyText == "Thanks"
        eventOf(tapped.out.eventSeq).senderUserId == READER
        tapped.out.event.postback.data == "order:7:thanks"
        tapped.out.event.postback.toEventSeq == cardSeq
        link.out.reasonCode == "notReply"
        own.out.reasonCode == "ownMessage"
        noted.out.postback.data == "order:7:thanks"
        noted.out.postback.toEventSeq == null
        bridged.out.reasonCode == "bridgedRoom"
    }
})

feature("a read receipt is past delivered, and a final one names the provider's content to delete", {
    when() {
        const BRIDGE = "RICH_SPEC_BRIDGE"
        const content = "HX" + "1".repeat(32)
        sys(() => {
            const row = ec.entity.makeValue("moqui.chat.ChatBridgeDelivery")
            row.setFields({chatBridgeDeliveryId: "RICH_SPEC_DELIVERY", chatRoomId: roomId, eventSeq: cardSeq, chatRoomBridgeId: "RICH_SPEC_MAP",
                chatBridgeId: BRIDGE, statusEnumId: "CdsAccepted", remoteEventId: "SM-rich-1", providerContentId: content,
                createdDate: ec.user.nowTimestamp}, true, null, null)
            row.create()
        })
        const receipt = (status) => sys(() => run("org.moqui.chat.ChatBridgeServices.note#DeliveryReceipt", {chatBridgeId: BRIDGE, remoteEventId: "SM-rich-1", status}))
        const sent = receipt("sent")
        const delivered = receipt("delivered")
        const read = receipt("read")
        const back = receipt("delivered")
        const cleared = sys(() => run("org.moqui.chat.ChatBridgeServices.note#DeliveryContent", {chatBridgeId: BRIDGE, remoteEventId: "SM-rich-1", deletedContentId: content}))
        const row = sys(() => ec.entity.find("moqui.chat.ChatBridgeDelivery").condition("chatBridgeDeliveryId", "RICH_SPEC_DELIVERY").one())
        return {content, sent, delivered, read, back, cleared, row}
    },
    then({content, sent, delivered, read, back, cleared, row}) {
        sent.out.providerContentId == null
        delivered.out.providerContentId == content
        read.out.changed == true
        back.out.changed == false
        cleared.out.found == true
        row.receiptStatus == "read"
        row.providerContentId == null
    }
})
