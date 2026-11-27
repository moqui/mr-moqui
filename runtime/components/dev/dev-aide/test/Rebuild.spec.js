/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// Rebuild from the app (plans/archive/design-dev-aide.md 6.5, src/rebuild.rs): offered to the full tier where the runtime has
// moq beside it, and refused below the full tier before any terminal opens. Nothing here starts a
// build: the session it would open, and its exit status, are the Terminal's (the lib tests check the command line and
// how a session's end reads).
// Run: ./moq test dev-aide --spec Rebuild
const call = (name, parameters) => {
    try {
        const out = ec.service.sync().name("org.moqui.devaide.LocalRunServices." + name).parameters(parameters || {}).call() || {}
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
        ec.artifactExecution.disableAuthz()
        try {
            const make = (entity, fields) => { const row = ec.entity.makeValue(entity); row.setFields(fields, true, null, null); row.createOrUpdate() }
            const since = new Date(Date.now() - 60000)
            make("moqui.security.UserGroup", {userGroupId: "SPEC_RB_READERS", description: "Dev AIDE rebuild spec"})
            make("moqui.security.UserGroupPermission", {userGroupId: "SPEC_RB_READERS", userPermissionId: "DEV_AIDE_VIEW", fromDate: since})
            make("moqui.security.ArtifactAuthz", {artifactAuthzId: "SPEC_RB_READERS_APP", artifactGroupId: "DEV_AIDE_APP", userGroupId: "SPEC_RB_READERS",
                authzTypeEnumId: "AUTHZT_ALLOW", authzActionEnumId: "AUTHZA_ALL"})
            make("moqui.security.UserAccount", {userId: "specRbReader", username: "spec.rb.reader", userFullName: "Spec Rebuild Reader"})
            make("moqui.security.UserGroupMember", {userGroupId: "SPEC_RB_READERS", userId: "specRbReader", fromDate: since})
        } finally {
            ec.artifactExecution.enableAuthz()
        }
    },
    cleanupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        try {
            const remove = (entity, field, value) => ec.entity.find(entity).condition(field, value).deleteAll()
            remove("moqui.security.UserGroupMember", "userId", "specRbReader")
            remove("moqui.security.UserPreference", "userId", "specRbReader")
            remove("moqui.security.UserAccount", "userId", "specRbReader")
            remove("moqui.security.ArtifactAuthz", "artifactAuthzId", "SPEC_RB_READERS_APP")
            remove("moqui.security.UserGroupPermission", "userGroupId", "SPEC_RB_READERS")
            remove("moqui.security.UserGroup", "userGroupId", "SPEC_RB_READERS")
        } finally {
            ec.artifactExecution.enableAuthz()
        }
    }
})

feature("the full tier is offered a rebuild where the runtime has moq, and a reader is not", {
    when() {
        ec.user.internalLoginUser("john.doe")
        // moq beside the runtime: a checkout's runtime has it, a scratch runtime of the run may not
        const runtime = String(ec.factory.getRuntimePath())
        const moqThere = !!ec.resource.getLocationReference("file:" + runtime.replace(/\/[^/]*\/?$/, "") + "/moq").getExists()
        const full = call("get#Rebuild")
        // a session the Terminal does not list reads as gone
        const gone = call("get#Rebuild", {sessionId: "NO_SUCH_SESSION"})
        ec.user.internalLoginUser("spec.rb.reader")
        const reader = call("get#Rebuild")
        const refused = call("rebuild#Server")
        return {full, gone, reader, refused, moqThere}
    },
    then({full, gone, reader, refused, moqThere}) {
        full.errors == ""
        full.out.available == moqThere
        gone.out.state == "gone"
        reader.errors == ""
        reader.out.available == false
        refused.errors.includes("needs the full tier")
        !refused.out.sessionId
    }
})
