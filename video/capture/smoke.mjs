import { openPanel, record, glide, sleep } from './rec.mjs'
const { browser, page } = await openPanel()
await sleep(1500)
const rec = await record(page, 'smoke')
await glide(page, 'text=Ciclo de vida dos arquivos', 1200)
await sleep(2500)
await rec.stop()
await page.screenshot({ path: 'public/clips/smoke.png' })
await browser.close()
