// m-markdown on the vendored Marked, in the page. Was js/WebrootVue.qvt.markdown.test.js.
// The marked UMD installs globalThis.marked. WebrootVue.qvt.js does not start the app on this page.
spec({ server: false, timeout: 60 })

feature("code editor names reach the editable content, restored buffers and the plain-text fallback", {
    expect() {
        const page = browser.newPage()
        page.allowProblems(/The code editor could not load; editing as plain text[\s\S]*fixture bundle unavailable/)
        page.setContent(`<!doctype html><html><head><link rel="stylesheet" href="/libs/quasar/quasar.prod.css"></head><body><div id="app">
            <section id="field"><m-text-area editor-type="code" name="description" language="json" :label="label" v-model="content"></m-text-area></section>
            <m-code-editor v-if="showBuffer" ref="buffer" id="buffer" state-key="name-fixture" :aria-label="bufferLabel" :aria-labelledby="labelledby" v-model="buffer"></m-code-editor>
            <span id="buffer-heading">Related source</span><m-code-editor id="unnamed" model-value="unnamed"></m-code-editor>
            </div><div id="fallback"></div>
            <script src="/libs/vue/vue.global.prod.js"></script><script src="/libs/quasar/quasar.umd.prod.js"></script>
            <script src="/libs/codemirror/codemirror.min.js"></script><script src="/js/MoquiLib.js"></script>
            <script src="/js/WebrootVue.qvt.js"></script><script src="/js/WebrootCode.js"></script><script>
            const app=Vue.createApp({data(){return {label:'Description (JSON)',content:'{}',buffer:'abc',bufferLabel:'Source',labelledby:undefined,showBuffer:true};}});
            app.use(Quasar);app.component('m-text-area',moqui.qvt.components['m-text-area']);app.component('m-code-editor',moqui.codeEditor.components['m-code-editor']);
            window.vm=app.mount('#app');window.mountFallback=()=>{
                const load=moqui.codeEditor.load;moqui.codeEditor.load=()=>Promise.reject(new Error('fixture bundle unavailable'));
                const fallbackApp=Vue.createApp({template:'<m-code-editor aria-label="Fallback source" name="fallbackSource" model-value="plain text"></m-code-editor>'});
                fallbackApp.use(Quasar);fallbackApp.component('m-code-editor',moqui.codeEditor.components['m-code-editor']);fallbackApp.mount('#fallback');moqui.codeEditor.load=load;
            };</script></body></html>`)
        page.waitFor(() => document.querySelectorAll('.cm-content').length === 3)
        assert.equal(page.locator('#field .cm-content[role="textbox"]').evaluate(el => el.getAttribute('aria-label')), 'Description (JSON)')
        assert.equal(page.locator('#field input[type="hidden"]').evaluate(el => el.name), 'description')
        assert.equal(page.locator('#unnamed .cm-content').evaluate(el => el.hasAttribute('aria-label') || el.hasAttribute('aria-labelledby')), false)
        page.evaluate(async () => {vm.label='Revised description';await Vue.nextTick();return true})
        assert.equal(page.locator('#field .cm-content').evaluate(el => el.getAttribute('aria-label')), 'Revised description')
        page.evaluate(async () => {
            vm.$refs.buffer.view.dispatch({changes:{from:3,insert:'!'}});await Vue.nextTick()
            vm.showBuffer=false;await Vue.nextTick();vm.bufferLabel='Restored source';vm.showBuffer=true;await Vue.nextTick();return true
        })
        page.waitFor(() => !!document.querySelector('#buffer .cm-content'))
        assert.equal(page.locator('#buffer .cm-content').evaluate(el => el.getAttribute('aria-label')), 'Restored source')
        assert.equal(page.evaluate(() => vm.$refs.buffer.view.state.doc.toString()), 'abc!')
        page.evaluate(async () => {vm.labelledby='buffer-heading';await Vue.nextTick();return true})
        assert.equal(page.locator('#buffer .cm-content').evaluate(el => el.getAttribute('aria-labelledby')), 'buffer-heading')
        page.evaluate(async () => {CodeMirror.undo(vm.$refs.buffer.view);await Vue.nextTick();return true})
        assert.equal(page.evaluate(() => vm.buffer), 'abc', 'naming updates preserve the retained undo history and model binding')
        page.evaluate(async () => {vm.bufferLabel=undefined;vm.labelledby=undefined;await Vue.nextTick();return true})
        assert.equal(page.locator('#buffer .cm-content').evaluate(el => el.hasAttribute('aria-label') || el.hasAttribute('aria-labelledby')), false)
        page.evaluate(() => {mountFallback();return true})
        page.waitFor(() => !!document.querySelector('#fallback textarea'))
        assert.deepEqual(page.locator('#fallback textarea').evaluate(el => ({label:el.getAttribute('aria-label'),name:el.name,value:el.value})),
            {label:'Fallback source',name:'fallbackSource',value:'plain text'})
    },
})

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/WebrootVue.qvt.js", "/libs/marked/marked.umd.min.js"],
    test: "/files/runtime/base-component/webroot/test/client/pages/WebrootVue.qvt.markdown.page.js",
})

