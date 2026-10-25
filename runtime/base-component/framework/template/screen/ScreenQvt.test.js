const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

function renderList({ saved = true, header = true, denied = false, hideNav = false, noSearch = false, finds = [], active = null, scheduled = null, fields = [], hidden = {}, headerColumns = null, headerFields = [] } = {}) {
    const factory = vm.runInNewContext(fs.readFileSync(path.join(__dirname, 'ScreenQvt.js'), 'utf8'));
    const module = factory({ stmt: fn => fn, noops: () => ({}) }, {
        anyMacros: () => ({}), allElements: [], opaqueElements: []
    });
    let html = '';
    const url = () => ({
        path: '/qapps/FindThing', getDisableLink: () => denied,
        cloneUrlInstance: url, removeParameter() { return this; },
        getParameterMap: () => ({ statusId: 'PmntProposed' }),
        addParameter(name, value) { this.pathWithParams = this.path + '?' + name + '=' + value; return this; }
    });
    const context = {
        sri: {
            write: text => { html += text; }, buildUrl: url, makeUrlByType: url,
            getScreenUrlInstance: url, getCurrentScreenUrl: url,
            getFormHiddenParameters: () => hidden, getFormListHeaderValues: () => ({}),
            getUserFormListFinds: () => finds, getUserDefaultFormListFindId: () => '', getFormListFindScreenScheduled: () => scheduled,
            getSavedFindFullLocation: (name) => '/qapps/FindThing.xml.form_list$' + name,
            pushSingleFormMapContext() {}, popContext() {}, safeCloseList() {}
        },
        ec: {
            context: {
                get: key => key === 'hideNav' && hideNav ? 'true' : key === 'formListFindId' ? active : null,
                sharedMap: { _entityListNoSearchParms: noSearch }
            },
            l10n: { localize: text => text }, resource: { expand: text => text }
        }
    };
    vm.runInNewContext(module.runtime, context);
    if (headerColumns) {
        context.$mqQvt.headerCell(headerColumns, { headerDialog: true });
        return html;
    }
    context.$mqQvt.formList({ count: 0 }, { attrs: {
        name: 'PaymentList', list: 'payments', 'saved-finds': String(saved), 'header-dialog': String(header)
    } }, [], { headerForm: header, valueDescs: { h: [] }, allFields: fields, headerFields });
    return html;
}

test('empty entity displays preserve a line in list cells, not single-form inputs', () => {
    const factory = vm.runInNewContext(fs.readFileSync(path.join(__dirname, 'ScreenQvt.js'), 'utf8'));
    const module = factory({ stmt: fn => fn, noops: () => ({}) }, {
        anyMacros: () => ({}), allElements: [], opaqueElements: []
    });
    for (const kind of ['form-list', 'form-single']) {
        for (const value of ['', 'Asset <Good>']) {
            let html = '';
            const field = { attrs: { name: 'productClassEnumId' }, parent: { name: kind, attrs: { name: 'ProductsForm' } } };
            const widget = { attrs: {}, parent: { attrs: {}, parent: field } };
            const context = {
                sri: { write: text => { html += text; }, getFieldEntityValue: () => value, getFieldValuePlainString: () => '' },
                ec: { context: { get: () => null }, resource: { expand: text => text }, l10n: { localize: text => text } }
            };
            vm.runInNewContext(module.runtime, context);
            context.$mqQvt.displayEntity(widget);
            if (kind === 'form-list') assert.ok(html.includes('<template v-slot:default>' + (value ? 'Asset &lt;Good&gt;' : '&nbsp;') + '</template>'));
            else assert.doesNotMatch(html, /&nbsp;|v-slot:default/);
        }
    }
});

function dialog(html, id) {
    return html.match(new RegExp('<m-container-dialog id="' + id + '"[^>]*>([\\s\\S]*?)</m-container-dialog>'))?.[1];
}

