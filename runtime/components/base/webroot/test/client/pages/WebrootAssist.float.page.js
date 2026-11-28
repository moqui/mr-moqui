/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// Assist undocked (plans/archive/Terminal-Fancy.md §10.5, §10.7), in the page: Vue and Quasar, the shell's root
// options, the drawer in a q-layout and the float layer. The body is one instance in the drawer and in the float: its
// run, its draft and the thread's scroll stay across Undock and Dock; Mod+J and Esc act on whichever form is current;
// Assist never reaches its own body.
"use strict";
const assist = moqui.assist;
const floats = moqui.floats;
const qvt = moqui.qvt;
const s = assist.state;

floats.viewport = () => ({ w: 1440, h: 1000, top: 50 });
try { localStorage.removeItem("moqui.floats"); sessionStorage.clear(); } catch (err) { /* no storage */ }
floats.load(true);
floats.measure();

// a conversation with a run going and enough rows that the thread scrolls
s.available = true;
s.bootstrap = { agents: [{ agentId: "assist", name: "Assist" }], features: {} };
s.agentId = "assist";
s.off = { prompts: true };
const items = [];
for (let i = 1; i <= 40; i++) items.push({ messageId: "M" + i, seq: i, role: i % 2 ? "user" : "assistant", runId: "R1", clientMessageId: i % 2 ? "m-" + i : undefined,
    parts: [{ part: 1, kind: "text", text: "Line " + i + " of a conversation long enough to scroll the thread." }] });
assist.snapshot(s.convo, { conversation: { conversationId: "C1", title: "Long", agentId: "assist" },
    activeRun: { runId: "R1", state: "TASK_STATE_WORKING", statusId: "AgrWorking", lastEventSeq: 40, stepCount: 1 }, runs: [], items,
    pending: { approvals: [], questions: [], handoffs: [], queued: [] }, follow: { clientId: "tab-a", contextVersion: 1, following: true } }, { clientId: "tab-a", followOn: true });
s.convo.status = "Working";

const options = qvt.rootOptions({ basePath: "/apps", linkBasePath: "/qapps", appRootPath: "" });
const host = document.createElement("div");
document.body.appendChild(host);
// a page dialog (m-dialog) with a form, for Assist beside it (issue 206)
const app = Vue.createApp({ data() { return Object.assign(options.data.call(this), { testDialog: false }); }, computed: options.computed, methods: options.methods, provide: options.provide,
    template: '<q-layout view="hHh LpR fFf"><q-header id="top"><div style="height:50px"></div></q-header><m-shell-assist></m-shell-assist>' +
        '<q-page-container id="m-canvas"><q-page><form id="PageForm"><input name="title"></form>' +
        '<div data-assist-self="terminal"><form id="TermForm"><input name="cmd"></form></div>' +
        '<m-dialog id="TestDialog" title="Edit" width="500" v-model="testDialog"><form id="DialogForm"><input name="dname"></form></m-dialog></q-page></q-page-container></q-layout><m-float-layer></m-float-layer>' });
app.use(Quasar);
Object.keys(qvt.components).forEach((name) => app.component(name, qvt.components[name]));
app.config.globalProperties.moqui = moqui;
const vm = app.mount(host);
assist.vm = vm;
assist.registerFloat();

const settle = async () => {
    for (let i = 0; i < 4; i++) { await Vue.nextTick(); await new Promise((resolve) => setTimeout(resolve, 0)); }
};
const $ = (selector) => document.querySelector(selector);
const textarea = () => $("#m-assist-composer textarea");
const thread = () => $("#m-assist-thread");
const where = () => (thread().closest("#m-float-body-assist") ? "float" : thread().closest("#m-assist-drawer") ? "drawer" : "elsewhere");
const esc = (el) => el.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
// Esc as Quasar hears it: a keydown and a keyup with keyCode 27
const escBoth = (el) => ["keydown", "keyup"].forEach((type) => el.dispatchEvent(new KeyboardEvent(type, { key: "Escape", code: "Escape", keyCode: 27, bubbles: true, cancelable: true })));
const waitFor = async (check, what) => {
    for (let i = 0; i < 100; i++) { if (check()) return; await new Promise((resolve) => setTimeout(resolve, 20)); }
    throw new Error("timed out waiting for " + what);
};
// whether an element is drawn inside another's box, and what is on top at its middle
const inside = (el, box) => { const a = el.getBoundingClientRect(), b = box.getBoundingClientRect(); return a.top >= b.top - 1 && a.bottom <= b.bottom + 1; };
const onTop = (el) => { const r = el.getBoundingClientRect(); return document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2); };
const openDialog = async (open) => {
    vm.testDialog = open;
    await waitFor(() => s.pageModals === (open ? 1 : 0) && (open ? !!$("#DialogForm") : !$(".q-dialog#TestDialog")), open ? "the dialog shown" : "the dialog gone");
    await settle();
};

