// The endpoint-change rule for every record that keeps a secret beside the address it is sent to (plans/Security-Review-Low.md
// SEC-TLS-116; Security.eecas.xml *SecretMoves, admin/EndpointSecrets.js): a new address clears the stored secret unless a
// new one comes with it, and says so; a model endpoint's and an entity sync's address is an ADMIN member's change only; a
// change of another field keeps the secret; only a seed or install load keeps it beside a new address.
// Run: ./moq test framework --spec EndpointSecrets
const ENDPOINT = "moqui.llm.LlmEndpoint"
const SYNC = "moqui.entity.sync.EntitySync"
const REMOTE = "moqui.service.message.SystemMessageRemote"
const SCRAPE = "moqui.monitor.ScrapeTarget"
const SERVER = "moqui.basic.email.EmailServer"
const MCP = "moqui.agent.AgentMcpServer"
const endpointId = "SPEC_SECRET_ENDPOINT"
const entitySyncId = "SPEC_SECRET_SYNC"
const remoteId = "SPEC_SECRET_REMOTE"
const targetId = "SPEC_SECRET_SCRAPE"
const emailServerId = "SPEC_SECRET_MAIL"
const mcpServerId = "SPEC_SECRET_MCP"
const userId = "SPEC_SECRET_EDITOR"
const groupId = "SPEC_SECRET_EDITORS"
const artifactGroupId = "SPEC_SECRET_ENTITIES"
const editor = "spec.secret.editor"

const find = (entityName, conditions) => ec.entity.find(entityName).condition(conditions).disableAuthz().useCache(false)
const make = (entityName, fields) => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
const row = (entityName, conditions) => find(entityName, conditions).one()
// signed in as username with authorization on: the error's text ("" when it was written) and the messages
function as(username, body) {
    ec.user.internalLoginUser(username)
    ec.artifactExecution.enableAuthz()
    try {
        body()
        const error = ec.message.hasError() ? ec.message.getErrorsString() : ""
        return {error, messages: [...ec.message.messages].join(" | ")}
    } catch (e) {
        return {error: String(e.message || e), messages: ""}
    } finally {
        ec.message.clearAll()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
    }
}
// a screen's entity-auto service, as its transition calls it
const service = (username, name, parameters) => as(username, () => ec.service.sync().name(name).parameters(parameters).call())
// a generic entity write, as a Tools edit or a REST PATCH makes it
const update = (username, entityName, fields) =>
    as(username, () => {
        const value = ec.entity.makeValue(entityName)
        value.setFields(fields, true, null, null)
        value.update()
    })

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        make(ENDPOINT, {endpointId, protocol: "fake", baseUrl: "https://models.example.com", apiKey: "spec-key-one"})
        make(SYNC, {entitySyncId, targetServerUrl: "https://sync.example.com", targetUsername: "sync", targetPassword: "spec-pass-one"})
        make(REMOTE, {systemMessageRemoteId: remoteId, sendUrl: "https://remote.example.com/in", username: "remote", password: "spec-remote-pass"})
        make(SCRAPE, {targetId, url: "https://metrics.example.com/metrics", bearerToken: "spec-bearer", enabled: "N"})
        make(SERVER, {emailServerId, smtpHost: "smtp.example.com", mailUsername: "mail", mailPassword: "spec-mail-pass", disabled: "Y"})
        make(MCP, {mcpServerId, serverName: "Spec", url: "https://mcp.example.com/a", authJson: '{"type":"bearer","token":"spec-mcp"}', disabled: "Y"})
        // an editor of the entities and their entity-auto services outside ADMIN, and john.doe (ADMIN) through the same
        // group: ADMIN alone may only view them on a demo instance
        make("moqui.security.UserAccount", {userId, username: editor, userFullName: "Spec secret editor"})
        make("moqui.security.UserGroup", {userGroupId: groupId, description: "Spec secret editors"})
        for (const member of [userId, "EX_JOHN_DOE"]) make("moqui.security.UserGroupMember", {userGroupId: groupId, userId: member, fromDate: 0})
        make("moqui.security.ArtifactGroup", {artifactGroupId, description: "Spec secret entities"})
        for (const artifactName of [ENDPOINT, SYNC, REMOTE, SCRAPE, SERVER, MCP])
            make("moqui.security.ArtifactGroupMember", {artifactGroupId, artifactName, artifactTypeEnumId: "AT_ENTITY", nameIsPattern: "N"})
        for (const artifactName of ["store#" + ENDPOINT, "update#" + SYNC])
            make("moqui.security.ArtifactGroupMember", {artifactGroupId, artifactName, artifactTypeEnumId: "AT_SERVICE", nameIsPattern: "N"})
        make("moqui.security.ArtifactAuthz", {artifactAuthzId: artifactGroupId, userGroupId: groupId, artifactGroupId,
            authzTypeEnumId: "AUTHZT_ALWAYS", authzActionEnumId: "AUTHZA_ALL"})
    },
    cleanupSpec() {
        ec.artifactExecution.disableAuthz()
        find(ENDPOINT, {endpointId}).deleteAll()
        find(SYNC, {entitySyncId}).deleteAll()
        find(REMOTE, {systemMessageRemoteId: remoteId}).deleteAll()
        find(SCRAPE, {targetId}).deleteAll()
        find(SERVER, {emailServerId}).deleteAll()
        find(MCP, {mcpServerId}).deleteAll()
        find("moqui.security.ArtifactAuthz", {artifactAuthzId: artifactGroupId}).deleteAll()
        find("moqui.security.ArtifactGroupMember", {artifactGroupId}).deleteAll()
        find("moqui.security.ArtifactGroup", {artifactGroupId}).deleteAll()
        find("moqui.security.UserGroupMember", {userGroupId: groupId}).deleteAll()
        find("moqui.security.UserGroup", {userGroupId: groupId}).deleteAll()
        find("moqui.security.UserAccount", {userId}).deleteAll()
        ec.artifactExecution.enableAuthz()
    }
})

