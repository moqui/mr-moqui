/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const { execFileSync } = require('node:child_process');

test('Java dropdown widget opens inside a native modal and submits native values', () => {
    const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'moqui-select2-'));
    const web = path.join(__dirname, '..');
    const read = relative => fs.readFileSync(path.join(web, relative), 'utf8').replace(/<\/script/gi, '<\\/script');
    try {
        const html = `<!doctype html><html data-theme="light"><style>${read('css/moqui-html.css')}
            ${read('libs/select2/select2.min.css')}${read('libs/select2/select2-bootstrap.min.css')}</style>
            <button data-dialog="customer" id="open">Customer</button><dialog id="customer"><header>Customer</header><div class="body">
            <form id="form"><div class="field"><label>Types</label><div class="widget"><select id="types" name="types" multiple data-dropdown="select2"><option value="a">A</option></select></div></div><div class="field"><label>Role</label><div class="widget"><select id="role" name="role" data-dropdown="select2">
            <option value="customer">Customer</option><option value="employee">Employee</option></select></div></div><select id="region" name="region" data-dropdown="select2" data-url="/regions"
            data-depends='[["role","role"]]' data-value="value" data-label="label"></select></form></div></dialog>
            <pre id="result"></pre><script>${read('libs/jquery/jquery.min.js')}</script><script>${read('libs/select2/select2.js')}</script>
            <script>
            const requests = [];
            let initial; 
            window.fetch = async (url, options) => {
                const role = new URLSearchParams(options.body).get('role');requests.push(role);
                return new Response(JSON.stringify([{value:role+'-region',label:role+' region'}]));
            };</script><script>${read('js/moqui-html.js')}</script><script>
            document.getElementById('open').click();
            initial = document.getElementById('role').getBoundingClientRect().width;
            setTimeout(() => {
                try {
                const select = document.getElementById('role'), dialog = document.getElementById('customer');
                let nativeChanges = 0;
                select.addEventListener('change', () => nativeChanges++);
                const combo = select.nextElementSibling.querySelector('[role=combobox]');
                if (!combo) throw new Error('No dropdown initialized: ' + select.outerHTML + ' jQuery: ' + jQuery.fn.jquery);
                jQuery(select).select2('open');
                const popup = dialog.querySelector('.select2-dropdown');
                const dropdownInsideDialog = !!popup && popup.getBoundingClientRect().height > 0;
                const rows = [...dialog.querySelectorAll('.select2-results__option')];
                const employee = rows.find(row => row.textContent === 'Employee');
                if (employee) employee.dispatchEvent(new MouseEvent('mouseup', {bubbles:true}));
                setTimeout(() => { document.getElementById('result').textContent = JSON.stringify({
                    multipleHeight:document.querySelector('#types + .select2-container .select2-selection').getBoundingClientRect().height,combo:!!combo,height:combo?.getBoundingClientRect().height,dropdownInsideDialog,
                    widthMatchesNative:combo?.getBoundingClientRect().width === initial,baseline:getComputedStyle(select.nextElementSibling).marginTop, selected:new FormData(document.getElementById('form')).get('role'),nativeChanges,
                    stillOpen:dialog.open,closedPopup:!dialog.querySelector('.select2-dropdown'),
                    region:new FormData(document.getElementById('form')).get('region'),requests
                }); }, 100);
                } catch (error) {document.getElementById('result').textContent=JSON.stringify({error:error.stack});}
            }, 100);</script>`;
        const file = path.join(directory, 'select.html');fs.writeFileSync(file, html);
        const dom = execFileSync(process.env.CHROMIUM_PATH || 'chromium', [
            '--headless','--no-sandbox','--disable-gpu','--disable-dev-shm-usage',
            '--virtual-time-budget=2000','--window-size=1440,1000','--dump-dom',file
        ], {encoding:'utf8',timeout:15000,maxBuffer:4*1024*1024,stdio:['ignore','pipe','ignore']});
        const result = JSON.parse(dom.match(/<pre id="result">(.*?)<\/pre>/s)[1]);
        assert.deepEqual(result, {multipleHeight:34,combo:true,height:34,dropdownInsideDialog:true,widthMatchesNative:true,baseline:'-2px',selected:'employee',nativeChanges:1,stillOpen:true,closedPopup:true,region:'employee-region',requests:['customer','employee']});
    } finally {
        fs.rmSync(directory, {recursive:true,force:true});
    }
});
