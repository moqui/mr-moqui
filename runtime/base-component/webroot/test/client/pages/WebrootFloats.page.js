/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// Floats (js/WebrootFloats.js), in the page: a real Vue application with Quasar, the layer, and a kind of test content (a note
// with a draft in its own data) that its component moves into a window by a teleport, as Assist and the tray tabs do.
"use strict";
const floats = moqui.floats;

const compiler = globalThis.VueCompilerDOM;

// a known viewport: 1200 by 850 with a 50 px header, so the area the windows keep to is 1200 by 800 from y 50
const view = { w: 1200, h: 850, top: 50 };
floats.viewport = () => Object.assign({}, view);
try { localStorage.removeItem("moqui.floats"); } catch (err) { /* no storage */ }
floats.load(true);
floats.measure();

const docked = [];
const counts = { a: 3 };
floats.register("note", { title: (key) => "Note " + key, icon: "sym_o_note", badge: (key) => counts[key] || "", defaultSize: { w: 400, h: 300 },
    minSize: { w: 200, h: 150 }, place: "right", inReach: true, dock: (shown, key) => docked.push([key, shown]) });

const host = document.createElement("div");
document.body.appendChild(host);
const app = Vue.createApp({
    data() { return { notes: ["a", "b"], handheld: false, labels: {} }; },
    template: '<div id="home"><m-test-note v-for="key in notes" :key="key" :note="key"></m-test-note></div><m-float-layer></m-float-layer>'
});
app.use(Quasar);
Object.keys(floats.components).forEach((name) => app.component(name, floats.components[name]));
// the content: its element goes into the window's body while it floats, and its component stays where it is
app.component("m-test-note", {
    props: ["note"],
    data() { return { draft: "" }; },
    computed: { target() { return floats.target("note", this.note); } },
    template: '<teleport :to="target || \'body\'" :disabled="!target"><div class="col m-test-note" :data-note="note"><input :data-input="note" v-model="draft"></div></teleport>'
});
const vm = app.mount(host);

const settle = async () => {
    for (let i = 0; i < 4; i++) { await Vue.nextTick(); await new Promise((resolve) => setTimeout(resolve, 0)); }
};
const $ = (selector) => document.querySelector(selector);
const win = (id) => document.getElementById(floats.domId(id));
const pointer = (el, type, x, y, extra) => el.dispatchEvent(new PointerEvent(type, Object.assign({ pointerId: 7, pointerType: "mouse", button: 0, buttons: 1,
    clientX: x, clientY: y, bubbles: true, cancelable: true }, extra)));
const drag = (el, from, to) => {
    pointer(el, "pointerdown", from[0], from[1]);
    pointer(el, "pointermove", (from[0] + to[0]) / 2, (from[1] + to[1]) / 2);
    pointer(el, "pointermove", to[0], to[1]);
    pointer(el, "pointerup", to[0], to[1]);
};
const rect = (f) => ({ x: f.x, y: f.y, w: f.w, h: f.h });
const reset = async () => {
    floats.dockAll(false);
    docked.length = 0;
    view.w = 1200; view.h = 850;
    floats.measure();
    floats.saved.places = {};
    await settle();
};

test("the components compile, and the layer draws nothing until a window opens", () => {
    Object.keys(floats.components).forEach((name) => {
        const errors = [];
        compiler.compile(floats.components[name].template, { mode: "function", onError: (err) => errors.push(err.message) });
        assert.deepEqual(errors, [], name);
    });
    assert.equal($("[data-float-layer]"), null);
    assert.equal(floats.canFloat(), true);
    assert.equal(floats.target("note", "a"), null);
});

