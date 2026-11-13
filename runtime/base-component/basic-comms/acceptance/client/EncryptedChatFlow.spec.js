// Encrypted chat rooms in browsers (plans/Comms-Framework.md §4.5, C7; docs/interfaces/Chat-Keys.md "Encrypted rooms"):
// john.doe makes an encrypted room with calls.client.b and calls.client.c (CallClientFixtures). Each page holds its own
// crypto device (moqui.crypto over matrix-sdk-crypto); messages are Megolm-encrypted in the sending page and decrypted in
// the others (moqui.chat.sealMessage, openEvents), an image goes up encrypted and comes back as a decrypted blob, and
// nothing the sender posted, nor its outbox in localStorage, holds the plaintext. When calls.client.c leaves, the next
// message is in a new Megolm session that c cannot decrypt. A second tab of the same browser (a window the page opens, which
// shares the browser's storage, Web Locks and BroadcastChannel) reads through the tab that holds the store, and takes the store
// when that one closes it. A new device of john.doe (a page of its own, so a new store) reads nothing older until it
// restores the key backup with the recovery key, and after "New recovery key" the old key still restores its older version.
// With only verified devices on (the sender's setting, or the room's), an unverified device cannot read the next message. A
// store re-made under another key is reported and kept, never deleted unasked, until "Start over". A store from before the
// derived store key (its random key stored, set here through Tools) moves to a new device with its room keys.
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
        // a page that knows the user and deviceId (localStorage) but not the store key finds no leader to ask
        const probe = b.evaluate(([roomId, item]) => new Promise((resolve) => {
            const user = window.moqui.webrootVue.userId
            const names = ["moqui-crypto:" + user + ":" + window.moqui.crypto.thisDeviceId(), "moqui-crypto:" + user + ":" + window.moqui.crypto.thisDeviceId() + ":0"]
            const channels = names.map((name) => new BroadcastChannel(name))
            const heard = []
            channels.forEach((ch) => { ch.onmessage = (e) => heard.push(e.data && e.data.op) })
            channels.forEach((ch, i) => ch.postMessage({ op: "call", id: "probe" + i, tab: "probe", name: "decryptRoom", args: [roomId, item] }))
            setTimeout(() => { channels.forEach((ch) => ch.close()); resolve(heard) }, 1000)
        }), [room.id, item])
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
        return {item, relayed, probe, taken}
    },
    then({item, relayed, probe, taken}) {
        probe == []
        relayed.leader == "leader"
        relayed.role == "follower"
        relayed.body == LATER
        taken.role == "leader"
        taken.body == LATER
    }
})

feature("only verified devices: with the sender's setting, or the room's, a device nobody verified gets no key of what follows", {
    when() {
        const members = [room.a, room.b]
        const readBy = (page, seq) => page.evaluate(([roomId, seq]) => window.__e2ee.get("timeline", {chatRoomId: roomId, eventSeq: seq})
            .then((p) => window.moqui.crypto.decryptRoom(roomId, p.eventList[0])).then((out) => (out.content ? out.content.body : null)), [room.id, seq])
        const setMine = (page, on) => page.evaluate((on) => window.moqui.request("/apps/BasicComms/Settings/Devices/storeKeySettings",
            {method: "POST", params: {verifiedDevicesOnly: on ? "true" : "false"}}).then(() => window.moqui.crypto.settingsChanged()), on)
        // b's own setting: john.doe's device is not verified for b, so it cannot read b's next message
        setMine(b, true)
        const mine = b.evaluate(([roomId, members]) => window.__e2ee.send(roomId, members, "<p>Only verified b</p>"), [room.id, members])
        const aReadsMine = readBy(a, mine.eventSeq)
        const bReadsMine = readBy(b, mine.eventSeq)
        setMine(b, false)
        const open = b.evaluate(([roomId, members]) => window.__e2ee.send(roomId, members, "<p>To all b</p>"), [room.id, members])
        const aReadsOpen = readBy(a, open.eventSeq)
        // the room's setting, by its owner john.doe: b's device is not verified for john.doe
        const roomOn = a.evaluate((roomId) => window.__e2ee.post("setVerifiedDevicesOnly", {chatRoomId: roomId, verifiedDevicesOnly: "true"})
            .then((out) => { window.moqui.crypto.settingsChanged(); return out }), room.id)
        const notMember = c.evaluate((roomId) => window.__e2ee.post("setVerifiedDevicesOnly", {chatRoomId: roomId, verifiedDevicesOnly: "false"}), room.id)
        const roomMessage = a.evaluate(([roomId, members]) => window.__e2ee.send(roomId, members, "<p>Only verified room</p>"), [room.id, members])
        const bReadsRoom = readBy(b, roomMessage.eventSeq)
        a.evaluate((roomId) => window.__e2ee.post("setVerifiedDevicesOnly", {chatRoomId: roomId, verifiedDevicesOnly: "false"})
            .then(() => window.moqui.crypto.settingsChanged()), room.id)
        return {aReadsMine, bReadsMine, aReadsOpen, roomOn, notMember, bReadsRoom}
    },
    then({aReadsMine, bReadsMine, aReadsOpen, roomOn, notMember, bReadsRoom}) {
        aReadsMine == null
        bReadsMine == "Only verified b"
        aReadsOpen == "To all b"
        roomOn.verifiedDevicesOnly == true
        notMember.reasonCode == "notFound"
        bReadsRoom == null
    }
})