test("docked: the body is in the drawer, which pushes the page; Undock is offered beside Close", async () => {
    assist.openDrawer();
    await settle();
    assert.equal(where(), "drawer");
    assert.equal($("#m-assist-undock").nextElementSibling.id, "m-assist-close");
    assert.equal($(".m-assist-resize") !== null, true, "the width handle is the drawer's");
    assert.equal($(".q-page-container").style.paddingRight, "420px");
});

test("Undock and Dock keep the run, the draft and the thread's scroll; Close in either place docks it shut", async () => {
    s.draft = "a draft";
    await settle();
    const input = textarea(), list = thread();
    assert.equal(input.value, "a draft");
    assert.equal(list.scrollHeight > list.clientHeight + 200, true, "the thread scrolls");
    list.scrollTop = 150;
    list.dispatchEvent(new Event("scroll"));
    assert.deepEqual(assist.threadScroll, { top: 150, pinned: false });
    $("#m-assist-undock").click();
    await settle();
    assert.equal(where(), "float");
    assert.equal(s.open, false, "the drawer shut");
    assert.equal($(".q-page-container").style.paddingRight || "0px", "0px", "a shut drawer pushes nothing");
    const float = floats.find("assist", "");
    assert.deepEqual([float.x, float.y, float.w, float.h], [1440 - 420 - 8, 58, 420, 640]);
    assert.equal($("#m-float-assist").getAttribute("aria-label"), "Assist");
    assert.equal(textarea(), input, "the same composer, not made again");
    assert.equal(thread(), list);
    assert.equal(input.value, "a draft");
    assert.equal(list.scrollTop, 150, "the scroll put back after the move");
    assert.equal(s.convo.activeRunId, "R1");
    assert.equal($("#m-float-body-assist #m-assist-status").textContent.trim(), "Working");
    assert.equal($("#m-assist-undock"), null, "no Undock while floated");
    assert.equal($(".m-assist-resize"), null, "a float is sized by its frame");
    assert.equal(assist.shown(), true);
    // the float's Dock: back in the drawer, open
    floats.dock(float.id);
    await settle();
    assert.equal(where(), "drawer");
    assert.equal(s.open, true);
    assert.equal(textarea(), input);
    assert.equal(list.scrollTop, 150);
    // the float's Close: back, shut; Assist's own Close while floated is that Close
    $("#m-assist-undock").click();
    await settle();
    $("#m-float-assist [data-float-action=close]").click();
    await settle();
    assert.equal(where(), "drawer");
    assert.equal(s.open, false);
    assert.equal(floats.find("assist", ""), null);
    assist.openDrawer();
    $("#m-assist-undock").click();
    await settle();
    $("#m-assist-close").click();
    await settle();
    assert.equal(floats.find("assist", ""), null);
    assert.equal(s.open, false);
    assert.equal(where(), "drawer");
    assert.equal(input.value, "a draft");
});

test("Mod+J and the header button show and hide whichever form is current; a hidden float is still a float", async () => {
    // docked: from the page Mod+J opens, from inside it closes; the button toggles
    $("#PageForm input").focus();
    assist.toggle(true);
    await settle();
    assert.equal(s.open, true);
    textarea().focus();
    assist.toggle(true);
    assert.equal(s.open, false);
    assist.toggle(false);
    assert.equal(s.open, true);
    // floated
    assist.undock(false);
    await settle();
    const id = floats.find("assist", "").id;
    textarea().focus();
    assist.toggle(true);
    assert.equal(floats.get(id).hidden, true, "hidden from inside");
    assert.equal(s.open, false, "the drawer stays shut");
    assist.toggle(true);
    assert.equal(floats.shown(floats.get(id)), true, "shown again");
    $("#PageForm input").focus();
    assist.toggle(true);
    assert.equal(floats.shown(floats.get(id)), true, "from the page it stays shown, and takes the keys");
    assist.toggle(false);
    assert.equal(floats.get(id).hidden, true, "the header button hides it");
    assist.openDrawer(true);
    assert.equal(floats.shown(floats.get(id)), true, "anything that opens Assist shows the float");
    assert.equal(s.open, false);
    floats.close(id);
    await settle();
});

