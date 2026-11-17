// The calls' browser suite on shaped networks (plans/Comms-Messaging.md §8.4, phase 5): john.doe and calls.client.b in a
// video call, the profiles of §8.4 applied with tc netem in Linux network namespaces, each on both media paths: through
// the media server (relay: the callee keeps Always relay my calls on) and peer to peer (direct). Each row checks its §8.4
// pass line from the pages' getStats: audio concealed (lost and played as filled in), the time to connect or to come
// back, video freezes, and the path taken.
//
// It needs root or CAP_NET_ADMIN, so it runs only under scripts/call-network.sh, which makes the namespaces, starts the
// server and the shaper (scripts/call-network-control.py) and runs this spec in the client namespace:
//   sudo scripts/call-network.sh [--debug]
// Anywhere else (./moq accept basic-comms) it skips, saying so. Both pages are in one Chromium, behind the one shaped
// link: the relay path is shaped on each page's link to the media server, the direct path once each way between the pages.
// Run: sudo scripts/call-network.sh --spec CallNetwork
const CALLEE = "calls.client.b"
const PASSWORD = "CallsClient1!!"
let caller = null
let callee = null
let control = null

// a page's media as getStats has it: audio received and concealed, video decoded and frozen, the path's candidates
const statsOf = page =>
    page.evaluate(() => {
        const call = window.moqui.call
        const out = {path: call.state.path, phase: call.state.phase, received: 0, samples: 0, concealed: 0, frames: 0, freezes: 0,
            frozen: 0, height: 0, local: null, protocol: null, limited: null}
        if (!call.pc) return out
        return call.pc.getStats().then(stats => {
            const byId = {}
            stats.forEach(s => { byId[s.id] = s })
            stats.forEach(s => {
                if (s.type === "inbound-rtp" && s.kind === "audio") {
                    out.received += s.packetsReceived || 0
                    out.samples += s.totalSamplesReceived || 0
                    out.concealed += s.concealedSamples || 0
                }
                if (s.type === "inbound-rtp" && s.kind === "video") {
                    out.frames += s.framesDecoded || 0
                    out.freezes += s.freezeCount || 0
                    out.frozen += s.totalFreezesDuration || 0
                    out.height = Math.max(out.height, s.frameHeight || 0)
                }
                if (s.type === "outbound-rtp" && s.kind === "video" && s.qualityLimitationReason) out.limited = s.qualityLimitationReason
                if (s.type === "candidate-pair" && s.nominated && s.state === "succeeded") {
                    const local = byId[s.localCandidateId]
                    if (local) { out.local = local.candidateType; out.protocol = local.protocol }
                }
            })
            return out
        })
    })

// what changed between two samples of a page: the share of audio concealed, the frames and the freezes
function delta(a, b) {
    const samples = b.samples - a.samples
    return {concealed: samples > 0 ? (b.concealed - a.concealed) / samples : 1, frames: b.frames - a.frames,
        freezes: b.freezes - a.freezes, frozen: b.frozen - a.frozen, received: b.received - a.received, height: b.height,
        path: b.path, local: b.local, protocol: b.protocol}
}

const shape = name => {
    const res = control.post(params.netemControl + "/profile", {json: {name}, csrf: false})
    if (!res.ok) throw new Error("the shaper did not take " + name + ": " + res.text)
}

const alwaysRelay = (page, on) =>
    page.evaluate(on => window.moqui.request("/apps/BasicComms/Settings/Calls/saveCallPreferences",
        {method: "POST", params: {alwaysRelay: on ? "true" : "false"}}).then(result => result.json && result.json.alwaysRelay), on)

