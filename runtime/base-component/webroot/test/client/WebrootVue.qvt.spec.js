// The qvt components, in the page. Was js/WebrootVue.qvt.test.js.
// WebrootVue.qvt.js starts the Vue app only when the page has #moqui-config plus Vue and Quasar. This page has none of those.
spec({ server: false, timeout: 60 })

feature("Material confirmations cancel cleanly and approve the original validated action once", {
    expect() {
        const page = browser.newPage()
        page.setContent(`<!doctype html><html><head><link rel="stylesheet" href="/libs/quasar/quasar.prod.css"></head><body><div id="app">
            <m-form action="/confirmed-save" :fields-initial="{customerName:''}" v-slot="form">
                <m-text-line id="confirm-name" name="customerName" label="Customer name" required v-model="form.fields.customerName"></m-text-line>
                <q-btn id="confirmed-submit" type="submit" name="CreateRecord" value="Y" label="Create" data-confirm="Create &lt;b&gt;record&lt;/b&gt;?"></q-btn>
            </m-form>
            </div><script src="/libs/vue/vue.global.prod.js"></script><script src="/libs/quasar/quasar.umd.prod.js"></script>
            <script src="/js/MoquiLib.js"></script><script src="/js/WebrootVue.qvt.js"></script><script>
            // Animation timing belongs to acceptance; the real dialog, focus stack and event handlers stay in this rule check.
            Quasar.QDialog.props.transitionDuration.default=0;
            window.requests=[];window.nativeConfirmCalls=0;
            window.confirm=()=>{nativeConfirmCalls++;throw new Error('native confirmation must not run');};
            const realFetch=window.fetch;window.fetch=async(url,init)=>{
                if(String(url)==='/confirmed-save'){requests.push(Object.fromEntries(init.body.entries()));return new Response(JSON.stringify({messageInfos:['Saved']}),{status:200});}
                return realFetch(url,init);
            };
            const app=Vue.createApp({data(){return {appRootPath:'',moquiSessionToken:'fixture-token',loading:0};},
                methods:{getCsrfToken(){},addNotify(){}}});
            app.use(Quasar,{plugins:moqui.qvt.quasarPlugins(Quasar)});app.config.globalProperties.moqui=moqui;
            ['m-form','m-text-line'].forEach(name=>app.component(name,moqui.qvt.components[name]));
            window.vm=moqui.webrootVue=app.mount('#app');document.addEventListener('click',moqui.qvt.onConfirmClick,true);
            </script></body></html>`)
        page.waitFor(() => window.vm && document.querySelector('#confirmed-submit'))
        const approve = () => {
            page.locator('.q-dialog-plugin button:last-child').click()
            page.waitFor(() => !document.querySelector('.q-dialog-plugin'))
        }
        page.locator('#confirmed-submit').press('Enter')
        page.waitFor(() => document.activeElement.closest('.q-dialog-plugin') && document.activeElement.textContent.includes('Cancel'))
        assert.equal(page.locator('.q-dialog-plugin .q-dialog__message').evaluate(el => el.textContent), 'Create <b>record</b>?')
        assert.equal(page.locator('.q-dialog-plugin .q-dialog__message b').count(), 0)
        page.evaluate(() => {document.getElementById('confirmed-submit').click();return true})
        assert.equal(page.locator('.q-dialog-plugin').count(), 1, 'repeated clicks do not stack confirmations')
        page.keyboard.press('Escape')
        page.waitFor(() => !document.querySelector('.q-dialog-plugin'))
        assert.equal(page.evaluate(() => document.activeElement.id), 'confirmed-submit')
        assert.equal(page.evaluate(() => requests.length), 0)
        page.locator('#confirmed-submit').press('Enter')
        approve()
        page.waitFor(() => document.querySelector('#confirm-name').getAttribute('aria-invalid') === 'true')
        assert.equal(page.evaluate(() => requests.length), 0, 'approval does not bypass field validation')
        page.locator('#confirm-name').fill('Customer')
        page.locator('#confirmed-submit').press('Enter')
        approve()
        page.waitFor(() => requests.length === 1)
        assert.deepEqual(page.evaluate(() => requests), [{customerName:'Customer',moquiSessionToken:'fixture-token',CreateRecord:'Y'}])

        assert.equal(page.evaluate(() => nativeConfirmCalls), 0)
    },
})

