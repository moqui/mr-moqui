// BasicComms, the framework's Comms app (plans/Comms-Framework.md §3, A1): every screen under the root renders in qvt (with its
// static data) and in tui; the screens render the same under an app's own mount (IronStone's /qapps/Comms, when it is
// installed) with that app's extensions, and the launcher has one Comms; a user with no party (users only) uses the mail
// settings; the user search leaves out disabled and system accounts; the provider hooks answer for the apps the user may
// open, and a hook nobody declares is unavailable.
// Run: ./moq test basic-comms --spec BasicComms
const root = "component://basic-comms/screen/BasicComms.xml"
const webroot = "component://webroot/screen/webroot.xml"
const USER = "BASIC_COMMS_SPEC"
const DISABLED = "BASIC_COMMS_SPEC_OFF"
const bossInstalled = ec.resource.getLocationReference("component://mr-boss/screen/Comms.xml").getExists()

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
function render(rootLocation, path, mode, parameters) {
    let test = ec.screen.makeTest().rootScreen(rootLocation).renderMode(mode)
    if (mode === "qvt") test = test.withStaticData()
    const str = test.render(path, Object.assign({lastStandalone: "-2"}, parameters || {}), null)
    const output = String(str.output || "")
    ec.message.clearErrors()
    return {
        status: str.status,
        errors: str.errorMessages.join(" | "),
        output,
        broken: /Error rendering|Exception in render/.test(output)
    }
}
// every screen file under the root, as a path from it, with the browser mode it renders in: qvue for a .qvue screen, else qvt
function screens() {
    const files = []
    ec.resource.getLocationReference("component://basic-comms/screen/BasicComms.xml").walkChildTree(files, null)
    return files
        .filter(file => file.location.endsWith(".xml") && file.path !== "BasicComms")
        .map(file => {
            const modes = (/render-modes="([^"]*)"/.exec(ec.resource.getLocationText(file.location, false) || "") || [])[1] || ""
            return {
                path: file.path.substring("BasicComms".length + 1),
                browser: modes.split(",").includes("qvue") ? "qvue" : "qvt"
            }
        })
}
function makeUser(userId, username, disabled) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() > 0) return
        const account = ec.entity.makeValue("moqui.security.UserAccount")
        account.setFields(
            {userId, username, userFullName: "Basic Comms " + username, disabled: disabled ? "Y" : "N"},
            true,
            null,
            null
        )
        account.create()
    })
}

spec({
    setupSpec() {
        makeUser(USER, "basic.comms.spec", false)
        makeUser(DISABLED, "basic.comms.spec.off", true)
    }
})

feature("every BasicComms screen renders in qvt and tui", {
    when() {
        login("john.doe")
        const problems = []
        const found = screens()
        for (const {path, browser} of found) {
            for (const mode of [browser, "tui"]) {
                const shown = render(root, path, mode, {})
                if (shown.status != 200 || shown.broken)
                    problems.push(`${mode} ${path}: ${shown.status} ${shown.errors} ${shown.output.slice(0, 160)}`)
            }
        }
        return {problems, count: found.length}
    },
    then({problems, count}) {
        count > 15
        problems.join("\n") == ""
    }
})

feature("a user with no party sees the mail settings, the wizard and no Internal account without an app for it", {
    when() {
        ec.user.internalLoginUser("basic.comms.spec")
        const index = render(root, "Settings/Index", "qvt", {})
        const list = render(root, "Settings/MailAccounts/List", "qvt", {})
        const wizard = render(root, "Settings/MailAccounts/Add/Address", "qvt", {fresh: "Y"})
        // as the screens call them: authorized by the app's screen, here by the spec
        const accounts = sys(() => call("org.moqui.comms.MailSettingsServices.get#MailAccountList", {})) || {}
        accounts.errors = ec.message.hasError() ? ec.message.errorsString : ""
        ec.message.clearErrors()
        const providers = sys(() => call("org.moqui.impl.ShellServices.get#Providers", {hook: "inbox-source"})) || {}
        login("john.doe")
        const internalShown = Array.from(accounts.accountList || []).some(account => account.isInternal)
        const internalSource = Array.from(providers.providerList || []).length > 0
        return {index, list, wizard, accounts, internalShown, internalSource}
    },
    then({index, list, wizard, accounts, internalShown, internalSource}) {
        index.status == 200
        index.output.includes("Identities and signatures")
        list.status == 200
        !list.broken
        wizard.status == 200
        !wizard.broken
        accounts.errors == ""
        accounts.mailAccountCount == 0
        // the Internal account is listed only when an app the user may open answers for it
        internalShown == internalSource
    }
})

feature("the user search finds active people by name and leaves out disabled and system accounts", {
    when() {
        login("john.doe")
        const byName = sys(() => call("org.moqui.comms.CommsServices.search#Users", {term: "basic comms"})).userList
        const agent = sys(() => call("org.moqui.comms.CommsServices.search#Users", {term: "_agent"})).userList
        return {byName, agent}
    },
    then({byName, agent}) {
        byName.map(user => user.id).includes(USER)
        !byName.map(user => user.id).includes(DISABLED)
        agent.length == 0
    }
})

feature("a hook nobody declares is unavailable and a hook name that is not one is refused", {
    when() {
        login("john.doe")
        const none = sys(() => call("org.moqui.impl.ShellServices.call#Providers", {hook: "call-context"})) || {}
        let refused = ""
        try {
            sys(() => call("org.moqui.impl.ShellServices.call#Providers", {hook: "not-a-hook"}))
        } catch (err) {
            refused = String(err.message || err)
        }
        refused += ec.message.hasError() ? ec.message.errorsString : ""
        ec.message.clearErrors()
        return {none, refused}
    },
    then({none, refused}) {
        none.unavailable == true
        none.results.length == 0
        refused.includes("not a provider hook")
    }
})

