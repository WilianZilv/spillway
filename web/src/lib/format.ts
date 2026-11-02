export const KiB = 1024
export const MiB = KiB * 1024
export const GiB = MiB * 1024
export const TiB = GiB * 1024

const dec = (v: number, d: number) => v.toFixed(d).replace('.', ',')

export function bytes(b: number, d?: number): string {
  b = +b || 0
  if (b >= TiB) return `${dec(b / TiB, d ?? 2)} TiB`
  if (b >= GiB) return `${dec(b / GiB, d ?? 2)} GiB`
  if (b >= MiB) return `${dec(b / MiB, d ?? 1)} MiB`
  if (b >= KiB) return `${dec(b / KiB, d ?? 0)} KiB`
  return `${b.toFixed(0)} B`
}

/** Splits "1,23 GiB" into ["1,23", "GiB"] for big-number tiles. */
export function bytesParts(b: number): [string, string] {
  const s = bytes(b)
  const i = s.lastIndexOf(' ')
  return [s.slice(0, i), s.slice(i + 1)]
}

export const rate = (bps: number) => `${bytes(bps, bps >= GiB ? 2 : 1)}/s`
/** Compact axis ticks: "750 MiB/s", "1,5 GiB/s". */
export const rateAxis = (bps: number) => `${bytes(bps, bps >= GiB ? 1 : 0)}/s`
export const bytesAxis = (b: number) => bytes(b, b >= GiB ? 1 : 0)
export const num = (n: number) => (+n || 0).toLocaleString('pt-BR')

export function compact(n: number): string {
  n = +n || 0
  if (n >= 1e9) return `${dec(n / 1e9, 1)} bi`
  if (n >= 1e6) return `${dec(n / 1e6, 1)} mi`
  if (n >= 1e4) return `${dec(n / 1e3, 1)} mil`
  return num(n)
}

export function dur(s: number): string {
  s = Math.max(0, +s || 0)
  if (s < 60) return `${dec(s, s < 10 ? 1 : 0)} s`
  if (s < 3600) return `${Math.floor(s / 60)} min ${Math.round(s % 60)} s`
  if (s < 86400) return `${Math.floor(s / 3600)} h ${Math.round((s % 3600) / 60)} min`
  return `${Math.floor(s / 86400)} d ${Math.round((s % 86400) / 3600)} h`
}

export function ago(ms: number): string {
  if (!ms) return '—'
  const s = (Date.now() - ms) / 1000
  if (s < 2) return 'agora'
  if (s < 60) return `${Math.round(s)} s atrás`
  return `${dur(s)} atrás`
}

export const pct = (v: number, d = 1) => `${dec(+v || 0, d)}%`
export const hhmm = (ms: number) => new Date(ms).toLocaleTimeString('pt-BR', { hour: '2-digit', minute: '2-digit' })
export const hhmmss = (ms: number) => new Date(ms).toLocaleTimeString('pt-BR')
export const dateTime = (ms: number) => new Date(ms).toLocaleString('pt-BR')
export const mbit = (bps: number) => `${dec(bps / 1e6, 2)} Mbit/s`
