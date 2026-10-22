/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// Tests of the suite shell in WebrootVue.qvt.js that need no browser: the palette's prefixes, ranking and groups, the key map
// (chords, scopes, the KEYMAP overrides and the rule that a text input keeps single keys), the forms of the screen context,
// and the root's shell data: the + menu, the tray tabs, the header counters, the notification topics and the quick-create
// dialog closing on a submit. What needs a browser is in crates/moqui-host/tests/gate3_browser.rs (the shell smoke).
// Run: node --test runtime/base-component/webroot/screen/webroot/js/WebrootVue.qvt.shell.test.js
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");

globalThis.moqui = require("./MoquiLib.js");
globalThis.moqui.l10n = require("./MoquiL10n.js");
require("./WebrootVue.qvt.js");
const qvt = moqui.qvt;
const palette = moqui.shellPalette;
const keymap = moqui.keymap;

const sources = [{ group: "Records", label: "Records", prefix: null }, { group: "People", label: "People", prefix: "@" },
    { group: "Rooms", label: "Messages and rooms", prefix: "#" }, { group: "GalleryEnumerations", label: "Enumerations", prefix: "~" },
    { group: "Contacts", label: "Contacts", prefix: "@" }];

test("the palette's prefixes: > actions, : screens, a source's prefix its groups, else everything", () => {
    assert.deepEqual(palette.parse(">  new task", sources), { text: "new task", mode: "actions", prefix: ">", groups: [] });
    assert.deepEqual(palette.parse(":orders", sources), { text: "orders", mode: "screens", prefix: ":", groups: [] });
    assert.deepEqual(palette.parse("@John", sources), { text: "John", mode: "sources", prefix: "@", groups: ["People", "Contacts"] });
    assert.deepEqual(palette.parse("#general", sources), { text: "general", mode: "sources", prefix: "#", groups: ["Rooms"] });
    assert.deepEqual(palette.parse(" ~quasar ", sources), { text: "quasar", mode: "sources", prefix: "~", groups: ["GalleryEnumerations"] });
    assert.deepEqual(palette.parse("  review open  ", sources), { text: "review open", mode: "all", prefix: "", groups: [] });
    assert.deepEqual(palette.parse(null, sources), { text: "", mode: "all", prefix: "", groups: [] });
    // a prefix no source has is text; the longest prefix wins
    assert.equal(palette.parse("!x", sources).mode, "all");
    assert.deepEqual(palette.parse("@@x", sources.concat([{ group: "Twice", prefix: "@@" }])).groups, ["Twice"]);
});

test("screens rank by title: the whole title, its start, word starts, then anywhere; ties keep the menu order", () => {
    assert.equal(palette.score("Orders", "orders"), 0);
    assert.equal(palette.score("Order Detail", "order"), 1);
    assert.equal(palette.score("Sales Order Detail", "ord det"), 2);
    assert.equal(palette.score("Reorder points", "order"), 3);
    assert.equal(palette.score("Customers", "order"), -1);
    assert.equal(palette.score("Anything", ""), 0);
    const list = [{ title: "Reorder points" }, { title: "Customers" }, { title: "Sales Orders" }, { title: "Orders" }, { title: "Order Detail" }];
    assert.deepEqual(palette.rank(list, "order").map((item) => item.title), ["Orders", "Order Detail", "Sales Orders", "Reorder points"]);
    assert.deepEqual(palette.rank(list, "orders").map((item) => item.title), ["Orders", "Sales Orders"]);
    assert.equal(palette.rank(list, "").length, 5);
});

