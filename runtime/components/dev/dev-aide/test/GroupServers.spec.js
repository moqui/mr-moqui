// Group servers in Dev AIDE Settings (plans/design-dev-aide.md 21.2, P11): a connection's state, what the server offers
// and what Connect would set, never its token; Settings shows them and the plan of one server; a server's address is
// https (http only for this machine). Connect against a server (the app sign-in, merge requests, push to review, check
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
    user: {signedIn: true, username: "john.doe"}
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
        !JSON.stringify(servers).includes("spec-s3cret-key")
    }
})

feature("Settings shows the group servers and the plan of one", {
    expect() {
        const page = dev().render("Settings", {serverId: "SPEC_SERVER"}, null)
        page.status == 200 && page.errorMessages.length == 0
        !page.output.includes("Error rendering") && !page.output.includes("Exception in render")
        page.output.includes("Group servers")
        page.output.includes("Team (https://team.example.com)")
        page.output.includes("Set up from Team")
        page.output.includes("Add the repository team")
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
