// Who may co-edit a file (WorkspaceServices.get#CoeditAccess, plans/design-dev-aide.md F15), which a file's room in
// moqui-collab asks as the user (its rooms, bounds and seeding are moqui-collab's text_tests.rs): only on a shared dev
// server; a writer of the file as Save would allow, with its text in LF and the bounds; the upstream lock refuses as
// Save does; a path that is not the file's own name in the root is refused; and the answer is never for another user.
// On a throw-away root (test/GuidedFixture.js).
// Run: ./moq test dev-aide --spec CoeditAccess
const fixture = () => ec.resource.script("component://dev-aide/test/GuidedFixture.js")
const ROOT = "dev-aide-coedit-spec"
const FILES = {"service/a/Things.xml": "<services>\r\n    <service verb=\"get\" noun=\"Thing\"/>\r\n</services>\r\n"}
const access = parameters => {
    try {
        const out =
            ec.service
                .sync()
                .name("org.moqui.devaide.WorkspaceServices.get#CoeditAccess")
                .parameters(Object.assign({rootId: ROOT, path: "service/a/Things.xml", userId: ec.user.userId, withText: true}, parameters))
                .call() || {}
        const errors = ec.message.getErrorsString()
        ec.message.clearErrors()
        return {out, errors}
    } catch (error) {
        ec.message.clearErrors()
        return {out: {}, errors: String(error.message)}
    }
}

spec({
    setupSpec() {
        login("john.doe")
        fixture().setup(ROOT, FILES)
    },
    cleanupSpec() {
        fixture().cleanup(ROOT, Object.keys(FILES))
    }
})

feature("a writer co-edits a file only on a shared server, with its text in LF and the bounds", {
    when() {
        const local = access({})
        setConf("dev-aide@shared", "true")
        const shared = access({})
        const noText = access({withText: false})
        return {local, shared, noText}
    },
    then({local, shared, noText}) {
        local.errors == ""
        local.out.canEdit == false
        local.out.reason.includes("shared")
        local.out.text == null
        shared.errors == ""
        shared.out.canEdit == true
        shared.out.text == "<services>\n    <service verb=\"get\" noun=\"Thing\"/>\n</services>\n"
        shared.out.maxBytes == 2097152
        shared.out.maxRooms == 20
        shared.out.maxTotalBytes == 67108864
        noText.out.canEdit == true
        noText.out.text == null
    }
})

feature("the upstream lock, a path not named as the root names it, and another user's answer are refused", {
    when() {
        setConf("dev-aide@shared", "true")
        ec.user.setPreference("DEV_AIDE_UPSTREAM_EDITS", "false")
        let locked
        try {
            locked = access({})
        } finally {
            ec.user.setPreference("DEV_AIDE_UPSTREAM_EDITS", "true")
        }
        const roundabout = access({path: "service/a/../a/Things.xml"})
        const missing = access({path: "service/a/Nothing.xml"})
        const another = access({userId: "EX_JOHN_DOE_OTHER"})
        return {locked, roundabout, missing, another}
    },
    then({locked, roundabout, missing, another}) {
        locked.out.canEdit == false
        locked.out.reason.includes("upstream")
        locked.out.text == null
        roundabout.out.canEdit == false
        missing.out.canEdit == false
        another.errors.includes("for the user only")
    }
})
