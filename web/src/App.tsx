import { useEffect, useState } from 'react'
import { Activity, Bell, Film, Folder, HardDrive, LayoutDashboard, Moon, Network, SlidersHorizontal, Sun } from 'lucide-react'
import { ThemeProvider, useTheme } from 'next-themes'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Toaster } from '@/components/ui/sonner'
import { TooltipProvider } from '@/components/ui/tooltip'
import { Empty } from '@/components/spw'
import { setAuthNeeded, setToken, useAuthNeeded, useLive, type Range } from '@/lib/api'
import { dur } from '@/lib/format'
import { cn } from '@/lib/utils'
import { Cluster } from '@/pages/Cluster'
import { Destinations } from '@/pages/Destinations'
import { Events } from '@/pages/Events'
import { Files } from '@/pages/Files'
import { Overview } from '@/pages/Overview'
import { Recordings } from '@/pages/Recordings'
import { Settings } from '@/pages/Settings'

const TABS = [
  { id: 'overview', label: 'Visão geral', icon: LayoutDashboard },
  { id: 'dests', label: 'Destinos', icon: HardDrive },
  { id: 'cluster', label: 'Cluster', icon: Network },
  { id: 'rec', label: 'Gravações', icon: Film },
  { id: 'files', label: 'Arquivos', icon: Folder },
  { id: 'events', label: 'Eventos', icon: Bell },
  { id: 'settings', label: 'Configurações', icon: SlidersHorizontal },
] as const
type TabId = (typeof TABS)[number]['id']

const readHash = (): TabId => {
  const h = location.hash.slice(1)
  return (TABS.find((t) => t.id === h)?.id ?? 'overview') as TabId
}

function Logo() {
  return (
    <svg viewBox="0 0 32 32" className="size-8 shrink-0" aria-hidden>
      <rect width="32" height="32" rx="8" className="fill-brand" />
      <path d="M7 11h18M7 16h12M7 21h7" stroke="white" strokeWidth="3" strokeLinecap="round" />
    </svg>
  )
}

function ThemeToggle() {
  const { resolvedTheme, setTheme } = useTheme()
  const dark = resolvedTheme === 'dark'
  return (
    <Button variant="outline" size="icon" onClick={() => setTheme(dark ? 'light' : 'dark')} aria-label="Alternar tema">
      {dark ? <Sun /> : <Moon />}
    </Button>
  )
}

function TokenDialog({ onDone }: { onDone: () => void }) {
  const open = useAuthNeeded()
  const [v, setV] = useState('')
  return (
    <Dialog open={open} onOpenChange={() => {}}>
      <DialogContent showCloseButton={false}>
        <form
          className="grid gap-4"
          onSubmit={(e) => {
            e.preventDefault()
            setToken(v.trim())
            setAuthNeeded(false)
            onDone()
          }}
        >
          <DialogHeader>
            <DialogTitle>Acesso ao painel</DialogTitle>
            <DialogDescription>
              Este master exige um token (<span className="font-mono">web.token</span>).
            </DialogDescription>
          </DialogHeader>
          <Input type="password" autoFocus value={v} onChange={(e) => setV(e.target.value)} placeholder="Token" autoComplete="current-password" />
          <DialogFooter>
            <Button type="submit">Entrar</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

function Shell() {
  const [tab, setTab] = useState<TabId>(readHash)
  const [range, setRangeState] = useState<Range>(() => (localStorage.getItem('spw-range') as Range) || '15m')
  const [nonce, setNonce] = useState(0)
  const { ov, series, events, connected } = useLive(range, nonce)

  useEffect(() => {
    const on = () => {
      setTab(readHash())
      window.scrollTo({ top: 0 })
    }
    addEventListener('hashchange', on)
    return () => removeEventListener('hashchange', on)
  }, [])
  const setRange = (r: Range) => {
    setRangeState(r)
    try {
      localStorage.setItem('spw-range', r)
    } catch {
      /* ignore */
    }
  }
  useEffect(() => {
    document.title = ov ? `spillway · ${ov.hostname}` : 'spillway'
  }, [ov?.hostname])

  const bad = ov?.dests.filter((d) => ['error', 'offline', 'suspended'].includes(d.status)).length ?? 0

  return (
    <div className="min-h-svh bg-muted/40">
      <header className="sticky top-0 z-30 border-b bg-background/85 backdrop-blur">
        <div className="mx-auto flex max-w-[1480px] flex-wrap items-center gap-x-5 gap-y-2 px-4 py-2.5 md:px-6">
          <div className="flex min-w-0 items-center gap-2.5">
            <Logo />
            <div className="min-w-0">
              <div className="text-[15px] leading-tight font-semibold tracking-tight">spillway</div>
              <div className="max-w-[300px] truncate text-xs text-muted-foreground">
                {ov ? `${ov.hostname} · ${ov.mount_path} · v${ov.version} · ativo há ${dur((ov.now_ms - ov.started_ms) / 1000)}` : 'conectando…'}
              </div>
            </div>
          </div>
          <nav className="order-3 -mx-1 flex w-full gap-0.5 overflow-x-auto px-1 [scrollbar-width:none] lg:order-none lg:w-auto lg:flex-1" role="tablist">
            {TABS.map((t) => (
              <a
                key={t.id}
                href={`#${t.id}`}
                role="tab"
                aria-selected={tab === t.id}
                className={cn(
                  'inline-flex h-8 items-center gap-1.5 rounded-lg px-3 text-sm font-medium whitespace-nowrap text-muted-foreground transition-colors hover:bg-muted hover:text-foreground',
                  tab === t.id && 'bg-muted text-foreground ring-1 ring-border',
                )}
              >
                <t.icon className="size-4" />
                {t.label}
                {t.id === 'dests' && bad > 0 && <span className="ml-0.5 grid h-4.5 min-w-4.5 place-items-center rounded-full bg-critical px-1 text-[11px] font-bold text-white">{bad}</span>}
              </a>
            ))}
          </nav>
          <div className="ml-auto flex items-center gap-3">
            <span className="inline-flex items-center gap-1.5 text-xs text-muted-foreground" title="Conexão ao vivo com o master">
              <i className={cn('size-2 rounded-full', connected ? 'bg-good ring-3 ring-good/25' : 'bg-critical')} />
              {connected ? 'ao vivo' : 'reconectando'}
            </span>
            <ThemeToggle />
          </div>
        </div>
      </header>
      <main className="mx-auto max-w-[1480px] px-4 pt-5 pb-12 md:px-6">
        {!ov ? (
          <Empty icon={Activity}>Conectando ao master…</Empty>
        ) : tab === 'overview' ? (
          <Overview ov={ov} series={series} events={events} range={range} setRange={setRange} />
        ) : tab === 'dests' ? (
          <Destinations ov={ov} series={series} range={range} setRange={setRange} />
        ) : tab === 'cluster' ? (
          <Cluster ov={ov} />
        ) : tab === 'rec' ? (
          <Recordings ov={ov} />
        ) : tab === 'files' ? (
          <Files />
        ) : tab === 'events' ? (
          <Events events={events} />
        ) : (
          <Settings ov={ov} />
        )}
      </main>
      <TokenDialog onDone={() => setNonce((n) => n + 1)} />
      <Toaster position="bottom-center" richColors />
    </div>
  )
}

export default function App() {
  return (
    <ThemeProvider attribute="class" defaultTheme="system" enableSystem storageKey="spw-theme" disableTransitionOnChange>
      <TooltipProvider delay={150}>
        <Shell />
      </TooltipProvider>
    </ThemeProvider>
  )
}