const apps = [
    { name: "Commons", title: "IronStone Commons", path: "/qapps/Commons", image: "sym_o_hub", imageType: "icon", menu: [
        { title: "Home", path: "/qapps/Commons/Home", depth: 1 },
        { title: "Contacts", path: "/qapps/Commons/Contacts", image: "sym_o_contacts", imageType: "icon", depth: 1 },
        { title: "Duplicates", path: "/qapps/Commons/Contacts/Duplicates", depth: 2 },
        { title: "Tasks", path: "/qapps/Commons/Tasks", depth: 1 }],
      providers: { quickCreate: [{ kind: "task", label: "Task", icon: "sym_o_task_alt", url: "/qapps/Commons/QuickCreate?kind=task", requiresRecord: false },
          { kind: "activity", label: "Activity", url: "/qapps/Commons/QuickCreate?kind=activity", requiresRecord: true }],
          paletteActions: [{ label: "New event", icon: "sym_o_event", url: "/qapps/Commons/Calendar?new=true" }],
          paletteSources: [{ group: "Records", label: "Records", prefix: null }, { group: "People", label: "People", prefix: "@" }],
          accountMenuItems: [{ label: "Preferences", icon: "sym_o_tune", url: "/qapps/Commons/Settings/Preferences" }],
          notificationTopics: ["ChatterMention"],
          keymaps: [{ scope: "global", keys: [{ keys: "g c", label: "Contacts", url: "/qapps/Commons/Contacts", action: null }] }] } },
    { name: "gallery", title: "Gallery", path: "/qapps/gallery", menu: [{ title: "Widgets", path: "/qapps/gallery/Widgets", depth: 1 }],
      providers: { quickCreate: [{ kind: "galleryNote", label: "Gallery note", url: "/qapps/gallery/QuickNote", requiresRecord: false }],
          paletteActions: [{ label: "Open the widget gallery", url: "/qapps/gallery/Widgets" }],
          paletteSources: [{ group: "GalleryEnumerations", label: "Enumerations", prefix: "~" }, { group: "Records", label: "Records again" }],
          accountMenuItems: [{ label: "Preferences again", url: "/qapps/Commons/Settings/Preferences" }],
          keymaps: [{ scope: "gallery", keys: [{ keys: "g w", label: "Go to the widgets", url: "/qapps/gallery/Widgets" }] }] } }];

test("the Screens rows are the app roots and their menus with the app and the parent as the subtitle", () => {
    const rows = palette.screens(apps);
    assert.deepEqual(rows.map((row) => row.title), ["IronStone Commons", "Home", "Contacts", "Duplicates", "Tasks", "Gallery", "Widgets"]);
    assert.equal(rows[0].url, "/qapps/Commons");
    assert.equal(rows[2].subtitle, "IronStone Commons");
    assert.equal(rows[3].subtitle, "IronStone Commons › Contacts");
    assert.equal(rows[4].subtitle, "IronStone Commons");
    assert.equal(rows[2].image, "sym_o_contacts");
});

test("the groups: Recent before typing; Screens, Saved finds, Actions, then the provider groups, five each with more", () => {
    const labels = { recent: "Recent", screens: "Screens", savedFinds: "Saved finds", actions: "Actions", more: "More" };
    const screens = palette.screens(apps).concat([1, 2, 3, 4, 5, 6].map((n) => ({ title: "Contacts " + n, url: "/qapps/c" + n })));
    const input = { labels, recent: [{ title: "Tasks", url: "/qapps/Commons/Tasks" }], screens,
        savedFinds: [{ title: "My open contacts", url: "/qapps/Commons/Contacts?formListFindId=1" }],
        actions: [{ title: "New contact", url: "/qapps/Commons/Contacts?new=true" }, { title: "New event", url: "/qapps/Commons/Calendar?new=true" }],
        remote: [{ group: "Records", label: "Records", icon: "sym_o_search", rows: [1, 2, 3, 4, 5, 6].map((n) => ({ title: "Contact record " + n, url: "/qapps/r" + n })),
            moreUrl: "/qapps/Commons/Search?q=contact" },
            { group: "People", label: "People", rows: [{ title: "John Doe", subtitle: "Person", url: "/qapps/Commons/Contacts/ContactDetail?partyId=EX_JOHN_DOE" },
                { title: "No link" }] }] };
    const before = palette.groups(palette.parse("", sources), input);
    assert.deepEqual(before.map((group) => group.key), ["recent"]);
    const groups = palette.groups(palette.parse("contact", sources), input);
    assert.deepEqual(groups.map((group) => group.key), ["screens", "savedFinds", "actions", "source:Records", "source:People"]);
    assert.equal(groups[0].rows.length, 5);
    assert.equal(groups[0].rows[0].title, "Contacts");
    // a provider group: its five rows and a last row to the more link; a row without a link is left out
    assert.equal(groups[3].rows.length, 6);
    assert.deepEqual(groups[3].rows[5], { more: true, title: "More: Records", url: "/qapps/Commons/Search?q=contact", icon: "sym_o_arrow_forward", index: 12 });
    assert.equal(groups[3].rows[0].icon, "sym_o_search");
    assert.deepEqual(groups[4].rows.map((row) => row.title), ["John Doe"]);
    // the index of each row in the whole list, for the arrow keys
    assert.deepEqual(palette.rows(groups).map((row) => row.index), Array.from({ length: 14 }, (v, i) => i));
    // the input is not changed
    assert.equal(input.remote[0].rows[0].index, undefined);

    // > is the actions alone, : the screens alone with up to 25 rows, a prefix only its sources' groups
    assert.deepEqual(palette.groups(palette.parse(">", sources), input).map((group) => [group.key, group.rows.length]), [["actions", 2]]);
    assert.deepEqual(palette.groups(palette.parse(">event", sources), input)[0].rows.map((row) => row.title), ["New event"]);
    assert.deepEqual(palette.groups(palette.parse(":contacts", sources), input).map((group) => [group.key, group.rows.length]), [["screens", 7]]);
    assert.deepEqual(palette.groups(palette.parse("@john", sources), input).map((group) => group.key), ["source:People"]);
    assert.deepEqual(palette.groups(palette.parse("zzz", sources), Object.assign({}, input, { remote: [] })), []);
});

