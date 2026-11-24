/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
test("the shared palette keeps search results and Enter with their query and opening, and leaves composition to the editor", async () => {
    const qvt = moqui.qvt;
    qvt.applyRenders();
    const duration = Quasar.QDialog.props.transitionDuration.default;
    Quasar.QDialog.props.transitionDuration.default = 40;
    const visits = [], requests = [], notices = [];
    const originals = { request: moqui.request, handleAjaxError: moqui.handleAjaxError,
        notifyMessages: moqui.notifyMessages, unreachable: moqui.unreachable };
    moqui.handleAjaxError = (err) => notices.push(err);
    moqui.notifyMessages = (messages) => notices.push(messages);
    moqui.unreachable = (err) => err.offline === true;
    moqui.request = (url, { params }) => new Promise((resolve, reject) => {
        requests.push({ text: params.text, reject, answer(titles) {
            resolve({ json: { groups: [{ group: "Rooms", label: "Rooms", rows: titles.map((title) => ({ title, url: "/" + title })) }] } });
        } });
    });
    const methods = qvt.rootOptions({}).methods;
    const app = Vue.createApp({
        data() { return { paletteOpen: false, labels: qvt.shellLabels, appRootPath: "/apps", paletteLead: ["Rooms"],
            paletteSources: [{ group: "Rooms", label: "Rooms", prefix: "#" }], paletteRecent: [], paletteScreens: [],
            paletteActions: [], paletteSavedFinds: [], assistOn: false }; },
        methods: { openPalette: methods.openPalette, shellKeyBindings: methods.shellKeyBindings,
            setUrl(url) { visits.push(url); }, closeTopmost: methods.closeTopmost, shellPopupOpen: methods.shellPopupOpen },
        render() { return Vue.h("main", [Vue.h("button", { id: "opener", onClick: this.openPalette }, "Search"),
            Vue.h("div", { id: "editor", contenteditable: true }), Vue.h(qvt.components["m-shell-palette"])]); }
    });
    app.use(Quasar);
    for (const [name, component] of Object.entries(qvt.components)) app.component(name, component);
    const host = document.body.appendChild(document.createElement("div"));
    const vm = app.mount(host);
    const off = moqui.keymap.registerScope("global", vm.shellKeyBindings());
    const onKey = (event) => moqui.keymap.handle(event);
    window.addEventListener("keydown", onKey);
    const opener = host.querySelector("#opener"), editor = host.querySelector("#editor");
    const input = () => document.getElementById("m-palette-input");
    const key = (el, name, extra, type = "keydown") => {
        const event = new KeyboardEvent(type, Object.assign({ key: name, keyCode: name === "Escape" ? 27 : 0,
            bubbles: true, cancelable: true }, extra));
        el.dispatchEvent(event);
        return event;
    };
    const type = (text) => { input().value = text; input().dispatchEvent(new Event("input", { bubbles: true })); };
    const until = async (check, what) => {
        const end = Date.now() + 2000;
        while (!check()) {
            if (Date.now() >= end) throw new Error("Timed out waiting for " + what);
            await new Promise((resolve) => setTimeout(resolve, 10));
        }
    };
    const open = async () => {
        opener.focus();
        key(opener, "k", { ctrlKey: true });
        await until(() => input() && document.activeElement === input(), "palette focus");
    };
    const flush = async () => { await Promise.resolve(); await Vue.nextTick(); };
    const result = () => Array.from(document.querySelectorAll("#m-palette [data-index]")).map((el) => el.textContent.trim());
    try {
        editor.focus();
        assert.equal(key(editor, "k", { ctrlKey: true }).defaultPrevented, false);
        assert.equal(vm.paletteOpen, false, "rich text keeps its link shortcut");
        await open();

        type("a"); key(input(), "Enter");
        assert.equal(requests[0].text, "a");
        type("b");
        requests[0].answer(["Obsolete"]);
        await flush();
        assert.deepEqual(result(), [], "the old answer did not populate the next query's debounce");
        assert.deepEqual(visits, []);
        key(input(), "Enter");
        assert.equal(requests[1].text, "b");
        requests[1].answer(["Beta"]);
        await until(() => !vm.paletteOpen, "deferred Enter opening Beta");
        assert.deepEqual(visits, ["/Beta"]);
        await until(() => !input(), "dialog dismissal");

        await open();
        type("same"); key(input(), "Enter");
        key(input(), "Escape"); key(input(), "Escape", {}, "keyup");
        await until(() => !input() && document.activeElement === opener, "Escape restoring the opener");
        await open();
        type("same");
        requests[2].answer(["BeforeDismissal"]);
        await flush();
        assert.deepEqual(result(), [], "the earlier opening's answer was dropped");
        assert.deepEqual(visits, ["/Beta"]);
        await until(() => requests.length === 4, "the reopened query");
        requests[3].answer(["First", "Second"]);
        await flush();
        assert.ok(result()[0].includes("First"));
        assert.ok(result()[1].includes("Second"));
        for (const name of ["ArrowDown", "ArrowUp", "Enter", "Escape"]) {
            assert.equal(key(input(), name, { isComposing: true }).defaultPrevented, false, name);
        }
        key(input(), "Escape", {}, "keyup");
        await flush();
        assert.equal(vm.paletteOpen, true);
        assert.equal(document.querySelector("#m-palette .m-palette-active").dataset.index, "0");
        type("lost");
        await Vue.nextTick();
        assert.deepEqual(result(), [], "the previous query's two visible rows disappear before the next request");
        assert.equal(requests.length, 4);
        key(input(), "Enter");
        const lost = requests[4];
        type("refused"); key(input(), "Enter");
        const refused = requests[5];
        type("current");
        await until(() => requests.length === 7, "the current query");
        requests[6].answer(["Current", "Chosen"]);
        await flush();
        const currentRows = result();
        assert.ok(currentRows[0].includes("Current") && currentRows[1].includes("Chosen"));
        lost.reject({ offline: true });
        refused.reject({ status: 500 });
        await flush();
        assert.deepEqual(result(), currentRows, "superseded failures leave the current rows visible");
        assert.deepEqual(notices, [], "no stale offline notification or error");
        assert.deepEqual(visits, ["/Beta"], "neither superseded Enter navigated");
        key(input(), "ArrowDown");
        await Vue.nextTick();
        key(input(), "Enter");
        assert.deepEqual(visits, ["/Beta", "/Chosen"]);
    } finally {
        window.removeEventListener("keydown", onKey);
        off();
        app.unmount();
        host.remove();
        Object.assign(moqui, originals);
        Quasar.QDialog.props.transitionDuration.default = duration;
    }
});
