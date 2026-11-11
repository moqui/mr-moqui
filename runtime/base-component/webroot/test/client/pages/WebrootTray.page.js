/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// The tray's tabs undocked (plans/Terminal-Fancy.md §10.6, §10.7, §10.9), in the page: Vue and Quasar, the shell's root options,
// m-shell-tray in a q-layout and the float layer. A tab's screen is a stand-in m-dynamic-container with a draft of its own, so
// what is checked is the tray's: the same instance in the strip and in a float, its count, its reloads, Esc and the reach.
"use strict";
const floats = moqui.floats;
const qvt = moqui.qvt;

floats.viewport = () => ({ w: 1440, h: 1000, top: 50 });
try { localStorage.removeItem("moqui.floats"); sessionStorage.clear(); } catch (err) { /* no storage */ }
floats.load(true);
floats.measure();

const reloads = [];
const options = qvt.rootOptions({ basePath: "/apps", linkBasePath: "/qapps", appRootPath: "" });
const host = document.createElement("div");
document.body.appendChild(host);
const app = Vue.createApp({ data: options.data, computed: options.computed, methods: options.methods, provide: options.provide,
    template: '<q-layout view="hHh LpR fFf"><q-header id="top"><div style="height:50px"></div></q-header>' +
        '<q-page-container id="m-canvas"><q-page><m-shell-tray v-if="trayTabs.length"></m-shell-tray></q-page></q-page-container></q-layout><m-float-layer></m-float-layer>' });
app.use(Quasar);
Object.keys(qvt.components).forEach((name) => app.component(name, qvt.components[name]));
// a tab's screen: a draft in its own data and a list long enough to scroll; shellRefresh is how the tray reloads it
app.component("m-dynamic-container", {
    props: ["url", "id"],
    data() { return { draft: "" }; },
    methods: { shellRefresh() { reloads.push(this.id); } },
    template: '<div :id="id"><input class="m-test-draft" v-model="draft"><div v-for="n in 60" :key="n" style="height:24px">Row {{n}}</div></div>'
});
app.config.globalProperties.moqui = moqui;
const vm = app.mount(host);
vm.shellData = { apps: [], shell: { trayToday: "/qapps/Hub/Panel/Today", trayChat: "/qapps/Mail/Panel/Chat", trayNotifications: "/qapps/Hub/Panel/Notifications" }, counts: {} };
vm.shellCounts = { chat: 2 };

const settle = async () => {
    for (let i = 0; i < 4; i++) { await Vue.nextTick(); await new Promise((resolve) => setTimeout(resolve, 0)); }
};
const $ = (selector) => document.querySelector(selector);
const tabs = () => Array.from(document.querySelectorAll("#m-tray .q-tab .q-tab__label")).map((label) => label.textContent.trim());
const body = (key) => $("[data-tray-tab=" + key + "]");

test("docked, the tray is as it was, with Undock in its strip; Undock floats the current tab with its label and count", async () => {
    vm.openTray("chat");
    await settle();
    assert.deepEqual(tabs(), ["Today", "Chat", "Alerts"]);
    assert.equal($("#m-tray-undock") !== null, true);
    const input = body("chat").querySelector(".m-test-draft");
    input.value = "half a message";
    input.dispatchEvent(new Event("input"));
    body("chat").scrollTop = 200;
    body("chat").dispatchEvent(new Event("scroll"));
    assert.equal(vm.trayScroll.chat, 200);
    $("#m-tray-undock").click();
    await settle();
    const float = floats.find("tray", "chat");
    assert.deepEqual([float.x, float.y, float.w, float.h], [1440 - 400 - 8, 58, 400, 560]);
    assert.equal($("#m-float-tray-chat").getAttribute("aria-label"), "Chat");
    assert.equal($("#m-float-tray-chat [data-float-badge]").textContent, "2");
    // it left the strip, and the tray (open on it) closed
    assert.equal(vm.trayOpen, false);
    assert.deepEqual(vm.trayDockedTabs.map((tab) => tab.key), ["today", "notifications"]);
    assert.equal(body("chat").closest("#m-float-body-tray-chat") !== null, true);
    assert.equal(body("chat").querySelector(".m-test-draft"), input, "the same screen, not loaded again");
    assert.equal(input.value, "half a message");
    assert.equal(body("chat").scrollTop, 200, "its scroll put back");
    vm.openTray("today");
    await settle();
    assert.deepEqual(tabs(), ["Today", "Alerts"]);
    assert.equal(body("chat").style.display, "", "a floated tab shows whatever the tray's tab");
});