test("a kind's content moves into its window and back with its draft; Close docks it closed and Dock shown", async () => {
    const input = $("[data-input=a]");
    input.value = "half typed";
    input.dispatchEvent(new Event("input"));
    const id = floats.open("note", "a");
    assert.equal(id, "note:a");
    await settle();
    const frame = win(id);
    assert.equal(frame.getAttribute("role"), "dialog");
    assert.equal(frame.getAttribute("aria-modal"), "false");
    assert.equal(frame.getAttribute("aria-label"), "Note a");
    assert.equal(frame.querySelector("[data-float-badge]").textContent, "3");
    // at the right below the header, at the kind's size
    assert.deepEqual(rect(floats.get(id)), { x: 1200 - 400 - 8, y: 58, w: 400, h: 300 });
    assert.equal(floats.target("note", "a"), "#m-float-body-note-a");
    const moved = $("[data-input=a]");
    assert.equal(moved, input, "the same element, not made again");
    assert.equal(moved.closest("[data-float-body]").id, "m-float-body-note-a");
    assert.equal(moved.value, "half typed");
    assert.deepEqual(floats.reach(), ["m-float-body-note-a"], "a kind that says inReach");
    // Close: the window goes, the content docks closed, the draft stays
    frame.querySelector("[data-float-action=close]").click();
    await settle();
    assert.deepEqual(docked, [["a", false]]);
    assert.equal(win(id), null);
    assert.equal($("[data-input=a]").closest("#home") !== null, true);
    assert.equal($("[data-input=a]").value, "half typed");
    // opened again it takes the place it had; Dock docks it shown
    floats.get(floats.open("note", "a"));
    await settle();
    assert.deepEqual(rect(floats.find("note", "a")), { x: 792, y: 58, w: 400, h: 300 });
    win(id).querySelector("[data-float-action=dock]").click();
    await settle();
    assert.deepEqual(docked, [["a", false], ["a", true]]);
    assert.equal($("[data-input=a]").closest("#home") !== null, true);
    // whenReady: the body of an open window, null for one that is not
    assert.equal(await floats.whenReady("note", "a"), null);
    const waiting = floats.whenReady("note", "b");
    floats.open("note", "b");
    assert.equal((await floats.whenReady("note", "b")).id, "m-float-body-note-b");
    assert.equal(await waiting, null, "asked before the window opened");
    await reset();
});

test("the title bar drags with pointer events, a click raises, a window stays in the viewport and a grip keeps the minimum", async () => {
    const a = floats.open("note", "a", { rect: { x: 100, y: 100, w: 400, h: 300 } });
    const b = floats.open("note", "b", { rect: { x: 300, y: 200, w: 400, h: 300 } });
    await settle();
    assert.equal(floats.isTop(floats.get(b)), true);
    const bar = win(a).querySelector("[data-float-bar]");
    drag(bar, [200, 110], [260, 150]);
    await settle();
    assert.deepEqual(rect(floats.get(a)), { x: 160, y: 140, w: 400, h: 300 });
    assert.equal(floats.isTop(floats.get(a)), true, "a press raises the window");
    assert.equal(Number(win(a).style.zIndex) > Number(win(b).style.zIndex), true);
    // past the right and the bottom (away from the edges, which snap): kept inside the area, below the header
    drag(bar, [260, 150], [1100, 600]);
    await settle();
    assert.deepEqual([floats.get(a).x, floats.get(a).y], [800, 550]);
    drag(bar, [900, 580], [900, 70]);
    assert.equal(floats.get(a).y, 50);
    assert.equal(floats.state.snap, null);
    // the se grip: wider and taller, then smaller than the kind allows
    Object.assign(floats.get(a), { x: 100, y: 100 });
    const grip = win(a).querySelector("[data-float-grip=se]");
    const at = floats.get(a);
    drag(grip, [at.x + at.w, at.y + at.h], [at.x + at.w + 50, at.y + at.h + 40]);
    assert.deepEqual([floats.get(a).w, floats.get(a).h], [450, 340]);
    drag(grip, [at.x + at.w, at.y + at.h], [0, 0]);
    assert.deepEqual([floats.get(a).w, floats.get(a).h], [200, 150]);
    // the nw grip keeps the opposite corner where it is
    const before = rect(floats.get(a));
    drag(win(a).querySelector("[data-float-grip=nw]"), [before.x, before.y], [before.x - 30, before.y + 400]);
    assert.deepEqual(rect(floats.get(a)), { x: before.x - 30, y: before.y, w: 230, h: 150 });
    // Esc is the content's: the window takes no key but its title bar's arrows
    win(a).querySelector("[data-input=a]").dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    bar.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await settle();
    assert.equal(floats.shown(floats.get(a)), true);
    await reset();
});