// From WebrootCode.spec.js, merged here to share a spec run.
// The code editor's helpers and the vendored CodeMirror bundle, in the page. Was js/WebrootCode.test.js.

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/libs/codemirror/codemirror.min.js", "/js/WebrootCode.js"],
    test: "/files/runtime/base-component/webroot/test/client/pages/WebrootCode.page.js",
})

// Tools' SQL screen (component://tools/screen/Tools/Sql.qvue) mounted in the page with its transitions answered by fixtures
// shaped like org.moqui.impl.DatabaseServices' answers: a statement at the cursor with a :name parameter, its result in the
// grid and Fetch more, a run with changes through classify#Sql's confirmation and the typed database name, and an
// explain's plan with its marks. The services themselves are tested in crates/moqui-tools.
feature("the SQL screen runs a statement, fetches more, confirms changes and shows a plan", {
    expect() {
        const page = browser.newPage()
        page.setContent(`<!doctype html><html><head><link rel="stylesheet" href="/libs/quasar/quasar.prod.css"></head><body><div id="app"></div>
            <script src="/libs/vue/vue.global.prod.js"></script><script src="/libs/quasar/quasar.umd.prod.js"></script>
            <script src="/libs/codemirror/codemirror.min.js"></script><script src="/js/MoquiLib.js"></script>
            <script src="/js/WebrootVue.qvt.js"></script><script src="/js/WebrootCode.js"></script><script>
            const plan = JSON.stringify([{Plan: {"Node Type": "Hash Join", "Hash Batches": 4, "Plan Rows": 100, Plans: [
                {"Node Type": "Seq Scan", "Relation Name": "enumeration", "Plan Rows": 50000}]}}])
            const answers = {
                sqlInfo: () => ({db: "group:transactional", backend: "postgres", canRun: true, databases: [{value: "group:transactional", label: "transactional (postgres)"},
                    {value: "group:content", label: "content (postgres)"}]}),
                catalog: () => ({dialect: "postgres", tables: {enumeration: ["enum_id", "description"]}, entities: {"moqui.basic.Enumeration": "enumeration"}}),
                run: (p) => p.sqlText == "select fail" ? {errors: "relation fail does not exist"} : p.offset ? {results: [{seq: 1, sql: p.sqlText, givesRows: true, columns: ["a", "b"], records: [[3, "z"]], limitReached: false, rerunnable: true}]}
                    : p.allowChanges ? {results: [{seq: 1, sql: "select 1", givesRows: true, columns: ["x"], records: [[1]]}, {seq: 2, sql: "update t set x = 1", givesRows: false, rowsAffected: 3}]}
                    : {results: [{seq: 1, sql: p.sqlText, givesRows: true, columns: ["a", "b"], records: [[1, null], [2, "x"]], limitReached: true, rerunnable: true}]},
                classify: (p) => /update/.test(p.sqlText) ? {statements: [{sql: "select 1", changes: false}, {sql: "update t set x = 1", changes: true}],
                    changeCount: 1, token: "T1", confirmDatabaseName: "moqui", implicitCommit: false}
                    : {statements: [{sql: p.sqlText, changes: false}], changeCount: 0, token: "T2", confirmDatabaseName: "moqui"},
                explain: () => ({format: "json", plan, analyzed: false, relationRows: {enumeration: 50000}})
            }
            window.calls = []
            moqui.request = (url, options) => {
                const name = url.split("/").pop(), params = Object.assign({}, options && options.params)
                calls.push({name, method: (options && options.method) || "GET", params})
                return Promise.resolve({ok: true, status: 200, json: answers[name] ? answers[name](params) : {}})
            }
            window.notified = []
            moqui.notifyMessages = (messages, errors) => { notified.push(errors || messages) }
            moqui.handleAjaxError = (err) => { notified.push(String(err)) }
            fetch("/files/runtime/base-component/tools/screen/Tools/Sql.qvue").then((r) => r.text()).then((text) => {
                const module = {}
                // the script and template of the component file, cut by index: a closing script tag would end this script
                const part = (tag) => text.slice(text.indexOf("<" + tag + ">") + tag.length + 2, text.lastIndexOf("<" + "/" + tag + ">"))
                new Function("module", part("script"))(module)
                const component = Object.assign(module.exports, {template: part("template")})
                const app = Vue.createApp({data() { return {basePath: "/apps", linkBasePath: "/qapps", currentPathList: ["tools", "Sql"]} },
                    template: '<sql-screen ref="screen"></sql-screen>'})
                app.use(Quasar)
                app.component("sql-screen", component)
                app.component("m-code-editor", moqui.codeEditor.components["m-code-editor"])
                window.vm = app.mount("#app")
            })
            </script></body></html>`)
        page.waitFor(() => !!(window.vm && vm.$refs.screen && vm.$refs.screen.databases.length === 2 && vm.$refs.screen.schema && window.Tabulator && moqui.sql))
        assert.equal(page.evaluate(() => vm.$refs.screen.editorLanguage), "sql:postgres")
        assert.deepEqual(page.evaluate(() => JSON.parse(JSON.stringify(vm.$refs.screen.tableEntities))), {enumeration: ["moqui.basic.Enumeration"]})
        // the statement at the cursor, its :id parameter bound by name
        page.evaluate(async () => { const s = vm.$refs.screen; s.sqlText = "select :id as a, b from enumeration;\nupdate t set x = 1"; s.cursor = 3
            s.parameterValues.id = "A1"; await Vue.nextTick(); s.view = null; s.run("statement"); return true })
        page.waitFor(() => document.querySelectorAll("#app .tabulator-row").length === 2)
        const run = page.evaluate(() => JSON.parse(JSON.stringify(calls.filter((call) => call.name === "run")[0])))
        assert.equal(run.method, "POST")
        assert.equal(run.params.sqlText, "select :id as a, b from enumeration")
        assert.equal(run.params.parametersJson, JSON.stringify({id: "A1"}))
        assert.equal(run.params.db, "group:transactional")
        assert.ok(!run.params.allowChanges && !run.params.token, "read only unless Allow changes")
        assert.equal(page.evaluate(() => document.querySelector("#app .tabulator-cell.m-grid-null").textContent), "null")
        // Fetch more: the same statement from the rows the grid has
        page.evaluate(() => { const s = vm.$refs.screen; s.fetchMore(s.current); return true })
        page.waitFor(() => document.querySelectorAll("#app .tabulator-row").length === 3)
        assert.equal(page.evaluate(() => calls.filter((call) => call.name === "run")[1].params.offset), 2)
        // Allow changes: classify, the confirmation waits for the typed name, then the run with its token
        page.evaluate(async () => { const s = vm.$refs.screen; s.allowChanges = true; s.run("script"); return true })
        page.waitFor(() => vm.$refs.screen.confirmOpen === true)
        assert.equal(page.evaluate(() => calls.filter((call) => call.name === "run").length), 2, "nothing runs before the confirmation")
        assert.deepEqual(page.evaluate(() => JSON.parse(JSON.stringify(vm.$refs.screen.confirm.statements.map((statement) => statement.sql)))), ["update t set x = 1"])
        page.evaluate(async () => { const s = vm.$refs.screen; s.typedName = "moqui"; s.confirmed(); return true })
        page.waitFor(() => vm.$refs.screen.lines.length === 1)
        const changed = page.evaluate(() => JSON.parse(JSON.stringify(calls.filter((call) => call.name === "run")[2].params)))
        assert.equal(changed.allowChanges, true)
        assert.equal(changed.token, "T1")
        assert.equal(changed.confirmDatabaseName, "moqui")
        assert.equal(page.evaluate(() => vm.$refs.screen.lines[0].text), "Statement 2: 3 rows changed")
        // Explain: the plan tree with its marks
        page.evaluate(() => { const s = vm.$refs.screen; s.cursor = 3; s.explain(false); return true })
        page.waitFor(() => vm.$refs.screen.planOpen === true && document.body.textContent.includes("Seq scan over 10k rows"))
        assert.ok(page.evaluate(() => document.body.textContent.includes("Spilled to disk")))
        assert.equal(page.evaluate(() => JSON.stringify(notified)), "[]")
        // Allow changes on, nothing that changes data: run read only, no confirmation, no token
        page.evaluate(async () => { const s = vm.$refs.screen; s.planOpen = false; s.sqlText = "select 9"; s.cursor = 1; s.run("statement"); return true })
        page.waitFor(() => calls.filter((call) => call.name === "run").length === 4)
        const readOnly = page.evaluate(() => JSON.parse(JSON.stringify(calls.filter((call) => call.name === "run")[3].params)))
        assert.ok(!readOnly.allowChanges && !readOnly.token, "nothing to confirm")
        assert.equal(page.evaluate(() => vm.$refs.screen.confirmOpen), false)
        // a run that fails leaves no results of the run before it
        page.evaluate(async () => { const s = vm.$refs.screen; s.allowChanges = false; s.sqlText = "select fail"; s.cursor = 1; s.run("statement"); return true })
        page.waitFor(() => vm.$refs.screen.results.length === 0 && notified.length === 1)
        assert.equal(page.evaluate(() => notified[0]), "relation fail does not exist")
    },
})

// The SQL editor's helpers (js/WebrootSql.js), in the page.

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/WebrootSql.js"],
    test: "/files/runtime/base-component/webroot/test/client/pages/WebrootSql.page.js",
})

// From WebrootVue.qvt.code.spec.js, merged here to share a spec run.
// m-code on the vendored highlight.js, in the page. Was js/WebrootVue.qvt.code.test.js.

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/WebrootVue.qvt.js", "/libs/highlightjs/highlight.min.js"],
    test: "/files/runtime/base-component/webroot/test/client/pages/WebrootVue.qvt.code.page.js",
})

// From WebrootVue.qvt.coedit.spec.js, merged here to share a spec run.
// m-coedit's provider on the vendored Tiptap bundle, in the page. Was js/WebrootVue.qvt.coedit.test.js.

browser.runInPage({
    scripts: ["/libs/tiptap/tiptap.min.js", "/js/MoquiLib.js", "/js/WebrootVue.qvt.js"],
    test: "/files/runtime/base-component/webroot/test/client/pages/WebrootVue.qvt.coedit.page.js",
})

