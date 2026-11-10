/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */
// The demo lock-down (README.md): the stubbed tool screens, the security rows, what john.doe may still do, the guarded
// account services, and the servers that are off. The component is off by default, so turn it on for the run:
// ./moq test demo -D demo_disabled=false

const DISABLED_TEXT = "This screen is disabled on this demo server."
const REFUSED_TEXT = "The demo login cannot be changed on this demo server."
const USER_SERVICES = "org.moqui.impl.UserServices."
const JOHN = "EX_JOHN_DOE"

const stubbed = [
    ["component://tools/screen/Tools.xml", "Sql"],
    ["component://tools/screen/Tools.xml", "SqlTui"],
    ["component://tools/screen/Tools.xml", "SqlDownload"],
    ["component://tools/screen/Tools.xml", "Data/Load"],
    ["component://tools/screen/Tools.xml", "Data/Export"],
    ["component://tools/screen/Tools.xml", "Data/Snapshots"],
    ["component://tools/screen/Tools.xml", "Database/SpeedTest"],
    ["component://tools/screen/Tools.xml", "Database/Activity"],
    ["component://tools/screen/Tools.xml", "Database/Queries"],
    ["component://tools/screen/Tools.xml", "Database/Connections"],
    ["component://tools/screen/Tools.xml", "Service/ServiceRun"],
    ["component://tools/screen/Tools.xml", "Service/ServiceLoadRunner"],
]
const viewOnlyAuthz = [
    "TOOLS_APP_ADMIN", "SYSTEM_APP_ADMIN", "MOQUI_API_ADMIN", "EntitySyncServicesADMIN", "SystemMessageServicesADMIN",
    "ResourceCommandServicesADMIN",
    "GitRepositoriesADMIN", "MONITOR_APP_ADMIN", "REPORT_GIT_SYNC_ADMIN", "REPORT_CONTACT_ADMIN",
]
// ended for ADMIN (data/DemoSecurityData.xml)
const endedPermissions = [
    "SearchRemote", "DashboardRemote", "ADMIN_PASSWORD", "SSH_SFTP", "SSH_GIT", "MONITOR_INGEST", "REPORT_SAVE_LOCATION",
    "MONITOR_RECOVER", "MONITOR_RECOVER_ADMIN", "MONITOR_SETTINGS",
]
// what john.doe no longer has: those, but SSH_GIT, which BOSS's project groups also grant (SSH is off), and the
// ADMIN_ADV permissions
const johnLacks = endedPermissions.filter((id) => id != "SSH_GIT").concat(["SQL_RUNNER_WEB", "SSH_SHELL", "ADMIN_LOGIN_AS"])

// The errors a service call left, cleared. Authorization is off, as for a call from a screen the user may open: the
// refusal is by the account changed, not by the caller's rights.
function errorsOf(name, parameters) {
    ec.artifactExecution.disableAuthz()
    try {
        ec.service.sync().name(name).parameters(parameters).call()
    } finally {
        ec.artifactExecution.enableAuthz()
    }
    const errors = ec.message.getErrorsString()
    ec.message.clearErrors()
    return errors
}

function johnAccount() {
    return ec.entity.find("moqui.security.UserAccount").condition("userId", JOHN).disableAuthz().one()
}

spec({
    setupSpec() { login("john.doe") },
})

// One stub renders the disabled screen; the others share its template (data/DemoScreenData.xml), so rendering each one
// again checks nothing more. Every stub is checked through the menus below.
feature("a stubbed screen renders the disabled screen", {
    expect() {
        const [root, path] = stubbed.find(([, path]) => path == "Sql")
        // qvt sends the screens above as the shell and loads the last one on its own, as lastStandalone asks
        const str = ec.screen.makeTest().rootScreen(root).renderMode("qvt").render(path, { lastStandalone: "true" }, null)
        str.status == 200
        str.errorMessages.length == 0
        str.output.includes(DISABLED_TEXT)
    },
})

feature("the Tools, Data, Database and Service menus leave the stubs out", {
    expect() {
        // the menus the browser shell reads
        const menu = (path) => String(ec.screen.makeTest().rootScreen("component://webroot/screen/webroot.xml")
            .render("menuData/apps/tools/" + path, {}, null).output)
        const tools = menu("dashboard")
        const data = menu("Data")
        const database = menu("Database")
        const service = menu("Service")
        tools.includes(`"Schema"`)
        !tools.includes(`"SQL"`)
        data.includes(`"Grid"`)
        !["Load", "Export", "Snapshots"].some((title) => data.includes(`"${title}"`))
        database.includes(`"Overview"`)
        !["Speed Test", "Activity", "Queries"].some((title) => database.includes(`"${title}"`))
        ![tools, data, database].some((text) => text.includes(`"Disabled"`))
        service.includes(`"Service Reference"`)
        !["Service Run", "Service Load Runner"].some((title) => service.includes(`"${title}"`))
    },
})

feature("the security rows are the demo's", {
    expect() {
        const authz = ec.entity.find("moqui.security.ArtifactAuthz").condition("artifactAuthzId", "in", viewOnlyAuthz)
            .disableAuthz().list()
        const permissions = ec.entity.find("moqui.security.UserGroupPermission").condition("userGroupId", "ADMIN")
            .condition("userPermissionId", "in", endedPermissions).disableAuthz().list()
        const advanced = ec.entity.find("moqui.security.UserGroupMember").condition("userGroupId", "ADMIN_ADV")
            .condition("userId", JOHN).disableAuthz().list()
        authz.length == viewOnlyAuthz.length
        authz.every((row) => row.authzActionEnumId == "AUTHZA_VIEW")
        permissions.length == endedPermissions.length
        permissions.every((row) => row.thruDate != null)
        advanced.length == 1
        advanced[0].thruDate != null
    },
})

