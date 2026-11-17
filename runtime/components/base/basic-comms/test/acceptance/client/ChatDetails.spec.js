// The room's details tabs and a voice note (plans/Comms-Messaging.md §5, "Done when"; §5.1): on a phone (390 x 844, the
// bundled shell's home) and on a desktop (1280 x 800, the bundled Chat), with a stand-in host and server
// (pages/ChatDetails.page.js) whose lists answer by what the tabs ask. A room with photos, a video, a PDF, a voice note, two
// links and three reactions: Media shows the photos and the video as a grid, whose viewer swipes from one to the next; Files
// the PDF only; Voice the voice note with its player; Links both links, the cached title for one; Reactions the two
// messages, the latest reaction first, and To my messages the one of mine; People the members; Pinned says there are none.
// Then a voice note: with no text the microphone takes Send's place, shows its time while it records, and Send uploads it
// as an audio file and sends it, and it plays in the room. An acceptance test (a walk at two widths):
// ./moq accept basic-comms --spec ChatDetails
const SCRIPTS = ["/libs/vue/vue.runtime.global.prod.js", "/libs/quasar/quasar.umd.prod.js", "/libs/quasar/lang/en-US.umd.prod.js",
    "/js/MoquiLib.js", "/js/MoquiL10n.js", "/js/WebrootChat.js", "/js/WebrootExt.js", "/js/WebrootEmoji.js", "/js/WebrootCrypto.js",
    "/js/WebrootVue.qvt.render.js", "/js/WebrootVue.qvt.js", "/files/crates/moqui-desktop/shell/js/bundled-screens.js",
    "/files/component/basic-comms/test/acceptance/client/pages/ChatDetails.page.js"]
const STYLES = ["/libs/material-symbols/outlined.css", "/libs/quasar/quasar.prod.css", "/css/WebrootVue.qvt.css", "/files/crates/moqui-desktop/shell/shell.css"]
let pages = {}

function open(width, height) {
    const page = browser.newPage({ viewport: { width, height } })
    // the stand-in server keeps no file: the pictures, video and sounds the tabs and the room show are 404s
    page.allowProblems(/Chat\/attachment\?chatAttachmentId=/)
    page.setContent("<!doctype html><html><head><meta name='viewport' content='width=device-width, initial-scale=1'></head><body></body></html>")
    for (const style of STYLES) page.addStyle(style)
    for (const script of SCRIPTS) page.addScript(script)
    page.evaluate("window.details.start()")
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

// what is wrong with the tabs a walk saw: one problem string, the same for both widths
function problems(seen) {
    const out = []
    const want = (ok, what) => { if (!ok) out.push(what) }
    want(seen.media.join(",") == "F3,F1,F2" && seen.video, "Media: the video and the two photos, newest first " + JSON.stringify(seen.media))
    want(seen.viewerFirst == "F3" && seen.viewerCount.includes("1 / 3"), "the viewer at the video, 1 of 3: " + seen.viewerFirst + " " + seen.viewerCount)
    want(seen.viewerNext == "F1" && seen.viewerImage, "a swipe to the next, a photo: " + seen.viewerNext)
    want(seen.paneAfterViewer, "the Media tab stays when the viewer closes")
    want(seen.files.join(",") == "F4", "Files: the PDF only " + JSON.stringify(seen.files))
    want(seen.voice.join(",") == "F5" && seen.voicePlayer, "Voice: the voice note with its player " + JSON.stringify(seen.voice))
    want(seen.links.join(" | ") == "https://example.com/guide The guide | https://example.org/page example.org", "Links: both, the title for one " + JSON.stringify(seen.links))
    want(seen.reacted.join(",") == "4,1" && seen.keys.join("") == "❤️👍🎉", "Reactions: the latest first, with their keys " + JSON.stringify(seen))
    want(seen.toMe.join(",") == "4" && seen.askedToMe == "true", "To my messages: mine only, asked of the server")
    want(seen.people.some((name) => name.includes("Bob Baker")) && seen.people.some((name) => name.includes("Cy Cole")), "People: the members " + JSON.stringify(seen.people))
    want(seen.pinned.includes("No pinned messages"), "Pinned: none yet")
    want(seen.fileAsks.slice().sort().join(" ") == "-image/,video/,audio/ audio/ image/,video/", "the tabs ask for their types " + JSON.stringify(seen.fileAsks))
    want(seen.closed, "Back closes the details")
    return out.join("; ")
}

feature("on a phone each tab of the details shows the right items, and the viewer swipes", {
    when() { return pages.phone.evaluate("window.details.tabs()") },
    then(seen) {
        seen.width == 390
        problems(seen) == ""
    }
})

feature("on a phone a voice note is recorded in place of Send, sent as an audio file and played in the room", {
    when() { return pages.phone.evaluate("window.details.voiceNote()") },
    then({ mic, sendShown, time, uploaded, attachments, inline, micAgain }) {
        mic == "Record a voice note"
        !sendShown
        /^0:0[1-9]$/.test(time)
        uploaded == 1
        attachments == "UP1"
        inline
        micAgain
    }
})

feature("on a desktop each tab of the details shows the same items, in the right pane", {
    when() { return pages.desktop.evaluate("window.details.tabs()") },
    then(seen) {
        seen.width == 1280
        problems(seen) == ""
    }
})

feature("on a desktop a voice note is recorded and sent the same way", {
    when() { return pages.desktop.evaluate("window.details.voiceNote()") },
    then({ mic, uploaded, attachments, inline }) {
        mic == "Record a voice note"
        uploaded == 1
        attachments == "UP1"
        inline
    }
})
