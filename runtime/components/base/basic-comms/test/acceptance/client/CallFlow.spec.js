// A call in two browsers (plans/archive/Comms-Framework.md W1, §7.3 "fake-media Chromium proves the application flow"): john.doe
// calls calls.client.b from the page's call store, the callee's tab rings and is answered from its ring bar, both pages
// connect their media through the server's SFU and receive the other's audio and video, john.doe's raised hand reaches the
// other page and the speaker view lays out the call, the call shows in the side panel of a desktop's width and, off its
// screen and panel, in the floating LiveCall window, and leaving from it ends the call on the other page. Chromium's fake
// camera and microphone stand in for devices. Needs a binary with the rtc feature and a server holding call media (the
// suite's --serve server sets media-server@port).
// The call's flow (plans/Comms-Messaging.md §8.3 items 1-7, phase 4): a voice call starts with audio only, and video
// turned on in the call reaches the other side as a prompt, not a new call, and goes off again; Move to this device takes
// a call to a second page of the same user while the other side's connection goes on; call waiting holds a call to answer
// another, swaps back, and declines a third with a quick text that lands in the caller's direct room.
// Run: ./moq accept basic-comms --spec CallFlow
const CALLEE = "calls.client.b"
const THIRD = "calls.client.c"
const PASSWORD = "CallsClient1!!"
let caller = null
let callee = null

// what the page received of the call: its phase and, of each other leg, the inbound audio and video packets
const mediaOf = page =>
    page.evaluate(() => {
        const call = window.moqui && window.moqui.call
        if (!call || !call.pc) return {phase: call ? call.state.phase : "none", audio: 0, video: 0}
        return call.pc.getStats().then(stats => {
            let audio = 0
            let video = 0
            stats.forEach(s => {
                if (s.type === "inbound-rtp" && s.kind === "audio") audio += s.packetsReceived || 0
                if (s.type === "inbound-rtp" && s.kind === "video") video += s.framesDecoded || 0
            })
            return {phase: call.state.phase, audio, video, current: call.state.current}
        })
    })

// until the page is connected and receives more than `audio` audio packets and `video` decoded video frames
const receiving = (page, audio, video, timeout) => {
    try {
        return page.waitFor(([audio, video]) => {
            const call = window.moqui.call
            if (!call.pc || call.state.phase !== "connected") return false
            return call.pc.getStats().then(stats => {
                let a = 0
                let v = 0
                stats.forEach(s => {
                    if (s.type === "inbound-rtp" && s.kind === "audio") a += s.packetsReceived || 0
                    if (s.type === "inbound-rtp" && s.kind === "video") v += s.framesDecoded || 0
                })
                return a > audio && v > video
            })
        }, {arg: [audio, video], timeout: timeout || 20})
    } catch (err) {
        return false
    }
}
const userIdOf = (page, label) => page.evaluate(label => window.moqui.request("/apps/BasicComms/Calls/searchUsers", {params: {term: label}})
    .then(result => ((result.json || []).find(user => user.label && user.label.indexOf(label) >= 0) || {}).value || null), label)
// a call from the page's call store to the user with that label; its callSessionId
const callTo = (page, label, video) => page.evaluate(([id, video]) => window.moqui.call.startCall({calleeUserIdList: id, video})
    .then(json => json.callSessionId), [userIdOf(page, label), video])
const ringsFor = (page, id) => page.waitFor(id => window.moqui.call.state.ringing.includes(id), {arg: id, timeout: 15})
const alwaysRelay = (page, on) =>
    page.evaluate(on => window.moqui.request("/apps/BasicComms/Settings/Calls/saveCallPreferences",
        {method: "POST", params: {alwaysRelay: on ? "true" : "false"}}).then(result => result.json && result.json.alwaysRelay), on)
// until the page has seen each other leg for longer than the prompt's settling time (moqui.call.timing.settle)
const settled = page => page.waitFor(() => {
    const remotes = Object.values(window.moqui.call.state.remotes)
    return remotes.length > 0 && remotes.every(r => r.firstSeen && Date.now() - r.firstSeen > window.moqui.call.timing.settle + 100)
}, {timeout: 15})
const hangUp = page => page.evaluate(() => (window.moqui.call.state.current ? window.moqui.call.leave() : null))

