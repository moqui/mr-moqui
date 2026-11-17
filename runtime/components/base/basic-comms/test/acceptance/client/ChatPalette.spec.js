// The quick switcher on the web (plans/Comms-Messaging.md §3, "Done when"): in Comms, Mod+K, two letters and Enter open a
// room. john.doe's room of CryptoClientFixtures ("Crypto client spec") is found by "cr" through the server's palette
// (shellSearch, search#PaletteRooms), whose Chat rooms lead while Comms is the current app, with no # prefix. The same
// walk in the desktop app's shell is ChatDesktop. Comms is the server's comms app (calls.appRoot of the page's config):
// BasicComms alone, or an app that mounts it, such as BOSS's Comms, which hides BasicComms from the launcher, so BasicComms
// is then no app of the shell's and has no lead group. An acceptance test (a walk against the suite's server):
// ./moq accept basic-comms --spec ChatPalette
let page = null

spec({
    timeout: 60,
    setupSpec() {
        page = browser.newPage({user: "john.doe", timeout: 30})
        page.allowProblems(/userPhoto\?userId=/)
    }
})

feature("in Comms, Mod+K, two letters and Enter open a room", {
    when() {
        // the comms app of this server (/qapps/BasicComms, or /qapps/Comms with BOSS)
        page.goto("/qapps/BasicComms/Chat")
        page.locator(".m-chat-root").waitFor({timeout: 20})
        const appRoot = page.evaluate(() => (window.moqui.config && window.moqui.config.calls && window.moqui.config.calls.appRoot) || "/qapps/BasicComms")
        if (appRoot !== "/qapps/BasicComms") {
            page.goto(appRoot + "/Chat")
            page.locator(".m-chat-root").waitFor({timeout: 20})
        }
        const lead = page.evaluate(() => window.moqui.webrootVue.paletteLead.slice())
        page.keyboard.press("Control+K")
        page.waitFor(() => document.getElementById("m-palette-input") === document.activeElement, {timeout: 10})
        page.keyboard.type("cr")
        page.keyboard.press("Enter")
        page.waitForURL(/chatRoomId=/, {timeout: 15})
        const url = page.url()
        const roomId = decodeURIComponent((/[?&]chatRoomId=([^&#]*)/.exec(url) || [])[1] || "") || null
        const roomName = page.evaluate(id => window.moqui.request(id[1].replace(/^\/qapps/, "/apps") + "/Chat/room", {params: {chatRoomId: id[0]}})
            .then(result => (result.json && result.json.room && result.json.room.roomName) || null), [roomId, appRoot])
        return {lead, roomId, roomName, appRoot, path: url.replace(/^[a-z]+:\/\/[^/]+/, "").split("?")[0]}
    },
    then({lead, roomId, roomName, appRoot, path}) {
        lead.includes("Rooms")
        roomId != null
        roomName == "Crypto client spec"
        path == appRoot + "/Chat"
    }
})
