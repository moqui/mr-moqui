// m-markdown on the vendored Marked, in the page. Was js/WebrootVue.qvt.markdown.test.js.
// The marked UMD installs globalThis.marked. WebrootVue.qvt.js does not start the app on this page.
spec({ server: false, timeout: 60 })

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

feature("Mermaid defaults follow fresh and changed color modes while explicit themes stay fixed", {
    expect() {
        const page = browser.newPage()
        page.setContent(`<!doctype html><html><head>
            <link rel="stylesheet" href="/libs/quasar/quasar.prod.css">
            </head><body><div id="app">
            <m-markdown id="markdown" ref="markdown" :text="markdown"></m-markdown>
            <m-mermaid id="standalone" :text="diagram"></m-mermaid>
            <m-mermaid id="explicit" :text="diagram" :config="{theme:'forest'}"></m-mermaid>
            </div>
            <script src="/libs/vue/vue.global.prod.js"></script>
            <script src="/libs/quasar/quasar.umd.prod.js"></script>
            <script src="/js/MoquiLib.js"></script>
            <script src="/js/WebrootVue.qvt.js"></script>
            <script>
            const diagram = 'erDiagram\\n Person ||--o{ Task : owns';
            const app = Vue.createApp({ data() { return {diagram, markdown: '\u0060\u0060\u0060mermaid\\n' + diagram + '\\n\u0060\u0060\u0060'}; } });
            app.use(Quasar);
            Quasar.Dark.set(true);
            for (const name of ['m-markdown', 'm-mermaid', 'm-diagram-zoom']) app.component(name, moqui.qvt.components[name]);
            app.mount('#app');
            window.diagramState = () => {
                const ids = {}, strokes = {}, contrast = {};
                const luminance = (color) => {
                    const values = color.match(/[\\d.]+/g).slice(0, 3).map(Number).map(value => {
                        value /= 255; return value <= 0.04045 ? value / 12.92 : Math.pow((value + 0.055) / 1.055, 2.4);
                    });
                    return values[0] * 0.2126 + values[1] * 0.7152 + values[2] * 0.0722;
                };
                const background = luminance(Quasar.Dark.isActive ? 'rgb(18,18,18)' : 'rgb(255,255,255)');
                for (const name of ['markdown', 'standalone', 'explicit']) {
                    const svg = document.querySelector('#' + name + ' .m-md-mermaid-drawn svg');
                    const line = svg && svg.querySelector('.relationshipLine');
                    if (!line) return null;
                    ids[name] = svg.id;
                    strokes[name] = getComputedStyle(line).stroke;
                    const foreground = luminance(strokes[name]);
                    contrast[name] = (Math.max(foreground, background) + 0.05) / (Math.min(foreground, background) + 0.05);
                }
                return {ids, strokes, contrast};
            };
            </script></body></html>`)
        assert.equal(page.evaluate(() => Quasar.Dark.isActive), true)
        const dark = page.waitFor(() => window.diagramState())
        assert.ok(dark.contrast.markdown >= 3, JSON.stringify(dark))
        assert.ok(dark.contrast.standalone >= 3, JSON.stringify(dark))
        assert.equal(dark.strokes.markdown, dark.strokes.standalone)
        page.locator("#markdown .m-md-mermaid-zoom").click()
        page.locator(".m-diagram-zoom").waitFor()
        let previous = dark
        for (const mode of [false, true, false]) {
            page.evaluate(mode => { Quasar.Dark.set(mode); return true }, mode)
            const changed = page.waitFor(ids => {
                const state = window.diagramState()
                return state && state.ids.markdown !== ids.markdown && state.ids.standalone !== ids.standalone ? state : null
            }, { arg: previous.ids })
            assert.ok(changed.contrast.markdown >= 3, JSON.stringify(changed))
            assert.ok(changed.contrast.standalone >= 3, JSON.stringify(changed))
            assert.equal(changed.strokes.explicit, dark.strokes.explicit)
            assert.equal(changed.ids.explicit, dark.ids.explicit)
            assert.equal(changed.strokes.markdown, changed.strokes.standalone)
            previous = changed
        }
        page.locator(".m-diagram-zoom").waitFor({ state: "hidden" })
        page.evaluate(async () => { Quasar.Dark.set(true); await Vue.nextTick(); Quasar.Dark.set(false); return true })
        const settled = page.waitFor(ids => {
            const state = window.diagramState()
            return state && state.ids.markdown !== ids.markdown && state.ids.standalone !== ids.standalone ? state : null
        }, { arg: previous.ids })
        assert.deepEqual(settled.strokes, previous.strokes)
    },
})
