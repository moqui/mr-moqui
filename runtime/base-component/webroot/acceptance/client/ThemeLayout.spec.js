// Measured theme and layout checks of the shared qvt components, in the page with Vue and Quasar: semantic text and
// focus contrast in both default themes, Mermaid redraws on a color-mode change, and box headers at phone width.
// Acceptance (./moq accept webroot), not regression: they measure rendered colors and geometry rather than check a rule.
// Moved from test/client/WebrootVue.qvt.spec.js, WebrootVue.qvt.markdown.spec.js and WebrootVue.qvt.layout.spec.js.
spec({ server: false, timeout: 60 })

feature("field feedback and selected navigation retain readable text in both themes", {
    expect() {
        const page = browser.newPage({ viewport: { width: 1920, height: 1080 } })
        page.allowProblems(/notification: warning negative feedback Supporting detail OPEN/)
        page.setContent(`<!doctype html><html><head><link rel="stylesheet" href="/libs/quasar/quasar.prod.css">
            <link rel="stylesheet" href="/css/WebrootVue.qvt.css">
            <!-- Theme-transition timing is inspected live; this rule measures the settled colors. -->
            <style>.m-drawer > .q-item, .m-drawer .q-focus-helper, .m-drawer .q-focus-helper::before, .m-drawer .q-focus-helper::after { transition:none; }</style></head><body><div id="app">
            <q-card><m-form ref="form" action="/unused" :fields-initial="{name:'Original', explicit:'Original'}" v-slot="form">
                <m-text-line id="feedback-field" name="name" label="Customer name" v-model="form.fields.name"></m-text-line>
                <m-text-line id="explicit-field" name="explicit" label="Explicit fill" bg-color="blue-10" v-model="form.fields.explicit"></m-text-line>
            </m-form></q-card>
            <div class="q-layout"><aside class="q-drawer" :class="{'q-dark':$q.dark.isActive}" style="position:static;width:240px"><nav class="m-drawer">
                <span id="drawer-start" tabindex="-1"></span>
                <q-item id="drawer-app" clickable tag="a" class="m-drawer-app m-drawer-in-app m-drawer-on-path" href="#">Comms</q-item>
                <q-item id="drawer-current" clickable tag="a" class="m-drawer-screen m-drawer-in-app m-drawer-current" href="#" aria-current="page">Accounts</q-item>
            </nav></aside></div></div>
            <script src="/libs/vue/vue.global.prod.js"></script><script src="/libs/quasar/quasar.umd.prod.js"></script>
            <script src="/js/MoquiLib.js"></script><script src="/js/WebrootVue.qvt.js"></script><script>
            const app = Vue.createApp({}); app.use(Quasar, {config:{brand:{info:'#1e7b8e'}}});
            ['m-form','m-text-line'].forEach(name=>app.component(name,moqui.qvt.components[name]));
            window.vm=app.mount('#app');
            window.measureContrast=(element,background,color)=>{
                const ctx=document.createElement('canvas').getContext('2d'), style=getComputedStyle(element);
                const luminance=()=>Array.from(ctx.getImageData(0,0,1,1).data).slice(0,3).map(v=>{
                    const c=v/255;return c<=.04045?c/12.92:Math.pow((c+.055)/1.055,2.4);
                }).reduce((sum,v,i)=>sum+v*[.2126,.7152,.0722][i],0);
                ctx.fillStyle=background;ctx.fillRect(0,0,1,1);const bg=luminance();
                ctx.globalAlpha=Number(style.opacity);ctx.fillStyle=color || style.color;ctx.fillRect(0,0,1,1);const fg=luminance();
                return (Math.max(bg,fg)+.05)/(Math.min(bg,fg)+.05);
            };
            window.drawerBackground=(row,focused)=>{
                const ctx=document.createElement('canvas').getContext('2d'), ancestors=[];
                for (let node=row;node;node=node.parentElement) ancestors.unshift(node);
                ctx.fillStyle='#fff';ctx.fillRect(0,0,1,1);
                for (const node of ancestors) {ctx.fillStyle=getComputedStyle(node).backgroundColor;ctx.fillRect(0,0,1,1);}
                const helper=row.querySelector('.q-focus-helper');
                if (focused && helper) {
                    // Its pseudo-elements paint inside the helper before its group opacity composites over the row.
                    const layer=document.createElement('canvas'), fill=layer.getContext('2d');
                    for (const pseudo of [null,'::before','::after']) {
                        const style=getComputedStyle(helper,pseudo);
                        fill.globalAlpha=pseudo ? Number(style.opacity) : 1;
                        fill.fillStyle=style.backgroundColor;fill.fillRect(0,0,1,1);
                    }
                    ctx.globalAlpha=Number(getComputedStyle(helper).opacity);ctx.drawImage(layer,0,0);
                }
                return 'rgb('+Array.from(ctx.getImageData(0,0,1,1).data).slice(0,3).join(',')+')';
            };
            window.actions=0;
            ['positive','negative','warning','info'].forEach(type=>Quasar.Notify.create({type,message:type+' feedback',caption:'Supporting detail',
                attrs:{'data-feedback':type},actions:[{label:'Open',handler:()=>actions++}],timeout:0,group:false}));
            Quasar.Notify.create({type:'positive',color:'purple',textColor:'white',message:'Custom feedback',attrs:{'data-feedback':'custom'},timeout:0,group:false});
            </script></body></html>`)
        page.waitFor(() => window.vm && document.querySelectorAll("[data-feedback]").length === 5)
        page.evaluate(async () => {
            vm.$refs.form.fields.name = "Changed"; vm.$refs.form.fields.explicit = "Changed"; await Vue.nextTick()
            vm.$refs.form.setFieldErrors([{ field: "name", message: "Enter the customer name" }]); await Vue.nextTick()
            return true
        })
        for (const dark of [false, true, false]) {
            const results = page.evaluate(async mode => {
                Quasar.Dark.set(mode); await Vue.nextTick()
                const input = document.getElementById('feedback-field'), field = input.closest('.q-field'), control = field.querySelector('.q-field__control')
                const panel = getComputedStyle(document.querySelector('.q-card')).backgroundColor
                const results = []
                for (const focus of [false,true]) {
                    if (focus) input.focus(); else input.blur()
                    await Vue.nextTick()
                    results.push({role:'error label '+focus,ratio:measureContrast(field.querySelector('.q-field__label'),getComputedStyle(control).backgroundColor)})
                    results.push({role:'error helper '+focus,ratio:measureContrast(field.querySelector('.q-field__bottom'),panel)})
                }
                for (const notification of document.querySelectorAll('[data-feedback]:not([data-feedback="custom"])')) {
                    const background = getComputedStyle(notification).backgroundColor
                    for (const selector of ['.q-notification__message','.q-notification__caption','button']) {
                        results.push({role:notification.dataset.feedback+' '+selector,ratio:measureContrast(notification.querySelector(selector),background)})
                    }
                }
                for (const row of document.querySelectorAll('.m-drawer > .q-item')) {
                    // color-mix tints are translucent: measure against the painted ancestor stack, not transparent black.
                    const ctx=document.createElement('canvas').getContext('2d'), ancestors=[];
                    for (let node=row;node;node=node.parentElement) ancestors.unshift(node);
                    ctx.fillStyle='#fff';ctx.fillRect(0,0,1,1);
                    for (const node of ancestors) {ctx.fillStyle=getComputedStyle(node).backgroundColor;ctx.fillRect(0,0,1,1);}
                    const rgb=Array.from(ctx.getImageData(0,0,1,1).data).slice(0,3);
                    results.push({role:row.id,ratio:measureContrast(row,'rgb('+rgb.join(',')+')'),
                        foreground:getComputedStyle(row).color,background:rgb,
                        surfaces:ancestors.map(node=>[node.tagName,node.className,getComputedStyle(node).backgroundColor])});
                }
                return {results,explicit:getComputedStyle(document.getElementById('explicit-field').closest('.q-field').querySelector('.q-field__control')).backgroundColor,
                    custom:getComputedStyle(document.querySelector('[data-feedback="custom"]')).backgroundColor,
                    selectedIndicator:getComputedStyle(document.getElementById('drawer-current')).boxShadow}
            }, dark)
            for (const result of results.results) assert.ok(result.ratio >= 4.5, JSON.stringify({dark,...result}))
            assert.equal(results.explicit, "rgb(13, 71, 161)", "the authored field background survives")
            assert.equal(results.custom, "rgb(156, 39, 176)", "the authored notification color survives")
            assert.ok(results.selectedIndicator.includes("rgb(25, 118, 210)"), "the selection bar keeps its brand accent")
            page.evaluate(() => { document.getElementById('drawer-start').focus(); return true })
            for (const id of ['drawer-app','drawer-current']) {
                page.keyboard.press("Tab")
                const ring=page.evaluate(() => {
                    const row=document.activeElement, style=getComputedStyle(row);
                    return {id:row.id,visible:row.matches(':focus-visible'),width:style.outlineWidth,style:style.outlineStyle,
                        offset:style.outlineOffset,ratio:measureContrast(row,drawerBackground(row,true),style.outlineColor),
                        selectedIndicator:getComputedStyle(document.getElementById('drawer-current')).boxShadow};
                })
                assert.equal(ring.id,id)
                assert.equal(ring.visible,true)
                assert.equal(ring.width,"2px")
                assert.equal(ring.style,"solid")
                assert.equal(ring.offset,"-5px")
                assert.ok(ring.ratio>=3,JSON.stringify({dark,...ring}))
                assert.ok(ring.selectedIndicator.includes("rgb(25, 118, 210)"),"keyboard focus preserves the selection bar")
            }
            page.evaluate(() => { document.activeElement.blur(); return true })
            assert.equal(page.evaluate(() => getComputedStyle(document.getElementById('drawer-current')).outlineStyle),"none")
        }
        assert.equal(page.evaluate(() => {
            const selected=document.getElementById('drawer-current');selected.style.setProperty('--m-drawer-current-fg','#123456');
            return getComputedStyle(selected).color;
        }), "rgb(18, 52, 86)", "an authored selected-text override remains supported")
        page.locator('[data-feedback="positive"] button').click()
        assert.equal(page.evaluate(() => actions), 1)
    },
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

feature("box headers retain their title beside wrapping phone actions and name the keyboard toggle", {
    expect() {
        const page = browser.newPage({ viewport: { width: 1920, height: 1080 } })
        page.setContent(`<!doctype html><html><head><link rel="stylesheet" href="/libs/quasar/quasar.prod.css">
            <link rel="stylesheet" href="/libs/material-icons/material-icons.css">
            <link rel="stylesheet" href="/css/WebrootVue.qvt.css"></head><body><div id="app">
            <m-container-box title="Model default"><template #toolbar>
                <q-btn label="Test"></q-btn><q-btn label="Probe Capabilities"></q-btn>
                <q-btn label="Calls"></q-btn><q-btn label="All Models"></q-btn>
            </template><span id="box-fixture-body">Model configuration</span></m-container-box></div>
            <script src="/libs/vue/vue.global.prod.js"></script><script src="/libs/quasar/quasar.umd.prod.js"></script>
            <script src="/js/MoquiLib.js"></script><script src="/js/WebrootVue.qvt.js"></script><script>
            const app=Vue.createApp({});app.use(Quasar);
            app.component('m-container-box',moqui.qvt.components['m-container-box']);window.vm=app.mount('#app');
            </script></body></html>`)
        page.waitFor(() => !!window.vm && !!document.querySelector('.m-box-title') && document.fonts.check('24px "Material Icons"'))
        const desktop = page.evaluate(() => {
            const title=document.querySelector('.m-box-title').getBoundingClientRect();
            const toolbar=document.querySelector('.m-box-toolbar').getBoundingClientRect();
            return {titleWidth:title.width, sameRow:title.top < toolbar.bottom && toolbar.top < title.bottom};
        })
        assert(desktop.titleWidth > 100)
        assert.equal(desktop.sameRow, true, 'desktop actions stay alongside the title')
        page.setViewport(390, 844)
        page.waitFor(() => document.documentElement.clientWidth === 390)
        const phone = page.evaluate(() => {
            const header=document.querySelector('.m-box-head'), title=document.querySelector('.m-box-title');
            const toolbar=document.querySelector('.m-box-toolbar');
            const titleRect=title.getBoundingClientRect(), toolbarRect=toolbar.getBoundingClientRect();
            return {titleWidth:titleRect.width, titleText:title.textContent, actionsBelow:toolbarRect.top >= titleRect.bottom,
                fits:header.scrollWidth <= header.clientWidth && document.documentElement.scrollWidth <= 390,
                actionsVisible:Array.from(toolbar.querySelectorAll('button')).every(button=>{
                    const rect=button.getBoundingClientRect();return rect.width>0 && rect.left>=0 && rect.right<=390;
                })};
        })
        assert(phone.titleWidth > 100, 'phone actions must not shrink the title to zero')
        assert.equal(phone.titleText, 'Model default')
        assert.equal(phone.actionsBelow, true)
        assert.equal(phone.fits, true)
        assert.equal(phone.actionsVisible, true)
        const toggle = page.locator('.m-box-toggle')
        assert.equal(toggle.evaluate(el=>el.getAttribute('aria-label')), 'Collapse Model default')
        assert.equal(toggle.evaluate(el=>el.getAttribute('aria-expanded')), 'true')
        toggle.press('Enter')
        page.waitFor(() => getComputedStyle(document.querySelector('.m-box-content')).display === 'none')
        assert.equal(toggle.evaluate(el=>el.getAttribute('aria-label')), 'Expand Model default')
        assert.equal(toggle.evaluate(el=>el.getAttribute('aria-expanded')), 'false')
        assert.equal(toggle.evaluate(el=>el===document.activeElement), true)
        toggle.press('Enter')
        page.waitFor(() => getComputedStyle(document.querySelector('.m-box-content')).display !== 'none')
        assert.equal(toggle.evaluate(el=>el.getAttribute('aria-expanded')), 'true')
    },
})
