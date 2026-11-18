// Location messages (plans/Comms-Messaging.md §18.3 phase 20; ChatLocation.js, ChatLocationServices): a place with its
// coordinates or an address, its words and an OpenStreetMap link (what bridges and texts get), refused when it is not a
// place or in an encrypted room. The inbound RCS location (Sms.js) is the providers' tests (moqui-channels).
// Run: ./moq test framework --spec ChatLocation
const A = "CHAT_LOC_A"
const B = "CHAT_LOC_B"
const USERS = [A, B]
const CHAT = "org.moqui.chat.ChatServices."
const LOC = "org.moqui.chat.ChatLocationServices."
const GROUP = "CHAT_LOC_SPEC"

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
const shared = {}

spec({
    stepwise: true,
    setupSpec() {
        sys(() => {
            for (const userId of USERS) {
                if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() === 0) {
                    ec.entity.makeValue("moqui.security.UserAccount").setFields({userId, username: username(userId), userFullName: "Loc " + userId.slice(9),
                        disabled: "N"}, true, null, null).create()
                }
            }
        })
        make("moqui.security.UserGroup", {userGroupId: GROUP, description: "Chat locations spec"})
        make("moqui.security.ArtifactGroup", {artifactGroupId: GROUP, description: "Chat locations spec"})
        make("moqui.security.ArtifactGroupMember", {artifactGroupId: GROUP, artifactName: "org\\.moqui\\.chat\\..*", artifactTypeEnumId: "AT_SERVICE",
            nameIsPattern: "Y", inheritAuthz: "Y"})
        make("moqui.security.ArtifactAuthz", {artifactAuthzId: GROUP, userGroupId: GROUP, artifactGroupId: GROUP, authzTypeEnumId: "AUTHZT_ALWAYS",
            authzActionEnumId: "AUTHZA_ALL"})
        for (const userId of USERS) make("moqui.security.UserGroupMember", {userGroupId: GROUP, userId, fromDate: new Date(Date.now() - 86400000)})
        shared.team = room(A, {roomName: "Loc team", memberUserIdList: [B]})
    },
    cleanupSpec() {
        ec.user.logoutUser()
    }
})

feature("a place with its coordinates or an address: its words and map link, its content in the timeline; what is not a place is refused", {
    when() {
        const here = as(A, LOC + "send#Location", {chatRoomId: shared.team, location: {latitude: 40.7452101234, longitude: -111.875, label: "Liberty Park",
            address: "600 E 900 S, Salt Lake City", accuracy: 12.4, source: "here"}})
        const typed = as(A, LOC + "send#Location", {chatRoomId: shared.team, location: {address: "10 Downing St, London"}})
        const refused = {
            none: as(A, LOC + "send#Location", {chatRoomId: shared.team, location: {label: "Somewhere"}}).reasonCode,
            half: as(A, LOC + "send#Location", {chatRoomId: shared.team, location: {latitude: 10}}).reasonCode,
            range: as(A, LOC + "send#Location", {chatRoomId: shared.team, location: {latitude: 91, longitude: 0}}).reasonCode,
            word: as(A, LOC + "send#Location", {chatRoomId: shared.team, location: {latitude: "north", longitude: 0}}).reasonCode,
            sealed: as(A, LOC + "send#Location", {chatRoomId: room(A, {roomName: "Loc sealed", memberUserIdList: [B], encrypted: true}),
                location: {latitude: 1, longitude: 2}}).reasonCode,
        }
        const timeline = Array.from(as(B, CHAT + "get#Timeline", {chatRoomId: shared.team}).eventList || []).find((one) => one.eventSeq === here.eventSeq)
        const words = as(A, LOC + "words#Location", {location: {latitude: "45.5", longitude: "-122.6", label: "The shop"}})
        return {here, hereText: row("moqui.chat.ChatEvent", {chatRoomId: shared.team, eventSeq: here.eventSeq}).bodyText,
            typedText: row("moqui.chat.ChatEvent", {chatRoomId: shared.team, eventSeq: typed.eventSeq}).bodyText, refused, timeline, words}
    },
    then({here, hereText, typedText, refused, timeline, words}) {
        here.errors == ""
        hereText == "Location: Liberty Park, 600 E 900 S, Salt Lake City\nhttps://www.openstreetmap.org/?mlat=40.745210&mlon=-111.875000#map=17/40.745210/-111.875000"
        typedText == "Location: 10 Downing St, London\nhttps://www.openstreetmap.org/search?query=10%20Downing%20St%2C%20London"
        refused.none == "badLocation"
        refused.half == "badLocation"
        refused.range == "badLocation"
        refused.word == "badLocation"
        refused.sealed == "encryptedRoom"
        timeline.location.latitude == 40.74521
        timeline.location.accuracy == 12
        timeline.location.source == "here"
        words.text == "Location: The shop\nhttps://www.openstreetmap.org/?mlat=45.500000&mlon=-122.600000#map=17/45.500000/-122.600000"
    }
})
