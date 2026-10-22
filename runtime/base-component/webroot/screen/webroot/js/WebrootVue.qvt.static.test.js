/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// The client of server-static qvt (plans/QvtServerStatic.md) without a browser: the script the server sends for crates/moqui-core's
// Static/Items fixture is registered and run on the real Vue runtime with a fake DOM, against the package and the rows the server answered
// for the same fixture (testdata/Static/golden, checked by the Rust tests). The restricted ec, the scope chain, the helpers, the static form-list
// and the handler that loads the script are checked too.
// Run: node --test runtime/base-component/webroot/screen/webroot/js/WebrootVue.qvt.static.test.js
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const webroot = path.join(__dirname, "..");
const golden = path.join(__dirname, "../../../../../../crates/moqui-core/src/screen/testdata/Static/golden");
vm.runInThisContext(fs.readFileSync(path.join(webroot, "libs/vue/vue.global.prod.js"), "utf8"));
globalThis.moqui = require("./MoquiLib.js");
globalThis.moqui.l10n = require("./MoquiL10n.js");
require("./WebrootVue.qvt.js");
const { Vue } = globalThis;
const qvt = moqui.qvt;

const goldenText = (name) => fs.readFileSync(path.join(golden, name), "utf8");
const scriptText = goldenText("Items.script.js");
const pkg = JSON.parse(goldenText("Items.package.json"));
const formPkg = JSON.parse(goldenText("Items.ItemList.json"));
const multiPkg = JSON.parse(goldenText("Items.ItemMulti.json"));

// ---- a Vue renderer over plain objects ----

const nodeOps = {
    patchProp(el, key, prev, next) { if (next === null || next === undefined) delete el.props[key]; else el.props[key] = next; },
    insert(child, parent, anchor) {
        if (child.parent) child.parent.children.splice(child.parent.children.indexOf(child), 1);
        const at = anchor ? parent.children.indexOf(anchor) : parent.children.length;
        parent.children.splice(at, 0, child);
        child.parent = parent;
    },
    remove(child) { if (child.parent) child.parent.children.splice(child.parent.children.indexOf(child), 1); child.parent = null; },
    createElement(tag) { return { tag, props: {}, children: [], parent: null }; },
    createText(text) { return { text, parent: null }; },
    createComment(text) { return { comment: text, parent: null }; },
    setText(node, text) { node.text = text; },
    setElementText(el, text) { el.children = [{ text, parent: el }]; },
    parentNode(node) { return node.parent; },
    nextSibling(node) { const siblings = node.parent ? node.parent.children : []; return siblings[siblings.indexOf(node) + 1] || null; },
    querySelector() { return null; }
};
const renderer = Vue.createRenderer(nodeOps);

const html = (node) => {
    if (node.text !== undefined) return node.text;
    if (node.comment !== undefined) return "";
    const attrs = Object.keys(node.props).filter((key) => typeof node.props[key] !== "function")
        .map((key) => " " + key + "='" + (typeof node.props[key] === "object" ? JSON.stringify(node.props[key]) : String(node.props[key])) + "'").join("");
    return "<" + node.tag + attrs + ">" + node.children.map(html).join("") + "</" + node.tag + ">";
};

// the m-* wrappers are stubs that write their attributes; the form keeps its fields as the real one does
const leaf = (tag) => ({ inheritAttrs: false, render() { return Vue.h(tag, this.$attrs, this.$slots.default ? this.$slots.default() : undefined); } });
const stubs = {
    "m-form": {
        inheritAttrs: false, props: ["fieldsInitial"],
        data() { return { fields: Object.assign({}, this.fieldsInitial) }; },
        render() { return Vue.h("m-form", { name: this.$attrs.name, fields: this.fields }, this.$slots.default({ fields: this.fields, hasFieldsChanged: false })); }
    },
    "m-form-paginate": { props: ["paginate", "formList"], render() { if (this.formList) captured.lists[this.formList.name] = this.formList; return Vue.h("m-form-paginate", { paginate: this.paginate }); } },
    "m-form-go-page": { props: ["idVal", "formList"], render() { return Vue.h("m-form-go-page"); } }
};
["m-display", "m-text-line", "m-text-area", "m-drop-down", "m-link", "q-btn", "m-form-link", "m-container-box", "m-container-dialog",
    "q-list", "q-item", "q-item-section", "q-expansion-item", "q-badge", "q-tooltip"].forEach((tag) => { stubs[tag] = leaf(tag); });