test("Esc: the first gives the keys to the page in both forms, the second closes the drawer or hides the float", async () => {
    assist.openDrawer();
    await settle();
    textarea().focus();
    esc(textarea());
    assert.equal(document.activeElement.id, "m-canvas");
    assert.equal(s.open, true);
    esc(textarea());
    assert.equal(s.open, false);
    assist.undock(false);
    await settle();
    const id = floats.find("assist", "").id;
    textarea().focus();
    esc(textarea());
    assert.equal(document.activeElement.id, "m-canvas");
    assert.equal(floats.shown(floats.get(id)), true);
    esc(textarea());
    assert.equal(floats.get(id).hidden, true);
    assert.equal(floats.find("assist", "") !== null, true, "hidden, still a float");
    floats.close(id);
    await settle();
});

test("reach: a floated Assist is out of it, a floated kind that says inReach is in, anything marked data-assist-self never", async () => {
    floats.register("note", { title: "Note", inReach: true, dock() {} });
    floats.open("note", "");
    assist.undock(false);
    await settle();
    const noteBody = document.getElementById("m-float-body-note");
    noteBody.innerHTML = '<form id="NoteForm"><input name="body"></form>';
    // a form of Assist's own, in its floated body
    const own = document.createElement("form");
    own.id = "OwnForm";
    own.innerHTML = '<input name="x">';
    $("#m-float-body-assist .m-assist-body").appendChild(own);
    assert.deepEqual(vm.assistReach, ["m-canvas", "m-float-body-note"], "the note's body, not Assist's");
    const fake = (id) => ({ $el: document.getElementById(id), $options: { name: "mForm" }, $attrs: { name: id }, fields: {}, fieldsOriginal: {} });
    const before = moqui.screenForms;
    moqui.screenForms = ["PageForm", "TermForm", "NoteForm", "OwnForm"].map(fake);
    try {
        const reach = {};
        assist.gatherForms().forEach((form) => { reach[form.name] = form.inReach; });
        assert.deepEqual(reach, { PageForm: true, TermForm: false, NoteForm: true, OwnForm: false });
        // docked, Assist's own form is still out: the mark is on the body, not the drawer's id
        floats.dock(floats.find("assist", "").id);
        await settle();
        assert.equal($("#m-assist-drawer #OwnForm") !== null, true);
        assert.equal(assist.gatherForms().find((form) => form.name === "OwnForm").inReach, false);
    } finally {
        moqui.screenForms = before;
        own.remove();
        floats.dockAll(false);
        await settle();
    }
});

test("restored at a load: floated as it was, and docked when it was not", async () => {
    assist.undock(false);
    await settle();
    floats.hide(floats.find("assist", "").id);
    // a load: no windows yet, the kept state read, Assist registers and asks
    floats.state.floats.splice(0);
    await settle();
    floats.load(true);
    assert.equal(floats.wasFloated("assist", ""), true);
    assert.equal(assist.undock(true), true);
    await settle();
    assert.equal(floats.find("assist", "").hidden, true, "hidden as it was");
    assert.equal(where(), "float");
    floats.close(floats.find("assist", "").id);
    await settle();
    floats.load(true);
    assert.equal(floats.wasFloated("assist", ""), false);
});