const key = (k, extra) => Object.assign({ key: k, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, target: { tagName: "BODY" },
    preventDefault() { this.prevented = true; } }, extra);

test("key texts parse into steps, and a step matches a keydown as the keyboard sends it", () => {
    assert.deepEqual(keymap.parse("Mod+K"), [{ key: "k", ctrl: false, alt: false, shift: false, meta: false, mod: true }]);
    assert.deepEqual(keymap.parse("g c").map((step) => step.key), ["g", "c"]);
    assert.deepEqual(keymap.parse("Ctrl+Shift+A")[0], { key: "a", ctrl: true, alt: false, shift: true, meta: false, mod: false });
    assert.equal(keymap.parse("Esc")[0].key, "escape");
    assert.equal(keymap.parse("Ctrl++")[0].key, "+");
    assert.equal(keymap.parse("Ctrl++")[0].ctrl, true);
    assert.deepEqual(keymap.parse(""), []);
    const [modK] = keymap.parse("Mod+K");
    assert.equal(keymap.matches(modK, key("k", { ctrlKey: true })), true);
    assert.equal(keymap.matches(modK, key("K", { metaKey: true })), true);
    assert.equal(keymap.matches(modK, key("k")), false);
    const [ctrlK] = keymap.parse("Ctrl+K");
    assert.equal(keymap.matches(ctrlK, key("k", { metaKey: true })), false);
    // a letter needs Shift only when the step has it; ? is typed with Shift and matches without saying so
    assert.equal(keymap.matches(keymap.parse("c")[0], key("C", { shiftKey: true })), false);
    assert.equal(keymap.matches(keymap.parse("Shift+C")[0], key("C", { shiftKey: true })), true);
    assert.equal(keymap.matches(keymap.parse("?")[0], key("?", { shiftKey: true })), true);
    assert.equal(keymap.matches(keymap.parse("Esc")[0], key("Escape")), true);
    assert.equal(keymap.format("Mod+K"), keymap.isMac ? "⌘+K" : "Ctrl+K");
});

