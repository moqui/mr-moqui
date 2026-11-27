/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// MapsDialogWalk (plans/archive/Maps.md §3.2, §9): the local-map dialog in a browser on the qvt shell, at desktop and phone width.
// Saved views' View map opens it as an app opens it beside an address, for a plain point (the saved view's place): its
// label and coordinates show, Escape closes it and the focus is back on View map; on a phone the dialog fills the screen.
// A record the user may write (MapsWalkFixtures: maps.walk and the test adapter's LOCATED): its place, source and Open in
// Maps, a click on the map fills Choose location's latitude and longitude, and Save location saves it through the
// adapter, on LocalMap's own page and in a dialog over Saved views (the row's dialog loading LocalMap for the record, as an
// app's View map would), where the dialog closes and the page behind it reloads in place. A record the user may not read
// (DENIED, maps.walk.plain with ALL_USERS alone): not available, nothing of it, and no Open in Maps.
// Run: ./moq accept tools --spec MapsDialogWalk
spec({timeout: 120})

const password = "MapsWalk1!!"
function page(user, extra) {
    const opened = browser.newPage(Object.assign({user, password}, extra || {}))
    // the shell asks for the user's photo and their calls, which a user without Comms is refused
    opened.allowProblems(/40[34] [A-Za-z ]+ .*\/(userPhoto\?userId=\w+|activeCalls)$/)
    return opened
}
const record = recordId =>
    "/qapps/maps/LocalMap?adapterKey=MAPS_WALK_ADAPTER&recordKey=" + encodeURIComponent(JSON.stringify({recordId}))

// Saved views, with the fixture's Walk harbor (-87.6, 41.89) and its View map
function viewsPage(extra) {
    const views = page("maps.walk", extra)
    views.goto("/qapps/maps/Views").status == 200
    views.waitFor(() => document.body.textContent.includes("Walk harbor"))
    return views
}
function openViews(extra) {
    const views = viewsPage(extra)
    // the row's own View map: the nearest box around the view's link that holds one
    views.evaluate(() => {
        const button = box => Array.from(box.querySelectorAll("button")).find(one => one.textContent.includes("View map"))
        let box = Array.from(document.querySelectorAll("a")).find(link => link.textContent.trim() === "Walk harbor")
        while (box && !button(box)) box = box.parentElement
        button(box).click()
        return true
    })
    views.waitFor(() => {
        const dialog = document.querySelector(".q-dialog")
        return dialog && dialog.textContent.includes("41.89000, -87.60000")
    })
    return views
}

feature("desktop: View map opens the dialog over the page; Escape closes it and the focus is back on View map", {
    expect() {
        const views = openViews()
        views.evaluate(() => document.querySelector(".q-dialog").textContent.includes("Walk harbor"))
        views.waitFor(() => !!document.querySelector(".q-dialog #LocalMapView"))
        views.press("Escape")
        views.waitFor(() => !document.querySelector(".q-dialog"))
        views.waitFor(() => ((document.activeElement && document.activeElement.textContent) || "").includes("View map"))
    }
})

feature("desktop: a record the user may write: its place, Open in Maps, and Save location from a click on the map", {
    expect() {
        const writer = page("maps.walk")
        // the dialog's form names its dialog, which a page of its own does not have
        writer.allowProblems(/Container with ID LocalMapDialog not found/)
        writer.goto(record("LOCATED")).status == 200
        writer.waitFor(() => document.body.textContent.includes("Spec place"))
        writer.text().includes("41.87810, -87.62980")
        writer.text().includes("Spec data set")
        writer.getByRole("link", {name: "Open in Maps"}).count() >= 1
        writer.waitFor(() => !!document.querySelector("#LocalMapView .ol-viewport, #LocalMapView .m-map-fallback"), {timeout: 60})
        // a click on the map fills the form's latitude and longitude (pick-form)
        const drawnMap = writer.evaluate(() => !!document.querySelector("#LocalMapView .ol-viewport"))
        if (drawnMap) {
            // the map is on the record: moved off it from the keyboard, then Enter chooses its center
            writer.locator("#LocalMapView .m-map-canvas").focus()
            writer.press("ArrowRight")
            writer.press("ArrowRight")
            writer.press("Enter")
            writer.waitFor(() => {
                const input = document.querySelector('form[name="ChooseLocation"] input[name="latitude"]')
                const other = document.querySelector('form[name="ChooseLocation"] input[name="longitude"]')
                return input && other && other.value && other.value !== "-87.629800"
            })
        } else {
            writer.fill('form[name="ChooseLocation"] input[name="latitude"]', "41.9")
            writer.fill('form[name="ChooseLocation"] input[name="longitude"]', "-87.6")
        }
        writer.getByRole("button", {name: "Save location"}).click()
        writer.waitFor(() => document.body.textContent.includes("The location is saved"))
    }
})

