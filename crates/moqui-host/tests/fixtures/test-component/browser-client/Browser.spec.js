// The browser of moqui test-client (crates/moqui-host/tests/client_browser.rs, opt-in because it needs Chromium):
// pages over the gallery screens of the /qapps shell and the login form, roles and names on a fixture of plain HTML,
// fill, click, a dialog, settle, the page problems that fail a feature and the ways to allow them, a screenshot and a
// PDF. "a provoked console error fails the feature" fails on purpose; the test checks its line and its failure files.

spec({})

feature("the login form signs in with fill, click and waitForURL", {
    expect() {
        const page = browser.newPage()
        const res = page.goto("/Login")
        res.status == 200
        page.locator("#login_form [name=username]").fill("john.doe")
        page.getByLabel("Username").value() == "john.doe"
        page.fill("#login_form [name=password]", "moqui")
        page.locator("#login_form").getByRole("button").first().click()
        page.waitForURL((url) => !/\/Login/.test(url))
        !/\/Login/.test(page.url())
    },
})

feature("a gallery dialog opens, takes a value and closes on Escape", {
    expect() {
        const page = browser.newPage({ user: "john.doe" })
        page.goto("/qapps/gallery/Dialog").status == 200
        page.text().includes("Comparison operators")
        page.getByRole("button", { name: "New", exact: true }).click()
        const dialog = page.getByRole("dialog")
        dialog.waitFor()
        dialog.getByText("New enumeration").isVisible()
        dialog.getByLabel("ID").fill("GATE_BROWSER")
        dialog.getByLabel("ID").value() == "GATE_BROWSER"
        dialog.getByLabel("ID").isEditable()
        page.screenshot("gallery-dialog")
        page.keyboard.press("Escape")
        dialog.waitFor({ state: "hidden" })
        page.getByRole("dialog").count() == 0
    },
})

// A user's pages share the spec's one sign-in of the user; when its session ends (a sign-out here), the next page
// signs in again on its first goto.
feature("a sign-out ends the kept sign-in and the next page signs in again", {
    expect() {
        const first = browser.newPage({ user: "john.doe" })
        first.goto("/qapps/gallery/Dialog").status == 200
        first.goto("/Login/logout")
        /\/Login/.test(first.url())
        const page = browser.newPage({ user: "john.doe" })
        page.goto("/qapps/gallery/Dialog").status == 200
        !/\/Login/.test(page.url())
        page.text().includes("Comparison operators")
    },
})

