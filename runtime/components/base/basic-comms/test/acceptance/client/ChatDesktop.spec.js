// The desktop gaps of Chat (plans/Comms-Messaging.md §3, "Done when") in the bundled shell of the desktop and mobile app,
// with a stand-in host and server (pages/ChatDesktop.page.js): Mod+K, two letters and Enter open a room, though a screen
// starts with the same letters (the comms app's Chat rooms lead the palette), and a person with no direct chat gets an unsaved
// message, the chat made by its first send;
// the Activity view lists a mention and a reaction from another user and opens the message; the Threads view lists the
// threads I am in (find#ChatThreads), with their latest reply; All has the Recent, Phone and Bridged groups, and Phone folds and is kept so; a
// person's direct chat has their latest mail thread as a line that says email is not end-to-end encrypted and opens it in
// the Inbox, on a desktop (1280 x 800) and on the phone's home (390 x 844), whose Phone filter keeps the number's room.
// The same palette walk on the web, against a server, is ChatPalette. An acceptance test (a walk at two widths):
// ./moq accept basic-comms --spec ChatDesktop
const SCRIPTS = ["/libs/vue/vue.runtime.global.prod.js", "/libs/quasar/quasar.umd.prod.js", "/libs/quasar/lang/en-US.umd.prod.js",
    "/js/MoquiLib.js", "/js/MoquiL10n.js", "/js/WebrootChat.js", "/js/WebrootExt.js", "/js/WebrootEmoji.js", "/js/WebrootCrypto.js",
    "/js/WebrootVue.qvt.render.js", "/js/WebrootVue.qvt.js", "/files/crates/moqui-desktop/shell/js/bundled-screens.js",
    "/files/component/basic-comms/test/acceptance/client/pages/ChatDesktop.page.js"]
const STYLES = ["/libs/material-symbols/outlined.css", "/libs/quasar/quasar.prod.css", "/css/WebrootVue.qvt.css", "/files/crates/moqui-desktop/shell/shell.css"]
let desktop = null
let phone = null

function open(width, height) {
    const page = browser.newPage({ viewport: { width, height } })
    page.setContent("<!doctype html><html><head><meta name='viewport' content='width=device-width, initial-scale=1'></head><body></body></html>")
    for (const style of STYLES) page.addStyle(style)
    for (const script of SCRIPTS) page.addScript(script)
    page.evaluate("window.desk.start()")
    return page
}
// Mod+K, the text and Enter at once, as a person types them
function palette(page, text) {
    page.keyboard.press("Control+K")
    page.evaluate("window.desk.paletteReady()")
    page.keyboard.type(text)
    page.keyboard.press("Enter")
}

spec({
    server: false,
    timeout: 90,
    setupSpec() {
        desktop = open(1280, 800)
        phone = open(390, 844)
    }
})

feature("in the desktop app Mod+K, two letters and Enter open a room, before a screen of the same letters", {
    when() {
        palette(desktop, "ge")
        return desktop.evaluate("window.desk.afterEnter('R2')")
    },
    then({ view, parameters, searched, remote, paletteOpen, search, lead }) {
        view == "chat"
        parameters.chatRoomId == "R2"
        searched.includes("ge")
        // Getting started was not opened in an app window
        remote.length == 0
        !paletteOpen
        search
        lead.join(",") == "Rooms"
    }
})

feature("a person with no direct chat, chosen in the palette, opens an unsaved message; the direct chat is made by its first send", {
    when() {
        desktop.keyboard.press("Control+K")
        desktop.evaluate("window.desk.paletteReady()")
        desktop.keyboard.type("dee")
        return desktop.evaluate("window.desk.person()")
    },
    then({ order, title, madeBefore, dm, after, made, sent }) {
        // People to message never leads: after the messages
        order.indexOf("People to message") > order.indexOf("Messages")
        title == "Dee Dunn"
        madeBefore == 0
        dm == "dee"
        made.join(",") == "dee"
        sent.join("|") == "D_dee <p>Hi Dee</p>"
        after.view == "chat"
        after.parameters.chatRoomId == "D_dee"
    }
})

feature("the Activity view lists a mention and a reaction from another user, newest first, and opens the message", {
    when() { return desktop.evaluate("window.desk.activity()") },
    then({ kinds, titles, captions, asked, opened }) {
        kinds.join(",") == "mention,reaction"
        titles[0].startsWith("Bob Baker mentioned you")
        titles[1].startsWith("Bob Baker reacted 👍 to your message")
        captions[0] == "General · @Ann can you look at the plan?"
        captions[1] == "Bob Baker · Lunch at noon?"
        asked == ""
        opened.parameters.chatRoomId == "R2"
        opened.parameters.eventSeq == "7"
    }
})

feature("the Threads view lists the threads I am in, mine with only my reply too, the latest reply first, and opens the thread", {
    when() { return desktop.evaluate("window.desk.threads()") },
    then({ titles, rooms, asked, opened }) {
        asked
        rooms.join(",") == "R2,B1,R1"
        // "and others" by people: two others replied in General, one in the lounge; my own last reply in my thread
        titles[0].startsWith("Bob Baker and others replied in a thread")
        titles[1].startsWith("Bob Baker replied in a thread")
        titles[2].startsWith("You replied in a thread")
        opened.parameters.chatRoomId == "R2"
        opened.parameters.thread == "5"
    }
})

feature("All has the Recent, Phone and Bridged groups; Phone folds, and is kept folded for the user", {
    when() { return desktop.evaluate("window.desk.groups()") },
    then({ groups, phoneRow, bridgedRow, foldedRow, expanded, kept }) {
        groups.join(",") == "recent,phone,bridged"
        phoneRow
        bridgedRow
        !foldedRow
        expanded == "false"
        kept.length == 1
        JSON.parse(kept[0]).phone == true
    }
})

feature("a person's direct chat has their latest mail thread, with the plain-mail label and no lock, opening in the Inbox", {
    when() { return desktop.evaluate("window.desk.mailLine()") },
    then({ text, label, others, lock, opened }) {
        text == "Email: Q3 plan · Email is not end-to-end encrypted"
        label == "Email: Q3 plan. Email is not end-to-end encrypted"
        others == 1
        !lock
        opened.view == "mail"
        opened.parameters.thread == "email:M1"
    }
})

feature("on the phone's home the mail line is there too, and the Phone filter keeps the number's room alone", {
    when() { return {mail: phone.evaluate("window.desk.mailLine()"), filter: phone.evaluate("window.desk.phoneFilter()")} },
    then({ mail, filter }) {
        mail.label == "Email: Q3 plan. Email is not end-to-end encrypted"
        mail.opened.view == "mail"
        mail.opened.parameters.thread == "email:M1"
        filter.all.includes("R1")
        filter.all.includes("X1")
        filter.phoneOnly.join(",") == "X1"
    }
})
