// The screen archetype gallery (screen/Gallery.xml): every gallery screen renders in its render mode without errors
// and with real content. The screens are the fixture of the IronStone BOSS style guide
// (runtime/components/addon/mr-boss/plan/style-guide.md §10); a placeholder or a render error fails here.
// Run: ./moq test example
let screens

// name under the Gallery root, and the render mode the screen is written for (qvue screens answer <path>.qvue)
const galleryScreens = [
    ["Landing", "qvt"], ["List", "qvt"], ["Detail", "qvt"], ["Record360", "qvt"], ["Task", "qvt"],
    ["Board", "qvue"], ["Calendar", "qvue"], ["Split", "qvt"], ["Settings", "qvt"], ["Wizard", "qvt"],
    ["Dialog", "qvt"], ["Report", "qvt"], ["Panel", "qvt"], ["Widgets", "qvt"], ["Editor", "qvt"], ["QuickNote", "qvt"], ["QuickTask", "qvue"],
    ["Chatter", "qvt"], ["RecordFiles", "qvt"], ["Message", "qvt"], ["Views", "qvt"], ["Search", "qvt"], ["Handheld", "qvt"],
    ["NavCards", "qvt"],
]

const queryParameters = url => Object.fromEntries(String(url || "").replace(/&amp;/g, "&").split("?").slice(1).join("?")
    .split("&").filter(Boolean).map(part => {
        const at = part.indexOf("=")
        return [decodeURIComponent(at < 0 ? part : part.slice(0, at)), decodeURIComponent(at < 0 ? "" : part.slice(at + 1))]
    }))

spec({
    setupSpec() {
        login("john.doe")
        screens = ec.screen.makeTest().rootScreen("component://example/screen/Gallery.xml")
    },
    cleanupSpec() {
        log.info(`Rendered ${screens.renderCount} gallery screens (${screens.errorCount} errors) in ${(Date.now() - screens.startTime) / 1000}s`)
    },
    setup() { ec.artifactExecution.disableAuthz() },
    cleanup() { ec.artifactExecution.enableAuthz() },
})

feature("the gallery root lists every archetype screen", {
    when() {
        return screens.render("", {}, null)
    },
    then(str) {
        str.status == 200
        str.errorMessages.length == 0
    },
})

feature("gallery screen #name renders in #mode without errors", {
    where: galleryScreens.map(([name, mode]) => ({ name, mode })),
    expect({ name, mode }) {
        const test = ec.screen.makeTest().rootScreen("component://example/screen/Gallery.xml").renderMode(mode)
        // the screen as the shell fetches it: a fragment of the last standalone screen, not the root placeholder
        const parameters = {lastStandalone: "-2"}
        // Reuse Editor's sweep render for explicit Home and an unavailable comparison, with its recovery controls.
        if (name === "Editor") Object.assign(parameters, {pagePath: null, versionNameLhs: "missing-version"})
        const str = test.render(name, parameters, null)
        log.info(`Rendered gallery ${name} (${mode}) in ${str.renderTime}ms, ${str.output?.length} characters`)
        str.status == 200
        str.errorMessages.length == 0
        str.output.length > 200
        !str.output.includes("is not written yet")
        !/Error rendering|Exception in render/.test(str.output)
        if (name === "Editor") {
            assert(str.output.includes('Example Wiki'), "an explicit empty path opens the root page")
            assert(str.output.includes('Comparison unavailable'), "an unavailable version is a local warning")
            assert(str.output.includes('Choose versions from History'), "the warning explains recovery")
            assert(str.output.includes('id="EditPage"'), "the page remains editable after a comparison failure")
            const close = Array.from(str.output.matchAll(/<m-link\b[^>]*href="([^"]+)"[^>]*>([\s\S]*?)<\/m-link>/g))
                .find(match => match[2].replace(/<[^>]*>/g, "").trim() === "Close")
            assert(close, "the unavailable comparison has a Close link")
            const query = queryParameters(close[1])
            assert.equal(query.pagePath, "\u00a0", "Close retains the explicit root page")
            assert.equal(Object.hasOwn(query, "versionNameLhs"), false)
            const root = ec.resource.getLocationReference('dbresource://ExampleWiki/EXAMPLE.md')
            const answer = test.header("Accept", "application/json").render("Editor/savePage",
                {wikiSpaceId: "EXAMPLE", pagePath: "", pageText: root.getText()}, "post")
            assert.equal(answer.errorMessages.length, 0)
            assert.equal(queryParameters(JSON.parse(answer.output).screenUrl).pagePath, "\u00a0",
                "saving the unchanged Home page returns to Home")
        }
        if (name === "Dialog") {
            const form = /<m-form\b[^>]*\bid="NewEnum"[^>]*>/.exec(str.output)
            assert(form && form[0].includes("'enumTypeId':'ComparisonOperator'"), "New starts with the comparison operator type")
        }
        if (name === "Task") {
            // The existing page render is enough; inspect these transition answers without rendering another page.
            test.header("Accept", "application/json")
            for (const [action, lineIndex] of [["short", 0], ["skip", 1]]) {
                const answer = test.render("Task/" + action, {stepOnly: "Y", lineIndex, lineSeq: lineIndex + 1}, "post")
                assert.equal(answer.errorMessages.length, 0)
                const query = queryParameters(JSON.parse(answer.output).screenUrl)
                assert.equal(query.lineIndex, String(lineIndex + 1))
                assert.equal(Object.hasOwn(query, "stepOnly"), false, action + " returns to the whole page")
            }
        }
    },
})

