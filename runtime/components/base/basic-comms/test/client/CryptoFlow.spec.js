// Crypto devices in two browsers (plans/archive/Comms-Framework.md §4.5, C7 core; docs/interfaces/Chat-Keys.md): john.doe and
// calls.client.b, who share a room (CryptoClientFixtures), each load matrix-sdk-crypto's OlmMachine (moqui.crypto,
// js/WebrootCrypto.js), make a device and upload its keys; john.doe's page claims a one-time key of the other's device,
// Olm-encrypts a to-device event for it and sends it; the other page is woken by the push, syncs and decrypts it, and
// knows who sent it from which device. What john.doe's page sent to the server never holds the plaintext. Then the other
// user revokes their device in Settings, My devices (its transition), and a new send from john.doe reaches no device.
// Before that, two devices of calls.client.b verify each other by emoji: the device that did not start the comparison
// accepts it, so both show the same seven emoji, and once both confirm each is verified (COM-UI-012). The first device has a
// key backup, so the second needs setting up (Comms-Messaging §6.5); once verified it asks the first for the secrets it lacks
// and restores the backup with the backup key the first sends it, with no recovery key.
// Run: ./moq test-client basic-comms --serve --spec CryptoFlow
const OTHER = "calls.client.b"
const PASSWORD = "CallsClient1!!"
const SECRET = "the-call-key-0123456789"
let sender = null
let receiver = null
let second = null

spec({
    setupSpec() {
        sender = browser.newPage({user: "john.doe", timeout: 30})
        receiver = browser.newPage({user: OTHER, password: PASSWORD, timeout: 30})
        // another browser of the same user: a device of its own
        second = browser.newPage({user: OTHER, password: PASSWORD, timeout: 30})
        for (const page of [sender, receiver, second]) page.allowProblems(/userPhoto\?userId=/)
    }
})

feature("two devices of one user verify each other by emoji: the same seven emoji on both, then both verified; the new one gets the backup key", {
    when() {
        receiver.goto("/qapps/BasicComms/Home")
        second.goto("/qapps/BasicComms/Home")
        const first = receiver.evaluate(() => window.moqui.crypto.ensureDevice())
        const other = second.evaluate(() => window.moqui.crypto.ensureDevice())
        // the first device's key backup (its recovery key is not used below): the second has something to be set up from
        const backup = receiver.evaluate(() => window.moqui.crypto.enableBackup().then((out) => out.version))
        const setupBefore = second.evaluate(() => window.moqui.crypto.setupStatus().then((status) => JSON.parse(JSON.stringify(status))))
        // the first device asks once it knows the second (its own user's device list, by a sync)
        const flowId = receiver.waitFor(async (d) => {
            await window.moqui.crypto.sync()
            try { return await window.moqui.crypto.verifyDevice(d.userId, d.deviceId) } catch (err) { return null }
        }, {arg: other, timeout: 15})
        const shown = (page, check) => page.waitFor(([id, test]) => {
            const v = (window.moqui.crypto.state.verifications || []).find((item) => item.flowId === id)
            const checks = {requested: (x) => x.phase === "requested", ready: (x) => x.ready, emoji: (x) => x.emojis && x.emojis.length === 7, done: (x) => x.done}
            // a copy: the list is reactive, and a proxy comes back from the page as an empty object
            return v && checks[test](v) ? JSON.parse(JSON.stringify(v)) : null
        }, {arg: [flowId, check], timeout: 15})
        shown(second, "requested")
        second.evaluate((id) => window.moqui.crypto.verification(id, "accept").then(() => true), flowId)
        shown(receiver, "ready")
        receiver.evaluate((id) => window.moqui.crypto.verification(id, "sas").then(() => true), flowId)
        // the regression: the second device accepts the comparison it did not start, so the emoji come on both
        const askerEmoji = shown(receiver, "emoji").emojis.map((e) => e.symbol)
        const askedEmoji = shown(second, "emoji").emojis.map((e) => e.symbol)
        for (const page of [receiver, second]) page.evaluate((id) => window.moqui.crypto.verification(id, "confirm").then(() => true), flowId)
        const askerDone = shown(receiver, "done")
        const askedDone = shown(second, "done")
        const trusted = receiver.waitFor(async (d) => {
            const list = await window.moqui.crypto.devicesOf(d.userId)
            return (list.find((device) => device.deviceId === d.deviceId) || {}).verified === true
        }, {arg: other, timeout: 15})
        // the second device asked the first for its secrets once verified: the backup key comes, opens the current backup, and
        // the second holds it (each page syncs in turn: a push lost between them does not stall this)
        let setupAfter = null
        for (let round = 0; round < 6 && !setupAfter; round++) {
            receiver.evaluate(() => window.moqui.crypto.sync().then(() => true))
            try {
                setupAfter = second.waitFor(async () => {
                    await window.moqui.crypto.sync()
                    const status = await window.moqui.crypto.setupStatus()
                    return status.restored ? JSON.parse(JSON.stringify(status)) : null
                }, {timeout: 3})
            } catch (err) { setupAfter = null }
        }
        // the second device goes, so the exchange below sends to one device of this user
        const revoked = second.evaluate((deviceId) => window.moqui.request("/apps/BasicComms/Settings/Devices/revoke",
            {method: "POST", params: {deviceId}}).then((result) => result.json || {}), other.deviceId)
        return {first, other, backup, setupBefore, setupAfter, flowId, askerEmoji, askedEmoji, askerDone, askedDone, trusted, revoked}
    },
    then({first, other, backup, setupBefore, setupAfter, flowId, askerEmoji, askedEmoji, askerDone, askedDone, trusted, revoked}) {
        first.deviceId != other.deviceId
        flowId.length > 0
        askerEmoji.length == 7
        askerEmoji.join(" ") == askedEmoji.join(" ")
        askerDone.done == true && askedDone.done == true
        trusted == true
        backup.length > 0
        setupBefore.backup == backup && setupBefore.restored == false && setupBefore.verified == false
        setupBefore.otherDevices.some((device) => device.deviceId == first.deviceId)
        setupAfter != null
        setupAfter.restored == true && setupAfter.verified == true && setupAfter.backup == backup
        !revoked.errors
    }
})

