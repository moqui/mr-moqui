// Insert a real Mermaid diagram in an editor and inspect the resulting Excalidraw frame and shapes.
spec({server:false})
feature("an inserted Mermaid frame keeps its source and contains the converted diagram", {
    expect() {
        const page = browser.newPage()
        page.setContent(`<!doctype html><html><body><div id="app"><m-whiteboard ref="board"></m-whiteboard></div>
            <script src="/libs/vue/vue.global.prod.js"></script><script src="/libs/quasar/quasar.umd.prod.js"></script>
            <script src="/js/MoquiLib.js"></script><script src="/js/WebrootVue.qvt.js"></script><script>
            const app = Vue.createApp({})
            app.use(Quasar)
            Object.entries(moqui.qvt.components).forEach(([key,component]) => app.component(key,component))
            window.vm = app.mount("#app")
            </script></body></html>`)
        page.waitFor(() => !!vm.$refs.board.api)
        page.evaluate(async () => {
            const b=vm.$refs.board
            b.editMermaid()
            b.updateSource("flowchart LR\n A[Receive] --> B[Review]")
            await b.insertMermaid()
            return true
        })
        assert.equal(page.evaluate(() => vm.$refs.board.error), "")
        page.waitFor(() => vm.$refs.board.api.getSceneElements().some(e => e.type === "frame" && e.customData.mermaid.includes("Receive")))
        assert.equal(page.evaluate(() => vm.$refs.board.api.getSceneElements().some(e => e.type === "arrow" && e.frameId)), true)
    }
})
