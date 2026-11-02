import { ArrowDownToLine, ArrowRight, ArrowUpFromLine, Flame, HardDrive, Info, ListOrdered, MemoryStick, Plus, TriangleAlert } from 'lucide-react'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Card, CardAction, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Legend, TimeChart } from '@/components/charts'
import { DestSummary, Empty, EventRow, Pipeline, RamMeter, SectionTitle, ShareBar, StatTile, destColor } from '@/components/spw'
import { RangePicker } from '@/components/range'
import { HooksPanel } from '@/components/hooks'
import { bytes, bytesAxis, bytesParts, compact, dur, num, rate, rateAxis } from '@/lib/format'
import type { Range } from '@/lib/api'
import type { Overview as Ov, Sample, SpwEvent } from '@/lib/types'

export function ThroughputCard({ series, range, setRange }: { series: Sample[]; range: Range; setRange: (r: Range) => void }) {
  const s = [
    { key: 'ingest_bps', label: 'Entrada (escrita na RAM)', color: 'var(--chart-1)' },
    { key: 'egress_bps', label: 'Saída (para os discos)', color: 'var(--chart-2)' },
    { key: 'read_bps', label: 'Leitura pelo mount', color: 'var(--chart-3)' },
  ]
  return (
    <Card>
      <CardHeader>
        <CardTitle>Vazão</CardTitle>
        <CardDescription>Bytes por segundo</CardDescription>
        <CardAction>
          <RangePicker value={range} onChange={setRange} />
        </CardAction>
      </CardHeader>
      <CardContent>
        <Legend items={s} />
        <TimeChart data={series as unknown as Record<string, number>[]} series={s} fmt={rate} axisFmt={rateAxis} />
      </CardContent>
    </Card>
  )
}

/** Per-destination egress lines. Keys are positional so odd ids never become invalid CSS variables. */
export function DestRateChart({ ov, series }: { ov: Ov; series: Sample[] }) {
  const ids = ov.dests.map((d) => d.id)
  const s = ids.map((id, i) => ({ key: `d${i}`, label: id, color: destColor(id) }))
  const data = series.map((row) => {
    const o: Record<string, number> = { t_ms: row.t_ms }
    ids.forEach((id, i) => (o[`d${i}`] = row.dest_bps?.[id] ?? 0))
    return o
  })
  return (
    <>
      <Legend items={s} />
      <TimeChart data={data} series={s} fmt={rate} axisFmt={rateAxis} height={200} />
    </>
  )
}

function Banner({ ov }: { ov: Ov }) {
  const writable = ov.dests.filter((d) => d.status === 'healthy').length
  if (ov.thief.active)
    return (
      <Alert variant="destructive" className="mb-4">
        <Flame />
        <AlertTitle>Thief ativo: a RAM chegou ao teto de {bytes(ov.ram.thief_max_bytes)}</AlertTitle>
        <AlertDescription>
          Os arquivos mais antigos ainda não copiados estão sendo descartados. {num(ov.thief.episode_files)} arquivos ({bytes(ov.thief.episode_bytes)}) neste episódio
          {ov.thief.stalled && ' · travado: nada mais pode ser descartado'}.
        </AlertDescription>
      </Alert>
    )
  if (!ov.dests.length)
    return (
      <Alert className="mb-4">
        <Info />
        <AlertTitle>Nenhum destino configurado</AlertTitle>
        <AlertDescription>
          <span>
            Os arquivos ficam na RAM até existir um destino. <a className="font-medium text-brand underline-offset-4 hover:underline" href="#dests">Adicionar destino</a>
          </span>
        </AlertDescription>
      </Alert>
    )
  if (!writable)
    return (
      <Alert variant="destructive" className="mb-4">
        <TriangleAlert />
        <AlertTitle>Nenhum destino pode receber arquivos</AlertTitle>
        <AlertDescription>
          {bytes(ov.ram.used_bytes)} acumulados na RAM; o thief entra em {bytes(ov.ram.thief_max_bytes)}.
        </AlertDescription>
      </Alert>
    )
  const bad = ov.dests.filter((d) => ['error', 'offline', 'suspended'].includes(d.status))
  if (bad.length)
    return (
      <Alert className="mb-4 border-warning/50 text-warning-fg">
        <TriangleAlert />
        <AlertTitle>
          {bad.length === 1 ? 'Um destino está fora' : `${bad.length} destinos estão fora`}: {bad.map((d) => d.id).join(', ')}
        </AlertTitle>
        <AlertDescription>O tráfego foi redistribuído para os destinos saudáveis.</AlertDescription>
      </Alert>
    )
  return null
}