test("snap: an edge takes its half and a corner its quarter, with the outline before the drop; a drag out takes its size back", async () => {
    const id = floats.open("note", "a", { rect: { x: 300, y: 300, w: 400, h: 300 } });
    await settle();
    const bar = win(id).querySelector("[data-float-bar]");
    pointer(bar, "pointerdown", 400, 310);
    pointer(bar, "pointermove", 200, 400);
    pointer(bar, "pointermove", 5, 400);
    await settle();
    assert.deepEqual(floats.state.snap.rect, { x: 0, y: 50, w: 600, h: 800 });
    assert.equal($("[data-float-snap]").style.width, "600px", "the outline is drawn before the drop");
    pointer(bar, "pointerup", 5, 400);
    await settle();
    assert.deepEqual(rect(floats.get(id)), { x: 0, y: 50, w: 600, h: 800 });
    assert.equal($("[data-float-snap]"), null);
    // out of the half it is its own size again, under the pointer
    drag(bar, [300, 60], [500, 300]);
    assert.deepEqual([floats.get(id).w, floats.get(id).h], [400, 300]);
    // the corners
    assert.equal(floats.snapZone(1195, 120, floats.state.area), "top-right");
    assert.equal(floats.snapZone(1150, 845, floats.state.area), "bottom-right");
    assert.equal(floats.snapZone(600, 55, floats.state.area), "top");
    assert.equal(floats.snapZone(600, 400, floats.state.area), null);
    drag(bar, [600, 310], [1195, 60]);
    assert.deepEqual(rect(floats.get(id)), { x: 600, y: 50, w: 600, h: 400 });
    await reset();
});

test("a drag to the window's home docks it at the drop, with a Dock outline of the home before; a kind with no home snaps there", async () => {
    // a kind whose home is the right edge (as Assist's drawer and the tray), and one whose home is a strip (the Terminal's tabs)
    let away = false;
    floats.register("drawer", { title: "Drawer", defaultSize: { w: 300, h: 300 }, dock: (shown, key) => docked.push(["drawer", shown]),
        home: () => (away ? null : { edge: "right", rect: { x: 820, y: 50, w: 380, h: 800 } }) });
    floats.register("tabs", { title: "Tabs", defaultSize: { w: 300, h: 300 }, dock: (shown, key) => docked.push(["tabs", shown]),
        home: () => ({ rect: { x: 100, y: 60, w: 500, h: 40 } }) });
    const id = floats.open("drawer", "", { rect: { x: 300, y: 300, w: 300, h: 300 } });
    await settle();
    let bar = win(id).querySelector("[data-float-bar]");
    pointer(bar, "pointerdown", 400, 310);
    pointer(bar, "pointermove", 800, 400);
    pointer(bar, "pointermove", 1195, 400);
    await settle();
    assert.deepEqual(floats.state.snap, { id, zone: "home", rect: { x: 820, y: 50, w: 380, h: 800 } });
    assert.equal($("[data-float-snap]").dataset.floatSnapZone, "home");
    assert.equal($("[data-float-snap]").textContent, "Dock", "the outline says what the drop does");
    // back out of the edge before the drop: no dock
    pointer(bar, "pointermove", 700, 400);
    await settle();
    assert.equal(floats.state.snap, null);
    pointer(bar, "pointermove", 1195, 400);
    pointer(bar, "pointerup", 1195, 400);
    await settle();
    assert.equal(floats.get(id), null, "the drop docked it");
    assert.deepEqual(docked, [["drawer", true]], "docked shown, as the Dock button");
    assert.equal(floats.saved.places["drawer|"].x, 300, "the place kept is the one before the drag");
    // the corner still takes its quarter
    const again = floats.open("drawer", "", { rect: { x: 300, y: 300, w: 300, h: 300 } });
    await settle();
    drag(win(again).querySelector("[data-float-bar]"), [400, 310], [1195, 60]);
    assert.deepEqual(rect(floats.get(again)), { x: 600, y: 50, w: 600, h: 400 });
    // with no home now (the kind says null) the right edge snaps to the half
    away = true;
    drag(win(again).querySelector("[data-float-bar]"), [700, 60], [1195, 400]);
    assert.deepEqual(rect(floats.get(again)), { x: 600, y: 50, w: 600, h: 800 });
    // a home with no edge: the pointer over its rect
    const tabs = floats.open("tabs", "", { rect: { x: 300, y: 300, w: 300, h: 300 } });
    await settle();
    bar = win(tabs).querySelector("[data-float-bar]");
    drag(bar, [400, 310], [400, 450]);
    assert.ok(floats.get(tabs), "away from the strip it only moves");
    drag(bar, [400, 460], [300, 80]);
    assert.equal(floats.get(tabs), null);
    assert.deepEqual(docked.slice(-1), [["tabs", true]]);
    // a kind with no home snaps at the right edge, as before
    const note = floats.open("note", "a", { rect: { x: 300, y: 300, w: 400, h: 300 } });
    await settle();
    drag(win(note).querySelector("[data-float-bar]"), [400, 310], [1195, 400]);
    assert.deepEqual(rect(floats.get(note)), { x: 600, y: 50, w: 600, h: 800 });
    await reset();
});

