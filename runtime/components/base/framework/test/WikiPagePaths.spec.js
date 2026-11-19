// A wiki page path names one page, whatever its slashes (org.moqui.resource.WikiServices get#WikiPageId,
// get#WikiPageReference, get#WikiPageFile, get#WikiPageAttachment, WikiPageName.js; plans/Security-Review-Medium.md
// SEC-BOS-85): a doubled, leading or trailing slash finds the same record as the file, so a page restricted to its members
// is not read, nor its attachments, by a path its record is not at; a file of a two-part type (Name.md.fancy) is page Name.
// Run: ./moq test framework --spec WikiPagePaths
const W = "org.moqui.resource.WikiServices."
const ownerId = "SPEC_WP_OWNER"
const readerId = "SPEC_WP_READER"
const SPACE = "SPWPATH"
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
function asSystem(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}

spec({
    setupSpec() {
        asSystem(() => {
            make("moqui.security.UserAccount", {userId: ownerId, username: "spec.wp.owner", userFullName: "Spec Wiki Owner"})
            make("moqui.security.UserAccount", {userId: readerId, username: "spec.wp.reader", userFullName: "Spec Wiki Reader"})
        })
        invokeAs("spec.wp.owner", "create#WikiSpace", {wikiSpaceId: SPACE, description: "Spec page paths"})
        invokeAs("spec.wp.owner", "update#WikiPage", {wikiSpaceId: SPACE, pageName: "Policies", pageText: "# Policies\n"})
        const secret = invokeAs("spec.wp.owner", "update#WikiPage", {
            wikiSpaceId: SPACE,
            parentPath: "Policies",
            pageName: "Secret",
            pageText: "# Secret\n\nzq-door-code\n"
        })
        asSystem(() => {
            const row = ec.entity.find("moqui.resource.wiki.WikiPage").condition("wikiPageId", secret.wikiPageId).one()
            row.restrictView = "Y"
            row.update()
            // its one member, who reads it
            make("moqui.resource.wiki.WikiPageUser", {wikiPageId: secret.wikiPageId, userId: ownerId, allowView: "Y", allowUpdate: "Y"})
            ec.resource.getLocationReference(`dbresource://WikiSpace/${SPACE}/Policies/Secret/_attachments/note.txt`).putText("zq-attachment")
            // a script type's file written past the services, as a git push writes one: page Plain, not Plain.md
            ec.resource.getLocationReference(`dbresource://WikiSpace/${SPACE}/Plain.md.fancy`).putText("plain")
        })
    }
})

feature("a restricted page is not read by a path with a doubled, leading or trailing slash, nor its attachment", {
    when() {
        const info = path => invokeAs("spec.wp.reader", "get#WikiPageInfo", {wikiSpaceId: SPACE, pagePath: path})
        const attachment = path =>
            invokeAs("spec.wp.reader", "get#WikiPageAttachment", {wikiSpaceId: SPACE, pagePath: path, filename: "note.txt"})
        return {
            plain: info("Policies/Secret"),
            doubled: info("Policies//Secret"),
            leading: info("/Policies/Secret"),
            trailing: info("Policies/Secret/"),
            attachmentPlain: attachment("Policies/Secret"),
            attachmentDoubled: attachment("Policies//Secret"),
            attachmentLeading: attachment("/Policies/Secret/"),
            owner: invokeAs("spec.wp.owner", "get#WikiPageInfo", {wikiSpaceId: SPACE, pagePath: "Policies//Secret"}),
            ownerAttachment: invokeAs("spec.wp.owner", "get#WikiPageAttachment", {
                wikiSpaceId: SPACE,
                pagePath: "Policies//Secret",
                filename: "note.txt"
            })
        }
    },
    then(r) {
        r.plain.error.includes("not found")
        r.doubled.error.includes("not found") && r.doubled.pageReference == null
        r.leading.error.includes("not found") && r.leading.pageReference == null
        r.trailing.error.includes("not found") && r.trailing.pageReference == null
        r.attachmentPlain.error.includes("not found")
        r.attachmentDoubled.error.includes("not found") && r.attachmentDoubled.attachmentReference == null
        r.attachmentLeading.error.includes("not found") && r.attachmentLeading.attachmentReference == null
        // the page's own member reads it by any of those paths, at its own path
        r.owner.error == "" && r.owner.pagePath == "Policies/Secret" && r.owner.wikiPageId != null
        r.ownerAttachment.error == "" && r.ownerAttachment.attachmentReference != null
    }
})

feature("a file of a two-part script type is the page of its name less the type: Plain.md.fancy is page Plain", {
    when() {
        return {
            file: invokeAs("spec.wp.owner", "get#WikiPageFile", {wikiSpaceId: SPACE, pagePath: "Plain"}),
            children: JSON.parse(
                JSON.stringify(invokeAs("spec.wp.owner", "get#WikiPageChildren", {wikiSpaceId: SPACE, pagePath: ""}).childPageInfoList || [])
            )
        }
    },
    then({file, children}) {
        file.error == "" && file.exists == true && file.filePagePath == "Plain"
        children.some(child => child.pageName == "Plain" && child.pagePath == "Plain")
        !children.some(child => child.pageName == "Plain.md")
    }
})
