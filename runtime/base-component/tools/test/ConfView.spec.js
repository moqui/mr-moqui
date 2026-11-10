// System › Conf's Settings and Edit tabs (docs/tools/Component-Deploy.md, "Viewing and editing the conf"): the merged
// conf names the file and line of each value, its class, and the default conf's comments, with secrets masked; a check
// of an edited conf file previews each change with its class; the file services need CONF_EDIT (an ADMIN without it is
// refused where john.doe, in ADMIN_ADV, is not), and a save is refused for the default conf and for a file changed since
// it was read. No file is written here: the write and the reload of a save are checked in
// crates/moqui-devkit/src/deploy/conf_edit_tests.rs and crates/moqui-core/src/context/conf_reload/view_tests.rs.
// The screens render once in each of their modes: View.qvue and View's tui tree, Edit.qvue and EditTui.
// Run: ./moq test tools --spec ConfView
const systemRoot = "component://tools/screen/System.xml"
const adminId = "SPEC_CONF_ADMIN"

function call(name, parameters) {
    const out = ec.service.sync().name("org.moqui.impl.ComponentServices." + name).parameters(parameters || {}).disableAuthz().call() || {}
    const errors = ec.message.getErrorsString()
    ec.message.clearErrors()
    return {out, errors}
}

function render(path, mode) {
    const str = ec.screen.makeTest().rootScreen(systemRoot).renderMode(mode).render(path, {lastStandalone: "-2"}, null)
    const output = String(str.output || "")
    ec.message.clearErrors()
    const failed = output.includes("Error rendering") || output.includes("Exception in render")
    return {status: str.status, errors: str.errorMessages.join(" | ") + (failed ? " | render error in output" : ""), output}
}

const child = (node, name, key) => (node?.children || []).find(found => found.name == name && (key == null || found.key == key))
const attr = (node, name) => (node?.attrs || []).find(found => found.name == name)

// the runtime conf when the run has one, else the tools component's conf: a file a save could write
function editableFile(view) {
    return view.files.find(file => file.role == "runtime") || view.files.find(file => file.component == "tools")
}

spec({
    setupSpec() {
        login("john.doe")
        ec.artifactExecution.disableAuthz()
        const make = (entityName, fields) => {
            const row = ec.entity.makeValue(entityName)
            row.setFields(fields, true, null, null)
            row.create()
        }
        // an ADMIN who is not in ADMIN_ADV, so without CONF_EDIT
        make("moqui.security.UserAccount", {userId: adminId, username: "spec.conf.admin", userFullName: "Spec Conf Admin"})
        make("moqui.security.UserGroupMember", {userGroupId: "ADMIN", userId: adminId, fromDate: ec.user.nowTimestamp})
        ec.artifactExecution.enableAuthz()
    },
    cleanupSpec() {
        ec.artifactExecution.disableAuthz()
        ec.entity.find("moqui.security.UserGroupMember").condition("userId", adminId).deleteAll()
        ec.entity.find("moqui.security.UserAccount").condition("userId", adminId).deleteAll()
        ec.artifactExecution.enableAuthz()
    }
})

feature("the merged conf names each value's file and class, with the default conf's comments and secrets masked", {
    when() {
        const {out, errors} = call("get#ConfView")
        const view = out.view || {files: [], root: {}}
        const purpose = child(view.root, "default-property", "instance_purpose")
        const webapp = child(child(view.root, "webapp-list"), "webapp", "webroot")
        const apps = child(child(view.root, "screen-facade"), "screen", "component://webroot/screen/webroot/apps.xml")
        const system = child(apps, "subscreens-item", "system")
        const hostKey = child(view.root, "default-property", "ssh_host_key")
        return {
            errors,
            canEdit: out.canEdit,
            firstRole: view.files[0]?.role,
            purposePath: purpose?.path,
            purposeFrom: view.files[purpose?.sources?.[0]?.file]?.role,
            purposeComments: (purpose?.comments || []).join(" "),
            portClass: attr(webapp, "http-port")?.class,
            webappClass: webapp?.class,
            systemFrom: view.files[system?.sources?.[0]?.file]?.component,
            hostKey: attr(hostKey, "value")?.value
        }
    },
    then(found) {
        found.errors == ""
        found.canEdit == true
        found.firstRole == "default"
        found.purposePath == "default-property[instance_purpose]"
        found.purposeFrom == "default"
        found.purposeComments.includes("recommended values: production, dev, test")
        found.portClass == "restart"
        found.webappClass == "live"
        // the System mount is the tools component's, appended whole
        found.systemFrom == "tools"
        found.hostKey == "••••••"
    }
})