stubs["q-btn-dropdown"] = { inheritAttrs: false, render() {
    return Vue.h("q-btn-dropdown", this.$attrs, [this.$slots.label ? Vue.h("label", {}, this.$slots.label()) : null].concat(this.$slots.default ? this.$slots.default() : []));
} };

const captured = { lists: {} };
// the checkbox set records its props and gives its slot the selection state the real one does
stubs["m-checkbox-set"] = {
    props: ["checkboxCount", "checkboxParameter", "checkboxListMode", "checkboxValues"],
    data() { return { checkboxStates: new Array(this.checkboxCount).fill(false), checkboxAllState: false }; },
    render() {
        captured.checkboxSet = this;
        return Vue.h("m-checkbox-set", {}, this.$slots.default({ checkboxStates: this.checkboxStates, checkboxAllState: this.checkboxAllState,
            setCheckboxAllState: () => {}, clickCheckbox: () => {}, addCheckboxParameters: () => true }));
    }
};
const answers = {};
const requests = [];
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

const setup = () => {
    requests.length = 0;
    moqui.request = (url, options) => {
        requests.push({ url, params: options && options.params });
        const body = answers[url];
        if (body === undefined) return Promise.reject({ status: 404, text: "no answer for " + url });
        return Promise.resolve({ ok: true, status: 200, text: JSON.stringify(body), json: body, headers: new Map() });
    };
    moqui.config = { userId: "EX_JOHN_DOE", username: "john.doe", locale: "en_US", timeZone: "UTC", userFullName: "John Doe",
        userAccount: { partyId: "P1", emailAddress: "j@example.com", currentPassword: "NOPE", passwordSalt: "NOPE" }, preferences: { QUASAR_DARK: "false" } };
    qvt.config = moqui.config;
    qvt.hostEc = null;
    moqui.l10n.setLocale("en_US");
    moqui.l10n.setTimeZone("UTC");
    moqui.l10n.setCatalog({});
    answers["/Items/actions"] = pkg;
    answers["/Items/actions/ItemList"] = formPkg;
    answers["/Items/actions/ItemMulti"] = multiPkg;
};

const registerScript = (key, text) => {
    const at = key || "/Items";
    delete moqui.qvtStatic[at];
    vm.runInThisContext(text || scriptText);
    return moqui.qvtStatic[at];
};

// The fixture of a key: its script, package and form answers are the goldens of the Rust tests.
const fixture = (name) => {
    const text = goldenText(name + ".script.js");
    answers["/" + name + "/actions"] = JSON.parse(goldenText(name + ".package.json"));
    fs.readdirSync(golden).filter((file) => file.startsWith(name + ".") && file.endsWith(".json") && file !== name + ".package.json")
        .forEach((file) => { answers["/" + name + "/actions/" + file.slice(name.length + 1, -5)] = JSON.parse(goldenText(file)); });
    return text;
};

const mountStatic = async (search, name) => {
    const key = "/" + (name || "Items");
    const factory = registerScript(key, name ? fixture(name) : scriptText);
    const comp = qvt.staticComponent(key, factory, { path: key, search: search || "", divId: (name || "Items") + "Div" });
    const app = renderer.createApp(comp);
    Object.keys(qvt.components).forEach((name) => { if (!stubs[name]) app.component(name, qvt.components[name]); });
    Object.keys(stubs).forEach((name) => app.component(name, stubs[name]));
    const container = nodeOps.createElement("root");
    app.mount(container);
    await flush();
    await flush();
    return { app, container };
};