test("the key map runs a binding, a chord within its time, a screen's scope before the global one, and the input rule", () => {
    keymap.scopes.length = 0;
    keymap.setOverrides(null);
    const ran = [];
    const offGlobal = keymap.registerScope("global", [
        { keys: "Mod+K", action: "palette", label: "Search", notInEditor: true, run: () => { ran.push("palette"); } },
        { keys: "g c", label: "Contacts", run: () => { ran.push("contacts"); } },
        { keys: "c", action: "quickCreate", label: "Create", run: () => { ran.push("create"); } },
        { keys: "x", label: "Unused", run: () => false },
        { keys: "?", action: "help", label: "Keys", run: () => { ran.push("help"); } }]);
    const event = key("k", { ctrlKey: true });
    assert.equal(keymap.handle(event, 1000), true);
    assert.equal(event.prevented, true);
    assert.deepEqual(ran, ["palette"]);
    // g then c: the chord; c alone is the create key
    assert.equal(keymap.handle(key("g"), 2000), true);
    assert.equal(keymap.handle(key("c"), 2500), true);
    assert.equal(keymap.handle(key("c"), 3000), true);
    assert.deepEqual(ran, ["palette", "contacts", "create"]);
    // the chord's time runs out: c is the create key again
    keymap.handle(key("g"), 4000);
    keymap.handle(key("c"), 4000 + keymap.CHORD_MS + 1);
    assert.deepEqual(ran.slice(-1), ["create"]);
    // a binding whose run answers false did not use the key
    const unused = key("x");
    assert.equal(keymap.handle(unused, 9000), false);
    assert.equal(unused.prevented, undefined);
    // a text input keeps single keys, not Ctrl+K; a rich-text editor keeps Ctrl+K too
    ran.length = 0;
    const input = { tagName: "INPUT", type: "text" };
    assert.equal(keymap.handle(key("c", { target: input }), 10000), false);
    assert.equal(keymap.handle(key("?", { shiftKey: true, target: { tagName: "TEXTAREA" } }), 10000), false);
    assert.equal(keymap.handle(key("k", { ctrlKey: true, target: input }), 10000), true);
    assert.equal(keymap.handle(key("k", { ctrlKey: true, target: { tagName: "DIV", isContentEditable: true } }), 10000), false);
    assert.equal(keymap.handle(key("c", { target: { tagName: "INPUT", type: "checkbox" } }), 10000), true);
    assert.deepEqual(ran, ["palette", "create"]);
    // a screen's scope wins while it is registered, and its unregister function takes it back
    const offInbox = keymap.registerScope("inbox", [{ keys: "c", action: "compose", label: "Compose", run: () => { ran.push("compose"); } },
        { keys: "g t", label: "Sent", run: () => { ran.push("sent"); } }]);
    keymap.handle(key("c"), 11000);
    keymap.handle(key("g"), 11100);
    keymap.handle(key("t"), 11200);
    keymap.handle(key("g"), 11300);
    keymap.handle(key("c"), 11400);
    assert.deepEqual(ran.slice(-3), ["compose", "sent", "contacts"]);
    offInbox();
    keymap.handle(key("c"), 12000);
    assert.deepEqual(ran.slice(-1), ["create"]);
    // a binding that is off (when) is not tried, and the typed keys wait while the shell says so (a dialog is open)
    keymap.typedBlocked = () => true;
    assert.equal(keymap.handle(key("c"), 13000), false);
    assert.equal(keymap.handle(key("k", { ctrlKey: true }), 13000), true);
    keymap.typedBlocked = null;
    const offWhen = keymap.registerScope("app", [{ keys: "c", label: "Off", when: () => false, run: () => { ran.push("off"); } }]);
    keymap.handle(key("c"), 14000);
    assert.notEqual(ran[ran.length - 1], "off");
    offWhen();
    // the ? overlay lists the bindings with a label, the keys as shown
    const described = keymap.describe();
    assert.deepEqual(described.find((entry) => entry.label === "Search"), { scope: "global", keys: keymap.isMac ? "⌘+K" : "Ctrl+K", label: "Search" });
    assert.equal(described.filter((entry) => entry.scope === "global").length, 5);
    offGlobal();
    assert.deepEqual(keymap.scopes, []);
});

test("the KEYMAP preference changes a binding's keys by scope and action, else by its keys", () => {
    keymap.scopes.length = 0;
    const ran = [];
    assert.equal(keymap.setOverrides('{"global": {"palette": "Mod+P", "g c": "g x", "help": 4}, "inbox": "no"}'), true);
    assert.deepEqual(keymap.overrides, { global: { palette: "Mod+P", "g c": "g x" } });
    const off = keymap.registerScope("global", [{ keys: "Mod+K", action: "palette", label: "Search", run: () => { ran.push("palette"); } },
        { keys: "g c", label: "Contacts", run: () => { ran.push("contacts"); } }]);
    assert.equal(keymap.handle(key("k", { ctrlKey: true }), 1000), false);
    assert.equal(keymap.handle(key("p", { ctrlKey: true }), 1000), true);
    keymap.handle(key("g"), 2000);
    keymap.handle(key("x"), 2100);
    assert.deepEqual(ran, ["palette", "contacts"]);
    assert.equal(keymap.describe().find((entry) => entry.label === "Contacts").keys, "g x");
    // a value that is not JSON of a map is ignored, and said so
    assert.equal(keymap.setOverrides("{not json"), false);
    assert.deepEqual(keymap.overrides, {});
    assert.equal(keymap.setOverrides("[1]"), false);
    assert.equal(keymap.setOverrides(""), true);
    off();
});

