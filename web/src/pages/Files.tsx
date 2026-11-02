import { Fragment, useCallback, useEffect, useState } from 'react'
import { FileVideo, Folder, RefreshCw, TriangleAlert } from 'lucide-react'
import { toast } from 'sonner'
import { Badge } from '@/components/ui/badge'
import { Breadcrumb, BreadcrumbItem, BreadcrumbLink, BreadcrumbList, BreadcrumbSeparator } from '@/components/ui/breadcrumb'
import { Button } from '@/components/ui/button'
import { Card, CardAction, CardContent, CardHeader } from '@/components/ui/card'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { Chip, Empty, PageHeader, SectionTitle } from '@/components/spw'
import { api, usePoll } from '@/lib/api'
import { bytes, dateTime, dur, hhmmss, num } from '@/lib/format'
import type { BrowseEntry, RecentFile } from '@/lib/types'

const FSTATE: Record<string, string> = { open: 'escrevendo', writing: 'aguardando assentar', sealing: 'selando', deciding: 'aguardando o hook', queued: 'na fila', inflight: 'copiando' }
const OUTCOME: Record<RecentFile['outcome'], ['good' | 'critical' | 'serious' | 'neutral', string]> = {
  moved: ['good', 'Movido'],
  stolen: ['critical', 'Descartado (thief)'],
  invalid: ['serious', 'Inválido'],
  empty: ['neutral', 'Vazio'],
  dropped: ['serious', 'Recusado pelo hook'],
}

