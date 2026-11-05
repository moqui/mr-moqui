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

