// Where the server writes a file is not the user's to choose (SEC-PFS-82, SEC-BAP-82): a write at a location with a
// '#' is refused where a reference would cut the location there and write the file before it, a reference of
// `location#suffix` writes the location as given (never the file before the '#'), a blog post's image goes under the
// content root setting with its base name only, and a drive under the resource-facade@drive-root setting, whatever
// the user's own preferences say. The Rust rules are in crates/moqui-core/src/context/resource/tests.rs.
// Run: ./moq test framework --spec ResourceWriteRoots
const W = "org.moqui.resource.WikiServices."
const F = "org.moqui.resource.FileServices."
const TMP = "runtime://tmp/spec-write-roots"
function invoke(name, parameters) {
    try {
        const result = ec.service.sync().name(name).parameters(parameters).call() || {}
        const error = ec.message.hasError() ? ec.message.getErrorsString() : ""
        ec.message.clearErrors()
        return Object.assign({}, result, {error})
    } catch (e) {
        ec.message.clearErrors()
        return {error: String(e.message || e)}
    }
}
const textAt = location => {
    const ref = ec.resource.getLocationReference(location)
    return ref.exists ? ref.getText() : null
}
const thrown = fn => {
    try {
        fn()
        return ""
    } catch (e) {
        return String(e.message || e)
    }
}

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        invoke(W + "create#WikiSpace", {wikiSpaceId: "SPWRROOT", description: "Spec write roots"})
    },
    setup() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
    },
    cleanupSpec() {
        ec.user.internalLoginUser("john.doe")
        for (const key of ["mantle.content.root", "mantle.content.large.root", "moqui.drive.root"]) ec.user.setPreference(key, null)
        const tmp = ec.resource.getLocationReference(TMP)
        if (tmp.exists) ec.resource.makeChangeSet("runtime://tmp").delete("spec-write-roots").commit(null)
    }
})

feature("a write at a location with '#' is refused, or written as given, and the file before the '#' is left as it was", {
    when() {
        ec.resource.getLocationReference(TMP + "/App.xml").putText("<screen/>")
        ec.resource.getLocationReference("dbresource://spec/writeRoots/page.md").putText("one")
        return {
            cut: thrown(() => ec.resource.getLocationReference(TMP + "/App.xml#").putText("replaced")),
            under: thrown(() => ec.resource.getLocationReference(TMP + "/App.xml#/notes.md").putText("replaced")),
            upload: thrown(() => ec.resource.getLocationReference(TMP + "/App.xml#").putUpload(uploadedFile("a.txt", "replaced"))),
            db: thrown(() => ec.resource.getLocationReference("dbresource://spec/writeRoots/page.md#/notes.md").putText("two")),
            screen: textAt(TMP + "/App.xml"),
            page: textAt("dbresource://spec/writeRoots/page.md")
        }
    },
    then({cut, under, upload, screen, page}) {
        cut.includes("'#'") && under.includes("'#'") && upload.includes("'#'")
        screen == "<screen/>"
        page == "one"
    }
})

feature("a blog post's image is kept in the post's folder under the content root, not the user's preference", {
    when() {
        ec.user.setPreference("mantle.content.root", TMP + "/moved")
        const post = invoke(W + "create#WikiBlog", {wikiSpaceId: "SPWRROOT", title: "Write roots",
            blogText: "<p>x</p>", smallImage: uploadedFile("../../x#y.png", "png", "image/png")})
        const created = ec.entity.find("moqui.resource.wiki.WikiBlog").condition("wikiBlogId", post.wikiBlogId).one()
        const updated = invoke(W + "update#WikiBlog", {wikiBlogId: post.wikiBlogId, smallImage: uploadedFile("..", "png", "image/png")})
        const after = ec.entity.find("moqui.resource.wiki.WikiBlog").condition("wikiBlogId", post.wikiBlogId).one()
        ec.user.setPreference("mantle.content.root", null)
        const root = String(ec.factory.getConfValue("mantle@content-root") || "dbresource://mantle/content").replace(/\/+$/, "")
        return {post, updated, root, created: created.smallImageLocation, after: after.smallImageLocation,
            moved: ec.resource.getLocationReference(TMP + "/moved").exists}
    },
    then({post, updated, root, created, after, moved}) {
        post.error == "" && updated.error == ""
        created == root + "/WikiBlog/" + post.wikiBlogId + "/smallImage/x_y.png"
        after == root + "/WikiBlog/" + post.wikiBlogId + "/smallImage/smallImage"
        textAt(created) == "png"
        !moved
    }
})

feature("a drive is made under the drive root setting, not the user's moqui.drive.root preference", {
    when() {
        ec.user.setPreference("moqui.drive.root", "dbresource://WikiSpace/SPWRROOT")
        const preferred = invoke(F + "create#Drive", {driveName: "Write roots"})
        const set = withConf("resource-facade@drive-root", "dbresource://spec/writeRoots/drives", () =>
            invoke(F + "create#Drive", {driveName: "Write roots set"}))
        ec.user.setPreference("moqui.drive.root", null)
        return {preferred, set}
    },
    then({preferred, set}) {
        preferred.error == "" && preferred.rootLocation == "dbresource://drive/shared/" + preferred.driveId
        set.error == "" && set.rootLocation == "dbresource://spec/writeRoots/drives/shared/" + set.driveId
    }
})
