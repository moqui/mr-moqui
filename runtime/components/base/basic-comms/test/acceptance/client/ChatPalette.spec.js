// The quick switcher on the web (plans/Comms-Messaging.md §3, "Done when"): in Comms, Mod+K, two letters and Enter open a
// room. john.doe's room of CryptoClientFixtures ("Crypto client spec") is found by "cr" through the server's palette
// (shellSearch, search#PaletteRooms), whose Chat rooms lead while Comms is the current app, with no # prefix. The same
// walk in the desktop app's shell is ChatDesktop. An acceptance test (a walk against the suite's server):
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
        page.goto("/qapps/BasicComms/Chat")
        page.locator(".m-chat-root").waitFor({timeout: 20})
        const lead = page.evaluate(() => window.moqui.webrootVue.paletteLead.slice())
        page.keyboard.press("Control+K")
        page.waitFor(() => document.getElementById("m-palette-input") === document.activeElement, {timeout: 10})
        page.keyboard.type("cr")
        page.keyboard.press("Enter")
        page.waitForURL(/chatRoomId=/, {timeout: 15})
        const url = page.url()
        const roomId = decodeURIComponent((/[?&]chatRoomId=([^&#]*)/.exec(url) || [])[1] || "") || null
        const roomName = page.evaluate(id => window.moqui.request("/apps/BasicComms/Chat/room", {params: {chatRoomId: id}})
            .then(result => (result.json && result.json.room && result.json.room.roomName) || null), roomId)
        return {lead, roomId, roomName, path: url.replace(/^[a-z]+:\/\/[^/]+/, "").split("?")[0]}
    },
    then({lead, roomId, roomName, path}) {
        lead.includes("Rooms")
        roomId != null
        roomName == "Crypto client spec"
        path == "/qapps/BasicComms/Chat"
    }
})