// Every gallery screen is also a terminal screen (plans/archive/Screen-TUI-Impl.md 2.8): its tui render is one JSON document, and
// no section of it failed. A qvue screen with no tui text yet answers its placeholder, which is not an error.
const tuiErrors = (nodes, found) => {
    for (const node of nodes || []) {
        if (!node || typeof node !== "object") continue
        if (node.t === "error") found.push(node.text)
        tuiErrors(node.c, found)
        if (node.t === "formList") {
            tuiErrors(node.headerFields, found)
            for (const row of node.rows || []) tuiErrors(row.cells, found)
        }
    }
    return found
}

feature("gallery screen #name renders in tui as one document without errors", {
    where: galleryScreens.map(([name]) => ({ name })),
    expect({ name }) {
        const test = ec.screen.makeTest().rootScreen("component://example/screen/Gallery.xml").renderMode("tui")
        // The local TUI may carry the literal sentinel rather than HTTP's parsed null value.
        const parameters = name === "Editor" ? {pagePath: "\u00a0", versionNameLhs: "missing-version"} : {}
        const str = test.render(name, parameters, null)
        const doc = JSON.parse(str.output)
        const errors = tuiErrors(doc.c, [])
        if (errors.length) log.warn(`Gallery ${name} tui errors: ${errors.join(" | ")}`)
        str.status == 200
        doc.v == 1
        errors.length == 0
        str.errorMessages.length == 0
        if (name === "Editor") {
            assert(str.output.includes("Example Wiki"), "Home is reachable in the terminal")
            assert(str.output.includes("Comparison unavailable"), "the terminal also retains comparison recovery")
            assert(str.output.includes("Choose versions from History"))
        }
    },
})

// The Handheld screen (density="handheld", scan-input): the scan fields over the screen's scanResolve transition, which runs
// the gallery resolver with the field's allowed types: one match, two (the chooser), none, a GS1 code read into its product and
// lot with the parsed fields, and the put-away field that takes bins only.
feature("the Handheld screen's scan fields resolve through its scanResolve transition", {
    expect() {
        const gallery = () => ec.screen.makeTest().rootScreen("component://example/screen/Gallery.xml").renderMode("qvt")
        const page = gallery().render("Handheld", { lastStandalone: "-2" }, null)
        const scan = (form, field, text) => {
            const str = gallery().render("Handheld/scanResolve", { scanForm: form, scanField: field, scanText: text, scanId: "G-1",
                facilityId: "ZIRET_WH" }, "post")
            return JSON.parse(str.output)
        }
        const one = scan("ScanAnything", "itemId", "a-100")
        const two = scan("ScanAnything", "itemId", "DUP-1")
        const none = scan("ScanAnything", "itemId", "NOPE")
        const gs1 = scan("ScanAnything", "itemId", "(01)04006381333931(10)LOT-7")
        const bin = scan("PutAway", "binId", "DUP-1")
        const binOk = scan("PutAway", "binId", "B-01-07")
        page.status == 200
        page.output.includes('<m-scan-input name="itemId" id="ScanAnything_itemId"')
        page.output.includes("'scanForm':'ScanAnything','scanField':'itemId','facilityId':'ZIRET_WH'")
        page.output.includes('scan-id-field="binScanId"')
        one.matches.length == 1
        one.matches[0].id == "A-100"
        one.scanId == "G-1"
        two.matches.map((item) => item.identityType).join(",") == "asset,lot"
        none.matches.length == 0
        gs1.matches.map((item) => item.id).join(",") == "LOT-7,WIDGET-RED-L"
        gs1.parsed.applicationIdentifiers["10"] == "LOT-7"
        bin.matches.length == 0
        binOk.matches[0].detail == "Reserve · ZIRET_WH"
    },
})

