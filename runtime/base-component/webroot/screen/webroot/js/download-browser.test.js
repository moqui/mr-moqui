/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs'), path = require('node:path'), os = require('node:os');
const {execFileSync} = require('node:child_process');

test('inline chart CSV export preserves payload, filename and MIME type and cleans its object URL', () => {
    const directory = fs.mkdtempSync(path.join(os.tmpdir(),'moqui-download-'));
    try {
        const script=fs.readFileSync(path.join(__dirname,'moqui-html.js'),'utf8');
        const file=path.join(directory,'download.html');
        fs.writeFileSync(file,`<!doctype html><html><button id="csv" onclick="moqui.downloadData('Name,Value\\nCafé,2\\n','report.csv','text/csv')">Get CSV</button><pre id="result"></pre>
          <script>const state={};let blob;
          URL.createObjectURL=value=>{blob=value;return 'blob:fixture';};
          URL.revokeObjectURL=value=>state.revoked=value;
          HTMLAnchorElement.prototype.click=function(){state.filename=this.download;state.url=this.href;state.attached=this.isConnected;};
          window.addEventListener('error',event=>state.error=event.message);</script>
          <script>${script}</script><script>document.getElementById('csv').click();
          setTimeout(async()=>{if(blob){state.type=blob.type;state.text=await blob.text();}state.links=document.querySelectorAll('a[download]').length;document.getElementById('result').textContent=JSON.stringify(state);},100);</script>`);
        const dom=execFileSync(process.env.CHROMIUM_PATH || 'chromium',['--headless','--no-sandbox','--disable-gpu','--disable-dev-shm-usage','--virtual-time-budget=2000','--dump-dom',file],{encoding:'utf8',timeout:15000,maxBuffer:8*1024*1024,stdio:['ignore','pipe','ignore']});
        const result=JSON.parse(dom.match(/<pre id="result">(.*?)<\/pre>/s)[1]);
        assert.deepEqual(result,{filename:'report.csv',url:'blob:fixture',attached:true,revoked:'blob:fixture',type:'text/csv',text:'Name,Value\nCafé,2\n',links:0});
    } finally {fs.rmSync(directory,{recursive:true,force:true});}
});
