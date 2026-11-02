import { Area, AreaChart, CartesianGrid, ReferenceLine, XAxis, YAxis } from 'recharts'
import { ChartContainer, ChartTooltip, ChartTooltipContent, type ChartConfig } from '@/components/ui/chart'
import { hhmm, hhmmss } from '@/lib/format'

const hms = (ms: number) => new Date(ms).toLocaleTimeString('pt-BR', { hour: '2-digit', minute: '2-digit', second: '2-digit' })

export interface TimeSeries {
  key: string
  label: string
  color: string
  /** Light wash under the line (single-series magnitude charts). */
  area?: boolean
}

function niceMax(v: number) {
  if (v <= 0) return 1
  const p = Math.pow(10, Math.floor(Math.log10(v)))
  for (const m of [1, 1.2, 1.5, 2, 2.5, 3, 4, 5, 6, 8, 10]) if (m * p >= v) return m * p
  return 10 * p
}

/**
 * Time-series chart: 2px lines, optional 10% area wash, hairline grid, one
 * y-axis, reference lines (thresholds) and a crosshair tooltip.
 */
export function TimeChart({
  data,
  series,
  fmt,
  axisFmt,
  refs = [],
  height = 220,
}: {
  data: Record<string, number>[]
  series: TimeSeries[]
  fmt: (v: number) => string
  /** Shorter formatter for y-axis ticks (defaults to `fmt`). */
  axisFmt?: (v: number) => string
  refs?: { y: number; label: string }[]
  height?: number
}) {
  const span = data.length > 1 ? data[data.length - 1].t_ms - data[0].t_ms : 0
  const xFmt = span < 10 * 60_000 ? hms : hhmm
  const config: ChartConfig = Object.fromEntries(series.map((s) => [s.key, { label: s.label, color: s.color }]))
  let max = 0
  for (const row of data) for (const s of series) max = Math.max(max, row[s.key] ?? 0)
  for (const r of refs) max = Math.max(max, r.y)
  const top = niceMax(max * 1.05)
  return (
    <ChartContainer config={config} className="aspect-auto w-full" style={{ height }}>
      <AreaChart data={data} margin={{ top: 14, right: 8, left: 4, bottom: 0 }}>
        <CartesianGrid vertical={false} />
        <XAxis
          dataKey="t_ms"
          type="number"
          scale="time"
          domain={['dataMin', 'dataMax']}
          tickFormatter={xFmt}
          tickLine={false}
          axisLine={false}
          minTickGap={48}
          tickMargin={8}
        />
        <YAxis domain={[0, top]} tickCount={5} tickFormatter={(v: number) => (axisFmt ?? fmt)(v)} tickLine={false} axisLine={false} width={84} />
        {refs.map((r) => (
          <ReferenceLine
            key={r.label}
            y={r.y}
            stroke="var(--muted-foreground)"
            strokeWidth={1}
            ifOverflow="extendDomain"
            label={(p: { viewBox?: { x?: number; y?: number; width?: number } }) => (
              <text x={(p.viewBox?.x ?? 0) + (p.viewBox?.width ?? 0) - 4} y={(p.viewBox?.y ?? 0) - 5} textAnchor="end" className="fill-muted-foreground text-[11px] font-semibold">
                {r.label}
              </text>
            )}
          />
        ))}
        <ChartTooltip
          cursor={{ stroke: 'var(--border)' }}
          content={
            <ChartTooltipContent
              labelFormatter={(_, payload) => (payload?.[0]?.payload?.t_ms ? hhmmss(payload[0].payload.t_ms as number) : '')}
              formatter={(value, name, item) => (
                <div className="flex w-full items-center gap-2">
                  <i className="size-2.5 shrink-0 rounded-[3px]" style={{ background: item.color }} />
                  <span className="text-muted-foreground">{config[name as string]?.label ?? name}</span>
                  <span className="tabular ml-auto pl-3 font-medium text-foreground">{fmt(Number(value))}</span>
                </div>
              )}
            />
          }
        />
        {series.map((s) => (
          <Area
            key={s.key}
            dataKey={s.key}
            type="linear"
            stroke={`var(--color-${s.key})`}
            strokeWidth={2}
            fill={`var(--color-${s.key})`}
            fillOpacity={s.area ? 0.1 : 0}
            isAnimationActive={false}
            dot={false}
            activeDot={{ r: 4, strokeWidth: 2, stroke: 'var(--card)' }}
          />
        ))}
      </AreaChart>
    </ChartContainer>
  )
}

export function Legend({ items }: { items: { label: string; color: string }[] }) {
  return (
    <div className="mb-2 flex flex-wrap gap-x-4 gap-y-1 text-[12.5px] text-muted-foreground">
      {items.map((i) => (
        <span key={i.label} className="inline-flex items-center gap-1.5">
          <i className="h-[3px] w-3 rounded-sm" style={{ background: i.color }} />
          {i.label}
        </span>
      ))}
    </div>
  )
}
