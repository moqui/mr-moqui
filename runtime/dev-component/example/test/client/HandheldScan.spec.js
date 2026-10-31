// The gallery's Handheld screen in the real /qapps shell at phone width (390): density="handheld" wins over the compact
// preference (48 px fields, 18 px text, no drawer, a header of the title and the account), and the scan-input resolves a
// typed scan (keys as a keyboard wedge sends them, ending in Enter) through the screen's scanResolve transition: one match
// submits with its id, type and scan id; two open the chooser; none is named and keeps the focus; the put-away bin waits for
// Confirm.
// Run: moqui test-client example --serve --spec HandheldScan
const user = params.user ?? "john.doe"
const password = params.password ?? "moqui"

let page

spec({
    setupSpec() {
        login(user, password)
        page = browser.newPage({ user, password, viewport: { width: 390, height: 844 }, timeout: 30 })
        page.allowProblems(/userPhoto\?userId=/)
    },
})

feature("the handheld screen has its density over the compact preference, no drawer and a reduced header", {
    expect() {
        page.goto("/qapps/gallery/Handheld").status == 200
        page.waitFor(() => !!document.querySelector(".m-scan-input"))
        page.settle()
        page.evaluate(() => { window.moqui.applyPreference("SHELL_DENSITY", "compact"); return true })
        const shell = page.evaluate(() => {
            const body = document.body.classList
            const field = document.querySelector("#ScanAnything_itemId").closest(".q-field").querySelector(".q-field__control")
            return { handheld: body.contains("m-density-handheld"), compact: body.contains("m-density-compact"),
                drawer: !!document.querySelector(".q-drawer--left"), search: !!document.querySelector("#m-search"),
                account: !!document.querySelector("#m-account"), fieldHeight: Math.round(field.getBoundingClientRect().height),
                fontSize: getComputedStyle(document.body).fontSize, focused: document.activeElement && document.activeElement.id }
        })
        shell.handheld
        !shell.compact
        !shell.drawer
        !shell.search
        shell.account
        shell.fieldHeight >= 48
        shell.fontSize == "18px"
        shell.focused == "ScanAnything_itemId"
    },
})

feature("one match submits with the id, type and scan id; two open the chooser; none is named", {
    expect() {
        page.goto("/qapps/gallery/Handheld")
        page.waitFor(() => !!document.querySelector("#ScanAnything_itemId"))
        page.settle()
        page.click("#ScanAnything_itemId")
        page.keyboard.type("A-100")
        page.keyboard.press("Enter")
        page.waitFor(() => /Scanned asset A-100 from A-100, operation [0-9a-f-]{8,}/.test(document.body.innerText))

        page.click("#ScanAnything_itemId")
        page.keyboard.type("DUP-1")
        page.keyboard.press("Enter")
        page.waitFor(() => document.querySelectorAll(".m-scan-choice").length === 2)
        // the number key picks: 1 is the asset
        page.keyboard.press("1")
        page.waitFor(() => /Scanned asset DUP-1/.test(document.body.innerText))

        page.click("#ScanAnything_itemId")
        page.keyboard.type("NOPE")
        page.keyboard.press("Enter")
        page.waitFor(() => /No match for NOPE/.test(document.body.innerText))
        page.evaluate(() => document.activeElement && document.activeElement.id) == "ScanAnything_itemId"
    },
})

feature("the put-away bin waits for Confirm", {
    expect() {
        page.goto("/qapps/gallery/Handheld")
        page.waitFor(() => !!document.querySelector("#PutAway_binId"))
        page.settle()
        page.click("#PutAway_binId")
        page.keyboard.type("B-01-07")
        page.keyboard.press("Enter")
        page.waitFor(() => !!document.querySelector(".m-scan-confirm"))
        !/Put away to B-01-07/.test(page.text())
        page.getByRole("button", { name: "Confirm" }).click()
        page.waitFor(() => /Put away to B-01-07, operation [0-9a-f-]{8,}/.test(document.body.innerText))
    },
})