feature("quick-create success links wait for modal hide and refusals keep the draft open", {
    expect() {
        const page = browser.newPage()
        page.setContent(`<!doctype html><html><head><link rel="stylesheet" href="/libs/quasar/quasar.prod.css"></head><body><div id="app">
            <m-form ref="noteForm" action="/quick-note" :fields-initial="{noteText:''}" @submitted="submitted++" v-slot="form">
                <m-text-line id="note-body" name="noteText" label="Note" v-model="form.fields.noteText"></m-text-line>
                <q-btn id="save-note" type="submit" label="Save"></q-btn>
            </m-form></div><script src="/libs/vue/vue.global.prod.js"></script><script src="/libs/quasar/quasar.umd.prod.js"></script>
            <script src="/js/MoquiLib.js"></script><script src="/js/WebrootVue.qvt.js"></script><script>
            window.saves=[];window.routes=[];window.answered=0;
            const realFetch=window.fetch;window.fetch=async(url,init)=>{
                if(String(url)==='/quick-note'){saves.push(Object.fromEntries(init.body.entries()));return new Response(JSON.stringify({
                    messageInfos:[{message:'Note saved',type:'success',link:'/qapps/Commons/Notes?wikiPageId=review-note'}]
                }),{status:200});}return realFetch(url,init);
            };
            const closeMethods=moqui.qvt.components['m-shell-quick-create'].methods;
            const app=Vue.createApp({data(){return {appRootPath:'',moquiSessionToken:'fixture-token',loading:0,quickCreateShow:true,submitted:0};},
                provide(){return {mSubmitHide:{hide:()=>closeMethods.hide.call(window.closeFrame)}};},
                methods:{getCsrfToken(){},addNotify(){},setUrl(url){routes.push(url);}}});
            app.use(Quasar,{plugins:moqui.qvt.quasarPlugins(Quasar)});app.config.globalProperties.moqui=moqui;
            ['m-form','m-text-line'].forEach(name=>app.component(name,moqui.qvt.components[name]));
            window.vm=moqui.webrootVue=app.mount('#app');
            window.closeFrame={$root:vm,isShown:true,hideResolvers:[]};
            window.finishHide=()=>closeMethods.onHide.call(closeFrame);
            moqui.onFormAnswer(()=>answered++);
            </script></body></html>`)
        page.waitFor(() => window.vm && document.querySelector('#save-note'))
        page.locator('#note-body').fill('Submitted note')
        page.locator('#save-note').click()
        page.waitFor(() => saves.length === 1 && vm.submitted === 1 && !vm.quickCreateShow)
        assert.equal(page.locator('.q-notification').count(), 0, 'Open is not offered while the modal still owns focus and hit targets')
        assert.deepEqual(page.evaluate(() => saves), [{noteText:'Submitted note',moquiSessionToken:'fixture-token'}])
        assert.equal(page.evaluate(() => answered), 1)
        page.locator('#note-body').fill('Further draft edits')
        page.evaluate(() => {finishHide();return true})
        page.waitFor(() => document.querySelector('.q-notification button'))
        assert.equal(page.evaluate(() => vm.$refs.noteForm.fields.noteText), 'Further draft edits')
        page.locator('.q-notification button').click()
        page.waitFor(() => routes.length === 1)
        assert.deepEqual(page.evaluate(() => routes), ['/qapps/Commons/Notes?wikiPageId=review-note'])
        page.evaluate(() => {finishHide();return true})
        assert.equal(page.evaluate(() => vm.submitted), 1)
        assert.equal(page.evaluate(() => saves.length), 1)
        page.evaluate(() => {
            vm.quickCreateShow=true;closeFrame.isShown=true;vm.$refs.noteForm.answerTold=false;
            vm.$refs.noteForm.handleResponse({errors:['Save refused']});return true;
        })
        page.waitFor(() => Array.from(document.querySelectorAll('.q-notification')).some(node => node.textContent.includes('Save refused')))
        assert.equal(page.evaluate(() => vm.quickCreateShow), true)
        assert.equal(page.evaluate(() => closeFrame.hideResolvers.length), 0)
        assert.equal(page.evaluate(() => vm.$refs.noteForm.fields.noteText), 'Further draft edits')
        assert.equal(page.evaluate(() => vm.submitted), 1)
        assert.equal(page.evaluate(() => saves.length), 1)
        // A dialog closed before it finished showing emits no on-hide: its messages are not left waiting.
        assert.equal(page.evaluate(async () => {
            vm.quickCreateShow=true;closeFrame.isShown=false;
            await closeMethods.hide.call(closeFrame);return closeFrame.hideResolvers.length === 0 && !vm.quickCreateShow
        }), true)
    },
})

