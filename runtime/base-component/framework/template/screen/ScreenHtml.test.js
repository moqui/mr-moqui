const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

function renderDialog(label) {
    const factory = vm.runInNewContext(fs.readFileSync(path.join(__dirname, 'ScreenHtml.js'), 'utf8'));
    // Only runtime rendering is exercised; macro callbacks are registered, not compiled.
    const module = factory({ stmt: fn => fn, noops: () => ({}) }, {
        anyMacros: () => ({}), allElements: [], opaqueElements: []
    });
    let html = '';
    const context = {
        sri: { write: text => { html += text; } },
        ec: { resource: { expand: text => text }, context: { get: () => null } }
    };
    vm.runInNewContext(module.runtime, context);
    context.$mqHtml.containerDialog({ attrs: { id: 'CreateSalesOrderDialog', 'button-text': label } });
    return html;
}

function renderList({ fields = [], columns = [], header = true, skipHeader = false, hideNav = false, orderBy = '' } = {}) {
    const factory = vm.runInNewContext(fs.readFileSync(path.join(__dirname, 'ScreenHtml.js'), 'utf8'));
    const module = factory({ stmt: fn => fn, noops: () => ({}) }, {
        anyMacros: () => ({}), allElements: [], opaqueElements: []
    });
    let html = '';
    const context = {
        footer_scripts: new Set(), footer_stylesheets: new Set(),
        sri: {
            write: text => { html += text; },
            makeUrlByType: () => ({ url: '/apps/FindOrder' }),
            getCurrentScreenUrl: () => ({ url: '/apps/FindOrder' }),
            getScreenUrlInstance: () => ({ cloneUrlInstance: () => ({
                addParameter(name, value) { this.url = '/apps/FindOrder?' + name + '=' + encodeURIComponent(value); return this; },
                getUrlWithParams() { return this.url; }
            }) }),
            getFormHiddenParameters: () => ({ organizationPartyId: 'ORG_TEST' }),
            safeCloseList() {}
        },
        ec: {
            context: { get: key => key === 'hideNav' && hideNav ? 'true' : key === 'orderByField' ? orderBy : null }, resource: { expand: text => text }, web: { sessionToken: 'test-token' },
            l10n: { localize: text => text }
        }
    };
    vm.runInNewContext(module.runtime, context);
    context.$mqHtml.formList({ count: 0 }, { attrs: {
        name: 'OrderList', list: 'orders', 'header-dialog': String(header), 'skip-header': String(skipHeader)
    } }, columns, { headerForm: header, allFields: [], summaryFields: fields, hiddenHeaderCells: [] });
    return html;
}

test('compiled summaries retain header-only fields excluded from column selection', () => {
    const load = file => vm.runInNewContext(fs.readFileSync(path.join(__dirname, file), 'utf8'));
    const T = load('../TemplateSupport.js');
    const S = load('ScreenSupport.js')(T);
    const module = load('ScreenHtml.js')(T, S);
    const node = (name, attrs = {}, children = []) => {
        const result = { name, attrs, children };
        children.forEach(child => { child.parent = result; });
        return result;
    };
    const form = node('form-list', { name: 'OrderList' }, [
        node('field', { name: 'orderType' }, [node('header-field', {}, [node('radio')])])
    ]);
    let config;
    const context = {
        sri: { doBoundaryComments: () => false, openFormList: () => ({}), pushContext() {}, popContext() {} },
        $h: { formList: (info, form, columns, cfg) => { config = cfg; }, valueText: () => 'Purchase' }
    };
    vm.runInNewContext(module.macros['form-list'](form, []), context);
    assert.equal(config.allFields.length, 0);
    assert.equal(config.summaryFields.length, 1);
    assert.equal(config.summaryFields[0].name, 'orderType');
    assert.equal(config.summaryFields[0].summary(), 'Purchase');
});

