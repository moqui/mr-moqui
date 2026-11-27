// The services the XMPP server calls (org.moqui.chat.XmppServices; framework/entity/XmppEntities.xml; plans/archive/Comms-Framework.md
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
                    "moqui.xmpp.XmppBlockedJid", "moqui.xmpp.XmppPushRegistration", "moqui.security.UserAppPassword"]) {
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
        passwords.first = sys(() => ec.service.sync().name("org.moqui.impl.UserServices.create#UserAppPassword").parameter("name", "phone").parameter("scope", "xmpp").call()).password
        passwords.second = sys(() => ec.service.sync().name("org.moqui.impl.UserServices.create#UserAppPassword").parameter("name", "laptop").parameter("scope", "xmpp").call()).password
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

// SEC-COM-15: an account past its terminateDate signs in to XMPP no more, and its session ends at the next check
feature("a session ends when its account passes its terminate date", {
    when() {
        ec.user.internalLoginUser(username(C))
        const appPasswordId = sys(() => ec.service.sync().name("org.moqui.impl.UserServices.create#UserAppPassword").parameter("name", "terminal").parameter("scope", "xmpp").call()).appPasswordId
        const check = {userId: C, appPasswordId}
        const before = as(null, "check#Credential", check)
        const terminate = (date) => sys(() => {
            const account = ec.entity.find("moqui.security.UserAccount").condition("userId", C).one()
            account.terminateDate = date
            account.update()
        })
        terminate(new Date(Date.now() - 60000))
        const after = as(null, "check#Credential", check)
        const account = as(null, "get#Account", {username: username(C)})
        terminate(null)
        return {appPasswordId, before, after, account}
    },
    then({appPasswordId, before, after, account}) {
        !!appPasswordId
        before.valid == true
        after.valid == false
        account.canLogin == false
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

feature("user location (XEP-0080) is refused at the services too, in any spelling, and a row kept earlier is not served", {
    when() {
        const GEOLOC = "http://jabber.org/protocol/geoloc"
        const location = (node) => ({node, itemId: "current", payloadXml: "<geoloc xmlns='" + GEOLOC + "'><lat>48.85</lat></geoloc>",
            options: {access_model: "open"}})
        const published = as(A, "publish#PepItem", location(GEOLOC))
        const variant = as(A, "publish#PepItem", location(" HTTP://Jabber.org/protocol/GEOLOC "))
        const kept = Array.from(sys(() => ec.entity.find("moqui.xmpp.XmppPepItem").condition("userId", A).list()))
            .filter((row) => /geoloc/i.test(String(row.node))).length
        // a location row from before the rule, in an open node
        sys(() => {
            const node = ec.entity.makeValue("moqui.xmpp.XmppPepNode")
            node.setFields({userId: A, node: GEOLOC, accessModel: "open", createdDate: ec.user.nowTimestamp}, true, null, null)
            node.createOrUpdate()
            const item = ec.entity.makeValue("moqui.xmpp.XmppPepItem")
            item.setFields({userId: A, node: GEOLOC, itemId: "old", payloadXml: "<geoloc xmlns='" + GEOLOC + "'><lat>1</lat></geoloc>",
                payloadBytes: 50, publishedDate: ec.user.nowTimestamp}, true, null, null)
            item.createOrUpdate()
        })
        const own = as(A, "get#PepItems", {ownerUserId: A, node: GEOLOC})
        const coMember = as(B, "get#PepItems", {ownerUserId: A, node: " Http://jabber.org/protocol/geoloc"})
        const ordinary = as(A, "publish#PepItem", {node: "urn:xmpp:avatar:metadata", itemId: "a1",
            payloadXml: "<metadata xmlns='urn:xmpp:avatar:metadata'/>"})
        sys(() => {
            ec.entity.find("moqui.xmpp.XmppPepItem").condition("userId", A).condition("node", GEOLOC).deleteAll()
            ec.entity.find("moqui.xmpp.XmppPepNode").condition("userId", A).condition("node", GEOLOC).deleteAll()
        })
        // ASCII whitespace alone is trimmed, as the XMPP session trims it (Rust's trim keeps U+FEFF, JavaScript's strips it)
        const check = (name) => as(null, "get#PepNodeRefused", {node: name}).refused
        const spaced = check("\t" + GEOLOC + "\r\n"), marked = check("\uFEFF" + GEOLOC)
        return {published, variant, kept, own, coMember, ordinary, spaced, marked}
    },
    then({published, variant, kept, own, coMember, ordinary, spaced, marked}) {
        // refused (the session answers forbidden), not an error (which it would answer internal-server-error)
        published.errors == ""
        published.refused == true
        variant.refused == true
        kept == 0
        own.refused == true
        own.denied == true
        own.itemList.length == 0
        coMember.denied == true
        coMember.itemList.length == 0
        ordinary.errors == ""
        ordinary.refused == false
        spaced == true
        marked == false
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

// SEC-COM-20: a user's PEP bytes, block list and push registrations stop at their caps (refused with resourceConstraint,
// nothing kept), and an upload no message was given is deleted a day after it was put
feature("PEP bytes, blocked JIDs and push registrations stop at their caps; an unattached upload goes after a day", {
    when() {
        const capped = withConf("xmpp-server@pep-max-bytes", "300", () => withConf("xmpp-server@block-max-jids", "2", () =>
            withConf("xmpp-server@push-max-registrations", "1", () => {
                // 42 bytes of element around the text
                const item = (itemId, size) => as(C, "publish#PepItem", {node: "urn:xmpp:avatar:data", itemId,
                    payloadXml: "<data xmlns='urn:xmpp:avatar:data'>" + "x".repeat(size) + "</data>"})
                const first = item("a", 100)
                const replacing = item("a", 200)
                const over = item("b", 100)
                const items = sys(() => ec.entity.find("moqui.xmpp.XmppPepItem").condition("userId", C).count())
                const blocked = as(C, "block#Jids", {jidList: ["one@x.test", "two@x.test"]})
                const reblocked = as(C, "block#Jids", {jidList: ["two@x.test"]})
                const third = as(C, "block#Jids", {jidList: ["three@x.test", "one@x.test"]})
                const blockList = as(C, "get#BlockedJids", {}).jidList
                const push = as(C, "enable#Push", {pushJid: "push.x.test", node: "n1"})
                const repush = as(C, "enable#Push", {pushJid: "push.x.test", node: "n1"})
                const another = as(C, "enable#Push", {pushJid: "push.x.test", node: "n2"})
                const pushes = sys(() => ec.entity.find("moqui.xmpp.XmppPushRegistration").condition("userId", C).count())
                return {first, replacing, over, items, blocked, reblocked, third, blockList, push, repush, another, pushes}
            })))
        // three uploads of A put 25 hours ago, one given to a message, and one put an hour ago
        const put = (fileName, hoursAgo) => {
            as(A, "create#UploadSlot", {fileName, fileSize: 5, contentType: "text/plain"})
            return sys(() => {
                const row = ec.entity.find("moqui.xmpp.XmppUpload").condition("userId", A).condition("fileName", fileName).one()
                row.completedDate = new Date(Number(ec.user.nowTimestamp) - hoursAgo * 3600000)
                row.update()
                return row
            })
        }
        put("old.txt", 25)
        const attached = put("given.txt", 25)
        put("new.txt", 1)
        sys(() => {
            const file = ec.entity.makeValue("moqui.chat.ChatAttachment")
            file.setSequencedIdPrimary()
            file.setFields({chatRoomId: room, location: attached.location, fileName: "given.txt", uploadedByUserId: A}, true, null, null)
            file.create()
        })
        const cleaned = as(null, "clean#Uploads", {})
        const left = sys(() => Array.from(ec.entity.find("moqui.xmpp.XmppUpload").condition("userId", A)
                .condition("fileName", "in", ["old.txt", "given.txt", "new.txt"]).list()).map((row) => row.fileName).sort())
        return Object.assign(capped, {cleaned, left})
    },
    then({first, replacing, over, items, blocked, reblocked, third, blockList, push, repush, another, pushes, cleaned, left}) {
        first.errors == "" && first.resourceConstraint == false
        replacing.errors == "" && replacing.resourceConstraint == false
        over.errors == ""
        over.resourceConstraint == true
        items == 1
        blocked.resourceConstraint == false
        reblocked.resourceConstraint == false
        third.resourceConstraint == true
        blockList.length == 2
        !blockList.includes("three@x.test")
        push.resourceConstraint == false
        repush.resourceConstraint == false
        another.resourceConstraint == true
        pushes == 1
        cleaned.errors == ""
        left.join(",") == "given.txt,new.txt"
    }
})

feature("TURN for XMPP apps is off by default; on, an external user and a user past the hour's rate get none", {
    when() {
        // the same parameters for each user: none (the credential is keyed to the signed-in user)
        const ask = (userId) => as(userId, "get#IceServers", {})
        const off = withConf("xmpp-server@turn-enabled", null, () => ask(A))
        return withConf("media-server/turn-server@external-urls", "turn:turn.example.org:3478?transport=udp", () =>
            withConf("xmpp-server@turn-enabled", "true", () => withConf("xmpp-server@turn-per-hour", "2", () => {
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

feature("two accounts whose usernames differ only by case are one JID: neither is the account of that name", {
    when() {
        const make = (userId, name) => sys(() => {
            if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() === 0) {
                const account = ec.entity.makeValue("moqui.security.UserAccount")
                account.setFields({userId, username: name, userFullName: "Xmpp case " + name, disabled: "N"}, true, null, null)
                account.create()
            }
        })
        make("XMPP_SPEC_CASE_1", "xmpp.case.twin")
        const alone = as(null, "get#Account", {username: "Xmpp.Case.Twin"})
        make("XMPP_SPEC_CASE_2", "Xmpp.Case.Twin")
        const both = as(null, "get#Account", {username: "xmpp.case.twin"})
        sys(() => ec.entity.find("moqui.security.UserAccount").condition("userId", "in", ["XMPP_SPEC_CASE_1", "XMPP_SPEC_CASE_2"]).deleteAll())
        return {alone, both}
    },
    then({alone, both}) {
        alone.userId == "XMPP_SPEC_CASE_1"
        both.userId == null
        both.canLogin == false
    }
})
