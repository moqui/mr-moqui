// Encrypted chat rooms in browsers (plans/Comms-Framework.md §4.5, C7; docs/interfaces/Chat-Keys.md "Encrypted rooms"):
// john.doe makes an encrypted room with calls.client.b and calls.client.c (CallClientFixtures). Each page holds its own
// crypto device (moqui.crypto over matrix-sdk-crypto); messages are Megolm-encrypted in the sending page and decrypted in
// the others (moqui.chat.sealMessage, openEvents), an image goes up encrypted and comes back as a decrypted blob, and
// nothing the sender posted, nor its outbox in localStorage, holds the plaintext. When calls.client.c leaves, the next
// message is in a new Megolm session that c cannot decrypt. A second tab of the same browser (a window the page opens, which
// shares the browser's storage, Web Locks and BroadcastChannel) reads through the tab that holds the store, and takes the store
// when that one closes it. A new device of john.doe (a page of its own, so a new store) reads nothing older until it
// restores the key backup with the recovery key.
// Run: ./moq test-client basic-comms --serve --spec EncryptedChatFlow
const PASSWORD = "CallsClient1!!"
const SECRET = "Encrypted lunch plans 7f3a"
const LATER = "After c left 91be"
// a 1x1 PNG
const PNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="
let a = null
let b = null
let c = null
const room = {}

// helpers defined in each page: a Chat transition, an encrypted send, a timeline page opened
const HELPERS = `(() => {
    const base = "/apps/BasicComms/Chat/";
    window.__e2ee = {
        post: (name, params) => window.moqui.request(base + name, { method: "POST", params }).then((r) => r.json || {}),
        get: (name, params) => window.moqui.request(base + name, { params }).then((r) => r.json || {}),
        send: (roomId, members, html, files) => window.moqui.chat.sealMessage(roomId, members, html, files || []).then((sealed) =>
            window.__e2ee.post("send", Object.assign({ chatRoomId: roomId, clientRef: window.moqui.chat.newClientRef(),
                chatAttachmentIds: (files || []).map((f) => f.chatAttachmentId).join(",") }, sealed))),
        page: (roomId) => window.__e2ee.get("timeline", { chatRoomId: roomId, limit: 50 }).then((page) =>
            window.moqui.chat.openEvents(roomId, page.eventList || [], (file) => base + "attachment?chatAttachmentId=" + file.chatAttachmentId)
                .then((list) => list.filter((e) => e.type === "message")))
    };
    window.__posted = [];
    const fetch = window.fetch;
    window.fetch = (url, init) => { if (init && init.body && typeof init.body === "string") window.__posted.push(init.body); return fetch(url, init); };
    return true;
})()`

spec({
    setupSpec() {
        a = browser.newPage({user: "john.doe", timeout: 30})
        b = browser.newPage({user: "calls.client.b", password: PASSWORD, timeout: 30})
        c = browser.newPage({user: "calls.client.c", password: PASSWORD, timeout: 30})
        for (const page of [a, b, c]) {
            page.allowProblems(/userPhoto\?userId=/)
            page.goto("/qapps/BasicComms/Home")
            page.evaluate(HELPERS)
        }
        room.a = a.evaluate(() => window.moqui.webrootVue.userId)
        room.b = b.evaluate(() => window.moqui.webrootVue.userId)
        room.c = c.evaluate(() => window.moqui.webrootVue.userId)
        room.id = a.evaluate((ids) => window.__e2ee.post("createRoom", {roomTypeEnumId: "private", roomName: "Encrypted spec " + Date.now(),
            memberUserIds: ids.join(","), encrypted: "true"}).then((out) => out.chatRoomId), [room.b, room.c])
        // each device made first, so the room key reaches it
        for (const page of [a, b, c]) page.evaluate(() => window.moqui.crypto.ensureDevice().then(() => true))
    }
})

