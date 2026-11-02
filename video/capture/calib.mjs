import { openPanel, sleep, PANEL } from './rec.mjs'
const { browser, page } = await openPanel()
for (const t of ['dests', 'cluster', 'rec', 'files', 'settings']) {
  await page.goto(`${PANEL}/#${t}`)
  await sleep(1800)
  await page.screenshot({ path: `public/clips/calib-${t}.png`, fullPage: true })
}
await browser.close()
