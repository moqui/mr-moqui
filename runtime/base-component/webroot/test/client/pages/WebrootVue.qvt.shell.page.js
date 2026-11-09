// The suite shell, in the page. Was js/WebrootVue.qvt.shell.test.js.
// WebrootVue.qvt.js starts the Vue app only when the page has #moqui-config plus Vue and Quasar. This page has none of those.
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
    { name: "Hub", title: "Hub", path: "/qapps/Hub", image: "sym_o_hub", imageType: "icon", menu: [
        { title: "Home", path: "/qapps/Hub/Home", depth: 1 },
        { title: "Contacts", path: "/qapps/Hub/Contacts", image: "sym_o_contacts", imageType: "icon", depth: 1 },
        { title: "Duplicates", path: "/qapps/Hub/Contacts/Duplicates", depth: 2 },
        { title: "Tasks", path: "/qapps/Hub/Tasks", depth: 1 }],
      providers: { quickCreate: [{ kind: "task", label: "Task", icon: "sym_o_task_alt", url: "/qapps/Hub/QuickCreate?kind=task", requiresRecord: false },
          { kind: "activity", label: "Activity", url: "/qapps/Hub/QuickCreate?kind=activity", requiresRecord: true }],
          paletteActions: [{ label: "New event", icon: "sym_o_event", url: "/qapps/Hub/Calendar?new=true" }],
          paletteSources: [{ group: "Records", label: "Records", prefix: null }, { group: "People", label: "People", prefix: "@" }],
          accountMenuItems: [{ label: "Preferences", icon: "sym_o_tune", url: "/qapps/Hub/Settings/Preferences" }],
          notificationTopics: ["ChatterMention"],
          keymaps: [{ scope: "global", keys: [{ keys: "g c", label: "Contacts", url: "/qapps/Hub/Contacts", action: null }] }] } },
    { name: "gallery", title: "Gallery", path: "/qapps/gallery", menu: [{ title: "Widgets", path: "/qapps/gallery/Widgets", depth: 1 }],
      providers: { quickCreate: [{ kind: "galleryNote", label: "Gallery note", url: "/qapps/gallery/QuickNote", requiresRecord: false }],
          paletteActions: [{ label: "Open the widget gallery", url: "/qapps/gallery/Widgets" }],
          paletteSources: [{ group: "GalleryEnumerations", label: "Enumerations", prefix: "~" }, { group: "Records", label: "Records again" }],
          accountMenuItems: [{ label: "Preferences again", url: "/qapps/Hub/Settings/Preferences" }],
          keymaps: [{ scope: "gallery", keys: [{ keys: "g w", label: "Go to the widgets", url: "/qapps/gallery/Widgets" }] }] } }];

test("the Screens rows are the app roots and their menus with the app and the parent as the subtitle", () => {
    const rows = palette.screens(apps);
    assert.deepEqual(rows.map((row) => row.title), ["Hub", "Home", "Contacts", "Duplicates", "Tasks", "Gallery", "Widgets"]);
    assert.equal(rows[0].url, "/qapps/Hub");
    assert.equal(rows[2].subtitle, "Hub");
    assert.equal(rows[3].subtitle, "Hub › Contacts");
    assert.equal(rows[4].subtitle, "Hub");
    assert.equal(rows[2].image, "sym_o_contacts");
});

