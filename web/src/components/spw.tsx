import type { ReactNode } from 'react'
import {
  Ban,
  CircleAlert,
  CircleCheck,
  CircleOff,
  CirclePause,
  ArrowDownToLine,
  ChevronRight,
  Flame,
  HardDrive,
  Hourglass,
  Info,
  ListOrdered,
  type LucideIcon,
  PenLine,
  ScanLine,
  Send,
  Server,
  TriangleAlert,
  Webhook,
  WifiOff,
  XCircle,
} from 'lucide-react'
import { cn } from '@/lib/utils'
import { Card, CardContent } from '@/components/ui/card'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { bytes, compact, hhmmss, num, pct, rate } from '@/lib/format'
import type { DestStatus, DestView, Overview, SpwEvent } from '@/lib/types'

// ---------------------------------------------------------------- status

type Tone = 'good' | 'warning' | 'serious' | 'critical' | 'info' | 'neutral'

const TONE: Record<Tone, string> = {
  good: 'text-good-fg bg-good/12',
  warning: 'text-warning-fg bg-warning/18',
  serious: 'text-serious-fg bg-serious/16',
  critical: 'text-critical-fg bg-critical/13',
  info: 'text-brand bg-brand/10',
  neutral: 'text-muted-foreground bg-muted border-border',
}

export function Chip({ tone, icon: Icon, children, className }: { tone: Tone; icon?: LucideIcon; children: ReactNode; className?: string }) {
  return (
    <span className={cn('inline-flex h-5.5 items-center gap-1 rounded-full border border-transparent px-2 text-xs font-semibold whitespace-nowrap', TONE[tone], className)}>
      {Icon && <Icon className="size-3.5" aria-hidden />}
      {children}
    </span>
  )
}

export const DEST_STATUS: Record<DestStatus, [Tone, LucideIcon, string]> = {
  healthy: ['good', CircleCheck, 'Gravando'],
  full: ['warning', TriangleAlert, 'Cheio'],
  suspended: ['serious', CirclePause, 'Suspenso'],
  error: ['critical', XCircle, 'Com falha'],
  offline: ['critical', WifiOff, 'Offline'],
  pending: ['neutral', Hourglass, 'Verificando'],
  draining: ['info', ArrowDownToLine, 'Só leitura'],
  disabled: ['neutral', Ban, 'Desativado'],
}

export function StatusBadge({ status }: { status: DestStatus }) {
  const [tone, icon, label] = DEST_STATUS[status] ?? ['neutral', Info, status]
  return (
    <Chip tone={tone} icon={icon}>
      {label}
    </Chip>
  )
}

export const LEVEL: Record<SpwEvent['level'], [string, LucideIcon]> = {
  critical: ['text-critical-fg', Flame],
  error: ['text-critical-fg', XCircle],
  warn: ['text-warning-fg', TriangleAlert],
  info: ['text-brand', Info],
}

export function EventRow({ e, full }: { e: SpwEvent; full?: boolean }) {
  const [cls, Icon] = LEVEL[e.level] ?? LEVEL.info
  return (
    <div className="grid grid-cols-[20px_64px_minmax(0,1fr)] items-start gap-2 border-b py-2 text-[13px] last:border-b-0">
      <Icon className={cn('mt-0.5 size-4', cls)} aria-label={e.level} />
      <span className="tabular pt-px text-xs text-muted-foreground" title={new Date(e.t_ms).toLocaleString('pt-BR')}>
        {hhmmss(e.t_ms)}
      </span>
      <div className="min-w-0 break-words">
        {e.message} {full !== false && <span className="font-mono text-[11px] text-muted-foreground">{e.code}</span>}
      </div>
    </div>
  )
}

export function Empty({ icon: Icon = Info, children }: { icon?: LucideIcon; children: ReactNode }) {
  return (
    <div className="flex flex-col items-center gap-2 px-3 py-8 text-center text-sm text-muted-foreground">
      <Icon className="size-7 text-muted-foreground/50" aria-hidden />
      {children}
    </div>
  )
}