test('sortable headers separate title and sort links with whitespace like Java', () => {
    const html = renderList({ headerColumns: [
        { name: 'projectId', title: () => 'Project <ID>', orderBy: 'true' },
        { name: 'purpose', title: () => 'Purpose' }
    ] });
    assert.match(html, /Project &lt;ID&gt;\s+<span class="form-order-by">\s*<m-link/);
    assert.match(html, /<\/m-link>\s+<m-link/);
    assert.match(html, /href="\/qapps\/FindThing\?orderByField=projectId"/);
    assert.match(html, /href="\/qapps\/FindThing\?orderByField=-projectId"/);
    assert.match(html, />Purpose<\/div>/);
    assert.equal((html.match(/class="form-order-by"/g) || []).length, 1);
});

test('Find Options includes header-only filters and sort choices in declaration order', () => {
    const rendered = [];
    const html = renderList({ headerFields: [
        { name: 'orderType', title: () => 'Order Type', dialogField: () => rendered.push('orderType') },
        { name: 'orderName', title: () => 'Name', orderBy: 'case-insensitive', dialogField: () => rendered.push('orderName') }
    ] });
    assert.deepEqual(rendered, ['orderType', 'orderName']);
    const find = dialog(html, 'PaymentList_hdialog');
    assert.match(find, /'value':'\^orderName'/);
    assert.match(find, /'value':'-\^orderName'/);
});

test('compiled Find Options keeps header-only filters outside table columns', () => {
    const load = file => vm.runInNewContext(fs.readFileSync(path.join(__dirname, file), 'utf8'));
    const T = load('../TemplateSupport.js');
    const module = load('ScreenQvt.js')(T, load('ScreenSupport.js')(T));
    const node = (name, attrs = {}, children = []) => {
        const result = { name, attrs, children,
            source: name === 'radio' ? '$v.radio();' : name === 'text-find' ? '$v.textFind();' : '' };
        children.forEach(child => { child.parent = result; });
        return result;
    };
    const form = node('form-list', { name: 'OrderList' }, [
        node('field', { name: 'orderType' }, [node('header-field', {}, [node('radio')])]),
        node('field', { name: 'orderName' }, [node('header-field', { 'show-order-by': 'case-insensitive' }, [node('text-find')])])
    ]);
    let config;
    const rendered = [];
    const context = {
        sri: { doBoundaryComments: () => false, openFormList: () => ({}), pushContext() {}, popContext() {}, write() {} },
        $v: { formList: (info, form, columns, cfg) => { config = cfg; },
            fieldBox() {}, radio: () => rendered.push('radio'), textFind: () => rendered.push('text-find') }
    };
    vm.runInNewContext(module.macros['form-list'](form, []), context);
    assert.equal(config.allFields.length, 0);
    assert.deepEqual(Array.from(config.headerFields, field => field.name), ['orderType', 'orderName']);
    assert.equal(config.headerFields[1].orderBy, 'case-insensitive');
    config.headerFields.forEach(field => field.dialogField());
    assert.deepEqual(rendered, ['radio', 'text-find']);
});

test('empty-list warning reads the shared no-search-parameters marker', () => {
    const required = renderList({ noSearch: true });
    assert.match(required, /Find Options required to view results/);
    assert.doesNotMatch(required, /No results found/);
    const searched = renderList();
    assert.match(searched, /No results found/);
    assert.doesNotMatch(searched, /Find Options required to view results/);
});

test('saved finds have a separate named dialog, not inside Find Options', () => {
    const html = renderList();
    const saved = dialog(html, 'PaymentList_sfdialog');
    assert.ok(saved);
    assert.match(saved, /icon="find_in_page" label="Saved Finds"/);
    assert.match(saved, /label="Save New Find"/);
    assert.match(saved, /\/qapps\/FindThing\.xml\.form_list\$PaymentList/);
    const find = dialog(html, 'PaymentList_hdialog');
    assert.match(find, /label="Find Options"/);
    assert.match(find, /label="Clear Parameters"/);
    assert.doesNotMatch(find, /Save New Find|_NewFind/);
});

test('saved finds remain available without a header dialog', () => {
    const html = renderList({ header: false });
    assert.ok(dialog(html, 'PaymentList_sfdialog'));
    assert.equal(dialog(html, 'PaymentList_hdialog'), undefined);
});

test('disabled, unauthorized and hidden navigation do not expose saved finds', () => {
    for (const options of [{ saved: false }, { denied: true }, { hideNav: true }]) {
        assert.equal(dialog(renderList(options), 'PaymentList_sfdialog'), undefined);
    }
});