test("the groups: Recent before typing; Screens, Saved finds, Actions, then the provider groups, five each with more", () => {
    const labels = { recent: "Recent", screens: "Screens", savedFinds: "Saved finds", actions: "Actions", more: "More" };
    const screens = palette.screens(apps).concat([1, 2, 3, 4, 5, 6].map((n) => ({ title: "Contacts " + n, url: "/qapps/c" + n })));
    const input = { labels, recent: [{ title: "Tasks", url: "/qapps/Hub/Tasks" }], screens,
        savedFinds: [{ title: "My open contacts", url: "/qapps/Hub/Contacts?formListFindId=1" }],
        actions: [{ title: "New contact", url: "/qapps/Hub/Contacts?new=true" }, { title: "New event", url: "/qapps/Hub/Calendar?new=true" }],
        remote: [{ group: "Records", label: "Records", icon: "sym_o_search", rows: [1, 2, 3, 4, 5, 6].map((n) => ({ title: "Contact record " + n, url: "/qapps/r" + n })),
            moreUrl: "/qapps/Hub/Search?q=contact" },
            { group: "People", label: "People", rows: [{ title: "John Doe", subtitle: "Person", url: "/qapps/Hub/Contacts/ContactDetail?partyId=EX_JOHN_DOE" },
                { title: "No link" }] }] };
    const before = palette.groups(palette.parse("", sources), input);
    assert.deepEqual(before.map((group) => group.key), ["recent"]);
    const groups = palette.groups(palette.parse("contact", sources), input);
    assert.deepEqual(groups.map((group) => group.key), ["screens", "savedFinds", "actions", "source:Records", "source:People"]);
    assert.equal(groups[0].rows.length, 5);
    assert.equal(groups[0].rows[0].title, "Contacts");
    // a provider group: its five rows and a last row to the more link; a row without a link is left out
    assert.equal(groups[3].rows.length, 6);
    assert.deepEqual(groups[3].rows[5], { more: true, title: "More: Records", url: "/qapps/Hub/Search?q=contact", icon: "sym_o_arrow_forward", index: 12 });
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
    const vm = Object.assign(options.data(), { currentPath: "/apps/Hub/Contacts", currentParameters: {} }, extra);
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
    vm.shellData = { apps, shell: { trayToday: "/qapps/Hub/Panel/Today", trayChat: null, trayNotifications: "/qapps/Hub/Panel/Notifications",
        mailScreen: "/qapps/Mail/Inbox", messagesScreen: "/qapps/Mail/Messages", quickCreate: "/qapps/Hub/QuickCreate" },
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
    assert.deepEqual(vm.trayTabs.map((tab) => [tab.key, tab.loadUrl]), [["today", "/apps/Hub/Panel/Today"], ["notifications", "/apps/Hub/Panel/Notifications"]]);
    // the tray's buttons first, in its tab order, then a divider and the links to screens
    assert.deepEqual(vm.headerCounters.map((counter) => [counter.key, counter.tray, counter.url || null, !!counter.dividerBefore]), [["today", true, null, false],
        ["notifications", true, null, false], ["email", false, "/qapps/Mail/Inbox", true], ["messages", false, "/qapps/Mail/Messages", false]]);
    // the tray tab has the short name, the header button the full one
    assert.deepEqual(vm.trayTabs.map((tab) => tab.label), ["Today", "Alerts"]);
    assert.equal(vm.headerCounters[1].label, "Notifications");
    assert.equal(vm.countOf("today"), 3);
    assert.equal(vm.countText("email"), "99+");
    assert.deepEqual(vm.headerCounters.map((counter) => vm.counterText(counter)), ["3", "0", "99+", "2"]);
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
    assert.equal(vm.headerCounters[1].url, "/qapps/Mail/Inbox");
    vm.shellData.shell.mailScreen = null;
    assert.deepEqual(vm.headerCounters.map((counter) => counter.key), ["tray", "messages"], "Messages alone stays");
    vm.shellData.shell.mailScreen = "/qapps/Mail/Inbox";
    // the missed calls (the calls count) link to the theme's calls screen; below 600 px only while there are some
    vm.shellData.shell.callsScreen = "/qapps/Comms/Calls/History?outcome=missed";
    assert.deepEqual(vm.headerCounters.map((counter) => counter.key), ["tray", "mail"], "no missed calls: no button on a phone");
    vm.shellCounts = Object.assign({}, vm.shellCounts, { calls: 2 });
    assert.deepEqual(vm.headerCounters.map((counter) => counter.key), ["tray", "mail", "calls"]);
    assert.equal(vm.headerCounters[2].url, "/qapps/Comms/Calls/History?outcome=missed");
    assert.equal(vm.counterText(vm.headerCounters[2]), "2");
    vm.$q = { screen: { lt: { sm: false } } };
    vm.shellCounts = Object.assign({}, vm.shellCounts, { calls: 0 });
    assert.deepEqual(vm.headerCounters.map((counter) => counter.key), ["today", "notifications", "email", "messages", "calls"], "always on a wide screen");
    vm.$q = { screen: { lt: { sm: true } } };
    vm.shellData.shell.callsScreen = null;
    vm.shellCounts = { email: 3, messages: 2, today: 1, notifications: 4 };
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
    assert.deepEqual(vm.headerCounters.map((counter) => counter.key), ["notifications", "mail"]);
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
    const vm = makeRoot({}, { currentPathList: ["Hub", "Contacts"] });
    vm.shellData = { apps, shell: { trayToday: "/qapps/Hub/Panel/Today", trayChat: "/qapps/Mail/Panel/Chat", trayNotifications: "/qapps/Hub/Panel/Notifications" }, counts: {} };
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
    vm.screenContexts.push({ key: "c1", value: { recordEntity: "moqui.test.Party", recordId: "EX_JOHN_DOE", title: "John & Co" } });
    vm.openQuickCreate(vm.quickCreateItems.find((item) => item.kind === "task"));
    assert.equal(vm.quickCreateShow, true);
    assert.equal(vm.quickCreateTitle, "Task");
    assert.equal(vm.quickCreateUrl, "/apps/Hub/QuickCreate?kind=task&recordEntity=moqui.test.Party&recordId=EX_JOHN_DOE&recordTitle=John%20%26%20Co");
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
    // the tray and then the call dock close the page container
    assert.match(shell, /<m-shell-tray v-if="trayTabs.length"><\/m-shell-tray>\n {8}<m-call-dock v-if="callsOn"><\/m-call-dock>\n {4}<\/q-page-container>/);
    assert.match(shell, /<m-shell-palette><\/m-shell-palette>/);
    // a handheld screen: no drawer, menu button, launcher or footer; of the header's buttons only the account; the title at every width
    assert.match(shell, /<q-drawer v-if="!handheld"/);
    assert.match(shell, /<q-footer v-if="!handheld"/);
    assert.match(shell, /<q-btn v-if="!handheld" flat round dense icon="menu"/);
    assert.match(shell, /<q-btn v-if="!handheld" flat dense no-caps id="m-launcher"/);
    assert.match(shell, /<template v-if="!handheld">\n {8}<q-btn flat dense no-caps id="m-search"/);
    assert.match(shell, /<\/template>\n {8}<q-btn flat round dense id="m-account"/);
    assert.match(shell, /id="m-app-screens" class="m-app-screens text-subtitle1 text-weight-medium" :class="handheld \? '' : 'gt-xs'"/);
    // the notify-history menu only without a Notifications tab
    assert.match(shell, /<q-btn v-if="!shellValues.trayNotifications" flat round dense icon="notifications" :aria-label="labels.notifyHistory">/);
    // the icon-only buttons are named for a screen reader, not by their icon ligature; the launcher opens the AppList
    assert.match(shell, /icon="menu" :aria-label="labels.menu"/);
    assert.match(shell, /id="m-launcher" class="q-mx-xs" :aria-label="labels.allApplications" @click="setUrl\(linkBasePath \+ '\/AppList'\)"/);
    assert.match(shell, /id="m-search" icon="sym_o_search" :aria-label="labels.search"/);
    assert.match(shell, /id="m-quick-create-menu" icon="sym_o_add" :aria-label="labels.quickCreate"/);
    assert.match(shell, /:icon="counter.icon" :aria-label="counter.label"/);
    assert.match(shell, /icon="help_outline" :aria-label="labels.documentation"/);
    assert.match(shell, /id="m-account" :aria-label="userFullName \|\| labels.account"/);
    assert.match(qvt.components["m-shell-tray"].template, /<q-card id="m-tray"/);
    assert.match(qvt.components["m-shell-tray"].template, /:aria-label="\$root\.labels\.closeTray"/);
    assert.match(qvt.components["m-container-dialog"].template, /:aria-label="!buttonText \? \(tooltip \|\| title\) : undefined"/);
    assert.match(qvt.components["m-shell-quick-create"].template, /id="m-quick-create"/);
});

test("a preference applies at once: dark mode, the drawer, the key overrides and the density; the start app opens on /qapps alone", () => {
    const savedConfig = qvt.config;
    // Density toggles a class on the real body. The page cannot replace document, so the class is cleared afterwards.
    qvt.config = { preferences: { OTHER: "x" } };
    let dark = null;
    moqui.webrootVue = { $q: { dark: { set: (value) => { dark = value; } } }, leftOpen: false };
    try {
        moqui.applyPreference("QUASAR_DARK", "true");
        assert.equal(dark, true);
        moqui.applyPreference("QUASAR_LEFT_OPEN", "true");
        assert.equal(moqui.webrootVue.leftOpen, true);
        moqui.applyPreference("SHELL_DENSITY", "compact");
        assert.ok(document.body.classList.contains("m-density-compact"));
        assert.equal(qvt.config.density, "compact");
        // the X-Moqui-Preferences answer of a request is a map, applied the same way
        moqui.onPreferences({ SHELL_DENSITY: "comfortable", KEYMAP: '{"global": {"palette": "Mod+P"}}', NEW_KEY: "v" });
        assert.ok(!document.body.classList.contains("m-density-compact"));
        assert.deepEqual(keymap.overrides, { global: { palette: "Mod+P" } });
        assert.deepEqual(qvt.config.preferences, { OTHER: "x", QUASAR_DARK: "true", QUASAR_LEFT_OPEN: "true", SHELL_DENSITY: "comfortable",
            KEYMAP: '{"global": {"palette": "Mod+P"}}', NEW_KEY: "v" });
        moqui.applyPreference("KEYMAP", "");
        assert.deepEqual(keymap.overrides, {});
        assert.equal(qvt.config.preferences.KEYMAP, undefined, "an empty value is removed");
        // SHELL_START_APP
        assert.equal(moqui.startPath("/qapps", "/qapps", "Hub"), "/qapps/Hub");
        assert.equal(moqui.startPath("/qapps/", "/qapps", "Hub"), "/qapps/Hub");
        assert.equal(moqui.startPath("/qapps/Orders/Order", "/qapps", "Hub"), "/qapps/Orders/Order");
        assert.equal(moqui.startPath("/qapps", "/qapps", ""), "/qapps");
    } finally {
        delete moqui.webrootVue;
        qvt.config = savedConfig;
        document.body.classList.remove("m-density-compact");
    }
});

test("the appearance applies at once: the scheme and font sheets, the tokens, a checked accent color and the body classes", () => {
    const savedConfig = qvt.config;
    // Link elements stay detached stand-ins, so the browser does not fetch the scheme or font sheets.
    // Tokens and body classes are read off the real document and cleared afterwards. A removed custom
    // property reads as "" here; the Node stand-in stored the value as missing.
    const links = {};
    const realCreate = document.createElement.bind(document);
    const realAppend = document.head.appendChild.bind(document.head);
    const realGet = document.getElementById.bind(document);
    const link = () => ({ id: "", attrs: {}, getAttribute(name) { return this.attrs[name]; }, setAttribute(name, value) { this.attrs[name] = value; },
        remove() { delete links[this.id]; } });
    document.createElement = (tag) => tag === "link" ? link() : realCreate(tag);
    document.head.appendChild = (element) => {
        if (element && element.attrs) { links[element.id] = element; return element; }
        return realAppend(element);
    };
    document.getElementById = (id) => links[id] || realGet(id);
    const htmlProp = (name) => document.documentElement.style.getPropertyValue(name);
    const bodyProp = (name) => document.body.style.getPropertyValue(name);
    qvt.config = { preferences: {}, appearance: {
        colorSchemes: [{ id: "DEFAULT", custom: false }, { id: "SLATE", custom: true, version: "v1" }],
        fonts: [{ id: "INTER", kind: "sans", family: "Inter, sans-serif", stylesheet: "https://fonts.example/inter.css" }, { id: "SYSTEM_MONO", kind: "mono", family: "monospace" }],
        values: { THEME_COLOR_SCHEME: "SLATE", THEME_SIZE_LARGE: "1.25", THEME_RADIUS: "square" } } };
    try {
        moqui.applyAppearanceAll();
        assert.equal(links["m-color-scheme"].attrs.href, "/apps/colorSchemeCss?colorSchemeId=SLATE&v=v1");
        assert.equal(htmlProp("--m-fs-large-scale"), "1.25");
        assert.ok(document.body.classList.contains("m-radius-square"));
        // a scheme without colors is the theme's own: no sheet
        moqui.applyPreference("THEME_COLOR_SCHEME", "DEFAULT");
        assert.equal(links["m-color-scheme"], undefined);
        moqui.applyPreference("THEME_FONT_BODY", "INTER");
        assert.equal(htmlProp("--m-font-body"), "Inter, sans-serif");
        assert.equal(links["m-font-body"].attrs.href, "https://fonts.example/inter.css");
        moqui.applyPreference("THEME_FONT_MONO", "SYSTEM_MONO");
        assert.equal(links["m-font-mono"], undefined, "a system font links no sheet");
        moqui.applyPreference("THEME_FONT_BODY", "");
        assert.equal(htmlProp("--m-font-body"), "");
        assert.equal(links["m-font-body"], undefined);
        // a size outside the steps, and 100%, are the default
        moqui.applyPreference("THEME_SIZE_SMALL", "3");
        assert.equal(htmlProp("--m-fs-small-scale"), "");
        moqui.applyPreference("THEME_SIZE_SMALL", "1.1");
        assert.equal(htmlProp("--m-fs-small-scale"), "1.1");
        moqui.applyPreference("THEME_LINE_HEIGHT", "relaxed");
        assert.equal(htmlProp("--m-line-height"), "1.75");
        // the accent colors only when they are CSS colors, over the scheme and the brand: the light one for dark mode too until
        // the dark one is set
        const accent = () => (document.getElementById("m-accent") || {}).textContent || "";
        moqui.applyPreference("THEME_PRIMARY_COLOR", "#336699");
        assert.match(accent(), /html body:not\(\.body--dark\) \{ --q-primary:#336699 !important; \}/);
        assert.match(accent(), /html body\.body--dark \{ --q-primary:#336699 !important; --m-accent-dark-link:color-mix\(in srgb, #336699 60%, white\); \}/);
        assert.equal(bodyProp("--q-primary"), "", "not on the body, where it would be both modes");
        moqui.applyPreference("THEME_PRIMARY_COLOR_DARK", "#99ccff");
        assert.match(accent(), /:not\(\.body--dark\) \{ --q-primary:#336699 !important; \}/);
        assert.match(accent(), /body\.body--dark \{ --q-primary:#99ccff !important; --m-accent-dark-link:color-mix\(in srgb, #99ccff 60%, white\); \}/);
        moqui.applyPreference("THEME_PRIMARY_COLOR_DARK", "");
        moqui.applyPreference("THEME_PRIMARY_COLOR", "red; background:url(x)");
        assert.equal(document.getElementById("m-accent"), null);
        assert.ok(moqui.isCssColor("oklch(0.6 0.15 250)") && moqui.isCssColor("rgb(10, 20, 30)") && !moqui.isCssColor("var(--x)"));
        moqui.applyPreference("THEME_MOTION", "reduce");
        moqui.applyPreference("THEME_LINKS_UNDERLINE", "true");
        moqui.applyPreference("THEME_RADIUS", "");
        assert.deepEqual([...document.body.classList].filter((name) => name.startsWith("m-")).sort(), ["m-links-underline", "m-motion-reduce"]);
        assert.deepEqual(qvt.config.appearance.values, { THEME_COLOR_SCHEME: "DEFAULT", THEME_SIZE_LARGE: "1.25", THEME_FONT_MONO: "SYSTEM_MONO",
            THEME_SIZE_SMALL: "1.1", THEME_LINE_HEIGHT: "relaxed", THEME_PRIMARY_COLOR: "red; background:url(x)", THEME_MOTION: "reduce",
            THEME_LINKS_UNDERLINE: "true" }, "the values are what the user saved, a bad color kept but not applied");
        // QUASAR_DARK auto follows the system
        let dark = null;
        moqui.webrootVue = { $q: { dark: { set: (value) => { dark = value; } } } };
        moqui.applyPreference("QUASAR_DARK", "auto");
        assert.equal(dark, "auto");
        assert.match(qvt.components["m-qapps-root"].template, /<m-appearance-dialog><\/m-appearance-dialog>/);
        assert.match(qvt.components["m-qapps-root"].template, /id="m-account-appearance"/);
    } finally {
        delete moqui.webrootVue;
        qvt.config = savedConfig;
        document.createElement = realCreate;
        document.head.appendChild = realAppend;
        document.getElementById = realGet;
        ["--m-fs-large-scale", "--m-fs-small-scale", "--m-font-body", "--m-line-height"].forEach((name) => document.documentElement.style.removeProperty(name));
        ["m-accent", "m-scheme-colors"].forEach((id) => { const style = realGet(id); if (style) style.remove(); });
        ["m-radius-square", "m-radius-round", "m-links-underline", "m-motion-reduce", "m-motion-full"].forEach((name) => document.body.classList.remove(name));
    }
});

test("the theme's appearance: its default scheme and the scheme's fonts, the user's color overrides, and the keys its policy locks", () => {
    const savedConfig = qvt.config;
    const links = {};
    const realCreate = document.createElement.bind(document);
    const realAppend = document.head.appendChild.bind(document.head);
    const realGet = document.getElementById.bind(document);
    const link = () => ({ id: "", attrs: {}, getAttribute(name) { return this.attrs[name]; }, setAttribute(name, value) { this.attrs[name] = value; },
        remove() { delete links[this.id]; } });
    document.createElement = (tag) => tag === "link" ? link() : realCreate(tag);
    document.head.appendChild = (element) => {
        if (element && element.attrs) { links[element.id] = element; return element; }
        return realAppend(element);
    };
    document.getElementById = (id) => links[id] || realGet(id);
    const htmlProp = (name) => document.documentElement.style.getPropertyValue(name);
    qvt.config = { preferences: {}, appearance: {
        colorSchemes: [{ id: "PLAIN", custom: false }, { id: "LAB", custom: true, version: "v2", fonts: { body: "INTER" } }],
        fonts: [{ id: "INTER", kind: "sans", family: "Inter, sans-serif", stylesheet: "https://fonts.example/inter.css" },
            { id: "SERIF", kind: "serif", family: "Georgia, serif" }],
        defaultSchemeId: "LAB", values: {}, policy: { accent: false, lockedKeys: ["THEME_PRIMARY_COLOR", "THEME_PRIMARY_COLOR_DARK"] } } };
    try {
        // no scheme of the user's: the theme's default, with its font
        moqui.applyAppearanceAll();
        assert.equal(moqui.appearanceScheme().id, "LAB");
        assert.equal(links["m-color-scheme"].attrs.href, "/apps/colorSchemeCss?colorSchemeId=LAB&v=v2");
        assert.equal(htmlProp("--m-font-body"), "Inter, sans-serif");
        assert.equal(links["m-font-body"].attrs.href, "https://fonts.example/inter.css");
        // the user's font wins over the scheme's; leaving it gives the scheme's back
        moqui.applyPreference("THEME_FONT_BODY", "SERIF");
        assert.equal(htmlProp("--m-font-body"), "Georgia, serif");
        assert.equal(links["m-font-body"], undefined);
        moqui.applyPreference("THEME_FONT_BODY", "");
        assert.equal(htmlProp("--m-font-body"), "Inter, sans-serif");
        // another scheme takes its fonts away with it
        moqui.applyPreference("THEME_COLOR_SCHEME", "PLAIN");
        assert.equal(links["m-color-scheme"], undefined);
        assert.equal(htmlProp("--m-font-body"), "");
        // the user's own tokens over the scheme, by the rules of a scheme's sheet
        moqui.applyPreference("THEME_COLOR_OVERRIDES", JSON.stringify({ primary: { light: "#c00" }, "header-bg": { light: "linear-gradient(#fff, #eee)" },
            text: { light: "red; }" }, "panel-bg": { dark: "#101010" }, link: { dark: "#9cf" }, page: "#fff" }));
        const overrides = realGet("m-scheme-colors").textContent;
        assert.equal(overrides, "html body:not(.body--dark) { --m-header-bg:linear-gradient(#fff, #eee); --q-primary:#c00 !important; }\n" +
            "html body.body--dark { --m-link:var(--m-accent-dark-link, #9cf); --m-panel-bg:#101010; --q-dark:#101010 !important; }\n");
        moqui.applyPreference("THEME_COLOR_OVERRIDES", "not json");
        assert.equal(realGet("m-scheme-colors"), null);
        // what the policy locks
        assert.ok(!moqui.appearanceAllowed("THEME_PRIMARY_COLOR") && !moqui.appearanceAllowed("THEME_PRIMARY_COLOR_DARK"));
        assert.ok(moqui.appearanceAllowed("THEME_COLOR_SCHEME") && moqui.appearanceAllowed("QUASAR_DARK"));
        assert.match(qvt.components["m-qapps-root"].template, /v-if="appearanceMenuAllowed\(\)" id="m-account-appearance"/);
        assert.match(qvt.components["m-appearance-dialog"].template, /v-if="allowed\('QUASAR_DARK'\)"/);
    } finally {
        qvt.config = savedConfig;
        document.createElement = realCreate;
        document.head.appendChild = realAppend;
        document.getElementById = realGet;
        document.documentElement.style.removeProperty("--m-font-body");
        ["m-accent", "m-scheme-colors"].forEach((id) => { const style = realGet(id); if (style) style.remove(); });
    }
});

test("the account menu's Theme: shown for two themes the policy leaves open; a pick is posted and the page reloads, the current one does nothing", async () => {
    const savedConfig = qvt.config, savedRequest = moqui.request, savedReload = moqui.reloadPage;
    const methods = qvt.rootOptions({ basePath: "/apps", linkBasePath: "/qapps", appRootPath: "" }).methods;
    const vm = { userId: "john", appRootPath: "", moquiSessionToken: "tok" };
    Object.keys(methods).forEach((key) => { vm[key] = methods[key].bind(vm); });
    const posts = [];
    let reloads = 0;
    moqui.request = (url, options) => { posts.push([url, options.params]); return Promise.resolve({ status: 200 }); };
    moqui.reloadPage = () => { reloads++; };
    const themes = [{ id: "DEFAULT_QUASAR", description: "Default" }, { id: "GREEN_SCREEN_QUASAR", description: "Green Screen" }];
    try {
        assert.match(qvt.components["m-qapps-root"].template, /v-if="themeMenuAllowed\(\)" id="m-account-theme"/);
        qvt.config = { appearance: { screenThemeId: "DEFAULT_QUASAR", themes: themes.slice(0, 1), policy: {} } };
        assert.ok(!vm.themeMenuAllowed(), "one theme: nothing to choose");
        qvt.config = { appearance: { screenThemeId: "DEFAULT_QUASAR", themes, policy: { theme: false } } };
        assert.ok(!vm.themeMenuAllowed(), "the policy locks the choice");
        qvt.config = { appearance: { screenThemeId: "DEFAULT_QUASAR", themes, policy: { appearance: false } } };
        assert.ok(!vm.themeMenuAllowed(), "the policy locks all of the appearance");
        qvt.config = { appearance: { screenThemeId: "DEFAULT_QUASAR", themes, policy: {} } };
        assert.ok(vm.themeMenuAllowed());
        assert.ok(!methods.themeMenuAllowed.call(Object.assign({}, vm, { userId: "" })), "no user");
        assert.deepEqual(vm.appearanceThemes().map((theme) => theme.id), ["DEFAULT_QUASAR", "GREEN_SCREEN_QUASAR"]);
        assert.equal(vm.currentThemeId(), "DEFAULT_QUASAR");
        vm.setScreenTheme("DEFAULT_QUASAR");
        assert.equal(posts.length, 0, "the current theme");
        vm.setScreenTheme("GREEN_SCREEN_QUASAR");
        await Promise.resolve();
        await Promise.resolve();
        assert.deepEqual(posts, [["/apps/setScreenTheme", { moquiSessionToken: "tok", screenThemeId: "GREEN_SCREEN_QUASAR" }]]);
        assert.equal(reloads, 1);
    } finally {
        qvt.config = savedConfig;
        moqui.request = savedRequest;
        moqui.reloadPage = savedReload;
    }
});

// The drawer's rows (moqui.drawerRows) from the menu data: the apps root without a row, the open app's top-level screens one
// level below the apps, a level more for each deeper screen, the saved finds of the last screen, and one current row.
const appsRoot = (activeName) => ({ name: "apps", title: "Applications", path: "/apps", subscreens: [
    { name: "Hub", title: "Hub", path: "/apps/Hub", pathWithParams: "/apps/Hub", image: "sym_o_hub", imageType: "icon", active: activeName === "Hub" },
    { name: "Orders", title: "Orders", path: "/apps/Orders", pathWithParams: "/apps/Orders", active: activeName === "Orders" }] });
const hubApp = (activeName) => ({ name: "Hub", title: "Hub", path: "/apps/Hub", pathWithParams: "/apps/Hub", image: "sym_o_hub", imageType: "icon",
    subscreens: [{ name: "Home", title: "Home", path: "/apps/Hub/Home", pathWithParams: "/apps/Hub/Home", active: activeName === "Home" },
        { name: "Tasks", title: "Tasks", path: "/apps/Hub/Tasks", pathWithParams: "/apps/Hub/Tasks", active: activeName === "Tasks" },
        { name: "Admin", title: "Admin", path: "/apps/Hub/Admin", disableLink: true }] });
const brief = (rows) => rows.map((row) => [row.kind, row.level, row.title, row.onPath, row.current, row.inApp]);

test("the drawer lists the apps without the apps root, and nothing is open or current on the app list", () => {
    assert.deepEqual(brief(moqui.drawerRows([appsRoot("")])), [["app", 0, "Hub", false, false, false], ["app", 0, "Orders", false, false, false]]);
    assert.deepEqual(moqui.drawerRows([]), []);
    assert.deepEqual(moqui.drawerRows(null), []);
});

test("the open app's top-level screens are one level below the app, inside the app, and the app is current at its root", () => {
    const rows = moqui.drawerRows([appsRoot("Hub"), hubApp("")]);
    assert.deepEqual(brief(rows), [["app", 0, "Hub", true, true, true], ["screen", 1, "Home", false, false, true],
        ["screen", 1, "Tasks", false, false, true], ["screen", 1, "Admin", false, false, true], ["app", 0, "Orders", false, false, false]]);
    assert.equal(rows[0].image, "sym_o_hub");
    assert.equal(rows[1].to, "/apps/Hub/Home");
    // a disabled link has no target
    assert.equal(rows[3].to, "");
});

test("a deeper path is a level more for each screen, the last one current, a screen with no subscreens shows its child", () => {
    const tasks = { name: "Tasks", title: "Tasks", path: "/apps/Hub/Tasks", pathWithParams: "/apps/Hub/Tasks",
        subscreens: [{ name: "Open", title: "Open", path: "/apps/Hub/Tasks/Open", active: true }, { name: "Done", title: "Done", path: "/apps/Hub/Tasks/Done" }] };
    // Open has no subscreens and is not the last: its child, Task, is shown under it
    const open = { name: "Open", title: "Open", path: "/apps/Hub/Tasks/Open", pathWithParams: "/apps/Hub/Tasks/Open" };
    const task = { name: "Task", title: "Task T1", path: "/apps/Hub/Tasks/Open/Task", pathWithParams: "/apps/Hub/Tasks/Open/Task?taskId=T1" };
    const rows = moqui.drawerRows([appsRoot("Hub"), hubApp("Tasks"), tasks, open, task]);
    assert.deepEqual(brief(rows), [["app", 0, "Hub", true, false, true], ["screen", 1, "Home", false, false, true],
        ["screen", 1, "Tasks", true, false, true], ["screen", 2, "Open", true, false, true], ["screen", 3, "Task T1", true, true, true],
        ["screen", 2, "Done", false, false, true], ["screen", 1, "Admin", false, false, true], ["app", 0, "Orders", false, false, false]]);
    assert.equal(rows[4].to, "/apps/Hub/Tasks/Open/Task?taskId=T1");
    assert.equal(new Set(rows.map((row) => row.key)).size, rows.length);
});

test("a screen whose active child is its own list (same title) shows the list in its place, one level up", () => {
    const app = { name: "Hub", title: "Hub", path: "/apps/Hub", pathWithParams: "/apps/Hub",
        subscreens: [{ name: "WorkItems", title: "Work items", path: "/apps/Hub/WorkItems", pathWithParams: "/apps/Hub/WorkItems", active: true },
            { name: "Backlog", title: "Backlog", path: "/apps/Hub/Backlog", pathWithParams: "/apps/Hub/Backlog" }] };
    const screen = { name: "WorkItems", title: "Work items", path: "/apps/Hub/WorkItems", pathWithParams: "/apps/Hub/WorkItems",
        subscreens: [{ name: "List", title: "Work items", path: "/apps/Hub/WorkItems/List", pathWithParams: "/apps/Hub/WorkItems/List", active: true }] };
    const list = { name: "List", title: "Work items", path: "/apps/Hub/WorkItems/List", pathWithParams: "/apps/Hub/WorkItems/List",
        savedFinds: [{ name: "F1", title: "Mine", path: "/apps/Hub/WorkItems/List", pathWithParams: "/apps/Hub/WorkItems/List?formListFindId=F1", active: true },
            { name: "F2", title: "Late", path: "/apps/Hub/WorkItems/List", pathWithParams: "/apps/Hub/WorkItems/List?formListFindId=F2" }] };
    const rows = moqui.drawerRows([appsRoot("Hub"), app, screen, list]);
    assert.deepEqual(brief(rows), [["app", 0, "Hub", true, false, true], ["screen", 1, "Work items", true, false, true],
        ["find", 2, "Mine", false, true, true], ["find", 2, "Late", false, false, true], ["screen", 1, "Backlog", false, false, true],
        ["app", 0, "Orders", false, false, false]]);
});

test("the same-title list gives way wherever it is among its siblings, they come up with it, and an app keeps its row", () => {
    const app = { name: "Hub", title: "Hub", path: "/apps/Hub", pathWithParams: "/apps/Hub",
        subscreens: [{ name: "WorkItems", title: "Work items", path: "/apps/Hub/WorkItems", pathWithParams: "/apps/Hub/WorkItems", active: true }] };
    const screen = { name: "WorkItems", title: "Work items", path: "/apps/Hub/WorkItems", pathWithParams: "/apps/Hub/WorkItems",
        subscreens: [{ name: "Board", title: "Board", path: "/apps/Hub/WorkItems/Board", pathWithParams: "/apps/Hub/WorkItems/Board" },
            { name: "List", title: "Work items", path: "/apps/Hub/WorkItems/List", pathWithParams: "/apps/Hub/WorkItems/List", active: true }] };
    const list = { name: "List", title: "Work items", path: "/apps/Hub/WorkItems/List", pathWithParams: "/apps/Hub/WorkItems/List" };
    assert.deepEqual(brief(moqui.drawerRows([appsRoot("Hub"), app, screen, list])), [["app", 0, "Hub", true, false, true],
        ["screen", 1, "Board", false, false, true], ["screen", 1, "Work items", true, true, true], ["app", 0, "Orders", false, false, false]]);
    // an app whose open screen has the app's title keeps the app row
    const hub = { name: "Hub", title: "Hub", path: "/apps/Hub/Hub", pathWithParams: "/apps/Hub/Hub" };
    const hubOnly = { name: "Hub", title: "Hub", path: "/apps/Hub", pathWithParams: "/apps/Hub", subscreens: [Object.assign({ active: true }, hub)] };
    assert.deepEqual(brief(moqui.drawerRows([appsRoot("Hub"), hubOnly, hub])), [["app", 0, "Hub", true, false, true],
        ["screen", 1, "Hub", true, true, true], ["app", 0, "Orders", false, false, false]]);
});

test("the last screen's saved finds are its children, and an active one is current in place of the screen", () => {
    const finds = (active) => ({ name: "Tasks", title: "Tasks", path: "/apps/Hub/Tasks", pathWithParams: "/apps/Hub/Tasks", savedFinds: [
        { name: "F1", title: "Mine", path: "/apps/Hub/Tasks", pathWithParams: "/apps/Hub/Tasks?formListFindId=F1", active: active },
        { name: "F2", title: "Late", path: "/apps/Hub/Tasks", pathWithParams: "/apps/Hub/Tasks?formListFindId=F2" }] });
    let rows = moqui.drawerRows([appsRoot("Hub"), hubApp("Tasks"), finds(false)]);
    assert.deepEqual(brief(rows).slice(2, 5), [["screen", 1, "Tasks", true, true, true], ["find", 2, "Mine", false, false, true],
        ["find", 2, "Late", false, false, true]]);
    rows = moqui.drawerRows([appsRoot("Hub"), hubApp("Tasks"), finds(true)]);
    assert.deepEqual(brief(rows).slice(2, 4), [["screen", 1, "Tasks", true, false, true], ["find", 2, "Mine", false, true, true]]);
    assert.equal(rows[3].to, "/apps/Hub/Tasks?formListFindId=F1");
    assert.notEqual(rows[3].key, rows[4].key);
});

test("a menu whose root has no subscreens starts with the root as its app", () => {
    assert.deepEqual(brief(moqui.drawerRows([{ name: "Hub", title: "Hub", path: "/apps/Hub" }])), [["app", 0, "Hub", true, true, true]]);
    const rows = moqui.drawerRows([{ name: "Hub", title: "Hub", path: "/apps/Hub" }, { name: "Home", title: "Home", path: "/apps/Hub/Home" }]);
    assert.deepEqual(brief(rows), [["app", 0, "Hub", true, false, true], ["screen", 1, "Home", true, true, true]]);
});

test("a handheld screen's density comes from the deepest screen of the path that sets one, and wins over compact", () => {
    assert.equal(moqui.screenDensity([{ name: "apps" }, { name: "Warehouse", density: "handheld" }, { name: "Move" }]), "handheld");
    assert.equal(moqui.screenDensity([{ name: "apps" }, { name: "Warehouse" }]), "");
    assert.equal(moqui.screenDensity(null), "");
    const body = document.body;
    moqui.applyPreference("SHELL_DENSITY", "compact");
    assert.ok(body.classList.contains("m-density-compact"));
    moqui.setScreenDensity("handheld");
    assert.ok(body.classList.contains("m-density-handheld"));
    assert.ok(!body.classList.contains("m-density-compact"), "the screen's density wins over the preference");
    moqui.applyPreference("SHELL_DENSITY", "compact");
    assert.ok(!body.classList.contains("m-density-compact"), "still while the handheld screen is shown");
    moqui.setScreenDensity("");
    assert.ok(!body.classList.contains("m-density-handheld"));
    assert.ok(body.classList.contains("m-density-compact"), "leaving it puts the preference back");
    moqui.applyPreference("SHELL_DENSITY", null);
    assert.ok(!body.classList.contains("m-density-compact"));
});

test("a scan's outcome is none, one or many matches; a match needs an id", () => {
    assert.equal(qvt.scanOutcome(null).kind, "none");
    assert.equal(qvt.scanOutcome({ matches: [{ identityType: "asset" }] }).kind, "none");
    const one = qvt.scanOutcome({ matches: [{ identityType: "asset", id: "A1" }, { label: "no id" }] });
    assert.equal(one.kind, "one");
    assert.equal(one.match.id, "A1");
    const many = qvt.scanOutcome({ matches: [{ id: "A1" }, { id: "LOT1" }] });
    assert.equal(many.kind, "many");
    assert.equal(many.matches.length, 2);
    assert.equal(qvt.scanMatchText({ id: "A1", label: "Asset 1", detail: "Bin 7" }), "Asset 1 · Bin 7");
    assert.equal(qvt.scanMatchText({ id: "A1" }), "A1");
});

test("the same text within two seconds is the same scan intent; other text or later is a new one", () => {
    let next = 0;
    const makeId = () => "S" + (++next);
    const first = qvt.scanIntent(null, "A1", 1000, makeId);
    assert.deepEqual(first, { text: "A1", id: "S1", time: 1000, repeat: false });
    const twice = qvt.scanIntent(first, "A1", 2500, makeId);
    assert.deepEqual(twice, { text: "A1", id: "S1", time: 1000, repeat: true }, "the window is from the first scan");
    assert.equal(qvt.scanIntent(twice, "A1", 3100, makeId).id, "S2", "a repeat does not slide the window");
    assert.equal(qvt.scanIntent(twice, "B2", 2600, makeId).id, "S3", "other text is a new intent");
    assert.ok(qvt.scanId() !== qvt.scanId());
});

test("a random id is a version 4 UUID, from getRandomValues where randomUUID is missing (an insecure origin)", () => {
    const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
    assert.match(moqui.randomId(), uuid);
    Object.defineProperty(crypto, "randomUUID", { value: undefined, configurable: true });
    try {
        const one = moqui.randomId();
        assert.match(one, uuid);
        assert.notEqual(one, moqui.randomId());
    } finally {
        delete crypto.randomUUID;
    }
    assert.equal(typeof crypto.randomUUID, "function", "randomUUID is back");
});

// m-scan-input's methods over a stand-in for the component and its form
const scanVm = (extra) => {
    const methods = qvt.components["m-scan-input"].methods;
    const vm = Object.assign({ text: "", loading: false, message: "", messageType: "", matches: [], chooser: false, pending: null, done: null,
        last: null, firstKey: 0, parsed: null, name: "assetId", scanIdField: "scanId", confirm: false, autoSubmit: false, emitted: [],
        mForm: { fields: { assetId: "OLD", assetId_type: "asset", assetId_scanText: "OLD", scanId: "S0" } },
        $emit(name, value) { this.emitted.push([name, value]); }, $nextTick(fn) { fn(); }, $refs: {} }, extra);
    Object.keys(methods).forEach((key) => { vm[key] = methods[key].bind(vm); });
    return vm;
};

test("a new scan clears the last one's fields; a late answer for a replaced intent is dropped", () => {
    const vm = scanVm({ resolveUrl: "/scanResolve", resolveParameters: {} });
    const answers = [];
    const savedRequest = moqui.request;
    moqui.request = () => new Promise((resolve) => answers.push(resolve));
    try {
        vm.resolve("A1");
        assert.deepEqual(vm.mForm.fields, { assetId: "", assetId_type: "", assetId_scanText: "", scanId: "" });
        const first = vm.last;
        vm.resolve("B2");
        assert.notEqual(vm.last.id, first.id);
        assert.ok(!vm.isCurrent(first), "the A1 answer is late now");
        assert.ok(vm.isCurrent(vm.last));
    } finally {
        moqui.request = savedRequest;
    }
});

test("a confirm match waits with the focus in the field; Enter on its text confirms, a double scan says Already scanned", () => {
    const vm = scanVm({ confirm: true });
    const intent = { text: "B-01-07", id: "S7", time: Date.now(), repeat: false };
    vm.last = intent;
    vm.answered({ matches: [{ identityType: "location", id: "B-01-07" }] }, intent);
    assert.equal(vm.pending.match.id, "B-01-07");
    assert.equal(vm.mForm.fields.assetId, "OLD", "nothing is filled before Confirm");
    vm.text = "B-01-07";
    vm.onKey({ key: "Enter", preventDefault() {}, stopPropagation() {} });
    assert.equal(vm.pending, null);
    assert.equal(vm.mForm.fields.assetId, "B-01-07");
    assert.equal(vm.mForm.fields.scanId, "S7");
    assert.equal(vm.done, "S7");
    vm.resolve("B-01-07");
    assert.match(vm.message, /Already scanned B-01-07/);
    // the form's answer took the scan: a later submit carries no scan id
    vm.onFormAnswer({ ok: false, form: vm.mForm });
    assert.equal(vm.mForm.fields.scanId, "S7", "a refused answer keeps the scan for a retry");
    vm.onFormAnswer({ ok: true, form: vm.mForm });
    assert.equal(vm.mForm.fields.scanId, "");
    assert.equal(vm.mForm.fields.assetId, "");
});

test("a Tab ends a scan only after a burst of keys", () => {
    assert.ok(qvt.scanEndsWithTab(12, 1000, 1200), "12 keys in 200 ms: a scanner");
    assert.ok(!qvt.scanEndsWithTab(12, 1000, 3000), "12 keys in 2 s: a person");
    assert.ok(!qvt.scanEndsWithTab(2, 1000, 1010), "too short to be a code");
    assert.ok(!qvt.scanEndsWithTab(12, 0, 1000), "no keys timed");
});
