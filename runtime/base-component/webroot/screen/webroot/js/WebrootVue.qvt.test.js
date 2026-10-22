/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// Tests of WebrootVue.qvt.js that need no browser: every component template compiles with the Vue compiler, the router
// stand-in answers Quasar's calls, and loadComponent chooses by the X-Moqui-Qvt header.
// Run: node --test runtime/base-component/webroot/screen/webroot/js/WebrootVue.qvt.test.js
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const webroot = path.join(__dirname, "..");
globalThis.moqui = require("./MoquiLib.js");
globalThis.moqui.l10n = require("./MoquiL10n.js");
vm.runInThisContext(fs.readFileSync(path.join(webroot, "libs/vue/compiler-dom.cjs.js"), "utf8"));
require("./WebrootVue.qvt.js");
const compiler = globalThis.VueCompilerDOM;
const qvt = moqui.qvt;

test("every component has a template that compiles", () => {
    const names = Object.keys(qvt.components);
    assert.ok(names.length >= 38, "components: " + names.length);
    names.forEach((name) => {
        const template = qvt.components[name].template;
        if (!template) return;
        const errors = [];
        compiler.compile(template, { mode: "function", onError: (err) => errors.push(err.message) });
        assert.deepEqual(errors, [], name);
    });
});

test("the components the macros write are all defined", () => {
    const used = new Set();
    for (const file of ["ScreenQvt.js", "ScreenQvtStatic.js"]) {
        const macros = fs.readFileSync(path.join(__dirname, "../../../../framework/template/screen/" + file), "utf8");
        for (const match of macros.matchAll(/<(m-[a-z-]+)/g)) used.add(match[1]);
    }
    // m-box-body and the like are written through a string that holds the whole tag
    assert.ok(used.size >= 20, "tags: " + used.size);
    used.forEach((name) => assert.ok(qvt.components[name], name + " is written by ScreenQvt.js but not defined"));
});