export function Overview({ ov, series, events, range, setRange }: { ov: Ov; series: Sample[]; events: SpwEvent[]; range: Range; setRange: (r: Range) => void }) {
  const r = ov.ram
  const T = ov.totals
  const rt = ov.rate
  const writable = ov.dests.filter((d) => d.status === 'healthy')
  const free = writable.reduce((a, d) => a + d.free_bytes, 0)
  const [uv, uu] = bytesParts(r.used_bytes)
  const rv = (b: number) => {
    const [v, u] = bytesParts(b)
    return [v, `${u}/s`] as const
  }
  const [iv, iu] = rv(rt?.ingest_bps ?? 0)
  const [ev, eu] = rv(rt?.egress_bps ?? 0)
  const localId = ov.nodes.find((n) => n.local)?.id

  return (
    <div>
      <Banner ov={ov} />
      <div className="grid grid-cols-1 gap-4 min-[420px]:grid-cols-2 lg:grid-cols-3 2xl:grid-cols-6">
        <StatTile icon={MemoryStick} label="RAM em uso" value={uv} unit={uu} foot={`${num(r.files)} arquivos · teto ${bytes(r.thief_max_bytes)}`} alert={ov.thief.active}>
          <RamMeter ram={r} />
        </StatTile>
        <StatTile icon={ArrowDownToLine} label="Entrada" value={iv} unit={iu} foot={`${compact(T.files_created)} arquivos · ${bytes(T.ingest_bytes)} no total`} />
        <StatTile icon={ArrowUpFromLine} label="Saída para discos" value={ev} unit={eu} foot={`${compact(T.moved_files)} movidos · ${bytes(T.moved_bytes)}`} />
        <StatTile
          icon={ListOrdered}
          label="Fila"
          value={num(r.queued)}
          foot={`${num(r.inflight)} em trânsito · ${num(r.open_writers)} abertos${r.queued ? ` · mais antigo ${dur(r.oldest_queued_age_s)}` : ''}`}
        />
        <StatTile
          icon={Flame}
          label="Thief"
          value={ov.thief.active ? 'Ativo' : 'Em espera'}
          foot={T.thief_files ? `${num(T.thief_files)} descartados (${bytes(T.thief_bytes)}) · ${ov.thief.activations} ativações` : 'Nenhum arquivo descartado'}
          alert={ov.thief.active}
        />
        <StatTile icon={HardDrive} label="Destinos gravando" value={writable.length} unit={`de ${ov.dests.length}`} foot={`${bytes(free)} livres para gravação`} />
      </div>

      <SectionTitle>Ciclo de vida dos arquivos</SectionTitle>
      <Pipeline ov={ov} />

      <SectionTitle>Hooks</SectionTitle>
      <HooksPanel hooks={ov.hooks} />

      <div className="mt-4 grid grid-cols-1 gap-4 xl:grid-cols-2">
        <ThroughputCard series={series} range={range} setRange={setRange} />
        <Card>
          <CardHeader>
            <CardTitle>RAM em uso</CardTitle>
            <CardDescription>Com o teto do thief e o ponto em que ele para</CardDescription>
          </CardHeader>
          <CardContent>
            <TimeChart
              data={series as unknown as Record<string, number>[]}
              series={[{ key: 'ram_used', label: 'RAM em uso', color: 'var(--chart-1)', area: true }]}
              fmt={(v) => bytes(v, 1)}
              axisFmt={bytesAxis}
              refs={[
                { y: r.thief_max_bytes, label: `teto ${bytes(r.thief_max_bytes, 1)}` },
                { y: r.thief_resume_bytes, label: `para em ${bytes(r.thief_resume_bytes, 1)}` },
              ]}
              height={244}
            />
          </CardContent>
        </Card>
      </div>

      <SectionTitle
        right={
          <Button variant="ghost" size="sm" render={<a href="#dests" />}>
            Gerenciar <ArrowRight />
          </Button>
        }
      >
        Destinos
      </SectionTitle>
      {ov.dests.length ? (
        <div className="grid grid-cols-1 gap-4 md:grid-cols-2 2xl:grid-cols-3">
          {ov.dests.map((d) => (
            <Card key={d.id} className="py-4">
              <CardContent className="px-4">
                <DestSummary d={d} local={d.node === localId} nowBps={rt?.dest_bps?.[d.id] ?? 0} />
              </CardContent>
            </Card>
          ))}
        </div>
      ) : (
        <Card>
          <Empty icon={HardDrive}>
            Nenhum destino ainda
            <Button size="sm" render={<a href="#dests" />}>
              <Plus /> Adicionar
            </Button>
          </Empty>
        </Card>
      )}

      <div className="mt-4 grid grid-cols-1 gap-4 xl:grid-cols-[minmax(0,2fr)_minmax(320px,1fr)]">
        <Card>
          <CardHeader>
            <CardTitle>Saída por destino</CardTitle>
            <CardDescription>Bytes por segundo gravados em cada destino</CardDescription>
          </CardHeader>
          <CardContent>
            <DestRateChart ov={ov} series={series} />
            <div className="mt-3">
              <div className="text-xs text-muted-foreground">Divisão do volume já movido</div>
              <ShareBar parts={ov.dests.map((d) => ({ label: d.id, v: d.moved_bytes, color: destColor(d.id), detail: bytes(d.moved_bytes) }))} />
            </div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader>
            <CardTitle>Eventos recentes</CardTitle>
            <CardAction>
              <Button variant="ghost" size="sm" render={<a href="#events" />}>
                Todos <ArrowRight />
              </Button>
            </CardAction>
          </CardHeader>
          <CardContent>
            {events.length ? events.slice(0, 9).map((e) => <EventRow key={e.seq} e={e} full={false} />) : <Empty>Sem eventos</Empty>}
          </CardContent>
        </Card>
      </div>
    </div>
  )
}
