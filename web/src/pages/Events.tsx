import { useState } from 'react'
import { Bell } from 'lucide-react'
import { Card, CardContent } from '@/components/ui/card'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { Empty, EventRow, PageHeader } from '@/components/spw'
import type { SpwEvent } from '@/lib/types'

const RANK = { info: 0, warn: 1, error: 2, critical: 3 } as const

export function Events({ events }: { events: SpwEvent[] }) {
  const [min, setMin] = useState('0')
  const shown = events.filter((e) => RANK[e.level] >= +min)
  return (
    <div>
      <PageHeader title="Eventos" sub="Mudanças de estado de destinos e nodes, thief e configuração.">
        <ToggleGroup variant="outline" size="sm" spacing={0} value={[min]} onValueChange={(v: string[]) => v[0] && setMin(v[0])} aria-label="Nível">
          <ToggleGroupItem value="0">Todos</ToggleGroupItem>
          <ToggleGroupItem value="1">Avisos e erros</ToggleGroupItem>
          <ToggleGroupItem value="2">Só erros</ToggleGroupItem>
        </ToggleGroup>
      </PageHeader>
      <Card>
        <CardContent>{shown.length ? shown.map((e) => <EventRow key={e.seq} e={e} />) : <Empty icon={Bell}>Nada por aqui</Empty>}</CardContent>
      </Card>
    </div>
  )
}