// The Landing counters are number cards in a dashboard, each a link into the List with its type filter.
feature("the Landing counters are number cards in a dashboard that link to the filtered list", {
    expect() {
        const test = ec.screen.makeTest().rootScreen("component://example/screen/Gallery.xml").renderMode("qvt")
        const str = test.render("Landing", { lastStandalone: "-2" }, null)
        const cards = str.output.match(/<m-number-card /g) || []
        const linked = /<m-number-card title="[^"]+" value="[0-9,]+" href="\/[^"]*List\?enumTypeId=[A-Za-z]+"/.test(str.output)
        str.status == 200
        str.output.includes('<div class="m-dashboard q-mx-sm" style="--m-dashboard-min:260px">')
        str.output.includes('m-dashboard-each')
        str.output.includes('<m-number-card title="All enumerations"')
        cards.length == 6
        linked
        !str.output.includes("text-overline text-grey-6")
    },
})

// The gallery's shell-providers (Gallery.xml) as the shell gets them from the shellData transition of the root screen, and its
// palette source run by shellSearch; the Detail screen's context is an m-screen-context with the record.
feature("the shell data has the gallery's providers, and its palette source finds enumerations", {
    expect() {
        const webroot = ec.screen.makeTest().rootScreen("component://webroot/screen/webroot.xml")
        const data = webroot.render("shellData", {}, null)
        const apps = JSON.parse(data.output).apps
        log.info(`Shell data: ${apps.length} apps (${apps.map((app) => app.name).join(", ")}) in ${data.renderTime}ms`)
        const gallery = apps.find((app) => app.name === "gallery")
        const search = webroot.render("shellSearch", { text: "Quasar", groups: "GalleryEnumerations" }, null)
        const groups = JSON.parse(search.output).groups
        const detail = ec.screen.makeTest().rootScreen("component://example/screen/Gallery.xml").renderMode("qvt")
            .render("Detail", { lastStandalone: "-2", enumId: "STT_INTERNAL_QUASAR" }, null)
        data.status == 200
        data.errorMessages.length == 0
        gallery.providers.quickCreate[0].url == "/qapps/gallery/QuickNote"
        gallery.providers.quickCreate[1].url == "/qapps/gallery/QuickTask"
        gallery.providers.paletteActions[0].url == "/qapps/gallery/Widgets"
        gallery.providers.paletteSources[0].group == "GalleryEnumerations"
        gallery.providers.paletteSources[0].service === undefined
        gallery.providers.keymaps[0].keys[0].url == "/qapps/gallery/Widgets"
        gallery.menu.some((item) => item.path == "/qapps/gallery/Widgets" && item.depth == 1)
        search.status == 200
        groups.length == 1
        groups[0].rows.length > 0
        groups[0].rows[0].url.startsWith("/qapps/gallery/Detail?enumId=")
        groups[0].moreUrl == "/qapps/gallery/List?description=Quasar"
        detail.output.includes('<m-screen-context record-entity="moqui.basic.Enumeration" record-id="STT_INTERNAL_QUASAR"')
        detail.output.includes('<m-chatter id="DetailChatter" record-entity="moqui.basic.Enumeration" record-id="STT_INTERNAL_QUASAR" default-tab="log">')
        detail.output.includes('<m-record-files id="DetailFiles" record-entity="moqui.basic.Enumeration" record-id="STT_INTERNAL_QUASAR"')
        // every app root gets its record panels; the gallery mounts its own Chatter and RecordFiles under the names a theme may give
        apps.every((app) => app.recordScreens && "chatter" in app.recordScreens && "recordFiles" in app.recordScreens)
    },
})

