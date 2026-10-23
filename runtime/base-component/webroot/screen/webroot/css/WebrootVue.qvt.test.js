/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// CHROMIUM_PATH=/usr/bin/chromium node --test runtime/base-component/webroot/screen/webroot/css/WebrootVue.qvt.test.js
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const { execFileSync } = require('node:child_process');

test('Quasar theme colors, labels and sort layout match Java', () => {
    const css = fs.readFileSync(path.join(__dirname, 'WebrootVue.qvt.css'), 'utf8');
    const quasar = fs.readFileSync(path.join(__dirname, '../libs/quasar/quasar.prod.css'), 'utf8');
    const html = `<style>
        ${quasar}
        :root { --q-primary:rgb(1,2,3); --q-positive:rgb(4,5,6); --q-negative:rgb(7,8,9); }
        ${css}
        .q-menu, .q-toolbar { color:rgb(10,11,12); }
    </style>
    <a id="link" href="#">Link</a>
    <span id="success" class="text-success">Success</span>
    <span id="danger" class="text-danger">Danger</span>
    <span id="sort" class="form-order-by"><a id="active" class="active" href="#">▲</a> <a id="descending" href="#">▼</a></span>
    <div class="q-menu"><a id="menu" href="#">Menu</a></div>
    <div class="q-toolbar"><a id="toolbar" href="#">Toolbar</a></div>
    <p id="paragraph" class="text-inline">Promotion note</p>
    <span id="inline" class="text-inline">Inline label</span>
    <button id="dense" class="q-btn q-btn--dense"><span class="q-btn__content">Find Options</span></button>
    <a id="flat" class="q-btn q-btn--dense q-btn--flat m-link"><span class="q-btn__content">Product</span></a>
    <button id="round" class="q-btn q-btn--dense q-btn--round">Icon</button>
    <span id="muted" class="m-muted">Muted caption</span>
    <div id="chip" class="q-chip m-tonal">Approved</div>
    <div id="container" class="q-table__container q-table__card" style="width:600px">
        <div class="table q-table"><div class="thead"><div class="tr">
            <div id="selHead" class="th m-sel">x</div><div class="th">Name</div><div class="th">Description</div>
        </div></div></div>
    </div>
    <pre id="result"></pre>
    <script>
        const style = id => getComputedStyle(document.getElementById(id));
        document.getElementById('result').textContent = JSON.stringify({
            link:style('link').color, success:style('success').color, danger:style('danger').color,
            active:style('active').color,
            menu:style('menu').color, toolbar:style('toolbar').color,
            paragraph:style('paragraph').display, inline:style('inline').display,
            densePadding:style('dense').padding, flatPadding:style('flat').padding,
            flatMinHeight:style('flat').minHeight, roundPadding:style('round').padding,
            sort:style('sort').display, sortLink:style('active').display,
            sortSameLine:document.getElementById('active').getBoundingClientRect().top ===
                document.getElementById('descending').getBoundingClientRect().top,
            muted:style('muted').color, chipColor:style('chip').color,
            chipTinted:style('chip').backgroundColor !== 'rgb(224, 224, 224)',
            containerOverflow:style('container').overflowX,
            selNarrow:document.getElementById('selHead').getBoundingClientRect().width < 60
        });
    </script>`;
    const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'moqui-qvt-css-'));
    let dom;
    try {
        const fixture = path.join(profile, 'fixture.html');
        fs.writeFileSync(fixture, html);
        dom = execFileSync(process.env.CHROMIUM_PATH || 'chromium', [
            '--headless', '--disable-gpu', '--user-data-dir=' + profile,
            '--dump-dom', 'file://' + fixture
        ], { encoding: 'utf8', timeout: 20000, maxBuffer: 1024 * 1024, stdio: ['ignore', 'pipe', 'pipe'] });
    } finally {
        fs.rmSync(profile, { recursive: true, force: true });
    }
    const result = JSON.parse(dom.match(/<pre id="result">([^<]+)<\/pre>/)[1]);
    assert.deepEqual(result, {
        link: 'rgb(1, 2, 3)', success: 'rgb(4, 5, 6)', danger: 'rgb(7, 8, 9)',
        active: 'rgb(1, 2, 3)',
        menu: 'rgb(10, 11, 12)', toolbar: 'rgb(10, 11, 12)',
        paragraph: 'block', inline: 'inline',
        densePadding: '4px 8px', flatPadding: '0px 4px', flatMinHeight: '21px', roundPadding: '0px',
        sort: 'inline', sortLink: 'inline', sortSameLine: true,
        // the muted caption is grey-7; a tonal chip is primary text over a tint, not Quasar's grey chip; a list wider than its
        // card scrolls inside it; the selection column is as narrow as its checkbox
        muted: 'rgb(97, 97, 97)', chipColor: 'rgb(1, 2, 3)', chipTinted: true, containerOverflow: 'auto', selNarrow: true
    });
});

