// The listeners switched by a person, org.moqui.admin.ListenerServices (plans/Runtime-Config.md §3.6;
// crates/moqui-core/src/service/listeners.rs over moqui_core::listeners, whose guards are unit tested in
// crates/moqui-core/src/listeners/manage_tests.rs): set#Listener needs the sealed LISTENER_CONTROL beside ADMIN (an ADMIN
// without it is refused, naming it, where a holder of it through ADMIN_ADV is not) and a recent sign-in; turning a
// listener off needs a reason; leaving no way in over the network (SSH on loopback is none, nor is HTTP moved to loopback,
// which HTTPS shares), and turning plain HTTP on at a public address with no proxy, are refused until confirmed; endNow
// is taken; get#Listeners lists a row per switch with where each setting comes from. No listeners run
// under moqui test, so a change is kept for their start (persist false here, a process property) and each setting this
// spec changes is put back by the testkit when it ends.
// Run: ./moq test framework --spec AdminListeners
const S = "org.moqui.admin.ListenerServices."
const PASSWORD = "Listener-spec-9!"
// in ADMIN and ADMIN_ADV, which holds LISTENER_CONTROL
const HOLDER = "LISTENER_SPEC_HOLDER"
// in ADMIN alone
const ADMIN = "LISTENER_SPEC_ADMIN"
const WEBROOT = "webapp-list/webapp[webroot]@"
const HTTP = WEBROOT + "http-listen-enabled"
const HTTPS = WEBROOT + "https-listen-enabled"
const SSH = "ssh-server@enabled"
const BIND = WEBROOT + "http-bind"
const PROXIES = WEBROOT + "trusted-proxies"
const SOURCES = ["conf file", "runtime/.env", "the environment", "-D", "set while running, not kept past a restart"]

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
const username = userId => userId.toLowerCase().replace(/_/g, ".")
const one = (entityName, conditions) => sys(() => ec.entity.find(entityName).condition(conditions).one())
// a call as the signed-in user, with authorization on as a screen's transition runs it
function call(name, parameters) {
    const wasDisabled = ec.artifactExecution.authzDisabled()
    if (wasDisabled) ec.artifactExecution.enableAuthz()
    let out = {}
    try {
        out =
            ec.service
                .sync()
                .name(name)
                .parameters(parameters || {})
                .call() || {}
    } catch (err) {
        out = {thrown: String(err.message || err)}
    } finally {
        if (wasDisabled) ec.artifactExecution.disableAuthz()
    }
    const errors = [...ec.message.errors].join(" | ")
    ec.message.clearAll()
    return Object.assign({errors}, out)
}
// signed in by an internal login: a session, not a recent sign-in
const signIn = userId => ec.user.internalLoginUser(username(userId))
// the signed-in user's password again: a recent sign-in
const confirm = () =>
    sys(() =>
        ec.service.sync().name("org.moqui.sso.RecentAuthServices.reauthenticate#User").parameters({password: PASSWORD}).call()
    )
const set = parameters => call(S + "set#Listener", Object.assign({persist: false}, parameters))
const conf = selector => String(ec.factory.getConfValue(selector) ?? "")

spec({
    setupSpec() {
        sys(() => {
            for (const [userId, groups] of [
                [HOLDER, ["ADMIN", "ADMIN_ADV"]],
                [ADMIN, ["ADMIN"]]
            ]) {
                if (one("moqui.security.UserAccount", {userId}) == null) {
                    const account = ec.entity.makeValue("moqui.security.UserAccount")
                    account.setFields(
                        {userId, username: username(userId), userFullName: "Listener spec " + userId.slice(14)},
                        true,
                        null,
                        null
                    )
                    account.create()
                }
                const account = ec.entity.find("moqui.security.UserAccount").condition("userId", userId).one()
                account.setFields(
                    {
                        currentPassword: ec.user.hashPassword(PASSWORD),
                        passwordHashType: "argon2",
                        passwordSalt: "",
                        passwordBase64: "N",
                        successiveFailedLogins: 0,
                        disabled: "N",
                        disabledDateTime: null,
                        requirePasswordChange: "N"
                    },
                    true,
                    null,
                    null
                )
                account.update()
                ec.entity.find("moqui.security.UserAuthcFactor").condition("userId", userId).deleteAll()
                ec.entity.find("moqui.security.UserGroupMember").condition("userId", userId).deleteAll()
                for (const userGroupId of groups) {
                    const member = ec.entity.makeValue("moqui.security.UserGroupMember")
                    member.setFields({userGroupId, userId, fromDate: 0}, true, null, null)
                    member.create()
                }
            }
        })
        // each listener setting the features change is put back when the spec ends; they start from HTTPS on, HTTP off on
        // every address with no proxy believed in front, and SSH on, as the dev conf has them
        for (const selector of [HTTP, HTTPS, SSH, BIND, PROXIES]) setConf(selector, null)
        for (const [selector, value] of [
            [HTTP, "false"],
            [HTTPS, "true"],
            [SSH, "true"],
            [BIND, ""],
            [PROXIES, ""]
        ])
            if (conf(selector) !== value) setConf(selector, value)
    },
    cleanupSpec() {
        sys(() => {
            for (const userId of [HOLDER, ADMIN])
                ec.entity.find("moqui.security.UserGroupMember").condition("userId", userId).deleteAll()
        })
        ec.user.logoutUser()
    }
})

