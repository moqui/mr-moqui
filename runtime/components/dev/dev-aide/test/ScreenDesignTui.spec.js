// The screen builder's terminal twin (screen/DevAide/Screens/DesignTui.xml, plans/design-dev-aide.md D14): an intent on
// the selected element is staged on the screen's file and nothing is written, and the twin's one tui render (its own tui
// texts: the tabs, the outline tree, the preview) shows the outline, the element's properties from the schema digest
// and its intents; an upstream screen in extension mode stages its edits in the site's screen-extend, at the element's
// anchor or the nearest one, and the screen's own terminal render from the buffer (moquiPreview=buffer:<id>) shows them;
// Save writes a throw-away root's file. The gallery's QuickNote is the screen designed: its file is staged, then
// discarded, never written; dev-aide stands in as the site, and its screen-extend of it is never saved.
// Run: ./moq test dev-aide --spec ScreenDesignTui
const fixture = () => ec.resource.script("component://dev-aide/test/GuidedFixture.js")
const dev = () => ec.screen.makeTest().rootScreen("component://dev-aide/screen/DevAide.xml")
const ROOT = "dev-aide-guided-screen-spec"
const PAGE = "/qapps/gallery/QuickNote"
const NOTE = "screen:component://example/screen/Gallery/QuickNote.xml"
const noteFile = "component://example/screen/Gallery/QuickNote.xml"
const setting = value => {
    ec.artifactExecution.disableAuthz()
    try {
        if (value) {
            const row = ec.entity.makeValue("moqui.dev.DevAideSetting")
            row.setFields({settingName: "siteComponents", settingValue: value}, true, null, null)
            row.createOrUpdate()
        } else ec.entity.find("moqui.dev.DevAideSetting").condition("settingName", "siteComponents").deleteAll()
    } finally {
        ec.artifactExecution.enableAuthz()
    }
}
const edit = (twin, parameters) => twin.render("Screens/DesignTui/designEdit", Object.assign({page: PAGE}, parameters), "post")
const staged = files =>
    ec.service
        .sync()
        .name("org.moqui.devaide.WorkspaceServices.get#EditBuffer")
        .parameters({target: NOTE, files: JSON.stringify(files || [])})
        .call()

spec({
    setupSpec() {
        login("john.doe")
        fixture().setup(ROOT, {"screen/Fixture.xml": "<screen>\n    <widgets/>\n</screen>\n"})
    },
    cleanupSpec() {
        fixture().discard(NOTE)
        setting("")
        fixture().cleanup(ROOT, ["screen/Fixture.xml"])
    }
})

feature(
    "in place, an intent is staged on the screen's file, and the twin in the terminal shows the outline, properties and intents",
    {
        when() {
            const before = ec.resource.getLocationText(noteFile, false)
            const twin = dev()
            // widgets is element 3: its first label, the conditional label, then the form GalleryNote
            const field = edit(twin, {
                node: "3.2",
                intent: "addField",
                fieldName: "noteTitle",
                fieldTitle: "Title",
                widget: "text-line",
                after: "noteText"
            })
            const property = edit(twin, {
                node: "3.0",
                intent: "setProperty",
                attrName: "text",
                attrValue: "Changed in the terminal"
            })
            const inserted = edit(twin, {node: "3.0", intent: "insert", where: "after", snippet: "label"})
            const view = ec.service
                .sync()
                .name("org.moqui.devaide.ScreenServices.get#ScreenView")
                .parameters({page: PAGE})
                .call().view
            const text = staged([{rootId: view.base.rootId, path: view.base.path}]).files[0].text
            // the label went in after the first: the form is element 3.3 now
            const r = twin.renderMode("tui").render("Screens/DesignTui", {page: PAGE, tab: "design", node: "3.3"}, null)
            const removed = edit(dev(), {node: "3.1", intent: "remove"})
            const afterRemove = staged([{rootId: view.base.rootId, path: view.base.path}]).files[0].text
            fixture().discard(NOTE)
            return {
                before,
                field,
                property,
                inserted,
                text,
                r,
                out: String(r.output),
                removed,
                afterRemove,
                now: ec.resource.getLocationText(noteFile, false)
            }
        },
        then({before, field, property, inserted, text, r, out, removed, afterRemove, now}) {
            field.status == 302
            property.status == 302
            inserted.status == 302
            // the field after noteText with its own default-field, the label's text, and a new label after it, as the browser makes them
            text.includes('<field name="noteTitle"><default-field title="Title"><text-line/></default-field></field>')
            text.includes('text="Changed in the terminal"')
            text.includes('<label text="New label" type="p"/>')
            // the one render: no error; the header and pending line, the tabs, the outline as the tree component, the form's
            // properties from the schema digest, its intents
            r.status == 200
            r.errorMessages.length == 0
            !out.includes("Error rendering")
            !out.includes('"t":"error"')
            out.includes("Edits go to ")
            out.includes("1 unsaved change in QuickNote.xml")
            out.includes('"t":"subscreens"')
            out.includes('"name":"tree"')
            out.includes("form-single GalleryNote")
            out.includes('"NodeProperties"')
            out.includes('"transition"')
            out.includes("Add field")
            out.includes("Insert")
            // Remove stages its removal; Discard drops it all; the file was never written
            removed.status == 302
            !afterRemove.includes("New label")
            now == before
        }
    }
)

