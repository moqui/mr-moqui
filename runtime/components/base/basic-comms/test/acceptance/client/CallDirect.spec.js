// The direct path in two browsers (plans/Comms-Messaging.md §8.3 item 15, phase 5; docs/interfaces/WebRTC.md "The direct
// path"): john.doe calls calls.client.b one to one, and the two pages connect peer to peer, signaled with rtc peer frames
// through the call's media node, with media both ways. Ringing calls.client.c in moves the call to the media server: both
// pages join it there and the media flows again. A page that moves on its own takes the other along, and a page that
// joins again after a move goes to the media server. An encrypted call stays SFrame-encrypted on both paths. With Always
// relay my calls (Settings, Calls) set by the callee, the next call goes through the media server from the start. Both pages are in one Chromium, so their direct path is the
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

// the frames the page's SFrame worker decrypted of a leg
const framesOf = (page, leg) => page.evaluate(leg => ((window.moqui.callKeys.state.frames || {})[leg] || {}).received || 0, leg)

// until the page, connected on `path`, has decrypted 20 more of the leg's frames than `from` and decodes video
const decryptingOn = (page, path, leg, from) =>
    page.waitFor(([path, leg, from]) => {
        const call = window.moqui.call
        const frames = window.moqui.callKeys.state.frames || {}
        if (!call.pc || call.state.path !== path || call.state.phase !== "connected") return false
        if (!frames[leg] || frames[leg].received <= from + 20) return false
        return call.pc.getStats().then(stats => {
            let video = 0
            stats.forEach(s => { if (s.type === "inbound-rtp" && s.kind === "video") video += s.framesDecoded || 0 })
            return video > 5
        })
    }, {arg: [path, leg, from], timeout: 25})

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

// One page moves to the media server on its own (as on a timeout): the server marks the call relayed, the other page's
// view flips and it follows. That page then joins again knowing nothing of the move (a reload, another device): the
// call's view sends it to the media server, and it hears the other.
feature("after one page moves a direct call to the media server, a page that joins again goes there and hears the other", {
    when() {
        const id = callAndAnswer()
        const direct = [caller, callee].map(page => { try { return connectedOn(page, "direct") } catch (err) { return false } })
        caller.evaluate(() => window.moqui.call.toRelay("test"))
        const relay = [caller, callee].map(page => { try { return connectedOn(page, "relay") } catch (err) { return false } })
        // the callee's page joins again as a new page would: no memory of the move, the call's view read again
        const view = callee.evaluate(id => {
            const call = window.moqui.call
            const legId = call.state.legId
            call.hangUp("test")
            call.relayOnly = {}
            return call.request("callState", {callSessionId: id}, "GET").then(json => {
                const direct = json.call ? json.call.direct : null
                return call.join(id, legId, {video: true}).then(() => direct)
            })
        }, id)
        let again = false
        try { again = connectedOn(callee, "relay") } catch (err) { again = false }
        const after = pathOf(callee)
        caller.evaluate(() => window.moqui.call.endForAll())
        callee.waitFor(() => window.moqui.call.state.current === null, {timeout: 15})
        return {direct, relay, view, again, after}
    },
    then({direct, relay, view, again, after}) {
        direct.every(ok => ok)
        relay.every(ok => ok)
        view == false
        again == true
        after.path == "relay" && after.audio > 20
    }
})

// An end-to-end encrypted one-to-one call (§6.1 rule 4): on the direct path every frame is SFrame-decrypted with the
// other's key, and after ringing a third person moves it to the media server it is still decrypted there: never a
// plain call on either path.
feature("an encrypted one-to-one call goes direct under SFrame, and stays encrypted on the media server", {
    when() {
        callee.goto("/qapps/BasicComms/Home")
        caller.goto("/qapps/BasicComms/Calls")
        const b = caller.evaluate(label => window.moqui.request("/apps/BasicComms/Calls/searchUsers", {params: {term: label}})
            .then(result => ((result.json || []).find(user => user.label && user.label.indexOf(label) >= 0) || {}).value || null), "Calls Client B")
        const started = caller.evaluate(id => window.moqui.call.startCall({calleeUserIdList: id, video: true, requireE2ee: true})
            .then(json => ({callSessionId: json.callSessionId, e2ee: !!(json.call && json.call.e2ee), direct: !!(json.call && json.call.direct)})), b)
        callee.waitFor(() => window.moqui.call.state.ringing.length > 0, {timeout: 15})
        callee.evaluate(id => window.moqui.call.answer(id, true), started.callSessionId)
        const aLeg = caller.evaluate(() => window.moqui.call.state.legId)
        const bLeg = callee.evaluate(() => window.moqui.call.state.legId)
        const direct = [[caller, bLeg], [callee, aLeg]].map(([page, leg]) => { try { return decryptingOn(page, "direct", leg, 0) } catch (err) { return false } })
        const onDirect = [framesOf(caller, bLeg), framesOf(callee, aLeg)]
        caller.evaluate(username =>
            window.moqui.request("/apps/BasicComms/Calls/searchUsers", {params: {term: username}}).then(result => {
                const found = (result.json || []).find(user => user.label && user.label.indexOf("Calls Client C") >= 0)
                return window.moqui.call.callAction("addParticipant", {callSessionId: window.moqui.call.state.current, userId: found.value})
            }), THIRD)
        const relay = [[caller, bLeg, onDirect[0]], [callee, aLeg, onDirect[1]]].map(([page, leg, from]) => {
            try { return decryptingOn(page, "relay", leg, from) } catch (err) { return false }
        })
        const view = caller.evaluate(id => { const v = window.moqui.call.state.calls[id]; return {e2ee: !!v.e2ee, direct: !!v.direct} }, started.callSessionId)
        const errors = [caller, callee].map(page => page.evaluate(() => window.moqui.callKeys.state.error || null))
        caller.evaluate(() => window.moqui.call.endForAll())
        callee.waitFor(() => window.moqui.call.state.current === null, {timeout: 15})
        return {started, direct, relay, view, errors}
    },
    then({started, direct, relay, view, errors}) {
        started.e2ee == true && started.direct == true
        direct.every(ok => ok)
        relay.every(ok => ok)
        view.e2ee == true && view.direct == false
        errors.every(e => e == null)
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
