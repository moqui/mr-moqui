// Replace across files (WorkspaceServices.replace#Files, plans/design-dev-aide.md 4.3) on a throw-away root
// (test/GuidedFixture.js): the preview lists each file's matches with their lines before and after and its hash; the
// matches chosen are written through the save pipeline (audited), the others kept; a file changed since its preview is
// refused by its hash while the others are still written; and with the upstream lock on every file of the root is
// refused, as Save refuses it; a regular expression (a syntax git grep has not) replaces with its groups and leaves a
// file's mixed line endings as they were. The client's rule for open tabs is DevAide.page.js's.
// Run: ./moq test dev-aide --spec ReplaceFiles
const fixture = () => ec.resource.script("component://dev-aide/test/GuidedFixture.js")
const ROOT = "dev-aide-replace-spec"
const service = (verb, noun) =>
    '    <service verb="' + verb + '" noun="' + noun + '"><in-parameters><parameter name="orderId"/></in-parameters></service>\n'
const FILES = {
    "service/a/Orders.xml": "<services>\n" + service("get", "Order") + service("put", "Order") + "</services>\n",
    "service/a/Items.xml": "<services>\n" + service("get", "Item") + "</services>\n",
    "service/a/Other.xml": "<services/>\n",
    // mixed line endings, and a regular expression's own groups
    "service/a/Mixed.xml": "<services>\r\n    <m code=\"k5\"/>\n    <m code=\"k6\"/>\r\n</services>\n"
}
const replace = parameters => {
    try {
        const out =
            ec.service
                .sync()
                .name("org.moqui.devaide.WorkspaceServices.replace#Files")
                .parameters(Object.assign({rootId: ROOT, query: "orderId", replacement: "salesOrderId"}, parameters))
                .call() || {}
        const errors = ec.message.getErrorsString()
        ec.message.clearErrors()
        return {out, errors}
    } catch (error) {
        ec.message.clearErrors()
        return {out: {}, errors: String(error.message)}
    }
}
const byPath = files => Object.fromEntries((files || []).map(file => [file.path, file]))

spec({
    setupSpec() {
        login("john.doe")
        fixture().setup(ROOT, FILES)
    },
    cleanupSpec() {
        fixture().cleanup(ROOT, Object.keys(FILES))
    }
})

feature("the preview lists each file's matches, then only the chosen ones are written, audited, and a stale file is refused", {
    when() {
        const f = fixture()
        const preview = replace({})
        const files = byPath(preview.out.files)
        const orders = files["service/a/Orders.xml"]
        const items = files["service/a/Items.xml"]
        // the second match of Orders.xml only; Items.xml changes on disk after its preview
        f.write(ROOT, "service/a/Items.xml", FILES["service/a/Items.xml"].replace("Item", "Line"))
        const applied = replace({
            files: JSON.stringify([
                {path: orders.path, hash: orders.hash, matches: [1]},
                {path: items.path, hash: items.hash, matches: [0]}
            ])
        })
        const audit = ec.entity
            .find("moqui.dev.DevAideAudit")
            .condition("rootId", ROOT)
            .condition("filePath", "service/a/Orders.xml")
            .disableAuthz()
            .list()
        const text = f.read(ROOT, "service/a/Orders.xml")
        return {preview, orders, items, applied, audit, text, itemsText: f.read(ROOT, "service/a/Items.xml")}
    },
    then({preview, orders, items, applied, audit, text, itemsText}) {
        preview.errors == ""
        preview.out.files.length == 2
        preview.out.matchCount == 3
        orders.matches.length == 2
        orders.matches[1].line == 3
        orders.matches[1].after.includes('name="salesOrderId"')
        orders.matches[1].text.includes('name="orderId"')
        orders.refused == null
        items.hash.length == 64
        applied.errors == ""
        applied.out.replaced.length == 1
        applied.out.replaced[0].count == 1
        // the first match kept, the second replaced
        text.split("\n")[1].includes('name="orderId"')
        text.split("\n")[2].includes('name="salesOrderId"')
        applied.out.refused.length == 1
        applied.out.refused[0].path == "service/a/Items.xml"
        applied.out.refused[0].reason.includes("changed since the preview")
        itemsText.includes('name="orderId"')
        audit.some(row => String(row.detail || "").includes('"replace"'))
    }
})