test("the script registers a render function and holds no request data", () => {
    const factory = registerScript();
    assert.equal(typeof factory, "function");
    assert.equal(typeof factory(Vue), "function");
    assert.ok(!/\bwith\s*\(/.test(scriptText) && !scriptText.includes("new Function") && !scriptText.includes("eval("));
    assert.ok(!scriptText.includes("ZEBRA"));
});

test("the screen is fetched from actions, and renders with the package and the form rows", async () => {
    setup();
    const { container } = await mountStatic("itemId=I9&page=2");
    assert.equal(requests[0].url, "/Items/actions");
    assert.deepEqual(requests[0].params, { itemId: "I9", page: "2" });
    assert.deepEqual(requests.slice(1).map((request) => request.url).sort(), ["/Items/actions/ItemList", "/Items/actions/ItemMulti"]);
    const text = html(container);
    // the label has the package's greeting through ec.resource.expand with its expression table
    assert.match(text, /<h4 class='text-inline' textContent='Hello ZEBRA greeting for Ada Lovelace'><\/h4>/);
    // the container div has the id of its place
    assert.match(text, /<div id='ItemsDiv'>/);
    // the form-single reads its map: the item
    assert.match(text, /<m-form name='ItemForm' fields=/);
    assert.match(text, /"name":"ZEBRA Widget"/);
    assert.match(text, /"itemId":"ZEBRA-I1"/);
    // the decimal price of the item is formatted from text; the conditional field picks the branch that holds
    assert.match(text, /"price_display":"1,234.50"/);
    assert.match(text, /"mode_display":"is active: ZEBRA Widget"/);
    // section-iterate pushes the entry: a v-for over tags with the entry and the form's map
    assert.match(text, /textContent='Tag ZEBRA-t1 of ZEBRA Widget'/);
    assert.match(text, /textContent='Tag ZEBRA-t2 of ZEBRA Widget'/);
    // the link has its path and its parameter from the scope
    assert.match(text, /href='\/Items\/viewItem\?itemId=ZEBRA-I1'/);
    // each row of the form-list pushes the row on the scope: its own fields start the row form
    assert.match(text, /"itemId":"ZEBRA-R1","itemId_display":"ZEBRA-R1","name":"ZEBRA row one"/);
    assert.match(text, /"itemId":"ZEBRA-R2"/);
    assert.match(text, /"price_display":"12.50"/);
    // the column titles are localized in the browser
    assert.match(text, /textContent='Item ID'/);
});

test("paging and sorting a static form-list change the request and fetch again", async () => {
    setup();
    answers["/Items/actions/ItemList"] = Object.assign({}, formPkg, { itemListCount: 45, itemListPageIndex: 1, itemListPageSize: 20, itemListPageMaxIndex: 2, itemListPageRangeLow: 21, itemListPageRangeHigh: 40 });
    const { container } = await mountStatic("itemGroup=G1");
    // the page keys of the answer are the paginate of the page buttons
    assert.match(html(container), /<m-form-paginate paginate='\{"count":45,"pageIndex":1,"pageSize":20,"pageMaxIndex":2,"pageRangeLow":21,"pageRangeHigh":40\}'/);
    const list = captured.lists.ItemList;
    assert.ok(list, "the page buttons got the form-list");
    const listRequests = () => requests.filter((request) => request.url === "/Items/actions/ItemList");
    assert.equal(listRequests().length, 1);
    list.setPageIndex(1);
    await flush();
    assert.equal(listRequests().length, 2);
    assert.deepEqual(listRequests()[1].params, { itemGroup: "G1", pageIndex: 1 });
    list.sortBy("-itemId");
    await flush();
    assert.deepEqual(listRequests()[2].params, { itemGroup: "G1", pageIndex: 0, orderByField: "-itemId" });
    assert.equal(list.orderBy("-itemId"), "desc");
    assert.equal(list.orderBy("itemId"), "");
    // the screen package is not asked for again
    assert.equal(requests.filter((request) => request.url === "/Items/actions").length, 1);
});

test("a multi form-list is one form with each row's fields under name_rowIndex, and the nav row has the exports", async () => {
    setup();
    const { container } = await mountStatic("itemGroup=G1");
    const text = html(container);
    // one m-form for the multi list: its fields are every row's, numbered, with the form name and the multi flag
    const multi = text.match(/<m-form name='ItemMulti' fields='([^']*)'/);
    assert.ok(multi, text);
    const fields = JSON.parse(multi[1]);
    assert.equal(fields.moquiFormName, "ItemMulti");
    assert.equal(fields._isMulti, "true");
    assert.equal(fields.name_0, "ZEBRA row one");
    assert.equal(fields.name_1, "ZEBRA row two");
    assert.equal(fields.itemId_1, "ZEBRA-R2");
    assert.ok(!("name" in fields), "a multi row writes its own numbered fields");
    // the row widgets bind through the row's formProps: the second row's text-line shows the second row's name
    const list = captured.lists.ItemMulti;
    const second = list.rowScope(1, qvt.makeScope(() => ({}), null), { fields });
    assert.equal(second.formProps.fields.name, "ZEBRA row two");
    second.formProps.fields.name = "changed";
    assert.equal(fields.name_1, "changed");
    // the csv item of the export menu of ItemList goes to the screen in csv with the find parameters
    // (v-close-popup compiles to a directive, so it does not show in this props-only render)
    assert.match(text, /<q-item clickable='' tag='a' href='\/Items\?itemGroup=G1&renderMode=csv&pageNoLimit=true&lastStandalone=true&saveFilename=ItemList.csv'/);
});

