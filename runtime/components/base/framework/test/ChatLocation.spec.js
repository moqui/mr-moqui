// Location messages (plans/Comms-Messaging.md §18.3 phase 20; ChatLocation.js, ChatLocationServices): a place with its
// coordinates or an address, its words and an OpenStreetMap link (what bridges and texts get), refused when it is not a
// place or in an encrypted room. The inbound RCS location (Sms.js) is the providers' tests (moqui-channels). A live
// location (plans/archive/Location-Sharing.md §3.5) is a share with the room's members and a message carrying its id only.
// Run: ./moq test framework --spec ChatLocation
const A = "CHAT_LOC_A"
const B = "CHAT_LOC_B"
// C: added to a room after its live location started
const C = "CHAT_LOC_C"
// left out of a live location when it starts: D an agent, E who put the room out of sight, F who blocked A, G whom A
// blocked, H a disabled account; I taken out of the room by A, J blocked by A after it started
const D = "CHAT_LOC_D"
const E = "CHAT_LOC_E"
const F = "CHAT_LOC_F"
const G = "CHAT_LOC_G"
const H = "CHAT_LOC_H"
const I = "CHAT_LOC_I"
const J = "CHAT_LOC_J"
const USERS = [A, B, C, D, E, F, G, H, I, J]
const CHAT = "org.moqui.chat.ChatServices."
const LOC = "org.moqui.chat.ChatLocationServices."
const MAPS = "org.moqui.maps.LocationServices."
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
        // a live share a failed run left on is stopped, so the spec runs again on --db existing
        for (const share of sys(() => Array.from(ec.entity.find("moqui.maps.LocationShare").condition("ownerUserId", "in", [A, B]).condition("endedDate", null).list())))
            as(share.ownerUserId, MAPS + "stop#Share", {shareId: share.shareId})
        // the blocks and the disabled account of the exclusions, so the spec runs again
        sys(() => {
            ec.entity.find("moqui.chat.ChatUserBlock").condition("userId", "in", USERS).deleteAll()
            ec.entity.find("moqui.security.UserAccount").condition("userId", H).updateAll({disabled: "N"})
        })
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
        // as phase 10's: a label alone, and coordinates out of range, keep the label with no link
        const labelOnly = as(A, LOC + "words#Location", {location: {label: "The shop"}})
        const outOfRange = as(A, LOC + "words#Location", {location: {latitude: 95, longitude: 10, label: "The shop"}})
        return {labelOnly, outOfRange, here, hereText: row("moqui.chat.ChatEvent", {chatRoomId: shared.team, eventSeq: here.eventSeq}).bodyText,
            typedText: row("moqui.chat.ChatEvent", {chatRoomId: shared.team, eventSeq: typed.eventSeq}).bodyText, refused, timeline, words}
    },
    then({labelOnly, outOfRange, here, hereText, typedText, refused, timeline, words}) {
        labelOnly.text == "Location: The shop"
        labelOnly.errors == ""
        outOfRange.text == "Location: The shop"
        outOfRange.location == null
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

// Live location in a room (plans/archive/Location-Sharing.md §3.5, LL4): a share with the room's members when it starts, as users,
// and a message that carries the share's id, never a position; its members see it through get#Share (which checks the
// audience) and nobody else does; one who joins the room later is not added and sees nothing of it, one who leaves stays
// in the share (the owner's act takes them out); an encrypted room is refused; a bridge gets the words only.
feature("live location in a room: its members see it and others do not, the message carries the share's id, a member added later sees nothing, the bridge gets the words", {
    when() {
        const live = room(A, {roomName: "Loc live", memberUserIdList: [B]})
        const hour = new Date(Date.now() + 3600000)
        const sent = as(A, LOC + "send#LiveLocation", {chatRoomId: live, thruDate: hour, clientRef: "loc-live-1"})
        // a send again by its clientRef answers the first message and starts no second share
        const again = as(A, LOC + "send#LiveLocation", {chatRoomId: live, thruDate: hour, clientRef: "loc-live-1"})
        const sharesAfterAgain = sys(() => ec.entity.find("moqui.maps.LocationShare").condition("ownerUserId", A).condition("endedDate", null).count())
        const message = row("moqui.chat.ChatEvent", {chatRoomId: live, eventSeq: sent.eventSeq})
        const content = JSON.parse(message.contentJson || "{}")
        const before = as(B, LOC + "get#LiveLocation", {chatRoomId: live, eventSeq: sent.eventSeq})
        const reported = as(A, MAPS + "report#Position", {shareId: sent.shareId, latitude: "40.7452", longitude: "-111.875", accuracy: "8",
            observedDate: new Date()})
        const member = as(B, LOC + "get#LiveLocation", {chatRoomId: live, eventSeq: sent.eventSeq})
        const owner = as(A, LOC + "get#LiveLocation", {chatRoomId: live, eventSeq: sent.eventSeq})
        // C is no member: the room is not theirs, and the share is not either
        const outsider = as(C, LOC + "get#LiveLocation", {chatRoomId: live, eventSeq: sent.eventSeq})
        const outsiderShare = as(C, MAPS + "get#Share", {shareId: sent.shareId})
        // a non-member learns nothing of a room, not even that it is encrypted
        const outsiderSends = as(C, LOC + "send#LiveLocation", {chatRoomId: live, thruDate: hour}).reasonCode
        const outsiderSealed = as(C, LOC + "send#LiveLocation", {chatRoomId: room(A, {roomName: "Loc live sealed outsider", memberUserIdList: [B], encrypted: true}),
            thruDate: hour}).reasonCode
        // C added to the room later: not added to the share
        as(A, CHAT + "add#Member", {chatRoomId: live, userId: C})
        const joined = as(C, LOC + "get#LiveLocation", {chatRoomId: live, eventSeq: sent.eventSeq})
        const joinedShare = as(C, MAPS + "get#Share", {shareId: sent.shareId})
        // B leaves the room: still in the share's audience until the owner takes them out (Who sees my location)
        as(B, CHAT + "remove#Member", {chatRoomId: live})
        const leftShare = as(B, MAPS + "get#Share", {shareId: sent.shareId})
        const sealed = as(A, LOC + "send#LiveLocation", {chatRoomId: room(A, {roomName: "Loc live sealed", memberUserIdList: [B], encrypted: true}),
            thruDate: hour}).reasonCode
        const alone = as(A, LOC + "send#LiveLocation", {chatRoomId: room(A, {roomName: "Loc live alone"}), thruDate: hour}).reasonCode
        // a refusal of the share (too long) keeps no message
        const tooLong = as(A, LOC + "send#LiveLocation", {chatRoomId: live, thruDate: new Date(Date.now() + 30 * 3600000), clientRef: "loc-live-long"})
        const longKept = sys(() => ec.entity.find("moqui.chat.ChatEvent").condition({chatRoomId: live, clientRef: "loc-live-long"}).count())
        // a bridged room: its delivery is the message's words, with no place in its content
        const bridged = room(A, {roomName: "Loc live bridged", memberUserIdList: [B]})
        make("moqui.chat.ChatBridge", {chatBridgeId: "CHAT_LOC_BRIDGE", name: "Spec bridge", adapterName: "none"})
        make("moqui.chat.ChatRoomBridge", {chatRoomBridgeId: "CHAT_LOC_MAP", chatRoomId: bridged, chatBridgeId: "CHAT_LOC_BRIDGE",
            externalRoomId: "+15550100", directionEnumId: "CrbOut", fromDate: new Date(Date.now() - 60000)})
        const viaBridge = as(A, LOC + "send#LiveLocation", {chatRoomId: bridged, thruDate: hour, precisionEnumId: "LspApproximate"})
        const delivered = sys(() => ec.entity.find("moqui.chat.ChatBridgeDelivery").condition({chatRoomId: bridged, eventSeq: viaBridge.eventSeq}).count())
        const bridgedEvent = row("moqui.chat.ChatEvent", {chatRoomId: bridged, eventSeq: viaBridge.eventSeq})
        // stopped: the room's member who joined later, and the owner, read it as ended
        as(A, MAPS + "stop#Share", {shareId: sent.shareId})
        as(A, MAPS + "stop#Share", {shareId: viaBridge.shareId})
        const ended = as(C, LOC + "get#LiveLocation", {chatRoomId: live, eventSeq: sent.eventSeq})
        const ownerEnded = as(A, LOC + "get#LiveLocation", {chatRoomId: live, eventSeq: sent.eventSeq})
        sys(() => {
            ec.entity.find("moqui.chat.ChatBridgeDelivery").condition("chatRoomId", bridged).deleteAll()
            ec.entity.find("moqui.chat.ChatRoomBridge").condition("chatRoomBridgeId", "CHAT_LOC_MAP").deleteAll()
        })
        return {sent, again, sharesAfterAgain, message, content, before, reported, member, owner, outsider, outsiderShare, outsiderSends, outsiderSealed, joined, joinedShare, leftShare,
            sealed, alone, tooLong, longKept, viaBridge, delivered, bridgedText: bridgedEvent.bodyText,
            bridgedPlace: JSON.parse(bridgedEvent.contentJson || "{}").location, ended, ownerEnded}
    },
    then({sent, again, sharesAfterAgain, message, content, before, reported, member, owner, outsider, outsiderShare, outsiderSends, outsiderSealed, joined, joinedShare, leftShare,
            sealed, alone, tooLong, longKept, viaBridge, delivered, bridgedText, bridgedPlace, ended, ownerEnded}) {
        sent.errors == ""
        sent.shareId != null
        sent.intervalSeconds == 5
        again.duplicate == true
        again.eventSeq == sent.eventSeq
        // the first send's share, so a client whose answer was lost starts sending it
        again.shareId == sent.shareId
        again.thruDate == sent.thruDate
        again.intervalSeconds == 5
        sharesAfterAgain == 1
        // the message carries the share's id, never a position
        content.location.liveShareId == sent.shareId
        content.location.source == "live"
        content.location.latitude == null
        message.bodyText.startsWith("Live location: Loc A, for 1 h\n")
        message.bodyText.includes("Live updates are in the app")
        !message.bodyText.includes("openstreetmap")
        before.state == "live"
        before.latest == null
        reported.statusCode == 200
        member.state == "live"
        member.owner == false
        member.latest.latitude == 40.7452
        owner.owner == true
        owner.share.audienceCount == 1
        outsider.state == null
        outsider.latest == null
        outsiderShare.reasonCode == "SHARE_NOT_FOUND"
        outsiderSends == "notMember"
        outsiderSealed == "notMember"
        joined.state == "notShared"
        joined.latest == null
        joinedShare.reasonCode == "SHARE_NOT_FOUND"
        leftShare.errors == ""
        leftShare.latest.latitude == 40.7452
        sealed == "encryptedRoom"
        alone == "noAudience"
        tooLong.reasonCode == "TOO_LONG"
        longKept == 0
        delivered == 1
        bridgedText.startsWith("Live location: Loc A, for 1 h\n")
        bridgedPlace.liveShareId == viaBridge.shareId
        bridgedPlace.approximate == true
        bridgedPlace.latitude == null
        ended.state == "ended"
        ownerEnded.state == "ended"
        ownerEnded.owner == true
    }
})

// Who a live location starts with and who loses it (plans/archive/Location-Sharing.md §3.5, the LL4 decisions): an agent, a member
// who put the room out of sight, one who blocked the owner, one the owner blocked and a disabled account are left out; a
// member taken out of the room by another, or blocked by the owner after it started, loses it; one who leaves by
// themselves keeps it, and the owner's card names them as no longer in the room.
feature("live location's audience: those left out when it starts; one taken out of the room or blocked later loses it, one who leaves keeps it", {
    when() {
        const crowd = room(A, {roomName: "Loc live crowd", memberUserIdList: [B, D, E, F, G, H, I, J]})
        sys(() => {
            ec.entity.find("moqui.chat.ChatRoomMember").condition({chatRoomId: crowd, userId: D}).updateAll({roleEnumId: "CmrAgent"})
            ec.entity.find("moqui.chat.ChatRoomMember").condition({chatRoomId: crowd, userId: E}).updateAll({filterEnumId: "CmfBlocked"})
            ec.entity.find("moqui.security.UserAccount").condition("userId", H).updateAll({disabled: "Y"})
        })
        make("moqui.chat.ChatUserBlock", {userId: F, blockedUserId: A, blockedDate: new Date()})
        make("moqui.chat.ChatUserBlock", {userId: A, blockedUserId: G, blockedDate: new Date()})
        const sent = as(A, LOC + "send#LiveLocation", {chatRoomId: crowd, thruDate: new Date(Date.now() + 3600000)})
        const audience = () => sys(() => Array.from(ec.entity.find("moqui.maps.LocationShareAudience").condition("shareId", sent.shareId)
            .condition("revokedDate", null).list()).map((one) => one.userId).sort())
        const started = audience()
        const linked = row("moqui.chat.ChatLiveShare", {chatRoomId: crowd, eventSeq: sent.eventSeq})
        // B's own live location in the room: A's block of J leaves it alone
        const other = as(B, LOC + "send#LiveLocation", {chatRoomId: crowd, thruDate: new Date(Date.now() + 3600000)})
        // I taken out by A (the room's owner), J blocked by A, B leaves by themselves
        const removed = as(A, CHAT + "remove#Member", {chatRoomId: crowd, userId: I})
        const blocked = as(A, "org.moqui.chat.ChatBlockServices.block#ChatUser", {blockedUserId: J})
        const left = as(B, CHAT + "remove#Member", {chatRoomId: crowd})
        // the revokes run once the removal and the block commit, on another thread (registerOnCommit)
        let after = audience()
        for (let tries = 0; tries < 100 && after.length > 1; tries++) {
            sleep(20)
            after = audience()
        }
        const removedSees = as(I, MAPS + "get#Share", {shareId: sent.shareId}).reasonCode
        const blockedSees = as(J, MAPS + "get#Share", {shareId: sent.shareId}).reasonCode
        const leftSees = as(B, MAPS + "get#Share", {shareId: sent.shareId})
        const blockedSeesOther = as(J, MAPS + "get#Share", {shareId: other.shareId})
        const ownerCard = as(A, LOC + "get#LiveLocation", {chatRoomId: crowd, eventSeq: sent.eventSeq})
        // revoke#ChatAudience acts on a share of the owner its row names only: a row naming A for B's share takes nobody out
        const wrongOwner = sys(() => ec.service.sync().name(MAPS + "revoke#ChatAudience").parameters({shareList: [{shareId: other.shareId, ownerUserId: A}],
            userId: F}).call())
        const wrongOwnerKept = as(F, MAPS + "get#Share", {shareId: other.shareId})
        as(A, MAPS + "stop#Share", {shareId: sent.shareId})
        as(B, MAPS + "stop#Share", {shareId: other.shareId})
        // an ended share keeps its link to the room (the end never waits on chat housekeeping) until the clean removes it
        const linkAfterStop = row("moqui.chat.ChatLiveShare", {chatRoomId: crowd, eventSeq: sent.eventSeq})
        sys(() => ec.entity.find("moqui.maps.LocationShare").condition("shareId", sent.shareId)
            .updateAll({endedDate: new Date(Date.now() - 40 * 86400000), trailPurgeDate: null}))
        sys(() => ec.service.sync().name(MAPS + "clean#LocationShares").call())
        const linkAfterClean = row("moqui.chat.ChatLiveShare", {chatRoomId: crowd, eventSeq: sent.eventSeq})
        return {sent, started, linked, other, removed, blocked, left, after, removedSees, blockedSees, leftSees, blockedSeesOther, ownerCard, wrongOwner,
            wrongOwnerKept, linkAfterStop, linkAfterClean}
    },
    then({sent, started, linked, other, removed, blocked, left, after, removedSees, blockedSees, leftSees, blockedSeesOther, ownerCard, wrongOwner,
            wrongOwnerKept, linkAfterStop, linkAfterClean}) {
        sent.errors == ""
        started == [B, I, J]
        linked.shareId == sent.shareId
        linked.ownerUserId == A
        removed.errors == ""
        blocked.errors == ""
        left.errors == ""
        after == [B]
        removedSees == "SHARE_NOT_FOUND"
        blockedSees == "SHARE_NOT_FOUND"
        leftSees.errors == ""
        ownerCard.share.audienceCount == 1
        ownerCard.share.leftRoomNames == ["Loc B"]
        ownerCard.share.intervalSeconds == 5
        // the card and the notice show the message's name, never a username
        ownerCard.share.label == "Loc A"
        // A's block of J takes J out of A's live locations only
        other.errors == ""
        blockedSeesOther.errors == ""
        wrongOwner.revokedCount == 0
        wrongOwnerKept.errors == ""
        linkAfterStop.shareId == sent.shareId
        linkAfterClean == null
    }
})
