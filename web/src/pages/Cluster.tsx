import { Check, Copy, Server, Trash2, WifiOff } from 'lucide-react'
import { useState } from 'react'
import { toast } from 'sonner'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Chip, PageHeader } from '@/components/spw'
import { act, api, usePoll } from '@/lib/api'
import { ago, dur } from '@/lib/format'
import type { MasterConfig, Overview } from '@/lib/types'

export function Cluster({ ov }: { ov: Overview }) {
  const [settings] = usePoll<{ config: MasterConfig }>('/api/settings', 30_000)
  const [copied, setCopied] = useState(false)
  const token = settings?.config.cluster.token ?? '<token-do-cluster>'
  const cmd = ['spillway node \\', '  --id node-b \\', `  --master ${location.origin} \\`, `  --token ${token} \\`, '  --listen 0.0.0.0:7071 \\', '  --advertise http://IP-DESTE-SERVIDOR:7071'].join('\n')
  const copy = () =>
    navigator.clipboard.writeText(cmd).then(
      () => {
        setCopied(true)
        toast.success('Comando copiado')
        setTimeout(() => setCopied(false), 1500)
      },
      () => toast.error('Não foi possível copiar'),
    )
  return (
    <div>
      <PageHeader title="Cluster" sub="O master guarda a RAM e o mount. Cada node serve os destinos do próprio servidor; o node local roda dentro do master." />
      <Card>
        <CardContent>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Node</TableHead>
                <TableHead>Estado</TableHead>
                <TableHead>Host</TableHead>
                <TableHead>Endereço</TableHead>
                <TableHead className="text-right">Destinos</TableHead>
                <TableHead>Versão</TableHead>
                <TableHead>Conectado há</TableHead>
                <TableHead>Último sinal</TableHead>
                <TableHead />
              </TableRow>
            </TableHeader>
            <TableBody>
              {ov.nodes.map((n) => (
                <TableRow key={n.id}>
                  <TableCell>
                    <span className="inline-flex items-center gap-2 font-semibold">
                      <Server className="size-4 text-muted-foreground" />
                      {n.id}
                      {n.local && <Badge variant="outline">local</Badge>}
                    </span>
                  </TableCell>
                  <TableCell>
                    {n.online ? (
                      <Chip tone="good" icon={Check}>
                        Online
                      </Chip>
                    ) : (
                      <Chip tone="critical" icon={WifiOff}>
                        Offline
                      </Chip>
                    )}
                  </TableCell>
                  <TableCell>{n.hostname}</TableCell>
                  <TableCell className="font-mono text-xs">{n.local ? '—' : n.url}</TableCell>
                  <TableCell className="tabular text-right">{ov.dests.filter((d) => d.node === n.id).length}</TableCell>
                  <TableCell className="font-mono text-xs">{n.version}</TableCell>
                  <TableCell>{n.online ? dur((ov.now_ms - n.connected_since_ms) / 1000) : '—'}</TableCell>
                  <TableCell>{n.local ? 'no processo' : ago(n.last_seen_ms)}</TableCell>
                  <TableCell className="text-right">
                    {!n.local && !n.online && (
                      <Button variant="ghost" size="sm" onClick={() => act(() => api(`/api/nodes/${encodeURIComponent(n.id)}`, { method: 'DELETE' }), 'Node removido da lista')}>
                        <Trash2 /> Esquecer
                      </Button>
                    )}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </CardContent>
      </Card>

      <Card className="mt-6">
        <CardHeader>
          <CardTitle>Conectar um novo node</CardTitle>
          <CardDescription>
            Rode no outro servidor (o binário é o mesmo). Ele aparece na tabela acima; depois adicione destinos para ele na aba <a className="text-brand hover:underline" href="#dests">Destinos</a>.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div className="relative rounded-lg border bg-muted/60 p-3 pr-12 font-mono text-[12.5px] break-all whitespace-pre-wrap">
            {cmd}
            <Button variant="outline" size="icon-sm" className="absolute top-2 right-2" onClick={copy} aria-label="Copiar comando">
              {copied ? <Check /> : <Copy />}
            </Button>
          </div>
          <p className="mt-3 text-xs text-muted-foreground">
            O master precisa alcançar o endereço anunciado (<span className="font-mono">--advertise</span>) e o node precisa alcançar o master. Toda requisição leva o token do cluster.
          </p>
        </CardContent>
      </Card>
    </div>
  )
}