feature("two browsers exchange an Olm-encrypted to-device message; the server sees only ciphertext; a revoked device gets nothing", {
    when() {
        receiver.goto("/qapps/BasicComms/Home")
        sender.goto("/qapps/BasicComms/Home")
        // the receiver's device first, with a listener for what it decrypts
        const receiverDevice = receiver.evaluate(() => {
            window.__received = []
            window.moqui.crypto.onToDevice((event) => window.__received.push(event))
            return window.moqui.crypto.ensureDevice()
        })
        // the open store holds its Web Lock, so another tab of this browser and user cannot open it at the same time
        const lockHeld = receiver.evaluate((d) => navigator.locks.request("moqui-crypto-store:moqui-crypto-" + d.userId + "-" + d.deviceId,
            {ifAvailable: true}, (lock) => !lock), receiverDevice)
        // the sender records every body its page posts to /ChatKeys, to look for the plaintext in them
        const sent = sender.evaluate(([userId, secret]) => {
            window.__posted = []
            const fetch = window.fetch
            window.fetch = (url, init) => {
                if (String(url).indexOf("/ChatKeys/") >= 0 && init && init.body) window.__posted.push(String(init.body))
                return fetch(url, init)
            }
            return window.moqui.crypto.ensureDevice().then((me) =>
                window.moqui.crypto.sendToDevices([userId], "io.moqui.test.key", {key: secret}).then((out) => ({me, out})))
        }, [receiverDevice.userId, SECRET])
        let woke = false
        try { woke = receiver.waitFor(() => window.__received.length > 0, {timeout: 10}) } catch (err) { woke = false }
        const received = receiver.evaluate(() => window.__received[0] || null)
        const posted = sender.evaluate(() => window.__posted)
        // the receiver revokes its device in Settings, My devices; the sender's next send reaches no device
        const revoked = receiver.evaluate((deviceId) => window.moqui.request("/apps/BasicComms/Settings/Devices/revoke",
            {method: "POST", params: {deviceId}}).then((result) => result.json || {}), receiverDevice.deviceId)
        const after = sender.evaluate((userId) => window.moqui.crypto.sendToDevices([userId], "io.moqui.test.key", {key: "later"}),
            receiverDevice.userId)
        return {receiverDevice, lockHeld, sent, woke, received, posted, revoked, after}
    },
    then({receiverDevice, lockHeld, sent, woke, received, posted, revoked, after}) {
        lockHeld == true
        receiverDevice.deviceId.length == 10
        sent.me.deviceId.length == 10
        sent.out.sent == 1
        sent.out.devices[0].deviceId == receiverDevice.deviceId
        woke == true
        received.type == "io.moqui.test.key"
        received.content.key == SECRET
        received.senderUserId == sent.me.userId
        received.senderDeviceId == sent.me.deviceId
        posted.some((body) => body.indexOf("m.room.encrypted") >= 0)
        posted.every((body) => body.indexOf(SECRET) < 0)
        !revoked.errors
        after.sent == 0
    }
})
