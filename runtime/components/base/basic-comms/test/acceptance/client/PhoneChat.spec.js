// The phone's chat (plans/Comms-Messaging.md §2.3): the bundled shell's home at 390 x 844, with a stand-in host and server
// (pages/PhoneChat.page.js, whose server answers Chat's transitions as BasicComms does and textRoom as the SMS pair room
// does for a user with a number that texts). The room list fills the screen and Start chat opens the full-screen picker;
// a direct chat with one person, a named group with two and a text to a number start from it; bubbles put the user's own
// messages on the right; attach offers the camera, the gallery, a file and a location; and Back (moqui.back.pop, which Android's back gesture asks) closes the menu, a sheet, the thread
// pane and then the room, and at the home closes nothing, so the app leaves. An acceptance test (a phone-width walk):
// ./moq accept basic-comms --spec PhoneChat
spec({
    server: false,
    timeout: 90,
    setupSpec() {
        page = browser.newPage({ viewport: { width: 390, height: 844 } })
        page.setContent("<!doctype html><html><head><meta name='viewport' content='width=device-width, initial-scale=1'></head><body></body></html>")
        for (const style of ["/libs/material-symbols/outlined.css", "/libs/quasar/quasar.prod.css", "/css/WebrootVue.qvt.css", "/files/crates/moqui-desktop/shell/shell.css"]) page.addStyle(style)
        for (const script of ["/libs/vue/vue.runtime.global.prod.js", "/libs/quasar/quasar.umd.prod.js", "/libs/quasar/lang/en-US.umd.prod.js",
            "/js/MoquiLib.js", "/js/MoquiL10n.js", "/js/WebrootChat.js", "/js/WebrootChatExpiry.js", "/js/WebrootExt.js", "/js/WebrootCrypto.js", "/js/WebrootVue.qvt.render.js",
            "/js/WebrootVue.qvt.js", "/files/crates/moqui-desktop/shell/js/bundled-screens.js",
            "/files/component/basic-comms/test/acceptance/client/pages/PhoneChat.page.js"]) page.addScript(script)
        page.evaluate("window.phone.start()")
    }
})
let page = null

feature("at 390 x 844 the room list fills the screen, and Start chat opens the full-screen picker, which Back closes", {
    when() { return page.evaluate("window.phone.home()") },
    then({ viewport, list, fab, row, search, picker, pickerBack, numbersAsked, closed }) {
        viewport.width == 390
        viewport.pageHeight == viewport.height
        list.left == 0
        list.width == viewport.layoutWidth
        list.bottom >= viewport.height - 2
        row.width >= 380
        search
        fab.right <= 390 && fab.right >= 360
        fab.bottom <= viewport.height && fab.bottom >= viewport.height - 40
        picker.width == 390
        picker.height == viewport.height
        pickerBack
        numbersAsked
        closed
    }
})

feature("a direct chat with one person starts from the picker and opens their room over the list", {
    when() { return page.evaluate("window.phone.direct()") },
    then({ post, title, nameField, shellHeader }) {
        post.params.userIds == "bob"
        title == "Bob Baker"
        !nameField
        !shellHeader
    }
})

feature("two people with a name make a private room of that name", {
    when() { return page.evaluate("window.phone.group()") },
    then({ post, title }) {
        post.params.roomTypeEnumId == "private"
        post.params.roomName == "Lunch crew"
        post.params.memberUserIds == "bob,cy"
        title == "Lunch crew"
    }
})

feature("a phone number typed in the picker opens the number's room, from the user's number; the list shows it with the text icon", {
    when() { return page.evaluate("window.phone.number()") },
    then({ post, title, label, hint, icon, rowIcon }) {
        label == "To: people or a phone number"
        hint.includes("A text from your number")
        post.params.textTo == "(555) 222-4000"
        post.params.textFrom == "+15550004000"
        title == "+15552224000"
        icon == "sms"
        rowIcon == "sms"
    }
})

feature("bubbles: the user's own message on the right, the other person's on the left; attach offers the camera, the gallery, a file and a location", {
    when() { return page.evaluate("window.phone.bubbles()") },
    then({ mine, theirs, mineClass, width, attach, camera, gallery }) {
        attach.join(",") == "camera,gallery,file,location"
        camera == "image/* environment"
        gallery == "image/*,video/* multiple"
        mineClass
        mine.right >= width - 24
        theirs.left <= 24
        mine.left > theirs.left
    }
})

feature("Back closes the menu, then a sheet, then the thread pane, then the room; at the home it closes nothing", {
    when() { return page.evaluate("window.phone.back()") },
    then({ room, menu, thread, sheet, pane, leave, home }) {
        room.depth == 1
        room.back
        !room.shellHeader
        menu.popup && menu.closed && menu.roomStays
        thread.depth == 2
        thread.back
        sheet.popup && sheet.closed && sheet.threadStays
        pane.closed && pane.roomStays
        leave.closed && leave.list
        leave.view == null
        home.depth == 0
        home.closed == false
    }
})
