/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// The browser binding must reconcile the same shape by Excalidraw revision, independent of Yjs client id.
spec({server: false})
feature("a mounted board merges local and remote edits and reconciles compacted deletions", {
    expect() {
        const page = browser.newPage()
        page.setContent(`<!doctype html><html><head><link rel="stylesheet" href="/libs/quasar/quasar.prod.css"></head>
            <body><div id="app"><m-whiteboard ref="board" wiki-page-id="BOARD" :socket-client="client"></m-whiteboard></div>
            <script src="/libs/vue/vue.global.prod.js"></script><script src="/libs/quasar/quasar.umd.prod.js"></script>
            <script src="/js/MoquiLib.js"></script><script src="/js/WebrootVue.qvt.js"></script>
            <script src="/libs/tiptap/tiptap.min.js"></script><script>
            const Y = Tiptap.Y, server = new Y.Doc();
            const c = moqui.coedit;
            let deliver;
            const client = Vue.markRaw({
                registerListener(topic, callback) { deliver = callback; return () => {}; },
                isOpen: () => true, onState: () => () => {},
                send(m) {
                    if (m.collab === 'join') deliver({type:'joined',client:m.client,canEdit:true,
                        sv:c.toBase64(Y.encodeStateVector(server)),update:c.toBase64(Y.encodeStateAsUpdate(server))});
                    if (m.collab === 'update') {
                        Y.applyUpdate(server,c.fromBase64(m.update));
                        deliver({type:'ack',client:m.client,id:m.id});
                    }
                }
            });
            window.synced = () => moqui.whiteboard.read(server);
            window.remoteEdit = () => {
                const target = synced()[0];
                const sv = Y.encodeStateVector(server);
                const e = {...target,version:target.version + 1,versionNonce:73,x:130};
                moqui.whiteboard.write(server,[e],{});
                deliver({type:'update',client:'other',update:c.toBase64(Y.encodeStateAsUpdate(server,sv))});
            };
            const app=Vue.createApp({data:()=>({client})});
            app.use(Quasar);Object.entries(moqui.qvt.components).forEach(([k,v])=>app.component(k,v));
            window.vm=app.mount('#app');
            </script></body></html>`)
        page.waitFor(() => !!(vm.$refs.board && vm.$refs.board.api && document.querySelector(".excalidraw canvas")))
        assert.deepEqual(page.evaluate(() => ["es-MX", "zh", "zh_TW", "unknown"].map(moqui.whiteboard.language)),
            ["es-ES", "zh-CN", "zh-TW", "en"])
        page.evaluate(() => {
            const e = MoquiBoardEditor.convertToExcalidrawElements([{type:"rectangle",x:20,y:60,width:100,height:65}])
            vm.$refs.board.api.updateScene({elements:e})
            return true
        })
        page.waitFor(() => synced().length === 1)
        page.evaluate(() => {remoteEdit(); return true})
        page.waitFor(() => vm.$refs.board.api.getSceneElements()[0].x === 130)
        const deleted = page.evaluate(() => {
            const known = synced()[0]
            const tombstone = {id: known.id, isDeleted: true, version: known.version + 1, versionNonce: 19}
            const local = new Map([[known.id, known]])
            const shown = moqui.whiteboard.shown([tombstone, {...tombstone, id: "unknown"}], local)
            return {count: shown.length, deleted: shown[0].isDeleted, preservesShape: shown[0].x === known.x,
                revision: shown[0].version === tombstone.version && shown[0].versionNonce === 19}
        })
        assert.deepEqual(deleted, {count: 1, deleted: true, preservesShape: true, revision: true})
    }
})

feature("a file board saves a readable scene and hands a declared element link back to its screen", {
    expect() {
        const page=browser.newPage()
        page.setContent(`<!doctype html><html><body><div id="app">
            <m-whiteboard ref="board" :skeleton="shapes" :link-schemes="['hemp']" @open-link="link=$event" save-url="/save"></m-whiteboard></div>
            <script src="/libs/vue/vue.global.prod.js"></script><script src="/libs/quasar/quasar.umd.prod.js"></script>
            <script src="/js/MoquiLib.js"></script><script src="/js/WebrootVue.qvt.js"></script><script>
            const req=window.fetch;
            window.fetch=(u,o)=>{if(String(u).endsWith('/save')){window.saved=JSON.parse(o.body);return Promise.resolve(new Response('{}'))}
                return req(u,o)};
            const app=Vue.createApp({data:()=>({link:'',shapes:[{type:'rectangle',x:25,y:30,width:150,height:70,link:'hemp:story/1',label:{text:'Story'}}]})});
            app.use(Quasar);Object.entries(moqui.qvt.components).forEach(([k,v])=>app.component(k,v));window.vm=app.mount('#app');
            </script></body></html>`)
        page.waitFor(() => !!vm.$refs.board?.api?.getSceneElements().length)
        page.evaluate(async () => {
            const editor=vm.$refs.board
            const e=editor.api.getSceneElements()[0]
            editor.api.updateScene({appState:{selectedElementIds:{[e.id]:true}}})
            await editor.save("Sketch")
            return true
        })
        page.getByRole("button", {name:"hemp:story/1", exact:true}).click()
        assert.equal(page.evaluate(() => vm.link), "hemp:story/1")
        assert.equal(page.evaluate(() => JSON.parse(saved.scene).elements[0].link), "hemp:story/1")
        assert.equal(page.evaluate(() => saved.preview.includes("<svg") && !window.Tiptap), true)
        assert.equal(page.evaluate(() => performance.getEntriesByType("resource").every(r =>
            !/^https?:/.test(r.name) || new URL(r.name).origin === location.origin)), true)
    }
})
