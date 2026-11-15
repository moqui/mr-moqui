// The service writer's terminal twin (screen/DevAide/Services/EditTui.xml, plans/design-dev-aide.md D14): an edit is
// staged in the user's edit buffer and nothing is written until Save; Discard drops it; Save writes the staged files of
// a throw-away root (test/GuidedFixture.js), and a file changed on disk since is a conflict with "Save mine over it"
// and "Take the disk's"; Try in sim through the twin; and the twin's one tui render (its own tui texts: the tabs, the
// pending line), with the Try's fields and result. The real service file is only staged, then discarded.
// Run: ./moq test dev-aide --spec ServiceEditTui
const fixture = () => ec.resource.script("component://dev-aide/test/GuidedFixture.js")
const dev = () => ec.screen.makeTest().rootScreen("component://dev-aide/screen/DevAide.xml")
const ROOT = "dev-aide-guided-service-spec"
const GREET = "service:example.ExampleServices.greet#WithTool"
const CREATE = "service:create#moqui.example.Example"
const exampleFile = "component://example/service/example/ExampleServices.xml"

spec({
    setupSpec() {
        login("john.doe")
        fixture().setup(ROOT, {"service/fixture/Things.xml": "<services>\n    <service verb=\"get\" noun=\"Thing\"/>\n</services>\n"})
        // the Try runs as the user: let ADMIN create examples
        ec.artifactExecution.disableAuthz()
        try {
            const make = (entity, fields) => { const row = ec.entity.makeValue(entity); row.setFields(fields, true, null, null); row.createOrUpdate() }
            make("moqui.security.ArtifactGroup", {artifactGroupId: "SPEC_DEV_GUIDED", description: "Examples to try in the terminal"})
            make("moqui.security.ArtifactGroupMember", {artifactGroupId: "SPEC_DEV_GUIDED", artifactName: "moqui.example.Example", artifactTypeEnumId: "AT_ENTITY",
                nameIsPattern: "N", inheritAuthz: "Y"})
            make("moqui.security.ArtifactGroupMember", {artifactGroupId: "SPEC_DEV_GUIDED", artifactName: "create#moqui.example.Example", artifactTypeEnumId: "AT_SERVICE",
                nameIsPattern: "N", inheritAuthz: "Y"})
            make("moqui.security.ArtifactAuthz", {artifactAuthzId: "SPEC_DEV_GUIDED", userGroupId: "ADMIN", artifactGroupId: "SPEC_DEV_GUIDED",
                authzTypeEnumId: "AUTHZT_ALWAYS", authzActionEnumId: "AUTHZA_ALL"})
        } finally {
            ec.artifactExecution.enableAuthz()
        }
    },
    cleanupSpec() {
        fixture().discard(GREET)
        fixture().discard(CREATE)
        fixture().cleanup(ROOT, ["service/fixture/Things.xml", "entity/FixtureEntities.xml"])
    },
})

feature("an edit is staged in the buffer and not written, and Discard drops it", {
    when() {
        const before = ec.resource.getLocationText(exampleFile, false)
        const twin = dev()
        const added = twin.render("Services/EditTui/addParameter", {artifact: GREET, which: "in-parameters", parameterName: "specNote",
            parameterType: "String", description: "A note"}, "post")
        const settings = twin.render("Services/EditTui/updateSettings", {artifact: GREET, authenticate: "anonymous-view", allowRemote: "", transactionType: "",
            transactionTimeout: ""}, "post")
        const view = ec.service.sync().name("org.moqui.devaide.ServiceWriterServices.get#ServiceView").parameters({serviceName: GREET.slice(8)}).call().view
        const file = {rootId: view.artifact.rootId, path: view.artifact.relativePath}
        const staged = fixture().staged(GREET, [file])
        const between = ec.resource.getLocationText(exampleFile, false)
        twin.render("Services/EditTui/discardBuffer", {artifact: GREET}, "post")
        const after = fixture().staged(GREET, [file])
        return {before, added, settings, staged, between, after, now: ec.resource.getLocationText(exampleFile, false)}
    },
    then({before, added, settings, staged, between, after, now}) {
        added.status == 302
        settings.status == 302
        staged.pending.count == 1
        staged.files[0].staged
        staged.files[0].text.includes("<parameter name=\"specNote\"><description>A note</description></parameter>")
        staged.files[0].text.includes("authenticate=\"anonymous-view\"")
        between == before
        after.pending.count == 0
        !after.files[0].staged
        now == before
    }
})

