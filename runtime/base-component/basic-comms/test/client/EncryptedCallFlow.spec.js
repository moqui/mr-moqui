// An end-to-end encrypted call in three browsers (plans/Comms-Framework.md §6.4, W8; docs/interfaces/WebRTC.md "Encrypted
// calls"): john.doe starts an encrypted video call to calls.client.b and calls.client.c (CallClientFixtures) from the page's
// call store. Each page takes its crypto device (moqui.crypto), names it on its leg, and sends its SFrame key to the
// others' devices over Olm (moqui.callKeys); every frame goes through the RTCRtpScriptTransform worker. The callee answers:
// media goes both ways, decrypted with the keys each got from the other's device. The third person answers later: they get
// both keys and the others get theirs. Recording the call is refused. The third leaves: the others rotate their keys, and
// the new keys go only to the two still in it. Chromium's fake camera and microphone stand in for devices; needs a binary
// with the rtc feature (the suite's --serve server sets rtc_media_port).
// Run: ./moq test-client basic-comms --serve --spec EncryptedCallFlow
const PASSWORD = "CallsClient1!!"
let caller = null
let callee = null
let third = null

// a page's call and keys: phase, the decoded media, and what its SFrame worker did with each leg's frames
const keysOf = page =>
    page.evaluate(() => {
        const call = window.moqui.call
        const keys = window.moqui.callKeys.state
        const out = {phase: call.state.phase, legId: call.state.legId, kid: keys.kid, epoch: keys.epoch, rotations: keys.rotations,
            sent: keys.sent, dropped: keys.dropped, keys: JSON.parse(JSON.stringify(keys.keys)), frames: JSON.parse(JSON.stringify(keys.frames)),
            lastSend: JSON.parse(JSON.stringify(keys.lastSend)), error: keys.error, audio: 0, video: 0}
        if (!call.pc) return out
        return call.pc.getStats().then(stats => {
            stats.forEach(s => {
                if (s.type === "inbound-rtp" && s.kind === "audio") out.audio += s.packetsReceived || 0
                if (s.type === "inbound-rtp" && s.kind === "video") out.video += s.framesDecoded || 0
            })
            return out
        })
    })
// waits until the page decrypts frames of each of the legs (both audio and video arrive and are decoded)
const decrypting = (page, legIds) => {
    try {
        return page.waitFor(legs => {
            const call = window.moqui.call
            const frames = window.moqui.callKeys.state.frames
            if (!call.pc || call.state.phase !== "connected") return false
            if (!legs.every(leg => frames[leg] && frames[leg].received > 20)) return false
            return call.pc.getStats().then(stats => {
                let video = 0
                stats.forEach(s => { if (s.type === "inbound-rtp" && s.kind === "video") video += s.framesDecoded || 0 })
                return video > 5
            })
        }, {timeout: 12, arg: legIds})
    } catch (err) {
        return false
    }
}
const userIdOf = (page, name) => page.evaluate(label => window.moqui.request("/apps/BasicComms/Calls/searchUsers", {params: {term: label}})
    .then(result => ((result.json || []).find(user => user.label && user.label.indexOf(label) >= 0) || {}).value || null), name)

spec({
    // slow: about 61 s, its media wait timing out (test-all, 2026-11-04); run only with --slow, and a candidate for deletion
    slow: true,
    requires: ["rtc"],
    setupSpec() {
        caller = browser.newPage({user: "john.doe", timeout: 30})
        callee = browser.newPage({user: "calls.client.b", password: PASSWORD, timeout: 30})
        third = browser.newPage({user: "calls.client.c", password: PASSWORD, timeout: 30})
        for (const page of [caller, callee, third]) page.allowProblems(/userPhoto\?userId=/)
    }
})

