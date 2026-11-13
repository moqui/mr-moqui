// One gallery sweep in the real /qapps shell at 1920. The affected rows also exercise their keyboard, recovery and
// phone workflows; they are acceptance activities, not another render sweep. Page problems fail their row.
// Run: moqui test-client example --serve --spec GallerySmoke
const user = params.user ?? "john.doe"
const password = params.password ?? "moqui"

// The same names Gallery.spec.js renders, in that order. menu-include=false screens are here only when that spec
// renders them (QuickNote, QuickTask, Chatter, RecordFiles, Search).
const screens = [
    "Landing", "List", "Detail", "Record360", "Task", "Board", "Calendar", "Split", "Settings", "Wizard",
    "Dialog", "Report", "Panel", "Widgets", "Editor", "QuickNote", "QuickTask", "Chatter", "RecordFiles",
    "Message", "Views", "Search", "Handheld",
]

let page

spec({
    setupSpec() {
        login(user, password)
        page = browser.newPage({ user, password, viewport: { width: 1920, height: 1080 }, timeout: 30 })
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
        if (activities[name]) activities[name]()
    },
})

// Keep each activity on the page its existing sweep row opens. Native key events exercise the controls' behavior.
const activities = {
    List() {
        const description = "Gallery saved find " + Date.now()
        page.locator("#EnumFind_description").fill("Equal")
        page.locator("#EnumFind").getByRole("button", { name: "Find", exact: true }).click()
        page.waitFor(() => new URL(location.href).searchParams.get("description") === "Equal")
        page.getByRole("button", { name: "Saved Finds", exact: true }).click()
        page.locator("#EnumList_NewFind_description").fill(description)
        page.getByRole("button", { name: "Save New Find", exact: true }).click()
        page.waitFor(() => {
            const id = new URL(location.href).searchParams.get("formListFindId")
            return id && id !== "_clear"
        })
        const findId = page.evaluate(() => new URL(location.href).searchParams.get("formListFindId"))
        if (page.locator(".m-saved-finds").isVisible()) {
            page.keyboard.press("Escape")
            page.locator(".m-saved-finds").waitFor({ state: "hidden" })
        }
        page.getByRole("button", { name: "Saved Finds", exact: true }).click()
        page.locator(".m-saved-find-list").getByRole("link", { name: description, exact: true }).waitFor()
        const deleteSelector = '.m-saved-find-list .q-item:has(a[href*="formListFindId=' + encodeURIComponent(findId) + '"]) button[name="DeleteFind"]'
        page.locator(deleteSelector).evaluate(element => { window.gallerySavedFindDeleteButton = element; return true })
        // Observe real writes; do not replace confirmation, submission, responses or the server fixture.
        page.evaluate(() => {
            window.gallerySavedFindWrites = []
            window.gallerySavedFindFetch = window.fetch
            window.fetch = function(input, init) {
                const url = typeof input === "string" ? input : input && input.url || String(input)
                const body = init && init.body
                if (/\/formSaveFind(?:\?|$)/.test(url) && body && typeof body.get === "function" && body.get("DeleteFind") === "Y") {
                    window.gallerySavedFindWrites.push(body.get("formListFindId"))
                }
                return window.gallerySavedFindFetch.apply(this, arguments)
            }
            return true
        })
        try {
            page.locator(deleteSelector).press("Enter")
            page.locator(".q-dialog-plugin").waitFor()
            assert(page.locator(".q-dialog-plugin").text().includes(description), "The Material confirmation names the saved find")
            page.locator(".q-dialog-plugin").getByRole("button", { name: "Cancel", exact: true }).click()
            page.waitFor(() => !document.querySelector(".q-dialog-plugin") && document.activeElement === window.gallerySavedFindDeleteButton)
            assert(page.evaluate(() => window.gallerySavedFindWrites.length) === 0, "Cancel sends no delete request")
            assert(page.locator(".m-saved-find-list").getByRole("link", { name: description, exact: true }).count() === 1, "Cancel keeps the saved find")
            page.locator(deleteSelector).press("Enter")
            page.locator(".q-dialog-plugin").waitFor()
            page.keyboard.press("Escape")
            page.waitFor(() => !document.querySelector(".q-dialog-plugin") && document.activeElement === window.gallerySavedFindDeleteButton)
            assert(page.evaluate(() => window.gallerySavedFindWrites.length) === 0, "Escape sends no delete request")
            assert(page.locator(".m-saved-find-list").getByRole("link", { name: description, exact: true }).count() === 1, "Escape keeps the saved find and its parent dialog")
            page.locator(deleteSelector).press("Enter")
            page.locator(".q-dialog-plugin").getByRole("button", { name: "Confirm", exact: true }).press("Enter")
            page.waitFor(() => window.gallerySavedFindWrites.length === 1 && !document.querySelector(".q-dialog-plugin"))
            page.settle()
            assert(JSON.stringify(page.evaluate(() => window.gallerySavedFindWrites)) === JSON.stringify([findId]), "Confirm deletes this find exactly once")
            if (!page.locator(".m-saved-finds").isVisible()) page.getByRole("button", { name: "Saved Finds", exact: true }).click()
            page.locator(".m-saved-finds").waitFor()
            assert(page.locator(".m-saved-find-list").getByRole("link", { name: description, exact: true }).count() === 0, "The deleted saved find is absent after reopening")
            page.keyboard.press("Escape")
        } finally {
            page.evaluate(() => {
                if (typeof window.gallerySavedFindFetch === "function") window.fetch = window.gallerySavedFindFetch
                delete window.gallerySavedFindFetch
                delete window.gallerySavedFindWrites
                delete window.gallerySavedFindDeleteButton
                return true
            })
        }
    },
    Task() {
        page.waitFor(() => !!document.querySelector("#PickStep_scanInput"))
        assert(page.text().includes("line 1 of 7") && page.text().includes("Remaining lines (6)"), "The first step includes its context")
        page.getByRole("button", { name: "Confirm", exact: true }).click()
        page.waitFor(() => document.body.innerText.includes("Line confirmed; the step reloaded"))
        page.getByRole("button", { name: "Short", exact: true }).click()
        page.waitForURL(/lineIndex=1/)
        assert(page.text().includes("Archetype 3.5 Task") && page.text().includes("Remaining lines (5)"), "Short keeps the full Task layout")
        assert(page.evaluate(() => !new URL(location.href).searchParams.has("stepOnly")), "The dynamic step parameter does not escape into navigation")
        page.getByRole("button", { name: "Skip", exact: true }).click()
        page.waitForURL(/lineIndex=2/)
        assert(page.text().includes("line 3 of 7") && page.text().includes("Remaining lines (4)"), "Skip advances with the remaining lines")
        assert(page.evaluate(() => !new URL(location.href).searchParams.has("stepOnly")), "Skip also clears the fragment-only parameter")
    },
    Board() {
        page.waitFor(() => document.querySelectorAll("[data-board-card]").length === 10)
        const card = '[data-board-card="OPP-1001"]'
        assert(page.locator('[data-board-stage="qualify"] ' + card).count() === 1, "Acme starts in Qualify")
        page.getByRole("button", { name: "Move Acme Co. warehouse expansion", exact: true }).press("Enter")
        page.locator(".q-menu .q-item").getByText("Move to Propose", { exact: true }).evaluate(element => {
            element.closest(".q-item").focus()
            return true
        })
        page.keyboard.press("Enter")
        page.waitFor(() => !!document.querySelector('[data-board-stage="propose"] [data-board-card="OPP-1001"]'))
        page.waitFor(() => document.activeElement.id === "BoardMove-OPP-1001")
        assert(page.locator('[data-board-stage="qualify"] [data-board-card]').count() === 2, "Qualify count decreases")
        assert(page.locator('[data-board-stage="propose"] [data-board-card]').count() === 3, "Propose count increases")
        assert(page.text().includes("nothing is saved"), "Demo persistence is explicit")
        page.locator(card + " a").press("Enter")
        page.waitForURL(/\/gallery\/Detail\?opportunityId=OPP-1001/)
    },
    Calendar() {
        page.waitFor(() => document.querySelectorAll('[aria-label^="Create event on "]').length >= 28)
        page.waitFor(() => document.querySelectorAll('.q-page .q-badge[role="img"]').length >= 11)
        assert(page.getByRole("button", { name: "Previous month", exact: true }).count() === 1, "Previous is named")
        assert(page.getByRole("button", { name: "Next month", exact: true }).count() === 1, "Next is named")
        page.getByRole("button", { name: "Next month", exact: true }).press("Enter")
        page.waitFor(() => /^\d{4}-\d{2}$/.test(new URL(location.href).searchParams.get("month") || ""))
        const nextMonth = page.evaluate(() => new URL(location.href).searchParams.get("month"))
        page.getByRole("button", { name: "Previous month", exact: true }).press("Enter")
        assert(page.evaluate(() => new URL(location.href).searchParams.get("month")) !== nextMonth, "Previous changes the bookmark")
        page.getByRole("button", { name: "Today", exact: true }).press("Enter")
        assert(page.evaluate(() => !new URL(location.href).searchParams.has("month")), "Today clears the month override")
        const date = page.locator('[aria-label^="Create event on "]').first().attribute("aria-label").slice(-10)
        page.locator('[aria-label^="Create event on "]').first().press("Enter")
        page.locator(".q-dialog #CalendarTitle").waitFor()
        assert(page.locator(".q-dialog #CalendarStart").value().startsWith(date), "Create prefills the chosen day")
        page.getByRole("button", { name: "Cancel", exact: true }).click()
        page.waitFor(() => !document.querySelector(".q-dialog #CalendarTitle"))
        for (const dark of [false, true]) {
            page.evaluate(async mode => { Quasar.Dark.set(mode); await Vue.nextTick(); return true }, dark)
            const contrasts = page.evaluate(calendarEventContrasts)
            assert(contrasts.length >= 11, "Events are present for contrast measurement")
            assert(contrasts.every(value => value >= 4.5), "Event text contrast in " + (dark ? "dark" : "light") + ": " + contrasts)
        }
        page.evaluate(async () => { Quasar.Dark.set(false); await Vue.nextTick(); return true })
        page.setViewport(390, 844)
        page.waitFor(() => !!document.querySelector('.q-page .q-badge[role="img"][aria-label^="Done:"]'))
        assert(page.evaluate(() => Array.from(document.querySelectorAll('.q-page .q-badge[role="img"]')).every(event =>
            /: .+, \d{4}-\d{2}-\d{2} \d{2}:\d{2} to /.test(event.getAttribute("aria-label")))), "Phone event dots retain full descriptions")
        page.setViewport(1920, 1080)
        // A same-document exit used to send the Calendar watcher to Task/events before Calendar unmounted.
        page.locator("#m-app-screens").click()
        page.locator(".q-menu").getByRole("link", { name: "Task", exact: true }).click()
        page.waitForURL(/\/gallery\/Task(?:\?|$)/)
        page.settle()
        assert(!page.text().includes("screen path not found: events"), "Calendar leaves without a request to the destination's events")
    },
    Split() {
        page.setViewport(1280, 900)
        page.locator('.m-split-side input[name="geoName"]').fill("United")
        page.locator('.m-split-side input[name="geoName"]').press("Enter")
        page.waitFor(() => new URL(location.href).searchParams.get("geoName") === "United")
        const countries = page.evaluate(() => Array.from(document.querySelectorAll(".m-split-side a"))
            .filter(link => new URL(link.href).searchParams.has("geoId")).map(link => link.textContent.trim()))
        assert(countries.length === 4, "The fixture has four matching countries")
        page.setViewport(390, 844)
        page.locator(".m-split-side").getByRole("link", { name: "United States", exact: true }).click()
        page.waitFor(() => new URL(location.href).searchParams.get("geoId") === "USA")
        assert(page.evaluate(() => new URL(location.href).searchParams.get("geoName")) === "United", "Selecting a country retains the find term")
        assert(page.evaluate(() => new URL(location.href).searchParams.get("geoName_op")) === "begins", "Selecting retains the find operator")
        page.getByRole("button", { name: "Countries", exact: true }).click()
        page.waitFor(() => !new URL(location.href).searchParams.has("geoId"))
        assert(page.locator('.m-split-side input[name="geoName"]').value() === "United", "Back keeps the field value")
        const returned = page.evaluate(() => Array.from(document.querySelectorAll(".m-split-side a"))
            .filter(link => new URL(link.href).searchParams.has("geoId")).map(link => link.textContent.trim()))
        assert(JSON.stringify(returned) === JSON.stringify(countries), "Back restores the same filtered country rows")
        page.setViewport(1920, 1080)
    },
    Wizard() {
        page.waitFor(() => !!document.querySelector("#WizardCustomer_customerName"))
        page.setViewport(390, 844)
        const titles = page.evaluate(() => Array.from(document.querySelectorAll(".q-stepper__title")).map(title => ({
            text: title.textContent.trim(), visible: !!title.getClientRects().length && getComputedStyle(title).display !== "none"
        })))
        assert(titles.length === 3 && titles.every(title => title.visible), "All three wizard step titles remain visible on a phone")
        assert(titles.map(title => title.text).join(",") === "Customer,Options,Review", "The step titles retain their context")
        assert(page.locator('.q-stepper__tab[aria-current="step"]').text().includes("Customer"), "The current step is identified")
        const customer = page.locator("#WizardCustomer_customerName").value()
        page.locator("#WizardCustomer_customerName").fill("")
        page.locator(".m-wizard-next").click()
        page.waitFor(() => document.body.innerText.includes("Enter the customer name"))
        // The screen's deliberate validation response currently has status 500. Consume only that expected failure;
        // later saves still fail the feature if they produce a response or console problem.
        const invalidProblems = page.takeProblems()
        assert(invalidProblems.length === 1 && /^response: 500 .*\/apps\/gallery\/Wizard\/Customer\/saveCustomer$/.test(invalidProblems[0]),
            "Only the deliberate empty-customer validation request fails: " + JSON.stringify(invalidProblems))
        assert(page.locator(".m-wizard").attribute("data-step") === "Customer", "Invalid input stays on the step")
        page.locator("#WizardCustomer_customerName").fill(customer)
        page.locator(".m-wizard-next").click()
        page.waitFor(() => document.querySelector(".m-wizard").getAttribute("data-step") === "Options")
        assert(page.locator('.q-stepper__tab[aria-current="step"]').text().includes("Options"), "Next identifies the Options step")
        assert(page.getByRole("button", { name: "Review", exact: true }).count() === 0, "A future step is not navigable")
        page.getByRole("button", { name: "Customer", exact: true }).press("Enter")
        page.waitFor(() => !!document.querySelector("#WizardCustomer_customerName"))
        assert(page.locator("#WizardCustomer_customerName").value() === customer, "Returning to a completed step keeps the saved draft")
        page.setViewport(1920, 1080)
    },
    Dialog() {
        page.getByRole("button", { name: "New", exact: true }).evaluate(element => { window.galleryNewTrigger = element; return true })
        page.getByRole("button", { name: "New", exact: true }).press("Enter")
        page.locator(".q-dialog #NewEnum_enumId").waitFor()
        page.waitFor(() => document.activeElement.id === "NewEnum_enumId")
        assert(page.locator(".q-dialog").text().includes("Comparison Operator"), "The new enumeration starts with its comparison type")
        assert(page.evaluate(() => new FormData(document.querySelector(".q-dialog form")).get("enumTypeId")) === "ComparisonOperator", "The type label matches its submitted key")
        page.keyboard.press("Escape")
        page.waitFor(() => !document.querySelector(".q-dialog #NewEnum_enumId"))
        page.waitFor(() => document.activeElement === window.galleryNewTrigger)
    },
    Editor() {
        const cleanVersionUrl = () => page.evaluate(() => {
            const query = new URL(location.href).searchParams
            return !["versionName", "versionNameLhs", "versionNameRhs"].some(name => query.has(name))
        })
        assert(page.getByRole("heading", { name: "Welcome", exact: true }).count() > 0, "Editor starts on Welcome")
        page.getByRole("link", { name: "Home", exact: true }).click()
        page.waitFor(() => new URL(location.href).searchParams.get("pagePath") === "\u00a0")
        assert(page.url().includes("pagePath=%C2%A0"), "Home has an explicit bookmark distinct from the default page")
        page.getByRole("heading", { name: "Example Wiki", exact: true }).waitFor()
        // This reload is part of the Home bookmark activity, not another sweep row.
        assert(page.goto(page.url()).status === 200, "The Home bookmark reloads")
        page.getByRole("heading", { name: "Example Wiki", exact: true }).waitFor()
        assert(page.evaluate(() => new URL(location.href).searchParams.get("pagePath")) === "\u00a0", "Reload retains Home")
        page.getByRole("button", { name: "Edit", exact: true }).click()
        page.locator(".q-dialog #EditPage_pageText").waitFor()
        assert(page.locator(".q-dialog").getByRole("textbox", { name: "Content", exact: true }).count() === 1, "The Markdown editor has an accessible Content name")
        assert(page.locator("#EditPage_pageText").value().startsWith("# Example Wiki"), "Edit opens the root page's Markdown")
        page.getByRole("button", { name: "Save as a new version", exact: true }).click()
        page.waitFor(() => !document.querySelector(".q-dialog #EditPage_pageText"))
        page.getByRole("heading", { name: "Example Wiki", exact: true }).waitFor()
        assert(page.evaluate(() => new URL(location.href).searchParams.get("pagePath")) === "\u00a0", "Saving unchanged Home returns to Home")
        const unavailableComparison = page.evaluate(() => {
            const url = new URL(location.href)
            url.searchParams.set("versionNameLhs", "missing-version")
            return url.pathname + url.search
        })
        assert(page.goto(unavailableComparison).status === 200, "An unavailable comparison remains a usable page")
        page.waitFor(() => document.querySelector(".q-page").innerText.includes("Comparison unavailable"))
        assert(page.text().includes("Choose versions from History"), "The comparison warning explains recovery")
        assert(page.getByRole("heading", { name: "Example Wiki", exact: true }).count() === 1, "The current Home content remains visible")
        assert(page.getByRole("button", { name: "Edit", exact: true }).isVisible(), "Edit remains available after the bad comparison")
        assert(page.getByRole("button", { name: "History", exact: true }).isVisible(), "History remains available after the bad comparison")
        page.getByRole("link", { name: "Close", exact: true }).click()
        assert(cleanVersionUrl(), "Close clears the unavailable comparison")
        assert(page.evaluate(() => new URL(location.href).searchParams.get("pagePath")) === "\u00a0", "Close preserves Home")
        page.getByRole("heading", { name: "Example Wiki", exact: true }).waitFor()
        page.getByRole("link", { name: "Welcome", exact: true }).click()
        page.getByRole("button", { name: "Edit", exact: true }).click()
        page.locator(".q-dialog #EditPage_pageText").waitFor()
        const marker = "Gallery acceptance review " + Date.now()
        page.locator("#EditPage_pageText").fill(page.locator("#EditPage_pageText").value() + "\n" + marker + "\n")
        page.locator("#EditPage_versionNote").fill(marker)
        page.getByRole("button", { name: "Save as a new version", exact: true }).click()
        page.waitFor(() => !document.querySelector(".q-dialog #EditPage_pageText"))
        page.waitFor(text => document.querySelector(".q-page").innerText.includes(text), { arg: marker })
        page.getByRole("button", { name: "History", exact: true }).click()
        page.locator(".q-dialog").getByRole("button", { name: "Publish", exact: true }).first().waitFor()
        const published = page.locator(".q-dialog").getByRole("button", { name: "Publish", exact: true }).first().evaluate(element => {
            const fields = new FormData(element.form)
            window.galleryPublishButton = element
            const version = fields.get("publishedVersionName")
            return { version, pageId: fields.get("wikiPageId"), shortName: version.length > 12 ? version.substring(0, 10) : version }
        })
        // Watch real requests; cancellation must not submit the hidden form or change the published marker.
        page.evaluate(() => {
            window.galleryPublishWrites = []
            window.galleryPublishFetch = window.fetch
            window.fetch = function(input, init) {
                const url = typeof input === "string" ? input : input && input.url || String(input)
                const body = init && init.body
                if (/\/publishVersion(?:\?|$)/.test(url) && body && typeof body.get === "function") {
                    window.galleryPublishWrites.push({ version: body.get("publishedVersionName"), pageId: body.get("wikiPageId") })
                }
                return window.galleryPublishFetch.apply(this, arguments)
            }
            return true
        })
        try {
            page.locator(".q-dialog").getByRole("button", { name: "Publish", exact: true }).first().press("Enter")
            page.locator(".q-dialog-plugin").waitFor()
            assert(page.locator(".q-dialog-plugin").text().includes("Publish version " + published.shortName + " of Welcome in EXAMPLE?"), "Publish confirmation identifies the page, space and selected version")
            page.locator(".q-dialog-plugin").getByRole("button", { name: "Cancel", exact: true }).click()
            page.waitFor(() => !document.querySelector(".q-dialog-plugin") && document.activeElement === window.galleryPublishButton)
            assert(page.evaluate(() => window.galleryPublishWrites.length) === 0, "Cancel sends no publish request")
            assert(page.evaluate(() => window.galleryPublishButton.isConnected && !window.galleryPublishButton.closest("tr, .tr").innerText.includes("published")), "The cancelled version remains unpublished")
            page.locator(".q-dialog").getByRole("button", { name: "Publish", exact: true }).first().press("Enter")
            page.locator(".q-dialog-plugin").getByRole("button", { name: "Confirm", exact: true }).press("Enter")
            page.waitFor(() => window.galleryPublishWrites.length === 1 && !document.querySelector(".q-dialog-plugin"))
            page.waitFor(shortName => {
                const box = document.querySelector(".q-page")
                return box && box.innerText.includes("· published " + shortName) && !window.moqui.webrootVue.loading
            }, { arg: published.shortName })
            const writes = page.evaluate(() => window.galleryPublishWrites)
            assert(writes.length === 1 && writes[0].version === published.version && writes[0].pageId === published.pageId,
                "Confirm publishes this page's full version ID exactly once: " + JSON.stringify(writes))
        } finally {
            page.evaluate(() => {
                if (typeof window.galleryPublishFetch === "function") window.fetch = window.galleryPublishFetch
                delete window.galleryPublishFetch
                delete window.galleryPublishWrites
                delete window.galleryPublishButton
                return true
            })
        }
        page.getByRole("button", { name: "History", exact: true }).click()
        page.locator(".q-dialog").getByRole("link", { name: published.shortName, exact: true }).waitFor()
        assert(page.locator(".q-dialog").getByRole("link", { name: published.shortName, exact: true }).evaluate(element => {
            const row = element.closest("tr, .tr")
            return row.innerText.includes("published") && !Array.from(row.querySelectorAll("button")).some(button => button.textContent.trim() === "Publish")
        }), "History marks the published version and no longer offers Publish on that row")
        page.locator(".q-dialog").getByRole("link", { name: "prev", exact: true }).first().click()
        page.waitFor(() => !!new URL(location.href).searchParams.get("versionNameLhs"))
        assert(page.text().includes("2 added, 0 removed"), "Appending a paragraph does not replace lines because of browser CRLF endings")
        page.getByRole("link", { name: "Close", exact: true }).click()
        assert(cleanVersionUrl(), "Closing comparison clears its version parameters")
        assert(page.getByRole("link", { name: "Close", exact: true }).count() === 0, "Closing removes the comparison")
        assert(page.evaluate(() => new URL(location.href).searchParams.get("pagePath")) === "Welcome", "Close keeps the page")
        page.getByRole("button", { name: "History", exact: true }).click()
        page.locator(".q-dialog").getByRole("link", { name: "prev", exact: true }).first().click()
        page.waitFor(() => !!new URL(location.href).searchParams.get("versionNameLhs"))
        page.getByRole("link", { name: "Notes", exact: true }).click()
        assert(cleanVersionUrl(), "Changing pages clears the other page's version selection")
        assert(page.evaluate(() => new URL(location.href).searchParams.get("pagePath")) === "Notes", "Notes opens")
        assert(page.getByRole("link", { name: "Close", exact: true }).count() === 0, "Notes has no stale comparison")
        page.getByRole("link", { name: "Welcome", exact: true }).click()
        page.getByRole("button", { name: "History", exact: true }).click()
        page.locator('.q-dialog a[href*="versionName="]').nth(1).click()
        page.waitFor(() => !!new URL(location.href).searchParams.get("versionName"))
        page.getByRole("link", { name: "Back to current", exact: true }).click()
        assert(cleanVersionUrl(), "Returning to current clears the old version")
    },
    Message() {
        page.getByRole("button", { name: "Show images", exact: true }).waitFor()
        page.getByRole("button", { name: "Show images", exact: true }).click()
        page.waitFor(() => {
            const frame = document.querySelector("#DesignedMessage iframe")
            const image = frame && frame.contentDocument && frame.contentDocument.getElementById("remotePicture")
            return image && image.complete && image.naturalWidth > 0
        })
        assert(page.evaluate(() => {
            const image = document.querySelector("#DesignedMessage iframe").contentDocument.getElementById("remotePicture")
            return new URL(image.src).origin === location.origin
        }), "The demo image uses this server's actual scheme and host")
        page.getByRole("button", { name: "Trust this sender", exact: true }).click()
        page.waitFor(() => new URL(location.href).searchParams.get("trusted") === "Y")
        page.waitFor(() => {
            const frame = document.querySelector("#DesignedMessage iframe")
            const image = frame && frame.contentDocument && frame.contentDocument.getElementById("remotePicture")
            return image && image.complete && image.naturalWidth > 0
        })
        assert(page.getByRole("button", { name: "Show images", exact: true }).count() === 0, "A trusted sender does not need Show images")
        page.getByRole("button", { name: "Hide images again", exact: true }).click()
        page.waitFor(() => new URL(location.href).searchParams.get("trusted") === "N")
        page.getByRole("button", { name: "Show images", exact: true }).waitFor()
    }
}

function calendarEventContrasts() {
    const context = document.createElement("canvas").getContext("2d")
    const rgba = color => {
        context.clearRect(0, 0, 1, 1)
        context.fillStyle = color
        context.fillRect(0, 0, 1, 1)
        return Array.from(context.getImageData(0, 0, 1, 1).data)
    }
    const luminance = channels => {
        const linear = channels.slice(0, 3).map(value => {
            const c = value / 255
            return c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4)
        })
        return linear[0] * 0.2126 + linear[1] * 0.7152 + linear[2] * 0.0722
    }
    return Array.from(document.querySelectorAll('.q-page .q-badge[role="img"]')).map(event => {
        const ancestors = []
        for (let element = event; element; element = element.parentElement) ancestors.unshift(element)
        let background = [255, 255, 255]
        for (const element of ancestors) {
            const color = rgba(getComputedStyle(element).backgroundColor)
            const alpha = color[3] / 255
            background = background.map((value, index) => value * (1 - alpha) + color[index] * alpha)
        }
        const foreground = luminance(rgba(getComputedStyle(event).color))
        const behind = luminance(background)
        return (Math.max(foreground, behind) + 0.05) / (Math.min(foreground, behind) + 0.05)
    })
}
