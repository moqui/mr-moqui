// System's administration screens shown under another app's root (README, "Mounting the administration screens";
// plans/Admin-Screens-Down.md): the mount's root sets adminMount and template/AdminMount.js answers where a screen stands.
// The instance administrator's parts (raw) are never shown under a mount and their transitions are refused there, for a
// delegate whose group holds an inheritable always-allow on the mount, as an app's area grant is, and for the instance
// administrator too; with no mount they are shown to a user who opens System by their own grant, and not to one whom only
// the stack lets through. A mount's routes replace and add to the framework's; a value that is not a map is a mount with
// nothing in it.
// Run: ./moq test tools --spec AdminMounts
const mount = "component://tools/test/screen/AdminMount.xml"
const probe = "component://tools/test/screen/AdminMount/Probe.xml"
const USER = "ADMIN_MOUNTS_SPEC"
const USERNAME = "admin.mounts.spec"
const GROUP = "ADMIN_MOUNTS_SPEC"
const ADMIN_USERNAME = "john.doe"
const PREFERENCE = "AdminMountsSpec.raw"

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
function make(entityName, fields) {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
// A render or post as the signed-in user with authorization on: its errors and output.
function screen(root, path, parameters, method) {
    ec.artifactExecution.enableAuthz()
    const result = ec.screen.makeTest().rootScreen(root).render(path, parameters || {}, method || null)
    const output = String(result.output || "")
    const failed = output.match(/(Error rendering|Exception in render)[^\n<]*/g) || []
    const errors = Array.from(result.errorMessages || [])
        .concat(failed)
        .join(" | ")
    ec.message.clearErrors()
    return {output: output.replace(/&quot;/g, '"'), errors}
}
// Whether the probe's raw transition ran for the signed-in user, forgetting it for the next post.
function rawRan() {
    const ran = ec.user.getPreference(PREFERENCE) === "ran"
    ec.user.setPreference(PREFERENCE, "")
    return ran
}

spec({
    setupSpec() {
        sys(() => {
            if (ec.entity.find("moqui.security.UserAccount").condition("userId", USER).count() === 0) {
                const from = new Date(Date.now() - 60000)
                make("moqui.security.UserAccount", {
                    userId: USER,
                    username: USERNAME,
                    userFullName: "Admin Mounts Spec",
                    disabled: "N"
                })
                make("moqui.security.UserGroup", {userGroupId: GROUP, description: "A delegated administrator (AdminMounts spec)"})
                make("moqui.security.UserGroupMember", {userGroupId: GROUP, userId: USER, fromDate: from})
                // the mount's root and the probe on its own, each opened as an app's area is: an inheritable always-allow, to the
                // delegate's group and to the instance administrator's; neither grant names System
                make("moqui.security.ArtifactGroup", {artifactGroupId: GROUP, description: "AdminMounts spec"})
                for (const artifactName of [mount, probe])
                    make("moqui.security.ArtifactGroupMember", {
                        artifactGroupId: GROUP,
                        artifactName,
                        artifactTypeEnumId: "AT_XML_SCREEN",
                        inheritAuthz: "Y"
                    })
                for (const userGroupId of [GROUP, "ADMIN"])
                    make("moqui.security.ArtifactAuthz", {
                        artifactAuthzId: GROUP + "_" + userGroupId,
                        artifactGroupId: GROUP,
                        userGroupId,
                        authzTypeEnumId: "AUTHZT_ALWAYS",
                        authzActionEnumId: "AUTHZA_ALL"
                    })
            }
        })
    },
    cleanupSpec() {
        ec.message.clearErrors()
        ec.artifactExecution.enableAuthz()
    }
})

feature("under a mount nothing raw is shown and a raw transition is refused, for a delegate and for the instance administrator", {
    when() {
        ec.user.internalLoginUser(USERNAME)
        const delegate = screen(mount, "Probe")
        const delegatePost = screen(mount, "Probe/rawOnly", {}, "post")
        const delegateRan = rawRan()
        ec.user.internalLoginUser(ADMIN_USERNAME)
        const administrator = screen(mount, "Probe")
        const administratorPost = screen(mount, "Probe/rawOnly", {}, "post")
        const administratorRan = rawRan()
        return {delegate, delegatePost, delegateRan, administrator, administratorPost, administratorRan}
    },
    then({delegate, delegatePost, delegateRan, administrator, administratorPost, administratorRan}) {
        delegate.errors == ""
        // the stack would open System to the delegate (the mount's inheritable always-allow); their own records do not
        delegate.output.includes("mounted=true raw=false stack=true hidden=scheduler")
        // the mount's route for a user replaces the framework's, and its own route is added
        delegate.output.includes('user=../Staff {"staffUserId":"U1"}')
        delegate.output.includes('store=../Store {"tab":"messages","storeId":"S1"}')
        delegatePost.errors.includes("Condition failed for transition")
        delegateRan == false
        administrator.output.includes("mounted=true raw=false")
        administratorPost.errors.includes("Condition failed for transition")
        administratorRan == false
    }
})

feature("with no mount the raw parts are shown to a user who opens System by their own grant, and a value that is no map shows none", {
    when() {
        ec.user.internalLoginUser(USERNAME)
        const delegate = screen(probe, "")
        const delegatePost = screen(probe, "rawOnly", {}, "post")
        const delegateRan = rawRan()
        ec.user.internalLoginUser(ADMIN_USERNAME)
        const administrator = screen(probe, "")
        const administratorPost = screen(probe, "rawOnly", {}, "post")
        const administratorRan = rawRan()
        // a root that sets no mount: a request parameter is a mount with nothing in it, so it adds no route and shows nothing raw
        const forged = screen(probe, "", {adminMount: "forged"})
        const forgedPost = screen(probe, "rawOnly", {adminMount: "forged"}, "post")
        const forgedRan = rawRan()
        return {delegate, delegatePost, delegateRan, administrator, administratorPost, administratorRan, forged, forgedPost, forgedRan}
    },
    then({delegate, delegatePost, delegateRan, administrator, administratorPost, administratorRan, forged, forgedPost, forgedRan}) {
        delegate.errors == ""
        delegate.output.includes("mounted=false raw=false stack=true")
        delegatePost.errors.includes("Condition failed for transition")
        delegateRan == false
        administrator.errors == ""
        administrator.output.includes("mounted=false raw=true")
        // the framework's own route for a user, and no route for a key no one registered
        administrator.output.includes('user=../../Security/UserAccount/UserAccountDetail {"userId":"U1"}')
        administrator.output.includes("store=none")
        administratorPost.errors == ""
        administratorRan == true
        forged.output.includes("mounted=true raw=false")
        forged.output.includes("store=none")
        forgedPost.errors.includes("Condition failed for transition")
        forgedRan == false
    }
})
