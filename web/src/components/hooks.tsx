import { CircleCheck, CirclePause, Inbox, Send, TriangleAlert, Webhook } from 'lucide-react'
import { Card, CardContent } from '@/components/ui/card'
import { Chip, KV } from '@/components/spw'
import { ago, compact, num } from '@/lib/format'
import type { HookHealth, HooksView } from '@/lib/types'

function HookState({ enabled, h }: { enabled: boolean; h: HookHealth }) {
  if (!enabled)
    return (
      <Chip tone="neutral" icon={CirclePause}>
        Desativado
      </Chip>
    )
  if (h.failing)
    return (
      <Chip tone="critical" icon={TriangleAlert}>
        Com falha
      </Chip>
    )
  if (!h.calls)
    return (
      <Chip tone="info" icon={Send}>
        Aguardando
      </Chip>
    )
  return (
    <Chip tone="good" icon={CircleCheck}>
      Respondendo
    </Chip>
  )
}

const latency = (h: HookHealth) => (h.calls ? `${h.ewma_ms.toFixed(h.ewma_ms < 10 ? 1 : 0).replace('.', ',')} ms` : '—')

function LastError({ h }: { h: HookHealth }) {
  if (!h.last_error) return null
  return (
    <p className="truncate text-xs text-muted-foreground" title={h.last_error[1]}>
      Último erro {ago(h.last_error[0])}: <span className="font-mono">{h.last_error[1]}</span>
    </p>
  )
}

/** Health of the validate and moved hooks. */
export function HooksPanel({ hooks }: { hooks: HooksView }) {
  const v = hooks.validate
  const m = hooks.moved
  return (
    <div className="grid grid-cols-1 gap-4 xl:grid-cols-2">
      <Card className="py-4">
        <CardContent className="flex flex-col gap-3 px-4">
          <div className="flex items-center gap-2.5">
            <span className="grid size-9 shrink-0 place-items-center rounded-lg bg-muted text-brand">
              <Webhook className="size-4.5" />
            </span>
            <div className="min-w-0 flex-1">
              <div className="font-semibold">Hook validate</div>
              <div className="truncate font-mono text-xs text-muted-foreground">{v.enabled ? v.url : 'antes de mover · desativado: tudo é movido'}</div>
            </div>
            <HookState enabled={v.enabled} h={v} />
          </div>
          <div className="grid grid-cols-4 gap-3 text-[13px] max-sm:grid-cols-2">
            <KV k="Movidos" v={compact(v.moved)} />
            <KV k="Recusados" v={compact(v.dropped)} />
            <KV k="Segurados" v={compact(v.held)} />
            <KV k="Sem resposta" v={compact(v.fallback)} />
            <KV k="Latência média" v={latency(v)} />
            <KV k="Lotes" v={num(v.calls)} />
            <KV k="Maior lote" v={num(v.max_batch)} />
            <KV k="Erros" v={num(v.errors)} />
          </div>
          <LastError h={v} />
        </CardContent>
      </Card>
      <Card className="py-4">
        <CardContent className="flex flex-col gap-3 px-4">
          <div className="flex items-center gap-2.5">
            <span className="grid size-9 shrink-0 place-items-center rounded-lg bg-muted text-brand">
              <Inbox className="size-4.5" />
            </span>
            <div className="min-w-0 flex-1">
              <div className="font-semibold">Hook moved</div>
              <div className="truncate font-mono text-xs text-muted-foreground">{m.enabled ? m.url : 'depois de mover · desativado'}</div>
            </div>
            <HookState enabled={m.enabled} h={m} />
          </div>
          <div className="grid grid-cols-4 gap-3 text-[13px] max-sm:grid-cols-2">
            <KV k="Na fila (outbox)" v={<span className={m.pending > 0 && m.failing ? 'text-warning-fg' : ''}>{num(m.pending)}</span>} />
            <KV k="Entregues" v={compact(m.files)} />
            <KV k="Latência média" v={latency(m)} />
            <KV k="Erros" v={num(m.errors)} />
          </div>
          {!m.outbox_ok && <p className="text-xs text-critical-fg">Outbox indisponível em {m.outbox_dir}: os eventos não estão sendo guardados.</p>}
          <LastError h={m} />
        </CardContent>
      </Card>
    </div>
  )
}