feature("find and compact checkbox controls name their field and preserve keyboard values", {
    expect() {
        const page = browser.newPage()
        page.setContent(`<!doctype html><html><head><link rel="stylesheet" href="/libs/quasar/quasar.prod.css"><link rel="stylesheet" href="/css/WebrootVue.qvt.css"></head><body><div id="app">
            <m-text-find id="entity-find" name="entity" label="Entity" :fields="fields"></m-text-find>
            <m-text-find id="user-find" name="user" label="User" :fields="fields"></m-text-find>
            <m-date-period id="date-find" name="changedDate" label="Date" :fields="fields"></m-date-period>
            <m-date-period id="created-find" name="createdDate" label="Created" :fields="fields"></m-date-period>
            <form id="compact-checks">
                <m-check v-model="compact" name="notify" :options="[{value:'Y',label:' '},{value:'N',label:'Keep visible'}]" tooltip="Email notifications for Mentions"></m-check>
                <m-check v-model="direct" name="digest" :options="[{value:'Y',label:'  '}]" tooltip="Daily digest for Mentions" no-wrapper></m-check>
            </form>
            <m-form id="saved-checks" action="/check-save" :fields-initial="{sharing:[],enabled:['Y'],tags:['A','B']}" no-validate v-slot:default="formProps">
                <m-check v-model="formProps.fields.sharing" name="sharing" :options="[{value:'Y',label:'Share repositories'}]"></m-check>
                <m-check v-model="formProps.fields.enabled" name="enabled" :options="[{value:'Y',label:'Enable sign in'}]"></m-check>
                <m-check v-model="formProps.fields.tags" name="tags" :options="[{value:'A',label:'Alpha'},{value:'B',label:'Beta'}]"></m-check>
                <q-btn id="save-checks" type="submit" label="Save checks"></q-btn>
                <q-btn id="save-checks-again" type="submit" label="Save checks again"></q-btn>
            </m-form>
            </div><script src="/libs/vue/vue.global.prod.js"></script><script src="/libs/quasar/quasar.umd.prod.js"></script>
            <script src="/js/MoquiLib.js"></script><script src="/js/MoquiL10n.js"></script><script src="/js/WebrootVue.qvt.js"></script><script>
            Quasar.QMenu.props.transitionDuration.default=0;
            moqui.l10n.loadCatalog('en-US',()=>({messages:{'Find options for {field}':'Options for {field}',
                'Clear {field} filter':'Clear filter: {field}'}})).then(()=>{
                const app=Vue.createApp({data(){return {compact:[],direct:[],fields:{entity:'Order',user:'john.doe',changedDate_period:'day',
                    changedDate_poffset:'0',changedDate_pdate:'2026-11-04',createdDate_pdate:'2026-11-01'},loading:0,appRootPath:'',moquiSessionToken:'fixture'};},methods:{getCsrfToken(){}}});
                const fetchPage=window.fetch;window.checkSaves=[];
                window.fetch=(url,init)=>url==='/check-save'?(checkSaves.push(Array.from(init.body.entries())),Promise.resolve(new Response('{}'))):fetchPage(url,init);
                app.use(Quasar);['m-text-find','m-text-line','m-date-period','m-date-time','m-check','m-field','m-form'].forEach(name=>app.component(name,moqui.qvt.components[name]));
                window.vm=app.mount('#app');
            });
            </script></body></html>`)
        page.waitFor(() => window.vm && document.querySelector('[aria-label="Options for Entity"]'))
        assert.equal(page.locator('button[aria-label="Options for User"]').count(), 1)
        page.locator('button[aria-label="Options for Entity"]').press('Enter')
        page.waitFor(() => document.querySelector('.m-text-find-options'))
        assert.equal(page.locator('.m-text-find-options').evaluate(el => el.textContent.includes('Operator')), true)
        page.keyboard.press('Escape')
        page.waitFor(() => !document.querySelector('.m-text-find-options'))
        page.locator('button[aria-label="Choose a period for Date"]').press('Enter')
        page.waitFor(() => document.querySelector('.m-date-period-options'))
        page.keyboard.press('Escape')
        page.waitFor(() => !document.querySelector('.m-date-period-options'))
        page.locator('button[aria-label="Use a date range for Date"]').press('Enter')
        page.waitFor(() => document.getElementById('date-find_from'))
        page.locator('#date-find_from').fill('2026-11-02')
        page.locator('#date-find_thru').fill('2026-11-04')
        page.locator('button[aria-label="Clear filter: Date"]').press('Enter')
        assert.deepEqual(page.evaluate(() => ['_pdate','_poffset','_period','_from','_thru'].map(suffix=>vm.fields['changedDate'+suffix])),
            // Quasar retains the empty date mask; m-form-link omits unfilled masks from the query.
            [null,null,null,'____-__-__','____-__-__'])
        assert.deepEqual(page.evaluate(() => [vm.fields.entity,vm.fields.user,vm.fields.createdDate_pdate]),
            ['Order','john.doe','2026-11-01'])
        page.locator('button[aria-label="Use a period for Date"]').press('Enter')
        page.waitFor(() => document.getElementById('date-find'))
        assert.equal(page.locator('button[aria-label="Choose a period for Date"]').count(), 1, 'the empty period control is still named')
        assert.equal(page.locator('button[aria-label="Clear filter: Created"]').count(), 1)
        const compact = page.getByRole('checkbox', {name:'Email notifications for Mentions', exact:true})
        assert.equal(compact.count(), 1, 'the blank option uses its authored tooltip as its accessible name')
        assert.equal(page.getByRole('checkbox', {name:'Daily digest for Mentions', exact:true}).count(), 1, 'unwrapped checkboxes use the same fallback')
        assert.equal(page.getByRole('checkbox', {name:'Keep visible', exact:true}).count(), 1, 'a visible option label stays authoritative')
        assert.equal(page.locator('#compact-checks .m-visually-hidden').evaluate(el => getComputedStyle(el).position), 'absolute', 'the fallback preserves the compact layout')
        compact.click()
        assert.equal(page.evaluate(() => JSON.stringify(vm.compact)), '["Y"]')
        assert.equal(page.evaluate(() => new FormData(document.getElementById('compact-checks')).get('notify')), 'Y', 'the authored checkbox value submits unchanged')
        compact.press('Enter')
        assert.equal(page.evaluate(() => JSON.stringify(vm.compact)), '[]')
        assert.equal(page.evaluate(() => new FormData(document.getElementById('compact-checks')).has('notify')), false, 'an unchecked option stays absent from native submission')
        const sharing = page.getByRole('checkbox', {name:'Share repositories', exact:true})
        sharing.click()
        page.locator('#save-checks').click()
        page.waitFor(() => window.checkSaves.length === 1)
        assert.deepEqual(page.evaluate(() => checkSaves[0].filter(([key]) => key !== 'moquiSessionToken')),
            [['sharing','Y'],['enabled','Y'],['tags','A'],['tags','B']], 'm-form submits one scalar Y per enabled flag and preserves valid multi-choice values')
        sharing.click()
        page.getByRole('checkbox', {name:'Enable sign in', exact:true}).click()
        page.locator('#save-checks-again').click()
        page.waitFor(() => window.checkSaves.length === 2)
        assert.deepEqual(page.evaluate(() => checkSaves[1].filter(([key]) => key !== 'moquiSessionToken')),
            [['tags','A'],['tags','B']], 'unchecked flags submit no stale N or Y values')
    },
})

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

