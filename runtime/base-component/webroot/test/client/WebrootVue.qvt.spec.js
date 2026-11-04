// The qvt components, in the page. Was js/WebrootVue.qvt.test.js.
// WebrootVue.qvt.js starts the Vue app only when the page has #moqui-config plus Vue and Quasar. This page has none of those.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/MoquiL10n.js", "/libs/vue/compiler-dom.cjs.js", "/js/WebrootVue.qvt.js"],
    test: "/files/runtime/base-component/webroot/test/client/pages/WebrootVue.qvt.page.js",
})

// From WebrootVue.qvt.start.spec.js, merged here to share a spec run.
// qvt.start, in the page, with stand-ins for Vue and Quasar: what is set before the mount, which waits for the language catalog.

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/MoquiL10n.js", "/js/WebrootVue.qvt.js"],
    test: "/files/runtime/base-component/webroot/test/client/pages/WebrootVue.qvt.start.page.js",
})

// From WebrootVueDisplay.spec.js, merged here to share a spec run.
// m-display in a real Vue page. Was js/WebrootVue.qvt.display.test.js.

feature("labelled display values use Java read-only outlined inputs and inline values stay text", {
    expect() {
        const page = browser.newPage()
        page.setContent(`<!doctype html><html><head>
            <link rel="stylesheet" href="/libs/quasar/quasar.prod.css">
            </head><body><div id="app">
            <m-display id="invoice" name="invoiceId" label="Invoice ID" label-wrapper model-value="UI_PARITY_INVOICE"></m-display>
            <m-display id="total" name="total" label="Total" label-wrapper display="$0.00"></m-display>
            <m-display id="zero" name="zero" label="Zero" label-wrapper display="0"></m-display>
            <m-display id="multiline" name="multiline" label="Notes" label-wrapper display="First&#10;Second"></m-display>
            <m-display id="inline" model-value="inline text"></m-display>
            </div>
            <script src="/libs/vue/vue.global.prod.js"></script>
            <script src="/libs/quasar/quasar.umd.prod.js"></script>
            <script src="/js/MoquiLib.js"></script>
            <script src="/js/WebrootVue.qvt.js"></script>
            <script>
            const app = Vue.createApp({});
            app.use(Quasar);
            app.component("m-display", moqui.qvt.components["m-display"]);
            app.mount("#app");
            </script></body></html>`)
        const result = page.waitFor(() => {
            const input = document.getElementById("invoice")
            if (!input || !input.value) return null
            const total = document.getElementById("total")
            const zero = document.getElementById("zero")
            const multiline = document.getElementById("multiline")
            return {
                value: input.value, name: input.name, readOnly: input.readOnly,
                outlined: !!input.closest(".q-field--outlined"), label: input.closest(".q-field").textContent,
                total: total.value, zero: zero.value, multiline: multiline.value,
                multilineIsTextarea: multiline.matches("textarea"),
                inline: document.getElementById("inline").textContent,
                inlineIsInput: document.getElementById("inline").matches("input,textarea"),
            }
        })
        assert.equal(result.value, "UI_PARITY_INVOICE")
        assert.equal(result.name, "invoiceId")
        assert.equal(result.readOnly, true)
        assert.equal(result.outlined, true)
        assert.match(result.label, /Invoice ID/)
        assert.equal(result.total, "$0.00")
        assert.equal(result.zero, "0")
        assert.equal(result.multiline, "First\nSecond")
        assert.equal(result.multilineIsTextarea, true)
        assert.equal(result.inline, "inline text")
        assert.equal(result.inlineIsInput, false)
    },
})
