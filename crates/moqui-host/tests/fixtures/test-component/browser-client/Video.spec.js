// moqui test-client --video (crates/moqui-host/tests/client_browser.rs): an offline page that changes, then a static
// stretch, and a feature skipped by a --param, so the video spans the spec and the timeline has both features.

spec({ server: false })

// read at the top level, as a spec deciding what to run would
const phase = params.phase

feature("a page changes and then stays the same", {
    expect() {
        const page = browser.newPage({ viewport: { width: 640, height: 480 } })
        page.setContent(`<!doctype html><html><body style="background:#fff"><h1 id="t">one</h1></body></html>`)
        sleep(300)
        page.evaluate(() => {
            document.getElementById("t").textContent = "two"
            document.body.style.background = "#cde"
            return true
        })
        sleep(700)
        page.text().includes("two")
    },
})

feature("a feature the before phase skips", {
    expect() {
        if (params.phase === "before") skip(`phase is ${phase}`)
        false
    },
})