// john.doe calls the callee; the seconds until both pages are connected with audio coming in
function callAndConnect() {
    callee.goto("/qapps/BasicComms/Home")
    caller.goto("/qapps/BasicComms/Calls")
    const start = Date.now()
    caller.evaluate(username =>
        window.moqui.request("/apps/BasicComms/Calls/searchUsers", {params: {term: username}}).then(result => {
            const found = (result.json || []).find(user => user.label && user.label.indexOf("Calls Client B") >= 0) || (result.json || [])[0]
            return window.moqui.call.startCall({calleeUserIdList: found.value, video: true}).then(json => json.callSessionId)
        }), CALLEE)
    callee.waitFor(() => window.moqui.call.state.ringing.length > 0, {timeout: 20})
    const answered = Date.now()
    callee.getByRole("button", {name: "Answer", exact: true}).click()
    for (const page of [caller, callee]) {
        page.waitFor(() => window.moqui.call.state.phase === "connected" && window.moqui.call.pc, {timeout: 45})
        page.waitFor(() => window.moqui.call.pc.getStats().then(stats => {
            let audio = 0
            stats.forEach(s => { if (s.type === "inbound-rtp" && s.kind === "audio") audio += s.packetsReceived || 0 })
            return audio > 5
        }), {timeout: 45})
    }
    return {ringSeconds: (answered - start) / 1000, connectSeconds: (Date.now() - answered) / 1000}
}

function hangUp() {
    try { caller.evaluate(() => window.moqui.call.endForAll()) } catch (err) { /* the page is gone */ }
    try { callee.waitFor(() => window.moqui.call.state.current === null, {timeout: 20}) } catch (err) { /* ended already */ }
}

// how long after the network comes back until the page hears the other again
function backAfter(page) {
    const before = statsOf(page).received
    const start = Date.now()
    page.waitFor(before => window.moqui.call.pc && window.moqui.call.pc.getStats().then(stats => {
        let audio = 0
        stats.forEach(s => { if (s.type === "inbound-rtp" && s.kind === "audio") audio += s.packetsReceived || 0 })
        return audio > before + 5
    }), {arg: before, timeout: 70})
    return (Date.now() - start) / 1000
}

spec({
    requires: ["rtc"],
    timeout: 3600,
    setupSpec() {
        if (!params.netemControl) {
            skip("needs root or CAP_NET_ADMIN for network namespaces and tc netem: run it with sudo scripts/call-network.sh " +
                "(docs/interfaces/WebRTC.md \"The network tests\")")
        }
        control = session()
        const listed = control.get(params.netemControl + "/profile")
        if (!listed.ok) skip("the shaper at " + params.netemControl + " does not answer: run this spec with scripts/call-network.sh")
        caller = browser.newPage({user: "john.doe", timeout: 60})
        callee = browser.newPage({user: CALLEE, password: PASSWORD, timeout: 60})
        for (const page of [caller, callee]) page.allowProblems(/userPhoto\?userId=|WebSocket|Failed to fetch|NetworkError/)
        callee.goto("/qapps/BasicComms/Home")
    },
    cleanup() {
        if (control) shape("clean")
        if (caller) hangUp()
    },
    cleanupSpec() {
        if (control) shape("clean")
        if (callee) alwaysRelay(callee, false)
    }
})

const PATHS = ["relay", "direct"]
const both = rows => rows.flatMap(row => PATHS.map(path => Object.assign({path}, row)))

// a call made on the shaped network: it connects within the row's seconds, and its audio is clear
feature("#profile, #path: the call connects within #connectMax s with audio concealed under #concealMax", {
    where: both([
        {profile: "lte", connectMax: 2, concealMax: 0.02},
        {profile: "congestedLte", connectMax: 2, concealMax: 0.03},
        {profile: "3g", connectMax: 2, concealMax: 0.03},
        {profile: "edge", connectMax: 4, concealMax: 0.05},
        {profile: "satellite", connectMax: 4, concealMax: 0.02},
    ]),
    timeout: 180,
    given({path}) { alwaysRelay(callee, path === "relay") },
    when({profile}) {
        shape(profile)
        const times = callAndConnect()
        const a = [statsOf(caller), statsOf(callee)]
        sleep(10000)
        const d = [delta(a[0], statsOf(caller)), delta(a[1], statsOf(callee))]
        return Object.assign({d}, times)
    },
    then({path, connectSeconds, concealMax, connectMax, d}) {
        connectSeconds <= connectMax
        d.every(x => x.path == path && x.concealed <= concealMax)
    }
})

