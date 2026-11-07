// Tools' report screens mounted under another app's root (docs/tools/Monitoring.md, "Mounting the report screens"; BOSS Insights
// P2, INS-04): the test mount test/screen/ReportMount.xml takes in Tools' Dashboard and Definition with their transitions and sets
// reportLinks. A mounted editor offers the datasets the author may read by each dataset's own artifact, without DASHBOARDS_APP or
// REPORT_DATASETS_ADMIN, starts a new panel on one of them and saves and returns under the mount; a mounted dashboard sends the
// mount's links to the browser and terminal (appLinks), leaves out the screens the mount does not offer and keeps its fixed
// variables over a viewer's; Tools' own Dashboards screens keep Tools' links.
// Run: ./moq test tools --spec ReportMounts
const mount = "component://tools/test/screen/ReportMount.xml"
const webroot = "component://webroot/screen/webroot.xml"
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
        const stored = sys(() => ec.entity.find("moqui.report.Definition").condition("definitionId", panelId).one())
        const text = decoded(editor.output)
        return {editor, text, newPanelDataset: /"datasetId":\s*"(\w+)"/.exec(text)?.[1], saved, owner: stored?.ownerId}
    },
    then({editor, text, newPanelDataset, saved, owner}) {
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
    }
})

feature("a mounted dashboard sends the mount's links and keeps its fixed variable; Tools' Dashboards keep Tools' links", {
    when() {
        const board = screen(mount, "Board", {definitionId: boardId, "var-region": "US"}, null, "qvt")
        // Tools' own Dashboards app (ADMIN): the same dashboard with Tools' links, and Home's edit going to Tools' editor
        login("john.doe")
        const tools = screen(webroot, "apps/dashboards/Dashboard", {definitionId: boardId}, null, "qvt")
        const edit = screen(webroot, "apps/dashboards/Home/edit", {definitionId: boardId, definitionType: "dashboard"})
        ec.user.internalLoginUser(USERNAME)
        return {board, mounted: decoded(board.output), tools, toolsText: decoded(tools.output), edit}
    },
    then({board, mounted, tools, toolsText, edit}) {
        board.status == 200
        board.errors == ""
        mounted.includes(
            '"appLinks":{"definition":"/Editor","panel":"/Editor","explore":"/Board","alertRule":null,"dashboard":"/Board"}'
        )
        mounted.includes('"variableValues":{"region":"EU"}')
        !mounted.includes('"US"')
        tools.errors == ""
        toolsText.includes('"definition":"/apps/dashboards/Definition"')
        toolsText.includes('"alertRule":"/apps/dashboards/AlertRule"')
        edit.location.includes("/apps/dashboards/Definition")
    }
})
