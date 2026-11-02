// Headless capture of the live panel for the demo video.
// No window ever opens: Remotion's chrome-headless-shell drives the page and
// frames come from the CDP screencast (JPEG q95, real paint timestamps), then
// ffmpeg turns them into a constant 60 fps clip.
import { chromium } from 'playwright-core'
import { spawnSync } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'

export const ROOT = path.resolve(import.meta.dirname, '..')
export const CLIPS = path.join(ROOT, 'public', 'clips')
const SHELL = path.join(ROOT, 'node_modules/.remotion/chrome-headless-shell/win64/chrome-headless-shell-win64/chrome-headless-shell.exe')
export const PANEL = 'http://localhost:7070'

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

/** Runs a bash command on the Linux side (WSL, as root, like the lab). */
export function lab(cmd, { quiet = false } = {}) {
  // the script goes through stdin: Windows argv quoting would mangle it
  const r = spawnSync('wsl', ['-u', 'root', '--', 'bash', '-l', '-s'], { input: cmd + '\n', encoding: 'utf8', maxBuffer: 64 << 20 })
  if (!quiet && r.status !== 0) console.error(`[lab] exit ${r.status}: ${cmd}\n${r.stderr}`)
  return (r.stdout || '').trimEnd()
}

/** Starts a long-running lab process detached from this script. */
export function labBg(cmd, log = '/dev/null') {
  // the child must detach before wsl.exe returns, or WSL tears it down
  lab(`setsid -f nohup bash -c '${cmd.replaceAll("'", `'\\''`)}' > ${log} 2>&1 < /dev/null; sleep 0.3`)
}

export async function api(p, method = 'GET', body) {
  const r = await fetch(PANEL + p, { method, headers: body ? { 'content-type': 'application/json' } : {}, body: body ? JSON.stringify(body) : undefined })
  const t = await r.text()
  return t ? JSON.parse(t) : null
}

export async function waitFor(fn, ms = 20000, what = 'condition') {
  const t0 = Date.now()
  while (Date.now() - t0 < ms) {
    if (await fn()) return
    await sleep(100)
  }
  throw new Error(`timeout waiting for ${what}`)
}

// A visible cursor (headless has none) with a soft click ripple.
const CURSOR = `
(() => {
  const css = document.createElement('style');
  css.textContent = \`
    #spw-cur{position:fixed;left:0;top:0;width:26px;height:26px;z-index:2147483647;pointer-events:none;
      transform:translate(-3px,-2px);filter:drop-shadow(0 2px 4px rgba(0,0,0,.45));transition:opacity .2s}
    .spw-rip{position:fixed;z-index:2147483646;pointer-events:none;width:12px;height:12px;margin:-6px 0 0 -6px;border-radius:50%;
      border:2px solid #5eead4;animation:spwrip .55s ease-out forwards}
    @keyframes spwrip{to{transform:scale(4.2);opacity:0}}\`;
  const cur = document.createElement('div');
  cur.id = 'spw-cur';
  cur.innerHTML = '<svg viewBox="0 0 24 24" width="26" height="26"><path d="M4 2.5l15.5 9.2-6.9 1.6 3.9 7.3-3 1.5-3.8-7.4L4.6 19z" fill="#fff" stroke="#0b1220" stroke-width="1.4" stroke-linejoin="round"/></svg>';
  const mount = () => { if (!document.body) return requestAnimationFrame(mount); document.head.appendChild(css); document.body.appendChild(cur); };
  mount();
  const pos = JSON.parse(sessionStorage.getItem('spw-cur') || '[960,600]');
  cur.style.left = pos[0] + 'px'; cur.style.top = pos[1] + 'px';
  addEventListener('mousemove', (e) => { cur.style.left = e.clientX + 'px'; cur.style.top = e.clientY + 'px';
    sessionStorage.setItem('spw-cur', JSON.stringify([e.clientX, e.clientY])); }, true);
  addEventListener('mousedown', (e) => { const r = document.createElement('div'); r.className = 'spw-rip';
    r.style.left = e.clientX + 'px'; r.style.top = e.clientY + 'px'; document.body.appendChild(r); setTimeout(() => r.remove(), 700); }, true);
})();`

export async function openPanel() {
  const browser = await chromium.launch({ executablePath: SHELL, headless: true, args: ['--hide-scrollbars', '--force-color-profile=srgb'] })
  // 1600x900 CSS px at 1.2x = 1920x1080 frames with crisp text
  const ctx = await browser.newContext({ viewport: { width: 1600, height: 900 }, deviceScaleFactor: 1.2, colorScheme: 'dark' })
  await ctx.addInitScript(() => {
    try {
      localStorage.setItem('spw-theme', 'dark')
      localStorage.setItem('spw-range', '15m')
    } catch {}
  })
  await ctx.addInitScript(CURSOR)
  const page = await ctx.newPage()
  page.on('pageerror', (e) => console.error('[page]', e.message))
  await page.goto(PANEL + '/#overview')
  await page.waitForSelector('text=ao vivo', { timeout: 20000 })
  return { browser, ctx, page }
}

/** Moves the drawn cursor smoothly (ease-in-out) to an element or point. */
export async function glide(page, target, ms = 700) {
  let x, y
  if (typeof target === 'string') {
    const box = await page.locator(target).first().boundingBox()
    if (!box) throw new Error(`no box for ${target}`)
    x = box.x + box.width / 2
    y = box.y + box.height / 2
  } else [x, y] = target
  const from = await page.evaluate(() => JSON.parse(sessionStorage.getItem('spw-cur') || '[960,600]'))
  const steps = Math.max(8, Math.round(ms / 16))
  for (let i = 1; i <= steps; i++) {
    const t = i / steps
    const e = t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2
    await page.mouse.move(from[0] + (x - from[0]) * e, from[1] + (y - from[1]) * e)
    await sleep(ms / steps)
  }
  return [x, y]
}

export async function click(page, target, ms = 700) {
  const [x, y] = await glide(page, target, ms)
  await sleep(120)
  await page.mouse.down()
  await sleep(70)
  await page.mouse.up()
  return [x, y]
}

/** Smooth page scroll, in CSS px. */
export async function scroll(page, dy, ms = 900) {
  await page.evaluate(
    ([dy, ms]) =>
      new Promise((done) => {
        const y0 = scrollY
        const t0 = performance.now()
        const step = (now) => {
          const t = Math.min(1, (now - t0) / ms)
          const e = t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2
          scrollTo(0, y0 + dy * e)
          t < 1 ? requestAnimationFrame(step) : done()
        }
        requestAnimationFrame(step)
      }),
    [dy, ms],
  )
}

/**
 * Records the page until stop() is called. Returns {stop} -> Promise<seconds>.
 * Screencast frames only arrive when something paints, so each frame is held
 * until the next one (ffconcat durations) and the clip is resampled to 60 fps.
 */
export async function record(page, name) {
  fs.mkdirSync(CLIPS, { recursive: true })
  const dir = path.join(CLIPS, `.${name}-frames`)
  fs.rmSync(dir, { recursive: true, force: true })
  fs.mkdirSync(dir, { recursive: true })
  const cdp = await page.context().newCDPSession(page)
  const frames = []
  cdp.on('Page.screencastFrame', async (f) => {
    const file = path.join(dir, `${String(frames.length).padStart(6, '0')}.jpg`)
    fs.writeFileSync(file, Buffer.from(f.data, 'base64'))
    frames.push({ file, t: f.metadata.timestamp })
    cdp.send('Page.screencastFrameAck', { sessionId: f.sessionId }).catch(() => {})
  })
  await cdp.send('Page.startScreencast', { format: 'jpeg', quality: 95, maxWidth: 1920, maxHeight: 1080, everyNthFrame: 1 })
  // force a first paint so the clip starts immediately
  await page.evaluate(() => document.body.style.setProperty('--spw-tick', String(Math.random())))
  const started = Date.now() / 1000
  return {
    async stop() {
      const ended = Date.now() / 1000
      await cdp.send('Page.stopScreencast').catch(() => {})
      await sleep(150)
      if (!frames.length) throw new Error(`no frames for ${name}`)
      const lines = ['ffconcat version 1.0']
      for (let i = 0; i < frames.length; i++) {
        const next = i + 1 < frames.length ? frames[i + 1].t : Math.max(ended, frames[i].t + 1 / 60)
        const start = i === 0 ? Math.min(frames[0].t, started) : frames[i].t
        lines.push(`file '${path.basename(frames[i].file)}'`, `duration ${Math.max(1 / 600, next - start).toFixed(4)}`)
      }
      lines.push(`file '${path.basename(frames.at(-1).file)}'`)
      fs.writeFileSync(path.join(dir, 'list.ffconcat'), lines.join('\n'))
      const out = path.join(CLIPS, `${name}.mp4`)
      const r = spawnSync(
        'ffmpeg',
        ['-y', '-loglevel', 'error', '-f', 'concat', '-safe', '0', '-i', 'list.ffconcat', '-vf', 'fps=60,scale=1920:1080:flags=lanczos,format=yuv420p', '-c:v', 'libx264', '-preset', 'slow', '-crf', '14', '-movflags', '+faststart', out],
        { cwd: dir, encoding: 'utf8' },
      )
      if (r.status !== 0) throw new Error(`ffmpeg ${name}: ${r.stderr}`)
      fs.rmSync(dir, { recursive: true, force: true })
      const secs = ended - Math.min(frames[0].t, started)
      console.log(`clip ${name}: ${frames.length} frames, ${secs.toFixed(1)} s`)
      return secs
    },
  }
}
