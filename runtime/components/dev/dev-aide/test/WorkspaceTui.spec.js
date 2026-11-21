// The Workspace's terminal form (screen/DevAide/WorkspaceTui.xml, its own tui template) on a throw-away root: a
// directory in the run's scratch runtime, added as a root and made a git repository, never the real files. The file tree's
// transition (a directory's entries with their git letters, the open path's directories with theirs), Save, a save over a
// file changed on disk (the conflict kept for the compare) and Save mine over it; one tui render of the Workspace (its
// dynamic container at a palette link's file and line) and one of the twin, with its header, tabs, tree, editor and that
// conflict's diff (and no References for a file that defines no artifact), with the artifacts of a file the index has,
// which the File menu's References takes; Repositories' and Home's rows of the root opening the Workspace at it with no
// path. The directory goes with the scratch runtime.
// Run: ./moq test dev-aide --spec WorkspaceTui
const WS = "org.moqui.devaide.WorkspaceServices."
const ws = (name, params) =>
    ec.service
        .sync()
        .name(WS + name)
        .parameters(params || {})
        .call() || {}
const devAide = () => ec.screen.makeTest().rootScreen("component://dev-aide/screen/DevAide.xml")
const fixture = () => ec.factory.getRuntimePath() + "/devaide-workspace-spec"
const fileRef = relative => ec.resource.getLocationReference("file://" + fixture() + "/" + relative)
const rootOf = () => Array.from(ws("get#Roots").roots || []).find(root => root.label == "Workspace spec")
const diskText = relative => String(fileRef(relative).getText() || "")
const conflictKept = () => JSON.parse(ec.web.sessionAttributes["DevAideTui.conflict"] || "null")

spec({
    setupSpec() {
        login("john.doe")
        fileRef("a.txt").putText("one\ntwo\n")
        fileRef("sub/b.xml").putText("<screen/>\n")
        ws("store#Root", {location: fixture(), label: "Workspace spec"})
        // a plain directory is no site component: writing in it takes the full tier's upstream edits, off again at the end
        ws("set#UpstreamEdits", {enabled: true})
        ec.service.sync().name("org.moqui.devaide.VcsServices.init#Repository").parameters({rootId: rootOf().id}).call()
    },
    cleanupSpec() {
        ws("set#UpstreamEdits", {enabled: false})
        ws("delete#Root", {location: fixture()})
        ec.web.sessionAttributes.remove("DevAideTui.conflict")
        ec.web.sessionAttributes.remove("DevAideTui.problems")
    }
})

feature("the file tree answers a directory's entries with their git letters, and the open path's directories with theirs", {
    when() {
        const rootId = rootOf().id
        const top = devAide().render("WorkspaceTui/fileTree", {rootId, treeNodeId: "#", treeOpenPath: "sub/b.xml"}, "post")
        const below = devAide().render("WorkspaceTui/fileTree", {rootId, treeNodeId: "sub"}, "post")
        return {
            rootId,
            top: top.jsonObject,
            below: below.jsonObject,
            errors: top.errorMessages.length + below.errorMessages.length
        }
    },
    then({rootId, top, below, errors}) {
        errors == 0
        rootOf().kind == "git"
        top.map(node => node.id).join(",") == "sub,a.txt"
        top[0].text == "? sub/"
        top[0].children.length == 1
        top[0].children[0].id == "sub/b.xml"
        top[1].text == "? a.txt"
        top[1].children === false
        top[1].a_attr.urlText.endsWith("/Workspace?rootId=" + encodeURIComponent(rootId) + "&path=a.txt")
        below.length == 1
        below[0].text == "? b.xml"
        below[0].a_attr.urlText.includes("path=sub%2Fb.xml")
    }
})

feature("a save writes the file read at its hash; a save over a file changed on disk keeps both texts and writes nothing", {
    when() {
        const rootId = rootOf().id
        const read = ws("get#File", {rootId, path: "a.txt"})
        const saved = devAide().render(
            "WorkspaceTui/saveFile",
            {rootId, path: "a.txt", baseHash: read.hash, fileText: "one\ntwo\nthree\n"},
            "post"
        )
        const afterSave = diskText("a.txt")
        // someone else writes it: the next save, still at the first hash, is a conflict
        fileRef("a.txt").putText("disk\n")
        const refused = devAide().render(
            "WorkspaceTui/saveFile",
            {rootId, path: "a.txt", baseHash: read.hash, fileText: "mine\n"},
            "post"
        )
        return {saved, afterSave, refused, afterConflict: diskText("a.txt"), kept: conflictKept()}
    },
    then({saved, afterSave, refused, afterConflict, kept}) {
        saved.errorMessages.length == 0
        afterSave == "one\ntwo\nthree\n"
        refused.errorMessages.length == 0
        afterConflict == "disk\n"
        kept.path == "a.txt"
        kept.text == "mine\n"
        kept.diskText == "disk\n"
    }
})