test("the column picker lays out the user's columns from the package, and its dialog tree lists the rest as not shown", async () => {
    setup();
    answers["/Items/actions/ItemList"] = Object.assign({}, formPkg, { mqColumns: [["name", "itemId"], ["price"]] });
    const { container } = await mountStatic("");
    const list = captured.lists.ItemList;
    assert.deepEqual(list.shownColumns(), [["name", "itemId"], ["price"]]);
    const tree = list.columnsTree();
    assert.deepEqual(tree.map((column) => column.id), ["hidden", "column_0", "column_1"]);
    assert.deepEqual(tree[0].children.map((field) => field.id), ["go"]);
    assert.deepEqual(tree[1].children, [{ id: "name", label: "Name" }, { id: "itemId", label: "Item ID" }]);
    // a row is a td for each column: the name's text-line comes before the id's display in the first, and the save button is not shown
    const text = html(container);
    const row = text.slice(text.indexOf("<m-form name='ItemList_0'"));
    const firstTd = row.slice(row.indexOf("<div class='td'>"), row.indexOf("</div></div>"));
    assert.ok(firstTd.indexOf("<m-text-line") >= 0 && firstTd.indexOf("<m-text-line") < firstTd.indexOf("<m-display"), firstTd);
    assert.ok(!/ItemList_go/.test(row.slice(0, row.indexOf("<m-form name='ItemList_1'"))), "the go field is not in a shown column");
    // the row form starts from the row's values, with the form name
    assert.match(text, /<m-form name='ItemList_0' fields='\{"itemId":"ZEBRA-R1","itemId_display":"ZEBRA-R1","name":"ZEBRA row one"/);
});

test("the show-total row of the form request is drawn after the rows, in bold", async () => {
    setup();
    assert.ok(formPkg.itemListTotal, "the form request answers the total row");
    const { container } = await mountStatic("");
    const text = html(container);
    assert.match(text, /<div class='tr text-weight-bold'><m-form name='ItemList_2' fields='[^']*"price_display":"15.50"/);
    // the data rows stay two: the total is not a row of the list
    assert.equal(formPkg.itemList.length, 2);
});

test("$mq.compare gives what the server's compare does for the same vectors", () => {
    const vectors = JSON.parse(fs.readFileSync(path.join(__dirname, "../../../../../../crates/moqui-lib/testdata/compare-vectors.json"), "utf8"));
    vectors.forEach(([field, operator, to, type, expected]) => {
        assert.equal(qvt.helpers.compare(field, operator, to, null, null, type), expected, JSON.stringify([field, operator, to, type]));
    });
});

test("Parity: a condition element is a v-if over and, or, not, compare and expression, with its fail-widgets", async () => {
    setup();
    const { container } = await mountStatic("", "Parity");
    const text = html(container);
    assert.match(text, /the condition holds/);
    assert.doesNotMatch(text, /the condition fails/);
    assert.match(text, /the second fails/);
    assert.doesNotMatch(text, /the second holds/);
});