feature("Save writes the staged files, and a file changed on disk since is a conflict to save over or take", {
    when() {
        const f = fixture()
        const twin = dev()
        const things = "service/fixture/Things.xml"
        f.discard(CREATE)
        const disk = f.staged(CREATE, [{rootId: ROOT, path: things}]).files[0]
        f.stage(CREATE, ROOT, things, disk.text.replace("noun=\"Thing\"/>", "noun=\"Thing\"><actions/></service>"), disk.baseHash)
        f.stage(CREATE, ROOT, "entity/FixtureEntities.xml", "<entities/>\n", "")
        f.guard(CREATE, ROOT)
        const saved = twin.render("Services/EditTui/saveBuffer", {artifact: CREATE}, "post")
        const written = {things: f.read(ROOT, things), entities: f.read(ROOT, "entity/FixtureEntities.xml"), pending: f.staged(CREATE).pending.count}
        // staged at the text just written, then changed on disk by someone else
        const base = f.staged(CREATE, [{rootId: ROOT, path: things}]).files[0].baseHash
        f.stage(CREATE, ROOT, things, "<services><mine/></services>\n", base)
        f.write(ROOT, things, "<services><theirs/></services>\n")
        f.guard(CREATE, ROOT)
        twin.render("Services/EditTui/saveBuffer", {artifact: CREATE}, "post")
        const left = JSON.parse(ec.web.sessionAttributes["DevAideGuided." + CREATE] || "{}")
        const unchanged = f.read(ROOT, things)
        f.guard(CREATE, ROOT)
        twin.render("Services/EditTui/saveMine", {artifact: CREATE, rootId: ROOT, path: things}, "post")
        const mine = f.read(ROOT, things)
        // and taking the disk's: staged again, then the disk's kept
        const again = f.staged(CREATE, [{rootId: ROOT, path: things}]).files[0]
        f.stage(CREATE, ROOT, things, "<services><other/></services>\n", again.baseHash)
        twin.render("Services/EditTui/takeDisk", {artifact: CREATE, rootId: ROOT, path: things}, "post")
        return {saved, written, left, unchanged, mine, taken: f.staged(CREATE).pending.count, disk: f.read(ROOT, things)}
    },
    then({saved, written, left, unchanged, mine, taken, disk}) {
        saved.status == 302
        written.things.includes("<actions/></service>")
        written.entities == "<entities/>\n"
        written.pending == 0
        left.conflict.path == "service/fixture/Things.xml"
        left.conflict.diff.includes("-<services><theirs/></services>")
        left.conflict.diff.includes("+<services><mine/></services>")
        unchanged == "<services><theirs/></services>\n"
        mine == "<services><mine/></services>\n"
        taken == 0
        disk == mine
    }
})

feature("the twin in the terminal: its header with the pending line, its tabs, Try's fields and a Try's result in sim", {
    when() {
        const twin = dev()
        const tried = twin.render("Services/EditTui/runTry", {artifact: CREATE, exampleName: "Tried in the terminal", exampleTypeEnumId: "EXT_MADE_UP",
            runSim: "runSim"}, "post")
        fixture().stage(CREATE, ROOT, "service/fixture/Things.xml", "<services><staged/></services>\n",
            fixture().staged(CREATE, [{rootId: ROOT, path: "service/fixture/Things.xml"}]).files[0].baseHash)
        const r = twin.renderMode("tui").render("Services/EditTui", {artifact: CREATE}, null)
        const kept = ec.entity.find("moqui.example.Example").condition("exampleName", "Tried in the terminal").disableAuthz().count()
        fixture().discard(CREATE)
        return {tried, r, out: String(r.output), kept}
    },
    then({tried, r, out, kept}) {
        tried.status == 302
        r.status == 200
        r.errorMessages.length == 0
        !out.includes("Error rendering")
        !out.includes("\"t\":\"error\"")
        // the header: the service and its pending line, with Save and Discard
        out.includes("\"text\":\"create#moqui.example.Example\"")
        out.includes("1 unsaved change in Things.xml")
        out.includes("\"PendingSave\"")
        out.includes("/Services/EditTui/discardBuffer")
        // the tabs, Try active
        out.includes("\"t\":\"subscreens\"")
        out.includes("\"title\":\"Try\"")
        // Try: a field per in-parameter, Run in sim and Run for real (with its confirmation)
        out.includes("\"TryForm.exampleName\"")
        out.includes("\"TryForm.runSim\"")
        out.includes("for real? What it writes is kept.")
        // the result of the Try: in sim, the row it would write, nothing kept
        out.includes("In sim, nothing kept")
        out.includes("\"TryRows\"")
        out.includes("moqui.example.Example")
        kept == 0
    }
})
