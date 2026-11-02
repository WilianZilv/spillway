import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import type { Range } from '@/lib/api'

export function RangePicker({ value, onChange }: { value: Range; onChange: (r: Range) => void }) {
  return (
    <ToggleGroup variant="outline" size="sm" spacing={0} value={[value]} onValueChange={(v: string[]) => v[0] && onChange(v[0] as Range)} aria-label="Período">
      <ToggleGroupItem value="15m">15 min</ToggleGroupItem>
      <ToggleGroupItem value="24h">24 h</ToggleGroupItem>
    </ToggleGroup>
  )
}
