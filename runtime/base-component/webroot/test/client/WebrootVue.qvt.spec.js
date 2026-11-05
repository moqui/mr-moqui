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

feature("find controls name their field and keyboard actions retain the other filters", {
    expect() {
        const page = browser.newPage()
        page.setContent(`<!doctype html><html><head><link rel="stylesheet" href="/libs/quasar/quasar.prod.css"></head><body><div id="app">
            <m-text-find id="entity-find" name="entity" label="Entity" :fields="fields"></m-text-find>
            <m-text-find id="user-find" name="user" label="User" :fields="fields"></m-text-find>
            <m-date-period id="date-find" name="changedDate" label="Date" :fields="fields"></m-date-period>
            <m-date-period id="created-find" name="createdDate" label="Created" :fields="fields"></m-date-period>
            </div><script src="/libs/vue/vue.global.prod.js"></script><script src="/libs/quasar/quasar.umd.prod.js"></script>
            <script src="/js/MoquiLib.js"></script><script src="/js/MoquiL10n.js"></script><script src="/js/WebrootVue.qvt.js"></script><script>
            Quasar.QMenu.props.transitionDuration.default=0;
            moqui.l10n.loadCatalog('en-US',()=>({messages:{'Find options for {field}':'Options for {field}',
                'Clear {field} filter':'Clear filter: {field}'}})).then(()=>{
                const app=Vue.createApp({data(){return {fields:{entity:'Order',user:'john.doe',changedDate_period:'day',
                    changedDate_poffset:'0',changedDate_pdate:'2026-11-04',createdDate_pdate:'2026-11-01'}};}});
                app.use(Quasar);['m-text-find','m-text-line','m-date-period','m-date-time'].forEach(name=>app.component(name,moqui.qvt.components[name]));
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

feature("routed shell menu items activate once from the keyboard", {
    expect() {
        const page = browser.newPage()
        page.setContent(`<!doctype html><html><head>
            <link rel="stylesheet" href="/libs/quasar/quasar.prod.css">
            <link rel="stylesheet" href="/css/WebrootVue.qvt.css">
            </head><body><div id="app"></div>
            <script src="/libs/vue/vue.global.prod.js"></script>
            <script src="/libs/quasar/quasar.umd.prod.js"></script>
            <script src="/js/MoquiLib.js"></script>
            <script src="/js/WebrootVue.qvt.js"></script>
            <script>
            // Menu animation timing belongs to acceptance; the keyboard and activation rules stay here.
            Quasar.QMenu.props.transitionDuration.default = 0;
            // Mount the actual shell navigation markup, without fetching or rendering an application screen.
            const source = moqui.qvt.components['m-qapps-root'].template;
            const start = source.indexOf('<q-btn v-if="appScreenMenu"');
            const crumbs = source.indexOf('<q-breadcrumbs v-if="!handheld"');
            const template = source.slice(start, source.indexOf('</q-btn>', start) + 8) +
                source.slice(crumbs, source.indexOf('</q-breadcrumbs>', crumbs) + 16) + '<button id="after-navigation">Search</button>';
            // The slices end at the first closing tag: a nested q-btn in the menu button would cut it short, so say so.
            const opened = (template.match(/<q-btn\\b/g) || []).length, closed = (template.match(/<\\/q-btn>/g) || []).length;
            if (start < 0 || crumbs < 0 || opened !== closed) throw new Error('the shell navigation markup changed: update this fixture');
            window.menu = { subscreens: [{ name: 'Home', title: 'Home', pathWithParams: '/qapps/dev/Home' }] };
            window.visits = []; window.vueErrors = []; window.enterPrevented = [];
            const app = Vue.createApp({ template, data() { return { appScreenMenu: window.menu, headerTitle: 'Dev AIDE', handheld: false,
                breadcrumbStart: 0, navMenuList: [
                    { title: 'LLM', subscreens: [{ name: 'Agents', title: 'Agents', pathWithParams: '/qapps/system/LLM/Agents' }] },
                    { title: 'Agents', subscreens: [{ name: 'Agent', title: 'Agent detail', pathWithParams: '/qapps/system/LLM/Agents/Agent' }] },
                    { title: 'Current agent' } ] }; },
                methods: { getLinkPath(path) { return path; }, setUrl(path) { visits.push(path); this.appScreenMenu = null; } } });
            app.use(Quasar);
            app.component('m-menu-icon', moqui.qvt.components['m-menu-icon']);
            app.config.globalProperties.$router = moqui.qvt.makeRouter();
            app.config.globalProperties.$route = { path: '/qapps/dev/Workspace', matched: [], params: {} };
            app.config.errorHandler = error => vueErrors.push(String(error));
            window.vm = moqui.webrootVue = app.mount('#app');
            document.addEventListener('keydown', event => { if (event.key === 'Enter') enterPrevented.push(event.defaultPrevented); });
            // Observe modified clicks without opening an unrelated browser tab.
            document.addEventListener('click', event => { if (event.ctrlKey || event.metaKey) event.preventDefault(); });
            </script></body></html>`)
        const openMenu = () => {
            page.evaluate(async () => { vm.appScreenMenu = window.menu; await Vue.nextTick(); return true })
            page.locator("#m-app-screens").click()
        }
        openMenu()
        assert.equal(page.locator(".q-menu a").evaluate(item => item.getAttribute("href")), "/qapps/dev/Home")
        page.locator(".q-menu a").press("Enter")
        assert.deepEqual(page.evaluate(() => ({ visits, vueErrors, enterPrevented })),
            { visits: ["/qapps/dev/Home"], vueErrors: [], enterPrevented: [true] })
        assert.equal(page.locator(".q-menu").count(), 0)
        openMenu()
        page.locator(".q-menu a").press("Space")
        assert.equal(page.evaluate(() => visits.length), 2)
        openMenu()
        page.locator(".q-menu a").evaluate(item => {
            item.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, ctrlKey: true }))
            item.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, metaKey: true }))
            return true
        })
        assert.equal(page.evaluate(() => visits.length), 2, "modified clicks retain native link handling")
        openMenu()
        page.locator(".q-menu a").click()
        assert.equal(page.evaluate(() => visits.length), 3)
        assert.deepEqual(page.evaluate(() => vueErrors), [])
        page.evaluate(async () => { vm.appScreenMenu = window.menu; await Vue.nextTick(); document.querySelector('#m-app-screens').focus(); return true })
        page.keyboard.press("Tab")
        assert.equal(page.evaluate(() => document.activeElement.textContent.replace(/\s/g, "")), "LLMarrow_drop_down")
        assert.equal(page.evaluate(() => document.activeElement.tagName), "BUTTON")
        assert.equal(page.evaluate(() => getComputedStyle(document.activeElement).outlineStyle), "solid")
        for (const dark of [false, true]) {
            const ring = page.evaluate(async mode => {
                Quasar.Dark.set(mode); await Vue.nextTick()
                const style = getComputedStyle(document.activeElement)
                const luminance = color => {
                    const context = document.createElement("canvas").getContext("2d")
                    context.fillStyle = color; context.fillRect(0, 0, 1, 1)
                    const channels = Array.from(context.getImageData(0, 0, 1, 1).data).slice(0, 3).map(value => {
                        const c = value / 255; return c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4)
                    })
                    return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722
                }
                const foreground = luminance(style.outlineColor)
                const background = luminance(getComputedStyle(document.body).getPropertyValue("--m-header-bg"))
                const text = luminance(style.color)
                return { contrast: (Math.max(foreground, background) + 0.05) / (Math.min(foreground, background) + 0.05), width: style.outlineWidth,
                    textContrast: (Math.max(text, background) + 0.05) / (Math.min(text, background) + 0.05) }
            }, dark)
            assert.equal(ring.width, "2px")
            assert.ok(ring.contrast >= 3, "breadcrumb focus contrast: " + ring.contrast)
            assert.ok(ring.textContrast >= 4.5, "breadcrumb text contrast: " + ring.textContrast)
        }
        page.keyboard.press("Enter")
        page.waitFor(() => !!document.activeElement.closest(".q-menu"))
        page.keyboard.press("Escape")
        page.waitFor(() => !document.querySelector(".q-menu"))
        assert.equal(page.evaluate(() => document.activeElement.textContent.replace(/\s/g, "")), "LLMarrow_drop_down")
        page.keyboard.press("Tab")
        assert.equal(page.evaluate(() => document.activeElement.textContent.replace(/\s/g, "")), "Agentsarrow_drop_down")
        page.keyboard.press("Tab")
        assert.equal(page.evaluate(() => document.activeElement.id), "after-navigation")
        page.keyboard.press("Shift+Tab")
        page.keyboard.press("Space")
        page.waitFor(() => !!document.querySelector(".q-menu"))
        page.locator(".q-menu a").press("Enter")
        assert.equal(page.evaluate(() => visits[3]), "/qapps/system/LLM/Agents/Agent")
        assert.equal(page.evaluate(() => visits.length), 4)
        assert.deepEqual(page.evaluate(() => vueErrors), [])
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