spec({
    requires: ["rtc"],
    stepwise: true,
    setupSpec() {
        caller = browser.newPage({user: "john.doe", timeout: 30})
        callee = browser.newPage({user: CALLEE, password: PASSWORD, timeout: 30})
        for (const page of [caller, callee]) page.allowProblems(/userPhoto\?userId=/)
    },
    cleanupSpec() {
        if (caller) alwaysRelay(caller, false)
        if (callee) alwaysRelay(callee, false)
    }
})

feature("a video call rings, is answered from the ring bar, carries media both ways, and leaving ends it", {
    when() {
        callee.goto("/qapps/BasicComms/Home")
        caller.goto("/qapps/BasicComms/Calls")
        const started = caller.evaluate(username => {
            return window.moqui.request("/apps/BasicComms/Calls/searchUsers", {params: {term: username}}).then(result => {
                const found = (result.json || []).find(user => user.label && user.label.indexOf("Calls Client B") >= 0) || (result.json || [])[0]
                return window.moqui.call.startCall({calleeUserIdList: found.value, video: true}).then(json => json.callSessionId)
            })
        }, CALLEE)
        callee.waitFor(() => window.moqui.call.state.ringing.length > 0, {timeout: 15})
        callee.getByRole("button", {name: "Answer", exact: true}).click()
        const both = [caller, callee].map(page => receiving(page, 20, 5))
        const media = [mediaOf(caller), mediaOf(callee)]
        // a desktop's width: the call shows in the side panel beside the screen, in its compact layout
        const panel = caller.locator(".m-call-panel .m-call-compact").count()
        // a raised hand reaches the other page, and the speaker view lays the call out around the other person on its
        // full-size screen
        caller.evaluate(id => { window.moqui.call.toggleHand(); window.moqui.call.setLayout("speaker"); window.moqui.call.open(id) }, started)
        caller.waitForURL(/\/Calls\/Call/)
        const hand = callee.waitFor(() => Object.values(window.moqui.call.state.remotes).some(r => r.hand), {timeout: 10})
        const speakerView = caller.waitFor(() => document.querySelectorAll(".m-call-tile").length >= 2, {timeout: 10})
        // to another screen in the app (a page load would end the page's media): the call goes on in the LiveCall window
        caller.evaluate(() => window.moqui.webrootVue.setUrl("/qapps/BasicComms/Home"))
        caller.waitForURL(/\/Home/)
        const liveCall = caller.locator(".m-live-call").waitFor({timeout: 10})
        caller.locator(".m-live-call").getByRole("button", {name: "Leave", exact: true}).click()
        callee.waitFor(() => window.moqui.call.state.current === null, {timeout: 15})
        const after = caller.evaluate(id => window.moqui.call.request("callState", {callSessionId: id}, "GET").then(json => json.call && json.call.status), started)
        return {started, both, media, panel, hand, speakerView, liveCall, after}
    },
    then({started, both, media, panel, hand, speakerView, liveCall, after}) {
        started != null
        both.every(ok => ok)
        media.every(m => m.audio > 20 && m.video > 5)
        panel == 1
        hand == true
        speakerView == true
        liveCall == true
        after == "ended"
    }
})

