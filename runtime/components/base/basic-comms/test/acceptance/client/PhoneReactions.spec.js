// Reactions, who reacted and Info by touch on a phone (plans/Comms-Messaging.md §4, "Done when"): the bundled shell at 390 x 844
// with a stand-in host and server (pages/PhoneReactions.page.js), driven only by made touch events (a tap is touchstart,
// touchend and the click a phone sends after it; a long press holds the touch past 450 ms). A long press on a message opens
// its sheet, where a quick reaction reacts; a long press on the reaction's chip (with the context menu Android's WebView
// sends during it) lists who reacted, opens no sheet and is not a tap; a tap on
// the chip takes the reaction back; "+" opens the emoji picker, where a search and a tap react; and Info, from the sheet,
// shows when it was sent and who has read up to it. An acceptance test (a phone walk): ./moq accept basic-comms --spec PhoneReactions
spec({
    server: false,
    timeout: 90,
    setupSpec() {
        page = browser.newPage({ viewport: { width: 390, height: 844 } })
        page.setContent("<!doctype html><html><head><meta name='viewport' content='width=device-width, initial-scale=1'></head><body></body></html>")
        for (const style of ["/libs/material-symbols/outlined.css", "/libs/quasar/quasar.prod.css", "/css/WebrootVue.qvt.css", "/files/crates/moqui-desktop/shell/shell.css"]) page.addStyle(style)
        for (const script of ["/libs/vue/vue.runtime.global.prod.js", "/libs/quasar/quasar.umd.prod.js", "/libs/quasar/lang/en-US.umd.prod.js",
            "/js/MoquiLib.js", "/js/MoquiL10n.js", "/js/WebrootChat.js", "/js/WebrootExt.js", "/js/WebrootEmoji.js", "/js/WebrootCrypto.js",
            "/js/WebrootVue.qvt.render.js", "/js/WebrootVue.qvt.js", "/files/crates/moqui-desktop/shell/js/bundled-screens.js",
            "/files/component/basic-comms/test/acceptance/client/pages/PhoneReactions.page.js"]) page.addScript(script)
        page.evaluate("window.phone.start()")
    }
})
let page = null

feature("a long press on a message opens its sheet, and a tap on a quick reaction reacts", {
    when() { return page.evaluate("window.phone.react()") },
    then({ sheet, actions, posted, chip }) {
        sheet
        actions.includes("info") && actions.includes("reply") && actions.includes("delete") == false
        posted.key == "👍"
        posted.eventSeq == 1
        chip.text == "👍 2"
        chip.mine
    }
})

feature("a long press on the reaction's chip lists who reacted, and is not a tap; a tap on the chip takes the reaction back", {
    when() { return page.evaluate("window.phone.whoReacted()") },
    then({ people, title, reactsAfterHold, sheet, lists, menuPrevented, closed, removed, notMine }) {
        // Android's context menu of the long press opens neither the message's sheet nor the list twice
        !sheet
        lists == 1
        menuPrevented
        people.slice().sort().join(",") == "Bob Baker,You"
        title.includes("👍")
        reactsAfterHold == 0
        closed
        removed.key == "👍"
        notMine
    }
})

feature("+ opens the emoji picker in the sheet, where a search and a tap react", {
    when() { return page.evaluate("window.phone.picker()") },
    then({ picker, tabs, found, posted, chip }) {
        picker
        tabs >= 8
        found.includes("🎉")
        posted.key == "🎉"
        chip
    }
})

feature("Info, from the sheet: when the message was sent and who has read up to it", {
    when() { return page.evaluate("window.phone.info()") },
    then({ asked, sent, readBy, note, closed }) {
        asked.eventSeq == 2
        sent.startsWith("Sent")
        readBy == "Read byBob Baker"
        note.includes("not known to the server")
        closed
    }
})