feature(
    "the Workspace in the terminal opens its terminal form at a palette link's file and line: the header, tabs, tree, editor and the conflict's diff",
    {
        when() {
            const rootId = rootOf().id
            // a palette row's link (artifacts.rs workspace_url): the Workspace with rootId, path and line
            const workspace = devAide().renderMode("tui").render("Workspace", {rootId, path: "a.txt", line: "2"}, null)
            const page = devAide().renderMode("tui").render("WorkspaceTui", {rootId, path: "a.txt", line: "2"}, null)
            const find = nodes =>
                Array.from(nodes || [])
                    .map(node => (node.t == "dynamicContainer" ? node : find(node.c)))
                    .find(node => node)
            // the artifacts of a file the index has, which a File menu's References takes: this component's services
            const base = Array.from(ws("get#Roots").roots || []).find(one =>
                Array.from(one.components || []).some(component => component.name == "dev-aide")
            )
            const component = base.components.find(one => one.name == "dev-aide")
            const inFile = ec.service
                .sync()
                .name("org.moqui.devaide.ArtifactServices.get#ArtifactsInFile")
                .parameters({
                    rootId: base.id,
                    path: (component.path ? component.path + "/" : "") + "service/org/moqui/devaide/ArtifactServices.xml"
                })
                .call()
            return {
                rootId,
                container: find(workspace.jsonObject.c) || {},
                page,
                out: String(page.output),
                inFile: Array.from(inFile.artifacts || [])
            }
        },
        then({rootId, container, page, out, inFile}) {
            container.t == "dynamicContainer"
            container.url.includes("/WorkspaceTui?")
            container.url.includes("rootId=" + rootId)
            container.url.includes("path=a.txt")
            container.url.includes("line=2")
            page.status == 200
            page.errorMessages.length == 0
            !/Error rendering|Exception in render/.test(out)
            out.includes('"WorkspaceRoot"')
            out.includes('"WorkspaceTabs"')
            out.includes('"title":"Changes"')
            out.includes('"WorkspaceFileTree"')
            out.includes('"name":"code"')
            out.includes('"form":"WorkspaceFile"')
            out.includes('"cursorLine":2')
            out.includes("a.txt changed on disk since it was opened")
            out.includes("-disk")
            out.includes("+mine")
            out.includes("Save mine over it")
            out.includes("Take the disk's")
            // a.txt defines no artifact: the File menu has no References or Browse
            !out.includes("References")
            !out.includes('"text":"Browse"')
            inFile.some(
                artifact =>
                    artifact.id == "service:org.moqui.devaide.ArtifactServices.get#ArtifactsInFile" &&
                    artifact.shortName == "get#ArtifactsInFile" &&
                    artifact.relativePath.endsWith("/ArtifactServices.xml")
            )
        }
    }
)

// Repositories' one render: a root's row opens the Workspace at the root, with no path, where the row's own absolute directory
// once filled Workspace's path ("... is absolute; paths are relative to their root")
feature("Repositories' row of a root opens the Workspace at that root with no path", {
    when() {
        const rootId = rootOf().id
        const page = devAide().render("Repositories", {}, null)
        const out = String(page.output)
        const link = (out.match(new RegExp('<a href="([^"]*)"[^>]*>Workspace spec[^<]*</a>')) || [])[1] || ""
        return {rootId, page, link}
    },
    then({rootId, page, link}) {
        page.status == 200
        page.errorMessages.length == 0
        link.includes("Workspace?")
        link.includes("rootId=" + rootId)
        !link.includes("path=")
    }
})

// Home's one render: its roots' rows open the Workspace as Repositories' do, with no path
feature("Home's row of a root opens the Workspace at that root with no path", {
    when() {
        const rootId = rootOf().id
        const page = devAide().render("Home", {}, null)
        const out = String(page.output)
        const link = (out.match(new RegExp('<a href="([^"]*)"[^>]*>Workspace spec[^<]*</a>')) || [])[1] || ""
        return {rootId, page, link}
    },
    then({rootId, page, link}) {
        page.status == 200
        page.errorMessages.length == 0
        link.includes("Workspace?")
        link.includes("rootId=" + rootId)
        !link.includes("path=")
    }
})

feature("Save mine over it writes the text the conflict kept over the disk's", {
    when() {
        const rootId = rootOf().id
        const over = devAide().render("WorkspaceTui/saveFile", {rootId, path: "a.txt", overDisk: "true"}, "post")
        return {over, after: diskText("a.txt"), kept: conflictKept()}
    },
    then({over, after, kept}) {
        over.errorMessages.length == 0
        after == "mine\n"
        kept == null
    }
})
