// Records whose text runs as script where they are used need DATABASE_ADMIN to write that text (Security.eecas.xml,
// crates/moqui-core/src/service/security_checks.rs assert_stored_template, assert_stored_expression, assert_translation;
// plans/Security-Review-Medium.md SEC-SCR-06, SEC-SCR-12; plans/Security-Review.md SEC-BAP-81): an email template's sender
// or subject with ${}, an entity filter's map, a subscreen row's menu title with ${}, a translation that adds an expression to
// its original, and a text block's body with a ${} that is not a ${name} placeholder. Plain text, a translation that keeps
// the original's placeholders, a text block's placeholders, and a holder of DATABASE_ADMIN pass.
// Run: ./moq test framework --spec StoredScript
const adminId = "SPEC_SS_ADMIN"
const dbaId = "SPEC_SS_DBA"
const make = (entityName, fields) => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
const one = (entityName, conditions) => ec.entity.find(entityName).condition(conditions).disableAuthz().one()
// a write as the user, authorized as from the System app's screens, and the error text (empty when it was written)
const writeAs = (username, entityName, fields) => {
    ec.user.internalLoginUser(username)
    let error = ""
    try {
        ec.artifactExecution.runAsUser("component://tools/screen/System.xml", () => make(entityName, fields))
    } catch (e) {
        error = String(e)
    }
    error += ec.message.errorsString
    ec.message.clearErrors()
    ec.user.logoutUser()
    return error
}
const cleanup = () => {
    ec.artifactExecution.disableAuthz()
    ec.entity.find("moqui.basic.email.EmailTemplate").condition("emailTemplateId", "like", "SPEC_SS_%").deleteAll()
    ec.entity.find("moqui.security.EntityFilter").condition("entityFilterId", "like", "SPEC_SS_%").deleteAll()
    ec.entity.find("moqui.security.EntityFilterSet").condition("entityFilterSetId", "SPEC_SS_SET").deleteAll()
    ec.entity.find("moqui.screen.SubscreensItem").condition("screenLocation", "component://spec/StoredScript.xml").deleteAll()
    ec.entity.find("moqui.screen.ScreenDocument").condition("screenLocation", "component://spec/StoredScript.xml").deleteAll()
    ec.entity.find("moqui.resource.wiki.WikiSpace").condition("wikiSpaceId", "like", "SPEC_SS_%").deleteAll()
    ec.entity.find("moqui.basic.LocalizedMessage").condition("original", "like", "Spec stored %").deleteAll()
    ec.entity.find("moqui.basic.email.EmailTextBlock").condition("emailTextBlockId", "like", "SPEC_SS_%").deleteAll()
    for (const userId of [adminId, dbaId]) {
        ec.entity.find("moqui.security.UserGroupMember").condition("userId", userId).deleteAll()
        ec.entity.find("moqui.security.UserAccount").condition("userId", userId).deleteAll()
    }
    ec.entity.find("moqui.security.UserGroupPermission").condition("userGroupId", "SPEC_SS_DBA_GROUP").deleteAll()
    ec.entity.find("moqui.security.UserGroup").condition("userGroupId", "SPEC_SS_DBA_GROUP").deleteAll()
    ec.artifactExecution.enableAuthz()
}

spec({
    setupSpec() {
        cleanup()
        ec.artifactExecution.disableAuthz()
        // an ADMIN who is not in ADMIN_ADV, so without DATABASE_ADMIN
        make("moqui.security.UserAccount", { userId: adminId, username: "spec.ss.admin", userFullName: "Spec Stored Script" })
        make("moqui.security.UserGroupMember", { userGroupId: "ADMIN", userId: adminId, fromDate: ec.user.nowTimestamp })
        // an ADMIN with DATABASE_ADMIN through a group of its own
        make("moqui.security.UserAccount", { userId: dbaId, username: "spec.ss.dba", userFullName: "Spec Stored Script DBA" })
        make("moqui.security.UserGroupMember", { userGroupId: "ADMIN", userId: dbaId, fromDate: ec.user.nowTimestamp })
        make("moqui.security.UserGroup", { userGroupId: "SPEC_SS_DBA_GROUP", description: "Spec stored script DBA" })
        make("moqui.security.UserGroupPermission", { userGroupId: "SPEC_SS_DBA_GROUP", userPermissionId: "DATABASE_ADMIN", fromDate: ec.user.nowTimestamp })
        make("moqui.security.UserGroupMember", { userGroupId: "SPEC_SS_DBA_GROUP", userId: dbaId, fromDate: ec.user.nowTimestamp })
        make("moqui.security.EntityFilterSet", { entityFilterSetId: "SPEC_SS_SET", description: "Spec stored script" })
        ec.artifactExecution.enableAuthz()
    },
    cleanupSpec() { cleanup() },
})

