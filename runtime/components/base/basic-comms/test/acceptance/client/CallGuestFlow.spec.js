// A guest by link in a browser (plans/archive/Comms-Framework.md W3): john.doe makes a call link and opens its meeting, a page with
// no account opens /call/<token>, gives a name and asks to join, waits in the lobby until john.doe lets them in from his
// call view, then the two pages carry each other's audio and video through the server's SFU over the guest's own socket
// (/callws), and the guest leaves. Chromium's fake camera and microphone stand in for devices. Needs a binary with the
// rtc feature and a server holding call media (the suite's --serve server sets rtc_media_port).
// Run: ./moq test-client basic-comms --serve --spec CallGuestFlow
let host = null
let visitor = null

const received = page =>
    page.evaluate(() => {
        const call = window.moqui && window.moqui.call
        if (!call || !call.pc || call.state.phase !== "connected") return {audio: 0, video: 0}
        return call.pc.getStats().then(stats => {
            let audio = 0
            let video = 0
            stats.forEach(s => {
                if (s.type === "inbound-rtp" && s.kind === "audio") audio += s.packetsReceived || 0
                if (s.type === "inbound-rtp" && s.kind === "video") video += s.framesDecoded || 0
            })
            return {audio, video}
        })
    })

spec({
    requires: ["rtc"],
    setupSpec() {
        host = browser.newPage({user: "john.doe", timeout: 30})
        visitor = browser.newPage({timeout: 30})
        host.allowProblems(/userPhoto\?userId=/)
    }
})

feature("a guest asks to join by link, is let in, and the two carry each other's media", {
    when() {
        host.goto("/qapps/BasicComms/Calls")
        const made = host.evaluate(() =>
            window.moqui.call.request("createCallLink", {name: "Client spec meeting"}).then(link =>
                window.moqui.call.request("openCallLink", {callLinkId: link.callLinkId}).then(opened => {
                    window.moqui.call.open(opened.callSessionId)
                    return window.moqui.call.join(opened.callSessionId, opened.callLegId, {video: true}).then(() => ({path: link.path,
                        callSessionId: opened.callSessionId}))
                })
            )
        )
        visitor.goto(made.path)
        visitor.getByLabel("Your name").fill("Visitor")
        visitor.getByRole("button", {name: "Ask to join"}).click()
        visitor.getByText("Waiting for someone in the call to let you in").waitFor({timeout: 10})
        host.getByText("is waiting to join").waitFor({timeout: 15})
        host.getByRole("button", {name: "Let in", exact: true}).click()
        const enough = m => m.audio > 20 && m.video > 5
        let both = false
        try {
            both = [visitor, host].every(page => page.waitFor(() => {
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
            }, {timeout: 20}))
        } catch (err) {
            both = false
        }
        const media = [received(visitor), received(host)]
        visitor.getByRole("button", {name: "Leave", exact: true}).click()
        visitor.getByText("You left the call.").waitFor({timeout: 10})
        const gone = host.waitFor(() => Object.keys(window.moqui.call.state.remotes).length === 0, {timeout: 15})
        return {made, both, media: media.map(enough), gone}
    },
    then({made, both, media, gone}) {
        made.path.startsWith("/call/")
        both == true
        media.every(ok => ok)
        gone == true
    }
})
