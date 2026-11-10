// Was js/WebrootVue.qvt.views.test.js.
// WebrootVue.qvt.js starts the Vue app only when the page has #moqui-config plus Vue and Quasar. This page has none of those.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/WebrootVue.qvt.js"],
    test: "/files/runtime/base-component/webroot/test/client/pages/WebrootVue.qvt.views.page.js",
})

// From WebrootVue.qvt.htmlview.spec.js, merged here to share a spec run.
// Was js/WebrootVue.qvt.htmlview.test.js.
// WebrootVue.qvt.js starts the Vue app only when the page has #moqui-config plus Vue and Quasar. This page has none of those.

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/WebrootVue.qvt.js"],
    test: "/files/runtime/base-component/webroot/test/client/pages/WebrootVue.qvt.htmlview.page.js",
})

// Change 4: Regression for the initial empty frame remaining blank after its srcdoc receives the message body.
feature("HTML message frames load their first plain and asynchronous HTML bodies and later updates", {
    expect() {
        const page = browser.newPage()
        // The test runner injects a script into every document; the sandbox correctly refuses it.
        page.allowProblems(/Blocked script execution in 'about:srcdoc' because the document's frame is sandboxed and the 'allow-scripts' permission is not set\./)
        page.setContent(`<!doctype html><html><body><div id="app" style="font-family:monospace;font-size:17px"></div>
            <script src="/libs/vue/vue.runtime.global.prod.js"></script><script src="/libs/quasar/quasar.umd.prod.js"></script>
            <script src="/js/MoquiLib.js"></script><script src="/js/WebrootVue.qvt.render.js"></script><script src="/js/WebrootVue.qvt.js"></script>
            <script>
            moqui.qvt.applyRenders();
            const view = moqui.qvt.components['m-html-view'];
            const app = Vue.createApp({
                data() { return {plain:'First plain body',html:'<p>First HTML body</p>'}; },
                render() { return Vue.h('div', [Vue.h(view,{id:'plain',text:this.plain}), Vue.h(view,{id:'html',html:this.html})]); }
            });
            app.use(Quasar); window.mountMessages = () => {window.vm = app.mount('#app')};
            window.messageBody = id => document.querySelector('#'+id+' iframe')?.contentDocument?.body?.textContent;
            </script></body></html>`)
        assert.equal(page.evaluate(async () => {
            mountMessages();
            const end = Date.now() + 5000;
            while (Date.now() < end) {
                if (messageBody('plain') === 'First plain body' && messageBody('html') === 'First HTML body') return true;
                await new Promise(resolve => setTimeout(resolve, 25));
            }
            return false;
        }), true, 'the mounted frames load real documents, not just srcdoc attributes')
        assert.equal(page.evaluate(() => document.querySelector('#plain iframe').contentDocument.defaultView.getComputedStyle(document.querySelector('#plain iframe').contentDocument.body).fontFamily), 'monospace')
        assert.equal(page.evaluate(() => document.querySelector('#html iframe').sandbox.contains('allow-scripts')), false)
        page.evaluate(() => { vm.plain = 'Updated plain body'; vm.html = '<p>Updated HTML body</p>'; return true })
        page.waitFor(() => messageBody('plain') === 'Updated plain body' && messageBody('html') === 'Updated HTML body')
        assert.equal(page.evaluate(() => document.querySelectorAll('.m-html-view iframe').length), 2)
    },
})
