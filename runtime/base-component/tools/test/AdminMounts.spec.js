// System's administration screens shown under another app's root (README, "Mounting the administration screens";
// plans/Admin-Screens-Down.md): the mount's root sets adminMount and template/AdminMount.js answers where a screen stands.
// The instance administrator's parts (raw) are never shown under a mount and their transitions are refused there, for a
// delegate whose group holds an inheritable always-allow on the mount, as an app's area grant is, and for the instance
// administrator too; with no mount they are shown to a user who opens System by their own grant, and not to one whom only
// the stack lets through. A mount's routes replace and add to the framework's, a plain one (another app's page) answered as
// a URL with its query string; a value that is not a map is a mount with nothing in it. A System screen's own raw
// transition (JobDetail's updateJob, an entity-auto update the mount's always-allow would pass) is refused under a mount
// that mounts the screen by location, and runs under System, as other areas' do (GroupDetail's updateUserGroup,
// Localization's createLocalizedMessage). And the rule that keeps it so, read from the screens' sources at no render's
// cost: every transition of an area's screens that acts either calls only org.moqui.admin services, which check the
// delegated permissions themselves, or carries raw() as its condition (the Data area has no raw part: its four
// transitions are all of the first kind).
// Run: ./moq test tools --spec AdminMounts
const mount = "component://tools/test/screen/AdminMount.xml"
const probe = "component://tools/test/screen/AdminMount/Probe.xml"
const USER = "ADMIN_MOUNTS_SPEC"
const USERNAME = "admin.mounts.spec"
const GROUP = "ADMIN_MOUNTS_SPEC"
const ADMIN_USERNAME = "john.doe"
const PREFERENCE = "AdminMountsSpec.raw"
const system = "component://tools/screen/System.xml"
const JOB = "moqui.service.job.ServiceJob"
const jobName = "AdminMountsSpec_job"
const MESSAGE = "moqui.basic.LocalizedMessage"
const original = "AdminMountsSpec message"
// The areas of System that another app may show under its root (screen/System/<Area>/): each phase of
// plans/Admin-Screens-Down.md adds its own.
const AREAS = ["Operations", "Integrations", "Protocols", "Access", "Security", "Settings", "Data", "Audit"]
// What an area's directory holds that the rule does not read, as a path under screen/System (a screen and the screens below
// it), with why.
const NOT_AREA_SCREENS = {
    "Security/UserAccount":
        "System's own account administration from before the areas (its transitions are entity-auto, for whoever opens " +
        "System), never shown by an app as a screen of an area; below it the Verify screens of a user's own factors, which " +
        "apps mount by location on purpose"
}
const RAW_CONDITION =
    "<condition><expression>ec.resource.script('component://tools/template/AdminMount.js').raw()</expression></condition>"
// A transition that is deliberately neither delegated nor raw, as "<file under screen/System>#<transition>", with why.
const UNGUARDED = {
    "Security/SignInPolicy.xml#reviewRule":
        "carries the change form's values to the review as parameters: no service, nothing written"
}

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
    const result = ec.screen
        .makeTest()
        .rootScreen(root)
        .render(path, parameters || {}, method || null)
    const output = String(result.output || "")
    const failed = output.match(/(Error rendering|Exception in render)[^\n<]*/g) || []
    const errors = Array.from(result.errorMessages || [])
        .concat(failed)
        .join(" | ")
    ec.message.clearErrors()
    return {output: output.replace(/&quot;/g, '"'), errors}
}
// The screen files under a directory, its subscreens' directories too.
function screenFiles(location, found = []) {
    for (const entry of ec.resource.getLocationReference(location).getDirectoryEntries()) {
        if (entry.isDirectory()) screenFiles(entry.getLocation(), found)
        else if (/\.xml$/.test(entry.getLocation())) found.push(entry.getLocation())
    }
    return found
}
// The transitions of a screen file that act (a service-call or actions), each {name, body}; one a screen takes in from
// another file (transition-include) is read there.
function actingTransitions(location) {
    const source = String(ec.resource.getLocationText(location) || "")
    const found = []
    for (const [, name, body] of source.matchAll(/<transition\s+name="([^"]+)"(?:[^>]*[^\/>])?>([\s\S]*?)<\/transition>/g))
        if (/<service-call\b|<actions>/.test(body)) found.push({name, body})
    for (const [, name, from] of source.matchAll(/<transition-include\s+name="([^"]+)"\s+location="([^"]+)"/g)) {
        const included = actingTransitions(from).find(transition => transition.name === name)
        if (included) found.push(included)
    }
    return found
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
                make("moqui.security.UserGroup", {
                    userGroupId: GROUP,
                    description: "A delegated administrator (AdminMounts spec)"
                })
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
        sys(() => ec.entity.find(JOB).condition("jobName", jobName).deleteAll())
        sys(() => ec.entity.find(MESSAGE).condition("original", original).deleteAll())
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
        // a screen path is not plain; a plain route (another app's page) answers its URL with the query string, the id and
        // what else the link carries in it, an empty value left out
        delegate.output.includes("storePlain=false ../Store?tab=messages")
        delegate.output.includes("mail=true /elsewhere/Mail?accountId=A%201")
        delegate.output.includes("view=queue") && !delegate.output.includes("empty=")
        delegatePost.errors.includes("Condition failed for transition")
        delegateRan == false
        administrator.output.includes("mounted=true raw=false")
        administratorPost.errors.includes("Condition failed for transition")
        administratorRan == false
    }
})