// A Boolean prop given as text is the text: "false" is truthy, so container-box initial="closed" stayed open. The macros bind a
// Boolean prop (":initial-open") or write it with no value; this reads every m-* tag of the render goldens.
test("no golden writes a Boolean prop of an m-* component as text", () => {
    const hyphen = (name) => name.replace(/[A-Z]/g, (ch) => "-" + ch.toLowerCase());
    const isBoolean = (prop) => prop === Boolean || (Array.isArray(prop) && prop.includes(Boolean)) ||
        (prop && (prop.type === Boolean || (Array.isArray(prop.type) && prop.type.includes(Boolean))));
    const booleans = {};
    Object.keys(qvt.components).forEach((name) => {
        const props = qvt.components[name].props;
        if (!props || Array.isArray(props)) return;
        Object.keys(props).forEach((key) => { if (isBoolean(props[key])) (booleans[name] = booleans[name] || new Set()).add(hyphen(key)); });
    });
    assert.ok(booleans["m-container-box"].has("initial-open"));
    const src = path.join(__dirname, "../../../../../../crates/moqui-core/src");
    const dirs = ["script/testdata/screen/qvt/render", "screen/testdata/Static/golden/render", "screen/testdata/Static/golden"];
    const found = [];
    let tags = 0;
    dirs.forEach((dir) => fs.readdirSync(path.join(src, dir)).filter((file) => /\.(qvt|html)$/.test(file)).forEach((file) => {
        const text = fs.readFileSync(path.join(src, dir, file), "utf8");
        for (const tag of text.matchAll(/<(m-[a-z-]+)((?:\s+[^\s=>]+(?:="[^"]*")?)*)\s*\/?>/g)) {
            const names = booleans[tag[1]];
            if (!names) continue;
            tags++;
            for (const at of tag[2].matchAll(/\s+([^\s=>]+)(?:="([^"]*)")?/g)) {
                if (names.has(at[1]) && at[2] !== undefined && at[2] !== "" && at[2] !== at[1]) found.push(dir + "/" + file + ": " + tag[1] + " " + at[1] + '="' + at[2] + '"');
            }
        }
    }));
    assert.ok(tags > 50, "tags read: " + tags);
    assert.deepEqual(found, []);
});

test("both shells install Quasar's Notify, LoadingBar and Dialog plugins", () => {
    const quasar = { Notify: { n: 1 }, LoadingBar: { l: 1 }, Dialog: { d: 1 }, Loading: { x: 1 } };
    assert.deepEqual(qvt.quasarPlugins(quasar), { Notify: quasar.Notify, LoadingBar: quasar.LoadingBar, Dialog: quasar.Dialog });
    assert.deepEqual(qvt.quasarPlugins({ Notify: quasar.Notify }), { Notify: quasar.Notify });
    for (const file of ["WebrootVue.qvt.js", "WebrootPublic.qvt.js"]) {
        assert.match(fs.readFileSync(path.join(__dirname, file), "utf8"), /plugins: (qvt && qvt\.quasarPlugins \? )?qvt\.quasarPlugins\(root\.Quasar\)/, file);
    }
});

test("the shell mounts: links, dark mode, and the page address", () => {
    const options = qvt.rootOptions({ basePath: "/apps", linkBasePath: "/qapps", appRootPath: "" });
    const link = options.methods.getLinkPath;
    const vmLike = { appRootPath: "", linkBasePath: "/qapps" };
    assert.equal(link.call(vmLike, "/apps/System/Foo?a=1"), "/qapps/System/Foo?a=1");
    assert.equal(link.call(vmLike, "/qapps/System"), "/qapps/System");
    assert.equal(link.call({ appRootPath: "/moqui", linkBasePath: "/moqui/qapps" }, "/apps/x"), "/moqui/qapps/x");
    assert.equal(link.call(vmLike, { path: "/apps/a", query: { b: "1 2" } }), "/qapps/a?b=1%202");
    const data = options.data();
    assert.equal(data.basePath, "/apps");
    assert.equal(options.computed.basePathSize.call({ basePath: "/moqui/apps", appRootPath: "/moqui" }), 1);
    assert.equal(options.computed.basePathSize.call({ basePath: "/apps", appRootPath: "" }), 1);
    // the current path text is the path list, then the extra path
    const cp = options.computed.currentPath;
    assert.equal(cp.get.call({ basePath: "/apps", currentPathList: ["System", "Foo"], extraPathList: ["x"] }), "/apps/System/Foo/x");
    const target = { basePath: "/apps", linkBasePath: "/qapps", currentPathList: [] };
    cp.set.call(target, "/qapps/System/Foo/");
    assert.deepEqual(target.currentPathList, ["System", "Foo"]);
    cp.set.call(target, "/apps/Bar");
    assert.deepEqual(target.currentPathList, ["Bar"]);
});

test("theme footer columns have a full-width row as their immediate parent", () => {
    // Quasar's .row > .col-md-6 selector needs a DOM parent, not display:contents.
    assert.match(qvt.components["m-qapps-root"].template,
        /<div v-html="shell.footerHtml" class="row full-width"><\/div>/);
});

test("the router stand-in resolves what Quasar asks and pushes through setUrl", async () => {
    const router = qvt.makeRouter();
    moqui.webrootVue = { getLinkPath: (p) => p.replace("/apps", "/qapps"), setUrl: (location) => { moqui.pushed = location; } };
    const resolved = router.resolve("/apps/System/Foo?x=1");
    assert.equal(resolved.href, "/qapps/System/Foo?x=1");
    assert.equal(resolved.matched.length, 1);
    await router.push("/apps/Other");
    assert.equal(moqui.pushed, "/apps/Other");
    delete moqui.webrootVue;
});

test("a merged fragment is the template in a div, with script and link made into components", () => {
    const comp = moqui.mergedFragmentComponent('<p>x</p><script>a()</script><link href="a.css">', "Dyn");
    assert.equal(comp.template, '<div id="Dyn"><p>x</p><m-script>a()</m-script><m-stylesheet href="a.css"></m-stylesheet></div>');
    assert.equal(moqui.mergedFragmentComponent("<b/>", "").template, "<div><b/></div>");
    const errors = [];
    compiler.compile(comp.template, { onError: (err) => errors.push(err.message) });
    assert.deepEqual(errors, []);
});

const answer = (text, headers) => Promise.resolve({ ok: true, status: 200, text, json: undefined, headers: new Map(Object.entries(headers || {})) });

test("loadComponent picks the handler from X-Moqui-Qvt and caches by Cache-Control", async () => {
    const requests = [];
    const original = moqui.request;
    moqui.request = (url) => {
        requests.push(url);
        if (url.indexOf(".qvt") > 0) return answer("<p>merged</p>", { "X-Moqui-Qvt": "merged-fragment", "Cache-Control": "private, no-store" });
        if (url.indexOf(".qjs") > 0) return answer("{ data: function () { return { n: 1 }; }, template: '<i/>' }", { "Cache-Control": "public, max-age=60" });
        return answer("<template><b>{{n}}</b></template><script>module.exports = { data() { return { n: 2 }; } }</script>");
    };
    const load = (info) => new Promise((resolve) => moqui.loadComponent(info, resolve, "id1"));
    const merged = await load({ path: "/apps/A" });
    assert.equal(merged.template, '<div id="id1"><p>merged</p></div>');
    assert.deepEqual(requests, ["/apps/A.qvt"]);
    const js = await load({ path: "/apps/B", renderModes: ["qjs"], search: "a=1" });
    assert.equal(js.data().n, 1);
    assert.equal(requests[1], "/apps/B.qjs?a=1");
    // max-age puts the component in the cache by path
    const again = await load({ path: "/apps/B", renderModes: ["qjs"] });
    assert.equal(again, js);
    assert.equal(requests.length, 2);
    const sfc = await load({ path: "/apps/C", renderModes: ["qvue"] });
    assert.equal(sfc.template, "<b>{{n}}</b>");
    assert.equal(sfc.data().n, 2);
    assert.equal(requests[2], "/apps/C.qvue");
    // the handler of a server-static template is installed (qvt.static tests run it), and a handler can be replaced
    assert.equal(typeof moqui.qvtHandlers["static-template"], "function");
    moqui.qvtHandlers["static-template"] = (text) => ({ template: "static:" + text });
    moqui.request = () => answer("<p/>", { "X-Moqui-Qvt": "static-template" });
    const stat = await load({ path: "/apps/E" });
    assert.equal(stat.template, "static:<p/>");
    moqui.request = original;
});

test("a message list notifies once for each item and says whether it did", () => {
    const seen = [];
    moqui.webrootVue = { $q: { notify: (options) => seen.push(options) }, addNotify() {} };
    assert.equal(moqui.notifyMessages(null, null, null), false);
    assert.equal(moqui.notifyMessages(["one", { message: "two", type: "success" }], "bad"), true);
    assert.deepEqual(seen.map((o) => [o.message, o.type]), [["one", "info"], ["two", "positive"], ["bad", "negative"]]);
    delete moqui.webrootVue;
});

test("a message info with a link notifies with a button that follows it: an app link in the shell, an external one in a window", () => {
    const seen = [];
    const history = [];
    const went = [];
    const opened = [];
    const savedLocation = globalThis.location;
    const savedOpen = globalThis.open;
    globalThis.location = { origin: "https://erp.example.com", href: "" };
    globalThis.open = (url, target, features) => opened.push([url, target, features]);
    moqui.webrootVue = { $q: { notify: (options) => seen.push(options) }, addNotify: (...args) => history.push(args), setUrl: (url) => went.push(url) };
    try {
        assert.equal(moqui.notifyMessages([{ message: "Task created", type: "success", link: "/qapps/Commons/Task/T1", linkText: "Open task" },
            { message: "Mail sent", link: "https://mail.example.org/m/1" }, "plain", { message: "no link", type: "info" }]), true);
        assert.deepEqual(seen.map((o) => [o.message, o.type, o.actions ? o.actions[0].label : null]),
            [["Task created", "positive", "Open task"], ["Mail sent", "info", "Open"], ["plain", "info", null], ["no link", "info", null]]);
        seen[0].actions[0].handler();
        seen[1].actions[0].handler();
        assert.deepEqual(went, ["/qapps/Commons/Task/T1"]);
        assert.deepEqual(opened, [["https://mail.example.org/m/1", "_blank", "noopener"]]);
        assert.deepEqual(history[0].slice(0, 3), ["Task created", "positive", "/qapps/Commons/Task/T1"]);
        // a URL of this origin and a relative link stay in the shell; another scheme or a protocol-relative URL leaves it
        moqui.openLink("https://erp.example.com/qapps/Sales/Order?orderId=1");
        moqui.openLink("Detail?x=1");
        assert.deepEqual(went.slice(1), ["/qapps/Sales/Order?orderId=1", "Detail?x=1"]);
        assert.ok(moqui.isExternalLink("mailto:a@b.c") && moqui.isExternalLink("//cdn.example.org/x") && !moqui.isExternalLink("/apps/x"));
    } finally {
        delete moqui.webrootVue;
        globalThis.location = savedLocation;
        globalThis.open = savedOpen;
    }
});

test("the QZ Tray component compiles and defines the print functions", () => {
    globalThis.qz = { websocket: { isActive: () => false }, security: {}, printers: {}, api: {}, configs: {}, print() {} };
    require("./MoquiQzComponent.js");
    const errors = [];
    compiler.compile(moqui.qzComponent.template, { mode: "function", onError: (err) => errors.push(err.message) });
    assert.deepEqual(errors, []);
    ["printUrlMain", "printUrlLabel", "printUrlFile", "getQzConfig", "isQzActive"].forEach((name) => assert.equal(typeof moqui[name], "function", name));
    delete globalThis.qz;
});

test("an m-form keeps its own field errors and leaves the rest to notify", () => {
    const options = qvt.components["m-form"];
    const form = options.data.call({ fieldsInitial: { moquiFormName: "EditThing", title: "x", note: "" } });
    form.setFieldErrors = options.methods.setFieldErrors.bind(form);
    const resp = {
        errors: "Title needs a letter\nOther failure",
        validationErrors: [
            { form: "EditThing", field: "title", message: "Title needs a letter", fieldPretty: "Title" },
            { form: "OtherForm", field: "title", message: "Other form" },
            { field: "missing", message: "No such field" }
        ]
    };
    const rest = moqui.placeFieldErrors(form, resp);
    assert.deepEqual(form.fieldErrors, { title: "Title needs a letter" });
    assert.equal(form.fieldErrorValues.title, "x");
    assert.equal(rest.errors, "Other failure");
    assert.deepEqual(rest.validationErrors.map((ve) => ve.message), ["Other form", "No such field"]);
    assert.equal(resp.validationErrors.length, 3, "the response itself is not changed");
    // a changed value drops its error
    form.fields.title = "xy";
    options.watch.fields.handler.call(form);
    assert.deepEqual(form.fieldErrors, {});
    // nothing placed is the same response
    assert.equal(moqui.placeFieldErrors(form, { errors: "e" }).errors, "e");
    // the server writes a validation error's line as Java's toStringPretty; a placed one's line goes too
    const pretty = { field: "title", message: "Field cannot be empty", fieldPretty: "Title", serviceName: "save#Thing", serviceNamePretty: "Save#Thing" };
    assert.equal(moqui.validationErrorText(pretty), "Field cannot be empty(for field Title of service Save#Thing");
    const placedPretty = moqui.placeFieldErrors(form, { errors: "Field cannot be empty(for field Title of service Save#Thing\nOther", validationErrors: [pretty] });
    assert.equal(placedPretty.errors, "Other");
});

test("a tree node draws the children it loads, is a leaf on false, and keeps the ones that came inline", async () => {
    const options = qvt.components["m-tree-item"];
    assert.match(options.template, /v-for="child in children"/, "the list drawn is the children computed, not model.children (the server's boolean)");
    const node = (model, loadedChildren, top) => {
        const self = { model, loadedChildren: loadedChildren === undefined ? null : loadedChildren, top: top || { items: "/apps/Gallery/Widgets/actions/EnumTree", currentPath: null, parameters: {} } };
        Object.keys(options.computed).forEach((name) => Object.defineProperty(self, name, { get: () => options.computed[name].call(self) }));
        return self;
    };
    // children: true is a folder with nothing to draw yet; false is a leaf; a list came inline (open-path)
    const lazy = node({ id: "A", text: "A", li_attr: { treeNodeName: "TypeNode" }, children: true });
    assert.equal(lazy.isFolder, true);
    assert.equal(lazy.hasChildren, false);
    const leaf = node({ id: "L", text: "L", children: false });
    assert.equal(leaf.isFolder, false);
    assert.equal(leaf.hasChildren, false);
    const inline = node({ id: "I", text: "I", state: { opened: true }, children: [{ id: "I/1", text: "one", children: false }] });
    assert.equal(inline.hasChildren, true);
    assert.equal(inline.children[0].id, "I/1");
    // opening a lazy node fetches <tree url> with the node's id and name, and the list answered is what it draws
    const original = moqui.request;
    const requests = [];
    moqui.request = (url, opts) => { requests.push([url, opts.params]); return Promise.resolve({ json: [{ id: "A/1", text: "a one", children: false }] }); };
    options.watch.open.call(lazy, true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    assert.deepEqual(requests, [["/apps/Gallery/Widgets/actions/EnumTree", { treeNodeId: "A", treeNodeName: "TypeNode", treeOpenPath: null }]]);
    assert.equal(lazy.hasChildren, true);
    assert.deepEqual(lazy.children.map((child) => child.id), ["A/1"]);
    // an inline node does not fetch, and an answer that is not a list leaves a lazy node to fetch again
    options.watch.open.call(inline, true);
    moqui.request = () => Promise.resolve({ json: { errors: "no" } });
    const other = node({ id: "B", text: "B", children: true });
    options.watch.open.call(other, true);
    await new Promise((resolve) => setTimeout(resolve, 0));
    assert.equal(requests.length, 1);
    assert.equal(other.loadedChildren, null);
    assert.equal(other.isFolder, true);
    moqui.request = original;
});

test("a field widget puts its form's error on the Quasar field", () => {
    const mixinOwner = qvt.components["m-text-line"];
    const errorAttrs = mixinOwner.mixins.map((m) => m.computed && m.computed.errorAttrs).find((f) => f);
    assert.deepEqual(errorAttrs.call({ name: "title", mForm: { fieldErrors: { title: "Bad" } } }), { error: true, errorMessage: "Bad" });
    assert.deepEqual(errorAttrs.call({ name: "note", mForm: { fieldErrors: { title: "Bad" } } }), {});
    assert.deepEqual(errorAttrs.call({ name: "note", mForm: null }), {});
});

test("a menu image of type icon is a Quasar icon name for q-icon, or CSS classes for an i element", () => {
    ["fa fa-home", "fa-home", "glyphicon glyphicon-cog"].forEach((image) => assert.equal(moqui.isIconClass(image), true, image));
    ["sym_o_hub", "home", "o_home", "mdi-home", "", null, undefined].forEach((image) => assert.equal(moqui.isIconClass(image), false, String(image)));
    const icon = qvt.components["m-menu-icon"];
    assert.equal(icon.computed.isClass.call({ name: "sym_o_receipt_long" }), false);
    assert.equal(icon.computed.isClass.call({ name: "fa fa-cog" }), true);
    // the drawer, the breadcrumbs' subscreen menus, the launcher and the palette (its Recent group is the screen history) all
    // draw a menu icon through it
    assert.match(qvt.components["m-menu-item-content"].template, /<m-menu-icon v-if="menuItem.imageType === 'icon'" :name="menuItem.image"/);
    const shell = qvt.components["m-qapps-root"].template;
    assert.match(shell, /<m-menu-icon v-if="subscreen.image && subscreen.imageType === 'icon'" :name="subscreen.image"/);
    assert.match(shell, /<m-menu-icon v-if="app.image && app.imageType === 'icon'" :name="app.image"/);
    assert.match(qvt.components["m-shell-palette"].template, /<m-menu-icon v-if="row.icon \|\| \(row.image && row.imageType === 'icon'\)" :name="row.icon \|\| row.image"/);
    assert.doesNotMatch(shell, /:class="(subscreen|histItem|app)\.image"/);
    // the screen history menu is gone from the header
    assert.doesNotMatch(shell, /navHistoryList|screenHistory/);
});

test("an m-screen-context registers with the root, follows its props and leaves on unmount; the latest one is the screen context", () => {
    const options = qvt.rootOptions({ basePath: "/apps", linkBasePath: "/qapps", appRootPath: "" });
    const data = options.data();
    assert.deepEqual(data.screenContexts, []);
    assert.deepEqual(options.computed.assistReach.call(data), ["m-canvas"]);
    const root = Object.assign(data, { currentPath: "/apps/gallery/Detail", currentParameters: { enumId: "STT_INTERNAL" } });
    Object.keys(options.methods).forEach((name) => { root[name] = options.methods[name].bind(root); });
    Object.defineProperty(root, "screenRecord", { get: () => options.computed.screenRecord.call(root) });
    const screenContext = () => options.computed.screenContext.call(root);
    assert.deepEqual(screenContext(), { path: "/apps/gallery/Detail", parameters: { enumId: "STT_INTERNAL" }, record: null, selection: null, filters: null, forms: [] });

    // the component as Vue runs it: its data, its computed value, mounted, a prop change, unmounted
    const comp = qvt.components["m-screen-context"];
    const make = (props) => {
        const self = Object.assign({ $root: root }, props, comp.data());
        Object.defineProperty(self, "value", { get: () => comp.computed.value.call(self) });
        return self;
    };
    const page = make({ recordEntity: "moqui.basic.Enumeration", recordId: "STT_INTERNAL", title: "Internal", selectionForm: undefined });
    comp.mounted.call(page);
    assert.deepEqual(screenContext().record, { entity: "moqui.basic.Enumeration", id: "STT_INTERNAL", title: "Internal" });
    assert.equal(screenContext().selection, null);
    // a dialog's screen over the page is the latest; when it goes the page's is back
    const dialog = make({ recordEntity: "moqui.basic.EnumerationType", recordId: 42, selectionForm: "EnumList" });
    comp.mounted.call(dialog);
    assert.deepEqual(screenContext().record, { entity: "moqui.basic.EnumerationType", id: "42", title: "" });
    // no list of that name is on the page, so nothing is selected
    assert.deepEqual(screenContext().selection, { form: "EnumList", ids: [] });
    // a prop change keeps the registration's place
    page.recordId = "STT_ERROR";
    comp.watch.value.call(page, page.value);
    assert.equal(screenContext().record.entity, "moqui.basic.EnumerationType");
    comp.unmounted.call(dialog);
    assert.deepEqual(screenContext().record, { entity: "moqui.basic.Enumeration", id: "STT_ERROR", title: "Internal" });
    // without an id (or with the text null an empty expression gives) there is no record
    page.recordId = "null";
    comp.watch.value.call(page, page.value);
    assert.equal(screenContext().record, null);
    comp.unmounted.call(page);
    assert.deepEqual(root.screenContexts, []);
    // the canvas the reach list names is in the shell
    assert.match(qvt.components["m-qapps-root"].template, /<q-page-container id="m-canvas">/);
});

test("loadComponent asks for the best client mode of a screen whose render modes it does not know", async () => {
    const requests = [];
    const original = moqui.request;
    moqui.request = (url, options) => {
        requests.push({ url, modes: options && options.headers ? options.headers["X-Moqui-Client-Modes"] : undefined });
        if (url.indexOf("/apps/QvueOnly.qvt") === 0) {
            return answer("<template><i>{{n}}</i></template><script>module.exports = { data() { return { n: 3 }; } }</script>", { "X-Moqui-Qvt": "qvue" });
        }
        if (url.indexOf("/apps/JsOnly.qvt") === 0) return answer("{ data: function () { return { n: 4 }; }, template: '<u/>' }", { "X-Moqui-Qvt": "qjs" });
        if (url.indexOf("/apps/Js.qvt") === 0) return answer("<p>template of qjs</p>", { "X-Moqui-Qvt": "merged-fragment" });
        if (url.indexOf("/apps/Js.qjs") === 0) return answer("{ data: function () { return { n: 5 }; } }");
        return answer("<p>merged</p>", { "X-Moqui-Qvt": "merged-fragment" });
    };
    const load = (info) => new Promise((resolve) => moqui.loadComponent(info, resolve, "id2"));
    try {
        // a container's string URL: .qvt with the client modes, and the answer is the mode the server says
        const sfc = await load("/apps/QvueOnly?recordId=R1");
        assert.deepEqual(requests[0], { url: "/apps/QvueOnly.qvt?recordId=R1", modes: "qvue,qjs" });
        assert.equal(sfc.template, "<i>{{n}}</i>");
        assert.equal(sfc.data().n, 3);
        const js = await load("/apps/JsOnly");
        assert.equal(js.template, "<u/>");
        assert.equal(js.data().n, 4);
        // a screen that stays qvt
        const merged = await load("/apps/Plain");
        assert.equal(merged.template, '<div id="id2"><p>merged</p></div>');
        assert.equal(requests[2].modes, "qvue,qjs");
        // the router knows the modes: no header; an explicit extension is kept as it is
        await load({ path: "/apps/Known", renderModes: ["qvt", "html"] });
        assert.deepEqual(requests[3], { url: "/apps/Known.qvt", modes: undefined });
        await load("/apps/Explicit.qvt?x=1");
        assert.deepEqual(requests[4], { url: "/apps/Explicit.qvt?x=1", modes: undefined });
        // a qjs without a template takes the qvt of the same screen
        const companion = await load("/apps/Js.qjs");
        assert.equal(requests[6].url, "/apps/Js.qvt");
        assert.equal(companion.template, "<p>template of qjs</p>");
    } finally {
        moqui.request = original;
    }
});

test("a container provides the screen it loaded as mScreen, and the root the page's", () => {
    const vm = { id: "Dyn1", curUrl: "/apps/gallery/QuickTask.qvue?recordEntity=moqui.basic.Enumeration&recordId=A%20B&tag=x&tag=y" };
    const screen = qvt.components["m-dynamic-container"].provide.call(vm).mScreen;
    assert.equal(screen.path, "/apps/gallery/QuickTask");
    assert.deepEqual(screen.parameters, { recordEntity: "moqui.basic.Enumeration", recordId: "A B", tag: ["x", "y"] });
    assert.equal(screen.url, vm.curUrl);
    assert.equal(screen.containerId, "Dyn1");
    // the members follow the next load
    vm.curUrl = "/apps/Other";
    assert.equal(screen.path, "/apps/Other");
    assert.deepEqual(screen.parameters, {});
    // a dialog gives the URL it loaded, with its dynamic parameters
    const dialog = { id: "Dlg", loadedUrl: "/apps/Edit?id=7" };
    const dialogScreen = qvt.components["m-dynamic-dialog"].provide.call(dialog).mScreen;
    assert.deepEqual([dialogScreen.path, dialogScreen.parameters.id], ["/apps/Edit", "7"]);
    // the root: the current path and parameters of the page
    const page = { currentPath: "/apps/gallery/Detail", currentSearch: "enumId=E1", currentParameters: { enumId: "E1" } };
    const pageScreen = qvt.rootOptions({ basePath: "/apps", linkBasePath: "/qapps" }).provide.call(page).mScreen;
    assert.deepEqual([pageScreen.url, pageScreen.path, pageScreen.parameters, pageScreen.containerId], ["/apps/gallery/Detail?enumId=E1", "/apps/gallery/Detail", { enumId: "E1" }, null]);
    page.currentParameters = { enumId: "E2" };
    assert.equal(pageScreen.parameters.enumId, "E2");
});

test("the tray refreshes a tab's screen in place when it has shellRefresh, and loads it again otherwise", () => {
    const container = qvt.components["m-dynamic-container"];
    assert.match(container.template, /<component :is="curComponent" ref="screen">/);
    const calls = [];
    const withHook = { id: "m-tray-chat", $refs: { screen: { shellRefresh: () => calls.push("in place") } }, reload: () => calls.push("reload") };
    container.methods.shellRefresh.call(withHook);
    container.methods.shellRefresh.call({ id: "m-tray-today", $refs: { screen: {} }, reload: () => calls.push("reload") });
    container.methods.shellRefresh.call({ id: "m-tray-empty", $refs: {}, reload: () => calls.push("reload") });
    assert.deepEqual(calls, ["in place", "reload", "reload"]);
    // a failing hook is logged, and the tab is left as it is
    const error = console.error;
    console.error = () => calls.push("logged");
    try { container.methods.shellRefresh.call({ id: "m-tray-bad", $refs: { screen: { shellRefresh() { throw new Error("no"); } } }, reload: () => calls.push("reload") }); }
    finally { console.error = error; }
    assert.deepEqual(calls.slice(3), ["logged"]);
    // the tray's reload of a tab goes through it
    const tray = qvt.components["m-shell-tray"].methods;
    tray.reload.call({ containers: { chat: { shellRefresh: () => calls.push("tray"), reload: () => calls.push("tray reload") } } }, "chat");
    assert.equal(calls[4], "tray");
});

test("a number card's delta has the tone of its sign against delta-good, and the card is a link only with an href", () => {
    const tone = (delta, good) => { const info = moqui.numberCardDelta(delta, good); return info ? [info.sign, info.tone, info.icon] : null; };
    assert.deepEqual(tone("+3"), [1, "good", "sym_o_trending_up"]);
    assert.deepEqual(tone("-12%"), [-1, "bad", "sym_o_trending_down"]);
    assert.deepEqual(tone("−4"), [-1, "bad", "sym_o_trending_down"], "a minus sign");
    assert.deepEqual(tone("7", "up"), [1, "good", "sym_o_trending_up"], "a number's own sign");
    assert.deepEqual(tone("+3", "down"), [1, "bad", "sym_o_trending_up"], "more late shipments are bad");
    assert.deepEqual(tone("-2 days", "down"), [-1, "good", "sym_o_trending_down"]);
    assert.deepEqual(tone("0"), [0, "neutral", "sym_o_trending_flat"]);
    assert.deepEqual(tone("steady"), [0, "neutral", "sym_o_trending_flat"]);
    assert.equal(tone(""), null);
    assert.equal(tone(null), null);
    const card = qvt.components["m-number-card"];
    const computed = (props) => {
        const self = Object.assign({ deltaGood: "up", size: "md", external: false }, props);
        Object.keys(card.computed).forEach((name) => Object.defineProperty(self, name, { get: () => card.computed[name].call(self) }));
        return self;
    };
    assert.equal(computed({}).wrapper, "div");
    assert.equal(computed({ href: "/apps/gallery/List" }).wrapper, "m-link");
    assert.equal(computed({ href: "https://example.com", external: true }).wrapper, "a");
    assert.equal(computed({ value: "" }).shownValue, "–");
    assert.equal(computed({ value: 42 }).shownValue, "42");
    assert.equal(computed({ size: "lg" }).valueClass, "text-h4");
    assert.equal(computed({ delta: "+3", deltaGood: "down" }).deltaClass, "text-negative");
    assert.equal(computed({ delta: "+3" }).deltaClass, "text-positive");
    assert.equal(computed({ delta: "0" }).deltaClass, "m-muted");
});

// a .qvue component's <style> is added once, whatever its text: a line break or a quote at its start made the old selector
// invalid (impl-projects.md F53)
test("a component style with any text is added to the head once", () => {
    const added = [];
    const style = () => ({ attrs: {}, setAttribute(name, value) { this.attrs[name] = value; }, getAttribute(name) { return this.attrs[name]; } });
    const head = { querySelectorAll: (selector) => { assert.equal(selector, "style[data-mq-style]"); return added; }, appendChild: (el) => added.push(el) };
    const hadDocument = Object.prototype.hasOwnProperty.call(globalThis, "document");
    const before = globalThis.document;
    globalThis.document = { head, createElement: (tag) => { assert.equal(tag, "style"); return style(); } };
    try {
        const css = '\n.a { content: "x" }\n.b { color: red }\n';
        moqui.addComponentStyle(css);
        moqui.addComponentStyle(css);
        moqui.addComponentStyle(".c { color: blue }");
        assert.equal(added.length, 2);
        assert.equal(added[0].textContent, css);
        assert.match(added[0].getAttribute("data-mq-style"), /^\d+-[0-9a-z]+$/);
    } finally {
        if (hadDocument) globalThis.document = before; else delete globalThis.document;
    }
});
