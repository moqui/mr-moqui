// Group servers in Dev AIDE Settings (plans/design-dev-aide.md 21.2, P11): a connection's state, what the server offers
// and what Connect would set, never its token; Settings shows them, the plan of one server and the identity providers
// it signs in with (Phase 1: the provider choice); the app the server offers for its tasks is in the plan and among the
// apps a task opens in; a server's address is https (http only for this machine). Connect against a server (the app sign-in, merge requests, push to review, check
// out; P12's threads in the diff and the gutter, a reply and resolve, statuses, the agreement and Sign) is the host's
// integration test (crates/moqui-host/tests/it/devaide_group.rs), with the instance as its own group server; the pure
// parts (myaddons.xml, the REST contracts of a plain and a Smithy server, threads of both shapes, failed tests as links,
// a signature under both lists) are the crate's tests.
// Run: ./moq test dev-aide --spec GroupServers
const hosting = (verb, params) => ec.service.sync().name("org.moqui.devaide.HostingServices." + verb).parameters(params || {}).call()
const dev = () => ec.screen.makeTest().rootScreen("component://dev-aide/screen/DevAide.xml")
const offer = {
    baseUrl: "https://team.example.com",
    git: {httpsUrlTemplate: "https://team.example.com/git/${name}.git", sshHost: "team.example.com", sshPort: "2222"},
    gateway: {enabled: true, url: "https://team.example.com/llm"},
    user: {signedIn: true, username: "john.doe"},
    // what refresh keeps of the server's get#AppProviders
    signIn: {appLogin: "/sso/appLogin", appProviders: "/sso/appProviders", providers: [{authFlowId: "TEAM_IDP", description: "Team IdP"}]},
    tasks: {kind: "moqui-rest", path: "/rest/s1/mantle/my/tasks", issuesPath: "/rest/s1/mantle/gitRepositories/{repositoryId}/issues",
        app: {name: "projects", title: "Team Projects", taskPath: "/qapps/Projects/WorkItems/WorkItemDetail?workEffortId={taskId}",
            projectPath: "https://elsewhere.example.com/{projectId}"}}
}

spec({
    setupSpec() {
        login("john.doe")
        ec.artifactExecution.disableAuthz()
        ec.entity.makeValue("moqui.dev.DevAideServer").setAll({serverId: "SPEC_SERVER", userId: ec.user.userId, label: "Team",
            baseUrl: "https://team.example.com", username: "john.doe", statusText: "connected", offerJson: JSON.stringify(offer)}).create()
        ec.entity.makeValue("moqui.dev.DevAideCredential").setAll({credentialId: "SPEC_SERVER_KEY", serverId: "SPEC_SERVER",
            userId: ec.user.userId, token: "spec-s3cret-key", urlPrefix: "https://team.example.com"}).create()
        ec.artifactExecution.enableAuthz()
    },
    cleanupSpec() {
        ec.artifactExecution.disableAuthz()
        ec.entity.find("moqui.dev.DevAideCredential").condition("credentialId", "SPEC_SERVER_KEY").deleteAll()
        ec.entity.find("moqui.dev.DevAideServer").condition("serverId", "SPEC_SERVER").deleteAll()
        ec.artifactExecution.enableAuthz()
    }
})

feature("a connection shows its state, offer and plan, never its token", {
    when() {
        servers = hosting("get#Servers").servers
        server = servers.find(item => item.serverId == "SPEC_SERVER")
    },
    then() {
        !ec.message.hasError()
        server.statusText == "connected"
        server.hasToken == true
        server.mine == true
        server.plan.addons.repositoryName == "team"
        server.plan.addons.url == "https://team.example.com/git/${name}.git"
        server.plan.gateway.available == true
        server.plan.gitCredential.prefix == "https://team.example.com"
        server.plan.gitCredential.mode == "helper"
        server.plan.taskSource.app.name == "server-SPEC_SERVER"
        server.plan.taskSource.app.title == "Team Projects (Team)"
        !JSON.stringify(servers).includes("spec-s3cret-key")
    }
})

feature("the app a server offers for its tasks opens them at the server, by its paths on the server only", {
    when() {
        return ec.service.sync().name("org.moqui.devaide.TaskServices.get#TaskSources").call().apps
            .find(app => app.name == "server-SPEC_SERVER")
    },
    then(app) {
        app.baseUrl == "https://team.example.com"
        app.taskPath == "/qapps/Projects/WorkItems/WorkItemDetail?workEffortId={taskId}"
        // a path at another origin is no path of the app
        app.projectPath == ""
    }
})

feature("Settings shows the group servers and the plan of one", {
    expect() {
        const page = dev().render("Settings", {serverId: "SPEC_SERVER", chooseSignIn: "true"}, null)
        page.status == 200 && page.errorMessages.length == 0
        !page.output.includes("Error rendering") && !page.output.includes("Exception in render")
        page.output.includes("Group servers")
        page.output.includes("Team (https://team.example.com)")
        page.output.includes("Set up from Team")
        page.output.includes("Add the repository team")
        page.output.includes("opening in Team Projects (Team)")
        // the providers the server signs in with, and its own login, as start#Connect asks (chooseSignIn); a connected
        // server shows them only so
        page.output.includes("Sign in to Team")
        page.output.includes("Sign in with Team IdP")
        page.output.includes("Sign in with Team&#39;s own login") || page.output.includes("Sign in with Team's own login")
        !page.output.includes("spec-s3cret-key")
    }
})

feature("a server's address is https, or http only for this machine", {
    when() {
        ec.message.pushErrors()
        ec.service.sync().name("org.moqui.devaide.HostingServices.start#Connect").parameters({baseUrl: "http://team.example.com"})
            .requireNewTransaction(true).call()
        refused = ec.message.hasError() ? ec.message.errorsString : ""
        ec.message.clearErrors()
        ec.message.popErrors()
    },
    then() {
        refused.includes("is not an https address")
    }
})

feature("a server is this machine by its IP literal, never localhost", {
    when() {
        ec.message.pushErrors()
        ec.service.sync().name("org.moqui.devaide.HostingServices.start#Connect").parameters({baseUrl: "http://localhost:8080"})
            .requireNewTransaction(true).call()
        refused = ec.message.hasError() ? ec.message.errorsString : ""
        ec.message.clearErrors()
        ec.message.popErrors()
    },
    then() {
        refused.includes("is not an https address")
    }
})

feature("the gateway's lines take a key for the gateway alone, never the connection's token", {
    when() {
        server = hosting("get#Servers").servers.find(item => item.serverId == "SPEC_SERVER")
    },
    then() {
        server.plan.gateway.lines[1].includes("a key for the gateway alone")
        !JSON.stringify(server.plan).includes("spec-s3cret-key")
    }
})
