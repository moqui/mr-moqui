// The conf reload and the components (plans/archive/Component-Deploy.md, Phases 1 and 2): on a server just booted, the conf
// the files make is the running conf, so a preview finds no change (the merge done again gives the same conf, with the
// components' test confs as moqui test merges them); the System Conf and Components screens render in qvt and tui
// with no error; and the services need the ADMIN group (a user without it is refused even with the service's
// authorization off). Loading a component is checked in crates/moqui-host/tests/it/component_deploy.rs.
// Run: ./moq test tools --spec ConfReload
const systemRoot = "component://tools/screen/System.xml"
const outsiderId = "SPEC_CONF_OUTSIDER"
const outsiderName = "spec.conf.outsider"

function render(path, mode) {
    let test = ec.screen.makeTest().rootScreen(systemRoot).renderMode(mode)
    if (mode === "qvt") test = test.withStaticData()
    const str = test.render(path, {lastStandalone: "-2"}, null)
    const output = String(str.output || "")
    ec.message.clearErrors()
    return {status: str.status, errors: str.errorMessages.join(" | "), output}
}

spec({
    setupSpec() {
        login("john.doe")
        // Change 1: The refusal fixture belongs to this spec, not optional demo data.
        const wasDisabled = ec.artifactExecution.disableAuthz()
        try {
            const user = ec.entity.makeValue("moqui.security.UserAccount")
            user.setFields({userId: outsiderId, username: outsiderName, userFullName: "Conf reload outsider"}, true, null, null)
            user.create()
        } finally {
            if (!wasDisabled) ec.artifactExecution.enableAuthz()
        }
    },
    cleanupSpec() {
        ec.user.internalLoginUser("john.doe")
        const wasDisabled = ec.artifactExecution.disableAuthz()
        try {
            ec.entity.find("moqui.security.UserAccount").condition("userId", outsiderId).deleteAll()
        } finally {
            if (!wasDisabled) ec.artifactExecution.enableAuthz()
        }
    },
})

feature("a preview on a server just booted finds no change", {
    when() {
        const out = ec.service.sync().name("org.moqui.impl.ComponentServices.preview#ConfReload").disableAuthz().call() || {}
        const errors = ec.message.getErrorsString()
        ec.message.clearErrors()
        const report = out.report || {}
        return {errors, changes: Array.from(report.changes || []).map((change) => `${change.unit} ${change.attribute || ""}`)}
    },
    then({errors, changes}) {
        errors == ""
        changes.length == 0
    }
})

feature("the System Conf screen renders in qvt and tui", {
    when() {
        return {qvt: render("Conf", "qvt"), tui: render("Conf", "tui")}
    },
    then({qvt, tui}) {
        qvt.errors == ""
        tui.errors == ""
        qvt.output.includes("The conf files and the running conf are the same")
    }
})

feature("the System Components screen renders in qvt and tui, with the loaded components", {
    when() {
        return {qvt: render("Components", "qvt"), tui: render("Components", "tui")}
    },
    then({qvt, tui}) {
        qvt.errors == ""
        tui.errors == ""
        qvt.output.includes("component://tools") || qvt.output.includes("tools")
    }
})

feature("a staged and a deployed component show with their actions", {
    when() {
        const rows = [
            {componentName: "spec-staged", source: "git", storage: "database", status: "staged", commitId: "0123456789abcdef",
                location: "/tmp/spec/deployed-component/spec-staged",
                lastReport: JSON.stringify({location: "/tmp/spec/deployed-component/spec-staged",
                    load: {refusals: [], restartNeeded: false, conf: {changes: []}, dataTypes: {seed: 2, demo: 1}}})},
            {componentName: "spec-deployed", source: "git", storage: "clone", status: "deployed", branch: "main",
                location: "/tmp/spec/component/spec-deployed"}
        ]
        const sys = (run) => {
            const wasDisabled = ec.artifactExecution.disableAuthz()
            try { return run() } finally { if (!wasDisabled) ec.artifactExecution.enableAuthz() }
        }
        const value = (fields) => {
            const row = ec.entity.makeValue("moqui.server.DeployedComponent")
            row.setFields(fields, true, null, null)
            return row
        }
        sys(() => rows.forEach((row) => value(row).createOrUpdate()))
        const page = render("Components", "qvt")
        sys(() => rows.forEach((row) => value({componentName: row.componentName}).delete()))
        return page
    },
    then({errors, output}) {
        errors == ""
        output.includes("spec-staged")
        output.includes("seed (2)")
        output.includes("Deploy")
        output.includes("Check for Update")
    }
})

feature("the services need the ADMIN group", {
    when() {
        // Change 1: Restore the administrator even if switching identity or the call fails.
        try {
            ec.user.internalLoginUser(outsiderName)
            if (ec.user.userId !== outsiderId) throw new Error("Conf reload outsider was not selected")
            const isAdmin = ec.user.isInGroup("ADMIN")
            ec.service.sync().name("org.moqui.impl.ComponentServices.reload#Conf").disableAuthz().call()
            return {errors: ec.message.getErrorsString(), isAdmin}
        } finally {
            ec.message.clearErrors()
            ec.user.internalLoginUser("john.doe")
        }
    },
    then({errors, isAdmin}) {
        isAdmin === false
        errors.includes("needs the ADMIN group")
    }
})

feature("a reload with no change applies nothing", {
    when() {
        const out = ec.service.sync().name("org.moqui.impl.ComponentServices.reload#Conf").disableAuthz().call() || {}
        const errors = ec.message.getErrorsString()
        ec.message.clearErrors()
        return {errors, report: out.report || {}}
    },
    then({errors, report}) {
        errors == ""
        report.applied == 0
        report.pendingRestart == 0
    }
})
