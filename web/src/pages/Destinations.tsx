import { useState } from 'react'
import { ArrowDownToLine, Ban, ChevronDown, ChevronUp, CircleCheck, FileLock2, HardDrive, Pencil, Plus, RefreshCw, Trash2 } from 'lucide-react'
import { toast } from 'sonner'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from '@/components/ui/card'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { DestRateChart } from '@/pages/Overview'
import { DEST_STATUS, DestSummary, Empty, KV, PageHeader } from '@/components/spw'
import { RangePicker } from '@/components/range'
import { act, api, type Range } from '@/lib/api'
import { ago, bytes, num, pct } from '@/lib/format'
import type { DestView, Overview, Sample } from '@/lib/types'

type FormState = { id: string; node: string; path: string; weight: string; max_use_pct: string; max_inflight: string; expect_mount: boolean }

function DestDialog({ ov, edit, open, onOpenChange }: { ov: Overview; edit: DestView | null; open: boolean; onOpenChange: (o: boolean) => void }) {
  const localId = ov.nodes.find((n) => n.local)?.id ?? 'local'
  const init = (): FormState =>
    edit
      ? { id: edit.id, node: edit.node, path: edit.path, weight: String(edit.weight), max_use_pct: String(edit.max_use_pct), max_inflight: String(edit.max_inflight), expect_mount: edit.expect_mount }
      : { id: '', node: localId, path: '', weight: '1', max_use_pct: '95', max_inflight: '4', expect_mount: false }
  const [f, setF] = useState<FormState>(init)
  const [busy, setBusy] = useState(false)
  const [lastKey, setLastKey] = useState<string | null>(null)
  const key = `${open}:${edit?.id ?? ''}`
  if (key !== lastKey) {
    setLastKey(key)
    setF(init())
  }
  const set = (k: keyof FormState) => (e: React.ChangeEvent<HTMLInputElement>) => setF({ ...f, [k]: e.target.value })
  const nodes = ov.nodes.map((n) => ({ value: n.id, label: `${n.id}${n.local ? ' (este master)' : ''}${n.online ? '' : ' (offline)'}` }))

  const submit = async (e: React.FormEvent) => {
    e.preventDefault()
    setBusy(true)
    const body = { node: f.node, path: f.path.trim(), weight: +f.weight, max_use_pct: +f.max_use_pct, max_inflight: +f.max_inflight, expect_mount: f.expect_mount }
    const ok = edit
      ? await act(() => api(`/api/destinations/${encodeURIComponent(edit.id)}`, { method: 'PATCH', body }), 'Destino atualizado')
      : await act(() => api('/api/destinations', { method: 'POST', body: { id: f.id.trim(), ...body, enabled: true, draining: false } }), 'Destino adicionado')
    setBusy(false)
    if (ok) onOpenChange(false)
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <form onSubmit={submit} className="grid gap-5">
          <DialogHeader>
            <DialogTitle>{edit ? `Editar ${edit.id}` : 'Adicionar destino'}</DialogTitle>
            <DialogDescription>O caminho é no servidor do node escolhido. O node testa o destino (escrita, fsync e leitura) antes de receber arquivos.</DialogDescription>
          </DialogHeader>
          <div className="grid grid-cols-2 gap-4">
            <div className="grid gap-1.5">
              <Label htmlFor="f-id">Identificador</Label>
              <Input id="f-id" className="font-mono" value={f.id} onChange={set('id')} readOnly={!!edit} required pattern="[A-Za-z0-9._\-]{1,64}" placeholder="disk2" />
            </div>
            <div className="grid gap-1.5">
              <Label>Node</Label>
              <Select items={nodes} value={f.node} onValueChange={(v) => v && setF({ ...f, node: v as string })}>
                <SelectTrigger className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {nodes.map((n) => (
                    <SelectItem key={n.value} value={n.value}>
                      {n.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="col-span-2 grid gap-1.5">
              <Label htmlFor="f-path">Caminho</Label>
              <Input id="f-path" className="font-mono" value={f.path} onChange={set('path')} required placeholder="/mnt/disk2" />
            </div>
            <div className="grid gap-1.5">
              <Label htmlFor="f-w">Peso (× espaço livre)</Label>
              <Input id="f-w" type="number" step="0.1" min="0.1" max="1000" value={f.weight} onChange={set('weight')} />
            </div>
            <div className="grid gap-1.5">
              <Label htmlFor="f-mu">Uso máximo (%)</Label>
              <Input id="f-mu" type="number" step="1" min="1" max="100" value={f.max_use_pct} onChange={set('max_use_pct')} />
            </div>
            <div className="grid gap-1.5">
              <Label htmlFor="f-mi">Cópias simultâneas</Label>
              <Input id="f-mi" type="number" step="1" min="1" max="64" value={f.max_inflight} onChange={set('max_inflight')} />
            </div>
            <div className="grid content-end gap-1">
              <Label className="flex items-center gap-2.5">
                <Switch checked={f.expect_mount} onCheckedChange={(v) => setF({ ...f, expect_mount: v })} />
                Exigir ponto de montagem
              </Label>
              <span className="text-xs text-muted-foreground">Não grava no disco raiz se o NFS cair</span>
            </div>
          </div>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancelar
            </Button>
            <Button type="submit" disabled={busy}>
              {edit ? 'Salvar' : 'Adicionar'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

function DestCard({ d, ov, i, n, onEdit, onRemove }: { d: DestView; ov: Overview; i: number; n: number; onEdit: () => void; onRemove: () => void }) {
  const enc = encodeURIComponent(d.id)
  const patch = (body: object, msg: string) => act(() => api(`/api/destinations/${enc}`, { method: 'PATCH', body }), msg)
  const probe = async () => {
    const t = toast.loading(`Testando ${d.id}…`)
    try {
      const v = await api<DestView>(`/api/destinations/${enc}/probe`, { method: 'POST' })
      const label = DEST_STATUS[v.status]?.[2] ?? v.status
      const good = ['healthy', 'full', 'draining'].includes(v.status)
      toast[good ? 'success' : 'error'](`${d.id}: ${label}${v.reason ? ` — ${v.reason}` : ''}`, { id: t })
    } catch (e) {
      toast.error((e as Error).message, { id: t })
    }
  }
  const move = (to: number) => act(() => api(`/api/destinations/${enc}/move?to=${to}`, { method: 'POST' }))
  const local = ov.nodes.find((x) => x.local)?.id === d.node
  return (
    <Card className="gap-3 py-4">
      <CardContent className="flex flex-col gap-3 px-4">
        <DestSummary d={d} local={local} nowBps={ov.rate?.dest_bps?.[d.id] ?? 0} />
        <div className="grid grid-cols-4 gap-3 text-[13px] max-sm:grid-cols-2">
          <KV k="Livre" v={bytes(d.free_bytes)} />
          <KV k="Média por cópia" v={d.ewma_mbps ? `${d.ewma_mbps.toFixed(1).replace('.', ',')} MiB/s` : '—'} />
          <KV
            k="Falhas"
            v={
              <>
                {num(d.failures)}
                {d.fail_streak > 0 && <span className="font-normal text-muted-foreground"> ({d.fail_streak} seguidas)</span>}
              </>
            }
          />
          <KV k="Teste (doctor)" v={d.probe_ms == null ? '—' : d.probe_ms < 1 ? '< 1 ms' : `${d.probe_ms.toFixed(0)} ms`} />
          <KV k="Peso" v={`×${d.weight}`} />
          <KV k="Uso máximo" v={pct(d.max_use_pct, 0)} />
          <KV
            k="Ponto de montagem"
            v={
              <>
                {d.is_mount == null ? '—' : d.is_mount ? 'sim' : 'não'}
                {d.expect_mount && <span className="font-normal text-muted-foreground"> (exigido)</span>}
              </>
            }
          />
          <KV k="Lido pelo mount" v={bytes(d.read_bytes)} />
        </div>
        {d.last_error && (
          <p className="text-xs text-muted-foreground">
            Último erro {ago(d.last_error[0])}: <span className="font-mono">{d.last_error[1]}</span>
          </p>
        )}
      </CardContent>
      <CardFooter className="flex-wrap gap-1.5 border-t px-4 pt-3">
        <Button variant="outline" size="sm" onClick={probe}>
          <RefreshCw /> Testar agora
        </Button>
        {ov.config_editable && (
          <>
            <Button variant="outline" size="sm" onClick={onEdit}>
              <Pencil /> Editar
            </Button>
            <Button variant="outline" size="sm" onClick={() => patch({ enabled: !d.enabled }, d.enabled ? 'Destino desativado' : 'Destino ativado')}>
              {d.enabled ? <Ban /> : <CircleCheck />} {d.enabled ? 'Desativar' : 'Ativar'}
            </Button>
            <Button variant="outline" size="sm" onClick={() => patch({ draining: !d.draining }, d.draining ? 'Destino voltou a gravar' : 'Destino em só leitura')}>
              <ArrowDownToLine /> {d.draining ? 'Voltar a gravar' : 'Só leitura'}
            </Button>
          </>
        )}
        <span className="flex-1" />
        {ov.config_editable && (
          <>
        <Tooltip>
          <TooltipTrigger render={<Button variant="ghost" size="icon-sm" disabled={i === 0} onClick={() => move(i - 1)} aria-label="Subir na ordem" />}>
            <ChevronUp />
          </TooltipTrigger>
          <TooltipContent>Subir na ordem de busca</TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger render={<Button variant="ghost" size="icon-sm" disabled={i === n - 1} onClick={() => move(i + 1)} aria-label="Descer na ordem" />}>
            <ChevronDown />
          </TooltipTrigger>
          <TooltipContent>Descer na ordem de busca</TooltipContent>
        </Tooltip>
        <Button variant="destructive" size="sm" onClick={onRemove}>
          <Trash2 /> Remover
        </Button>
          </>
        )}
      </CardFooter>
    </Card>
  )
}

/** Shown instead of the edit controls when the deploy manages the config file. */
export function ManagedBadge() {
  return (
    <span className="inline-flex items-center gap-1.5 rounded-lg border bg-muted px-2.5 py-1.5 text-sm text-muted-foreground" title="web.edit_config = false: altere o arquivo TOML e reinicie o master">
      <FileLock2 className="size-4" /> Configuração gerenciada pelo arquivo
    </span>
  )
}

export function Destinations({ ov, series, range, setRange }: { ov: Overview; series: Sample[]; range: Range; setRange: (r: Range) => void }) {
  const [dialog, setDialog] = useState<{ open: boolean; edit: DestView | null }>({ open: false, edit: null })
  const [removing, setRemoving] = useState<DestView | null>(null)
  return (
    <div>
      <PageHeader title="Destinos" sub="Arquivos selados vão para os destinos saudáveis, na proporção do espaço livre. A ordem define a prioridade de busca nas leituras.">
        {ov.config_editable ? (
          <Button onClick={() => setDialog({ open: true, edit: null })}>
            <Plus /> Adicionar destino
          </Button>
        ) : (
          <ManagedBadge />
        )}
      </PageHeader>
      {ov.dests.length ? (
        <div className="grid grid-cols-1 gap-4 xl:grid-cols-2">
          {ov.dests.map((d, i) => (
            <DestCard key={d.id} d={d} ov={ov} i={i} n={ov.dests.length} onEdit={() => setDialog({ open: true, edit: d })} onRemove={() => setRemoving(d)} />
          ))}
        </div>
      ) : (
        <Card>
          <Empty icon={HardDrive}>
            Nenhum destino configurado
            {ov.config_editable && (
              <Button size="sm" onClick={() => setDialog({ open: true, edit: null })}>
                <Plus /> Adicionar destino
              </Button>
            )}
          </Empty>
        </Card>
      )}
      <Card className="mt-4">
        <CardHeader>
          <CardTitle>Saída por destino</CardTitle>
          <CardDescription>Bytes por segundo</CardDescription>
          <div className="col-start-2 row-span-2 row-start-1 self-start justify-self-end">
            <RangePicker value={range} onChange={setRange} />
          </div>
        </CardHeader>
        <CardContent>
          <DestRateChart ov={ov} series={series} />
        </CardContent>
      </Card>

      <DestDialog ov={ov} edit={dialog.edit} open={dialog.open} onOpenChange={(o) => setDialog((s) => ({ ...s, open: o }))} />

      <AlertDialog open={!!removing} onOpenChange={(o) => !o && setRemoving(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Remover {removing?.id}?</AlertDialogTitle>
            <AlertDialogDescription>
              Os arquivos em{' '}
              <span className="font-mono">
                {removing?.node}:{removing?.path}
              </span>{' '}
              continuam no disco, mas deixam de aparecer no mount. Para manter os arquivos visíveis e só parar de gravar, use <b>Só leitura</b>.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancelar</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={async () => {
                if (removing && (await act(() => api(`/api/destinations/${encodeURIComponent(removing.id)}`, { method: 'DELETE' }), 'Destino removido'))) setRemoving(null)
              }}
            >
              <Trash2 /> Remover
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}