feature("column moves retain focus on the moved field through reorder, hide and display", {
    expect() {
        const page = browser.newPage()
        page.setContent(`<!doctype html><html><head>
            <link rel="stylesheet" href="/libs/quasar/quasar.prod.css">
            </head><body><div id="app">
            <m-form-column-config id="columns" action="/unused" form-location="test.List" :columns-initial="columns"></m-form-column-config>
            </div>
            <script src="/libs/vue/vue.global.prod.js"></script>
            <script src="/libs/quasar/quasar.umd.prod.js"></script>
            <script src="/js/MoquiLib.js"></script>
            <script src="/js/WebrootVue.qvt.js"></script>
            <script>
            const app = Vue.createApp({ data() { return { columns: [
                { id: 'hidden', label: 'Hidden', children: [] },
                { id: 'column_0', label: 'Column 1', children: [{ id: 'alpha', label: 'Alpha' }, { id: 'beta', label: 'Beta' }] },
                { id: 'column_1', label: 'Column 2', children: [{ id: 'gamma', label: 'Gamma' }] }
            ] }; } });
            app.use(Quasar);
            app.component("m-form", moqui.qvt.components["m-form"]);
            app.component("m-form-column-config", moqui.qvt.components["m-form-column-config"]);
            app.mount("#app");
            </script></body></html>`)
        const focusState = () => page.evaluate(() => {
            const active = document.activeElement
            const row = active && active.closest(".q-item")
            return { field: row && row.querySelector(".q-item__label").textContent,
                action: active && active.getAttribute("aria-label"),
                column: row && row.closest(".q-list").querySelector(".q-item__label--header").textContent }
        })
        page.getByText("Alpha", { exact: true }).evaluate(label => {
            label.closest(".q-item").querySelector('[aria-label="Move to next column"]').focus()
            return true
        })
        page.keyboard.press("Enter")
        assert.deepEqual(focusState(), { field: "Alpha", action: "Move to next column", column: "Column 2" })
        page.getByText("Alpha", { exact: true }).evaluate(label => {
            label.closest(".q-item").querySelector('[aria-label="Move up within column"]').focus()
            return true
        })
        page.keyboard.press("Enter")
        assert.deepEqual(focusState(), { field: "Alpha", action: "Move down within column", column: "Column 2" })
        page.getByText("Alpha", { exact: true }).evaluate(label => {
            label.closest(".q-item").querySelector('[aria-label="Hide field"]').focus()
            return true
        })
        page.keyboard.press("Enter")
        assert.deepEqual(focusState(), { field: "Alpha", action: "Display field", column: "Hidden" })
        page.keyboard.press("Enter")
        page.locator(".q-menu .q-item").first().press("Enter")
        assert.deepEqual(focusState(), { field: "Alpha", action: "Hide field", column: "Column 1" })
    },
})