feature("desktop: Save location in a dialog over Saved views closes the dialog and reloads the page behind it in place", {
    expect() {
        const views = viewsPage()
        // the row's dialog (ViewMap_MAPS_WALK_HARBOR) loads LocalMap for the record, with its own id, as an app's View map would
        const dialogId = views.evaluate(() =>
            Object.keys(moqui.webrootVue.activeContainers).find(id => id.startsWith("ViewMap_MAPS_WALK_HARBOR"))
        )
        dialogId != null
        views.evaluate(
            ([id, query]) => {
                const dialog = moqui.webrootVue.activeContainers[id]
                dialog.isShown = true
                // the dialog's own screen URL, with the record in place of the saved view's point
                dialog.$nextTick(() => dialog.load(dialog.url.split("?")[0] + "?" + query))
                // a mark on the page behind, which a reload of it takes away
                const link = Array.from(document.querySelectorAll("a")).find(one => one.textContent.trim() === "Walk harbor")
                link.setAttribute("data-walk-mark", "before")
                return true
            },
            [dialogId, record("LOCATED").split("?")[1] + "&dialogId=" + encodeURIComponent(dialogId)]
        )
        views.waitFor(() => {
            const input = document.querySelector('.q-dialog form[name="ChooseLocation"] input[name="latitude"]')
            return input && input.value === "41.878100"
        })
        views.fill('.q-dialog form[name="ChooseLocation"] input[name="latitude"]', "41.91")
        views.fill('.q-dialog form[name="ChooseLocation"] input[name="longitude"]', "-87.62")
        views.evaluate(() => {
            const dialog = document.querySelector(".q-dialog")
            Array.from(dialog.querySelectorAll("button"))
                .find(button => button.textContent.includes("Save location"))
                .click()
            return true
        })
        views.waitFor(() => document.body.textContent.includes("The location is saved"))
        views.waitFor(() => !document.querySelector(".q-dialog"))
        // the page behind is Saved views still, drawn again
        views.waitFor(() => !document.querySelector("[data-walk-mark]") && document.body.textContent.includes("Walk harbor"))
        views.url().split("?")[0].endsWith("/qapps/maps/Views")
    }
})

feature("a record the user may not read is not available, with nothing of it, and no way into the Maps app", {
    expect() {
        const plain = page("maps.walk.plain")
        plain.goto(record("DENIED")).status == 200
        plain.waitFor(() => document.body.textContent.includes("This record's place is not available"))
        !plain.text().includes("Spec place")
        plain.getByRole("button", {name: "Save location"}).count() == 0
        plain.getByRole("link", {name: "Open in Maps"}).count() == 0
        // and the app itself is refused
        plain.goto("/qapps/maps/Browse")
        plain.takeProblems()
        !plain.text().includes("Search saved places and records")
    }
})

feature("phone width: the dialog fills the screen", {
    expect() {
        const views = openViews({viewport: {width: 390, height: 844}})
        // once the dialog has finished opening
        views.waitFor(() => {
            const box = document.querySelector(".q-dialog .q-card").getBoundingClientRect()
            return box.width >= document.documentElement.clientWidth - 2
        })
        const card = views.locator(".q-dialog .q-card").boundingBox()
        card.width >= views.evaluate(() => document.documentElement.clientWidth) - 2
        card.height >= views.evaluate(() => window.innerHeight) - 2
    }
})