test("the forms of the screen context: forms with their fields, lists with rows and find fields, filters and the selection", () => {
    const form = (name, type, fields, extra) => Object.assign({ $options: { name: type }, $attrs: { name, id: name }, fields }, extra);
    const tables = ["EnumList", "MultiList"];
    const sel = { $options: { name: "mCheckboxSet" }, checkboxValues: ["A", "B", "C"], checkboxStates: [true, false, true],
        getCheckboxValueArray() { return this.checkboxValues.filter((v, i) => this.checkboxStates[i]); } };
    const vms = [
        form("EditEnum", "mForm", { enumId: "A", description: "One", moquiFormName: "EditEnum" }),
        form("EnumList_header", "mFormLink", { description: "", description_op: "contains", enumTypeId: "", pageIndex: "0" }),
        form("EnumList_0", "mForm", { enumId: "A", sequenceNum: "1" }),
        form("EnumList_1", "mForm", { enumId: "B", sequenceNum: "2", moquiSessionToken: "t" }),
        form("FindOrders", "mFormLink", { orderId: "", statusId: "" }),
        form("MultiList", "mForm", { enumId_0: "A", enumId_1: "B", sequenceNum_0: "1", _isMulti: "true" }),
        { $options: { name: "mFormList" }, name: "ClientList", rowList: [{ a: 1, b: 2 }, { a: 3 }], selectedRows() { return [this.rowList[1]]; } },
        sel,
        form("", "mForm", { x: "1" })];
    const dom = { hasTable: (name) => tables.indexOf(name) >= 0, holds: (vm, name) => vm === sel && name === "EnumList" };
    const params = { description: "Quasar", description_op: "contains", enumTypeId: "", pageIndex: "2", statusId: "OrderOpen", other: "x" };
    const out = moqui.screenFormsContext(vms, dom, params, "EnumList");
    assert.deepEqual(out.forms, [
        { name: "EditEnum", kind: "form", fields: ["enumId", "description"] },
        { name: "EnumList", kind: "list", fields: ["enumId", "sequenceNum"], findFields: ["description", "description_op", "enumTypeId"], rows: 2 },
        { name: "FindOrders", kind: "find", fields: ["orderId", "statusId"] },
        { name: "MultiList", kind: "list", fields: ["enumId", "sequenceNum"], rows: 2 },
        { name: "ClientList", kind: "list", fields: ["a", "b"], rows: 2 }]);
    assert.deepEqual(out.filters, [{ form: "EnumList", parameters: { description: "Quasar", description_op: "contains" } },
        { form: "FindOrders", parameters: { statusId: "OrderOpen" } }]);
    assert.deepEqual(out.selection, { form: "EnumList", ids: ["A", "C"] });
    // a client list's selection is its rows; no selection form, no selection; no find parameters, no filters
    assert.deepEqual(moqui.screenFormsContext(vms, dom, {}, "ClientList").selection, { form: "ClientList", ids: [], rows: [{ a: 3 }] });
    const none = moqui.screenFormsContext(vms, dom, {}, "");
    assert.equal(none.selection, null);
    assert.equal(none.filters, null);
    assert.deepEqual(moqui.screenFormsContext([], null, {}, ""), { forms: [], filters: null, selection: null });
});

// The root's options as a plain object, with its computed properties as getters over it.
const makeRoot = (config, extra) => {
    const options = qvt.rootOptions(Object.assign({ basePath: "/apps", linkBasePath: "/qapps", appRootPath: "" }, config));
    const vm = Object.assign(options.data(), { currentPath: "/apps/Commons/Contacts", currentParameters: {} }, extra);
    vm.labels = Object.assign({}, qvt.shellLabels, vm.labels);
    Object.keys(options.methods).forEach((name) => { vm[name] = options.methods[name].bind(vm); });
    Object.keys(options.computed).forEach((name) => {
        const computed = options.computed[name];
        if (name === "currentPath") return;
        Object.defineProperty(vm, name, { get: () => (typeof computed === "function" ? computed : computed.get).call(vm), configurable: true });
    });
    return vm;
};

