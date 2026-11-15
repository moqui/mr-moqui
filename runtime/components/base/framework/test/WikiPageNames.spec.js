// A wiki page's name (org.moqui.resource.WikiServices update#WikiPage, move#WikiPage and clone#WikiPage): letters and
// digits of any script, spaces and . , ' - _ : ( ) ! ? only, as Knowledge's (mr-boss 628669dc). The name is one segment of
// the page's file location, in the database and in a git repository, so a slash, . or .., a control or direction
// character and a name over 240 bytes are refused; the other rules of the wiki are tested in moqui-git's wiki_tests.rs.
// Run: ./moq test framework --spec WikiPageNames
const W = "org.moqui.resource.WikiServices."
function invoke(verb, parameters) {
    try {
        const result =
            ec.service
                .sync()
                .name(W + verb)
                .parameters(parameters)
                .call() || {}
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

spec({
    // the service authorization is the apps' (as Knowledge's); the wiki's own access rule (get#WikiPageAccess) still
    // applies, and the creator of a space may update it
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        invoke("create#WikiSpace", {wikiSpaceId: "SPWKNDB", description: "Spec page names"})
        invoke("create#WikiSpace", {wikiSpaceId: "SPWKNGT", description: "Spec page names on git", repositoryKind: "db"})
    },
    setup() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
    }
})

feature("a page named in another script is saved, renamed, moved and copied, its file at that name", {
    when() {
        const save = (wikiSpaceId, parentPath, pageName) =>
            invoke("update#WikiPage", {wikiSpaceId, parentPath, pageName, pageText: "# " + pageName + "\n"})
        const created = save("SPWKNDB", "", "产品说明")
        const child = save("SPWKNDB", "产品说明", "Ελληνικά (2)")
        const onGit = save("SPWKNGT", "", "Тест: страница!")
        const renamed = invoke("move#WikiPage", {wikiPageId: created.wikiPageId, toPageName: "产品说明二"})
        const childText = textAt("dbresource://WikiSpace/SPWKNDB/产品说明二/Ελληνικά (2).md")
        const moved = invoke("move#WikiPage", {
            wikiPageId: renamed.wikiPageId,
            toWikiSpaceId: "SPWKNGT",
            toParentPath: "Тест: страница!"
        })
        const copied = invoke("clone#WikiPage", {
            baseWikiSpaceId: "SPWKNGT",
            pagePath: "Тест: страница!",
            wikiSpaceId: "SPWKNDB",
            parentPath: "SPWKNDB",
            pageName: "عربي ١٢٣",
            copyHistory: true
        })
        const gitRoot = String(
            ec.entity.find("moqui.resource.wiki.WikiSpace").condition("wikiSpaceId", "SPWKNGT").disableAuthz().one()
                .rootPageLocation
        ).replace(/\.[^/.]*$/, "")
        return {
            created,
            child,
            onGit,
            renamed,
            moved,
            copied,
            childText,
            movedText: textAt(gitRoot + "/Тест: страница!/产品说明二.md"),
            movedChildText: textAt(gitRoot + "/Тест: страница!/产品说明二/Ελληνικά (2).md"),
            copiedText: textAt("dbresource://WikiSpace/SPWKNDB/عربي ١٢٣.md")
        }
    },
    then({created, child, onGit, renamed, moved, copied, childText, movedText, movedChildText, copiedText}) {
        created.error == "" && created.pagePath == "产品说明"
        child.error == "" && child.pagePath == "产品说明/Ελληνικά (2)"
        onGit.error == "" && onGit.pagePath == "Тест: страница!"
        renamed.error == "" && renamed.pagePath == "产品说明二"
        childText == "# Ελληνικά (2)\n"
        moved.error == "" && moved.wikiSpaceId == "SPWKNGT" && moved.pagePath == "Тест: страница!/产品说明二"
        movedText == "# 产品说明\n" && movedChildText == "# Ελληνικά (2)\n"
        copied.error == "" && copied.pagePath == "عربي ١٢٣"
        copiedText == "# Тест: страница!\n"
    }
})

feature("a slash, . or .., a control or direction character and a name over 240 bytes are refused on each verb", {
    when() {
        const page = invoke("update#WikiPage", {wikiSpaceId: "SPWKNDB", pageName: "Refusals", pageText: "x"})
        const save = pageName => invoke("update#WikiPage", {wikiSpaceId: "SPWKNDB", pageName, pageText: "x"}).error
        const rename = toPageName => invoke("move#WikiPage", {wikiPageId: page.wikiPageId, toPageName}).error
        const copy = pageName =>
            invoke("clone#WikiPage", {
                baseWikiSpaceId: "SPWKNDB",
                pagePath: "Refusals",
                wikiSpaceId: "SPWKNGT",
                pageName,
                copyHistory: true
            }).error
        const names = ["产品/说明", "..", ".", "a\u0007b", "a‮b", "a\\b", "说".repeat(81)]
        return {
            page,
            saves: names.map(save),
            renames: names.map(rename),
            copies: names.map(copy),
            longest: invoke("update#WikiPage", {wikiSpaceId: "SPWKNDB", pageName: "说".repeat(80), pageText: "x"})
        }
    },
    then({page, saves, renames, copies, longest}) {
        page.error == ""
        saves.slice(0, 6).every(error => error.includes("letters")) && saves[6].includes("too long")
        renames.slice(0, 6).every(error => error.includes("letters")) && renames[6].includes("too long")
        copies.slice(0, 6).every(error => error.includes("letters")) && copies[6].includes("too long")
        longest.error == ""
    }
})
