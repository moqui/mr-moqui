// Where the server writes a file is not the user's to choose (SEC-PFS-82, SEC-BAP-82): a write at a location with a
// '#' is refused on every storage (a reference or a read stops at the '#', so the write would change the file before it,
// or on the database tree write a file never found), a reference of `location#suffix` writes the location as given and
// derives no child, an uploaded file's name has its '#' made '_', Files refuses a name with '#', a blog post's image
// goes under the content root setting with its base name only, and a drive under the resource-facade@drive-root
// setting, whatever the user's own preferences say. The Rust rules are in crates/moqui-core/src/context/resource/tests.rs.
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
            dbName: thrown(() => ec.resource.getLocationReference("dbresource://spec/writeRoots/Invoice #12.pdf").putText("two")),
            child: thrown(() => ec.resource.getLocationReference(TMP + "/App.xml#").getChild("notes.md")),
            uploadName: uploadedFile("../C# notes?.md", "x").getName(),
            screen: textAt(TMP + "/App.xml"),
            page: textAt("dbresource://spec/writeRoots/page.md")
        }
    },
    then({cut, under, upload, db, dbName, child, uploadName, screen, page}) {
        cut.includes("'#'") && under.includes("'#'") && upload.includes("'#'")
        db.includes("'#'") && dbName.includes("'#'") && child.includes("'#'")
        uploadName == "C_ notes_.md"
        textAt("dbresource://spec/writeRoots/Invoice #12.pdf") == null
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
        // Files refuses a name with '#' before anything is written, and a path with one reaches no file
        invoke(F + "save#Text", {driveId: preferred.driveId, path: "C.md", text: "C"})
        const folder = invoke(F + "create#Folder", {driveId: preferred.driveId, path: "", name: "C# docs"})
        const renamed = invoke(F + "rename#Resource", {driveId: preferred.driveId, path: "C.md", newName: "C# notes.md"})
        const saved = invoke(F + "save#Text", {driveId: preferred.driveId, path: "C# notes.md", text: "x"})
        const versions = invoke(F + "get#Versions", {driveId: preferred.driveId, path: "C.md#"})
        return {preferred, set, folder, renamed, saved, versions}
    },
    then({preferred, set, folder, renamed, saved, versions}) {
        preferred.error == "" && preferred.rootLocation == "dbresource://drive/shared/" + preferred.driveId
        set.error == "" && set.rootLocation == "dbresource://spec/writeRoots/drives/shared/" + set.driveId
        folder.error.includes("# in it") && renamed.error.includes("# in it") && saved.error.includes("# in it")
        versions.error.includes("not found")
    }
})
