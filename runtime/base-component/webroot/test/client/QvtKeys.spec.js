// Keys of the qvt widgets, with Vue and Quasar in the page and real key events. Was js/qvt-keys-browser.test.js.
spec({ server: false, timeout: 60 })

const states = "[{value:'AL',label:'Alabama'},{value:'AK',label:'Alaska'},{value:'AZ',label:'Arizona'}]"

function mount(template, data, setup = "") {
    const page = browser.newPage({ viewport: { width: 1200, height: 900 } })
    page.setContent(`<!doctype html><html><head>
        <link rel="stylesheet" href="/libs/quasar/quasar.prod.css">
        </head><body><div id="app"></div>
        <script src="/libs/vue/vue.global.prod.js"></script>
        <script src="/libs/quasar/quasar.umd.prod.js"></script>
        <script src="/js/MoquiLib.js"></script>
        <script src="/js/MoquiL10n.js"></script>
        <script src="/js/WebrootVue.qvt.js"></script>
        <script>
        ${setup}
        const app = Vue.createApp({ data() { return ${JSON.stringify(data)}; }, template: ${JSON.stringify(template)} });
        app.use(Quasar);
        Object.keys(moqui.qvt.components).forEach((name) => app.component(name, moqui.qvt.components[name]));
        window.vm = app.mount("#app");
        window.lastKey = null;
        document.addEventListener("keydown", (event) => { window.lastKey = { key: event.key, prevented: event.defaultPrevented }; });
        </script></body></html>`)
    page.waitFor(() => document.querySelector("#app input"))
    sleep(200)
    return page
}

function focusAt(page, index) {
    page.evaluate((index) => document.querySelectorAll("#app input")[index].focus(), index)
}

feature("an m-drop-down takes the typed match on a quick ENTER or TAB, and TAB moves on", {
    expect() {
        const page = mount(`<div><m-drop-down id="one" name="one" v-model="one" allow-empty :options="${states}"></m-drop-down>
            <m-drop-down id="two" name="two" v-model="two" allow-empty :options="${states}"></m-drop-down><input id="next"></div>`, { one: null, two: null })
        focusAt(page, 0)
        page.keyboard.type("Ariz")
        page.keyboard.press("Enter")
        sleep(1000)
        const afterEnter = page.evaluate(() => window.vm.one)
        focusAt(page, 1)
        page.keyboard.type("Alas")
        page.keyboard.press("Tab")
        sleep(1000)
        const afterTab = page.evaluate(() => ({ value: window.vm.two, focus: document.activeElement.id }))
        focusAt(page, 0)
        page.keyboard.type("Alab")
        sleep(700)
        page.keyboard.press("Enter")
        sleep(300)
        const slowEnter = page.evaluate(() => window.vm.one)
        assert.equal(afterEnter, "AZ")
        assert.equal(afterTab.value, "AK")
        assert.equal(afterTab.focus, "next")
        assert.equal(slowEnter, "AL")
    },
})

feature("date keys step an m-date-time input; its q-date and q-time keep their own keys", {
    expect() {
        const page = mount(`<div><m-date-time id="day" name="day" type="date" v-model="day"></m-date-time>
            <m-date-time id="stamp" name="stamp" v-model="stamp" :minute-step="15"></m-date-time>
            <m-date-time id="at" name="at" type="time" v-model="at"></m-date-time></div>`, { day: "2026-01-31", stamp: "2026-10-25 23:50", at: "" })
        const press = (index, key) => { focusAt(page, index); page.keyboard.press(key); sleep(50) }
        press(0, "m")
        const m = page.evaluate(() => window.vm.day)
        press(0, "Shift+Y")
        const shiftY = page.evaluate(() => window.vm.day)
        press(0, "ArrowUp")
        const up = page.evaluate(() => window.vm.day)
        press(0, "w")
        const w = page.evaluate(() => window.vm.day)
        press(0, "Control+d")
        const ctrlD = page.evaluate(() => [window.vm.day, window.lastKey.prevented])
        press(0, "h")
        const hOnDate = page.evaluate(() => window.vm.day)
        press(1, "i")
        const i = page.evaluate(() => window.vm.stamp)
        press(1, "Shift+H")
        const shiftH = page.evaluate(() => window.vm.stamp)
        press(2, "d")
        // d is not a time key, so the handler does not take it. A real key is then typed into Quasar's mask,
        // which a synthetic keydown never did; the model stays empty or the mask, not a clock time.
        const dOnEmptyTime = page.evaluate(() => window.vm.at)
        press(2, "t")
        const tTime = page.evaluate(() => /^\d\d:\d\d$/.test(window.vm.at))
        assert.equal(m, "2026-02-28")
        assert.equal(shiftY, "2025-02-28")
        assert.equal(up, "2025-03-01")
        assert.equal(w, "2025-03-08")
        assert.deepEqual(ctrlD, ["2025-03-08", false])
        assert.equal(hOnDate, "2025-03-08")
        assert.equal(i, "2026-10-26 00:05")
        assert.equal(shiftH, "2026-10-25 23:05")
        assert.ok(dOnEmptyTime === "" || dOnEmptyTime === "__:__", dOnEmptyTime)
        assert.equal(tTime, true)
    },
})

feature("remote choices retain a selection through form renders and search pages, and clear it for changed dependencies", {
    expect() {
        const page = mount(`<div><m-drop-down id="remote" name="remote" v-model="fields.one" allow-empty server-search :server-min-length="0"
            options-url="/choices" :options="[]" :fields="fields" :depends-on="{organization:'organization'}"></m-drop-down>
            <input id="reason" v-model="fields.reason"><input id="organization" v-model="fields.organization"></div>`,
            { fields: { one: null, reason: "", organization: "first" } },
            `moqui.request = (url, init) => Promise.resolve({json: String(init.params.term || '').includes('Z') ? [{value:'Z',label:'Zebra'}] : [{value:'A',label:'Alpha'}]});`)
        page.locator("#remote").fill("Zebra")
        page.getByRole("option", {name:"Zebra",exact:true}).click()
        page.locator("#reason").fill("Reviewed selection")
        page.waitFor(()=>window.vm.fields.one === "Z")
        page.locator("#remote").fill("Alpha")
        page.getByRole("option", {name:"Alpha",exact:true}).waitFor()
        assert.equal(page.evaluate(()=>window.vm.fields.one),"Z")
        page.locator("#organization").fill("second")
        page.waitFor(()=>window.vm.fields.one === null)
    },
})
