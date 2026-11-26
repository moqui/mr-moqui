// The task apps added in Settings (plans/design-dev-aide.md 14, 16: DevAideTaskApp, src/tasks.rs): an extend-tier user
// adds one with its title and a project's and a task's path, each a path on this server with its placeholder, and it is
// among the apps a task opens in; another extend-tier user may neither change nor remove it, nor replace a conf
// task-app; who added it removes it, and the full tier removes anyone's. Settings renders them in GroupServers.spec.js.
// Run: ./moq test dev-aide --spec TaskApps
const TASKS = "org.moqui.devaide.TaskServices."
const call = (verb, parameters) => {
    ec.message.pushErrors()
    try {
        const out = ec.service.sync().name(TASKS + verb).parameters(parameters || {}).requireNewTransaction(true).call() || {}
        return {out, errors: ec.message.hasError() ? ec.message.errorsString : ""}
    } catch (error) {
        return {out: {}, errors: ec.message.errorsString || String(error.message)}
    } finally {
        ec.message.clearErrors()
        ec.message.popErrors()
    }
}
const asSystem = body => {
    ec.artifactExecution.disableAuthz()
    try {
        return body()
    } finally {
        ec.artifactExecution.enableAuthz()
    }
}
const as = (username, body) => {
    ec.user.internalLoginUser(username)
    try {
        return body()
    } finally {
        login("john.doe")
    }
}
const APP = {title: "Spec Tracker", projectPath: "/qapps/Spec/Project?projectId={projectId}", taskPath: "/qapps/Spec/Task?taskId={taskId}"}

spec({
    setupSpec() {
        asSystem(() => {
            const since = new Date(Date.now() - 60000)
            for (const [userId, username] of [["specTaskAppOwner", "spec.taskapp.owner"], ["specTaskAppOther", "spec.taskapp.other"]]) {
                ec.entity.makeValue("moqui.security.UserAccount").setAll({userId, username, userFullName: username}).createOrUpdate()
                ec.entity.makeValue("moqui.security.UserGroupMember").setAll({userGroupId: "DEV_AIDE_EXTENDERS", userId, fromDate: since}).createOrUpdate()
            }
        })
        login("john.doe")
    },
    cleanupSpec() {
        asSystem(() => {
            ec.entity.find("moqui.dev.DevAideTaskApp").condition("taskAppName", "spec-tracker").deleteAll()
            ec.entity.find("moqui.security.UserGroupMember").condition("userId", "in", ["specTaskAppOwner", "specTaskAppOther"]).deleteAll()
        })
    }
})

feature("a task app's paths are paths on this server, each with its placeholder", {
    when() {
        return as("spec.taskapp.owner", () => ({
            elsewhere: call("store#TaskApp", Object.assign({}, APP, {taskPath: "https://elsewhere.example.com/t/{taskId}"})).errors,
            protocolRelative: call("store#TaskApp", Object.assign({}, APP, {projectPath: "//elsewhere.example.com/{projectId}"})).errors,
            noPlaceholder: call("store#TaskApp", Object.assign({}, APP, {taskPath: "/qapps/Spec/Task"})).errors,
            // a browser drops a tab, a newline or a carriage return in a URL: /<tab>/evil.example is that host
            controls: ["\t", "\n", "\r"].map(ch =>
                call("store#TaskApp", Object.assign({}, APP, {taskPath: "/" + ch + "/evil.example/t/{taskId}"})).errors),
            confTitle: call("store#TaskApp", Object.assign({}, APP, {title: "IronStone Projects"})).errors,
            noPath: call("store#TaskApp", {title: "Spec Tracker"}).errors,
            added: call("store#TaskApp", APP)
        }))
    },
    then(result) {
        result.elsewhere.includes("is not a path on the server")
        result.protocolRelative.includes("is not a path on the server")
        result.noPlaceholder.includes("has no {taskId}")
        result.controls.length == 3 && result.controls.every(errors => errors.includes("is not a path on the server"))
        // another app under a conf app's title would pass for it
        result.confTitle.includes("is the title of a task app of the conf")
        result.noPath.includes("a project path or a task path")
        result.added.errors == ""
        // its name is its title's
        result.added.out.taskAppName == "spec-tracker"
    }
})

feature("a task app added is among the apps a task opens in, and only who added it or the full tier removes it", {
    when() {
        const offered = call("get#TaskSources").out.apps.find(app => app.name == "spec-tracker")
        const other = as("spec.taskapp.other", () => ({
            listed: call("get#TaskApps").out.apps.find(app => app.taskAppName == "spec-tracker"),
            change: call("store#TaskApp", Object.assign({}, APP, {title: "Mine now", taskAppName: "spec-tracker"})).errors,
            remove: call("delete#TaskApp", {taskAppName: "spec-tracker"}).errors,
            conf: call("store#TaskApp", Object.assign({}, APP, {taskAppName: "projects"})).errors
        }))
        const owner = as("spec.taskapp.owner", () => ({
            listed: call("get#TaskApps").out.apps.find(app => app.taskAppName == "spec-tracker"),
            remove: call("delete#TaskApp", {taskAppName: "spec-tracker"}).errors
        }))
        // the full tier removes anyone's
        as("spec.taskapp.owner", () => call("store#TaskApp", APP))
        const full = {listed: call("get#TaskApps").out.apps.find(app => app.taskAppName == "spec-tracker"),
            remove: call("delete#TaskApp", {taskAppName: "spec-tracker"}).errors}
        const left = call("get#TaskApps").out.apps.filter(app => app.taskAppName == "spec-tracker")
        return {offered, other, owner, full, left}
    },
    then(result) {
        result.offered.title == "Spec Tracker"
        result.offered.taskPath == "/qapps/Spec/Task?taskId={taskId}"
        result.other.listed.mayRemove == false
        result.other.change.includes("added by another")
        result.other.remove.includes("added by another")
        result.other.conf.includes("only the full tier replaces it")
        result.owner.listed.mayRemove == true
        result.owner.remove == ""
        result.full.listed.mayRemove == true
        result.full.remove == ""
        result.left.length == 0
    }
})
