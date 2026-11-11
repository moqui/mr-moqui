// The split and the wizard, in the page. Was js/WebrootVue.qvt.layout.test.js.
// WebrootVue.qvt.js starts the Vue app only when the page has #moqui-config plus Vue and Quasar. This page has none of those.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/WebrootVue.qvt.js"],
    test: "/files/runtime/base-component/webroot/test/client/pages/WebrootVue.qvt.layout.page.js",
})

feature("the tray fits its layout when a scrollbar reduces the available width", {
    expect() {
        // A constrained component fixture, not an application phone walk: the outer viewport remains desktop-sized.
        const page = browser.newPage({viewport: {width: 1920, height: 1080}})
        page.setContent(`<!doctype html><html><head><link rel="stylesheet" href="/libs/quasar/quasar.prod.css">
            <link rel="stylesheet" href="/css/WebrootVue.qvt.css"><style>
            .q-layout-container .scroll {overflow-y:scroll!important;} *::-webkit-scrollbar {width:15px;}
            </style></head><body><div id="app">
            <q-layout container style="width:390px;height:320px"><q-page-container><q-page style="min-height:1000px">
                <m-shell-tray></m-shell-tray>
            </q-page></q-page-container></q-layout></div>
            <script src="/libs/vue/vue.global.prod.js"></script><script src="/libs/quasar/quasar.umd.prod.js"></script>
            <script src="/js/MoquiLib.js"></script><script src="/js/WebrootVue.qvt.js"></script><script>
            const app=Vue.createApp({data(){return {trayMounted:true,trayOpen:true,trayTab:'today',trayLoaded:{},trayScroll:{},shellTray:null,
                trayTabs:[{key:'today',label:'Today',icon:'sym_o_today'}],trayFloating:{},labels:{closeTray:'Close tray'}};},
                computed:{trayDockedTabs(){return this.trayTabs;}},
                methods:{countOf(){return 0;},countText(){return '';},openTray(key){this.trayTab=key;this.trayOpen=true;},
                    closeTray(){this.trayOpen=false;},keepTrayScroll(){},trayFloatTarget(){return null;}}});
            app.use(Quasar);app.config.globalProperties.moqui=moqui;
            app.component('m-shell-tray',moqui.qvt.components['m-shell-tray']);window.trayApp=app.mount('#app');
            window.trayGeometry=()=>{const canvas=document.querySelector('.q-page-container').getBoundingClientRect();
                const sticky=document.querySelector('.m-tray-sticky').getBoundingClientRect();
                const card=document.getElementById('m-tray').getBoundingClientRect();return {canvasLeft:canvas.left,canvasRight:canvas.right,
                    canvasWidth:canvas.width,stickyLeft:sticky.left,stickyRight:sticky.right,cardLeft:card.left,cardRight:card.right,cardWidth:card.width};};
            </script></body></html>`)
        page.waitFor(() => window.trayApp && document.getElementById('m-tray') && trayGeometry().canvasWidth === 375 && trayGeometry().cardWidth > 0)
        const first = page.evaluate(() => trayGeometry())
        assert.equal(first.canvasWidth, 375, 'the fixture reserves a 15px scrollbar inside its 390px layout')
        assert.ok(first.stickyLeft >= first.canvasLeft - 1 && first.stickyRight <= first.canvasRight + 1)
        assert.ok(first.cardLeft >= first.canvasLeft - 1 && first.cardRight <= first.canvasRight + 1)
        assert.ok(first.cardWidth <= first.canvasWidth)
        page.evaluate(() => {trayApp.closeTray();document.querySelector('.q-layout-container').style.width='340px';return true;})
        page.evaluate(() => {trayApp.openTray('today');return true;})
        page.waitFor(() => trayGeometry().canvasWidth === 325)
        const resized = page.evaluate(() => trayGeometry())
        assert.ok(resized.stickyLeft >= resized.canvasLeft - 1 && resized.stickyRight <= resized.canvasRight + 1)
        assert.ok(resized.cardLeft >= resized.canvasLeft - 1 && resized.cardRight <= resized.canvasRight + 1)
    },
})