test("Parity: text-map puts the map's entries before the scope's for the text of a label and a link", async () => {
    setup();
    const { container } = await mountStatic("", "Parity");
    const text = html(container);
    assert.match(text, /Product Widget X at 9/);
    assert.match(text, /View Widget X/);
    assert.doesNotMatch(text, /the context's name/);
});

test("Parity: a field accordion opens its active group, and a button-menu lists the children whose condition holds", async () => {
    setup();
    const { container } = await mountStatic("", "Parity");
    const text = html(container);
    const items = text.match(/<q-expansion-item[^>]*>/g) || [];
    assert.equal(items.length, 2, text);
    assert.match(items[0], /label='Names'/);
    assert.doesNotMatch(items[0], /default-opened/);
    assert.match(items[1], /label='Amounts'/);
    assert.match(items[1], /default-opened/);
    assert.match(items[1], /group='ProductForm_accordion'/);
    // the menu's label is the text with the map's entry, its items the link and the label whose condition holds
    assert.match(text, /<q-btn-dropdown[^>]*>.*Menu of Widget X/);
    assert.match(text, /View it/);
    assert.match(text, /shown when qty is over 3/);
    assert.doesNotMatch(text, /hidden when qty is over 3/);
});

test("Parity: an aggregated list writes the sub-list titles once in the head and each row's sub-rows with their own values", async () => {
    setup();
    const { container } = await mountStatic("", "Parity");
    const text = html(container);
    const table = text.slice(text.indexOf("<div class='table q-table' id='OrderList_table'>"));
    assert.equal((table.match(/class='tr sub-header'/g) || []).length, 1, table);
    assert.match(table, /class='tr sub-header'.*Item.*Qty/);
    assert.equal((table.match(/class='tr sub-rows'/g) || []).length, 2);
    // the first order's sub-rows: Gear and Bolt, then the total of the quantities
    const first = table.slice(table.indexOf("class='tr sub-rows'"));
    assert.ok(first.indexOf("Gear") >= 0 && first.indexOf("Gear") < first.indexOf("Bolt"), first);
    assert.match(first, /textContent='7'|>7</);
    assert.match(table, /Nut/);
});

test("Parity: the first row is a form of its own over map-first-row, and the last row follows the rows", async () => {
    setup();
    const { container } = await mountStatic("", "Parity");
    const text = html(container);
    const table = text.slice(text.indexOf("<div class='table q-table' id='EditList_table'>"));
    const firstForm = table.match(/<m-form name='EditList_first' fields='([^']*)'/);
    assert.ok(firstForm, table);
    const fields = JSON.parse(firstForm[1]);
    assert.equal(fields.itemId, "NEW");
    assert.equal(fields.name, "new item name");
    assert.equal(fields.secret, "S1");
    assert.equal(fields.moquiFormName, "EditList");
    // the first row comes before the data rows, the last after them
    assert.ok(table.indexOf("EditList_first") < table.indexOf("EditList_0"), table);
    assert.ok(table.indexOf("the last row") > table.indexOf("EditList_1"), table);
    assert.match(table, /class='tr first'/);
    assert.match(table, /class='tr last'/);
});

test("Parity: row selection is a checkbox set over the rows' ids, a checkbox for each row, and the actions posting through it", async () => {
    setup();
    const { container } = await mountStatic("", "Parity");
    const text = html(container);
    const set = captured.checkboxSet;
    assert.ok(set, "the list is in a checkbox set");
    assert.deepEqual(set.checkboxValues, ["E1", "E2"]);
    assert.equal(set.checkboxParameter, "itemId");
    assert.equal(set.checkboxCount, 2);
    const table = text.slice(text.indexOf("<div class='table q-table' id='PickList_table'>"));
    assert.equal((table.match(/<q-checkbox dense='' size='sm'/g) || []).length, 2, table);
    // the menu of the head holds the action: its dialog and the form-single that posts the selected rows
    assert.match(table, /<m-container-dialog[^>]*button-text='Archive'/);
    assert.match(table, /<m-form name='ArchiveForm'/);
});

test("the checkbox mixin posts the selected ids as name_i with _isMulti, or one list value, and nothing when none is selected", () => {
    const methods = moqui.checkboxSetMixin.methods;
    const state = (states, listMode) => ({ checkboxValues: ["a", "b", "c"], checkboxStates: states, checkboxParameter: "id", checkboxListMode: listMode, getCheckboxValueArray: methods.getCheckboxValueArray });
    const formData = () => { const map = {}; return { map, set: (key, value) => { map[key] = value; } }; };
    const multi = formData();
    assert.equal(methods.addCheckboxParameters.call(state([false, true, true], false), multi), true);
    assert.deepEqual(multi.map, { id_0: "b", id_1: "c", _isMulti: "true" });
    const list = formData();
    methods.addCheckboxParameters.call(state([true, false, true], true), list);
    assert.deepEqual(list.map, { id: "a,c" });
    const none = formData();
    assert.equal(methods.addCheckboxParameters.call(state([false, false, false], false), none), false);
    assert.deepEqual(none.map, {});
});

test("Parity: a paginated section-iterate shows its page and the page buttons over the package's keys", async () => {
    setup();
    const { container } = await mountStatic("", "Parity");
    const text = html(container);
    assert.equal((text.match(/line L\d+/g) || []).length, 20);
    assert.match(text, /<m-form-paginate paginate='\{"count":25,"pageIndex":0,"pageSize":20,"pageMaxIndex":1,"pageRangeLow":1,"pageRangeHigh":20\}'/);
});

