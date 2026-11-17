// The E2EE labels (plans/Comms-Messaging.md §6.2, "Labels done when"; COM-UI-001): each state of a room shows on a phone
// (390 x 844, the bundled shell's home) and on a desktop (1280 x 800, the bundled Chat), from moqui.chat.e2eeLabel, with a
// stand-in host and server (pages/ChatLabels.page.js). The lock is in the header and the room list for an encrypted room
// only, with a check when every device is verified and Verify in its banner when not; a plain private room and a direct
// chat have the open lock and the plain banner; a public channel the open lock and no banner; the phone number's room the
// SMS banner and "Text message" in its composer; a bridged room the bridge banner and the network in its composer; the
// members pane says the label and checks a verified person, and Verify in the banner asks the other person's device. The call
// states are the call screen's (Comms-Messaging phase 4). An acceptance test (a walk at two widths):
// ./moq accept basic-comms --spec ChatLabels
const SCRIPTS = ["/libs/vue/vue.runtime.global.prod.js", "/libs/quasar/quasar.umd.prod.js", "/libs/quasar/lang/en-US.umd.prod.js",
    "/js/MoquiLib.js", "/js/MoquiL10n.js", "/js/WebrootChat.js", "/js/WebrootExt.js", "/js/WebrootCrypto.js", "/js/WebrootVue.qvt.render.js",
    "/js/WebrootVue.qvt.js", "/files/crates/moqui-desktop/shell/js/bundled-screens.js",
    "/files/component/basic-comms/test/acceptance/client/pages/ChatLabels.page.js"]
const STYLES = ["/libs/material-symbols/outlined.css", "/libs/quasar/quasar.prod.css", "/css/WebrootVue.qvt.css", "/files/crates/moqui-desktop/shell/shell.css"]
const SEALED = "Messages and calls are end-to-end encrypted. Only people in this chat can read them."
const PLAIN = "Not end-to-end encrypted. The server can read these messages."
const SMS = "Not end-to-end encrypted. Texts go through the phone network and the SMS provider."
const BRIDGED = "Not end-to-end encrypted on this server. The bridge reads messages to pass them on."
let pages = {}

function open(width, height) {
    const page = browser.newPage({ viewport: { width, height } })
    page.setContent("<!doctype html><html><head><meta name='viewport' content='width=device-width, initial-scale=1'></head><body></body></html>")
    for (const style of STYLES) page.addStyle(style)
    for (const script of SCRIPTS) page.addScript(script)
    page.evaluate("window.labels.start()")
    return page
}

spec({
    server: false,
    timeout: 120,
    setupSpec() {
        pages.phone = open(390, 844)
        pages.desktop = open(1280, 800)
    }
})

// what is wrong with the rooms a walk saw, by room: one problem string, the same for both widths
function problems(rooms) {
    const out = []
    const want = (id, ok, what) => { if (!ok) out.push(id + ": " + what + " " + JSON.stringify(rooms[id])) }
    // encrypted, every device verified: the lock with its check, in the header and the list
    want("V1", rooms.V1.header == "verified" && rooms.V1.lock && rooms.V1.check && rooms.V1.listLock, "the lock with a check")
    want("V1", rooms.V1.banner == "verified" && rooms.V1.bannerText == SEALED && !rooms.V1.verify, "the encrypted banner, no Verify")
    // encrypted, a device not verified: the lock, and Verify
    want("E1", rooms.E1.header == "encrypted" && rooms.E1.lock && !rooms.E1.check && rooms.E1.listLock, "the lock, no check")
    want("E1", rooms.E1.bannerText == SEALED && rooms.E1.verify, "the encrypted banner with Verify")
    // a plain private room and a plain direct chat: the open lock and the plain banner, no lock in the list
    for (const id of ["P1", "D1"]) want(id, rooms[id].header == "plain" && !rooms[id].lock && !rooms[id].listLock && rooms[id].bannerText == PLAIN, "the open lock and the plain banner")
    want("P1", rooms.P1.intro == PLAIN && rooms.P1.placeholder.startsWith("Message Ops"), "the plain line at its start, its own composer")
    // a public channel: the open lock, no banner
    want("C1", rooms.C1.header == "public" && !rooms.C1.lock && rooms.C1.banner == null, "the open lock and no banner")
    // the phone number's room: the SMS banner, and a text message in its composer
    want("S1", rooms.S1.header == "sms" && !rooms.S1.lock && rooms.S1.bannerText == SMS && rooms.S1.placeholder == "Text message", "the SMS banner and Text message")
    // a bridged room: the bridge's banner, and the network in its composer
    want("W1", rooms.W1.header == "bridged" && !rooms.W1.lock && rooms.W1.bannerText == BRIDGED && rooms.W1.placeholder == "WhatsApp message", "the bridge's banner and its network")
    return out.join("; ")
}

feature("on a phone each room shows its E2EE label: the lock for end to end only, the banner of its state, and the composer's hint for a text and a bridge", {
    when() { return pages.phone.evaluate("window.labels.walk()") },
    then({ width, rooms, members, verify }) {
        width == 390
        problems(rooms) == ""
        // the members pane says the label too (§6.1 rule 1) and checks the verified person (rule 3)
        members.label == "verified" && members.checks >= 1
        // Verify asks the other person's device (COM-UI-013)
        verify.title.includes("Verify Bob Baker") && verify.device.includes("Bob's laptop") && verify.verifyButton
    }
})

feature("on a desktop each room shows the same labels, in Chat's rail and room", {
    when() { return pages.desktop.evaluate("window.labels.walk()") },
    then({ width, rooms, members, verify }) {
        width == 1280
        problems(rooms) == ""
        members.label == "verified" && members.checks >= 1
        verify.title.includes("Verify Bob Baker") && verify.device.includes("Bob's laptop") && verify.verifyButton
    }
})