test("the root's shell: the current app and its title, the + menu, the account menu, the tray tabs and the header counters", () => {
    const vm = makeRoot({ shell: { title: "Theme title" } }, { currentPathList: ["gallery", "Detail"] });
    assert.equal(vm.headerTitle, "Theme title", "no shell data yet");
    assert.deepEqual(vm.quickCreateItems, []);
    assert.deepEqual(vm.trayTabs, []);
    assert.deepEqual(vm.headerCounters, []);
    vm.shellData = { apps, shell: { trayToday: "/qapps/Commons/Panel/Today", trayChat: null, trayNotifications: "/qapps/Commons/Panel/Notifications",
        mailScreen: "/qapps/Comms/Inbox", messagesScreen: "/qapps/Comms/Messages", quickCreate: "/qapps/Commons/QuickCreate" },
        counts: { today: 3, email: { total: 120 }, messages: 2, notifications: 0, updates: 7 } };
    vm.shellCounts = vm.shellData.counts;
    assert.equal(vm.currentApp.name, "gallery");
    assert.equal(vm.headerTitle, "Gallery");
    // the breadcrumbs start below the app root, whose title the header shows; outside an app, below the apps root
    assert.equal(vm.breadcrumbStart, 2);
    // the title's menu is that app-root entry: its subscreens are the top-level screens. A tab menu, and the AppList, have none.
    vm.navMenuList = [
        { name: "apps", title: "Applications", subscreens: [{ name: "gallery", title: "Gallery" }] },
        { name: "gallery", title: "Gallery", subscreens: [
            { name: "Widgets", title: "Widgets", pathWithParams: "/apps/gallery/Widgets" },
            { name: "Detail", title: "Detail", pathWithParams: "/apps/gallery/Detail", active: true }] }];
    assert.equal(vm.appScreenMenu.name, "gallery");
    assert.deepEqual(vm.appScreenMenu.subscreens.map((item) => item.name), ["Widgets", "Detail"]);
    vm.navMenuList[1].hasTabMenu = true;
    assert.equal(vm.appScreenMenu, null);
    vm.currentPathList = ["AppList"];
    assert.equal(vm.breadcrumbStart, 1);
    assert.equal(vm.appScreenMenu, null);
    vm.currentPathList = ["gallery", "Detail"];
    vm.navMenuList[1].hasTabMenu = false;
    // the current app's kinds first; a kind that needs a record only with one in context
    assert.deepEqual(vm.quickCreateItems.map((item) => item.label), ["Gallery note", "Task"]);
    assert.deepEqual(vm.currentAppQuickCreate.map((item) => item.label), ["Gallery note"]);
    vm.screenContexts.push({ key: "c1", value: { recordEntity: "moqui.basic.Enumeration", recordId: "STT_INTERNAL", title: "Internal" } });
    assert.deepEqual(vm.quickCreateItems.map((item) => item.label), ["Gallery note", "Task", "Activity"]);
    assert.deepEqual(vm.accountMenuItems.map((item) => item.label), ["Preferences again"]);
    assert.deepEqual(vm.paletteSources.map((source) => source.group), ["Records", "People", "GalleryEnumerations"]);
    assert.deepEqual(vm.paletteActions.map((action) => action.title), ["Open the widget gallery", "New event"]);
    // the tray: a tab for each screen the theme names, loaded from /apps; the counters in the header's order
    assert.deepEqual(vm.trayTabs.map((tab) => [tab.key, tab.loadUrl]), [["today", "/apps/Commons/Panel/Today"], ["notifications", "/apps/Commons/Panel/Notifications"]]);
    assert.deepEqual(vm.headerCounters.map((counter) => [counter.key, counter.tray, counter.url || null]), [["today", true, null],
        ["email", false, "/qapps/Comms/Inbox"], ["messages", false, "/qapps/Comms/Messages"], ["notifications", true, null]]);
    assert.equal(vm.countOf("today"), 3);
    assert.equal(vm.countText("email"), "99+");
    assert.deepEqual(vm.headerCounters.map((counter) => vm.counterText(counter)), ["3", "99+", "2", "0"]);
    assert.equal(vm.countOf("notifications"), 0);
    assert.equal(vm.countOf("chat"), 0);
    // below 600 px Mail and Messages are one button to the mail screen with the counts summed, and the tray tabs one tray button
    // with theirs; the updates have no button
    vm.$q = { screen: { lt: { sm: true } } };
    vm.shellCounts = { email: 3, messages: 2, today: 1, notifications: 4 };
    assert.deepEqual(vm.headerCounters.map((counter) => counter.key), ["tray", "mail"]);
    assert.deepEqual(vm.headerCounters[0].counts, ["today", "notifications"]);
    assert.equal(vm.headerCounters[0].label, "Today, Notifications");
    assert.equal(vm.headerCounters[0].icon, "sym_o_notifications");
    assert.equal(vm.counterText(vm.headerCounters[0]), "5");
    assert.equal(vm.counterText(vm.headerCounters[1]), "5");
    assert.equal(vm.headerCounters[1].url, "/qapps/Comms/Inbox");
    vm.shellData.shell.mailScreen = null;
    assert.deepEqual(vm.headerCounters.map((counter) => counter.key), ["tray", "messages"], "Messages alone stays");
    vm.shellData.shell.mailScreen = "/qapps/Comms/Inbox";
    // the tray button opens the tray on its last tab, else the first, and closes it
    vm.toggleTray(null);
    assert.equal(vm.trayOpen, true);
    assert.equal(vm.trayTab, "today");
    vm.toggleTray(null);
    assert.equal(vm.trayOpen, false);
    vm.trayTab = "notifications";
    vm.toggleTray(null);
    assert.equal(vm.trayTab, "notifications");
    vm.closeTray();
    vm.trayTab = null;
    vm.trayLoaded = {};
    // one tray tab alone keeps its own button
    const trayChat = vm.shellData.shell.trayToday;
    vm.shellData.shell.trayToday = null;
    assert.deepEqual(vm.headerCounters.map((counter) => counter.key), ["mail", "notifications"]);
    vm.shellData.shell.trayToday = trayChat;
    delete vm.$q;
    vm.shellCounts = vm.shellData.counts;
    // the tray opens on a tab it has, keeps it loaded, and closes; Esc closes it when no dialog or menu is open
    vm.openTray("chat");
    assert.equal(vm.trayOpen, false);
    vm.openTray("today");
    assert.equal(vm.trayOpen, true);
    assert.equal(vm.trayLoaded.today, true);
    assert.deepEqual(vm.assistReach, ["m-canvas", "m-tray"]);
    vm.toggleTray("notifications");
    assert.equal(vm.trayTab, "notifications");
    assert.equal(vm.closeTopmost(), true);
    assert.equal(vm.trayOpen, false);
    assert.equal(vm.closeTopmost(), false);
    assert.equal(vm.trayLoaded.today, true, "a tab stays loaded");
});

