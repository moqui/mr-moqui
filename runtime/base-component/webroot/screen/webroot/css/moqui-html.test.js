/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// CHROMIUM_PATH=/usr/bin/chromium node --test runtime/base-component/webroot/screen/webroot/css/moqui-html.test.js
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const { execFileSync } = require('node:child_process');

test('Java CSS metrics and untitled big-row layout are preserved', () => {
    const css = fs.readFileSync(path.join(__dirname, 'moqui-html.css'), 'utf8');
    const row = (id, width, title = '') => `<div style="width:${width}px"><div id="${id}" class="field big-row">
        ${title ? '<label>Title</label>' : ''}<div class="widget">
            <div class="big-item"><div class="field"><div class="widget"><input size="40"></div></div></div>
            <div class="big-item"><div class="field"><div class="widget"><button>Text Search</button></div></div></div>
        </div></div></div>`;
    const html = `<!DOCTYPE html><html data-theme="light"><style>${css}</style>
        <div class="field" id="text-field"><label>Text</label><div class="widget"><input id="text-control"></div></div>
        <div class="field" id="select-field"><label>Select</label><div class="widget"><select id="select-control"><option>One</option></select></div></div>
        <div class="field" id="static-field"><label>Static</label><div class="widget"><span class="display" id="static-control">Value</span></div></div>
        <div class="field"><label>Paragraph</label><div class="widget"><p id="plain-static-control">Value</p></div></div>
        <div class="field"><label>Inline</label><div class="widget"><span class="text-inline" id="inline-static-control">Value</span></div></div>
        <div class="field" id="link-field"><label>Link</label><div class="widget"><a id="link-control" href="#">Related value</a></div></div>
        <div class="field"><label>Message</label><div class="widget"><textarea id="textarea-control"></textarea></div></div>
        <div class="field"><label>Date</label><div class="widget"><input type="date" id="date-control"></div></div>
        <div class="field"><label>Check</label><div class="widget"><input type="checkbox" id="checkbox-control"></div></div>
        <div class="field" data-invalid><label>Invalid</label><div class="widget"><input id="invalid-control"></div></div>
        ${row('wide', 690)}${row('titled', 690, 'Title')}${row('narrow', 250)}
        <table class="form-list"><thead><tr id="heading"><th><div class="title">ID <span class="order-by"><a href="#">▲</a><a class="active" href="#">▼</a></span></div><div class="title">Name</div></th></tr></thead>
        <tbody><tr id="product"><td id="cell"><div><a href="#">DEMO_1_1</a></div><div><a href="#">Demo Product One-One</a></div></td>
        <td><div><span class="display">Inventory</span></div><div><span class="display">Inventory - Finished Good</span></div></td></tr></tbody></table>
        <table class="form-list"><tbody><tr><td id="compact-cell"><div class="field" id="table-field"><label>Compact</label><div class="widget"><input id="table-control"><span class="display" id="table-display">Compact</span></div></div></td></tr></tbody></table>
        <button id="button">Find Options</button><button id="NewProductDialog-button" data-dialog="NewProductDialog">New Product</button><strong id="strong">Label</strong>
        <div id="invoice-status"><span class="shaded-area condensed"><ul class="float-plain condensed"><li><div><strong>Status</strong></div><div><strong>In-Process</strong></div></li><li><button>Finalize</button></li><li><button>Cancel</button></li></ul></span></div>
        <table class="form-list"><thead><tr class="nav-row"><th><nav><select id="saved-find" class="saved-find"><option>Saved Finds</option></select><button id="find">Find Options</button><ul class="pagination"><li><span id="page">Page 1</span></li></ul><a id="csv" data-btn="default">CSV</a></nav></th></tr></thead></table>
        <section class="box" id="panel"><header id="panel-heading"><h5 id="panel-title">Payment Applications</h5><div class="toolbar" id="panel-toolbar"><button data-btn="primary" id="panel-action">New Payment</button></div></header><div class="body" id="panel-body">Applied Payments</div></section>
        <a data-btn="primary" id="small-action">Current PDF</a><a data-btn="plain" id="plain-action">Account</a><a data-btn="primary" data-size="lg" id="large-action">Large</a>
        <div id="grid" style="width:1440px;padding:0 15px"><div class="row">
            <div class="col" style="--lg:6" id="grid-left">
                <div class="field"><label>Invoice Date</label><div class="widget"><input id="grid-normal"></div></div>
                <div class="field-row"><div><div class="field"><label>Invoice ID</label><div class="widget"><input id="grid-pair"></div></div></div>
                    <div><div class="field"><label>Type</label><div class="widget"><select><option>Sale</option></select></div></div></div></div>
            </div><div class="col" style="--lg:6" id="grid-right"><section class="box" id="grid-panel"></section></div>
        </div></div>
        <nav class="tabs" id="mixed-tabs"><ul><li class="active"><a href="#" id="grid-tab">Invoice</a></li><li><a href="#" id="inactive-tab">Items</a></li><li class="disabled"><span>Orders</span></li></ul></nav>
        <div id="after-tabs">Following content</div>
        <nav class="tabs" id="plain-tabs"><ul><li><a href="#">Invoice</a></li><li><a href="#">Items</a></li></ul></nav>
        <nav class="tabs" id="wrapped-tabs" style="width:70px"><ul><li class="active"><a href="#">Invoice</a></li><li><a href="#">Items</a></li></ul></nav>
        <section class="box" id="table-panel"><div class="body-nopad">
            <div class="table-wrap"><table class="form-list" id="first-panel-table"><thead><tr><th>Payment Applications</th></tr></thead></table></div>
            <div class="table-wrap"><table class="form-list" id="last-panel-table"><thead><tr><th>GL Transactions</th></tr></thead></table></div>
        </div></section>
        <div class="table-wrap" id="overflow-wrapper" style="width:200px"><table class="form-list" style="min-width:400px"><thead><tr><th>Wide table</th></tr></thead></table></div>
        <table class="form-list" style="width:600px;table-layout:fixed">
            <thead><tr><th><div class="title" data-align="right" id="right-title">Total</div></th>
                <th><div class="title" data-align="center" id="center-title">Count</div></th>
                <th><div class="title" id="left-title">Account</div></th></tr></thead>
            <tbody><tr><td><div data-align="right" id="right-value"><span class="display currency">123.45</span></div></td>
                <td><div data-align="center" id="center-value"><span class="display Integer">7</span></div></td>
                <td><div id="left-value"><span class="display">Sales</span></div></td></tr></tbody>
        </table>
        <div style="width:690px" id="vertical-form">
            <div class="field-row" id="vertical-pair"><div><div class="field" id="paired-date"><label>Date</label><div class="widget"><input type="datetime-local" value="2026-10-01T12:00"></div></div></div>
                <div><div class="field"><label>Total</label><div class="widget"><span class="display">$0.00</span></div></div></div></div>
            <div class="field" id="standalone-select"><label>Store</label><div class="widget"><select name="store"><option value="test" selected>Test store</option></select></div></div>
            <div class="field" id="standalone-date"><label>Due Date</label><div class="widget"><input type="date" name="due" value="2026-10-31"></div></div>
            <div class="field" id="multiline-select"><label>Settlement<br>Term</label><div class="widget"><select><option>Net 30</option></select></div></div>
            <div class="field" id="baseline-textarea"><label>Message</label><div class="widget"><textarea name="message" rows="3">Invoice note</textarea></div></div>
            <div id="following-control"><button>Update</button></div>
        </div>
        <table class="form-list" id="filtered-list"><thead><tr class="nav-row"><th>Find Options</th></tr><tr><th class="filter-summary" style="font-weight:normal">
            <span class="filter"><strong>Type:</strong> <span class="filter-value" id="filter-value">Transfer</span></span>
            <form method="post"><button id="filter-clear" class="filter-clear" data-btn="primary" type="submit" name="clearParameters">×</button></form>
        </th></tr><tr id="after-filter"><th>ID</th></tr></thead></table>
        <pre id="result"></pre><script>
        const measure = id => {
            const row = document.getElementById(id);
            const rect = selector => {
                const { x, y, width } = row.querySelector(selector).getBoundingClientRect();
                return { x, y, width };
            };
            return { width:row.getBoundingClientRect().width, widget:rect(':scope > .widget'),
                input:rect('input'), button:rect('button') };
        };
        const statusMetrics = () => {
            const ul = document.querySelector('#invoice-status ul');
            const items = [...ul.querySelectorAll('li')];
            const rects = items.map(item => item.getBoundingClientRect());
            const ulStyle = getComputedStyle(ul);
            const itemStyle = getComputedStyle(items[0]);
            const shadeStyle = getComputedStyle(document.querySelector('#invoice-status span'));
            return { listStyle:ulStyle.listStyleType, listDisplay:ulStyle.display, listMargin:ulStyle.margin,
                listPadding:ulStyle.padding, itemPadding:itemStyle.padding, itemMargin:itemStyle.margin,
                itemDisplay:itemStyle.display, itemFloat:itemStyle.cssFloat,
                horizontal:rects.every((r, i) => i === 0 || (r.x > rects[i - 1].x && Math.abs(r.y - rects[0].y) < 1)),
                spanDisplay:shadeStyle.display, spanWhitespace:shadeStyle.whiteSpace, spanPadding:shadeStyle.padding,
                shadeBackground:shadeStyle.backgroundColor, shadeShadow:shadeStyle.boxShadow };
        };
        const textOffset = (id, alignment) => {
            const element = document.getElementById(id);
            const range = document.createRange();
            range.selectNodeContents(element);
            const text = range.getBoundingClientRect(), box = element.getBoundingClientRect();
            const offset = alignment === 'right' ? box.right - text.right : alignment === 'center'
                ? (text.left + text.right - box.left - box.right) / 2 : text.left - box.left;
            return Math.round(offset * 1000) / 1000;
        };
        const controlMetrics = () => ['text-control', 'select-control', 'textarea-control', 'date-control'].map(id => {
            const style = getComputedStyle(document.getElementById(id));
            return { color:style.color, border:style.borderTopColor, shadow:style.boxShadow };
        });
        const lightControls = controlMetrics();
        const lightStatus = statusMetrics();
        document.documentElement.dataset.theme = 'dark';
        const darkStatus = statusMetrics();
        const darkControls = controlMetrics();
        document.documentElement.dataset.theme = 'light';
        document.getElementById('text-control').focus();
        const focusStyle = getComputedStyle(document.getElementById('text-control'));
        document.getElementById('result').textContent = JSON.stringify({
            filterMetrics: {
                height:document.querySelector('#filtered-list .filter-summary').getBoundingClientRect().height,
                resetHeight:document.getElementById('filter-clear').getBoundingClientRect().height,
                resetFirst:document.getElementById('filter-clear').getBoundingClientRect().left < document.getElementById('filter-value').getBoundingClientRect().left,
                valueColor:getComputedStyle(document.getElementById('filter-value')).color,
                labelWeight:getComputedStyle(document.querySelector('#filtered-list strong')).fontWeight,
                fieldGap:getComputedStyle(document.querySelector('#filtered-list .filter')).marginRight
            },
            invalidBorder:getComputedStyle(document.getElementById('invalid-control')).borderTopColor,
            focusOutline:[focusStyle.outlineStyle, focusStyle.outlineWidth, focusStyle.outlineColor],
            wide:measure('wide'), titled:measure('titled'), narrow:measure('narrow'),
            formMetrics: {
                textHeight:document.getElementById('text-control').getBoundingClientRect().height,
                selectHeight:document.getElementById('select-control').getBoundingClientRect().height,
                staticHeight:document.getElementById('static-control').getBoundingClientRect().height,
                plainStaticHeight:document.getElementById('plain-static-control').getBoundingClientRect().height,
                inlineStaticHeight:document.getElementById('inline-static-control').getBoundingClientRect().height,
                linkHeight:document.getElementById('link-control').getBoundingClientRect().height,
                fieldGap:getComputedStyle(document.getElementById('text-field')).marginBottom,
                labelPadding:getComputedStyle(document.querySelector('#text-field > label')).paddingTop,
                tableControlPadding:getComputedStyle(document.getElementById('table-control')).padding,
                tableControlHeight:document.getElementById('table-control').getBoundingClientRect().height,
                tableDisplayHeight:document.getElementById('table-display').getBoundingClientRect().height,
                tableFieldGap:getComputedStyle(document.getElementById('table-field')).marginBottom,
                bigRowGap:getComputedStyle(document.querySelector('#wide')).marginBottom,
                bigItemGap:getComputedStyle(document.querySelector('#wide .big-item .field')).marginBottom
            },
            lightStatus, darkStatus, lightControls, darkControls,
            checkboxShadow:getComputedStyle(document.getElementById('checkbox-control')).boxShadow,
            alignmentMetrics: {
                rightTitle:textOffset('right-title', 'right'), rightValue:textOffset('right-value', 'right'),
                centerTitle:textOffset('center-title', 'center'), centerValue:textOffset('center-value', 'center'),
                leftTitle:textOffset('left-title', 'left'), leftValue:textOffset('left-value', 'left')
            },
            tableMetrics: {
                betweenTables:document.getElementById('last-panel-table').getBoundingClientRect().top - document.getElementById('first-panel-table').getBoundingClientRect().bottom,
                belowLastTable:document.getElementById('table-panel').getBoundingClientRect().bottom - document.getElementById('last-panel-table').getBoundingClientRect().bottom,
                overflowWidth:document.getElementById('overflow-wrapper').clientWidth,
                scrollWidth:document.getElementById('overflow-wrapper').scrollWidth,
                overflowX:getComputedStyle(document.getElementById('overflow-wrapper')).overflowX
            },
            verticalMetrics: {
                rows:['paired-date', 'standalone-select', 'standalone-date', 'multiline-select', 'baseline-textarea'].map(id => {
                    const field=document.getElementById(id), widget=field.querySelector('.widget');
                    const control=widget.querySelector('input,select,textarea');
                    return { height:field.getBoundingClientRect().height, widgetHeight:widget.getBoundingClientRect().height,
                        controlOffset:control.getBoundingClientRect().top - field.getBoundingClientRect().top,
                        controlHeight:control.getBoundingClientRect().height };
                }),
                pairHeight:document.getElementById('vertical-pair').getBoundingClientRect().height,
                followingGap:document.getElementById('following-control').getBoundingClientRect().top - document.getElementById('baseline-textarea').getBoundingClientRect().bottom,
                values:[document.querySelector('#standalone-select select').value, document.querySelector('#standalone-date input').value, document.querySelector('#baseline-textarea textarea').value]
            },
            tabMetrics: {
                mixedHeight:document.querySelector('#mixed-tabs ul').getBoundingClientRect().height,
                plainHeight:document.querySelector('#plain-tabs ul').getBoundingClientRect().height,
                wrappedHeight:document.querySelector('#wrapped-tabs ul').getBoundingClientRect().height,
                activeBottomGap:document.querySelector('#mixed-tabs ul').getBoundingClientRect().bottom - document.getElementById('grid-tab').getBoundingClientRect().bottom,
                inactiveBottomGap:document.querySelector('#mixed-tabs ul').getBoundingClientRect().bottom - document.getElementById('inactive-tab').getBoundingClientRect().bottom,
                followingGap:document.getElementById('after-tabs').getBoundingClientRect().top - document.querySelector('#mixed-tabs ul').getBoundingClientRect().bottom
            },
            gridMetrics: {
                columnWidth:document.getElementById('grid-left').getBoundingClientRect().width,
                columnPadding:getComputedStyle(document.getElementById('grid-left')).padding,
                normalInput:document.getElementById('grid-normal').getBoundingClientRect().left - document.getElementById('grid').getBoundingClientRect().left,
                pairInput:document.getElementById('grid-pair').getBoundingClientRect().left - document.getElementById('grid').getBoundingClientRect().left,
                rightPanel:document.getElementById('grid-panel').getBoundingClientRect().left - document.getElementById('grid').getBoundingClientRect().left,
                tabHeight:document.getElementById('grid-tab').getBoundingClientRect().height,
                tabPadding:getComputedStyle(document.getElementById('grid-tab')).padding
            },
            widgetMetrics: {
                actionHeight:document.getElementById('small-action').getBoundingClientRect().height,
                actionFont:getComputedStyle(document.getElementById('small-action')).fontSize,
                plainFont:getComputedStyle(document.getElementById('plain-action')).fontSize,
                largeFont:getComputedStyle(document.getElementById('large-action')).fontSize,
                headingPadding:getComputedStyle(document.getElementById('panel-heading')).padding,
                headingFont:getComputedStyle(document.getElementById('panel-title')).fontSize,
                headingWeight:getComputedStyle(document.getElementById('panel-title')).fontWeight,
                headingBorder:getComputedStyle(document.getElementById('panel-heading')).borderBottomColor,
                bodyPadding:getComputedStyle(document.getElementById('panel-body')).padding,
                toolbarTop:document.getElementById('panel-toolbar').getBoundingClientRect().top - document.getElementById('panel-heading').getBoundingClientRect().top,
                toolbarHeightIndependent:document.getElementById('panel-heading').getBoundingClientRect().height < document.getElementById('panel-toolbar').getBoundingClientRect().height + 20
            },
            toolbarMetrics: {
                savedHeight:document.getElementById('saved-find').getBoundingClientRect().height,
                savedWidth:document.getElementById('saved-find').getBoundingClientRect().width,
                savedMargin:getComputedStyle(document.getElementById('saved-find')).margin,
                csvHeight:document.getElementById('csv').getBoundingClientRect().height,
                csvFont:getComputedStyle(document.getElementById('csv')).fontSize
            },
            javaMetrics: {
                font:getComputedStyle(document.body).fontFamily, fontSize:getComputedStyle(document.body).fontSize,
                lineHeight:getComputedStyle(document.body).lineHeight, background:getComputedStyle(document.body).backgroundColor,
                color:getComputedStyle(document.body).color, cellPadding:getComputedStyle(document.getElementById('cell')).padding,
                cellAlign:getComputedStyle(document.getElementById('cell')).verticalAlign,
                rowHeight:document.getElementById('product').getBoundingClientRect().height,
                headerHeight:document.getElementById('heading').getBoundingClientRect().height,
                sortColor:getComputedStyle(document.querySelector('.order-by a.active')).color,
                dialogFont:getComputedStyle(document.getElementById('NewProductDialog-button')).fontSize,
                dialogPadding:getComputedStyle(document.getElementById('NewProductDialog-button')).padding,
                buttonPadding:getComputedStyle(document.getElementById('button')).padding,
                strongWeight:getComputedStyle(document.getElementById('strong')).fontWeight,
                stripe:getComputedStyle(document.getElementById('product')).backgroundColor,
                navMargin:getComputedStyle(document.getElementById('find')).margin,
                pagePadding:getComputedStyle(document.getElementById('page')).padding,
                paginationDisplay:getComputedStyle(document.querySelector('.pagination')).display,
                pageFloat:getComputedStyle(document.getElementById('page')).cssFloat,
                csvBeforePagination:document.getElementById('csv').getBoundingClientRect().x < document.getElementById('page').getBoundingClientRect().x
            }
        });
        </script>`;
    const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'moqui-html-css-'));
    let dom;
    try {
        dom = execFileSync(process.env.CHROMIUM_PATH || 'chromium', [
            '--headless', '--disable-gpu', '--window-size=1440,1000', '--user-data-dir=' + profile,
            '--dump-dom', 'data:text/html,' + encodeURIComponent(html)
        ], { encoding: 'utf8', timeout: 20000, maxBuffer: 1024 * 1024, stdio: ['ignore', 'pipe', 'pipe'] });
    } finally {
        fs.rmSync(profile, { recursive: true, force: true });
    }
    const { toolbarMetrics, filterMetrics, wide, titled, narrow, javaMetrics, formMetrics, lightStatus, darkStatus, widgetMetrics, gridMetrics, tableMetrics, alignmentMetrics, lightControls, darkControls, checkboxShadow, invalidBorder, focusOutline, tabMetrics, verticalMetrics } = JSON.parse(dom.match(/<pre id="result">([^<]+)<\/pre>/)[1]);
    assert.deepEqual(toolbarMetrics, { savedHeight:34, savedWidth:160, savedMargin:'6px 0px', csvHeight:34, csvFont:'14px' });
    // Java's quick-clear floats left at 20px, without making the filter header taller.
    assert.deepEqual(filterMetrics, { height:32, resetHeight:20, resetFirst:true,
        valueColor:'rgb(60, 118, 61)', labelWeight:'700', fieldGap:'0px' });
    // Java Bootstrap .form-control and select2-bootstrap use #555 text, #ccc borders and an inset shadow.
    assert.deepEqual(lightControls, Array(4).fill({ color:'rgb(85, 85, 85)', border:'rgb(204, 204, 204)',
        shadow:'rgba(0, 0, 0, 0.075) 0px 1px 1px 0px inset' }));
    // Keep the existing dark palette and native checkbox appearance.
    assert.deepEqual(darkControls, Array(4).fill({ color:'rgb(221, 227, 234)', border:'rgb(52, 61, 72)', shadow:'none' }));
    assert.equal(checkboxShadow, 'none');
    assert.equal(invalidBorder, 'rgb(217, 83, 79)');
    assert.deepEqual(focusOutline, ['solid', '2px', 'rgb(51, 122, 183)']);
    // Java input-group/select2 have -2px top margins. Textareas retain their baseline descent.
    // A paired static value and a multiline label still determine their respective row heights.
    assert.deepEqual(verticalMetrics, {
        rows:[{ height:32, widgetHeight:32, controlOffset:-2, controlHeight:34 },
            { height:32, widgetHeight:32, controlOffset:-2, controlHeight:34 },
            { height:32, widgetHeight:32, controlOffset:-2, controlHeight:34 },
            { height:47, widgetHeight:32, controlOffset:-2, controlHeight:34 },
            { height:79, widgetHeight:79, controlOffset:0, controlHeight:74 }],
        pairHeight:49, followingGap:15, values:['test', '2026-10-31', 'Invoice note']
    });
    // Bootstrap .nav-tabs gives every li a -1px bottom margin so links overlap the shared border.
    assert.deepEqual(tabMetrics, { mixedHeight:42, plainHeight:42, wrappedHeight:83,
        activeBottomGap:0, inactiveBottomGap:0, followingGap:8 });
    // Allow the browser's fractional-pixel text rounding, while checking actual glyph placement.
    for (const [field, offset] of Object.entries(alignmentMetrics)) {
        assert.ok(Math.abs(offset) < 0.02, field + ' alignment offset: ' + offset);
    }
    // Java tables have one 8px bottom margin; their scroll wrappers add no spacing.
    assert.deepEqual(tableMetrics, {
        betweenTables:8, belowLastTable:9, overflowWidth:200, scrollWidth:400, overflowX:'auto'
    });
    // Java webroot-layout.css/table-condensed and webroot-theme.css flattening, with Bootstrap typography.
    // Java Bootstrap 15px column padding/negative row margins and webroot-layout.css nav padding.
    assert.deepEqual(gridMetrics, {
        columnWidth:720, columnPadding:'0px 15px', normalInput:135, pairInput:135,
        rightPanel:735, tabHeight:42, tabPadding:'10px 6px'
    });
    assert.deepEqual(widgetMetrics, {
        actionHeight:30, actionFont:'12px', plainFont:'14px', largeFont:'15.4px',
        headingPadding:'10px 15px', headingFont:'16px', headingWeight:'500',
        headingBorder:'rgba(0, 0, 0, 0)', bodyPadding:'8px', toolbarTop:0,
        toolbarHeightIndependent:true
    });
    assert.deepEqual(javaMetrics, {
        font: '"Helvetica Neue", Helvetica, Arial, sans-serif', fontSize: '14px', lineHeight: '20px',
        background: 'rgb(255, 255, 255)', color: 'rgb(34, 34, 34)', cellPadding: '5px', cellAlign: 'middle',
        rowHeight: 51, headerHeight: 51, sortColor: 'rgb(255, 0, 0)', dialogFont: '12px', dialogPadding: '5px 10px',
        buttonPadding: '6px 12px', strongWeight: '700', stripe: 'rgba(0, 0, 0, 0)',
        navMargin: '6px 8px 0px 0px', pagePadding: '6px 12px',
        paginationDisplay: 'inline-block', pageFloat: 'left', csvBeforePagination: true
    });
    assert.equal(wide.widget.width, wide.width);
    assert.ok(wide.input.width > 300, 'size=40 input is not squeezed into the unused label column');
    assert.ok(Math.abs(wide.input.y - wide.button.y) < 10, 'input and button share a line');
    assert.ok(titled.widget.x > wide.widget.x + 50, 'titled rows still reserve label space');
    assert.ok(titled.widget.width < titled.width);
    assert.equal(narrow.widget.width, narrow.width);
    assert.ok(narrow.input.width <= narrow.width, 'inputs stay constrained at narrow widths');
    assert.ok(narrow.button.y > narrow.input.y + 20, 'controls may wrap when space is genuinely limited');
    assert.deepEqual(formMetrics, {
        textHeight: 34, selectHeight: 34, staticHeight: 34, plainStaticHeight: 34,
        inlineStaticHeight: 34, linkHeight: 34,
        fieldGap: '15px', labelPadding: '7px', tableControlPadding: '4px 8px', tableControlHeight: 30,
        tableDisplayHeight: 20, tableFieldGap: '0px',
        bigRowGap: '8px', bigItemGap: '0px'
    });
    for (const status of [lightStatus, darkStatus]) {
        assert.equal(status.listStyle, 'none');
        assert.equal(status.listDisplay, 'inline-block');
        assert.equal(status.listMargin, '0px');
        assert.equal(status.listPadding, '0px');
        assert.equal(status.itemPadding, '4px');
        assert.equal(status.itemMargin, '0px 4px 0px 0px');
        // CSSOM reports block after float:left computes the declared inline-block display.
        assert.equal(status.itemDisplay, 'block');
        assert.equal(status.itemFloat, 'left');
        assert.equal(status.horizontal, true, 'status actions occupy one horizontal row');
        assert.equal(status.spanDisplay, 'inline-block');
        assert.equal(status.spanWhitespace, 'normal');
        assert.equal(status.spanPadding, '4px');
    }
    assert.equal(lightStatus.shadeBackground, 'rgb(248, 248, 248)');
    assert.equal(lightStatus.shadeShadow, 'rgb(248, 248, 248) 0px 0px 0px 1px inset, rgb(204, 204, 204) 0px 0px 0px 1px');
    assert.match(lightStatus.shadeShadow, /rgb\(204, 204, 204\)/);
    assert.equal(darkStatus.shadeBackground, 'rgb(63, 63, 63)');
    assert.equal(darkStatus.shadeShadow, 'rgb(8, 8, 8) 0px 0px 0px 1px inset, rgb(51, 51, 51) 0px 0px 0px 1px');
    assert.match(darkStatus.shadeShadow, /rgb\(51, 51, 51\)/);
});
