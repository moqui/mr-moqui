// System › Conf › Services and ports (plans/Runtime-Config.md §3.7): the listeners switched from a screen, over
// org.moqui.admin.ListenerServices (whose rules are framework/test/AdminListeners.spec.js's and the Rust unit tests').
// The setListener transition, as a holder of LISTENER_CONTROL: a restart-class switch (SIP) is kept and the answer's
// notes come back as messages; without a recent sign-in it is refused with the reauth header the browser's form answers
// with "Confirm it is you"; a lockout is not an error but the form again, with the confirmation the answer named kept for
// the next render, and confirmed it goes through; a value the Change form showed and the user left as it was (its *Was)
// is not sent, so it is not kept again. A user without the permission is refused it with the same change. The screen is
// rendered once across the suite, inside System Home's dialog, where its lists, controls and kept form are checked
// (AdminOverview.spec.js); it is not rendered here. No listeners run under moqui test, so a change is kept for their start
// (persist N here, a process property) and each setting is put back by the testkit when the spec ends.
// Run: ./moq test tools --spec ServicesPorts
const PASSWORD = "Services-ports-9!"
// in ADMIN and ADMIN_ADV, which holds LISTENER_CONTROL
const HOLDER = "SERVICES_PORTS_HOLDER"
// in ADMIN alone
const VIEWER = "SERVICES_PORTS_VIEWER"
const WEBROOT = "webapp-list/webapp[webroot]@"
const HTTP = WEBROOT + "http-listen-enabled"
const HTTPS = WEBROOT + "https-listen-enabled"
const BIND = WEBROOT + "http-bind"
const PROXIES = WEBROOT + "trusted-proxies"
const SSH = "ssh-server@enabled"
const SIP = "sip-server@enabled"
const LOCKOUT_REASON = "spec: the last way in"

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
const username = userId => userId.toLowerCase().replace(/_/g, ".")
const conf = selector => String(ec.factory.getConfValue(selector) ?? "")
// signed in by an internal login: a session, not a recent sign-in
const signIn = userId => ec.user.internalLoginUser(username(userId))
// the signed-in user's password again: a recent sign-in
const confirm = () =>
    sys(() =>
        ec.service.sync().name("org.moqui.sso.RecentAuthServices.reauthenticate#User").parameters({password: PASSWORD}).call()
    )
const screens = () => ec.screen.makeTest().rootScreen("component://tools/screen/System.xml").renderMode("html")
// a post to the transition as the browser's form sends it (JSON), kept as a process property
function post(parameters) {
    const result = screens()
        .header("Accept", "application/json")
        .render("Conf/ServicesPorts/setListener", Object.assign({persist: "N"}, parameters), "post")
    const body = result.jsonObject || {}
    const errors = [...result.errorMessages, ...(body.errors || [])].join(" | ")
    ec.message.clearAll()
    return {
        status: result.status,
        reauth: result.header("X-Moqui-Reauth"),
        errors,
        opens: (body.screenParameters || {})._openDialog || "",
        messages: Array.from(body.messageInfos || []).map(info => info.type + ": " + info.message)
    }
}
// what the transition kept for the next render of the screen, taken out as the screen takes it
function pending() {
    const kept = ec.web.sessionAttributes.remove("listenerConfirm")
    return kept ? JSON.parse(kept) : {}
}

