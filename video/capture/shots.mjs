// Records every demo scene from the live lab (tests/demo-video.sh) and the
// real terminal outputs used by the terminal scenes.
// Usage: node capture/shots.mjs [scene ...]   (no args = all)
import fs from 'node:fs'
import path from 'node:path'
import { ROOT, PANEL, api, click, glide, lab, labBg, openPanel, record, scroll, sleep, waitFor } from './rec.mjs'

// tests/ as seen from the Linux side
const T = lab(`wslpath -u '${path.resolve(ROOT, '..', 'tests').replaceAll('\\', '/')}'`)
const BIN = process.env.SPILLWAY_BIN ?? `${T}/../target/release/spillway`
const NODE_CMD = `${BIN} node --id node-b --master http://127.0.0.1:7070 --token lab-token --listen 127.0.0.1:7071 --advertise http://127.0.0.1:7071`
const termFile = path.join(ROOT, 'public', 'term.json')
const term = fs.existsSync(termFile) ? JSON.parse(fs.readFileSync(termFile, 'utf8')) : {}
const saveTerm = () => fs.writeFileSync(termFile, JSON.stringify(term, null, 2))

const ov = () => api('/api/overview')
const dst = async (id) => (await ov()).dests.find((d) => d.id === id)
const setAll = async (body) => {
  for (const d of (await ov()).dests) await api(`/api/destinations/${d.id}`, 'PATCH', body)
}
// Keeps the loop disks from filling up between scenes (the lab disks are small).
const prune = () => lab(`for d in /root/spw/d1 /root/spw/d2 /root/spw/d3; do rm -rf $d/ingest-4k; find $d -name '*.mp4' -mmin +1 -delete; done; true`, { quiet: true })
const allHealthy = () =>
  waitFor(async () => (await ov()).dests.every((d) => !d.enabled || d.status === 'healthy'), 30000, 'all destinations healthy')
const nav = async (page, hash) => {
  await page.goto(`${PANEL}/#${hash}`)
  await page.waitForSelector('text=ao vivo')
  await sleep(1200)
}

