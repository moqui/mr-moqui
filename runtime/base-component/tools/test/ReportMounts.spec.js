// Tools' report screens mounted under another app's root (docs/tools/Monitoring.md, "Mounting the report screens"; BOSS Insights
// P2, INS-04): the test mount test/screen/ReportMount.xml takes in Tools' Dashboard and Definition with their transitions and sets
// reportLinks. A mounted editor offers the datasets the author may read by each dataset's own artifact, without DASHBOARDS_APP or
// REPORT_DATASETS_ADMIN, starts a new panel on one of them and saves and returns under the mount; a mounted dashboard sends the
// mount's links to the browser and terminal (appLinks), leaves out the screens the mount does not offer and keeps its fixed
// variables over a viewer's (in a panel inspect too); the mount's editor saves only its types and, where the mount shares in its own
// screen, keeps a definition's group over a posted one; Tools' own Dashboards screens keep Tools' links, whatever a request sends.
// The mount's policy (reportPolicy) beside its links: template/ReportMount.js answers both, every link key and every policy
// field with its default; a screen under the mount reads the policy the mount set (the dashboard's range), whatever a request
// sends, and Tools' own screens read Tools', whatever a request sends; with no mount the links and the policy are Tools' own,
// and a value that is not a map is a mount with nothing in it. An area of the mounting app adds to its root's policy
// through the module (policyWith: the recipient cap of the area that delivers), which with no mount makes none. And the rule
// that keeps it so, read from the screens' sources at no render's cost: Tools' two roots null every name of the mount, and
// no screen reads the two maps but through the module.
// Run: ./moq test tools --spec ReportMounts
const mount = "component://tools/test/screen/ReportMount.xml"
const webroot = "component://webroot/screen/webroot.xml"
const mountLib = "component://tools/template/ReportMount.js"
const screens = "component://tools/screen"
// the names a mounting root sets, which Tools' roots null
const MOUNT_NAMES = ["reportLinks", "reportPolicy", "reportFixedVariables", "reportHiddenComponents", "reportDefinitionTypes"]
const USER = "REPORT_MOUNTS_SPEC"
const USERNAME = "report.mounts.spec"
const GROUP = "REPORT_MOUNTS_SPEC"
const panelId = "REPORT_MOUNTS_PANEL"
const boardId = "REPORT_MOUNTS_BOARD"

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
function make(entityName, fields) {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
// A render or post as the spec's author with authorization on: its errors, output and where a transition sends the browser.
function screen(root, path, parameters, method, mode) {
    ec.artifactExecution.enableAuthz()
    const test = ec.screen.makeTest().rootScreen(root)
    if (mode) test.renderMode(mode).withStaticData()
    const result = test.render(path, Object.assign(mode ? {lastStandalone: "true"} : {}, parameters), method || null)
    const output = String(result.output || "")
    const failed = output.match(/(Error rendering|Exception in render)[^\n<]*/g) || []
    ec.message.clearErrors()
    return {
        status: result.status,
        output,
        location: String(result.header("Location") || ""),
        errors: Array.from(result.errorMessages || [])
            .concat(failed)
            .join(" | ")
    }
}
const decoded = text => text.replace(/&quot;/g, '"').replace(/&amp;/g, "&")
// The screen files under a directory, its subscreens' directories too.
function screenFiles(location, found = []) {
    for (const entry of ec.resource.getLocationReference(location).getDirectoryEntries()) {
        if (entry.isDirectory()) screenFiles(entry.getLocation(), found)
        else if (/\.xml$/.test(entry.getLocation())) found.push(entry.getLocation())
    }
    return found
}
function removeDefinitions() {
    sys(() => {
        for (const entityName of [
            "moqui.report.DefinitionReference",
            "moqui.report.DefinitionVersion",
            "moqui.report.Definition"
        ])
            ec.entity.find(entityName).condition("definitionId", "in", [panelId, boardId]).deleteAll()
    })
}

spec({
    setupSpec() {
        removeDefinitions()
        sys(() => {
            if (ec.entity.find("moqui.security.UserAccount").condition("userId", USER).count() === 0) {
                const from = new Date(Date.now() - 60000)
                make("moqui.security.UserAccount", {
                    userId: USER,
                    username: USERNAME,
                    userFullName: "Report Mounts Spec",
                    disabled: "N"
                })
                make("moqui.security.UserGroup", {userGroupId: GROUP, description: "A mounted report author (ReportMounts spec)"})
                make("moqui.security.UserGroupMember", {userGroupId: GROUP, userId: USER, fromDate: from})
                // the mount's root, as an app's root grant (no DASHBOARDS_APP, no REPORT_DATASETS_ADMIN), and one dataset's own artifact
                make("moqui.security.ArtifactGroup", {artifactGroupId: GROUP, description: "ReportMounts spec"})
                make("moqui.security.ArtifactGroupMember", {
                    artifactGroupId: GROUP,
                    artifactName: mount,
                    artifactTypeEnumId: "AT_XML_SCREEN",
                    inheritAuthz: "Y"
                })
                make("moqui.security.ArtifactGroupMember", {
                    artifactGroupId: GROUP,
                    artifactName: "org.moqui.report.dataset.otel_metrics",
                    artifactTypeEnumId: "AT_SERVICE"
                })
                make("moqui.security.ArtifactAuthz", {
                    artifactAuthzId: GROUP,
                    artifactGroupId: GROUP,
                    userGroupId: GROUP,
                    authzTypeEnumId: "AUTHZT_ALLOW",
                    authzActionEnumId: "AUTHZA_ALL"
                })
            }
        })
        ec.user.internalLoginUser(USERNAME)
        // the author's dashboard, with the variable the mount fixes, shared with ADMIN for Tools' render
        sys(() =>
            call("org.moqui.report.ReportServices.save#Definition", {
                definition: {
                    id: boardId,
                    type: "dashboard",
                    title: "Mount board",
                    userGroupId: "ADMIN",
                    body: {
                        variables: [{name: "region", kind: "textbox"}],
                        panels: {note: {title: "Note", visualization: "text", text: "Mounted"}}
                    }
                }
            })
        )
    },
    cleanupSpec() {
        ec.message.clearErrors()
        removeDefinitions()
        ec.artifactExecution.enableAuthz()
    }
})

feature("a mounted editor offers the author's datasets without DASHBOARDS_APP, and saves and returns under its mount", {
    when() {
        const editor = screen(mount, "Editor", {definitionType: "panel"}, null, "qvt")
        const body = JSON.stringify({
            title: "Mount panel",
            visualization: "table",
            queries: [{datasetId: "otel_metrics", mode: "documents"}]
        })
        const saved = screen(
            mount,
            "Editor/save",
            {
                definitionId: panelId,
                definitionType: "panel",
                definitionTitle: "Mount panel",
                definitionVersion: "0",
                definitionJson: body,
                note: "spec"
            },
            "post"
        )
        // the mount shares in its own screen, so a posted group is not taken; and the editor saves only the mount's types
        const regrouped = screen(
            mount,
            "Editor/save",
            {
                definitionId: panelId,
                definitionType: "panel",
                definitionTitle: "Mount panel",
                definitionVersion: "1",
                definitionJson: body,
                definitionUserGroupId: "ADMIN"
            },
            "post"
        )
        const search = screen(
            mount,
            "Editor/save",
            {definitionId: panelId + "_S", definitionType: "search", definitionTitle: "S", definitionJson: "{}"},
            "post"
        )
        const stored = sys(() => ec.entity.find("moqui.report.Definition").condition("definitionId", panelId).one())
        const text = decoded(editor.output)
        return {
            editor,
            text,
            newPanelDataset: /"datasetId":\s*"(\w+)"/.exec(text)?.[1],
            saved,
            owner: stored?.ownerId,
            regrouped,
            group: stored?.userGroupId,
            version: stored?.versionNumber,
            search
        }
    },
    then({editor, text, newPanelDataset, saved, owner, regrouped, group, version, search}) {
        editor.status == 200
        editor.errors == ""
        // the datasets the author may read, by the dataset's own artifact: otel_metrics, not otel_logs
        text.includes('"id":"otel_metrics"')
        !text.includes('"id":"otel_logs"')
        // the new panel starts on it, the types are the mount's and import is not offered
        newPanelDataset == "otel_metrics"
        !text.includes("Import definitions")
        saved.errors == ""
        saved.location.endsWith("/Editor?definitionId=" + panelId)
        owner == USER
        regrouped.errors == ""
        version == 2
        group == null
        search.errors.includes("This editor saves only panel, dashboard definitions")
    }
})

feature("a mounted dashboard reads the mount's links, policy and fixed variable; Tools' Dashboards keep Tools' own", {
    when() {
        // a mount's names sent as request parameters are not taken under the mount either: its root sets them
        const sent = {reportPolicy: {range: {from: "now-999d"}, features: {alert: true}}}
        const board = screen(mount, "Board", Object.assign({definitionId: boardId, "var-region": "US"}, sent), null, "qvt")
        const state = JSON.parse(screen(mount, "Board/mountState", sent).output)
        const inspect = screen(mount, "Board/inspectPanel", {
            definitionId: boardId,
            panelId: "note",
            variables: '{"region":"US"}'
        })
        // Tools' own Dashboards app (ADMIN): the same dashboard with Tools' links, a mount's names sent as request parameters not
        // taken, and Home's edit going to Tools' editor
        login("john.doe")
        const forged = {reportLinks: {definition: "/forged", dashboard: "/forged"}, reportPolicy: sent.reportPolicy}
        const tools = screen(webroot, "apps/dashboards/Dashboard", Object.assign({definitionId: boardId}, forged), null, "qvt")
        const edit = screen(
            webroot,
            "apps/dashboards/Home/edit",
            Object.assign({definitionId: boardId, definitionType: "dashboard"}, forged)
        )
        ec.user.internalLoginUser(USERNAME)
        return {board, mounted: decoded(board.output), state, inspect, tools, toolsText: decoded(tools.output), edit}
    },
    then({board, mounted, state, inspect, tools, toolsText, edit}) {
        board.status == 200
        board.errors == ""
        // the range the mount's policy names, not the request's
        mounted.includes('"requestedTime":{"from":"now-90d","thru":"now"}')
        !mounted.includes("now-999d")
        // what the module answers under the mount: the links the mount gave (a key it left out is not offered), the policy
        // it gave, and the default of each field it left out (a feature it did not name is not offered)
        state.mounted == true
        state.links.definition == "../Editor"
        state.links.alertRule == null
        state.links.split == null
        state.links.drill == null
        JSON.stringify(state.policy.range) == '{"from":"now-90d","thru":"now"}'
        JSON.stringify(state.policy.features) == '{"alert":false,"share":true}'
        state.policy.recipientCap == 25
        JSON.stringify(state.policy.shareGroups) == '[{"value":"REPORT_MOUNTS_WALL","label":"Wall"}]'
        state.policy.datasetIdPrefix == ""
        state.policy.recipientPermissionId == null
        state.policy.alertAudience == null
        state.policy.calendar == null
        mounted.includes(
            '"appLinks":{"definition":"/Editor","panel":"/Editor","explore":"/Board","alertRule":null,"dashboard":"/Board"}'
        )
        mounted.includes('"variableValues":{"region":"EU"}')
        !mounted.includes('"US"')
        // a panel inspect keeps the fixed variable over the viewer's variables
        inspect.errors == ""
        inspect.output.includes('"EU"')
        !inspect.output.includes('"US"')
        tools.errors == ""
        toolsText.includes('"definition":"/apps/dashboards/Definition"')
        toolsText.includes('"alertRule":"/apps/dashboards/AlertRule"')
        edit.location.includes("/apps/dashboards/Definition")
        !toolsText.includes("/forged")
        !edit.location.includes("/forged")
        // Tools' own range, whatever policy a request sends
        toolsText.includes('"requestedTime":{"from":"now-7d","thru":"now"}')
        !toolsText.includes("now-999d")
    }
})

feature("with no mount the links and the policy are Tools' own; Tools' roots null the mount's names and one module reads them", {
    when() {
        const lib = ec.resource.script(mountLib)
        const own = lib.state()
        // a value that is not a map: a mount with nothing in it
        ec.context.put("reportPolicy", "forged")
        const empty = lib.state()
        const emptyUrl = lib.url("dashboard")
        ec.context.put("reportPolicy", null)
        // an area's addition: nothing with no mount; under a mount the root's fields with the area's, which the area sets
        const noArea = lib.policyWith({recipientCap: 5})
        ec.context.put("reportPolicy", {range: {from: "now-90d"}, recipientCap: 25})
        const area = lib.policyWith({recipientCap: 5})
        ec.context.put("reportPolicy", area)
        const under = lib.state().policy
        ec.context.put("reportPolicy", null)
        const nulling = MOUNT_NAMES.map(name => name + "=null").join(";")
        const roots = ["/Dashboards.xml", "/System/Monitor.xml"].filter(
            file =>
                !String(ec.resource.getLocationText(screens + file) || "").includes(
                    "<always-actions><script>" + nulling + "</script>"
                )
        )
        const readers = screenFiles(screens + "/Dashboards")
            .concat(screenFiles(screens + "/System/Monitor"))
            .filter(location =>
                /(context\.get\(|context\.)['"]?(reportLinks|reportPolicy)\b/.test(
                    String(ec.resource.getLocationText(location) || "")
                )
            )
        return {own, empty, emptyUrl, noArea, area, under, roots, readers}
    },
    then({own, empty, emptyUrl, noArea, area, under, roots, readers}) {
        own.mounted == false
        own.links.definition == "//apps/dashboards/Definition"
        own.links.alerts == "//apps/dashboards/Alerts"
        own.links.import == "//apps/dashboards/Home"
        own.links.share == null
        own.links.drill == null
        JSON.stringify(own.policy) ==
            '{"features":{"alert":true,"share":true},"range":{"from":"now-7d","thru":"now"},"datasetIdPrefix":"",' +
                '"datasetScopeParameter":null,"datasetScopeLabel":"Scope field",' +
                '"datasetScopeHelp":"The field that carries each row\'s scope; the policy\'s own when left empty",' +
                '"recipientPermissionId":null,"reportEmailTemplateId":null,"alertAudience":null,' +
                '"shareGroups":[],"recipientCap":null,"calendar":null}'
        empty.mounted == true
        Object.values(empty.links).every(link => link == null)
        JSON.stringify(empty.policy.features) == '{"alert":false,"share":false}'
        empty.policy.range.from == "now-7d"
        // a transition's answer still has somewhere to go: Tools' screen, which opens to its own users only
        emptyUrl == "//apps/dashboards/Dashboard"
        noArea == null
        JSON.stringify(area) == '{"range":{"from":"now-90d"},"recipientCap":5}'
        under.recipientCap == 5 && under.range.from == "now-90d"
        roots.join(", ") == ""
        readers.join(", ") == ""
    }
})