feature("an app that mounts BasicComms shows its screens with its extensions under both mounts, and one Comms in the launcher", {
    when() {
        login("john.doe")
        if (!bossInstalled) return {skip: true}
        const own = render(root, "Settings/Index", "qvt", {})
        const app = render("component://mr-boss/screen/Comms.xml", "Settings/Index", "qvt", {})
        const shell = JSON.parse(String(ec.screen.makeTest().rootScreen(webroot).render("shellData", {}, null).output || "{}"))
        const appNames = (shell.apps || []).map(found => found.name)
        const inbox =
            sys(() =>
                call("org.moqui.impl.ShellServices.call#Providers", {hook: "inbox-source", parameters: {operation: "accounts"}})
            ) || {}
        return {own, app, appNames, inbox}
    },
    then({skip, own, app, appNames, inbox}) {
        skip || own.status == 200
        skip || app.status == 200
        // the extension applies by screen location, so through either mount
        skip || own.output.includes("Reading and keys")
        skip || app.output.includes("Reading and keys")
        skip || (appNames.includes("Comms") && !appNames.includes("BasicComms"))
        skip || inbox.results.map(result => result.sourceId).includes("boss.internal")
        skip || inbox.failedParts.length == 0
    }
})

feature("the System Voice screens render in qvt and tui through the System app, and History has Call a number", {
    when() {
        login("john.doe")
        const systemRoot = "component://tools/screen/System.xml"
        // a row in each list, so the row dialogs render too
        const voice = (name, parameters) => sys(() => ec.service.sync().name("org.moqui.impl.VoiceServices." + name).parameters(parameters).call()) || {}
        const trunk = voice("store#VoiceTrunk", {name: "Render trunk", host: "sip.render.test", emergencyCapable: "Y"}).voiceTrunkId
        const location = voice("store#EmergencyLocation", {name: "Render office", address1: "1 Main St", callbackNumber: "+15550109999"}).emergencyLocationId
        voice("store#PhoneNumber", {phoneNumber: "+15550109999", voiceTrunkId: trunk, routeId: USER, emergencyLocationId: location})
        voice("grant#CallerId", {phoneNumber: "+15550109999", userId: USER})
        voice("store#VoiceUserSetting", {userId: USER, outboundEnabled: "Y"})
        voice("record#AuthFailure", {sourceAddress: "198.51.100.7", username: "render"})
        sys(() => ec.service.sync().name("org.moqui.impl.SipDeviceServices.create#SipDevice").parameters({name: "Render phone"}).call())
        // V3: a ring group with a member and a schedule with a holiday
        const handling = (name, parameters) => sys(() => ec.service.sync().name("org.moqui.impl.CallHandlingServices." + name).parameters(parameters).call()) || {}
        const group = handling("store#RingGroup", {name: "Render group"})
        handling("store#RingGroupMember", {ringGroupId: group.ringGroupId, userId: USER})
        const hours = handling("store#CommsSchedule", {name: "Render hours", timeZone: "America/Chicago", shared: true})
        handling("store#CommsScheduleRule", {commsScheduleId: hours.commsScheduleId, ruleTypeEnumId: "CsrHoliday", onDate: "2026-12-25", repeatYearly: "Y"})
        ec.message.clearErrors()
        const problems = []
        const texts = {Status: "SIP servers", Trunks: "Render trunk", Numbers: "+15550109999", CallerIds: "Let someone call from a number",
            Fraud: "198.51.100.7", Emergency: "Render office", Phones: "Render phone", RingGroups: "Render group", Schedules: "Render hours"}
        for (const [path, text] of Object.entries(texts)) {
            for (const mode of ["qvt", "tui"]) {
                const shown = render(systemRoot, "Voice/" + path, mode, {})
                if (shown.status != 200 || shown.broken || !shown.output.includes(text))
                    problems.push(mode + " " + path + ": " + shown.status + " " + shown.errors + " " + shown.output.slice(0, 200))
            }
        }
        const history = render(root, "Calls/History", "qvt", {})
        const historyTui = render(root, "Calls/History", "tui", {})
        // V3: the user's call handling and the dialer, in the terminal
        const handlingTui = render(root, "Settings/CallHandlingTui", "tui", {})
        const dialerTui = render(root, "Calls/DialerTui", "tui", {text: "Render"})
        return {problems, history, historyTui, handlingTui, dialerTui}
    },
    then({problems, history, historyTui, handlingTui, dialerTui}) {
        problems.join("\n") == ""
        handlingTui.output.includes("Caller lists")
        !handlingTui.broken
        dialerTui.output.includes("call from the browser")
        !dialerTui.broken
        history.output.includes("Call a number")
        historyTui.output.includes("Call a number")
    }
})

feature("the component calls no Mantle or IronStone service, reads no Mantle entity and names no app component", {
    when() {
        const problems = []
        for (const dir of ["screen", "service", "template", "data"]) {
            const files = []
            const ref = ec.resource.getLocationReference("component://basic-comms/" + dir)
            if (!ref.getExists()) continue
            ref.walkChildTree(files, null)
            for (const file of files) {
                const text = ec.resource.getLocationText(file.location, false) || ""
                const found = text.match(/(mantle|ironstone)\.[a-z.]+\.[A-Z][A-Za-z]+|component:\/\/mr-[a-z]+/g)
                if (found) problems.push(file.location + ": " + Array.from(new Set(found)).join(", "))
            }
        }
        return {problems}
    },
    then({problems}) {
        problems.join("\n") == ""
    }
})