export function Files() {
  const [path, setPath] = useState('')
  const [data, setData] = useState<{ entries: BrowseEntry[]; truncated: boolean } | null>(null)
  const [loading, setLoading] = useState(false)
  const [recent, reloadRecent] = usePoll<RecentFile[]>('/api/recent', 3000)

  const load = useCallback(async (p: string) => {
    setLoading(true)
    try {
      setData(await api(`/api/browse?path=${encodeURIComponent(p)}`))
    } catch (e) {
      toast.error((e as Error).message)
    }
    setLoading(false)
  }, [])
  useEffect(() => {
    load(path)
  }, [path, load])

  const parts = path ? path.split('/') : []
  return (
    <div>
      <PageHeader title="Arquivos" sub="A visão unificada do mount: cada entrada mostra onde está agora, na RAM ou em qual destino." />
      <Card>
        <CardHeader>
          <Breadcrumb>
            <BreadcrumbList>
              <BreadcrumbItem>
                <BreadcrumbLink render={<button onClick={() => setPath('')} />} className="inline-flex items-center gap-1 font-mono">
                  <Folder className="size-4" /> /
                </BreadcrumbLink>
              </BreadcrumbItem>
              {parts.map((p, i) => (
                <Fragment key={i}>
                  <BreadcrumbSeparator />
                  <BreadcrumbItem>
                    <BreadcrumbLink render={<button onClick={() => setPath(parts.slice(0, i + 1).join('/'))} />} className="max-w-[24ch] truncate font-mono">
                      {p}
                    </BreadcrumbLink>
                  </BreadcrumbItem>
                </Fragment>
              ))}
            </BreadcrumbList>
          </Breadcrumb>
          <CardAction>
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                load(path)
                reloadRecent()
              }}
              disabled={loading}
            >
              <RefreshCw className={loading ? 'animate-spin' : ''} /> Atualizar
            </Button>
          </CardAction>
        </CardHeader>
        <CardContent>
          <div className="max-h-[560px] overflow-auto rounded-lg border [&_thead]:sticky [&_thead]:top-0 [&_thead]:z-10 [&_thead]:bg-card">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Nome</TableHead>
                <TableHead>Onde está</TableHead>
                <TableHead className="text-right">Tamanho</TableHead>
                <TableHead>Modificado</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {data?.entries.map((e) => {
                const p = path ? `${path}/${e.name}` : e.name
                return (
                  <TableRow key={e.name} className={e.dir ? 'cursor-pointer' : ''} onClick={e.dir ? () => setPath(p) : undefined}>
                    <TableCell>
                      <span className="inline-flex items-center gap-2 font-mono text-[12.5px]">
                        {e.dir ? <Folder className="size-4 text-muted-foreground" /> : <FileVideo className="size-4 text-muted-foreground" />}
                        {e.name}
                      </span>
                    </TableCell>
                    <TableCell>
                      <span className="inline-flex flex-wrap items-center gap-1">
                        {e.locations.map((l) =>
                          l === 'ram' ? (
                            <Badge key={l} className="bg-brand/12 font-mono text-brand">
                              RAM
                            </Badge>
                          ) : (
                            <Badge key={l} variant="outline" className="font-mono">
                              {l}
                            </Badge>
                          ),
                        )}
                        {e.state && <span className="text-xs text-muted-foreground">{FSTATE[e.state] ?? e.state}</span>}
                      </span>
                    </TableCell>
                    <TableCell className="tabular text-right">{e.dir ? '' : bytes(e.size)}</TableCell>
                    <TableCell>{e.mtime_ms ? dateTime(e.mtime_ms) : ''}</TableCell>
                  </TableRow>
                )
              })}
            </TableBody>
          </Table>
          {loading && !data && <Empty icon={RefreshCw}>Carregando…</Empty>}
          {data && !data.entries.length && <Empty icon={Folder}>Pasta vazia</Empty>}
          </div>
          {data && data.entries.length > 0 && (
            <p className="pt-2 text-xs text-muted-foreground">
              {num(data.entries.length)} entradas{data.truncated && ' (lista cortada em 5000)'}
            </p>
          )}
        </CardContent>
      </Card>

      <SectionTitle>Últimos arquivos processados</SectionTitle>
      <Card>
        <CardContent>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Quando</TableHead>
                <TableHead>Arquivo</TableHead>
                <TableHead>Resultado</TableHead>
                <TableHead>Destino</TableHead>
                <TableHead className="text-right">Tamanho</TableHead>
                <TableHead className="text-right">Duração</TableHead>
                <TableHead>Vídeo</TableHead>
                <TableHead className="text-right">Cópia</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {(recent ?? []).slice(0, 100).map((r, i) => {
                const [tone, label] = OUTCOME[r.outcome] ?? ['neutral', r.outcome]
                return (
                  <TableRow key={`${r.t_ms}-${i}`}>
                    <TableCell className="tabular">{hhmmss(r.t_ms)}</TableCell>
                    <TableCell className="max-w-[420px] truncate font-mono text-xs" title={r.path}>
                      {r.path}
                    </TableCell>
                    <TableCell>
                      <span className="inline-flex items-center gap-1.5">
                        <Chip tone={tone}>{label}</Chip>
                        {r.reason && <span className="max-w-[220px] truncate text-xs text-muted-foreground" title={r.reason}>{r.reason}</span>}
                        {r.labels &&
                          Object.entries(r.labels)
                            .slice(0, 3)
                            .map(([k, v]) => (
                              <Badge key={k} variant="outline" className="max-w-[180px] truncate font-mono" title={`${k}=${v}`}>
                                {k}={v}
                              </Badge>
                            ))}
                        {r.parse_error && (
                          <Tooltip>
                            <TooltipTrigger render={<TriangleAlert className="size-4 text-warning-fg" aria-label="Erro de parse" />} />
                            <TooltipContent>{r.parse_error}</TooltipContent>
                          </Tooltip>
                        )}
                      </span>
                    </TableCell>
                    <TableCell>{r.dest ? <Badge variant="outline" className="font-mono">{r.dest}</Badge> : '—'}</TableCell>
                    <TableCell className="tabular text-right">{bytes(r.size)}</TableCell>
                    <TableCell className="tabular text-right">{r.duration_sec != null ? dur(r.duration_sec) : '—'}</TableCell>
                    <TableCell>
                      {r.codec ?? '—'} {r.resolution && <span className="text-muted-foreground">{r.resolution}</span>}
                    </TableCell>
                    <TableCell className="tabular text-right">{r.transfer_ms != null ? `${r.transfer_ms.toFixed(0)} ms` : '—'}</TableCell>
                  </TableRow>
                )
              })}
            </TableBody>
          </Table>
          {recent && !recent.length && <Empty icon={FileVideo}>Nenhum arquivo processado ainda</Empty>}
        </CardContent>
      </Card>
    </div>
  )
}
