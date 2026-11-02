// Mirrors the master's JSON API (src/web.rs, src/cluster.rs, src/stats.rs).

export type DestStatus = 'healthy' | 'full' | 'suspended' | 'error' | 'offline' | 'pending' | 'draining' | 'disabled'

export interface Sample {
  t_ms: number
  ram_used: number
  ram_files: number
  queued: number
  inflight: number
  ingest_bps: number
  egress_bps: number
  read_bps: number
  thief_bps: number
  dest_bps: Record<string, number>
}

export interface DestView {
  id: string
  node: string
  path: string
  weight: number
  max_use_pct: number
  max_inflight: number
  enabled: boolean
  draining: boolean
  expect_mount: boolean
  status: DestStatus
  status_since_ms: number
  reason: string | null
  free_bytes: number
  total_bytes: number
  use_pct: number
  is_mount: boolean | null
  probe_ms: number | null
  probe_age_s: number | null
  share: number
  inflight: number
  fail_streak: number
  moved_files: number
  moved_bytes: number
  failures: number
  read_bytes: number
  ewma_mbps: number
  last_transfer_ms: number
  last_error: [number, string] | null
}

export interface NodeInfo {
  id: string
  url: string
  hostname: string
  version: string
  local: boolean
  started_at_ms: number
  last_seen_ms: number
  connected_since_ms: number
  online: boolean
}

export interface RamView {
  used_bytes: number
  files: number
  dirs: number
  writing: number
  open_writers: number
  sealing: number
  deciding: number
  deciding_bytes: number
  queued: number
  queued_bytes: number
  inflight: number
  inflight_bytes: number
  oldest_queued_age_s: number
  oldest_open_age_s: number
  thief_max_bytes: number
  thief_resume_bytes: number
  hard_limit_bytes: number
  tmpfs_bytes: number
}

export interface Totals {
  ingest_bytes: number
  files_created: number
  files_sealed: number
  zero_deleted: number
  invalid_dropped: number
  moved_files: number
  moved_bytes: number
  move_failures: number
  thief_files: number
  thief_bytes: number
  read_ram_bytes: number
  read_dest_bytes: number
  write_rejected: number
  hook_dropped: number
  hook_dropped_bytes: number
  outbox_errors: number
}

export interface ThiefState {
  active: boolean
  since_ms: number
  episode_files: number
  episode_bytes: number
  stalled: boolean
  activations: number
}

export interface Mp4Stats {
  ok: number
  failed: number
  fail_reasons: Record<string, number>
  fragmented: number
  progressive: number
  codecs: Record<string, number>
  resolutions: Record<string, number>
  duration_hist: number[]
  size_hist: number[]
  duration_total: number
  bytes_parsed: number
}

export interface HookHealth {
  calls: number
  errors: number
  files: number
  last_ok_ms: number
  last_error: [number, string] | null
  failing: boolean
  ewma_ms: number
  max_batch: number
}

export interface HooksView {
  validate: HookHealth & { enabled: boolean; url: string; moved: number; dropped: number; held: number; fallback: number; stale: number }
  moved: HookHealth & { enabled: boolean; url: string; pending: number; outbox_ok: boolean; outbox_dir: string; outbox_errors: number }
}

export interface Overview {
  version: string
  config_editable: boolean
  hostname: string
  started_ms: number
  now_ms: number
  mount_path: string
  spool_dir: string
  host_mem_bytes: number
  ram: RamView
  thief: ThiefState
  totals: Totals
  rate: Sample | null
  dests: DestView[]
  nodes: NodeInfo[]
  mp4: Mp4Stats
  /** label key → distinct values */
  groups: Record<string, number>
  hooks: HooksView
  last_event_seq: number
}

export interface SpwEvent {
  seq: number
  t_ms: number
  level: 'info' | 'warn' | 'error' | 'critical'
  code: string
  message: string
}

export interface GroupStat {
  key: string
  value: string
  files: number
  bytes: number
  duration_sec: number
  last_ms: number
  codec: string
  width: number
  height: number
  parse_fail: number
  lost_files: number
  lost_bytes: number
  dropped_files: number
  dropped_bytes: number
}

export interface RecentFile {
  t_ms: number
  path: string
  size: number
  outcome: 'moved' | 'stolen' | 'invalid' | 'empty' | 'dropped'
  dest: string | null
  transfer_ms: number | null
  duration_sec: number | null
  codec: string | null
  resolution: string | null
  parse_error: string | null
  reason: string | null
  labels: Record<string, string> | null
}

export interface BrowseEntry {
  name: string
  dir: boolean
  size: number
  mtime_ms: number
  locations: string[]
  state: 'open' | 'writing' | 'sealing' | 'deciding' | 'queued' | 'inflight' | null
}

export interface DestCfg {
  id: string
  node: string
  path: string
  weight: number
  max_use_pct: number
  max_inflight: number
  enabled: boolean
  draining: boolean
  expect_mount: boolean
}

export interface MasterConfig {
  mount: { path: string; allow_other: boolean; threads: number; allowed_extensions: string[]; allow_delete_on_dest: boolean; writeback_cache: boolean }
  ram: { spool_dir: string; manage_tmpfs: boolean; thief_max_gb: number; thief_resume_pct: number; hard_limit_pct: number; settle_secs: number }
  mover: { fail_streak: number; min_rate_mbps: number; fsync: boolean; avoid_secs: number; shutdown_flush_secs: number; pressure_pct: number }
  mp4: { drop_invalid: boolean }
  hooks: {
    validate: { url: string; token: string; batch_max: number; batch_wait_ms: number; inflight: number; timeout_ms: number; on_error: 'move' | 'hold'; hold_retry_ms: number }
    moved: { url: string; token: string; batch_max: number; batch_wait_ms: number; timeout_ms: number; outbox_dir: string }
  }
  web: { listen: string; token: string; edit_config: boolean }
  cluster: { token: string; local_node_id: string; node_timeout_secs: number }
  destination: DestCfg[]
}
