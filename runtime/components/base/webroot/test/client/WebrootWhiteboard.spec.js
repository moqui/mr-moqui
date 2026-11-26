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
            <script src="/libs/yjs/yjs.min.js"></script><script>
            const Y = Yjs.Y, server = new Y.Doc();
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
        assert.equal(page.evaluate(async () => {
            const installed = (await fetch("/libs/fonts-extra/installed.json").then(r => r.json())).includes("excalidraw-cjk")
            await document.fonts.load('16px Xiaolai', '漢字')
            const fetched = performance.getEntriesByType("resource").some(r => r.name.includes("/fonts/Xiaolai/"))
            return fetched === installed
        }), true)
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
        // what another editor may not put on a board: a page in an iframe, generated HTML, an editor setting
        page.evaluate(() => {
            const sv = Y.encodeStateVector(server)
            server.transact(() => {
                const map = server.getMap("elements")
                map.set('["page",1,1]', {id:"page",type:"iframe",x:0,y:0,width:50,height:50,version:1,versionNonce:1,index:"b0"})
                map.set('["html",1,1]', {id:"html",type:"rectangle",x:0,y:0,width:50,height:50,version:1,versionNonce:1,index:"b1",
                    customData:{generationData:{status:"done",html:"<script>parent.hacked=1<\/script>"}}})
                server.getMap("app").set("errorMessage", "Sign in again")
            })
            deliver({type:'update',client:'other',update:c.toBase64(Y.encodeStateAsUpdate(server,sv))})
            return true
        })
        page.waitFor(() => vm.$refs.board.api.getSceneElements()[0].x === 130 && !window.hacked)
        assert.deepEqual(page.evaluate(() => new Promise(done => requestAnimationFrame(() => requestAnimationFrame(() => done({
            ids: vm.$refs.board.api.getSceneElements().map(e => e.id).filter(id => id === "page" || id === "html"),
            iframes: document.querySelectorAll(".excalidraw iframe").length,
            error: vm.$refs.board.api.getAppState().errorMessage || null, hacked: !!window.hacked
        }))))), {ids: [], iframes: 0, error: null, hacked: false})
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
        assert.equal(page.evaluate(async () => {
            const converter = await moqui.whiteboard.converter()
            const scene = await converter.parseMermaidToExcalidraw("flowchart LR\n A --> B", {securityLevel:"strict"})
            return scene.elements.some(element => element.type === "arrow")
        }), true)
        page.getByRole("button", {name:"hemp:story/1", exact:true}).click()
        assert.equal(page.evaluate(() => vm.link), "hemp:story/1")
        assert.equal(page.evaluate(() => JSON.parse(saved.scene).elements[0].link), "hemp:story/1")
        assert.equal(page.evaluate(() => saved.preview.includes("<svg") && !window.Yjs), true)
        assert.equal(page.evaluate(() => performance.getEntriesByType("resource").every(r =>
            !/^https?:/.test(r.name) || new URL(r.name).origin === location.origin)), true)
    }
})

// A board's presence takes of a peer's awareness only its pointer (finite numbers, a tool), its button and its selected
// ids, with the name the server gave it: nothing it sends sets the name, the color or an avatar's URL.
feature("a peer's presence on a board is its pointer, button and selection only, named by the server", {
    expect() {
        const page = browser.newPage()
        page.setContent(`<!doctype html><html><body><script src="/js/MoquiLib.js"></script><script src="/js/WebrootVue.qvt.js"></script>
            <script src="/js/WebrootWhiteboard.js"></script></body></html>`)
        page.waitFor(() => !!(window.moqui && moqui.whiteboard && moqui.whiteboard.component))
        const shown = page.evaluate(() => {
            const scenes = []
            const evil = JSON.parse('{"user":{"color":"red;x","name":"Evil"},"cursor":{"pointer":{"x":1,"y":2,"tool":"laser"},"button":"down",' +
                '"username":"Evil","avatarUrl":"https://evil.example/a.png","color":{"stroke":"url(x)"}},"selection":{"a":true,"b":"x","__proto__":{"c":true}}}')
            const fake = { api: { updateScene: (data) => scenes.push(data) }, closed: false,
                awareness: { clientID: 1, getStates: () => new Map([[2, evil], [3, {user: {color: "#123456"}, cursor: {pointer: {x: "1", y: 2}}}]]) },
                provider: { peers: [{ awareness: 2, name: "Ann" }, { awareness: 3, name: "Bob" }] } }
            moqui.whiteboard.component.methods.presence.call(fake)
            const of = (id) => scenes[0].collaborators.get(id)
            return JSON.stringify({ ann: of("2"), bob: of("3") })
        })
        const {ann, bob} = JSON.parse(shown)
        assert.deepEqual(ann, { username: "Ann", color: { stroke: "#4267aa", background: "#4267aa" }, selectedElementIds: { a: true },
            pointer: { x: 1, y: 2, tool: "laser" }, button: "down" })
        assert.deepEqual(bob, { username: "Bob", color: { stroke: "#123456", background: "#123456" }, selectedElementIds: {}, button: "up" })
    }
})