export function PageHeader({ title, sub, children }: { title: string; sub: string; children?: ReactNode }) {
  return (
    <div className="mb-5 flex flex-wrap items-end gap-3">
      <div className="min-w-0 flex-1">
        <h1 className="text-xl font-semibold tracking-tight">{title}</h1>
        <p className="text-sm text-muted-foreground">{sub}</p>
      </div>
      {children}
    </div>
  )
}

export function SectionTitle({ children, right }: { children: ReactNode; right?: ReactNode }) {
  return (
    <div className="mt-8 mb-3 flex items-center gap-3">
      <h2 className="text-xs font-semibold tracking-wider text-muted-foreground uppercase">{children}</h2>
      <div className="flex-1" />
      {right}
    </div>
  )
}

// ---------------------------------------------------------------- meters

/** Horizontal meter with tick marks (thresholds); the fill turns warning/critical past them. */
export function Meter({
  value,
  scale,
  ticks = [],
  warnAt,
  critAt,
  label,
  className,
}: {
  value: number
  scale: number
  ticks?: { at: number; soft?: boolean }[]
  warnAt?: number
  critAt?: number
  label: string
  className?: string
}) {
  const p = (v: number) => Math.min(100, Math.max(0, (v / Math.max(scale, 1)) * 100))
  const fill = critAt != null && value >= critAt ? 'bg-critical' : warnAt != null && value >= warnAt ? 'bg-warning' : 'bg-brand'
  return (
    <Tooltip>
      <TooltipTrigger render={<div role="meter" aria-valuenow={value} aria-valuemax={scale} aria-label={label} className={cn('relative mt-1 h-2.5 rounded-full bg-muted', className)} />}>
        <span className={cn('absolute inset-y-0 left-0 rounded-full transition-[width] duration-500', fill)} style={{ width: `${p(value)}%` }} />
        {ticks.map((t, i) => (
          <span key={i} className={cn('absolute -inset-y-1 w-0.5 rounded-sm', t.soft ? 'bg-muted-foreground' : 'bg-foreground')} style={{ left: `${p(t.at)}%` }} />
        ))}
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  )
}

export function RamMeter({ ram, className }: { ram: Pick<Overview['ram'], 'used_bytes' | 'thief_max_bytes' | 'thief_resume_bytes' | 'hard_limit_bytes'>; className?: string }) {
  return (
    <Meter
      className={className}
      value={ram.used_bytes}
      scale={Math.max(ram.hard_limit_bytes, ram.used_bytes)}
      ticks={[{ at: ram.thief_resume_bytes, soft: true }, { at: ram.thief_max_bytes }]}
      warnAt={ram.thief_resume_bytes}
      critAt={ram.thief_max_bytes}
      label={`Em RAM ${bytes(ram.used_bytes)} · thief para em ${bytes(ram.thief_resume_bytes)} · ativa em ${bytes(ram.thief_max_bytes)} · escritas recusadas acima de ${bytes(ram.hard_limit_bytes)}`}
    />
  )
}

// ---------------------------------------------------------------- tiles

export function StatTile({
  icon: Icon,
  label,
  value,
  unit,
  foot,
  alert,
  children,
}: {
  icon: LucideIcon
  label: string
  value: ReactNode
  unit?: string
  foot: ReactNode
  alert?: boolean
  children?: ReactNode
}) {
  return (
    <Card className={cn('gap-0 py-4', alert && 'ring-2 ring-critical/40')}>
      <CardContent className="flex flex-col gap-1.5 px-4">
        <div className="flex items-center gap-1.5 text-[12.5px] font-medium text-muted-foreground">
          <Icon className="size-4" aria-hidden />
          {label}
        </div>
        <div className="tabular text-[26px] leading-tight font-semibold tracking-tight">
          {value}
          {unit && <span className="ml-1 text-sm font-medium tracking-normal text-muted-foreground">{unit}</span>}
        </div>
        {children}
        <div className="text-xs text-muted-foreground">{foot}</div>
      </CardContent>
    </Card>
  )
}

// ---------------------------------------------------------------- pipeline