feature("the compact wizard retains named previous steps and prevents forward header navigation", {
    expect() {
        const page = browser.newPage({ viewport: { width: 1920, height: 1080 } })
        page.setContent(`<!doctype html><html><head><link rel="stylesheet" href="/libs/quasar/quasar.prod.css">
            <link rel="stylesheet" href="/css/WebrootVue.qvt.css"></head><body><div id="app"><m-subscreens-wizard ref="wizard"></m-subscreens-wizard></div>
            <script src="/libs/vue/vue.global.prod.js"></script><script src="/libs/quasar/quasar.umd.prod.js"></script>
            <script src="/js/MoquiLib.js"></script><script src="/js/WebrootVue.qvt.js"></script><script>
            window.visits=[]; const app=Vue.createApp({data(){return {activeSubscreens:[],currentParameters:{},navMenuList:[{subscreens:[
                {name:'Customer',title:'Customer',pathWithParams:'/wizard/Customer'},
                {name:'Options',title:'Options',pathWithParams:'/wizard/Options',active:true},
                {name:'Review',title:'Review',pathWithParams:'/wizard/Review'}]}]};},
                methods:{getLinkPath(path){return path;},setUrl(path){visits.push(path);}}});
            app.use(Quasar);Quasar.Screen.setSizes({sm:2000,md:2200,lg:2400,xl:2600});
            app.component('m-subscreens-wizard',moqui.qvt.components['m-subscreens-wizard']);
            app.component('m-subscreens-active',{template:'<div>Step contents</div>'});window.wizardApp=app.mount('#app');</script></body></html>`)
        page.waitFor(() => document.querySelectorAll('.q-stepper__title').length === 3)
        assert.deepEqual(page.evaluate(() => Array.from(document.querySelectorAll('.q-stepper__title')).map(title => ({
            text:title.textContent, visible:title.getBoundingClientRect().width > 0 && title.getBoundingClientRect().height > 0
        }))), [{text:'Customer',visible:true},{text:'Options',visible:true},{text:'Review',visible:true}])
        assert.equal(page.locator('.q-stepper__tab[aria-current="step"]').evaluate(el => el.textContent.includes('Options')), true)
        assert.equal(page.locator('.q-stepper__tab[role="button"]').count(), 1)
        assert.equal(page.locator('.q-stepper__tab[role="button"]').evaluate(el => el.textContent.includes('Customer')), true)
        page.evaluate(() => {
            window.previousHeader=document.querySelector('.q-stepper__tab[role="button"]');
            previousHeader.focus(); wizardApp.$refs.wizard.submitting=true;
            return true;
        })
        page.waitFor(() => document.querySelector('.q-stepper__tab[role="button"]').getAttribute('aria-disabled') === 'true')
        assert.equal(page.locator('.q-stepper__tab[role="button"]').evaluate(el => el.getAttribute('tabindex')), '-1')
        assert.equal(page.evaluate(() => document.activeElement === previousHeader), true, 'disabling the header preserves its DOM and current focus')
        // Dispatch on the focused disabled header; the action helper otherwise waits for the deliberately active loading mask.
        page.evaluate(() => {
            for (const type of ['keydown','keyup']) document.activeElement.dispatchEvent(new KeyboardEvent(type,
                {key:'Enter',code:'Enter',keyCode:13,bubbles:true,cancelable:true}));
            return true;
        })
        assert.deepEqual(page.evaluate(() => visits), [])
        page.evaluate(() => {wizardApp.$refs.wizard.submitting=false; return true;})
        page.waitFor(() => !document.querySelector('.q-stepper__tab[role="button"]').hasAttribute('aria-disabled'))
        assert.equal(page.locator('.q-stepper__tab[role="button"]').evaluate(el => el.getAttribute('tabindex')), '0')
        page.locator('.q-stepper__tab[role="button"]').press('Enter')
        assert.deepEqual(page.evaluate(() => visits), ['/wizard/Customer'])
        page.locator('.q-stepper__tab:last-child').click()
        assert.deepEqual(page.evaluate(() => visits), ['/wizard/Customer'])
    },
})


