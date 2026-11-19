// The endpoint-change rule for a model endpoint's API key and an entity sync's password (plans/Security-Review-Low.md
// SEC-TLS-116; Security.eecas.xml LlmEndpointSecretMoves, EntitySyncSecretMoves, admin/EndpointSecrets.js): a new baseUrl
// or path, or a new targetServerUrl or targetUsername, clears the stored secret unless a new one is written with it, and
// is an ADMIN member's change only; a change of another field keeps the secret.
// Run: ./moq test framework --spec EndpointSecrets
const ENDPOINT = "moqui.llm.LlmEndpoint"
const SYNC = "moqui.entity.sync.EntitySync"
const endpointId = "SPEC_SECRET_ENDPOINT"
const entitySyncId = "SPEC_SECRET_SYNC"
const userId = "SPEC_SECRET_EDITOR"
const groupId = "SPEC_SECRET_EDITORS"
const artifactGroupId = "SPEC_SECRET_ENTITIES"

const find = (entityName, conditions) => ec.entity.find(entityName).condition(conditions).disableAuthz().useCache(false)
const make = (entityName, fields) => {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
const endpoint = () => find(ENDPOINT, {endpointId}).one()
const sync = () => find(SYNC, {entitySyncId}).one()
// an update with authorization on, signed in as username: the error's text, or "" when it was written
function update(username, entityName, fields) {
    ec.user.internalLoginUser(username)
    ec.artifactExecution.enableAuthz()
    try {
        const row = ec.entity.makeValue(entityName)
        row.setFields(fields, true, null, null)
        row.update()
        return ""
    } catch (e) {
        return String(e.message || e)
    } finally {
        ec.message.clearErrors()
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
    }
}

spec({
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
        ec.artifactExecution.disableAuthz()
        make(ENDPOINT, {endpointId, protocol: "fake", baseUrl: "https://models.example.com", apiKey: "spec-key-one"})
        make(SYNC, {entitySyncId, targetServerUrl: "https://sync.example.com", targetUsername: "sync", targetPassword: "spec-pass-one"})
        // an editor of both entities outside ADMIN
        make("moqui.security.UserAccount", {userId, username: "spec.secret.editor", userFullName: "Spec secret editor"})
        make("moqui.security.UserGroup", {userGroupId: groupId, description: "Spec secret editors"})
        make("moqui.security.UserGroupMember", {userGroupId: groupId, userId, fromDate: 0})
        // john.doe (ADMIN) writes the entities through the same group: ADMIN alone may only view them on a demo instance
        make("moqui.security.UserGroupMember", {userGroupId: groupId, userId: "EX_JOHN_DOE", fromDate: 0})
        make("moqui.security.ArtifactGroup", {artifactGroupId, description: "Spec secret entities"})
        for (const artifactName of [ENDPOINT, SYNC])
            make("moqui.security.ArtifactGroupMember", {artifactGroupId, artifactName, artifactTypeEnumId: "AT_ENTITY", nameIsPattern: "N"})
        make("moqui.security.ArtifactAuthz", {artifactAuthzId: artifactGroupId, userGroupId: groupId, artifactGroupId,
            authzTypeEnumId: "AUTHZT_ALWAYS", authzActionEnumId: "AUTHZA_ALL"})
    },
    cleanupSpec() {
        ec.artifactExecution.disableAuthz()
        find(ENDPOINT, {endpointId}).deleteAll()
        find(SYNC, {entitySyncId}).deleteAll()
        find("moqui.security.ArtifactAuthz", {artifactAuthzId: artifactGroupId}).deleteAll()
        find("moqui.security.ArtifactGroupMember", {artifactGroupId}).deleteAll()
        find("moqui.security.ArtifactGroup", {artifactGroupId}).deleteAll()
        find("moqui.security.UserGroupMember", {userGroupId: groupId}).deleteAll()
        find("moqui.security.UserGroup", {userGroupId: groupId}).deleteAll()
        find("moqui.security.UserAccount", {userId}).deleteAll()
        ec.artifactExecution.enableAuthz()
    }
})

feature("a new address clears the stored secret unless a new one comes with it, and is ADMIN's change only", {
    when() {
        const editor = "spec.secret.editor"
        const out = {}
        // outside ADMIN: another field changes, the address does not
        out.editorDescription = update(editor, ENDPOINT, {endpointId, description: "Spec endpoint"})
        out.editorUrl = update(editor, ENDPOINT, {endpointId, baseUrl: "https://collector.example.net"})
        out.editorSync = update(editor, SYNC, {entitySyncId, targetServerUrl: "https://collector.example.net"})
        out.afterEditor = {url: endpoint().baseUrl, key: endpoint().apiKey, syncUrl: sync().targetServerUrl}
        // ADMIN: a new address alone clears the secret, a new address with a new secret keeps the new one
        out.moved = update("john.doe", ENDPOINT, {endpointId, baseUrl: "https://models.example.org"})
        out.movedKey = endpoint().apiKey
        out.rekeyed = update("john.doe", ENDPOINT, {endpointId, path: "/v2/chat", apiKey: "spec-key-two"})
        out.rekeyedKey = endpoint().apiKey
        // the stored key again is no new one, and is kept when entered in a save of its own
        out.again = update("john.doe", ENDPOINT, {endpointId, baseUrl: "https://models.example.com", apiKey: "spec-key-two"})
        out.againKey = endpoint().apiKey
        out.keyed = update("john.doe", ENDPOINT, {endpointId, apiKey: "spec-key-two"})
        out.keyedKey = endpoint().apiKey
        out.renamed = update("john.doe", SYNC, {entitySyncId, targetUsername: "sync2"})
        out.renamedPassword = sync().targetPassword
        return out
    },
    then(out) {
        out.editorDescription == ""
        out.editorUrl.includes("ADMIN")
        out.editorSync.includes("ADMIN")
        out.afterEditor.url == "https://models.example.com"
        out.afterEditor.key == "spec-key-one"
        out.afterEditor.syncUrl == "https://sync.example.com"
        out.moved == ""
        out.movedKey == null
        out.rekeyed == ""
        out.rekeyedKey == "spec-key-two"
        out.again == ""
        out.againKey == null
        out.keyed == ""
        out.keyedKey == "spec-key-two"
        out.renamed == ""
        out.renamedPassword == null
    }
})
