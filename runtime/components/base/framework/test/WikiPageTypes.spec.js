// A wiki page's type, its file's extension (org.moqui.resource.WikiServices check#WikiPageType; plans/Security-Review.md
// SEC-SCR-81): md, html, txt or excalidraw for anyone; fancy, html.fancy, md.fancy and markdown.fancy, which the template renderers run
// as server script for whoever views the page, only for a holder of DATABASE_ADMIN, and nothing else (a path, an extension
// of no renderer). A user without it may not write a script page that is there either. An md page's raw HTML is cleaned as
// an html page is, unless the space allows any HTML; a txt page is kept as written. render#WikiPage chooses the renderer by
// the page's type and the space, so a file written past these services (a drive, a git push) is cleaned or shown as text,
// and a script page runs only in a space with allowScriptPages, which only DATABASE_ADMIN sets (Wiki.eecas.xml). The
// cleaning itself is moqui-lib safe_html's (clean_markdown).
// Run: ./moq test framework --spec WikiPageTypes
const W = "org.moqui.resource.WikiServices."
const writerId = "SPEC_WT_USER"
const dbaId = "SPEC_WT_DBA"
const adminId = "SPEC_WT_ADMIN"
const make = (entityName, fields) => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
// a call as the user, with the services' own authorization off (the apps' screens give it): the wiki's rules still hold
function invokeAs(username, verb, parameters) {
    ec.user.internalLoginUser(username)
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        const result = ec.service.sync().name(W + verb).parameters(parameters).call() || {}
        return Object.assign({}, result, {error: ec.message.hasError() ? ec.message.getErrorsString() : ""})
    } catch (e) {
        return {error: String(e.message || e)}
    } finally {
        ec.message.clearErrors()
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
        ec.user.logoutUser()
    }
}
const textAt = (location) => {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        const ref = ec.resource.getLocationReference(location)
        return ref.exists ? ref.getText() : null
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
const page = (spaceId, file) => textAt(`dbresource://WikiSpace/${spaceId}/${file}`)
// a file written past the wiki's services, as a drive upload or a git push writes one
const putPage = (spaceId, file, text) => {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try { ec.resource.getLocationReference(`dbresource://WikiSpace/${spaceId}/${file}`).putText(text) }
    finally { if (!wasDisabled) ec.artifactExecution.enableAuthz() }
}
// a WikiSpace row written as the user, authorized as from the System app's screens, and the error text
const spaceAs = (username, fields) => {
    ec.user.internalLoginUser(username)
    let error = ""
    try {
        ec.artifactExecution.runAsUser("component://tools/screen/System.xml", () => make("moqui.resource.wiki.WikiSpace", fields))
    } catch (e) {
        error = String(e)
    }
    error += ec.message.errorsString
    ec.message.clearErrors()
    ec.user.logoutUser()
    return error
}

spec({
    setupSpec() {
        ec.artifactExecution.disableAuthz()
        make("moqui.security.UserAccount", {userId: writerId, username: "spec.wt.user", userFullName: "Spec Wiki Writer"})
        make("moqui.security.UserAccount", {userId: dbaId, username: "spec.wt.dba", userFullName: "Spec Wiki DBA"})
        make("moqui.security.UserGroup", {userGroupId: "SPEC_WT_DBA_GROUP", description: "Spec wiki DBA"})
        make("moqui.security.UserGroupPermission", {userGroupId: "SPEC_WT_DBA_GROUP", userPermissionId: "DATABASE_ADMIN", fromDate: ec.user.nowTimestamp})
        make("moqui.security.UserGroupMember", {userGroupId: "SPEC_WT_DBA_GROUP", userId: dbaId, fromDate: ec.user.nowTimestamp})
        // an ADMIN without DATABASE_ADMIN, and the DBA an ADMIN too, for the space settings written as from System
        make("moqui.security.UserAccount", {userId: adminId, username: "spec.wt.admin", userFullName: "Spec Wiki Admin"})
        make("moqui.security.UserGroupMember", {userGroupId: "ADMIN", userId: adminId, fromDate: ec.user.nowTimestamp})
        make("moqui.security.UserGroupMember", {userGroupId: "ADMIN", userId: dbaId, fromDate: ec.user.nowTimestamp})
        ec.artifactExecution.enableAuthz()
        const members = [{userId: dbaId, allowUpdate: "Y"}]
        invokeAs("spec.wt.user", "create#WikiSpace", {wikiSpaceId: "SPWTYP", description: "Spec page types", memberList: members})
        invokeAs("spec.wt.user", "create#WikiSpace", {wikiSpaceId: "SPWTANY", description: "Spec any HTML", allowAnyHtml: "Y"})
        invokeAs("spec.wt.dba", "create#WikiSpace", {wikiSpaceId: "SPWTSCR", description: "Spec script pages", allowScriptPages: "Y"})
        invokeAs("spec.wt.dba", "create#WikiSpace", {wikiSpaceId: "SPWTHID", description: "Spec hidden", restrictView: "Y"})
        // script pages allowed, none written yet: a copy by a user without DATABASE_ADMIN is a plain space
        invokeAs("spec.wt.dba", "create#WikiSpace", {wikiSpaceId: "SPWTSC3", description: "Spec script space", allowScriptPages: "Y"})
    }
})

feature("without DATABASE_ADMIN a script page type, and a type outside the list, is refused", {
    when() {
        const save = (pageName, wikiType) => invokeAs("spec.wt.user", "update#WikiPage",
            {wikiSpaceId: "SPWTYP", pageName, wikiType, pageText: "${ec.user.userId}"}).error
        return {
            fancy: save("Plain", "fancy"),
            mdFancy: save("Plain", "md.fancy"),
            path: save("Plain", "../x"),
            noRenderer: save("Plain", "htm"),
            space: invokeAs("spec.wt.user", "create#WikiSpace", {wikiSpaceId: "SPWTFAN", wikiType: "html.fancy"}).error,
            types: invokeAs("spec.wt.user", "get#WikiPageTypes", {}).wikiTypeList,
            attachments: invokeAs("spec.wt.user", "get#WikiPageInfo", {wikiSpaceId: "SPWTYP", pagePath: "Plain/_attachments/x"}).error,
            files: [page("SPWTYP", "Plain.fancy"), page("SPWTYP", "Plain.md.fancy"), page("SPWTYP", "Plain.htm")]
        }
    },
    then(r) {
        r.fancy.includes("DATABASE_ADMIN") && r.mdFancy.includes("DATABASE_ADMIN")
        r.path.includes("md, html, txt or excalidraw") && r.noRenderer.includes("md, html, txt or excalidraw")
        r.space.includes("DATABASE_ADMIN")
        JSON.stringify(Array.from(r.types)) == '["md","html","txt","excalidraw"]'
        r.attachments.includes("attachments")
        r.files.every((text) => text == null)
    }
})

feature("with DATABASE_ADMIN a script page is written, and a user without it may not change or move it", {
    when() {
        const written = invokeAs("spec.wt.dba", "update#WikiPage",
            {wikiSpaceId: "SPWTYP", pageName: "Script", wikiType: "fancy", pageText: "Two is ${1 + 1}\n"})
        const types = invokeAs("spec.wt.dba", "get#WikiPageTypes", {}).wikiTypeList
        const changed = invokeAs("spec.wt.user", "update#WikiPage", {wikiSpaceId: "SPWTYP", pageName: "Script", pageText: "${ec.user.userId}"})
        const moved = invokeAs("spec.wt.user", "move#WikiPage", {wikiPageId: written.wikiPageId, toPageName: "Moved"})
        return {written, types, changed: changed.error, moved: moved.error, text: page("SPWTYP", "Script.fancy")}
    },
    then(r) {
        r.written.error == "" && r.written.wikiPageId
        Array.from(r.types).includes("md.fancy")
        r.changed.includes("DATABASE_ADMIN") && r.moved.includes("DATABASE_ADMIN")
        r.text == "Two is ${1 + 1}\n"
    }
})

feature("an md page's raw HTML is cleaned unless the space allows any HTML, and a txt page is kept as written", {
    when() {
        const text = "# Notes\n\n<script>alert(1)</script>\n\ntext <b onclick=\"x()\">bold</b> a < b\n"
        const save = (wikiSpaceId, pageName, wikiType) => invokeAs("spec.wt.user", "update#WikiPage",
            {wikiSpaceId, pageName, wikiType, pageText: text}).error
        return {
            errors: [save("SPWTYP", "Notes", "md"), save("SPWTYP", "Plain", "txt"), save("SPWTANY", "Notes", "md")],
            md: page("SPWTYP", "Notes.md"),
            txt: page("SPWTYP", "Plain.txt"),
            any: page("SPWTANY", "Notes.md"),
            text
        }
    },
    then(r) {
        r.errors.every((error) => error == "")
        r.md.startsWith("# Notes\n\n") && r.md.includes("<b>bold</b> a < b")
        !r.md.includes("<script") && !r.md.includes("onclick")
        r.txt == r.text
        r.any == r.text
    }
})

feature("a page the user may not view gives no location or reference, as null outputs a caller's out-map takes", {
    when() {
        const out = invokeAs("spec.wt.user", "get#WikiPageInfo", {wikiSpaceId: "SPWTHID", pagePath: ""})
        const rendered = invokeAs("spec.wt.user", "render#WikiPage", {wikiSpaceId: "SPWTHID", pagePath: ""})
        return {out, keys: Object.keys(out), rendered}
    },
    then(r) {
        r.out.error.includes("not found")
        r.keys.includes("pageLocation") && r.out.pageLocation == null && r.out.pageReference == null
        r.rendered.pageHtml == null
    }
})

feature("render#WikiPage chooses the renderer by the page's type and the space, whoever wrote the file", {
    when() {
        putPage("SPWTYP", "Pushed.md", "# Pushed\n\n<script>alert(1)</script>\n\n<b onclick=\"x()\">b</b> [l](javascript:x())\n")
        putPage("SPWTYP", "Planted.fancy", "Two is ${1 + 1} <b>")
        putPage("SPWTYP", "Old.ftl", "<script>x</script>")
        const render = (username, wikiSpaceId, pagePath) => invokeAs(username, "render#WikiPage", {wikiSpaceId, pagePath})
        invokeAs("spec.wt.dba", "update#WikiPage", {wikiSpaceId: "SPWTSCR", pageName: "Live", wikiType: "fancy", pageText: "Two is ${1 + 1}"})
        return {
            md: render("spec.wt.user", "SPWTYP", "Pushed"),
            planted: render("spec.wt.user", "SPWTYP", "Planted"),
            ftl: render("spec.wt.user", "SPWTYP", "Old"),
            txt: render("spec.wt.user", "SPWTYP", "Plain"),
            any: render("spec.wt.user", "SPWTANY", "Notes"),
            live: render("spec.wt.user", "SPWTSCR", "Live"),
            savedFtl: invokeAs("spec.wt.user", "update#WikiPage", {wikiSpaceId: "SPWTYP", pageName: "Old", pageText: "kept"}).error,
            ftlText: page("SPWTYP", "Old.ftl")
        }
    },
    then(r) {
        r.md.error == "" && r.md.renderedAs == "markdown" && r.md.pageHtml.includes("<h1>Pushed</h1>")
        !r.md.pageHtml.includes("<script") && !r.md.pageHtml.includes("onclick") && !r.md.pageHtml.includes("javascript")
        r.planted.renderedAs == "text" && r.planted.pageHtml.includes("${1 + 1} &lt;b&gt;")
        r.ftl.renderedAs == "text" && r.ftl.pageHtml.includes("&lt;script&gt;")
        r.txt.renderedAs == "text" && r.txt.pageHtml.includes("&lt;script&gt;") && r.txt.pageText.includes("<script>")
        r.any.pageHtml.includes("<script>alert(1)</script>")
        r.live.error == "" && r.live.renderedAs == "script" && r.live.pageHtml.includes("Two is 2")
        r.savedFtl == "" && r.ftlText == "kept"
    }
})

feature("a space's script and storage settings: allowScriptPages and a screen around its pages need DATABASE_ADMIN, a root outside its own place an administrator, .. never", {
    when() {
        const root = (id) => `dbresource://WikiSpace/${id}.md`
        return {
            script: spaceAs("spec.wt.admin", {wikiSpaceId: "SPWTS1", rootPageLocation: root("SPWTS1"), allowScriptPages: "Y"}),
            decorator: spaceAs("spec.wt.admin", {wikiSpaceId: "SPWTS2", rootPageLocation: root("SPWTS2"),
                decoratorScreenLocation: "component://framework/test/screen/MultiPin.xml"}),
            dotDot: spaceAs("spec.wt.admin", {wikiSpaceId: "SPWTS3", rootPageLocation: "dbresource://WikiSpace/SPWTS3/../SPWTYP.md"}),
            elsewhere: spaceAs("spec.wt.admin", {wikiSpaceId: "SPWTS4", rootPageLocation: "dbresource://WikiSpace/Elsewhere/SPWTS4.md"}),
            dba: spaceAs("spec.wt.dba", {wikiSpaceId: "SPWTS5", rootPageLocation: root("SPWTS5"), allowScriptPages: "Y"}),
            types: invokeAs("spec.wt.dba", "get#WikiPageTypes", {}).wikiTypeList
        }
    },
    then(r) {
        r.script.includes("DATABASE_ADMIN") && r.decorator.includes("DATABASE_ADMIN")
        r.dotDot.includes("own place") && r.elsewhere == "" && r.dba == ""
        Array.from(r.types).includes("fancy")
    }
})

// a service call as the user, authorized as from the System app's screens (authorization on), and its error text
const callAs = (username, verb, parameters) => {
    ec.user.internalLoginUser(username)
    let out = {}
    let error = ""
    try {
        out = ec.artifactExecution.runAsUser("component://tools/screen/System.xml",
            () => ec.service.sync().name(W + verb).parameters(parameters).call()) || {}
    } catch (e) {
        error = String(e)
    }
    error += ec.message.errorsString
    ec.message.clearErrors()
    ec.user.logoutUser()
    return Object.assign({}, out, {error})
}
const spaceRow = (wikiSpaceId) => {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try { return ec.entity.find("moqui.resource.wiki.WikiSpace").condition("wikiSpaceId", wikiSpaceId).one() }
    finally { if (!wasDisabled) ec.artifactExecution.enableAuthz() }
}

feature("script pages run only from a space's own place in the database; a copy keeps the settings only its maker may set; a link base is a path", {
    when() {
        putPage("SPWTYP", "Linked.md", "[other](Other)\n")
        const linked = (baseLinkUrl) => invokeAs("spec.wt.user", "render#WikiPage", {wikiSpaceId: "SPWTYP", pagePath: "Linked", baseLinkUrl}).pageHtml
        return {
            foreign: spaceAs("spec.wt.dba", {wikiSpaceId: "SPWTS6", rootPageLocation: "dbresource://WikiSpace/Elsewhere/SPWTS6.md", allowScriptPages: "Y"}),
            onGit: spaceAs("spec.wt.dba", {wikiSpaceId: "SPWTS7", rootPageLocation: "git://WIKI_SPWTS7/SPWTS7.md", allowScriptPages: "Y"}),
            cloned: callAs("spec.wt.admin", "clone#WikiSpace", {baseWikiSpaceId: "SPWTSC3", wikiSpaceId: "SPWTCLN"}),
            path: linked("/apps/wiki/SPWTYP"),
            host: linked("//evil.example/w"),
            scheme: linked("https://evil.example/w")
        }
    },
    then(r) {
        r.foreign.includes("own place in the database") && r.onGit.includes("own place in the database")
        r.cloned.error == "" && spaceRow("SPWTCLN")?.allowScriptPages == null && spaceRow("SPWTSC3").allowScriptPages == "Y"
        r.path.includes('href="/apps/wiki/SPWTYP/Other"')
        r.host.includes('href="Other"') && r.scheme.includes('href="Other"')
    }
})

feature("deleting a board page takes its co-editing, meeting boards and recurring link with it", {
    when() {
        const scene = JSON.stringify({type:"excalidraw", version:2, elements:[], files:{}})
        const page = invokeAs("spec.wt.user", "update#WikiPage", {wikiSpaceId:"SPWTYP", pageName:"Retired board", wikiType:"excalidraw", pageText:scene})
        const wikiPageId = page.wikiPageId
        const wasDisabled = ec.artifactExecution.disableAuthz()
        try {
            make("moqui.call.CallSession", {callSessionId:"SPWT_CALL", statusId:"CssEnded"})
            make("moqui.call.CallBoard", {callSessionId:"SPWT_CALL", wikiPageId, openedByUserId:writerId})
            make("moqui.call.CallLink", {callLinkId:"SPWT_LINK", name:"Weekly", ownerUserId:writerId, wikiPageId})
            make("moqui.resource.CollabDocument", {documentKey:"WikiPage:" + wikiPageId})
        } finally { if (!wasDisabled) ec.artifactExecution.enableAuthz() }
        const deleted = invokeAs("spec.wt.user", "delete#WikiPage", {wikiPageId, deleteAttachments:true})
        const count = (entity, keys) => ec.entity.find(entity).condition(keys).disableAuthz().count()
        return {page, deleted, boards:count("moqui.call.CallBoard", {wikiPageId}),
            documents:count("moqui.resource.CollabDocument", {documentKey:"WikiPage:" + wikiPageId}),
            link:ec.entity.find("moqui.call.CallLink").condition({callLinkId:"SPWT_LINK"}).disableAuthz().one(),
            pages:count("moqui.resource.wiki.WikiPage", {wikiPageId})}
    },
    then(r) {
        r.page.error === "" && r.deleted.error === ""
        r.boards === 0 && r.documents === 0 && r.pages === 0
        r.link != null && r.link.wikiPageId == null
    }
})

feature("an Excalidraw page versions its previews and enforces its image limits", {
    when() {
        const asWriter = (verb, params) => invokeAs("spec.wt.user", verb, params)
        const scene = JSON.stringify({type:"excalidraw", version:2, elements:[{id:"title", type:"text", text:"Design"}], files:{}})
        const page = asWriter("update#WikiPage", {wikiSpaceId:"SPWTYP", pageName:"Design", wikiType:"excalidraw", pageText:scene})
        const upload = (name, mime, text) => asWriter("upload#WikiPageAttachment",
                {wikiPageId:page.wikiPageId, attachmentFile:uploadedFile(name, text, mime)})
        const svg = '<svg xmlns="http://www.w3.org/2000/svg"/>'
        const files = withConf("board@max-images", "1", () => [
            upload("art.svg", "image/svg+xml", svg), upload("other.svg", "image/svg+xml", svg), upload("art.svg", "image/svg+xml", svg)
        ])
        return {page, files, type:upload("art.html", "text/html", "x"),
            size:withConf("board@max-image-bytes", "10", () => upload("art.svg", "image/svg+xml", svg)),
            total:withConf("board@max-total-image-bytes", "60", () => upload("art.svg", "image/svg+xml", svg)),
            info:asWriter("get#WikiPageInfoById", {wikiPageId:page.wikiPageId}),
            malformed:asWriter("update#WikiPage", {wikiSpaceId:"SPWTYP", pageName:"Design", pageText:"{}"}).error}
    },
    then(r) {
        r.page.error === "" && r.page.wikiPageId
        r.files[0].error === "" && r.files[2].error === ""
        r.files[1].error.includes("image limit") && r.type.error.includes("attachment must") && r.size.error.includes("size limit")
        r.total.error.includes("total size limit")
        r.info.attachmentList.some(a => a.filename === "_board.svg")
        r.info.attachmentList.some(a => a.filename === "_board-" + encodeURIComponent(r.page.versionName) + ".svg")
        r.malformed.includes("Excalidraw scene")
    }
})
