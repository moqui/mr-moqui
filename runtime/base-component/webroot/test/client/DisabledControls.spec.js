// Real DOM events and direct component calls must both respect disabled controls.
spec({ server: false, timeout: 60 })

function exerciseDisabledControls() {
    const page = browser.newPage()
    page.setContent(`<!doctype html><html><head>
        <link rel="stylesheet" href="/libs/quasar/quasar.prod.css"></head><body><div id="app"></div>
        <script src="/libs/vue/vue.global.prod.js"></script>
        <script src="/libs/quasar/quasar.umd.prod.js"></script>
        <script src="/js/MoquiLib.js"></script><script src="/js/MoquiL10n.js"></script>
        <script src="/js/WebrootVue.qvt.js"></script><script>
        window.calls = 0;
        moqui.request = () => { window.calls++; return Promise.resolve({json:{}}); };
        const app = Vue.createApp({
            data() { return {disabled:true}; },
            methods: { getLinkPath(path) { return path; }, setUrl() { window.calls++; } },
            template: '<div><m-link ref="link" id="link" href="/allowed" :disable="disabled">Go</m-link>' +
                '<m-form ref="form" action="/write" :disable="disabled" no-validate><input id="value"><button id="submit" type="submit">Save</button></m-form>' +
                '<m-form-link ref="formLink" action="/find" :disable="disabled" no-validate><input id="find"></m-form-link></div>'
        });
        app.use(Quasar);
        Object.keys(moqui.qvt.components).forEach(name => app.component(name, moqui.qvt.components[name]));
        window.vm = app.mount('#app');
        </script></body></html>`)
    page.waitFor(() => document.querySelector("#value"))
    page.evaluate(() => {
        document.querySelector("#link").click();
        document.querySelector("#submit").click();
        document.querySelector("#value").focus();
        window.vm.$refs.form.submitForm();
        window.vm.$refs.form.submitGo();
        window.vm.$refs.formLink.submitForm();
        window.vm.$refs.formLink.submitGo();
    })
    page.keyboard.press("Enter")
    assert.equal(page.evaluate(() => window.calls), 0)
    assert.equal(page.evaluate(() => document.querySelector("#link").getAttribute("aria-disabled")), "true")
    assert.equal(page.evaluate(() => document.querySelector("#link").hasAttribute("href")), false)
    page.evaluate(() => { window.vm.disabled = false; })
    page.waitFor(() => document.querySelector("#link").hasAttribute("href"))
    page.evaluate(() => document.querySelector("#link").click())
    assert.equal(page.evaluate(() => window.calls), 1)
    page.close()
    return true
}

feature("disabled links and forms refuse clicks, keyboard and programmatic submission", {
    expect() {
        const passed = exerciseDisabledControls()
        assert.equal(passed, true)
    }
})