spec({
    setupSpec() {
        sys(() => {
            for (const [userId, groups] of [
                [HOLDER, ["ADMIN", "ADMIN_ADV"]],
                [VIEWER, ["ADMIN"]]
            ]) {
                if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() === 0) {
                    const account = ec.entity.makeValue("moqui.security.UserAccount")
                    account.setFields(
                        {userId, username: username(userId), userFullName: "Services and ports spec"},
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
        // each setting the features change is put back when the spec ends; they start from HTTPS the only way in (HTTP and
        // SSH off, every address, no proxy) and SIP off
        for (const selector of [HTTP, HTTPS, BIND, PROXIES, SSH, SIP]) setConf(selector, null)
        for (const [selector, value] of [
            [HTTP, "false"],
            [HTTPS, "true"],
            [BIND, ""],
            [PROXIES, ""],
            [SSH, "false"],
            [SIP, "false"]
        ])
            if (conf(selector) !== value) setConf(selector, value)
    },
    cleanupSpec() {
        sys(() => {
            for (const userId of [HOLDER, VIEWER])
                ec.entity.find("moqui.security.UserGroupMember").condition("userId", userId).deleteAll()
        })
        ec.user.internalLoginUser("john.doe")
    }
})

feature(
    "a holder turns SIP on from the screen: kept for the next start, the answer's notes are messages, and Confirm it is you first",
    {
        when() {
            signIn(HOLDER)
            const stale = post({name: "SIP", enabled: "true"})
            const sipAfterStale = conf(SIP)
            confirm()
            const on = post({name: "SIP", enabled: "true"})
            return {stale, sipAfterStale, on, sipAfter: conf(SIP)}
        },
        then({stale, sipAfterStale, on, sipAfter}) {
            // refused for want of a recent sign-in: the error and the header the browser's form answers by asking, then sending again
            stale.errors.includes("Confirm it is you")
            stale.reauth == "required"
            sipAfterStale == "false"
            on.status == 200
            on.errors == ""
            on.opens == ""
            on.messages.includes("success: SIP set on; no listeners run in this process: they read it when they start")
            on.messages.some(
                message =>
                    message.startsWith("info: sip-server@enabled true: set as a process property") &&
                    message.includes("it applies at the next start")
            )
            sipAfter == "true"
        }
    }
)

feature("a lockout is the form again with its confirmation, and confirmed the change goes through", {
    when() {
        signIn(HOLDER)
        confirm()
        const turnOff = {name: "HTTPS", enabled: "false", reason: LOCKOUT_REASON, endNow: "Y"}
        const refused = post(turnOff)
        const kept = pending()
        const httpsKept = conf(HTTPS)
        const confirmed = post(Object.assign({confirmLockout: "Y"}, turnOff))
        return {refused, kept, httpsKept, confirmed, keptAfter: pending(), httpsAfter: conf(HTTPS)}
    },
    then({refused, kept, httpsKept, confirmed, keptAfter, httpsAfter}) {
        // not an error: the refusal as a message, Home's dialog opened, the form kept for the next render with the
        // confirmation the answer named and what was asked
        refused.status == 200
        refused.errors == ""
        refused.opens == "ServicesPorts"
        refused.messages.some(
            message => message.startsWith("warning: Not changed yet:") && message.includes("leaves no way in over the network")
        )
        kept.confirm == "confirmLockout"
        kept.name == "HTTPS"
        kept.enabled == false
        kept.reason == LOCKOUT_REASON
        kept.endNow == true
        kept.persist == false
        kept.message.includes("`moqui operator listener HTTPS on`")
        httpsKept == "true"
        confirmed.errors == ""
        confirmed.messages.some(message => message.startsWith("success: HTTPS set off"))
        confirmed.messages.some(message => message.startsWith("info: No way in over the network is left"))
        keptAfter.confirm == null
        httpsAfter == "false"
    }
})

feature("a user without LISTENER_CONTROL is refused the transition with a change a holder makes", {
    when() {
        setConf(SIP, "false")
        signIn(VIEWER)
        confirm()
        return {refused: post({name: "SIP", enabled: "true"}), sipAfter: conf(SIP)}
    },
    then({refused, sipAfter}) {
        refused.errors.includes("Condition failed for transition")
        refused.messages.length == 0
        sipAfter == "false"
    }
})

// The Change form shows HTTP's port and carries it as portWas: left as it was it is not sent, changed it is. What the
// transition sent is what it keeps for the form when a guard refuses (here the public HTTP one, HTTP being off on every
// address with no proxy).
feature("the Change form sends only what the user changed: a value left as it was is not asked again", {
    when() {
        signIn(HOLDER)
        confirm()
        const port = conf(WEBROOT + "http-listen-port")
        const turnOn = {name: "HTTP", enabled: "true", portWas: port, host: "", hostWas: ""}
        const left = post(Object.assign({port}, turnOn))
        const leftKept = pending()
        const changed = post(Object.assign({port: "18081"}, turnOn))
        return {left, leftKept, changedKept: pending(), httpAfter: conf(HTTP)}
    },
    then({left, leftKept, changedKept, httpAfter}) {
        left.opens == "ServicesPorts"
        leftKept.confirm == "confirmPublicHttp"
        leftKept.port == ""
        changedKept.port == "18081"
        httpAfter == "false"
    }
})

// The kept form's rule, run over the screen's actions with an answer and a kept change of its own (the screen is rendered
// once, on Home): a busy port shows the port and address to change at the running values, carried as their *Was, so left
// as they are they are not sent; a port the request carried is shown as asked and sent as it is.
feature("a busy port's form shows the port to change; a port the request carried is sent as asked", {
    expect() {
        const source = ec.resource.getLocationText("component://tools/screen/System/Conf/ServicesPorts.xml")
        const actions = /^    <actions><script><!\[CDATA\[([\s\S]*?)\]\]><\/script><\/actions>/m.exec(source)[1]
        const http = {
            name: "HTTP",
            state: "not-running",
            enabled: false,
            switchable: true,
            portSelector: WEBROOT + "http-listen-port",
            port: "8080",
            hostSelector: BIND,
            host: "",
            ownSettingList: [],
            subServiceList: [],
            settingList: []
        }
        const kept = more =>
            Object.assign(
                {name: "HTTP", enabled: true, port: "", host: "", access: "", reason: "", persist: true, endNow: false},
                {confirmLockout: false, confirmPublicHttp: true, confirmDependents: false, confirm: "", message: "refused"},
                more
            )
        const run = pending => {
            const state = {
                ec: {
                    l10n: ec.l10n,
                    logger: ec.logger,
                    user: {hasPermission: () => true},
                    message: {hasError: () => false, clearErrors: () => {}},
                    service: {
                        sync: () => ({name: () => ({call: () => ({listenerList: [http], running: false, recentChanges: []})})})
                    },
                    web: {sessionAttributes: {remove: () => JSON.stringify(pending)}}
                },
                servicesAllowed: null,
                servicesAvailable: null,
                servicesRunning: null,
                servicesAccessOptions: null,
                servicesRows: null,
                servicesSettingRows: null,
                servicesSubRows: null,
                servicesChangeRows: null,
                servicesConfirm: null
            }
            runInContext(actions, state)
            return state.servicesConfirm
        }
        const busy = run(kept({reasonCode: "portBusy"}))
        assert(busy.portBusy && busy.showPort && busy.showHost && !busy.showAccess, JSON.stringify(busy))
        assert.equal(busy.port, "8080")
        assert.equal(busy.portWas, "8080")
        assert.equal(busy.confirm, "")
        assert.equal(busy.confirmPublicHttp, "Y")
        const carried = run(kept({port: "18080", confirm: "confirmLockout", reasonCode: "lockout"}))
        assert(carried.showPort && !carried.showHost && !carried.portBusy, JSON.stringify(carried))
        assert.equal(carried.port, "18080")
        assert.equal(carried.portWas, "")
        assert.equal(carried.confirm, "confirmLockout")
    }
})