test('server-static forms also separate Saved Finds from Find Options', () => {
    const factory = vm.runInNewContext(fs.readFileSync(path.join(__dirname, 'ScreenQvtStatic.js'), 'utf8'));
    const module = factory({ stmt: fn => fn, noops: () => ({}) }, {
        anyMacros: () => ({}), allElements: [], opaqueElements: []
    });
    let html = '';
    const context = { sri: { write: text => { html += text; }, staticUrl: () => ({ path: '/qapps/FindThing' }) } };
    vm.runInNewContext(module.runtime, context);
    context.$mqQvtS.formList({ attrs: { name: 'PaymentList', list: 'payments', 'saved-finds': 'true', 'header-dialog': 'true' }, children: [] }, [], []);
    assert.match(dialog(html, 'PaymentList_sfdialog'), /icon="find_in_page"/);
    assert.match(dialog(html, 'PaymentList_sfdialog'), /Save New Find/);
    assert.match(dialog(html, 'PaymentList_sfdialog'), /nv\.findUrl\('_clear'\)/);
    assert.match(dialog(html, 'PaymentList_sfdialog'), /Clear Current Find/);
    assert.doesNotMatch(dialog(html, 'PaymentList_hdialog'), /Save New Find|_NewFind/);
    const saved = dialog(html, 'PaymentList_sfdialog');
    assert.match(saved, /<template v-if="nv\.activeFind">/);
    assert.match(saved, /v-if="nv\.activeScheduled"/);
    assert.match(saved, /<m-form v-else[^>]*id="PaymentList_SCHED"[^>]*:fields-initial="nv\.scheduleFields\(\)"/);
    assert.match(saved, /name="ScheduleFind" value="Y"/);
    assert.match(saved, /:data-confirm="ec\.l10n\.localize\('Setup a schedule/);
});

test('filter summary uses all fields, escapes labels and keeps hidden reset parameters only', () => {
    const html = renderList({
        fields: [
            { title: () => 'Type <&>', summary: () => 'Transfer <script>' },
            { title: () => 'Empty', summary: () => '' },
            { title: () => 'Ignored' }
        ],
        hidden: { organizationPartyId: 'ORG_TEST' }
    });
    assert.match(html, /<strong>Type &lt;&amp;&gt;:<\/strong> <span class="text-positive">Transfer &lt;script&gt;<\/span>/);
    assert.doesNotMatch(html, /<strong>Empty|<strong>Ignored/);
    const reset = html.match(/<m-form-link[^>]*id="PaymentList_header_clr"[^>]*>/)?.[0];
    assert.ok(reset);
    assert.match(reset, /organizationPartyId/);
    assert.doesNotMatch(reset, /statusId|PmntProposed/);
    assert.match(html, /<q-tooltip>Reset to Default<\/q-tooltip>/);
});

test('empty filters have no reset control; inline headers and hidden navigation have no summary', () => {
    assert.doesNotMatch(renderList(), /PaymentList_header_clr/);
    for (const options of [{ header: false }, { hideNav: true }]) {
        assert.doesNotMatch(renderList({ ...options, fields: [{ title: () => 'Type', summary: () => 'Transfer' }] }), /form-list-filter-summary|PaymentList_header_clr/);
    }
});

test('existing saved find actions stay in the Saved Finds dialog', () => {
    const html = renderList({ finds: [{
        formListFind: { formListFindId: 'mine' }, description: 'My payments', isByUserId: 'true'
    }] });
    const saved = dialog(html, 'PaymentList_sfdialog');
    assert.ok(saved);
    for (const label of ['My payments', 'Update to Current', 'Make Default', 'Delete', 'Clear Current Find']) {
        assert.ok(saved.includes(label), label);
    }
    assert.match(saved, /href="\/qapps\/FindThing\?formListFindId=_clear"/);
});

test('the active saved find can be scheduled, and a scheduled one shows its schedule', () => {
    const finds = [{ formListFind: { formListFindId: 'mine' }, description: 'My payments', isByUserId: 'true' }];
    assert.doesNotMatch(dialog(renderList({ finds }), 'PaymentList_sfdialog'), /_SCHED|Active Saved Find/);
    const open = dialog(renderList({ finds, active: 'mine' }), 'PaymentList_sfdialog');
    assert.match(open, /<strong>Active Saved Find:<\/strong> My payments/);
    const form = open.match(/<m-form[^>]*id="PaymentList_SCHED"[^>]*>[\s\S]*?<\/m-form>/)?.[0];
    assert.ok(form, open);
    assert.match(form, /'formListFindId':'mine'/);
    assert.match(form, /'screenPath':'\/qapps\/FindThing'/);
    assert.match(form, /name="renderMode"[\s\S]*'xsl-fo'/);
    assert.match(form, /name="cronSelected"[\s\S]*'0 0 6 \? \* MON-FRI'/);
    assert.match(form, /name="ScheduleFind" value="Y"/);
    assert.match(form, /data-confirm="Setup a schedule to send this saved find to you by email\?"/);
    assert.doesNotMatch(form, /onclick/);
    const scheduled = dialog(renderList({ finds, active: 'mine', scheduled: { renderMode: 'xsl-fo', cronDescription: 'at 06:00' } }), 'PaymentList_sfdialog');
    assert.match(scheduled, /<p>\(Scheduled for PDF at 06:00\)<\/p>/);
    assert.doesNotMatch(scheduled, /_SCHED/);
});

test('the filter summary shows a display header field by its value and a display-entity by its entity text', () => {
    const factory = vm.runInNewContext(fs.readFileSync(path.join(__dirname, 'ScreenQvt.js'), 'utf8'));
    const module = factory({ stmt: fn => fn, noops: () => ({}) }, { anyMacros: () => ({}), allElements: [], opaqueElements: [] });
    const calls = [];
    const context = {
        sri: {
            getFieldValueString: () => 'PMT_ID',
            getFieldEntityValue: () => 'Visa ending 1111',
            getFieldTransitionValue: (...args) => { calls.push(args); return 'Labelled'; }
        },
        ec: { context: { get: () => null }, l10n: { localize: text => text } }
    };
    vm.runInNewContext(module.runtime, context);
    const widget = (name, attrs = {}) => ({ name, attrs, parent: { parent: { attrs: { name: 'eitherPaymentMethodId' } } } });
    assert.equal(context.$mqQvt.valueText(widget('display', { text: 'Static text' })), 'PMT_ID');
    assert.equal(context.$mqQvt.valueText(widget('display-entity', { 'entity-name': 'PaymentMethod' })), 'Visa ending 1111');
    assert.equal(context.$mqQvt.valueText(widget('display', { 'dynamic-transition': 'getLabel' })), 'Labelled');
    assert.equal(calls[0][0], 'getLabel');
    assert.equal(calls[0][2], 'PMT_ID');
    assert.equal(context.$mqQvt.valueText(widget('hidden')), '');
});

test('dependent remote dropdowns load initial options with one dependency', () => {
    const factory = vm.runInNewContext(fs.readFileSync(path.join(__dirname, 'ScreenQvt.js'), 'utf8'));
    const module = factory({ stmt: fn => fn, noops: () => ({}) }, { anyMacros: () => ({}), allElements: [], opaqueElements: [] });
    let html = '';
    const context = {
        sri: {
            write: text => { html += text; }, getFieldOptions: () => new Map(), getFieldValuePlainString: () => '',
            makeUrlByType: () => ({ url: '/qapps/getGeoCountryStates', getParameterMap: () => ({}) })
        },
        ec: { context: { get: () => null }, resource: { expand: text => text }, l10n: { localize: text => text } }
    };
    vm.runInNewContext(module.runtime, context);
    const field = { attrs: { name: 'stateProvinceGeoId' }, parent: { attrs: { name: 'CreatePersonForm' } } };
    const sub = { attrs: { title: '' }, parent: field };
    const widget = { attrs: {}, parent: sub, children: [{ name: 'dynamic-options', attrs: { transition: 'getGeoCountryStates', 'value-field': 'geoId' }, children: [{ name: 'depends-on', attrs: { field: 'countryGeoId' } }] }] };
    context.$mqQvt.dropDown(widget);
    assert.match(html, /options-load-init/);
    assert.match(html, /countryGeoId/);
    assert.match(html, /value-field="geoId"/);
});