test("the root's shell: the notification topics set the counts and reload their tab, and quick create gets the record", () => {
    const vm = makeRoot({}, { currentPathList: ["Commons", "Contacts"] });
    vm.shellData = { apps, shell: { trayToday: "/qapps/Commons/Panel/Today", trayChat: "/qapps/Comms/Panel/Chat", trayNotifications: "/qapps/Commons/Panel/Notifications" }, counts: {} };
    const reloaded = [];
    vm.shellTray = { reload: (key) => reloaded.push(key) };
    ["today", "chat", "notifications"].forEach((tab) => { vm.trayLoaded[tab] = true; });
    vm.trayOpen = true;
    vm.trayTab = "chat";
    vm.onShellNotification({ topic: "shellCounts", message: { today: 2, chat: 1 } });
    assert.deepEqual(vm.shellCounts, { today: 2, chat: 1 });
    vm.onShellNotification({ topic: "shellCounts", message: '{"today": 5}' });
    assert.equal(vm.countOf("today"), 5);
    vm.onShellNotification({ topic: "todayItems" });
    vm.onShellNotification({ topic: "chatRoom:R100" });
    vm.onShellNotification({ topic: "notifications" });
    vm.onShellNotification({ topic: "ChatterMention" });
    vm.onShellNotification({ topic: "SomethingElse" });
    vm.onShellNotification(null);
    assert.deepEqual(reloaded, ["chat", "chat", "today", "chat", "notifications", "notifications"]);
    // a tab not loaded yet is not reloaded
    vm.trayLoaded.today = false;
    vm.onShellNotification({ topic: "todayItems" });
    assert.equal(reloaded.length, 6);

    // quick create: the kind's screen from /apps with the record of the screen context
    vm.screenContexts.push({ key: "c1", value: { recordEntity: "mantle.party.Party", recordId: "EX_JOHN_DOE", title: "John & Co" } });
    vm.openQuickCreate(vm.quickCreateItems.find((item) => item.kind === "task"));
    assert.equal(vm.quickCreateShow, true);
    assert.equal(vm.quickCreateTitle, "Task");
    assert.equal(vm.quickCreateUrl, "/apps/Commons/QuickCreate?kind=task&recordEntity=mantle.party.Party&recordId=EX_JOHN_DOE&recordTitle=John%20%26%20Co");
    assert.deepEqual(vm.assistReach, ["m-canvas", "m-quick-create"]);
    vm.screenContexts.length = 0;
    vm.openQuickCreate({ label: "Gallery note", url: "/qapps/gallery/QuickNote" });
    assert.equal(vm.quickCreateUrl, "/apps/gallery/QuickNote");
    assert.equal(vm.shellLoadPath("/qapps"), "/apps");
    assert.equal(vm.shellLoadPath("/qappsx/y"), "/qappsx/y");
    // the provider keys: a global scope everywhere, an app's scope only in that app
    keymap.scopes.length = 0;
    vm.setUrl = (url) => { vm.wentTo = url; };
    vm.registerProviderKeys();
    assert.deepEqual(keymap.active().map((entry) => entry.keys), ["g c"]);
    vm.currentPathList = ["gallery"];
    assert.deepEqual(keymap.active().map((entry) => entry.keys), ["g w", "g c"]);
    keymap.handle(key("g"), 1000);
    keymap.handle(key("w"), 1100);
    assert.equal(vm.wentTo, "/qapps/gallery/Widgets");
    vm.registerProviderKeys();
    assert.equal(keymap.scopes.length, 2, "registered again, not twice");
    keymap.scopes.length = 0;
});

