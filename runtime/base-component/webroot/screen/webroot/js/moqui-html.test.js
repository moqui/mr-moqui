/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// Tests of the pure functions of moqui-html.js. Run: node --test runtime/base-component/webroot/screen/webroot/js/moqui-html.test.js
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const u = require("./moqui-html.js");

test("a datetime-local value becomes a timestamp with a space", () => {
    assert.equal(u.normalizeDateTime("2024-03-05T14:07"), "2024-03-05 14:07");
    assert.equal(u.normalizeDateTime("2024-03-05T14:07:30"), "2024-03-05 14:07:30");
    assert.equal(u.normalizeDateTime("2024-03-05"), "2024-03-05");
    assert.equal(u.normalizeDateTime("12:30"), "12:30");
    assert.equal(u.normalizeDateTime("Tuesday"), "Tuesday");
    assert.equal(u.normalizeDateTime(""), "");
    assert.equal(u.normalizeDateTime(undefined), undefined);
});

test("parseJson falls back on empty or broken text", () => {
    assert.deepEqual(u.parseJson('{"a":1}', null), { a: 1 });
    assert.equal(u.parseJson("", 5), 5);
    assert.equal(u.parseJson("{nope", "x"), "x");
    assert.deepEqual(u.parseJson(undefined, []), []);
});

test("queryString encodes names and values and repeats lists", () => {
    assert.equal(u.queryString({ a: "x y", b: ["1", "2"], c: null, d: undefined, "e&f": "g=h" }), "a=x%20y&b=1&b=2&e%26f=g%3Dh");
    assert.equal(u.queryString(null), "");
    assert.equal(u.queryString({ n: 0, z: "" }), "n=0&z=");
});

test("optionList reads a list or an options object with the named fields", () => {
    assert.deepEqual(u.optionList([{ value: "a", label: "A" }, { value: "", label: "none" }]), [{ value: "a", text: "A" }, { value: " ", text: "none" }]);
    assert.deepEqual(u.optionList({ options: [{ id: 1, name: "One" }], count: 1 }, "id", "name"), [{ value: "1", text: "One" }]);
    assert.deepEqual(u.optionList({ nothing: true }), []);
    assert.deepEqual(u.optionList(null), []);
    assert.deepEqual(u.optionList([{ value: "k", text: "T" }]), [{ value: "k", text: "T" }]);
});

test("dependsParams needs every value unless the dependency is optional", () => {
    const values = { f1: "x", f2: "", f3: "z" };
    const read = (id) => values[id];
    assert.deepEqual(u.dependsParams([["f1", "one"], ["f3", "three"]], read, false), { one: "x", three: "z" });
    assert.equal(u.dependsParams([["f1", "one"], ["f2", "two"]], read, false), null);
    assert.deepEqual(u.dependsParams([["f1", "one"], ["f2", "two"]], read, true), { one: "x" });
    assert.deepEqual(u.dependsParams([], read, false), {});
    assert.deepEqual(u.dependsParams(undefined, read, false), {});
});

test("orderByValue keeps one direction for each column", () => {
    assert.equal(u.orderByValue(["name", "-qty"]), "name,-qty");
    assert.equal(u.orderByValue(["name", "-name", "qty"]), "name,qty");
    assert.equal(u.orderByValue(["^name", "-^name"]), "^name");
    assert.equal(u.orderByValue([]), "");
    assert.equal(u.orderByValue("one"), "one");
});

test("treeNodes reads the jstree shape the tree transitions return", () => {
    const nodes = u.treeNodes([
        { id: "a", text: "A", a_attr: { href: "/x?id=a" }, li_attr: { treeNodeName: "Top" }, state: { opened: true }, children: [{ id: "a1", text: "A1" }] },
        { id: "b", text: "B", children: true },
        { id: "c" }
    ]);
    assert.equal(nodes[0].href, "/x?id=a");
    assert.equal(nodes[0].name, "Top");
    assert.equal(nodes[0].opened, true);
    assert.equal(nodes[0].children[0].text, "A1");
    assert.equal(nodes[0].children[0].children, false);
    assert.equal(nodes[1].children, true);
    assert.equal(nodes[2].text, "");
    assert.deepEqual(u.treeNodes(null), []);
});

test("the options of an editable select from its editable-load answer", () => {
    assert.deepEqual(u.editableOptions([{ value: "A", label: "First" }, "B"]), { options: [{ value: "A", label: "First" }, { value: "B", label: "B" }] });
    assert.deepEqual(u.editableOptions({ E: "Letter E", selected: "E" }), { options: [{ value: "E", label: "Letter E" }], selected: "E" });
    assert.deepEqual(u.editableOptions(null), { options: [] });
});

test('saved-find changes navigate to the selected URL and ignore the placeholder and other selects', () => {
    const fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
    const listeners = {}, visited = [];
    const document = {
        documentElement: { classList: { add() {} } },
        querySelector: () => null, querySelectorAll: () => [],
        addEventListener: (name, handler) => { listeners[name] = handler; }
    };
    vm.runInNewContext(fs.readFileSync(path.join(__dirname, 'moqui-html.js'), 'utf8'), {
        window: { document, location: { assign: url => visited.push(url) } }
    });
    const change = url => listeners.change({ target: { closest: () => ({ selectedOptions: [{ dataset: { url } }] }) } });
    change('/apps/FindOrder?formListFindId=sales');
    change('/apps/FindOrder?formListFindId=_clear');
    change(undefined);
    listeners.change({ target: { closest: () => null } });
    assert.deepEqual(visited, ['/apps/FindOrder?formListFindId=sales', '/apps/FindOrder?formListFindId=_clear']);
});

