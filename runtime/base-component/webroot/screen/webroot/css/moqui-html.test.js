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
        ${row('wide', 690)}${row('titled', 690, 'Title')}${row('narrow', 250)}
        <table class="form-list"><thead><tr id="heading"><th><div class="title">ID <span class="order-by"><a href="#">▲</a><a class="active" href="#">▼</a></span></div><div class="title">Name</div></th></tr></thead>
        <tbody><tr id="product"><td id="cell"><div><a href="#">DEMO_1_1</a></div><div><a href="#">Demo Product One-One</a></div></td>
        <td><div><span class="display">Inventory</span></div><div><span class="display">Inventory - Finished Good</span></div></td></tr></tbody></table>
        <table class="form-list"><tbody><tr><td id="compact-cell"><div class="field" id="table-field"><label>Compact</label><div class="widget"><input id="table-control"><span class="display" id="table-display">Compact</span></div></div></td></tr></tbody></table>
        <button id="button">Find Options</button><button id="NewProductDialog-button" data-dialog="NewProductDialog">New Product</button><strong id="strong">Label</strong>
        <div id="invoice-status"><span class="shaded-area condensed"><ul class="float-plain condensed"><li><div><strong>Status</strong></div><div><strong>In-Process</strong></div></li><li><button>Finalize</button></li><li><button>Cancel</button></li></ul></span></div>
        <table class="form-list"><thead><tr class="nav-row"><th><nav><button id="find">Find Options</button><ul class="pagination"><li><span id="page">Page 1</span></li></ul><a id="csv" data-btn="default">CSV</a></nav></th></tr></thead></table>
        <section class="box" id="panel"><header id="panel-heading"><h5 id="panel-title">Payment Applications</h5><div class="toolbar" id="panel-toolbar"><button data-btn="primary" id="panel-action">New Payment</button></div></header><div class="body" id="panel-body">Applied Payments</div></section>
        <a data-btn="primary" id="small-action">Current PDF</a><a data-btn="plain" id="plain-action">Account</a><a data-btn="primary" data-size="lg" id="large-action">Large</a>
        <div id="grid" style="width:1440px;padding:0 15px"><div class="row">
            <div class="col" style="--lg:6" id="grid-left">
                <div class="field"><label>Invoice Date</label><div class="widget"><input id="grid-normal"></div></div>
                <div class="field-row"><div><div class="field"><label>Invoice ID</label><div class="widget"><input id="grid-pair"></div></div></div>
                    <div><div class="field"><label>Type</label><div class="widget"><select><option>Sale</option></select></div></div></div></div>
            </div><div class="col" style="--lg:6" id="grid-right"><section class="box" id="grid-panel"></section></div>
        </div></div>
        <nav class="tabs"><ul><li class="active"><a href="#" id="grid-tab">Invoice</a></li></ul></nav>
        <section class="box" id="table-panel"><div class="body-nopad">
            <div class="table-wrap"><table class="form-list" id="first-panel-table"><thead><tr><th>Payment Applications</th></tr></thead></table></div>
            <div class="table-wrap"><table class="form-list" id="last-panel-table"><thead><tr><th>GL Transactions</th></tr></thead></table></div>
        </div></section>
        <div class="table-wrap" id="overflow-wrapper" style="width:200px"><table class="form-list" style="min-width:400px"><thead><tr><th>Wide table</th></tr></thead></table></div>
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
        const lightStatus = statusMetrics();
        document.documentElement.dataset.theme = 'dark';
        const darkStatus = statusMetrics();
        document.documentElement.dataset.theme = 'light';
        document.getElementById('result').textContent = JSON.stringify({
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
            lightStatus, darkStatus,
            tableMetrics: {
                betweenTables:document.getElementById('last-panel-table').getBoundingClientRect().top - document.getElementById('first-panel-table').getBoundingClientRect().bottom,
                belowLastTable:document.getElementById('table-panel').getBoundingClientRect().bottom - document.getElementById('last-panel-table').getBoundingClientRect().bottom,
                overflowWidth:document.getElementById('overflow-wrapper').clientWidth,
                scrollWidth:document.getElementById('overflow-wrapper').scrollWidth,
                overflowX:getComputedStyle(document.getElementById('overflow-wrapper')).overflowX
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
    const { wide, titled, narrow, javaMetrics, formMetrics, lightStatus, darkStatus, widgetMetrics, gridMetrics, tableMetrics } = JSON.parse(dom.match(/<pre id="result">([^<]+)<\/pre>/)[1]);
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