export function Pipeline({ ov }: { ov: Overview }) {
  const r = ov.ram
  const T = ov.totals
  const stages: { icon: LucideIcon; name: string; value: string; sub: string; hot: boolean }[] = [
    { icon: PenLine, name: 'Escrevendo', value: num(r.open_writers), sub: `${num(r.writing - r.open_writers)} fechados, assentando`, hot: r.open_writers > 0 || r.writing > 0 },
    { icon: ScanLine, name: 'Selando + parse', value: num(r.sealing), sub: `${compact(T.files_sealed)} selados no total`, hot: r.sealing > 0 },
    {
      icon: Webhook,
      name: 'Validando',
      value: ov.hooks.validate.enabled ? num(r.deciding) : '—',
      sub: ov.hooks.validate.enabled ? `${compact(T.hook_dropped)} recusados pelo hook` : 'hook desativado',
      hot: r.deciding > 0,
    },
    { icon: ListOrdered, name: 'Na fila', value: num(r.queued), sub: bytes(r.queued_bytes), hot: r.queued > 0 },
    { icon: Send, name: 'Em trânsito', value: num(r.inflight), sub: bytes(r.inflight_bytes), hot: r.inflight > 0 },
    { icon: HardDrive, name: 'Nos discos', value: compact(T.moved_files), sub: `${bytes(T.moved_bytes)} · ${num(T.move_failures)} falhas de cópia`, hot: false },
  ]
  return (
    <div className="grid grid-cols-2 gap-2 md:grid-cols-6 md:gap-0">
      {stages.map((s, i) => (
        <div
          key={s.name}
          className={cn(
            'relative border bg-card px-4 py-3 max-md:rounded-xl md:first:rounded-l-xl md:last:rounded-r-xl md:[&+&]:border-l-0',
            s.hot && 'bg-brand/8',
          )}
        >
          <div className="flex items-center gap-1.5 text-xs font-medium text-muted-foreground">
            <s.icon className="size-3.5" aria-hidden />
            {s.name}
          </div>
          <div className="tabular mt-1 text-[22px] font-semibold tracking-tight">{s.value}</div>
          <div className="text-xs text-muted-foreground">{s.sub}</div>
          {i < stages.length - 1 && (
            <span className="absolute top-1/2 -right-2.5 z-10 hidden size-5 -translate-y-1/2 place-items-center rounded-full border bg-background text-muted-foreground md:grid">
              <ChevronRight className="size-3" aria-hidden />
            </span>
          )}
        </div>
      ))}
    </div>
  )
}

// ---------------------------------------------------------------- distributions

export function HBars({ items, fmt = num, color = 'var(--brand)' }: { items: { label: string; v: number; color?: string }[]; fmt?: (n: number) => string; color?: string }) {
  if (!items.length) return <Empty>Sem dados ainda</Empty>
  const max = Math.max(...items.map((i) => i.v), 1)
  return (
    <div className="flex flex-col gap-2">
      {items.map((i) => (
        <div key={i.label} className="grid grid-cols-[112px_minmax(0,1fr)_72px] items-center gap-2.5 text-[12.5px]">
          <span className="truncate text-muted-foreground" title={i.label}>
            {i.label}
          </span>
          <span className="relative h-2.5">
            <span className="absolute inset-y-0 left-0 min-w-0.5 rounded-r-[4px]" style={{ width: `${(i.v / max) * 100}%`, background: i.color ?? color }} />
          </span>
          <span className="tabular text-right">{fmt(i.v)}</span>
        </div>
      ))}
    </div>
  )
}

export function ShareBar({ parts }: { parts: { label: string; v: number; color: string; detail: string }[] }) {
  const total = parts.reduce((a, p) => a + p.v, 0)
  if (!total) return <p className="text-xs text-muted-foreground">Nada movido ainda</p>
  return (
    <div>
      <div className="my-2 flex gap-0.5">
        {parts
          .filter((p) => p.v > 0)
          .map((p) => (
            <Tooltip key={p.label}>
              <TooltipTrigger render={<span className="h-3 min-w-1 rounded-[3px]" style={{ flex: p.v, background: p.color }} />} />
              <TooltipContent>
                {p.label}: {p.detail} ({pct((p.v / total) * 100)})
              </TooltipContent>
            </Tooltip>
          ))}
      </div>
      <div className="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
        {parts.map((p) => (
          <span key={p.label} className="inline-flex items-center gap-1.5">
            <i className="size-2.5 rounded-[3px]" style={{ background: p.color }} />
            {p.label} <b className="tabular font-semibold text-foreground">{pct((p.v / total) * 100, 0)}</b>
          </span>
        ))}
      </div>
    </div>
  )
}

