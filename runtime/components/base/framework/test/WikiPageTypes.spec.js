// A wiki page's type, its file's extension (org.moqui.resource.WikiServices check#WikiPageType; plans/Security-Review.md
// SEC-SCR-81): md, html or txt for anyone; fancy, html.fancy, md.fancy and markdown.fancy, which the template renderers run
// as server script for whoever views the page, only for a holder of DATABASE_ADMIN, and nothing else (a path, an extension
// of no renderer). A user without it may not write a script page that is there either. An md page's raw HTML is cleaned as
// an html page is, unless the space allows any HTML. The cleaning itself is moqui-lib safe_html's (clean_markdown).
// Run: ./moq test framework --spec WikiPageTypes
const W = "org.moqui.resource.WikiServices."
const writerId = "SPEC_WT_USER"
const dbaId = "SPEC_WT_DBA"
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

spec({
    setupSpec() {
        ec.artifactExecution.disableAuthz()
        make("moqui.security.UserAccount", {userId: writerId, username: "spec.wt.user", userFullName: "Spec Wiki Writer"})
        make("moqui.security.UserAccount", {userId: dbaId, username: "spec.wt.dba", userFullName: "Spec Wiki DBA"})
        make("moqui.security.UserGroup", {userGroupId: "SPEC_WT_DBA_GROUP", description: "Spec wiki DBA"})
        make("moqui.security.UserGroupPermission", {userGroupId: "SPEC_WT_DBA_GROUP", userPermissionId: "DATABASE_ADMIN", fromDate: ec.user.nowTimestamp})
        make("moqui.security.UserGroupMember", {userGroupId: "SPEC_WT_DBA_GROUP", userId: dbaId, fromDate: ec.user.nowTimestamp})
        ec.artifactExecution.enableAuthz()
        const members = [{userId: dbaId, allowUpdate: "Y"}]
        invokeAs("spec.wt.user", "create#WikiSpace", {wikiSpaceId: "SPWTYP", description: "Spec page types", memberList: members})
        invokeAs("spec.wt.user", "create#WikiSpace", {wikiSpaceId: "SPWTANY", description: "Spec any HTML", allowAnyHtml: "Y"})
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
        r.path.includes("md, html or txt") && r.noRenderer.includes("md, html or txt")
        r.space.includes("DATABASE_ADMIN")
        JSON.stringify(Array.from(r.types)) == '["md","html","txt"]'
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

feature("an md or txt page's raw HTML is cleaned unless the space allows any HTML", {
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
        !r.txt.includes("<script") && !r.txt.includes("onclick")
        r.any == r.text
    }
})