feature("set#Listener needs LISTENER_CONTROL beside ADMIN, and a recent sign-in", {
    when() {
        const sshOff = {name: "SSH", enabled: false, reason: "spec: SSH off"}
        signIn(ADMIN)
        confirm()
        const admin = set(sshOff)
        const sshAfterAdmin = conf(SSH)
        signIn(HOLDER)
        const stale = set(sshOff)
        confirm()
        const holder = set(sshOff)
        return {admin, sshAfterAdmin, stale, holder, sshAfter: conf(SSH)}
    },
    then({admin, sshAfterAdmin, stale, holder, sshAfter}) {
        admin.errors.includes("Changing a listener needs the LISTENER_CONTROL permission")
        admin.reasonCode == "permission"
        sshAfterAdmin == "true"
        stale.errors.includes("Confirm it is you")
        stale.reasonCode == "reauthRequired"
        holder.errors == ""
        holder.enabled == false
        holder.applied == "no-listeners"
        holder.settingList[0].selector == "ssh-server@enabled"
        holder.settingList[0].value == "false"
        holder.settingList[0].kept == "a process property"
        holder.message.includes("no listeners run in this process")
        sshAfter == "false"
    }
})

feature("turning a listener off needs a reason; turning off the last way in is refused until confirmed, naming the ways back", {
    when() {
        signIn(HOLDER)
        confirm()
        const noReason = set({name: "HTTPS", enabled: false})
        const ssh = set({name: "SSH", enabled: false, reason: "spec: SSH off"})
        const last = set({name: "HTTPS", enabled: false, reason: "spec: the last way in"})
        const httpsKept = conf(HTTPS)
        const confirmed = set({name: "HTTPS", enabled: false, reason: "spec: the last way in", confirmLockout: true, endNow: true})
        return {noReason, ssh, last, httpsKept, confirmed, httpsAfter: conf(HTTPS)}
    },
    then({noReason, ssh, last, httpsKept, confirmed, httpsAfter}) {
        noReason.errors.includes("A reason is required to turn HTTPS off")
        noReason.reasonCode == "invalid"
        ssh.errors == ""
        last.reasonCode == "lockout"
        last.confirm == "confirmLockout"
        last.errors.includes("leaves no way in over the network")
        last.errors.includes("`moqui operator listener HTTPS on`")
        last.errors.includes("held as a process property")
        httpsKept == "true"
        confirmed.errors == ""
        confirmed.noteList.some(note => note.startsWith("No way in over the network is left"))
        httpsAfter == "false"
    }
})

feature("plain HTTP on at a public address with no proxy is refused until confirmed; on loopback, which HTTPS shares, no way in is left", {
    when() {
        signIn(HOLDER)
        confirm()
        const open = set({name: "HTTP", enabled: true})
        const httpKept = conf(HTTP)
        const confirmed = set({name: "HTTP", enabled: true, confirmPublicHttp: true})
        const local = set({name: "HTTP", enabled: true, host: "127.0.0.1"})
        return {open, httpKept, confirmed, local, bind: conf(BIND)}
    },
    then({open, httpKept, confirmed, local, bind}) {
        open.reasonCode == "publicHttp"
        open.confirm == "confirmPublicHttp"
        open.errors.includes("with no proxy believed in front")
        httpKept == "false"
        confirmed.errors == ""
        confirmed.enabled == true
        local.reasonCode == "lockout"
        local.errors.includes("leaves no way in over the network")
        bind == ""
    }
})

feature("get#Listeners lists a row per switch, for ADMIN, with where each setting comes from", {
    when() {
        signIn(HOLDER)
        confirm()
        set({name: "SSH", enabled: false, reason: "spec: SSH off"})
        signIn(ADMIN)
        const listed = call(S + "get#Listeners")
        const rows = listed.listenerList || []
        const ssh = rows.find(row => row.name == "SSH") || {settingList: []}
        const sshEnabled = ssh.settingList.find(setting => setting.selector == SSH) || {}
        const unknown = rows.flatMap(row =>
            row.settingList.filter(setting => !SOURCES.includes(setting.source)).map(setting => setting.selector)
        )
        return {listed, rows, ssh, sshEnabled, unknown}
    },
    then({listed, rows, ssh, sshEnabled, unknown}) {
        listed.errors == ""
        listed.running == false
        rows.length == 9
        rows.map(row => row.name).join(",") == "HTTP,HTTPS,gRPC,SSH,SMTP,XMPP,SIP,Call media,TURN"
        ssh.state == "not-running"
        ssh.wayIn == true
        ssh.enabled == false
        ssh.settingList.map(setting => setting.selector).includes("ssh-server@access")
        sshEnabled.value == "false"
        sshEnabled.source == "set while running, not kept past a restart"
        sshEnabled.envName == "moqui_conf__ssh_server__enabled"
        unknown.length == 0
    }
})