feature("stored script text needs DATABASE_ADMIN; plain text and kept placeholders do not", {
    expect() {
        // (in a function: each statement at the top of expect is a condition)
        const run = () => ({
            subject: writeAs("spec.ss.admin", "moqui.basic.email.EmailTemplate",
                { emailTemplateId: "SPEC_SS_1", description: "Spec", subject: "Hi ${ec.factory.moquiVersion}" }),
            plainSubject: writeAs("spec.ss.admin", "moqui.basic.email.EmailTemplate",
                { emailTemplateId: "SPEC_SS_2", description: "Spec", subject: "Hi there" }),
            dbaSubject: writeAs("spec.ss.dba", "moqui.basic.email.EmailTemplate",
                { emailTemplateId: "SPEC_SS_3", description: "Spec", subject: "Hi ${firstName}" }),
            filter: writeAs("spec.ss.admin", "moqui.security.EntityFilter", { entityFilterId: "SPEC_SS_F1",
                entityFilterSetId: "SPEC_SS_SET", entityName: "moqui.basic.Enumeration", filterMap: "[enumId: 'x']" }),
            menuTitle: writeAs("spec.ss.admin", "moqui.screen.SubscreensItem", { screenLocation: "component://spec/StoredScript.xml",
                subscreenName: "one", userGroupId: "ALL_USERS", subscreenLocation: "component://spec/One.xml", menuTitle: "One ${ec.user.userId}" }),
            addedExpression: writeAs("spec.ss.admin", "moqui.basic.LocalizedMessage",
                { original: "Spec stored ${count} items", locale: "fr", localized: "Spec ${ec.factory.moquiVersion} articles" }),
            keptPlaceholder: writeAs("spec.ss.admin", "moqui.basic.LocalizedMessage",
                { original: "Spec stored ${count} items", locale: "es", localized: "Spec ${count} artículos" }),
            blockScript: writeAs("spec.ss.admin", "moqui.basic.email.EmailTextBlock", { emailTextBlockId: "SPEC_SS_TB1",
                ownerUserId: adminId, title: "Spec", bodyHtml: "<p>Case ${requestId} ${ec.factory.moquiVersion}</p>" }),
            blockPlaceholders: writeAs("spec.ss.admin", "moqui.basic.email.EmailTextBlock", { emailTextBlockId: "SPEC_SS_TB2",
                ownerUserId: adminId, title: "Spec", bodyHtml: "<p>Case ${requestId} in ${ queueName } is open.</p>" }),
            // a placeholder whose removal leaves an expression (the rule removes them, then looks for ${)
            blockJoined: writeAs("spec.ss.admin", "moqui.basic.email.EmailTextBlock", { emailTextBlockId: "SPEC_SS_TB3",
                ownerUserId: adminId, title: "Spec", bodyHtml: "<p>$${requestId}{ec.factory.moquiVersion}</p>" }),
        })
        const out = run()
        out.subject.includes("DATABASE_ADMIN") && one("moqui.basic.email.EmailTemplate", {emailTemplateId: "SPEC_SS_1"}) == null
        out.plainSubject === "" && one("moqui.basic.email.EmailTemplate", {emailTemplateId: "SPEC_SS_2"}) != null
        out.dbaSubject === "" && one("moqui.basic.email.EmailTemplate", {emailTemplateId: "SPEC_SS_3"}) != null
        out.filter.includes("DATABASE_ADMIN") && one("moqui.security.EntityFilter", {entityFilterId: "SPEC_SS_F1"}) == null
        out.menuTitle.includes("DATABASE_ADMIN")
        out.addedExpression.includes("DATABASE_ADMIN")
        out.keptPlaceholder === ""
        out.blockScript.includes("DATABASE_ADMIN") && one("moqui.basic.email.EmailTextBlock", {emailTextBlockId: "SPEC_SS_TB1"}) == null
        out.blockPlaceholders === "" && one("moqui.basic.email.EmailTextBlock", {emailTextBlockId: "SPEC_SS_TB2"}) != null
        out.blockJoined.includes("DATABASE_ADMIN") && one("moqui.basic.email.EmailTextBlock", {emailTextBlockId: "SPEC_SS_TB3"}) == null
    },
})

// SEC-SCR-92: a field that names a location selects text run where its record is used (a screen definition, a document
// rendered by its extension's renderer, a decorator screen): a location other than a component's needs DATABASE_ADMIN
feature("a location of run text other than a component's needs DATABASE_ADMIN", {
    expect() {
        const run = () => ({
            remoteScreen: writeAs("spec.ss.admin", "moqui.screen.SubscreensItem", { screenLocation: "component://spec/StoredScript.xml",
                subscreenName: "remote", userGroupId: "ALL_USERS", subscreenLocation: "https://evil.example/Remote.xml", menuTitle: "Remote" }),
            componentScreen: writeAs("spec.ss.admin", "moqui.screen.SubscreensItem", { screenLocation: "component://spec/StoredScript.xml",
                subscreenName: "local", userGroupId: "ALL_USERS", subscreenLocation: "component://spec/Local.xml", menuTitle: "Local" }),
            document: writeAs("spec.ss.admin", "moqui.screen.ScreenDocument", { screenLocation: "component://spec/StoredScript.xml",
                docIndex: 1, docTitle: "Doc", docLocation: "dbresource://spec/doc.md.fancy" }),
            dbaDocument: writeAs("spec.ss.dba", "moqui.screen.ScreenDocument", { screenLocation: "component://spec/StoredScript.xml",
                docIndex: 2, docTitle: "Doc", docLocation: "dbresource://spec/doc.md" }),
            decorator: writeAs("spec.ss.admin", "moqui.resource.wiki.WikiSpace", { wikiSpaceId: "SPEC_SS_W1",
                rootPageLocation: "dbresource://WikiSpace/SPEC_SS_W1", decoratorScreenLocation: "dbresource://spec/Decorator.xml" }),
        })
        const out = run()
        out.remoteScreen.includes("DATABASE_ADMIN")
        out.componentScreen === ""
        out.document.includes("DATABASE_ADMIN") && one("moqui.screen.ScreenDocument", {screenLocation: "component://spec/StoredScript.xml", docIndex: 1}) == null
        out.dbaDocument === ""
        out.decorator.includes("DATABASE_ADMIN") && one("moqui.resource.wiki.WikiSpace", {wikiSpaceId: "SPEC_SS_W1"}) == null
    },
})

