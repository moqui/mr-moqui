// BasicComms, the framework's Comms app (plans/Comms-Framework.md §3, A1): the screens render the same under an app's own
// mount (IronStone's /qapps/Comms, when it is installed) with that app's extensions, and the launcher has one Comms; a
// user with no party (users only) uses the mail settings; the user search leaves out disabled and system accounts; the
// provider hooks answer for the apps the user may open, and a hook nobody declares is unavailable; Settings, Chat apps
// says which app passwords work with XMPP apps. The every-screen render is not a regression test (one sweep per app per
// mode belongs in acceptance/).
// Run: ./moq test basic-comms --spec BasicComms
const root = "component://basic-comms/screen/BasicComms.xml"
const webroot = "component://webroot/screen/webroot.xml"
const USER = "BASIC_COMMS_SPEC"
const DISABLED = "BASIC_COMMS_SPEC_OFF"
const bossInstalled = !!ec.resource.getLocationReference("component://mr-boss/screen/Comms.xml")?.getExists()

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
function makeUser(userId, username, disabled) {
    sys(() => {
        if (ec.entity.find("moqui.security.UserAccount").condition("userId", userId).count() == 0) {
            const account = ec.entity.makeValue("moqui.security.UserAccount")
            account.setFields(
                {userId, username, userFullName: "Basic Comms " + username, disabled: disabled ? "Y" : "N"},
                true,
                null,
                null
            )
            account.create()
        }
        // BasicComms is opened by BASIC_COMMS_USERS, not ALL_USERS; IronStone's Comms, when installed, by BOSS_USERS
        for (const group of ["BASIC_COMMS_USERS", "BOSS_USERS"]) {
            if (ec.entity.find("moqui.security.UserGroup").condition("userGroupId", group).count() == 0) continue
            const found = ec.entity.find("moqui.security.UserGroupMember").condition("userGroupId", group)
            if (found.condition("userId", userId).count() > 0) continue
            const member = ec.entity.makeValue("moqui.security.UserGroupMember")
            member.setFields({userGroupId: group, userId, fromDate: ec.user.nowTimestamp}, true, null, null)
            member.create()
        }
    })
}

spec({
    setupSpec() {
        makeUser(USER, "basic.comms.spec", false)
        makeUser(DISABLED, "basic.comms.spec.off", true)
    }
})

