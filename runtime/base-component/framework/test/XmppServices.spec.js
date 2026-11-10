// The services the XMPP server calls (org.moqui.chat.XmppServices; framework/entity/XmppEntities.xml; plans/Comms-Framework.md
// §4.3, C3a; docs/interfaces/XMPP.md): an app password's SCRAM verifiers, made with it and sharing the user's salt; PLAIN
// taking app passwords only; a FAST token and a session's sign-in check going with the app password they came from; the
// archive, an event by its clientRef, an OMEMO message kept unreadable, PEP by access model, and uploads attached only by
// their owner, each refused to another user with the very same parameters; TURN for XMPP apps (get#IceServers) off by
// default, and on, refused to an external user (EXTERNAL_USER) and past the hour's rate, with the parameters an allowed
// user has. The calls run with authorization on, as the
// XMPP session makes them (XmppClientServices, granted to ALL_USERS).
// Run: ./moq test framework --spec XmppServices
const A = "XMPP_SPEC_A"
const B = "XMPP_SPEC_B"
const C = "XMPP_SPEC_C"
// an external user (an app's client contact): the framework permission EXTERNAL_USER through a group
const D = "XMPP_SPEC_D"
const EXTERNAL_GROUP = "XMPP_SPEC_EXTERNAL"
const X = "org.moqui.chat.XmppServices."

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
function username(userId) {
    return userId.toLowerCase().replace(/_/g, ".")
}
/** A call as userId with authorization on (null: internal work, authorization off). */
function as(userId, name, parameters) {
    if (userId) ec.user.internalLoginUser(username(userId))
    let out = {}
    let thrown = ""
    const call = () => ec.service.sync().name(name.includes(".") ? name : X + name).parameters(parameters || {}).call() || {}
    try {
        out = userId ? call() : sys(call)
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : "")
    ec.message.clearErrors()
    return Object.assign({errors}, out)
}
function makeUser(userId) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() === 0) {
            const account = ec.entity.makeValue("moqui.security.UserAccount")
            account.setFields({userId, username: username(userId), userFullName: "Xmpp " + userId.slice(-1), disabled: "N"}, true, null, null)
            account.create()
        }
    })
}
let room = null
const passwords = {}

spec({
    setupSpec() {
        for (const u of [A, B, C, D]) makeUser(u)
        sys(() => {
            if (!ec.entity.find("moqui.security.UserGroup").condition("userGroupId", EXTERNAL_GROUP).one()) {
                const group = ec.entity.makeValue("moqui.security.UserGroup")
                group.setFields({userGroupId: EXTERNAL_GROUP, description: "Xmpp spec external"}, true, null, null)
                group.create()
                const grant = ec.entity.makeValue("moqui.security.UserGroupPermission")
                grant.setFields({userGroupId: EXTERNAL_GROUP, userPermissionId: "EXTERNAL_USER", fromDate: new Date(Date.now() - 60000)}, true, null, null)
                grant.create()
            }
            if (!ec.entity.find("moqui.security.UserGroupMember").condition({userGroupId: EXTERNAL_GROUP, userId: D}).one()) {
                const member = ec.entity.makeValue("moqui.security.UserGroupMember")
                member.setFields({userGroupId: EXTERNAL_GROUP, userId: D, fromDate: new Date(Date.now() - 60000)}, true, null, null)
                member.create()
            }
            ec.cache.getCache("moqui.xmpp.turn.issued").clear()
        })
        sys(() => {
            for (const entity of ["moqui.xmpp.XmppPepItem", "moqui.xmpp.XmppPepNode", "moqui.xmpp.XmppUpload", "moqui.xmpp.XmppFastToken",
                    "moqui.security.UserAppPassword"]) {
                ec.entity.find(entity).condition("userId", "in", [A, B, C]).deleteAll()
            }
        })
        ec.user.internalLoginUser(username(A))
        room = sys(() => ec.service.sync().name("org.moqui.chat.ChatServices.create#Room")
                .parameters({roomTypeEnumId: "CrtPrivate", roomName: "Xmpp spec", memberUserIdList: [B]}).call()).chatRoomId
        sys(() => ec.service.sync().name("org.moqui.chat.ChatServices.send#Message")
                .parameters({chatRoomId: room, body: "<p>First</p>", clientRef: "spec-ref-1"}).call())
    }
})

