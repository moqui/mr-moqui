// Key behavior of the html client, with real key events. Was js/keys-browser.test.js.
spec({ server: false, timeout: 60 })

const states = `<option value=""></option><option value="AL">Alabama</option><option value="AK">Alaska</option><option value="AZ">Arizona</option>`

function openHtml(body) {
    const page = browser.newPage({ viewport: { width: 1200, height: 900 } })
    page.setContent(`<!doctype html><html data-theme="light">
        <link rel="stylesheet" href="/css/moqui-html.css">
        <link rel="stylesheet" href="/libs/tom-select/tom-select.css">
        ${body}
        <script src="/libs/tom-select/tom-select.complete.min.js"></script>
        <script src="/js/moqui-html.js"></script>`)
    page.evaluate(() => {
        window.lastKey = null
        document.addEventListener("keydown", (event) => {
            window.lastKey = { key: event.key, prevented: event.defaultPrevented }
        })
    })
    return page
}

function focusField(page, id) {
    page.evaluate((id) => {
        const node = document.getElementById(id)
        const input = node.tomselect ? node.tomselect.control_input : node
        input.focus()
    }, id)
}

feature("a Tom Select drop-down takes the typed match on a quick ENTER or TAB, and TAB moves on", {
    expect() {
        const page = openHtml(`<form id="form"><select id="state" name="state" data-ts="select" data-empty>${states}</select>
            <select id="state2" name="state2" data-ts="select" data-empty>${states}</select><input id="next" name="next"></form>`)
        focusField(page, "state")
        page.keyboard.type("Ariz")
        page.keyboard.press("Enter")
        sleep(1000)
        const afterEnter = page.evaluate(() => ({ value: document.getElementById("state").value, prevented: window.lastKey && window.lastKey.prevented }))
        focusField(page, "state2")
        page.keyboard.type("Alas")
        page.keyboard.press("Tab")
        sleep(1000)
        const after = page.evaluate(() => ({ value: document.getElementById("state2").value, focus: document.activeElement.id }))
        assert.equal(afterEnter.value, "AZ")
        assert.equal(afterEnter.prevented, true)
        assert.equal(after.value, "AK")
        assert.equal(after.focus, "next")
    },
})

feature("a drop-down with the style submit-on-close submits its form", {
    expect() {
        const page = openHtml(`<form id="form"><select id="ts" name="ts" class="submit-on-close" data-ts="select" data-empty>${states}</select>
            <select id="plain" name="plain" class="submit-on-close">${states}</select></form>`)
        const armed = page.evaluate(() => {
            window.submits = []
            document.getElementById("form").addEventListener("submit", (event) => {
                event.preventDefault()
                window.submits.push(new FormData(event.target).get("ts") + "/" + new FormData(event.target).get("plain"))
            })
            return true
        })
        assert.ok(armed)
        focusField(page, "ts")
        page.keyboard.type("Alab")
        page.keyboard.press("Enter")
        sleep(1000)
        const changed = page.evaluate(() => {
            const plain = document.getElementById("plain")
            plain.value = "AK"
            plain.dispatchEvent(new Event("change", { bubbles: true }))
            return true
        })
        assert.ok(changed)
        sleep(100)
        const submits = page.evaluate(() => window.submits)
        assert.deepEqual(submits, ["AL/", "AL/AK"])
    },
})

feature("date keys step a native date input and a text date field, and leave the native arrows alone", {
    expect() {
        const page = openHtml(`<form><input type="date" id="date" name="date" value="2026-10-25">
            <input type="datetime-local" id="stamp" name="stamp" value="2026-10-25T10:00" step="900">
            <input type="text" id="text" name="text" value="25/10/2026" data-format="dd/MM/yyyy"><input type="text" id="plain" value="x"></form>`)
        const watching = page.evaluate(() => { window.changes = 0; document.addEventListener("change", () => window.changes++); return true })
        assert.ok(watching)
        const press = (id, key) => { focusField(page, id); page.keyboard.press(key); sleep(50) }
        press("date", "d")
        const d = page.evaluate(() => document.getElementById("date").value)
        press("date", "Shift+W")
        const shiftW = page.evaluate(() => document.getElementById("date").value)
        press("date", "m")
        const m = page.evaluate(() => document.getElementById("date").value)
        press("date", "ArrowUp")
        const nativeArrow = page.evaluate(() => [document.getElementById("date").value, window.lastKey.prevented])
        press("date", "Control+d")
        const ctrlD = page.evaluate(() => [document.getElementById("date").value, window.lastKey.prevented])
        press("stamp", "i")
        const i = page.evaluate(() => document.getElementById("stamp").value)
        press("stamp", "Shift+h")
        const shiftH = page.evaluate(() => document.getElementById("stamp").value)
        press("text", "ArrowUp")
        const textArrow = page.evaluate(() => document.getElementById("text").value)
        press("text", "y")
        const y = page.evaluate(() => document.getElementById("text").value)
        press("plain", "d")
        const plain = page.evaluate(() => window.lastKey.prevented)
        const changes = page.evaluate(() => window.changes)
        assert.equal(d, "2026-10-26")
        assert.equal(shiftW, "2026-10-19")
        assert.equal(m, "2026-11-19")
        // A trusted ArrowUp is the date input's own step. The handler does not cancel it. The Node test's
        // synthetic keydown never invoked that browser default, so the value stayed put there.
        assert.equal(nativeArrow[1], false)
        assert.equal(nativeArrow[0], "2026-12-19")
        assert.deepEqual(ctrlD, ["2026-12-19", false])
        assert.equal(i, "2026-10-25T10:15")
        assert.equal(shiftH, "2026-10-25T09:15")
        assert.equal(textArrow, "26/10/2026")
        assert.equal(y, "26/10/2027")
        assert.equal(plain, false)
        // The trusted arrow adds the browser's own change on top of the seven the handler dispatches.
        assert.equal(changes, 8)
    },
})