// loss, bursts and jitter come during the call: audio stays clear (or understandable), and the call stays up
feature("#profile, #path: audio concealed under #concealMax, the call stays up", {
    where: both([
        {profile: "loss5", concealMax: 0.03},
        {profile: "loss10", concealMax: 0.05},
        {profile: "loss20", concealMax: 0.15},
        {profile: "burst", concealMax: 0.15},
        {profile: "jitter200", concealMax: 0.05},
        {profile: "uplink50k", concealMax: 0.15},
    ]),
    timeout: 180,
    given({path}) { alwaysRelay(callee, path === "relay") },
    when({profile}) {
        callAndConnect()
        shape(profile)
        sleep(2000)
        const a = [statsOf(caller), statsOf(callee)]
        sleep(15000)
        const d = [delta(a[0], statsOf(caller)), delta(a[1], statsOf(callee))]
        const phases = [caller.evaluate(() => window.moqui.call.state.phase), callee.evaluate(() => window.moqui.call.state.phase)]
        return {d, phases}
    },
    then({concealMax, d, phases}) {
        d.every(x => x.concealed <= concealMax && x.received > 100)
        phases.every(p => p == "connected")
    }
})

// the bandwidth drops from 5 Mbit/s to 300 kbit/s and comes back: video goes down and up again, with no long freeze
feature("a bandwidth step, #path: video adapts with no freeze over a second", {
    where: PATHS.map(path => ({path})),
    timeout: 180,
    given({path}) { alwaysRelay(callee, path === "relay") },
    when() {
        shape("bw5m")
        callAndConnect()
        sleep(8000)
        const high = statsOf(callee).height
        const a = statsOf(callee)
        shape("bw300k")
        sleep(6000)
        const low = statsOf(callee).height
        shape("bw5m")
        sleep(12000)
        const d = delta(a, statsOf(callee))
        return {high, low, back: d.height, d}
    },
    then({high, low, back, d}) {
        low < high
        back > low
        d.frozen <= 1 + d.freezes
    }
})

// the network goes away and comes back: within 2 s for 2 and 10 s; after 30 s the call shows Reconnecting and comes back
feature("an outage of #seconds s, #path: audio is back within #backMax s", {
    where: both([{seconds: 2, backMax: 2}, {seconds: 10, backMax: 2}, {seconds: 30, backMax: 10}]),
    timeout: 240,
    given({path}) { alwaysRelay(callee, path === "relay") },
    when({seconds}) {
        callAndConnect()
        shape("outage")
        sleep(seconds * 1000)
        const reconnecting = seconds >= 30 ? caller.evaluate(() => window.moqui.call.state.phase) : null
        shape("clean")
        const back = [backAfter(caller), backAfter(callee)]
        return {reconnecting, back}
    },
    then({seconds, backMax, reconnecting, back}) {
        back.every(t => t <= backMax)
        seconds < 30 || reconnecting == "reconnecting"
    }
})

// Wi-Fi to mobile data: the client's address changes; the pages restart ICE and lose under a second of audio
feature("a new address, #path: under a second of audio lost", {
    where: PATHS.map(path => ({path})),
    timeout: 180,
    given({path}) { alwaysRelay(callee, path === "relay") },
    when() {
        callAndConnect()
        const a = [statsOf(caller), statsOf(callee)]
        shape("newAddress")
        sleep(8000)
        const d = [delta(a[0], statsOf(caller)), delta(a[1], statsOf(callee))]
        return {d}
    },
    then({d}) {
        // 8 s of audio, under one of them filled in
        d.every(x => x.concealed * 8 < 1 && x.received > 100)
    }
})

// UDP blocked: the media server's call goes over TCP (ICE-TCP, or TURN over TCP or TLS); the direct path cannot, and
// the call moves to the media server
feature("UDP blocked, #path: the call goes over TCP through the media server", {
    where: PATHS.map(path => ({path})),
    timeout: 180,
    given({path}) { alwaysRelay(callee, path === "relay") },
    when() {
        shape("udpBlocked")
        callAndConnect()
        sleep(12000)
        return {pages: [statsOf(caller), statsOf(callee)]}
    },
    then({pages}) {
        pages.every(p => p.path == "relay" && p.protocol == "tcp" && p.received > 100)
    }
})
