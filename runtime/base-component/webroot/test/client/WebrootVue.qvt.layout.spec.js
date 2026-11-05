// The split and the wizard, in the page. Was js/WebrootVue.qvt.layout.test.js.
// WebrootVue.qvt.js starts the Vue app only when the page has #moqui-config plus Vue and Quasar. This page has none of those.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/js/WebrootVue.qvt.js"],
    test: "/files/runtime/base-component/webroot/test/client/pages/WebrootVue.qvt.layout.page.js",
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
