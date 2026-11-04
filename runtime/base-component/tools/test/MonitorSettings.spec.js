// Monitor › Settings (plans/Monitor-Settings.md): the runtime settings services and screen. Listing needs Monitor
// access; a change needs MONITOR_SETTINGS. The spec's own user has Monitor access only, through a group of its own,
// until the second feature grants that group MONITOR_SETTINGS. The setting changed is the receiver's rate limit, which
// the conf leaves to its default.
// Run: ./moq test tools --spec MonitorSettings
const SERVICES = "org.moqui.monitor.MonitorServices."
const KEY = "telemetry/receiver@requests-per-minute"
const monitorRoot = "component://tools/screen/Monitor.xml"
const CHANGE = {key: KEY, value: "42", minutes: 15, reason: "monitor settings spec"}
const USER = "MONITOR_SETTINGS_SPEC"
const USERNAME = "monitor.settings.spec"
const GROUP = "MONITOR_SETTINGS_SPEC"

function sys(run) {
    ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        ec.artifactExecution.enableAuthz()
    }
}
function make(entityName, fields) {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
// The errors a call left, cleared.
function errorsOf(name, parameters) {
    ec.service.sync().name(SERVICES + name).parameters(parameters || {}).call()
    const errors = ec.message.getErrorsString()
    ec.message.clearErrors()
    return errors
}
function setting(key) {
    return Array.from(call(SERVICES + "list#RuntimeSettings").settings || []).find((row) => row.key === key)
}
function render() {
    const test = ec.screen.makeTest().rootScreen(monitorRoot).renderMode("qvt").withStaticData()
    const str = test.render("Settings", {lastStandalone: "-2"}, null)
    const output = String(str.output || "")
    ec.message.clearErrors()
    return {status: str.status, errors: str.errorMessages.join(" | "), output, broken: /Error rendering|Exception in render/.test(output)}
}
function grant() {
    sys(() => make("moqui.security.UserGroupPermission", {userGroupId: GROUP, userPermissionId: "MONITOR_SETTINGS", fromDate: new Date(Date.now() - 60000)}))
}
function ungrant() {
    sys(() => {
        const rows = ec.entity.find("moqui.security.UserGroupPermission").condition("userGroupId", GROUP).useCache(false).list()
        for (const row of rows) row.delete()
    })
}

spec({
    setupSpec() {
        sys(() => {
            if (ec.entity.find("moqui.security.UserAccount").condition("userId", USER).count() === 0) {
                make("moqui.security.UserAccount", {userId: USER, username: USERNAME, userFullName: "Monitor Settings Spec", disabled: "N"})
                make("moqui.security.UserGroup", {userGroupId: GROUP, description: "Monitor app only (MonitorSettings spec)"})
                make("moqui.security.UserGroupMember", {userGroupId: GROUP, userId: USER, fromDate: new Date(Date.now() - 60000)})
                make("moqui.security.ArtifactAuthz", {artifactAuthzId: GROUP, artifactGroupId: "MONITOR_APP", userGroupId: GROUP,
                    authzTypeEnumId: "AUTHZT_ALLOW", authzActionEnumId: "AUTHZA_ALL"})
            }
        })
        ec.user.internalLoginUser(USERNAME)
    },
    cleanupSpec() {
        // nothing may stay overridden for the specs after this one
        sys(() => {
            grant()
            ec.service.sync().name(SERVICES + "revert#AllRuntimeSettings").call()
            ec.message.clearErrors()
        })
        ungrant()
    }
})

feature("without MONITOR_SETTINGS a Monitor user sees the settings and cannot change them", {
    when() {
        return {row: setting(KEY), refused: errorsOf("set#RuntimeSetting", CHANGE), page: render(), after: setting(KEY)}
    },
    then({row, refused, page, after}) {
        row.onDisk == "600"
        row.source == "default"
        row.effective == "600"
        row.override == null
        refused.includes("MONITOR_SETTINGS permission is required")
        after.override == null
        page.status == 200
        !page.broken
        page.errors == ""
        page.output.includes("Changing these settings needs the MONITOR_SETTINGS permission")
        page.output.includes("Requests per minute")
        !page.output.includes("Change…")
        !page.output.includes("Add a target rule")
    }
})

feature("with MONITOR_SETTINGS a set shows beside the on-disk value until it is reverted", {
    given() {
        grant()
    },
    when() {
        const set = call(SERVICES + "set#RuntimeSetting", CHANGE)
        const during = setting(KEY)
        const page = render()
        const badDuration = errorsOf("set#RuntimeSetting", Object.assign({}, CHANGE, {minutes: 30}))
        const unknown = errorsOf("set#RuntimeSetting", Object.assign({}, CHANGE, {key: "telemetry/receiver@concurrent-requests"}))
        const reverted = call(SERVICES + "revert#RuntimeSetting", {key: KEY}).reverted
        const after = setting(KEY)
        ungrant()
        return {set, during, page, badDuration, unknown, reverted, after}
    },
    then({set, during, page, badDuration, unknown, reverted, after}) {
        set.override.value == "42"
        set.onDisk == "600"
        set.extended == false
        during.effective == "42"
        during.onDisk == "600"
        during.override.setBy == ec.user.userId
        during.override.reason == "monitor settings spec"
        page.status == 200
        !page.broken
        page.errors == ""
        page.output.includes("Active overrides")
        page.output.includes("Revert all")
        page.output.includes("Change…")
        page.output.includes("Add a target rule")
        badDuration.includes("15, 60, 240 or 1440")
        unknown.includes("is not a setting that can be changed here")
        reverted == true
        after.effective == "600"
        after.override == null
    }
})