feature("an encrypted call: media both ways under SFrame, a third joiner gets the keys, recording refused, a leave rotates the keys", {
    when() {
        callee.goto("/qapps/BasicComms/Home")
        third.goto("/qapps/BasicComms/Home")
        caller.goto("/qapps/BasicComms/Calls")
        const b = userIdOf(caller, "Calls Client B")
        const c = userIdOf(caller, "Calls Client C")
        const started = caller.evaluate(ids => window.moqui.call.startCall({calleeUserIdList: ids.join(","), video: true, requireE2ee: true})
            .then(json => ({callSessionId: json.callSessionId, e2ee: !!(json.call && json.call.e2ee), mode: json.call && json.call.encryptionMode})), [b, c])
        callee.waitFor(() => window.moqui.call.state.ringing.length > 0, {timeout: 10})
        third.waitFor(() => window.moqui.call.state.ringing.length > 0, {timeout: 10})
        callee.evaluate(id => window.moqui.call.answer(id, true), started.callSessionId)
        const aLeg = caller.evaluate(() => window.moqui.call.state.legId)
        const bLeg = callee.evaluate(() => window.moqui.call.state.legId)
        const twoWay = [decrypting(caller, [bLeg]), decrypting(callee, [aLeg])]
        const before = [keysOf(caller), keysOf(callee)]
        // the third person answers late: they get both keys, and the others get theirs
        third.evaluate(id => window.moqui.call.answer(id, true), started.callSessionId)
        const cLeg = third.evaluate(() => window.moqui.call.state.legId)
        const threeWay = [decrypting(third, [aLeg, bLeg]), decrypting(caller, [cLeg]), decrypting(callee, [cLeg])]
        // the host's Record is refused: the server cannot hear the call
        const recording = caller.evaluate(id => window.moqui.call.request("startRecording", {callSessionId: id}, "POST", true), started.callSessionId)
        const indicator = caller.locator(".m-call-e2ee").count()
        // nobody verified the others' devices here: the call warns, naming them
        let unverified = false
        try {
            unverified = caller.waitFor(() => window.moqui.callKeys.state.unverified.length >= 2 && !!document.querySelector(".m-call-unverified"), {timeout: 8})
        } catch (err) { unverified = false }
        // the third leaves: the others make new keys, sent only to the two still in the call
        const callerRotations = caller.evaluate(() => window.moqui.callKeys.state.rotations)
        const calleeRotations = callee.evaluate(() => window.moqui.callKeys.state.rotations)
        third.evaluate(() => window.moqui.call.leave())
        let rotated = false
        try {
            rotated = caller.waitFor(n => window.moqui.callKeys.state.rotations > n && window.moqui.callKeys.state.lastSend &&
                window.moqui.callKeys.state.lastSend.devices.length === 1, {timeout: 8, arg: callerRotations})
        } catch (err) { rotated = false }
        let calleeRotated = false
        try {
            calleeRotated = callee.waitFor(n => window.moqui.callKeys.state.rotations > n, {timeout: 8, arg: calleeRotations})
        } catch (err) { calleeRotated = false }
        const after = [keysOf(caller), keysOf(callee)]
        // the callee has the caller's new key
        let newKeyThere = false
        try {
            newKeyThere = callee.waitFor(([leg, kid]) => (window.moqui.callKeys.state.keys[leg] || {}).kid === kid, {timeout: 5, arg: [aLeg, after[0].lastSend.kid]})
        } catch (err) { newKeyThere = false }
        caller.evaluate(() => window.moqui.call.leave())
        return {started, aLeg, bLeg, cLeg, twoWay, before, threeWay, recording, indicator, unverified, rotated, calleeRotated, after, newKeyThere,
            thirdDevice: third.evaluate(() => window.moqui.crypto.state.deviceId)}
    },
    then({started, aLeg, bLeg, cLeg, twoWay, before, threeWay, recording, indicator, unverified, rotated, calleeRotated, after, newKeyThere, thirdDevice}) {
        started.e2ee == true
        started.mode == "e2ee-sframe"
        twoWay.every(ok => ok)
        before[0].keys[bLeg] != null
        before[1].keys[aLeg] != null
        before.every(k => k.sent > 0 && k.audio > 20 && k.video > 5)
        threeWay.every(ok => ok)
        recording.reasonCode == "encrypted"
        indicator >= 1
        unverified == true
        rotated == true
        calleeRotated == true
        after[0].lastSend.devices.every(d => d.deviceId != thirdDevice)
        after[0].epoch > before[0].epoch
        newKeyThere == true
        after.every(k => !k.error)
    }
})
