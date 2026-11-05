// HTML screen rendering, and the shared dynamic-filter summary. Moved from template/screen/ScreenHtml.test.js; no database, the module text comes from the component resources.
spec({ database: false });

function readModule(file) {
    const location = file === "../TemplateSupport.js"
        ? "component://framework/template/TemplateSupport.js"
        : "component://framework/template/screen/" + file;
    return runInContext(ec.resource.getLocationText(location), {});
}

// The ScreenSupport helpers a runtime takes as source (parseOrder); the macro tables are not compiled here.
function support() {
    return { anyMacros: () => ({}), allElements: [], opaqueElements: [], parseOrder: readModule('ScreenSupport.js')({}).parseOrder };
}

function renderDialog(label) {
    const factory = readModule('ScreenHtml.js');
    // Only runtime rendering is exercised; macro callbacks are registered, not compiled.
    const module = factory({ stmt: fn => fn, noops: () => ({}) }, support());
    let html = '';
    const context = {
        sri: { write: text => { html += text; } },
        ec: { resource: { expand: text => text }, context: { get: () => null } }
    };
    runInContext(module.runtime, context);
    context.$mqHtml.containerDialog({ attrs: { id: 'CreateSalesOrderDialog', 'button-text': label } });
    return html;
}

function renderList({ fields = [], columns = [], header = true, skipHeader = false, hideNav = false, orderBy = '', savedFinds = [], activeFind = '', withSavedFinds = false } = {}) {
    const factory = readModule('ScreenHtml.js');
    const module = factory({ stmt: fn => fn, noops: () => ({}) }, support());
    let html = '';
    const url = () => ({
        url: '/apps/FindOrder', path: '/apps/FindOrder',
        cloneUrlInstance: () => url(),
        removeParameter() { return this; },
        addParameter(name, value) { this.url = '/apps/FindOrder?' + name + '=' + encodeURIComponent(value); return this; },
        getParameterMap: () => ({}), getUrlWithParams() { return this.url; }
    });
    const context = {
        footer_scripts: new Set(), footer_stylesheets: new Set(),
        sri: {
            write: text => { html += text; },
            makeUrlByType: () => ({ url: '/apps/FindOrder' }),
            getCurrentScreenUrl: () => ({ url: '/apps/FindOrder' }),
            getScreenUrlInstance: url,
            buildUrl: () => ({ url: '/apps/formSaveFind', getDisableLink: () => false }),
            getUserFormListFinds: () => savedFinds,
            getUserDefaultFormListFindId: () => '',
            getSavedFindFullLocation: () => 'component://test/FindOrder.xml#OrderList',
            getFormHiddenParameters: () => ({ organizationPartyId: 'ORG_TEST' }),
            safeCloseList() {}
        },
        ec: {
            context: { get: key => key === 'hideNav' && hideNav ? 'true' : key === 'orderByField' ? orderBy : key === 'formListFindId' ? activeFind : null }, resource: { expand: text => text }, web: { sessionToken: 'test-token' },
            l10n: { localize: text => text }
        }
    };
    runInContext(module.runtime, context);
    context.$mqHtml.formList({ count: 0 }, { attrs: {
        name: 'OrderList', list: 'orders', 'header-dialog': String(header), 'skip-header': String(skipHeader), 'saved-finds': String(withSavedFinds)
    } }, columns, { headerForm: header, allFields: [], summaryFields: fields, hiddenHeaderCells: [] });
    return html;
}

feature('compiled summaries retain header-only fields excluded from column selection', { expect() { (function () {
    const T = readModule('../TemplateSupport.js');
    const S = readModule('ScreenSupport.js')(T);
    const module = readModule('ScreenHtml.js')(T, S);
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
    runInContext(module.macros['form-list'](form, []), context);
    assert.equal(config.allFields.length, 0);
    assert.equal(config.summaryFields.length, 1);
    assert.equal(config.summaryFields[0].name, 'orderType');
    assert.equal(config.summaryFields[0].summary(), 'Purchase');
})(); } });

feature('header filter summary includes fields outside visible columns in declaration order', { expect() { (function () {
    const type = { name: 'orderType', title: () => 'Order Type <&>', summary: () => 'Purchase <script>' };
    const status = { name: 'statusId', title: () => 'Status', summary: () => 'Placed' };
    const html = renderList({ fields: [type, status], columns: [[status]] });
    assert.match(html, /<strong>Order Type &lt;&amp;&gt;:<\/strong> Purchase &lt;script&gt;/);
    assert.match(html, /Order Type[\s\S]*<strong>Status:<\/strong> Placed/);
    assert.equal((html.match(/<strong>Status:/g) || []).length, 1);
    assert.match(html, /<th colspan="1" style="font-weight: normal"><span class="filter">/);
    assert.equal((html.match(/name="clearParameters"/g) || []).length, 1);
    assert.doesNotMatch(html, /<div class="title">Order Type/);
})(); } });