feature("a user with no party sees the mail settings, the wizard and no Internal account without an app for it", {
    when() {
        ec.user.internalLoginUser("basic.comms.spec")
        const index = render(root, "Settings/Index", "qvt", {})
        const list = render(root, "Settings/MailAccounts/List", "qvt", {})
        const wizard = render(root, "Settings/MailAccounts/Add/Address", "qvt", {lastStandalone:"-3", fresh: "Y"})
        // as the screens call them: authorized by the app's screen, here by the spec
        const accounts = sys(() => call("org.moqui.comms.MailSettingsServices.get#MailAccountList", {})) || {}
        accounts.errors = ec.message.hasError() ? ec.message.errorsString : ""
        ec.message.clearErrors()
        const providers = sys(() => call("org.moqui.ui.ShellServices.get#Providers", {hook: "inbox-source"})) || {}
        // SMTP alone is not an incoming mailbox. The existing Account render also checks its route back to this mount's Inbox.
        const hosted = sys(() => call("org.moqui.mail.EmailClientServices.create#EmailAccount", {
            accountKindEnumId: "EakHosted", emailAddress: "basic.comms.spec@example.invalid", accountName: "Spec hosted mail",
            statusId: "EasActive", smtpHost: "127.0.0.1", smtpPort: "1", smtpStartTls: "N", smtpSsl: "N"
        })) || {}
        const accountErrors = ec.message.hasError() ? ec.message.errorsString : ""
        ec.message.clearErrors()
        const hostedAccountId = hosted.emailAccountId
        const accountDetail = render(root, "Settings/MailAccounts/Account", "qvt", {lastStandalone:"-3", emailAccountId: hostedAccountId, setup: "done"})
        const shared = sys(() => call("org.moqui.mail.EmailClientServices.create#EmailAccount", {
            accountKindEnumId: "EakShared", emailAddress: "basic.comms.shared@example.invalid", statusId: "EasActive",
            smtpHost: "127.0.0.1", smtpPort: "1", smtpStartTls: "N", smtpSsl: "N"
        })) || {}
        const remote = sys(() => call("org.moqui.mail.EmailClientServices.create#EmailAccount", {
            accountKindEnumId: "EakExternal", emailAddress: "basic.comms.remote@example.invalid", statusId: "EasActive",
            storeHost: "127.0.0.1", storePort: "1", storeProtocol: "imap"
        })) || {}
        const classified = sys(() => call("org.moqui.comms.MailSettingsServices.get#MailAccountList", {})) || {}
        const capabilities = {
            errors: ec.message.hasError() ? ec.message.errorsString : "",
            hosted: classified.accountList?.find(account => account.emailAccountId === hostedAccountId),
            shared: classified.accountList?.find(account => account.emailAccountId === shared.emailAccountId),
            remote: classified.accountList?.find(account => account.emailAccountId === remote.emailAccountId)
        }
        ec.message.clearErrors()
        let refusedSync = {}, refusalThrown = ""
        try { refusedSync = sys(() => call("org.moqui.comms.MailSettingsServices.sync#UserAccounts", {emailAccountId: hostedAccountId})) || {} }
        catch (error) { refusalThrown = String(error.message || error) }
        capabilities.refusedSyncError = refusalThrown + (ec.message.hasError() ? ec.message.errorsString : "")
        capabilities.started = refusedSync.startedList || []
        ec.message.clearErrors()
        // Exercise the actual banner action for first-sync states without another Account render.
        const accountSource = ec.resource.getLocationText("component://basic-comms/screen/BasicComms/Settings/MailAccounts/Account.xml", false)
        const setupBannerAction = /        setupDoneSyncText = [\s\S]*?(?=\n        \/\/ how it signs in:)/.exec(accountSource)?.[0]
        assert.ok(setupBannerAction, "the Account setup banner action exists")
        const setupBannerProblems = []
        for (const scenario of [
            {account: capabilities.remote, expected: "The first sync is under way"},
            {account: Object.assign({}, capabilities.remote, {lastSyncText: "just now"}), expected: "Your mail is ready in the Inbox"},
            {account: Object.assign({}, capabilities.remote, {statusId: "EasError", problemText: "The last sync failed"}), expected: "The last sync failed"},
            {account: Object.assign({}, capabilities.remote, {statusId: "EasPaused", problemText: "Sync is paused"}), expected: "Sync is paused"},
            {account: {hasIncomingServer: false}, expected: "The account is ready"}
        ]) {
            const state = {account: scenario.account, L: text => text, setupDoneSyncText: null}
            runInContext(setupBannerAction, state)
            if (!String(state.setupDoneSyncText).startsWith(scenario.expected)) setupBannerProblems.push(scenario.expected)
            if (scenario.expected !== "The first sync is under way" && String(state.setupDoneSyncText).includes("under way"))
                setupBannerProblems.push("stale first-sync message")
        }
        const inboxLinkProblems = []
        const links = accountDetail.output.match(/<m-link\b[^>]*>[\s\S]*?<\/m-link>/g) || []
        for (const label of ["Open the Inbox", "Open in the Inbox"]) {
            const link = links.find(item => item.includes(label))
            const href = link?.match(/\bhref="([^"]+)"/)?.[1] || ""
            // With BasicComms as this test's root, the same relative destination starts at /Inbox.
            if (!/^\/Inbox(?:\/Mail)?\?/.test(href)) inboxLinkProblems.push(label + ": missing enabled Inbox link")
            const parameters = href.split("?")[1]?.replace(/&amp;/g, "&").split("&") || []
            if (!parameters.includes("account=" + encodeURIComponent(hostedAccountId))) inboxLinkProblems.push(label + ": account not retained")
        }
        // the account is this run's own: its owner deletes it, so a later run starts with no mail account again
        for (const emailAccountId of [hostedAccountId, shared.emailAccountId, remote.emailAccountId])
            if (emailAccountId) sys(() => call("org.moqui.mail.EmailClientServices.delete#EmailAccount", {emailAccountId, deleteLocalCopies: true}))
        const deleteErrors = ec.message.hasError() ? ec.message.errorsString : ""
        ec.message.clearErrors()
        const accountLeft = sys(() => ec.entity.find("moqui.basic.email.EmailAccount").condition("emailAccountId", hostedAccountId).count())
        login("john.doe")
        const internalShown = Array.from(accounts.accountList || []).some(account => account.isInternal)
        const internalSource = Array.from(providers.providerList || []).length > 0
        return {index, list, wizard, accounts, internalShown, internalSource, accountDetail, hostedAccountId, accountErrors, capabilities, setupBannerProblems, inboxLinkProblems, deleteErrors, accountLeft}
    },
    then({index, list, wizard, accounts, internalShown, internalSource, accountDetail, hostedAccountId, accountErrors, capabilities, setupBannerProblems, inboxLinkProblems, deleteErrors, accountLeft}) {
        index.status == 200
        index.output.includes("Identities and signatures")
        list.status == 200
        !list.broken
        wizard.status == 200
        !wizard.broken
        wizard.output.includes('draft-form="AccountOptions"')
        wizard.output.includes('draft-parameter="emailAccountId"')
        accounts.errors == ""
        accounts.mailAccountCount == 0
        accountErrors == ""
        !!hostedAccountId
        accountDetail.status == 200
        accountDetail.output.includes("Mail gateway")
        !accountDetail.output.includes("Bring in mail from")
        !accountDetail.output.includes("The sync maps the server's folders here")
        accountDetail.output.includes("no external mailbox is synced")
        accountDetail.output.includes("no external provider copy")
        !accountDetail.output.includes("your mail stays with your provider")
        accountDetail.errors == ""
        !accountDetail.broken
        accountDetail.output.includes("Spec hosted mail")
        accountDetail.output.includes("Delivered here")
        accountDetail.output.includes("Outgoing server (SMTP)")
        !accountDetail.output.includes("Waiting for the first sync")
        !accountDetail.output.includes("Sync now")
        capabilities.errors == ""
        capabilities.hosted.deliveredHere
        capabilities.hosted.hasServer
        !capabilities.hosted.hasIncomingServer
        capabilities.hosted.canSend
        capabilities.hosted.serverText.includes("SMTP 127.0.0.1")
        !capabilities.hosted.canSync
        !capabilities.hosted.canPause
        !capabilities.hosted.canResume
        capabilities.shared.deliveredHere
        capabilities.shared.statusLabel == "Delivered here"
        !capabilities.shared.canSync
        capabilities.remote.hasIncomingServer
        !capabilities.remote.deliveredHere
        capabilities.remote.statusLabel == "Waiting for the first sync"
        capabilities.remote.canSync
        capabilities.remote.canPause
        capabilities.refusedSyncError.includes("no incoming mailbox to sync")
        capabilities.started.length == 0
        setupBannerProblems.length == 0
        inboxLinkProblems.length == 0
        deleteErrors == ""
        accountLeft == 0
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
        // V5: call-context has providers now (the example gallery's, IronStone's); external-conversation has none yet
        const none = sys(() => call("org.moqui.ui.ShellServices.call#Providers", {hook: "external-conversation"})) || {}
        let refused = ""
        try {
            sys(() => call("org.moqui.ui.ShellServices.call#Providers", {hook: "not-a-hook"}))
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
                call("org.moqui.ui.ShellServices.call#Providers", {hook: "inbox-source", parameters: {operation: "accounts"}})
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
        const voice = (name, parameters) => sys(() => ec.service.sync().name("org.moqui.call.VoiceServices." + name).parameters(parameters).call()) || {}
        const trunk = voice("store#VoiceTrunk", {name: "Render trunk", host: "sip.render.test", emergencyCapable: "Y"}).voiceTrunkId
        const location = voice("store#EmergencyLocation", {name: "Render office", address1: "1 Main St", callbackNumber: "+15550109999"}).emergencyLocationId
        voice("store#PhoneNumber", {phoneNumber: "+15550109999", voiceTrunkId: trunk, routeId: USER, emergencyLocationId: location})
        voice("grant#CallerId", {phoneNumber: "+15550109999", userId: USER})
        voice("store#VoiceUserSetting", {userId: USER, outboundEnabled: "Y"})
        voice("record#AuthFailure", {sourceAddress: "198.51.100.7", username: "render"})
        sys(() => ec.service.sync().name("org.moqui.call.SipDeviceServices.create#SipDevice").parameters({name: "Render phone"}).call())
        // V3: a ring group with a member and a schedule with a holiday
        const handling = (name, parameters) => sys(() => ec.service.sync().name("org.moqui.call.CallHandlingServices." + name).parameters(parameters).call()) || {}
        const group = handling("store#RingGroup", {name: "Render group"})
        handling("store#RingGroupMember", {ringGroupId: group.ringGroupId, userId: USER})
        const hours = handling("store#CommsSchedule", {name: "Render hours", timeZone: "America/Chicago", shared: true})
        handling("store#CommsScheduleRule", {commsScheduleId: hours.commsScheduleId, ruleTypeEnumId: "CsrHoliday", onDate: "2026-12-25", repeatYearly: "Y"})
        ec.message.clearErrors()
        const problems = []
        // V5: an IVR menu with an option and a queue with an agent
        const ivr = (name, parameters) => sys(() => ec.service.sync().name("org.moqui.call.IvrServices." + name).parameters(parameters).call()) || {}
        const menu = ivr("store#IvrMenu", {name: "Render menu"})
        ivr("store#IvrOption", {ivrMenuId: menu.ivrMenuId, digits: "1", targetTypeEnumId: "CtgUser", targetId: USER})
        const queues = (name, parameters) => sys(() => ec.service.sync().name("org.moqui.call.CallQueueServices." + name).parameters(parameters).call()) || {}
        const queue = queues("store#CallQueue", {name: "Render queue"})
        queues("store#CallQueueAgent", {callQueueId: queue.callQueueId, userId: USER})
        ec.message.clearErrors()
        const texts = {Status: "SIP servers", Trunks: "Render trunk", Numbers: "+15550109999", CallerIds: "Let someone call from a number",
            Fraud: "198.51.100.7", Emergency: "Render office", Phones: "Render phone", RingGroups: "Render group", Schedules: "Render hours",
            IvrMenus: "Render menu", Queues: "Render queue"}
        for (const [path, text] of Object.entries(texts)) {
            for (const mode of ["qvt", "tui"]) {
                const shown = render(systemRoot, "Voice/" + path, mode, {})
                if (shown.status != 200 || shown.broken || !shown.output.includes(text))
                    problems.push(mode + " " + path + ": " + shown.status + " " + shown.errors + " " + shown.output.slice(0, 200))
            }
        }
        // Reuse these renders with inherited filter context: reset links must really remove it in both modes.
        const historyParameters = {outcome: "voicemail", folder: "trash", heard: "heard", pageIndex: "7"}
        const history = render(root, "Calls/History", "qvt", historyParameters)
        const historyTui = render(root, "Calls/History", "tui", historyParameters)
        const queryOf = url => Object.fromEntries((String(url).split("?")[1] || "").replace(/&amp;/g, "&").split("&").filter(Boolean).map(part => {
            const at = part.indexOf("=")
            return [decodeURIComponent(at < 0 ? part : part.slice(0, at)), decodeURIComponent(at < 0 ? "" : part.slice(at + 1))]
        }))
        const qvtHistoryLinks = Array.from(history.output.matchAll(/<m-link\b[^>]*href="([^"]+)"[^>]*>([\s\S]*?)<\/m-link>/g),
            match => ({text: match[2].replace(/<[^>]*>/g, "").trim(), query: queryOf(match[1])}))
        const tuiHistoryLinks = []
        const collectLinks = node => {
            if (!node || typeof node !== "object") return
            if (node.t === "link") tuiHistoryLinks.push({text: node.text, query: queryOf(node.url)})
            Object.values(node).forEach(collectLinks)
        }
        collectLinks(JSON.parse(historyTui.output))
        const filterLinkProblems = []
        for (const [mode, links] of [["qvt", qvtHistoryLinks], ["tui", tuiHistoryLinks]]) {
            const all = links.filter(link => link.text === "All")
            const overall = all.find(link => !Object.hasOwn(link.query, "outcome"))
            const heardAll = all.find(link => link.query.outcome === "voicemail")
            if (all.length !== 2 || !overall || !heardAll) filterLinkProblems.push(mode + ": both All controls need distinct reset destinations")
            if (overall && ["folder", "heard", "pageIndex"].some(key => Object.hasOwn(overall.query, key))) filterLinkProblems.push(mode + ": overall All retains voicemail filters")
            if (heardAll && (heardAll.query.folder !== "trash" || ["heard", "pageIndex"].some(key => Object.hasOwn(heardAll.query, key)))) filterLinkProblems.push(mode + ": heard All loses the folder or retains the filter/page")
            const inbox = links.find(link => link.text === "Inbox")
            if (!inbox || inbox.query.outcome !== "voicemail" || inbox.query.folder !== "inbox" || inbox.query.heard !== "heard" || Object.hasOwn(inbox.query, "pageIndex")) filterLinkProblems.push(mode + ": Inbox does not leave Trash while retaining the heard filter")
            const unheard = links.find(link => link.text === "Unheard")
            if (!unheard || unheard.query.outcome !== "voicemail" || unheard.query.folder !== "trash" || unheard.query.heard !== "unheard" || Object.hasOwn(unheard.query, "pageIndex")) filterLinkProblems.push(mode + ": Unheard does not reset the page within this folder")
            for (const outcome of ["missed", "placed", "answered"]) {
                const link = links.find(link => link.query.outcome === outcome)
                if (!link || ["folder", "heard", "pageIndex"].some(key => Object.hasOwn(link.query, key))) filterLinkProblems.push(mode + ": " + outcome + " inherits voicemail filters/page")
            }
        }
        // V5: the user's queues (an agent of Render queue), in the browser and the terminal
        ec.user.internalLoginUser("basic.comms.spec")
        const queuesQvt = render(root, "Calls/Queues", "qvt", {})
        const queuesTui = render(root, "Calls/Queues", "tui", {})
        login("john.doe")
        sys(() => {
            ec.entity.find("moqui.call.CallQueueAgent").condition("callQueueId", queue.callQueueId).deleteAll()
            ec.entity.find("moqui.call.CallQueue").condition("callQueueId", queue.callQueueId).deleteAll()
            ec.entity.find("moqui.call.IvrOption").condition("ivrMenuId", menu.ivrMenuId).deleteAll()
            ec.entity.find("moqui.call.IvrMenu").condition("ivrMenuId", menu.ivrMenuId).deleteAll()
        })
        // V3: the user's call handling and the dialer, in the terminal
        const handlingTui = render(root, "Settings/CallHandlingTui", "tui", {})
        const dialerTui = render(root, "Calls/DialerTui", "tui", {text: "Render"})
        return {problems, history, historyTui, filterLinkProblems, handlingTui, dialerTui, queuesQvt, queuesTui}
    },
    then({problems, history, historyTui, filterLinkProblems, handlingTui, dialerTui, queuesQvt, queuesTui}) {
        problems.join("\n") == ""
        filterLinkProblems.join("\n") == ""
        queuesQvt.status == 200
        queuesQvt.output.includes("Render queue")
        queuesTui.output.includes("Render queue")
        !queuesTui.broken
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

feature("Chat apps says which app passwords work with XMPP apps and offers a new one for XMPP, in qvt and tui", {
    when() {
        ec.user.internalLoginUser("basic.comms.spec")
        sys(() => ec.entity.find("moqui.security.UserAppPassword").condition("userId", USER).deleteAll())
        // an app password from before XMPP sign-in (no SCRAM verifiers), and one made now
        const older = sys(() => call("org.moqui.impl.UserServices.create#UserAppPassword", {name: "Old tablet"})).appPasswordId
        sys(() => {
            const row = ec.entity.find("moqui.security.UserAppPassword").condition("appPasswordId", older).one()
            row.setFields({scramSalt: null, scramIterations: null, scramStoredKeys: null, scramServerKeys: null}, true, null, null)
            row.update()
        })
        sys(() => call("org.moqui.impl.UserServices.create#UserAppPassword", {name: "Phone chat"}))
        const served = withProperty("xmpp_domain", "moqui.local", () => ({
            qvt: render(root, "Settings/ChatApps", "qvt", {}),
            tui: render(root, "Settings/ChatApps", "tui", {}),
            settings: sys(() => call("org.moqui.chat.XmppServices.get#ClientSettings", {}))
        }))
        // the terminal renders the sections that show (the qvt page carries both, for the browser to choose)
        const off = withProperty("xmpp_domain", "", () => render(root, "Settings/ChatApps", "tui", {}))
        sys(() => ec.entity.find("moqui.security.UserAppPassword").condition("userId", USER).deleteAll())
        login("john.doe")
        return {served, off}
    },
    then({served, off}) {
        served.qvt.status == 200
        !served.qvt.broken
        served.qvt.output.includes("New app password for XMPP")
        served.qvt.output.includes("basic.comms.spec@moqui.local")
        served.qvt.output.includes("Works with XMPP apps")
        served.qvt.output.includes("Made before XMPP sign-in")
        served.tui.status == 200
        !served.tui.broken
        served.tui.output.includes("Works with XMPP apps")
        served.tui.output.includes("Made before XMPP sign-in")
        served.settings.appPasswordList.length == 2
        served.settings.readyCount == 1
        served.settings.jid == "basic.comms.spec@moqui.local"
        off.status == 200
        off.output.includes("not set up on this server")
        !off.output.includes("New app password for XMPP")
    }
})


feature("out of office retains user-zone date strings and supplies instants to the static form", {
    expect() {
        const username = "basic.comms.ooo.spec"
        makeUser("BASIC_COMMS_OOO_SPEC", username, false)
        const previousUser = ec.user.username || "john.doe"
        ec.user.internalLoginUser(username)
        ec.user.setTimeZone("America/Chicago")
        const settings = (name, params) => sys(() => call("org.moqui.comms.MailSettingsServices." + name, params || {})) || {}
        let fixtureAccountId = null
        try {
            const account = sys(() => call("org.moqui.mail.EmailClientServices.create#EmailAccount", {
                accountKindEnumId:"EakHosted", emailAddress:"ooo.spec@example.invalid", accountName:"Out of office date fixture", statusId:"EasActive"
            })) || {}
            assert.ok(account.emailAccountId)
            const emailAccountId = account.emailAccountId
            fixtureAccountId = emailAccountId
            const save = (fromDate, thruDate) => settings("store#OutOfOffice", {emailAccountId, enabled:"N", fromDate, thruDate,
                subject:"Date fixture", bodyHtml:"<p>Disabled review reply</p>", replyOncePerDays:"4"})
            assert.equal(save("2026-11-08 09:00", "2026-11-09 09:00").storedCount, 1)
            assert.equal(ec.message.hasError(), false)
            const result = settings("get#OutOfOfficeSettings", {emailAccountId})
            assert.equal(result.reply.fromDate, "2026-11-08 09:00", "existing caller representation is unchanged")
            assert.equal(result.reply.thruDate, "2026-11-09 09:00")
            assert.equal(result.replyTimestamps.fromDate, Date.UTC(2026,10,8,15))
            assert.equal(result.replyTimestamps.thruDate, Date.UTC(2026,10,9,15))
            // Execute the screen's actual mapping without another application render. The host formatter must
            // receive temporal values, not numeric epochs (which it formats as numbers in Standard and TUI).
            const source = ec.resource.getLocationText("component://basic-comms/screen/BasicComms/Settings/OutOfOffice.xml")
            const actions = /<actions>\s*<service-call name="org\.moqui\.comms\.MailSettingsServices\.get#OutOfOfficeSettings"[^>]*\/>\s*<script><!\[CDATA\[([\s\S]*?)\]\]><\/script>/.exec(source)[1]
            const checkFormDates = output => {
                const state = {oooOut:output, oooAccounts:[], hasAccounts:false, selectedAccountId:null,
                    replyForm:{}, statusText:"", isActive:false, hasServer:false, selectedAccount:null, manyAccounts:false}
                runInContext(actions, state)
                assert.equal(ec.l10n.format(state.replyForm.fromDate, "yyyy-MM-dd HH:mm"), output.reply.fromDate)
                assert.equal(ec.l10n.format(state.replyForm.thruDate, "yyyy-MM-dd HH:mm"), output.reply.thruDate)
                for (const name of ["fromDate", "thruDate"]) {
                    if (output.replyTimestamps[name] == null) assert.equal(state.replyForm[name], null)
                    else {
                        assert.ok(state.replyForm[name] instanceof Date, "the screen passes a Date through the host Timestamp bridge")
                        assert.equal(Number(state.replyForm[name]), output.replyTimestamps[name])
                    }
                }
            }
            checkFormDates(result)
            // One screen template and its one actions package, the same boundary used by the static browser client.
            const screen = ec.screen.makeTest().rootScreen(root).renderMode("qvt")
            const page = screen.render("Settings/OutOfOffice", {lastStandalone:"-2",emailAccountId}, null)
            assert.equal(page.status, 200)
            assert.equal(page.errorMessages.length, 0)
            assert.equal(page.header("X-Moqui-Qvt"), "static-template")
            assert.ok(String(page.output).includes("m-date-time"))
            const data = screen.render("Settings/OutOfOffice/actions", {lastStandalone:"-2",emailAccountId}, null)
            assert.equal(data.status, 200)
            assert.equal(data.errorMessages.length, 0)
            const form = data.jsonObject.replyForm
            assert.equal(form.fromDate, String(result.replyTimestamps.fromDate), "static JSON serializes a Timestamp as epoch text")
            assert.equal(form.thruDate, String(result.replyTimestamps.thruDate))
            assert.equal(form.enabled, "N", "the fixture never enables an automatic reply")
            // Local09:00 straddles the fall DST change: it is14:00Z before and15:00Z afterward.
            assert.equal(save("2026-10-31 09:00", "2026-11-02 09:00").storedCount, 1)
            const dst = settings("get#OutOfOfficeSettings", {emailAccountId})
            assert.equal(dst.reply.fromDate, "2026-10-31 09:00")
            assert.equal(dst.reply.thruDate, "2026-11-02 09:00")
            assert.equal(dst.replyTimestamps.fromDate, Date.UTC(2026,9,31,14))
            assert.equal(dst.replyTimestamps.thruDate, Date.UTC(2026,10,2,15))
            checkFormDates(dst)
            assert.equal(save(null, null).storedCount, 1)
            const cleared = settings("get#OutOfOfficeSettings", {emailAccountId})
            assert.equal(cleared.reply.fromDate, "")
            assert.equal(cleared.reply.thruDate, "")
            assert.equal(cleared.replyTimestamps.fromDate, null)
            assert.equal(cleared.replyTimestamps.thruDate, null)
            checkFormDates(cleared)
            // TUI can edit dates and subject, but deliberately omits the read-only HTML fields. Exercise the actual
            // transition's preparation with real services; the one QVT template/actions render above remains enough.
            const prepare = /<transition name="storeReply">[\s\S]*?<script><!\[CDATA\[([\s\S]*?)\]\]><\/script>/.exec(source)[1]
            const parameters = {emailAccountId, enabled:"N", fromDate:"2026-11-08 09:00", thruDate:"2026-11-09 09:00",
                subject:"Read-only body fixture", replyOncePerDays:"4"}
            assert.equal(settings("store#OutOfOffice", Object.assign({}, parameters, {
                bodyHtml:"<p>Keep <strong>outside</strong> &amp; markup</p>", internalBodyHtml:"<p>Keep inside</p>"
            })).storedCount, 1)
            const original = settings("get#OutOfOfficeSettings", {emailAccountId})
            const prepareAndSave = supplied => {
                const state = {ec:{web:{requestParameters:supplied},service:ec.service,message:ec.message}, emailAccountId,
                    bodyHtml:supplied.bodyHtml ?? null, internalBodyHtml:supplied.internalBodyHtml ?? null}
                sys(() => runInContext(prepare, state))
                assert.equal(ec.message.hasError(), false)
                assert.equal(settings("store#OutOfOffice", Object.assign({}, parameters, supplied, {
                    bodyHtml:state.bodyHtml, internalBodyHtml:state.internalBodyHtml
                })).storedCount, 1)
                return settings("get#OutOfOfficeSettings", {emailAccountId})
            }
            const retained = prepareAndSave({subject:"Subject changed from terminal"})
            assert.equal(retained.reply.bodyHtml, original.reply.bodyHtml)
            assert.equal(retained.reply.internalBodyHtml, original.reply.internalBodyHtml)
            assert.equal(retained.reply.subject, "Subject changed from terminal")
            assert.equal(retained.replyTimestamps.fromDate, Date.UTC(2026,10,8,15))
            assert.equal(retained.replyTimestamps.thruDate, Date.UTC(2026,10,9,15))
            const oneCleared = prepareAndSave({bodyHtml:""})
            assert.equal(oneCleared.reply.bodyHtml, "", "explicit empty body still clears")
            assert.equal(oneCleared.reply.internalBodyHtml, original.reply.internalBodyHtml, "omitted internal HTML stays unchanged")
            const bothCleared = prepareAndSave({bodyHtml:"",internalBodyHtml:""})
            assert.equal(bothCleared.reply.bodyHtml, "")
            assert.equal(bothCleared.reply.internalBodyHtml, "")
        } finally {
            ec.message.clearErrors()
            // the fixture account goes with this run, deleted by its owner while still signed in
            if (fixtureAccountId) sys(() => call("org.moqui.mail.EmailClientServices.delete#EmailAccount", {emailAccountId: fixtureAccountId, deleteLocalCopies: true}))
            ec.message.clearErrors()
            ec.user.internalLoginUser(previousUser)
        }
    }
})
