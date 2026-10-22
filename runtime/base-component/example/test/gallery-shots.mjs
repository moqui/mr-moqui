// Screenshots and text dumps of /qapps screens through Chrome DevTools, for checking the gallery (and any qvt
// screen) in the real shell. No npm dependencies (after runtime/component/MrSimpleScreens/test/browser-smoke.mjs).
//
//   node runtime/base-component/example/test/gallery-shots.mjs <outDir> [--dark|--light] [--width 1280] [--settle ms] [--assist] <path>...
//   node runtime/base-component/example/test/gallery-shots.mjs /tmp/shots --light /qapps/gallery/List /qapps/gallery/Detail
//
// Env: BASE_URL (default http://127.0.0.1:8080), USERNAME (john.doe), PASSWORD (moqui), CHROME (/usr/bin/google-chrome).
// Without --dark or --light the page keeps the signed-in user's dark-mode preference. A dialog opens on load with
// ?_openDialog=<dialog id> in the path. With --assist the Assist drawer is opened first (a path with ?assist=<conversationId>
// opens that conversation in it; the gate's smokes take the same shots with MOQUI_SHELL_SHOTS). Writes <outDir>/<name>.png (full page), <name>.txt (body innerText) and
// results.json (browser exceptions and console errors per page). The demo user and password are the dev seed; use a
// disposable database.
import { spawn } from 'node:child_process';
import { mkdtemp, writeFile, mkdir, rm } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';

const args = process.argv.slice(2);
const outDir = args.shift();
let dark = null, width = 1280, settle = 0, assistOpen = false;
const urls = [];
for (let i = 0; i < args.length; i++) {
  if (args[i] === '--dark') dark = true;
  else if (args[i] === '--light') dark = false;
  else if (args[i] === '--width') width = Number(args[++i]);
  else if (args[i] === '--settle') settle = Number(args[++i]);
  else if (args[i] === '--assist') assistOpen = true;
  else urls.push(args[i]);
}
if (!outDir || !urls.length) { console.error('usage: node shot.mjs <outDir> [--dark|--light] [--width N] <path>...'); process.exit(2); }
await mkdir(outDir, { recursive: true });
const base = process.env.BASE_URL || 'http://127.0.0.1:8080';
const username = process.env.USERNAME || 'john.doe';
const password = process.env.PASSWORD || 'moqui';
const profile = await mkdtemp(path.join(os.tmpdir(), 'shot-chrome-'));
const browser = spawn(process.env.CHROME || '/usr/bin/google-chrome', ['--headless=new', '--disable-gpu', '--no-first-run',
  '--no-default-browser-check', '--remote-debugging-port=0', `--user-data-dir=${profile}`, `--window-size=${width},900`, 'about:blank'],
  { stdio: ['ignore', 'ignore', 'pipe'] });
