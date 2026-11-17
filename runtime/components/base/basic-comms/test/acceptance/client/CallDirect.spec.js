// The direct path in two browsers (plans/Comms-Messaging.md §8.3 item 15, phase 5; docs/interfaces/WebRTC.md "The direct
// path"): john.doe calls calls.client.b one to one, and the two pages connect peer to peer, signaled with rtc peer frames
// through the call's media node, with media both ways. Ringing calls.client.c in moves the call to the media server: both
// pages join it there and the media flows again. With Always relay my calls (Settings, Calls) set by the callee, the next
// call goes through the media server from the start. Both pages are in one Chromium, so their direct path is the
// machine's own: this checks the flow, not a network (the network profiles are CallNetwork's). Needs a binary with the rtc
// feature (the suite's --serve server sets rtc_media_port).
// Run: ./moq accept basic-comms --spec CallDirect
const CALLEE = "calls.client.b"
const THIRD = "calls.client.c"
const PASSWORD = "CallsClient1!!"
let caller = null
let callee = null

// a page's media path, phase and what it received of the others
const pathOf = page =>
    page.evaluate(() => {
        const call = window.moqui.call
        const out = {path: call.state.path, phase: call.state.phase, audio: 0, video: 0}
        if (!call.pc) return out
        return call.pc.getStats().then(stats => {
            stats.forEach(s => {
                if (s.type === "inbound-rtp" && s.kind === "audio") out.audio += s.packetsReceived || 0
                if (s.type === "inbound-rtp" && s.kind === "video") out.video += s.framesDecoded || 0
            })
            return out
        })
    })

// until the page is connected on `path` with audio and video coming in
const connectedOn = (page, path) =>
    page.waitFor(path => {
        const call = window.moqui.call
        if (!call.pc || call.state.path !== path || call.state.phase !== "connected") return false
        return call.pc.getStats().then(stats => {
            let audio = 0
            let video = 0
            stats.forEach(s => {
                if (s.type === "inbound-rtp" && s.kind === "audio") audio += s.packetsReceived || 0
                if (s.type === "inbound-rtp" && s.kind === "video") video += s.framesDecoded || 0
            })
            return audio > 20 && video > 5
        })
    }, {arg: path, timeout: 25})

// john.doe calls the callee from the page's call store; the callee answers from its ring bar
function callAndAnswer() {
    const id = caller.evaluate(username =>
        window.moqui.request("/apps/BasicComms/Calls/searchUsers", {params: {term: username}}).then(result => {
            const found = (result.json || []).find(user => user.label && user.label.indexOf("Calls Client B") >= 0) || (result.json || [])[0]
            return window.moqui.call.startCall({calleeUserIdList: found.value, video: true}).then(json => json.callSessionId)
        }), CALLEE)
    callee.waitFor(() => window.moqui.call.state.ringing.length > 0, {timeout: 15})
    callee.getByRole("button", {name: "Answer", exact: true}).click()
    return id
}

const alwaysRelay = (page, on) =>
    page.evaluate(on => window.moqui.request("/apps/BasicComms/Settings/Calls/saveCallPreferences",
        {method: "POST", params: {alwaysRelay: on ? "true" : "false"}}).then(result => result.json && result.json.alwaysRelay), on)

spec({
    requires: ["rtc"],
    stepwise: true,
    setupSpec() {
        caller = browser.newPage({user: "john.doe", timeout: 30})
        callee = browser.newPage({user: CALLEE, password: PASSWORD, timeout: 30})
        for (const page of [caller, callee]) page.allowProblems(/userPhoto\?userId=/)
        callee.goto("/qapps/BasicComms/Home")
        caller.goto("/qapps/BasicComms/Calls")
        alwaysRelay(callee, false)
    },
    cleanupSpec() {
        if (callee) alwaysRelay(callee, false)
    }
})

feature("a one-to-one call goes peer to peer, and ringing a third person moves it to the media server", {
    when() {
        const id = callAndAnswer()
        const direct = [caller, callee].map(page => { try { return connectedOn(page, "direct") } catch (err) { return false } })
        const onDirect = [pathOf(caller), pathOf(callee)]
        // a raised hand still reaches the other page: the media node relays it to a direct leg too
        caller.evaluate(() => window.moqui.call.toggleHand())
        const hand = callee.waitFor(() => Object.values(window.moqui.call.state.remotes).some(r => r.hand), {timeout: 10})
        const third = caller.evaluate(username =>
            window.moqui.request("/apps/BasicComms/Calls/searchUsers", {params: {term: username}}).then(result => {
                const found = (result.json || []).find(user => user.label && user.label.indexOf("Calls Client C") >= 0)
                return window.moqui.call.callAction("addParticipant", {callSessionId: window.moqui.call.state.current, userId: found.value})
                    .then(json => json.reasonCode || "added")
            }), THIRD)
        const relay = [caller, callee].map(page => { try { return connectedOn(page, "relay") } catch (err) { return false } })
        const onRelay = [pathOf(caller), pathOf(callee)]
        caller.evaluate(() => window.moqui.call.endForAll())
        callee.waitFor(() => window.moqui.call.state.current === null, {timeout: 15})
        return {id, direct, onDirect, hand, third, relay, onRelay}
    },
    then({id, direct, onDirect, hand, third, relay, onRelay}) {
        id != null
        direct.every(ok => ok)
        onDirect.every(p => p.path == "direct" && p.audio > 20 && p.video > 5)
        hand == true
        third == "added"
        relay.every(ok => ok)
        onRelay.every(p => p.path == "relay" && p.audio > 20 && p.video > 5)
    }
})

feature("with Always relay my calls the callee's next call goes through the media server from the start", {
    when() {
        const set = alwaysRelay(callee, true)
        callee.goto("/qapps/BasicComms/Home")
        caller.goto("/qapps/BasicComms/Calls")
        const id = callAndAnswer()
        const relay = [caller, callee].map(page => { try { return connectedOn(page, "relay") } catch (err) { return false } })
        const paths = [pathOf(caller), pathOf(callee)]
        const view = caller.evaluate(id => window.moqui.call.state.calls[id] && window.moqui.call.state.calls[id].direct, id)
        caller.evaluate(() => window.moqui.call.endForAll())
        callee.waitFor(() => window.moqui.call.state.current === null, {timeout: 15})
        const unset = alwaysRelay(callee, false)
        return {set, relay, paths, view, unset}
    },
    then({set, relay, paths, view, unset}) {
        set == true
        relay.every(ok => ok)
        paths.every(p => p.path == "relay")
        view == false
        unset == false
    }
})
