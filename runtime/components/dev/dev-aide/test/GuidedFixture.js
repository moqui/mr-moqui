/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// The throw-away root the guided editors' specs save into (ServiceEditTui, EntityEditTui and ScreenDesignTui): a
// directory of the test run's scratch runtime (runtime/tmp is not a place the dev-aide opens) added as a Dev AIDE root,
// with "Allow editing upstream components" on for the user so the plain root may be written, and both taken back
// after. The real repository files are only ever staged, never written.

const BUFFER = "org.moqui.devaide.WorkspaceServices."

function call(name, parameters) {
    return (
        ec.service
            .sync()
            .name(name)
            .parameters(parameters || {})
            .call() || {}
    )
}
function asSystem(body) {
    const already = ec.artifactExecution.disableAuthz()
    try {
        return body()
    } finally {
        if (!already) ec.artifactExecution.enableAuthz()
    }
}
function location(name, path) {
    return "runtime://dev-aide-fixtures/" + name + (path ? "/" + path : "")
}
function hash(text) {
    return ec.resource.digestText(text, "sha256")
}

// The root `name` (its directory's name is its id) with `files` ({path: text}) in it.
function setup(name, files) {
    for (const [path, text] of Object.entries(files || {})) ec.resource.getLocationReference(location(name, path)).putText(text)
    const directory = ec.factory.getRuntimePath() + "/dev-aide-fixtures/" + name
    asSystem(() => {
        const row = ec.entity.makeValue("moqui.dev.DevAideRoot")
        row.setFields({rootId: name, location: directory, label: name}, true, null, null)
        row.createOrUpdate()
    })
    ec.user.setPreference("DEV_AIDE_UPSTREAM_EDITS", "true")
    return {rootId: name, directory}
}
function cleanup(name, paths) {
    for (const path of paths || []) {
        try {
            ec.resource.getLocationReference(location(name, path)).delete()
        } catch (err) {
            ec.logger.info("Fixture " + path + " not removed: " + err)
        }
    }
    asSystem(() => ec.entity.find("moqui.dev.DevAideRoot").condition("rootId", name).deleteAll())
    ec.user.setPreference("DEV_AIDE_UPSTREAM_EDITS", "")
}
function read(name, path) {
    const reference = ec.resource.getLocationReference(location(name, path))
    return reference.getExists() ? String(reference.getText()) : null
}
function write(name, path, text) {
    ec.resource.getLocationReference(location(name, path)).putText(text)
}
// What the buffer of a target holds: each file's staged text by its path.
function staged(target, files) {
    const out = call(BUFFER + "get#EditBuffer", {target, files: JSON.stringify(files || [])})
    return {pending: out.pending || {count: 0}, files: out.files || []}
}
function stage(target, rootId, path, text, baseHash) {
    return call(BUFFER + "put#EditBuffer", {target, files: JSON.stringify([{rootId, path, text, baseHash: baseHash || ""}])})
}
function discard(target) {
    return call(BUFFER + "clear#EditBuffer", {target})
}
// Before a Save: the buffer holds the fixture root's files only, so a spec that failed half way never saves a real
// file it staged.
function guard(target, rootId) {
    const files = staged(target).pending.files || []
    const outside = files.filter(file => file.rootId !== rootId)
    if (outside.length)
        throw new Error("Not saving: the buffer holds " + outside.map(file => file.path).join(", ") + ", outside the fixture")
}

;({setup, cleanup, read, write, hash, staged, stage, discard, guard, location})
