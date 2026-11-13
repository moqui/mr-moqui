// The pure functions of moqui-html.js, in the page. Was js/moqui-html.test.js.
// The module script is what the file assigns api.pure to. The client still starts on this document.
spec({ server: false, timeout: 60 })

browser.runInPage({
    scripts: [
        "/files/component/webroot/test/client/pages/moqui-html-module.js",
        "/js/moqui-html.js",
    ],
    test: "/files/component/webroot/test/client/pages/moqui-html.page.js",
})

// From Dialog.spec.js, merged here to share a spec run.
// The dialog of moqui-html.js: focus and which clicks close it. Was js/dialog-browser.test.js.

feature("a dialog focuses its first form field, not a Tom Select search, and closes on a backdrop click only", {
    expect() {
        const page = browser.newPage({ viewport: { width: 1440, height: 1000 } })
        // The body is taller than the dialog, so the dialog scrolls and a click on its scroll bar has the dialog as target.
        page.setContent(`<!doctype html><html data-theme="light">
            <link rel="stylesheet" href="/css/moqui-html.css">
            <button id="open" data-dialog="customer">New Customer</button>
            <dialog id="customer"><header><h4>New Customer</h4><button class="close" data-close>×</button></header>
                <div class="body"><div class="ts-wrapper"><div class="ts-control"><input name="generatedSearch"></div></div><input name="firstName"><div style="height:2000px"></div></div></dialog>
            <pre id="result"></pre>
            <script src="/js/moqui-html.js"></script>
            <script>
            document.addEventListener("DOMContentLoaded", () => {
                document.getElementById("open").click();
                const dialog = document.getElementById("customer");
                const rect = dialog.getBoundingClientRect();
                const focused = document.activeElement.name;
                dialog.dispatchEvent(new MouseEvent("click", { bubbles: true, clientX: rect.right - 2, clientY: rect.top + rect.height / 2 }));
                const staysOpenInside = dialog.open;
                dialog.dispatchEvent(new MouseEvent("click", { bubbles: true, clientX: 1, clientY: 1 }));
                document.getElementById("result").textContent = JSON.stringify({ focused, staysOpenInside, closesOutside: !dialog.open });
            });
            </script>`)
        const text = page.waitFor(() => {
            const node = document.getElementById("result")
            return node && node.textContent ? node.textContent : null
        })
        assert.deepEqual(JSON.parse(text), { focused: "firstName", staysOpenInside: true, closesOutside: true })
    },
})

// From DisabledControls.spec.js, merged here to share a spec run.
// Real DOM events and direct component calls must both respect disabled controls.

function exerciseDisabledControls() {
    const page = browser.newPage()
    page.setContent(`<!doctype html><html><head>
        <link rel="stylesheet" href="/libs/quasar/quasar.prod.css"></head><body><div id="app"></div>
        <script src="/libs/vue/vue.global.prod.js"></script>
        <script src="/libs/quasar/quasar.umd.prod.js"></script>
        <script src="/js/MoquiLib.js"></script><script src="/js/MoquiL10n.js"></script>
        <script src="/js/WebrootVue.qvt.js"></script><script>
        window.calls = 0;
        moqui.request = () => { window.calls++; return Promise.resolve({json:{}}); };
        const app = Vue.createApp({
            data() { return {disabled:true}; },
            methods: { getLinkPath(path) { return path; }, setUrl() { window.calls++; } },
            template: '<div><m-link ref="link" id="link" href="/allowed" :disable="disabled">Go</m-link>' +
                '<m-form ref="form" action="/write" :disable="disabled" no-validate><input id="value"><button id="submit" type="submit">Save</button></m-form>' +
                '<m-form-link ref="formLink" action="/find" :disable="disabled" no-validate><input id="find"></m-form-link></div>'
        });
        app.use(Quasar);
        Object.keys(moqui.qvt.components).forEach(name => app.component(name, moqui.qvt.components[name]));
        window.vm = app.mount('#app');
        </script></body></html>`)
    page.waitFor(() => document.querySelector("#value"))
    page.evaluate(() => {
        document.querySelector("#link").click();
        document.querySelector("#submit").click();
        document.querySelector("#value").focus();
        window.vm.$refs.form.submitForm();
        window.vm.$refs.form.submitGo();
        window.vm.$refs.formLink.submitForm();
        window.vm.$refs.formLink.submitGo();
    })
    page.keyboard.press("Enter")
    assert.equal(page.evaluate(() => window.calls), 0)
    assert.equal(page.evaluate(() => document.querySelector("#link").getAttribute("aria-disabled")), "true")
    assert.equal(page.evaluate(() => document.querySelector("#link").hasAttribute("href")), false)
    page.evaluate(() => { window.vm.disabled = false; })
    page.waitFor(() => document.querySelector("#link").hasAttribute("href"))
    page.evaluate(() => document.querySelector("#link").click())
    assert.equal(page.evaluate(() => window.calls), 1)
    page.close()
    return true
}

