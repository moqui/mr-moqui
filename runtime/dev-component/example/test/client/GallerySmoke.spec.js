// The gallery screens Gallery.spec.js already renders, in the real /qapps shell at 1280: each loads with no render
// error, and page problems (console errors, uncaught exceptions, failed responses) fail its row.
// Run: moqui test-client example --serve --spec GallerySmoke
const user = params.user ?? "john.doe"
const password = params.password ?? "moqui"

// The same names Gallery.spec.js renders, in that order. menu-include=false screens are here only when that spec
// renders them (QuickNote, QuickTask, Chatter, RecordFiles, Search).
const screens = [
    "Landing", "List", "Detail", "Record360", "Task", "Board", "Calendar", "Split", "Settings", "Wizard",
    "Dialog", "Report", "Panel", "Widgets", "Editor", "QuickNote", "QuickTask", "Chatter", "RecordFiles",
    "Message", "Views", "Search",
]

let page

spec({
    setupSpec() {
        login(user, password)
        page = browser.newPage({ user, password, viewport: { width: 1280, height: 900 }, timeout: 30 })
        // This server mounts example only, so the default theme's Commons avatar URL 404s and the initials show.
        // The Message preview frame is sandboxed on purpose; Chromium reports the blocked script as a console error.
        page.allowProblems(/userPhoto\?userId=/)
        page.allowProblems(/Blocked script execution in 'about:srcdoc'/)
    },
})

feature("gallery #name", {
    where: screens.map((name) => ({ name })),
    expect({ name }) {
        page.goto("/qapps/gallery/" + name).status == 200
        // the shell fetches the screen after the document loads
        page.waitFor(() => {
            const box = document.querySelector(".q-page")
            const loading = !!document.querySelector(".q-loading-bar--visible, .q-inner-loading")
            const rootLoading = !!(window.moqui && window.moqui.webrootVue && window.moqui.webrootVue.loading)
            return !!(box && (box.innerText || "").trim().length > 20 && !loading && !rootLoading)
        })
        page.settle()
        !/Error rendering|Exception in render|is not written yet/.test(page.text())
    },
})
