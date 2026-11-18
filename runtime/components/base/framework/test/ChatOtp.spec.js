// One-time codes and screen security (plans/Comms-Messaging.md §18.3 phase 21; ChatOtp.js, ChatPrivacyServices): a
// code found in an incoming text and not in a phone number, price, date, time, year or link; kept on the message, and its
// delete after a day only when every local member asks for it and the code is the text's only one; Keep; the settings and
// a member's own Block screenshots here. An inbound SMS through Sms.js is moqui-channels' test; the push's code is
// chat_push_tests'.
// Run: ./moq test framework --spec ChatOtp
const A = "CHAT_OTP_A"
const B = "CHAT_OTP_B"
const C = "CHAT_OTP_C"
const USERS = [A, B, C]
const CHAT = "org.moqui.chat.ChatServices."
const PRIV = "org.moqui.chat.ChatPrivacyServices."
const GROUP = "CHAT_OTP_SPEC"
const DAY = 86400000

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
const username = (userId) => userId.toLowerCase().replace(/_/g, ".")
function as(userId, name, parameters) {
    ec.user.internalLoginUser(username(userId))
    let out = {}
    let thrown = ""
    try {
        out = ec.service.sync().name(name).parameters(parameters || {}).call() || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
const make = (entity, fields) => sys(() => ec.entity.makeValue(entity).setFields(fields, true, null, null).createOrUpdate())
const row = (entity, fields) => sys(() => ec.entity.find(entity).condition(fields).one())
const room = (userId, fields) => as(userId, CHAT + "create#Room", Object.assign({roomTypeEnumId: "CrtPrivate"}, fields)).chatRoomId
const detect = (text) => sys(() => ec.service.sync().name(PRIV + "detect#OneTimeCode").parameter("text", text).call())
const pref = (userId, key, value) => make("moqui.security.UserPreference", {userId, preferenceKey: key, preferenceValue: value})
// a text from outside: a message of the room with no local sender, as a bridge's ingest makes it
function outsideText(chatRoomId, text) {
    const seq = as(A, CHAT + "send#Message", {chatRoomId, body: "<p>" + text + "</p>"}).eventSeq
    sys(() => {
        const event = ec.entity.find("moqui.chat.ChatEvent").condition({chatRoomId, eventSeq: seq}).one()
        event.set("senderUserId", null)
        event.update()
    })
    return seq
}
const shared = {}

spec({
    stepwise: true,
    setupSpec() {
        sys(() => {
            for (const userId of USERS) {
                if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() === 0) {
                    ec.entity.makeValue("moqui.security.UserAccount").setFields({userId, username: username(userId), userFullName: "Otp " + userId.slice(9),
                        disabled: "N"}, true, null, null).create()
                }
            }
        })
        make("moqui.security.UserGroup", {userGroupId: GROUP, description: "Chat codes spec"})
        make("moqui.security.ArtifactGroup", {artifactGroupId: GROUP, description: "Chat codes spec"})
        make("moqui.security.ArtifactGroupMember", {artifactGroupId: GROUP, artifactName: "org\\.moqui\\.(chat|comms)\\..*", artifactTypeEnumId: "AT_SERVICE",
            nameIsPattern: "Y", inheritAuthz: "Y"})
        make("moqui.security.ArtifactAuthz", {artifactAuthzId: GROUP, userGroupId: GROUP, artifactGroupId: GROUP, authzTypeEnumId: "AUTHZT_ALWAYS",
            authzActionEnumId: "AUTHZA_ALL"})
        for (const userId of USERS) make("moqui.security.UserGroupMember", {userGroupId: GROUP, userId, fromDate: new Date(Date.now() - DAY)})
        sys(() => ec.entity.find("moqui.security.UserPreference").condition("userId", "in", USERS).deleteAll())
        shared.team = room(A, {roomName: "Otp team", memberUserIdList: [B]})
    },
    cleanupSpec() {
        sys(() => ec.entity.find("moqui.security.UserPreference").condition("userId", "in", USERS).deleteAll())
        ec.user.logoutUser()
    }
})

feature("a code near its keyword is found; phone numbers, prices, dates, times, years, links, two as near and no keyword are not", {
    when() {
        const found = ["Your code is 482913", "G-482913 is your Google verification code", "Your verification code: 123-456", "OTP: 9012. Do not share it",
            "Use 7788 to sign in"].map((text) => detect(text))
        const none = ["Call me at +1 555 123 4567 about the code", "Your code costs $1234", "Pay 1234.50 with code", "The code meeting on 11/17",
            "Code review at 12:30", "Our code since 2026", "Code at https://x.example/482913", "Code 1234 or code 5678", "Your order 482913 shipped",
            "Code 1234567890"].map((text) => detect(text).code || null)
        return {found, none}
    },
    then({found, none}) {
        found[0].code == "482913"
        found[0].strong == true
        found[1].code == "482913"
        found[2].code == "123456"
        found[3].code == "9012"
        found[4].code == "7788"
        none.every((code) => code == null)
    }
})

feature("a code is kept on an incoming text; it goes after a day only when every member asks, and Keep takes that back", {
    when() {
        const first = outsideText(shared.team, "Your code is 482913")
        const marked = as(A, PRIV + "mark#OneTimeCode", {chatRoomId: shared.team, eventSeq: first})
        const firstRow = row("moqui.chat.ChatEvent", {chatRoomId: shared.team, eventSeq: first})
        // one member asks: nothing goes
        pref(A, "CHAT_OTP_DELETE", "Y")
        const second = outsideText(shared.team, "Your code is 551177")
        as(A, PRIV + "mark#OneTimeCode", {chatRoomId: shared.team, eventSeq: second})
        const secondRow = row("moqui.chat.ChatEvent", {chatRoomId: shared.team, eventSeq: second})
        // both ask: a day after it came
        pref(B, "CHAT_OTP_DELETE", "Y")
        const third = outsideText(shared.team, "Your code is 663344")
        const thirdMarked = as(A, PRIV + "mark#OneTimeCode", {chatRoomId: shared.team, eventSeq: third})
        const thirdRow = row("moqui.chat.ChatEvent", {chatRoomId: shared.team, eventSeq: third})
        // a user's own message is no incoming text; a text with no code keeps nothing
        const own = as(A, CHAT + "send#Message", {chatRoomId: shared.team, body: "<p>my code is 123456</p>"}).eventSeq
        const ownMarked = as(A, PRIV + "mark#OneTimeCode", {chatRoomId: shared.team, eventSeq: own})
        const timeline = Array.from(as(B, CHAT + "get#Timeline", {chatRoomId: shared.team}).eventList || []).find((one) => one.eventSeq === third)
        const stranger = as(C, PRIV + "keep#CodeMessage", {chatRoomId: shared.team, eventSeq: third})
        const kept = as(B, PRIV + "keep#CodeMessage", {chatRoomId: shared.team, eventSeq: third})
        return {marked, firstOtp: JSON.parse(firstRow.contentJson).otp, firstExpires: firstRow.expireDate, secondExpires: secondRow.expireDate,
            thirdMarked, thirdExpires: Number(thirdRow.expireDate), thirdSent: Number(thirdRow.sentDate), ownMarked, timeline, stranger, kept,
            afterKeep: row("moqui.chat.ChatEvent", {chatRoomId: shared.team, eventSeq: third}).expireDate}
    },
    then({marked, firstOtp, firstExpires, secondExpires, thirdMarked, thirdExpires, thirdSent, ownMarked, timeline, stranger, kept, afterKeep}) {
        marked.code == "482913"
        firstOtp.code == "482913"
        firstExpires == null
        secondExpires == null
        thirdMarked.code == "663344"
        thirdExpires == thirdSent + DAY
        ownMarked.code == null
        timeline.otp.code == "663344"
        timeline.expireDate == thirdExpires
        stranger.reasonCode == "notFound"
        kept.kept == true
        afterKeep == null
    }
})

feature("the privacy settings are the user's, and Block screenshots here is the member's own", {
    when() {
        const saved = as(A, PRIV + "store#ChatPrivacyPrefs", {screenSecurity: "encrypted", otpDelete: "", fromForm: true})
        const prefs = as(A, "org.moqui.comms.ChatScreenServices.get#ChatPrefs", {}).prefs
        const bad = as(A, PRIV + "store#ChatPrivacyPrefs", {screenSecurity: "sometimes"})
        const on = as(B, PRIV + "set#RoomScreenSecure", {chatRoomId: shared.team, on: true})
        const member = row("moqui.chat.ChatRoomMember", {chatRoomId: shared.team, userId: B})
        const stranger = as(C, PRIV + "set#RoomScreenSecure", {chatRoomId: shared.team, on: true})
        return {saved, prefs, bad, on, member, stranger}
    },
    then({saved, prefs, bad, on, member, stranger}) {
        saved.errors == ""
        prefs.screenSecurity == "encrypted"
        prefs.otpDelete == false
        bad.errors.includes("Choose off")
        on.screenSecure == true
        member.screenSecure == "Y"
        stranger.reasonCode == "notFound"
    }
})