let ws;
try {
  const endpoint = await new Promise((resolve, reject) => {
    let text = '';
    const timer = setTimeout(() => reject(Error('Chrome startup timeout')), 30000);
    browser.stderr.on('data', (chunk) => { text += chunk; const m = text.match(/DevTools listening on (ws:\/\/[^\s]+)/); if (m) { clearTimeout(timer); resolve(m[1]); } });
    browser.on('exit', (code) => { clearTimeout(timer); reject(Error('Chrome exit ' + code)); });
  });
  ws = new WebSocket(endpoint);
  await new Promise((resolve, reject) => { ws.addEventListener('open', resolve, { once: true }); ws.addEventListener('error', reject, { once: true }); });
  let seq = 0; const pending = new Map(); const listeners = []; let session;
  let exceptions = []; let consoleErrors = [];
  ws.addEventListener('message', (event) => {
    const msg = JSON.parse(event.data);
    if (msg.id) { const cb = pending.get(msg.id); if (cb) { pending.delete(msg.id); msg.error ? cb.reject(Error(JSON.stringify(msg.error))) : cb.resolve(msg.result); } }
    else {
      if (msg.method === 'Runtime.exceptionThrown') exceptions.push(msg.params.exceptionDetails.text + ': ' + (msg.params.exceptionDetails.exception?.description || ''));
      if (msg.method === 'Runtime.consoleAPICalled' && (msg.params.type === 'error' || msg.params.type === 'warning'))
        consoleErrors.push(msg.params.type + ': ' + msg.params.args.map((a) => a.value ?? a.description ?? '').join(' ').slice(0, 500));
      for (const fn of [...listeners]) fn(msg);
    }
  });
  const send = (method, params = {}, sid = session) => new Promise((resolve, reject) => { const id = ++seq; pending.set(id, { resolve, reject }); ws.send(JSON.stringify({ id, method, params, ...(sid ? { sessionId: sid } : {}) })); });
  const wait = (method) => new Promise((resolve, reject) => {
    const timer = setTimeout(() => { listeners.splice(listeners.indexOf(fn), 1); reject(Error('Event timeout: ' + method)); }, 30000);
    const fn = (msg) => { if (msg.method === method) { clearTimeout(timer); listeners.splice(listeners.indexOf(fn), 1); resolve(msg.params); } };
    listeners.push(fn);
  });
  const { targetId } = await send('Target.createTarget', { url: 'about:blank' }, null);
  ({ sessionId: session } = await send('Target.attachToTarget', { targetId, flatten: true }, null));
  await send('Page.enable'); await send('Runtime.enable');
  await send('Emulation.setDeviceMetricsOverride', { width, height: 900, deviceScaleFactor: 1, mobile: width < 600 });
  const evaluate = async (expression) => { const out = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true }); if (out.exceptionDetails) throw Error(JSON.stringify(out.exceptionDetails)); return out.result.value; };
  const go = async (url) => { const loaded = wait('Page.loadEventFired'); await send('Page.navigate', { url: base + url }); await loaded; };
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  await go('/Login');
  let loaded = wait('Page.loadEventFired');
  await evaluate(`(()=>{const f=document.getElementById('login_form');f.username.value=${JSON.stringify(username)};f.password.value=${JSON.stringify(password)};f.requestSubmit();return true})()`);
  await loaded;
  const results = [];
  for (const url of urls) {
    exceptions = []; consoleErrors = [];
    await go(url);
    // the shell fetches the screen after load; wait until the page has content and nothing is loading, up to 8 s
    for (let i = 0; i < 40; i++) {
      await sleep(200);
      const state = await evaluate(`(()=>{const p=document.querySelector('.q-page');return {text:(p&&p.innerText||'').length, loading:!!document.querySelector('.q-loading-bar--visible, .q-inner-loading'), root:!!window.moqui&&!!window.moqui.webrootVue&&window.moqui.webrootVue.loading}})()`);
      if (state.text > 20 && !state.loading && !state.root && i > 3) break;
    }
    if (dark !== null) {
      const isDark = await evaluate(`document.body.classList.contains('body--dark')`);
      if (isDark !== dark) { await evaluate(`(()=>{const q=document.querySelector('#apps-root').__vue_app__;q.config.globalProperties.$q.dark.set(${dark});return true})()`); await sleep(400); }
    }
    if (assistOpen) { await evaluate(`(()=>{if(window.moqui&&moqui.assist&&moqui.assist.state.available)moqui.assist.openDrawer();return true})()`); await sleep(800); }
    if (settle > 0) await sleep(settle); // a page that loads a library lazily (charts, diagrams)
    const isDarkNow = await evaluate(`document.body.classList.contains('body--dark')`);
    const name = url.replace(/^\/qapps\//, '').replace(/[^A-Za-z0-9]+/g, '_') + (isDarkNow ? '_dark' : '_light') + (width < 1024 ? '_' + width : '') + (assistOpen ? '_assist' : '');
    const text = await evaluate('document.body.innerText');
    await writeFile(path.join(outDir, name + '.txt'), text);
    const shot = await send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: true });
    await writeFile(path.join(outDir, name + '.png'), Buffer.from(shot.data, 'base64'));
    const renderError = /Error rendering|Exception in render|is not written yet/.test(text);
    results.push({ url, name, chars: text.length, renderError, exceptions: [...exceptions], consoleErrors: [...consoleErrors] });
    console.log(`${url}: ${text.length} chars${renderError ? ' RENDER-ERROR-TEXT' : ''}${exceptions.length ? ' ' + exceptions.length + ' exceptions' : ''}${consoleErrors.length ? ' ' + consoleErrors.length + ' console errors' : ''}`);
  }
  await writeFile(path.join(outDir, 'results.json'), JSON.stringify(results, null, 2));
} finally {
  try { ws && ws.close(); } catch {}
  browser.kill();
  await rm(profile, { recursive: true, force: true });
}