feature("chart text follows the rendered theme without replacing chart state or explicit colors", {
    expect() {
        const page = browser.newPage()
        page.setContent(`<!doctype html><html><head>
            <link rel="stylesheet" href="/libs/quasar/quasar.prod.css">
            <link rel="stylesheet" href="/css/WebrootVue.qvt.css">
            <style>.chart-container { background:var(--m-panel-bg); }</style>
            </head><body><div id="app">
            <m-chart id="theme-chart" :config="config" height="220px" width="420px"></m-chart>
            <m-chart id="explicit-chart" :config="explicit" height="220px" width="420px"></m-chart>
            <m-chart id="radial-chart" :config="radial" height="220px" width="420px"></m-chart>
            <m-chart id="radial-explicit" :config="radialExplicit" height="220px" width="420px"></m-chart>
            </div>
            <script src="/libs/vue/vue.global.prod.js"></script>
            <script src="/libs/quasar/quasar.umd.prod.js"></script>
            <script src="/libs/chartjs/chart.umd.min.js"></script>
            <script src="/js/MoquiLib.js"></script>
            <script src="/js/WebrootVue.qvt.js"></script>
            <script>
            window.drawnText = [];
            const fillText = CanvasRenderingContext2D.prototype.fillText;
            const fillRect = CanvasRenderingContext2D.prototype.fillRect;
            CanvasRenderingContext2D.prototype.fillRect = function() {
                this.lastTextBackdrop = this.fillStyle;
                return fillRect.apply(this, arguments);
            };
            CanvasRenderingContext2D.prototype.fillText = function(text) {
                drawnText.push({ chart: this.canvas.parentElement && this.canvas.parentElement.id, text: String(text), color: this.fillStyle, backdrop: this.lastTextBackdrop });
                return fillText.apply(this, arguments);
            };
            const config = { type: 'line', data: { labels: ['Alpha', 'Beta', 'Gamma'], datasets: [{ label: 'Requests', data: [1, 2, 3] }] },
                options: { animation: false, responsive: false, maintainAspectRatio: false } };
            const explicit = moqui.deepCopy(config);
            explicit.options.color = '#123456';
            explicit.options.plugins = { legend: { labels: { color: '#a12345' } } };
            explicit.options.scales = { x: { ticks: { color: () => '#345678' } }, y: { ticks: { color: '#56789a' } } };
            const radial = moqui.deepCopy(config); radial.type = 'radar';
            const radialExplicit = moqui.deepCopy(config); radialExplicit.type = 'polarArea';
            radialExplicit.options.scales = { r: { ticks: { color: '#234567', backdropColor: '#cdefab' } } };
            const app = Vue.createApp({ data() { return { config, explicit, radial, radialExplicit }; } });
            app.use(Quasar); Quasar.Dark.set(true);
            app.component('m-chart', moqui.qvt.components['m-chart']);
            window.vm = app.mount('#app');
            window.chart = id => Chart.getChart(document.querySelector('#' + id + ' canvas'));
            window.chartState = () => {
                const current = chart('theme-chart'), authored = chart('explicit-chart'), round = chart('radial-chart'), roundAuthored = chart('radial-explicit');
                const radialText = drawnText.filter(item => item.chart === 'radial-chart' && /^[0-9.]+$/.test(item.text));
                if (!current || !authored || !round || !roundAuthored || !radialText.length || !drawnText.some(item => item.chart === 'theme-chart' && item.text === 'Requests')) return null;
                const luminance = (color, background) => {
                    const context = document.createElement('canvas').getContext('2d');
                    if (background) { context.fillStyle = background; context.fillRect(0, 0, 1, 1); }
                    context.fillStyle = color; context.fillRect(0, 0, 1, 1);
                    const rgb = Array.from(context.getImageData(0, 0, 1, 1).data).slice(0, 3).map(value => {
                        const c = value / 255; return c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
                    });
                    return rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722;
                };
                const background = luminance(getComputedStyle(document.getElementById('theme-chart')).backgroundColor);
                const labels = drawnText.filter(item => item.chart === 'theme-chart');
                const ratios = labels.map(item => { const foreground = luminance(item.color); return (Math.max(background, foreground) + 0.05) / (Math.min(background, foreground) + 0.05); });
                const radialBackground = getComputedStyle(document.getElementById('radial-chart')).backgroundColor;
                const radialRatios = radialText.map(item => {
                    const foreground = luminance(item.color), backdrop = luminance(item.backdrop, radialBackground);
                    return (Math.max(backdrop, foreground) + 0.05) / (Math.min(backdrop, foreground) + 0.05);
                });
                return { dark: Quasar.Dark.isActive, id: current.id, minimumContrast: Math.min(...ratios),
                    color: current.options.color, legend: labels.filter(item => item.text === 'Requests').pop().color,
                    zoom: [current.options.scales.x.min, current.options.scales.x.max],
                    explicit: { color: authored.options.color, legend: authored.legend.legendItems[0].fontColor,
                        x: authored.scales.x.options.ticks.color, y: authored.scales.y.options.ticks.color },
                    radialContrast: Math.min(...radialRatios), radialBackdrop: round.scales.r.options.ticks.backdropColor,
                    radialExplicit: { color: roundAuthored.scales.r.options.ticks.color, backdrop: roundAuthored.scales.r.options.ticks.backdropColor },
                    untouched: Chart.defaults.color === '#666' && vm.config.options.color === undefined && vm.radial.options.scales === undefined };
            };
            </script></body></html>`)
        const initial = page.waitFor(() => chartState())
        assert.equal(initial.dark, true)
        assert.ok(initial.minimumContrast >= 4.5, "fresh-dark canvas text contrast: " + initial.minimumContrast)
        assert.ok(initial.radialContrast >= 4.5, "fresh-dark radial tick/backdrop contrast: " + initial.radialContrast)
        assert.deepEqual(initial.radialExplicit, { color: "#234567", backdrop: "#cdefab" })
        const explicit = { color: "#123456", legend: "#a12345", x: "#345678", y: "#56789a" }
        assert.deepEqual(initial.explicit, explicit)
        page.evaluate(() => {
            const current = chart("theme-chart")
            current.options.scales.x.min = 1; current.options.scales.x.max = 2
            window.initialData = current.data
            current.update("none")
            return true
        })
        for (const dark of [false, true, false]) {
            page.evaluate(mode => { drawnText.length = 0; Quasar.Dark.set(mode); return true }, dark)
            const state = page.waitFor(() => chartState())
            assert.equal(state.dark, dark)
            assert.equal(state.id, initial.id)
            assert.deepEqual(state.zoom, [1, 2])
            assert.deepEqual(state.explicit, explicit)
            assert.ok(state.minimumContrast >= 4.5, "rendered canvas text contrast: " + state.minimumContrast)
            assert.ok(state.radialContrast >= 4.5, "radial tick/backdrop contrast: " + state.radialContrast)
            assert.deepEqual(state.radialExplicit, { color: "#234567", backdrop: "#cdefab" })
            assert.equal(state.untouched, true)
            assert.equal(page.evaluate(() => chart("theme-chart").data === initialData), true)
        }
    },
})