feature("a voice call starts with audio only; video turned on in it reaches the other side as a prompt, not a new call, and goes off again", {
    when() {
        const id = callTo(caller, "Calls Client B", false)
        ringsFor(callee, id)
        callee.evaluate(id => window.moqui.call.answer(id, false), id)
        const voice = [caller, callee].map(page => receiving(page, 20, -1))
        const before = [caller, callee].map(page => page.evaluate(() => ({sends: window.moqui.call.hasVideo(),
            avatar: !!document.querySelector(".m-call-voice")})))
        // the caller turns on video from the call's controls: the same call, a new track
        // past the other leg's first seconds, whose tracks are its start and not a change to ask about
        settled(callee)
        caller.locator(".m-call-video-toggle").click()
        let prompt = false
        try {
            prompt = callee.waitFor(() => !!window.moqui.call.state.videoPrompt && window.moqui.call.state.videoPrompt.on &&
                !!document.querySelector(".m-call-video-prompt"), {timeout: 15})
        } catch (err) { prompt = false }
        const calleeSees = receiving(callee, 0, 5)
        const sameCall = callee.evaluate(() => window.moqui.call.state.current)
        // the callee takes the prompt: video both ways
        callee.getByRole("button", {name: "Turn on my video"}).click()
        const callerSees = receiving(caller, 0, 5)
        // and back to voice: the other side is asked whether to turn theirs off too
        caller.evaluate(() => window.moqui.call.dropVideo())
        let offPrompt = false
        try {
            offPrompt = callee.waitFor(() => !!window.moqui.call.state.videoPrompt && !window.moqui.call.state.videoPrompt.on, {timeout: 15})
        } catch (err) { offPrompt = false }
        const callerVoice = caller.evaluate(() => window.moqui.call.hasVideo())
        hangUp(caller)
        callee.waitFor(() => window.moqui.call.state.current === null, {timeout: 15})
        return {id, voice, before, prompt, calleeSees, sameCall, callerSees, offPrompt, callerVoice}
    },
    then({id, voice, before, prompt, calleeSees, sameCall, callerSees, offPrompt, callerVoice}) {
        id != null
        voice.every(ok => ok)
        before.every(b => b.sends == false && b.avatar == true)
        prompt == true
        calleeSees == true
        sameCall == id
        callerSees == true
        offPrompt == true
        callerVoice == false
    }
})

feature("Move to this device: a second page of the caller takes the call, the first leaves once it is connected, and the other side's connection goes on", {
    when() {
        // through the media server: the direct path pairs two pages, and a move there first takes the call to the server
        alwaysRelay(caller, true)
        const id = callTo(caller, "Calls Client B", false)
        ringsFor(callee, id)
        callee.evaluate(id => window.moqui.call.answer(id, false), id)
        const connected = [caller, callee].map(page => receiving(page, 20, -1))
        const firstLeg = caller.evaluate(() => window.moqui.call.state.legId)
        // the other side's connection and phase, watched through the move
        callee.evaluate(() => {
            window.__watch = {pc: window.moqui.call.pc, phases: []}
            window.__watchTimer = setInterval(() => window.__watch.phases.push(window.moqui.call.state.phase), 100)
        })
        const second = browser.newPage({user: "john.doe", timeout: 30})
        second.allowProblems(/userPhoto\?userId=/)
        second.goto("/qapps/BasicComms/Home")
        second.locator(".m-call-elsewhere").waitFor({timeout: 15})
        second.getByRole("button", {name: "Move to this device"}).click()
        const moved = receiving(second, 20, -1)
        const secondLeg = second.evaluate(() => window.moqui.call.state.legId)
        let firstLeft = false
        try {
            firstLeft = caller.waitFor(id => window.moqui.call.state.current === null && !!window.moqui.call.state.movedAway &&
                window.moqui.call.state.movedAway.callSessionId === id, {arg: id, timeout: 15})
        } catch (err) { firstLeft = false }
        let hearsNew = false
        try {
            hearsNew = callee.waitFor(leg => {
                const r = window.moqui.call.state.remotes[leg]
                return !!(r && r.stream && r.stream.getAudioTracks().length)
            }, {arg: secondLeg, timeout: 15})
        } catch (err) { hearsNew = false }
        const watch = callee.evaluate(() => {
            clearInterval(window.__watchTimer)
            return {samePc: window.__watch.pc === window.moqui.call.pc, phases: Array.from(new Set(window.__watch.phases))}
        })
        const legs = callee.evaluate(id => window.moqui.call.request("callState", {callSessionId: id}, "GET").then(json =>
            (json.call.participants || []).flatMap(p => p.legs || []).map(l => ({callLegId: l.callLegId, status: l.status}))), id)
        const banner = caller.locator(".m-call-elsewhere").count()
        hangUp(second)
        callee.waitFor(() => window.moqui.call.state.current === null, {timeout: 15})
        alwaysRelay(caller, false)
        return {connected, firstLeg, moved, secondLeg, firstLeft, hearsNew, watch, legs, banner}
    },
    then({connected, firstLeg, moved, secondLeg, firstLeft, hearsNew, watch, legs, banner}) {
        connected.every(ok => ok)
        moved == true
        secondLeg != null && secondLeg != firstLeg
        firstLeft == true
        hearsNew == true
        watch.samePc == true
        watch.phases.every(p => p == "connected")
        legs.find(l => l.callLegId == firstLeg).status == "left"
        legs.find(l => l.callLegId == secondLeg).status == "answered"
        banner >= 1
    }
})

