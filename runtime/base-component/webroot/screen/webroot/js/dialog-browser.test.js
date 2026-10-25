/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// CHROMIUM_PATH=/path/to/chrome node --test dialog-browser.test.js
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const { execFileSync } = require('node:child_process');

test('dialogs use Java top spacing and dismiss only on backdrop clicks', () => {
    const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'moqui-dialog-'));
    try {
        const css = fs.readFileSync(path.join(__dirname, '../css/moqui-html.css'), 'utf8');
        const js = fs.readFileSync(path.join(__dirname, 'moqui-html.js'), 'utf8');
        const file = path.join(directory, 'dialog.html');
        fs.writeFileSync(file, `<!doctype html><html data-theme="light"><style>${css}</style>
            <button id="open" data-dialog="customer">New Customer</button>
            <dialog id="customer"><header><h4>New Customer</h4><button class="close" data-close>×</button></header>
                <div class="body"><input name="firstName">
                <div class="field big-row"><label>Name</label><div class="widget">
                <div class="big-item"><div class="field"><div class="widget"><input id="first" size="30"></div></div></div>
                <div class="big-item"><div class="field"><div class="widget"><input id="last" size="30"></div></div></div>
                </div></div><div style="height:300px"></div></div></dialog>
            <pre id="result"></pre><script>${js}</script><script>
            document.getElementById('open').click();
            const dialog = document.getElementById('customer');
            const rect = dialog.getBoundingClientRect();
            const header = dialog.querySelector('header').getBoundingClientRect();
            const closeTop = dialog.querySelector('[data-close]').getBoundingClientRect().top;
            const focused = document.activeElement.name;
            const first = document.getElementById('first').getBoundingClientRect();
            const last = document.getElementById('last').getBoundingClientRect();
            dialog.dispatchEvent(new MouseEvent('click', {bubbles:true,clientX:rect.left+2,clientY:rect.top+2}));
            const staysOpenInside = dialog.open;
            dialog.dispatchEvent(new MouseEvent('click', {bubbles:true,clientX:1,clientY:1}));
            document.getElementById('result').textContent = JSON.stringify({
                top:rect.top,left:rect.left,width:rect.width,headerHeight:header.height,
                focused,staysOpenInside,closesOutside:!dialog.open,
                namesShareRow:Math.abs(first.top-last.top)<1,closeTop
            });</script>`);
        for (const width of [1440, 600]) {
            const html = execFileSync(process.env.CHROMIUM_PATH || '/usr/bin/chromium', [
                '--headless', '--virtual-time-budget=2000', '--disable-extensions', '--no-first-run', '--no-default-browser-check', '--disable-background-networking', '--no-sandbox', '--disable-gpu', '--disable-dev-shm-usage',
                `--window-size=${width},1000`, '--dump-dom', file
            ], { encoding:'utf8', timeout:15000, stdio:['ignore','pipe','ignore'] });
            const metrics = JSON.parse(html.match(/<pre id="result">(.*?)<\/pre>/s)[1]);
            assert.equal(metrics.top, width >= 768 ? 30 : 10);
            assert.ok(Math.abs(metrics.left - (width - metrics.width) / 2) < 1, 'dialog is horizontally centered');
            assert.ok(Math.abs(metrics.headerHeight - 56.7) < 1, `header preserves Java title and padding metrics: ${JSON.stringify(metrics)}`);
            assert.equal(metrics.focused, 'firstName');
            assert.equal(metrics.closeTop, metrics.top + 14, 'close control aligns with Java header');
            if (width >= 768) assert.equal(metrics.namesShareRow, true, 'customer first and last name fit on one desktop row');
            assert.equal(metrics.staysOpenInside, true, 'clicking dialog padding must not dismiss it');
            assert.equal(metrics.closesOutside, true, 'clicking the backdrop dismisses it');
        }
    } finally {
        fs.rmSync(directory, {recursive:true, force:true});
    }
});