feature(
    "with no mount the raw parts are shown to a user who opens System by their own grant, and a value that is no map shows none",
    {
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
            return {
                delegate,
                delegatePost,
                delegateRan,
                administrator,
                administratorPost,
                administratorRan,
                forged,
                forgedPost,
                forgedRan
            }
        },
        then({
            delegate,
            delegatePost,
            delegateRan,
            administrator,
            administratorPost,
            administratorRan,
            forged,
            forgedPost,
            forgedRan
        }) {
            delegate.errors == ""
            delegate.output.includes("mounted=false raw=false stack=true")
            delegatePost.errors.includes("Condition failed for transition")
            delegateRan == false
            administrator.errors == ""
            administrator.output.includes("mounted=false raw=true")
            // the framework's own route for a user, and no route for a key no one registered
            administrator.output.includes('user=../../Security/UserAccount/UserAccountDetail {"userId":"U1"}')
            administrator.output.includes("store=none") && administrator.output.includes("mail=none")
            administratorPost.errors == ""
            administratorRan == true
            forged.output.includes("mounted=true raw=false")
            forged.output.includes("store=none")
            forgedPost.errors.includes("Condition failed for transition")
            forgedRan == false
        }
    }
)

feature("a System screen's raw transition is refused under a mount that mounts it by location, and runs under System", {
    when() {
        const description = () => sys(() => ec.entity.find(JOB).condition("jobName", jobName).one()?.description)
        sys(() => {
            ec.entity.find(JOB).condition("jobName", jobName).deleteAll()
            make(JOB, {
                jobName,
                description: "before",
                serviceName: "org.moqui.impl.ServiceServices.clean#ServiceJobRun",
                paused: "Y"
            })
        })
        // the delegate's group holds the mount's inheritable always-allow, which passes a transition and its entity operation
        ec.user.internalLoginUser(USERNAME)
        const mounted = screen(mount, "JobDetail/updateJob", {jobName, description: "by the delegate"}, "post")
        const afterMount = description()
        ec.user.internalLoginUser(ADMIN_USERNAME)
        const administratorMounted = screen(
            mount,
            "JobDetail/updateJob",
            {jobName, description: "by the administrator, mounted"},
            "post"
        )
        const afterAdministratorMount = description()
        const own = screen(system, "Operations/JobDetail/updateJob", {jobName, description: "by the administrator"}, "post")
        // System's own root sets the mount to null, whatever a request sends
        const forged = screen(
            system,
            "Operations/JobDetail/updateJob",
            {jobName, description: "past a forged mount", adminMount: "forged"},
            "post"
        )
        // another area's: a group's record changed directly on Access › GroupDetail, here the spec's own group
        const ownGroup = screen(
            system,
            "Access/GroupDetail/updateUserGroup",
            {userGroupId: GROUP, description: "directly"},
            "post"
        )
        const groupDescription = sys(
            () => ec.entity.find("moqui.security.UserGroup").condition("userGroupId", GROUP).one().description
        )
        // and a stored translation row created directly on Settings › Localization
        const ownMessage = screen(
            system,
            "Settings/Localization/createLocalizedMessage",
            {original, locale: "es", localized: "directly"},
            "post"
        )
        const messageText = sys(() => ec.entity.find(MESSAGE).condition({original, locale: "es"}).one()?.localized)
        return {
            mounted,
            afterMount,
            administratorMounted,
            afterAdministratorMount,
            own,
            afterOwn: description(),
            forged,
            ownGroup,
            groupDescription,
            ownMessage,
            messageText
        }
    },
    then({
        mounted,
        afterMount,
        administratorMounted,
        afterAdministratorMount,
        own,
        afterOwn,
        forged,
        ownGroup,
        groupDescription,
        ownMessage,
        messageText
    }) {
        mounted.errors.includes("Condition failed for transition")
        afterMount == "before"
        administratorMounted.errors.includes("Condition failed for transition")
        afterAdministratorMount == "before"
        own.errors == ""
        forged.errors == ""
        afterOwn == "past a forged mount"
        ownGroup.errors == "" && groupDescription == "directly"
        ownMessage.errors == "" && messageText == "directly"
    }
})

feature("every acting transition of an area's screens calls only org.moqui.admin services or carries raw() as its condition", {
    when() {
        const base = "component://tools/screen/System/"
        const problems = []
        let delegated = 0
        let raw = 0
        for (const area of AREAS)
            for (const location of screenFiles(base + area)) {
                const file = location.substring(base.length)
                if (Object.keys(NOT_AREA_SCREENS).some(path => file === path + ".xml" || file.startsWith(path + "/"))) continue
                for (const {name, body} of actingTransitions(location)) {
                    const services = Array.from(body.matchAll(/<service-call\s+name="([^"]+)"/g)).map(match => match[1])
                    if (body.includes(RAW_CONDITION)) raw++
                    else if (services.length > 0 && services.every(service => service.startsWith("org.moqui.admin."))) delegated++
                    else if (!Object.hasOwn(UNGUARDED, file + "#" + name)) problems.push(file + "#" + name)
                }
            }
        return {problems: problems.join(", "), delegated, raw}
    },
    then({problems, delegated, raw}) {
        // a transition named here acts past the delegated services with no raw() condition: under an app's inheritable
        // always-allow it would run for everyone the app lets into the area
        problems == ""
        // the rule read something of each kind: JobDetail's saveJob and updateJob among them, GroupDetail's storeGroup and
        // updateUserGroup, Localization's storeMessage and createLocalizedMessage, and Data's four, all delegated (Backups'
        // runBackup, DataExport's download, ImportDetail's retry and results)
        delegated >= 38
        raw >= 56
    }
})