feature("with the upstream lock on, each file is refused in the preview and in the replace, as Save refuses it", {
    when() {
        const f = fixture()
        ec.user.setPreference("DEV_AIDE_UPSTREAM_EDITS", "false")
        try {
            const preview = replace({})
            const file = (preview.out.files || [])[0]
            const before = f.read(ROOT, file.path)
            const applied = replace({files: JSON.stringify([{path: file.path, hash: file.hash, matches: [0]}])})
            const bad = replace({regex: true, query: "a*"})
            return {preview, applied, bad, before, text: f.read(ROOT, file.path)}
        } finally {
            ec.user.setPreference("DEV_AIDE_UPSTREAM_EDITS", "true")
        }
    },
    then({preview, applied, bad, before, text}) {
        preview.out.files.length >= 1
        preview.out.files.every(file => String(file.refused || "").includes("upstream"))
        applied.out.replaced.length == 0
        applied.out.refused[0].reason.includes("upstream")
        text == before
        bad.errors.includes("empty")
    }
})

feature("a regular expression replaces with its groups, $1a as group 1 then a, and mixed line endings stay as they were", {
    when() {
        const f = fixture()
        const query = {query: 'code="(?<letter>[a-z])(\\d)"', regex: true, caseSensitive: true, replacement: 'code="${letter}$2a"', pathFilter: "Mixed"}
        const preview = replace(query)
        const file = (preview.out.files || [])[0]
        const applied = replace(Object.assign({files: JSON.stringify([{path: file.path, hash: file.hash, matches: [1]}])}, query))
        return {preview, applied, text: f.read(ROOT, "service/a/Mixed.xml")}
    },
    then({preview, applied, text}) {
        preview.errors == ""
        preview.out.files.length == 1
        preview.out.files[0].matches.length == 2
        preview.out.files[0].matches[1].after.includes('code="k6a"')
        applied.out.replaced[0].count == 1
        text == "<services>\r\n    <m code=\"k5\"/>\n    <m code=\"k6a\"/>\r\n</services>\n"
    }
})

feature("Search in files and Replace read a regular expression alike, across lines too, Search answering by line", {
    when() {
        const query = {query: "</service>\\r?\\n    <service verb=\"put\"", regex: true, caseSensitive: true, pathFilter: "Orders"}
        const searched = ec.service.sync().name("org.moqui.devaide.WorkspaceServices.search#Files")
            .parameters(Object.assign({rootId: ROOT}, query)).call() || {}
        ec.message.clearErrors()
        const preview = replace(query)
        return {searched, preview}
    },
    then({searched, preview}) {
        searched.results.length == 1
        searched.results[0].path == "service/a/Orders.xml"
        searched.results[0].line == 2
        preview.out.files[0].matches[0].line == 2
    }
})

feature("a text is searched with git grep in a repository root, its output read up to a cap", {
    when() {
        const roots = ec.service.sync().name("org.moqui.devaide.WorkspaceServices.get#Roots").call().roots || []
        const repository = roots.find(root => root.kind == "git")
        const searched = repository
            ? ec.service.sync().name("org.moqui.devaide.WorkspaceServices.search#Files")
                  .parameters({rootId: repository.id, query: "throw-away root the guided editors", pathFilter: "runtime/components/dev/dev-aide/test"})
                  .call() || {}
            : null
        ec.message.clearErrors()
        return {repository, searched}
    },
    then({repository, searched}) {
        !repository || searched.results.some(hit => hit.path.endsWith("test/GuidedFixture.js") && hit.line == 3)
        !repository || searched.truncated == false
    }
})