feature("through the screens' services a new address clears the key or password unless a new one comes with it, and is ADMIN's", {
    when() {
        const out = {}
        const endpoint = () => row(ENDPOINT, {endpointId})
        const sync = () => row(SYNC, {entitySyncId})
        // outside ADMIN: another field changes, the address does not
        out.editorDescription = service(editor, "store#" + ENDPOINT, {endpointId, description: "Spec endpoint"})
        out.editorUrl = service(editor, "store#" + ENDPOINT, {endpointId, baseUrl: "https://collector.example.net"})
        out.editorSync = service(editor, "update#" + SYNC, {entitySyncId, targetServerUrl: "https://collector.example.net"})
        out.afterEditor = {url: endpoint().baseUrl, key: endpoint().apiKey, syncUrl: sync().targetServerUrl}
        // ADMIN: a new address alone clears the key and says so; with a new key, the new one is kept
        out.moved = service("john.doe", "store#" + ENDPOINT, {endpointId, baseUrl: "https://models.example.org"})
        out.movedKey = endpoint().apiKey
        out.rekeyed = service("john.doe", "store#" + ENDPOINT, {endpointId, path: "/v2/chat", apiKey: "spec-key-two"})
        out.rekeyedKey = endpoint().apiKey
        // a new auth header moves the key too
        out.header = service("john.doe", "store#" + ENDPOINT, {endpointId, authHeader: "X-Collect"})
        out.headerKey = endpoint().apiKey
        // the same key again is no new one; entered in a save of its own it is kept
        service("john.doe", "store#" + ENDPOINT, {endpointId, apiKey: "spec-key-two"})
        out.again = service("john.doe", "store#" + ENDPOINT, {endpointId, baseUrl: "https://models.example.com", apiKey: "spec-key-two"})
        out.againKey = endpoint().apiKey
        out.keyed = service("john.doe", "store#" + ENDPOINT, {endpointId, apiKey: "spec-key-two"})
        out.keyedKey = endpoint().apiKey
        // an entity sync: a new address with a new password keeps the new one, a new username alone clears it
        out.syncMoved = service("john.doe", "update#" + SYNC, {entitySyncId, targetServerUrl: "https://sync.example.org", targetPassword: "spec-pass-two"})
        out.syncPassword = sync().targetPassword
        out.renamed = service("john.doe", "update#" + SYNC, {entitySyncId, targetUsername: "sync2"})
        out.renamedPassword = sync().targetPassword
        return out
    },
    then(out) {
        out.editorDescription.error == ""
        out.editorUrl.error.includes("ADMIN")
        out.editorSync.error.includes("ADMIN")
        out.afterEditor.url == "https://models.example.com"
        out.afterEditor.key == "spec-key-one"
        out.afterEditor.syncUrl == "https://sync.example.com"
        out.moved.error == ""
        out.moved.messages.includes("enter it again")
        out.movedKey == null
        out.rekeyed.error == ""
        out.rekeyedKey == "spec-key-two"
        out.header.error == ""
        out.headerKey == null
        out.again.error == ""
        out.againKey == null
        out.keyed.error == ""
        out.keyedKey == "spec-key-two"
        out.syncMoved.error == ""
        out.syncPassword == "spec-pass-two"
        out.renamed.error == ""
        out.renamedPassword == null
    }
})