feature('dialog headers retain an empty summary row between navigation and column titles', { expect() { (function () {
    const column = { name: 'orderId', title: () => 'ID' };
    const html = renderList({ columns: [[column], [{ name: 'orderName', title: () => 'Name' }]] });
    assert.match(html, /<\/nav><\/th><\/tr>\s*<tr><th colspan="2" style="font-weight: normal"><\/th><\/tr><tr><th>/);
    assert.doesNotMatch(html, /name="clearParameters"/);
    for (const options of [{ header: false }, { skipHeader: true }, { hideNav: true }]) {
        assert.doesNotMatch(renderList({ ...options, columns: [[column]] }), /style="font-weight: normal"/);
    }
})(); } });

feature('sortable HTML titles preserve spacing and request their named direction', { expect() { (function () {
    const columns = [[{ name: 'orderId', title: () => 'ID <&>', orderBy: 'case-insensitive' }]];
    const html = renderList({ columns });
    assert.match(html, /ID &lt;&amp;&gt; <span class="order-by"> <a/);
    assert.match(html, /<\/a> <a/);
    assert.match(html, /href="\/apps\/FindOrder\?orderByField=%5EorderId"/);
    assert.match(html, /href="\/apps\/FindOrder\?orderByField=-%5EorderId"/);
    for (const [orderBy, target, direction] of [['^orderId', '%5EorderId', 'ascending'], ['-^orderId', '-%5EorderId', 'descending']]) {
        const active = renderList({ columns, orderBy }).match(/<a[^>]*class="active"[^>]*>/)[0];
        assert.ok(active.includes('orderByField=' + target + '"'));
        assert.ok(active.includes('aria-label="Sort ID &lt;&amp;&gt; ' + direction + '"'));
        assert.ok(active.includes('aria-current="true"'));
    }
    assert.doesNotMatch(renderList({ columns: [[{ title: () => 'Name' }]] }), /class="order-by"/);
})(); } });

feature('header summary omits hidden, empty and non-filter fields', { expect() { (function () {
    const html = renderList({ fields: [
        { title: () => 'Hidden', hide: () => true, summary: () => 'secret' },
        { title: () => 'Empty', summary: () => '' },
        { title: () => 'Ignored' }
    ] });
    assert.doesNotMatch(html, /<span class="filter">|name="clearParameters"/);
})(); } });

feature('inline and skipped headers do not render active filter summaries', { expect() { (function () {
    const field = { title: () => 'Order Type', summary: () => 'Purchase' };
    for (const options of [{ header: false }, { skipHeader: true }, { hideNav: true }]) {
        assert.doesNotMatch(renderList({ ...options, fields: [field] }), /<span class="filter">|name="clearParameters"/);
    }
})(); } });

feature('dialog trigger has an explicit accessible name independent of icon font content', { expect() { (function () {
    const button = renderDialog('Create Sales Order').match(/<button[^>]*>/)[0];
    assert.match(button, /aria-label="Create Sales Order"/);
    assert.match(button, /data-dialog="CreateSalesOrderDialog"/);
})(); } });

feature('dialog accessible name is HTML-escaped', { expect() { (function () {
    const button = renderDialog('Create "Sales" & Orders').match(/<button[^>]*>/)[0];
    assert.match(button, /aria-label="Create &quot;Sales&quot; &amp; Orders"/);
})(); } });

feature('order-by choices include sortable header fields that are not displayed columns', { expect() { (function () {
    const visible = {name:'amount',orderBy:true,title:()=> 'Amount'};
    const hidden = {name:'customer',orderBy:'case-insensitive',title:()=> 'Customer'};
    const html = renderList({fields:[visible,hidden],columns:[[visible]]});
    assert.match(html, /<select multiple[^>]*data-ts="select" data-orderby="OrderList_header_orderByField"/);
    assert.match(html, /<option value="amount">Amount \(Asc\)<\/option>/);
    assert.match(html, /<option value="\^customer">Customer \(Asc\)<\/option>/);
    assert.match(html, /<option value="-\^customer">Customer \(Desc\)<\/option>/);
})(); } });