feature("an app password gets SCRAM verifiers sharing the user's salt, and PLAIN takes app passwords only", {
    when() {
        ec.user.internalLoginUser(username(A))
        passwords.first = sys(() => ec.service.sync().name("org.moqui.impl.UserServices.create#UserAppPassword").parameter("name", "phone").call()).password
        passwords.second = sys(() => ec.service.sync().name("org.moqui.impl.UserServices.create#UserAppPassword").parameter("name", "laptop").call()).password
        const rows = sys(() => Array.from(ec.entity.find("moqui.security.UserAppPassword").condition("userId", A).list()))
        const credentials = as(null, "get#ScramCredentials", {username: username(A)})
        const nobody = as(null, "get#ScramCredentials", {username: "no.such.user"})
        const plain = as(null, "check#PlainPassword", {username: username(A), password: passwords.first})
        const wrong = as(null, "check#PlainPassword", {username: username(A), password: "wrongwrongwrongwrong"})
        const other = as(null, "check#PlainPassword", {username: username(B), password: passwords.first})
        return {rows, credentials, nobody, plain, wrong, other}
    },
    then({rows, credentials, nobody, plain, wrong, other}) {
        rows.length == 2
        rows[0].scramSalt == rows[1].scramSalt
        Number(rows[0].scramIterations) == 10000
        rows[0].scramStoredKeys.split(" ").length == 2
        credentials.salt == rows[0].scramSalt
        credentials.storedKeyList.length == 4
        credentials.serverKeyList.length == 4
        credentials.appPasswordIdList.length == 4
        credentials.appPasswordIdList.every((id) => rows.some((row) => row.appPasswordId == id))
        !nobody.salt
        plain.errors == ""
        plain.userId == A
        rows.some((row) => row.appPasswordId == plain.appPasswordId)
        wrong.errors != ""
        other.errors != ""
    }
})

feature("a session and its FAST token go with the app password signed in with: revoked, both are refused", {
    when() {
        const plain = as(null, "check#PlainPassword", {username: username(A), password: passwords.first})
        const issued = as(null, "issue#FastToken", {userId: A, userAgentId: "spec-agent", mechanism: "HT-SHA-256-EXPR",
            token: "spec-token-1", lifetimeSeconds: 600, appPasswordId: plain.appPasswordId})
        const tokens = {username: username(A), userAgentId: "spec-agent"}
        const check = {userId: A, appPasswordId: plain.appPasswordId}
        const before = as(null, "get#FastToken", tokens)
        const valid = as(null, "check#Credential", check)
        const anotherUser = as(null, "check#Credential", Object.assign({}, check, {userId: B}))
        // as on the user's App passwords screen
        ec.user.internalLoginUser(username(A))
        const revoked = as(null, "org.moqui.impl.UserServices.revoke#UserAppPassword", {appPasswordId: plain.appPasswordId})
        const after = as(null, "get#FastToken", tokens)
        const invalid = as(null, "check#Credential", check)
        return {plain, issued, before, valid, anotherUser, revoked, after, invalid}
    },
    then({plain, issued, before, valid, anotherUser, revoked, after, invalid}) {
        plain.errors == ""
        issued.errors == ""
        before.tokenList.length == 1
        before.tokenList[0].token == "spec-token-1"
        before.tokenList[0].appPasswordId == plain.appPasswordId
        valid.valid == true
        anotherUser.valid == false
        revoked.errors == ""
        after.tokenList.length == 0
        invalid.valid == false
    }
})

feature("the archive and an event by its clientRef are a member's; another user is refused with the same parameters", {
    when() {
        const params = {chatRoomId: room, limit: 10, latest: true}
        const member = as(B, "get#Archive", params)
        const outsider = as(C, "get#Archive", params)
        const ref = {chatRoomId: room, clientRef: "spec-ref-1"}
        const found = as(A, "find#EventByRef", ref)
        const notFound = as(C, "find#EventByRef", ref)
        const roster = as(A, "get#Roster", {})
        return {member, outsider, found, notFound, roster}
    },
    then({member, outsider, found, notFound, roster}) {
        member.errors == ""
        member.eventList.length == 1
        member.eventList[0].clientRef == "spec-ref-1"
        member.eventList[0].bodyText == "First"
        outsider.errors != ""
        !outsider.eventList || outsider.eventList.length == 0
        found.errors == ""
        Number(found.eventSeq) >= 1
        notFound.errors != ""
        roster.errors == ""
    }
})

feature("an OMEMO message is kept as it came with no text; another user is refused with the same parameters", {
    when() {
        const params = {chatRoomId: room, xml: "<encrypted xmlns='urn:xmpp:omemo:2'><payload>CIPHER</payload></encrypted>",
            namespace: "urn:xmpp:omemo:2", clientRef: "spec-omemo-1"}
        const stored = as(A, "store#EncryptedMessage", params)
        const again = as(A, "store#EncryptedMessage", params)
        const outsider = as(C, "store#EncryptedMessage", params)
        const notOmemo = as(A, "store#EncryptedMessage", Object.assign({}, params, {namespace: "jabber:x:encrypted", clientRef: "spec-omemo-2"}))
        const row = sys(() => ec.entity.find("moqui.chat.ChatEvent").condition("chatRoomId", room).condition("eventSeq", stored.eventSeq).one())
        return {stored, again, outsider, notOmemo, row}
    },
    then({stored, again, outsider, notOmemo, row}) {
        stored.errors == ""
        again.eventSeq == stored.eventSeq
        outsider.errors != ""
        notOmemo.errors != ""
        row.bodyText == null
        JSON.parse(row.contentJson).xmppEncrypted.xml.includes("CIPHER")
        JSON.parse(row.contentJson).unreadable == true
    }
})

