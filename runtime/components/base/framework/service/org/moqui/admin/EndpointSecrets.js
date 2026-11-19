// The endpoint-change rule (ConfigurationConnections.js, mr-boss plan/design-admin.md §8.1) for a record that keeps a secret
// beside the address it is sent to, written by a plain entity store (plans/Security-Review-Low.md SEC-TLS-116): the
// Security.eecas.xml rules LlmEndpointSecretMoves (apiKey, sent to baseUrl and path) and EntitySyncSecretMoves
// (targetPassword, sent as Basic to targetServerUrl as targetUsername).
//
// moved(entityValue, originalValue, endpointFields, secretField), run before an update: when a field of endpointFields
// given in the write differs from the stored row, the change is refused to a caller outside ADMIN, and the stored secret is
// cleared unless a new one is given in the same write. The stored one again is not a new one: an update of an entity with
// an audit-logged field (EntitySync) carries the whole stored row, so a secret the caller did not give looks given;
// after a new address, the same secret is entered again in a save of its own. A write with authorization disabled (a
// data load, a setup service) answers for itself.
function moved(entityValue, originalValue, endpointFields, secretField) {
    if (originalValue == null || ec.artifactExecution.authzDisabled()) return
    const plain = value => (value == null ? "" : String(value))
    const changed = endpointFields.filter(
        name => entityValue.isFieldSet(name) && plain(entityValue.get(name)) !== plain(originalValue.get(name))
    )
    if (!changed.length) return
    if (!ec.user.isInGroup("ADMIN"))
        throw new Error("Changing " + changed.join(" or ") + " sends the stored secret to another address: an administrator (ADMIN) changes it")
    const stored = plain(originalValue.get(secretField))
    const given = entityValue.isFieldSet(secretField) ? plain(entityValue.get(secretField)) : ""
    if (stored !== "" && (given === "" || given === stored)) entityValue.set(secretField, null)
}

;({moved})