test("a form in the quick-create dialog closes it when it submits without errors; its own submit-hide-id comes first", () => {
    const comp = qvt.components["m-form"];
    assert.deepEqual(comp.inject.mSubmitHide, { default: null });
    assert.deepEqual(qvt.components["m-shell-quick-create"].provide.call({ me: 1 }), { mSubmitHide: { me: 1 } });
    const hidden = [];
    const emitted = [];
    const make = (extra) => Object.assign({ $root: { setUrl() {}, hideContainer: (id) => hidden.push("container " + id), reloadContainer() {} },
        submitHideId: null, submitReloadId: null, submitMessage: "Saved", mSubmitHide: { hide: () => hidden.push("dialog") }, fields: {},
        $emit: (name, value) => emitted.push([name, value]) }, extra);
    comp.methods.handleResponse.call(make(), { messageInfos: [] });
    comp.methods.handleResponse.call(make(), { errors: ["no"] });
    comp.methods.handleResponse.call(make(), { validationErrors: [{ field: "x", message: "bad" }] });
    comp.methods.handleResponse.call(make({ submitHideId: "MyDialog" }), {});
    comp.methods.handleResponse.call(make({ mSubmitHide: null }), {});
    assert.deepEqual(hidden, ["dialog", "container MyDialog"]);
    // submitted: once for each answer without errors, with the answer
    assert.deepEqual(comp.emits, ["submitted"]);
    assert.deepEqual(emitted, [["submitted", { messageInfos: [] }], ["submitted", {}], ["submitted", {}]]);
    assert.equal(moqui.responseHasErrors({ errors: "" }), false);
    assert.equal(moqui.responseHasErrors({ errors: [] }), false);
    assert.equal(moqui.responseHasErrors("text"), false);
});

test("the shell components compile and the header has each element of the design", () => {
    const shell = qvt.components["m-qapps-root"].template;
    ["m-launcher", "m-app-screens", "m-search", "m-quick-create-menu", "m-account"].forEach((id) => assert.match(shell, new RegExp('id="' + id + '"'), id));
    assert.match(shell, /<m-shell-tray v-if="trayTabs.length"><\/m-shell-tray>\n {4}<\/q-page-container>/);
    assert.match(shell, /<m-shell-palette><\/m-shell-palette>/);
    // the notify-history menu only without a Notifications tab
    assert.match(shell, /<q-btn v-if="!shellValues.trayNotifications" flat round dense icon="notifications">/);
    assert.match(qvt.components["m-shell-tray"].template, /<q-card id="m-tray"/);
    assert.match(qvt.components["m-shell-quick-create"].template, /id="m-quick-create"/);
});

test("a preference applies at once: dark mode, the drawer, the key overrides and the density; the start app opens on /qapps alone", () => {
    const savedConfig = qvt.config;
    const savedDocument = globalThis.document;
    const classes = new Set();
    globalThis.document = { body: { classList: { toggle: (name, on) => { if (on) classes.add(name); else classes.delete(name); } } } };
    qvt.config = { preferences: { OTHER: "x" } };
    let dark = null;
    moqui.webrootVue = { $q: { dark: { set: (value) => { dark = value; } } }, leftOpen: false };
    try {
        moqui.applyPreference("QUASAR_DARK", "true");
        assert.equal(dark, true);
        moqui.applyPreference("QUASAR_LEFT_OPEN", "true");
        assert.equal(moqui.webrootVue.leftOpen, true);
        moqui.applyPreference("SHELL_DENSITY", "compact");
        assert.ok(classes.has("m-density-compact"));
        assert.equal(qvt.config.density, "compact");
        // the X-Moqui-Preferences answer of a request is a map, applied the same way
        moqui.onPreferences({ SHELL_DENSITY: "comfortable", KEYMAP: '{"global": {"palette": "Mod+P"}}', NEW_KEY: "v" });
        assert.ok(!classes.has("m-density-compact"));
        assert.deepEqual(keymap.overrides, { global: { palette: "Mod+P" } });
        assert.deepEqual(qvt.config.preferences, { OTHER: "x", QUASAR_DARK: "true", QUASAR_LEFT_OPEN: "true", SHELL_DENSITY: "comfortable",
            KEYMAP: '{"global": {"palette": "Mod+P"}}', NEW_KEY: "v" });
        moqui.applyPreference("KEYMAP", "");
        assert.deepEqual(keymap.overrides, {});
        assert.equal(qvt.config.preferences.KEYMAP, undefined, "an empty value is removed");
        // SHELL_START_APP
        assert.equal(moqui.startPath("/qapps", "/qapps", "Commons"), "/qapps/Commons");
        assert.equal(moqui.startPath("/qapps/", "/qapps", "Commons"), "/qapps/Commons");
        assert.equal(moqui.startPath("/qapps/Sales/Order", "/qapps", "Commons"), "/qapps/Sales/Order");
        assert.equal(moqui.startPath("/qapps", "/qapps", ""), "/qapps");
    } finally {
        delete moqui.webrootVue;
        qvt.config = savedConfig;
        globalThis.document = savedDocument;
    }
});
