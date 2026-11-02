import { useEffect, useMemo, useState } from 'react'
import { Pencil, Save, TriangleAlert, Undo2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { Slider } from '@/components/ui/slider'
import { Switch } from '@/components/ui/switch'
import { PageHeader, RamMeter } from '@/components/spw'
import { act, api } from '@/lib/api'
import { bytes, GiB, MiB } from '@/lib/format'
import type { MasterConfig, Overview } from '@/lib/types'

type Field = {
  path: string[]
  label: string
  unit?: string
  type: 'number' | 'bool' | 'text' | 'list' | 'secret' | 'select'
  hint: string
  min?: number
  max?: number
  step?: number
  options?: { value: string; label: string }[]
  placeholder?: string
}

const FIELDS: Field[] = [
  { path: ['ram', 'thief_resume_pct'], label: 'Thief para em', unit: '% do teto', type: 'number', hint: 'O thief descarta até a RAM voltar a este nível.', min: 10, max: 99, step: 1 },
  { path: ['ram', 'hard_limit_pct'], label: 'Limite rígido', unit: '% do teto', type: 'number', hint: 'Acima disso novas escritas recebem ENOSPC.', min: 100, max: 200, step: 1 },
  { path: ['ram', 'settle_secs'], label: 'Tempo de assentamento', unit: 's', type: 'number', hint: 'Depois do último close, o arquivo precisa ficar este tempo sem escrita para ser selado e copiado.', min: 0.2, max: 600, step: 0.1 },
  { path: ['mover', 'pressure_pct'], label: 'Modo pressão a partir de', unit: '% do teto', type: 'number', hint: 'Acima disso o balanceador usa toda vaga livre em vez de respeitar estritamente a proporção.', min: 0, max: 100, step: 1 },
  { path: ['mover', 'fail_streak'], label: 'Falhas seguidas para suspender', unit: 'falhas', type: 'number', hint: 'O destino volta no próximo teste bem-sucedido.', min: 1, max: 100, step: 1 },
  { path: ['mover', 'min_rate_mbps'], label: 'Velocidade mínima esperada', unit: 'MiB/s', type: 'number', hint: 'Timeout de cada cópia: 30 s + tamanho ÷ velocidade.', min: 0.1, step: 0.1 },
  { path: ['mover', 'avoid_secs'], label: 'Evitar destino após falha', unit: 's', type: 'number', hint: 'Um arquivo que falhou num destino tenta outro durante este tempo.', min: 0, step: 1 },
  { path: ['mover', 'shutdown_flush_secs'], label: 'Descarga ao desligar', unit: 's', type: 'number', hint: 'Tempo para esvaziar a RAM ao parar o serviço.', min: 0, step: 1 },
  { path: ['mover', 'fsync'], label: 'fsync após cada cópia', type: 'bool', hint: 'Mais seguro contra queda de energia do destino, mais lento.' },
  { path: ['mp4', 'drop_invalid'], label: 'Descartar MP4 inválidos', type: 'bool', hint: 'Apaga em vez de mover arquivos que falham no parse.' },
  { path: ['mount', 'allowed_extensions'], label: 'Extensões permitidas', type: 'list', hint: 'Separadas por vírgula. Vazio aceita qualquer extensão.' },
  { path: ['mount', 'allow_delete_on_dest'], label: 'Permitir apagar pelo mount', type: 'bool', hint: 'Permite apagar pelo mount arquivos que já estão nos destinos.' },
  { path: ['cluster', 'node_timeout_secs'], label: 'Node offline após', unit: 's', type: 'number', hint: 'Sem heartbeat por este tempo.', min: 1, step: 0.5 },
  { path: ['cluster', 'token'], label: 'Token do cluster', type: 'text', hint: 'Usado pelos nodes. Trocar exige reiniciar os nodes com o novo token.' },
  { path: ['hooks', 'validate', 'url'], label: 'URL do hook validate', type: 'text', placeholder: 'http://127.0.0.1:7090/spillway/validate', hint: 'Chamado antes de mover. Vazio desativa: tudo é movido.' },
  { path: ['hooks', 'validate', 'token'], label: 'Token', type: 'secret', hint: 'Enviado como Authorization: Bearer.' },
  {
    path: ['hooks', 'validate', 'on_error'],
    label: 'Se o hook falhar',
    type: 'select',
    hint: 'Vale para erro, timeout ou arquivo sem resposta.',
    options: [
      { value: 'move', label: 'Mover mesmo assim (fail-open)' },
      { value: 'hold', label: 'Segurar na RAM e perguntar de novo' },
    ],
  },
  { path: ['hooks', 'validate', 'batch_max'], label: 'Tamanho máximo do lote', unit: 'arquivos', type: 'number', hint: 'O lote sai ao juntar isto...', min: 1, max: 10000, step: 1 },
  { path: ['hooks', 'validate', 'batch_wait_ms'], label: 'Espera máxima do lote', unit: 'ms', type: 'number', hint: '...ou quando o arquivo mais antigo esperou isto.', min: 0, step: 10 },
  { path: ['hooks', 'validate', 'inflight'], label: 'Lotes simultâneos', type: 'number', hint: 'Um hook lento não segura a fila inteira.', min: 1, max: 64, step: 1 },
  { path: ['hooks', 'validate', 'timeout_ms'], label: 'Timeout', unit: 'ms', type: 'number', hint: 'Por lote.', min: 50, step: 100 },
  { path: ['hooks', 'validate', 'hold_retry_ms'], label: 'Nova tentativa após', unit: 'ms', type: 'number', hint: 'Para "segurar" em caso de erro.', min: 0, step: 100 },
  { path: ['hooks', 'moved', 'url'], label: 'URL do hook moved', type: 'text', placeholder: 'http://127.0.0.1:7090/spillway/moved', hint: 'Recebe os arquivos que chegaram aos destinos. Vazio desativa.' },
  { path: ['hooks', 'moved', 'token'], label: 'Token', type: 'secret', hint: 'Enviado como Authorization: Bearer.' },
  { path: ['hooks', 'moved', 'batch_max'], label: 'Tamanho máximo do lote', unit: 'eventos', type: 'number', hint: 'Entrega pelo menos uma vez: o receptor precisa ser idempotente.', min: 1, max: 10000, step: 1 },
  { path: ['hooks', 'moved', 'batch_wait_ms'], label: 'Espera máxima do lote', unit: 'ms', type: 'number', hint: 'Agrupa eventos antes de enviar.', min: 0, step: 100 },
  { path: ['hooks', 'moved', 'timeout_ms'], label: 'Timeout', unit: 'ms', type: 'number', hint: 'Falhou: nova tentativa com espera crescente até 30 s.', min: 50, step: 100 },
]

type Form = MasterConfig
type Obj = Record<string, unknown>
const getAt = (o: unknown, path: string[]): unknown => path.reduce<unknown>((acc, k) => (acc as Obj | undefined)?.[k], o)
const setAt = (o: Obj, path: string[], v: unknown): Obj => {
  const [k, ...rest] = path
  return { ...o, [k]: rest.length ? setAt((o[k] as Obj) ?? {}, rest, v) : v }
}

/** Nested patch with only the leaves that changed (arrays compared whole). */
function diff(a: unknown, b: unknown): Obj | undefined {
  if (typeof b !== 'object' || b === null || Array.isArray(b)) return JSON.stringify(a) === JSON.stringify(b) ? undefined : (b as Obj)
  const out: Obj = {}
  for (const k of Object.keys(b as Obj)) {
    const d = diff((a as Obj | undefined)?.[k], (b as Obj)[k])
    if (d !== undefined) out[k] = d
  }
  return Object.keys(out).length ? out : undefined
}

function FieldInput({ f, form, set }: { f: Field; form: Form; set: (path: string[], v: unknown) => void }) {
  const id = `s-${f.path.join('-')}`
  const v = getAt(form, f.path)
  if (f.type === 'bool')
    return (
      <div className="grid content-start gap-1.5">
        <Label htmlFor={id} className="flex items-center gap-2.5">
          <Switch id={id} checked={!!v} onCheckedChange={(c) => set(f.path, c)} />
          {f.label}
        </Label>
        <span className="text-xs text-muted-foreground">{f.hint}</span>
      </div>
    )
  if (f.type === 'select')
    return (
      <div className="grid content-start gap-1.5">
        <Label>{f.label}</Label>
        <Select items={f.options} value={String(v ?? '')} onValueChange={(x) => x && set(f.path, x)}>
          <SelectTrigger className="w-full">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {f.options!.map((o) => (
              <SelectItem key={o.value} value={o.value}>
                {o.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <span className="text-xs text-muted-foreground">{f.hint}</span>
      </div>
    )
  const val = f.type === 'list' ? ((v as string[]) ?? []).join(', ') : String(v ?? '')
  return (
    <div className="grid content-start gap-1.5">
      <Label htmlFor={id}>{f.label}</Label>
      <div className="flex">
        <Input
          id={id}
          type={f.type === 'number' ? 'number' : f.type === 'secret' ? 'password' : 'text'}
          autoComplete="off"
          min={f.min}
          max={f.max}
          step={f.step}
          placeholder={f.placeholder}
          className={f.unit ? 'rounded-r-none' : f.type !== 'number' ? 'font-mono' : ''}
          value={val}
          onChange={(e) => {
            const raw = e.target.value
            set(
              f.path,
              f.type === 'number'
                ? raw === ''
                  ? ''
                  : +raw
                : f.type === 'list'
                  ? raw
                      .split(',')
                      .map((s) => s.trim().replace(/^\./, ''))
                      .filter(Boolean)
                  : raw.trim(),
            )
          }}
        />
        {f.unit && <span className="grid place-items-center rounded-r-lg border border-l-0 bg-muted px-2.5 text-xs whitespace-nowrap text-muted-foreground">{f.unit}</span>}
      </div>
      <span className="text-xs text-muted-foreground">{f.hint}</span>
    </div>
  )
}

export function Settings({ ov }: { ov: Overview }) {
  const [saved, setSaved] = useState<Form | null>(null)
  const [form, setForm] = useState<Form | null>(null)
  const [busy, setBusy] = useState(false)

  const load = async () => {
    const s = await api<{ config: MasterConfig }>('/api/settings')
    setSaved(s.config)
    setForm(structuredClone(s.config))
  }
  useEffect(() => {
    load().catch(() => {})
  }, [])

  const patch = useMemo(() => (saved && form ? (diff(saved, form) ?? {}) : {}), [saved, form])
  const dirty = Object.keys(patch).length > 0

  if (!form || !saved)
    return (
      <div className="grid gap-4">
        <Skeleton className="h-10 w-64" />
        <Skeleton className="h-48" />
        <Skeleton className="h-48" />
      </div>
    )

  const set = (path: string[], v: unknown) => setForm((f) => (f ? (setAt(f as unknown as Obj, path, v) as unknown as Form) : f))
  const r = form.ram
  const max = r.thief_max_gb * GiB
  const preview = { used_bytes: ov.ram.used_bytes, thief_max_bytes: max, thief_resume_bytes: (max * r.thief_resume_pct) / 100, hard_limit_bytes: (max * r.hard_limit_pct) / 100 }
  const hostGb = ov.host_mem_bytes / GiB
  const sliderMax = hostGb > 0 ? Math.max(1, Math.floor(hostGb)) : Math.max(512, Math.ceil(saved.ram.thief_max_gb * 2))
  const tooBig = hostGb > 0 && (preview.hard_limit_bytes / GiB) > hostGb * 0.85

  const save = async () => {
    setBusy(true)
    if (await act(() => api('/api/settings', { method: 'PUT', body: patch }), 'Configurações salvas')) await load()
    setBusy(false)
  }

  const group = (title: string, sub: string, prefixes: string[]) => (
    <Card>
      <CardHeader>
        <CardTitle>{title}</CardTitle>
        <CardDescription>{sub}</CardDescription>
      </CardHeader>
      <CardContent className="grid grid-cols-1 gap-x-6 gap-y-5 sm:grid-cols-2 xl:grid-cols-3">
        {FIELDS.filter((f) => prefixes.includes(f.path.slice(0, -1).join('.'))).map((f) => (
          <FieldInput key={f.path.join('.')} f={f} form={form} set={set} />
        ))}
      </CardContent>
    </Card>
  )

  return (
    <div>
      <PageHeader title="Configurações" sub="Aplicadas na hora e gravadas no arquivo TOML do master.">
        {dirty && (
          <span className="inline-flex items-center gap-1.5 text-sm text-warning-fg">
            <Pencil className="size-4" /> Alterações não salvas
          </span>
        )}
        <Button variant="outline" disabled={!dirty} onClick={() => setForm(structuredClone(saved))}>
          <Undo2 /> Descartar
        </Button>
        <Button disabled={!dirty || busy} onClick={save}>
          <Save /> Salvar
        </Button>
      </PageHeader>

      <div className="grid gap-4">
        <Card>
          <CardHeader>
            <CardTitle>Teto de RAM</CardTitle>
            <CardDescription>
              Quantidade máxima em RAM antes do thief começar a descartar os arquivos mais antigos que ainda não foram para os discos.
              {ov.host_mem_bytes > 0 && ` Esta máquina tem ${bytes(ov.host_mem_bytes)} de RAM.`}
            </CardDescription>
          </CardHeader>
          <CardContent>
            <div className="grid grid-cols-[minmax(0,1fr)_180px] items-center gap-5">
              <Slider min={0.5} max={sliderMax} step={0.5} value={r.thief_max_gb} onValueChange={(v) => set(['ram', 'thief_max_gb'], Array.isArray(v) ? v[0] : v)} aria-label="Teto de RAM" />
              <div className="flex">
                <Input type="number" min={0.1} step={0.1} value={r.thief_max_gb} onChange={(e) => set(['ram', 'thief_max_gb'], +e.target.value)} className="rounded-r-none" aria-label="Teto em GiB" />
                <span className="grid place-items-center rounded-r-lg border border-l-0 bg-muted px-2.5 text-xs text-muted-foreground">GiB</span>
              </div>
            </div>
            {tooBig && (
              <p className="mt-3 flex items-center gap-1.5 text-sm text-warning-fg">
                <TriangleAlert className="size-4" />
                O limite rígido ({bytes(preview.hard_limit_bytes)}) passa de 85% da RAM desta máquina ({bytes(ov.host_mem_bytes)}). O sistema pode ficar sem memória.
              </p>
            )}
            <RamMeter ram={preview} className="mt-5 h-3.5" />
            <div className="mt-3 flex flex-wrap gap-x-5 gap-y-1 text-xs text-muted-foreground">
              <span className="inline-flex items-center gap-1.5">
                <i className="size-2.5 rounded-[3px] bg-brand" />
                Em RAM agora {bytes(ov.ram.used_bytes)}
              </span>
              <span className="inline-flex items-center gap-1.5">
                <i className="h-3 w-0.5 bg-muted-foreground" />
                Thief para em {bytes(preview.thief_resume_bytes)}
              </span>
              <span className="inline-flex items-center gap-1.5">
                <i className="h-3 w-0.5 bg-foreground" />
                Thief ativa em {bytes(max)}
              </span>
              <span>Escritas recusadas acima de {bytes(preview.hard_limit_bytes)}</span>
              <span>tmpfs {bytes(preview.hard_limit_bytes + 256 * MiB)}</span>
            </div>
          </CardContent>
        </Card>
        {group('RAM e selagem', 'Quando um arquivo é considerado pronto e quanto cabe na RAM.', ['ram'])}
        {group('Transferência para os destinos', 'Balanceamento, falhas e desligamento.', ['mover'])}
        {group('MP4 e mount', 'Parse e regras do sistema de arquivos.', ['mp4', 'mount'])}
        {group('Cluster', 'Comunicação com os nodes.', ['cluster'])}
        {group('Hook validate', 'Antes de mover, o spillway pergunta a um serviço seu se cada arquivo deve ir para os discos, ser descartado ou esperar.', ['hooks.validate'])}
        {group('Hook moved', 'Depois de mover, o spillway avisa um serviço seu. Os eventos ficam num outbox em disco até serem confirmados.', ['hooks.moved'])}
        <Card>
          <CardHeader>
            <CardTitle>Somente no arquivo</CardTitle>
            <CardDescription>
              Exigem reiniciar o master: ponto de montagem <span className="font-mono">{form.mount.path}</span>, spool <span className="font-mono">{form.ram.spool_dir}</span>, {form.mount.threads}{' '}
              threads FUSE, write-back cache {form.mount.writeback_cache ? 'ligado' : 'desligado'}, painel em <span className="font-mono">{form.web.listen}</span>, token do painel e
              outbox do hook moved em <span className="font-mono">{form.hooks.moved.outbox_dir}</span>.
            </CardDescription>
          </CardHeader>
        </Card>
      </div>
    </div>
  )
}