// The locate script's roles and accessible names (plans/archive/ClientTestHarness.md 5.5) on HTML of its own.
feature("roles and accessible names on a fixture", {
    expect() {
        const page = browser.newPage({ user: "john.doe" })
        page.goto("/qapps/gallery/Dialog")
        page.evaluate((html) => { const box = document.createElement("div"); box.id = "fixture"; box.innerHTML = html;
            document.body.prepend(box); return true }, `
            <button aria-label="Close the panel"><i aria-hidden="true">close</i></button>
            <span id="save-label">Save draft</span><div role="button" aria-labelledby="save-label">ignored text</div>
            <button><i class="material-icons" aria-hidden="true">search</i>Find</button>
            <input type="submit" value="Send it">
            <a href="/x">Next page</a><a>no href</a>
            <label for="fx-name">Full name</label><input id="fx-name" type="text" value="Ada">
            <label><input type="checkbox" checked> Subscribe</label>
            <div role="tablist"><div role="tab">First tab</div><div role="tab">Second tab</div></div>
            <div role="menu"><div role="menuitem">Open item</div></div>
            <h2>Fixture heading</h2>
            <p>Some <b>bold</b> text</p>
            <button style="display:none">Hidden button</button>
            <button disabled>Disabled one</button>`)
        const fixture = page.locator("#fixture")
        fixture.getByRole("button", { name: "Close the panel" }).count() == 1
        fixture.getByRole("button", { name: "Save draft" }).count() == 1
        fixture.getByRole("button", { name: "ignored" }).count() == 0
        // an icon's ligature is aria-hidden, so it is not in the name
        fixture.getByRole("button", { name: "Find", exact: true }).count() == 1
        fixture.getByRole("button", { name: "Send it" }).count() == 1
        fixture.getByRole("button", { name: /^send/i }).count() == 1
        fixture.getByRole("button", { name: "Hidden button" }).count() == 0
        fixture.getByRole("button", { name: "Disabled one" }).isEnabled() == false
        fixture.getByRole("link").count() == 1
        fixture.getByRole("link", { name: "next PAGE" }).count() == 1
        fixture.getByRole("link", { name: "next PAGE", exact: true }).count() == 0
        fixture.getByRole("textbox", { name: "Full name" }).value() == "Ada"
        fixture.getByLabel("Full name").value() == "Ada"
        fixture.getByRole("checkbox", { name: "Subscribe" }).count() == 1
        fixture.getByRole("tab").count() == 2
        fixture.getByRole("tab").nth(1).text() == "Second tab"
        fixture.getByRole("tab").last().text() == "Second tab"
        fixture.getByRole("menuitem", { name: "Open item" }).count() == 1
        fixture.getByRole("heading", { name: "Fixture heading" }).count() == 1
        // the deepest element with the text
        fixture.getByText("bold", { exact: true }).evaluate((el) => el.tagName) == "B"
        fixture.getByText("Some bold text").evaluate((el) => el.tagName) == "P"
        fixture.getByText(/fixture HEADING/i).count() == 1
        fixture.locator("p").attribute("id") == null
        fixture.locator("h2").boundingBox().height > 0
        fixture.locator(".nothing-here").count() == 0
        fixture.locator(".nothing-here").isVisible() == false
    },
})

feature("a provoked console error fails the feature", {
    expect() {
        const page = browser.newPage({ user: "john.doe" })
        page.goto("/qapps/gallery/Dialog").status == 200
        page.evaluate(() => { console.error("provoked by the spec"); return true })
    },
})

feature("allowPageProblems lets a console error pass", {
    allowPageProblems: true,
    expect() {
        const page = browser.newPage({ user: "john.doe" })
        page.goto("/qapps/gallery/Dialog")
        page.evaluate(() => { console.error("provoked and allowed"); return true })
        page.problems().some((problem) => problem.includes("provoked and allowed"))
    },
})

feature("allowProblems and takeProblems keep a page problem from failing the feature", {
    expect() {
        const page = browser.newPage({ user: "john.doe" })
        page.goto("/qapps/gallery/Dialog")
        page.allowProblems(/expected noise/)
        page.evaluate(() => { console.error("expected noise"); console.error("taken"); return true })
        page.settle()
        const taken = page.takeProblems()
        taken.length == 1
        taken[0].includes("taken")
        page.problems().length == 0
        // a response of 400 and up is a problem too
        page.evaluate(() => fetch("/apps/no-such-screen-here").then((r) => r.status)) == 404
        page.settle()
        page.takeProblems().some((problem) => problem.includes("404"))
    },
})

feature("a screenshot, a PDF, the viewport and the color scheme", {
    expect() {
        const page = browser.newPage({ user: "john.doe", viewport: { width: 800, height: 600 }, colorScheme: "dark" })
        page.goto("/qapps/gallery/Dialog")
        page.evaluate(() => innerWidth) == 800
        page.evaluate(() => matchMedia("(prefers-color-scheme: dark)").matches) == true
        page.setViewport(1024, 700)
        page.evaluate(() => innerWidth) == 1024
        page.title().length > 0
        page.html().includes("<html")
        page.waitFor(() => document.readyState === "complete") == true
        page.waitFor("document.body.innerText.length") > 0
        page.screenshot("gallery-full", { fullPage: true }).endsWith("gallery-full.png")
        const pdf = page.pdf("gallery", { landscape: true })
        pdf.magic == "%PDF"
        pdf.size > 1000
        pdf.path.endsWith("gallery.pdf")
        page.close()
    },
})