test("issue 206: a new approval is scrolled into view though the user scrolled up; the status line goes back to it", async () => {
    assist.openDrawer();
    await settle();
    const list = thread();
    list.scrollTop = 0;
    list.dispatchEvent(new Event("scroll"));
    assert.equal(assist.threadScroll.pinned, false, "scrolled up");
    let seq = s.convo.seq.R1 || 0;
    const next = (type, payload) => assist.apply(s.convo, "R1", { seq: ++seq, type, payload }, { clientId: "tab-a", followOn: true });
    next("message.start", { messageId: "M41" });
    next("tool.call", { toolCallId: "T1", messageId: "M41", name: "screen", arguments: { action: "save" }, classification: "write" });
    await settle();
    list.scrollTop = 0;
    list.dispatchEvent(new Event("scroll"));
    // the step was there before its approval: the rows do not change, the card comes under it
    next("approval.requested", { approvalId: "AP1", runId: "R1", toolCallId: "T1", classification: "write", targetKind: "screen", summary: "Save order",
        changes: [{ field: "name", label: "Name", current: "A", proposed: "B" }], canEdit: false, canPreview: false });
    next("run.state", { state: "TASK_STATE_AUTH_REQUIRED", pauseKind: "AgpsApproval", pauseRef: "AP1" });
    s.convo.status = assist.statusOf(s.convo);
    await settle();
    const card = $('#m-assist-thread [data-approval-id="AP1"]');
    assert.equal(!!card, true);
    assert.equal(inside(card, list), true, "the card in view");
    assert.equal(list.scrollTop > 0, true);
    // scrolled away again, the status line brings it back and gives Approve the keys
    list.scrollTop = 0;
    list.dispatchEvent(new Event("scroll"));
    assert.equal(inside(card, list), false);
    const status = $("#m-assist-status");
    assert.equal(status.getAttribute("role"), "button");
    status.click();
    await settle();
    assert.equal(inside(card, list), true, "back in view");
    assert.equal(document.activeElement, card.querySelector('[data-assist-action="approve"]'));
    next("approval.decided", { approvalId: "AP1", toolCallId: "T1", decision: "approve", status: "approved" });
    next("run.state", { state: "TASK_STATE_WORKING" });
    s.convo.status = "Working";
    await settle();
    assert.equal($("#m-assist-status").getAttribute("role"), null, "nothing to go to");
});

test("issue 206: docked, a page dialog leaves the drawer's strip, the focus may go to Assist, and Esc there gives it back", async () => {
    assist.openDrawer();
    await settle();
    await openDialog(true);
    const dialog = $(".q-dialog#TestDialog");
    assert.equal(dialog.classList.contains("m-dialog--beside-assist"), true);
    assert.equal(dialog.getAttribute("aria-modal"), "false");
    const drawer = $("#m-assist-drawer");
    assert.equal(dialog.querySelector(".q-card").getBoundingClientRect().right <= drawer.getBoundingClientRect().left + 1, true, "the dialog left of the drawer");
    assert.equal(dialog.querySelector(".q-dialog__backdrop").getBoundingClientRect().right <= drawer.getBoundingClientRect().left + 1, true, "so is its backdrop");
    assert.equal(drawer.contains(onTop(textarea())), true, "the composer is not under the backdrop");
    assert.equal(drawer.contains(onTop($("#m-assist-new"))), true);
    textarea().focus();
    await settle();
    assert.equal(document.activeElement, textarea(), "the dialog does not take the focus back");
    escBoth(textarea());
    await settle();
    assert.equal(vm.testDialog, true, "Esc in Assist leaves the dialog open");
    assert.equal(document.activeElement, $("#DialogForm input"), "and gives it the keys");
    await openDialog(false);
    assert.equal(s.open, true, "the drawer stays open");
});

test("issue 206: floated, the float layer goes over a page dialog with the other floats hidden, then back", async () => {
    floats.register("note", { title: "Note", dock() {} });
    floats.open("note", "");
    assist.undock(false);
    await settle();
    const note = $("#m-float-note");
    assert.equal(note.classList.contains("m-float--hidden"), false);
    await openDialog(true);
    const layer = $("[data-float-layer]");
    assert.equal(layer.classList.contains("m-float-layer--over-modal"), true);
    assert.equal(getComputedStyle(layer).zIndex, "6001");
    assert.equal(document.body.classList.contains("m-float-over-modal"), true);
    assert.equal(note.classList.contains("m-float--hidden"), true, "the other float waits hidden");
    assert.equal($(".q-dialog#TestDialog").getAttribute("aria-modal"), "false");
    assert.equal($(".q-dialog#TestDialog").classList.contains("m-dialog--beside-assist"), false, "a float leaves the dialog where it is");
    assert.equal($("#m-float-assist").contains(onTop(textarea())), true, "the composer is over the dialog");
    textarea().focus();
    await settle();
    assert.equal(document.activeElement, textarea());
    await openDialog(false);
    assert.equal(layer.classList.contains("m-float-layer--over-modal"), false);
    assert.equal(document.body.classList.contains("m-float-over-modal"), false);
    assert.equal(note.classList.contains("m-float--hidden"), false, "shown again where it was");
    floats.dockAll(false);
    await settle();
});

test("issue 206: with Assist shut a page dialog is modal as before", async () => {
    assist.closeDrawer();
    await settle();
    await openDialog(true);
    const dialog = $(".q-dialog#TestDialog");
    assert.equal(dialog.getAttribute("aria-modal"), "true");
    assert.equal(dialog.classList.contains("m-dialog--beside-assist"), false);
    assert.equal(assist.beside(), null);
    await openDialog(false);
});