const scenes = {
  // Burst of 2000 complete MP4 segments on top of the 60-camera traffic.
  async overview(page) {
    await nav(page, 'overview')
    await glide(page, [1300, 700], 10)
    const rec = await record(page, 'overview')
    await sleep(1500)
    labBg(`python3 ${T}/demotraffic.py --burst 3000 --threads 24 --seconds 8 > /root/spw/burst.out`)
    await glide(page, 'text=Entrada', 900)
    await sleep(1600)
    await glide(page, 'text=Escrevendo', 900)
    await sleep(1200)
    await glide(page, 'text=Na fila', 900)
    await sleep(1200)
    await glide(page, 'text=Nos discos', 900)
    await sleep(2600)
    await glide(page, 'text=Saída para discos', 900)
    await sleep(2500)
    await rec.stop()
    await waitFor(async () => lab('cat /root/spw/burst.out', { quiet: true }).includes('burst'), 30000, 'burst')
    term.burst = lab('cat /root/spw/burst.out')
  },

  // A disk turns read-only under traffic, traffic fails over, the disk recovers.
  async failover(page) {
    await nav(page, 'dests')
    await glide(page, [1250, 760], 10)
    const rec = await record(page, 'failover')
    await sleep(1800)
    lab('for i in $(seq 1 20); do mount -o remount,ro /root/spw/d2 && break; sleep 0.2; done')
    await glide(page, 'text=disk2', 1000)
    await waitFor(async () => (await dst('disk2')).status !== 'healthy', 15000, 'd2 failing')
    await sleep(1500)
    await glide(page, [480, 520], 1100)
    await sleep(2200)
    lab('for i in $(seq 1 20); do mount -o remount,rw /root/spw/d2 && break; sleep 0.2; done')
    await glide(page, [1660, 213], 1000)
    await waitFor(async () => (await dst('disk2')).status === 'healthy', 20000, 'd2 back')
    await sleep(2500)
    await rec.stop()
  },

  // A second server joins the cluster: the node appears and its disk starts receiving.
  async cluster(page) {
    lab(`pkill -9 -f '${BIN} node' ; true`)
    await waitFor(async () => (await dst('disk3')).status === 'offline', 20000, 'node offline')
    await api('/api/nodes/node-b', 'DELETE')
    await nav(page, 'cluster')
    await glide(page, [1100, 560], 10)
    const rec = await record(page, 'cluster')
    await sleep(1500)
    await glide(page, 'text=spillway node', 900)
    await sleep(900)
    labBg(NODE_CMD, '/root/spw/node.log')
    await page.waitForSelector('text=node-b', { timeout: 15000 })
    await sleep(500)
    await glide(page, 'text=node-b', 900)
    await sleep(1800)
    await click(page, 'nav >> text=Destinos', 900)
    await sleep(800)
    await glide(page, 'text=disk3', 900)
    await waitFor(async () => (await dst('disk3')).status === 'healthy', 15000, 'nfs healthy')
    await sleep(3000)
    await rec.stop()
  },

  // Hooks section: decisions, refusals and the durable outbox.
  async hooks(page) {
    await nav(page, 'overview')
    await scroll(page, 330, 10)
    await glide(page, [800, 330], 10)
    const rec = await record(page, 'hooks')
    await sleep(1000)
    await glide(page, 'text=Recusados', 1000)
    await sleep(1800)
    await glide(page, 'text=Latência média', 900)
    await sleep(1500)
    await glide(page, 'text=Entregues', 1000)
    await sleep(2500)
    await rec.stop()
  },

  // Files: where each file lives right now, and why the hook refused some.
  async files(page) {
    // start when a refusal is near the top of the recent list
    await waitFor(async () => (await api('/api/recent')).slice(0, 4).some((r) => r.outcome === 'dropped'), 30000, 'recent refusal')
    await nav(page, 'files')
    await glide(page, [800, 420], 10)
    const rec = await record(page, 'files')
    await sleep(1200)
    await glide(page, 'text=Onde está', 900)
    await sleep(1400)
    await scroll(page, 560, 1400)
    await sleep(300)
    const refused = page.locator('text=Recusado pelo hook').first()
    if (await refused.count()) await glide(page, 'text=Recusado pelo hook', 1000)
    await sleep(3000)
    await rec.stop()
  },

  // Parse stats and groups by the labels the hook returned.
  async rec(page) {
    await nav(page, 'rec')
    await glide(page, [900, 500], 10)
    const rec = await record(page, 'rec')
    await sleep(1000)
    await glide(page, 'text=Fragmentados (fMP4)', 900)
    await sleep(900)
    await glide(page, 'text=Codecs e resoluções', 900)
    await sleep(1000)
    await click(page, 'button:has-text("camera (")', 900)
    await sleep(700)
    await click(page, '[role="option"]:has-text("empresa")', 700)
    await sleep(1200)
    await glide(page, [700, 760], 900)
    await sleep(2500)
    await rec.stop()
  },

  // Raise the RAM ceiling live: the tmpfs is resized on the spot.
  async settings(page) {
    term.ram_before = lab('df -h /root/spw/ram | tail -1')
    await nav(page, 'settings')
    const thumb = page.locator('[data-slot="slider-thumb"]').first()
    const box = await thumb.boundingBox()
    await glide(page, [box.x - 120, box.y + 140], 10)
    const rec = await record(page, 'settings')
    await sleep(900)
    await glide(page, [box.x + box.width / 2, box.y + box.height / 2], 900)
    await page.mouse.down()
    const track = await page.locator('[data-slot="slider-track"]').first().boundingBox()
    const input = thumb.locator('input')
    const min = Number(await input.getAttribute('min')) || 0.5
    const max = Number(await input.getAttribute('max')) || 16
    const targetX = track.x + (track.width * (4 - min)) / (max - min)
    const sx = box.x + box.width / 2
    for (let i = 1; i <= 40; i++) {
      const t = i / 40
      await page.mouse.move(sx + (targetX - sx) * (1 - Math.pow(1 - t, 3)), box.y + box.height / 2)
      await sleep(22)
    }
    await page.mouse.up()
    await sleep(1400)
    await click(page, 'button:has-text("Salvar")', 900)
    await sleep(2600)
    await rec.stop()
    term.ram_after = lab('df -h /root/spw/ram | tail -1')
    await api('/api/settings', 'PUT', { ram: { thief_max_gb: 1.0 } })
  },

  // Every destination off: RAM fills up to the ceiling and the thief discards the oldest.
  async thief(page) {
    await nav(page, 'overview')
    await glide(page, [1250, 640], 10)
    const rec = await record(page, 'thief')
    await sleep(1000)
    await setAll({ enabled: false })
    await glide(page, 'text=Destinos gravando', 900)
    labBg(`mkdir -p /root/spw/videos/ingest-4k/2026-10-22; for i in $(seq -w 1 40); do cp /root/spw/demobig/big.mp4 /root/spw/videos/ingest-4k/2026-10-22/cam4k-$i.mp4; sleep 0.18; done; echo done > /root/spw/fill.out`)
    await sleep(1200)
    await glide(page, 'text=RAM em uso', 900)
    await waitFor(async () => (await ov()).ram.used_bytes > 0.95 * (await ov()).ram.thief_max_bytes, 30000, 'ram at ceiling')
    await glide(page, 'text=Thief', 900)
    await waitFor(async () => lab('cat /root/spw/fill.out 2>/dev/null', { quiet: true }).includes('done'), 30000, 'fill done')
    await sleep(2200)
    await setAll({ enabled: true })
    await glide(page, 'text=Saída para discos', 1000)
    await waitFor(async () => (await ov()).ram.used_bytes < 120 * 2 ** 20, 30000, 'ram drained')
    await sleep(1500)
    await rec.stop()
    lab('rm -f /root/spw/fill.out')
  },

  // Clean hero shot of the overview to close on.
  async hero(page) {
    await nav(page, 'overview')
    await glide(page, [1560, 860], 10)
    const rec = await record(page, 'hero')
    await sleep(7000)
    await rec.stop()
  },
}

