// Screenshots of the gallery screens Gallery.spec.js already renders, in the real /qapps shell: one light page at
// 1280 for each, plus List and Detail in the dark and at phone width, and the Assist drawer on List when it is
// available. A render-error phrase fails the row. Page problems fail it too.
// Run: moqui test-client example --serve --spec GalleryShots
const user = params.user ?? "john.doe"
const password = params.password ?? "moqui"

// The same names Gallery.spec.js renders, in that order. menu-include=false screens are here only when that spec
// renders them (QuickNote, QuickTask, Chatter, RecordFiles, Search).
const screens = [
    "Landing", "List", "Detail", "Record360", "Task", "Board", "Calendar", "Split", "Settings", "Wizard",
    "Dialog", "Report", "Panel", "Widgets", "Editor", "QuickNote", "QuickTask", "Chatter", "RecordFiles",
    "Message", "Views", "Search",
]

const row = (name, path, extra) => Object.assign({ name, path, dark: false, width: 1280, assist: false }, extra)
const rows = screens.map((name) => row(name, "/qapps/gallery/" + name))
rows.push(row("List-dark", "/qapps/gallery/List", { dark: true }))
rows.push(row("Detail-dark", "/qapps/gallery/Detail", { dark: true }))
rows.push(row("List-390", "/qapps/gallery/List", { width: 390 }))
rows.push(row("Detail-390", "/qapps/gallery/Detail", { width: 390 }))
rows.push(row("List-assist", "/qapps/gallery/List", { assist: true }))

let wide
let phone

spec({
    setupSpec() {
        login(user, password)
        wide = browser.newPage({ user, password, viewport: { width: 1280, height: 900 }, timeout: 30 })
        phone = browser.newPage({ user, password, viewport: { width: 390, height: 900 }, timeout: 30 })
        // This server mounts example only, so the default theme's Commons avatar URL 404s and the initials show.
        // The Message preview frame is sandboxed on purpose; Chromium reports the blocked script as a console error.
        for (const page of [wide, phone]) {
            page.allowProblems(/userPhoto\?userId=/)
            page.allowProblems(/Blocked script execution in 'about:srcdoc'/)
        }
    },
})

feature("gallery #name", {
    where: rows,
    expect({ name, path, dark, width, assist }) {
        const page = width < 600 ? phone : wide
        page.goto(path).status == 200
        // the shell fetches the screen after the document loads
        page.waitFor(() => {
            const box = document.querySelector(".q-page")
            const loading = !!document.querySelector(".q-loading-bar--visible, .q-inner-loading")
            const rootLoading = !!(window.moqui && window.moqui.webrootVue && window.moqui.webrootVue.loading)
            return !!(box && (box.innerText || "").trim().length > 20 && !loading && !rootLoading)
        })
        page.settle()
        // Quasar dark mode is $q.dark.set. colorScheme does not change it.
        page.evaluate((dark) => {
            const root = document.querySelector("#apps-root")
            const app = root && root.__vue_app__
            if (!app) return false
            app.config.globalProperties.$q.dark.set(!!dark)
            return true
        }, dark) == true
        page.waitFor((dark) => document.body.classList.contains("body--dark") === !!dark, { arg: dark })
        if (assist) {
            const available = page.evaluate(() => !!(window.moqui && window.moqui.assist && window.moqui.assist.state
                && window.moqui.assist.state.available))
            if (!available) skip("Assist is not available")
            page.evaluate(() => { window.moqui.assist.openDrawer(); return true }) == true
            page.settle()
        }
        !/Error rendering|Exception in render|is not written yet/.test(page.text())
        page.screenshot("Gallery-" + name, { fullPage: true })
    },
})