// ---------------------------------------------------------------- destinations

const SERIES = ['var(--chart-1)', 'var(--chart-2)', 'var(--chart-3)', 'var(--chart-4)', 'var(--chart-5)', 'var(--chart-6)', 'var(--chart-7)', 'var(--chart-8)']
const destColorMap = new Map<string, string>()
/** Color follows the destination (assigned once, in order of first appearance). */
export function destColor(id: string): string {
  if (!destColorMap.has(id)) destColorMap.set(id, SERIES[destColorMap.size % SERIES.length])
  return destColorMap.get(id)!
}

export function DestSummary({ d, local, nowBps }: { d: DestView; local: boolean; nowBps: number }) {
  const soft = ['full', 'draining', 'pending', 'disabled'].includes(d.status)
  const Icon = local ? HardDrive : Server
  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center gap-2.5">
        <span className="grid size-9 shrink-0 place-items-center rounded-lg bg-muted" style={{ color: destColor(d.id) }}>
          <Icon className="size-4.5" aria-hidden />
        </span>
        <div className="min-w-0 flex-1">
          <div className="truncate font-semibold">{d.id}</div>
          <div className="truncate font-mono text-xs text-muted-foreground" title={`${d.node}:${d.path}`}>
            {d.node}:{d.path}
          </div>
        </div>
        <StatusBadge status={d.status} />
      </div>
      {d.reason && d.status !== 'healthy' && (
        <div className={cn('flex gap-2 rounded-lg px-2.5 py-1.5 text-[12.5px]', soft ? 'bg-warning/12 text-warning-fg' : 'bg-critical/8 text-critical-fg')}>
          {soft ? <CircleAlert className="mt-0.5 size-3.5 shrink-0" /> : <CircleOff className="mt-0.5 size-3.5 shrink-0" />}
          <span className="min-w-0 break-words">{d.reason}</span>
        </div>
      )}
      <div>
        <div className="flex items-center text-[12.5px]">
          <span className="text-muted-foreground">{d.total_bytes ? `${bytes(d.total_bytes - d.free_bytes)} de ${bytes(d.total_bytes)}` : 'capacidade desconhecida'}</span>
          <span className="flex-1" />
          <b className="tabular">{pct(d.use_pct)}</b>
        </div>
        <Meter value={d.use_pct} scale={100} ticks={[{ at: d.max_use_pct, soft: true }]} warnAt={d.max_use_pct - 5} critAt={d.max_use_pct} label={`Uso ${pct(d.use_pct)} · para de receber em ${pct(d.max_use_pct, 0)}`} />
      </div>
      <div className="grid grid-cols-4 gap-3 text-[13px]">
        <KV k="Fatia" v={d.status === 'healthy' ? pct(d.share * 100, 0) : '—'} />
        <KV k="Agora" v={rate(nowBps)} />
        <KV k="Movidos" v={compact(d.moved_files)} />
        <div>
          <span className="block text-[11.5px] text-muted-foreground">Cópias</span>
          <Tooltip>
            <TooltipTrigger render={<span className="inline-flex gap-0.5 align-middle" />}>
              {Array.from({ length: Math.min(d.max_inflight, 16) }, (_, i) => (
                <i key={i} className={cn('h-3 w-1.5 rounded-[2px]', i < d.inflight ? 'bg-brand' : 'bg-muted')} />
              ))}
            </TooltipTrigger>
            <TooltipContent>
              {d.inflight} de {d.max_inflight} cópias simultâneas
            </TooltipContent>
          </Tooltip>
        </div>
      </div>
    </div>
  )
}

export function KV({ k, v }: { k: string; v: ReactNode }) {
  return (
    <div className="min-w-0">
      <span className="block text-[11.5px] text-muted-foreground">{k}</span>
      <b className="tabular font-semibold">{v}</b>
    </div>
  )
}