test("a floated tab refreshes as a docked one: shellCounts, its own topic and reloadTrayTab; its count follows", async () => {
    reloads.length = 0;
    vm.closeTray();
    vm.onShellNotification({ topic: "shellCounts", message: { chat: 5, today: 1 } });
    await settle();
    assert.deepEqual(reloads, ["m-tray-chat"], "the floated tab, though the tray is shut");
    assert.equal($("#m-float-tray-chat [data-float-badge]").textContent, "5");
    vm.onShellNotification({ topic: "chatRoom:R1" });
    vm.reloadTrayTab("chat");
    assert.deepEqual(reloads, ["m-tray-chat", "m-tray-chat", "m-tray-chat"]);
    vm.openTray("today");
    vm.onShellNotification({ topic: "shellCounts", message: { chat: 5 } });
    assert.deepEqual(reloads.slice(3), ["m-tray-today", "m-tray-chat"], "the open tab and the floated one");
});

test("the header's buttons show, raise or hide a floated tab's window; Esc closes the tray and leaves the float", async () => {
    const id = floats.find("tray", "chat").id;
    floats.open("tray", "notifications");
    await settle();
    assert.equal(floats.isTop(floats.get(id)), false);
    vm.toggleTray("chat");
    assert.equal(floats.isTop(floats.get(id)), true, "raised");
    assert.equal(vm.trayTab, "today", "the tray is not opened on it");
    vm.toggleTray("chat");
    assert.equal(floats.get(id).hidden, true, "the top one hides");
    assert.equal(vm.trayFloatShown("chat"), false);
    assert.deepEqual(vm.assistReach.filter((one) => one.indexOf("chat") >= 0), [], "a hidden float is not in reach");
    vm.toggleTray("chat");
    assert.equal(vm.trayFloatShown("chat"), true);
    // Esc: the tray closes, the floats stay
    vm.openTray("today");
    assert.equal(vm.closeTopmost(), true);
    assert.equal(vm.trayOpen, false);
    assert.equal(vm.closeTopmost(), false);
    assert.equal(floats.shown(floats.get(id)), true);
    // the reach: a floated tray tab's body is in it, the tray only while it is open
    assert.deepEqual(vm.assistReach, ["m-canvas", "m-float-body-tray-chat", "m-float-body-tray-notifications"]);
    // with every tab floated the tray has nothing to open
    floats.open("tray", "today");
    await settle();
    vm.toggleTray(null);
    assert.equal(vm.trayOpen, false);
    vm.trayTab = "";
    floats.close(floats.find("tray", "today").id);
    floats.close(floats.find("tray", "notifications").id);
    await settle();
});

test("Dock puts a tab back and opens the tray on it; Close puts it back in the strip and leaves the tray as it was", async () => {
    const input = body("chat").querySelector(".m-test-draft");
    // hidden, shown and raised, given the keys: its scroll is where it was (a hidden window is not display:none, and its content
    // takes focus without scrolling)
    assert.deepEqual([vm.trayScroll.chat, body("chat").scrollTop], [200, 200]);
    floats.dock(floats.find("tray", "chat").id);
    await settle();
    assert.equal(vm.trayOpen, true);
    assert.equal(vm.trayTab, "chat");
    assert.deepEqual(tabs(), ["Today", "Chat", "Alerts"]);
    assert.equal(body("chat").closest("#m-tray") !== null, true);
    assert.equal(body("chat").querySelector(".m-test-draft"), input);
    assert.equal(input.value, "half a message");
    assert.equal(body("chat").scrollTop, 200);
    vm.undockTrayTab("chat");
    vm.openTray("today");
    await settle();
    floats.close(floats.find("tray", "chat").id);
    await settle();
    assert.equal(vm.trayOpen, true);
    assert.equal(vm.trayTab, "today", "as it was");
    assert.deepEqual(tabs(), ["Today", "Chat", "Alerts"]);
    vm.closeTray();
    floats.close(floats.find("tray", "chat") ? floats.find("tray", "chat").id : "");
});

test("restored at a load: a floated tab floats again; the tray's own open state stays in sessionStorage", async () => {
    vm.undockTrayTab("notifications");
    vm.openTray("today");
    await settle();
    // a load: no windows yet, the kept state read again, then the tray restores
    floats.state.floats.splice(0);
    vm.trayOpen = false;
    vm.trayTab = "";
    await settle();
    floats.load(true);
    vm.restoreTray();
    await settle();
    assert.equal(floats.find("tray", "notifications") !== null, true);
    assert.equal(vm.trayOpen, true);
    assert.equal(vm.trayTab, "today");
    assert.equal(body("notifications").closest("#m-float-body-tray-notifications") !== null, true);
    // below 600 px nothing floats: no Undock in the strip, and the floated tab is docked
    floats.viewport = () => ({ w: 500, h: 900, top: 50 });
    floats.onViewport();
    await settle();
    assert.equal($("#m-tray-undock"), null);
    assert.equal(floats.find("tray", "notifications"), null);
    assert.deepEqual(tabs(), ["Today", "Chat", "Alerts"]);
    assert.equal(vm.undockTrayTab("chat"), false);
});
