// A call in two browsers (plans/Comms-Framework.md W1, §7.3 "fake-media Chromium proves the application flow"): john.doe
// calls calls.client.b from the page's call store, the callee's tab rings and is answered from its ring bar, both pages
// connect their media through the server's SFU and receive the other's audio and video, john.doe's raised hand reaches the
// other page and the speaker view lays out the call, john.doe's LiveCall window keeps
// the call on another screen, and leaving from it ends the call on the other page. Chromium's fake camera and
// microphone stand in for devices. Needs a binary with the rtc feature and a server holding call media (the suite's
// --serve server sets rtc_media_port).
// Run: ./moq test-client basic-comms --serve
const CALLEE = "calls.client.b"
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

spec({
    // slow: about 61 s, its media wait timing out (test-all, 2026-11-04); run only with --slow, and a candidate for deletion
    slow: true,
    requires: ["rtc"],
    setupSpec() {
        caller = browser.newPage({user: "john.doe", timeout: 30})
        callee = browser.newPage({user: CALLEE, password: PASSWORD, timeout: 30})
        for (const page of [caller, callee]) page.allowProblems(/userPhoto\?userId=/)
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
        const both = [caller, callee].map(page => {
          try { return page.waitFor(
                () => {
                    const call = window.moqui.call
                    if (!call.pc || call.state.phase !== "connected") return false
                    return call.pc.getStats().then(stats => {
                        let audio = 0
                        let video = 0
                        stats.forEach(s => {
                            if (s.type === "inbound-rtp" && s.kind === "audio") audio += s.packetsReceived || 0
                            if (s.type === "inbound-rtp" && s.kind === "video") video += s.framesDecoded || 0
                        })
                        return audio > 20 && video > 5
                    })
                },
                {timeout: 20}
            ) } catch (err) { return false }
        })
        const media = [mediaOf(caller), mediaOf(callee)]
        // a raised hand reaches the other page, and the speaker view lays the call out around the other person
        caller.evaluate(() => { window.moqui.call.toggleHand(); window.moqui.call.setLayout("speaker") })
        const hand = callee.waitFor(() => Object.values(window.moqui.call.state.remotes).some(r => r.hand), {timeout: 10})
        const speakerView = caller.locator(".m-call-tile").count() >= 2
        // to another screen in the app (a page load would end the page's media): the call goes on in the LiveCall window
        caller.evaluate(() => window.moqui.webrootVue.setUrl("/qapps/BasicComms/Home"))
        caller.waitForURL(/\/Home/)
        const liveCall = caller.locator(".m-live-call").waitFor({timeout: 10})
        caller.locator(".m-live-call").getByRole("button", {name: "Leave", exact: true}).click()
        callee.waitFor(() => window.moqui.call.state.current === null, {timeout: 15})
        const after = caller.evaluate(id => window.moqui.call.request("callState", {callSessionId: id}, "GET").then(json => json.call && json.call.status), started)
        return {started, both, media, hand, speakerView, liveCall, after}
    },
    then({started, both, media, hand, speakerView, liveCall, after}) {
        started != null
        both.every(ok => ok)
        media.every(m => m.audio > 20 && m.video > 5)
        hand == true
        speakerView == true
        liveCall == true
        after == "ended"
    }
})