feature("a check of an edited conf file previews each change with its class and the lines of the file", {
    when() {
        const view = call("get#ConfView").out.view
        const file = editableFile(view)
        const read = call("get#ConfFile", {fileIndex: file.index})
        const text = String(read.out.text || "")
        const added = '    <webapp-list><webapp name="webroot" http-port="18080"><session-config timeout="61"/></webapp></webapp-list>\n'
        const edited = text.replace("</moqui-conf>", added + "</moqui-conf>")
        const addedLine = edited.slice(0, edited.indexOf("<webapp-list>")).split("\n").length
        const {out, errors} = call("check#ConfFile", {fileIndex: file.index, text: edited})
        const changes = (out.report?.changes || []).map(change => `${change.unit}@${change.attribute} ${change.class} ${change.outcome}`)
        const line = (out.lines || []).find(info => info.line == addedLine && info.path == "webapp-list/webapp[webroot]")
        return {readErrors: read.errors, hadWebappList: text.includes("<webapp-list"), errors, savable: out.savable, changes,
            lineClass: line?.class, lineAttrs: JSON.stringify(line?.attrs || [])}
    },
    then(found) {
        found.readErrors == ""
        // the edit adds the element only to a file that has none
        found.hadWebappList == false
        found.errors == ""
        found.savable == true
        found.changes.includes("webapp-list/webapp[webroot]@http-port restart pending-restart")
        found.changes.includes("webapp-list/webapp[webroot]/session-config@timeout live would-apply")
        found.lineClass == "live"
        found.lineAttrs.includes("http-port")
    }
})

feature("the conf file services need CONF_EDIT, which john.doe has through ADMIN_ADV", {
    when() {
        const file = editableFile(call("get#ConfView").out.view)
        const allowed = call("get#ConfFile", {fileIndex: file.index})
        ec.user.internalLoginUser("spec.conf.admin")
        const view = call("get#ConfView")
        const read = call("get#ConfFile", {fileIndex: file.index})
        const save = call("save#ConfFile", {fileIndex: file.index, text: String(allowed.out.text || ""), expectedHash: allowed.out.textHash})
        logout()
        login("john.doe")
        return {allowed: allowed.errors, allowedText: String(allowed.out.text || "").includes("<moqui-conf"), viewErrors: view.errors,
            canEdit: view.out.canEdit, read: read.errors, save: save.errors}
    },
    then(found) {
        found.allowed == ""
        found.allowedText == true
        // viewing needs only the ADMIN group
        found.viewErrors == ""
        found.canEdit == false
        found.read.includes("needs the CONF_EDIT permission")
        found.save.includes("needs the CONF_EDIT permission")
    }
})

feature("a save is refused for the default conf and for a file changed since it was read", {
    when() {
        const view = call("get#ConfView").out.view
        const file = editableFile(view)
        const text = String(call("get#ConfFile", {fileIndex: file.index}).out.text || "")
        return {
            defaultConf: call("save#ConfFile", {fileIndex: 0, text, expectedHash: "x"}).errors,
            stale: call("save#ConfFile", {fileIndex: file.index, text, expectedHash: "not-the-hash"}).errors
        }
    },
    then(found) {
        found.defaultConf.includes("The default conf is not saved here")
        found.stale.includes("changed since it was read")
    }
})

feature("the Settings and Edit tabs render in their modes", {
    when() {
        return {
            view: render("Conf/View", "qvue"),
            viewTui: render("Conf/View", "tui"),
            edit: render("Conf/Edit", "qvue"),
            editTui: render("Conf/EditTui", "tui")
        }
    },
    then({view, viewTui, edit, editTui}) {
        view.errors == ""
        view.output.includes("Merged in this order")
        viewTui.errors == ""
        viewTui.output.includes("Merged from")
        viewTui.output.includes("default-property[instance_purpose]")
        edit.errors == ""
        edit.output.includes("Review and save")
        editTui.errors == ""
        editTui.output.includes("Save and reload")
    }
})