test("the restricted ec has the table of members and throws on anything else", () => {
    setup();
    const ec = qvt.makeEc(moqui.config);
    assert.equal(ec.user.username, "john.doe");
    assert.equal(ec.user.userId, "EX_JOHN_DOE");
    assert.equal(ec.user.locale, "en_US");
    assert.equal(ec.user.timeZone, "UTC");
    assert.equal(ec.user.userAccount.partyId, "P1");
    assert.equal(ec.user.userAccount.userFullName, "John Doe");
    assert.ok(!("currentPassword" in ec.user.userAccount) && !("passwordSalt" in ec.user.userAccount), "credential columns stay on the server");
    assert.equal(ec.user.getPreference("QUASAR_DARK"), "false");
    assert.equal(ec.user.getPreference("NO_SUCH"), null);
    assert.equal(ec.l10n.format("1234.125", "#,##0.00"), "1,234.12");
    assert.equal(ec.l10n.format("2024-03-05 14:07:00.0", "yyyy-MM-dd HH:mm"), "2024-03-05 14:07");
    assert.equal(ec.l10n.formatCurrency("1234.5", "USD"), "$1,234.50");
    assert.equal(ec.l10n.formatCurrencyNoSymbol("1234.5", "USD"), "1,234.50");
    assert.equal(ec.l10n.localize("Plain"), "Plain");
    // anything else throws with a message that says what to do
    for (const area of ["entity", "web", "context", "service", "message", "screen", "logger"]) {
        assert.throws(() => ec[area], new RegExp("ec\\." + area + " is not available in a server-static qvt screen"));
    }
    assert.throws(() => ec.entity.find("A"), /ec\.entity is not available/);
    assert.equal(ec.then, undefined);
});

test("the browser utilities give what the server's do", () => {
    setup();
    // the same vectors as moqui_lib::string::camel_case_to_pretty
    assert.equal(qvt.StringUtilities.camelCaseToPretty("orderId"), "Order ID");
    assert.equal(qvt.StringUtilities.camelCaseToPretty("foo.bar"), "Foo Bar");
    assert.equal(qvt.StringUtilities.camelCaseToPretty("productStoreId"), "Product Store ID");
    assert.equal(qvt.StringUtilities.camelCaseToPretty("toFieldNameAlias"), "To Field Name Alias");
    assert.equal(qvt.StringUtilities.camelCaseToPretty("aaAa"), "Aa");
    assert.equal(qvt.StringUtilities.camelCaseToPretty("item2Id"), "Item 2 ID");
    assert.equal(qvt.StringUtilities.camelCaseToPretty("#hash"), "Hash");
    assert.equal(qvt.StringUtilities.camelCaseToPretty(""), "");
    assert.equal(qvt.StringUtilities.camelCaseToPretty(null), "");
    assert.equal(qvt.makeEc({ moquiVersion: "1.2.3" }).factory.moquiVersion, "1.2.3");
});

