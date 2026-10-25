/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs'), path = require('node:path'), os = require('node:os');
const {execFileSync} = require('node:child_process');

test('Java calendar selects a day, submits its formatted value and signals native changes', () => {
    const directory = fs.mkdtempSync(path.join(os.tmpdir(),'moqui-calendar-'));
    const web = path.join(__dirname,'..');
    const read = name => fs.readFileSync(path.join(web,name),'utf8').replace(/<\/script/gi,'<\\/script');
    try {
        const html = `<!doctype html><html lang="en" data-theme="light"><style>${read('css/moqui-html.css')}
          ${read('libs/datetimepicker/bootstrap-scoped.css')}${read('libs/datetimepicker/bootstrap-datetimepicker.min.css')}</style>
          <form id="form"><div class="field"><label>Birth Date</label><div class="widget">
          <div class="input-group date" id="birth" data-date-picker="YYYY-MM-DD" data-date-kind="date"><input name="birthDate" value="2026-10-25" size="10"><span class="input-group-addon"><span class="fa fa-calendar"></span></span></div>
          </div></div><div class="input-group date" id="stamp" data-date-picker="YYYY-MM-DD HH:mm" data-date-kind="date-time" data-date-step="5"><input name="timestamp" value="2026-10-25 10:15"><span class="input-group-addon"></span></div></form><pre id="result"></pre>
          <script>${read('libs/jquery/jquery.min.js')}</script><script>${read('libs/datetimepicker/moment-with-locales.min.js')}</script>
          <script>${read('libs/datetimepicker/bootstrap-datetimepicker.js')}</script><script>${read('js/moqui-html.js')}</script>
          <script>setTimeout(()=>{try{
            const node=document.getElementById('birth'), input=node.querySelector('input');let changes=0;
            node.addEventListener('change',()=>changes++);
            const addon=node.querySelector('.input-group-addon'), before=addon.getBoundingClientRect();
            addon.click();
            const popup=node.querySelector('.bootstrap-datetimepicker-widget');
            const visible=!!popup && popup.getBoundingClientRect().height>0;
            if(getComputedStyle(popup).position!=="absolute")throw new Error("Calendar must float: "+getComputedStyle(popup).position);
            const after=addon.getBoundingClientRect();
            if(before.x!==after.x||before.y!==after.y)throw new Error("Calendar moved the click target");
            const day=popup && [...popup.querySelectorAll('td.day')].find(e=>e.textContent==='26'&&!e.classList.contains('old')&&!e.classList.contains('new'));
            if(!day)throw new Error('No visible calendar day');
            day.click();
            const closed=!node.querySelector('.bootstrap-datetimepicker-widget');
            const stamp=document.getElementById('stamp');stamp.querySelector('.input-group-addon').click();
            stamp.querySelector('[data-action=togglePicker]').click();
            const timeVisible=stamp.querySelector('.timepicker').getBoundingClientRect().height>0;
            stamp.querySelector('[data-action=incrementMinutes]').click();
            document.getElementById('result').textContent=JSON.stringify({visible,value:new FormData(document.getElementById('form')).get('birthDate'),changes,closed,timeVisible,timestamp:new FormData(document.getElementById('form')).get('timestamp')});
          }catch(error){document.getElementById('result').textContent=JSON.stringify({error:error.stack});}},100);</script>`;
        const file=path.join(directory,'calendar.html');fs.writeFileSync(file,html);
        const dom=execFileSync(process.env.CHROMIUM_PATH || 'chromium',['--headless','--no-sandbox','--disable-gpu','--disable-dev-shm-usage','--virtual-time-budget=2000','--window-size=1440,1000','--dump-dom',file],{encoding:'utf8',timeout:15000,maxBuffer:8*1024*1024,stdio:['ignore','pipe','ignore']});
        const result=JSON.parse(dom.match(/<pre id="result">(.*?)<\/pre>/s)[1]);
        assert.deepEqual(result,{visible:true,value:'2026-10-26',changes:1,closed:true,timeVisible:true,timestamp:'2026-10-25 10:20'});
    } finally {fs.rmSync(directory,{recursive:true,force:true});}
});
