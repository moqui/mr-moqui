// Crypto devices in two browsers (plans/Comms-Framework.md §4.5, C7 core; docs/interfaces/Chat-Keys.md): john.doe and
// calls.client.b, who share a room (CryptoClientFixtures), each load matrix-sdk-crypto's OlmMachine (moqui.crypto,
// js/WebrootCrypto.js), make a device and upload its keys; john.doe's page claims a one-time key of the other's device,
// Olm-encrypts a to-device event for it and sends it; the other page is woken by the push, syncs and decrypts it, and
// knows who sent it from which device. What john.doe's page sent to the server never holds the plaintext. Then the other
// user revokes their device in Settings, My devices (its transition), and a new send from john.doe reaches no device.
// Run: ./moq test-client basic-comms --serve --spec CryptoFlow
const OTHER = "calls.client.b"
const PASSWORD = "CallsClient1!!"
const SECRET = "the-call-key-0123456789"
let sender = null
let receiver = null

spec({
    setupSpec() {
        sender = browser.newPage({user: "john.doe", timeout: 30})
        receiver = browser.newPage({user: OTHER, password: PASSWORD, timeout: 30})
        for (const page of [sender, receiver]) page.allowProblems(/userPhoto\?userId=/)
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