// The submit field is deliberately absent from the displayed table columns.
feature('find dialog renders header-only submit and filter fields once in declaration order', { expect() { (function () {
    const calls = [];
    const field = name => ({name,title:()=>name,find:()=>{},dialogField:()=>calls.push(name)});
    const name = field('name');
    const find = field('searchButton');
    const hiddenFilter = field('statusId');
    renderList({fields:[name,hiddenFilter,find],columns:[[name]]});
    assert.deepEqual(calls,['name','statusId','searchButton']);
})(); } });

feature('saved finds are directly selectable in list navigation, with an explicit clear option', { expect() { (function () {
    const savedFinds = [{ description: 'AR <Customers>', formListFind: { formListFindId: 'ar&customer' } }];
    const html = renderList({ withSavedFinds: true, savedFinds, activeFind: 'ar&customer' });
    const nav = html.match(/<nav>([\s\S]*?)<\/nav>/)[1];
    assert.match(nav, /<select[^>]*id="OrderList_QuickSavedFind"[^>]*data-saved-finds[^>]*aria-label="Saved Finds"/);
    assert.match(nav, /value="_clear"[^>]*data-url="\/apps\/FindOrder\?formListFindId=_clear"/);
    assert.match(nav, /value="ar&amp;customer"[^>]*data-url="\/apps\/FindOrder\?formListFindId=ar%26customer"[^>]*selected[^>]*>AR &lt;Customers&gt;<\/option>/);
    assert.ok(nav.indexOf('QuickSavedFind') < nav.indexOf('data-dialog'));
    assert.doesNotMatch(renderList({ withSavedFinds: true }), /data-saved-finds/);
    assert.doesNotMatch(renderList({ withSavedFinds: true, savedFinds, hideNav: true }), /data-saved-finds/);
})(); } });

function dynamicFilter(mode) {
    const factory = readModule(`Screen${mode}.js`);
    const module = factory({ stmt: fn => fn, noops: () => ({}) }, support());
    let current = 'ORG_TEST,Missing';
    const calls = [];
    const dynamic = { name: 'dynamic-options', attrs: { transition: 'partyLookup', 'label-field': 'partyLabel' } };
    const widget = { name: 'drop-down', attrs: { 'allow-multiple': 'true' }, children: [dynamic], parent: {
        parent: { attrs: { name: 'fromPartyId' } }
    } };
    const context = {
        sri: {
            getFieldValuePlainString: () => current,
            getFieldValueString: () => current,
            getFieldOptions: () => new Map([['Static', 'Static label']]),
            getFieldTransitionValue: (...args) => {
                calls.push(args);
                return args[2] === 'ORG_TEST' ? 'Example <&> Company' : '';
            }
        },
        ec: {
            context: { get: () => null },
            web: { parameters: { fromPartyId_not: 'Y' } },
            resource: { expand: text => text.replace('${description}', 'Current description') },
            l10n: { localize: text => text }
        }
    };
    runInContext(module.runtime, context);
    const summary = () => context[`$mq${mode}`].valueText(widget);
    assert.equal(summary(), 'Example <&> Company, Missing (Not)');
    assert.deepEqual(calls[0], ['partyLookup', dynamic, 'ORG_TEST', 'partyLabel', false]);
    calls.length = 0;
    current = '[ORG_TEST, Missing]';
    assert.equal(summary(), 'Example <&> Company, Missing (Not)');
    widget.attrs['allow-multiple'] = 'false';
    assert.equal(summary(), 'Example <&> Company (Not)');
    delete dynamic.attrs['label-field'];
    calls.length = 0;
    summary();
    assert.equal(calls[0][3], 'label');
    widget.children.push({ name: 'list-options' });
    calls.length = 0;
    current = 'Static';
    assert.equal(summary(), 'Static label (Not)');
    assert.equal(calls.length, 0);
    current = 'ORG_TEST';
    widget.attrs['current-description'] = '${description}';
    assert.equal(summary(), 'Current description (Not)');
    assert.equal(calls.length, 0);
    delete widget.attrs['current-description'];
    widget.children = [];
    assert.equal(summary(), 'ORG_TEST (Not)');
    widget.attrs['no-current-selected-key'] = 'Static';
    current = '';
    assert.equal(summary(), 'Static label (Not)');
    delete widget.attrs['no-current-selected-key'];
    current = '';
    assert.equal(summary(), '');
    
}

feature("Html: dynamic filter summaries use configured labels with ID fallback", { expect() { dynamicFilter("Html"); } });
feature("Qvt: dynamic filter summaries use configured labels with ID fallback", { expect() { dynamicFilter("Qvt"); } });