feature("an encrypted message and an encrypted image go both ways; the server and the outbox never hold the plaintext", {
    when() {
        const members = [room.a, room.b, room.c]
        // john.doe: an image encrypted and uploaded, then a message naming it, through the outbox (localStorage)
        const sent = a.evaluate(([roomId, members, secret, png]) => {
            const bytes = Uint8Array.from(atob(png), (ch) => ch.charCodeAt(0))
            return window.moqui.chat.sealFile(new Blob([bytes], {type: "image/png"})).then((sealed) => {
                const form = new FormData()
                form.append("chatRoomId", roomId)
                form.append("file", sealed.blob, "encrypted")
                if (window.moqui.sessionToken) form.append("moquiSessionToken", window.moqui.sessionToken)
                return window.moqui.request("/apps/BasicComms/Chat/upload", {method: "POST", params: form}).then((r) => {
                    const stored = r.json.uploadList[0]
                    const file = {chatAttachmentId: stored.chatAttachmentId, fileName: "dot.png", contentType: "image/png", fileSize: bytes.length, info: sealed.info}
                    return window.moqui.chat.sealMessage(roomId, members, "<p>" + secret + "</p>", [file]).then((params) => {
                        params.chatAttachmentIds = file.chatAttachmentId
                        let outboxSeen = ""
                        const save = window.moqui.chat.saveOutbox
                        window.moqui.chat.saveOutbox = (list) => { outboxSeen += JSON.stringify(list); return save(list) }
                        return window.moqui.chat.send({chatRoomId: roomId, url: "/apps/BasicComms/Chat/send", params, text: "", sealed: true}).then((out) => {
                            window.moqui.chat.saveOutbox = save
                            return {out, stored, outboxSeen}
                        })
                    })
                })
            })
        }, [room.id, members, SECRET, PNG])
        // calls.client.b: the room in Chat shows the message, decrypted, and the image from a decrypted blob
        b.goto("/qapps/BasicComms/Chat?chatRoomId=" + room.id)
        const shown = b.waitFor((secret) => document.body.innerText.indexOf(secret) >= 0, {arg: SECRET, timeout: 15})
        const image = b.waitFor(() => { const img = document.querySelector("img[src^='blob:']"); return img && img.naturalWidth > 0 ? img.naturalWidth : 0 }, {timeout: 15})
        b.evaluate(HELPERS)
        const reply = b.evaluate(([roomId, members]) => window.__e2ee.send(roomId, members, "<p>Reply from b</p>"), [room.id, members])
        const atA = a.evaluate((roomId) => window.__e2ee.page(roomId).then((list) => list.map((e) => e.bodyText)), room.id)
        const serverRow = b.evaluate((roomId) => window.__e2ee.get("timeline", {chatRoomId: roomId, limit: 50}).then((page) => JSON.stringify(page)), room.id)
        const posted = a.evaluate(() => window.__posted)
        const stored = a.evaluate(() => JSON.stringify(window.localStorage))
        return {sent, shown, image, reply, atA, serverRow, posted, stored}
    },
    then({sent, shown, image, reply, atA, serverRow, posted, stored}) {
        sent.out.sent == true
        sent.stored.fileName == "encrypted"
        sent.outboxSeen.indexOf("encryptedJson") >= 0
        sent.outboxSeen.indexOf(SECRET) < 0
        shown == true
        image == 1
        reply.eventSeq > 0
        atA.indexOf("Reply from b") >= 0
        atA.indexOf(SECRET) >= 0
        serverRow.indexOf(SECRET) < 0
        serverRow.indexOf("m.megolm.v1.aes-sha2") >= 0
        posted.length > 0
        posted.every((body) => body.indexOf(SECRET) < 0)
        stored.indexOf(SECRET) < 0
    }
})

feature("a member who leaves gets no key of the next message: it is in a new session they cannot decrypt", {
    when() {
        const before = a.evaluate(([roomId, members]) => window.__e2ee.send(roomId, members, "<p>Before c leaves</p>"), [room.id, [room.a, room.b, room.c]])
        const cRead = c.evaluate(([roomId, seq]) => window.__e2ee.page(roomId).then((list) => (list.find((e) => e.eventSeq === seq) || {}).bodyText || null),
            [room.id, before.eventSeq])
        c.evaluate((roomId) => window.__e2ee.post("leave", {chatRoomId: roomId}), room.id)
        // john.doe's page hears the leave (rotateRoom) and sends to the members now in the room
        a.evaluate((roomId) => window.moqui.crypto.rotateRoom(roomId), room.id)
        const after = a.evaluate(([roomId, members, text]) => window.__e2ee.send(roomId, members, "<p>" + text + "</p>"), [room.id, [room.a, room.b], LATER])
        const sealedAfter = a.evaluate(([roomId, seq]) => window.__e2ee.get("timeline", {chatRoomId: roomId, eventSeq: seq}).then((p) => p.eventList[0]),
            [room.id, after.eventSeq])
        const sealedBefore = a.evaluate(([roomId, seq]) => window.__e2ee.get("timeline", {chatRoomId: roomId, eventSeq: seq}).then((p) => p.eventList[0]),
            [room.id, before.eventSeq])
        // c is given the ciphertext (it cannot read the room any more) and still cannot decrypt it
        const cAfter = c.evaluate(([roomId, item]) => window.moqui.crypto.decryptRoom(roomId, item), [room.id, sealedAfter])
        const bAfter = b.evaluate(([roomId, item]) => window.moqui.crypto.decryptRoom(roomId, item), [room.id, sealedAfter])
        return {cRead, sealedBefore, sealedAfter, cAfter, bAfter}
    },
    then({cRead, sealedBefore, sealedAfter, cAfter, bAfter}) {
        cRead == "Before c leaves"
        sealedAfter.encrypted.session_id != sealedBefore.encrypted.session_id
        !!cAfter.error
        bAfter.content.body == LATER
    }
})