test("a double click maximizes and restores; the title bar's arrows move and Shift resizes", async () => {
    const id = floats.open("note", "a", { rect: { x: 300, y: 300, w: 400, h: 300 } });
    await settle();
    const bar = win(id).querySelector("[data-float-bar]");
    bar.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    await settle();
    assert.equal(floats.get(id).maximized, true);
    assert.deepEqual([win(id).style.left, win(id).style.top, win(id).style.width, win(id).style.height], ["0px", "50px", "1200px", "800px"]);
    win(id).querySelector("[data-float-action=maximize]").click();
    await settle();
    assert.equal(floats.get(id).maximized, false);
    assert.equal(win(id).style.width, "400px");
    bar.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true, cancelable: true }));
    bar.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }));
    bar.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", shiftKey: true, bubbles: true, cancelable: true }));
    assert.deepEqual(rect(floats.get(id)), { x: 284, y: 316, w: 416, h: 300 });
    await reset();
});

test("tile, cascade, column at the right and minimize all; a chip shows its window again; toggle shows, raises and hides", async () => {
    const a = floats.open("note", "a"), b = floats.open("note", "b");
    floats.register("other", { title: "Other", dock() {} });
    const c = floats.open("other", "");
    await settle();
    floats.arrange("tile");
    // three: two in the first row, one across the second
    assert.deepEqual(rect(floats.get(a)), { x: 8, y: 58, w: 588, h: 388 });
    assert.deepEqual(rect(floats.get(b)), { x: 604, y: 58, w: 588, h: 388 });
    assert.deepEqual(rect(floats.get(c)), { x: 8, y: 454, w: 1184, h: 388 });
    floats.arrange("column");
    assert.deepEqual([a, b, c].map((id) => floats.get(id).x), [712, 712, 712]);
    assert.deepEqual([a, b, c].map((id) => floats.get(id).w), [480, 480, 480]);
    assert.equal(floats.get(c).y + floats.get(c).h <= 850, true);
    floats.arrange("cascade");
    assert.deepEqual([a, b, c].map((id) => [floats.get(id).x, floats.get(id).y]), [[8, 58], [40, 90], [72, 122]]);
    assert.equal(floats.isTop(floats.get(c)), true, "the last on top");
    floats.arrange("minimizeAll");
    await settle();
    // not drawn, but laid out still, so what it holds keeps its scroll
    assert.equal(getComputedStyle(win(a)).visibility, "hidden");
    assert.equal(win(a).getAttribute("aria-hidden"), "true");
    const chips = Array.from(document.querySelectorAll("[data-float-chip]"));
    assert.deepEqual(chips.map((chip) => chip.getAttribute("data-float-chip")), [a, b, c]);
    chips[1].click();
    await settle();
    assert.equal(floats.shown(floats.get(b)), true);
    assert.equal(floats.isTop(floats.get(b)), true);
    assert.equal(document.querySelectorAll("[data-float-chip]").length, 2);
    // toggle: the top one hides (still a float), a hidden one shows
    floats.toggle("note", "b");
    assert.equal(floats.get(b).hidden, true);
    assert.equal(floats.target("note", "b") !== null, true, "a hidden window keeps its body");
    floats.toggle("note", "b");
    assert.equal(floats.shown(floats.get(b)), true);
    // the Arrange menu is a button in the strip
    assert.equal($("#m-float-arrange").getAttribute("aria-label"), floats.labels.arrange);
    await reset();
});

