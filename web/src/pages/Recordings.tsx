import { useEffect, useMemo, useState } from 'react'
import { ArrowDown, ArrowUp, CircleCheck, Search, Tags } from 'lucide-react'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Empty, HBars, PageHeader, SectionTitle } from '@/components/spw'
import { usePoll } from '@/lib/api'
import { ago, bytes, dur, mbit, num, pct } from '@/lib/format'
import type { GroupStat, Overview } from '@/lib/types'

const DUR_EDGES = [0, 30, 60, 120, 180, 300, 600, 1800]
const SIZE_EDGES = [0, 0.5, 1, 2, 4, 8, 16, 32, 64]
const REASONS: Record<string, string> = {
  not_mp4: 'Não é MP4',
  moov_missing: 'Sem moov',
  truncated: 'Truncado',
  duration_zero: 'Duração zero',
  box_invalid: 'Box inválido',
  io_error: 'Erro de leitura',
}
const edges = (e: number[], unit: string) => e.map((v, i) => (i < e.length - 1 ? `${String(v).replace('.', ',')}–${String(e[i + 1]).replace('.', ',')} ${unit}` : `≥ ${v} ${unit}`))

function Mini({ k, v }: { k: string; v: string }) {
  return (
    <div className="rounded-xl border bg-card px-3.5 py-3">
      <div className="text-[11.5px] text-muted-foreground">{k}</div>
      <div className="tabular text-lg font-semibold">{v}</div>
    </div>
  )
}

type SortKey = 'value' | 'files' | 'bytes' | 'duration_sec' | 'bitrate' | 'codec' | 'last_ms' | 'lost_files' | 'dropped_files'

