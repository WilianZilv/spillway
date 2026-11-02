import { useEffect, useRef, useState, useSyncExternalStore } from 'react'
import { toast } from 'sonner'
import type { Overview, Sample, SpwEvent } from './types'

const TOKEN_KEY = 'spw-token'

export function getToken(): string {
  try {
    return localStorage.getItem(TOKEN_KEY) ?? ''
  } catch {
    return ''
  }
}

export function setToken(t: string) {
  try {
    localStorage.setItem(TOKEN_KEY, t)
  } catch {
    /* private mode */
  }
}

export class AuthError extends Error {}

const authListeners = new Set<() => void>()
let authNeeded = false
export const useAuthNeeded = () =>
  useSyncExternalStore(
    (cb) => {
      authListeners.add(cb)
      return () => authListeners.delete(cb)
    },
    () => authNeeded,
  )
export function setAuthNeeded(v: boolean) {
  authNeeded = v
  authListeners.forEach((l) => l())
}

export async function api<T = unknown>(path: string, opts: { method?: string; body?: unknown } = {}): Promise<T> {
  const headers: Record<string, string> = {}
  if (opts.body !== undefined) headers['content-type'] = 'application/json'
  const tok = getToken()
  if (tok) headers.authorization = `Bearer ${tok}`
  const r = await fetch(path, { method: opts.method ?? 'GET', headers, body: opts.body !== undefined ? JSON.stringify(opts.body) : undefined })
  if (r.status === 401) {
    setAuthNeeded(true)
    throw new AuthError('token necessário')
  }
  const text = await r.text()
  const data = text ? JSON.parse(text) : null
  if (!r.ok) throw new Error(data?.error ?? `HTTP ${r.status}`)
  return data as T
}

/** Runs a mutation, toasts the result. Returns true on success. */
export async function act(fn: () => Promise<unknown>, ok?: string): Promise<boolean> {
  try {
    await fn()
    if (ok) toast.success(ok)
    return true
  } catch (e) {
    toast.error((e as Error).message)
    return false
  }
}

export type Range = '15m' | '24h'

const lastToast = new Map<string, number>()

export interface Live {
  ov: Overview | null
  series: Sample[]
  events: SpwEvent[]
  connected: boolean
}

/**
 * Live state: initial snapshot over REST, then the SSE stream pushes an
 * overview every second (appended to the 15 min series) plus new events.
 */
export function useLive(range: Range, nonce: number): Live {
  const [ov, setOv] = useState<Overview | null>(null)
  const [series, setSeries] = useState<Sample[]>([])
  const [events, setEvents] = useState<SpwEvent[]>([])
  const [connected, setConnected] = useState(false)
  const rangeRef = useRef(range)
  rangeRef.current = range
  const startedRef = useRef(0)

  useEffect(() => {
    let es: EventSource | null = null
    let cancelled = false
    let retry: ReturnType<typeof setTimeout> | undefined

    const start = async () => {
      try {
        const [o, ev, s] = await Promise.all([
          api<Overview>('/api/overview'),
          api<SpwEvent[]>('/api/events'),
          api<Sample[]>(`/api/series?range=${rangeRef.current}`),
        ])
        if (cancelled) return
        setOv(o)
        setEvents(ev.slice().reverse())
        setSeries(s)
      } catch (e) {
        if (!(e instanceof AuthError)) retry = setTimeout(start, 3000)
        setConnected(false)
        return
      }
      const tok = getToken()
      es = new EventSource('/api/stream' + (tok ? `?token=${encodeURIComponent(tok)}` : ''))
      es.addEventListener('overview', (m) => {
        const o = JSON.parse((m as MessageEvent).data) as Overview
        setConnected(true)
        if (startedRef.current && startedRef.current !== o.started_ms) {
          // the master restarted: its history is new, drop ours
          api<Sample[]>(`/api/series?range=${rangeRef.current}`).then(setSeries).catch(() => {})
          api<SpwEvent[]>('/api/events').then((ev) => setEvents(ev.slice().reverse())).catch(() => {})
        }
        startedRef.current = o.started_ms
        setOv(o)
        const r = o.rate
        if (r && rangeRef.current === '15m') {
          setSeries((prev) => {
            if (prev.length && prev[prev.length - 1].t_ms >= r.t_ms) return prev
            const next = prev.concat(r)
            return next.length > 900 ? next.slice(next.length - 900) : next
          })
        }
      })
      es.addEventListener('events', (m) => {
        const evs = JSON.parse((m as MessageEvent).data) as SpwEvent[]
        for (const e of evs) {
          // one toast per event code per minute: a flapping destination must not bury the screen
          if ((e.level === 'critical' || e.level === 'error') && Date.now() - (lastToast.get(e.code) ?? 0) > 60_000) {
            lastToast.set(e.code, Date.now())
            toast.error(e.message)
          }
        }
        setEvents((prev) => evs.slice().reverse().concat(prev).slice(0, 500))
      })
      es.onerror = () => setConnected(false)
    }
    start()
    return () => {
      cancelled = true
      clearTimeout(retry)
      es?.close()
    }
  }, [nonce])

  // Series for the selected range (24 h is refreshed every minute; 15 min grows from SSE).
  useEffect(() => {
    let alive = true
    const load = () =>
      api<Sample[]>(`/api/series?range=${range}`)
        .then((s) => alive && setSeries(s))
        .catch(() => {})
    load()
    const t = range === '24h' ? setInterval(load, 60_000) : undefined
    return () => {
      alive = false
      clearInterval(t)
    }
  }, [range])

  return { ov, series, events, connected }
}

/** Polls a JSON endpoint while mounted. */
export function usePoll<T>(path: string | null, everyMs: number): [T | null, () => void] {
  const [data, setData] = useState<T | null>(null)
  const [tick, setTick] = useState(0)
  useEffect(() => {
    if (!path) return
    let alive = true
    const load = () =>
      api<T>(path)
        .then((d) => alive && setData(d))
        .catch(() => {})
    load()
    const t = setInterval(load, everyMs)
    return () => {
      alive = false
      clearInterval(t)
    }
  }, [path, everyMs, tick])
  return [data, () => setTick((x) => x + 1)]
}
