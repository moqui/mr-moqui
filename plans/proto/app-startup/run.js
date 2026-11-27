const { chromium } = require('playwright')
// Each variant: 3 visits to fill its caches, then N cold starts (a new browser process on the same profile) each
// followed by a reload. The time to content is the paint of the first list row (Element Timing).
const N = 10, PORT = process.argv[2], EXE = process.env.CHROME || undefined
const q = (a, f) => { const s = [...a].sort((x, y) => x - y); return s[Math.min(s.length - 1, Math.floor(s.length * f))] }
const launch = dir => chromium.launchPersistentContext(dir, { headless: true, executablePath: EXE })
async function page(ctx, cpu) { const p = ctx.pages()[0] || await ctx.newPage(); const c = await ctx.newCDPSession(p); await c.send('Emulation.setCPUThrottlingRate', { rate: cpu }); return p }
async function measure(p, url, reload) { try { return await measure0(p, url, reload) } catch (e) { console.error('FAIL', url, reload ? 'reload' : 'cold', e.message.split('\n')[0]); return null } }
async function measure0(p, url, reload) {
  if (reload) await p.reload({ waitUntil: 'commit' }); else await p.goto(url, { waitUntil: 'commit' })
  await p.waitForFunction(() => performance.getEntriesByName('fresh').length, null, { timeout: 15000 })
  await p.waitForTimeout(50)
  return p.evaluate(() => new Promise(ok => {
    new PerformanceObserver(l => {
      const rows = l.getEntries().filter(e => e.identifier === 'row')
      const fcp = performance.getEntriesByName('first-contentful-paint')[0]?.startTime
      ok({ content: Math.round(Math.min(...rows.map(e => e.renderTime || e.loadTime))), fcp: Math.round(fcp ?? -1),
        fresh: Math.round(performance.getEntriesByName('fresh')[0].startTime) })
    }).observe({ type: 'element', buffered: true })
  }))
}
;(async () => {
  const res = []
  for (const cpu of [1, 4]) for (const v of (process.env.VARIANTS || 'a,b,d,e,f').split(',')) {
    const url = `http://127.0.0.1:${PORT}/${v}/`, dir = `${process.env.PROFILES}/profile-${v}-${cpu}`
    let ctx = await launch(dir), p = await page(ctx, cpu)
    for (let i = 0; i < 3; i++) { await measure(p, url, i > 0); await p.waitForTimeout(600) }  // prime caches
    await ctx.close()
    const cold = [], warm = []
    for (let i = 0; i < N; i++) {
      ctx = await launch(dir); p = await page(ctx, cpu)
      cold.push(await measure(p, url, false)); await p.waitForTimeout(600)
      warm.push(await measure(p, url, true)); await p.waitForTimeout(600)
      await ctx.close()
    }
    cold.splice(0, cold.length, ...cold.filter(Boolean)); warm.splice(0, warm.length, ...warm.filter(Boolean))
    const f = (rs, k) => `${q(rs.map(r => r[k]), 0.5)} / ${q(rs.map(r => r[k]), 0.9)}`
    res.push({ cpu, v, n: cold.length + '/' + warm.length, coldContent: f(cold, 'content'), coldFcp: f(cold, 'fcp'), reloadContent: f(warm, 'content'), coldFresh: f(cold, 'fresh') })
  }
  console.table(res)
})().catch(e => { console.error(e); process.exit(1) })