feature("john.doe has none of the ended permissions", {
    expect() {
        johnLacks.filter((permission) => ec.user.hasPermission(permission)).join(", ") == ""
    },
})

// the view-only rights of the demo: each row's answer, with the rows that differ named
const permittedRows = [
        { name: "component://tools/screen/Tools.xml", type: "AT_XML_SCREEN", action: "view", allowed: true },
        { name: "component://tools/screen/Tools.xml", type: "AT_XML_SCREEN", action: "update", allowed: false },
        { name: "component://tools/screen/System.xml", type: "AT_XML_SCREEN", action: "view", allowed: true },
        { name: "component://tools/screen/System.xml", type: "AT_XML_SCREEN", action: "update", allowed: false },
        { name: "component://tools/screen/Monitor.xml", type: "AT_XML_SCREEN", action: "update", allowed: false },
        { name: "/moqui", type: "AT_REST_PATH", action: "update", allowed: false },
        { name: "org.moqui.impl.EntitySyncServices.put#EntitySyncData", type: "AT_SERVICE", action: "all", allowed: false },
        { name: "org.moqui.impl.SystemMessageServices.receive#IncomingSystemMessage", type: "AT_SERVICE", action: "all",
            allowed: false },
        // reports stay editable
        { name: "component://tools/screen/Dashboards.xml", type: "AT_XML_SCREEN", action: "update", allowed: true },
].map((row) => ({ ...row, can: row.allowed ? "can" : "cannot" }))

feature("john.doe has view-only rights, and reports stay editable", {
    expect() {
        permittedRows.filter((row) => ec.artifactExecution.isPermitted(row.name, row.type, row.action) != row.allowed)
            .map((row) => `${row.can} ${row.action} ${row.name}`).join("; ") == ""
    },
})

// the account changes the shared login refuses
const refusedCalls = [
        ["update#Password", { userId: JOHN, oldPassword: "moqui", newPassword: "Demo-pass-1", newPasswordVerify: "Demo-pass-1" }],
        ["update#Password", { username: "john.doe", oldPassword: "moqui", newPassword: "Demo-pass-1",
            newPasswordVerify: "Demo-pass-1" }],
        ["reset#Password", { username: "john.doe" }],
        ["update#UserAccount", { userId: JOHN, username: "john.doe.2" }],
        ["update#UserAccount", { userId: JOHN, disabled: "Y" }],
        ["update#UserAccount", { userId: JOHN, requirePasswordChange: "Y" }],
        ["disable#UserAccount", { userId: JOHN }],
        ["create#UserAuthcFactorEmail", { userId: JOHN, factorOption: "visitor@example.com" }],
        ["create#UserAuthcFactorSms", { userId: JOHN, factorOption: "+15555550100" }],
        ["create#UserAuthcFactorTotp", { userId: JOHN }],
        ["setup#UserAuthcFactorTotp", { userId: JOHN }],
        ["create#SingleUseAuthcCodes", { userId: JOHN }],
        ["create#SingleUseAuthcFactor", { userId: JOHN }],
].map(([service, parameters]) => ({ service, parameters }))

feature("every account change is refused for the shared login", {
    expect() {
        // the refusal alone, with no failed commit after it; the calls answered otherwise are named
        refusedCalls.map(({ service, parameters }) => [service, errorsOf(USER_SERVICES + service, parameters)])
            .filter(([, errors]) => errors != REFUSED_TEXT).map(([service, errors]) => `${service}: ${errors}`)
            .join("; ") == ""
    },
})

feature("the shared login still works and is unchanged", {
    when() {
        logout()
        login("john.doe")
        const factors = ec.entity.find("moqui.security.UserAuthcFactor").condition("userId", JOHN).disableAuthz().list()
        return { account: johnAccount(), factors }
    },
    then({ account, factors }) {
        ec.user.userId == JOHN
        account.username == "john.doe"
        account.disabled != "Y"
        account.requirePasswordChange != "Y"
        factors.length == 0
    },
})

feature("john.doe's name and locale may still change", {
    expect() {
        const account = johnAccount()
        errorsOf(USER_SERVICES + "update#UserAccount", { userId: JOHN, userFullName: account.userFullName,
            locale: account.locale }) == ""
    },
})

feature("another user still changes their own password", {
    when() {
        ec.artifactExecution.disableAuthz()
        try {
            call(USER_SERVICES + "create#UserAccount", { username: "demo.spec.other", newPassword: "Other-pass-1",
                newPasswordVerify: "Other-pass-1", emailAddress: "demo.spec.other@example.com" })
        } finally {
            ec.artifactExecution.enableAuthz()
        }
        logout()
        login("demo.spec.other", "Other-pass-1")
        const errors = errorsOf(USER_SERVICES + "update#Password", { oldPassword: "Other-pass-1",
            newPassword: "Other-pass-2", newPasswordVerify: "Other-pass-2" })
        logout()
        login("demo.spec.other", "Other-pass-2")
        const userId = ec.user.userId
        logout()
        login("john.doe")
        return { errors, userId }
    },
    then({ errors, userId }) {
        errors == ""
        userId != null
        userId != JOHN
    },
})

feature("DAV is off and the sync addresses offer no SFTP", {
    when() {
        ec.artifactExecution.disableAuthz()
        try {
            return { out: call("org.moqui.resource.DavServices.get#SyncAddresses", {}) }
        } finally {
            ec.artifactExecution.enableAuthz()
        }
    },
    then({ out }) {
        out.enabled == false
        out.sftpUrl == null
    },
})