test('header filter summary includes fields outside visible columns in declaration order', () => {
    const type = { name: 'orderType', title: () => 'Order Type <&>', summary: () => 'Purchase <script>' };
    const status = { name: 'statusId', title: () => 'Status', summary: () => 'Placed' };
    const html = renderList({ fields: [type, status], columns: [[status]] });
    assert.match(html, /<strong>Order Type &lt;&amp;&gt;:<\/strong> <span class="filter-value">Purchase &lt;script&gt;<\/span>/);
    assert.match(html, /Order Type[\s\S]*<strong>Status:<\/strong> <span class="filter-value">Placed/);
    assert.equal((html.match(/<strong>Status:/g) || []).length, 1);
    assert.match(html, /<th colspan="1" class="filter-summary" style="font-weight: normal"><span class="filter">/);
    assert.equal((html.match(/name="clearParameters"/g) || []).length, 1);
    assert.match(html, /<form[^>]*method="post"[^>]*action="\/apps\/FindOrder"/);
    assert.match(html, /name="organizationPartyId" value="ORG_TEST"/);
    assert.match(html, /<button[^>]*class="filter-clear"[^>]*data-btn="primary"[^>]*aria-label="Clear"/);
    assert.doesNotMatch(html, /<div class="title">Order Type/);
});

test('dialog headers retain an empty summary row between navigation and column titles', () => {
    const column = { name: 'orderId', title: () => 'ID' };
    const html = renderList({ columns: [[column], [{ name: 'orderName', title: () => 'Name' }]] });
    assert.match(html, /<\/nav><\/th><\/tr>\s*<tr><th colspan="2" class="filter-summary" style="font-weight: normal"><\/th><\/tr><tr><th>/);
    assert.doesNotMatch(html, /name="clearParameters"/);
    for (const options of [{ header: false }, { skipHeader: true }, { hideNav: true }]) {
        assert.doesNotMatch(renderList({ ...options, columns: [[column]] }), /style="font-weight: normal"/);
    }
});

test('sortable HTML titles keep Java whitespace and active direction toggles', () => {
    const columns = [[{ name: 'orderId', title: () => 'ID <&>', orderBy: 'case-insensitive' }]];
    const html = renderList({ columns });
    assert.match(html, /ID &lt;&amp;&gt; <span class="order-by"> <a/);
    assert.match(html, /<\/a> <a/);
    assert.match(html, /href="\/apps\/FindOrder\?orderByField=%5EorderId"/);
    assert.match(html, /href="\/apps\/FindOrder\?orderByField=-%5EorderId"/);
    for (const [orderBy, target] of [['^orderId', '-%5EorderId'], ['-^orderId', '%5EorderId']]) {
        const active = renderList({ columns, orderBy }).match(/<a[^>]*class="active"[^>]*>/)[0];
        assert.ok(active.includes('orderByField=' + target + '"'));
    }
    assert.doesNotMatch(renderList({ columns: [[{ title: () => 'Name' }]] }), /class="order-by"/);
});

test('header summary omits hidden, empty and non-filter fields', () => {
    const html = renderList({ fields: [
        { title: () => 'Hidden', hide: () => true, summary: () => 'secret' },
        { title: () => 'Empty', summary: () => '' },
        { title: () => 'Ignored' }
    ] });
    assert.doesNotMatch(html, /<span class="filter">|name="clearParameters"/);
});

test('inline and skipped headers do not render active filter summaries', () => {
    const field = { title: () => 'Order Type', summary: () => 'Purchase' };
    for (const options of [{ header: false }, { skipHeader: true }, { hideNav: true }]) {
        assert.doesNotMatch(renderList({ ...options, fields: [field] }), /<span class="filter">|name="clearParameters"/);
    }
});

test('dialog trigger has an explicit accessible name independent of icon font content', () => {
    const button = renderDialog('Create Sales Order').match(/<button[^>]*>/)[0];
    assert.match(button, /aria-label="Create Sales Order"/);
    assert.match(button, /data-dialog="CreateSalesOrderDialog"/);
});

test('dialog accessible name is HTML-escaped', () => {
    const button = renderDialog('Create "Sales" & Orders').match(/<button[^>]*>/)[0];
    assert.match(button, /aria-label="Create &quot;Sales&quot; &amp; Orders"/);
});