async function collectTerminal() {
  prune()
  const moved = (await api('/api/recent')).find((r) => r.outcome === 'moved' && !r.path.startsWith('ingest'))
  const dir = moved.path.split('/').slice(0, -1).join('/')
  term.ls_path = `/videos/${dir}`
  term.ls = lab(`ls -l /root/spw/videos/${dir} | grep -v drop- | grep mp4 | tail -4 | awk '{print $5, $9}'`)
  term.inspect_file = moved.path
  term.inspect = lab(`${BIN} inspect /root/spw/videos/${moved.path}`).replaceAll('Âµ', 'µ')
  term.mount = lab(`mount | grep -E ' /root/spw/(videos|ram) ' | sed -E 's/\\(.*//'`)
  term.df = lab('df -h /root/spw/videos | cat')
  term.version = lab(`${BIN} --version`, { quiet: true })
  saveTerm()
}

const want = process.argv.slice(2)
const { browser, page } = await openPanel()
try {
  for (const [name, fn] of Object.entries(scenes)) {
    if (want.length && !want.includes(name)) continue
    console.log(`scene ${name}`)
    prune()
    await allHealthy()
    await fn(page)
    saveTerm()
    await sleep(1500)
  }
  if (!want.length || want.includes('terminal')) await collectTerminal()
} finally {
  await setAll({ enabled: true }).catch(() => {})
  lab('mount -o remount,rw /root/spw/d2 2>/dev/null; true', { quiet: true })
  await browser.close()
}