feature("disabled links and forms refuse clicks, keyboard and programmatic submission", {
    expect() {
        const passed = exerciseDisabledControls()
        assert.equal(passed, true)
    }
})

// From Download.spec.js, merged here to share a spec run.
// moqui.downloadData writes a blob link and revokes it. Was js/download-browser.test.js.

feature("inline chart CSV export preserves payload, filename and MIME type and cleans its object URL", {
    expect() {
        const page = browser.newPage()
        page.setContent(`<!doctype html><html><button id="csv" onclick="moqui.downloadData('Name,Value\\nCafé,2\\n','report.csv','text/csv')">Get CSV</button><pre id="result"></pre>
            <script>
            const state = {}; let blob;
            URL.createObjectURL = (value) => { blob = value; return "blob:fixture"; };
            URL.revokeObjectURL = (value) => { state.revoked = value; };
            HTMLAnchorElement.prototype.click = function () { state.filename = this.download; state.url = this.href; state.attached = this.isConnected; };
            window.addEventListener("error", (event) => { state.error = event.message; });
            </script>
            <script src="/js/moqui-html.js"></script>
            <script>
            document.getElementById("csv").click();
            setTimeout(async () => {
                if (blob) { state.type = blob.type; state.text = await blob.text(); }
                state.links = document.querySelectorAll("a[download]").length;
                document.getElementById("result").textContent = JSON.stringify(state);
            }, 100);
            </script>`)
        const text = page.waitFor(() => {
            const node = document.getElementById("result")
            return node && node.textContent ? node.textContent : null
        })
        assert.deepEqual(JSON.parse(text), {
            filename: "report.csv", url: "blob:fixture", attached: true, revoked: "blob:fixture",
            type: "text/csv", text: "Name,Value\nCafé,2\n", links: 0,
        })
    },
})

// From HeadClient.spec.js, merged here to share a spec run.
// Header.html.fancy loads moqui-html.js in the head without defer. Was js/head-client-browser.test.js.

feature("the html client in the head has moqui.request while the body parses and binds the page after", {
    expect() {
        const page = browser.newPage()
        page.setContent(`<!doctype html><html><head><meta name="moquiSessionToken" content="initial"><script>
            window.calls = [];
            window.fetch = (url, options) => { calls.push({ url, credentials: options.credentials }); return Promise.resolve(new Response("[]", { headers: { "Content-Type": "application/json" } })); };
            </script><script src="/js/moqui-html.js"></script></head><body>
            <button data-dialog="detail">Details</button><dialog id="detail">Loaded</dialog><pre id="result"></pre><script>
            const before = { request: typeof moqui.request, ready: document.readyState, bound: document.documentElement.classList.contains("js") };
            const answer = moqui.request("/calendar/events");
            document.addEventListener("DOMContentLoaded", () => { setTimeout(async () => { await answer; document.querySelector("button").click();
                document.getElementById("result").textContent = JSON.stringify({ before, calls, bound: document.documentElement.classList.contains("js"), opened: document.querySelector("dialog").open }); }, 0); }, { once: true });
            </script></body></html>`)
        const text = page.waitFor(() => {
            const node = document.getElementById("result")
            return node && node.textContent ? node.textContent : null
        })
        const result = JSON.parse(text)
        assert.deepEqual(result.before, { request: "function", ready: "loading", bound: false })
        assert.deepEqual(result.calls, [{ url: "/calendar/events", credentials: "same-origin" }])
        assert.equal(result.bound, true)
        assert.equal(result.opened, true)
    },
})

// From InlineSelect.spec.js, merged here to share a spec run.
// An inline remote select sizes to its options. Was js/inline-select-browser.test.js.