// The gallery's Chatter panel, as the chatter element loads it: the record and the hints from the query, the record card's
// transition, and the Log note form with the light rich-text editor whose @ mentions search users. Its note service cleans the
// editor's HTML with allow-html="safe", which keeps the mention span, and reads the mentions back.
feature("the gallery's Chatter panel has the rich-text note form, its card and mention transitions, and keeps a mention", {
    expect() {
        const gallery = () => ec.screen.makeTest().rootScreen("component://example/screen/Gallery.xml").renderMode("qvt")
        const record = { recordEntity: "moqui.basic.Enumeration", recordId: "STT_INTERNAL_QUASAR", appRoot: "gallery", defaultTab: "log" }
        const panel = gallery().render("Chatter", Object.assign({ lastStandalone: "-2" }, record), null)
        const card = JSON.parse(gallery().render("Chatter/recordCard", record, null).output)
        const mentions = JSON.parse(gallery().render("Chatter/mentionSearch", { term: "john" }, null).output)
        const html = '<p>Hi <span data-mention-id="EX_JOHN_DOE">@John Doe</span> <strong>now</strong><script>x()</script></p>'
        const cleaned = ec.service.sync().name("example.ExampleServices.clean#GalleryNote").parameters({ noteText: html }).call()
        panel.status == 200
        panel.errorMessages.length == 0
        panel.output.includes('editor-type="rich-light"')
        panel.output.match(/mention-url="[^"]*\/Chatter\/mentionSearch"/) !== null
        panel.output.includes("<m-record-card ")
        panel.output.includes("opens on log")
        card.title == "Internal Applications - Quasar"
        card.status.label == "Active"
        mentions.some((row) => row.id == "EX_JOHN_DOE" && row.label == "John Doe")
        cleaned.noteText == '<p>Hi <span data-mention-id="EX_JOHN_DOE">@John Doe</span> <strong>now</strong></p>'
        cleaned.mentionIds.length == 1
        cleaned.mentionIds[0] == "EX_JOHN_DOE"
    },
})

// The Split archetype is a container-split whose selection is the geoId of the URL, and the side list keeps its find.
feature("the Split is a container-split with the country in the URL", {
    expect() {
        const test = ec.screen.makeTest().rootScreen("component://example/screen/Gallery.xml").renderMode("qvt")
        const find = {geoName: "C", geoName_op: "begins", geoName_not: "N", geoName_ic: "Y", orderByField: "-geoName"}
        const str = test.render("Split", Object.assign({lastStandalone: "-2", geoId: "CAN"}, find), null)
        str.status == 200
        str.errorMessages.length == 0
        str.output.includes('<m-container-split id="CountrySplit" side-width="320" min-main="480" select-parameter="geoId" back-text="Countries">')
        str.output.includes("<template v-slot:side>")
        str.output.includes("<template v-slot:main>")
        str.output.includes("Canada")
        // the find narrows the side list, the selection does not
        str.output.includes("Cameroon")
        !str.output.includes("Albania")
        const selectedLink = Array.from(str.output.matchAll(/<m-link\b[^>]*href="([^"]+)"[^>]*>([\s\S]*?)<\/m-link>/g))
            .find(match => match[2].replace(/<[^>]*>/g, "").trim() === "Canada")
        const selection = selectedLink && queryParameters(selectedLink[1])
        assert(selection, "the country list links to Canada")
        for (const name of Object.keys(find)) assert.equal(selection[name], find[name], "country link retains " + name)
        const edited = test.header("Accept", "application/json")
            .render("Split/editCountry", Object.assign({geoId: "CAN"}, find), "post")
        assert.equal(edited.errorMessages.length, 0)
        const returned = queryParameters(JSON.parse(edited.output).screenUrl)
        assert.equal(returned.geoId, "CAN")
        for (const name of Object.keys(find)) assert.equal(returned[name], find[name], "Edit retains " + name)
    },
})

// The Wizard archetype: subscreens-panel type="wizard" over three step screens, each of whose forms posts to its own
// transition. A step's answer with errors keeps the wizard on it; the draft the steps keep reaches the review, and the review's
// answer names the first step again.
feature("the Wizard's steps check their entries, keep a draft and end on the first step", {
    expect() {
        const wizard = (path, parameters, method) => ec.screen.makeTest().rootScreen("component://example/screen/Gallery.xml").renderMode("qvt")
            .render("Wizard" + (path ? "/" + path : ""), Object.assign({ lastStandalone: "-2" }, parameters || {}), method || null)
        const panel = ec.screen.makeTest().rootScreen("component://example/screen/Gallery.xml").renderMode("qvt").render("Wizard", { lastStandalone: "-1" }, null)
        const empty = wizard("Customer/saveCustomer", { customerName: "" }, "post")
        const saved = wizard("Customer/saveCustomer", { customerName: "Spec Customer", emailAddress: "spec@example.com" }, "post")
        const options = wizard("Options/saveOptions", { countryGeoId: "CAN", currencyUomId: "CAD", newsletter: "Y" }, "post")
        const review = wizard("Review", {}, null)
        const created = wizard("Review/createCustomer", {}, "post")
        const after = wizard("Review", {}, null)
        panel.status == 200
        panel.output.includes('<m-subscreens-wizard id="CustomerWizard" finish-text="Create customer"></m-subscreens-wizard>')
        empty.errorMessages.join(" ").includes("Enter the customer name")
        saved.errorMessages.length == 0
        options.errorMessages.length == 0
        review.output.includes("Spec Customer")
        review.output.includes("Canada")
        created.errorMessages.length == 0
        !after.output.includes("Spec Customer")
    },
})