feature("call waiting: hold and answer a second call, swap back, and decline a third with a quick text to the caller's room", {
    when() {
        alwaysRelay(callee, true)
        const third = browser.newPage({user: THIRD, password: PASSWORD, timeout: 30})
        third.allowProblems(/userPhoto\?userId=/)
        third.goto("/qapps/BasicComms/Home")
        const first = callTo(caller, "Calls Client B", false)
        ringsFor(callee, first)
        callee.evaluate(id => window.moqui.call.answer(id, false), first)
        const firstUp = receiving(callee, 20, -1)
        // a second call while in the first: the ring bar offers hold and answer
        const second = callTo(third, "Calls Client B", false)
        ringsFor(callee, second)
        const waitingShown = callee.locator(".m-call-waiting").count()
        callee.getByRole("button", {name: "Hold and answer"}).click()
        let onSecond = false
        try {
            onSecond = callee.waitFor(id => window.moqui.call.state.current === id && window.moqui.call.state.phase === "connected", {arg: second, timeout: 20})
        } catch (err) { onSecond = false }
        const secondUp = receiving(third, 20, -1)
        let callerSeesHold = false
        try {
            callerSeesHold = caller.waitFor(() => !!document.querySelector(".m-call-held-by"), {timeout: 10})
        } catch (err) { callerSeesHold = false }
        const held = callee.evaluate(() => window.moqui.call.state.held && window.moqui.call.state.held.callSessionId)
        // back to the first: the second goes on hold in its place
        callee.locator(".m-call-held").getByRole("button", {name: "Swap"}).click()
        let backOnFirst = false
        try {
            backOnFirst = callee.waitFor(id => window.moqui.call.state.current === id && window.moqui.call.state.phase === "connected", {arg: first, timeout: 20})
        } catch (err) { backOnFirst = false }
        const firstAgain = receiving(callee, 20, -1)
        let thirdSeesHold = false
        try {
            thirdSeesHold = third.waitFor(() => !!document.querySelector(".m-call-held-by"), {timeout: 10})
        } catch (err) { thirdSeesHold = false }
        callee.evaluate(() => window.moqui.call.leaveHeld())
        third.waitFor(() => window.moqui.call.state.current === null, {timeout: 15})
        hangUp(callee)
        caller.waitFor(() => window.moqui.call.state.current === null, {timeout: 15})
        // a third call declined with a quick text: the message is in the caller's direct room with the callee
        const again = third.evaluate(id => window.moqui.call.startCall({calleeUserIdList: id, video: false})
            .then(json => ({callSessionId: json.callSessionId, chatRoomId: json.call && json.call.chatRoomId})), userIdOf(third, "Calls Client B"))
        ringsFor(callee, again.callSessionId)
        callee.locator(".m-call-ring .m-call-quick").click()
        callee.getByText("Can't talk now. I'll call you back.", {exact: true}).click()
        let texted = false
        try {
            texted = third.waitFor(roomId => window.moqui.request("/apps/BasicComms/Chat/roomView", {params: {chatRoomId: roomId}})
                .then(result => JSON.stringify(result.json || {}).includes("talk now")), {arg: again.chatRoomId, timeout: 15})
        } catch (err) { texted = false }
        const declined = third.evaluate(id => window.moqui.call.request("callState", {callSessionId: id}, "GET").then(json => json.call && json.call.endReason), again.callSessionId)
        alwaysRelay(callee, false)
        return {firstUp, waitingShown, onSecond, secondUp, callerSeesHold, held, backOnFirst, firstAgain, thirdSeesHold, again, texted, declined, first}
    },
    then({firstUp, waitingShown, onSecond, secondUp, callerSeesHold, held, backOnFirst, firstAgain, thirdSeesHold, again, texted, declined, first}) {
        firstUp == true
        waitingShown >= 1
        onSecond == true
        secondUp == true
        callerSeesHold == true
        held == first
        backOnFirst == true
        firstAgain == true
        thirdSeesHold == true
        again.chatRoomId != null
        texted == true
        declined == "declined"
    }
})