feature("inline remote options set the natural width and resize after a dependency changes", {
    expect() {
        const page = browser.newPage({ viewport: { width: 1440, height: 1000 } })
        page.setContent(`<!doctype html><html data-theme="light">
            <link rel="stylesheet" href="/css/moqui-html.css">
            <link rel="stylesheet" href="/libs/tom-select/tom-select.css">
            <form><select id="country" name="country"><option value="USA">USA</option><option value="CAN">Canada</option></select>
            <div style="width:840px;padding:0 15px"><div class="field big-row"><label>State</label><div class="widget">
                <div class="big-item"><div class="field"><div class="widget"><select id="state" name="state" data-ts="remote" data-url="/states" data-depends='[["country","country"]]'><option selected value="USA_CO">CO - Colorado</option></select></div></div></div>
                <div class="big-item"><div class="field"><div class="widget"><select id="search" data-ts="remote" data-search><option>A</option></select></div></div></div>
            </div></div><div class="field"><label>Owner</label><div class="widget"><select id="owner" data-ts="remote"><option>A</option></select></div></div></div></form>
            <pre id="result"></pre>
            <script>
            const responses = { USA: [{ value: "USA_FM", label: "FM - Federated States of Micronesia" }, { value: "USA_CO", label: "CO - Colorado" }], CAN: [{ value: "CAN_BC", label: "BC - British Columbia" }] };
            let calls = 0;
            window.fetch = (url, options) => { calls++; const country = new URLSearchParams(options.body).get("country"); return Promise.resolve({ ok: true, headers: new Headers(), text: () => Promise.resolve(JSON.stringify(responses[country])) }); };
            </script>
            <script src="/libs/tom-select/tom-select.complete.min.js"></script>
            <script src="/js/moqui-html.js"></script>
            <script>
            const metric = () => { const select = document.getElementById("state"); return { width: select.tomselect.wrapper.getBoundingClientRect().width, value: select.value, count: Object.keys(select.tomselect.options).filter(Boolean).length, submitted: new FormData(document.querySelector("form")).get("state") }; };
            const natural = () => { const s = document.createElement("select"); s.style.cssText = "position:absolute;width:auto;min-width:0;max-width:none;padding:0;border:1px solid;font:" + getComputedStyle(document.getElementById("state")).font;
                for (const o of document.getElementById("state").options) s.appendChild(new Option(o.text, o.value)); document.body.appendChild(s); const w = s.getBoundingClientRect().width; s.remove(); return w; };
            setTimeout(() => {
                const initial = metric(), expected = natural();
                const owner = document.getElementById("owner").tomselect.wrapper.getBoundingClientRect().width;
                const ownerWidget = document.getElementById("owner").closest(".widget").clientWidth;
                const search = document.getElementById("search").tomselect.wrapper.style.width;
                document.getElementById("country").value = "CAN"; document.getElementById("country").dispatchEvent(new Event("change"));
                setTimeout(() => { document.getElementById("result").textContent = JSON.stringify({ initial, expected, owner, ownerWidget, search, updated: metric(), calls, temporarySelects: document.querySelectorAll("select[aria-hidden=true]").length }); }, 50);
            }, 50);
            </script>`)
        const text = page.waitFor(() => {
            const node = document.getElementById("result")
            return node && node.textContent ? node.textContent : null
        })
        const result = JSON.parse(text)
        assert.ok(Math.abs(result.initial.width - (result.expected + 24)) < 0.5, JSON.stringify(result))
        assert.ok(result.initial.width > 200)
        assert.equal(result.initial.value, "USA_CO")
        assert.equal(result.initial.submitted, "USA_CO")
        assert.equal(result.initial.count, 2)
        assert.ok(result.updated.width < result.initial.width)
        assert.ok(result.updated.width >= 60)
        assert.equal(result.updated.value, "CAN_BC")
        assert.equal(result.updated.submitted, "CAN_BC")
        assert.equal(result.updated.count, 1)
        assert.ok(result.owner > 600, JSON.stringify(result))
        assert.equal(result.search, "")
        assert.equal(result.calls, 2)
        assert.equal(result.temporarySelects, 0)
    },
})

// From TablePopup.spec.js, merged here to share a spec run.
// Menus and drop-downs in a sideways-scrolling table. Was js/table-popup-browser.test.js.

