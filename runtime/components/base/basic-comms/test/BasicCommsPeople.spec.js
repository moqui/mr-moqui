// One person across networks (plans/Comms-Messaging.md §11.1, §18.3 phase 22): org.moqui.chat.ChatPeopleServices, as
// Chat's transitions call them. A user's people come from their rooms (a direct room of two, a phone room of one number),
// their own merges, splits and names, and the contacts hook (test/service/.../PeopleTestServices.xml, named by
// chat@contact-service in test/MoquiConf.xml), asked only about their rooms' addresses; a person who is a user shows no
// number or address the hook gave; a contact naming an unknown user or one who blocked the viewer is left out whole;
// a typed address cannot probe what the hook knows; one user's changes are theirs alone; an automatic person's key stays
// when a message comes; the ways come best first (Moqui chat, then the network they wrote on, then SMS, then mail) and a
// blocked number is never best; the mail line of an outside person comes from the contact's address (find#DirectChatMail)
// with the hook asked once for both; with the hook's hour used, rename and merge wait. Nothing is sent.
// Run: ./moq test basic-comms --spec BasicCommsPeople
const A = "CHAT_PEOPLE_SPEC_A"
const B = "CHAT_PEOPLE_SPEC_B"
const C = "CHAT_PEOPLE_SPEC_C"
// R blocked A; R's contact (the hook's) carries a number and an address of its own
const R = "CHAT_PEOPLE_SPEC_R"
const BLOCKER = "+15553334006"
const HIDDEN = "+15553334008"
const SMS = "CHAT_PEOPLE_SPEC_SMS"
const RCS = "CHAT_PEOPLE_SPEC_RCS"
const OURS = "+15553330001"
const OURS_RCS = "+15553330002"
const JANE = "+15553334001"
const PAT = "+15553334002"
const SAM = "+15553334003"
const GONE = "+15553334005"
const TYPED = "+15553334777"

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
const username = (userId) => userId.toLowerCase().replace(/_/g, ".")
// a service as a user, with authorization off (the fixtures)
function as(userId, name, parameters) {
    ec.user.internalLoginUser(username(userId))
    let out = {}
    let thrown = ""
    try {
        out = sys(() => ec.service.sync().name(name).parameters(parameters || {}).call()) || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
// a Chat transition as the user, as the page calls it: its JSON
function chat(userId, transition, parameters) {
    ec.user.internalLoginUser(username(userId))
    const str = ec.screen.makeTest().rootScreen("component://basic-comms/screen/BasicComms.xml")
        .render("Chat/" + transition, parameters || {}, transition === "people" || transition === "mailLines" ? null : "post")
    const output = String(str.output || "")
    ec.message.clearErrors()
    try { return JSON.parse(output) } catch (err) { return {unparsed: output.slice(0, 300)} }
}
function store(entityName, fields) {
    sys(() => {
        const value = ec.entity.makeValue(entityName)
        value.setFields(fields, true, null, null)
        value.createOrUpdate()
    })
}
function makeUser(userId, name) {
    store("moqui.security.UserAccount", {userId, username: username(userId), userFullName: name, emailAddress: userId.slice(-1).toLowerCase() + "@people-spec.example", disabled: "N"})
    sys(() => {
        if (ec.entity.find("moqui.security.UserGroup").condition("userGroupId", "BASIC_COMMS_USERS").count() == 0) return
        if (ec.entity.find("moqui.security.UserGroupMember").condition("userGroupId", "BASIC_COMMS_USERS").condition("userId", userId).count() > 0) return
        const member = ec.entity.makeValue("moqui.security.UserGroupMember")
        member.setFields({userGroupId: "BASIC_COMMS_USERS", userId, fromDate: ec.user.nowTimestamp}, true, null, null)
        member.create()
    })
}
function number(phoneNumber, bridgeId) {
    store("moqui.call.PhoneNumber", {phoneNumber, messagingBridgeId: bridgeId, routeTypeEnumId: "PnrUser", routeId: A})
    sys(() => {
        if (ec.entity.find("moqui.call.PhoneNumberUser").condition({phoneNumber, userId: A}).count() > 0) return
        const grant = ec.entity.makeValue("moqui.call.PhoneNumberUser")
        grant.setFields({phoneNumber, userId: A, fromDate: new Date(Date.now() - 60000)}, true, null, null)
        grant.create()
    })
}
const pairRoom = (ours, theirs) => sys(() => ec.service.sync().name("org.moqui.call.SmsServices.ensure#PairRoom")
    .parameters({ourNumber: ours, theirNumber: theirs}).call().chatRoomId)
const people = (userId) => chat(userId, "people")
const personOf = (out, chatRoomId) => (out.personList || []).find((person) => person.personKey === (out.roomPersonMap || {})[chatRoomId]) || null
const keys = (person) => (person ? person.addresses.map((one) => one.addressKey) : []).sort().join(" ")
const asked = () => String(ec.cache.getCache("moqui.test.people.asked").get(A) || "")
// the hook's answer kept for a user, forgotten so the next people ask the hook again
const forgetContacts = () => { for (const userId of [A, B, C, R]) ec.cache.getCache("moqui.chat.people.contacts").put(userId, "") }
// the hook's asks this hour for A (ChatPeople.js hookSlot)
const hourKey = () => A + ":" + Math.floor(Date.now() / 3600000)
const slots = () => Number(ec.cache.getCache("moqui.chat.people.hook.asked").get(hourKey()) || 0)
const rowsOf = (userId) => sys(() => Array.from(ec.entity.find("moqui.chat.ChatPersonAddress").condition("ownerUserId", userId).list()).map((row) => row.addressKey))
const rooms = {}

spec({
    stepwise: true,
    setupSpec() {
        makeUser(A, "People A")
        makeUser(B, "People B")
        makeUser(C, "People C")
        makeUser(R, "People R")
        // a run before this one left its rows and the hook's kept answer: each user's people start automatic
        sys(() => {
            for (const userId of [A, B, C, R]) {
                ec.entity.find("moqui.chat.ChatPersonAddress").condition("ownerUserId", userId).deleteAll()
                ec.entity.find("moqui.chat.ChatPerson").condition("ownerUserId", userId).deleteAll()
            }
        })
        forgetContacts()
        // R blocked A: R is to A as a user that does not exist, save the room R made with A
        store("moqui.chat.ChatUserBlock", {userId: R, blockedUserId: A, blockedDate: new Date()})
        store("moqui.chat.ChatBridge", {chatBridgeId: SMS, typeEnumId: "CbtSms", adapterName: "sms", name: "People spec texts", statusId: "CbsConnected",
            configJson: JSON.stringify({apiKey: "spec", apiBaseUrl: "http://127.0.0.1:9"})})
        store("moqui.chat.ChatBridge", {chatBridgeId: RCS, typeEnumId: "CbtRcs", adapterName: "rcs-twilio", name: "People spec RCS", statusId: "CbsConnected",
            configJson: JSON.stringify({accountSid: "spec", apiBaseUrl: "http://127.0.0.1:9"})})
        number(OURS, SMS)
        number(OURS_RCS, RCS)
        rooms.withB = as(A, "org.moqui.chat.ChatServices.create#Room", {roomTypeEnumId: "direct", memberUserIdList: [B]}).chatRoomId
        rooms.withC = as(A, "org.moqui.chat.ChatServices.create#Room", {roomTypeEnumId: "direct", memberUserIdList: [C]}).chatRoomId
        rooms.jane = pairRoom(OURS, JANE)
        rooms.pat = pairRoom(OURS, PAT)
        rooms.sam = pairRoom(OURS, SAM)
        rooms.samRcs = pairRoom(OURS_RCS, SAM)
        rooms.gone = pairRoom(OURS, GONE)
        rooms.blocker = pairRoom(OURS, BLOCKER)
        rooms.withR = as(R, "org.moqui.chat.ChatServices.create#Room", {roomTypeEnumId: "direct", memberUserIdList: [A]}).chatRoomId
        // Sam last wrote on RCS: a message from the other side (no local sender)
        sys(() => {
            const room = ec.entity.find("moqui.chat.ChatRoom").condition("chatRoomId", rooms.samRcs).one()
            const seq = Number(room.lastEventSeq || 0) + 1
            store("moqui.chat.ChatEvent", {chatRoomId: rooms.samRcs, eventSeq: seq, eventTypeEnumId: "CetMessage", senderUserId: null,
                sentDate: new Date(Date.now() - 60000), bodyText: "From Sam on RCS"})
            room.setFields({lastEventSeq: seq, lastEventDate: new Date(Date.now() - 60000), lastMessageSeq: seq}, true, null, null)
            room.update()
        })
    },
    cleanupSpec() {
        ec.user.internalLoginUser(username(A))
        try { sys(() => ec.resource.script("component://framework/service/org/moqui/call/CallHandling.js").execute("unblockNumber", {phoneNumber: "+15553334009"})) } catch (err) { ec.logger.warn("unblock: " + err) }
        ec.message.clearErrors()
    }
})

feature("each kind of room is a person; the contacts hook names an outside number and brings its address; a hook's link to a user groups them", {
    when() { return {out: people(A), hooked: asked()} },
    then({out, hooked}) {
        Array.isArray(out.personList)
        // the direct room and Pat's phone room are B (the hook's contact holds Pat's number and user B)
        personOf(out, rooms.withB).personKey == personOf(out, rooms.pat).personKey
        personOf(out, rooms.withB).userId == B
        personOf(out, rooms.withB).name == "People B"
        personOf(out, rooms.withB).nameFrom == "user"
        // a user's person shows only my own rooms' addresses: not the contact's other number or mail address
        keys(personOf(out, rooms.withB)) == ["tel:" + PAT, "user:" + B].sort().join(" ")
        personOf(out, rooms.withB).best.kind == "room"
        personOf(out, rooms.withB).best.chatRoomId == rooms.withB
        // Jane: an outside contact, with its other number and its address
        personOf(out, rooms.jane).name == "Jane Contact"
        personOf(out, rooms.jane).nameFrom == "contact"
        personOf(out, rooms.jane).contactUrl == "/qapps/spec/Contact?partyId=SPEC_JANE"
        keys(personOf(out, rooms.jane)) == ["email:jane.contact@people-spec.example", "tel:" + JANE, "tel:+15553334009"].sort().join(" ")
        personOf(out, rooms.jane).best.chatRoomId == rooms.jane
        personOf(out, rooms.jane).ways.some((way) => way.kind == "mail" && way.address == "jane.contact@people-spec.example" && way.url.includes("/Inbox/Compose?to="))
        // a new text to the contact's other number, from my numbers
        personOf(out, rooms.jane).ways.some((way) => way.kind == "text" && way.number == "+15553334009" && way.fromNumber == OURS)
        // Sam: SMS and RCS rooms, one person; the best is RCS, where Sam wrote last
        personOf(out, rooms.sam).personKey == personOf(out, rooms.samRcs).personKey
        personOf(out, rooms.sam).best.chatRoomId == rooms.samRcs
        personOf(out, rooms.sam).best.network == "rcs"
        // a contact naming an unknown user is left out whole: Gone's number is a person alone, no user, name or other address
        personOf(out, rooms.gone).userId == null
        keys(personOf(out, rooms.gone)) == "tel:" + GONE
        personOf(out, rooms.gone).name == null
        // the same for a contact naming R, who blocked A: no name from the hook, none of its addresses, no new chat with R
        keys(personOf(out, rooms.blocker)) == "tel:" + BLOCKER
        personOf(out, rooms.blocker).name == null
        !personOf(out, rooms.blocker).ways.some((way) => way.kind == "direct")
        // the room R made with A stays, as R's, with no new chat offered and no nudge (reachable false)
        personOf(out, rooms.withR).personKey != personOf(out, rooms.blocker).personKey
        personOf(out, rooms.withR).reachable == false
        // the hook was asked about the rooms' numbers
        hooked.includes("tel:" + JANE) && hooked.includes("tel:" + SAM)
    }
})

feature("merge: a typed number joins a person and never reaches the hook; the person is edited; another user sees none of it", {
    when() {
        const before = people(A)
        const sam = personOf(before, rooms.sam)
        const merged = chat(A, "mergePeople", {personKeys: sam.personKey, addressKeys: "tel:" + TYPED})
        const after = people(A)
        return {merged, after, hooked: asked(), other: people(C)}
    },
    then({merged, after, hooked, other}) {
        merged.reasonCode == null
        merged.personKey.startsWith("p:")
        personOf(after, rooms.sam).personKey == merged.personKey
        personOf(after, rooms.sam).edited
        keys(personOf(after, rooms.sam)).includes("tel:" + TYPED)
        !hooked.includes(TYPED)
        // A's rows are not C's: C's one person is A, automatic, with nothing A typed
        Array.isArray(other.personList)
        other.personList.length == 1
        other.personList[0].userId == A
        !other.personList[0].edited
        !other.personList.some((person) => keys(person).includes(TYPED))
    }
})

feature("merge refuses an address not mine to name, a bad number, two users, the 21st address, a user who blocked me, and a number not shown to me for a user", {
    when() {
        const out = people(A)
        const sam = personOf(out, rooms.sam).personKey
        const bob = personOf(out, rooms.withB).personKey
        return {
            ext: chat(A, "mergePeople", {personKeys: sam, addressKeys: "ext:XA_NOT_MINE"}),
            bad: chat(A, "mergePeople", {personKeys: sam, addressKeys: "tel:12"}),
            twoUsers: chat(A, "mergePeople", {addressKeys: "user:" + B + ",user:" + C}),
            many: chat(A, "mergePeople", {personKeys: sam, addressKeys: Array.from({length: 20}, (_, i) => "tel:+1555333" + String(5000 + i)).join(",")}),
            blocker: chat(A, "mergePeople", {personKeys: sam, addressKeys: "user:" + R}),
            // B's contact has HIDDEN, which A is not shown: typing it neither finds B nor joins B to it
            hidden: chat(A, "mergePeople", {personKeys: bob, addressKeys: "tel:" + HIDDEN}),
            rows: rowsOf(A),
        }
    },
    then({ext, bad, twoUsers, many, blocker, hidden, rows}) {
        ext.reasonCode == "notYourRoom"
        bad.reasonCode == "badAddress"
        twoUsers.reasonCode == "twoUsers"
        many.reasonCode == "tooManyAddresses"
        blocker.reasonCode == "userNotFound"
        hidden.reasonCode == "notShown"
        !rows.includes("tel:" + HIDDEN)
    }
})

feature("a typed number only B's contact knows joins an outside person alone, never B", {
    when() {
        const before = people(A)
        const merged = chat(A, "mergePeople", {personKeys: personOf(before, rooms.gone).personKey, addressKeys: "tel:" + HIDDEN})
        return {merged, after: people(A)}
    },
    then({merged, after}) {
        merged.personKey.startsWith("p:")
        keys(personOf(after, rooms.gone)) == ["tel:" + GONE, "tel:" + HIDDEN].sort().join(" ")
        personOf(after, rooms.gone).userId == null
        personOf(after, rooms.withB).personKey != personOf(after, rooms.gone).personKey
    }
})

feature("an automatic person's key stays when a new message comes in another of their rooms, so Rename still finds it", {
    when() {
        const before = personOf(people(A), rooms.jane).personKey
        // a newer message in Jane's room
        sys(() => {
            const room = ec.entity.find("moqui.chat.ChatRoom").condition("chatRoomId", rooms.jane).one()
            room.setFields({lastEventDate: new Date(), lastMessageDate: new Date()}, true, null, null)
            room.update()
        })
        const after = personOf(people(A), rooms.jane).personKey
        return {before, after, renamed: chat(A, "renamePerson", {personKey: before, displayName: "Jane at home"}), named: people(A)}
    },
    then({before, after, renamed, named}) {
        before.startsWith("a:")
        after == before
        renamed.personKey.startsWith("p:")
        personOf(named, rooms.jane).name == "Jane at home"
        // the rows are what A was shown: the contact's address and other number too
        keys(personOf(named, rooms.jane)).includes("email:jane.contact@people-spec.example")
    }
})

feature("separate, rename and reset: Pat's number out of B, named, then automatic again", {
    when() {
        const separated = chat(A, "separatePerson", {addressKey: "tel:" + PAT})
        const apart = people(A)
        const renamed = chat(A, "renamePerson", {personKey: personOf(apart, rooms.pat).personKey, displayName: "Pat at the desk"})
        const named = people(A)
        const reset = chat(A, "resetPerson", {personKey: personOf(named, rooms.pat).personKey})
        return {separated, apart, renamed, named, reset, again: people(A)}
    },
    then({separated, apart, named, reset, again}) {
        separated.personKey.startsWith("p:")
        personOf(apart, rooms.pat).personKey != personOf(apart, rooms.withB).personKey
        personOf(apart, rooms.pat).userId == null
        // separated by the user: the contact does not join it back, nor name it
        personOf(apart, rooms.pat).nameFrom == null
        personOf(named, rooms.pat).name == "Pat at the desk"
        personOf(named, rooms.pat).nameFrom == "mine"
        reset.reasonCode == null
        personOf(again, rooms.pat).personKey == personOf(again, rooms.withB).personKey
    }
})

feature("a number on my Blocked list is a way marked blocked, never the best", {
    when() {
        ec.user.internalLoginUser(username(A))
        sys(() => ec.resource.script("component://framework/service/org/moqui/call/CallHandling.js").execute("blockNumber", {phoneNumber: "+15553334009"}))
        ec.message.clearErrors()
        return {out: people(A)}
    },
    then({out}) {
        const jane = personOf(out, rooms.jane)
        jane.ways.find((way) => way.kind == "text" && way.number == "+15553334009").blocked
        !jane.best.blocked
        jane.best.chatRoomId == rooms.jane
    }
})

feature("the mail line of an outside person: the contact's address gives their phone room a line; a user's contact address does not", {
    when() {
        store("moqui.basic.email.EmailAccount", {emailAccountId: "PEOPLE_SPEC_MINE", ownerUserId: A, accountKindEnumId: "EakHosted",
            emailAddress: "people_spec_mine@people-spec.example", accountName: "People spec", statusId: "EasActive"})
        store("moqui.basic.email.EmailFolder", {emailFolderId: "PEOPLE_SPEC_MINE_IN", emailAccountId: "PEOPLE_SPEC_MINE", folderName: "Inbox", folderPath: "INBOX",
            folderKindEnumId: "EfkInbox", subscribed: "Y"})
        const at = new Date(Date.now() - 3600000)
        for (const [id, from] of [["PEOPLE_SPEC_M_JANE", "Jane.Contact@people-spec.example"], ["PEOPLE_SPEC_M_PAT", "pat.private@people-spec.example"]]) {
            store("moqui.basic.email.EmailMessage", {emailMessageId: id, rootEmailMessageId: id, emailAccountId: "PEOPLE_SPEC_MINE", emailFolderId: "PEOPLE_SPEC_MINE_IN",
                statusId: "ES_RECEIVED", messageDate: at, receivedDate: at, isRead: "N", subject: id, fromAddress: from})
        }
        return {lines: chat(A, "mailLines").mailList || []}
    },
    then({lines}) {
        lines.some((line) => line.chatRoomId == rooms.jane && line.subject == "PEOPLE_SPEC_M_JANE" && line.userId == null)
        // B's contact address (the hook's) is not B's to be credited with: only their account's and the ones I link
        !lines.some((line) => line.subject == "PEOPLE_SPEC_M_PAT")
    }
})

feature("the room list asks the hook once for the people and the mail lines; with the hour's asks used, the people say so and rename and merge wait", {
    when() {
        forgetContacts()
        const before = slots()
        people(A)
        chat(A, "mailLines")
        const spent = slots() - before
        // the hour's asks used, and no kept answer
        forgetContacts()
        ec.cache.getCache("moqui.chat.people.hook.asked").put(hourKey(), 60)
        const away = people(A)
        const rename = chat(A, "renamePerson", {personKey: personOf(away, rooms.sam).personKey, displayName: "Sam again"})
        const merge = chat(A, "mergePeople", {personKeys: personOf(away, rooms.sam).personKey, addressKeys: "tel:+15553334888"})
        ec.cache.getCache("moqui.chat.people.hook.asked").put(hourKey(), 0)
        return {spent, away, rename, merge}
    },
    then({spent, away, rename, merge}) {
        spent == 1
        away.contactsSkipped == true
        // Sam was changed by the merge above (a person with rows): its rename needs no contacts; a merge does
        rename.personKey == personOf(away, rooms.sam).personKey
        merge.reasonCode == "contactsUnavailable"
    }
})

feature("with the hook's asks used, an automatic person's rename waits too", {
    when() {
        forgetContacts()
        ec.cache.getCache("moqui.chat.people.hook.asked").put(hourKey(), 60)
        const away = people(A)
        const rename = chat(A, "renamePerson", {personKey: personOf(away, rooms.blocker).personKey, displayName: "Somebody"})
        ec.cache.getCache("moqui.chat.people.hook.asked").put(hourKey(), 0)
        return {rename}
    },
    then({rename}) {
        rename.reasonCode == "contactsUnavailable"
    }
})
