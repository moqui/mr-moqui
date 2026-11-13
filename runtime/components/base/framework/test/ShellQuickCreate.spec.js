// A quick-create kind with a condition-service (xml-screen-3.xsd quick-create; docs/ui/Qvt-Client.md "Shell data and
// providers"): the shell data offers it only where its service answers offer true for the session's user, run as the app root
// that declares it, and leaves the kinds without a condition as they are. The gallery's galleryTask is offered unless the user
// turned it off (example.ExampleServices.offer#GalleryQuickCreate). A spec-local user who opens the gallery alone keeps the shell
// data small. A failing or missing condition service, which hides only its own kind and is logged, is cargo's (moqui-core
// screen::tests::shell).
// Run: ./moq test framework --spec ShellQuickCreate
const WEBROOT = "component://webroot/screen/webroot.xml"
const USER = "SHELL_QC_SPEC"

function galleryKinds() {
    const out = ec.screen.makeTest().rootScreen(WEBROOT).render("shellData", {}, null)
    const errors = Array.from(out.errorMessages || []).join("\n")
    ec.message.clearErrors()
    const shell = JSON.parse(String(out.output || "{}"))
    const gallery = Array.from(shell.apps || []).find(app => app.name === "gallery")
    return {errors, found: !!gallery, kinds: Array.from(gallery?.providers?.quickCreate || []).map(entry => entry.kind)}
}

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        call("create#moqui.security.UserGroup", {userGroupId: USER, description: "Shell quick create spec"})
        call("create#moqui.security.UserAccount", {userId: USER, username: "shell.qc.spec", userFullName: "Shell Spec"})
        call("create#moqui.security.UserGroupMember", {userGroupId: USER, userId: USER, fromDate: ec.user.nowTimestamp})
        // the gallery alone (its own group is ADMIN's)
        call("create#moqui.security.ArtifactAuthz", {
            artifactAuthzId: USER,
            userGroupId: USER,
            artifactGroupId: "EXAMPLE_GALLERY",
            authzTypeEnumId: "AUTHZT_ALLOW",
            authzActionEnumId: "AUTHZA_ALL"
        })
        ec.artifactExecution.enableAuthz()
        ec.user.internalLoginUser("shell.qc.spec")
    },
    cleanupSpec() {
        ec.user.internalLoginUser("john.doe")
    }
})

feature("a quick create with a condition is offered only where its service says so; the others stay", {
    when() {
        ec.user.setPreference("GALLERY_QUICK_TASK", "Y")
        const offered = galleryKinds()
        ec.user.setPreference("GALLERY_QUICK_TASK", "N")
        const withheld = galleryKinds()
        return {offered, withheld}
    },
    then({offered, withheld}) {
        offered.found && offered.errors == ""
        offered.kinds.includes("galleryNote") && offered.kinds.includes("galleryTask")
        withheld.errors == ""
        withheld.kinds.includes("galleryNote") && !withheld.kinds.includes("galleryTask")
    }
})