feature("an upstream screen in extension mode stages its edits in the site's screen-extend, and its terminal render shows them", {
    when() {
        // the site's own component, and the upstream lock on again (the fixture turned it off for the user)
        setting("dev-aide")
        ec.user.setPreference("DEV_AIDE_UPSTREAM_EDITS", "")
        try {
            const twin = dev()
            const view = ec.service
                .sync()
                .name("org.moqui.devaide.ScreenServices.get#ScreenView")
                .parameters({page: PAGE})
                .call().view
            const anchored = edit(twin, {node: "3.2", intent: "insert", where: "after", snippet: "label"})
            const merged = edit(twin, {
                node: "3.2",
                intent: "addField",
                fieldName: "noteTitle",
                fieldTitle: "Title",
                widget: "text-line",
                after: "noteText"
            })
            const nearest = edit(twin, {node: "3.0", intent: "insert", where: "after", snippet: "link"})
            const refused = edit(twin, {node: "3.0", intent: "remove"})
            const buffer = staged([{rootId: view.extension.rootId, path: view.extension.path}])
            const preview = ec.screen
                .makeTest()
                .rootScreen("component://example/screen/Gallery.xml")
                .renderMode("tui")
                .render("QuickNote", {moquiPreview: "buffer:" + buffer.bufferId}, null)
            const onDisk = ec.resource
                .getLocationReference("component://dev-aide/" + view.extension.path.split("/dev-aide/").pop())
                .getExists()
            return {view, anchored, merged, nearest, refused, buffer, preview, out: String(preview.output), onDisk}
        } finally {
            fixture().discard(NOTE)
            setting("")
            ec.user.setPreference("DEV_AIDE_UPSTREAM_EDITS", "true")
        }
    },
    then({view, anchored, merged, nearest, refused, buffer, preview, out, onDisk}) {
        !view.inPlace
        view.extension.path.endsWith("screen-extend/example/screen/Gallery/QuickNote.xml")
        anchored.status == 302
        merged.status == 302
        nearest.status == 302
        refused.errorMessages.some(text => text.includes("for a screen edited in place"))
        buffer.files[0].missing
        buffer.files[0].text.includes(
            '<widgets-extend name="GalleryNote" where="after"><label text="New label" type="p"/></widgets-extend>'
        )
        buffer.files[0].text.includes('<form-single name="GalleryNote"><field name="noteTitle" after="noteText">')
        // the first label has no name: the link goes before the nearest anchor, the form
        buffer.files[0].text.includes(
            '<widgets-extend name="GalleryNote" where="before"><link url="." text="New link" btn-type="default"/></widgets-extend>'
        )
        // the overlay applies to the terminal render, the screen-extend that is not on disk yet included
        preview.status == 200
        out.includes("New label")
        out.includes("New link")
        out.includes("noteTitle")
        !onDisk
    }
})

feature("Save writes a throw-away root's staged screen file", {
    when() {
        const f = fixture()
        f.discard(NOTE)
        const disk = staged([{rootId: ROOT, path: "screen/Fixture.xml"}]).files[0]
        ec.service
            .sync()
            .name("org.moqui.devaide.WorkspaceServices.apply#EditBufferOps")
            .parameters({
                target: NOTE,
                rootId: ROOT,
                path: "screen/Fixture.xml",
                ops: JSON.stringify([{op: "insert", at: [0], position: "last", xml: '<label text="Saved from the terminal"/>'}])
            })
            .call()
        f.guard(NOTE, ROOT)
        const saved = dev().render("Screens/DesignTui/saveBuffer", {page: PAGE}, "post")
        return {disk, saved, text: f.read(ROOT, "screen/Fixture.xml"), pending: staged().pending}
    },
    then({disk, saved, text, pending}) {
        !disk.staged
        saved.status == 302
        text == '<screen>\n    <widgets>\n        <label text="Saved from the terminal"/>\n    </widgets>\n</screen>\n'
        pending.count == 0
    }
})