test('list typography preserves Java text size and multiline row heights in both densities and themes', () => {
    const css = fs.readFileSync(path.join(__dirname, 'WebrootVue.qvt.css'), 'utf8');
    const quasar = fs.readFileSync(path.join(__dirname, '../libs/quasar/quasar.prod.css'), 'utf8');
    const table = (theme, dense) => `<div class="${theme === 'dark' ? 'body--dark' : ''}">
        <div data-case="${theme}-${dense}" class="q-table__container q-table--horizontal-separator ${dense ? 'q-table--dense' : ''} ${theme === 'dark' ? 'q-table--dark' : ''}">
            <div class="q-table table"><div class="thead"><div class="tr"><div class="th">Product</div><div class="th">Type</div></div></div>
                <div class="tbody"><div class="tr"><div class="td"><div>DEMO_1_1</div><div>Demo Product One-One</div></div><div class="td">Inventory</div></div>
                    <div class="tr"><div class="td">DEMO_1_2</div><div class="td">Inventory</div></div></div>
            </div>
        </div></div>`;
    const html = `<!DOCTYPE html><style>${quasar}\n${css}</style>` +
        ['light', 'dark'].flatMap(theme => [false, true].map(dense => table(theme, dense))).join('') +
        `<pre id="result"></pre><script>
            document.getElementById('result').textContent = JSON.stringify([...document.querySelectorAll('[data-case]')].map(table => {
                const cell = table.querySelector('.tbody .td'), heading = table.querySelector('.th');
                return { name: table.dataset.case, headerSize: getComputedStyle(heading).fontSize,
                    bodySize: getComputedStyle(cell).fontSize, lineHeight: getComputedStyle(cell).lineHeight,
                    rowHeight: cell.getBoundingClientRect().height };
            }));
        </script>`;
    const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'moqui-list-type-'));
    try {
        const fixture = path.join(profile, 'fixture.html');
        fs.writeFileSync(fixture, html);
        const dom = execFileSync(process.env.CHROMIUM_PATH || 'chromium', [
            '--headless', '--disable-gpu', '--user-data-dir=' + profile, '--dump-dom', 'file://' + fixture
        ], { encoding: 'utf8', timeout: 20000, maxBuffer: 8 * 1024 * 1024, stdio: ['ignore', 'pipe', 'pipe'] });
        const match = dom.match(/<pre id="result">([^<]+)<\/pre>/);
        assert.ok(match, 'browser did not report table metrics');
        const result = JSON.parse(match[1].replace(/&amp;/g, '&').replace(/&quot;/g, '"'));
        assert.deepEqual(result, ['light', 'dark'].flatMap(theme => [false, true].map(dense => ({
            name: `${theme}-${dense}`, headerSize: '13px', bodySize: '14px', lineHeight: '21px', rowHeight: dense ? 51 : 57
        }))));
    } finally {
        fs.rmSync(profile, { recursive: true, force: true });
    }
});

test('alternate list views retain header tools and hide rows and footers in both renderers', () => {
    const css = fs.readFileSync(path.join(__dirname, 'WebrootVue.qvt.css'), 'utf8');
    const cases = ['list', 'board', 'calendar', 'tree'].flatMap(view => ['merged', 'static'].map(renderer => {
        const top = '<div class="form-list-nav-row" data-top>Find Options</div>';
        const footer = '<div class="form-list-nav-row" data-footer>Next page</div>';
        const head = '<div class="thead"><div class="form-header-field">Filter</div>' + (renderer === 'merged' ? top : '') + '</div>';
        const rows = '<div class="tbody"><div data-row>Invoice</div>' + (renderer === 'merged' ? footer : '') + '</div>';
        return `<div data-case="${view}-${renderer}" class="m-list-views--${view}"><div class="m-list-views-list">
            <div class="q-table__container">${renderer === 'static' ? top : ''}<div class="table">${head}${rows}</div>${renderer === 'static' ? footer : ''}</div>
        </div></div>`;
    }));
    const html = `<!DOCTYPE html><style>${css}</style>${cases.join('')}<pre id="result"></pre><script>
        document.getElementById('result').textContent = JSON.stringify([...document.querySelectorAll('[data-case]')].map(el => ({
            name: el.dataset.case,
            top: el.querySelector('[data-top]').getClientRects().length > 0,
            row: el.querySelector('[data-row]').getClientRects().length > 0,
            footer: el.querySelector('[data-footer]').getClientRects().length > 0
        })));
    </script>`;
    const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'moqui-list-views-css-'));
    try {
        const fixture = path.join(profile, 'fixture.html');
        fs.writeFileSync(fixture, html);
        const dom = execFileSync(process.env.CHROMIUM_PATH || 'chromium', [
            '--headless', '--disable-gpu', '--user-data-dir=' + profile, '--dump-dom', 'file://' + fixture
        ], { encoding: 'utf8', timeout: 20000, maxBuffer: 1024 * 1024, stdio: ['ignore', 'pipe', 'pipe'] });
        const match = dom.match(/<pre id="result">([^<]+)<\/pre>/);
        assert.ok(match, 'browser did not report list visibility');
        assert.deepEqual(JSON.parse(match[1]), ['list', 'board', 'calendar', 'tree'].flatMap(view => ['merged', 'static'].map(renderer => ({
            name: `${view}-${renderer}`, top: true, row: view === 'list', footer: view === 'list'
        }))));
    } finally {
        fs.rmSync(profile, { recursive: true, force: true });
    }
});