/** Aggregates per label value. Labels come from the validate hook; spillway does not interpret them. */
function Groups({ ov }: { ov: Overview }) {
  const keys = Object.keys(ov.groups)
  const [key, setKey] = useState<string | null>(null)
  const active = key && keys.includes(key) ? key : (keys[0] ?? null)
  useEffect(() => {
    if (!key && keys[0]) setKey(keys[0])
  }, [key, keys])
  const [data] = usePoll<{ groups: GroupStat[] }>(active ? `/api/groups?key=${encodeURIComponent(active)}` : null, 5000)
  const [q, setQ] = useState('')
  const [sort, setSort] = useState<{ k: SortKey; d: 1 | -1 }>({ k: 'last_ms', d: -1 })

  const rows = useMemo(() => {
    let r = (data?.groups ?? []).map((g) => ({ ...g, bitrate: g.duration_sec ? (g.bytes * 8) / g.duration_sec : 0 }))
    const needle = q.trim().toLowerCase()
    if (needle) r = r.filter((g) => g.value.toLowerCase().includes(needle))
    r.sort((a, b) => (a[sort.k] > b[sort.k] ? sort.d : a[sort.k] < b[sort.k] ? -sort.d : 0))
    return r
  }, [data, q, sort])

  const cols: [SortKey, string, boolean][] = [
    ['value', active ?? 'Valor', false],
    ['files', 'Arquivos', true],
    ['bytes', 'Volume', true],
    ['duration_sec', 'Duração', true],
    ['bitrate', 'Bitrate', true],
    ['codec', 'Vídeo', false],
    ['last_ms', 'Último', false],
    ['dropped_files', 'Recusados', true],
    ['lost_files', 'Descartados', true],
  ]
  const items = keys.map((k) => ({ value: k, label: `${k} (${num(ov.groups[k])})` }))

  return (
    <>
      <SectionTitle
        right={
          keys.length > 0 && (
            <div className="flex items-center gap-2">
              <Select items={items} value={active ?? ''} onValueChange={(v) => v && setKey(v as string)}>
                <SelectTrigger className="w-48" aria-label="Agrupar por label">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {items.map((i) => (
                    <SelectItem key={i.value} value={i.value}>
                      {i.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <label className="relative">
                <Search className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
                <Input className="w-56 pl-8" placeholder="Filtrar valores" value={q} onChange={(e) => setQ(e.target.value)} aria-label="Filtrar grupos" />
              </label>
            </div>
          )
        }
      >
        Grupos por label
      </SectionTitle>
      <Card>
        <CardContent>
          {keys.length === 0 ? (
            <Empty icon={Tags}>
              Nenhum label ainda.
              <span className="max-w-md">O hook validate pode devolver labels para cada arquivo (por exemplo câmera ou cliente); o painel agrupa as estatísticas por eles.</span>
            </Empty>
          ) : (
            <>
              <Table>
                <TableHeader>
                  <TableRow>
                    {cols.map(([k, label, right]) => (
                      <TableHead key={k} className={right ? 'text-right' : ''}>
                        <button
                          className="inline-flex items-center gap-1 hover:text-foreground"
                          onClick={() => setSort((s) => ({ k, d: s.k === k ? (s.d === 1 ? -1 : 1) : -1 }))}
                          aria-sort={sort.k === k ? (sort.d === 1 ? 'ascending' : 'descending') : 'none'}
                        >
                          {label}
                          {sort.k === k && (sort.d === 1 ? <ArrowUp className="size-3" /> : <ArrowDown className="size-3" />)}
                        </button>
                      </TableHead>
                    ))}
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {rows.slice(0, 300).map((g) => (
                    <TableRow key={g.value}>
                      <TableCell className="max-w-[360px] truncate font-mono text-xs" title={g.value}>
                        {g.value}
                      </TableCell>
                      <TableCell className="tabular text-right">{num(g.files)}</TableCell>
                      <TableCell className="tabular text-right">{bytes(g.bytes)}</TableCell>
                      <TableCell className="tabular text-right">{dur(g.duration_sec)}</TableCell>
                      <TableCell className="tabular text-right">{g.bitrate ? mbit(g.bitrate) : '—'}</TableCell>
                      <TableCell>
                        {g.codec || '—'} {g.width > 0 && <span className="text-muted-foreground">{g.width}×{g.height}</span>}
                      </TableCell>
                      <TableCell>{ago(g.last_ms)}</TableCell>
                      <TableCell className="tabular text-right">{g.dropped_files ? <span className="font-semibold text-serious-fg">{num(g.dropped_files)}</span> : 0}</TableCell>
                      <TableCell className="tabular text-right">{g.lost_files ? <span className="font-semibold text-critical-fg">{num(g.lost_files)}</span> : 0}</TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
              {!rows.length && <Empty>Nada para este filtro</Empty>}
              {rows.length > 300 && <p className="pt-2 text-xs text-muted-foreground">Mostrando 300 de {num(rows.length)}</p>}
            </>
          )}
        </CardContent>
      </Card>
    </>
  )
}

export function Recordings({ ov }: { ov: Overview }) {
  const m = ov.mp4
  const attempted = m.ok + m.failed
  const dl = edges(DUR_EDGES, 's')
  const sl = edges(SIZE_EDGES, 'MiB')
  const avgBitrate = m.duration_total ? (m.bytes_parsed * 8) / m.duration_total : 0

  return (
    <div>
      <PageHeader title="Gravações" sub="Cada arquivo é analisado ao ser selado: estrutura MP4/fMP4, duração e codec. Os agrupamentos vêm dos labels do hook." />
      <div className="grid grid-cols-2 gap-3 sm:grid-cols-4 xl:grid-cols-8">
        <Mini k="Arquivos analisados" v={num(attempted)} />
        <Mini k="MP4 válidos" v={attempted ? pct((m.ok / attempted) * 100) : '—'} />
        <Mini k="Fragmentados (fMP4)" v={num(m.fragmented)} />
        <Mini k="Progressivos" v={num(m.progressive)} />
        <Mini k="Duração média" v={m.ok ? dur(m.duration_total / m.ok) : '—'} />
        <Mini k="Bitrate médio" v={avgBitrate ? mbit(avgBitrate) : '—'} />
        <Mini k="Horas gravadas" v={`${(m.duration_total / 3600).toFixed(1).replace('.', ',')} h`} />
        <Mini k="Recusados pelo hook" v={num(ov.totals.hook_dropped)} />
      </div>
      <div className="mt-4 grid grid-cols-1 gap-4 md:grid-cols-2 2xl:grid-cols-4">
        <Card>
          <CardHeader>
            <CardTitle>Duração dos segmentos</CardTitle>
            <CardDescription>Arquivos por faixa</CardDescription>
          </CardHeader>
          <CardContent>
            <HBars items={m.duration_hist.map((v, i) => ({ label: dl[i], v })).filter((x) => x.v)} />
          </CardContent>
        </Card>
        <Card>
          <CardHeader>
            <CardTitle>Tamanho dos arquivos</CardTitle>
            <CardDescription>Arquivos por faixa</CardDescription>
          </CardHeader>
          <CardContent>
            <HBars items={m.size_hist.map((v, i) => ({ label: sl[i], v })).filter((x) => x.v)} />
          </CardContent>
        </Card>
        <Card>
          <CardHeader>
            <CardTitle>Codecs e resoluções</CardTitle>
            <CardDescription>Trilha de vídeo</CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-4">
            <HBars items={Object.entries(m.codecs).sort((a, b) => b[1] - a[1]).map(([k, v]) => ({ label: k, v }))} />
            {Object.keys(m.resolutions).length > 0 && (
              <HBars items={Object.entries(m.resolutions).sort((a, b) => b[1] - a[1]).slice(0, 8).map(([k, v]) => ({ label: k, v }))} />
            )}
          </CardContent>
        </Card>
        <Card>
          <CardHeader>
            <CardTitle>Falhas de parse</CardTitle>
            <CardDescription>Ainda são movidos, a menos que configurado</CardDescription>
          </CardHeader>
          <CardContent>
            {m.failed ? (
              <HBars items={Object.entries(m.fail_reasons).sort((a, b) => b[1] - a[1]).map(([k, v]) => ({ label: REASONS[k] ?? k, v }))} color="var(--serious)" />
            ) : (
              <Empty icon={CircleCheck}>Nenhuma falha</Empty>
            )}
          </CardContent>
        </Card>
      </div>
      <Groups ov={ov} />
    </div>
  )
}
