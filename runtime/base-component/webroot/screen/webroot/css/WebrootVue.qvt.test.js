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
    <div id="denseField" class="q-field q-field--dense q-field--labeled q-field--float"><span id="fieldLabel" class="q-field__label">Email</span><input id="fieldInput" class="q-field__native"></div><div class="row"><div id="rowField" class="q-field q-field--dense"></div></div>
    <section id="plainCard" class="q-card__section q-card__section--vert"><div id="bigRow" class="big-row" style="height:60px"><div id="bigFirst" class="q-mx-sm q-my-auto big-row-item" style="width:50px;height:40px"></div><div id="bigSecond" class="q-mx-sm q-my-auto big-row-item" style="width:30px;height:20px"></div></div></section>
    <pre id="result"></pre>
    <script>
        const style = id => getComputedStyle(document.getElementById(id));
        document.getElementById('result').textContent = JSON.stringify({
            denseMargin:style('denseField').margin, rowFieldMargin:style('rowField').margin,
            labelTop:style('fieldLabel').top, labelPadding:style('fieldLabel').paddingLeft,
            inputPadding:style('fieldInput').paddingTop,
            cardPadding:style('plainCard').padding,
            bigSpacing:document.getElementById('bigSecond').getBoundingClientRect().left - document.getElementById('bigFirst').getBoundingClientRect().right,
            bigAligned:document.getElementById('bigFirst').getBoundingClientRect().top + 20 === document.getElementById('bigSecond').getBoundingClientRect().top + 10,
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
            '--headless', '--disable-gpu',
            '--dump-dom', 'file://' + fixture
        ], { encoding: 'utf8', timeout: 20000, maxBuffer: 1024 * 1024, stdio: ['ignore', 'pipe', 'pipe'] });
    } finally {
        fs.rmSync(profile, { recursive: true, force: true });
    }
    const result = JSON.parse(dom.match(/<pre id="result">([^<]+)<\/pre>/)[1]);
    assert.deepEqual(result, {
        denseMargin: '6px 0px', rowFieldMargin: '0px', labelTop: '0px', labelPadding: '4px', inputPadding: '8px',
        cardPadding: '8px', bigSpacing: 16, bigAligned: true,
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

// The page and the record header (style-guide §3.3, §5.5) at a width: computed styles of a fixture, from headless Chromium.
function recordHeaderStyles(width) {
    const css = fs.readFileSync(path.join(__dirname, 'WebrootVue.qvt.css'), 'utf8');
    const quasar = fs.readFileSync(path.join(__dirname, '../libs/quasar/quasar.prod.css'), 'utf8');
    const header = (id) => `<div id="${id}" class="m-record-header">
        <div class="row"><div class="col-12 col-md m-record-title"><h5 id="${id}Title">Call the supplier</h5></div>
            <div class="col-12 col-md-auto m-record-status"><div id="${id}Bar" class="m-status-bar row items-center">
                <div id="${id}Stages" class="m-status-stages row items-center"><span>Open</span><span>Done</span></div>
                <div class="m-status-actions row items-center"><button>Complete</button></div></div></div></div>
        <div id="${id}Foot" class="row m-record-foot"><div class="col-12 col-md">Due</div>
            <div id="${id}Actions" class="col-12 col-md-auto row items-center m-record-actions"><button>Reassign</button></div></div></div>`;
    const html = `<style>${quasar}\n${css}</style>
    <div id="card" class="q-card">Card</div>
    ${header('light')}
    <div class="body--dark">${header('dark')}</div>
    <pre id="result"></pre>
    <script>
        const style = id => getComputedStyle(document.getElementById(id));
        document.getElementById('result').textContent = JSON.stringify({
            page:getComputedStyle(document.body).backgroundColor, card:style('card').backgroundColor,
            header:style('light').backgroundColor, headerBorder:style('light').borderTopColor,
            darkHeaderBorder:style('dark').borderTopColor, titleMargin:style('lightTitle').marginTop,
            titleLineHeight:style('lightTitle').lineHeight, footBorder:style('lightFoot').borderTopStyle,
            barDirection:style('lightBar').flexDirection, stagesJustify:style('lightStages').justifyContent,
            actionsJustify:style('lightActions').justifyContent
        });
    </script>`;
    const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'moqui-qvt-css-'));
    let dom;
    try {
        const fixture = path.join(profile, 'fixture.html');
        fs.writeFileSync(fixture, html);
        dom = execFileSync(process.env.CHROMIUM_PATH || 'chromium', [
            '--headless', '--disable-gpu',  '--window-size=' + width + ',800',
            '--dump-dom', 'file://' + fixture
        ], { encoding: 'utf8', timeout: 20000, maxBuffer: 4 * 1024 * 1024, stdio: ['ignore', 'pipe', 'pipe'] });
    } finally {
        fs.rmSync(profile, { recursive: true, force: true });
    }
    return JSON.parse(dom.match(/<pre id="result">([^<]+)<\/pre>/)[1]);
}

test('a light page under white cards, and the record header with its status-bar at the right from 1024px', () => {
    const common = {
        // the light page is a shade darker than its cards, as the dark page is; the header is a white bordered card in light,
        // with the dark separator in dark mode; the title has its own room
        page: 'rgb(244, 245, 247)', card: 'rgb(255, 255, 255)', header: 'rgb(255, 255, 255)',
        headerBorder: 'rgba(0, 0, 0, 0.12)', darkHeaderBorder: 'rgba(255, 255, 255, 0.28)',
        titleMargin: '0px', titleLineHeight: '24px', footBorder: 'solid'
    };
    assert.deepEqual(recordHeaderStyles(1280), Object.assign({}, common,
        { barDirection: 'column', stagesJustify: 'flex-end', actionsJustify: 'flex-end' }));
    assert.deepEqual(recordHeaderStyles(600), Object.assign({}, common,
        { barDirection: 'row', stagesJustify: 'normal', actionsJustify: 'normal' }));
});
