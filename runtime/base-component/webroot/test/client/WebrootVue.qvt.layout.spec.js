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
            <link rel="stylesheet" href="/css/WebrootVue.qvt.css"></head><body><div id="app"><m-subscreens-wizard></m-subscreens-wizard></div>
            <script src="/libs/vue/vue.global.prod.js"></script><script src="/libs/quasar/quasar.umd.prod.js"></script>
            <script src="/js/MoquiLib.js"></script><script src="/js/WebrootVue.qvt.js"></script><script>
            window.visits=[]; const app=Vue.createApp({data(){return {activeSubscreens:[],currentParameters:{},navMenuList:[{subscreens:[
                {name:'Customer',title:'Customer',pathWithParams:'/wizard/Customer'},
                {name:'Options',title:'Options',pathWithParams:'/wizard/Options',active:true},
                {name:'Review',title:'Review',pathWithParams:'/wizard/Review'}]}]};},
                methods:{getLinkPath(path){return path;},setUrl(path){visits.push(path);}}});
            app.use(Quasar);Quasar.Screen.setSizes({sm:2000,md:2200,lg:2400,xl:2600});
            app.component('m-subscreens-wizard',moqui.qvt.components['m-subscreens-wizard']);
            app.component('m-subscreens-active',{template:'<div>Step contents</div>'});app.mount('#app');</script></body></html>`)
        page.waitFor(() => document.querySelectorAll('.q-stepper__title').length === 3)
        assert.deepEqual(page.evaluate(() => Array.from(document.querySelectorAll('.q-stepper__title')).map(title => ({
            text:title.textContent, visible:title.getBoundingClientRect().width > 0 && title.getBoundingClientRect().height > 0
        }))), [{text:'Customer',visible:true},{text:'Options',visible:true},{text:'Review',visible:true}])
        assert.equal(page.locator('.q-stepper__tab[aria-current="step"]').evaluate(el => el.textContent.includes('Options')), true)
        assert.equal(page.locator('.q-stepper__tab[role="button"]').count(), 1)
        assert.equal(page.locator('.q-stepper__tab[role="button"]').evaluate(el => el.textContent.includes('Customer')), true)
        page.locator('.q-stepper__tab[role="button"]').press('Enter')
        assert.deepEqual(page.evaluate(() => visits), ['/wizard/Customer'])
        page.locator('.q-stepper__tab:last-child').click()
        assert.deepEqual(page.evaluate(() => visits), ['/wizard/Customer'])
    },
})