test("ec.resource.expand localizes the pattern, then reads the expressions from the table", () => {
    setup();
    moqui.l10n.setCatalog({ "Total ${sum} for ${who}": "Summe ${sum} fuer ${who}" });
    const ec = qvt.makeEc(moqui.config);
    const table = { sum: () => 12.5, who: () => "Ada" };
    assert.equal(ec.resource.expand("Total ${sum} for ${who}", table), "Summe 12.5 fuer Ada");
    assert.equal(ec.resource.expandNoL10n("Total ${sum} for ${who}", table), "Total 12.5 for Ada");
    assert.equal(ec.resource.expand("no expression"), "no expression");
    assert.equal(ec.resource.expand("price $"), "price $");
    assert.equal(ec.resource.expand("a ${x} b", { x: () => null }), "a  b");
    // an expression that the template did not carry cannot be evaluated: no eval
    assert.throws(() => ec.resource.expand("Total ${other}", table), /not part of this screen's template/);
    // braces and quotes inside an expression are parsed
    assert.equal(ec.resource.expand("v ${ a.b('}') }", { "a.b('}')": () => "ok" }), "v ok");
    moqui.l10n.setCatalog({});
});

test("a scope looks in the extra map, the map, then the scope around it, and $scope is the scope", () => {
    const outer = qvt.makeScope({ a: 1, b: 2, shared: "outer" }, null);
    const inner = qvt.makeScope({ b: 20, shared: "inner" }, outer, { shared: "extra" });
    assert.equal(inner.a, 1);
    assert.equal(inner.b, 20);
    assert.equal(inner.shared, "extra");
    assert.equal(outer.shared, "outer");
    assert.equal(inner.$scope, inner);
    assert.equal(inner["not here"], undefined);
    assert.ok("a" in inner && !("zzz" in inner));
    const lazy = qvt.makeScope(() => ({ q: 5 }), outer);
    assert.equal(lazy.q, 5);
    inner.b = 21;
    assert.equal(inner.b, 21);
    inner.a = 9;
    assert.equal(outer.a, 9);
});

test("a merged template calls $mq on the Vue app, not the page global", () => {
    // The browser compiler resolves $mq through the render proxy. That proxy does not see the page
    // global, so the assertion deletes it: a pass has to come from app.config.globalProperties.
    moqui.thousandSeparator = ",";
    moqui.decimalSeparator = ".";
    const render = Vue.compile("<span>{{ $mq.isStringNumber(value) }}</span>");
    const saved = globalThis.$mq;
    delete globalThis.$mq;
    const textFor = (value) => {
        const app = renderer.createApp({ data: () => ({ value }), render });
        app.config.globalProperties.$mq = moqui;
        const container = nodeOps.createElement("root");
        app.mount(container);
        const text = html(container);
        app.unmount();
        return text;
    };
    try {
        assert.match(textFor("1,234.50"), />true</);
        assert.match(textFor("1a"), />false</);
    } finally {
        globalThis.$mq = saved;
    }
});

test("a number field's rule runs over the scope, as the compiled template does", () => {
    // The server compiles the template with Vue's prefixIdentifiers, so the rule is over _ctx, which is
    // the scope: the scope's $mq has the number checks and the scope's ec localizes the message.
    const scope = qvt.makeScope({}, null, { ec: qvt.ec(), $mq: qvt.helpers });
    const rule = (value) => !value || scope.$mq.isStringNumber(value) || scope.ec.l10n.localize("Please enter a valid number");
    // a rule returns true when the value is good, and the message when it is not
    assert.equal(rule(""), true);
    assert.equal(rule("12"), true);
    assert.equal(rule("1,234.50"), true);
    assert.equal(rule("1a"), "Please enter a valid number");
});

test("the helpers a template calls", () => {
    const mq = qvt.helpers;
    assert.equal(mq.str(null), "");
    assert.equal(mq.str(0), "0");
    assert.equal(mq.dflt("", "x"), "x");
    assert.equal(mq.dflt("v", "x"), "v");
    assert.equal(mq.nul(""), null);    // the number checks of the field rules the server writes, which a static template calls as $mq
    assert.ok(mq.isStringNumber("1,234.50") && mq.isStringNumber("-5") && !mq.isStringNumber("1a") && !mq.isStringNumber(5));
    assert.ok(mq.isStringInteger("1,234") && !mq.isStringInteger("1.5"));
    assert.equal(mq.parseNumber("1,234.5"), 1234.5);
    assert.equal(mq.parseNumber("1,234"), 1234);
    assert.deepEqual(mq.cur("A,B", true, ""), ["A", "B"]);
    assert.equal(mq.cur("A,B", false, ""), "A");
    assert.equal(mq.cur("[A, B]", false, ""), "A");
    assert.equal(mq.cur(null, false, "N/A"), "N/A");
    assert.deepEqual(mq.cur(["x"], true, ""), ["x"]);
    assert.equal(mq.fmt("1234.5", "#,##0.00"), "1,234.50");
    assert.equal(mq.fmt("2024-03-05", "MM/dd/yyyy"), "03/05/2024");
    assert.equal(mq.fmt("2024-03-05 14:07:09.0", "yyyy-MM-dd HH:mm", "date-time"), "2024-03-05 14:07");
    assert.equal(mq.fmt("12:30:00", "HH:mm"), "12:30");
    assert.equal(mq.fmt("", "#"), "");
    // epoch millis text, as a time stamp is in the data: with any date pattern, and with none (as ec.l10n.format on the server)
    assert.equal(mq.fmt("1709647629000", "h:mm a"), "2:07 PM");
    assert.equal(mq.fmt("1709647629000", null), "2024-03-05 14:07");
    assert.equal(mq.fmt("100050", null), "100050");
    assert.equal(mq.fmt("1709647629000", "#,##0"), "1,709,647,629,000");
    assert.equal(mq.url("/a/b", { x: "1 2", y: null, z: "" }, { m: "q&r" }), "/a/b?m=q%26r&x=1%202");
    assert.equal(mq.url("/a", {}, null), "/a");
    // a url a ${} expression gave, resolved against the screen's path as the server resolves a link's url
    assert.equal(mq.link("Detail?x=1", "/apps/gallery/Landing"), "/apps/gallery/Landing/Detail?x=1");
    assert.equal(mq.link("../Orders/./Find", "/apps/gallery/Landing"), "/apps/gallery/Orders/Find");
    assert.equal(mq.link(".", "/apps/gallery/Landing"), "/apps/gallery/Landing");
    assert.equal(mq.link("/qapps/Sales/Order", "/apps/x"), "/qapps/Sales/Order");
    assert.equal(mq.link("https://example.com/late", "/apps/x"), "https://example.com/late");
    assert.equal(mq.link("", "/apps/x"), "/apps/x");
    assert.equal(mq.link("Orders?statusId=Held", "/"), "/Orders?statusId=Held");
    // a link's target parameters sit under its parameter-map and its own parameters
    assert.equal(mq.url("/a", { id: "own" }, mq.merge({ id: "target", page: "2", none: null }, { page: "3" })), "/a?id=own&page=3");
    assert.deepEqual(mq.given({ a: "1", b: null, c: "", d: undefined, e: 0 }), { a: "1", e: 0 });
    assert.deepEqual(mq.merge(mq.given({ a: "", b: "2" }), null, { a: "x" }), { b: "2", a: "x" });
    assert.deepEqual(mq.shown("B,C", "C"), { display: "" });
    assert.deepEqual(mq.shown("B", "A"), { display: "none" });
});

test("the handler loads the script with a script element and builds a component for each use", async () => {
    setup();
    const added = [];
    globalThis.document = {
        createElement: () => ({ remove() {} }),
        head: { appendChild: (el) => { added.push(el); setTimeout(() => { vm.runInThisContext(scriptText); el.onload(); }, 0); } }
    };
    delete moqui.qvtStatic["/Items"];
    delete moqui.qvtStaticTags["/Items"];
    const headers = new Map([["ETag", 'W/"abc"'], ["X-Moqui-Qvt", "static-template"], ["Cache-Control", "max-age=86400"]]);
    const made = await moqui.qvtHandlers["static-template"]("(script text is not evaluated)", { path: "/Items", headers });
    assert.equal(added.length, 1);
    assert.equal(added[0].src, "/Items.qvt");
    const comp = made.mqStatic({ path: "/Items", search: "a=1" });
    assert.equal(typeof comp.created, "function");
    // the same validator does not load the script again
    await moqui.qvtHandlers["static-template"]("", { path: "/Items", headers });
    assert.equal(added.length, 1);
    // another validator does
    await moqui.qvtHandlers["static-template"]("", { path: "/Items", headers: new Map([["ETag", 'W/"def"']]) });
    assert.equal(added.length, 2);
    // a script that does not register is an error
    delete moqui.qvtStatic["/Missing"];
    globalThis.document.head.appendChild = (el) => { setTimeout(() => el.onload(), 0); };
    await assert.rejects(moqui.qvtHandlers["static-template"]("", { path: "/Missing", headers: new Map() }), /did not register/);
    globalThis.document.head.appendChild = (el) => { setTimeout(() => el.onerror(), 0); };
    await assert.rejects(moqui.qvtHandlers["static-template"]("", { path: "/Other", headers: new Map() }), /Could not load/);
    delete globalThis.document;
});

test("loadComponent gives each use its own component from a cached static template", async () => {
    setup();
    globalThis.document = {
        createElement: () => ({ remove() {} }),
        head: { appendChild: (el) => { setTimeout(() => { vm.runInThisContext(scriptText); el.onload(); }, 0); } }
    };
    delete moqui.qvtStatic["/Items"];
    delete moqui.qvtStaticTags["/Items"];
    moqui.componentCache.clear && moqui.componentCache.clear();
    const headers = new Map([["ETag", 'W/"abc"'], ["X-Moqui-Qvt", "static-template"], ["Cache-Control", "max-age=86400, must-revalidate, public"]]);
    moqui.request = () => Promise.resolve({ ok: true, status: 200, text: "(script)", json: undefined, headers });
    const load = (info) => new Promise((resolve) => moqui.loadComponent(info, resolve, "dc"));
    const first = await load({ path: "/Items", search: "a=1" });
    const second = await load({ path: "/Items", search: "a=2" });
    assert.notEqual(first, second, "a component for each use");
    assert.equal(first.name, "mStaticScreen");
    delete globalThis.document;
});

test("a page on the runtime build refuses a merged fragment", () => {
    qvt.config = { qvt: { build: "runtime" } };
    assert.throws(() => moqui.qvtHandlers["merged-fragment"]("<p/>", { divId: "x" }), /runtime build/);
    qvt.config = { qvt: { build: "compiler" } };
    assert.equal(moqui.qvtHandlers["merged-fragment"]("<p/>", { divId: "x" }).template, '<div id="x"><p/></div>');
});