test('inline remote options set the natural width and resize after a dependency changes', () => {
    const fs = require('node:fs'), path = require('node:path'), os = require('node:os');
    const { execFileSync } = require('node:child_process');
    const webroot = path.join(__dirname, '..');
    const css = fs.readFileSync(path.join(webroot, 'css/moqui-html.css'), 'utf8');
    const pluginCss = fs.readFileSync(path.join(webroot, 'libs/tom-select/tom-select.css'), 'utf8');
    const plugin = fs.readFileSync(path.join(webroot, 'libs/tom-select/tom-select.complete.min.js'), 'utf8');
    const client = fs.readFileSync(path.join(__dirname, 'moqui-html.js'), 'utf8');
    const html = `<!doctype html><html data-theme="light"><style>${css}</style><style>${pluginCss}</style>
        <form><select id="country" name="country"><option value="USA">USA</option><option value="CAN">Canada</option></select>
        <div style="width:840px;padding:0 15px"><div class="field big-row"><label>State</label><div class="widget">
            <div class="big-item"><div class="field"><div class="widget"><select id="state" name="state" data-ts="remote" data-url="/states" data-depends='[["country","country"]]'><option selected value="USA_CO">CO - Colorado</option></select></div></div></div>
            <div class="big-item"><div class="field"><div class="widget"><select id="search" data-ts="remote" data-search><option>A</option></select></div></div></div>
        </div></div><div class="field"><label>Owner</label><div class="widget"><select id="owner" data-ts="remote"><option>A</option></select></div></div></div></form>
        <select id="expected" style="align-self:flex-start;font:14px Arial;padding:0;border:1px solid;width:auto;min-width:0;max-width:none"><option>FM - Federated States of Micronesia</option><option selected>CO - Colorado</option></select>
        <pre id="result"></pre><script>${plugin}</script><script>
        const responses={USA:[{value:'USA_FM',label:'FM - Federated States of Micronesia'},{value:'USA_CO',label:'CO - Colorado'}],CAN:[{value:'CAN_BC',label:'BC - British Columbia'}]};
        let calls=0;window.fetch=(url,options)=>{calls++;const country=new URLSearchParams(options.body).get('country');return Promise.resolve({ok:true,headers:new Headers(),text:()=>Promise.resolve(JSON.stringify(responses[country]))})};
        </script><script>${client}</script><script>
        const metric=()=>{const select=document.getElementById('state');return {width:select.tomselect.wrapper.getBoundingClientRect().width,value:select.value,count:new Set([...select.options].map(o=>o.value).filter(Boolean)).size,submitted:new FormData(document.querySelector('form')).get('state')}};
        setTimeout(()=>{
            const initial=metric(),expected=document.getElementById('expected').getBoundingClientRect().width;
            const owner=document.getElementById('owner').tomselect.wrapper.getBoundingClientRect().width;
            const search=document.getElementById('search').tomselect.wrapper.style.width;
            document.getElementById('country').value='CAN';document.getElementById('country').dispatchEvent(new Event('change'));
            setTimeout(()=>{document.getElementById('result').textContent=JSON.stringify({initial,expected,owner,search,updated:metric(),calls,temporarySelects:document.querySelectorAll('select[aria-hidden="true"]').length})},50);
        },50);
        </script>`;
    const directory=fs.mkdtempSync(path.join(os.tmpdir(),'moqui-inline-select-'));
    let dom;
    try {
        const file=path.join(directory,'test.html');fs.writeFileSync(file,html);
        dom=execFileSync(process.env.CHROMIUM_PATH||'chromium',[
            '--headless','--disable-gpu','--window-size=1440,1000','--user-data-dir='+path.join(directory,'profile'),
            '--virtual-time-budget=1000','--dump-dom','file://'+file
        ],{encoding:'utf8',timeout:20000,maxBuffer:3*1024*1024,stdio:['ignore','pipe','pipe']});
    } finally {fs.rmSync(directory,{recursive:true,force:true});}
    const match=dom.match(/<pre id="result">([^<]+)<\/pre>/);assert.ok(match,'async option population completes');
    const m=JSON.parse(match[1]);
    assert.equal(m.initial.width,m.expected+24);assert.equal(m.initial.width,274);assert.ok(m.initial.width>200);
    assert.equal(m.initial.value,'USA_CO');assert.equal(m.initial.submitted,'USA_CO');assert.equal(m.initial.count,2);
    assert.ok(m.updated.width<m.initial.width);assert.ok(m.updated.width>=60);
    assert.equal(m.updated.value,'CAN_BC');assert.equal(m.updated.submitted,'CAN_BC');assert.equal(m.updated.count,1);
    assert.ok(Math.abs(m.owner-670)<0.05);assert.equal(m.search,'');assert.equal(m.calls,2);assert.equal(m.temporarySelects,0);
});