feature("a menu and a drop-down in a scrolling table open outside it, and the menu closes when the table scrolls", {
    expect() {
        const options = Array.from({ length: 12 }, (_, i) => `<option value="${i}">Option ${i}</option>`).join("")
        const page = browser.newPage({ viewport: { width: 1000, height: 800 } })
        page.setContent(`<!doctype html><html data-theme="light">
            <link rel="stylesheet" href="/css/moqui-html.css">
            <link rel="stylesheet" href="/libs/tom-select/tom-select.css">
            <div style="width:500px"><div class="table-wrap" id="wrap"><table class="form-list" style="width:900px"><tbody><tr>
                <td><details class="menu" id="menu"><summary data-btn="primary">Actions</summary><ul><li><a href="#one">One</a></li><li><a href="#two">Two</a></li><li><a href="#three" id="last">Three</a></li></ul></details></td>
                <td><select id="pick" data-ts="select">${options}</select></td><td style="width:600px">Wide</td></tr></tbody></table></div></div>
            <pre id="result"></pre>
            <script src="/libs/tom-select/tom-select.complete.min.js"></script>
            <script src="/js/moqui-html.js"></script>
            <script>
            setTimeout(() => {
                const menu = document.getElementById("menu"); menu.open = true;
                setTimeout(() => {
                    const rect = document.getElementById("last").getBoundingClientRect();
                    const menuVisible = document.elementFromPoint(rect.x + rect.width / 2, rect.y + rect.height / 2) === document.getElementById("last");
                    const pick = document.getElementById("pick").tomselect; pick.open();
                    const dropdownOnBody = pick.dropdown.parentElement === document.body;
                    const wrap = document.getElementById("wrap"); wrap.scrollLeft = 50; wrap.dispatchEvent(new Event("scroll"));
                    setTimeout(() => { document.getElementById("result").textContent = JSON.stringify({ menuVisible, dropdownOnBody, menuClosed: !menu.open, scrolls: wrap.scrollWidth > wrap.clientWidth }); }, 50);
                }, 20);
            }, 50);
            </script>`)
        const text = page.waitFor(() => {
            const node = document.getElementById("result")
            return node && node.textContent ? node.textContent : null
        })
        assert.deepEqual(JSON.parse(text), { menuVisible: true, dropdownOnBody: true, menuClosed: true, scrolls: true })
    },
})

// visible-when and require-when of the html mode (moqui-form-when.js; docs/ui/Db-Forms.md): a box shows and its inputs
// are required while the rule holds, a hidden box's input is not required, and a hidden field counts as empty.
feature("a field's box shows and is required by the value of another field of its form", {
    expect() {
        const page = browser.newPage({ viewport: { width: 800, height: 600 } })
        const rule = (field, operator, values) => JSON.stringify({ form: "F", field, operator, values }).replace(/"/g, "&quot;")
        page.setContent(`<!doctype html><html data-theme="light">
            <form id="F"><select name="kind" id="kind"><option value="A">A</option><option value="B">B</option></select>
            <div class="field" id="serialBox" data-require-when="${rule("kind", "equals", ["B"])}"><label>Serial</label><div class="widget"><input name="serial" id="serial"></div></div>
            <div class="field" id="noteBox" data-visible-when="${rule("kind", "not-equals", ["A"])}" data-require-when="${rule("kind", "not-empty", [""])}"><label>Note</label><div class="widget"><input name="note" id="note" value="x"></div></div>
            <div class="field" id="extraBox" data-visible-when="${rule("note", "not-empty", [""])}"><label>Extra</label><div class="widget"><input name="extra"></div></div>
            </form><pre id="result"></pre>
            <script src="/js/moqui-form-when.js"></script>
            <script>
            document.addEventListener("DOMContentLoaded", () => {
                const state = () => ({ note: !document.getElementById("noteBox").hidden, extra: !document.getElementById("extraBox").hidden,
                    serial: document.getElementById("serial").required, noteRequired: document.getElementById("note").required,
                    marked: document.getElementById("serialBox").hasAttribute("data-required") });
                const first = state();
                const kind = document.getElementById("kind"); kind.value = "B"; kind.dispatchEvent(new Event("change", { bubbles: true }));
                document.getElementById("result").textContent = JSON.stringify({ first, second: state() });
            });
            </script>`)
        const text = page.waitFor(() => {
            const node = document.getElementById("result")
            return node && node.textContent ? node.textContent : null
        })
        assert.deepEqual(JSON.parse(text), {
            first: { note: false, extra: false, serial: false, noteRequired: false, marked: false },
            second: { note: true, extra: true, serial: true, noteRequired: true, marked: true },
        })
    },
})