feature("a second tab of the same browser reads through the tab that holds the store, and takes the store when that tab closes it", {
    when() {
        const item = a.evaluate(([roomId]) => window.__e2ee.get("timeline", {chatRoomId: roomId, limit: 50})
            .then((p) => p.eventList.filter((e) => e.encrypted).pop()), [room.id])
        b.evaluate(() => { window.__secondTab = window.open("/qapps/BasicComms/Home", "secondTab"); return !!window.__secondTab })
        b.waitFor(() => { const w = window.__secondTab; return !!(w && w.moqui && w.moqui.crypto && w.moqui.webrootVue && w.moqui.webrootVue.userId) }, {timeout: 15})
        const relayed = b.evaluate(([roomId, item]) => {
            const other = window.__secondTab
            return other.moqui.crypto.decryptRoom(roomId, item).then((out) => ({role: other.moqui.crypto.state.role, body: out.content ? out.content.body : null,
                leader: window.moqui.crypto.state.role}))
        }, [room.id, item])
        // the first tab closes its store: the second takes it and reads on its own
        const taken = b.evaluate(([roomId, item]) => {
            const other = window.__secondTab
            return window.moqui.crypto.close().then(() => new Promise((resolve) => {
                const started = Date.now()
                const look = () => {
                    if (other.moqui.crypto.state.role === "leader" || Date.now() - started > 10000) resolve(other.moqui.crypto.state.role)
                    else setTimeout(look, 100)
                }
                look()
            })).then((role) => other.moqui.crypto.decryptRoom(roomId, item).then((out) => ({role, body: out.content ? out.content.body : null})))
        }, [room.id, item])
        return {item, relayed, taken}
    },
    then({item, relayed, taken}) {
        relayed.leader == "leader"
        relayed.role == "follower"
        relayed.body == LATER
        taken.role == "leader"
        taken.body == LATER
    }
})

feature("a new device of the same user reads older messages only after restoring the key backup with the recovery key", {
    when() {
        const backup = a.evaluate(() => window.moqui.crypto.enableBackup().then((out) => window.moqui.crypto.backupStatus().then((status) => ({out, status}))))
        const item = a.evaluate(([roomId]) => window.__e2ee.get("timeline", {chatRoomId: roomId, limit: 50})
            .then((p) => p.eventList.filter((e) => e.encrypted).pop()), [room.id])
        // a new browser of john.doe: a new store, so a new device
        const fresh = browser.newPage({user: "john.doe", timeout: 30})
        fresh.allowProblems(/userPhoto\?userId=/)
        fresh.goto("/qapps/BasicComms/Home")
        const device = fresh.evaluate(() => window.moqui.crypto.ensureDevice())
        const before = fresh.evaluate(([roomId, item]) => window.moqui.crypto.decryptRoom(roomId, item), [room.id, item])
        const wrong = fresh.evaluate(() => window.moqui.crypto.restoreBackup("EsTc 1jgL yjxH uy2t 4ZJG 9FmB zmbV ZgCp 7Fku nYbp sk6y 3PeW").then(() => null, (err) => err.reasonCode))
        const restored = fresh.evaluate((key) => window.moqui.crypto.restoreBackup(key), backup.out.recoveryKey)
        const after = fresh.evaluate(([roomId, item]) => window.moqui.crypto.decryptRoom(roomId, item), [room.id, item])
        const firstDevice = a.evaluate(() => window.moqui.crypto.state.deviceId)
        return {backup, device, before, wrong, restored, after, firstDevice}
    },
    then({backup, device, before, wrong, restored, after, firstDevice}) {
        backup.out.recoveryKey.split(" ").length == 12
        backup.status.enabled == true
        backup.status.backedUp == backup.status.total
        device.deviceId != firstDevice
        !!before.error
        wrong == "wrongRecoveryKey" || wrong == "badRecoveryKey"
        restored.imported > 0
        after.content.body == LATER
    }
})