feature("a message remote, a scrape target, a mail server and an MCP server lose their secret to a new address on a plain entity write", {
    when() {
        const out = {}
        // outside ADMIN too: the rule clears, it does not refuse
        out.remote = update(editor, REMOTE, {systemMessageRemoteId: remoteId, sendUrl: "https://collector.example.net/in"})
        out.remotePassword = row(REMOTE, {systemMessageRemoteId: remoteId}).password
        out.scrape = update(editor, SCRAPE, {targetId, url: "https://collector.example.net/metrics"})
        out.scrapeToken = row(SCRAPE, {targetId}).bearerToken
        // to the edge node and back the mail server keeps its password; to another host it does not
        update("john.doe", SERVER, {emailServerId, smtpHost: "edge:"})
        update("john.doe", SERVER, {emailServerId, smtpHost: "smtp.example.com"})
        out.edgeKept = row(SERVER, {emailServerId}).mailPassword
        out.mail = update("john.doe", SERVER, {emailServerId, smtpHost: "smtp.collector.example.net"})
        out.mailPassword = row(SERVER, {emailServerId}).mailPassword
        // an MCP server's URL counts by its origin, as its store service decides
        update("john.doe", MCP, {mcpServerId, url: "https://mcp.example.com/b"})
        out.samePlace = row(MCP, {mcpServerId}).authJson
        out.mcp = update("john.doe", MCP, {mcpServerId, url: "https://collector.example.net/mcp"})
        out.mcpAuth = row(MCP, {mcpServerId}).authJson
        return out
    },
    then(out) {
        out.remote.error == ""
        out.remotePassword == null
        out.scrape.error == ""
        out.scrapeToken == null
        out.edgeKept == "spec-mail-pass"
        out.mail.error == ""
        out.mailPassword == null
        out.samePlace == '{"type":"bearer","token":"spec-mcp"}'
        out.mcp.error == ""
        out.mcpAuth == null
    }
})

feature("only a seed or install load keeps a secret beside a new address; a load that restricts sensitive entities does not", {
    when() {
        ec.artifactExecution.disableAuthz()
        const reset = () => {
            const value = ec.entity.makeValue(ENDPOINT)
            value.setFields({endpointId, baseUrl: "https://models.example.com", apiKey: "spec-key-load"}, true, null, null)
            ec.user.internalLoginUser("john.doe")
            value.update()
        }
        const xml = url => `<entity-facade-xml type="seed"><moqui.llm.LlmEndpoint endpointId="${endpointId}" baseUrl="${url}"/></entity-facade-xml>`
        const out = {}
        // a request's load, as Tools Data Load makes it (authorization off, sensitive entities restricted)
        reset()
        ec.entity.makeDataLoader().restrictSensitiveEntities(true).xmlText(xml("https://collector.example.net/a")).load()
        out.requestKey = row(ENDPOINT, {endpointId}).apiKey
        // a seed load of a data file
        reset()
        const location = "file:" + ec.factory.getRuntimePath() + "/tmp/SpecSecretSeed.xml"
        const file = ec.resource.getLocationReference(location)
        file.putText(xml("https://models.example.org"))
        ec.entity.makeDataLoader().location(location).load()
        file.delete()
        out.seedKey = row(ENDPOINT, {endpointId}).apiKey
        out.seedUrl = row(ENDPOINT, {endpointId}).baseUrl
        ec.message.clearAll()
        return out
    },
    then(out) {
        out.requestKey == null
        out.seedKey == "spec-key-load"
        out.seedUrl == "https://models.example.org"
    }
})
