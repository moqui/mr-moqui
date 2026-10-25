/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const { execFileSync } = require('node:child_process');

test('Java order-by widget filters chosen directions and submits changes inside a modal', () => {
    const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'moqui-order-by-'));
    const web = path.join(__dirname, '..');
    const read = relative => fs.readFileSync(path.join(web, relative), 'utf8').replace(/<\/script/gi, '<\\/script');
    try {
        const html = `<!doctype html><html><style>${read('css/moqui-html.css')}${read('libs/selectivity/selectivity-full.min.css')}</style>
        <button data-dialog="find" id="open">Find</button><dialog id="find"><header>Find</header><div class="body"><form id="form">
        <div class="field"><label>Order By</label><div class="widget"><select name="orderBySelect" multiple id="order" style="width:100%" data-orderby="value">
        <option value="^name">Name (Asc)</option><option value="-^name">Name (Desc)</option><option value="amount">Amount (Asc)</option>
        </select><input type="hidden" id="value" name="orderByField" value="^name"></div></div>
        <input id="actual" name="query"></form></div></dialog><pre id="result"></pre>
        <script>${read('libs/jquery/jquery.min.js')}</script><script>${read('libs/selectivity/selectivity-full.min.js')}</script>
        <script>const errors=[];console.error=(...args)=>errors.push(args.map(a=>a?.stack || String(a)).join(' '));</script><script>${read('js/moqui-html.js')}</script><script>
        document.getElementById('open').click();
        setTimeout(()=>{try {
            const control=document.getElementById('order'), instance=control.selectivity;
            const initialFocus=document.activeElement.id;
            const before=instance.value().slice(),inputHeight=control.querySelector('.selectivity-multiple-input').getBoundingClientRect().height;
            instance.open();
            const popup=document.querySelector('.selectivity-dropdown');
            const popupInside=!!popup.closest('dialog'),width=popup.getBoundingClientRect().width;
            const rows=[...popup.querySelectorAll('.selectivity-result-item')].map(e=>e.textContent);
            const amount=[...popup.querySelectorAll('.selectivity-result-item')].find(e=>e.textContent==='Amount (Asc)');
            amount.click();
            const added=new FormData(document.getElementById('form')).get('orderByField');
            instance.remove('^name');
            document.getElementById('result').textContent=JSON.stringify({before,inputHeight,initialFocus,rows,added,errors,
                removed:new FormData(document.getElementById('form')).get('orderByField'),
                popupInside,width,dialogOpen:document.getElementById('find').open});
        } catch(error) {document.getElementById('result').textContent=JSON.stringify({error:error.stack,errors});}},100);
        </script>`;
        const file=path.join(directory,'order.html');fs.writeFileSync(file,html);
        const dom=execFileSync(process.env.CHROMIUM_PATH || 'chromium',[
            '--headless','--no-sandbox','--disable-gpu','--disable-dev-shm-usage','--virtual-time-budget=2000',
            '--window-size=1440,1000','--dump-dom',file
        ],{encoding:'utf8',timeout:15000,maxBuffer:4*1024*1024,stdio:['ignore','pipe','ignore']});
        const result=JSON.parse(dom.match(/<pre id="result">(.*?)<\/pre>/s)[1]);
        assert.deepEqual(result,{before:['^name'],inputHeight:32,initialFocus:'actual',rows:['Name (Desc)','Amount (Asc)'],added:'^name,amount',removed:'amount',errors:[],popupInside:true,width:300,dialogOpen:true});
    } finally {fs.rmSync(directory,{recursive:true,force:true});}
});
