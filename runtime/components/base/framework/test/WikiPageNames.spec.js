// A wiki page's name (org.moqui.resource.WikiServices update#WikiPage, move#WikiPage and clone#WikiPage): letters,
// digits and combining marks (never first) of any script, spaces and . , ' - _ : ( ) ! ? only, as Knowledge's (mr-boss
// 628669dc), saved in NFC so a decomposed name is its composed twin's page. The name is one segment of
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

feature("a blog post's title names its page by the same rule (create#WikiBlog and update#WikiBlog)", {
    when() {
        const post = invoke("create#WikiBlog", {wikiSpaceId: "SPWKNDB", title: "博客 Ελληνικά", blogText: "<p>x</p>"})
        return {
            post,
            postText: textAt("dbresource://WikiSpace/SPWKNDB/博客 Ελληνικά.html"),
            slash: invoke("create#WikiBlog", {wikiSpaceId: "SPWKNDB", title: "博客/x", blogText: "<p>x</p>"}).error,
            renamed: invoke("update#WikiBlog", {wikiBlogId: post.wikiBlogId, title: "博客二"}).error,
            long: invoke("update#WikiBlog", {wikiBlogId: post.wikiBlogId, title: "说".repeat(81)}).error
        }
    },
    then({post, postText, slash, renamed, long}) {
        post.error == "" && post.wikiBlogId && postText == "<p>x</p>"
        slash.includes("letters") && renamed == "" && long.includes("too long")
    }
})

feature("combining marks: Devanagari and Thai names are saved, a decomposed (NFD) name is its NFC twin's page, a leading mark is refused", {
    when() {
        const save = (pageName, pageText = "x") => invoke("update#WikiPage", {wikiSpaceId: "SPWKNDB", pageName, pageText})
        const pages = path => ec.entity.find("moqui.resource.wiki.WikiPage").condition("wikiSpaceId", "SPWKNDB")
            .condition("pagePath", path).disableAuthz().count()
        const devanagari = save("हिन्दी पृष्ठ")
        const thai = save("หน้าภาษาไทย")
        const composed = save("Café crème", "composed")
        const decomposed = save("Cafe\u0301 cre\u0300me", "decomposed")
        // Hangul jamo are letters: the decomposed syllables are saved as the syllables
        const jamo = save("\u1112\u1161\u11AB\u1100\u1173\u11AF")
        const syllables = save("한글")
        const renamed = invoke("move#WikiPage", {wikiPageId: thai.wikiPageId, toPageName: "Pha\u0302\u0300 Thai"})
        const page = save("Marks", "x")
        const leading = "\u0301abc", afterSpace = "a \u0301b"
        return {
            devanagari,
            thai,
            composed,
            decomposed,
            jamo,
            syllables,
            renamed,
            cafes: pages("Café crème"),
            cafeText: textAt("dbresource://WikiSpace/SPWKNDB/Café crème.md"),
            saves: [leading, afterSpace].map(name => save(name).error),
            renames: [leading, afterSpace].map(toPageName => invoke("move#WikiPage", {wikiPageId: page.wikiPageId, toPageName}).error),
            copies: [leading, afterSpace].map(pageName => invoke("clone#WikiPage", {
                baseWikiSpaceId: "SPWKNDB", pagePath: "Marks", wikiSpaceId: "SPWKNGT", pageName}).error),
            post: invoke("create#WikiBlog", {wikiSpaceId: "SPWKNDB", title: "Blog cafe\u0301", blogText: "<p>x</p>"}),
            leadingPost: invoke("create#WikiBlog", {wikiSpaceId: "SPWKNDB", title: leading, blogText: "<p>x</p>"}).error
        }
    },
    then(r) {
        r.devanagari.error == "" && r.devanagari.pagePath == "हिन्दी पृष्ठ"
        r.thai.error == "" && r.thai.pagePath == "หน้าภาษาไทย"
        r.composed.error == "" && r.decomposed.error == ""
        r.decomposed.wikiPageId == r.composed.wikiPageId && r.decomposed.pagePath == "Café crème"
        r.cafes == 1 && r.cafeText == "decomposed"
        r.jamo.error == "" && r.jamo.pagePath == "한글" && r.syllables.wikiPageId == r.jamo.wikiPageId
        r.renamed.error == "" && r.renamed.pagePath == "Phầ Thai"
        r.saves.every(error => error.includes("letters"))
        r.renames.every(error => error.includes("letters"))
        r.copies.every(error => error.includes("letters"))
        r.post.error == "" && ec.entity.find("moqui.resource.wiki.WikiBlog").condition("wikiBlogId", r.post.wikiBlogId)
            .disableAuthz().one().title == "Blog café"
        r.leadingPost.includes("letters")
    }
})