// The Editor archetype writes into its demo wiki spaces: a save makes a version, compare renders the server diff,
// an old version renders by location#version, and a restore makes the old text the newest version. Each storage:
// EXAMPLE (dbresource history) and EXGIT (a git repository in the database, made by the screen's createGitSpace).
const editor = (path, parameters, method) => {
    const test = ec.screen.makeTest().rootScreen("component://example/screen/Gallery.xml").renderMode("qvt")
    if (method === "post") test.header("Accept", "application/json")
    return test.render("Editor" + (path ? "/" + path : ""), Object.assign({ lastStandalone: "-2" }, parameters || {}), method || null)
}
const pageText = (space, path) => ec.resource.getLocationReference(
    ec.entity.find("moqui.resource.wiki.WikiSpace").condition("wikiSpaceId", space).one().rootPageLocation
        .replace(/\.md$/, "") + "/" + path + ".md")

feature("the editor's #space space keeps versions, compares, views and restores them", {
    where: [{ space: "EXAMPLE" }, { space: "EXGIT", requires: ["git"] }],
    expect({ space }) {
        if (space === "EXGIT" && !ec.entity.find("moqui.resource.wiki.WikiSpace").condition("wikiSpaceId", "EXGIT").one())
            editor("createGitSpace", {}, "post")
        const page = pageText(space, "Welcome")
        // Reproduce an older browser multipart save without rewriting resource history in production.
        const originalText = page.getText().replace(/\r\n?/g, "\n")
        page.putText(originalText.replace(/\n/g, "\r\n"), "Legacy browser line endings fixture")
        const beforeHistory = page.getVersionHistory(), before = beforeHistory.length
        const stale = {versionName:beforeHistory[0].versionName, versionNameLhs:beforeHistory[0].versionName,
            versionNameRhs:beforeHistory[0].versionName}
        const savedText = originalText + "\nSaved by the gallery spec.\n"
        const saved = editor("savePage", Object.assign({ wikiSpaceId: space, pagePath: "Welcome",
            pageText: savedText.replace(/\n/g, "\r\n").replace(/\r\n$/, "\r") }, stale), "post")
        const writtenText = page.getText()
        const history = page.getVersionHistory()
        const newest = history[0].versionName, previous = history[1].versionName
        const compared = editor("", { wikiSpaceId: space, pagePath: "Welcome", versionName: newest,
            versionNameLhs: previous, versionNameRhs: newest })
        const viewed = editor("", { wikiSpaceId: space, pagePath: "Welcome", versionName: previous })
        // The old page still renders its text; the added paragraph is only in the Edit dialog's current text.
        const oldLine = page.getText(previous).split("\n").map((line) => line.replace(/^[-#\s]+/, "").trim())
            .find((line) => line.length > 10 && !line.includes("*"))
        const restoredAnswer = editor("restoreVersion", { wikiSpaceId: space, pagePath: "Welcome", versionName: previous,
            versionNameLhs: previous, versionNameRhs: newest }, "post")
        const restored = page.getVersionHistory()
        saved.errorMessages.length == 0
        writtenText == savedText
        history.length == before + 1
        compared.status == 200
        compared.errorMessages.length == 0
        compared.output.includes("moqui-diff")
        compared.output.includes("Saved by the gallery spec.")
        compared.output.includes("2 added, 0 removed")
        viewed.status == 200
        viewed.output.includes("not the current text")
        viewed.output.includes(oldLine)
        !viewed.output.includes("<p>Saved by the gallery spec.</p>")
        restored.length == before + 2
        page.getText() == page.getText(previous)
        // Reuse the existing comparison and old-version renders: their links must leave stale history behind.
        const links = output => Array.from(output.matchAll(/<m-link\b[^>]*href="([^"]+)"[^>]*>([\s\S]*?)<\/m-link>/g),
            match => ({query:queryParameters(match[1]), text:match[2].replace(/<[^>]*>/g, "").trim()}))
        const currentPage = query => {
            assert.equal(query.wikiSpaceId, space)
            assert.equal(query.pagePath, "Welcome")
            for (const name of ["versionName", "versionNameLhs", "versionNameRhs"])
                assert.equal(Object.hasOwn(query, name), false, "current page clears " + name)
        }
        currentPage(queryParameters(JSON.parse(saved.output).screenUrl))
        assert.equal(restoredAnswer.errorMessages.length, 0)
        currentPage(queryParameters(JSON.parse(restoredAnswer.output).screenUrl))
        const comparedLinks = links(compared.output), viewedLinks = links(viewed.output)
        const close = comparedLinks.find(link => link.text === "Close")
        assert(close, "the comparison has a Close link")
        currentPage(close.query)
        const back = viewedLinks.find(link => link.text === "Back to current")
        assert(back, "an old version has a return to current link")
        currentPage(back.query)
        const version = comparedLinks.find(link => link.query.versionName === previous && !link.text.includes("Restore"))
        assert(version, "history links to the previous version")
        assert.equal(Object.hasOwn(version.query, "versionNameLhs"), false)
        assert.equal(Object.hasOwn(version.query, "versionNameRhs"), false)
        const withCurrent = viewedLinks.find(link => link.text === "Compare with current")
        assert(withCurrent, "the old version can be compared with current")
        assert.equal(withCurrent.query.versionNameLhs, previous)
        assert.equal(Object.hasOwn(withCurrent.query, "versionNameRhs"), false)
        const otherSpace = comparedLinks.find(link => link.text === "dbresource: EXAMPLE")
        assert(otherSpace, "space navigation is available while comparing")
        assert.equal(otherSpace.query.wikiSpaceId, "EXAMPLE")
        for (const name of ["pagePath", "versionName", "versionNameLhs", "versionNameRhs"])
            assert.equal(Object.hasOwn(otherSpace.query, name), false, "space selection clears " + name)
        if (space === "EXAMPLE") {
            const home = comparedLinks.find(link => link.text === "Home")
            assert(home, "the page list links to Home")
            assert.equal(home.query.pagePath, "\u00a0", "Home has an explicit null page path")
            assert.equal(Object.hasOwn(home.query, "versionNameLhs"), false)
            const notes = comparedLinks.find(link => link.text === "Notes")
            assert(notes, "the Notes page is available while comparing")
            assert.equal(notes.query.pagePath, "Notes")
            for (const name of ["versionName", "versionNameLhs", "versionNameRhs"])
                assert.equal(Object.hasOwn(notes.query, name), false, "page selection clears " + name)
        }
    },
})

// The Detail's status-bar over the framework's EntitySync flow: the stages with the current one, the transitions from it (the next
// stage the primary action, Error the danger one), and setStatus answering the screen with the new status; the root screen's
// statusBar transition answers the same flow for a server-static screen.
feature("the Detail's status-bar shows the EntitySync flow and its transition moves the status", {
    expect() {
        const gallery = () => ec.screen.makeTest().rootScreen("component://example/screen/Gallery.xml").renderMode("qvt")
        const detail = gallery().render("Detail", { lastStandalone: "-2", enumId: "STT_INTERNAL_QUASAR", syncStatusId: "EsRunning" }, null)
        const bar = (detail.output.match(/<m-status-bar id="DetailStatus"[^>]*>/) || [""])[0]
        const moved = gallery().render("Detail/setStatus", { enumId: "STT_INTERNAL_QUASAR", statusId: "EsComplete" }, "post")
        const webroot = ec.screen.makeTest().rootScreen("component://webroot/screen/webroot.xml")
        const flow = JSON.parse(webroot.render("statusBar", { statusId: "EsRunning" }, null).output)
        const colors = ec.service.sync().name("org.moqui.ui.StatusServices.get#StatusColors")
            .parameter("statusIdList", ["EsNotStarted", "EsRunning", "EsComplete", "EsOtherError"]).call().colors
        detail.status == 200
        detail.errorMessages.length == 0
        bar.includes("'statusId':'EsRunning','label':'Running','color':'primary','state':'current'")
        bar.includes("'statusId':'EsNotStarted','label':'Not Started','color':'grey-7','state':'done'")
        bar.includes("'toStatusId':'EsComplete','label':'Complete','color':'positive','isNext':true")
        bar.includes("'toStatusId':'EsOtherError','label':'Error','color':'negative','isNext':false,'isDanger':true")
        bar.includes("/Detail/setStatus\"")
        bar.includes("'enumId':'STT_INTERNAL_QUASAR'")
        moved.errorMessages.length == 0
        flow.current.statusId == "EsRunning"
        flow.stages.map((stage) => stage.statusId).join(",") == "EsNotStarted,EsRunning,EsComplete"
        flow.next.toStatusId == "EsComplete"
        colors.EsNotStarted == "grey-7" && colors.EsRunning == "primary" && colors.EsComplete == "positive" && colors.EsOtherError == "negative"
    },
})

// The Widgets list whose cells hold a section: the section's dialog and the links after it are the row's (in-cell buttons and
// text links with the row index), the filter chips take their btn-type from an expression, and an editable is a select, a date
// picker or an empty value with its placeholder.
feature("the Widgets list cells keep their row after a section, and chips and editables take expressions", {
    expect() {
        const test = ec.screen.makeTest().rootScreen("component://example/screen/Gallery.xml").renderMode("qvt")
        const str = test.render("Widgets", { lastStandalone: "-2", chip: "mine" }, null)
        str.status == 200
        str.errorMessages.length == 0
        str.output.includes('<m-container-dialog id="RefuseDialog_0"')
        str.output.includes('<m-container-dialog id="RefuseDialog_2"')
        !str.output.includes('<m-container-dialog id="RefuseDialog_1"')
        str.output.includes('id="ApprovalList_open_1_openDetail"')
        str.output.includes('id="ApprovalList_open_2_openDetail"')
        str.output.includes('color="primary" class="m-link q-mr-xs">Mine</q-btn>')
        str.output.includes('class="m-btn-tonal text-primary m-link q-mr-xs">All</q-btn>')
        str.output.includes('id="WidgetEditableKind"') && str.output.includes('widget-type="select"') && str.output.includes('value="b"')
        str.output.includes('widget-type="date"')
        str.output.includes('id="ApprovalNote_1"') && str.output.includes('placeholder="Click to edit"')
        str.output.includes('<div class="q-px-xs col-md-6 col-sm-12 col-xs-12">')
    },
})

// The List views screen (style-guide §8.2): one form-list of the example records as a board by status (every Example status in
// its flow's order, with labels, colors and the flow) and a calendar; its transitions move a status through the flow (a move
// the flow does not allow is refused with the entity-auto message), move an example's times and create one.
feature("the Views board and calendar show the examples, and their transitions move a status, refuse one and move times", {
    // the refused move leaves its error in the messages
    allowMessageErrors: true,
    expect() {
        const gallery = () => ec.screen.makeTest().rootScreen("component://example/screen/Gallery.xml").renderMode("qvt")
        const str = gallery().render("Views", { lastStandalone: "-2" }, null)
        const tag = (str.output.match(/<m-list-views id="ExampleList_views"[^>]*>/) || [""])[0]
        const cards = str.output.match(/<template v-slot:card_\d+>/g) || []
        const before = ec.entity.find("moqui.example.Example").condition("exampleId", "EX_102").one()
        const moved = gallery().render("Views/updateExampleStatus", { exampleId: "EX_102", statusId: "EXST_APPROVED" }, "post")
        const afterMove = ec.entity.find("moqui.example.Example").condition("exampleId", "EX_102").useCache(false).one()
        const refused = gallery().render("Views/updateExampleStatus", { exampleId: "EX_100", statusId: "EXST_COMPLETE" }, "post")
        const afterRefused = ec.entity.find("moqui.example.Example").condition("exampleId", "EX_100").useCache(false).one()
        const start = Date.UTC(2026, 9, 21, 15, 0)
        const timed = gallery().render("Views/updateExampleTimes", { exampleId: "EX_103", exampleDate: String(start), exampleEndDate: String(start + 3600000),
            exampleTypeEnumId: "EXT_REAL_WORLD" }, "post")
        const afterTimes = ec.entity.find("moqui.example.Example").condition("exampleId", "EX_103").useCache(false).one()
        const created = gallery().render("Views/createExample", { exampleName: "Spec example", exampleDate: "2026-10-24 09:00", exampleEndDate: "2026-10-24 10:00" }, "post")
        const made = ec.entity.find("moqui.example.Example").condition("exampleName", "Spec example").useCache(false).list()
        const webroot = ec.screen.makeTest().rootScreen("component://webroot/screen/webroot.xml")
        const info = JSON.parse(webroot.render("listViewColumns", { values: "EXST_DEFINED,EXT_MADE_UP", status: "true" }, null).output)
        str.status == 200
        str.errorMessages.length == 0
        tag.includes(":views=\"['board','list','calendar']\"")
        tag.includes("'groupField':'statusId'")
        tag.includes("{'value':'EXST_IN_DESIGN','label':'In Design','color':'grey-7'},{'value':'EXST_DEFINED','label':'Defined','color':'info'}")
        tag.includes("{'value':'EXST_CANCELLED','label':'Cancelled','color':'negative'}")
        tag.includes("'flow':{'EXST_APPROVED':['EXST_CANCELLED','EXST_IMPLEMENTED']")
        tag.includes("/Views/updateExampleStatus'")
        tag.includes("/Views/createExample'")
        tag.includes("'colors':{'EXST_")
        tag.includes("'exampleTypeEnumId':'EXT_REAL_WORLD','exampleTypeEnumId_display':'Real World'")
        cards.length >= 11
        before.statusId == "EXST_DEFINED"
        moved.errorMessages.length == 0
        afterMove.statusId == "EXST_APPROVED"
        refused.errorMessages.some((message) => message.includes("Status change not allowed from In Design to Complete"))
        afterRefused.statusId == "EXST_IN_DESIGN"
        timed.errorMessages.length == 0
        Number(afterTimes.exampleDate) == start || String(afterTimes.exampleDate) == String(start)
        afterTimes.exampleTypeEnumId == "EXT_REAL_WORLD"
        created.errorMessages.length == 0
        made.length == 1 && made[0].statusId == "EXST_IN_DESIGN" && made[0].allDay == "N"
        info.columns.map((column) => column.value).join(",") == "EXST_IN_DESIGN,EXST_DEFINED,EXST_APPROVED,EXST_IMPLEMENTED,EXST_TESTED,EXST_COMPLETE,EXST_CANCELLED,EXT_MADE_UP"
        info.columns[7].label == "Made Up" && info.columns[7].color == null
        info.transitions.EXST_IN_DESIGN.join(",") == "EXST_CANCELLED,EXST_DEFINED"
        info.colors.EXST_COMPLETE == "positive"
    },
    cleanup() {
        ec.entity.find("moqui.example.Example").condition("exampleName", "Spec example").deleteAll()
    },
})

// The Views screen's board groups by status or type with all rows, its list peeks into ExampleDetail, which renders compact with
// peek=true and has the time since the start; the work breakdown is a tree whose levels come through the form request and whose
// roll-ups get#GalleryWorkItems computes.
feature("the Views board groups by type, its list peeks into ExampleDetail, and the work breakdown is a tree with roll-ups", {
    expect() {
        const gallery = () => ec.screen.makeTest().rootScreen("component://example/screen/Gallery.xml").renderMode("qvt")
        const views = gallery().render("Views", { lastStandalone: "-2" }, null)
        const level = JSON.parse(gallery().render("Views/actions/WorkTree", { parentItemId: "W-4" }, null).output)
        const ancestors = JSON.parse(gallery().render("Views/workItemAncestors", { ids: "W-8,W-2" }, null).output)
        const found = ec.service.sync().name("example.ExampleServices.get#GalleryWorkItems").parameters({ itemName: "synonyms" }).call()
        const top = ec.service.sync().name("example.ExampleServices.get#GalleryWorkItems").parameters({}).call()
        const compact = gallery().render("ExampleDetail", { lastStandalone: "-2", exampleId: "EX_104", peek: "true" }, null)
        const full = gallery().render("ExampleDetail", { lastStandalone: "-2", exampleId: "EX_104" }, null)
        views.status == 200
        views.errorMessages.length == 0
        views.output.includes("'groups':[{'field':'statusId'")
        views.output.includes("{'field':'exampleTypeEnumId','label':'Type'")
        views.output.includes("'allRows':true")
        views.output.match(/:peek="\{'url':'[^']*\/ExampleDetail','parameters':\{\},'keyField':'exampleId'\}"/) !== null
        views.output.includes(`<m-list-views id="WorkTree_views"`)
        views.output.includes("'parentField':'parentItemId'")
        views.output.includes('<template v-slot:tree_0="mqTree">')
        level.workItemList.map((row) => row.itemId).join(",") == "W-9,W-11"
        level.workItemList[1].childCount == 3
        level.workItemList[1].totalEstimate == 14
        level.workItemList[1].doneOfTotal == "0/3"
        ancestors.workItemList.map((row) => row.itemId).join(",") == "W-8,W-2"
        found.workItemList.length == 1
        found.workItemList[0].parentItemId == "W-15"
        top.workItemList.map((row) => row.itemId).join(",") == "W-1,W-2,W-3"
        top.workItemList[0].totalEstimate == 59
        top.workItemList[0].doneOfTotal == "2/14"
        compact.status == 200
        compact.errorMessages.length == 0
        compact.output.includes("<m-elapsed ")
        !compact.output.includes("Back to the views")
        full.output.includes("Back to the views")
    },
})