feature("PEP items are read by their node's access model; a whitelist node is its owner's alone", {
    when() {
        const devices = {node: "urn:xmpp:omemo:2:devices", itemId: "current", payloadXml: "<devices xmlns='urn:xmpp:omemo:2'><device id='7'/></devices>",
            options: {access_model: "presence"}}
        const published = as(A, "publish#PepItem", devices)
        const conflicting = as(A, "publish#PepItem", Object.assign({}, devices, {options: {access_model: "whitelist"}}))
        const read = {ownerUserId: A, node: "urn:xmpp:omemo:2:devices"}
        const coMember = as(B, "get#PepItems", read)
        const stranger = as(C, "get#PepItems", read)
        const secret = as(A, "publish#PepItem", {node: "urn:xmpp:bookmarks:1", itemId: "room@rooms", payloadXml: "<conference xmlns='urn:xmpp:bookmarks:1'/>",
            options: {access_model: "whitelist"}})
        const bookmarks = {ownerUserId: A, node: "urn:xmpp:bookmarks:1"}
        const own = as(A, "get#PepItems", bookmarks)
        const others = as(B, "get#PepItems", bookmarks)
        return {published, conflicting, coMember, stranger, secret, own, others}
    },
    then({published, conflicting, coMember, stranger, secret, own, others}) {
        published.errors == ""
        published.notifyUserIdList.includes(B)
        !published.notifyUserIdList.includes(C)
        conflicting.preconditionFailed == true
        coMember.itemList.length == 1
        coMember.itemList[0].payloadXml.includes("device id='7'")
        stranger.denied == true
        secret.errors == ""
        own.itemList.length == 1
        others.denied == true
    }
})

feature("an upload is attached only by the user who put it; a slot over the limit is refused", {
    when() {
        const slot = as(A, "create#UploadSlot", {fileName: "plan.txt", fileSize: 5, contentType: "text/plain"})
        sys(() => {
            const row = ec.entity.find("moqui.xmpp.XmppUpload").condition("userId", A).condition("fileName", "plan.txt").one()
            row.completedDate = ec.user.nowTimestamp
            row.update()
        })
        const url = `https://moqui.local/xmpp-upload/${slot.token}/${slot.urlName}`
        const params = {chatRoomId: room, url}
        const notTheirs = as(B, "attach#Upload", params)
        const owner = as(A, "attach#Upload", params)
        const tooLarge = as(A, "create#UploadSlot", {fileName: "big.bin", fileSize: 999999999999})
        return {slot, notTheirs, owner, tooLarge}
    },
    then({slot, notTheirs, owner, tooLarge}) {
        slot.errors == ""
        slot.token.length == 32
        notTheirs.errors != ""
        owner.errors == ""
        owner.chatAttachmentId != null
        tooLarge.errors != ""
    }
})

feature("TURN for XMPP apps is off by default; on, an external user and a user past the hour's rate get none", {
    when() {
        // the same parameters for each user: none (the credential is keyed to the signed-in user)
        const ask = (userId) => as(userId, "get#IceServers", {})
        const off = withProperty("xmpp_turn_enabled", null, () => ask(A))
        return withProperty("turn_external_urls", "turn:turn.example.org:3478?transport=udp", () =>
            withProperty("xmpp_turn_enabled", "true", () => withProperty("xmpp_turn_per_hour", "2", () => {
                const allowed = ask(A)
                const external = ask(D)
                const second = ask(A)
                const third = ask(A)
                const other = ask(B)
                return {off, allowed, external, second, third, other}
            })))
    },
    then({off, allowed, external, second, third, other}) {
        off.errors == ""
        off.serviceList.length == 0
        off.reasonCode == "turnOff"
        allowed.errors == ""
        allowed.serviceList.length == 1
        allowed.serviceList[0].host == "turn.example.org"
        allowed.serviceList[0].username.endsWith(":xmpp." + A)
        external.errors == ""
        external.serviceList.length == 0
        external.reasonCode == "notAllowed"
        second.serviceList.length == 1
        third.serviceList.length == 0
        third.reasonCode == "rateLimited"
        other.serviceList.length == 1
    }
})