test("places, sizes, order and minimized are kept in localStorage; a kind that asks at the next load floats again as it was", async () => {
    const a = floats.open("note", "a", { rect: { x: 100, y: 120, w: 420, h: 320 } });
    const b = floats.open("note", "b", { rect: { x: 500, y: 200, w: 300, h: 260 } });
    floats.minimize(b);
    floats.raise(a);
    const kept = JSON.parse(localStorage.getItem(moqui.scopedKey("moqui.floats")));
    assert.deepEqual(kept.floats.map((f) => [f.kind, f.key, f.x, f.y, f.w, f.h, f.minimized]),
        [["note", "a", 100, 120, 420, 320, false], ["note", "b", 500, 200, 300, 260, true]]);
    const za = floats.get(a).z, zb = floats.get(b).z;
    // a load: the windows are not open yet, the kind asks and opens them
    floats.state.floats.splice(0);
    await settle();
    floats.load(true);
    assert.equal(floats.wasFloated("note", "a"), true);
    assert.equal(floats.wasFloated("note", "c"), false);
    floats.open("note", "b", { restore: true });
    floats.open("note", "a", { restore: true });
    assert.deepEqual(rect(floats.get(a)), { x: 100, y: 120, w: 420, h: 320 });
    assert.equal(floats.get(b).minimized, true);
    assert.deepEqual([floats.get(a).z, floats.get(b).z], [za, zb], "the order kept");
    assert.equal(floats.wasFloated("note", "a"), false, "asked once: opened since");
    // a viewport smaller than the place kept: the window stays inside it
    floats.close(a);
    view.w = 700; view.h = 500;
    floats.onViewport();
    floats.open("note", "a");
    const f = floats.get(a);
    assert.equal(f.x + f.w <= 700 && f.y + f.h <= 500, true);
    // storage that throws changes nothing
    const setItem = Storage.prototype.setItem;
    Storage.prototype.setItem = () => { throw new Error("quota"); };
    try { floats.raise(b); } finally { Storage.prototype.setItem = setItem; }
    await reset();
});

test("nothing floats below 600 px or on a handheld screen: open refuses, and what floats docks closed", async () => {
    floats.open("note", "a");
    await settle();
    view.w = 500;
    floats.onViewport();
    await settle();
    assert.equal(floats.canFloat(), false);
    assert.deepEqual(docked, [["a", false]]);
    assert.equal(floats.open("note", "a"), false);
    assert.equal(floats.state.floats.length, 0);
    view.w = 1200;
    floats.onViewport();
    assert.equal(floats.canFloat(), true);
    floats.open("note", "b");
    await settle();
    vm.handheld = true;
    await settle();
    assert.equal(floats.canFloat(), false);
    assert.deepEqual(docked, [["a", false], ["b", false]]);
    assert.equal(floats.open("note", "b"), false);
    vm.handheld = false;
    await settle();
    assert.equal(floats.canFloat(), true);
    await reset();
});