feature("a store this browser cannot open is kept and reported, never deleted unasked; Start over makes a new device", {
    when() {
        const page = browser.newPage({user: "calls.client.c", password: PASSWORD, timeout: 30})
        page.allowProblems(/userPhoto\?userId=|crypto store|encryption keys/)
        page.goto("/qapps/BasicComms/Home")
        const result = page.evaluate(async () => {
            const crypto = window.moqui.crypto
            const first = await crypto.ensureDevice()
            const prefix = "moqui-crypto-" + first.userId + "-" + first.deviceId
            await crypto.close()
            // the store made again under another key: what a wrong key, or a store spoilt on disk, is to this browser
            const drop = (name) => new Promise((resolve) => { const req = indexedDB.deleteDatabase(name); req.onsuccess = req.onerror = req.onblocked = () => resolve() })
            await drop(prefix + "::matrix-sdk-crypto")
            await drop(prefix + "::matrix-sdk-crypto-meta")
            const sdk = await import("/libs/matrix-sdk-crypto/index.mjs")
            await sdk.initAsync()
            const other = await sdk.OlmMachine.initialize(new sdk.UserId(first.matrixUserId), new sdk.DeviceId(first.deviceId), prefix, "not-this-device-key")
            other.close()
            const refused = await crypto.ensureDevice().then(() => null, (err) => err)
            const kept = (await indexedDB.databases()).map((d) => d.name).filter((name) => name && name.indexOf(prefix) === 0).length
            const problem = crypto.state.problem ? crypto.state.problem.reasonCode : null
            const again = await crypto.ensureDevice().then(() => null, (err) => err && err.reasonCode)
            const started = await crypto.startOver()
            const next = await crypto.ensureDevice()
            const keptAfter = (await indexedDB.databases()).map((d) => d.name).filter((name) => name && name.indexOf(prefix) === 0).length
            return {first: first.deviceId, refused: refused && refused.reasonCode, kept, problem, again, started, next: next.deviceId, keptAfter,
                problemAfter: crypto.state.problem}
        })
        const listed = page.evaluate(() => window.moqui.request("/apps/BasicComms/Settings/Devices/devices").then((r) => (r.json.deviceList || []).map((d) => d.deviceId)))
        page.close()
        return {result, listed}
    },
    then({result, listed}) {
        result.refused == "storeUnreadable"
        result.kept > 0
        result.problem == "storeUnreadable"
        result.again == "storeUnreadable"
        result.started == true
        result.next != result.first
        // the delete is asked; a database the failed open still holds goes when that connection closes
        result.keptAfter < result.kept
        result.problemAfter == null
        listed.indexOf(result.first) < 0
        listed.indexOf(result.next) >= 0
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
        // the newest encrypted message, as its sender reads it
        const expected = a.evaluate(([roomId, item]) => window.moqui.crypto.decryptRoom(roomId, item).then((out) => out.content.body), [room.id, item])
        // "New recovery key": a new backup version; the old one keeps its keys, and the old recovery key still restores them
        const second = a.evaluate(() => window.moqui.crypto.enableBackup())
        const later = browser.newPage({user: "john.doe", timeout: 30})
        later.allowProblems(/userPhoto\?userId=/)
        later.goto("/qapps/BasicComms/Home")
        later.evaluate(() => window.moqui.crypto.ensureDevice().then(() => true))
        const older = later.evaluate((key) => window.moqui.crypto.restoreBackup(key), backup.out.recoveryKey)
        const olderRead = later.evaluate(([roomId, item]) => window.moqui.crypto.decryptRoom(roomId, item), [room.id, item])
        return {backup, device, before, wrong, restored, after, firstDevice, second, older, olderRead, expected}
    },
    then({backup, device, before, wrong, restored, after, firstDevice, second, older, olderRead, expected}) {
        second.version != backup.out.version
        older.version == backup.out.version
        older.current == false
        older.imported > 0
        olderRead.content.body == expected
        backup.out.recoveryKey.split(" ").length == 12
        backup.status.enabled == true
        backup.status.backedUp == backup.status.total
        device.deviceId != firstDevice
        !!before.error
        wrong == "wrongRecoveryKey" || wrong == "badRecoveryKey"
        restored.imported > 0
        after.content.body == expected
    }
})
